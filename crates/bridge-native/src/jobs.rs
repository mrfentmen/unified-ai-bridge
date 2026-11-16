//! Durable command jobs. A narrowly scoped Rust worker owns each approved
//! command so a Chrome/native-messaging reconnect does not restart the command.
use crate::{
    app::App,
    config::HomePaths,
    failure::ToolFailure,
    fsops::PathLock,
    tools::{
        commands::{self, PreparedCommand},
        parse_args, ToolContext, ToolOutcome,
    },
    workspace::Workspace,
};
use anyhow::{bail, Result};
use bridge_core::provider::Agent;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub workspace_id: String,
    pub root: PathBuf,
    pub owner: String,
    pub agent: Agent,
    pub session_id: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub state: String,
    pub message: String,
    pub heartbeat: i64,
    pub worker_id: Option<u32>,
    pub process_id: Option<u32>,
    pub command: PreparedCommand,
    pub result: Option<Value>,
    #[serde(default)]
    pub awake_until: Option<i64>,
}
impl Job {
    pub fn terminal(&self) -> bool {
        matches!(
            self.state.as_str(),
            "completed" | "failed" | "cancelled" | "interrupted"
        )
    }
    pub fn metadata(&self) -> Value {
        json!({"job_id":self.id,"workspace_id":self.workspace_id,"agent":self.agent.dir(),"created_at":self.created_at,
            "started_at":self.started_at,"finished_at":self.finished_at,"state":self.state,"message":self.message,
            "command_id":self.command.grant.command_id,"timeout_secs":self.command.grant.timeout_secs,"max_output_bytes":self.command.grant.max_output_bytes,
            "process_id":if self.terminal(){None}else{self.process_id},"awake_until":self.awake_until,"result":self.result})
    }
}
fn internal(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Internal(e.to_string())
}

pub fn owner(ctx: &ToolContext<'_>) -> Result<String, ToolFailure> {
    if ctx.session_id == "mcp" {
        return Ok(format!("mcp:{}", ctx.agent.dir()));
    }
    let (session, agent) = ctx
        .app
        .session_agent(ctx.session_id)?
        .ok_or_else(|| ToolFailure::PermissionDenied("job requires a registered session".into()))?;
    if agent != ctx.agent || session.workspace_id != ctx.app.workspace.id() {
        return Err(ToolFailure::PermissionDenied(
            "job session/workspace does not match".into(),
        ));
    }
    let conversation = session
        .conversation
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| format!("session:{}", session.id));
    Ok(crate::fsops::sha256_bytes(
        format!("{}\0{}\0{}", session.provider, agent.dir(), conversation).as_bytes(),
    ))
}

