//! Optional local OCR. Fast, CPU-only English Vision request in a restricted
//! worker. It recognizes raster text; it does not give a model image vision.
use super::{binary, documents, parse_args, ToolContext, ToolOutcome};
use crate::{failure::ToolFailure, fsops};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{Read, Write};

#[cfg(target_os = "macos")]
mod macos;
const MAX_INPUT: u64 = 8 * 1024 * 1024;
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
struct Args {
    path: String,
    expected_sha256: Option<String>,
    max_bytes: Option<usize>,
}
pub async fn recognize(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Args = parse_args(args)?;
    let limit = args.max_bytes.unwrap_or(12000);
    if !(256..=24000).contains(&limit) {
        return Err(invalid("OCR max_bytes must be 256–24000"));
    }
    let bytes = binary::read_source(ctx, "ocr_image", &args.path, MAX_INPUT)?;
    let hash = fsops::sha256_bytes(&bytes);
    if args.expected_sha256.is_some_and(|h| h != hash) {
        return Err(ToolFailure::Conflict(
            "image changed; inspect it again before OCR".into(),
        ));
    }
    if !cfg!(target_os = "macos") {
        return Err(unsupported("local OCR requires macOS 10.15 or later with a working Vision fast English backend; no OCR backend is installed on this platform"));
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(unsupported(
            "OCR currently accepts PNG only; render PDF pages or create a supported PNG copy first",
        ));
    }
    let mut input = Vec::with_capacity(bytes.len() + 4);
    input.extend((limit as u32).to_le_bytes());
    input.extend(bytes);
    let mut response = documents::run_worker(ctx, "ocr-worker", &input, 64000, 12).await?;
    let mut data = response["ocr"].take();
    if !data.is_object() {
        return Err(io("OCR worker omitted result"));
    }
    data["sha256"] = json!(hash);
    data["path"] = json!(args.path);
    Ok(ToolOutcome::read(
        "Local OCR text (may contain recognition errors)",
        "OCR text and geometry are in data. Recognition can be wrong; no image was sent to a website.",
        Some(args.path),
    )
    .with_data(data))
}
pub fn worker() -> anyhow::Result<()> {
    for (resource, limit) in [(libc::RLIMIT_CORE, 0), (libc::RLIMIT_CPU, 8)] {
        let bound = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        anyhow::ensure!(
            unsafe { libc::setrlimit(resource, &bound) } == 0,
            "cannot bound OCR worker"
        );
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT + 5)
        .read_to_end(&mut input)?;
    anyhow::ensure!(
        input.len() >= 4 && input.len() as u64 <= MAX_INPUT + 4,
        "invalid OCR input size"
    );
    let limit = u32::from_le_bytes(input[..4].try_into()?) as usize;
    anyhow::ensure!((256..=24000).contains(&limit), "invalid OCR output limit");
    #[cfg(target_os = "macos")]
    let result = macos::process(&input[4..], limit);
    #[cfg(not(target_os = "macos"))]
    let result: Result<Value, ToolFailure> =
        Err(unsupported("Vision OCR is unavailable on this platform"));
    let response = match result {
        Ok(ocr) => json!({"ocr":ocr}),
        Err(e) => json!({"error":e.to_string()}),
    };
    let bytes = serde_json::to_vec(&response)?;
    anyhow::ensure!(bytes.len() <= 64000, "OCR worker output exceeded limit");
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}
