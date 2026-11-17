//! Transport fixtures prove authorization, not website integration.
use base64::Engine;
use bridge_core::{
    browser_files::{Action, MAX_UPLOAD_BYTES},
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    tools,
    wire::{BridgeEvent, ToolResultMessage},
    workspace::Workspace,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String, String) {
    let base = temp.path().canonicalize().unwrap().join("home");
    let root = temp.path().canonicalize().unwrap().join("workspace");
    let home = HomePaths {
        base: base.clone(),
        config_file: base.join("config.toml"),
        database: base.join("db.sqlite3"),
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
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let sid = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/upload",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    let data = b"exact local file for upload\n";
    std::fs::write(root.join("files/test.txt"), data).unwrap();
    (app, sid, bridge_native::fsops::sha256_bytes(data))
}
async fn attempt(
    app: &Arc<App>,
    sid: &str,
    hash: &str,
    allow: bool,
    change: &str,
) -> (ToolResultMessage, usize) {
    let mut events = app.events.subscribe();
    let call = tools::dispatch(
        app,
        Agent::ChatGpt,
        sid,
        "attach_file_to_chat",
        json!({"path":"files/test.txt","expected_sha256":hash}),
    );
    let mut uploads = 0;
    let controller = async {
        loop {
            match events.recv().await.unwrap() {
                BridgeEvent::BrowserOperation {
                    id, scope, action, ..
                } => {
                    assert_eq!(scope.session_id, sid);
                    assert_eq!(scope.conversation, "upload");
                    let reply = match action {
                        Action::InspectUpload => Ok(json!({"turn_id":"visible-user-one"})),
                        Action::Upload {
                            file,
                            attempt_id,
                            turn_id,
                        } => {
                            uploads += 1;
                            assert!(allow);
                            assert_eq!(turn_id, "visible-user-one");
                            assert_eq!(file.sha256, hash);
                            assert_eq!(file.name, "test.txt");
                            assert_eq!(
                                base64::engine::general_purpose::STANDARD
                                    .decode(file.base64)
                                    .unwrap(),
                                b"exact local file for upload\n"
                            );
                            if change == "uncertain" {
                                Err("upload interrupted; inspect composer".into())
                            } else {
                                Ok(json!({"state":"attached","upload_id":attempt_id,"sha256":hash}))
                            }
                        }
                        _ => panic!("unexpected action"),
                    };
                    assert!(app.browser_files.resolve(&id, &scope, reply));
                }
                BridgeEvent::ApprovalRequired { approval } => {
                    assert_eq!(approval.tool, "attach_file_to_chat");
                    assert!(approval.reason.contains(hash));
                    assert!(approval.reason.contains("conversation upload"));
                    assert!(approval.path.is_none());
                    match change {
                        "file" => {
                            std::fs::write(app.workspace.root.join("files/test.txt"), "different")
                                .unwrap()
                        }
                        "session" => app.end_session(sid).unwrap(),
                        "lock" => {
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
                        _ => {}
                    }
                    assert!(app.approvals.resolve(&approval.id, allow));
                }
                _ => {}
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {r=call=>r,_=controller=>unreachable!()}
    })
    .await
    .unwrap();
    (result, uploads)
}

#[tokio::test]
async fn exact_approved_bytes_and_destination_are_required_and_attempts_survive_restart() {
    for state in ["allow", "deny", "file", "session", "lock", "uncertain"] {
        let temp = tempfile::tempdir().unwrap();
        let (app, sid, hash) = fixture(&temp);
        let (result, count) = attempt(&app, &sid, &hash, state != "deny", state).await;
        assert_eq!(
            count,
            usize::from(matches!(state, "allow" | "uncertain")),
            "{state}: {result:?}"
        );
        assert_eq!(
            result.status,
            if state == "allow" { "ok" } else { "error" },
            "{state}: {result:?}"
        );
        if matches!(state, "allow" | "uncertain") {
            let reopened = App::open_with_config(
                app.home.clone(),
                app.config.clone(),
                app.workspace.clone(),
                true,
            )
            .unwrap();
            let (repeat, count) = attempt(&reopened, &sid, &hash, true, "").await;
            assert_eq!(count, 0);
            assert!(repeat
                .error
                .unwrap()
                .contains("already has an upload attempt"));
        }
        assert!(app.store.verify_audit().unwrap().valid);
    }
}

#[tokio::test]
async fn unsafe_sources_types_sizes_identity_and_providers_never_request_approval_or_upload() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid, hash) = fixture(&temp);
    let root = &app.workspace.root;
    std::fs::write(
        root.join("files/large.txt"),
        vec![b'x'; MAX_UPLOAD_BYTES + 1],
    )
    .unwrap();
    std::fs::write(root.join("files/binary.txt"), [0, 255]).unwrap();
    std::fs::write(root.join("files/fake.png"), b"fake").unwrap();
    std::fs::write(root.join("files/other.exe"), b"no").unwrap();
    std::os::unix::fs::symlink(root.join("files/test.txt"), root.join("files/link.txt")).unwrap();
    let outside = temp.path().join("outside.txt");
    std::fs::write(&outside, b"private").unwrap();
    std::fs::hard_link(&outside, root.join("files/hard.txt")).unwrap();
    for path in [
        "../outside.txt",
        "files/large.txt",
        "files/binary.txt",
        "files/fake.png",
        "files/other.exe",
        "files/link.txt",
        "files/hard.txt",
    ] {
        let h = std::fs::read(root.join(path))
            .map(|b| bridge_native::fsops::sha256_bytes(&b))
            .unwrap_or_default();
        let result = tools::dispatch(
            &app,
            Agent::ChatGpt,
            &sid,
            "attach_file_to_chat",
            json!({"path":path,"expected_sha256":h}),
        )
        .await;
        assert_eq!(result.status, "error", "{path}: {result:?}");
        assert_eq!(app.approvals.pending_count(), 0);
    }
    for args in [
        json!({"path":"files/test.txt","expected_sha256":"0".repeat(64)}),
        json!({"path":"files/test.txt","expected_sha256":hash,"provider":"claude"}),
    ] {
        assert_eq!(
            tools::dispatch(&app, Agent::ChatGpt, &sid, "attach_file_to_chat", args)
                .await
                .status,
            "error"
        );
    }
    let other = app
        .register_session(
            Provider::Claude,
            2,
            1,
            "https://claude.ai/chat/upload",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::Claude,
            &other,
            "attach_file_to_chat",
            json!({"path":"files/test.txt","expected_sha256":hash})
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
}
