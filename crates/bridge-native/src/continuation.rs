//! Explicit, same-provider/task continuation. A single transaction freezes the
//! old admission budget; a second binds its remainder to a fresh conversation.
//! No old tool request is submitted to the new conversation for execution.
use crate::{app::App, store::SessionRecord, tasks::Task};
use anyhow::{ensure, Result};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Prepare { task_id: String },
    Bind { handoff_id: String, task_id: String },
    Cancel { handoff_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Handoff {
    id: String,
    source: Task,
    tab_id: i64,
    expires_at: i64,
    remaining_calls: u32,
    target: Option<Task>,
    cancelled: bool,
}

fn scope(app: &App, session: &SessionRecord) -> Result<String> {
    Ok(serde_json::to_string(&(
        &app.workspace.root,
        &session.provider,
        session
            .conversation
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("wait for a conversation ID"))?,
    ))?)
}

pub fn control(app: &App, workspace: &str, session_id: &str, action: Control) -> Result<Value> {
    let (session, agent) = app
        .session_agent(session_id)?
        .ok_or_else(|| anyhow::anyhow!("unknown session"))?;
    ensure!(
        workspace == app.workspace.id() && session.workspace_id == workspace,
        "workspace changed; handoff refused"
    );
    // Journal intent before changing admission state. Never record user prompts.
    app.audit_workspace(
        "task.handoff_requested",
        json!({"session":session_id,"action":format!("{action:?}")}),
    )?;
    let handoff = match action {
        Control::Prepare { task_id } => {
            ensure!(
                bridge_core::execution::valid_turn_id(&task_id),
                "invalid task ID"
            );
            let observed = Task::observed(app, &session, &scope(app, &session)?, &task_id);
            let mut conn = app.store.conn.lock().expect("store mutex");
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let raw: String = tx.query_row(
                "SELECT record FROM chat_tasks WHERE id=?1",
                [&observed.id],
                |r| r.get(0),
            )?;
            let mut source: Task = serde_json::from_str(&raw)?;
            ensure!(
                source.workspace_id == workspace && source.provider == session.provider,
                "task ownership mismatch"
            );
            let existing: Option<String> = tx
                .query_row(
                    "SELECT record FROM task_handoffs WHERE task_id=?1",
                    [&source.id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(raw) = existing {
                let previous: Handoff = serde_json::from_str(&raw)?;
                ensure!(
                    !previous.cancelled
                        && previous.target.is_none()
                        && previous.tab_id == session.tab_id
                        && source.state == "transferring"
                        && previous.expires_at > chrono::Utc::now().timestamp(),
                    "handoff already consumed, expired or cancelled; inspect task controls"
                );
                previous
            } else {
                ensure!(
                    source.state == "running",
                    "only a running task can continue"
                );
                ensure!(
                    source.handoff_count < 3,
                    "this task reached its three-handoff limit"
                );
                let uncertain: u32 = tx.query_row("SELECT COUNT(*) FROM response_requests WHERE scope=?1 AND task_id=?2 AND (status='running' OR json_extract(result_json,'$.code') IN ('request_timeout','request_in_progress'))", params![source.scope, source.task_id], |r|r.get(0))?;
                ensure!(
                    uncertain == 0,
                    "task has running or uncertain calls; inspect their outcomes before continuing"
                );
                let job_owner = crate::fsops::sha256_bytes(format!("{}\0{}\0{}",session.provider,agent.dir(),source.conversation).as_bytes());
                let active_jobs: u32 = tx.query_row("SELECT COUNT(*) FROM jobs WHERE workspace_id=?1 AND owner=?2 AND json_extract(record,'$.state') NOT IN ('completed','failed','cancelled')", params![workspace,job_owner], |r|r.get(0))?;
                ensure!(active_jobs == 0, "conversation has running or interrupted command jobs; review their receipts before continuing");
                let used: u32 = tx.query_row("SELECT COUNT(*) FROM response_requests WHERE scope=?1 AND task_id=?2 AND status!='rejected'", params![source.scope,source.task_id], |r|r.get(0))?;
                let remaining = source
                    .call_limit
                    .min(crate::task_limits::current(app).calls_per_task)
                    .saturating_sub(used);
                ensure!(
                    remaining > 0,
                    "task budget exhausted; a new chat cannot reset it"
                );
                source.state = "transferring".into();
                source.revision += 1;
                source.updated_at = chrono::Utc::now().to_rfc3339();
                let handoff = Handoff {
                    id: crate::approvals::ApprovalManager::new_id(),
                    source: source.clone(),
                    tab_id: session.tab_id,
                    expires_at: chrono::Utc::now().timestamp() + 1800,
                    remaining_calls: remaining,
                    target: None,
                    cancelled: false,
                };
                tx.execute(
                    "UPDATE chat_tasks SET record=?2 WHERE id=?1",
                    params![source.id, serde_json::to_string(&source)?],
                )?;
                tx.execute(
                    "INSERT INTO task_handoffs VALUES (?1,?2,?3)",
                    params![handoff.id, source.id, serde_json::to_string(&handoff)?],
                )?;
                tx.commit()?;
                handoff
            }
        }
        action => {
            let id = match &action {
                Control::Bind { handoff_id, .. } | Control::Cancel { handoff_id } => handoff_id,
                _ => unreachable!(),
            };
            let mut conn = app.store.conn.lock().expect("store mutex");
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let raw: String =
                tx.query_row("SELECT record FROM task_handoffs WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
            let mut h: Handoff = serde_json::from_str(&raw)?;
            ensure!(
                h.tab_id == session.tab_id
                    && h.source.provider == session.provider
                    && h.source.workspace_id == workspace,
                "handoff belongs to another tab, provider or workspace"
            );
            let raw: String = tx.query_row(
                "SELECT record FROM chat_tasks WHERE id=?1",
                [&h.source.id],
                |r| r.get(0),
            )?;
            let mut source: Task = serde_json::from_str(&raw)?;
            match action {
                Control::Bind { task_id, .. } => {
                    ensure!(
                        bridge_core::execution::valid_turn_id(&task_id),
                        "invalid new task ID"
                    );
                    let mut target =
                        Task::observed(app, &session, &scope(app, &session)?, &task_id);
                    ensure!(
                        target.conversation != source.conversation,
                        "handoff requires a fresh conversation"
                    );
                    if let Some(existing) = &h.target {
                        ensure!(
                            existing.id == target.id && !h.cancelled,
                            "handoff was already consumed by another task"
                        );
                        return Ok(
                            json!({"handoff_id":h.id,"task":existing,"remaining_calls":existing.call_limit,"bound":true}),
                        );
                    }
                    ensure!(
                        !h.cancelled && h.expires_at > chrono::Utc::now().timestamp(),
                        "handoff expired or cancelled"
                    );
                    ensure!(
                        source.state == "transferring" && source.revision == h.source.revision,
                        "source task was stopped or changed"
                    );
                    target.parent_task = Some(source.id.clone());
                    target.handoff_count = source.handoff_count + 1;
                    let used: u32 = tx.query_row("SELECT COUNT(*) FROM response_requests WHERE scope=?1 AND task_id=?2 AND status!='rejected'", params![source.scope,source.task_id], |r|r.get(0))?;
                    target.call_limit = h.remaining_calls.min(source.call_limit
                        .min(crate::task_limits::current(app).calls_per_task).saturating_sub(used));
                    ensure!(target.call_limit > 0, "task budget was lowered and is now exhausted; handoff refused");
                    h.remaining_calls = target.call_limit;
                    target.response_limit =
                        source.response_limit.min(crate::task_limits::current(app).calls_per_response);
                    source.state = "stopped".into();
                    source.revision += 1;
                    // INSERT (not replace): an existing task must never get a new budget.
                    tx.execute(
                        "INSERT INTO chat_tasks VALUES (?1,?2,?3,?4,?5)",
                        params![
                            target.id,
                            workspace,
                            target.scope,
                            target.task_id,
                            serde_json::to_string(&target)?
                        ],
                    )?;
                    h.target = Some(target);
                }
                Control::Cancel { .. } => {
                    ensure!(
                        h.target.is_none(),
                        "bound handoffs cannot be cancelled; stop the new task"
                    );
                    if !h.cancelled {
                        ensure!(
                            matches!(source.state.as_str(), "transferring" | "stopped"),
                            "source task changed"
                        );
                        if source.state != "stopped" {
                            source.state = "paused".into();
                        }
                        source.revision += 1;
                        h.cancelled = true;
                    }
                }
                _ => unreachable!(),
            }
            source.updated_at = chrono::Utc::now().to_rfc3339();
            tx.execute(
                "UPDATE chat_tasks SET record=?2 WHERE id=?1",
                params![source.id, serde_json::to_string(&source)?],
            )?;
            tx.execute(
                "UPDATE task_handoffs SET record=?2 WHERE id=?1",
                params![h.id, serde_json::to_string(&h)?],
            )?;
            tx.commit()?;
            h
        }
    };
    let state = if handoff.cancelled {
        "cancelled"
    } else if handoff.target.is_some() {
        "bound"
    } else {
        "prepared"
    };
    app.audit_workspace("task.handoff_applied",json!({"handoff_id":handoff.id,"source":handoff.source.id,"target":handoff.target.as_ref().map(|t|&t.id),"state":state,"remaining_calls":handoff.remaining_calls}))?;
    app.record_activity(
        agent,
        session_id,
        "task_handoff",
        "task",
        None,
        &format!(
            "Task handoff {state}; {} calls remain",
            handoff.remaining_calls
        ),
        "allowed",
    )?;
    let mut lineage=vec![handoff.source.clone()];
    while let Some(parent)=lineage.last().and_then(|t|t.parent_task.as_deref()) {
        ensure!(lineage.len()<4,"invalid handoff lineage");
        let task=app.store.task(parent)?.ok_or_else(||anyhow::anyhow!("missing parent task"))?;
        ensure!(task.workspace_id==workspace && task.provider==session.provider,"invalid parent task ownership");
        lineage.push(task);
    }
    let mut segments=Vec::new();
    let mut remaining=24;
    let mut count=0;
    for task in &lineage {
        count+=app.store.task_step_count(task)?;
        if remaining>0 {
            let rows=app.store.recent_task_steps(task,remaining)?;
            remaining-=rows.len() as u32;
            segments.push(rows);
        }
    }
    let receipts=segments.into_iter().rev().flatten().collect::<Vec<_>>();
    Ok(
        json!({"handoff_id":handoff.id,"task":handoff.target,"remaining_calls":handoff.remaining_calls,
        "bound":state=="bound","cancelled":handoff.cancelled,"expires_at":handoff.expires_at,
        "source_conversation":handoff.source.conversation,"receipts":receipts,
        "more_receipts":count>receipts.len() as u64,"receipt_count":count}),
    )
}
