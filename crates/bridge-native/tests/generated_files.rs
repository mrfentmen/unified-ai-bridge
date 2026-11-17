use bridge_core::{
    browser_files::{Action, DownloadMethod, GeneratedFile},
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
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String) {
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
            "https://chatgpt.com/c/files",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}
fn file(method: DownloadMethod) -> GeneratedFile {
    GeneratedFile {
        handle: "a".repeat(32),
        name: "asset.svg".into(),
        response_id: "assistant-one".into(),
        source_key: "b".repeat(64),
        method,
    }
}
async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tokio::time::timeout(
        Duration::from_secs(4),
        tools::dispatch(app, Agent::ChatGpt, sid, tool, args),
    )
    .await
    .unwrap()
}

/// The event responder is a transport fixture, not a provider integration.
async fn download(
    app: &Arc<App>,
    sid: &str,
    metadata: GeneratedFile,
    allow: bool,
    reply: Result<Value, String>,
    on_approval: impl FnOnce(),
) -> (ToolResultMessage, usize) {
    let mut events = app.events.subscribe();
    let request = call(
        app,
        sid,
        "request_download",
        json!({"handle":metadata.handle}),
    );
    let controller = async {
        let mut downloads = 0;
        let mut change = Some(on_approval);
        loop {
            match events.recv().await.unwrap() {
                BridgeEvent::BrowserOperation {
                    id, scope, action, ..
                } => {
                    assert_eq!(scope.session_id, sid);
                    assert_eq!(scope.workspace_id, app.workspace.id());
                    assert_eq!(scope.conversation, "files");
                    let response = match action {
                        Action::Inspect { .. } => Ok(json!(metadata)),
                        Action::Download { file } => {
                            assert_eq!(file, metadata);
                            downloads += 1;
                            reply.clone()
                        }
                        _ => panic!("unexpected list"),
                    };
                    assert!(app.browser_files.resolve(&id, &scope, response));
                }
                BridgeEvent::ApprovalRequired { approval } => {
                    assert_eq!(approval.tool, "request_download");
                    assert!(approval.reason.contains("asset.svg"));
                    if let Some(f) = change.take() {
                        f();
                    }
                    assert!(app.approvals.resolve(&approval.id, allow));
                }
                BridgeEvent::Activity { activity } if activity.tool == "request_download" => break,
                _ => {}
            }
        }
        downloads
    };
    tokio::join!(request, controller)
}

