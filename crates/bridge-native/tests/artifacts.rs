use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    fsops,
    tools::{
        self,
        artifacts::{self, SelectedDownload, MAX_BYTES},
    },
    wire::{BridgeEvent, ToolResultMessage},
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc};

fn fixture(temp: &tempfile::TempDir, profile: Profile) -> (Arc<App>, String) {
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
            "https://chatgpt.com/c/artifacts",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    (app, sid)
}
fn selected(path: &Path, id: i32) -> SelectedDownload {
    SelectedDownload {
        download_id: id,
        filename: path.to_string_lossy().into(),
        bytes: std::fs::metadata(path).unwrap().len(),
        mime: "application/octet-stream".into(),
        origin: "files.example".into(),
    }
}
fn share(app: &App, sid: &str, path: &Path, id: i32) -> Value {
    artifacts::register(app, sid, &app.workspace.id(), selected(path, id)).unwrap()
}
async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tools::dispatch(app, Agent::ChatGpt, sid, tool, args).await
}
async fn approval(
    app: &Arc<App>,
    sid: &str,
    args: Value,
    allow: bool,
    during: impl FnOnce(),
) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let task_app = app.clone();
    let session = sid.to_owned();
    let task =
        tokio::spawn(async move { call(&task_app, &session, "import_artifact", args).await });
    loop {
        if let BridgeEvent::ApprovalRequired { approval } =
            tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            during();
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
}
fn ok(result: ToolResultMessage) -> Value {
    assert_eq!(result.status, "ok", "{result:?}");
    result.data.unwrap()
}

#[tokio::test]
async fn binary_import_approval_hash_original_audit_persistence_and_dedup() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::General);
    let bytes: Vec<u8> = (0..2_000_000).map(|n| (n % 256) as u8).collect();
    let source = temp.path().canonicalize().unwrap().join("asset.bin");
    std::fs::write(&source, &bytes).unwrap();
    let file = share(&app, &sid, &source, 10);
    let id = file["artifact_id"].as_str().unwrap();
    assert_eq!(share(&app, &sid, &source, 10), file);
    let list = ok(call(&app, &sid, "list_artifacts", json!({})).await);
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(source.to_str().unwrap()));
    let args = json!({"artifact_id":id,"path":"files/asset.bin"});
    assert_eq!(
        approval(&app, &sid, args.clone(), false, || {})
            .await
            .code
            .as_deref(),
        Some("approval_denied")
    );
    assert!(!app.workspace.root.join("files/asset.bin").exists());
    let imported = ok(approval(&app, &sid, args.clone(), true, || {}).await);
    assert_eq!(imported["sha256"], fsops::sha256_bytes(&bytes));
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/asset.bin")).unwrap(),
        bytes
    );
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        call(
            &app,
            &sid,
            "import_artifact",
            json!({"artifact_id":id,"path":"files/duplicate.bin"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    let loaded = App::load(app.home.clone(), true).unwrap();
    assert_eq!(
        ok(call(&loaded, &sid, "list_artifacts", json!({})).await)["artifacts"][0]["state"],
        "imported"
    );
    let audit = std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    for action in [
        "artifact.share",
        "approval.request:import_artifact",
        "tool.import_artifact",
        "artifact.prepare",
    ] {
        assert!(audit.contains(action), "{action}");
    }
    assert!(!audit.contains(source.to_str().unwrap()));
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn session_workspace_scope_and_strict_arguments_are_enforced() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::General);
    let path = temp.path().canonicalize().unwrap().join("source.bin");
    std::fs::write(&path, b"secret bytes").unwrap();
    let file = share(&app, &sid, &path, 1);
    let other = app
        .register_session(
            Provider::Claude,
            2,
            1,
            "https://claude.ai/chat/artifacts",
            None,
            None,
        )
        .unwrap()
        .session
        .id;
    assert_eq!(
        ok(call(&app, &other, "list_artifacts", json!({})).await)["artifacts"],
        json!([])
    );
    assert_eq!(
        call(
            &app,
            &other,
            "import_artifact",
            json!({"artifact_id":file["artifact_id"],"path":"files/x"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(&app, "unknown", "list_artifacts", json!({}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(&app, &sid, "list_artifacts", json!({"session_id":other}))
            .await
            .code
            .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "import_artifact",
            json!({"artifact_id":file["artifact_id"],"path":"files/x","source":path})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert!(artifacts::register(&app, &sid, "wrong", selected(&path, 2)).is_err());
    let second =
        Workspace::initialize_profile(&temp.path().join("second"), Profile::General).unwrap();
    assert!(artifacts::register(
        &app.with_workspace(second),
        &sid,
        &app.workspace.id(),
        selected(&path, 2)
    )
    .is_err());
}

#[tokio::test]
async fn approval_revalidates_source_destination_policy_and_revocation() {
    for case in [
        "hash",
        "inode",
        "source_symlink",
        "source_parent_symlink",
        "destination_symlink",
        "destination_parent_symlink",
        "collision",
        "locked",
        "revoked",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&temp, Profile::General);
        let dir = temp.path().canonicalize().unwrap().join("downloads");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("source.bin");
        std::fs::write(&path, b"original").unwrap();
        let file = share(&app, &sid, &path, 1);
        let dest = app.workspace.root.join("files/import.bin");
        let outside = temp.path().canonicalize().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let args = json!({"artifact_id":file["artifact_id"],"path":"files/import.bin"});
        let result = approval(&app, &sid, args, true, || match case {
            "hash" => std::fs::write(&path, b"modified").unwrap(),
            "inode" => {
                std::fs::rename(&path, dir.join("old.bin")).unwrap();
                std::fs::write(&path, b"original").unwrap();
            }
            "source_symlink" => {
                std::fs::rename(&path, dir.join("old.bin")).unwrap();
                std::os::unix::fs::symlink(dir.join("old.bin"), &path).unwrap();
            }
            "source_parent_symlink" => {
                let old = dir.with_file_name("old-downloads");
                std::fs::rename(&dir, &old).unwrap();
                std::os::unix::fs::symlink(&old, &dir).unwrap();
            }
            "destination_symlink" => {
                std::os::unix::fs::symlink(outside.join("escape"), &dest).unwrap()
            }
            "destination_parent_symlink" => {
                std::fs::rename(dest.parent().unwrap(), app.workspace.root.join("old-files"))
                    .unwrap();
                std::os::unix::fs::symlink(&outside, dest.parent().unwrap()).unwrap();
            }
            "collision" => std::fs::write(&dest, b"keep me").unwrap(),
            "locked" => {
                let mut info = app
                    .store
                    .workspaces()
                    .unwrap()
                    .into_iter()
                    .find(|w| w.id == app.workspace.id())
                    .unwrap();
                info.read_only = true;
                app.store.save_workspace(&info).unwrap();
            }
            "revoked" => {
                artifacts::revoke(
                    &app,
                    &sid,
                    &app.workspace.id(),
                    file["artifact_id"].as_str().unwrap(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        })
        .await;
        assert_eq!(result.status, "error", "{case}: {result:?}");
        assert!(!outside.join("import.bin").exists());
        assert!(!outside.join("escape").exists());
        if case == "collision" {
            assert_eq!(std::fs::read(&dest).unwrap(), b"keep me");
        } else {
            assert!(!dest.is_file(), "{case}");
        }
        assert!(!std::fs::read_dir(app.workspace.root.join(
            if case == "destination_parent_symlink" {
                "old-files"
            } else {
                "files"
            }
        ))
        .unwrap()
        .any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".bridge-import-")));
    }
}

#[tokio::test]
async fn size_private_files_symlinks_special_files_and_path_denials() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::General);
    let path = temp.path().canonicalize().unwrap().join("source.bin");
    std::fs::write(&path, b"original").unwrap();
    let mut large = selected(&path, 1);
    large.bytes = MAX_BYTES + 1;
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), large).is_err());
    let sparse = path.with_file_name("large.bin");
    std::fs::File::create(&sparse)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), selected(&sparse, 2)).is_err());
    let secret = path.with_file_name(".env");
    std::fs::write(&secret, b"private").unwrap();
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), selected(&secret, 3)).is_err());
    let link = path.with_file_name("link.bin");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), selected(&link, 4)).is_err());
    let hard = path.with_file_name("hard.bin");
    std::fs::hard_link(&path, &hard).unwrap();
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), selected(&hard, 5)).is_err());
    std::fs::remove_file(hard).unwrap();
    let mut dir = selected(&path, 6);
    dir.filename = temp.path().canonicalize().unwrap().to_string_lossy().into();
    assert!(artifacts::register(&app, &sid, &app.workspace.id(), dir).is_err());
    let file = share(&app, &sid, &path, 7);
    for denied in [
        "../escape",
        "/etc/escape",
        ".bridge/metadata",
        "files/.env",
        "missing-parent/file",
        "files",
    ] {
        let result = call(
            &app,
            &sid,
            "import_artifact",
            json!({"artifact_id":file["artifact_id"],"path":denied}),
        )
        .await;
        assert_eq!(result.status, "error", "{denied}: {result:?}");
    }
}

