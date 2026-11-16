use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    store::{RequestClaim, ResponseRequest, SessionRecord},
    tasks::{self, Control, Task},
    wire::BridgeEvent,
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, SessionRecord) {
    let base = temp.path().join("home");
    let root = temp.path().join("workspace");
    let home = HomePaths {
        base: base.clone(),
        config_file: base.join("config.toml"),
        database: base.join("bridge.sqlite3"),
        logs_dir: base.join("logs"),
        default_project: root.clone(),
        native_hosts_dir: base.join("hosts"),
        install_dir: base.join("bin"),
    };
    let mut workspace = Workspace::initialize_profile(&root, Profile::General).unwrap();
    workspace.permission_mode = bridge_core::workspace::PermissionMode::Strict;
    let config = Config {
        project_root: workspace.root.clone(),
        max_calls_per_task: 3,
        max_calls_per_response: 2,
        approval_timeout_secs: 10,
        ..Config::default()
    };
    home.save_config(&config).unwrap();
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let session = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/tasks",
            None,
            None,
        )
        .unwrap()
        .session;
    (app, session)
}
fn observe(app: &App, session: &SessionRecord, id: &str) -> Task {
    let scope = serde_json::to_string(&(
        &app.workspace.root,
        &session.provider,
        &session.conversation,
    ))
    .unwrap();
    app.store
        .ensure_task(&Task::observed(app, session, &scope, id))
        .unwrap()
}
fn claim(app: &App, task: &Task, response: &str, timeout: u64) -> RequestClaim {
    app.store
        .claim_response(ResponseRequest {
            scope: &task.scope,
            response,
            task: &task.task_id,
            index: 0,
            tool: "read_file",
            arguments: &json!({"path":"files/source.txt"}),
            response_limit: 32,
            task_limit: 256,
            timeout_secs: timeout,
        })
        .unwrap()
}
fn control(app: &App, action: Control) -> Value {
    tasks::control(app, &app.workspace.id(), action).unwrap()
}
fn detail(app: &App, t: &Task) -> Value {
    control(
        app,
        Control::Detail {
            task_id: t.id.clone(),
            after: 0,
        },
    )
}

#[test]
fn task_budgets_survive_pause_resume_reconnect_and_configuration_increase() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    assert!(matches!(claim(&app, &task, "r1", 30), RequestClaim::New));
    app.store
        .finish_response_timed(&task.scope, "r1", 0, "{\"status\":\"ok\"}", 7)
        .unwrap();
    control(
        &app,
        Control::Pause {
            task_id: task.id.clone(),
        },
    );
    assert!(
        matches!(claim(&app,&task,"r2",30),RequestClaim::Rejected(s) if s.contains("task_paused"))
    );
    let home = app.home.clone();
    let config = app.config.clone();
    let workspace = app.workspace.clone();
    drop(app);
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    assert_eq!(detail(&app, &task)["task"]["state"], "paused");
    assert_eq!(detail(&app, &task)["task"]["remaining_calls"], 2);
    control(
        &app,
        Control::Resume {
            task_id: task.id.clone(),
        },
    );
    assert!(
        matches!(claim(&app,&task,"r2",30),RequestClaim::Completed(s) if s.contains("task_paused"))
    );
    assert!(matches!(claim(&app,&task,"r1",30),RequestClaim::Completed(s) if s.contains("ok")));
    assert!(matches!(claim(&app, &task, "r3", 30), RequestClaim::New));
    assert!(matches!(claim(&app, &task, "r4", 30), RequestClaim::New));
    assert!(
        matches!(claim(&app,&task,"r5",30),RequestClaim::Rejected(s) if s.contains("task_budget_exceeded"))
    );
    assert_eq!(detail(&app, &task)["task"]["remaining_calls"], 0);
    assert!(app.store.verify_audit().unwrap().valid);
}

#[test]
fn stopped_tasks_cannot_resume_or_change_identity_but_new_user_tasks_can_start() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    assert!(matches!(claim(&app, &task, "r1", 30), RequestClaim::New));
    control(
        &app,
        Control::Stop {
            task_id: task.id.clone(),
        },
    );
    assert!(
        matches!(claim(&app,&task,"r2",30),RequestClaim::Rejected(s) if s.contains("task_stopped"))
    );
    assert!(tasks::control(
        &app,
        &app.workspace.id(),
        Control::Resume {
            task_id: task.id.clone()
        }
    )
    .is_err());
    assert!(tasks::control(&app, "another-workspace", Control::List).is_err());
    let mut second = task.clone();
    second.task_id = "user-2".into();
    assert!(
        matches!(claim(&app,&second,"r1",30),RequestClaim::Rejected(s) if s.contains("request_conflict"))
    );
    let new = observe(&app, &session, "user-2");
    assert!(matches!(claim(&app, &new, "r3", 30), RequestClaim::New));
    assert!(serde_json::from_value::<Control>(
        json!({"operation":"resume","task_id":task.id,"provider":"claude"})
    )
    .is_err());
}