pub async fn start(
    ctx: &ToolContext<'_>,
    command: PreparedCommand,
) -> Result<ToolOutcome, ToolFailure> {
    let owner = owner(ctx)?;
    let deployment_guard = running_guard(&ctx.app.home).map_err(internal)?;
    let _admission =
        PathLock::acquire(&ctx.app.home.base.join("locks"), "job-admission").map_err(internal)?;
    let known = ctx
        .app
        .store
        .jobs(&ctx.app.workspace.id(), None)
        .map_err(internal)?;
    let mut active = 0;
    for job in known {
        if !reconcile(ctx.app, job).map_err(internal)?.terminal() {
            active += 1;
        }
    }
    if active >= 4 {
        return Err(ToolFailure::PermissionDenied(
            "workspace already has four active command jobs".into(),
        ));
    }
    let mut random = [0; 16];
    rand::rngs::OsRng.fill_bytes(&mut random);
    let id = hex::encode(random);
    let job = Job {
        id: id.clone(),
        workspace_id: ctx.app.workspace.id(),
        root: ctx.app.workspace.root.clone(),
        owner,
        agent: ctx.agent,
        session_id: ctx.session_id.into(),
        created_at: App::now(),
        started_at: None,
        finished_at: None,
        state: "queued".into(),
        message: "Approved command queued for its worker".into(),
        heartbeat: chrono::Utc::now().timestamp(),
        worker_id: None,
        process_id: None,
        command,
        result: None,
        awake_until: None,
    };
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "job.create",
            Some(job.command.grant.cwd.to_string_lossy().into()),
            "allowed",
            &job.metadata().to_string(),
        )
        .map_err(internal)?;
    ctx.app.store.insert_job(&job).map_err(internal)?;
    let worker = ctx.app.home.installed_binary();
    // Prefer the running host in production. Test installations supply the real
    // binary under HomePaths rather than accidentally launching a test harness.
    let current = std::env::current_exe().map_err(internal)?;
    let executable = if current
        .file_name()
        .is_some_and(|s| s == crate::config::HOST_EXECUTABLE_NAME)
    {
        current
    } else {
        worker
    };
    let child = tokio::process::Command::new(executable)
        .arg("--home")
        .arg(&ctx.app.home.base)
        .arg("job-worker")
        .arg(&id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    match child {
        Ok(mut child) => {
            let store = ctx.app.store.clone();
            let watch_id = id.clone();
            tokio::spawn(async move {
                let _deployment_guard = deployment_guard;
                use tokio::io::AsyncReadExt;
                let mut stderr = child.stderr.take().expect("piped worker stderr");
                let drain = async {
                    let mut detail = Vec::new();
                    let mut buf = [0; 1024];
                    while let Ok(n) = stderr.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        detail.extend_from_slice(
                            &buf[..n.min(2000usize.saturating_sub(detail.len()))],
                        );
                    }
                    String::from_utf8_lossy(&detail).into_owned()
                };
                let (exit, detail) = tokio::join!(child.wait(), drain);
                if let Ok(exit) = exit {
                    if !exit.success() {
                        let _ = store.update_job(&watch_id, |j| {
                            if !j.terminal() {
                                j.state = if j.started_at.is_none() {
                                    "failed"
                                } else {
                                    "interrupted"
                                }
                                .into();
                                j.finished_at = Some(App::now());
                                j.message =
                                    format!("Worker exited without a terminal receipt: {detail}");
                            }
                            Ok(())
                        });
                    }
                }
            });
        }
        Err(error) => {
            ctx.app
                .store
                .update_job(&id, |j| {
                    j.state = "failed".into();
                    j.finished_at = Some(App::now());
                    j.message = format!("could not launch command worker: {error}");
                    Ok(())
                })
                .map_err(internal)?;
            return Err(internal(format!(
                "could not launch command worker: {error}"
            )));
        }
    }
    Ok(ToolOutcome::write(format!("Command job {id} queued"),"Poll job_status/read_job_output at a bounded interval. A job receipt is not a successful command result. Do not repeat run_command to poll.",None).with_data(job.metadata()))
}

/// A stale heartbeat is not by itself proof of failure. The worker holds an
/// exclusive per-job lock; only a missing lock owner makes an outcome unknown.
pub fn reconcile(app: &App, job: Job) -> Result<Job> {
    if job.terminal() || chrono::Utc::now().timestamp() - job.heartbeat < 5 {
        return Ok(job);
    }
    if let Ok(_guard) = PathLock::acquire(
        &app.home.base.join("locks"),
        &format!("job-worker:{}", job.id),
    ) {
        let updated=app.store.update_job(&job.id,|j|{
            if !j.terminal() && chrono::Utc::now().timestamp()-j.heartbeat>=5 {
                j.state="interrupted".into();j.finished_at=Some(App::now());j.message="Worker stopped without a terminal receipt. Inspect output/files before starting a new job; no automatic retry.".into();
            } Ok(())
        })?;
        if updated.state == "interrupted" {
            app.audit_path_access(
                job.agent,
                &job.session_id,
                "job.interrupted",
                None,
                "uncertain",
                &updated.metadata().to_string(),
            )?;
        }
        Ok(updated)
    } else {
        Ok(job)
    }
}

