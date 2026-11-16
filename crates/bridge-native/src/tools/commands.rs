use crate::{
    failure::ToolFailure,
    tools::{parse_args, ToolContext, ToolOutcome},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    command_id: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    background: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PreparedCommand {
    pub grant: crate::config::AllowedCommand,
    pub args: Vec<String>,
    pub program_sha256: String,
    pub cwd: std::path::PathBuf,
}

/// Suggestions only. The user must grant each exact invocation explicitly.
pub fn presets(app: &crate::app::App) -> Vec<crate::config::AllowedCommand> {
    let mut results = Vec::new();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let find = |name: &str| {
        [
            home.join(".cargo/bin").join(name),
            std::path::PathBuf::from("/usr/bin").join(name),
            std::path::PathBuf::from("/opt/homebrew/bin").join(name),
            std::path::PathBuf::from("/usr/local/bin").join(name),
        ]
        .into_iter()
        .find(|path| path.is_file())
    };
    let mut add = |id: &str, program: &str, args: &[&str], writes: bool| {
        if let Some(path) = find(program) {
            let command = crate::config::AllowedCommand {
                command_id: id.into(),
                program: path,
                fixed_args: args.iter().map(|s| s.to_string()).collect(),
                allowed_args: vec![],
                cwd: ".".into(),
                timeout_secs: 120,
                max_output_bytes: 20_000,
                writes,
                always_allow: false,
            };
            if command.validate().is_ok() {
                results.push(command);
            }
        }
    };
    if app.workspace.root.join(".git").exists() {
        add(
            "git-status",
            "git",
            &["--no-pager", "status", "--short"],
            false,
        );
        add(
            "git-diff",
            "git",
            &["--no-pager", "diff", "--no-ext-diff", "--no-textconv"],
            false,
        );
        add(
            "git-log",
            "git",
            &["--no-pager", "log", "-n", "10", "--oneline"],
            false,
        );
    }
    if app.workspace.root.join("Cargo.toml").is_file() {
        add("cargo-check", "cargo", &["check", "--locked"], true);
        add("cargo-test", "cargo", &["test", "--locked"], true);
    }
    // npm is itself an interpreter launcher. Do not quietly loosen the command
    // validator to make this suggestion work.
    results
}

pub async fn list_allowed_commands(
    ctx: &ToolContext<'_>,
    args: Value,
) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    let commands = ctx
        .app
        .config
        .workspace_commands
        .get(&ctx.app.workspace.id())
        .cloned()
        .unwrap_or_default();
    let data = json!({"commands":commands,"workspace_id":ctx.app.workspace.id()});
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.list_allowed_commands",
            None,
            "allowed",
            "listed workspace command grants",
        )
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    Ok(ToolOutcome::read(
        format!("{} allowed commands", commands.len()),
        serde_json::to_string_pretty(&data).unwrap_or_default(),
        None,
    )
    .with_data(data))
}

/// Drain both streams continuously, retaining only a shared byte budget. Large
/// output cannot deadlock the process or allocate unbounded host memory.
async fn capture(
    mut stream: impl AsyncRead + Unpin,
    budget: Arc<AtomicUsize>,
    sink: Option<(Arc<crate::store::Store>, String, &'static str)>,
) -> std::io::Result<(String, usize)> {
    let mut kept = Vec::new();
    let mut seen = 0usize;
    let mut buffer = [0u8; 8192];
    let mut pending_utf8 = Vec::new();
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        seen = seen.saturating_add(count);
        let prior = budget
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                Some(remaining.saturating_sub(count))
            })
            .unwrap();
        kept.extend_from_slice(&buffer[..count.min(prior)]);
        if let Some((store, id, name)) = &sink {
            if prior > 0 {
                // The persisted text is capped by the shared budget. Process
                // output is untrusted text, never commands for the next step.
                pending_utf8.extend_from_slice(&buffer[..count.min(prior)]);
                let text = decode_output(&mut pending_utf8, false);
                if !text.is_empty() {
                    store
                        .append_job_output(id, name, &text)
                        .map_err(std::io::Error::other)?;
                }
            }
        }
    }
    if let Some((store, id, name)) = &sink {
        let text = decode_output(&mut pending_utf8, true);
        if !text.is_empty() {
            store
                .append_job_output(id, name, &text)
                .map_err(std::io::Error::other)?;
        }
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    let mut boundary = kept.len().min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    Ok((text, seen))
}

