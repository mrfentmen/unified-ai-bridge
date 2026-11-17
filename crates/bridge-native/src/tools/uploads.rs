//! Outbound disclosure is separate from read access. Bytes are sent only after
//! a one-time approval bound to their hash and the current browser destination.
use super::{binary, generated_files::scope, parse_args, ToolContext, ToolOutcome};
use crate::failure::ToolFailure;
use base64::Engine;
use bridge_core::browser_files::{source_key, valid_name, Action, UploadFile, MAX_UPLOAD_BYTES};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    path: String,
    expected_sha256: String,
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}
fn conflict(s: &str) -> ToolFailure {
    ToolFailure::Conflict(s.into())
}

fn source(ctx: &ToolContext<'_>, args: &Args) -> Result<UploadFile, ToolFailure> {
    let data = binary::read_source(
        ctx,
        "attach_file_to_chat",
        &args.path,
        MAX_UPLOAD_BYTES as u64,
    )?;
    let name = std::path::Path::new(&args.path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| valid_name(s))
        .ok_or_else(|| ToolFailure::InvalidArguments("Attachment needs a safe file name.".into()))?
        .to_string();
    let sha256 = crate::fsops::sha256_bytes(&data);
    if sha256 != args.expected_sha256 {
        return Err(conflict(
            "File hash changed. Inspect the file before requesting a new upload.",
        ));
    }
    let ext = std::path::Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" => {
            let actual = image::guess_format(&data).map_err(|_|conflict("Invalid image header."))?;
            match (ext.as_str(), actual) {
                ("png", image::ImageFormat::Png) => "image/png",
                ("jpg" | "jpeg", image::ImageFormat::Jpeg) => "image/jpeg",
                ("webp", image::ImageFormat::WebP) => "image/webp",
                _ => return Err(conflict("Image format and file extension differ.")),
            }
        }
        "txt" | "md" | "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "json" | "toml" | "yaml" | "yml" | "csv" | "html" | "css" | "c" | "h" | "cpp" | "lua" | "luau" | "xml" => {
            let text = std::str::from_utf8(&data).map_err(|_|conflict("Text attachments must be UTF-8."))?;
            if text.contains('\0') { return Err(conflict("Binary data is not a text attachment.")); }
            "text/plain"
        }
        _ => return Err(ToolFailure::Unsupported("Uploads support UTF-8 text/source files and PNG/JPEG/WebP up to 512 KiB. Other formats require manual attachment.".into())),
    };
    Ok(UploadFile {
        name,
        mime: mime.into(),
        bytes: data.len(),
        sha256,
        base64: base64::engine::general_purpose::STANDARD.encode(data),
    })
}

fn writable(ctx: &ToolContext<'_>) -> Result<(), ToolFailure> {
    if ctx
        .app
        .store
        .workspaces()
        .map_err(internal)?
        .iter()
        .any(|w| w.id == ctx.app.workspace.id() && !w.read_only)
    {
        Ok(())
    } else {
        Err(ToolFailure::PermissionDenied(
            "Workspace is locked or removed; outbound upload is blocked.".into(),
        ))
    }
}

pub async fn attach(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Args = parse_args(args)?;
    let destination = scope(ctx)?;
    if destination.provider != "chatgpt" {
        return Err(ToolFailure::Unsupported("Automatic attachments currently support ChatGPT only. Attach the file manually on this provider.".into()));
    }
    writable(ctx)?;
    let file = source(ctx, &args)?;
    let preflight = ctx
        .app
        .browser_files
        .request(ctx.app, destination.clone(), Action::InspectUpload)
        .await?;
    let turn = preflight["turn_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 2000)
        .ok_or_else(|| conflict("Cannot identify the upload's conversation turn."))?
        .to_string();
    let reason = format!("Upload this local file to ChatGPT, conversation {}, tab {}.\nPath: {}\nFile: {}\nType: {}\nBytes: {}\nSHA-256: {}\nThis discloses its full bytes to the website. The attachment accompanies the bridge result (or waits for manual Send when requested from the panel). No automatic retry.", destination.conversation, destination.tab_id, args.path, file.name, file.mime, file.bytes, file.sha256);
    // No path parameter: this cannot be auto-approved by a local-write grant or YOLO.
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "attach_file_to_chat",
            &reason,
            None,
        )
        .await?;
    if scope(ctx)? != destination {
        return Err(conflict(
            "Upload destination changed during approval. No file was sent.",
        ));
    }
    writable(ctx)?;
    let checked = source(ctx, &args)?;
    if checked.bytes != file.bytes || checked.mime != file.mime {
        return Err(conflict("File changed during approval. No file was sent."));
    }
    crate::tasks::checkpoint(ctx.app)?;
    let owner = source_key(&[
        &destination.workspace_id,
        &destination.provider,
        &destination.conversation,
    ]);
    let key = source_key(&[&turn, &file.name, &file.sha256]);
    if !ctx
        .app
        .store
        .claim_browser_upload(&owner, &key)
        .map_err(internal)?
    {
        return Err(conflict("This file already has an upload attempt for this turn. Inspect the composer/chat; uncertain uploads are never retried automatically."));
    }
    let attempt_id = crate::approvals::ApprovalManager::new_id();
    let metadata = json!({"upload_id":attempt_id,"name":file.name,"sha256":file.sha256,"bytes":file.bytes,"mime":file.mime,"conversation":destination.conversation,"workspace_id":destination.workspace_id});
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "browser.upload_prepare",
            Some(args.path.clone()),
            "allowed",
            &metadata.to_string(),
        )
        .map_err(internal)?;
    let reply = ctx
        .app
        .browser_files
        .request(
            ctx.app,
            destination.clone(),
            Action::Upload {
                file: checked,
                turn_id: turn,
                attempt_id: attempt_id.clone(),
            },
        )
        .await?;
    if scope(ctx)? != destination {
        return Err(conflict(
            "Upload may have completed but the session changed. Inspect the website; no retry.",
        ));
    }
    if reply["state"] != "attached"
        || reply["upload_id"] != attempt_id
        || reply["sha256"] != file.sha256
    {
        return Err(conflict(
            "Website attachment was not confirmed. Inspect the composer; no automatic retry.",
        ));
    }
    let mut data = metadata;
    data["state"] = "attached".into();
    ctx.app
        .store
        .finish_browser_upload(&owner, &key, &data)
        .map_err(internal)?;
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "browser.upload_ready",
            Some(args.path.clone()),
            "allowed",
            &data.to_string(),
        )
        .map_err(internal)?;
    Ok(ToolOutcome::write(format!("{} uploaded to the ChatGPT composer",file.name),"The real file is ready to accompany this result. If requested from panel controls, inspect the attachment and send your message in the website. Uploaded bytes cannot be recalled by the bridge; do not repeat this tool.",Some(args.path)).with_data(data))
}
