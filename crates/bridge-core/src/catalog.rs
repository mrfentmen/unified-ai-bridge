use serde::Serialize;

/// A tool exposed to every bridge session. The catalog is intentionally one
/// flat namespace: the model-facing protocol stays simple and validated.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
    /// Human-readable argument summary used in the injected instructions.
    pub arguments: &'static str,
    /// JSON Schema 2020-12 object used by the RMCP server.
    pub schema: &'static str,
    /// True when the tool can mutate the workspace.
    pub writes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Filesystem,
    Research,
    Collaboration,
    Development,
    Desktop,
    Network,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Filesystem,
        Category::Research,
        Category::Collaboration,
        Category::Development,
        Category::Desktop,
        Category::Network,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Filesystem => "FILESYSTEM",
            Category::Research => "RESEARCH",
            Category::Collaboration => "COLLABORATION",
            Category::Development => "DEVELOPMENT",
            Category::Desktop => "DESKTOP",
            Category::Network => "NETWORK",
        }
    }
}

const PATH: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path"}},"required":["path"],"additionalProperties":false}"#;

pub const TOOLS: &[ToolSpec] = &[
    ToolSpec{name:"fetch_url_text",category:Category::Network,description:"After explicit approval, GET bounded anonymous public UTF-8 text/HTML from a user-granted origin. Network disabled by default. No private/local IPs, cookies, auth, query strings, URL credentials or custom ports. Static HTML tokens only; no CSS/JavaScript/rendering. Caps: 512 KiB body, 24 KiB text, 20 seconds, 3 checked redirects.",arguments:"url (public HTTP/S), max_bytes? (256–24000 text)",schema:r#"{"type":"object","properties":{"url":{"type":"string","maxLength":2048},"max_bytes":{"type":"integer","minimum":256,"maximum":24000}},"required":["url"],"additionalProperties":false}"#,writes:false},
    ToolSpec{name:"download_file",category:Category::Network,description:"Approve an anonymous public GET from an explicitly granted origin into a NEW workspace file, at most 8 MiB/20 seconds/3 checked redirects. Network off by default; no cookies/auth/private addresses/query strings/custom ports. Never executes content or overwrites files. Authenticated chat files use the browser artifact importer instead.",arguments:"url (public HTTP/S), destination (new file)",schema:r#"{"type":"object","properties":{"url":{"type":"string","maxLength":2048},"destination":{"type":"string"}},"required":["url","destination"],"additionalProperties":false}"#,writes:true},
    ToolSpec{name:"inspect_workbook",category:Category::Filesystem,description:"Inspect a bounded non-macro XLSX package: sheet names, visibility, stored cell/formula counts and edit limitations. No formula execution or external links. XLS/XLSB/XLSM/ODS unavailable.",arguments:"path (.xlsx)",schema:PATH,writes:false},
    ToolSpec{name:"read_sheet_range",category:Category::Filesystem,description:"Read up to 400 XLSX cells using an exact sheet name and uppercase A1 range. Returns stored typed values, physical cell addresses and formula/cache metadata. No recalculation; caches may be stale or absent. Shared-derived formulas are not expanded. Pass the inspected hash for consistent reads.",arguments:"path, sheet, range (A1 or A1:D20), expected_sha256?",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"sheet":{"type":"string"},"range":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","sheet","range"],"additionalProperties":false}"#,writes:false},
    ToolSpec{name:"write_sheet_range",category:Category::Filesystem,description:"Create a new explicitly approved XLSX by changing 1–100 existing plain cells. Preserves original, selected styles and all untouched package payloads. Requires source hash, new destination and a rectangular literal matrix. Rejects formulas, merges, tables, drawings, protection and other unsupported editing features; no overwrites.",arguments:"path, destination (new .xlsx), expected_sha256, sheet, range, values (rows of string/number/boolean/null)",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"destination":{"type":"string"},"expected_sha256":{"type":"string"},"sheet":{"type":"string"},"range":{"type":"string"},"values":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"array","minItems":1,"maxItems":100,"items":{"type":["string","number","boolean","null"]}}}},"required":["path","destination","expected_sha256","sheet","range","values"],"additionalProperties":false}"#,writes:true},
    ToolSpec{name:"integration_status",category:Category::Development,description:"Check current workspace integration grant and executable hash without starting a process. Reports configured_unprobed or blocked; never treats configuration as a live connection. In-flight calls stop on grant removal/workspace lock with an explicit uncertain outcome.",arguments:"server_id",schema:r#"{"type":"object","properties":{"server_id":{"type":"string"}},"required":["server_id"],"additionalProperties":false}"#,writes:false},
    ToolSpec {name:"document_info",category:Category::Filesystem,description:"Inspect non-macro DOCX body metadata or PDF physical pages (macOS only) in an isolated native worker. PDF input capped at 16 MiB/200 pages; encrypted PDFs refused. DOCX has no reliable page count. OCR/DOC/DOCM unavailable.",arguments:"path (.docx or .pdf)",schema:PATH,writes:false},
    ToolSpec {name:"extract_document_text",category:Category::Filesystem,description:"Read bounded DOCX body or PDF embedded text (macOS) with byte offset and SHA256 continuation. PDF returns physical page ranges and pages without text; no OCR. DOCX reports omitted content and cached fields. No links, scripts or external references are opened.",arguments:"path (.docx or .pdf), offset? (returned byte offset), max_bytes? (256–24000), expected_sha256? (required for continuation)",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":256,"maximum":24000},"expected_sha256":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,writes:false},
    ToolSpec {name:"create_document",category:Category::Filesystem,description:"Create an explicitly approved new PDF report with title, headings, paragraphs, bullets and page breaks. Fixed A4 template with embedded fonts; simple left-to-right Latin text, 24,000 UTF-8 bytes, 200 blocks, 40 pages and 4 MiB output. No overwrites, markup, links, images, forms, complex shaping or DOCX generation.",arguments:"destination (new .pdf), format=pdf, template=report, title, blocks [{type:heading|paragraph|bullet,text}|{type:page_break}]",schema:r#"{"type":"object","properties":{"destination":{"type":"string"},"format":{"const":"pdf"},"template":{"const":"report"},"title":{"type":"string","minLength":1,"maxLength":240},"blocks":{"type":"array","minItems":1,"maxItems":200,"items":{"oneOf":[{"type":"object","properties":{"type":{"const":"heading"},"text":{"type":"string","minLength":1,"maxLength":240}},"required":["type","text"],"additionalProperties":false},{"type":"object","properties":{"type":{"enum":["paragraph","bullet"]},"text":{"type":"string","minLength":1,"maxLength":4000}},"required":["type","text"],"additionalProperties":false},{"type":"object","properties":{"type":{"const":"page_break"}},"required":["type"],"additionalProperties":false}]}}},"required":["destination","format","template","title","blocks"],"additionalProperties":false}"#,writes:true},
    ToolSpec {name:"render_document_pages",category:Category::Filesystem,description:"On macOS, render 1–4 PDF physical pages as PNG page-content previews into a new approved folder. SHA256 rechecked after approval; no overwrites. At most 1600 pixels per side and 16 MiB total PNGs. Annotations/forms, DOCX rendering, OCR and chat image attachment are not included.",arguments:"path (.pdf), destination (new folder), expected_sha256, first_page (1-based), last_page? (inclusive), max_dimension? (128–1600)",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"destination":{"type":"string"},"expected_sha256":{"type":"string"},"first_page":{"type":"integer","minimum":1,"maximum":200},"last_page":{"type":"integer","minimum":1,"maximum":200},"max_dimension":{"type":"integer","minimum":128,"maximum":1600}},"required":["path","destination","expected_sha256","first_page"],"additionalProperties":false}"#,writes:true},
    ToolSpec {
        name:"read_table",category:Category::Filesystem,
        description:"Read bounded UTF-8 CSV/TSV data with headers, selected columns, physical line/record numbers and hash-checked pages. Up to 4 MiB, 10,000 records, 250,000 cells. Strings remain strings; formulas never execute. XLSX is not supported.",
        arguments:"path, delimiter? (comma|tab), columns? (1–20 names; default first 20), offset?, limit? (1–100), expected_sha256? (required after offset zero)",
        schema:r#"{"type":"object","properties":{"path":{"type":"string"},"delimiter":{"enum":["comma","tab"]},"columns":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":20},"offset":{"type":"integer","minimum":0,"maximum":10000},"limit":{"type":"integer","minimum":1,"maximum":100},"expected_sha256":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        writes:false,
    },
    ToolSpec {
        name:"query_table",category:Category::Filesystem,
        description:"Query bounded CSV/TSV using ANDed structured text/empty/numeric-range filters. Optional finite-f64 aggregates cover all matched records; rounding and skipped values reported. No SQL, formula evaluation, writes or partial aggregates.",
        arguments:"path, delimiter?, columns?, filters? [{op: equals|contains|is_empty|number_range,column,value? min? max?}], aggregate_columns? (up to 8), offset?, limit?, expected_sha256?",
        schema:r#"{"type":"object","properties":{"path":{"type":"string"},"delimiter":{"enum":["comma","tab"]},"columns":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":20},"filters":{"type":"array","maxItems":8,"items":{"oneOf":[{"type":"object","properties":{"op":{"enum":["equals","contains"]},"column":{"type":"string"},"value":{"type":"string","maxLength":1024}},"required":["op","column","value"],"additionalProperties":false},{"type":"object","properties":{"op":{"const":"is_empty"},"column":{"type":"string"}},"required":["op","column"],"additionalProperties":false},{"type":"object","properties":{"op":{"const":"number_range"},"column":{"type":"string"},"min":{"type":"number"},"max":{"type":"number"}},"required":["op","column"],"additionalProperties":false}]}},"aggregate_columns":{"type":"array","items":{"type":"string"},"maxItems":8},"offset":{"type":"integer","minimum":0,"maximum":10000},"limit":{"type":"integer","minimum":1,"maximum":100},"expected_sha256":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,
        writes:false,
    },
    ToolSpec{name:"list_symbols",category:Category::Development,description:"List Rust .rs syntax declarations with file/line/column and SHA256. Bounded isolated parser; no macros executed or expanded, no semantic reference resolution, no external-module traversal. Other languages use search_files. Paginate with next_offset and the returned SHA256; max 1000 declarations parsed.",arguments:"path, max_results? (1-100), offset?, expected_sha256? (required after offset zero)",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":100},"offset":{"type":"integer","minimum":0,"maximum":1000},"expected_sha256":{"type":"string"}},"required":["path"],"additionalProperties":false}"#,writes:false},
    ToolSpec {name:"replace_file",category:Category::Filesystem,description:"Replace an existing regular workspace file with another file up to 64 MiB after separate approval. Both source and destination SHA256 required; preserves source, private snapshot and original inode in trash. Budgeted storage; list_changes/restore_change provide undo. A detach/publish conflict requires reconciliation, never blind retry. No directory, symlink, hard-link or Git-internal replacement.",arguments:"source, destination, source_sha256, expected_sha256",schema:r#"{"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"},"source_sha256":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["source","destination","source_sha256","expected_sha256"],"additionalProperties":false}"#,writes:true},
    ToolSpec{name:"image_info",category:Category::Filesystem,description:"Inspect bounded PNG/JPEG/WebP metadata, dimensions, animation, orientation and SHA256. This does not validate every pixel or send/understand the image. Up to 16 MiB input, 8192 per side and 16,777,216 pixels.",arguments:"path",schema:PATH,writes:false},
    ToolSpec{name:"transform_image",category:Category::Filesystem,description:"Create a NEW image after approval: exact resize, crop, clockwise 90/180/270 rotation or conversion. PNG/JPEG/WebP 8-bit still images; refuses animation, embedded ICC and transparent JPEG. Applies EXIF orientation; other metadata stripped. Input hash required, no overwrite or binary undo. Up to 32 MiB output.",arguments:"path, destination, expected_sha256, operation: {kind: resize, width, height} | {kind: crop, x, y, width, height} | {kind: rotate, degrees} | {kind: convert}, format? (png/jpeg/webp; default png)",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"destination":{"type":"string"},"expected_sha256":{"type":"string"},"format":{"enum":["png","jpeg","webp"]},"operation":{"oneOf":[{"type":"object","properties":{"kind":{"const":"resize"},"width":{"type":"integer","minimum":1,"maximum":8192},"height":{"type":"integer","minimum":1,"maximum":8192}},"required":["kind","width","height"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"crop"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"width":{"type":"integer","minimum":1,"maximum":8192},"height":{"type":"integer","minimum":1,"maximum":8192}},"required":["kind","x","y","width","height"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"rotate"},"degrees":{"enum":[90,180,270]}},"required":["kind","degrees"],"additionalProperties":false},{"type":"object","properties":{"kind":{"const":"convert"}},"required":["kind"],"additionalProperties":false}]}},"required":["path","destination","expected_sha256","operation"],"additionalProperties":false}"#,writes:true},
    ToolSpec { name:"list_archive",category:Category::Filesystem,description:"Inspect a ZIP's entries and SHA256 without extracting. ZIP32 with one unambiguous end record, Stored/Deflate only; up to 256 entries, 32 MiB archive, 16 MiB per file, 64 MiB expanded, depth 12. Unsafe paths, links, duplicate/colliding names and encrypted ZIPs are rejected.",arguments:"path",schema:PATH,writes:false },
    ToolSpec { name:"extract_archive",category:Category::Filesystem,description:"Extract a reviewed ZIP into a NEW folder under an existing parent, after separate approval. Requires SHA256 from list_archive. Checks CRC, paths and size budgets; refuses overwrites and protected destinations. Does not execute files or unpack nested archives.",arguments:"path, destination, expected_sha256",schema:r#"{"type":"object","properties":{"path":{"type":"string"},"destination":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","destination","expected_sha256"],"additionalProperties":false}"#,writes:true },
    ToolSpec { name:"create_archive",category:Category::Filesystem,description:"Create a NEW ZIP from 1–256 explicitly selected regular workspace files after separate approval; no directory recursion. Preserves sources, rechecks hashes, strips execution permissions and refuses overwrite. Up to 64 MiB input and 32 MiB output; no automatic binary undo.",arguments:"paths: [workspace-relative file paths], destination",schema:r#"{"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":256},"destination":{"type":"string"}},"required":["paths","destination"],"additionalProperties":false}"#,writes:true },
    ToolSpec { name:"preview_patch",category:Category::Filesystem,description:"Preview 1–20 UTF-8 file replacements/creations with expected SHA256 for existing files. Saves a conversation-bound patch ID for review; changes no workspace files. Total new content <=512,000 bytes.",arguments:"operations: [{path, content, expected_sha256?}]",schema:r#"{"type":"object","properties":{"operations":{"type":"array","minItems":1,"maxItems":20,"items":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":["string","null"]}},"required":["path","content"],"additionalProperties":false}}},"required":["operations"],"additionalProperties":false}"#,writes:false },
    ToolSpec { name:"apply_patch",category:Category::Filesystem,description:"Apply a saved patch after explicit approval of all paths and differences. Rechecks hashes/policy. Files commit separately; partial/uncertain states must be inspected, never blindly retried.",arguments:"patch_id",schema:r#"{"type":"object","properties":{"patch_id":{"type":"string"}},"required":["patch_id"],"additionalProperties":false}"#,writes:true },
    ToolSpec { name:"patch_status",category:Category::Filesystem,description:"Inspect bounded per-file patch and recovery receipts for this conversation/workspace, without exposing stored file contents.",arguments:"patch_id",schema:r#"{"type":"object","properties":{"patch_id":{"type":"string"}},"required":["patch_id"],"additionalProperties":false}"#,writes:false },
    ToolSpec { name:"restore_patch",category:Category::Filesystem,description:"Review and reverse committed patch steps, refusing later file changes. Requires a fresh approval; new files go to recoverable trash. Uncertain steps require individual inspection and restore_change.",arguments:"patch_id",schema:r#"{"type":"object","properties":{"patch_id":{"type":"string"}},"required":["patch_id"],"additionalProperties":false}"#,writes:true },
    ToolSpec { name:"list_jobs",category:Category::Development,description:"List persistent command jobs belonging to this conversation and workspace, including interrupted outcomes. Does not resume or rerun commands.",arguments:"(none)",schema:r#"{"type":"object","properties":{},"additionalProperties":false}"#,writes:false },
    ToolSpec { name:"job_status",category:Category::Development,description:"Read a command job's status and terminal result. A queued or running job is not a successful command result.",arguments:"job_id",schema:r#"{"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"],"additionalProperties":false}"#,writes:false },
    ToolSpec { name:"read_job_output",category:Category::Development,description:"Read at most eight bounded output chunks after a cursor. Reuse next_cursor; poll sparingly and never repeat run_command to poll.",arguments:"job_id, after? (cursor)",schema:r#"{"type":"object","properties":{"job_id":{"type":"string"},"after":{"type":"integer","minimum":0}},"required":["job_id"],"additionalProperties":false}"#,writes:false },
    ToolSpec { name:"cancel_job",category:Category::Development,description:"Request cancellation of a job owned by this conversation. Works in read-only mode; check status for acknowledgement. Does not undo changes already made.",arguments:"job_id",schema:r#"{"type":"object","properties":{"job_id":{"type":"string"}},"required":["job_id"],"additionalProperties":false}"#,writes:true },
    ToolSpec { name:"inspect_processes",category:Category::Development,description:"List bridge-owned processes for this conversation/workspace without exposing command arguments, environments or unrelated processes.",arguments:"(none)",schema:r#"{"type":"object","properties":{},"additionalProperties":false}"#,writes:false },
    ToolSpec { name: "workspace_info", category: Category::Filesystem, description: "Inspect the current root, profile, policy, available tools, limits and granted command IDs; no secret configuration values.", arguments: "(none)", schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#, writes: false },
    ToolSpec { name: "system_info", category: Category::Development, description: "Inspect OS, architecture, host/protocol versions and workspace free disk. OS permission grants are unknown until actually exercised.", arguments: "(none)", schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#, writes: false },
    ToolSpec {
        name: "list_artifacts", category: Category::Filesystem,
        description: "List browser downloads explicitly shared with this session and workspace. Returns metadata only; never browses Downloads or discovers website files automatically.",
        arguments: "(none)", schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "import_artifact", category: Category::Filesystem,
        description: "Copy one explicitly shared browser download (up to 64 MiB) to a new file in an existing workspace directory. Always requires approval. Verifies SHA256, preserves original, refuses overwrites. No extraction or execution; binary recovery is not supported.",
        arguments: "artifact_id, path",
        schema: r#"{"type":"object","properties":{"artifact_id":{"type":"string"},"path":{"type":"string"}},"required":["artifact_id","path"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {name:"list_integrations",category:Category::Development,description:"List user-enabled MCP servers and granted tools, 32 per page. Supply server_id and tool for one complete granted schema. Does not start processes.",arguments:"server_id?, tool?, offset?",schema:r#"{"type":"object","properties":{"server_id":{"type":"string"},"tool":{"type":"string"},"offset":{"type":"integer","minimum":0}},"additionalProperties":false}"#,writes:false},
    ToolSpec {name:"call_integration_tool",category:Category::Development,description:"Invoke a granted tool on a user-configured native MCP server. Requires approval every time; external programs retain OS account permissions. Never automatically retry uncertain results.",arguments:"server_id, tool, arguments",schema:r#"{"type":"object","properties":{"server_id":{"type":"string"},"tool":{"type":"string"},"arguments":{"type":"object"}},"required":["server_id","tool","arguments"],"additionalProperties":false}"#,writes:true},
    ToolSpec {
        name: "preview_document", category: Category::Filesystem,
        description: "Preview a UTF-8 replacement and obtain its expected hash without writing.",
        arguments: "path, content",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "list_changes", category: Category::Filesystem,
        description: "List recoverable changes in the active workspace; private backup bytes are never exposed.",
        arguments: "limit? (1-100)",
        schema: r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100}},"required":[],"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "restore_change", category: Category::Filesystem,
        description: "Restore a change by ID after approval; refuses to overwrite subsequent edits.",
        arguments: "change_id",
        schema: r#"{"type":"object","properties":{"change_id":{"type":"string"}},"required":["change_id"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {
        name: "delete_file", category: Category::Filesystem,
        description: "Move one regular file up to 64 MiB to recoverable workspace trash after approval; requires its current hash. Files over 1 MiB use bounded private snapshots.",
        arguments: "path, expected_sha256",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","expected_sha256"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {
        name: "append_document", category: Category::Filesystem,
        description: "Append UTF-8 text atomically with a recoverable backup and stale-version protection.",
        arguments: "path, content, expected_sha256",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","content","expected_sha256"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {
        name: "copy_file", category: Category::Filesystem,
        description: "Copy a regular file up to 1 MiB within the workspace; refuses overwriting.",
        arguments: "source, destination",
        schema: r#"{"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"required":["source","destination"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {
        name: "read_multiple_files", category: Category::Filesystem,
        description: "Read up to 20 UTF-8 files with a shared byte budget and explicit continuation offsets.",
        arguments: "paths, max_bytes? (128-24000)",
        schema: r#"{"type":"object","properties":{"paths":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":20},"max_bytes":{"type":"integer","minimum":128,"maximum":24000}},"required":["paths"],"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "glob_files", category: Category::Filesystem,
        description: "Find workspace paths using a bounded filename glob; protected directories are not traversed.",
        arguments: "pattern, path?, max_depth?, max_results?",
        schema: r#"{"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"max_depth":{"type":"integer","minimum":1,"maximum":12},"max_results":{"type":"integer","minimum":1,"maximum":500}},"required":["pattern"],"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "file_tree", category: Category::Filesystem,
        description: "Show a bounded directory tree with file sizes and depths.",
        arguments: "path?, max_depth?, max_results?",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"max_depth":{"type":"integer","minimum":1,"maximum":12},"max_results":{"type":"integer","minimum":1,"maximum":500}},"required":[],"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "diff_files", category: Category::Filesystem,
        description: "Compare a UTF-8 file against another file or supplied text using a unified diff.",
        arguments: "path, other_path OR content",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"other_path":{"type":"string"},"content":{"type":"string"}},"required":["path"],"additionalProperties":false}"#, writes: false,
    },

    ToolSpec {
        name: "list_allowed_commands", category: Category::Development,
        description: "List this workspace's user-approved command IDs, fixed arguments, optional allowed argument sequences, cwd, output limits and approval rules.",
        arguments: "(none)", schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#, writes: false,
    },
    ToolSpec {
        name: "run_command", category: Category::Development,
        description: "Run a command ID explicitly granted for this workspace. No shell; optional arguments must exactly match an approved sequence. Requires approval unless always allowed. Output is capped. background=true returns a persistent job ID for longer commands, not a completed result.",
        arguments: "command_id, args? (one approved argument sequence), background? (default false)",
        schema: r#"{"type":"object","properties":{"command_id":{"type":"string"},"args":{"type":"array","items":{"type":"string"},"maxItems":32},"background":{"type":"boolean"}},"required":["command_id"],"additionalProperties":false}"#, writes: true,
    },
    ToolSpec {
        name: "list_files",
        category: Category::Filesystem,
        description: "List files and folders at a workspace-relative path. Does not follow symlinks.",
        arguments: "path? (default .), recursive? (default false), max_depth? (1-12)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"},"max_depth":{"type":"integer","minimum":1,"maximum":12}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "read_file",
        category: Category::Filesystem,
        description: "Read a UTF-8 text file with line numbers. Refuses binary files and files outside approved areas.",
        arguments: "path, start_line?, end_line?, start_byte? (continuation), max_bytes? (128-40000)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"start_byte":{"type":"integer","minimum":0},"max_bytes":{"type":"integer","minimum":128,"maximum":40000}},"required":["path"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "search_files",
        category: Category::Filesystem,
        description: "Bounded case-insensitive literal search with file/line/excerpt/SHA256 and continuation. Up to 100 files, 8 MiB, 5000 tree entries, depth 12 and two seconds per call. Protected paths are excluded; tree limits require a narrower folder. Not semantic reference resolution.",
        arguments: "query, path? (default own workspace), max_results? (1-100), cursor? (pass next_cursor unchanged)",
        schema: r#"{"type":"object","properties":{"query":{"type":"string","minLength":1},"path":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"sha256":{"type":["string","null"]}},"required":["path","line","sha256"],"additionalProperties":false}},"required":["query"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "write_document",
        category: Category::Filesystem,
        description: "Create or replace a UTF-8 file atomically. General/code profiles accept source code and text files; research permits Markdown/text/JSON/CSV/SVG. Existing files are checkpointed and policy-checked.",
        arguments: "path, content, create_only?, expected_sha256 (required when replacing)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"create_only":{"type":"boolean"},"expected_sha256":{"type":"string"}},"required":["path","content"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "edit_document",
        category: Category::Filesystem,
        description: "Replace an exact text fragment in a document. The fragment must occur exactly once unless replace_all is true. Checkpointed and atomic.",
        arguments: "path, old_text, new_text, expected_sha256, replace_all? (default false)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string","minLength":1},"new_text":{"type":"string"},"replace_all":{"type":"boolean"},"expected_sha256":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "create_folder",
        category: Category::Filesystem,
        description: "Create a folder and any missing parents inside an allowed workspace area.",
        arguments: "path",
        schema: PATH,
        writes: true,
    },
    ToolSpec {
        name: "move_file",
        category: Category::Filesystem,
        description: "Move or rename a file inside the workspace without overwriting the destination. Both sides must be writable.",
        arguments: "source, destination",
        schema: r#"{"type":"object","properties":{"source":{"type":"string"},"destination":{"type":"string"}},"required":["source","destination"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "inspect_file",
        category: Category::Filesystem,
        description: "Inspect a file's type, size, SHA-256, text/binary classification, and a short preview.",
        arguments: "path",
        schema: PATH,
        writes: false,
    },
    ToolSpec {
        name: "inspect_file_metadata",
        category: Category::Filesystem,
        description: "Read filesystem metadata (size, timestamps, mode, file type, SHA-256) without reading whole contents.",
        arguments: "path",
        schema: PATH,
        writes: false,
    },
    ToolSpec {
        name: "inspect_dataset",
        category: Category::Research,
        description: "Inspect a CSV, TSV, JSON, or JSON Lines dataset: shape, columns, inferred types, missing counts, and a sample. Read-only.",
        arguments: "path, sample_rows? (0-50), delimiter? (single character)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"sample_rows":{"type":"integer","minimum":0,"maximum":50},"delimiter":{"type":"string","minLength":1,"maxLength":1}},"required":["path"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "calculate_statistics",
        category: Category::Research,
        description: "Calculate descriptive statistics for numeric columns of a CSV/TSV or JSON/JSONL dataset. Reports counts, missing values, mean, median, min, max, and population standard deviation.",
        arguments: "path, columns? (default all numeric columns)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"columns":{"type":"array","items":{"type":"string"},"maxItems":50}},"required":["path"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "generate_visualization",
        category: Category::Research,
        description: "Create a real SVG line, bar, or scatter chart from a CSV/TSV dataset. The SVG is written atomically and its path and SHA-256 are returned.",
        arguments: "path, x_column, y_column, chart_type (line|bar|scatter), output_path (.svg), title?",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"x_column":{"type":"string"},"y_column":{"type":"string"},"chart_type":{"type":"string","enum":["line","bar","scatter"]},"output_path":{"type":"string"},"title":{"type":"string"}},"required":["path","x_column","y_column","chart_type","output_path"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "create_research_report",
        category: Category::Research,
        description: "Create a Markdown research report with author, timestamp, status, agent-supplied sections, and evidence references. The bridge does not invent findings or conclusions.",
        arguments: "path (.md), title, abstract?, sections? [{heading, content}], evidence? [string]",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"title":{"type":"string"},"abstract":{"type":"string"},"sections":{"type":"array","items":{"type":"object","properties":{"heading":{"type":"string"},"content":{"type":"string"}},"required":["heading","content"],"additionalProperties":false}},"evidence":{"type":"array","items":{"type":"string"}}},"required":["path","title"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "inspect_research_manifest",
        category: Category::Research,
        description: "Read a dataset manifest (CSV or Markdown). Defaults to SHARED/datasets/DATA_MANIFEST.csv, then PROJECT/DATASET_MANIFEST.md.",
        arguments: "path?",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "calculate_sha256",
        category: Category::Research,
        description: "Stream a file and return its SHA-256 hash and size without modifying it.",
        arguments: "path",
        schema: PATH,
        writes: false,
    },
    ToolSpec {
        name: "verify_file_integrity",
        category: Category::Research,
        description: "Verify files against a CSV manifest with path and sha256 columns. Reports matches, mismatches, missing files, and malformed entries. Never repairs or writes files.",
        arguments: "manifest_path",
        schema: r#"{"type":"object","properties":{"manifest_path":{"type":"string"}},"required":["manifest_path"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "list_agents",
        category: Category::Collaboration,
        description: "List the supported agent identities, their workspace paths, and counts of published findings and proposals.",
        arguments: "(none)",
        schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "inspect_agent_workspace",
        category: Category::Collaboration,
        description: "Inspect another agent's workspace structure and README without modifying it. Content access stays limited to that agent's findings.",
        arguments: "agent (CHATGPT|CLAUDE|DEEPSEEK|GEMINI|GROK|QWEN|KIMI)",
        schema: r#"{"type":"object","properties":{"agent":{"type":"string","enum":["CHATGPT","CLAUDE","DEEPSEEK","GEMINI","GROK","QWEN","KIMI"]}},"required":["agent"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "read_agent_findings",
        category: Category::Collaboration,
        description: "Read the published Markdown findings of another agent. Notes, experiments, logs, and drafts are not exposed.",
        arguments: "agent, limit? (1-25 files)",
        schema: r#"{"type":"object","properties":{"agent":{"type":"string","enum":["CHATGPT","CLAUDE","DEEPSEEK","GEMINI","GROK","QWEN","KIMI"]},"limit":{"type":"integer","minimum":1,"maximum":25}},"required":["agent"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "submit_research_proposal",
        category: Category::Collaboration,
        description: "Submit a collaboration proposal with attribution and timestamp under COLLABORATION/proposals/.",
        arguments: "title, body, related_paths? [string]",
        schema: r#"{"type":"object","properties":{"title":{"type":"string"},"body":{"type":"string"},"related_paths":{"type":"array","items":{"type":"string"}}},"required":["title","body"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "submit_peer_review",
        category: Category::Collaboration,
        description: "Review another agent's published finding. Records verdict, evidence-based body, reviewer attribution, and timestamp under COLLABORATION/peer_reviews/.",
        arguments: "target_agent, target_path, verdict (supports|challenges|mixed|insufficient_evidence), body",
        schema: r#"{"type":"object","properties":{"target_agent":{"type":"string","enum":["CHATGPT","CLAUDE","DEEPSEEK","GEMINI","GROK","QWEN","KIMI"]},"target_path":{"type":"string"},"verdict":{"type":"string","enum":["supports","challenges","mixed","insufficient_evidence"]},"body":{"type":"string"}},"required":["target_agent","target_path","verdict","body"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "record_disagreement",
        category: Category::Collaboration,
        description: "Record a scientific disagreement without erasing either position under COLLABORATION/disagreements/.",
        arguments: "topic, positions, analysis",
        schema: r#"{"type":"object","properties":{"topic":{"type":"string"},"positions":{"type":"string"},"analysis":{"type":"string"}},"required":["topic","positions","analysis"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "git_status",
        category: Category::Development,
        description: "Inspect a bounded ordinary local Git repository with protected paths omitted. No parent discovery, linked worktrees, config includes, alternates, external filters, hooks or network. General/code default to workspace root.",
        arguments: "path? (repository path)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "git_diff",
        category: Category::Development,
        description: "Inspect staged or unstaged differences for permitted files in an ordinary local repository. Optional files are workspace-relative. External filters/textconv disabled, private paths omitted, oversized diffs refused rather than silently truncated.",
        arguments: "path?, staged? (default false), files? (1–100 workspace-relative paths)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"staged":{"type":"boolean"},"files":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":100}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "inspect_repository",
        category: Category::Development,
        description: "Inspect a workspace repository: Git root, branch, top-level entries, README excerpt, and working-tree state.",
        arguments: "path?",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "preview_git_commit",
        category: Category::Development,
        description: "Preview a new bridge/ branch commit from 1–20 selected existing UTF-8 working files. Saves an exact diff, hashes and base HEAD; does not change Git. No deletes, binary files, filters, hooks or push.",
        arguments: "path?, files [workspace-relative path], branch (new bridge/ name), message",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"files":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":20},"branch":{"type":"string","maxLength":120},"message":{"type":"string","minLength":1,"maxLength":2000}},"required":["files","branch","message"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "git_log",
        category: Category::Development,
        description: "Read bounded local commit IDs, timestamps and subjects. Continue with returned base_commit as commit and next_offset. Does not fetch or return file contents.",
        arguments: "path?, commit? (full object ID; defaults to HEAD), limit? (1–20), offset? (0–10000)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"commit":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20},"offset":{"type":"integer","minimum":0,"maximum":10000}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "git_show",
        category: Category::Development,
        description: "Read one permitted historical UTF-8 file at a full commit ID, up to 48,000 bytes. Current private-path policy applies to history too. No arbitrary revision expressions or binary content.",
        arguments: "path? (repository), commit (full object ID), file (workspace-relative)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"commit":{"type":"string"},"file":{"type":"string"}},"required":["commit","file"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "apply_git_commit",
        category: Category::Development,
        description: "After mandatory approval of the saved diff, create the reviewed commit on a new branch using a private index. Recheck files, base HEAD, policy and expiry. Preserve user's index, HEAD and working files. Never repeat uncertain publication; inspect git_commit_status.",
        arguments: "review_id",
        schema: r#"{"type":"object","properties":{"review_id":{"type":"string"}},"required":["review_id"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "git_commit_status",
        category: Category::Development,
        description: "Read a conversation-owned commit receipt after reconnect/interruption. Reconcile only by inspecting the intended ref; never rerun writes.",
        arguments: "review_id",
        schema: r#"{"type":"object","properties":{"review_id":{"type":"string"}},"required":["review_id"],"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "run_approved_script",
        category: Category::Development,
        description: "Run a script listed by name in the user's local configuration, with fixed arguments and no shell. Requires approval unless the tool has no host available.",
        arguments: "name",
        schema: r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "inspect_test_results",
        category: Category::Development,
        description: "Read a test-result file (text, JSON, or JUnit XML) from an allowed workspace path and summarize a bounded excerpt.",
        arguments: "path",
        schema: PATH,
        writes: false,
    },
    ToolSpec {
        name: "desktop_screenshot",
        category: Category::Desktop,
        description: "Capture the macOS screen to a PNG in your own workspace. Requires the user's macOS Screen Recording permission.",
        arguments: "path? (default experiments/screenshots/<timestamp>.png), region? \"x,y,w,h\"",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"region":{"type":"string","pattern":"^-?[0-9]+,-?[0-9]+,[0-9]+,[0-9]+$"}},"additionalProperties":false}"#,
        writes: true,
    },
    ToolSpec {
        name: "active_application",
        category: Category::Desktop,
        description: "Return the name of the frontmost macOS application. Requires Accessibility permission in the browser host process.",
        arguments: "(none)",
        schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "list_open_applications",
        category: Category::Desktop,
        description: "List visible macOS applications. Requires Accessibility permission.",
        arguments: "(none)",
        schema: r#"{"type":"object","properties":{},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "inspect_accessible_window",
        category: Category::Desktop,
        description: "Inspect accessible windows of the frontmost or named macOS application using System Events. Read-only; requires Accessibility permission.",
        arguments: "application?",
        schema: r#"{"type":"object","properties":{"application":{"type":"string"}},"additionalProperties":false}"#,
        writes: false,
    },
];

pub fn find(name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|tool| tool.name == name)
}

pub fn by_category(category: Category) -> impl Iterator<Item = &'static ToolSpec> {
    TOOLS.iter().filter(move |tool| tool.category == category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn catalog_is_unique_and_schemas_are_objects() {
        let mut seen = BTreeSet::new();
        for tool in TOOLS {
            assert!(seen.insert(tool.name), "duplicate {}", tool.name);
            let schema: serde_json::Value = serde_json::from_str(tool.schema).unwrap();
            assert_eq!(schema["type"], "object", "{}", tool.name);
        }
    }

    #[test]
    fn all_required_tools_are_present() {
        let required = [
            "list_files",
            "read_file",
            "search_files",
            "write_document",
            "edit_document",
            "create_folder",
            "move_file",
            "inspect_file",
            "inspect_file_metadata",
            "inspect_dataset",
            "calculate_statistics",
            "generate_visualization",
            "create_research_report",
            "inspect_research_manifest",
            "calculate_sha256",
            "verify_file_integrity",
            "list_agents",
            "inspect_agent_workspace",
            "read_agent_findings",
            "submit_research_proposal",
            "submit_peer_review",
            "record_disagreement",
            "git_status",
            "git_diff",
            "inspect_repository",
            "run_approved_script",
            "inspect_test_results",
            "desktop_screenshot",
            "active_application",
            "list_open_applications",
            "inspect_accessible_window",
        ];
        for name in required {
            assert!(find(name).is_some(), "missing {name}");
        }
    }
}