/// Keep incomplete code points across pipe reads. Invalid bytes become one
/// ASCII marker, so persisted text cannot exceed the raw output-byte budget.
fn decode_output(pending: &mut Vec<u8>, flush: bool) -> String {
    let mut text = String::new();
    while !pending.is_empty() {
        match std::str::from_utf8(pending) {
            Ok(valid) => {
                text.push_str(valid);
                pending.clear();
            }
            Err(error) => {
                let valid = error.valid_up_to();
                text.push_str(std::str::from_utf8(&pending[..valid]).expect("valid UTF-8 prefix"));
                pending.drain(..valid);
                match error.error_len() {
                    Some(n) => {
                        text.push('?');
                        pending.drain(..n);
                    }
                    None if flush => {
                        text.push('?');
                        pending.clear();
                    }
                    None => break,
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod output_tests {
    use super::decode_output;
    #[test]
    fn unicode_split_across_reads_and_invalid_bytes_stay_bounded() {
        let mut pending = vec![b'a', 0xf0, 0x9f];
        assert_eq!(decode_output(&mut pending, false), "a");
        pending.extend_from_slice(&[0x8c, 0x8d, b'b', 0xff, 0xe2]);
        assert_eq!(decode_output(&mut pending, false), "🌍b?");
        assert_eq!(decode_output(&mut pending, true), "?");
        assert!(pending.is_empty());
    }
}

// Also reap descendants on timeout or cancellation, including descendants
// holding stdout open after the original program exits.
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.0 > 0 {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }
}

async fn prepare(ctx: &ToolContext<'_>, args: &RunArgs) -> Result<PreparedCommand, ToolFailure> {
    let grant = ctx
        .app
        .config
        .workspace_commands
        .get(&ctx.app.workspace.id())
        .and_then(|commands| {
            commands
                .iter()
                .find(|command| command.command_id == args.command_id)
        })
        .ok_or_else(|| {
            ToolFailure::PermissionDenied(format!(
                "command `{}` is not allowlisted for this workspace",
                args.command_id
            ))
        })?;
    grant
        .validate()
        .map_err(|err| ToolFailure::PermissionDenied(err.to_string()))?;
    if !args.args.is_empty() && !grant.allowed_args.contains(&args.args) {
        return Err(ToolFailure::PermissionDenied(
            "arguments do not match an explicitly allowlisted argument sequence".into(),
        ));
    }
    let cwd = ctx.resolve_existing(&grant.cwd.to_string_lossy())?;
    ctx.check_read("run_command", &cwd)?;
    if !cwd.is_dir {
        return Err(ToolFailure::InvalidArguments(
            "command cwd must be a directory".into(),
        ));
    }
    let program = grant
        .program
        .canonicalize()
        .map_err(|err| ToolFailure::from_io("resolve command program", err))?;
    let policy = ctx
        .app
        .store
        .workspaces()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .into_iter()
        .find(|w| w.id == ctx.app.workspace.id())
        .ok_or_else(|| ToolFailure::PermissionDenied("workspace is no longer granted".into()))?;
    if policy.read_only {
        return Err(ToolFailure::PermissionDenied(
            "workspace was locked read-only".into(),
        ));
    }
    if !grant.always_allow
        || policy.permission_mode == bridge_core::workspace::PermissionMode::Strict
    {
        ctx.app
            .request_approval(
                ctx.session_id,
                ctx.agent,
                "run_command",
                &format!(
                    "Run {}: {} {:?} {:?}; cwd {}; writes expected: {}",
                    grant.command_id,
                    program.display(),
                    grant.fixed_args,
                    args.args,
                    cwd.display(),
                    grant.writes
                ),
                Some(cwd.display()),
            )
            .await?;
    }
    let latest = ctx
        .app
        .home
        .load_config()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?;
    let current = latest
        .workspace_commands
        .get(&ctx.app.workspace.id())
        .and_then(|commands| commands.iter().find(|c| c.command_id == grant.command_id));
    if current.map(|c| serde_json::to_value(c).unwrap_or_default())
        != Some(serde_json::to_value(grant).unwrap_or_default())
    {
        return Err(ToolFailure::PermissionDenied(
            "command grant changed while awaiting approval".into(),
        ));
    }
    let rechecked = ctx.resolve_existing(&grant.cwd.to_string_lossy())?;
    ctx.check_read("run_command", &rechecked)?;
    if rechecked.absolute != cwd.absolute || !rechecked.is_dir {
        return Err(ToolFailure::PermissionDenied(
            "command directory changed while awaiting approval".into(),
        ));
    }
    if ctx
        .app
        .store
        .workspaces()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .iter()
        .find(|w| w.id == ctx.app.workspace.id())
        .is_none_or(|w| w.read_only)
    {
        return Err(ToolFailure::PermissionDenied(
            "workspace was locked or removed while awaiting approval".into(),
        ));
    }
    Ok(PreparedCommand {
        grant: grant.clone(),
        args: args.args.clone(),
        program_sha256: crate::fsops::sha256_file(&program)
            .map_err(|e| ToolFailure::Internal(e.to_string()))?
            .0,
        cwd: cwd.absolute,
    })
}

pub async fn run_command(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: RunArgs = parse_args(args)?;
    let prepared = prepare(ctx, &args).await?;
    if args.background {
        crate::jobs::start(ctx, prepared).await
    } else {
        execute(ctx, prepared, None).await
    }
}

pub async fn execute(
    ctx: &ToolContext<'_>,
    prepared: PreparedCommand,
    job_id: Option<&str>,
) -> Result<ToolOutcome, ToolFailure> {
    let grant = &prepared.grant;
    let args = &prepared;
    let cwd = ctx.resolve_existing(&grant.cwd.to_string_lossy())?;
    ctx.check_read("run_command", &cwd)?;
    let current = ctx
        .app
        .home
        .load_config()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?;
    if current
        .workspace_commands
        .get(&ctx.app.workspace.id())
        .into_iter()
        .flatten()
        .find(|c| c.command_id == grant.command_id)
        .map(|c| serde_json::to_value(c).unwrap_or_default())
        != Some(serde_json::to_value(grant).unwrap_or_default())
        || cwd.absolute != prepared.cwd
        || !cwd.is_dir
    {
        return Err(ToolFailure::PermissionDenied(
            "command grant or directory changed before execution".into(),
        ));
    }
    let policy = ctx
        .app
        .store
        .workspaces()
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .into_iter()
        .find(|w| w.id == ctx.app.workspace.id());
    if policy.is_none_or(|w| w.read_only) {
        return Err(ToolFailure::PermissionDenied(
            "workspace locked or removed before execution".into(),
        ));
    }
    grant
        .validate()
        .map_err(|e| ToolFailure::PermissionDenied(e.to_string()))?;
    let program = grant
        .program
        .canonicalize()
        .map_err(|e| ToolFailure::from_io("resolve command", e))?;
    if crate::fsops::sha256_file(&program)
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .0
        != prepared.program_sha256
    {
        return Err(ToolFailure::PermissionDenied(
            "approved executable changed before execution".into(),
        ));
    }
    ctx.app.audit_path_access(ctx.agent,ctx.session_id,"command.start",Some(cwd.display()),"allowed",
        &json!({"command_id":grant.command_id,"program":program,"args":args.args,"workspace_id":ctx.app.workspace.id()}).to_string())
        .map_err(|err|ToolFailure::Internal(err.to_string()))?;
    // Keep argv[0] from the approved path: rustup's cargo/rustc proxies use it
    // to select the executable. Canonicalizing the launch path breaks them.
    let mut command = Command::new(&grant.program);
    let home = std::env::var_os("HOME").unwrap_or_default();
    let mut search_paths = vec![grant
        .program
        .parent()
        .unwrap_or(std::path::Path::new("/usr/bin"))
        .to_path_buf()];
    search_paths.push(std::path::PathBuf::from(&home).join(".cargo/bin"));
    search_paths.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(std::path::PathBuf::from));
    let search_path = std::env::join_paths(search_paths)
        .map_err(|err| ToolFailure::InvalidArguments(err.to_string()))?;
    command
        .args(&grant.fixed_args)
        .args(&args.args)
        .current_dir(&cwd.absolute)
        .env_clear()
        .env("PATH", search_path)
        .env("LANG", "en_US.UTF-8")
        .env("HOME", home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|err| ToolFailure::from_io("start allowed command", err))?;
    let group = ProcessGroup(child.id().unwrap_or_default());
    if let Some(id) = job_id {
        ctx.app
            .store
            .update_job(id, |job| {
                job.process_id = child.id();
                Ok(())
            })
            .map_err(|e| ToolFailure::Internal(e.to_string()))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolFailure::Internal("missing command stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolFailure::Internal("missing command stderr".into()))?;
    let budget = Arc::new(AtomicUsize::new(grant.max_output_bytes));
    let run = async {
        tokio::try_join!(
            child.wait(),
            capture(
                stdout,
                budget.clone(),
                job_id.map(|id| (ctx.app.store.clone(), id.to_string(), "stdout"))
            ),
            capture(
                stderr,
                budget,
                job_id.map(|id| (ctx.app.store.clone(), id.to_string(), "stderr"))
            )
        )
    };
    let result = tokio::time::timeout(Duration::from_secs(grant.timeout_secs), run).await;
    drop(group);
    let (exit, stdout, stderr, bytes_seen, timed_out) = match result {
        Ok(Ok((status, (stdout, stdout_bytes), (stderr, stderr_bytes)))) => (
            status.code(),
            stdout,
            stderr,
            stdout_bytes.saturating_add(stderr_bytes),
            false,
        ),
        Ok(Err(err)) => return Err(ToolFailure::from_io("read command result", err)),
        Err(_) => {
            let _ = child.wait().await;
            (None, String::new(), String::new(), 0, true)
        }
    };
    let truncated = bytes_seen > grant.max_output_bytes;
    let data = json!({"command_id":grant.command_id,"exit_code":exit,"cwd":cwd.display(),"stdout":stdout,"stderr":stderr,"truncated":truncated,"bytes_seen":bytes_seen,"max_output_bytes":grant.max_output_bytes,"timed_out":timed_out});
    let success = !timed_out && exit == Some(0);
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.run_command",
            Some(cwd.display()),
            if success { "allowed" } else { "error" },
            &data.to_string(),
        )
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
    let output = format!(
        "Command: {}\nExit: {:?}\nSTDOUT:\n{}\nSTDERR:\n{}{}",
        grant.command_id,
        exit,
        stdout,
        stderr,
        if timed_out {
            "\n[bridge: command timed out; process group terminated]"
        } else if truncated {
            "\n[bridge: output truncated at the configured byte budget]"
        } else {
            ""
        }
    );
    let mut outcome = ToolOutcome::write(
        format!(
            "{}: {}",
            grant.command_id,
            if success { "completed" } else { "failed" }
        ),
        output,
        Some(cwd.display()),
    )
    .with_data(data);
    if !success {
        outcome.failure = Some((
            if timed_out {
                "command_timeout"
            } else {
                "command_exit"
            },
            format!(
                "command {} {}",
                grant.command_id,
                if timed_out {
                    "timed out".into()
                } else {
                    format!("exited with {:?}", exit)
                }
            ),
        ));
    }
    Ok(outcome)
}
