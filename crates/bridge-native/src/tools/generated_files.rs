//! Generated-file tools go through normal host admission, approvals and audit.
//! The browser supplies opaque DOM capabilities, never arbitrary model URLs.
use super::{parse_args, ToolContext, ToolOutcome};
use crate::{failure::ToolFailure, tools::artifacts};
use bridge_core::browser_files::{Action, DownloadMethod, GeneratedFile, Scope};
use serde::Deserialize;
use serde_json::{json, Value};

fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
pub(super) fn scope(ctx: &ToolContext<'_>) -> Result<Scope, ToolFailure> {
    if !ctx.app.interactive {
        return Err(ToolFailure::Unsupported(
            "Generated files require a connected browser extension.".into(),
        ));
    }
    let (session, agent) = ctx
        .app
        .session_agent(ctx.session_id)
        .map_err(internal)?
        .ok_or_else(|| ToolFailure::PermissionDenied("Unknown browser session.".into()))?;
    if agent != ctx.agent
        || session.workspace_id != ctx.app.workspace.id()
        || ctx
            .app
            .store
            .active_workspace_id()
            .map_err(internal)?
            .as_deref()
            != Some(&session.workspace_id)
    {
        return Err(ToolFailure::PermissionDenied(
            "Browser session or active workspace changed.".into(),
        ));
    }
    Ok(Scope {
        session_id: session.id,
        workspace_id: session.workspace_id,
        provider: session.provider,
        tab_id: session.tab_id,
        conversation: session
            .conversation
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                ToolFailure::Unsupported(
                    "Open a saved conversation before discovering files.".into(),
                )
            })?,
        task_id: crate::tasks::original_task_id(ctx.app)?,
    })
}
fn writable(ctx: &ToolContext<'_>) -> Result<(), ToolFailure> {
    if ctx.app.workspace.read_only
        || !ctx
            .app
            .store
            .workspaces()
            .map_err(internal)?
            .iter()
            .any(|w| w.id == ctx.app.workspace.id() && !w.read_only)
    {
        return Err(ToolFailure::PermissionDenied(
            "The workspace was locked or removed; no generated-file download is allowed.".into(),
        ));
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Listing {
    files: Vec<GeneratedFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    handle: String,
}
#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Downloaded {
    Shared {
        download: artifacts::SelectedDownload,
    },
    ManualShareRequired,
}

pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    let scope = scope(ctx)?;
    let value = ctx
        .app
        .browser_files
        .request(ctx.app, scope.clone(), Action::List)
        .await?;
    if self::scope(ctx)? != scope {
        return Err(ToolFailure::Conflict("Browser scope changed.".into()));
    }
    let listing: Listing = parse_args(value)?;
    if listing.files.len() > 40 || listing.files.iter().any(|f| !f.valid()) {
        return Err(ToolFailure::InvalidArguments(
            "Invalid generated-file metadata.".into(),
        ));
    }
    Ok(ToolOutcome::read(format!("{} rendered generated file controls",listing.files.len()),
        "Only rendered assistant file controls are listed. Handles expire after five minutes or a task, session, workspace or route change. Other website file controls may require manual download. Website-button downloads require explicit selection in Browser files before import.",None)
        .with_data(json!({"files":listing.files})))
}

pub async fn download(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Request = parse_args(args)?;
    if args.handle.len() != 32 || !args.handle.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ToolFailure::InvalidArguments(
            "Use a current handle returned by list_generated_files.".into(),
        ));
    }
    let scope = scope(ctx)?;
    writable(ctx)?;
    let value = ctx
        .app
        .browser_files
        .request(
            ctx.app,
            scope.clone(),
            Action::Inspect {
                handle: args.handle.clone(),
            },
        )
        .await?;
    let file: GeneratedFile = parse_args(value)?;
    if !file.valid() || file.handle != args.handle {
        return Err(ToolFailure::InvalidArguments(
            "Invalid generated-file metadata.".into(),
        ));
    }
    let owner = bridge_core::browser_files::source_key(&[
        &scope.workspace_id,
        &scope.provider,
        &scope.conversation,
    ]);
    if ctx
        .app
        .store
        .browser_download_attempted(&owner, &file.source_key)
        .map_err(internal)?
    {
        return Err(ToolFailure::Conflict("This file already has a download attempt. Inspect Browser files; an uncertain action cannot be retried automatically.".into()));
    }
    let delivery = match file.method {
        DownloadMethod::DirectLink => "Download the rendered link through Chrome and share that exact completed download with this conversation. Importing into the workspace requires separate approval.",
        DownloadMethod::WebsiteButton => "Click this file's website Download button once. Chrome does not identify the originating conversation for this button: select and share the completed file in Browser files before importing. This approval does not share any unrelated download.",
    };
    ctx.app.request_approval(ctx.session_id, ctx.agent, "request_download",
        &format!("{}\nProvider: {}\nConversation: {}\nFile: {}\nResponse: {}\nNo opening, execution, extraction or automatic retry. Import limit: 64 MiB.",delivery,scope.provider,scope.conversation,file.name,file.response_id),None).await?;
    if self::scope(ctx)? != scope {
        return Err(ToolFailure::Conflict(
            "Browser scope changed during approval.".into(),
        ));
    }
    writable(ctx)?;
    crate::tasks::checkpoint(ctx.app)?;
    // Persist before sending the browser mutation. Rediscovery cannot bypass
    // this claim, including after a crash or a new host-issued session.
    if !ctx
        .app
        .store
        .claim_browser_download(&owner, &file.source_key)
        .map_err(internal)?
    {
        return Err(ToolFailure::Conflict("This generated file already has a download attempt. Inspect Browser files; the outcome may be uncertain. It will not be downloaded again automatically.".into()));
    }
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "browser.download_prepare",
            None,
            "allowed",
            &json!({"file":file,"workspace_id":scope.workspace_id}).to_string(),
        )
        .map_err(internal)?;
    let response = ctx
        .app
        .browser_files
        .request(
            ctx.app,
            scope.clone(),
            Action::Download { file: file.clone() },
        )
        .await?;
    if self::scope(ctx)? != scope {
        return Err(ToolFailure::Conflict(
            "Workspace/session changed; the download may exist but was not shared.".into(),
        ));
    }
    let downloaded: Downloaded = parse_args(response)?;
    let data = match downloaded {
        Downloaded::Shared { download } if file.method == DownloadMethod::DirectLink => {
            let artifact =
                artifacts::register(ctx.app, ctx.session_id, &scope.workspace_id, download)?;
            json!({"state":"shared","artifact":artifact})
        }
        Downloaded::ManualShareRequired => json!({"state":"manual_share_required","name":file.name,
            "next_step":"The download action was requested, but completion and conversation ownership cannot be proven. In Browser files, refresh downloads, select this file and share it with this session; then use list_artifacts and import_artifact. Do not repeat request_download."}),
        _ => {
            return Err(ToolFailure::PermissionDenied(
                "Website-button downloads cannot be automatically shared.".into(),
            ))
        }
    };
    ctx.app
        .store
        .finish_browser_download(&owner, &file.source_key, &data)
        .map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "browser.download_result",
            None,
            "allowed",
            &data.to_string(),
        )
        .map_err(internal)?;
    Ok(ToolOutcome::write(format!("Generated file {}: {}",file.name,data["state"].as_str().unwrap_or_default()),
        "The original download remains in Chrome's download directory. No workspace import has been performed.",None).with_data(data))
}
