use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::failure::ToolFailure;
use crate::fsops;
use crate::tools::{parse_args, require_text_extension, ToolContext, ToolOutcome};

const MAX_DATASET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ROWS: usize = 200_000;
const MAX_COLUMNS: usize = 200;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetArgs {
    path: String,
    #[serde(default)]
    sample_rows: Option<usize>,
    #[serde(default)]
    delimiter: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatsArgs {
    path: String,
    #[serde(default)]
    columns: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VisualizationArgs {
    path: String,
    x_column: String,
    y_column: String,
    chart_type: String,
    output_path: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportSection {
    heading: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestArgs {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntegrityArgs {
    manifest_path: String,
}

#[derive(Debug, Clone)]
struct Dataset {
    columns: Vec<String>,
    rows: Vec<HashMap<String, String>>,
    delimiter: char,
    format: &'static str,
    total_bytes: u64,
}

impl Dataset {
    fn value(&self, row: &HashMap<String, String>, column: &str) -> String {
        row.get(column).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
struct ColumnStats {
    column: String,
    non_missing: usize,
    missing: usize,
    numeric: usize,
    mean: Option<f64>,
    median: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    stddev: Option<f64>,
}

pub async fn inspect_dataset(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: DatasetArgs = parse_args(args)?;
    let sample_rows = args.sample_rows.unwrap_or(5).min(50);
    let delimiter = parse_delimiter(args.delimiter.as_deref(), &args.path)?;
    let dataset = load_dataset(ctx, "inspect_dataset", &args.path, delimiter)?;
    let mut types = Vec::new();
    for column in &dataset.columns {
        let mut numeric = 0usize;
        let mut missing = 0usize;
        let mut non_empty = 0usize;
        for row in dataset.rows.iter().take(5_000) {
            let value = dataset.value(row, column);
            if is_missing(&value) {
                missing += 1;
            } else {
                non_empty += 1;
                if value.trim().parse::<f64>().is_ok() {
                    numeric += 1;
                }
            }
        }
        let inferred = if non_empty > 0 && numeric == non_empty {
            "numeric"
        } else if missing == dataset.rows.len().min(5_000) {
            "empty"
        } else {
            "text"
        };
        types.push(format!("{column} ({inferred})"));
    }
    let mut output = format!(
        "Dataset: {}\nFormat: {} (delimiter {:?})\nBytes: {}\nRows: {}\nColumns: {} ({})\n\n",
        args.path,
        dataset.format,
        dataset.delimiter,
        dataset.total_bytes,
        dataset.rows.len(),
        dataset.columns.len(),
        types.join(", ")
    );
    output.push_str("Sample:\n");
    output.push_str(&markdown_table(
        &dataset.columns,
        &dataset.rows,
        sample_rows,
    ));
    output.push_str("\n\nRead-only inspection. No files were modified.");
    let data = serde_json::json!({
        "path": args.path,
        "format": dataset.format,
        "rows": dataset.rows.len(),
        "columns": dataset.columns,
        "dtypes": types,
        "bytes": dataset.total_bytes,
    });
    Ok(ToolOutcome::read(
        format!(
            "inspected {} rows × {} columns",
            dataset.rows.len(),
            dataset.columns.len()
        ),
        output,
        Some(args.path),
    )
    .with_data(data))
}

pub async fn calculate_statistics(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: StatsArgs = parse_args(args)?;
    let delimiter = parse_delimiter(None, &args.path)?;
    let dataset = load_dataset(ctx, "calculate_statistics", &args.path, delimiter)?;
    if dataset.rows.is_empty() {
        return Err(ToolFailure::Unsupported("dataset has no rows".into()));
    }
    let requested: Vec<String> = args.columns.unwrap_or_else(|| dataset.columns.clone());
    if requested.len() > 50 {
        return Err(ToolFailure::InvalidArguments(
            "at most 50 columns can be requested".into(),
        ));
    }
    for column in &requested {
        if !dataset.columns.contains(column) {
            return Err(ToolFailure::InvalidArguments(format!(
                "unknown column `{column}`; available: {}",
                dataset.columns.join(", ")
            )));
        }
    }
    let mut stats = Vec::new();
    for column in requested {
        let values: Vec<f64> = dataset
            .rows
            .iter()
            .filter_map(|row| {
                let value = dataset.value(row, &column);
                if is_missing(&value) {
                    None
                } else {
                    value.trim().parse::<f64>().ok()
                }
            })
            .collect();
        let missing = dataset
            .rows
            .iter()
            .filter(|row| is_missing(&dataset.value(row, &column)))
            .count();
        let non_missing = dataset.rows.len() - missing;
        if values.is_empty() {
            stats.push(ColumnStats {
                column,
                non_missing,
                missing,
                numeric: 0,
                mean: None,
                median: None,
                min: None,
                max: None,
                stddev: None,
            });
            continue;
        }
        let sum: f64 = values.iter().sum();
        let mean = sum / values.len() as f64;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
        let mut sorted = values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if sorted.len() % 2 == 0 {
            (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
        } else {
            sorted[sorted.len() / 2]
        };
        stats.push(ColumnStats {
            column,
            non_missing,
            missing,
            numeric: values.len(),
            mean: Some(mean),
            median: Some(median),
            min: sorted.first().copied(),
            max: sorted.last().copied(),
            stddev: Some(variance.sqrt()),
        });
    }
    let mut output = format!(
        "Descriptive statistics for {} ({} rows)\n\n| column | non-missing | missing | numeric | mean | median | min | max | stddev |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|\n",
        args.path,
        dataset.rows.len()
    );
    for stat in &stats {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            stat.column.replace('|', "\\|"),
            stat.non_missing,
            stat.missing,
            stat.numeric,
            fmt_opt(stat.mean),
            fmt_opt(stat.median),
            fmt_opt(stat.min),
            fmt_opt(stat.max),
            fmt_opt(stat.stddev)
        ));
    }
    output.push_str(
        "\nStandard deviation is population SD (divisor n). Non-numeric values are excluded from numeric statistics. No files were modified.",
    );
    let data = serde_json::json!({
        "path": args.path,
        "rows": dataset.rows.len(),
        "statistics": stats.iter().map(|s| serde_json::json!({
            "column": s.column,
            "non_missing": s.non_missing,
            "missing": s.missing,
            "numeric": s.numeric,
            "mean": s.mean,
            "median": s.median,
            "min": s.min,
            "max": s.max,
            "stddev": s.stddev,
        })).collect::<Vec<_>>(),
    });
    Ok(ToolOutcome::read(
        format!("calculated statistics for {} columns", stats.len()),
        output,
        Some(args.path),
    )
    .with_data(data))
}

pub async fn generate_visualization(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: VisualizationArgs = parse_args(args)?;
    let chart = args.chart_type.to_ascii_lowercase();
    if !matches!(chart.as_str(), "line" | "bar" | "scatter") {
        return Err(ToolFailure::InvalidArguments(
            "chart_type must be line, bar, or scatter".into(),
        ));
    }
    let output_path = ctx.resolve_any(&args.output_path)?;
    require_text_extension(&output_path.absolute, &["svg"])?;
    let delimiter = parse_delimiter(None, &args.path)?;
    let dataset = load_dataset(ctx, "generate_visualization", &args.path, delimiter)?;
    if !dataset.columns.contains(&args.x_column) || !dataset.columns.contains(&args.y_column) {
        return Err(ToolFailure::InvalidArguments(format!(
            "x_column and y_column must both exist; available columns: {}",
            dataset.columns.join(", ")
        )));
    }
    let mut points: Vec<(String, f64)> = dataset
        .rows
        .iter()
        .filter_map(|row| {
            let x = dataset.value(row, &args.x_column);
            let y = dataset.value(row, &args.y_column);
            if is_missing(&x) || is_missing(&y) {
                return None;
            }
            y.trim()
                .parse::<f64>()
                .ok()
                .map(|y| (x.trim().to_string(), y))
        })
        .collect();
    if points.is_empty() {
        return Err(ToolFailure::Unsupported(
            "no numeric y values were available for the chosen columns".into(),
        ));
    }
    // Bound rendering while preserving the overall shape of long series.
    let max_points = 250usize;
    if points.len() > max_points {
        let step = points.len() as f64 / max_points as f64;
        points = (0..max_points)
            .filter_map(|index| points.get((index as f64 * step).floor() as usize).cloned())
            .collect();
    }
    let title = args.title.unwrap_or_else(|| {
        format!(
            "{} of {} by {}",
            args.chart_type, args.y_column, args.x_column
        )
    });
    let svg = render_svg(&title, &args.x_column, &args.y_column, &chart, &points);
    let (path, written) = ctx
        .write_text("generate_visualization", &args.output_path, &svg, false)
        .await?;
    let output = format!(
        "Generated a real SVG {} chart at {} ({} bytes, sha256 {}).\nPoints plotted: {}. Source: {}.\nThis figure is a rendering of the source values; it is not an independent finding.",
        chart,
        path.display(),
        written.bytes,
        written.sha256,
        points.len(),
        args.path
    );
    let data = serde_json::json!({
        "path": path.display(),
        "chart_type": chart,
        "points": points.len(),
        "sha256": written.sha256,
        "source": args.path,
        "x_column": args.x_column,
        "y_column": args.y_column,
    });
    Ok(ToolOutcome::write(
        format!("generated SVG visualization at {}", path.display()),
        output,
        Some(path.display()),
    )
    .with_data(data))
}

pub async fn create_research_report(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    // The JSON schema calls the optional field `abstract`; `abstract` is a Rust
    // keyword, so accept it through serde's rename.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        path: String,
        title: String,
        #[serde(default, rename = "abstract")]
        abstract_text: Option<String>,
        #[serde(default)]
        sections: Vec<ReportSection>,
        #[serde(default)]
        evidence: Vec<String>,
    }
    let args: Input = parse_args(args)?;
    if args.title.trim().is_empty() {
        return Err(ToolFailure::InvalidArguments(
            "title cannot be empty".into(),
        ));
    }
    if args.sections.len() > 40 {
        return Err(ToolFailure::InvalidArguments(
            "at most 40 sections are allowed".into(),
        ));
    }
    if args.evidence.len() > 100 {
        return Err(ToolFailure::InvalidArguments(
            "at most 100 evidence references are allowed".into(),
        ));
    }
    let mut markdown = format!("# {}\n\n", args.title.trim());
    markdown.push_str(&format!(
        "- **Author:** {}\n- **Created:** {}\n- **Status:** draft, not yet verified\n- **Agent workspace:** `AGENTS/{}/`\n\n",
        ctx.agent.display(),
        crate::app::App::now(),
        ctx.agent.dir()
    ));
    if let Some(abstract_text) = args
        .abstract_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        markdown.push_str("## Abstract\n\n");
        markdown.push_str(abstract_text);
        markdown.push_str("\n\n");
    }
    for section in args.sections {
        if section.heading.trim().is_empty() {
            return Err(ToolFailure::InvalidArguments(
                "section headings cannot be empty".into(),
            ));
        }
        markdown.push_str(&format!(
            "## {}\n\n{}\n\n",
            section.heading.trim(),
            section.content.trim()
        ));
    }
    markdown.push_str("## Evidence references\n\n");
    if args.evidence.is_empty() {
        markdown.push_str(
            "_No evidence references were supplied. This report's claims are unverified._\n",
        );
    } else {
        for reference in args.evidence {
            markdown.push_str(&format!("- {}\n", reference.trim()));
        }
    }
    markdown.push_str(
        "\n---\n\nThis report was authored by the named agent. Attribution and timestamps are preserved. Do not interpret agreement with other agents as proof.\n",
    );
    let (path, written) = ctx
        .write_text("create_research_report", &args.path, &markdown, false)
        .await?;
    Ok(ToolOutcome::write(
        format!("created research report {}", path.display()),
        format!(
            "Created {} ({} bytes, sha256 {}). Author: {}. Status: draft.",
            path.display(),
            written.bytes,
            written.sha256,
            ctx.agent.display()
        ),
        Some(path.display()),
    )
    .with_data(serde_json::json!({
        "path": path.display(),
        "author": ctx.agent.dir(),
        "status": "draft",
        "sha256": written.sha256,
    })))
}

pub async fn inspect_research_manifest(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: ManifestArgs = parse_args(args)?;
    let candidate = if let Some(path) = args.path {
        path
    } else if ctx
        .resolve_existing("SHARED/datasets/DATA_MANIFEST.csv")
        .is_ok()
    {
        "SHARED/datasets/DATA_MANIFEST.csv".into()
    } else {
        "PROJECT/DATASET_MANIFEST.md".into()
    };
    let path = ctx.resolve_existing(&candidate)?;
    if !path.is_dir {
        require_text_extension(
            &path.absolute,
            &["csv", "tsv", "md", "markdown", "txt", "json"],
        )?;
    }
    let (path, text) = ctx.read_text("inspect_research_manifest", &candidate, 1_048_576)?;
    let extension = path
        .absolute
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "csv" | "tsv") {
        let delimiter = if extension == "tsv" { '\t' } else { ',' };
        let dataset = parse_delimited(&text, delimiter, "manifest")?;
        let mut output = format!(
            "Manifest: {}\nRows: {}\nColumns: {}\n\n",
            path.display(),
            dataset.rows.len(),
            dataset.columns.join(", ")
        );
        output.push_str(&markdown_table(&dataset.columns, &dataset.rows, 50));
        Ok(ToolOutcome::read(
            format!("read manifest with {} entries", dataset.rows.len()),
            output,
            Some(path.display()),
        ))
    } else {
        Ok(ToolOutcome::read(
            format!("read manifest {}", path.display()),
            format!("Manifest: {}\n\n{}", path.display(), text),
            Some(path.display()),
        ))
    }
}

pub async fn calculate_sha256(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    let path = ctx.resolve_existing(&args.path)?;
    ctx.check_read("calculate_sha256", &path)?;
    if path.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "calculate_sha256 expects a file".into(),
        ));
    }
    let (hash, bytes) =
        fsops::sha256_file(&path.absolute).map_err(|err| ToolFailure::Io(err.to_string()))?;
    Ok(ToolOutcome::read(
        format!("sha256 for {}", path.display()),
        format!("SHA-256: {hash}\nBytes: {bytes}\nPath: {}", path.display()),
        Some(path.display()),
    )
    .with_data(serde_json::json!({"path": path.display(), "sha256": hash, "bytes": bytes})))
}

