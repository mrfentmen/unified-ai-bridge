//! Initial media subset: bounded PCM WAV inspection and lossless frame trims.
//! No external codecs, metadata interpretation, playback or network loading.
use super::{
    artifacts::directory,
    binary::{self, Stage},
    parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;

mod wav;
const MAX_INPUT: u64 = 32 * 1024 * 1024;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Info {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transform {
    path: String,
    destination: String,
    expected_sha256: String,
    operation: Operation,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Trim { start_frame: u64, end_frame: u64 },
}
fn source(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<(Vec<u8>, String), ToolFailure> {
    let bytes = binary::read_source(ctx, tool, path, MAX_INPUT)?;
    let hash = fsops::sha256_bytes(&bytes);
    if expected.is_some_and(|h| h != hash) {
        return Err(ToolFailure::Conflict(
            "media changed; inspect it again before transforming".into(),
        ));
    }
    Ok((bytes, hash))
}
pub async fn info(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Info = parse_args(args)?;
    let _slot = SLOTS
        .try_acquire()
        .map_err(|_| invalid("media processing busy; wait for a running operation"))?;
    let (bytes, hash) = source(ctx, "media_info", &args.path, None)?;
    let wave = wav::parse(&bytes)?;
    let mut data = wave.metadata();
    data["sha256"] = json!(hash);
    data["bytes"] = json!(bytes.len());
    Ok(ToolOutcome::read("PCM WAV information", "Container metadata and sample layout only. No playback, transcription, hearing or upload occurred.",Some(args.path)).with_data(data))
}
pub async fn transform(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Transform = parse_args(args)?;
    if !std::path::Path::new(&args.destination)
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("wav"))
    {
        return Err(invalid("media destination must be a new .wav file"));
    }
    let _slot = SLOTS
        .try_acquire()
        .map_err(|_| invalid("media processing busy; wait for a running operation"))?;
    let (bytes, hash) = source(
        ctx,
        "transform_media",
        &args.path,
        Some(&args.expected_sha256),
    )?;
    let input = wav::parse(&bytes)?;
    let Operation::Trim {
        start_frame,
        end_frame,
    } = args.operation;
    let output = input.trim(start_frame, end_frame)?;
    drop(bytes);
    let initial = binary::target(ctx, "transform_media", &args.destination)?;
    let mut data = wav::parse(&output)?.metadata();
    data["source_sha256"] = json!(hash);
    data["sha256"] = json!(fsops::sha256_bytes(&output));
    data["bytes"] = json!(output.len());
    data["source"] = json!(args.path);
    data["destination"] = json!(initial.display());
    data["start_frame"] = json!(start_frame);
    data["end_frame_exclusive"] = json!(end_frame);
    let review=format!("Create a trimmed PCM WAV copy. Frames are zero-based: start inclusive, end exclusive. Channel/sample bytes are preserved exactly; no resampling, normalization or fades. Only container padding is discarded. Original preserved; no overwrite or audio upload.\n{}",serde_json::to_string_pretty(&data).map_err(io)?);
    // Retain bounded buffers across approval: at most two pending media operations.
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "transform_media",
            &review,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let target = binary::target(ctx, "transform_media", &args.destination)?;
    source(ctx, "transform_media", &args.path, Some(&hash))?;
    let mut stage = Stage::new(directory(target.absolute.parent().unwrap())?, false)?;
    data["state"] = json!("prepared");
    data["staging_name"] = json!(stage.name.to_string_lossy());
    let staged = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "media.prepare",
                Some(target.display()),
                "allowed",
                &data.to_string(),
            )
            .map_err(io)?;
        stage.file.write_all(&output).map_err(io)?;
        binary::target(ctx, "transform_media", &args.destination)?;
        stage.publish(&target.absolute)
    })();
    binary::staged_result(
        ctx,
        "transform_media",
        &target.display(),
        &mut stage,
        staged,
    )?;
    data["state"] = json!("completed");
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.transform_media",
            Some(target.display()),
            "allowed",
            &data.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "WAV published but final audit failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write(
        "Trimmed WAV saved as a new file",
        "Exact selected interleaved frames copied. Original preserved; no playback or upload.",
        Some(target.display()),
    )
    .with_data(data))
}
