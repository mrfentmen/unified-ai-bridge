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
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
}
fn seed(app: &App) -> (Vec<u8>, Vec<u8>) {
    let before: Vec<_> = (0..2 * 1024 * 1024 + 17).map(|i| (i % 251) as u8).collect();
    let after: Vec<_> = (0..3 * 1024 * 1024 + 29).map(|i| (i % 239) as u8).collect();
    std::fs::write(app.workspace.root.join("files/before.bin"), &before).unwrap();
    std::fs::write(app.workspace.root.join("files/after.bin"), &after).unwrap();
    (before, after)
}
fn args(before: &[u8], after: &[u8]) -> Value {
    json!({"source":"files/after.bin","destination":"files/before.bin","source_sha256":sha256_bytes(after),"expected_sha256":sha256_bytes(before)})
}
#[tokio::test]
async fn larger_replace_restore_trash_restore_preserves_exact_bytes_and_receipts() {
    let _s = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (before, after) = seed(&app);
    let result = ok(decide(
        &app,
        &sid,
        "replace_file",
        args(&before, &after),
        true,
        || {},
    )
    .await);
    let path = app.workspace.root.join("files/before.bin");
    assert_eq!(std::fs::read(&path).unwrap(), after);
    assert_eq!(
        std::fs::read(app.workspace.root.join(result["trash"].as_str().unwrap())).unwrap(),
        before
    );
    assert!(result.get("before_base64").is_none());
    assert_eq!(result["binary"], true);
    ok(decide(
        &app,
        &sid,
        "restore_change",
        json!({"change_id":result["change_id"]}),
        true,
        || {},
    )
    .await);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let result = ok(decide(
        &app,
        &sid,
        "delete_file",
        json!({"path":"files/before.bin","expected_sha256":sha256_bytes(&before)}),
        true,
        || {},
    )
    .await);
    assert!(!path.exists());
    ok(decide(
        &app,
        &sid,
        "restore_change",
        json!({"change_id":result["change_id"]}),
        true,
        || {},
    )
    .await);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/after.bin")).unwrap(),
        after
    );
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
async fn denied_changed_locked_and_symlink_inputs_leave_user_bytes_untouched() {
    let _s = TEST_LOCK.lock().await;
    for mode in [
        "deny",
        "source",
        "destination",
        "locked",
        "symlink",
        "parent",
    ] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let (before, after) = seed(&app);
        let result = decide(
            &app,
            &sid,
            "replace_file",
            args(&before, &after),
            mode != "deny",
            || match mode {
                "source" => {
                    std::fs::write(app.workspace.root.join("files/after.bin"), b"new source")
                        .unwrap()
                }
                "destination" => {
                    std::fs::write(app.workspace.root.join("files/before.bin"), b"later edit")
                        .unwrap()
                }
                "locked" => {
                    let mut w = app
                        .store
                        .workspaces()
                        .unwrap()
                        .into_iter()
                        .find(|w| w.id == app.workspace.id())
                        .unwrap();
                    w.read_only = true;
                    app.store.save_workspace(&w).unwrap();
                }
                "symlink" => {
                    std::fs::remove_file(app.workspace.root.join("files/after.bin")).unwrap();
                    std::os::unix::fs::symlink(
                        t.path().join("outside"),
                        app.workspace.root.join("files/after.bin"),
                    )
                    .unwrap();
                }
                "parent" => {
                    std::fs::rename(
                        app.workspace.root.join("files"),
                        app.workspace.root.join("moved"),
                    )
                    .unwrap();
                    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("files")).unwrap();
                }
                _ => {}
            },
        )
        .await;
        assert_eq!(result.status, "error", "{mode}: {result:?}");
        let target = app.workspace.root.join(if mode == "parent" {
            "moved/before.bin"
        } else {
            "files/before.bin"
        });
        assert_eq!(
            std::fs::read(target).unwrap(),
            if mode == "destination" {
                b"later edit".to_vec()
            } else {
                before
            }
        );
        assert!(app
            .store
            .file_changes(&app.workspace.id(), 100)
            .unwrap()
            .is_empty());
    }
}
#[tokio::test]
async fn restore_refuses_later_edits_and_corrupt_private_backup() {
    let _s = TEST_LOCK.lock().await;
    for mode in ["later", "backup"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let (before, after) = seed(&app);
        let result = ok(decide(
            &app,
            &sid,
            "replace_file",
            args(&before, &after),
            true,
            || {},
        )
        .await);
        let id = result["change_id"].as_str().unwrap();
        let target = app.workspace.root.join("files/before.bin");
        if mode == "later" {
            std::fs::write(&target, b"later user edits").unwrap();
        } else {
            std::fs::write(
                app.home
                    .base
                    .join("recovery")
                    .join(app.workspace.id())
                    .join(id),
                b"corrupt",
            )
            .unwrap();
        }
        let result = call(&app, &sid, "restore_change", json!({"change_id":id})).await;
        assert_eq!(result.status, "error");
        assert_eq!(app.approvals.pending_count(), 0);
        assert_eq!(
            std::fs::read(target).unwrap(),
            if mode == "later" {
                b"later user edits".to_vec()
            } else {
                after
            }
        );
    }
}
#[tokio::test]
async fn quotas_retention_and_explicit_cleanup_keep_audit_and_current_files() {
    use tools::large_recovery::{control, Control, Settings};
    let _s = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (before, after) = seed(&app);
    let settings = Settings {
        max_snapshots: 1,
        ..Default::default()
    };
    control(&app, &app.workspace.id(), Control::Settings { settings }).unwrap();
    let result = ok(decide(
        &app,
        &sid,
        "replace_file",
        args(&before, &after),
        true,
        || {},
    )
    .await);
    let id = result["change_id"].as_str().unwrap().to_string();
    let fail = decide(
        &app,
        &sid,
        "restore_change",
        json!({"change_id":id}),
        true,
        || {},
    )
    .await;
    assert_eq!(fail.status, "error");
    assert!(fail.error.as_deref().unwrap().contains("budget"));
    assert!(control(
        &app,
        &app.workspace.id(),
        Control::Cleanup {
            change_ids: vec![id.clone()]
        }
    )
    .is_err());
    let mut receipt = app
        .store
        .file_change(&id, &app.workspace.id())
        .unwrap()
        .unwrap();
    receipt.created_at = "2020-01-01T00:00:00Z".into();
    app.store.save_file_change(&receipt).unwrap();
    let status = control(&app, &app.workspace.id(), Control::Status {}).unwrap();
    assert_eq!(status["expired"].as_array().unwrap().len(), 1);
    let status = control(
        &app,
        &app.workspace.id(),
        Control::Cleanup {
            change_ids: vec![id.clone()],
        },
    )
    .unwrap();
    assert_eq!(status["snapshots"], 0);
    assert_eq!(status["reserved_bytes"], 0);
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/before.bin")).unwrap(),
        after
    );
    assert_eq!(
        app.store
            .file_change(&id, &app.workspace.id())
            .unwrap()
            .unwrap()
            .state,
        "backup_removed"
    );
    assert_eq!(
        call(&app, &sid, "restore_change", json!({"change_id":id}))
            .await
            .status,
        "error"
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn changed_retained_original_and_uncertain_receipts_cannot_be_cleaned_or_restored() {
    use tools::large_recovery::{control, Control};
    let _s = TEST_LOCK.lock().await;
    for mode in ["retained", "uncertain"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let (before, after) = seed(&app);
        let result = ok(decide(
            &app,
            &sid,
            "replace_file",
            args(&before, &after),
            true,
            || {},
        )
        .await);
        let id = result["change_id"].as_str().unwrap().to_string();
        let mut c = app
            .store
            .file_change(&id, &app.workspace.id())
            .unwrap()
            .unwrap();
        c.created_at = "2020-01-01T00:00:00Z".into();
        if mode == "retained" {
            std::fs::write(
                app.workspace.root.join(c.trash.as_ref().unwrap()),
                b"later descriptor write",
            )
            .unwrap();
        } else {
            c.state = "original_detached".into();
        }
        app.store.save_file_change(&c).unwrap();
        assert!(control(
            &app,
            &app.workspace.id(),
            Control::Cleanup {
                change_ids: vec![id.clone()]
            }
        )
        .is_err());
        assert!(app
            .home
            .base
            .join("recovery")
            .join(app.workspace.id())
            .join(&id)
            .exists());
        if mode == "uncertain" {
            assert_eq!(
                call(&app, &sid, "restore_change", json!({"change_id":id}))
                    .await
                    .status,
                "error"
            );
        }
    }
}
#[tokio::test]
async fn traversal_hard_links_research_and_unknown_fields_are_denied() {
    let _s = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (before, after) = seed(&app);
    for path in ["../outside", ".bridge/private", ".git/config", ".env"] {
        let mut a = args(&before, &after);
        a["destination"] = json!(path);
        assert_eq!(call(&app, &sid, "replace_file", a).await.status, "error");
    }
    let mut a = args(&before, &after);
    a["provider"] = json!("claude");
    assert_eq!(
        call(&app, &sid, "replace_file", a).await.code.as_deref(),
        Some("invalid_arguments")
    );
    std::fs::hard_link(
        app.workspace.root.join("files/after.bin"),
        app.workspace.root.join("files/link.bin"),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "replace_file", args(&before, &after))
            .await
            .status,
        "error"
    );
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    std::fs::write(
        app.workspace.root.join("AGENTS/CHATGPT/notes/input.bin"),
        &after,
    )
    .unwrap();
    for p in [
        "SHARED/datasets/protected.bin",
        "AGENTS/CLAUDE/notes/private.bin",
    ] {
        let a = json!({"source":"AGENTS/CHATGPT/notes/input.bin","destination":p,"source_sha256":sha256_bytes(&after),"expected_sha256":sha256_bytes(&before)});
        assert_eq!(
            call(&app, &sid, "replace_file", a).await.code.as_deref(),
            Some("permission_denied")
        );
    }
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
        approval_timeout_secs: 10,
        ..Config::default()
    };
    home.save_config(&config).unwrap();
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let sid = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/archives",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}