pub async fn worker(home: HomePaths, id: &str) -> Result<()> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid job ID");
    }
    let _guard = PathLock::acquire(&home.base.join("locks"), &format!("job-worker:{id}"))?;
    let _deployment_guard = running_guard(&home)?;
    let base = App::load(home, false)?;
    let job = base
        .store
        .job(id)?
        .ok_or_else(|| anyhow::anyhow!("unknown job"))?;
    if job.state != "queued" {
        bail!("job has already been claimed or cancelled");
    }
    let workspace = Workspace::load(&job.root)?;
    if workspace.id() != job.workspace_id {
        bail!("job workspace identity changed");
    }
    let app = base.with_workspace(workspace);
    app.store.update_job(id, |j| {
        if j.state != "queued" {
            bail!("job no longer queued");
        }
        j.state = "running".into();
        j.worker_id = Some(std::process::id());
        j.started_at = Some(App::now());
        j.heartbeat = chrono::Utc::now().timestamp();
        j.message = "Running approved command".into();
        Ok(())
    })?;
    let ctx = ToolContext::new(&app, job.agent, &job.session_id);
    let execution = commands::execute(&ctx, job.command.clone(), Some(id));
    let monitor = async {
        let mut power: Option<tokio::process::Child> = None;
        let mut power_until = None;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let current = app
                .store
                .job(id)?
                .ok_or_else(|| anyhow::anyhow!("job disappeared"))?;
            if current.state == "cancel_requested" {
                return Ok::<String, anyhow::Error>("Cancelled by request".into());
            }
            if current.state != "running" {
                return Ok("Job state changed; command stopped".into());
            }
            if current.awake_until != power_until {
                if let Some(mut child) = power.take() {
                    let _ = child.kill().await;
                }
                power_until = current.awake_until;
                if let Some(until) = power_until.filter(|t| *t > chrono::Utc::now().timestamp()) {
                    #[cfg(target_os = "macos")]
                    {
                        // Fixed OS utility; panel-only opt-in, bounded both by
                        // deadline and worker lifetime. Never prevents lock.
                        power = Some(
                            tokio::process::Command::new("/usr/bin/caffeinate")
                                .args([
                                    "-i",
                                    "-t",
                                    &(until - chrono::Utc::now().timestamp()).to_string(),
                                    "-w",
                                    &std::process::id().to_string(),
                                ])
                                .stdin(Stdio::null())
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .kill_on_drop(true)
                                .spawn()?,
                        );
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = until;
                }
            }
            if chrono::Utc::now().timestamp() > current.heartbeat {
                let config = app.home.load_config()?;
                let granted = config
                    .workspace_commands
                    .get(&job.workspace_id)
                    .into_iter()
                    .flatten()
                    .find(|g| g.command_id == job.command.grant.command_id);
                if granted.map(|g| serde_json::to_value(g).unwrap_or_default())
                    != Some(serde_json::to_value(&job.command.grant)?)
                {
                    return Ok("Command grant revoked or changed; command stopped".into());
                }
                if app
                    .store
                    .workspaces()?
                    .iter()
                    .find(|w| w.id == job.workspace_id)
                    .is_none_or(|w| w.read_only)
                {
                    return Ok("Workspace locked or removed; command stopped".into());
                }
                app.store.update_job(id, |j| {
                    j.heartbeat = chrono::Utc::now().timestamp();
                    Ok(())
                })?;
            }
        }
    };
    let (state, message, result) = tokio::select! {
        outcome=execution=>match outcome {
            Ok(outcome)=>{
                let state=if outcome.failure.is_some(){"failed"}else{"completed"};
                let result=json!({"summary":outcome.summary,"output":outcome.output,"data":outcome.data,"error":outcome.failure.map(|(_,e)|e)});
                (state.to_string(),result["summary"].as_str().unwrap_or_default().to_string(),Some(result))
            },
            Err(error)=>("failed".into(),error.to_string(),Some(json!({"code":error.code(),"error":error.to_string()}))),
        },
        reason=monitor=>("cancelled".into(),reason.unwrap_or_else(|e|format!("Job monitoring failed; command stopped: {e}")),None),
    };
    let final_job = app.store.update_job(id, |j| {
        j.state = state;
        j.message = message;
        j.finished_at = Some(App::now());
        j.result = result;
        j.heartbeat = chrono::Utc::now().timestamp();
        Ok(())
    })?;
    app.audit_path_access(
        job.agent,
        &job.session_id,
        "job.finish",
        Some(job.command.grant.cwd.to_string_lossy().into()),
        &final_job.state,
        &final_job.metadata().to_string(),
    )?;
    app.record_activity(
        job.agent,
        &job.session_id,
        "run_command",
        "job",
        None,
        &format!("Job {}: {}", id, final_job.message),
        &final_job.state,
    )?;
    Ok(())
}

