# Read and query CSV/TSV files

The 0.5.12 candidate adds `read_table` and `query_table` to all profiles. These are read-only operations for UTF-8 delimited text with a header row. They do not edit workbooks, evaluate formulas, run SQL, fetch external references, or read XLSX files.

In **Files, preview and restore → CSV and TSV tables**, enter a path and choose **Read table**. A `.csv` extension selects comma separation; `.tsv` selects tabs. The tool runner can override `delimiter` with `comma` or `tab`. Standard quoted delimiters and multiline fields are supported. Headers must be unique and nonempty; strings such as `001` stay strings.

`columns` selects 1–20 exact header names; omitted selection returns the first 20. Every returned row includes its 1-based data-record number and starting physical file line. A multiline field counts as one record. Results include the complete header list, omitted-column count, source SHA-256, and the total number of data records.

Use `next_offset` and the same `expected_sha256`, columns and filters for continuation. The panel saves those arguments in its next-page control. A changed file refuses continuation. The file is parsed again on each call; this is not a live table snapshot held open between calls.

## Structured queries

For example, the query control accepts:

```json
{
  "columns": ["category", "amount"],
  "filters": [
    {"op": "equals", "column": "category", "value": "supplies"},
    {"op": "number_range", "column": "amount", "min": 10, "max": 100}
  ],
  "aggregate_columns": ["amount"],
  "limit": 30
}
```

Filters are ANDed. `equals` and `contains` compare text case-sensitively; `is_empty` matches exactly an empty cell, not whitespace. `number_range` parses trimmed finite numbers and accepts inclusive `min`, `max`, or both. Non-numeric and non-finite values do not match a numeric range. There are no expressions, joins, implicit date conversions or arbitrary sort programs.

For each requested aggregate column, results include numeric/empty/non-numeric counts, sum, mean, min and max. Aggregates cover **all matched records**, even when the displayed page contains only one row. They use approximate `f64` arithmetic, which can round large integers and decimal values. Empty/non-numeric cells are counted and skipped. No numeric values produces null aggregate values; an overflowing sum returns an error. These operations are unsuitable for exact financial accounting without an additional exact-decimal implementation.

## Limits and failures

- Input: 4 MiB, 10,000 records, 128 columns, 250,000 cells, 8,192 bytes per cell and 8,000 bytes of serialized header metadata.
- Output page: 1–100 records, 20 projected columns, 24,000 bytes of row data; full result capped at 56,000 bytes.
- Query: at most eight filters and eight aggregate columns. Filter text is capped at 1,024 bytes.
- Parsing: two-second budget checked between records. Filesystem operations are not preempted by this check.

An oversized table, malformed record, invalid UTF-8, duplicate header or overflow returns an error rather than a partial aggregate. A single projected row that exceeds the output budget asks for fewer columns. Private files, symlink/hard-link sources, traversal and unknown arguments are denied. Research dataset reads leave the original bytes unchanged.

Five dedicated tests cover quoted Unicode/multiline data, pagination, stale files, complete aggregates, numeric exclusions, limits, malformed input, policy denials and literal formula-looking text. These are temporary test fixtures, not provider integrations. Native transport verification uses CSV records from this repository's actual Git history.

XLSX support is provided by the separate [workbook tools](WORKBOOKS.md) in 0.5.16. UAB-18 remains open for live browser acceptance and broader editing workflows; these CSV/TSV tools remain read-only.
