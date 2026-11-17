//! Durable admission controls for observed chat tasks, not an AI scheduler.
//! Already executing operations drain. No paused/rejected slot is replayed.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{app::App, failure::ToolFailure, store::SessionRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub scope: String,
    pub task_id: String,
    pub workspace_id: String,
    pub root: String,
    pub provider: String,
    pub conversation: String,
    pub original_session: String,
    pub state: String,
    pub revision: u64,
    pub response_limit: u32,
    pub call_limit: u32,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub parent_task: Option<String>,
    #[serde(default)]
    pub handoff_count: u32,
}

impl Task {
    pub fn observed(app: &App, session: &SessionRecord, scope: &str, task: &str) -> Self {
        let id = hex::encode(Sha256::digest(
            serde_json::to_vec(&(scope, task)).expect("string tuple"),
        ));
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id,
            scope: scope.into(),
            task_id: task.into(),
            workspace_id: app.workspace.id(),
            root: app.workspace.root.display().to_string(),
            provider: session.provider.clone(),
            conversation: session.conversation.clone().unwrap_or_default(),
            original_session: session.id.clone(),
            state: "running".into(),
            revision: 0,
            response_limit: crate::task_limits::current(app).calls_per_response,
            call_limit: crate::task_limits::current(app).calls_per_task,
            created_at: now.clone(),
            updated_at: now,
            parent_task: None,
            handoff_count: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    List,
    Detail {
        task_id: String,
        #[serde(default)]
        after: i64,
    },
    Pause {
        task_id: String,
    },
    Resume {
        task_id: String,
    },
    Stop {
        task_id: String,
    },
    ExportReport {
        task_id: String,
    },
}

const NOTES: &[&str] = &[
    "This is a snapshot of observed tool requests, not proof that the user's whole task is complete.",
    "A completed run_command request may only have queued a background job. Check the linked job receipt for its actual outcome.",
    "Pause/Stop block new and queued calls and close waiting approvals. Already executing operations may finish; committed changes are not undone.",
    "Background command jobs have separate Stop controls and are not terminated by these admission controls.",
    "Resume permits new response slots with the original remaining budget. It does not send a chat message, restart the model or replay a rejected/uncertain call.",
    "Unknown outcomes require inspection. Paths are requested targets; only successful result receipts establish changes. External programs may change additional files.",
];

pub fn control(app: &App, workspace: &str, action: Control) -> Result<Value> {
    if workspace != app.workspace.id() {
        bail!("workspace changed; refresh chat task controls");
    }
    if matches!(action, Control::List) {
        let tasks = app
            .store
            .tasks(workspace)?
            .iter()
            .map(|task| snapshot(app, task))
            .collect::<Result<Vec<_>>>()?;
        return Ok(json!({"tasks": tasks, "notes": NOTES}));
    }
    let id = match &action {
        Control::Detail { task_id, .. }
        | Control::Pause { task_id }
        | Control::Resume { task_id }
        | Control::Stop { task_id }
        | Control::ExportReport { task_id } => task_id,
        Control::List => unreachable!(),
    };
    let task = app
        .store
        .task(id)?
        .filter(|t| t.workspace_id == workspace)
        .ok_or_else(|| anyhow::anyhow!("task is outside this workspace or unknown"))?;
    match action {
        Control::List => unreachable!(),
        Control::Detail { after, .. } => {
            if after < 0 {
                bail!("invalid task receipt cursor");
            }
            let rows = app.store.task_steps(&task, after, 50)?;
            let next = rows
                .last()
                .and_then(|v| v["cursor"].as_i64())
                .unwrap_or(after);
            Ok(
                json!({"task": snapshot(app, &task)?, "steps": rows, "next_cursor": next, "notes": NOTES}),
            )
        }
        Control::Pause { .. } | Control::Resume { .. } | Control::Stop { .. } => {
            let state = match action {
                Control::Pause { .. } => "paused",
                Control::Resume { .. } => "running",
                _ => "stopped",
            };
            // Audit the intent first. Failed persistence cannot silently grant resume.
            app.audit_workspace(
                "task.control_requested",
                json!({"task_id":task.id,"from":task.state,"to":state}),
            )?;
            let updated = app.store.set_task_state(&task.id, task.revision, state)?;
            app.audit_workspace(
                "task.control_applied",
                json!({"task_id":updated.id,"state":updated.state,"revision":updated.revision}),
            )?;
            Ok(json!({"task":snapshot(app,&updated)?, "notes":NOTES}))
        }
        Control::ExportReport { .. } => {
            use std::io::Write;
            let directory = app.home.base.join("reports/tasks");
            std::fs::create_dir_all(&directory)?;
            let path = directory.join(format!(
                "{}-{}.json",
                task.id,
                crate::approvals::ApprovalManager::new_id()
            ));
            // Bounded summaries, never raw arguments or file contents.
            let steps = app.store.task_steps(&task, 0, 512)?;
            let report = json!({"task":snapshot(app,&task)?,"steps":steps,"step_limit":512,
                "more_steps":app.store.task_step_count(&task)? > 512,
                "audit_database":app.home.database,"audit_log":app.home.base.join("audit.jsonl"),
                "notes":NOTES});
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
            app.audit_workspace("task.export_report", json!({"task_id":task.id,"path":path}))?;
            Ok(json!({"report_path":path}))
        }
    }
}

fn snapshot(app: &App, task: &Task) -> Result<Value> {
    let mut value = serde_json::to_value(task)?;
    value.as_object_mut().unwrap().remove("scope");
    let counts = app.store.task_counts(task)?;
    value["effective_call_limit"] = json!(task.call_limit.min(crate::task_limits::current(app).calls_per_task));
    value["effective_response_limit"] = json!(task.response_limit.min(crate::task_limits::current(app).calls_per_response));
    value["remaining_calls"] = json!(task
        .call_limit
        .min(crate::task_limits::current(app).calls_per_task)
        .saturating_sub(counts["reserved_calls"].as_u64().unwrap_or(0) as u32));
    value["counts"] = counts;
    value["observed_age_seconds"] = json!(chrono::DateTime::parse_from_rfc3339(&task.created_at)
        .map(|t| (chrono::Utc::now().timestamp() - t.timestamp()).max(0))
        .unwrap_or(0));
    Ok(value)
}

tokio::task_local! { static EXECUTION: (String, u64); }

pub fn original_task_id(app: &App) -> Result<Option<String>, ToolFailure> {
    let Ok((id, _)) = EXECUTION.try_with(Clone::clone) else { return Ok(None); };
    app.store.task(&id).map_err(|e|ToolFailure::Internal(e.to_string()))?
        .map(|task|Some(task.task_id)).ok_or_else(||ToolFailure::Internal("Missing browser task.".into()))
}

/// Install only around a browser task's dispatch. Panel tools/MCP remain separate.
pub async fn execute(
    app: &App,
    id: &str,
    revision: u64,
    agent: bridge_core::provider::Agent,
    session: &str,
    tool: &str,
    arguments: Value,
) -> crate::wire::ToolResultMessage {
    EXECUTION
        .scope((id.into(), revision), async {
            if let Err(err) = checkpoint(app) {
                let _ = app.audit_denial(agent, session, tool, None, &err.to_string(), "denied");
                return crate::wire::ToolResultMessage::error(tool, err.code(), err.to_string());
            }
            crate::tools::dispatch(app, agent, session, tool, arguments).await
        })
        .await
}

pub fn checkpoint(app: &App) -> Result<(), ToolFailure> {
    let Ok((id, revision)) = EXECUTION.try_with(Clone::clone) else {
        return Ok(());
    };
    let task = app
        .store
        .task(&id)
        .map_err(|e| ToolFailure::Internal(e.to_string()))?
        .ok_or_else(|| ToolFailure::Internal("task record is missing".into()))?;
    if task.state != "running" || task.revision != revision {
        return Err(ToolFailure::TaskHalted(format!("Chat task was paused or stopped. This call will not be replayed. Current admission state: {}.",task.state)));
    }
    Ok(())
}

pub async fn wait_until_halted(app: &App) -> ToolFailure {
    if EXECUTION.try_with(|_| ()).is_err() {
        return std::future::pending().await;
    }
    loop {
        if let Err(err) = checkpoint(app) {
            return err;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