pub async fn verify_file_integrity(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: IntegrityArgs = parse_args(args)?;
    let manifest = ctx.resolve_existing(&args.manifest_path)?;
    ctx.check_read("verify_file_integrity", &manifest)?;
    if manifest.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "manifest_path must be a file".into(),
        ));
    }
    let (_, text) = ctx.read_text(
        "verify_file_integrity",
        &args.manifest_path,
        4 * 1024 * 1024,
    )?;
    let delimiter = if manifest.absolute.extension().and_then(|e| e.to_str()) == Some("tsv") {
        '\t'
    } else {
        ','
    };
    let dataset = parse_delimited(&text, delimiter, "integrity manifest")?;
    let path_column = find_column(&dataset.columns, &["path", "file", "relative_path"]);
    let hash_column = find_column(&dataset.columns, &["sha256", "sha_256", "hash"]);
    let (Some(path_column), Some(hash_column)) = (path_column, hash_column) else {
        return Err(ToolFailure::InvalidArguments(
            "manifest must contain path and sha256 columns".into(),
        ));
    };
    let manifest_dir = manifest.absolute.parent().unwrap_or(ctx.workspace_root());
    let mut matched = 0usize;
    let mut mismatched = 0usize;
    let mut missing = 0usize;
    let mut unreadable = 0usize;
    let mut lines = Vec::new();
    for (index, row) in dataset.rows.iter().enumerate() {
        let listed_path = dataset.value(row, &path_column);
        let expected = dataset.value(row, &hash_column).trim().to_ascii_lowercase();
        if listed_path.trim().is_empty() {
            continue;
        }
        if expected.is_empty() || expected == "pending" {
            mismatched += 1;
            lines.push(format!(
                "line {}: {} has no recorded SHA-256",
                index + 2,
                listed_path
            ));
            continue;
        }
        let relative_to_manifest = manifest_dir.join(&listed_path);
        let candidate = if listed_path.starts_with("PROJECT/")
            || listed_path.starts_with("SHARED/")
            || listed_path.starts_with("AGENTS/")
            || listed_path.starts_with("COLLABORATION/")
            || listed_path.starts_with("VERIFICATION/")
        {
            listed_path.clone()
        } else if let Ok(relative) = relative_to_manifest.strip_prefix(ctx.workspace_root()) {
            relative.to_string_lossy().to_string()
        } else {
            unreadable += 1;
            lines.push(format!(
                "line {}: {} resolves outside the workspace",
                index + 2,
                listed_path
            ));
            continue;
        };
        let resolved = match ctx
            .app
            .sandbox()
            .resolve(&candidate, crate::sandbox::Existence::MustExist)
        {
            Ok(resolved) => resolved,
            Err(_) => {
                missing += 1;
                lines.push(format!("line {}: MISSING {}", index + 2, listed_path));
                continue;
            }
        };
        if ctx.check_read("verify_file_integrity", &resolved).is_err() {
            unreadable += 1;
            lines.push(format!("line {}: UNREADABLE {}", index + 2, listed_path));
            continue;
        }
        let (actual, _) = fsops::sha256_file(&resolved.absolute)
            .map_err(|err| ToolFailure::Io(err.to_string()))?;
        if actual.eq_ignore_ascii_case(&expected) {
            matched += 1;
        } else {
            mismatched += 1;
            lines.push(format!(
                "line {}: MISMATCH {} (expected {}, actual {})",
                index + 2,
                listed_path,
                &expected[..expected.len().min(16)],
                &actual[..16]
            ));
        }
    }
    let output = format!(
        "Integrity manifest: {}\nEntries checked: {}\nMatched: {matched}\nMismatched: {mismatched}\nMissing: {missing}\nUnreadable: {unreadable}\n\n{}",
        manifest.display(),
        matched + mismatched + missing + unreadable,
        if lines.is_empty() {
            "All listed files matched their recorded hashes.".to_string()
        } else {
            lines.join("\n")
        }
    );
    Ok(ToolOutcome::read(
        format!("verified {matched} matching, {mismatched} mismatched, {missing} missing"),
        output,
        Some(manifest.display()),
    )
    .with_data(serde_json::json!({
        "manifest": manifest.display(),
        "matched": matched,
        "mismatched": mismatched,
        "missing": missing,
        "unreadable": unreadable,
    })))
}

