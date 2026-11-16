use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::failure::ToolFailure;
use crate::fsops;
use crate::sandbox::{Access, Decision, Existence};
use crate::tools::{parse_args, require_text_extension, ToolContext, ToolOutcome, MAX_READ_BYTES};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    max_depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
    #[serde(default)]
    start_byte: Option<usize>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
    #[serde(default)]
    create_only: bool,
    #[serde(default)]
    expected_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    expected_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    source: String,
    destination: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}

pub async fn list_files(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: ListArgs = parse_args(args)?;
    let raw = args.path.unwrap_or_else(|| ".".into());
    let path = ctx.resolve_existing(&raw)?;
    if !path.is_dir {
        return Err(ToolFailure::InvalidArguments(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    ctx.check_read("list_files", &path)?;
    let depth = args
        .max_depth
        .unwrap_or(if args.recursive { 6 } else { 1 })
        .clamp(1, 12);
    let root = ctx.workspace_root();
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in WalkDir::new(&path.absolute)
        .follow_links(false)
        .max_depth(depth)
        .sort_by_file_name()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if entry.path() == path.absolute {
            continue;
        }
        if entry.file_type().is_symlink() {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let resolved = match ctx
            .app
            .sandbox()
            .resolve(&rel.to_string_lossy(), Existence::Any)
        {
            Ok(resolved) => resolved,
            Err(_) => continue,
        };
        match ctx.app.sandbox().decide(ctx.agent, Access::Read, &resolved) {
            Decision::Allow | Decision::Approval(_) => {}
            Decision::Deny(_) => continue,
        }
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let suffix = if meta.is_dir() { "/" } else { "" };
        entries.push(format!(
            "{}{}  {}{}",
            rel.to_string_lossy(),
            suffix,
            if meta.is_dir() {
                "dir".to_string()
            } else {
                format_bytes(meta.len())
            },
            if meta.is_dir() {
                String::new()
            } else {
                format!("  {}", modified(&meta))
            }
        ));
        if entries.len() >= 500 {
            truncated = true;
            break;
        }
    }
    let mut output = format!("{} entries under {}\n\n", entries.len(), path.display());
    output.push_str(&entries.join("\n"));
    if truncated {
        output.push_str("\n\n[bridge: limited to 500 entries; narrow the path or depth]");
    }
    if entries.is_empty() {
        output = format!("{} is empty.", path.display());
    }
    Ok(ToolOutcome::read(
        format!("listed {} entries", entries.len()),
        output,
        Some(path.display()),
    ))
}

pub async fn read_file(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: ReadArgs = parse_args(args)?;
    let (path, text) = ctx.read_text("read_file", &args.path, MAX_READ_BYTES)?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len();
    let start_line = args.start_line.unwrap_or(1).max(1);
    if args.start_byte.is_some() && (args.start_line.is_some() || args.end_line.is_some()) {
        return Err(ToolFailure::InvalidArguments(
            "use byte continuation or line range, not both".into(),
        ));
    }
    if start_line > total.max(1) {
        return Err(ToolFailure::InvalidArguments(
            "start_line is beyond the file".into(),
        ));
    }
    let start = args
        .start_byte
        .unwrap_or_else(|| lines.iter().take(start_line - 1).map(|s| s.len()).sum());
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(ToolFailure::InvalidArguments(
            "start_byte must be a UTF-8 boundary within the file".into(),
        ));
    }
    let range_end = args
        .end_line
        .map(|n| lines.iter().take(n).map(|s| s.len()).sum())
        .unwrap_or(text.len());
    if range_end < start {
        return Err(ToolFailure::InvalidArguments(
            "end_line precedes start_line".into(),
        ));
    }
    let budget = args.max_bytes.unwrap_or(24_000).clamp(128, 40_000);
    let mut end = range_end.min(start.saturating_add(budget));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let page = &text[start..end];
    let line_cap: usize = page.split_inclusive('\n').take(4000).map(str::len).sum();
    end = start + line_cap;
    let first = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
    let mut output = format!(
        "{} ({} lines, {} bytes)\n\n",
        path.display(),
        total,
        text.len()
    );
    let mut emitted_end = start;
    for (index, line) in text[start..end].split_inclusive('\n').enumerate() {
        let rendered = format!(
            "{:>6}\t{}\n",
            first + index,
            line.trim_end_matches(['\r', '\n'])
        );
        if output.len() + rendered.len() > 55_000 {
            break;
        }
        output.push_str(&rendered);
        emitted_end += line.len();
    }
    end = emitted_end;
    let next_byte = (end < text.len()).then_some(end);
    if let Some(offset) = next_byte {
        output.push_str(&format!(
            "\n[bridge: more content; continue with start_byte={offset}]\n"
        ));
    }
    let data = serde_json::json!({"path":path.display(),"lines":total,"sha256":fsops::sha256_bytes(text.as_bytes()),"range":[first,first+text[start..end].lines().count().saturating_sub(1)],"next_byte":next_byte,"truncated":end<range_end});
    Ok(ToolOutcome::read(
        format!("read {} bytes from {}", end - start, path.display()),
        output,
        Some(path.display()),
    )
    .with_data(data))
}

pub async fn search_files(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    super::code_navigation::search(ctx,args).await
}

pub async fn write_document(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: WriteArgs = parse_args(args)?;
    if args.content.len() > 800_000 {
        return Err(ToolFailure::InvalidArguments(
            "content exceeds 800,000 bytes".into(),
        ));
    }
    let path = ctx.resolve_any(&args.path)?;
    if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
        require_text_extension(
            &path.absolute,
            &["md", "markdown", "txt", "json", "csv", "tsv", "svg"],
        )?;
    }
    if args.create_only && path.exists {
        return Err(ToolFailure::Conflict("destination already exists".into()));
    }
    let (path, written, change) = super::recovery::mutate(
        ctx,
        "write_document",
        &args.path,
        args.expected_sha256.as_deref(),
        Some(args.content.as_bytes()),
        false,
    )
    .await?;
    let checkpoint = written
        .checkpoint
        .as_ref()
        .map(|p| fsops::display_relative(ctx.workspace_root(), p));
    let output = format!(
        "Wrote {} ({} bytes, sha256 {}).{}",
        path.display(),
        written.bytes,
        written.sha256,
        checkpoint
            .as_ref()
            .map(|p| format!(" Previous version checkpointed at {p}."))
            .unwrap_or_default()
    );
    Ok(ToolOutcome::write(
        format!("wrote {}", path.display()),
        output,
        Some(path.display()),
    )
    .with_data(serde_json::json!({
        "path": path.display(),
        "sha256": written.sha256,
        "bytes": written.bytes,
        "checkpoint": checkpoint,
        "change_id": change.id,
    })))
}

pub async fn edit_document(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: EditArgs = parse_args(args)?;
    if args.old_text.len() > 500_000 || args.new_text.len() > 800_000 {
        return Err(ToolFailure::InvalidArguments(
            "edit text is too large".into(),
        ));
    }
    let path = ctx.resolve_existing(&args.path)?;
    if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
        require_text_extension(
            &path.absolute,
            &["md", "markdown", "txt", "json", "csv", "tsv", "svg"],
        )?;
    }
    let (_, original) = ctx.read_text("edit_document", &args.path, MAX_READ_BYTES)?;
    let expected = args.expected_sha256.as_deref().ok_or_else(|| {
        ToolFailure::Conflict("expected_sha256 is required; read the file before editing".into())
    })?;
    if fsops::sha256_bytes(original.as_bytes()) != expected {
        return Err(ToolFailure::Conflict("file changed; read it again".into()));
    }
    let matches = original.matches(&args.old_text).count();
    if args.old_text.is_empty() || matches == 0 || (matches > 1 && !args.replace_all) {
        return Err(ToolFailure::InvalidArguments(format!(
            "old_text has {matches} matches; provide a unique nonempty fragment or replace_all"
        )));
    }
    let updated = if args.replace_all {
        original.replace(&args.old_text, &args.new_text)
    } else {
        original.replacen(&args.old_text, &args.new_text, 1)
    };
    let (path, written, change) = super::recovery::mutate(
        ctx,
        "edit_document",
        &args.path,
        Some(expected),
        Some(updated.as_bytes()),
        false,
    )
    .await?;
    let checkpoint = written
        .checkpoint
        .as_ref()
        .map(|p| fsops::display_relative(ctx.workspace_root(), p));
    Ok(ToolOutcome::write(
        format!("edited {}", path.display()),
        format!(
            "Edited {} atomically. New sha256 {}.{}",
            path.display(),
            written.sha256,
            checkpoint
                .as_ref()
                .map(|p| format!(" Previous version checkpointed at {p}."))
                .unwrap_or_default()
        ),
        Some(path.display()),
    )
    .with_data(serde_json::json!({
        "path": path.display(),
        "sha256": written.sha256,
        "checkpoint": checkpoint,
        "change_id": change.id,
    })))
}

