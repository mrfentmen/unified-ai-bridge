//! Bounded CSV/TSV records and structured queries. Cells remain data: no SQL,
//! expressions, formulas, external references, or automatic numeric coercion.
use super::{binary, parse_args, ToolContext, ToolOutcome};
use crate::{failure::ToolFailure, fsops};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROWS: usize = 10000;
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Delimiter {
    Comma,
    Tab,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    path: String,
    delimiter: Option<Delimiter>,
    columns: Option<Vec<String>>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    expected_sha256: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Filter {
    Equals {
        column: String,
        value: String,
    },
    Contains {
        column: String,
        value: String,
    },
    IsEmpty {
        column: String,
    },
    NumberRange {
        column: String,
        min: Option<f64>,
        max: Option<f64>,
    },
}
impl Filter {
    fn column(&self) -> &str {
        match self {
            Self::Equals { column, .. }
            | Self::Contains { column, .. }
            | Self::IsEmpty { column }
            | Self::NumberRange { column, .. } => column,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    path: String,
    delimiter: Option<Delimiter>,
    columns: Option<Vec<String>>,
    #[serde(default)]
    filters: Vec<Filter>,
    #[serde(default)]
    aggregate_columns: Vec<String>,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
    expected_sha256: Option<String>,
}
struct Row {
    line: u64,
    values: Vec<String>,
}
struct Table {
    headers: Vec<String>,
    rows: Vec<Row>,
    sha256: String,
    delimiter: &'static str,
}
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn load(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    delimiter: Option<Delimiter>,
    expected: Option<&str>,
) -> Result<Table, ToolFailure> {
    let delimiter=match delimiter {Some(v)=>v,None=>match std::path::Path::new(path).extension().and_then(|x|x.to_str()).map(str::to_ascii_lowercase).as_deref(){Some("csv")=>Delimiter::Comma,Some("tsv")=>Delimiter::Tab,_=>return Err(unsupported("choose a CSV/TSV file or explicitly select comma/tab delimiter; XLSX is not supported by this tool"))}};
    let bytes = binary::read_source(ctx, tool, path, MAX_BYTES)?;
    if bytes.contains(&0) {
        return Err(unsupported("table input contains binary NUL bytes"));
    }
    let sha256 = fsops::sha256_bytes(&bytes);
    if expected.is_some_and(|h| h != sha256) {
        return Err(ToolFailure::Conflict(
            "table changed; start a new read/query".into(),
        ));
    }
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(match delimiter {
            Delimiter::Comma => b',',
            Delimiter::Tab => b'\t',
        })
        .has_headers(true)
        .flexible(false)
        .from_reader(bytes.as_slice());
    let headers = reader
        .headers()
        .map_err(|e| invalid(format!("invalid UTF-8 table header: {e}")))?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if headers.is_empty()
        || headers.len() > 128
        || headers.iter().any(|h| h.is_empty() || h.len() > 256)
    {
        return Err(invalid(
            "table requires 1–128 unique, nonempty headers of at most 256 bytes",
        ));
    }
    let unique: std::collections::HashSet<_> = headers.iter().collect();
    if unique.len() != headers.len() {
        return Err(invalid("duplicate table headers are ambiguous"));
    }
    if serde_json::to_vec(&headers).map_err(internal)?.len() > 8000 {
        return Err(unsupported("table header metadata exceeds 8,000 bytes"));
    }
    let mut rows = vec![];
    let mut cells = 0;
    let end = Instant::now() + Duration::from_secs(2);
    for record in reader.records() {
        if Instant::now() >= end {
            return Err(unsupported(
                "table parsing exceeded its checked two-second budget; use a smaller table",
            ));
        }
        let row = record.map_err(|e| invalid(format!("invalid table record: {e}")))?;
        cells += row.len();
        if rows.len() >= MAX_ROWS || cells > 250000 {
            return Err(unsupported(
                "table exceeds 10,000 records or 250,000 cells; no partial aggregate is reported",
            ));
        }
        if row.iter().any(|s| s.len() > 8192) {
            return Err(unsupported("table cell exceeds 8,192 bytes"));
        }
        rows.push(Row {
            line: row.position().map(|p| p.line()).unwrap_or(0),
            values: row.iter().map(str::to_owned).collect(),
        });
    }
    Ok(Table {
        headers,
        rows,
        sha256,
        delimiter: match delimiter {
            Delimiter::Comma => "comma",
            Delimiter::Tab => "tab",
        },
    })
}
fn columns(table: &Table, names: Option<Vec<String>>) -> Result<Vec<usize>, ToolFailure> {
    let names = names.unwrap_or_else(|| table.headers.iter().take(20).cloned().collect());
    if names.is_empty() || names.len() > 20 {
        return Err(invalid("select 1–20 columns"));
    }
    let mut indices = vec![];
    for name in names {
        let i = table
            .headers
            .iter()
            .position(|h| h == &name)
            .ok_or_else(|| invalid(format!("unknown column: {name}")))?;
        if indices.contains(&i) {
            return Err(invalid("duplicate selected column"));
        }
        indices.push(i);
    }
    Ok(indices)
}
fn pagination(
    offset: usize,
    limit: Option<usize>,
    hash: Option<&str>,
) -> Result<usize, ToolFailure> {
    let limit = limit.unwrap_or(30);
    if !(1..=100).contains(&limit) || offset > MAX_ROWS {
        return Err(invalid("limit must be 1–100; offset at most 10,000"));
    }
    if offset > 0 && hash.is_none() {
        return Err(invalid(
            "continuation requires expected_sha256 from the first page",
        ));
    }
    Ok(limit)
}
fn page(
    table: &Table,
    selected: &[usize],
    matching: &[usize],
    offset: usize,
    limit: usize,
) -> Result<Value, ToolFailure> {
    let mut rows = vec![];
    let mut bytes = 0;
    let mut next = None;
    for (index, &original) in matching.iter().enumerate().skip(offset) {
        let source = &table.rows[original];
        let values: Vec<_> = selected.iter().map(|i| &source.values[*i]).collect();
        let row = json!({"row":original+1,"line":source.line,"values":values});
        let size = row.to_string().len();
        if bytes + size > 24000 || rows.len() >= limit {
            if rows.is_empty() {
                return Err(unsupported(
                    "one projected record exceeds 24,000 bytes; select fewer columns",
                ));
            }
            next = Some(index);
            break;
        }
        bytes += size;
        rows.push(row);
    }
    Ok(
        json!({"sha256":table.sha256,"delimiter":table.delimiter,"headers":table.headers,"columns":selected.iter().map(|i|&table.headers[*i]).collect::<Vec<_>>(),"columns_omitted":table.headers.len()-selected.len(),"rows":rows,"total_rows":table.rows.len(),"matched_rows":matching.len(),"next_offset":next,"truncated":next.is_some(),"row_numbering":"row is the 1-based data record; line is its starting physical file line"}),
    )
}
fn outcome(tool: &str, path: &str, data: Value) -> Result<ToolOutcome, ToolFailure> {
    let output = serde_json::to_string(&data).map_err(internal)?;
    if output.len() > 56000 {
        return Err(unsupported(
            "table result exceeds 56,000 bytes; select fewer columns",
        ));
    }
    Ok(ToolOutcome::read(tool, output, Some(path.into())).with_data(data))
}
pub async fn read(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Read = parse_args(args)?;
    let limit = pagination(args.offset, args.limit, args.expected_sha256.as_deref())?;
    let table = load(
        ctx,
        "read_table",
        &args.path,
        args.delimiter,
        args.expected_sha256.as_deref(),
    )?;
    let selected = columns(&table, args.columns)?;
    let matching = (0..table.rows.len()).collect::<Vec<_>>();
    outcome(
        "CSV/TSV table page",
        &args.path,
        page(&table, &selected, &matching, args.offset, limit)?,
    )
}
fn numeric(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}
pub async fn query(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Query = parse_args(args)?;
    let limit = pagination(args.offset, args.limit, args.expected_sha256.as_deref())?;
    if args.filters.len() > 8 || args.aggregate_columns.len() > 8 {
        return Err(invalid("at most eight filters and eight aggregate columns"));
    }
    let table = load(
        ctx,
        "query_table",
        &args.path,
        args.delimiter,
        args.expected_sha256.as_deref(),
    )?;
    let selected = columns(&table, args.columns)?;
    let mut filters = vec![];
    for filter in &args.filters {
        let index = table
            .headers
            .iter()
            .position(|h| h == filter.column())
            .ok_or_else(|| invalid("unknown filter column"))?;
        match filter {
            Filter::Equals { value, .. } | Filter::Contains { value, .. } if value.len() > 1024 => {
                return Err(invalid("filter text exceeds 1,024 bytes"))
            }
            Filter::NumberRange { min, max, .. }
                if (min.is_none() && max.is_none())
                    || min.is_some_and(|n| !n.is_finite())
                    || max.is_some_and(|n| !n.is_finite())
                    || matches!((min,max),(Some(a),Some(b)) if a>b) =>
            {
                return Err(invalid(
                    "numeric range requires finite ordered min and/or max",
                ))
            }
            _ => {}
        }
        filters.push((index, filter));
    }
    let matching: Vec<_> = table
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            filters.iter().all(|(i, f)| {
                let value = &row.values[*i];
                match f {
                    Filter::Equals {
                        value: expected, ..
                    } => value == expected,
                    Filter::Contains { value: needle, .. } => value.contains(needle),
                    Filter::IsEmpty { .. } => value.is_empty(),
                    Filter::NumberRange { min, max, .. } => numeric(value).is_some_and(|n| {
                        min.is_none_or(|min| n >= min) && max.is_none_or(|max| n <= max)
                    }),
                }
            })
        })
        .map(|(i, _)| i)
        .collect();
    let mut data = page(&table, &selected, &matching, args.offset, limit)?;
    let mut aggregates = vec![];
    let mut seen = std::collections::HashSet::new();
    for column in args.aggregate_columns {
        if !seen.insert(column.clone()) {
            return Err(invalid("duplicate aggregate column"));
        }
        let index = table
            .headers
            .iter()
            .position(|h| h == &column)
            .ok_or_else(|| invalid("unknown aggregate column"))?;
        let mut count = 0;
        let mut empty = 0;
        let mut non_numeric = 0;
        let mut sum = 0.0;
        let mut min: Option<f64> = None;
        let mut max: Option<f64> = None;
        for &i in &matching {
            let value = &table.rows[i].values[index];
            if value.is_empty() {
                empty += 1;
                continue;
            }
            if let Some(n) = numeric(value) {
                count += 1;
                sum += n;
                min = Some(min.map_or(n, |v| v.min(n)));
                max = Some(max.map_or(n, |v| v.max(n)));
            } else {
                non_numeric += 1;
            }
        }
        if !sum.is_finite() {
            return Err(unsupported(
                "numeric aggregate overflowed; no partial aggregate is reported",
            ));
        }
        aggregates.push(json!({"column":column,"numeric_count":count,"empty_count":empty,"non_numeric_count":non_numeric,"sum":if count>0{Some(sum)}else{None},"mean":if count>0{Some(sum/count as f64)}else{None},"min":min,"max":max}));
    }
    data["filters"] = json!(args.filters);
    data["aggregates"] = json!(aggregates);
    data["semantics"]=json!("Filters are ANDed; text is case-sensitive and empty means exactly empty. Numeric comparisons/aggregates parse finite f64 values, skip nonnumeric cells, and may round. Aggregates cover every matched record, not only this page. Cells/formulas are never evaluated.");
    outcome("CSV/TSV query", &args.path, data)
}
