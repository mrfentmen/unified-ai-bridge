use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{AllowedCommand, Config, HomePaths},
    tools,
    wire::ToolResultMessage,
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String) {
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
    let workspace = Workspace::initialize_profile(&root, Profile::General).unwrap();
    let config = Config {
        project_root: workspace.root.clone(),
        ..Config::default()
    };
    home.save_config(&config).unwrap();
    std::fs::create_dir_all(&home.install_dir).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_unified-ai-bridge-native"),
        home.installed_binary(),
    )
    .unwrap();
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let sid = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/jobs",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}
fn grant(app: &Arc<App>, id: &str, program: &str, args: &[&str], timeout: u64) -> Arc<App> {
    let program = [format!("/usr/bin/{program}"), format!("/bin/{program}")]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .unwrap();
    app.grant_command(AllowedCommand {
        command_id: id.into(),
        program,
        fixed_args: args.iter().map(|s| s.to_string()).collect(),
        allowed_args: vec![],
        cwd: ".".into(),
        timeout_secs: timeout,
        max_output_bytes: 128,
        writes: false,
        always_allow: true,
    })
    .unwrap()
}
async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tools::dispatch(app, Agent::ChatGpt, sid, tool, args).await
}
fn ok(value: ToolResultMessage) -> Value {
    assert_eq!(value.status, "ok", "{value:?}");
    value.data.unwrap()
}
async fn terminal(app: &App, sid: &str, id: &str) -> Value {
    let start = Instant::now();
    loop {
        let status = ok(call(app, sid, "job_status", json!({"job_id":id})).await);
        if ["completed", "failed", "cancelled", "interrupted"]
            .contains(&status["state"].as_str().unwrap())
        {
            return status;
        }
        assert!(start.elapsed() < Duration::from_secs(12), "{status}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn running(app: &App, sid: &str, id: &str) {
    let started = Instant::now();
    loop {
        let value = ok(call(app, sid, "job_status", json!({"job_id":id})).await);
        if value["process_id"].as_u64().is_some() {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(8), "{value}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn real_worker_persists_output_across_host_reload_and_same_conversation_reconnect() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "print", "printf", &["durable output\n"], 5);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"print","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    let reopened = App::load(app.home.clone(), true).unwrap();
    let next = reopened
        .register_session(
            Provider::ChatGpt,
            4,
            1,
            "https://chatgpt.com/c/jobs",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    let final_status = terminal(&reopened, &next, id).await;
    assert_eq!(final_status["state"], "completed", "{final_status}");
    let output = ok(call(&reopened, &next, "read_job_output", json!({"job_id":id})).await);
    assert_eq!(output["chunks"][0]["text"], "durable output\n");
    assert_eq!(
        ok(call(
            &reopened,
            &next,
            "read_job_output",
            json!({"job_id":id,"after":output["next_cursor"]})
        )
        .await)["chunks"],
        json!([])
    );
    assert!(app.store.verify_audit().unwrap().valid);
    let mut worker = tokio::process::Command::new(app.home.installed_binary())
        .arg("--home")
        .arg(&app.home.base)
        .arg("job-worker")
        .arg(id)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        !worker.wait().await.unwrap().success(),
        "completed job must not execute twice"
    );
}

#[tokio::test]
async fn jobs_enforce_owner_arguments_allowlist_and_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "print", "printf", &["ok"], 5);
    assert_eq!(
        call(
            &app,
            &sid,
            "run_command",
            json!({"command_id":"missing","background":true})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "run_command",
            json!({"command_id":"print","background":true,"args":["unexpected"]})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"print","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    terminal(&app, &sid, id).await;
    let other = app
        .register_session(
            Provider::ChatGpt,
            2,
            1,
            "https://chatgpt.com/c/other",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    for tool in ["job_status", "cancel_job", "read_job_output"] {
        assert_eq!(
            call(&app, &other, tool, json!({"job_id":id}))
                .await
                .code
                .as_deref(),
            Some("permission_denied")
        );
    }
    assert_eq!(
        ok(call(&app, &other, "list_jobs", json!({})).await)["jobs"],
        json!([])
    );
    assert_eq!(
        call(&app, &sid, "job_status", json!({"job_id":id,"owner":sid}))
            .await
            .code
            .as_deref(),
        Some("invalid_arguments")
    );
    let second =
        Workspace::initialize_profile(&temp.path().join("second"), Profile::General).unwrap();
    assert_eq!(
        call(
            &app.with_workspace(second),
            &sid,
            "job_status",
            json!({"job_id":id})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
}

#[tokio::test]
async fn running_job_can_be_cancelled_and_output_is_capped() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "repeat", "yes", &["bounded-output"], 5);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"repeat","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    running(&app, &sid, id).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let output = ok(call(&app, &sid, "read_job_output", json!({"job_id":id})).await);
    let kept: usize = output["chunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap().len())
        .sum();
    assert!(kept <= 128);
    ok(call(&app, &sid, "cancel_job", json!({"job_id":id})).await);
    assert_eq!(terminal(&app, &sid, id).await["state"], "cancelled");
    assert_eq!(
        ok(call(&app, &sid, "inspect_processes", json!({})).await)["processes"],
        json!([])
    );
}

#[tokio::test]
async fn command_grant_revocation_stops_an_active_worker() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "wait", "sleep", &["20"], 30);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"wait","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    running(&app, &sid, id).await;
    let _ = app.revoke_command("wait").unwrap();
    let result = terminal(&app, &sid, id).await;
    assert_eq!(result["state"], "cancelled", "{result}");
    assert!(result["message"].as_str().unwrap().contains("revoked"));
}

#[tokio::test]
async fn crashed_receipt_is_reconciled_without_rerun() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "print", "printf", &["once"], 5);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"print","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    terminal(&app, &sid, id).await;
    app.store
        .update_job(id, |j| {
            j.state = "running".into();
            j.heartbeat = 0;
            j.result = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        ok(call(&app, &sid, "job_status", json!({"job_id":id})).await)["state"],
        "interrupted"
    );
    assert_eq!(app.store.job_output(id, 0).unwrap().len(), 1);
}

#[tokio::test]
async fn system_tools_are_bounded_strict_and_report_real_workspace_policy() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let sys = ok(call(&app, &sid, "system_info", json!({})).await);
    assert_eq!(sys["architecture"], std::env::consts::ARCH);
    assert!(sys["workspace_free_bytes"].as_u64().unwrap() > 0);
    let info = ok(call(&app, &sid, "workspace_info", json!({})).await);
    assert_eq!(info["profile"], "general");
    assert_eq!(info["limits"]["calls_per_task"], 16);
    for tool in ["system_info", "workspace_info"] {
        assert_eq!(
            call(&app, &sid, tool, json!({"secret":true}))
                .await
                .code
                .as_deref(),
            Some("invalid_arguments")
        );
    }
}

#[tokio::test]
async fn timeout_exit_locking_and_panel_scope_have_terminal_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let app = grant(&app, "fail", "false", &[], 5);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"fail","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    let result = terminal(&app, &sid, id).await;
    assert_eq!(result["state"], "failed");
    assert_eq!(result["result"]["data"]["exit_code"], 1);
    let app = grant(&app, "timeout", "sleep", &["10"], 1);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"timeout","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    assert_eq!(
        terminal(&app, &sid, id).await["result"]["data"]["timed_out"],
        true
    );
    let app = grant(&app, "wait", "sleep", &["10"], 20);
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"wait","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    running(&app, &sid, id).await;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(app.home.base.join("locks/deployment.lock"))
        .unwrap();
    assert!(
        fs2::FileExt::try_lock_exclusive(&lock).is_err(),
        "deployment must not replace a live worker"
    );
    assert!(bridge_native::jobs::control(
        &app,
        "wrong-workspace",
        bridge_native::jobs::Control::Cancel { job_id: id.into() }
    )
    .is_err());
    bridge_native::jobs::control(
        &app,
        &app.workspace.id(),
        bridge_native::jobs::Control::Cancel { job_id: id.into() },
    )
    .unwrap();
    terminal(&app, &sid, id).await;
    let report = bridge_native::jobs::control(
        &app,
        &app.workspace.id(),
        bridge_native::jobs::Control::ExportReport { job_id: id.into() },
    )
    .unwrap();
    let saved: Value =
        serde_json::from_slice(&std::fs::read(report["report_path"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(saved["job"]["state"], "cancelled");
}

#[tokio::test]
#[ignore = "real 245-second cargo test; run explicitly for release acceptance"]
async fn real_cargo_job_runs_beyond_foreground_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    std::fs::write(
        app.workspace.root.join("Cargo.toml"),
        "[package]\nname=\"bridge-long-job-test\"\nversion=\"0.0.0\"\nedition=\"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(app.workspace.root.join("src")).unwrap();
    std::fs::write(app.workspace.root.join("src/lib.rs"),"#[test] fn long_build_check(){ std::thread::sleep(std::time::Duration::from_secs(245)); assert_eq!(2+2,4); }\n").unwrap();
    let app = app
        .grant_command(AllowedCommand {
            command_id: "long-cargo-test".into(),
            program: PathBuf::from(env!("CARGO")),
            fixed_args: vec!["test".into(), "--offline".into()],
            allowed_args: vec![],
            cwd: ".".into(),
            timeout_secs: 300,
            max_output_bytes: 20000,
            writes: true,
            always_allow: true,
        })
        .unwrap();
    let start = Instant::now();
    let job = ok(call(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"long-cargo-test","background":true}),
    )
    .await);
    let id = job["job_id"].as_str().unwrap();
    let reopened = App::load(app.home.clone(), true).unwrap();
    loop {
        let status = ok(call(&reopened, &sid, "job_status", json!({"job_id":id})).await);
        if ["completed", "failed", "cancelled", "interrupted"]
            .contains(&status["state"].as_str().unwrap())
        {
            assert_eq!(status["state"], "completed", "{status}");
            assert!(start.elapsed() > Duration::from_secs(240));
            eprintln!(
                "REAL LONG JOB PASS: elapsed={:?} job={id} exit={} output={}",
                start.elapsed(),
                status["result"]["data"]["exit_code"],
                status["result"]["output"]
            );
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(310), "{status}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn file_grant(_app: &App, sid: &str) -> bridge_native::grants::Request {
    bridge_native::grants::Request {
        session_id: sid.into(),
        directory: "files".into(),
        tools: vec![
            "write_document".into(),
            "append_document".into(),
            "create_folder".into(),
        ],
        seconds: 60,
        max_calls: 4,
        max_bytes: 100,
        max_files: 2,
    }
}
#[tokio::test]
async fn scoped_grants_allow_strict_writes_but_stop_changed_paths_tools_budgets_expiry_and_revocation(
) {
    use bridge_native::grants;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    // Strict mode with no connected approval UI: the grant is the only path to success.
    let mut info = app
        .store
        .workspaces()
        .unwrap()
        .into_iter()
        .find(|w| w.id == app.workspace.id())
        .unwrap();
    info.permission_mode = bridge_core::workspace::PermissionMode::Strict;
    app.store.save_workspace(&info).unwrap();
    let app = App::load(app.home.clone(), false).unwrap();
    let args = json!({"path":"files/allowed.md","content":"first","create_only":true});
    assert_eq!(
        call(&app, &sid, "write_document", args.clone())
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    grants::create(&app, &app.workspace.id(), file_grant(&app, &sid)).unwrap();
    ok(call(&app, &sid, "write_document", args).await);
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/allowed.md")).unwrap(),
        "first"
    );
    for args in [
        json!({"path":"notes/out.md","content":"no"}),
        json!({"path":"../escape.md","content":"no"}),
    ] {
        assert_eq!(
            call(&app, &sid, "write_document", args)
                .await
                .code
                .as_deref(),
            Some("permission_denied")
        );
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "copy_file",
            json!({"source":"files/allowed.md","destination":"files/copied.md"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            json!({"path":"files/allowed.md","content":"stale","expected_sha256":"wrong"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    ok(call(&app, &sid, "create_folder", json!({"path":"files/sub"})).await);
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            json!({"path":"files/third.md","content":"no"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let mut request = file_grant(&app, &sid);
    request.max_bytes = 3;
    grants::create(&app, &app.workspace.id(), request).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            json!({"path":"files/big.md","content":"1234"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let mut record = app
        .store
        .write_grant(&app.workspace.id(), &sid)
        .unwrap()
        .unwrap();
    record.expires_at = 0;
    app.store.save_write_grant(&record).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            json!({"path":"files/expired.md","content":"1"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    grants::create(&app, &app.workspace.id(), file_grant(&app, &sid)).unwrap();
    grants::revoke(&app, &app.workspace.id(), &sid).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            json!({"path":"files/revoked.md","content":"1"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn grants_are_panel_only_and_cannot_widen_research_or_follow_symlinks() {
    use bridge_native::grants;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    assert_eq!(
        call(&app, &sid, "create_write_grant", json!({}))
            .await
            .code
            .as_deref(),
        Some("unknown_tool")
    );
    let mut bad = file_grant(&app, &sid);
    bad.tools = vec!["run_command".into()];
    assert!(grants::create(&app, &app.workspace.id(), bad).is_err());
    assert!(grants::create(&app, "wrong", file_grant(&app, &sid)).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path(), app.workspace.root.join("files/escape")).unwrap();
        let mut request = file_grant(&app, &sid);
        request.directory = "files/escape".into();
        assert!(grants::create(&app, &app.workspace.id(), request).is_err());
    }
    let research =
        Workspace::initialize_profile(&temp.path().join("research"), Profile::Research).unwrap();
    let app = app.with_workspace(research);
    let new_sid = app
        .register_session(
            Provider::ChatGpt,
            5,
            1,
            "https://chatgpt.com/c/research",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    let mut request = file_grant(&app, &new_sid);
    request.directory = "AGENTS/CLAUDE".into();
    assert!(grants::create(&app, &app.workspace.id(), request).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_granted_writes_cannot_overdraw_call_budget() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut request = file_grant(&app, &sid);
    request.max_calls = 2;
    request.max_files = 20;
    bridge_native::grants::create(&app, &app.workspace.id(), request).unwrap();
    let mut handles = vec![];
    for i in 0..8 {
        let app = app.clone();
        let sid = sid.clone();
        handles.push(tokio::spawn(async move {
            call(
                &app,
                &sid,
                "write_document",
                json!({"path":format!("files/race-{i}.md"),"content":"x","create_only":true}),
            )
            .await
        }));
    }
    let mut allowed = 0;
    for handle in handles {
        let value = handle.await.unwrap();
        if value.status == "ok" {
            allowed += 1;
        } else {
            assert_eq!(
                value.code.as_deref(),
                Some("permission_denied"),
                "{value:?}"
            );
        }
    }
    assert_eq!(allowed, 2);
    assert_eq!(
        app.store
            .write_grant(&app.workspace.id(), &sid)
            .unwrap()
            .unwrap()
            .used_calls,
        2
    );
}
