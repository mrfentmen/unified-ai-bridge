use std::fs;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use crate::failure::ToolFailure;
use crate::fsops;
use crate::tools::{parse_args, require_text_extension, ToolContext, ToolOutcome};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenshotArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    region: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowArgs {
    #[serde(default)]
    application: Option<String>,
}

pub async fn desktop_screenshot(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    ensure_enabled(ctx)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = args;
        return Err(ToolFailure::Unsupported(
            "desktop_screenshot currently supports macOS only".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let args: ScreenshotArgs = parse_args(args)?;
        let path = match args.path {
            Some(path) => path,
            None => {
                if ctx.app.workspace.profile == bridge_core::workspace::Profile::Research {
                    format!(
                        "AGENTS/{}/experiments/screenshots/{}.png",
                        ctx.agent.dir(),
                        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
                    )
                } else {
                    format!(
                        "output/screenshots/{}.png",
                        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
                    )
                }
            }
        };
        let resolved = ctx.resolve_any(&path)?;
        require_text_extension(&resolved.absolute, &["png"])?;
        if resolved.exists {
            return Err(ToolFailure::InvalidArguments(
                "screenshot destination already exists; choose a new path".into(),
            ));
        }
        let approved = ctx.authorize_write("desktop_screenshot", &resolved).await?;
        let capture = tempfile::tempdir().map_err(|err| ToolFailure::from_io("create capture directory", err))?;
        let captured = capture.path().join("capture.png");
        let mut command = Command::new("/usr/sbin/screencapture");
        command.arg("-x").arg("-t").arg("png");
        if let Some(region) = args.region.as_deref() {
            if !valid_region(region) {
                return Err(ToolFailure::InvalidArguments(
                    "region must be x,y,width,height with integer values".into(),
                ));
            }
            command.arg("-R").arg(region);
        }
        command
            .arg(&captured)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|err| ToolFailure::Io(format!("start screencapture: {err}")))?;
        let output =
            match tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await {
                Ok(Ok(output)) => output,
                _ => return Err(ToolFailure::Io("screencapture timed out".into())),
            };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolFailure::PermissionDenied(format!(
                "screencapture failed ({}). On macOS, grant Screen Recording permission to the terminal/Chrome host in System Settings → Privacy & Security → Screen Recording. {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }
        let _lock = fsops::PathLock::acquire(&ctx.app.workspace.lock_dir, &resolved.display())
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        let current = ctx.resolve_any(&path)?;
        if current.absolute != resolved.absolute || current.exists {
            return Err(ToolFailure::Conflict("screenshot destination changed during approval/capture".into()));
        }
        ctx.recheck_access(crate::sandbox::Access::Write, &current, approved)?;
        let content = fs::read(&captured).map_err(|err| ToolFailure::from_io("read capture", err))?;
        let written = fsops::atomic_write_locked(&ctx.app.workspace.checkpoint_dir,
            &current.relative, &current.absolute, &content, true)
            .map_err(|err| ToolFailure::Io(err.to_string()))?;
        let (sha256, bytes) = (written.sha256, written.bytes);
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "tool.desktop_screenshot",
                Some(resolved.display()),
                "allowed",
                &serde_json::json!({"sha256": sha256, "bytes": bytes}).to_string(),
            )
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        Ok(ToolOutcome::write(
            format!("captured screenshot {}", resolved.display()),
            format!(
                "Screenshot saved to {} ({} bytes, sha256 {}). User must have granted Screen Recording permission.",
                resolved.display(),
                bytes,
                sha256
            ),
            Some(resolved.display()),
        )
        .with_data(serde_json::json!({
            "path": resolved.display(),
            "bytes": bytes,
            "sha256": sha256,
        })))
    }
}

pub async fn active_application(ctx: &ToolContext<'_>) -> Result<ToolOutcome, ToolFailure> {
    ensure_enabled(ctx)?;
    #[cfg(not(target_os = "macos"))]
    return Err(ToolFailure::Unsupported(
        "desktop tools support macOS only".into(),
    ));
    #[cfg(target_os = "macos")]
    {
        let output = run_osascript(
            "tell application \"System Events\" to get name of first application process whose frontmost is true",
            &[],
        )
        .await?;
        Ok(ToolOutcome::read(
            format!("frontmost application: {}", output.trim()),
            format!("Frontmost application: {}", output.trim()),
            None,
        ))
    }
}

pub async fn list_open_applications(ctx: &ToolContext<'_>) -> Result<ToolOutcome, ToolFailure> {
    ensure_enabled(ctx)?;
    #[cfg(not(target_os = "macos"))]
    return Err(ToolFailure::Unsupported(
        "desktop tools support macOS only".into(),
    ));
    #[cfg(target_os = "macos")]
    {
        let script = "tell application \"System Events\" to get name of every application process whose background only is false";
        let output = run_osascript(script, &[]).await?;
        let apps: Vec<String> = output
            .split(',')
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        Ok(ToolOutcome::read(
            format!("{} visible applications", apps.len()),
            format!(
                "Visible applications:\n{}",
                apps.iter()
                    .map(|app| format!("- {app}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            None,
        )
        .with_data(serde_json::json!({"applications": apps})))
    }
}

pub async fn inspect_accessible_window(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    ensure_enabled(ctx)?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = args;
        return Err(ToolFailure::Unsupported(
            "desktop tools support macOS only".into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let args: WindowArgs = parse_args(args)?;
        let script = r#"on run argv
set targetName to ""
if (count of argv) > 0 then set targetName to item 1 of argv
tell application "System Events"
    if targetName is "" then
        set proc to first application process whose frontmost is true
    else
        set proc to first application process whose name is targetName
    end if
    set outputText to "Application: " & (name of proc as text) & linefeed
    repeat with w in windows of proc
        set outputText to outputText & "- " & (name of w as text)
        try
            set outputText to outputText & " | role: " & (role of w as text)
        end try
        try
            set outputText to outputText & " | position: " & (position of w as text)
        end try
        try
            set outputText to outputText & " | size: " & (size of w as text)
        end try
        set outputText to outputText & linefeed
    end repeat
    return outputText
end tell
end run"#;
        let arg = args.application.unwrap_or_default();
        let argv: Vec<&str> = if arg.is_empty() {
            vec![]
        } else {
            vec![arg.as_str()]
        };
        let output = run_osascript(script, &argv).await?;
        Ok(ToolOutcome::read(
            "inspected accessible windows",
            output,
            None,
        ))
    }
}

fn ensure_enabled(ctx: &ToolContext<'_>) -> Result<(), ToolFailure> {
    if ctx.app.config.desktop_tools_enabled {
        Ok(())
    } else {
        Err(ToolFailure::PermissionDenied(
            "desktop tools are disabled in the bridge configuration".into(),
        ))
    }
}

fn valid_region(value: &str) -> bool {
    let parts: Vec<&str> = value.split(',').collect();
    parts.len() == 4 && parts.iter().all(|part| part.trim().parse::<i64>().is_ok())
}

#[cfg(target_os = "macos")]
async fn run_osascript(script: &str, args: &[&str]) -> Result<String, ToolFailure> {
    let mut command = Command::new("/usr/bin/osascript");
    command.arg("-e").arg(script);
    if !args.is_empty() {
        command.arg("--").args(args);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|err| ToolFailure::Io(format!("start osascript: {err}")))?;
    let output = match tokio::time::timeout(Duration::from_secs(15), child.wait_with_output()).await
    {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(ToolFailure::Io(format!("osascript failed: {err}"))),
        Err(_) => return Err(ToolFailure::Io("osascript timed out".into())),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ToolFailure::PermissionDenied(format!(
            "System Events query failed. Grant Accessibility permission in System Settings → Privacy & Security → Accessibility. {}",
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