fn load_dataset(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    delimiter: char,
) -> Result<Dataset, ToolFailure> {
    let resolved = ctx.resolve_existing(path)?;
    if resolved.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "dataset path must be a file".into(),
        ));
    }
    let meta = fs::metadata(&resolved.absolute).map_err(|err| ToolFailure::Io(err.to_string()))?;
    if meta.len() > MAX_DATASET_BYTES {
        return Err(ToolFailure::InvalidArguments(format!(
            "dataset is {} bytes; the limit is {}",
            meta.len(),
            MAX_DATASET_BYTES
        )));
    }
    let (_, text) = ctx.read_text(tool, path, MAX_DATASET_BYTES)?;
    let extension = resolved
        .absolute
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut dataset = match extension.as_str() {
        "json" => parse_json_dataset(&text)?,
        "jsonl" | "ndjson" => parse_jsonl_dataset(&text)?,
        _ => parse_delimited(&text, delimiter, "dataset")?,
    };
    dataset.total_bytes = meta.len();
    Ok(dataset)
}

fn parse_delimiter(given: Option<&str>, path: &str) -> Result<char, ToolFailure> {
    if let Some(value) = given {
        let mut chars = value.chars();
        let Some(ch) = chars.next() else {
            return Err(ToolFailure::InvalidArguments(
                "delimiter cannot be empty".into(),
            ));
        };
        if chars.next().is_some() {
            return Err(ToolFailure::InvalidArguments(
                "delimiter must be one character".into(),
            ));
        }
        return Ok(ch);
    }
    if Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tsv"))
    {
        Ok('\t')
    } else {
        Ok(',')
    }
}

