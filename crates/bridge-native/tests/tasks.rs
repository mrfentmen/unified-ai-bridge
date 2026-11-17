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

#[test]
fn continuation_preserves_remaining_budget_and_rejects_cross_tab_replay() {
    use bridge_native::continuation::{self, Control as H};
    let temp = tempfile::tempdir().unwrap();
    let (app, source) = fixture(&temp);
    let task = observe(&app,&source,"original-task");
    assert!(matches!(claim(&app,&task,"completed-read",30),RequestClaim::New));
    app.store.finish_response(&task.scope,"completed-read",0,"{\"status\":\"ok\",\"summary\":\"read real source\"}").unwrap();
    let prepare = || continuation::control(&app,&app.workspace.id(),&source.id,H::Prepare{task_id:task.task_id.clone()});
    let handoff = prepare().unwrap();
    assert_eq!(handoff["remaining_calls"],2);
    assert_eq!(prepare().unwrap()["handoff_id"],handoff["handoff_id"]);
    assert!(matches!(claim(&app,&task,"blocked-old",30),RequestClaim::Rejected(_)));
    assert!(tasks::control(&app,&app.workspace.id(),Control::Resume{task_id:task.id.clone()}).is_err());
    let target = app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/continued",None,None).unwrap().session;
    let bind = H::Bind{handoff_id:handoff["handoff_id"].as_str().unwrap().into(),task_id:"handoff-message".into()};
    let other = app.register_session(Provider::ChatGpt,2,1,"https://chatgpt.com/c/continued",None,None).unwrap().session;
    assert!(continuation::control(&app,&app.workspace.id(),&other.id,bind.clone()).is_err());
    assert!(continuation::control(&app,"wrong-workspace",&target.id,bind.clone()).is_err());
    let bound = continuation::control(&app,&app.workspace.id(),&target.id,bind.clone()).unwrap();
    let child: Task = serde_json::from_value(bound["task"].clone()).unwrap();
    assert_eq!(child.parent_task,Some(task.id.clone()));
    assert_eq!(child.call_limit,2);
    assert_eq!(child.handoff_count,1);
    assert_eq!(continuation::control(&app,&app.workspace.id(),&target.id,bind).unwrap()["task"]["id"],child.id);
    assert_eq!(app.store.task(&task.id).unwrap().unwrap().state,"stopped");
    assert!(matches!(claim(&app,&child,"new1",30),RequestClaim::New));
    assert!(matches!(claim(&app,&child,"new2",30),RequestClaim::New));
    assert!(matches!(claim(&app,&child,"new3",30),RequestClaim::Rejected(s) if s.contains("task_budget_exceeded")));
    assert!(app.store.verify_audit().unwrap().valid);
}

#[test]
fn continuation_blocks_uncertain_work_and_cancel_or_stop_cannot_grant_more_calls() {
    use bridge_native::continuation::{self, Control as H};
    let temp = tempfile::tempdir().unwrap();
    let (app, source) = fixture(&temp);
    let task = observe(&app,&source,"task");
    let prepare = || continuation::control(&app,&app.workspace.id(),&source.id,H::Prepare{task_id:"task".into()});
    assert!(matches!(claim(&app,&task,"running",30),RequestClaim::New));
    assert!(prepare().unwrap_err().to_string().contains("uncertain"));
    app.store.finish_response(&task.scope,"running",0,"{\"status\":\"ok\"}").unwrap();
    let h = prepare().unwrap();
    let handoff_id = h["handoff_id"].as_str().unwrap().to_string();
    continuation::control(&app,&app.workspace.id(),&source.id,H::Cancel{handoff_id:handoff_id.clone()}).unwrap();
    assert_eq!(app.store.task(&task.id).unwrap().unwrap().state,"paused");
    let target = app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/after-cancel",None,None).unwrap().session;
    assert!(continuation::control(&app,&app.workspace.id(),&target.id,H::Bind{handoff_id,task_id:"new".into()}).is_err());
    assert_eq!(detail(&app,&task)["task"]["remaining_calls"],2);
    assert!(serde_json::from_value::<H>(json!({"operation":"prepare","task_id":"x","provider":"claude"})).is_err());
}

#[test]
fn continuation_survives_host_restart_without_reopening_the_source_task() {
    use bridge_native::continuation::{self, Control as H};
    let temp = tempfile::tempdir().unwrap();
    let (app, source) = fixture(&temp);
    let task = observe(&app,&source,"task");
    let h = continuation::control(&app,&app.workspace.id(),&source.id,H::Prepare{task_id:"task".into()}).unwrap();
    let (home,config,workspace)=(app.home.clone(),app.config.clone(),app.workspace.clone());
    drop(app);
    let app = App::open_with_config(home,config,workspace,true).unwrap();
    assert_eq!(app.store.task(&task.id).unwrap().unwrap().state,"transferring");
    let target = app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/after-restart",None,None).unwrap().session;
    let h = continuation::control(&app,&app.workspace.id(),&target.id,H::Bind{handoff_id:h["handoff_id"].as_str().unwrap().into(),task_id:"new".into()}).unwrap();
    assert_eq!(h["task"]["call_limit"],3);
    assert_eq!(app.store.task(&task.id).unwrap().unwrap().state,"stopped");
}