#[tokio::test]
async fn pause_resume_pulse_blocks_a_queued_write_without_spending_a_new_grant() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    control(
        &app,
        Control::Pause {
            task_id: task.id.clone(),
        },
    );
    control(
        &app,
        Control::Resume {
            task_id: task.id.clone(),
        },
    );
    let r = tasks::execute(
        &app,
        &task.id,
        task.revision,
        Agent::ChatGpt,
        &session.id,
        "write_document",
        json!({"path":"files/queued.txt","content":"never publish"}),
    )
    .await;
    assert_eq!(r.code.as_deref(), Some("task_halted"));
    assert!(!app.workspace.root.join("files/queued.txt").exists());
    assert_eq!(app.approvals.pending_count(), 0);
    let current = app.store.task(&task.id).unwrap().unwrap();
    std::fs::write(
        app.workspace.root.join("files/source.txt"),
        "actual content",
    )
    .unwrap();
    let read = tasks::execute(
        &app,
        &current.id,
        current.revision,
        Agent::ChatGpt,
        &session.id,
        "read_file",
        json!({"path":"files/source.txt"}),
    )
    .await;
    assert_eq!(read.status, "ok");
    assert!(read.output.unwrap().contains("actual content"));
}

#[tokio::test]
async fn pause_closes_waiting_approval_and_late_approval_cannot_release_write() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    let mut events = app.events.subscribe();
    let a = app.clone();
    let t = task.clone();
    let sid = session.id.clone();
    let pending = tokio::spawn(async move {
        tasks::execute(
            &a,
            &t.id,
            t.revision,
            Agent::ChatGpt,
            &sid,
            "write_document",
            json!({"path":"files/pending.txt","content":"not authorized after pause"}),
        )
        .await
    });
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                break approval;
            }
        }
    })
    .await
    .unwrap();
    control(
        &app,
        Control::Pause {
            task_id: task.id.clone(),
        },
    );
    control(
        &app,
        Control::Resume {
            task_id: task.id.clone(),
        },
    );
    // Even if the panel's approval wins the poll race, the revision check fails.
    app.approvals.resolve(&approval.id, true);
    let result = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.code.as_deref(), Some("task_halted"));
    assert!(!app.workspace.root.join("files/pending.txt").exists());
    assert_eq!(app.approvals.pending_count(), 0);
    assert!(!app.approvals.resolve(&approval.id, true));
    assert!(app.store.verify_audit().unwrap().valid);
}

#[test]
fn report_has_bounded_receipts_unknown_outcomes_and_no_raw_content() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    assert!(matches!(claim(&app, &task, "r1", 0), RequestClaim::New));
    assert!(matches!(claim(&app, &task, "r2", 30), RequestClaim::New));
    app.store.finish_response_timed(&task.scope,"r2",0,&json!({"status":"ok","summary":"read source","output":"PRIVATE FILE CONTENT","data":{"change_id":"receipt-1"}}).to_string(),12).unwrap();
    assert!(matches!(claim(&app, &task, "r3", 30), RequestClaim::New));
    app.store
        .finish_response(
            &task.scope,
            "r3",
            0,
            &json!({"status":"error","code":"approval_denied","error":"denied"}).to_string(),
        )
        .unwrap();
    let snapshot = detail(&app, &task);
    assert_eq!(snapshot["task"]["counts"]["unverified"], 1);
    assert_eq!(snapshot["task"]["counts"]["completed"], 1);
    assert_eq!(snapshot["task"]["counts"]["denied"], 1);
    let first = app.store.task_steps(&task, 0, 1).unwrap();
    let next = app
        .store
        .task_steps(&task, first[0]["cursor"].as_i64().unwrap(), 1)
        .unwrap();
    assert_eq!(next[0]["duration_ms"], 12);
    assert_eq!(next[0]["change_id"], "receipt-1");
    let report = control(
        &app,
        Control::ExportReport {
            task_id: task.id.clone(),
        },
    );
    let path = std::path::Path::new(report["report_path"].as_str().unwrap());
    let bytes = std::fs::read_to_string(path).unwrap();
    assert!(!bytes.contains("PRIVATE FILE CONTENT"));
    assert_eq!(
        serde_json::from_str::<Value>(&bytes).unwrap()["steps"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        matches!(claim(&app,&task,"r1",60),RequestClaim::Completed(s) if s.contains("request_timeout"))
    );
    assert!(!app
        .store
        .finish_response(&task.scope, "r1", 0, "{}")
        .unwrap());
    assert!(app.store.verify_audit().unwrap().valid);
}

#[test]
fn workspace_and_provider_scopes_keep_control_and_budgets_separate() {
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let task = observe(&app, &session, "user-1");
    control(
        &app,
        Control::Stop {
            task_id: task.id.clone(),
        },
    );
    let other = app
        .register_session(
            Provider::Claude,
            2,
            1,
            "https://claude.ai/chat/tasks",
            None,
            None,
        )
        .unwrap()
        .session;
    let second = observe(&app, &other, "user-1");
    assert_ne!(task.id, second.id);
    assert!(matches!(claim(&app, &second, "r1", 60), RequestClaim::New));
    let next = app
        .create_workspace(
            "other",
            temp.path().join("other").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let switched = app.select_workspace(&next.id).unwrap();
    assert!(tasks::control(
        &switched,
        &next.id,
        Control::Detail {
            task_id: task.id,
            after: 0
        }
    )
    .is_err());
    assert!(control(&switched, Control::List)["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
    // Calls already admitted against the old App continue to use its original root.
    assert_eq!(
        app.store.task(&second.id).unwrap().unwrap().root,
        app.workspace.root.display().to_string()
    );
}