fn parse_delimited(text: &str, delimiter: char, label: &str) -> Result<Dataset, ToolFailure> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter as u8)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|err| ToolFailure::InvalidArguments(format!("{label} header error: {err}")))?
        .iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if headers.is_empty() {
        return Err(ToolFailure::Unsupported(format!(
            "{label} has no header row"
        )));
    }
    if headers.len() > MAX_COLUMNS {
        return Err(ToolFailure::InvalidArguments(
            "dataset has too many columns".into(),
        ));
    }
    if headers.iter().any(|header| header.trim().is_empty()) {
        return Err(ToolFailure::InvalidArguments(
            "dataset has an empty column name".into(),
        ));
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record
            .map_err(|err| ToolFailure::InvalidArguments(format!("{label} row error: {err}")))?;
        if rows.len() >= MAX_ROWS {
            return Err(ToolFailure::InvalidArguments(format!(
                "dataset exceeds the {MAX_ROWS} row analysis limit"
            )));
        }
        let mut row = HashMap::new();
        for (index, column) in headers.iter().enumerate() {
            row.insert(
                column.clone(),
                record.get(index).unwrap_or("").trim().to_string(),
            );
        }
        rows.push(row);
    }
    Ok(Dataset {
        columns: headers,
        rows,
        delimiter,
        format: if delimiter == '\t' { "TSV" } else { "CSV" },
        total_bytes: text.len() as u64,
    })
}