#[test]
fn expired_or_stopped_handoffs_cannot_bind_and_lineage_keeps_receipts() {
    use bridge_native::continuation::{self,Control as H};
    let temp=tempfile::tempdir().unwrap();
    let (app,source)=fixture(&temp);
    let t=observe(&app,&source,"task");
    assert!(matches!(claim(&app,&t,"first",30),RequestClaim::New));
    app.store.finish_response(&t.scope,"first",0,"{\"status\":\"ok\",\"summary\":\"first completed receipt\"}").unwrap();
    let prepare=continuation::control(&app,&app.workspace.id(),&source.id,H::Prepare{task_id:"task".into()}).unwrap();
    let handoff_id=prepare["handoff_id"].as_str().unwrap();
    let conn=rusqlite::Connection::open(&app.home.database).unwrap();
    conn.execute("UPDATE task_handoffs SET record=json_set(record,'$.expires_at',0) WHERE id=?1",[handoff_id]).unwrap();
    let new=app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/expiry",None,None).unwrap().session;
    let bind=H::Bind{handoff_id:handoff_id.into(),task_id:"new".into()};
    assert!(continuation::control(&app,&app.workspace.id(),&new.id,bind.clone()).unwrap_err().to_string().contains("expired"));
    conn.execute("UPDATE task_handoffs SET record=json_set(record,'$.expires_at',?2) WHERE id=?1",rusqlite::params![handoff_id,chrono::Utc::now().timestamp()+1800]).unwrap();
    let bound=continuation::control(&app,&app.workspace.id(),&new.id,bind).unwrap();
    let child:Task=serde_json::from_value(bound["task"].clone()).unwrap();
    assert!(matches!(claim(&app,&child,"second",30),RequestClaim::New));
    app.store.finish_response(&child.scope,"second",0,"{\"status\":\"ok\",\"summary\":\"second completed receipt\"}").unwrap();
    let next=continuation::control(&app,&app.workspace.id(),&new.id,H::Prepare{task_id:"new".into()}).unwrap();
    assert_eq!(next["remaining_calls"],1);
    assert_eq!(next["receipt_count"],2);
    assert!(next["receipts"].to_string().contains("first completed receipt"));
    control(&app,Control::Stop{task_id:child.id});
    let other=app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/stopped",None,None).unwrap().session;
    assert!(continuation::control(&app,&app.workspace.id(),&other.id,H::Bind{handoff_id:next["handoff_id"].as_str().unwrap().into(),task_id:"later".into()}).unwrap_err().to_string().contains("stopped"));
}

#[test]
fn workspace_budget_controls_persist_validate_and_keep_existing_tasks_frozen() {
    use bridge_native::task_limits::{self, Control as B, Limits};
    let temp = tempfile::tempdir().unwrap();
    let (app, session) = fixture(&temp);
    let original = observe(&app, &session, "before-settings");
    let save = B::Save { limits: Limits { calls_per_task: 64, calls_per_response: 8 } };
    assert!(task_limits::control(&app, "wrong-workspace", save.clone()).is_err());
    for (task, response) in [(0,4), (257,4), (16,0), (16,33)] {
        assert!(task_limits::control(&app,&app.workspace.id(),B::Save {limits:Limits {
            calls_per_task:task,calls_per_response:response}}).is_err());
    }
    assert!(serde_json::from_value::<B>(json!({"operation":"save","limits":{"calls_per_task":64,"calls_per_response":8,"agent":"admin"}})).is_err());
    let (app, value) = task_limits::control(&app,&app.workspace.id(),save).unwrap();
    assert_eq!(value["limits"]["calls_per_task"],64);
    assert_eq!(detail(&app,&original)["task"]["remaining_calls"],3);
    assert_eq!(detail(&app,&original)["task"]["effective_response_limit"],2);
    let next = observe(&app,&session,"after-settings");
    assert_eq!((next.call_limit,next.response_limit),(64,8));
    for n in 0..3 { assert!(matches!(claim(&app,&original,&format!("old-{n}"),30),RequestClaim::New)); }
    assert!(matches!(claim(&app,&original,"old-exhausted",30),RequestClaim::Rejected(s) if s.contains("task_budget_exceeded")));
    let home=app.home.clone();
    let workspace=app.workspace.clone();
    drop(app);
    let config=home.load_config().unwrap();
    let app=App::open_with_config(home,config,workspace,true).unwrap();
    assert_eq!(task_limits::current(&app).calls_per_task,64);
    assert_eq!(detail(&app,&original)["task"]["remaining_calls"],0);
    let (reset, value)=task_limits::control(&app,&app.workspace.id(),B::Reset).unwrap();
    assert_eq!(value["override"],false);
    assert_eq!(task_limits::current(&reset).calls_per_task,3);
    assert_eq!(detail(&reset,&next)["task"]["effective_call_limit"],3);
    assert_eq!(detail(&reset,&original)["task"]["remaining_calls"],0);
    assert!(reset.store.verify_audit().unwrap().valid);
}

