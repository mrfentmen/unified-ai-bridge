//! Real filesystem and native policy tests; fixtures are isolated temporary roots.
use bridge_core::{
    provider::{Agent, Provider},
    workspace::{PermissionMode, Profile},
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    tools,
    wire::{BridgeEvent, ExtToHost, ToolResultMessage},
    workspace::Workspace,
    yolo::{self, Control},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn fixture(t: &tempfile::TempDir, profile: Profile) -> (Arc<App>, String) {
    let base = t.path().join("home");
    let root = t.path().join("workspace");
    let home = HomePaths {
        base: base.clone(),
        config_file: base.join("config.toml"),
        database: base.join("bridge.sqlite3"),
        logs_dir: base.join("logs"),
        default_project: root.clone(),
        native_hosts_dir: base.join("hosts"),
        install_dir: base.join("bin"),
    };
    let mut workspace = Workspace::initialize_profile(&root, profile).unwrap();
    workspace.permission_mode = PermissionMode::Strict;
    let config = Config {
        project_root: workspace.root.clone(),
        ..Default::default()
    };
    home.save_config(&config).unwrap();
    let app = App::open_with_config(home, config, workspace, true).unwrap();
    let sid = app
        .register_session(
            Provider::ChatGpt,
            1,
            1,
            "https://chatgpt.com/c/yolo-policy-test",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}
fn enable(app: &App, seconds: u64, cap: u32) {
    yolo::control(
        app,
        &app.workspace.id(),
        Control::Enable {
            seconds,
            max_approvals: cap,
        },
    )
    .unwrap();
}
async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tokio::time::timeout(
        Duration::from_secs(3),
        tools::dispatch(app, Agent::ChatGpt, sid, tool, args),
    )
    .await
    .expect("unexpected approval stall")
}
async fn denied_prompt(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let reject = async {
        loop {
            if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                assert!(app.approvals.resolve(&approval.id, false));
                break;
            }
        }
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(call(app, sid, tool, args), reject)
    })
    .await
    .expect("expected approval event");
    assert_eq!(result.code.as_deref(), Some("approval_denied"));
    result
}
fn write(path: &str) -> Value {
    json!({"path":path,"content":"YOLO policy fixture\n","create_only":true})
}

#[tokio::test]
async fn off_enable_real_write_patch_audit_and_stop() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    assert_eq!(yolo::status(&app)["active"], false);
    denied_prompt(&app, &sid, "write_document", write("files/off.rs")).await;
    enable(&app, 60, 10);
    assert_eq!(
        call(&app, &sid, "write_document", write("files/on.rs"))
            .await
            .status,
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/on.rs")).unwrap(),
        "YOLO policy fixture\n"
    );
    let p = call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/patch.rs","content":"fn main() {}\n"}]}),
    )
    .await;
    assert_eq!(p.status, "ok", "{p:?}");
    let id = p.data.unwrap()["patch_id"].clone();
    assert_eq!(
        call(&app, &sid, "apply_patch", json!({"patch_id":id}))
            .await
            .status,
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/patch.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert_eq!(yolo::status(&app)["used_approvals"], 2);
    yolo::control(&app, &app.workspace.id(), Control::Stop).unwrap();
    denied_prompt(&app, &sid, "write_document", write("files/stopped.rs")).await;
    assert!(!app.workspace.root.join("files/stopped.rs").exists());
    assert!(app.store.verify_audit().unwrap().valid);
    let audit = std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    for marker in [
        "yolo.enable",
        "yolo.stop",
        "approval.yolo:write_document",
        "approval.yolo:apply_patch",
    ] {
        assert!(audit.contains(marker), "{marker}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expiry_cap_and_concurrent_reservations_restore_normal_approval() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 1, 10);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(yolo::status(&app)["active"], false);
    denied_prompt(&app, &sid, "write_document", write("files/expired.txt")).await;
    enable(&app, 60, 1);
    let mut events = app.events.subscribe();
    let reject = async {
        loop {
            if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                assert!(app.approvals.resolve(&approval.id, false));
                break;
            }
        }
    };
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let launch = |path: &'static str| {
        let app = app.clone();
        let sid = sid.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            call(&app, &sid, "write_document", write(path)).await
        })
    };
    let a = launch("files/a.txt");
    let b = launch("files/b.txt");
    let (a, b, ()) =
        tokio::time::timeout(Duration::from_secs(4), async { tokio::join!(a, b, reject) })
            .await
            .unwrap();
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        [a.status, b.status].iter().filter(|s| *s == "ok").count(),
        1
    );
    assert_eq!(
        ["files/a.txt", "files/b.txt"]
            .iter()
            .filter(|p| app.workspace.root.join(p).exists())
            .count(),
        1
    );
    assert_eq!(yolo::status(&app)["used_approvals"], 1);
    assert_eq!(yolo::status(&app)["active"], false);
}

#[tokio::test]
async fn traversal_private_symlink_lock_and_delete_are_not_bypassed() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 60, 20);
    let outside = t.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, app.workspace.root.join("files/link")).unwrap();
    for path in [
        "../escape.txt",
        ".bridge/secret.txt",
        "files/.env",
        "files/link/escape.txt",
    ] {
        let r = call(&app, &sid, "write_document", write(path)).await;
        assert_eq!(
            r.code.as_deref(),
            Some("permission_denied"),
            "{path}: {r:?}"
        );
    }
    assert_eq!(
        call(&app, &sid, "write_document", write("files/keep.txt"))
            .await
            .status,
        "ok"
    );
    denied_prompt(&app,&sid,"delete_file",json!({"path":"files/keep.txt","expected_sha256":bridge_native::fsops::sha256_bytes(b"YOLO policy fixture\n")})).await;
    assert!(app.workspace.root.join("files/keep.txt").exists());
    let mut info = app.store.workspaces().unwrap().remove(0);
    info.read_only = true;
    app.store.save_workspace(&info).unwrap();
    assert!(yolo::control(
        &app,
        &app.workspace.id(),
        Control::Enable {
            seconds: 60,
            max_approvals: 10
        }
    )
    .is_err());
    assert_eq!(
        call(&app, &sid, "write_document", write("files/locked.txt"))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert!(!outside.join("escape.txt").exists());
}