fn parse_json_dataset(text: &str) -> Result<Dataset, ToolFailure> {
    let value: Value = serde_json::from_str(text)
        .map_err(|err| ToolFailure::InvalidArguments(format!("invalid JSON dataset: {err}")))?;
    let array = value.as_array().ok_or_else(|| {
        ToolFailure::Unsupported("JSON datasets must be an array of objects".into())
    })?;
    parse_json_objects(array)
}

fn parse_jsonl_dataset(text: &str) -> Result<Dataset, ToolFailure> {
    let mut objects = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|err| {
            ToolFailure::InvalidArguments(format!("invalid JSON on line {}: {err}", index + 1))
        })?;
        if !value.is_object() {
            return Err(ToolFailure::Unsupported(
                "JSON Lines records must be objects".into(),
            ));
        }
        objects.push(value);
        if objects.len() > MAX_ROWS {
            return Err(ToolFailure::InvalidArguments(format!(
                "dataset exceeds the {MAX_ROWS} row analysis limit"
            )));
        }
    }
    parse_json_objects(&objects)
}

fn parse_json_objects(array: &[Value]) -> Result<Dataset, ToolFailure> {
    let mut column_set = BTreeSet::new();
    for value in array {
        let object = value
            .as_object()
            .ok_or_else(|| ToolFailure::Unsupported("dataset rows must be JSON objects".into()))?;
        for key in object.keys() {
            column_set.insert(key.clone());
        }
    }
    let columns: Vec<String> = column_set.into_iter().collect();
    if columns.len() > MAX_COLUMNS {
        return Err(ToolFailure::InvalidArguments(
            "dataset has too many columns".into(),
        ));
    }
    let mut rows = Vec::with_capacity(array.len());
    for value in array {
        let object = value.as_object().expect("validated object");
        let mut row = HashMap::new();
        for column in &columns {
            let rendered = match object.get(column) {
                Some(Value::Null) | None => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
            };
            row.insert(column.clone(), rendered);
        }
        rows.push(row);
    }
    Ok(Dataset {
        columns,
        rows,
        delimiter: ',',
        format: "JSON",
        total_bytes: 0,
    })
}

