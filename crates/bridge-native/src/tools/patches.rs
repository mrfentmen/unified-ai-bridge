//! Reviewed, recoverable edits to a bounded set of text files. Files commit
//! individually; partial and uncertain outcomes are receipts, never retries.
use super::{parse_args, recovery, ToolContext, ToolOutcome};
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::{self, PathLock},
    sandbox::{Access, Decision},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_FILES: usize = 20;
const MAX_BYTES: usize = 512_000;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub expected_sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    operations: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    patch_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Step {
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub expected: Option<String>,
    pub change_id: String,
    pub state: String,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Patch {
    pub id: String,
    pub workspace_id: String,
    pub owner: String,
    pub created_at: String,
    pub expires_at: i64,
    pub state: String,
    pub steps: Vec<Step>,
    pub restore_id: Option<String>,
}
impl Patch {
    pub fn metadata(&self) -> Value {
        json!({"patch_id":self.id,"workspace_id":self.workspace_id,"created_at":self.created_at,
        "expires_at":self.expires_at,"state":self.state,"restore_id":self.restore_id,
        "steps":self.steps.iter().map(|s|json!({"path":s.path,"state":s.state,"change_id":s.change_id,"expected_sha256":s.expected,"after_sha256":s.content.as_ref().map(|b|fsops::sha256_bytes(b)),"bytes":s.content.as_ref().map(Vec::len),"error":s.error})).collect::<Vec<_>>(),
        "atomicity":"Files commit separately. Never retry an interrupted batch; inspect receipts and changed files."})
    }
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn id() -> String {
    let mut b = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}
fn owned(ctx: &ToolContext<'_>, id: &str) -> Result<Patch, ToolFailure> {
    let owner = crate::jobs::owner(ctx)?;
    ctx.app
        .store
        .patch(id, &ctx.app.workspace.id())
        .map_err(internal)?
        .filter(|p| p.owner == owner)
        .ok_or_else(|| {
            ToolFailure::PermissionDenied(
                "patch belongs to another workspace or conversation".into(),
            )
        })
}
fn validate(ctx: &ToolContext<'_>, step: &Step) -> Result<String, ToolFailure> {
    let path = ctx.resolve_any(&step.path)?;
    ctx.check_read("patch", &path)?;
    let access = if step.content.is_some() {
        Access::Write
    } else {
        Access::Remove
    };
    if let Decision::Deny(reason) = ctx.app.current_write_decision(ctx.agent, access, &path) {
        return Err(ToolFailure::PermissionDenied(reason));
    }
    if path.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "patches operate on regular files only".into(),
        ));
    }
    if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
        super::require_text_extension(
            &path.absolute,
            &["md", "markdown", "txt", "json", "csv", "tsv", "svg"],
        )?;
    }
    let before = recovery::snapshot(&path)?;
    if recovery::hash(&before) != step.expected {
        return Err(ToolFailure::Conflict(format!(
            "{} changed since preview",
            path.display()
        )));
    }
    if let Some(bytes) = &before {
        if std::str::from_utf8(bytes).is_err() {
            return Err(ToolFailure::Unsupported(
                "batch patching currently supports UTF-8 text files".into(),
            ));
        }
    }
    // Approval must contain every changed line. Do not use the ordinary
    // tool-output helper, which truncates long differences.
    let after = std::str::from_utf8(step.content.as_deref().unwrap_or_default())
        .map_err(|_| ToolFailure::Unsupported("patch content must be UTF-8".into()))?;
    let before = std::str::from_utf8(before.as_deref().unwrap_or_default())
        .map_err(|_| ToolFailure::Unsupported("patch content must be UTF-8".into()))?;
    Ok(similar::TextDiff::configure()
        .timeout(std::time::Duration::from_millis(250))
        .diff_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(
            &format!("a/{}", path.display()),
            &format!("b/{}", path.display()),
        )
        .to_string())
}
fn review_text(ctx: &ToolContext<'_>, patch: &Patch) -> Result<String, ToolFailure> {
    let mut text=String::from("Review all paths below. Changes are individually checkpointed, not filesystem-wide atomic.\n");
    for step in &patch.steps {
        let difference = validate(ctx, step)?;
        text.push_str(&format!(
            "\n{} · expected {} · {}\n{}\n",
            step.path,
            step.expected.as_deref().unwrap_or("new file"),
            if step.content.is_some() {
                "write"
            } else {
                "move to trash"
            },
            difference
        ));
        if text.len() > 50_000 {
            return Err(ToolFailure::InvalidArguments(
                "patch diff exceeds 50,000 bytes; split it into smaller reviewed batches".into(),
            ));
        }
    }
    Ok(text)
}
fn make(ctx: &ToolContext<'_>, steps: Vec<Step>, restoring: bool) -> Result<Patch, ToolFailure> {
    if steps.is_empty() || steps.len() > MAX_FILES {
        return Err(ToolFailure::InvalidArguments(
            "patch must contain 1–20 files".into(),
        ));
    }
    if steps
        .iter()
        .map(|s| s.content.as_ref().map_or(0, Vec::len))
        .sum::<usize>()
        > if restoring {
            MAX_FILES * super::MAX_READ_BYTES as usize
        } else {
            MAX_BYTES
        }
    {
        return Err(ToolFailure::InvalidArguments(
            "patch exceeds the 512,000-byte content budget".into(),
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for step in &steps {
        if !paths.insert(step.path.clone()) {
            return Err(ToolFailure::InvalidArguments(
                "duplicate patch destination".into(),
            ));
        }
    }
    for a in &steps {
        for b in &steps {
            if a.path != b.path && std::path::Path::new(&b.path).starts_with(&a.path) {
                return Err(ToolFailure::InvalidArguments(
                    "patch destinations may not be parents of other destinations".into(),
                ));
            }
        }
    }
    Ok(Patch {
        id: id(),
        workspace_id: ctx.app.workspace.id(),
        owner: crate::jobs::owner(ctx)?,
        created_at: App::now(),
        expires_at: chrono::Utc::now().timestamp() + 1800,
        state: "previewed".into(),
        steps,
        restore_id: None,
    })
}
pub async fn preview(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Preview = parse_args(args)?;
    let mut steps = vec![];
    for op in args.operations {
        let path = ctx.resolve_any(&op.path)?;
        steps.push(Step {
            path: path.display(),
            content: Some(op.content.into_bytes()),
            expected: op.expected_sha256,
            change_id: id(),
            state: "pending".into(),
            error: None,
        });
    }
    let patch = make(ctx, steps, false)?;
    let text = review_text(ctx, &patch)?;
    ctx.app.store.save_patch(&patch).map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "patch.preview",
            None,
            "allowed",
            &patch.metadata().to_string(),
        )
        .map_err(internal)?;
    Ok(
        ToolOutcome::read("Patch preview saved; no files changed", text, None)
            .with_data(patch.metadata()),
    )
}
pub async fn status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let mut patch = owned(ctx, &args.patch_id)?;
    if matches!(patch.state.as_str(), "applying" | "awaiting_approval") {
        if let Ok(_guard) = PathLock::acquire(
            &ctx.app.home.base.join("locks"),
            &format!("patch:{}", patch.id),
        ) {
            // The operation may have finished between the initial read and
            // acquiring this lock. Never overwrite that completed receipt.
            patch = owned(ctx, &patch.id)?;
            if !matches!(patch.state.as_str(), "applying" | "awaiting_approval") {
                return Ok(
                    ToolOutcome::read("Patch receipt", "No operation was repeated.", None)
                        .with_data(patch.metadata()),
                );
            }
            patch.state = "interrupted".into();
            for step in &mut patch.steps {
                if let Some(change) = ctx
                    .app
                    .store
                    .file_change(&step.change_id, &patch.workspace_id)
                    .map_err(internal)?
                {
                    step.state = if change.state == "committed" {
                        "committed"
                    } else {
                        "uncertain"
                    }
                    .into();
                }
            }
            ctx.app.store.save_patch(&patch).map_err(internal)?;
            ctx.app
                .audit_path_access(
                    ctx.agent,
                    ctx.session_id,
                    "patch.interrupted",
                    None,
                    "uncertain",
                    &patch.metadata().to_string(),
                )
                .map_err(internal)?;
        }
    }
    Ok(ToolOutcome::read(
        format!("Patch {}: {}", patch.id, patch.state),
        "Prepared/uncertain receipts need reconciliation; no mutations are automatically repeated.",
        None,
    )
    .with_data(patch.metadata()))
}
pub async fn apply(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let patch = owned(ctx, &args.patch_id)?;
    execute(ctx, patch, "apply_patch").await
}
async fn execute(
    ctx: &ToolContext<'_>,
    mut patch: Patch,
    tool: &str,
) -> Result<ToolOutcome, ToolFailure> {
    let _guard = PathLock::acquire(
        &ctx.app.home.base.join("locks"),
        &format!("patch:{}", patch.id),
    )
    .map_err(internal)?;
    patch = owned(ctx, &patch.id)?;
    if patch.state == "completed" {
        return Ok(ToolOutcome::read(
            "Patch already completed; not repeated",
            "Saved receipts are returned.",
            None,
        )
        .with_data(patch.metadata()));
    }
    if patch.state != "previewed" {
        return Err(ToolFailure::Conflict("patch is already claimed or interrupted; inspect patch_status/restore_change before making a new preview".into()));
    }
    if chrono::Utc::now().timestamp() >= patch.expires_at {
        return Err(ToolFailure::PermissionDenied(
            "patch preview expired; create a fresh preview".into(),
        ));
    }
    let text = review_text(ctx, &patch)?;
    patch.state = "awaiting_approval".into();
    ctx.app.store.save_patch(&patch).map_err(internal)?;
    let entries = patch
        .steps
        .iter()
        .map(|s| recovery::ReviewEntry {
            path: s.path.clone(),
            expected: s.expected.clone(),
            after: s.content.as_ref().map(|b| fsops::sha256_bytes(b)),
            change_id: s.change_id.clone(),
        })
        .collect();
    let review = match recovery::review_batch(ctx, tool, entries, &text).await {
        Ok(review) => review,
        Err(error) => {
            patch.state = "denied".into();
            ctx.app.store.save_patch(&patch).map_err(internal)?;
            return Err(error);
        }
    };
    // Recheck the whole batch after the approval wait, then each file again
    // under its existing mutation lock. This does not claim a global FS lock.
    if chrono::Utc::now().timestamp() >= patch.expires_at {
        patch.state = "expired".into();
        ctx.app.store.save_patch(&patch).map_err(internal)?;
        return Err(ToolFailure::PermissionDenied(
            "patch preview expired during approval".into(),
        ));
    }
    if let Err(error) = review_text(ctx, &patch) {
        patch.state = "stale".into();
        ctx.app.store.save_patch(&patch).map_err(internal)?;
        return Err(error);
    }
    patch.state = "applying".into();
    ctx.app.store.save_patch(&patch).map_err(internal)?;
    let mut failed = None;
    for i in 0..patch.steps.len() {
        let step = patch.steps[i].clone();
        match recovery::mutate_reviewed(
            ctx,
            tool,
            &step.path,
            step.expected.as_deref(),
            step.content.as_deref(),
            &review,
        )
        .await
        {
            Ok(_) => patch.steps[i].state = "committed".into(),
            Err(error) => {
                let record = ctx
                    .app
                    .store
                    .file_change(&step.change_id, &patch.workspace_id)
                    .map_err(internal)?;
                patch.steps[i].state = if record.is_some() {
                    "uncertain"
                } else {
                    "failed"
                }
                .into();
                patch.steps[i].error = Some(error.to_string());
                failed = Some(error);
                break;
            }
        }
        ctx.app.store.save_patch(&patch).map_err(internal)?;
        tokio::task::yield_now().await;
    }
    patch.state = if failed.is_some() {
        "partial"
    } else {
        "completed"
    }
    .into();
    ctx.app.store.save_patch(&patch).map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            &format!("patch.{tool}"),
            None,
            &patch.state,
            &patch.metadata().to_string(),
        )
        .map_err(internal)?;
    let mut outcome=ToolOutcome::write(format!("Patch {}: {}",patch.id,patch.state),"Inspect per-file receipts. restore_patch restores committed steps after a new approval; uncertain steps require inspection and restore_change.",None).with_data(patch.metadata());
    if let Some(error) = failed {
        outcome.failure = Some(("partial_patch", error.to_string()));
    }
    Ok(outcome)
}
pub async fn restore(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let mut original = owned(ctx, &args.patch_id)?;
    let _guard = PathLock::acquire(
        &ctx.app.home.base.join("locks"),
        &format!("patch:{}", original.id),
    )
    .map_err(internal)?;
    original = owned(ctx, &original.id)?;
    if let Some(restore) = &original.restore_id {
        return execute(ctx, owned(ctx, restore)?, "restore_patch").await;
    }
    if !matches!(original.state.as_str(), "completed" | "partial") {
        return Err(ToolFailure::Conflict(
            "batch is not ready for batch restore; inspect per-file receipts".into(),
        ));
    }
    if original.steps.iter().any(|s| s.state == "uncertain") {
        return Err(ToolFailure::Conflict(
            "batch has uncertain changes; inspect files and restore_change receipts individually"
                .into(),
        ));
    }
    let mut steps = vec![];
    for step in original
        .steps
        .iter()
        .rev()
        .filter(|s| s.state == "committed")
    {
        let change = ctx
            .app
            .store
            .file_change(&step.change_id, &original.workspace_id)
            .map_err(internal)?
            .ok_or_else(|| ToolFailure::Internal("missing recovery receipt".into()))?;
        steps.push(Step {
            path: step.path.clone(),
            content: change.before_bytes()?,
            expected: change.after_sha256,
            change_id: id(),
            state: "pending".into(),
            error: None,
        });
    }
    let restore = make(ctx, steps, true)?;
    review_text(ctx, &restore)?;
    ctx.app.store.save_patch(&restore).map_err(internal)?;
    original.restore_id = Some(restore.id.clone());
    ctx.app.store.save_patch(&original).map_err(internal)?;
    execute(ctx, restore, "restore_patch").await
}