pub async fn create_folder(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: FolderArgs = parse_args(args)?;
    let path = ctx.create_folder("create_folder", &args.path).await?;
    Ok(ToolOutcome::write(
        format!("created folder {}", path.display()),
        format!("Folder exists: {}", path.display()),
        Some(path.display()),
    ))
}

pub async fn move_file(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: MoveArgs = parse_args(args)?;
    let source = ctx.resolve_existing(&args.source)?;
    let destination = ctx.resolve_any(&args.destination)?;
    if source.is_dir {
        return Err(ToolFailure::Unsupported(
            "move_file only moves files, not folders".into(),
        ));
    }
    if destination.exists {
        return Err(ToolFailure::InvalidArguments(format!(
            "destination already exists: {}",
            destination.display()
        )));
    }
    ctx.check_read("move_file", &source)?;
    if !fs::symlink_metadata(&source.absolute)
        .map_err(|err| ToolFailure::from_io("inspect move source", err))?
        .is_file()
    {
        return Err(ToolFailure::Unsupported("only regular files can be moved".into()));
    }
    let before = fsops::sha256_file(&source.absolute)
        .map_err(|err| ToolFailure::Io(err.to_string()))?;
    let destination_approved = ctx.authorize_write("move_file", &destination).await?;
    let own_source = source
        .relative
        .to_string_lossy()
        .starts_with(&format!("AGENTS/{}/", ctx.agent.dir()));
    let source_access = if own_source
        && destination
            .relative
            .to_string_lossy()
            .starts_with(&format!("AGENTS/{}/", ctx.agent.dir()))
    {
        crate::sandbox::Access::Write
    } else {
        crate::sandbox::Access::Remove
    };
    let source_approved = if source_access == crate::sandbox::Access::Write {
        ctx.authorize_write("move_file", &source).await?
    } else {
        ctx.authorize_remove("move_file", &source).await?
    };
    let a = source.relative.to_string_lossy().to_string();
    let b = destination.relative.to_string_lossy().to_string();
    let (first, second) = if a <= b {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    };
    let _lock_one = fsops::PathLock::acquire(&ctx.app.workspace.lock_dir, &first)
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    let _lock_two = fsops::PathLock::acquire(&ctx.app.workspace.lock_dir, &second)
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    let current_source = ctx.resolve_existing(&args.source)?;
    let current_destination = ctx.resolve_any(&args.destination)?;
    if current_source.absolute != source.absolute
        || current_destination.absolute != destination.absolute
        || current_destination.exists
    {
        return Err(ToolFailure::Conflict("move target changed during approval".into()));
    }
    ctx.check_read("move_file", &current_source)?;
    ctx.recheck_access(source_access, &current_source, source_approved)?;
    ctx.recheck_access(crate::sandbox::Access::Write, &current_destination, destination_approved)?;
    if !fs::symlink_metadata(&current_source.absolute)
        .map_err(|err| ToolFailure::from_io("inspect move source", err))?
        .is_file()
        || fsops::sha256_file(&current_source.absolute)
            .map_err(|err| ToolFailure::Io(err.to_string()))? != before
    {
        return Err(ToolFailure::Conflict("move source changed during approval".into()));
    }
    let Some(parent) = destination.absolute.parent() else {
        return Err(ToolFailure::InvalidArguments("invalid destination".into()));
    };
    fs::create_dir_all(parent)
        .map_err(|err| ToolFailure::Io(format!("create destination folder: {err}")))?;
    fsops::rename_no_replace(&source.absolute, &destination.absolute).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            ToolFailure::Conflict("move destination appeared; no files were replaced".into())
        } else {
            ToolFailure::from_io("move file", err)
        }
    })?;
    let (hash, bytes) = before;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.move_file",
            Some(format!("{} -> {}", source.display(), destination.display())),
            "allowed",
            &serde_json::json!({"sha256": hash, "bytes": bytes}).to_string(),
        )
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    Ok(ToolOutcome::write(
        format!("moved file to {}", destination.display()),
        format!(
            "Moved {} to {} ({} bytes, sha256 {}).",
            source.display(),
            destination.display(),
            bytes,
            hash
        ),
        Some(destination.display()),
    ))
}