fn markdown_table(columns: &[String], rows: &[HashMap<String, String>], limit: usize) -> String {
    let mut output = String::new();
    output.push('|');
    for column in columns {
        output.push_str(&format!(" {} |", column.replace('|', "\\|")));
    }
    output.push_str("\n|");
    for _ in columns {
        output.push_str("---|");
    }
    output.push('\n');
    for row in rows.iter().take(limit) {
        output.push('|');
        for column in columns {
            let value = row.get(column).cloned().unwrap_or_default();
            let value: String = value.chars().take(80).collect();
            output.push_str(&format!(
                " {} |",
                value.replace('|', "\\|").replace('\n', " ")
            ));
        }
        output.push('\n');
    }
    if rows.len() > limit {
        output.push_str(&format!("\n_{} of {} rows shown._\n", limit, rows.len()));
    }
    output
}

fn is_missing(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "na" | "n/a" | "null" | "none" | "nan" | "."
        )
}

fn fmt_opt(value: Option<f64>) -> String {
    value
        .map(|value| {
            if value.abs() >= 1_000_000.0 || (value != 0.0 && value.abs() < 0.001) {
                format!("{value:.6e}")
            } else {
                format!("{value:.6}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            }
        })
        .unwrap_or_else(|| "—".into())
}

fn find_column(columns: &[String], candidates: &[&str]) -> Option<String> {
    columns
        .iter()
        .find(|column| {
            let lower = column.to_ascii_lowercase();
            candidates.iter().any(|candidate| lower == *candidate)
        })
        .cloned()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_svg(
    title: &str,
    x_label: &str,
    y_label: &str,
    chart: &str,
    points: &[(String, f64)],
) -> String {
    let width = 960.0;
    let height = 560.0;
    let left = 90.0;
    let right = 40.0;
    let top = 70.0;
    let bottom = 80.0;
    let plot_w = width - left - right;
    let plot_h = height - top - bottom;
    let min_y = points.iter().map(|(_, y)| *y).fold(f64::INFINITY, f64::min);
    let mut max_y = points
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max);
    if (max_y - min_y).abs() < f64::EPSILON {
        max_y = min_y + 1.0;
    }
    let mut min_y = min_y;
    if min_y > 0.0 {
        min_y = 0.0;
    }
    let y_for = |y: f64| top + plot_h - ((y - min_y) / (max_y - min_y)) * plot_h;
    let x_for = |index: usize| {
        if points.len() <= 1 {
            left + plot_w / 2.0
        } else {
            left + (index as f64 / (points.len() - 1) as f64) * plot_w
        }
    };
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{}">
<rect width="100%" height="100%" fill="#ffffff"/>
<text x="{}" y="34" font-family="Helvetica, Arial, sans-serif" font-size="20" font-weight="600" fill="#17212b">{}</text>
<line x1="{left}" y1="{}" x2="{}" y2="{}" stroke="#aab4c0" stroke-width="1.2"/>
<line x1="{left}" y1="{top}" x2="{left}" y2="{}" stroke="#aab4c0" stroke-width="1.2"/>
"##,
        xml_escape(title),
        width / 2.0,
        xml_escape(title),
        top + plot_h,
        left + plot_w,
        top + plot_h,
        top + plot_h
    );
    // Horizontal grid and y labels.
    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let y = top + plot_h - fraction * plot_h;
        let value = min_y + fraction * (max_y - min_y);
        svg.push_str(&format!(
            "<line x1=\"{left}\" y1=\"{y:.1}\" x2=\"{:.1}\" y2=\"{y:.1}\" stroke=\"#e6ebf0\" stroke-width=\"1\"/>\n",
            left + plot_w
        ));
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"12\" fill=\"#5d6a78\">{}</text>\n",
            left - 12.0,
            y + 4.0,
            xml_escape(&fmt_opt(Some(value)))
        ));
    }
    match chart {
        "bar" => {
            let bar_width = (plot_w / points.len() as f64 * 0.68).max(1.0);
            for (index, (_, y)) in points.iter().enumerate() {
                let x = x_for(index) - bar_width / 2.0;
                let y_px = y_for(*y);
                let base = y_for(0.0f64.max(min_y));
                svg.push_str(&format!(
                    "<rect x=\"{x:.1}\" y=\"{y_px:.1}\" width=\"{bar_width:.1}\" height=\"{:.1}\" fill=\"#2d6cdf\"><title>{}</title></rect>\n",
                    (base - y_px).abs(),
                    xml_escape(&format!("{}: {}", points[index].0, points[index].1))
                ));
            }
        }
        "scatter" => {
            for (index, (_, y)) in points.iter().enumerate() {
                svg.push_str(&format!(
                    "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"4\" fill=\"#2d6cdf\" fill-opacity=\"0.8\"><title>{}</title></circle>\n",
                    x_for(index),
                    y_for(*y),
                    xml_escape(&format!("{}: {}", points[index].0, points[index].1))
                ));
            }
        }
        _ => {
            let path = points
                .iter()
                .enumerate()
                .map(|(index, (_, y))| format!("{:.1},{:.1}", x_for(index), y_for(*y)))
                .collect::<Vec<_>>()
                .join(" ");
            svg.push_str(&format!(
                "<polyline points=\"{path}\" fill=\"none\" stroke=\"#2d6cdf\" stroke-width=\"2.2\" stroke-linejoin=\"round\"/>\n"
            ));
        }
    }
    // X labels: first, middle, last, plus a few evenly spaced labels.
    let label_count = points.len().min(8);
    for step in 0..label_count {
        let index = if label_count <= 1 {
            0
        } else {
            step * (points.len() - 1) / (label_count - 1)
        };
        let label = &points[index].0;
        let label: String = label.chars().take(18).collect();
        svg.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"11\" fill=\"#5d6a78\">{}</text>\n",
            x_for(index),
            top + plot_h + 22.0,
            xml_escape(&label)
        ));
    }
    svg.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"13\" fill=\"#3c4a58\">{}</text>\n",
        width / 2.0,
        height - 24.0,
        xml_escape(x_label)
    ));
    svg.push_str(&format!(
        "<text x=\"22\" y=\"{:.1}\" text-anchor=\"middle\" transform=\"rotate(-90 22 {:.1})\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"13\" fill=\"#3c4a58\">{}</text>\n",
        top + plot_h / 2.0,
        top + plot_h / 2.0,
        xml_escape(y_label)
    ));
    svg.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\" font-family=\"Helvetica, Arial, sans-serif\" font-size=\"10\" fill=\"#8793a0\">Generated by Unified AI Bridge · source column {}</text>\n",
        width - 20.0,
        height - 8.0,
        xml_escape(x_label)
    ));
    svg.push_str("</svg>\n");
    svg
}
