use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use crate::failure::ToolFailure;
use crate::tools::{parse_args, ToolContext, ToolOutcome};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptArgs {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestArgs {
    path: String,
}

pub async fn git_status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    super::git::status(ctx, args).await
}
pub async fn git_diff(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    super::git::diff(ctx, args).await
}
pub async fn inspect_repository(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    super::git::inspect(ctx, args).await
}

pub async fn run_approved_script(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: ScriptArgs = parse_args(args)?;
    let Some(script) = ctx
        .app
        .config
        .approved_scripts
        .iter()
        .find(|script| script.name == args.name)
    else {
        return Err(ToolFailure::PermissionDenied(format!(
            "no approved script named `{}`; add it to approved_scripts in the bridge configuration",
            args.name
        )));
    };
    let reason = format!(
        "run approved script `{}` ({} {:?})",
        script.name,
        script.program.display(),
        script.args
    );
    ctx.app
        .request_approval(
            ctx.session_id,
            ctx.agent,
            "run_approved_script",
            &reason,
            None,
        )
        .await?;
    let cwd = match &script.cwd {
        Some(relative) => {
            let resolved = ctx.resolve_existing(&relative.to_string_lossy())?;
            if !resolved.is_dir {
                return Err(ToolFailure::InvalidArguments(
                    "approved script cwd is not a directory".into(),
                ));
            }
            resolved.absolute
        }
        None => {
            if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
                ctx.workspace_root().join(ctx.agent.workspace_relative())
            } else {
                ctx.workspace_root().to_path_buf()
            }
        }
    };
    let mut command = Command::new(&script.program);
    command
        .args(&script.args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|err| ToolFailure::Io(format!("start approved script: {err}")))?;
    let timeout = Duration::from_secs(script.timeout_secs);
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(ToolFailure::Io(format!("approved script failed: {err}"))),
        Err(_) => {
            return Err(ToolFailure::Io(format!(
                "approved script timed out after {}s",
                script.timeout_secs
            )))
        }
    };
    let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if stdout.len() > 50_000 {
        stdout.truncate(50_000);
        stdout.push_str("\n[stdout truncated]");
    }
    let mut stderr_out = stderr;
    if stderr_out.len() > 20_000 {
        stderr_out.truncate(20_000);
        stderr_out.push_str("\n[stderr truncated]");
    }
    let exit = output.status.code().unwrap_or(-1);
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.run_approved_script",
            None,
            if output.status.success() {
                "allowed"
            } else {
                "error"
            },
            &serde_json::json!({"script": script.name, "exit_code": exit}).to_string(),
        )
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    Ok(ToolOutcome::write(
        format!("ran approved script `{}` (exit {})", script.name, exit),
        format!(
            "Script: {}\nExit code: {}\nWorking directory: {}\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
            script.name,
            exit,
            cwd.display(),
            if stdout.trim().is_empty() {
                "(empty)"
            } else {
                stdout.trim_end()
            },
            if stderr_out.trim().is_empty() {
                "(empty)"
            } else {
                stderr_out.trim_end()
            }
        ),
        None,
    )
    .with_data(
        serde_json::json!({"script": script.name, "exit_code": exit, "cwd": cwd.to_string_lossy()}),
    ))
}

pub async fn inspect_test_results(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let args: TestArgs = parse_args(args)?;
    let (path, text) = ctx.read_text("inspect_test_results", &args.path, 2 * 1024 * 1024)?;
    let lower = text.to_ascii_lowercase();
    let mut summary = String::new();
    if lower.contains("<testsuite") || lower.contains("<testsuites") {
        let tests = count_xml_attr(&text, "tests");
        let failures = count_xml_attr(&text, "failures");
        let errors = count_xml_attr(&text, "errors");
        let skipped = count_xml_attr(&text, "skipped");
        summary = format!(
            "JUnit-style results: tests={tests}, failures={failures}, errors={errors}, skipped={skipped}.\n\n"
        );
    } else if lower.contains("\"tests\"") || lower.contains("\"failures\"") {
        summary = "JSON-looking test output; inspect the bounded excerpt below.\n\n".into();
    }
    let excerpt: String = text.chars().take(40_000).collect();
    Ok(ToolOutcome::read(
        format!("inspected test results {}", path.display()),
        format!("File: {}\n{}Excerpt:\n{}", path.display(), summary, excerpt),
        Some(path.display()),
    )
    .with_data(
        serde_json::json!({"path": path.display(), "junit_like": lower.contains("<testsuite"),"diagnostics":super::code_navigation::diagnostics(ctx,ctx.workspace_root(),&text)}),
    ))
}

fn count_xml_attr(text: &str, attribute: &str) -> usize {
    let needle = format!("{attribute}=");
    let mut total = 0usize;
    let mut rest = text;
    while let Some(index) = rest.find(&needle) {
        rest = &rest[index + needle.len()..];
        let quote = rest.chars().next().unwrap_or('"');
        if quote == '"' || quote == '\'' {
            rest = &rest[quote.len_utf8()..];
            let end = rest.find(quote).unwrap_or(0);
            if let Ok(value) = rest[..end].parse::<usize>() {
                total += value;
            }
            rest = &rest[end.saturating_add(quote.len_utf8())..];
        }
    }
    total
}