#[tokio::test]
async fn research_identity_and_dataset_denials_still_apply() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::Research);
    enable(&app, 60, 20);
    for path in ["AGENTS/CLAUDE/notes/other.md", "SHARED/datasets/data.csv"] {
        let r = call(&app, &sid, "write_document", write(path)).await;
        assert_eq!(r.code.as_deref(), Some("permission_denied"), "{r:?}");
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "write_document",
            write("AGENTS/CHATGPT/notes/own.md")
        )
        .await
        .status,
        "ok"
    );
}

#[tokio::test]
async fn models_cannot_enable_or_expand_permissions_and_commands_stay_allowlisted() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 60, 20);
    assert_eq!(
        call(&app, &sid, "yolo_control", json!({"operation":"enable"}))
            .await
            .code
            .as_deref(),
        Some("unknown_tool")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "run_command",
            json!({"command_id":"sh","args":["-c","touch /tmp/escape"]})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    for control in [
        json!({"operation":"enable","seconds":0,"max_approvals":1}),
        json!({"operation":"enable","seconds":43201,"max_approvals":1}),
        json!({"operation":"enable","seconds":1,"max_approvals":10001}),
    ] {
        assert!(yolo::control(
            &app,
            &app.workspace.id(),
            serde_json::from_value(control).unwrap()
        )
        .is_err());
    }
    assert!(serde_json::from_value::<ExtToHost>(json!({"type":"yolo_control","request_key":"x","workspace_id":app.workspace.id(),"action":{"operation":"enable","seconds":60,"max_approvals":1,"allow_delete":true}})).is_err());
    assert!(yolo::control(
        &app,
        "another-workspace",
        Control::Enable {
            seconds: 60,
            max_approvals: 1
        }
    )
    .is_err());
}

#[tokio::test]
async fn narrower_session_grants_and_changed_patch_hashes_remain_authoritative() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 60, 20);
    let p = call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"files/stale.rs","content":"preview"}]}),
    )
    .await;
    std::fs::write(app.workspace.root.join("files/stale.rs"), "later user edit").unwrap();
    let r = call(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p.data.unwrap()["patch_id"]}),
    )
    .await;
    assert_eq!(r.code.as_deref(), Some("file_changed"));
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/stale.rs")).unwrap(),
        "later user edit"
    );
    bridge_native::grants::create(
        &app,
        &app.workspace.id(),
        bridge_native::grants::Request {
            session_id: sid.clone(),
            directory: "files".into(),
            tools: vec!["write_document".into()],
            seconds: 60,
            max_calls: 1,
            max_bytes: 1024,
            max_files: 1,
        },
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "write_document", write("files/granted.txt"))
            .await
            .status,
        "ok"
    );
    assert_eq!(
        call(&app, &sid, "write_document", write("files/exhausted.txt"))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let p = call(
        &app,
        &sid,
        "preview_patch",
        json!({"operations":[{"path":"output/outside-grant.rs","content":"preview"}]}),
    )
    .await;
    denied_prompt(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":p.data.unwrap()["patch_id"]}),
    )
    .await;
    assert!(!app.workspace.root.join("output/outside-grant.rs").exists());
    assert_eq!(yolo::status(&app)["used_approvals"], 0);
}

#[tokio::test]
async fn workspace_switch_and_restart_turn_off_yolo() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 60, 20);
    let reopened = App::load(app.home.clone(), true).unwrap();
    assert_eq!(yolo::status(&reopened)["active"], false);
    let next = app
        .create_workspace(
            "Second",
            t.path().join("second").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let switched = app.select_workspace(&next.id).unwrap();
    assert_eq!(yolo::status(&switched)["active"], false);
    assert_eq!(yolo::status(&app)["active"], false);
    let old = switched.select_workspace(&app.workspace.id()).unwrap();
    denied_prompt(
        &old,
        &sid,
        "write_document",
        write("files/after-switch.txt"),
    )
    .await;
}

#[tokio::test]
async fn revoked_or_expired_approval_is_rechecked_before_later_mutations() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t, Profile::General);
    enable(&app, 60, 20);
    yolo::scope(async {
        app.request_approval(
            &sid,
            Agent::ChatGpt,
            "write_document",
            "fixture",
            Some("files/lease.txt".into()),
        )
        .await
        .unwrap();
        yolo::checkpoint(&app).unwrap();
        yolo::stop(&app, "fixture revocation").unwrap();
        enable(&app, 60, 20); // Replacement grant cannot revive the old approval.
        assert!(yolo::checkpoint(&app).is_err());
        let path = app
            .sandbox()
            .resolve("files/lease.txt", bridge_native::sandbox::Existence::Any)
            .unwrap();
        assert!(matches!(
            app.current_write_decision(
                Agent::ChatGpt,
                bridge_native::sandbox::Access::Write,
                &path
            ),
            bridge_native::sandbox::Decision::Deny(_)
        ));
    })
    .await;
    enable(&app, 1, 20);
    yolo::scope(async {
        app.request_approval(
            &sid,
            Agent::ChatGpt,
            "write_document",
            "fixture",
            Some("files/lease.txt".into()),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(yolo::checkpoint(&app).is_err());
    })
    .await;
}
