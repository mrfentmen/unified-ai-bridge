//! Recoverable file mutations. Backups live in host-owned SQLite, never in model output.
use super::{parse_args, ToolContext, ToolOutcome, MAX_READ_BYTES};
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::{self, AtomicWrite, PathLock},
    sandbox::{Access, Decision, ResolvedPath},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;

#[derive(Clone, Serialize, Deserialize)]
pub struct Change {
    pub id: String,
    pub workspace_id: String,
    pub created_at: String,
    pub agent: String,
    pub path: String,
    pub operation: String,
    pub before_sha256: Option<String>,
    pub after_sha256: Option<String>,
    pub state: String,
    before_base64: Option<String>,
    pub trash: Option<String>,
}

impl Change {
    pub fn metadata(&self) -> Value {
        json!({"change_id":self.id,"workspace_id":self.workspace_id,"created_at":self.created_at,
            "agent":self.agent,"path":self.path,"operation":self.operation,"before_sha256":self.before_sha256,
            "after_sha256":self.after_sha256,"state":self.state,"trash":self.trash})
    }
}

fn internal(error: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(error.to_string())
}
pub fn snapshot(path: &ResolvedPath) -> Result<Option<Vec<u8>>, ToolFailure> {
    if !path.exists {
        return Ok(None);
    }
    let meta = fs::symlink_metadata(&path.absolute)
        .map_err(|e| ToolFailure::from_io("inspect file", e))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(ToolFailure::PermissionDenied(
            "only regular files are supported".into(),
        ));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(ToolFailure::InvalidArguments(
            "recoverable files are limited to 1 MiB".into(),
        ));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(&path.absolute)
        .map_err(|e| ToolFailure::from_io("open recovery file", e))?
        .take(MAX_READ_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ToolFailure::from_io("read recovery file", e))?;
    if bytes.len() as u64 > MAX_READ_BYTES {
        return Err(ToolFailure::InvalidArguments(
            "file grew beyond 1 MiB".into(),
        ));
    }
    Ok(Some(bytes))
}

pub fn hash(bytes: &Option<Vec<u8>>) -> Option<String> {
    bytes.as_ref().map(|bytes| fsops::sha256_bytes(bytes))
}

pub fn difference(path: &str, before: &[u8], after: &[u8]) -> String {
    match (std::str::from_utf8(before), std::str::from_utf8(after)) {
        (Ok(a), Ok(b)) => super::truncate_output(
            similar::TextDiff::configure()
                .timeout(std::time::Duration::from_millis(250))
                .diff_lines(a, b)
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{path}"), &format!("b/{path}"))
                .to_string(),
        ),
        _ => format!("Binary file: {} → {} bytes", before.len(), after.len()),
    }
}

