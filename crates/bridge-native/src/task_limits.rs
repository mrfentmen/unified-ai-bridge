//! User-selected workspace ceilings. Observed tasks keep their original ceiling.
use crate::app::App;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub calls_per_task: u32,
    pub calls_per_response: u32,
}

impl Limits {
    pub fn validate(self) -> Result<()> {
        if !(1..=256).contains(&self.calls_per_task)
            || !(1..=32).contains(&self.calls_per_response)
        {
            bail!("Choose 1–256 calls per task and 1–32 per assistant response.");
        }
        Ok(())
    }
}

pub fn current(app: &App) -> Limits {
    app.config.workspace_task_limits.get(&app.workspace.id()).copied().unwrap_or(Limits {
        calls_per_task: app.config.max_calls_per_task,
        calls_per_response: app.config.max_calls_per_response,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Status,
    Save { limits: Limits },
    Reset,
}

pub fn control(app: &Arc<App>, workspace: &str, action: Control) -> Result<(Arc<App>, Value)> {
    if workspace != app.workspace.id() {
        bail!("Workspace changed; refresh task budgets before saving.");
    }
    let next = if matches!(action, Control::Status) {
        app.clone()
    } else {
        let before = current(app);
        let mut config = app.config.clone();
        match action {
            Control::Save { limits } => {
                limits.validate()?;
                config.workspace_task_limits.insert(workspace.into(), limits);
            }
            Control::Reset => { config.workspace_task_limits.remove(workspace); }
            Control::Status => unreachable!(),
        }
        // These controls never originate in a tool call or provider content script.
        app.audit_workspace("task.budgets_requested", json!({"workspace_id":workspace,
            "before":before,"override":config.workspace_task_limits.get(workspace)}))?;
        app.home.save_config(&config)?;
        let next = Arc::new(App {
            home: app.home.clone(), config, workspace: app.workspace.clone(),
            store: app.store.clone(), approvals: app.approvals.clone(),
            browser_files: app.browser_files.clone(),
            events: app.events.clone(), interactive: app.interactive,
        });
        next.audit_workspace("task.budgets_applied", json!({"workspace_id":workspace,"limits":current(&next)}))?;
        next
    };
    let value = json!({"limits":current(&next),
        "workspace_id":workspace,"override":next.config.workspace_task_limits.contains_key(workspace),
        "maximum":{"calls_per_task":256,"calls_per_response":32}});
    Ok((next,value))
}