#[tokio::test]
async fn research_agent_and_dataset_protections_still_apply() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp, Profile::Research);
    let source = temp.path().canonicalize().unwrap().join("source.bin");
    std::fs::write(&source, b"bytes").unwrap();
    let file = share(&app, &sid, &source, 1);
    for path in [
        "AGENTS/CLAUDE/notes/asset.bin",
        "PROJECT/datasets/asset.bin",
        "SHARED/DATASETS/asset.bin",
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "import_artifact",
                json!({"artifact_id":file["artifact_id"],"path":path})
            )
            .await
            .status,
            "error"
        );
    }
    ok(approval(
        &app,
        &sid,
        json!({"artifact_id":file["artifact_id"],"path":"AGENTS/CHATGPT/notes/asset.bin"}),
        true,
        || {},
    )
    .await);
}

#[tokio::test]
async fn expired_grants_and_interrupted_imports_fail_closed_after_restart() {
    for state in ["expired", "importing"] {
        let temp = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&temp, Profile::General);
        let source = temp.path().canonicalize().unwrap().join("source.bin");
        std::fs::write(&source, b"bytes").unwrap();
        let file = share(&app, &sid, &source, 1);
        let record = app
            .store
            .artifacts(&app.workspace.id(), &sid)
            .unwrap()
            .pop()
            .unwrap();
        let mut value = serde_json::to_value(record).unwrap();
        if state == "expired" {
            value["expires_at"] = json!(0);
        } else {
            value["state"] = json!(state);
            value["destination"] = json!("files/interrupted.bin");
        }
        app.store
            .save_artifact(&serde_json::from_value(value).unwrap())
            .unwrap();
        let reopened = App::load(app.home.clone(), true).unwrap();
        assert_eq!(
            ok(call(&reopened, &sid, "list_artifacts", json!({})).await)["artifacts"][0]["state"],
            state
        );
        assert_eq!(
            call(
                &reopened,
                &sid,
                "import_artifact",
                json!({"artifact_id":file["artifact_id"],"path":"files/retry.bin"})
            )
            .await
            .status,
            "error"
        );
        assert!(!app.workspace.root.join("files/retry.bin").exists());
    }
}
