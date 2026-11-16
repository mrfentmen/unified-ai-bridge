//! PDFKit text and CoreGraphics page-content rendering in a disposable Mac
//! worker. No PDFView, action execution, URL loading, password or OCR workflow.
use super::{
    artifacts::directory,
    binary::{self, Entry, Stage},
    documents, parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
    sandbox::Access,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
mod macos;
const MAX_INPUT: u64 = 16 * 1024 * 1024;
const MAX_OUTPUT: u64 = 24 * 1024 * 1024;
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
pub(super) fn is_pdf(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Info,
    Text,
    Render {
        first_page: usize,
        last_page: usize,
        max_dimension: u32,
    },
}
#[derive(Serialize, Deserialize)]
struct PageText {
    page: usize,
    start: usize,
    end: usize,
}
#[derive(Serialize, Deserialize)]
struct Text {
    text: String,
    pages: Vec<PageText>,
}
#[derive(Serialize, Deserialize)]
struct PageImage {
    page: usize,
    width: u32,
    height: u32,
    png: String,
}
fn available() -> Result<(), ToolFailure> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(unsupported("PDF text and rendering require macOS PDFKit/CoreGraphics; this host has no PDF backend"))
    }
}
fn source(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<(Vec<u8>, String), ToolFailure> {
    // Check paths even on unsupported systems, without granting arbitrary reads.
    let p = ctx.resolve_existing(path)?;
    ctx.check_read(tool, &p)?;
    available()?;
    if !is_pdf(path) {
        return Err(unsupported(
            "page rendering currently supports PDF only; DOCX layout rendering is unavailable",
        ));
    }
    let bytes = binary::read_resolved(&p, MAX_INPUT)?;
    let hash = fsops::sha256_bytes(&bytes);
    if expected.is_some_and(|h| h != hash) {
        return Err(ToolFailure::Conflict(
            "document changed; inspect it again and restart extraction".into(),
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(unsupported("not a supported PDF header"));
    }
    Ok((bytes, hash))
}
async fn run(ctx: &ToolContext<'_>, bytes: &[u8], request: Request) -> Result<Value, ToolFailure> {
    let header = serde_json::to_vec(&request).map_err(io)?;
    let mut input = Vec::with_capacity(bytes.len() + header.len() + 4);
    input.extend_from_slice(&(header.len() as u32).to_le_bytes());
    input.extend_from_slice(&header);
    input.extend_from_slice(bytes);
    documents::run_worker(ctx, "pdf-worker", &input, MAX_OUTPUT, 10).await
}
fn metadata(count: &Value, hash: &str) -> Value {
    json!({"format":"pdf","sha256":hash,"page_count":count,"backend":"macOS PDFKit/CoreGraphics","encrypted":false,"ocr":"separate optional ocr_image tool for PNG on macOS","page_rendering":"approved PNG page-content previews","text_scope":"Embedded text in physical page order; reading order and hidden-text visibility may differ from the visible page. No OCR or semantic layout. An empty text layer may mean scans, outlines, blank pages or unsupported encoding. Links, actions, JavaScript, attachments and external references are never opened. Rendered previews omit annotations and interactive form appearances."})
}
pub(super) async fn info(ctx: &ToolContext<'_>, path: &str) -> Result<ToolOutcome, ToolFailure> {
    let (bytes, hash) = source(ctx, "document_info", path, None)?;
    let response = run(ctx, &bytes, Request::Info).await?;
    let mut data = metadata(&response["page_count"], &hash);
    data["pages"] = response["pages"].clone();
    data["text_layer"] = json!("not inspected; use extract_document_text");
    Ok(ToolOutcome::read("PDF information", data.to_string(), Some(path.into())).with_data(data))
}
pub(super) async fn extract(
    ctx: &ToolContext<'_>,
    path: &str,
    offset: usize,
    limit: usize,
    expected: Option<&str>,
) -> Result<ToolOutcome, ToolFailure> {
    let (bytes, hash) = source(ctx, "extract_document_text", path, expected)?;
    let mut response = run(ctx, &bytes, Request::Text).await?;
    let parsed: Text = serde_json::from_value(response["document"].take()).map_err(io)?;
    if offset > parsed.text.len() || !parsed.text.is_char_boundary(offset) {
        return Err(invalid("offset must be a returned UTF-8 byte boundary"));
    }
    let mut end = offset.saturating_add(limit).min(parsed.text.len());
    let mut data = metadata(&response["page_count"], &hash);
    data["offset"] = json!(offset);
    data["text_bytes"] = json!(parsed.text.len());
    data["pages_without_text"] = json!(parsed
        .pages
        .iter()
        .filter(|p| p.start == p.end)
        .map(|p| p.page)
        .collect::<Vec<_>>());
    loop {
        while !parsed.text.is_char_boundary(end) {
            end -= 1;
        }
        data["text"] = json!(&parsed.text[offset..end]);
        data["page_ranges"] = json!(parsed.pages.iter().filter(|p|p.end>offset&&p.start<end).map(|p|json!({"page":p.page,"start":p.start.max(offset)-offset,"end":p.end.min(end)-offset})).collect::<Vec<_>>());
        data["next_offset"] = json!((end < parsed.text.len()).then_some(end));
        data["truncated"] = json!(end < parsed.text.len());
        if data.to_string().len() <= 48000 {
            break;
        }
        end = offset + (end - offset) / 2;
        if end == offset {
            return Err(unsupported("PDF metadata exceeds response budget"));
        }
    }
    Ok(ToolOutcome::read(
        "PDF text chunk with page references",
        data.to_string(),
        Some(path.into()),
    )
    .with_data(data))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Render {
    path: String,
    destination: String,
    expected_sha256: String,
    first_page: usize,
    last_page: Option<usize>,
    max_dimension: Option<u32>,
}
fn page_name(page: usize) -> String {
    format!("page-{page:04}.png")
}
fn output_policy(
    ctx: &ToolContext<'_>,
    destination: &str,
    first: usize,
    last: usize,
) -> Result<(), ToolFailure> {
    for page in first..=last {
        let p = ctx.resolve_any(&format!("{destination}/{}", page_name(page)))?;
        ctx.check_read("render_document_pages", &p)?;
        ctx.recheck_access(Access::Write, &p, true)?;
    }
    Ok(())
}
pub async fn render(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Render = parse_args(args)?;
    let first = args.first_page;
    let last = args.last_page.unwrap_or(first);
    let dimension = args.max_dimension.unwrap_or(1200);
    if first == 0
        || last < first
        || last - first >= 4
        || last > 200
        || !(128..=1600).contains(&dimension)
    {
        return Err(invalid(
            "render 1–4 physical pages numbered 1–200; max_dimension must be 128–1600",
        ));
    }
    let (bytes, hash) = source(
        ctx,
        "render_document_pages",
        &args.path,
        Some(&args.expected_sha256),
    )?;
    let initial = binary::target(ctx, "render_document_pages", &args.destination)?;
    output_policy(ctx, &initial.display(), first, last)?;
    let meta = run(ctx, &bytes, Request::Info).await?;
    if last > meta["page_count"].as_u64().unwrap_or(0) as usize {
        return Err(invalid("page range extends beyond this PDF"));
    }
    let description=format!("Render PDF {} (SHA256 {}) physical pages {}–{} into new folder {}. Maximum {} pixels per side. Original preserved; no overwrites, annotation/form appearances or image upload. Up to 16 MiB of PNG output.",args.path,hash,first,last,initial.display(),dimension);
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "render_document_pages",
            &description,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let destination = binary::target(ctx, "render_document_pages", &args.destination)?;
    output_policy(ctx, &destination.display(), first, last)?;
    let (bytes, _) = source(ctx, "render_document_pages", &args.path, Some(&hash))?;
    let mut result = run(
        ctx,
        &bytes,
        Request::Render {
            first_page: first,
            last_page: last,
            max_dimension: dimension,
        },
    )
    .await?;
    let pages: Vec<PageImage> = serde_json::from_value(result["images"].take()).map_err(io)?;
    if pages.len() != last - first + 1 {
        return Err(io("PDF worker returned incomplete pages"));
    }
    let mut output = Vec::new();
    let mut total = 0usize;
    for (index, page) in pages.into_iter().enumerate() {
        if page.page != first + index
            || page.width == 0
            || page.height == 0
            || page.width > dimension
            || page.height > dimension
        {
            return Err(io("PDF worker returned invalid page metadata"));
        }
        let png = STANDARD.decode(&page.png).map_err(io)?;
        total += png.len();
        if total > 16 * 1024 * 1024 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(io("PDF worker output exceeded PNG budget or is invalid"));
        }
        output.push((page, png));
    }
    // Recheck after rendering as policy and source may have changed while the worker ran.
    source(ctx, "render_document_pages", &args.path, Some(&hash))?;
    binary::target(ctx, "render_document_pages", &args.destination)?;
    output_policy(ctx, &destination.display(), first, last)?;
    let mut stage = Stage::new(directory(destination.absolute.parent().unwrap())?, true)?;
    let mut receipt = json!({"source":args.path,"source_sha256":hash,"destination":destination.display(),"first_page":first,"last_page":last,"bytes":total,"state":"prepared","staging_name":stage.name.to_string_lossy(),"pages":[]});
    let staged = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "pdf.prepare",
                Some(destination.display()),
                "allowed",
                &receipt.to_string(),
            )
            .map_err(io)?;
        let end = Instant::now() + Duration::from_secs(10);
        let mut entries = Vec::new();
        for (page, png) in &output {
            let name = page_name(page.page);
            let hash = stage.write_entry(
                &Entry {
                    path: name.clone(),
                    directory: false,
                    bytes: png.len() as u64,
                },
                &mut png.as_slice(),
                end,
            )?;
            entries.push(json!({"page":page.page,"path":format!("{}/{name}",destination.display()),"width":page.width,"height":page.height,"sha256":hash,"bytes":png.len()}));
        }
        receipt["pages"] = json!(entries);
        binary::target(ctx, "render_document_pages", &args.destination)?;
        output_policy(ctx, &destination.display(), first, last)?;
        stage.publish(&destination.absolute)
    })();
    binary::staged_result(
        ctx,
        "render_document_pages",
        &destination.display(),
        &mut stage,
        staged,
    )?;
    receipt["state"] = json!("completed");
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.render_document_pages",
            Some(destination.display()),
            "allowed",
            &receipt.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "PDF pages published but audit finalization failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write("PDF previews saved","Page-content previews omit annotations/forms. Original preserved; no image has been attached to a chat. OCR is a separate optional ocr_image call on a saved PNG.",Some(destination.display())).with_data(receipt))
}
pub fn worker() -> anyhow::Result<()> {
    for (resource, limit) in [(libc::RLIMIT_CORE, 0), (libc::RLIMIT_CPU, 6)] {
        let bound = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        anyhow::ensure!(
            unsafe { libc::setrlimit(resource, &bound) } == 0,
            "cannot bound PDF worker"
        );
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT + 1025)
        .read_to_end(&mut input)?;
    anyhow::ensure!(
        input.len() >= 4 && input.len() as u64 <= MAX_INPUT + 1024,
        "PDF worker input budget exceeded"
    );
    let len = u32::from_le_bytes(input[..4].try_into()?) as usize;
    anyhow::ensure!(
        len <= 1020 && 4 + len <= input.len(),
        "invalid PDF request header"
    );
    let request: Request = serde_json::from_slice(&input[4..4 + len])?;
    let bytes = &input[4 + len..];
    anyhow::ensure!(bytes.len() as u64 <= MAX_INPUT, "PDF exceeds 16 MiB");
    #[cfg(target_os = "macos")]
    let result = macos::process(bytes, request);
    #[cfg(not(target_os = "macos"))]
    let result: Result<Value, ToolFailure> = {
        let _ = (bytes, request);
        Err(unsupported("PDF backend unavailable on this OS"))
    };
    let response = match result {
        Ok(v) => v,
        Err(e) => json!({"error":e.to_string()}),
    };
    let raw = serde_json::to_vec(&response)?;
    anyhow::ensure!(
        raw.len() as u64 <= MAX_OUTPUT,
        "PDF worker output exceeded budget"
    );
    std::io::stdout().write_all(&raw)?;
    Ok(())
}
