//! Bounded XLSX reads and explicitly approved create-only literal-cell edits.
//! Formula execution, macro execution, links and full spreadsheet fidelity are
//! outside this declared subset. Existing workbooks are never replaced.
mod parser;
use super::{
    artifacts::directory,
    binary::{self, Stage},
    documents, parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Write};
const MAX_INPUT: u64 = 16 * 1024 * 1024;
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Inspect,
    Read {
        sheet: String,
        range: String,
    },
    Edit {
        sheet: String,
        range: String,
        values: Vec<Vec<Value>>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRange {
    path: String,
    sheet: String,
    range: String,
    expected_sha256: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    path: String,
    destination: String,
    sheet: String,
    range: String,
    values: Vec<Vec<Value>>,
    expected_sha256: String,
}
fn xlsx(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("xlsx"))
}
fn source(
    ctx: &ToolContext<'_>,
    tool: &str,
    path: &str,
    expected: Option<&str>,
) -> Result<(Vec<u8>, String), ToolFailure> {
    let p = ctx.resolve_existing(path)?;
    ctx.check_read(tool, &p)?;
    if !xlsx(path) {
        return Err(unsupported("workbook tools support a non-macro XLSX subset only; XLS/XLSB/XLSM/ODS are unavailable"));
    }
    let bytes = binary::read_resolved(&p, MAX_INPUT)?;
    let hash = fsops::sha256_bytes(&bytes);
    if expected.is_some_and(|h| h != hash) {
        return Err(ToolFailure::Conflict(
            "workbook changed; inspect it again".into(),
        ));
    }
    Ok((bytes, hash))
}
async fn run(ctx: &ToolContext<'_>, bytes: &[u8], request: Request) -> Result<Value, ToolFailure> {
    let header = serde_json::to_vec(&request).map_err(io)?;
    if header.len() > 48000 {
        return Err(invalid("workbook request exceeds 48,000 bytes"));
    }
    let mut input = Vec::with_capacity(4 + header.len() + bytes.len());
    input.extend_from_slice(&(header.len() as u32).to_le_bytes());
    input.extend_from_slice(&header);
    input.extend_from_slice(bytes);
    documents::run_worker(ctx, "xlsx-worker", &input, 24 * 1024 * 1024, 8).await
}
pub async fn inspect(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Inspect = parse_args(args)?;
    let (bytes, hash) = source(ctx, "inspect_workbook", &args.path, None)?;
    let mut data = run(ctx, &bytes, Request::Inspect).await?;
    data["sha256"] = json!(hash);
    Ok(ToolOutcome::read(
        "XLSX workbook information",
        data.to_string(),
        Some(args.path),
    )
    .with_data(data))
}
pub async fn read(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: ReadRange = parse_args(args)?;
    parser::selection(&args.range)?;
    let (bytes, hash) = source(
        ctx,
        "read_sheet_range",
        &args.path,
        args.expected_sha256.as_deref(),
    )?;
    let mut data = run(
        ctx,
        &bytes,
        Request::Read {
            sheet: args.sheet,
            range: args.range,
        },
    )
    .await?;
    data["sha256"] = json!(hash);
    Ok(
        ToolOutcome::read("XLSX stored cell values", data.to_string(), Some(args.path))
            .with_data(data),
    )
}
pub async fn write(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Edit = parse_args(args)?;
    parser::selection(&args.range)?;
    if !xlsx(&args.destination) {
        return Err(invalid("destination must be a new .xlsx file"));
    }
    let (bytes, hash) = source(
        ctx,
        "write_sheet_range",
        &args.path,
        Some(&args.expected_sha256),
    )?;
    let initial = binary::target(ctx, "write_sheet_range", &args.destination)?;
    let mut response = run(
        ctx,
        &bytes,
        Request::Edit {
            sheet: args.sheet,
            range: args.range,
            values: args.values,
        },
    )
    .await?;
    let encoded = response["xlsx"].take();
    let output = STANDARD
        .decode(
            encoded
                .as_str()
                .ok_or_else(|| io("workbook worker omitted output"))?,
        )
        .map_err(io)?;
    if output.len() as u64 > MAX_INPUT {
        return Err(unsupported("edited workbook exceeds 16 MiB"));
    }
    response["source_sha256"] = json!(hash);
    response["sha256"] = json!(fsops::sha256_bytes(&output));
    response["destination"] = json!(initial.display());
    response["bytes"] = json!(output.len());
    let review=format!("Create a new edited XLSX from {}. Original preserved; selected cells keep their style. No formulas are evaluated.\n{}",args.path,serde_json::to_string_pretty(&response).map_err(io)?);
    if review.len() > 50000 {
        return Err(unsupported(
            "workbook change review exceeds 50,000 bytes; select fewer cells",
        ));
    }
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "write_sheet_range",
            &review,
            Some(initial.display()),
        )
        .await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let target = binary::target(ctx, "write_sheet_range", &args.destination)?;
    source(ctx, "write_sheet_range", &args.path, Some(&hash))?;
    let mut stage = Stage::new(directory(target.absolute.parent().unwrap())?, false)?;
    response["state"] = json!("prepared");
    response["staging_name"] = json!(stage.name.to_string_lossy());
    let staged = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "xlsx.prepare",
                Some(target.display()),
                "allowed",
                &response.to_string(),
            )
            .map_err(io)?;
        stage.file.write_all(&output).map_err(io)?;
        binary::target(ctx, "write_sheet_range", &args.destination)?;
        stage.publish(&target.absolute)
    })();
    binary::staged_result(
        ctx,
        "write_sheet_range",
        &target.display(),
        &mut stage,
        staged,
    )?;
    response["state"] = json!("completed");
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.write_sheet_range",
            Some(target.display()),
            "allowed",
            &response.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "workbook published but final audit failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write("Edited XLSX saved as a new file","Original preserved. Only reviewed existing literal cells changed; formulas and complex workbooks are refused.",Some(target.display())).with_data(response))
}
pub fn worker() -> anyhow::Result<()> {
    for (resource, limit) in [(libc::RLIMIT_CORE, 0), (libc::RLIMIT_CPU, 5)] {
        let bound = libc::rlimit {
            rlim_cur: limit,
            rlim_max: limit,
        };
        anyhow::ensure!(
            unsafe { libc::setrlimit(resource, &bound) } == 0,
            "cannot bound workbook worker"
        );
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_INPUT + 48005)
        .read_to_end(&mut input)?;
    anyhow::ensure!(
        input.len() >= 4 && input.len() as u64 <= MAX_INPUT + 48004,
        "workbook input exceeds budget"
    );
    let n = u32::from_le_bytes(input[..4].try_into()?) as usize;
    anyhow::ensure!(
        n <= 48000 && 4 + n <= input.len(),
        "invalid workbook header"
    );
    let req: Request = serde_json::from_slice(&input[4..4 + n])?;
    let bytes = &input[4 + n..];
    anyhow::ensure!(bytes.len() as u64 <= MAX_INPUT, "workbook exceeds 16 MiB");
    let result = match parser::process(bytes, req) {
        Ok(v) => v,
        Err(e) => json!({"error":e.to_string().chars().take(2000).collect::<String>()}),
    };
    let raw = serde_json::to_vec(&result)?;
    anyhow::ensure!(
        raw.len() <= 24 * 1024 * 1024,
        "workbook output exceeded budget"
    );
    std::io::stdout().write_all(&raw)?;
    Ok(())
}
