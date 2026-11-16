# Read XLSX cells and save reviewed edits

The 0.5.16 candidate adds `inspect_workbook`, `read_sheet_range` and `write_sheet_range`. The implementation uses Rust, Calamine 0.38.0 and the existing ZIP/XML validation inside a disposable native worker. This is a declared XLSX subset, not Excel automation or a general workbook editor. Browser acceptance remains pending.

In **Files, preview and restore → XLSX cells**, inspect a `.xlsx`, enter an exact worksheet name and an uppercase A1 range, then read its cells. To edit, choose a new `.xlsx` destination and supply a rectangular JSON matrix matching the range. The preview lists each old typed value and proposed literal value before approval.

## Reading

`inspect_workbook` returns worksheet names, visibility, stored-cell and formula counts, last stored row/column, source SHA-256 and known editing limitations. Hidden worksheets remain identifiable as hidden. A missing stored cell is not counted; formatted empty cells may be counted. `edit_supported` means the package passed the coarse feature check; individual cells and requested ranges have additional checks.

`read_sheet_range` accepts one cell such as `A1`, or a rectangle such as `A1:D20`, up to 400 cells. Addresses are one-based uppercase Excel notation, without dollar signs or sheet prefixes. Returned rows contain explicit cell addresses, stored typed values, formula metadata and a `cached` flag. Empty/missing cells return null. Pass `expected_sha256` from inspection for consistent reads across multiple ranges. Changed files return `file_changed`.

Formulas are **never evaluated**. Their stored result caches may be missing or stale. Ordinary formula text and shared-formula anchor text are exposed; shared-derived cells identify their group without inventing expanded formula text. Dates include Excel serial values and interpreted components; the reader does not reproduce display formatting, time zones, workbook layout or chart meaning. Strings remain strings, including text that resembles formulas. External relationships are never opened.

## Editing a new copy

`write_sheet_range` requires a source hash and creates a **new** file after explicit approval. A matrix of 1–100 existing cells can contain strings, numbers, booleans or null. A leading `=` in a string remains literal text. Null clears a stored value while retaining its cell style. Numeric input uses Excel/reader floating-point semantics; values beyond ±9,007,199,254,740,991 are refused. Use strings for long identifiers or exact decimal text.

The current editor changes existing plain cell elements only. It does not insert missing cells, add sheets, extend dimensions, recalculate formulas, autofit columns or change formatting. Longer text may overflow or clip in the existing layout. It preserves selected style indices, all bytes outside selected elements in the worksheet XML, and the uncompressed payload of every other package part. ZIP container bytes and selected entry metadata can differ. The saved workbook is reopened and its cells and untouched package payloads checked before approval.

Editing is refused for formulas anywhere in the workbook, defined names, merges, tables, drawings, external links, protection, rich/phonetic shared strings, signatures, extensions and other unsupported package features. Individual rich/phonetic cells or unusual cell metadata are also refused. No feature is silently flattened to make an edit succeed. Reading may support a workbook whose editing is refused; the limitations remain explicit.

Before publication, the source hash and current path/workspace policy are checked again. The operation stages a new file, atomically refuses existing destinations and writes an audit receipt. Denial, changed source, changed destination, workspace locking, symlink/hard-link sources, private paths, research datasets and other agents' research folders do not permit replacement. In-place or large-file replacement is not part of this tool; original workbooks remain available for recovery.

## Bounds

- Non-macro XLSX only; XLS, XLSB, XLSM, ODS and encrypted workbooks are unsupported.
- Input 16 MiB, at most 256 ZIP entries, 32 MiB declared expansion and 8 MiB per XML part. ZIP traversal, collisions, links, unsupported compression and malformed data are refused. Nothing is unpacked to filesystem paths supplied by the workbook.
- XML: no DTDs, UTF-8 only, at most 150,000 nodes per part, 300,000 overall and 64 levels. Declared counts/shared indices are capped at 100,000 before the reader can reserve memory from them.
- At most 50 worksheets; metadata scans up to 100,000 stored cells across the workbook. A selected sheet read scans up to 100,000 cells using the streaming API, without allocating a dense sheet from its dimension field.
- Cell text 8,192 bytes; reads at most 400 cells and 45,000 result bytes. A larger result asks for a smaller range rather than discarding cells.
- Edits at most 100 cells, 48,000 request bytes, 50,000 approval-review bytes and 16 MiB output. Every proposed value is visible in the review.
- Worker: two shared document slots, five CPU seconds, eight wall seconds, bounded pipes and disabled core dumps. The worker receives bytes, not workspace paths or configuration. This is reliability isolation; unlike the macOS PDF worker, it does not install an OS filesystem/network sandbox. Maintained parsing code performs no network or referenced-file access.

Six dedicated integration tests cover typed reads/caches/hidden sheets, Unicode and escaped literal text, preserved styles/parts, approval and stale-state denials, malformed packages, hostile declared counts, unsupported feature refusal and research/private-path policy. Public-workbook and independent-viewer evidence are recorded in [verification](VERIFICATION.md).

UAB-18 remains open for live browser acceptance and broader editing workflows. These tools do not claim formula recalculation, arbitrary workbook fidelity or a connection to the Excel application.