async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tools::dispatch(app, Agent::ChatGpt, sid, tool, args),
    )
    .await
    .expect("binary tool should finish or request approval promptly")
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
            tokio::time::timeout(std::time::Duration::from_secs(15), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            change();
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(15), pending)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn cleanup_or_corruption_during_restore_approval_and_oversized_files_are_refused() {
    use tools::large_recovery::{control, Control};
    let _s = TEST_LOCK.lock().await;
    for mode in ["cleanup", "corrupt"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let (before, after) = seed(&app);
        let result = ok(decide(
            &app,
            &sid,
            "replace_file",
            args(&before, &after),
            true,
            || {},
        )
        .await);
        let id = result["change_id"].as_str().unwrap().to_string();
        let mut c = app
            .store
            .file_change(&id, &app.workspace.id())
            .unwrap()
            .unwrap();
        c.created_at = "2020-01-01T00:00:00Z".into();
        app.store.save_file_change(&c).unwrap();
        let r = decide(
            &app,
            &sid,
            "restore_change",
            json!({"change_id":id}),
            true,
            || {
                if mode == "cleanup" {
                    control(
                        &app,
                        &app.workspace.id(),
                        Control::Cleanup {
                            change_ids: vec![id.clone()],
                        },
                    )
                    .unwrap();
                } else {
                    std::fs::write(
                        app.home
                            .base
                            .join("recovery")
                            .join(app.workspace.id())
                            .join(&id),
                        b"corruption during approval",
                    )
                    .unwrap();
                }
            },
        )
        .await;
        assert_eq!(r.status, "error");
        assert_eq!(
            std::fs::read(app.workspace.root.join("files/before.bin")).unwrap(),
            after
        );
    }
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (before, after) = seed(&app);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(app.workspace.root.join("files/after.bin"))
        .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        call(&app, &sid, "replace_file", args(&before, &after))
            .await
            .status,
        "error"
    );
    assert_eq!(app.approvals.pending_count(), 0);
}
