//! Bounded, deterministic PDF reports. No user fonts, markup, actions or URLs.
use super::{
    artifacts::directory,
    binary::{self, Stage},
    parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;

mod layout;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    destination: String,
    format: Format,
    template: Template,
    title: String,
    blocks: Vec<Block>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Format {
    Pdf,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Template {
    Report,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Block {
    Heading { text: String },
    Paragraph { text: String },
    Bullet { text: String },
    PageBreak,
}
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}

pub async fn create(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let mut args: Document = parse_args(args)?;
    if !std::path::Path::new(&args.destination)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return Err(invalid("create_document requires a new .pdf destination"));
    }
    let initial = binary::target(ctx, "create_document", &args.destination)?;
    let (output, pages) = layout::generate(&mut args)?;
    let hash = fsops::sha256_bytes(&output);
    // Full bounded content, including normalization, is visible for approval.
    let review = format!("Create a new PDF report: {} pages, {} bytes, SHA256 {}. All text below is literal content; no links, scripts or attachments are created. Original files are never replaced.\n{}", pages, output.len(), hash, serde_json::to_string_pretty(&args).map_err(io)?);
    if review.len() > 50000 {
        return Err(invalid(
            "document approval exceeds 50,000 bytes; reduce content",
        ));
    }
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "create_document",
            &review,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let target = binary::target(ctx, "create_document", &args.destination)?;
    let mut stage = Stage::new(directory(target.absolute.parent().unwrap())?, false)?;
    let mut data = json!({"format":"pdf","template":"report","destination":target.display(),"pages":pages,"bytes":output.len(),"sha256":hash,"state":"prepared","staging_name":stage.name.to_string_lossy(),"layout":"A4; embedded Noto Sans; left-to-right Latin text; no shaping, tables, images, hyperlinks, forms or PDF/A claim"});
    let staged = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "document.prepare",
                Some(target.display()),
                "allowed",
                &data.to_string(),
            )
            .map_err(io)?;
        stage.file.write_all(&output).map_err(io)?;
        binary::target(ctx, "create_document", &args.destination)?;
        stage.publish(&target.absolute)
    })();
    binary::staged_result(
        ctx,
        "create_document",
        &target.display(),
        &mut stage,
        staged,
    )?;
    data["state"] = json!("completed");
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.create_document",
            Some(target.display()),
            "allowed",
            &data.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "PDF published but final audit failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write(
        "PDF report saved as a new file",
        "Open or render the PDF to inspect layout. Saving this file does not attach it to a chat.",
        Some(target.display()),
    )
    .with_data(data))
}
