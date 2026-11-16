//! Bounded literal code navigation and compiler diagnostics. No semantic symbol
//! resolution, macro execution, language-server startup or arbitrary commands.
use super::{binary, parse_args, ToolContext, ToolOutcome};
use crate::{
    failure::ToolFailure,
    fsops,
    sandbox::{Access, Decision},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use walkdir::WalkDir;
const FILE_BYTES: u64 = 1024 * 1024;
const TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODES: usize = 5000;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    path: String,
    line: usize,
    sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    cursor: Option<Cursor>,
}
fn invalid(s: &str) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
pub async fn search(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Search = parse_args(args)?;
    if args.query.trim().is_empty() || args.query.len() > 500 {
        return Err(invalid("query must contain 1–500 bytes"));
    }
    let raw = args.path.unwrap_or_else(|| {
        if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
            ctx.agent.workspace_relative()
        } else {
            ".".into()
        }
    });
    let root = ctx.resolve_existing(&raw)?;
    ctx.check_read("search_files", &root)?;
    if !root.is_dir {
        return Err(invalid("search path must be a folder"));
    }
    if let Some(c) = &args.cursor {
        let p = ctx.resolve_existing(&c.path)?;
        ctx.check_read("search_files", &p)?;
        if !p.absolute.starts_with(&root.absolute)
            || c.line == 0
            || c.line > FILE_BYTES as usize + 1
        {
            return Err(invalid(
                "cursor is outside this search root or has an invalid line",
            ));
        }
        if c.line > 1 && c.sha256.is_none() {
            return Err(invalid(
                "continuing within a file requires its returned SHA256",
            ));
        }
    }
    let limit = args.max_results.unwrap_or(30);
    if !(1..=100).contains(&limit) {
        return Err(invalid("max_results must be 1–100"));
    }
    // Compile the unchanged read policy once for this call. Never cache paths
    // or policy across calls/workspace changes.
    let sandbox = ctx.app.sandbox();
    let end = Instant::now() + Duration::from_secs(2);
    let mut files = std::collections::BTreeSet::new();
    let nodes = std::cell::Cell::new(0usize);
    let tree_budget = std::cell::Cell::new(false);
    let mut tree_truncated = false;
    let mut skipped = 0;
    for entry in WalkDir::new(&root.absolute)
        .follow_links(false)
        .max_depth(12)
        .into_iter()
        .filter_entry(|e| {
            nodes.set(nodes.get() + 1);
            if nodes.get() > MAX_NODES || Instant::now() >= end {
                tree_budget.set(true);
                // Yield this entry so the outer loop can stop immediately,
                // even when a directory contains many excluded filenames.
                return true;
            }
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | ".bridge" | "target" | "node_modules" | ".locks" | "__pycache__"
            ) {
                return false;
            }
            e.path()
                .strip_prefix(ctx.workspace_root())
                .ok()
                .and_then(|r| {
                    sandbox
                        .resolve(&r.to_string_lossy(), crate::sandbox::Existence::Any)
                        .ok()
                })
                .is_some_and(|p| sandbox.decide(ctx.agent, Access::Read, &p) == Decision::Allow)
        })
    {
        if tree_budget.get() || Instant::now() >= end {
            tree_truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };
        if entry.file_type().is_dir() && entry.depth() == 12 {
            tree_truncated = true;
        }
        if !entry.file_type().is_file() || entry.file_type().is_symlink() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(ctx.workspace_root())
            .map_err(|_| invalid("search path escaped"))?
            .to_string_lossy()
            .to_string();
        if rel.len() > 2048 {
            skipped += 1;
            continue;
        }
        files.insert(rel);
    }
    if let Some(c) = &args.cursor {
        if !files.contains(&c.path) {
            return Err(ToolFailure::Conflict(
                "cursor file is no longer in the bounded search; start again with a narrower path"
                    .into(),
            ));
        }
    }
    let needle = args.query.to_lowercase();
    let mut hits = vec![];
    let mut scanned = 0;
    let mut read_bytes = 0;
    let mut result_bytes = 0;
    let mut next = None;
    'files: for path in files {
        if args.cursor.as_ref().is_some_and(|c| path < c.path) {
            continue;
        }
        if scanned >= 100 || read_bytes + FILE_BYTES as usize > TOTAL_BYTES || Instant::now() >= end
        {
            next = Some(Cursor {
                path,
                line: 1,
                sha256: None,
            });
            break;
        }
        let read = sandbox
            .resolve(&path, crate::sandbox::Existence::MustExist)
            .and_then(|p| {
                if sandbox.decide(ctx.agent, Access::Read, &p) != Decision::Allow {
                    return Err(ToolFailure::PermissionDenied(
                        "search file is protected".into(),
                    ));
                }
                binary::read_resolved(&p, FILE_BYTES)
            });
        let bytes = match read {
            Ok(b) => b,
            Err(e) => {
                if args.cursor.as_ref().is_some_and(|c| c.path == path) {
                    return Err(e);
                }
                skipped += 1;
                continue;
            }
        };
        read_bytes += bytes.len();
        scanned += 1;
        let hash = fsops::sha256_bytes(&bytes);
        let start = if let Some(c) = args.cursor.as_ref().filter(|c| c.path == path) {
            if c.sha256.as_ref().is_some_and(|h| *h != hash) {
                return Err(ToolFailure::Conflict(
                    "search cursor file changed; start this search again".into(),
                ));
            }
            c.line
        } else {
            1
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            skipped += 1;
            continue;
        };
        for (i, line) in text.lines().enumerate().skip(start - 1) {
            if Instant::now() >= end {
                next = Some(Cursor {
                    path: path.clone(),
                    line: i + 1,
                    sha256: Some(hash.clone()),
                });
                break 'files;
            }
            if line.to_lowercase().contains(&needle) {
                let excerpt: String = line.trim().chars().take(300).collect();
                let hit = json!({"path":path,"line":i+1,"excerpt":excerpt,"excerpt_truncated":line.trim().chars().count()>300,"sha256":hash});
                let size = hit.to_string().len();
                if result_bytes + size > 48000 {
                    next = Some(Cursor {
                        path: path.clone(),
                        line: i + 1,
                        sha256: Some(hash.clone()),
                    });
                    break 'files;
                }
                hits.push(hit);
                result_bytes += size;
                if hits.len() >= limit {
                    next = Some(Cursor {
                        path: path.clone(),
                        line: i + 2,
                        sha256: Some(hash.clone()),
                    });
                    break 'files;
                }
            }
        }
    }
    let output = hits
        .iter()
        .map(|h| {
            format!(
                "{}:{}: {}",
                h["path"].as_str().unwrap(),
                h["line"],
                h["excerpt"].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let truncated = next.is_some() || tree_truncated;
    Ok(ToolOutcome::read(format!("{} literal matches",hits.len()),format!("{}\n{}",if output.is_empty(){"No matches in this bounded page."}else{&output},if tree_truncated{"Tree budget reached; narrow the search folder. A cursor covers only the bounded file set."}else if truncated{"More text may remain; pass next_cursor unchanged as cursor."}else{"Search complete for readable files within the stated bounds."}),Some(root.display())).with_data(json!({"matches":hits,"next_cursor":next,"truncated":truncated,"tree_truncated":tree_truncated,"files_scanned":scanned,"bytes_scanned":read_bytes,"skipped_files":skipped,"mode":"literal_case_insensitive","limits":{"file_bytes":FILE_BYTES,"total_bytes":TOTAL_BYTES,"files":100,"tree_nodes":MAX_NODES,"depth":12,"seconds":2}})))
}

/// Parse only bounded, explicit Cargo/rustc compiler-message objects. A span
/// points into the granted root only; macro expansions are not followed.
pub fn diagnostics(ctx: &ToolContext<'_>, cwd: &std::path::Path, text: &str) -> Value {
    let mut entries = vec![];
    let mut output_bytes = 0;
    let mut truncated = false;
    let mut recognized = 0;
    let mut skipped_spans = 0;
    for line in text.lines().take(2000) {
        if line.len() > 128 * 1024 {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let message = if value["reason"] == "compiler-message" {
            &value["message"]
        } else if value["$message_type"] == "diagnostic" {
            &value
        } else {
            continue;
        };
        let (Some(level), Some(body), Some(spans)) = (
            message["level"].as_str(),
            message["message"].as_str(),
            message["spans"].as_array(),
        ) else {
            continue;
        };
        recognized += 1;
        let mut places = vec![];
        for span in spans.iter().take(32) {
            let Some(raw) = span["file_name"].as_str().filter(|s| s.len() <= 2048) else {
                skipped_spans += 1;
                continue;
            };
            let target = std::path::Path::new(raw);
            let absolute = if target.is_absolute() {
                target.to_path_buf()
            } else {
                cwd.join(target)
            };
            let Ok(relative) = absolute.strip_prefix(ctx.workspace_root()) else {
                skipped_spans += 1;
                continue;
            };
            let Ok(path) = ctx.resolve_existing(&relative.to_string_lossy()) else {
                skipped_spans += 1;
                continue;
            };
            if ctx.app.decide(ctx.agent, Access::Read, &path) != Decision::Allow || path.is_dir {
                skipped_spans += 1;
                continue;
            }
            let (Some(line), Some(column)) =
                (span["line_start"].as_u64(), span["column_start"].as_u64())
            else {
                skipped_spans += 1;
                continue;
            };
            if line == 0 || column == 0 || line > 10_000_000 || column > 10_000_000 {
                skipped_spans += 1;
                continue;
            }
            places.push(json!({"path":path.display(),"line":line,"column":column,"primary":span["is_primary"].as_bool().unwrap_or(false)}));
        }
        let entry = json!({"level":level.chars().take(40).collect::<String>(),"code":message["code"]["code"].as_str().map(|s|s.chars().take(80).collect::<String>()),"message":body.chars().take(500).collect::<String>(),"locations":places});
        output_bytes += entry.to_string().len();
        if entries.len() >= 100 || output_bytes > 24000 {
            truncated = true;
            break;
        }
        entries.push(entry);
    }
    if text.lines().count() > 2000 {
        truncated = true;
    }
    json!({"format":"cargo_rustc_json","diagnostics":entries,"recognized":recognized,"skipped_spans":skipped_spans,"truncated":truncated,"note":"Compiler-reported locations only. No semantic reference resolution. Plain text and unsupported compiler formats remain available as original output."})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Symbols {
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    expected_sha256: Option<String>,
}
static PARSERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
pub async fn symbols(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let args: Symbols = parse_args(args)?;
    let p = ctx.resolve_existing(&args.path)?;
    ctx.check_read("list_symbols", &p)?;
    if p.absolute.extension().and_then(|s| s.to_str()) != Some("rs") {
        return Err(ToolFailure::Unsupported("list_symbols currently parses Rust .rs syntax only; use search_files for other languages or reference candidates".into()));
    }
    let limit = args.max_results.unwrap_or(50);
    if !(1..=100).contains(&limit) || args.offset > 1000 {
        return Err(invalid("symbol limit must be 1–100 and offset 0–1000"));
    }
    if args.offset > 0 && args.expected_sha256.is_none() {
        return Err(invalid(
            "symbol continuation requires expected_sha256 from the previous page",
        ));
    }
    let bytes = binary::read_source(ctx, "list_symbols", &args.path, FILE_BYTES)?;
    let hash = fsops::sha256_bytes(&bytes);
    if args.expected_sha256.as_ref().is_some_and(|h| h != &hash) {
        return Err(ToolFailure::Conflict(
            "symbol file changed; inspect from offset zero again".into(),
        ));
    }
    std::str::from_utf8(&bytes).map_err(|_| invalid("Rust source must be UTF-8"))?;
    let _slot = PARSERS
        .try_acquire()
        .map_err(|_| ToolFailure::Conflict("both Rust parser slots are busy".into()))?;
    let current = std::env::current_exe().map_err(|e| ToolFailure::Io(e.to_string()))?;
    let executable = if current
        .file_name()
        .is_some_and(|s| s == "unified-ai-bridge-native")
    {
        current
    } else {
        ctx.app.home.installed_binary()
    };
    let mut child = tokio::process::Command::new(executable)
        .arg("rust-symbol-worker")
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolFailure::Unsupported(format!("Rust parser helper unavailable: {e}")))?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| invalid("parser stdin unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| invalid("parser stdout unavailable"))?;
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        input.write_all(&bytes).await?;
        input.shutdown().await?;
        drop(input);
        let mut raw = vec![];
        output.take(512 * 1024 + 1).read_to_end(&mut raw).await?;
        if raw.len() > 512 * 1024 {
            return Err(std::io::Error::other("Rust parser output exceeded limit"));
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(std::io::Error::other(
                "Rust parser exited unsuccessfully; source may exceed parser limits",
            ));
        }
        Ok(raw)
    })
    .await;
    let raw = match result {
        Ok(Ok(raw)) => raw,
        other => {
            let _ = child.kill().await;
            return Err(ToolFailure::Unsupported(format!("Rust syntax parser failed or exceeded its three-second budget: {other:?}. Use bounded text search.")));
        }
    };
    let worker: Value =
        serde_json::from_slice(&raw).map_err(|_| invalid("invalid Rust parser result"))?;
    if let Some(error) = worker["error"].as_str() {
        return Err(ToolFailure::Unsupported(format!(
            "Rust syntax parse failed: {error}; use search_files while the file is incomplete"
        )));
    }
    let all = worker["symbols"]
        .as_array()
        .ok_or_else(|| invalid("missing parser symbols"))?;
    let mut entries = vec![];
    let mut total = 0;
    for item in all.iter().skip(args.offset).take(limit) {
        let mut item = item.clone();
        item["path"] = json!(p.display());
        let size = item.to_string().len();
        if total + size > 24000 {
            break;
        }
        total += size;
        entries.push(item);
    }
    let next = args.offset + entries.len();
    let next_offset = (next < all.len()).then_some(next);
    Ok(ToolOutcome::read(format!("{} Rust syntax declarations",entries.len()),"Syntax only: includes cfg-disabled declarations, does not expand macros, follow external modules, resolve names/types, index function-local declarations, or prove references. Use search_files for literal reference candidates.",Some(p.display())).with_data(json!({"language":"rust","mode":"syntax_declarations","sha256":hash,"symbols":entries,"next_offset":next_offset,"parser_truncated":worker["truncated"],"truncated":next_offset.is_some()||worker["truncated"]==true})))
}

/// Internal bounded worker, isolated so parser stack/resource failures cannot
/// kill the Native Messaging connection. No config, files or macros are loaded.
pub fn rust_symbol_worker() -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let cpu = libc::rlimit {
        rlim_cur: 2,
        rlim_max: 2,
    };
    // SAFETY: resource structs live for these process-local calls.
    anyhow::ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) } == 0,
        "could not disable parser core dumps"
    );
    anyhow::ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } == 0,
        "could not bound parser CPU"
    );
    let mut bytes = vec![];
    std::io::stdin()
        .take(FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() as u64 <= FILE_BYTES, "source exceeds 1 MiB");
    let text = std::str::from_utf8(&bytes)?;
    let result = match syn::parse_file(text) {
        Ok(file) => {
            let mut out = vec![];
            let mut truncated = false;
            declarations(&file.items, "", 0, &mut out, &mut truncated);
            json!({"symbols":out,"truncated":truncated})
        }
        Err(e) => {
            json!({"error":format!("line {}, column {}: {}",e.span().start().line,e.span().start().column+1,e)})
        }
    };
    std::io::stdout().write_all(&serde_json::to_vec(&result)?)?;
    Ok(())
}
fn declaration(
    out: &mut Vec<Value>,
    truncated: &mut bool,
    kind: &str,
    id: &syn::Ident,
    scope: &str,
) {
    if out.len() >= 1000 {
        *truncated = true;
        return;
    }
    let start = id.span().start();
    let name = id.to_string();
    out.push(json!({"name":name.chars().take(128).collect::<String>(),"name_truncated":name.chars().count()>128,"kind":kind,"scope":scope.chars().take(256).collect::<String>(),"line":start.line,"column":start.column+1}));
}
fn declarations(
    items: &[syn::Item],
    scope: &str,
    depth: usize,
    out: &mut Vec<Value>,
    truncated: &mut bool,
) {
    if depth > 32 {
        *truncated = true;
        return;
    }
    for item in items {
        if out.len() >= 1000 {
            *truncated = true;
            break;
        }
        use syn::Item;
        match item {
            Item::Fn(v) => declaration(out, truncated, "function", &v.sig.ident, scope),
            Item::Struct(v) => declaration(out, truncated, "struct", &v.ident, scope),
            Item::Enum(v) => {
                declaration(out, truncated, "enum", &v.ident, scope);
                for child in &v.variants {
                    declaration(
                        out,
                        truncated,
                        "variant",
                        &child.ident,
                        &format!("{scope}::{}", v.ident),
                    );
                }
            }
            Item::Union(v) => declaration(out, truncated, "union", &v.ident, scope),
            Item::Type(v) => declaration(out, truncated, "type", &v.ident, scope),
            Item::Const(v) => declaration(out, truncated, "const", &v.ident, scope),
            Item::Static(v) => declaration(out, truncated, "static", &v.ident, scope),
            Item::Mod(v) => {
                declaration(out, truncated, "module", &v.ident, scope);
                if let Some((_, items)) = &v.content {
                    declarations(
                        items,
                        &format!("{scope}::{}", v.ident),
                        depth + 1,
                        out,
                        truncated,
                    );
                }
            }
            Item::Trait(v) => {
                declaration(out, truncated, "trait", &v.ident, scope);
                for child in &v.items {
                    match child {
                        syn::TraitItem::Fn(m) => declaration(
                            out,
                            truncated,
                            "trait_method",
                            &m.sig.ident,
                            &format!("{scope}::{}", v.ident),
                        ),
                        syn::TraitItem::Type(m) => declaration(
                            out,
                            truncated,
                            "associated_type",
                            &m.ident,
                            &format!("{scope}::{}", v.ident),
                        ),
                        syn::TraitItem::Const(m) => declaration(
                            out,
                            truncated,
                            "associated_const",
                            &m.ident,
                            &format!("{scope}::{}", v.ident),
                        ),
                        _ => {}
                    }
                }
            }
            Item::Impl(v) => {
                let location = if let syn::Type::Path(p) = v.self_ty.as_ref() {
                    p.path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::")
                } else {
                    "<impl type>".into()
                };
                for child in &v.items {
                    match child {
                        syn::ImplItem::Fn(m) => declaration(
                            out,
                            truncated,
                            "method",
                            &m.sig.ident,
                            &format!("{scope}::{location}"),
                        ),
                        syn::ImplItem::Type(m) => declaration(
                            out,
                            truncated,
                            "associated_type",
                            &m.ident,
                            &format!("{scope}::{location}"),
                        ),
                        syn::ImplItem::Const(m) => declaration(
                            out,
                            truncated,
                            "associated_const",
                            &m.ident,
                            &format!("{scope}::{location}"),
                        ),
                        _ => {}
                    }
                }
            }
            Item::Macro(v) => {
                if let Some(id) = &v.ident {
                    declaration(out, truncated, "macro", id, scope);
                }
            }
            _ => {}
        }
    }
}
