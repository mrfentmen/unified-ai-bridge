use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    fsops::sha256_bytes,
    tools,
    wire::{BridgeEvent, ToolResultMessage},
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::sync::Arc;
fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(temp, Profile::General)
}
fn fixture_profile(temp: &tempfile::TempDir, profile: Profile) -> (Arc<App>, String) {
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
    let workspace = Workspace::initialize_profile(&root, profile).unwrap();
    let config = Config {
        project_root: workspace.root.clone(),
        ..Config::default()
    };
    home.save_config(&config).unwrap();
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let sid = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/patches",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}

#[tokio::test]
async fn locked_and_expired_previews_never_apply() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let p = ok(call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/locked.txt","content":"x"}]}),
    )
    .await);
    let result = decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || {
            let mut workspace = app
                .store
                .workspaces()
                .unwrap()
                .into_iter()
                .find(|w| w.id == app.workspace.id())
                .unwrap();
            workspace.read_only = true;
            app.store.save_workspace(&workspace).unwrap();
        },
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert!(!app.workspace.root.join("files/locked.txt").exists());
    let mut workspace = app
        .store
        .workspaces()
        .unwrap()
        .into_iter()
        .find(|w| w.id == app.workspace.id())
        .unwrap();
    workspace.read_only = false;
    app.store.save_workspace(&workspace).unwrap();
    let p = ok(call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/expired.txt","content":"x"}]}),
    )
    .await);
    let id = p["patch_id"].as_str().unwrap();
    let mut record = app.store.patch(id, &app.workspace.id()).unwrap().unwrap();
    record.expires_at = chrono::Utc::now().timestamp() - 1;
    app.store.save_patch(&record).unwrap();
    assert_eq!(
        call(&app, &sid, "apply_patch", json!({"patch_id":id}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(app.approvals.pending_count(), 0);
    assert!(!app.workspace.root.join("files/expired.txt").exists());
}

#[tokio::test]
async fn research_policy_is_preserved_and_restores_refuse_later_user_edits() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&temp, Profile::Research);
    for path in [
        "SHARED/datasets/raw.csv",
        "AGENTS/CLAUDE/notes/other.md",
        "AGENTS/CHATGPT/notes/program.rs",
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "preview_patch",
                json!({"operations":[{"path":path,"content":"x"}]})
            )
            .await
            .status,
            "error"
        );
        assert!(!app.workspace.root.join(path).exists());
    }
    let path = "AGENTS/CHATGPT/notes/own.md";
    let p = ok(call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":path,"content":"allowed"}]}),
    )
    .await);
    ok(decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || {},
    )
    .await);
    std::fs::write(app.workspace.root.join(path), "later user edit").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "restore_patch",
            json!({"patch_id":p["patch_id"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join(path)).unwrap(),
        "later user edit"
    );
    assert_eq!(app.approvals.pending_count(), 0);
}