#[test]
fn workspace_budget_changes_are_isolated_and_handoffs_cannot_refill_them() {
    use bridge_native::{task_limits::{self,Control as B,Limits}, continuation::{self,Control as H}};
    let temp=tempfile::tempdir().unwrap();
    let (app,session)=fixture(&temp);
    let task=observe(&app,&session,"source");
    assert!(matches!(claim(&app,&task,"completed",30),RequestClaim::New));
    app.store.finish_response_timed(&task.scope,"completed",0,"{\"status\":\"ok\"}",1).unwrap();
    let (app,_)=task_limits::control(&app,&app.workspace.id(),B::Save{limits:Limits{calls_per_task:128,calls_per_response:8}}).unwrap();
    let prepared=continuation::control(&app,&app.workspace.id(),&session.id,H::Prepare{task_id:"source".into()}).unwrap();
    assert_eq!(prepared["remaining_calls"],2);
    let target=app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/new-budget-chat",None,None).unwrap().session;
    let bound=continuation::control(&app,&app.workspace.id(),&target.id,H::Bind{handoff_id:prepared["handoff_id"].as_str().unwrap().into(),task_id:"handoff".into()}).unwrap();
    assert_eq!(bound["task"]["call_limit"],2);
    assert_eq!(bound["task"]["response_limit"],2);
    let other=app.create_workspace("second",temp.path().join("second-workspace").to_str().unwrap(),Profile::General).unwrap();
    let other=app.select_workspace(&other.id).unwrap();
    assert_eq!(task_limits::current(&other).calls_per_task,3);
    assert!(task_limits::control(&other,&app.workspace.id(),B::Reset).is_err());
    assert_eq!(task_limits::current(&app).calls_per_task,128);
}

#[test]
fn long_task_handoff_selects_the_latest_receipts_not_the_first_page() {
    use bridge_native::{task_limits::{self,Control as B,Limits},continuation::{self,Control as H}};
    let temp=tempfile::tempdir().unwrap();
    let (app,session)=fixture(&temp);
    let (app,_)=task_limits::control(&app,&app.workspace.id(),B::Save{limits:Limits{calls_per_task:256,calls_per_response:4}}).unwrap();
    let task=observe(&app,&session,"long-task");
    for n in 0..150 {
        let response=format!("long-{n}");
        assert!(matches!(claim(&app,&task,&response,30),RequestClaim::New));
        app.store.finish_response_timed(&task.scope,&response,0,"{\"status\":\"ok\"}",1).unwrap();
    }
    let prepared=continuation::control(&app,&app.workspace.id(),&session.id,H::Prepare{task_id:"long-task".into()}).unwrap();
    assert_eq!(prepared["remaining_calls"],106);
    assert_eq!(prepared["receipt_count"],150);
    assert_eq!(prepared["more_receipts"],true);
    let rows=prepared["receipts"].as_array().unwrap();
    assert_eq!(rows.len(),24);
    assert_eq!(rows.first().unwrap()["response_id"],"long-126");
    assert_eq!(rows.last().unwrap()["response_id"],"long-149");
}

#[test]
fn lowering_budgets_during_a_prepared_handoff_cannot_restore_spent_calls() {
    use bridge_native::{task_limits::{self,Control as B,Limits},continuation::{self,Control as H}};
    let temp=tempfile::tempdir().unwrap();
    let (app,session)=fixture(&temp);
    let task=observe(&app,&session,"lowered");
    assert!(matches!(claim(&app,&task,"spent",30),RequestClaim::New));
    app.store.finish_response_timed(&task.scope,"spent",0,"{\"status\":\"ok\"}",1).unwrap();
    let h=continuation::control(&app,&app.workspace.id(),&session.id,H::Prepare{task_id:"lowered".into()}).unwrap();
    assert_eq!(h["remaining_calls"],2);
    let (app,_)=task_limits::control(&app,&app.workspace.id(),B::Save{limits:Limits{calls_per_task:1,calls_per_response:1}}).unwrap();
    let target=app.register_session(Provider::ChatGpt,1,1,"https://chatgpt.com/c/lowered-budget",None,None).unwrap().session;
    let result=continuation::control(&app,&app.workspace.id(),&target.id,H::Bind{handoff_id:h["handoff_id"].as_str().unwrap().into(),task_id:"new".into()});
    assert!(result.unwrap_err().to_string().contains("exhausted"));
    assert_eq!(detail(&app,&task)["task"]["remaining_calls"],0);
}
