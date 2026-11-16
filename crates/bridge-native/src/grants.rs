//! Panel-created, expiring write grants. A grant narrows a session's file-write
//! scope; expiry/revocation/budget exhaustion never falls back to Trusted mode.
use crate::{
    app::App,
    failure::ToolFailure,
    fsops::PathLock,
    sandbox::{Access, Decision, ResolvedPath},
    tools::ToolContext,
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const TOOLS: &[&str] = &[
    "write_document",
    "edit_document",
    "append_document",
    "copy_file",
    "create_folder",
];
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub session_id: String,
    pub directory: String,
    pub tools: Vec<String>,
    pub seconds: u64,
    pub max_calls: u64,
    pub max_bytes: u64,
    pub max_files: usize,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    pub agent: bridge_core::provider::Agent,
    pub workspace_id: String,
    pub session_id: String,
    pub directory: String,
    pub tools: Vec<String>,
    pub created_at: String,
    pub expires_at: i64,
    pub revoked: bool,
    pub max_calls: u64,
    pub max_bytes: u64,
    pub max_files: usize,
    pub used_calls: u64,
    pub used_bytes: u64,
    pub paths: Vec<String>,
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn lock(app: &App, sid: &str) -> Result<PathLock> {
    // Ordinary writes from the same session may arrive concurrently. Budget
    // commits contain no async waits; briefly serialize them, with a deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        match PathLock::acquire(
            &app.home.base.join("locks"),
            &format!("write-grant:{}:{sid}", app.workspace.id()),
        ) {
            Ok(guard) => return Ok(guard),
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

pub fn create(app: &App, workspace_id: &str, request: Request) -> Result<Value> {
    if workspace_id != app.workspace.id() {
        bail!("workspace changed; review the grant again");
    }
    if request.seconds == 0
        || request.seconds > 43200
        || request.max_calls == 0
        || request.max_calls > 10000
        || request.max_bytes == 0
        || request.max_bytes > 64 * 1024 * 1024
        || request.max_files == 0
        || request.max_files > 1000
    {
        bail!("grant limits: 1–43200 seconds, 1–10000 calls, 1–67108864 bytes, 1–1000 paths");
    }
    if request.tools.is_empty()
        || request.tools.len() > TOOLS.len()
        || request.tools.iter().any(|t| !TOOLS.contains(&t.as_str()))
    {
        bail!("select only supported file-write tools");
    }
    let (session, agent) = app
        .session_agent(&request.session_id)?
        .ok_or_else(|| anyhow::anyhow!("session is no longer registered"))?;
    if session.workspace_id != workspace_id {
        bail!("session belongs to a different workspace");
    }
    let ctx = ToolContext::new(app, agent, &request.session_id);
    let path = ctx.resolve_existing(&request.directory)?;
    ctx.check_read("write_grant", &path)?;
    if !path.is_dir {
        bail!("grant directory must already exist");
    }
    if let Decision::Deny(reason) = app.current_write_decision(agent, Access::Write, &path) {
        bail!("{reason}");
    }
    let _guard = lock(app, &request.session_id)?;
    let grant = Grant {
        agent,
        workspace_id: workspace_id.into(),
        session_id: request.session_id,
        directory: path.display(),
        tools: request.tools,
        created_at: App::now(),
        expires_at: chrono::Utc::now().timestamp() + request.seconds as i64,
        revoked: false,
        max_calls: request.max_calls,
        max_bytes: request.max_bytes,
        max_files: request.max_files,
        used_calls: 0,
        used_bytes: 0,
        paths: vec![],
    };
    app.audit_path_access(
        agent,
        "panel",
        "grant.create",
        Some(path.display()),
        "allowed",
        &serde_json::to_string(&grant)?,
    )?;
    app.store.save_write_grant(&grant)?;
    Ok(serde_json::to_value(grant)?)
}
pub fn revoke(app: &App, workspace_id: &str, sid: &str) -> Result<Value> {
    if workspace_id != app.workspace.id() {
        bail!("workspace changed; refresh grants");
    }
    let _guard = lock(app, sid)?;
    let mut grant = app
        .store
        .write_grant(workspace_id, sid)?
        .ok_or_else(|| anyhow::anyhow!("no grant for this session"))?;
    grant.revoked = true;
    app.store.save_write_grant(&grant)?;
    app.audit_path_access(
        grant.agent,
        "panel",
        "grant.revoke",
        None,
        "allowed",
        &json!({"session_id":sid,"workspace_id":workspace_id}).to_string(),
    )?;
    Ok(json!({"revoked":true,"session_id":sid}))
}
fn validate(
    ctx: &ToolContext<'_>,
    grant: &Grant,
    tool: &str,
    path: &ResolvedPath,
    bytes: usize,
) -> Result<(), ToolFailure> {
    let reason = if grant.revoked {
        Some("write grant was revoked")
    } else if chrono::Utc::now().timestamp() >= grant.expires_at {
        Some("write grant expired")
    } else if !grant.tools.iter().any(|t| t == tool) {
        Some("tool is outside the session write grant")
    } else if grant.used_calls >= grant.max_calls {
        Some("write grant call budget exhausted")
    } else if (bytes as u64) > grant.max_bytes.saturating_sub(grant.used_bytes) {
        Some("write grant byte budget exhausted")
    } else if !grant.paths.contains(&path.display()) && grant.paths.len() >= grant.max_files {
        Some("write grant file budget exhausted")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(ToolFailure::PermissionDenied(reason.into()));
    }
    let root = ctx.resolve_existing(&grant.directory)?;
    if !root.is_dir || !path.absolute.starts_with(&root.absolute) {
        return Err(ToolFailure::PermissionDenied(
            "path is outside the session write grant".into(),
        ));
    }
    let (session, agent) = ctx
        .app
        .session_agent(ctx.session_id)?
        .ok_or_else(|| ToolFailure::PermissionDenied("grant session disconnected".into()))?;
    if session.workspace_id != grant.workspace_id || agent != ctx.agent {
        return Err(ToolFailure::PermissionDenied(
            "grant session identity changed".into(),
        ));
    }
    if let Decision::Deny(reason) = ctx
        .app
        .current_write_decision(ctx.agent, Access::Write, path)
    {
        return Err(ToolFailure::PermissionDenied(reason));
    }
    Ok(())
}
/// Read-only check before any approval wait. No grant means ordinary policy.
pub fn applicable(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &ResolvedPath,
    bytes: usize,
) -> Result<bool, ToolFailure> {
    if !TOOLS.contains(&tool) {
        return Ok(false);
    }
    let Some(grant) = ctx
        .app
        .store
        .write_grant(&ctx.app.workspace.id(), ctx.session_id)
        .map_err(internal)?
    else {
        return Ok(false);
    };
    validate(ctx, &grant, tool, path, bytes)?;
    Ok(true)
}
/// Reserve budgets and hold the grant lock through the synchronous file commit.
/// Failed writes consume their reservation; uncertain work is never refunded.
pub fn reserve(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &ResolvedPath,
    bytes: usize,
) -> Result<Option<PathLock>, ToolFailure> {
    if !TOOLS.contains(&tool) {
        return Ok(None);
    }
    let guard = lock(ctx.app, ctx.session_id).map_err(internal)?;
    let Some(mut grant) = ctx
        .app
        .store
        .write_grant(&ctx.app.workspace.id(), ctx.session_id)
        .map_err(internal)?
    else {
        return Ok(None);
    };
    validate(ctx, &grant, tool, path, bytes)?;
    grant.used_calls += 1;
    grant.used_bytes += bytes as u64;
    if !grant.paths.contains(&path.display()) {
        grant.paths.push(path.display());
    }
    ctx.app.store.save_write_grant(&grant).map_err(internal)?;
    ctx.app.audit_path_access(ctx.agent,ctx.session_id,"grant.reserve",Some(path.display()),"allowed",&json!({"tool":tool,"bytes":bytes,"used_calls":grant.used_calls,"used_bytes":grant.used_bytes,"used_files":grant.paths.len(),"expires_at":grant.expires_at}).to_string()).map_err(internal)?;
    Ok(Some(guard))
}
