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
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Filesystem,
        Category::Research,
        Category::Collaboration,
        Category::Development,
        Category::Desktop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Filesystem => "FILESYSTEM",
            Category::Research => "RESEARCH",
            Category::Collaboration => "COLLABORATION",
            Category::Development => "DEVELOPMENT",
            Category::Desktop => "DESKTOP",
        }
    }
}

const PATH: &str = r#"{"type":"object","properties":{"path":{"type":"string","description":"Workspace-relative path"}},"required":["path"],"additionalProperties":false}"#;

pub const TOOLS: &[ToolSpec] = &[
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
        description: "Move one regular file to recoverable workspace trash after approval; requires its current hash.",
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
        description: "Search text files for a literal string. Returns file, line, and a short excerpt. Skips binary and huge files.",
        arguments: "query, path? (default own workspace), max_results? (1-100)",
        schema: r#"{"type":"object","properties":{"query":{"type":"string","minLength":1},"path":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false}"#,
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
        description: "Run fixed-argument git status inside a Git repository contained in the workspace. No shell.",
        arguments: "path? (repository path)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}"#,
        writes: false,
    },
    ToolSpec {
        name: "git_diff",
        category: Category::Development,
        description: "Run fixed-argument git diff inside a workspace repository. Returns a bounded diff. No shell.",
        arguments: "path?, staged? (default false)",
        schema: r#"{"type":"object","properties":{"path":{"type":"string"},"staged":{"type":"boolean"}},"additionalProperties":false}"#,
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
