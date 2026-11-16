use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    tools::{
        self,
        network::{self, Control},
    },
    wire::BridgeEvent,
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn fixture(temp: &tempfile::TempDir, profile: Profile) -> (Arc<App>, String) {
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
    let session = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/network",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, session)
}
fn enable(app: &Arc<App>) -> Arc<App> {
    network::control(
        app,
        &app.workspace.id(),
        Control::Save {
            enabled: true,
            origins: vec!["https://example.com".into()],
        },
    )
    .unwrap()
    .0
}

#[tokio::test]
async fn network_off_default_unknown_arguments_origins_and_paths_are_denied_before_transfer() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::General);
    let result = tools::dispatch(
        &app,
        Agent::ChatGpt,
        &sid,
        "fetch_url_text",
        json!({"url":"https://example.com/"}),
    )
    .await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert_eq!(app.approvals.pending_count(), 0);
    assert!(!network::policy(&app).enabled);
    let app = enable(&app);
    for args in [
        json!({"url":"https://example.org/"}),
        json!({"url":"https://example.com/","headers":{"Authorization":"no"}}),
        json!({"url":"http://169.254.169.254/"}),
        json!({"url":"https://example.com/?token=secret"}),
        json!({"url":"https://example.com/","max_bytes":24001}),
    ] {
        assert_eq!(
            tools::dispatch(&app, Agent::ChatGpt, &sid, "fetch_url_text", args)
                .await
                .status,
            "error"
        );
    }
    for path in [
        "../escape.bin",
        ".env",
        ".bridge/private.bin",
        "/etc/bridge-denied.bin",
    ] {
        assert_eq!(
            tools::dispatch(
                &app,
                Agent::ChatGpt,
                &sid,
                "download_file",
                json!({"url":"https://example.com/","destination":path})
            )
            .await
            .status,
            "error"
        );
    }
    std::fs::write(app.workspace.root.join("files/exists.bin"), b"original").unwrap();
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::ChatGpt,
            &sid,
            "download_file",
            json!({"url":"https://example.com/","destination":"files/exists.bin"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/exists.bin")).unwrap(),
        b"original"
    );
    assert_eq!(app.approvals.pending_count(), 0);
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn network_approval_denial_and_revocation_send_nothing_and_save_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::General);
    let app = enable(&app);
    for revoke in [false, true] {
        let mut events = app.events.subscribe();
        let a = app.clone();
        let s = sid.clone();
        let task = tokio::spawn(async move {
            tools::dispatch(
                &a,
                Agent::ChatGpt,
                &s,
                "download_file",
                json!({"url":"https://example.com/","destination":"output/refused.bin"}),
            )
            .await
        });
        let approval = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                    break approval;
                }
            }
        })
        .await
        .unwrap();
        if revoke {
            network::control(
                &app,
                &app.workspace.id(),
                Control::Save {
                    enabled: false,
                    origins: vec![],
                },
            )
            .unwrap();
        } else {
            assert!(app.approvals.resolve(&approval.id, false));
        }
        let r = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            r.code.as_deref(),
            Some(if revoke {
                "permission_denied"
            } else {
                "approval_denied"
            })
        );
        assert!(!app.workspace.root.join("output/refused.bin").exists());
        assert_eq!(app.approvals.pending_count(), 0);
        assert!(!app.approvals.resolve(&approval.id, true));
    }
    let audit = std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    assert!(!audit.contains("network.request"));
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn network_grants_are_scoped_and_research_guards_still_apply() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::Research);
    let app = enable(&app);
    for path in ["AGENTS/CLAUDE/notes/no.bin", "SHARED/datasets/no.bin"] {
        assert_eq!(
            tools::dispatch(
                &app,
                Agent::ChatGpt,
                &sid,
                "download_file",
                json!({"url":"https://example.com/","destination":path})
            )
            .await
            .code
            .as_deref(),
            Some("permission_denied")
        );
    }
    let fresh = app
        .create_workspace(
            "Fresh",
            temp.path().join("fresh").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let next = app.select_workspace(&fresh.id).unwrap();
    assert!(!network::policy(&next).enabled);
    assert!(network::control(
        &next,
        &app.workspace.id(),
        Control::Save {
            enabled: true,
            origins: vec!["https://example.com".into()]
        }
    )
    .is_err());
    for origin in [
        "http://127.0.0.1",
        "https://example.com/path",
        "https://example.com/?token=x",
    ] {
        assert!(network::control(
            &next,
            &next.workspace.id(),
            Control::Save {
                enabled: true,
                origins: vec![origin.into()]
            }
        )
        .is_err());
    }
    let before = network::policy(&app);
    let next = enable(&app);
    assert_ne!(before.revision, network::policy(&next).revision);
    let value: Value = serde_json::to_value(next.config.clone()).unwrap();
    assert!(value["workspace_network"][app.workspace.id()]["enabled"]
        .as_bool()
        .unwrap());
    assert!(app.store.verify_audit().unwrap().valid);
}