/// The expected hash is captured before awaiting approval, then checked again
/// under the path lock. A missing expected hash means the destination must not exist.
pub async fn mutate(
    ctx: &ToolContext<'_>,
    tool: &str,
    raw: &str,
    expected: Option<&str>,
    content: Option<&[u8]>,
    force_approval: bool,
) -> Result<(ResolvedPath, AtomicWrite, Change), ToolFailure> {
    if content.is_some_and(|b| b.len() as u64 > MAX_READ_BYTES) {
        return Err(ToolFailure::InvalidArguments(
            "content exceeds 1 MiB".into(),
        ));
    }
    let initial = ctx.resolve_any(raw)?;
    ctx.check_read(tool, &initial)?;
    let access = if content.is_some() {
        Access::Write
    } else {
        Access::Remove
    };
    let decision = ctx.app.current_write_decision(ctx.agent, access, &initial);
    if let Decision::Deny(reason) = &decision {
        return Err(ToolFailure::PermissionDenied(reason.clone()));
    }
    let before = snapshot(&initial)?;
    let before_hash = hash(&before);
    if before_hash.as_deref() != expected {
        return Err(ToolFailure::Conflict(
            "file changed or expected_sha256 missing; read the file again before editing".into(),
        ));
    }
    if content.is_none() && before.is_none() {
        return Err(ToolFailure::NotFound("file does not exist".into()));
    }
    let preview = difference(
        &initial.display(),
        before.as_deref().unwrap_or_default(),
        content.unwrap_or_default(),
    );
    let approved = force_approval || matches!(decision, Decision::Approval(_));
    if approved {
        ctx.app
            .request_approval(
                ctx.session_id,
                ctx.agent,
                tool,
                &format!(
                    "{}\nExpected version: {}\n{}",
                    if content.is_none() {
                        "Move file to recoverable trash"
                    } else {
                        "Apply file change"
                    },
                    before_hash.as_deref().unwrap_or("new file"),
                    preview
                ),
                Some(initial.display()),
            )
            .await?;
    }
    let _lock =
        PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(internal)?;
    let path = ctx.resolve_any(raw)?;
    ctx.check_read(tool, &path)?;
    if path.absolute != initial.absolute {
        return Err(ToolFailure::Conflict(
            "file target changed while waiting for approval; preview it again".into(),
        ));
    }
    match ctx.app.current_write_decision(ctx.agent, access, &path) {
        Decision::Deny(reason) => return Err(ToolFailure::PermissionDenied(reason)),
        Decision::Approval(_) if !approved => {
            return Err(ToolFailure::PermissionDenied(
                "write policy changed; request approval again".into(),
            ))
        }
        _ => {}
    }
    if hash(&snapshot(&path)?).as_deref() != expected {
        return Err(ToolFailure::Conflict(
            "file changed while waiting for approval; preview it again".into(),
        ));
    }
    let mut random = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let mut change = Change {
        id: hex::encode(random),
        workspace_id: ctx.app.workspace.id(),
        created_at: App::now(),
        agent: ctx.agent.dir().into(),
        path: path.display(),
        operation: tool.into(),
        before_sha256: before_hash,
        after_sha256: content.map(fsops::sha256_bytes),
        state: "prepared".into(),
        before_base64: before.map(|b| STANDARD.encode(b)),
        trash: None,
    };
    // Reserve the trash destination before recording intent. The model cannot
    // address bridge metadata, even when requesting a restore.
    let trash = if content.is_none() {
        let root = ctx.workspace_root().join(".bridge");
        if root.exists()
            && fs::symlink_metadata(&root)
                .map_err(internal)?
                .file_type()
                .is_symlink()
        {
            return Err(ToolFailure::PermissionDenied(
                "bridge directory is a symlink".into(),
            ));
        }
        let dir = root.join("trash");
        if dir.exists()
            && fs::symlink_metadata(&dir)
                .map_err(internal)?
                .file_type()
                .is_symlink()
        {
            return Err(ToolFailure::PermissionDenied(
                "trash directory is a symlink".into(),
            ));
        }
        fs::create_dir_all(&dir).map_err(internal)?;
        let destination = dir.join(&change.id);
        if destination.exists() {
            return Err(ToolFailure::Conflict("trash destination exists".into()));
        }
        change.trash = Some(format!(".bridge/trash/{}", change.id));
        Some(destination)
    } else {
        None
    };
    ctx.app.store.save_file_change(&change).map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "change.prepare",
            Some(path.display()),
            "allowed",
            &change.metadata().to_string(),
        )
        .map_err(internal)?;
    let result = if let Some(bytes) = content {
        fsops::atomic_write_locked(
            &ctx.app.workspace.checkpoint_dir,
            &path.relative,
            &path.absolute,
            bytes,
            expected.is_none(),
        )
        .map_err(internal)?
    } else {
        fsops::rename_no_replace(&path.absolute, &trash.expect("trash destination"))
            .map_err(|e| ToolFailure::from_io("move to trash", e))?;
        if let Some(parent) = path.absolute.parent() {
            if let Ok(dir) = fs::File::open(parent) {
                dir.sync_all().map_err(internal)?;
            }
        }
        AtomicWrite {
            sha256: String::new(),
            bytes: 0,
            checkpoint: None,
        }
    };
    change.state = "committed".into();
    ctx.app.store.save_file_change(&change).map_err(|e| {
        ToolFailure::Internal(format!(
            "file changed; recovery record finalization failed: {e}"
        ))
    })?;
    let mut detail = change.metadata();
    detail["checkpoint"] = json!(result
        .checkpoint
        .as_ref()
        .map(|p| fsops::display_relative(ctx.workspace_root(), p)));
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            &format!("tool.{tool}"),
            Some(path.display()),
            "allowed",
            &detail.to_string(),
        )
        .map_err(|e| ToolFailure::Internal(format!("file changed; audit append failed: {e}")))?;
    Ok((path, result, change))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    path: String,
    content: String,
}
pub async fn preview(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Preview = parse_args(args)?;
    if args.content.len() > 800_000 {
        return Err(ToolFailure::InvalidArguments(
            "content exceeds 800,000 bytes".into(),
        ));
    }
    let path = ctx.resolve_any(&args.path)?;
    ctx.check_read("preview_document", &path)?;
    let before = snapshot(&path)?;
    let output = difference(
        &path.display(),
        before.as_deref().unwrap_or_default(),
        args.content.as_bytes(),
    );
    Ok(ToolOutcome::read("change preview; nothing written",output,Some(path.display())).with_data(json!({"expected_sha256":hash(&before),"new_sha256":fsops::sha256_bytes(args.content.as_bytes()),"exists":path.exists})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default)]
    limit: Option<usize>,
}
pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: List = parse_args(args)?;
    let limit = args.limit.unwrap_or(30).clamp(1, 100);
    let changes: Vec<Value> = ctx
        .app
        .store
        .file_changes(&ctx.app.workspace.id(), 100)
        .map_err(internal)?
        .into_iter()
        .filter(|c| {
            ctx.resolve_any(&c.path)
                .is_ok_and(|p| ctx.app.decide(ctx.agent, Access::Read, &p) == Decision::Allow)
        })
        .take(limit)
        .map(|c| c.metadata())
        .collect();
    Ok(ToolOutcome::read(
        format!("{} recoverable changes", changes.len()),
        serde_json::to_string_pretty(&changes).map_err(internal)?,
        None,
    )
    .with_data(json!({"changes":changes})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delete {
    path: String,
    expected_sha256: String,
}
pub async fn delete(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Delete = parse_args(args)?;
    let (path, _, change) = mutate(
        ctx,
        "delete_file",
        &args.path,
        Some(&args.expected_sha256),
        None,
        true,
    )
    .await?;
    Ok(ToolOutcome::write(
        "file moved to trash",
        format!(
            "{} moved to trash; restore change {}",
            path.display(),
            change.id
        ),
        Some(path.display()),
    )
    .with_data(change.metadata()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    change_id: String,
}
pub async fn restore(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Restore = parse_args(args)?;
    let change = ctx
        .app
        .store
        .file_change(&args.change_id, &ctx.app.workspace.id())
        .map_err(internal)?
        .ok_or_else(|| ToolFailure::NotFound("no change in this workspace".into()))?;
    let before = change
        .before_base64
        .as_ref()
        .map(|b| STANDARD.decode(b).map_err(internal))
        .transpose()?;
    if hash(&before) != change.before_sha256 {
        return Err(ToolFailure::Internal(
            "recovery backup failed SHA256 verification".into(),
        ));
    }
    // The original post-image must still be present. A restore itself is a new
    // recoverable operation and always asks for approval.
    let (path, _, restored) = mutate(
        ctx,
        "restore_change",
        &change.path,
        change.after_sha256.as_deref(),
        before.as_deref(),
        true,
    )
    .await?;
    Ok(ToolOutcome::write(
        "change restored",
        format!("Restored {} from change {}", path.display(), change.id),
        Some(path.display()),
    )
    .with_data(restored.metadata()))
}