fn running_guard(home: &HomePaths) -> Result<std::fs::File> {
    std::fs::create_dir_all(home.base.join("locks"))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(home.base.join("locks/deployment.lock"))?;
    fs2::FileExt::try_lock_shared(&file).map_err(|_| {
        anyhow::anyhow!("host deployment is in progress; wait before starting a job")
    })?;
    Ok(file)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    job_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    job_id: String,
    #[serde(default)]
    after: u64,
}

fn owned(ctx: &ToolContext<'_>, id: &str) -> Result<Job, ToolFailure> {
    let job = ctx
        .app
        .store
        .job(id)
        .map_err(internal)?
        .filter(|j| j.workspace_id == ctx.app.workspace.id())
        .ok_or_else(|| ToolFailure::PermissionDenied("job is outside this workspace".into()))?;
    if job.owner != owner(ctx)? {
        return Err(ToolFailure::PermissionDenied(
            "job belongs to a different conversation".into(),
        ));
    }
    reconcile(ctx.app, job).map_err(internal)
}
pub async fn list(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    let jobs = ctx
        .app
        .store
        .jobs(&ctx.app.workspace.id(), Some(&owner(ctx)?))
        .map_err(internal)?;
    let jobs: Vec<_> = jobs
        .into_iter()
        .map(|j| {
            reconcile(ctx.app, j).map(|j| {
                let mut m = j.metadata();
                m.as_object_mut().unwrap().remove("result");
                m
            })
        })
        .collect::<Result<_>>()
        .map_err(internal)?;
    Ok(ToolOutcome::read(
        format!("{} jobs in this conversation", jobs.len()),
        "Only jobs bound to this conversation and workspace are included.",
        None,
    )
    .with_data(json!({"jobs":jobs})))
}
pub async fn status(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let job = owned(ctx, &args.job_id)?;
    Ok(ToolOutcome::read(
        format!("Job {}: {}", job.id, job.state),
        job.message.clone(),
        None,
    )
    .with_data(job.metadata()))
}
pub async fn output(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Output = parse_args(args)?;
    let job = owned(ctx, &args.job_id)?;
    let chunks = ctx
        .app
        .store
        .job_output(&job.id, args.after)
        .map_err(internal)?;
    let next = chunks
        .last()
        .and_then(|c| c["cursor"].as_u64())
        .unwrap_or(args.after);
    Ok(ToolOutcome::read("Command output page",chunks.iter().map(|c|format!("[{}] {}",c["stream"].as_str().unwrap_or_default(),c["text"].as_str().unwrap_or_default())).collect::<Vec<_>>().join("\n"),None)
        .with_data(json!({"job_id":job.id,"state":job.state,"chunks":chunks,"next_cursor":next,"terminal":job.terminal(),"output_limit":job.command.grant.max_output_bytes,"truncated":job.result.as_ref().and_then(|v|v["data"]["truncated"].as_bool())})))
}
pub fn cancel_owned(app: &App, id: &str) -> Result<Value> {
    let job = app.store.update_job(id, |j| {
        if !j.terminal() {
            j.state = if j.state == "queued" {
                "cancelled"
            } else {
                "cancel_requested"
            }
            .into();
            j.message = "Cancellation requested; waiting for worker acknowledgement".into();
            if j.state == "cancelled" {
                j.finished_at = Some(App::now());
            }
        }
        Ok(())
    })?;
    app.audit_path_access(
        job.agent,
        &job.session_id,
        "job.cancel",
        None,
        "allowed",
        &job.metadata().to_string(),
    )?;
    Ok(job.metadata())
}
pub async fn cancel(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Id = parse_args(args)?;
    let job = owned(ctx, &args.job_id)?;
    let value = cancel_owned(ctx.app, &job.id).map_err(internal)?;
    Ok(ToolOutcome::write("Cancellation requested","Check job_status for a terminal receipt; cancellation does not undo files already written.",None).with_data(value))
}
pub async fn processes(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let _: Empty = parse_args(args)?;
    let jobs = ctx
        .app
        .store
        .jobs(&ctx.app.workspace.id(), Some(&owner(ctx)?))
        .map_err(internal)?;
    let mut processes = vec![];
    for job in jobs {
        let j = reconcile(ctx.app, job).map_err(internal)?;
        if !j.terminal() {
            processes.push(json!({"job_id":j.id,"command_id":j.command.grant.command_id,"pid":j.process_id,"state":j.state}));
        }
    }
    Ok(ToolOutcome::read(
        "Bridge-owned processes",
        "Command arguments, environments and unrelated processes are not exposed.",
        None,
    )
    .with_data(json!({"processes":processes})))
}

