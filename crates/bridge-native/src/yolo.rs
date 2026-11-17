//! Explicit panel-only approval of routine local writes. Never a sandbox bypass.
//! The grant is held by this native connection and is lost on host restart.
use crate::{
    app::App,
    failure::ToolFailure,
    sandbox::{Access, Decision, Existence},
};
use anyhow::{bail, Result};
use bridge_core::provider::Agent;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const TOOLS: &[&str] = &[
    "write_document",
    "edit_document",
    "append_document",
    "copy_file",
    "create_folder",
    "create_document",
    "write_sheet_range",
    "transform_image",
    "transform_media",
    "render_document_pages",
    "create_archive",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Enable { seconds: u64, max_approvals: u32 },
    Stop,
}

struct Grant {
    id: String,
    workspace: String,
    deadline: Instant,
    expires_at: i64,
    used: u32,
    limit: u32,
}
#[derive(Default)]
pub struct Manager {
    grant: Mutex<Option<Grant>>,
}
#[derive(Clone)]
struct Lease {
    id: String,
    workspace: String,
}
tokio::task_local! { static LEASE: RefCell<Option<Lease>>; }

pub async fn scope<T>(future: impl std::future::Future<Output = T>) -> T {
    LEASE.scope(RefCell::new(None), future).await
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn valid_time(g: &Grant) -> bool {
    Instant::now() < g.deadline && chrono::Utc::now().timestamp() < g.expires_at
}

pub fn status(app: &App) -> Value {
    let guard = app.approvals.yolo.grant.lock().expect("YOLO mutex");
    match guard.as_ref().filter(|g| g.workspace == app.workspace.id()) {
        Some(g) => json!({"active":valid_time(g) && g.used < g.limit,"workspace_id":g.workspace,
            "expires_at":g.expires_at,"remaining_seconds":if valid_time(g){g.deadline.saturating_duration_since(Instant::now()).as_secs()}else{0},
            "used_approvals":g.used,"max_approvals":g.limit}),
        None => json!({"active":false,"workspace_id":app.workspace.id()}),
    }
}

pub fn stop(app: &App, reason: &str) -> Result<()> {
    let previous = app.approvals.yolo.grant.lock().expect("YOLO mutex").take();
    if let Some(g) = previous {
        app.audit_workspace("yolo.stop", json!({"grant_id":g.id,"workspace_id":g.workspace,"reason":reason,"used_approvals":g.used}))?;
    }
    Ok(())
}

pub fn control(app: &App, workspace: &str, control: Control) -> Result<Value> {
    if !app.interactive {
        bail!("YOLO requires a connected extension panel");
    }
    if workspace != app.workspace.id() {
        bail!("workspace changed; review YOLO scope again");
    }
    match control {
        Control::Stop => stop(app, "user stopped automatic approvals")?,
        Control::Enable {
            seconds,
            max_approvals,
        } => {
            if !(1..=43200).contains(&seconds) || !(1..=10000).contains(&max_approvals) {
                bail!("YOLO limits: 1–43200 seconds and 1–10000 automatic approvals");
            }
            if app
                .store
                .workspaces()?
                .iter()
                .find(|w| w.id == workspace)
                .is_none_or(|w| w.read_only)
            {
                bail!("unlock the workspace before enabling YOLO");
            }
            let grant = Grant {
                id: crate::approvals::ApprovalManager::new_id(),
                workspace: workspace.into(),
                deadline: Instant::now() + Duration::from_secs(seconds),
                expires_at: chrono::Utc::now().timestamp() + seconds as i64,
                used: 0,
                limit: max_approvals,
            };
            let mut guard = app.approvals.yolo.grant.lock().expect("YOLO mutex");
            app.audit_workspace("yolo.enable", json!({"grant_id":grant.id,"workspace_id":workspace,
                "root":app.workspace.root,"expires_at":grant.expires_at,"max_approvals":max_approvals,
                "tools":TOOLS,"batch_tool":"apply_patch (writes only)","restart":"off"}))?;
            *guard = Some(grant);
        }
    }
    Ok(json!({"yolo":status(app)}))
}

/// Called only by native approval sites, after ordinary tool validation.
/// Batch patches supply every destination and cannot include a deletion.
pub(crate) fn approve(
    app: &App,
    session: &str,
    agent: Agent,
    tool: &str,
    paths: &[String],
    batch: bool,
) -> Result<bool, ToolFailure> {
    // Batch approvals also pass here without App::request_approval. Preserve
    // task Pause/Stop checks at that admission point.
    crate::tasks::checkpoint(app)?;
    if !app.interactive
        || LEASE.try_with(|_| ()).is_err()
        || if batch {
            tool != "apply_patch"
        } else {
            !TOOLS.contains(&tool)
        }
        || paths.is_empty()
    {
        return Ok(false);
    }
    checkpoint(app)?;
    let Some((record, owner)) = app.session_agent(session).map_err(internal)? else {
        return Ok(false);
    };
    if owner != agent || record.workspace_id != app.workspace.id() {
        return Ok(false);
    }
    // A narrower session grant takes precedence. Other tools still need their
    // ordinary manual approval; YOLO cannot enlarge that session's file scope.
    if app
        .store
        .write_grant(&app.workspace.id(), session)
        .map_err(internal)?
        .is_some()
    {
        return Ok(false);
    }
    // Path rules remain authoritative, including research, private files and locks.
    for raw in paths {
        let path = app.sandbox().resolve(raw, Existence::Any)?;
        if let Decision::Deny(reason) = app.current_write_decision(agent, Access::Write, &path) {
            return Err(ToolFailure::PermissionDenied(reason));
        }
    }
    let mut guard = app.approvals.yolo.grant.lock().expect("YOLO mutex");
    let Some(g) = guard
        .as_mut()
        .filter(|g| g.workspace == app.workspace.id() && valid_time(g) && g.used < g.limit)
    else {
        return Ok(false);
    };
    // Reserve before executing; failed or uncertain attempts are not refunded.
    g.used += 1;
    app.audit_path_access(agent,session,&format!("approval.yolo:{tool}"),paths.first().cloned(),"approved",
        &json!({"grant_id":g.id,"workspace_id":g.workspace,"paths":paths,"used_approvals":g.used,"max_approvals":g.limit,"expires_at":g.expires_at}).to_string()).map_err(internal)?;
    LEASE.with(|lease| {
        *lease.borrow_mut() = Some(Lease {
            id: g.id.clone(),
            workspace: g.workspace.clone(),
        })
    });
    Ok(true)
}

/// Rechecked at each existing write-policy checkpoint. Stop does not undo
/// committed changes, cancel external jobs or replay any request.
pub fn checkpoint(app: &App) -> Result<(), ToolFailure> {
    let Some(lease) = LEASE.try_with(|l| l.borrow().clone()).ok().flatten() else {
        return Ok(());
    };
    let guard = app.approvals.yolo.grant.lock().expect("YOLO mutex");
    if guard.as_ref().is_some_and(|g| {
        g.id == lease.id
            && g.workspace == lease.workspace
            && g.workspace == app.workspace.id()
            && valid_time(g)
    }) {
        Ok(())
    } else {
        Err(ToolFailure::PermissionDenied(
            "YOLO expired or was stopped; this operation will not be replayed".into(),
        ))
    }
}