#[tokio::test]
async fn discovery_is_scoped_bounded_audited_and_has_deny_paths() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut events = app.events.subscribe();
    let request = call(&app, &sid, "list_generated_files", json!({}));
    let respond = async {
        if let BridgeEvent::BrowserOperation {
            id, scope, action, ..
        } = events.recv().await.unwrap()
        {
            assert!(matches!(action, Action::List));
            assert_eq!(scope.task_id, None);
            assert!(app.browser_files.resolve(
                &id,
                &scope,
                Ok(json!({"files":[file(DownloadMethod::WebsiteButton)]}))
            ));
        } else {
            panic!("missing browser request");
        }
    };
    let (result, _) = tokio::join!(request, respond);
    assert_eq!(result.status, "ok", "{result:?}");
    assert_eq!(result.data.unwrap()["files"][0]["name"], "asset.svg");
    for args in [
        json!({"url":"https://example.test"}),
        json!({"provider":"claude"}),
    ] {
        assert_eq!(
            call(&app, &sid, "list_generated_files", args)
                .await
                .code
                .as_deref(),
            Some("invalid_arguments")
        );
    }
    assert_eq!(
        call(&app, "wrong", "list_generated_files", json!({}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let headless = App::load(app.home.clone(), false).unwrap();
    assert_eq!(
        call(&headless, &sid, "list_generated_files", json!({}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn button_approval_deny_manual_result_and_restart_dedup() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let metadata = file(DownloadMethod::WebsiteButton);
    let manual = Ok(json!({"state":"manual_share_required"}));
    let (denied, count) =
        download(&app, &sid, metadata.clone(), false, manual.clone(), || {}).await;
    assert_eq!(denied.code.as_deref(), Some("approval_denied"));
    assert_eq!(count, 0);
    let (result, count) = download(&app, &sid, metadata.clone(), true, manual.clone(), || {}).await;
    assert_eq!(result.status, "ok", "{result:?}");
    assert_eq!(count, 1);
    assert_eq!(result.data.unwrap()["state"], "manual_share_required");
    assert!(app
        .store
        .artifacts(&app.workspace.id(), &sid)
        .unwrap()
        .is_empty());
    let loaded = App::load(app.home.clone(), true).unwrap();
    let sid2 = loaded
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/files",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    let (result, count) = download(
        &loaded,
        &sid2,
        GeneratedFile {
            handle: "c".repeat(32),
            ..metadata
        },
        true,
        manual,
        || {},
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("file_changed"));
    assert_eq!(count, 0);
    let audit = std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    for entry in [
        "approval.request:request_download",
        "browser.download_prepare",
        "browser.download_result",
    ] {
        assert!(audit.contains(entry));
    }
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn exact_link_download_shares_real_bytes_but_does_not_import() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let path = temp.path().canonicalize().unwrap().join("download.svg");
    std::fs::write(&path, b"<svg/>").unwrap();
    let selected = json!({"download_id":23,"filename":path,"bytes":6,"mime":"image/svg+xml","origin":"chatgpt.com"});
    let (result, count) = download(
        &app,
        &sid,
        file(DownloadMethod::DirectLink),
        true,
        Ok(json!({"state":"shared","download":selected})),
        || {},
    )
    .await;
    assert_eq!(count, 1);
    assert_eq!(result.status, "ok", "{result:?}");
    let data = result.data.unwrap();
    assert_eq!(data["state"], "shared");
    assert_eq!(
        data["artifact"]["sha256"],
        bridge_native::fsops::sha256_bytes(b"<svg/>")
    );
    assert!(!data.to_string().contains(path.to_str().unwrap()));
    assert_eq!(
        std::fs::read_dir(app.workspace.root.join("files"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        app.store
            .artifacts(&app.workspace.id(), &sid)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn readonly_workspace_change_and_stale_browser_reply_prevent_downloads() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut root = app.workspace.clone();
    root.read_only = true;
    let locked = app.with_workspace(root);
    assert_eq!(
        call(
            &locked,
            &sid,
            "request_download",
            json!({"handle":"a".repeat(32)})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let (result, count) = download(
        &app,
        &sid,
        file(DownloadMethod::WebsiteButton),
        true,
        Err("DOM handle changed".into()),
        || {},
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("file_changed"));
    assert_eq!(count, 1);
    // Failed attempts remain claimed, rather than blindly replaying a possibly
    // completed browser mutation under a new tool request key.
    let (result, count) = download(
        &app,
        &sid,
        file(DownloadMethod::WebsiteButton),
        true,
        Ok(json!({"state":"manual_share_required"})),
        || {},
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("file_changed"));
    assert_eq!(count, 0);
    let next = Workspace::initialize_profile(
        &temp.path().canonicalize().unwrap().join("other"),
        Profile::General,
    )
    .unwrap();
    app.store
        .save_workspace(&next.info("other".into()))
        .unwrap();
    let mut other = file(DownloadMethod::WebsiteButton);
    other.source_key = "d".repeat(64);
    let (result, count) = download(
        &app,
        &sid,
        other,
        true,
        Ok(json!({"state":"manual_share_required"})),
        || {
            app.store.set_active_workspace(&next.id()).unwrap();
        },
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert_eq!(count, 0);
}

#[tokio::test]
async fn button_reply_cannot_register_a_source_file() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let(result,_)=download(&app,&sid,file(DownloadMethod::WebsiteButton),true,Ok(json!({"state":"shared","download":{"download_id":1,"filename":"/tmp/unrelated","bytes":0,"mime":"text/plain","origin":"chatgpt.com"}})),||{}).await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert!(app
        .store
        .artifacts(&app.workspace.id(), &sid)
        .unwrap()
        .is_empty());
    assert_eq!(
        call(
            &app,
            &sid,
            "request_download",
            json!({"handle":"a".repeat(32),"filename":"/tmp/unrelated"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
}

#[tokio::test]
async fn policy_lock_during_approval_prevents_the_browser_action() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let (result, count) = download(
        &app,
        &sid,
        file(DownloadMethod::WebsiteButton),
        true,
        Ok(json!({"state":"manual_share_required"})),
        || {
            let mut info = app.workspace.info("locked".into());
            info.read_only = true;
            app.store.save_workspace(&info).unwrap();
        },
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert_eq!(count, 0);
}

#[tokio::test]
async fn oversized_or_untrusted_discovery_metadata_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut bad = file(DownloadMethod::WebsiteButton);
    bad.name = "../secret".into();
    for response in [
        json!({"files":vec![file(DownloadMethod::WebsiteButton);41]}),
        json!({"files":[bad]}),
        json!({"files":[],"url":"https://secret.example"}),
    ] {
        let mut events = app.events.subscribe();
        let request = call(&app, &sid, "list_generated_files", json!({}));
        let respond = async {
            if let BridgeEvent::BrowserOperation { id, scope, .. } = events.recv().await.unwrap() {
                app.browser_files.resolve(&id, &scope, Ok(response));
            } else {
                panic!("missing browser request");
            }
        };
        let (result, _) = tokio::join!(request, respond);
        assert_eq!(result.code.as_deref(), Some("invalid_arguments"));
    }
}

#[tokio::test]
async fn expired_enclosing_request_never_dispatches_a_browser_action() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut events = app.events.subscribe();
    let result = bridge_native::browser_files::deadline(
        0,
        call(&app, &sid, "list_generated_files", json!({})),
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("file_changed"));
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, BridgeEvent::BrowserOperation { .. }));
    }
}