/// These operations are exposed only to the extension's own controls, never to
/// model tools. Every request carries the workspace the user was viewing.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    List,
    Detail {
        job_id: String,
        #[serde(default)]
        after: u64,
    },
    Cancel {
        job_id: String,
    },
    KeepAwake {
        job_id: String,
        seconds: u64,
    },
    ExportReport {
        job_id: String,
    },
}
pub fn control(app: &App, workspace_id: &str, action: Control) -> Result<Value> {
    if workspace_id != app.workspace.id() {
        bail!("workspace changed; refresh job controls");
    }
    if matches!(action, Control::List) {
        let jobs = app
            .store
            .jobs(workspace_id, None)?
            .into_iter()
            .map(|j| {
                let mut value = reconcile(app, j)?.metadata();
                value.as_object_mut().unwrap().remove("result");
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(json!({"jobs":jobs}));
    }
    let id = match &action {
        Control::Detail { job_id, .. }
        | Control::Cancel { job_id }
        | Control::KeepAwake { job_id, .. }
        | Control::ExportReport { job_id } => job_id,
        Control::List => unreachable!(),
    };
    let job = reconcile(
        app,
        app.store
            .job(id)?
            .filter(|j| j.workspace_id == workspace_id)
            .ok_or_else(|| anyhow::anyhow!("job is outside this workspace"))?,
    )?;
    match action {
        Control::List => unreachable!(),
        Control::Detail { after, .. } => {
            let chunks = app.store.job_output(&job.id, after)?;
            let next = chunks
                .last()
                .and_then(|v| v["cursor"].as_u64())
                .unwrap_or(after);
            Ok(json!({"job":job.metadata(),"chunks":chunks,"next_cursor":next}))
        }
        Control::Cancel { .. } => cancel_owned(app, &job.id),
        Control::KeepAwake { seconds, .. } => {
            if !cfg!(target_os = "macos") {
                bail!("bounded keep-awake is currently available on macOS only");
            }
            if seconds > 3600 {
                bail!("keep-awake must be between 0 and 3600 seconds; 0 releases it");
            }
            if job.terminal() {
                bail!("job already finished; no power assertion is needed");
            }
            let until = if seconds == 0 {
                None
            } else {
                Some(
                    chrono::Utc::now().timestamp()
                        + seconds.min(job.command.grant.timeout_secs) as i64,
                )
            };
            app.audit_path_access(
                job.agent,
                "panel",
                "job.keep_awake",
                None,
                "allowed",
                &json!({"job_id":job.id,"awake_until":until}).to_string(),
            )?;
            let updated = app.store.update_job(&job.id, |j| {
                if j.terminal() {
                    bail!("job finished before request");
                }
                j.awake_until = until;
                Ok(())
            })?;
            Ok(updated.metadata())
        }
        Control::ExportReport { .. } => {
            use std::io::Write;
            let directory = app.home.base.join("reports/jobs");
            std::fs::create_dir_all(&directory)?;
            let path = directory.join(format!(
                "{}-{}.json",
                job.id,
                chrono::Utc::now().timestamp_millis()
            ));
            let report = json!({"job":job.metadata(),"workspace_root":job.root,"original_session":job.session_id,
                "audit_database":app.home.database,"notes":["Command receipts do not verify arbitrary changes made by project code.","Interrupted jobs are never automatically repeated.","Use read_job_output for resumable captured output."]});
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path)?;
            file.write_all(&serde_json::to_vec_pretty(&report)?)?;
            file.sync_all()?;
            app.audit_path_access(
                job.agent,
                "panel",
                "job.export_report",
                Some(path.display().to_string()),
                "allowed",
                &job.id,
            )?;
            Ok(json!({"report_path":path}))
        }
    }
}

pub fn active_count(app: &App) -> Result<usize> {
    let mut count = 0;
    for job in app.store.jobs(&app.workspace.id(), None)? {
        if !reconcile(app, job)?.terminal() {
            count += 1;
        }
    }
    Ok(count)
}