pub async fn inspect_file(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    let path = ctx.resolve_existing(&args.path)?;
    ctx.check_read("inspect_file", &path)?;
    if path.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "inspect_file expects a file".into(),
        ));
    }
    let meta =
        fs::metadata(&path.absolute).map_err(|err| ToolFailure::from_io("file metadata", err))?;
    let mut file =
        fs::File::open(&path.absolute).map_err(|err| ToolFailure::from_io("open file", err))?;
    use std::io::Read;
    let mut head = vec![0u8; 8192.min(meta.len() as usize)];
    let read = file
        .read(&mut head)
        .map_err(|err| ToolFailure::from_io("read file head", err))?;
    head.truncate(read);
    let binary = head.contains(&0) || std::str::from_utf8(&head).is_err();
    let preview = if binary {
        format!(
            "binary preview ({} bytes): {}",
            head.len(),
            hex::encode(head.iter().take(64).copied().collect::<Vec<u8>>())
        )
    } else {
        String::from_utf8_lossy(&head)
            .chars()
            .take(1200)
            .collect::<String>()
    };
    let (sha256, _) =
        fsops::sha256_file(&path.absolute).map_err(|err| ToolFailure::Internal(err.to_string()))?;
    let kind = kind_for(&path.absolute);
    let output = format!(
        "Path: {}\nKind: {}\nSize: {} bytes\nModified: {}\nSHA-256: {}\nBinary: {}\n\nPreview:\n{}",
        path.display(),
        kind,
        meta.len(),
        modified(&meta),
        sha256,
        binary,
        preview
    );
    Ok(ToolOutcome::read(
        format!("inspected {} ({})", path.display(), kind),
        output,
        Some(path.display()),
    )
    .with_data(serde_json::json!({
        "path": path.display(),
        "kind": kind,
        "bytes": meta.len(),
        "sha256": sha256,
        "binary": binary
    })))
}