#[tokio::test]
async fn invalid_batches_and_omitted_preview_differences_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    for args in [
        json!({"operations":[]}),
        json!({"operations":[{"path":"files/a","content":"a","agent":"fake"}]}),
        json!({"operations":[{"path":"files/a","content":"a"},{"path":"files/./a","content":"b"}]}),
        json!({"operations":[{"path":"files/a","content":"a".repeat(512_001)}]}),
        json!({"operations":[{"path":"files/a","content":"a\n".repeat(30_000)}]}),
        json!({"operations":(0..21).map(|i|json!({"path":format!("files/{i}"),"content":"x"})).collect::<Vec<_>>()}),
    ] {
        assert_eq!(
            call(&app, &sid, "preview_patch", args)
                .await
                .code
                .as_deref(),
            Some("invalid_arguments")
        );
    }
    std::fs::write(app.workspace.root.join("files/binary"), [0xff, 0xfe]).unwrap();
    assert_eq!(call(&app,&sid,"preview_patch",json!({"operations":[{"path":"files/binary","content":"x","expected_sha256":sha256_bytes(&[0xff,0xfe])}]})).await.code.as_deref(),Some("unsupported"));
    assert!(!app.workspace.root.join("files/a").exists());
    assert!(app
        .store
        .file_changes(&app.workspace.id(), 100)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn interrupted_committed_steps_are_reconciled_without_repeating_writes() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let p = ok(call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/once.txt","content":"once"}]}),
    )
    .await);
    ok(decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || {},
    )
    .await);
    let id = p["patch_id"].as_str().unwrap();
    let mut record = app.store.patch(id, &app.workspace.id()).unwrap().unwrap();
    record.state = "applying".into();
    record.steps[0].state = "pending".into();
    app.store.save_patch(&record).unwrap();
    let result = ok(call(&app, &sid, "patch_status", json!({"patch_id":id})).await);
    assert_eq!(result["state"], "interrupted");
    assert_eq!(result["steps"][0]["state"], "committed");
    assert_eq!(
        call(&app, &sid, "apply_patch", json!({"patch_id":id}))
            .await
            .status,
        "error"
    );
    assert_eq!(
        app.store
            .file_changes(&app.workspace.id(), 100)
            .unwrap()
            .len(),
        1
    );
}
async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tools::dispatch(app, Agent::ChatGpt, sid, tool, args).await
}
fn ok(result: ToolResultMessage) -> Value {
    assert_eq!(result.status, "ok", "{result:?}");
    result.data.unwrap()
}
async fn decide(
    app: &Arc<App>,
    sid: &str,
    tool: &str,
    args: Value,
    allow: bool,
    change: impl FnOnce(),
) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let a = app.clone();
    let sid = sid.to_string();
    let tool = tool.to_string();
    let pending = tokio::spawn(async move { call(&a, &sid, &tool, args).await });
    loop {
        if let BridgeEvent::ApprovalRequired { approval } =
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            change();
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn multiple_real_files_apply_restore_and_replays_keep_one_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/existing.rs"), "fn before() {}\n").unwrap();
    let preview=ok(call(&app,&sid,"preview_patch",json!({"operations":[{"path":"files/existing.rs","content":"fn after() {}\n","expected_sha256":sha256_bytes(b"fn before() {}\n")},{"path":"files/new.rs","content":"fn caller() { after(); }\n"}]})).await);
    let id = preview["patch_id"].clone();
    assert!(!root.join("files/new.rs").exists());
    let applied = ok(decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":id}),
        true,
        || {},
    )
    .await);
    assert_eq!(applied["state"], "completed");
    assert_eq!(
        std::fs::read_to_string(root.join("files/existing.rs")).unwrap(),
        "fn after() {}\n"
    );
    assert_eq!(applied["steps"][1]["state"], "committed");
    let repeat = ok(call(&app, &sid, "apply_patch", json!({"patch_id":id})).await);
    assert_eq!(repeat["steps"], applied["steps"]);
    let restored = ok(decide(
        &app,
        &sid,
        "restore_patch",
        json!({"patch_id":id}),
        true,
        || {},
    )
    .await);
    assert_eq!(restored["state"], "completed");
    assert!(!root.join("files/new.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("files/existing.rs")).unwrap(),
        "fn before() {}\n"
    );
    let again = ok(call(&app, &sid, "restore_patch", json!({"patch_id":id})).await);
    assert_eq!(again["patch_id"], restored["patch_id"]);
    assert_eq!(
        app.store
            .file_changes(&app.workspace.id(), 100)
            .unwrap()
            .len(),
        4
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn approval_denial_stale_files_identity_and_unsafe_paths_never_write() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    let args = json!({"operations":[{"path":"files/denied.txt","content":"denied"}]});
    let p = ok(call(&app, &sid, "preview_patch", args).await);
    assert_eq!(
        decide(
            &app,
            &sid,
            "apply_patch",
            json!({"patch_id":p["patch_id"]}),
            false,
            || {}
        )
        .await
        .code
        .as_deref(),
        Some("approval_denied")
    );
    assert!(!root.join("files/denied.txt").exists());
    std::fs::write(root.join("files/current.txt"), "old").unwrap();
    let p=ok(call(&app,&sid,"preview_patch",json!({"operations":[{"path":"files/current.txt","content":"new","expected_sha256":sha256_bytes(b"old")},{"path":"files/not-written.txt","content":"x"}]})).await);
    let result = decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || std::fs::write(root.join("files/current.txt"), "user edit").unwrap(),
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("file_changed"));
    assert!(!root.join("files/not-written.txt").exists());
    for ops in [
        json!([{"path":"../escape.txt","content":"x"}]),
        json!([{"path":".bridge/private","content":"x"}]),
        json!([{"path":"files/a","content":"x"},{"path":"files/a/child","content":"x"}]),
    ] {
        assert_eq!(
            call(&app, &sid, "preview_patch", json!({"operations":ops}))
                .await
                .status,
            "error"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path(), root.join("files/escape")).unwrap();
        assert_eq!(
            call(
                &app,
                &sid,
                "preview_patch",
                json!({"operations":[{"path":"files/escape/out","content":"x"}]})
            )
            .await
            .code
            .as_deref(),
            Some("permission_denied")
        );
    }
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
    for tool in ["apply_patch", "restore_patch", "patch_status"] {
        assert_eq!(
            call(&app, &other, tool, json!({"patch_id":p["patch_id"]}))
                .await
                .code
                .as_deref(),
            Some("permission_denied")
        );
        assert_eq!(
            call(
                &app,
                &sid,
                tool,
                json!({"patch_id":p["patch_id"],"identity":"fake"})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
}
#[tokio::test]
async fn mid_batch_path_lock_failure_leaves_accurate_partial_receipt_and_recoverable_first_file() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = app.workspace.root.clone();
    std::fs::write(root.join("files/second.txt"), "old").unwrap();
    let p=ok(call(&app,&sid,"preview_patch",json!({"operations":[{"path":"files/first.txt","content":"first"},{"path":"files/second.txt","content":"second","expected_sha256":sha256_bytes(b"old")}]})).await);
    let held = bridge_native::fsops::PathLock::acquire(&app.workspace.lock_dir, "files/second.txt")
        .unwrap();
    let applied = decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || {},
    )
    .await;
    assert_eq!(
        applied.code.as_deref(),
        Some("partial_patch"),
        "{applied:?}"
    );
    let data = applied.data.unwrap();
    assert_eq!(data["steps"][0]["state"], "committed");
    assert_eq!(data["steps"][1]["state"], "failed");
    drop(held);
    let restored = ok(decide(
        &app,
        &sid,
        "restore_patch",
        json!({"patch_id":p["patch_id"]}),
        true,
        || {},
    )
    .await);
    assert_eq!(restored["state"], "completed");
    assert!(!root.join("files/first.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("files/second.txt")).unwrap(),
        "old"
    );
}
#[tokio::test]
async fn interrupted_claim_is_reported_and_never_reapplied() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let p = ok(call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/never.txt","content":"x"}]}),
    )
    .await);
    let id = p["patch_id"].as_str().unwrap();
    let mut record = app.store.patch(id, &app.workspace.id()).unwrap().unwrap();
    record.state = "applying".into();
    app.store.save_patch(&record).unwrap();
    let status = ok(call(&app, &sid, "patch_status", json!({"patch_id":id})).await);
    assert_eq!(status["state"], "interrupted");
    assert_eq!(
        call(&app, &sid, "apply_patch", json!({"patch_id":id}))
            .await
            .code
            .as_deref(),
        Some("file_changed")
    );
    assert!(!app.workspace.root.join("files/never.txt").exists());
}
