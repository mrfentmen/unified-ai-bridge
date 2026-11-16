use super::{parse_args, recovery, ToolContext, ToolOutcome, MAX_READ_BYTES};
use crate::{
    failure::ToolFailure,
    fsops,
    sandbox::{Access, Decision},
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Append {
    path: String,
    content: String,
    expected_sha256: String,
}
pub async fn append(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Append = parse_args(args)?;
    let (path, mut text) = ctx.read_text("append_document", &args.path, MAX_READ_BYTES)?;
    if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
        super::require_text_extension(
            &path.absolute,
            &["md", "markdown", "txt", "json", "csv", "tsv", "svg"],
        )?;
    }
    if fsops::sha256_bytes(text.as_bytes()) != args.expected_sha256 {
        return Err(ToolFailure::Conflict("file changed; read it again".into()));
    }
    text.push_str(&args.content);
    let (path, result, change) = recovery::mutate(
        ctx,
        "append_document",
        &args.path,
        Some(&args.expected_sha256),
        Some(text.as_bytes()),
        false,
    )
    .await?;
    Ok(ToolOutcome::write(
        "text appended",
        format!(
            "Appended to {}; {} bytes; change {}",
            path.display(),
            result.bytes,
            change.id
        ),
        Some(path.display()),
    )
    .with_data(json!({"sha256":result.sha256,"change_id":change.id})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Copy {
    source: String,
    destination: String,
}
pub async fn copy(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Copy = parse_args(args)?;
    let source = ctx.resolve_existing(&args.source)?;
    ctx.check_read("copy_file", &source)?;
    let bytes = recovery::snapshot(&source)?
        .ok_or_else(|| ToolFailure::NotFound("source disappeared".into()))?;
    let (destination, result, change) = recovery::mutate(
        ctx,
        "copy_file",
        &args.destination,
        None,
        Some(&bytes),
        false,
    )
    .await?;
    Ok(ToolOutcome::write(
        "file copied",
        format!("Copied {} → {}", source.display(), destination.display()),
        Some(destination.display()),
    )
    .with_data(json!({"sha256":result.sha256,"change_id":change.id,"bytes":result.bytes})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    paths: Vec<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}
pub async fn read_multiple(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Batch = parse_args(args)?;
    if args.paths.is_empty() || args.paths.len() > 20 {
        return Err(ToolFailure::InvalidArguments(
            "paths must contain 1–20 files".into(),
        ));
    }
    // Check every path before returning any content; a denied path is a denial,
    // not an indistinguishable missing file.
    for raw in &args.paths {
        let path = ctx.resolve_existing(raw)?;
        ctx.check_read("read_multiple_files", &path)?;
    }
    let mut remaining = args.max_bytes.unwrap_or(24_000).clamp(128, 24_000);
    let mut files = Vec::new();
    for raw in &args.paths {
        let (path, text) = ctx.read_text("read_multiple_files", raw, MAX_READ_BYTES)?;
        let mut boundary = text.len().min(remaining);
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        remaining -= boundary;
        files.push(json!({"path":path.display(),"sha256":fsops::sha256_bytes(text.as_bytes()),"content":&text[..boundary],"truncated":boundary<text.len(),"next_byte":if boundary<text.len(){Some(boundary)}else{None}}));
    }
    // Content appears once, in structured data, to avoid doubling the payload.
    Ok(ToolOutcome::read(
        format!("read {} files", files.len()),
        "File contents and continuation offsets are in data.files.",
        None,
    )
    .with_data(json!({"files":files,"byte_budget_remaining":remaining})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scan {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_depth: Option<usize>,
    #[serde(default)]
    max_results: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Glob {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_depth: Option<usize>,
    #[serde(default)]
    max_results: Option<usize>,
}
fn scan(
    ctx: &ToolContext<'_>,
    tool: &str,
    args: Scan,
    pattern: Option<&str>,
) -> Result<ToolOutcome, ToolFailure> {
    let raw = args.path.unwrap_or_else(|| ".".into());
    let root = ctx.resolve_existing(&raw)?;
    ctx.check_read(tool, &root)?;
    if !root.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "scan path must be a directory".into(),
        ));
    }
    let matcher = pattern
        .map(|p| {
            globset::GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolFailure::InvalidArguments(e.to_string()))
        })
        .transpose()?;
    let depth = args.max_depth.unwrap_or(4).clamp(1, 12);
    let limit = args.max_results.unwrap_or(100).clamp(1, 500);
    let mut entries = Vec::new();
    let mut truncated = false;
    let allowed = |entry: &walkdir::DirEntry| {
        if entry.file_type().is_symlink() {
            return false;
        }
        let Ok(relative) = entry.path().strip_prefix(ctx.workspace_root()) else {
            return false;
        };
        ctx.resolve_existing(&relative.to_string_lossy())
            .is_ok_and(|p| ctx.app.decide(ctx.agent, Access::Read, &p) == Decision::Allow)
    };
    for (scanned, entry) in walkdir::WalkDir::new(&root.absolute)
        .min_depth(1)
        .max_depth(depth)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(allowed)
        .enumerate()
    {
        if scanned >= 10_000 {
            truncated = true;
            break;
        }
        let entry = entry.map_err(|e| ToolFailure::Io(e.to_string()))?;
        if !allowed(&entry) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root.absolute)
            .map_err(|e| ToolFailure::Internal(e.to_string()))?;
        if matcher.as_ref().is_some_and(|m| !m.is_match(relative)) {
            continue;
        }
        if entries.len() >= limit {
            truncated = true;
            break;
        }
        let meta = entry
            .metadata()
            .map_err(|e| ToolFailure::Io(e.to_string()))?;
        entries.push(json!({"path":entry.path().strip_prefix(ctx.workspace_root()).unwrap().to_string_lossy(),"directory":meta.is_dir(),"bytes":if meta.is_file(){Some(meta.len())}else{None},"depth":entry.depth()}));
    }
    let mut output = String::new();
    for entry in &entries {
        output.push_str(&format!(
            "{}{}{}  {}\n",
            "  ".repeat(entry["depth"].as_u64().unwrap_or(1).saturating_sub(1) as usize),
            entry["path"].as_str().unwrap_or_default(),
            if entry["directory"] == true { "/" } else { "" },
            entry["bytes"]
        ));
    }
    if truncated {
        output.push_str("[bridge: results truncated; narrow path, depth, or pattern]\n");
    }
    Ok(ToolOutcome::read(
        format!("{} entries", entries.len()),
        output,
        Some(root.display()),
    )
    .with_data(json!({"entries":entries,"truncated":truncated})))
}
pub async fn tree(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    scan(ctx, "file_tree", parse_args(args)?, None)
}
pub async fn glob(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Glob = parse_args(args)?;
    if args.pattern.len() > 512 {
        return Err(ToolFailure::InvalidArguments(
            "glob pattern too long".into(),
        ));
    }
    scan(
        ctx,
        "glob_files",
        Scan {
            path: args.path,
            max_depth: args.max_depth,
            max_results: args.max_results,
        },
        Some(&args.pattern),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    path: String,
    #[serde(default)]
    other_path: Option<String>,
    #[serde(default)]
    content: Option<String>,
}
pub async fn diff(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Diff = parse_args(args)?;
    let (path, text) = ctx.read_text("diff_files", &args.path, MAX_READ_BYTES)?;
    let other = match (args.other_path, args.content) {
        (Some(p), None) => ctx.read_text("diff_files", &p, MAX_READ_BYTES)?.1,
        (None, Some(s)) if s.len() as u64 <= MAX_READ_BYTES => s,
        _ => {
            return Err(ToolFailure::InvalidArguments(
                "provide exactly one of other_path or content (at most 1 MiB)".into(),
            ))
        }
    };
    Ok(ToolOutcome::read("file comparison",recovery::difference(&path.display(),text.as_bytes(),other.as_bytes()),Some(path.display())).with_data(json!({"sha256":fsops::sha256_bytes(text.as_bytes()),"other_sha256":fsops::sha256_bytes(other.as_bytes())})))
}