pub async fn inspect_file_metadata(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    let path = ctx.resolve_existing(&args.path)?;
    ctx.check_read("inspect_file_metadata", &path)?;
    let meta = fs::symlink_metadata(&path.absolute)
        .map_err(|err| ToolFailure::from_io("file metadata", err))?;
    let sha256 = if meta.is_file() {
        Some(
            fsops::sha256_file(&path.absolute)
                .map_err(|err| ToolFailure::Internal(err.to_string()))?
                .0,
        )
    } else {
        None
    };
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        format!("{:o}", meta.permissions().mode() & 0o777)
    };
    #[cfg(not(unix))]
    let mode = format!("{}", meta.permissions().readonly());
    let metadata = serde_json::json!({
        "path": path.display(),
        "kind": kind_for(&path.absolute),
        "is_file": meta.is_file(),
        "is_dir": meta.is_dir(),
        "is_symlink": meta.file_type().is_symlink(),
        "bytes": meta.len(),
        "modified": modified(&meta),
        "accessed": meta.accessed().ok().map(fmt_system_time),
        "created": meta.created().ok().map(fmt_system_time),
        "mode": mode,
        "sha256": sha256,
    });
    let output = format!(
        "Path: {}\nKind: {}\nFile: {}\nFolder: {}\nSymlink: {}\nBytes: {}\nModified: {}\nMode: {}\nSHA-256: {}",
        path.display(),
        kind_for(&path.absolute),
        meta.is_file(),
        meta.is_dir(),
        meta.file_type().is_symlink(),
        meta.len(),
        modified(&meta),
        mode,
        sha256.unwrap_or_else(|| "n/a".into())
    );
    Ok(ToolOutcome::read(
        format!("metadata for {}", path.display()),
        output,
        Some(path.display()),
    )
    .with_data(metadata))
}

fn kind_for(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => "Markdown document".into(),
        "txt" => "plain text".into(),
        "csv" => "CSV dataset".into(),
        "tsv" => "TSV dataset".into(),
        "json" => "JSON".into(),
        "jsonl" => "JSON Lines".into(),
        "svg" => "SVG figure".into(),
        "png" => "PNG image".into(),
        "jpg" | "jpeg" => "JPEG image".into(),
        "pdf" => "PDF".into(),
        "py" => "Python source".into(),
        "rs" => "Rust source".into(),
        "toml" => "TOML".into(),
        "" => "file".into(),
        other => format!("{other} file"),
    }
}

fn modified(meta: &fs::Metadata) -> String {
    meta.modified()
        .ok()
        .map(fmt_system_time)
        .unwrap_or_else(|| "unknown".into())
}

fn fmt_system_time(time: std::time::SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let seconds = duration.as_secs() as i64;
            chrono::DateTime::from_timestamp(seconds, 0)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                .unwrap_or_else(|| "unknown".into())
        }
        Err(_) => "pre-epoch".into(),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}
