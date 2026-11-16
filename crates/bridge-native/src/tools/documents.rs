//! Read a declared DOCX text subset in a disposable native parser worker.
//! Package entries never reach the filesystem; external references are inert.
use super::{archives, binary, parse_args, ToolContext, ToolOutcome};
use crate::{failure::ToolFailure, fsops};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Cursor, Read, Write};
const MAX_INPUT: u64 = 16 * 1024 * 1024;
const MAX_XML: u64 = 2 * 1024 * 1024;
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
static PARSERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extract {
    path: String,
    #[serde(default)]
    offset: usize,
    max_bytes: Option<usize>,
    expected_sha256: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Parsed {
    text: String,
    paragraphs: usize,
    tables: usize,
    drawings: usize,
    explicit_page_breaks: usize,
    tracked_changes: bool,
    fields: bool,
    alternate_content_skipped: bool,
    package_entries: usize,
    omitted_parts: Vec<String>,
}
fn xml(text: &str) -> Result<roxmltree::Document<'_>, ToolFailure> {
    if text.contains("<!DOCTYPE") {
        return Err(unsupported("DTD declarations are refused"));
    }
    let document = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 50000,
            ..Default::default()
        },
    )
    .map_err(|e| invalid(format!("unsupported or malformed document XML: {e}")))?;
    for node in document.descendants() {
        if node.ancestors().take(129).count() > 128 {
            return Err(unsupported("document XML nesting exceeds 128 levels"));
        }
    }
    Ok(document)
}
fn part(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    limit: u64,
) -> Result<String, ToolFailure> {
    let file = archive
        .by_name(name)
        .map_err(|e| invalid(format!("required DOCX part {name} unavailable: {e}")))?;
    if file.size() > limit {
        return Err(unsupported(format!(
            "DOCX part {name} exceeds {limit} bytes"
        )));
    }
    let mut data = vec![];
    file.take(limit + 1).read_to_end(&mut data).map_err(io)?;
    if data.len() as u64 > limit {
        return Err(unsupported("expanded DOCX part exceeds its budget"));
    }
    String::from_utf8(data).map_err(|_| unsupported("DOCX XML parts must be UTF-8 in this subset"))
}
fn parse(bytes: &[u8]) -> Result<Parsed, ToolFailure> {
    let (mut archive, plan) = archives::inspect(bytes)?;
    if plan.expanded > 32 * 1024 * 1024 {
        return Err(unsupported("DOCX declared expansion exceeds 32 MiB"));
    }
    let mut omitted = vec![];
    for entry in &plan.entries {
        let name = entry.path.to_ascii_lowercase();
        if name.contains("vbaproject") || name.contains("vba_data") {
            return Err(unsupported("macro-bearing DOCX packages are refused"));
        }
        if name.starts_with("word/header")
            || name.starts_with("word/footer")
            || name.starts_with("word/footnotes")
            || name.starts_with("word/endnotes")
            || name.starts_with("word/comments")
            || name.starts_with("word/embeddings/")
        {
            omitted.push(entry.path.clone());
        }
    }
    let types = part(&mut archive, "[Content_Types].xml", 128 * 1024)?;
    if types.to_ascii_lowercase().contains("macroenabled")
        || types.to_ascii_lowercase().contains("vba")
    {
        return Err(unsupported("macro-bearing document types are refused"));
    }
    let content_types = xml(&types)?;
    if !content_types.descendants().any(|n|n.has_tag_name(("http://schemas.openxmlformats.org/package/2006/content-types","Override"))&&n.attribute("PartName")==Some("/word/document.xml")&&n.attribute("ContentType")==Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")){
        return Err(unsupported("requires a non-macro DOCX main part at word/document.xml"));
    }
    let body = part(&mut archive, "word/document.xml", MAX_XML)?;
    let document = xml(&body)?;
    if !document.root_element().has_tag_name((W, "document")) {
        return Err(unsupported(
            "unsupported WordprocessingML namespace or document root",
        ));
    }
    let body = document
        .root_element()
        .children()
        .find(|n| n.has_tag_name((W, "body")))
        .ok_or_else(|| invalid("DOCX body is missing"))?;
    let skip = |n: roxmltree::Node<'_, '_>| {
        n.ancestors().any(|a| {
            a.has_tag_name((W, "del"))
                || a.has_tag_name((W, "moveFrom"))
                || a.has_tag_name((MC, "AlternateContent"))
        })
    };
    let mut result = Parsed {
        text: String::new(),
        paragraphs: 0,
        tables: 0,
        drawings: 0,
        explicit_page_breaks: 0,
        tracked_changes: false,
        fields: false,
        alternate_content_skipped: false,
        package_entries: plan.entries.len(),
        omitted_parts: omitted,
    };
    for node in body.descendants().filter(|n| n.is_element()) {
        result.tracked_changes |= node.has_tag_name((W, "ins"))
            || node.has_tag_name((W, "del"))
            || node.has_tag_name((W, "moveFrom"))
            || node.has_tag_name((W, "moveTo"));
        result.fields |= node.has_tag_name((W, "fldSimple")) || node.has_tag_name((W, "fldChar"));
        result.alternate_content_skipped |= node.has_tag_name((MC, "AlternateContent"));
        if skip(node) {
            continue;
        }
        if node.has_tag_name((W, "tbl")) {
            result.tables += 1;
        }
        if node.has_tag_name((W, "drawing")) || node.has_tag_name((W, "pict")) {
            result.drawings += 1;
        }
        if node.has_tag_name((W, "br")) && node.attribute((W, "type")) == Some("page") {
            result.explicit_page_breaks += 1;
        }
        if !node.has_tag_name((W, "p")) {
            continue;
        }
        if result.paragraphs >= 5000 {
            return Err(unsupported("DOCX exceeds 5,000 body paragraphs"));
        }
        result.paragraphs += 1;
        for child in node.descendants().skip(1).filter(|n| n.is_element()) {
            if skip(child)
                || child.ancestors().skip(1).find(|a| a.has_tag_name((W, "p"))) != Some(node)
            {
                continue;
            }
            if child.has_tag_name((W, "t")) {
                result.text.push_str(child.text().unwrap_or_default());
            } else if child.has_tag_name((W, "tab")) {
                result.text.push('\t');
            } else if child.has_tag_name((W, "br")) || child.has_tag_name((W, "cr")) {
                result.text.push('\n');
            }
            if result.text.len() > 256000 {
                return Err(unsupported("DOCX body text exceeds 256,000 bytes"));
            }
        }
        result.text.push('\n');
    }
    if result.text.len() > 256000 {
        return Err(unsupported("DOCX body text exceeds 256,000 bytes"));
    }
    Ok(result)
}
pub fn worker() -> anyhow::Result<()> {
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let cpu = libc::rlimit {
        rlim_cur: 2,
        rlim_max: 2,
    };
    anyhow::ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) } == 0,
        "cannot disable core dumps"
    );
    anyhow::ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } == 0,
        "cannot bound parser CPU"
    );
    let mut bytes = vec![];
    std::io::stdin()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_INPUT,
        "document input exceeded 16 MiB"
    );
    let result = match parse(&bytes) {
        Ok(parsed) => json!({"document":parsed}),
        Err(error) => json!({"error":error.to_string()}),
    };
    std::io::stdout().write_all(&serde_json::to_vec(&result)?)?;
    Ok(())
}
async fn load(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<(Parsed, String), ToolFailure> {
    let p = ctx.resolve_existing(path)?;
    ctx.check_read(tool, &p)?;
    if p.absolute
        .extension()
        .and_then(|s| s.to_str())
        .is_none_or(|s| !s.eq_ignore_ascii_case("docx"))
    {
        return Err(unsupported("this reader supports DOCX body text and PDF on macOS; DOC, DOCM and OCR are unavailable"));
    }
    let bytes = binary::read_source(ctx, tool, path, MAX_INPUT)?;
    let hash = fsops::sha256_bytes(&bytes);
    if expected.is_some_and(|h| h != hash) {
        return Err(ToolFailure::Conflict(
            "document changed; restart extraction at offset zero".into(),
        ));
    }
    let mut value = run_worker(ctx, "docx-worker", &bytes, 2 * 1024 * 1024, 4).await?;
    let parsed: Parsed = serde_json::from_value(value["document"].take()).map_err(io)?;
    Ok((parsed, hash))
}
pub(super) async fn run_worker(
    ctx: &ToolContext<'_>,
    worker: &str,
    bytes: &[u8],
    output_limit: u64,
    seconds: u64,
) -> Result<Value, ToolFailure> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _slot = PARSERS
        .try_acquire()
        .map_err(|_| ToolFailure::Conflict("both document parser slots are busy".into()))?;
    let current = std::env::current_exe().map_err(io)?;
    let exe = if current
        .file_name()
        .is_some_and(|s| s == "unified-ai-bridge-native")
    {
        current
    } else {
        ctx.app.home.installed_binary()
    };
    let mut child = tokio::process::Command::new(exe)
        .arg(worker)
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| unsupported(format!("document parser unavailable: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io("document parser stdin missing"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io("document parser stdout missing"))?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(seconds), async {
        let feed = async {
            stdin.write_all(&bytes).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        };
        let read = async {
            let mut data = vec![];
            stdout.take(output_limit + 1).read_to_end(&mut data).await?;
            if data.len() as u64 > output_limit {
                return Err(std::io::Error::other("document output exceeded budget"));
            }
            Ok(data)
        };
        let (_, raw) = tokio::try_join!(feed, read)?;
        if !child.wait().await?.success() {
            return Err(std::io::Error::other(
                "document parser exited unsuccessfully",
            ));
        }
        Ok(raw)
    })
    .await;
    let raw = match result {
        Ok(Ok(raw)) => raw,
        other => {
            let _ = child.kill().await;
            return Err(unsupported(format!(
                "document parser failed or exceeded {seconds}-second budget: {other:?}"
            )));
        }
    };
    let value: Value = serde_json::from_slice(&raw).map_err(io)?;
    if let Some(error) = value["error"].as_str() {
        return Err(unsupported(format!("document parse refused: {error}")));
    }
    Ok(value)
}
fn metadata(parsed: &Parsed, hash: &str) -> Value {
    json!({"format":"docx","sha256":hash,"text_bytes":parsed.text.len(),"body_paragraphs":parsed.paragraphs,"tables":parsed.tables,"drawings":parsed.drawings,"explicit_page_breaks":parsed.explicit_page_breaks,"page_count":null,"tracked_changes":parsed.tracked_changes,"cached_fields":parsed.fields,"alternate_content_skipped":parsed.alternate_content_skipped,"package_entries":parsed.package_entries,"omitted_parts":parsed.omitted_parts,"ocr":"separate optional ocr_image tool for PNG on macOS","page_rendering":"unavailable","text_scope":"Main body only; table cells become paragraphs. Insertions included, deletions/move-from/alternate-content omitted. Field results are cached text, not evaluated. Formatting, hidden-text visibility and layout are not reproduced. Headers, footers, notes, comments, embedded objects and images are not extracted. Page breaks are not page count. No relationship target is opened."})
}
pub async fn info(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: PathArgs = parse_args(args)?;
    if super::pdf::is_pdf(&args.path) {
        return super::pdf::info(ctx, &args.path).await;
    }
    let (parsed, hash) = load(ctx, "document_info", &args.path, None).await?;
    let data = metadata(&parsed, &hash);
    Ok(
        ToolOutcome::read("DOCX body information", data.to_string(), Some(args.path))
            .with_data(data),
    )
}
pub async fn extract(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Extract = parse_args(args)?;
    let limit = args.max_bytes.unwrap_or(12000);
    if !(256..=24000).contains(&limit) {
        return Err(invalid("max_bytes must be 256–24,000"));
    }
    if args.offset > 0 && args.expected_sha256.is_none() {
        return Err(invalid(
            "continuation requires expected_sha256 from the first result",
        ));
    }
    if super::pdf::is_pdf(&args.path) {
        return super::pdf::extract(
            ctx,
            &args.path,
            args.offset,
            limit,
            args.expected_sha256.as_deref(),
        )
        .await;
    }
    let (parsed, hash) = load(
        ctx,
        "extract_document_text",
        &args.path,
        args.expected_sha256.as_deref(),
    )
    .await?;
    if args.offset > parsed.text.len() || !parsed.text.is_char_boundary(args.offset) {
        return Err(invalid("offset must be a returned UTF-8 byte boundary"));
    }
    let mut end = (args.offset + limit).min(parsed.text.len());
    while !parsed.text.is_char_boundary(end) {
        end -= 1;
    }
    let mut data = metadata(&parsed, &hash);
    data["offset"] = json!(args.offset);
    loop {
        let next = (end < parsed.text.len()).then_some(end);
        data["next_offset"] = json!(next);
        data["truncated"] = json!(next.is_some());
        data["text"] = json!(&parsed.text[args.offset..end]);
        if data.to_string().len() <= 48000 {
            break;
        }
        end = args.offset + (end - args.offset) / 2;
        while !parsed.text.is_char_boundary(end) {
            end -= 1;
        }
        if end == args.offset {
            return Err(unsupported("document metadata exceeds response budget"));
        }
    }
    Ok(
        ToolOutcome::read("DOCX body text chunk", data.to_string(), Some(args.path))
            .with_data(data),
    )
}
