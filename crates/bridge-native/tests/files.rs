use bridge_core::{provider::Agent, workspace::Profile};
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

fn fixture(temp: &tempfile::TempDir, profile: Profile) -> Arc<App> {
    let base = temp.path().join("home");
    let root = temp.path().join("workspace");
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
    let mut config = Config::default();
    config.project_root = workspace.root.clone();
    home.save_config(&config).unwrap();
    App::open_with_config(home, config, workspace, true).unwrap()
}
async fn call(app: &App, tool: &str, args: Value) -> ToolResultMessage {
    tools::dispatch(app, Agent::ChatGpt, "file-test", tool, args).await
}
async fn approve(app: &Arc<App>, tool: &str, args: Value, allow: bool) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let a = app.clone();
    let name = tool.to_string();
    let pending = tokio::spawn(async move { call(&a, &name, args).await });
    loop {
        if let BridgeEvent::ApprovalRequired { approval } =
            tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    pending.await.unwrap()
}
fn ok(result: ToolResultMessage) -> Value {
    assert_eq!(result.status, "ok", "{result:?}");
    result.data.unwrap()
}

async fn during_approval(
    app: &Arc<App>, tool: &'static str, args: Value, change: impl FnOnce(),
) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let a = app.clone();
    let pending = tokio::spawn(async move { call(&a, tool, args).await });
    loop {
        if let BridgeEvent::ApprovalRequired { approval } =
            tokio::time::timeout(std::time::Duration::from_secs(3), events.recv())
                .await.unwrap().unwrap()
        {
            change();
            assert!(app.approvals.resolve(&approval.id, true));
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(3), pending).await.unwrap().unwrap()
}

fn policy(app: &App, read_only: bool, strict: bool) {
    use bridge_core::workspace::PermissionMode;
    let mut info = app.store.workspaces().unwrap().into_iter()
        .find(|w| w.id == app.workspace.id()).unwrap();
    info.read_only = read_only;
    info.permission_mode = if strict { PermissionMode::Strict } else { PermissionMode::Standard };
    app.store.save_workspace(&info).unwrap();
}

#[tokio::test]
async fn strict_folder_creation_requires_approval_in_every_profile() {
    for profile in [Profile::Research, Profile::General, Profile::Code] {
        let temp = tempfile::tempdir().unwrap();
        let app = fixture(&temp, profile);
        policy(&app, false, true);
        let path = if profile == Profile::Research { "AGENTS/CHATGPT/strict-folder" } else { "strict-folder" };
        let args = json!({"path":path});
        let denied = approve(&app, "create_folder", args.clone(), false).await;
        assert_eq!(denied.code.as_deref(), Some("approval_denied"));
        assert!(!app.workspace.root.join(path).exists());
        let allowed = approve(&app, "create_folder", args, true).await;
        assert_eq!(allowed.status, "ok", "{allowed:?}");
        assert!(app.workspace.root.join(path).is_dir());
        if profile == Profile::Research {
            let result = approve(&app, "create_folder", json!({"path":"COLLABORATION/proposals/strict-folder"}), true).await;
            assert_eq!(result.status, "ok");
            for denied in ["SHARED/datasets/strict-folder", "AGENTS/CLAUDE/strict-folder"] {
                let result = tokio::time::timeout(std::time::Duration::from_secs(3),
                    call(&app, "create_folder", json!({"path":denied}))).await.unwrap();
                assert_eq!(result.code.as_deref(), Some("permission_denied"));
                assert!(!app.workspace.root.join(denied).exists());
            }
        }
        assert!(app.store.verify_audit().unwrap().valid);
    }
}

#[tokio::test]
async fn move_refuses_destination_created_during_approval() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let source = app.workspace.root.join("files/source.md");
    let destination = app.workspace.root.join("files/destination.md");
    std::fs::write(&source, "source").unwrap();
    let result = during_approval(&app, "move_file",
        json!({"source":"files/source.md","destination":"files/destination.md"}), || {
            std::fs::write(&destination, "independent new destination").unwrap();
        }).await;
    assert_eq!(result.code.as_deref(), Some("file_changed"), "{result:?}");
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    assert_eq!(std::fs::read(&destination).unwrap(), b"independent new destination");
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn move_rechecks_source_and_policy_after_approval() {
    for mutation in ["source", "lock", "strict"] {
        let temp = tempfile::tempdir().unwrap();
        let app = fixture(&temp, Profile::General);
        let source = app.workspace.root.join("files/source.md");
        std::fs::write(&source, "source").unwrap();
        let result = during_approval(&app, "move_file",
            json!({"source":"files/source.md","destination":"files/new/destination.md"}), || {
                match mutation {
                    "source" => std::fs::write(&source, "user edit").unwrap(),
                    "lock" => policy(&app, true, false),
                    _ => policy(&app, false, true),
                }
            }).await;
        assert_eq!(result.code.as_deref(), Some(if mutation == "source" { "file_changed" } else { "permission_denied" }), "{result:?}");
        assert_eq!(std::fs::read(&source).unwrap(), if mutation == "source" { b"user edit".as_slice() } else { b"source".as_slice() });
        assert!(!app.workspace.root.join("files/new").exists());
        assert!(app.store.verify_audit().unwrap().valid);
    }
}

#[tokio::test]
async fn move_approval_allow_and_deny_preserve_expected_files() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let source = app.workspace.root.join("files/source.md");
    let destination = app.workspace.root.join("files/new/destination.md");
    std::fs::write(&source, "source").unwrap();
    let args = json!({"source":"files/source.md","destination":"files/new/destination.md"});
    assert_eq!(approve(&app, "move_file", args.clone(), false).await.code.as_deref(), Some("approval_denied"));
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    assert!(!destination.exists());
    assert_eq!(approve(&app, "move_file", args, true).await.status, "ok");
    assert!(!source.exists());
    assert_eq!(std::fs::read(destination).unwrap(), b"source");
    assert!(app.store.verify_audit().unwrap().valid);
}

#[cfg(unix)]
#[tokio::test]
async fn legacy_mutations_reject_symlink_swaps_while_approval_waits() {
    for tool in ["move_file", "create_folder"] {
        for swap in ["source", "destination"] {
            let temp = tempfile::tempdir().unwrap();
            let app = fixture(&temp, Profile::General);
            let outside = temp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            let source = app.workspace.root.join("files/source.md");
            std::fs::write(&source, "source").unwrap();
            let destination_parent = app.workspace.root.join("files/new");
            std::fs::create_dir(&destination_parent).unwrap();
            if tool == "create_folder" { policy(&app, false, true); }
            let args = if tool == "move_file" {
                json!({"source":"files/source.md","destination":"files/new/destination.md"})
            } else { json!({"path":"files/new/folder"}) };
            let result = during_approval(&app, tool, args, || {
                if tool == "move_file" && swap == "source" {
                    std::fs::remove_file(&source).unwrap();
                    std::os::unix::fs::symlink(outside.join("original"), &source).unwrap();
                } else {
                    std::fs::remove_dir(&destination_parent).unwrap();
                    std::os::unix::fs::symlink(&outside, &destination_parent).unwrap();
                }
            }).await;
            assert_eq!(result.code.as_deref(), Some("permission_denied"), "{result:?}");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
            assert!(app.store.verify_audit().unwrap().valid);
        }
    }
}

#[tokio::test]
async fn folder_and_create_only_write_recheck_approval_wait_changes() {
    for tool in ["create_folder", "write_document"] {
        let temp = tempfile::tempdir().unwrap();
        let app = fixture(&temp, Profile::General);
        policy(&app, false, true);
        let target = app.workspace.root.join("files/new.md");
        let args = if tool == "create_folder" { json!({"path":"files/new.md"}) }
            else { json!({"path":"files/new.md","content":"replace","create_only":true}) };
        let result = during_approval(&app, tool, args, || std::fs::write(&target, "keep").unwrap()).await;
        assert_eq!(result.code.as_deref(), Some("file_changed"), "{result:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::Research);
    policy(&app, false, true);
    let result = during_approval(&app, "create_folder", json!({"path":"AGENTS/CHATGPT/locked"}),
        || policy(&app, true, true)).await;
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert!(!app.workspace.root.join("AGENTS/CHATGPT/locked").exists());
}

#[tokio::test]
async fn real_files_preview_stale_edits_restore_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let first = ok(call(
        &app,
        "write_document",
        json!({"path":"files/note.md","content":"first\n","create_only":true}),
    )
    .await);
    let preview = ok(call(
        &app,
        "preview_document",
        json!({"path":"files/note.md","content":"second\n"}),
    )
    .await);
    assert_eq!(preview["expected_sha256"], first["sha256"]);
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/note.md")).unwrap(),
        "first\n"
    );
    assert_eq!(
        call(
            &app,
            "write_document",
            json!({"path":"files/note.md","content":"no hash"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    let changed=ok(call(&app,"edit_document",json!({"path":"files/note.md","old_text":"first","new_text":"second","expected_sha256":first["sha256"]})).await);
    assert_eq!(
        call(
            &app,
            "append_document",
            json!({"path":"files/note.md","content":"stale","expected_sha256":first["sha256"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    let reloaded = App::load(app.home.clone(), true).unwrap();
    let changes = ok(call(&reloaded, "list_changes", json!({})).await);
    assert_eq!(changes["changes"].as_array().unwrap().len(), 2);
    assert!(!changes.to_string().contains("before_base64"));
    ok(approve(
        &reloaded,
        "restore_change",
        json!({"change_id":changed["change_id"]}),
        true,
    )
    .await);
    assert_eq!(
        std::fs::read_to_string(app.workspace.root.join("files/note.md")).unwrap(),
        "first\n"
    );
    assert_eq!(
        call(
            &reloaded,
            "restore_change",
            json!({"change_id":changed["change_id"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    assert!(reloaded.store.verify_audit().unwrap().valid);
    println!("real file edited and restored after host reload; stale overwrite/append/restore refused; audit valid");
}

#[tokio::test]
async fn file_toolbox_allow_deny_and_bounded_results() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let initial = ok(call(
        &app,
        "write_document",
        json!({"path":"files/a.rs","content":"fn main() {}\n"}),
    )
    .await);
    let appended = ok(call(
        &app,
        "append_document",
        json!({"path":"files/a.rs","content":"// appended\n","expected_sha256":initial["sha256"]}),
    )
    .await);
    assert_eq!(
        appended["sha256"],
        sha256_bytes(b"fn main() {}\n// appended\n")
    );
    ok(call(
        &app,
        "copy_file",
        json!({"source":"files/a.rs","destination":"files/b.rs"}),
    )
    .await);
    assert!(call(
        &app,
        "copy_file",
        json!({"source":"files/a.rs","destination":"files/b.rs"})
    )
    .await
    .code
    .is_some());
    for tool in ["file_tree", "glob_files"] {
        let args = if tool == "glob_files" {
            json!({"pattern":"**/*.rs","max_results":1})
        } else {
            json!({"path":"files","max_results":1})
        };
        let data = ok(call(&app, tool, args).await);
        assert_eq!(data["entries"].as_array().unwrap().len(), 1);
        assert_eq!(data["truncated"], true);
    }
    let batch = ok(call(
        &app,
        "read_multiple_files",
        json!({"paths":["files/a.rs","files/b.rs"]}),
    )
    .await);
    assert!(batch["files"][0]["content"]
        .as_str()
        .unwrap()
        .contains("appended"));
    ok(call(
        &app,
        "diff_files",
        json!({"path":"files/a.rs","content":"replacement\n"}),
    )
    .await);
    let deny_cases = [
        (
            "preview_document",
            json!({"path":"../outside","content":"x"}),
        ),
        (
            "append_document",
            json!({"path":"../outside","content":"x","expected_sha256":"x"}),
        ),
        (
            "copy_file",
            json!({"source":"files/a.rs","destination":"../outside"}),
        ),
        ("file_tree", json!({"path":"../outside"})),
        ("glob_files", json!({"path":"../outside","pattern":"*"})),
        (
            "read_multiple_files",
            json!({"paths":["files/a.rs","../outside"]}),
        ),
        (
            "diff_files",
            json!({"path":"files/a.rs","other_path":"../outside"}),
        ),
        (
            "delete_file",
            json!({"path":"../outside","expected_sha256":"x"}),
        ),
    ];
    for (tool, args) in deny_cases {
        assert_eq!(
            call(&app, tool, args).await.code.as_deref(),
            Some("permission_denied"),
            "{tool}"
        );
    }
    for tool in [
        "preview_document",
        "append_document",
        "copy_file",
        "file_tree",
        "glob_files",
        "read_multiple_files",
        "diff_files",
        "delete_file",
        "restore_change",
        "list_changes",
    ] {
        assert_eq!(
            call(&app, tool, json!({"unknown_field":true}))
                .await
                .code
                .as_deref(),
            Some("invalid_arguments"),
            "{tool}"
        );
    }
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn trash_requires_approval_and_restore_does_not_clobber() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let data = ok(call(
        &app,
        "write_document",
        json!({"path":"notes/delete.md","content":"keep me"}),
    )
    .await);
    let args = json!({"path":"notes/delete.md","expected_sha256":data["sha256"]});
    assert_eq!(
        approve(&app, "delete_file", args.clone(), false)
            .await
            .code
            .as_deref(),
        Some("approval_denied")
    );
    assert!(app.workspace.root.join("notes/delete.md").is_file());
    let deleted = ok(approve(&app, "delete_file", args, true).await);
    assert!(!app.workspace.root.join("notes/delete.md").exists());
    assert_eq!(
        std::fs::read(app.workspace.root.join(deleted["trash"].as_str().unwrap())).unwrap(),
        b"keep me"
    );
    ok(approve(
        &app,
        "restore_change",
        json!({"change_id":deleted["change_id"]}),
        true,
    )
    .await);
    assert_eq!(
        std::fs::read(app.workspace.root.join("notes/delete.md")).unwrap(),
        b"keep me"
    );
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn approval_rechecks_hash_and_symlink_and_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let data = ok(call(
        &app,
        "write_document",
        json!({"path":"files/a.txt","content":"before"}),
    )
    .await);
    let mut events = app.events.subscribe();
    let a = app.clone();
    let args = json!({"path":"files/a.txt","expected_sha256":data["sha256"]});
    let pending = tokio::spawn(async move { call(&a, "delete_file", args).await });
    let approval = loop {
        if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
            break approval;
        }
    };
    std::fs::write(app.workspace.root.join("files/a.txt"), "user edit").unwrap();
    app.approvals.resolve(&approval.id, true);
    assert_eq!(pending.await.unwrap().code.as_deref(), Some("file_changed"));
    let other = app
        .create_workspace(
            "Other",
            temp.path().join("other").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let switched = app.select_workspace(&other.id).unwrap();
    assert_eq!(
        call(
            &switched,
            "restore_change",
            json!({"change_id":data["change_id"]})
        )
        .await
        .code
        .as_deref(),
        Some("not_found")
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path(), app.workspace.root.join("escape")).unwrap();
        assert_eq!(
            call(
                &app,
                "copy_file",
                json!({"source":"files/a.txt","destination":"escape/outside"})
            )
            .await
            .code
            .as_deref(),
            Some("permission_denied")
        );
    }
}

#[tokio::test]
async fn research_changes_remain_private_and_dataset_writes_denied() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::Research);
    ok(call(
        &app,
        "write_document",
        json!({"path":"AGENTS/CHATGPT/notes/a.md","content":"private"}),
    )
    .await);
    let result = tools::dispatch(&app, Agent::Claude, "other", "list_changes", json!({})).await;
    assert_eq!(ok(result)["changes"].as_array().unwrap().len(), 0);
    for tool in ["write_document", "append_document", "delete_file"] {
        let args = match tool {
            "write_document" => json!({"path":"SHARED/datasets/immutable.csv","content":"no"}),
            "append_document" => {
                json!({"path":"AGENTS/CLAUDE/notes/a.md","content":"no","expected_sha256":"x"})
            }
            _ => json!({"path":"SHARED/datasets/immutable.csv","expected_sha256":"x"}),
        };
        let result = call(&app, tool, args).await;
        assert_ne!(result.status, "ok");
    }
}

#[tokio::test]
async fn qwen_and_kimi_receive_bound_sessions_and_keep_research_permissions() {
    use bridge_core::provider::Provider;
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::Research);
    for (provider, url) in [
        (Provider::Qwen, "https://chat.qwen.ai/c/provider-proof"),
        (Provider::Kimi, "https://www.kimi.ai/chat/provider-proof"),
    ] {
        let opened = app.register_session(provider, 90, 1, url, Some("provider-proof".into()), Some("native-test".into())).unwrap();
        let agent = provider.agent();
        assert_eq!(opened.session.agent, agent.dir());
        assert!(opened.instructions.contains(agent.dir()));
        let path = format!("AGENTS/{}/notes/provider-proof.md", agent.dir());
        let content = format!("{} actual temporary file\n", agent.display());
        ok(tools::dispatch(&app, agent, &opened.session.id, "write_document", json!({"path":path,"content":content,"create_only":true})).await);
        assert_eq!(std::fs::read_to_string(app.workspace.root.join(&path)).unwrap(), content);
        let read = tools::dispatch(&app, agent, &opened.session.id, "read_file", json!({"path":path})).await;
        assert_eq!(read.status, "ok");
        assert!(read.output.unwrap().contains(content.trim()));
        for forbidden in ["AGENTS/CHATGPT/notes/provider-denied.md", "SHARED/datasets/provider-denied.md", "../provider-denied.md"] {
            let result = tools::dispatch(&app, agent, &opened.session.id, "write_document", json!({"path":forbidden,"content":"must not exist"})).await;
            assert_eq!(result.code.as_deref(), Some("permission_denied"), "{result:?}");
            assert!(!app.workspace.root.join(forbidden).exists());
        }
        assert!(app.register_session(provider, 91, 1, "https://chatgpt.com/c/provider-proof", None, None).is_err());
    }
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn reads_are_chunked_and_private_files_are_denied() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp, Profile::General);
    let text = "é".repeat(300);
    std::fs::write(app.workspace.root.join("files/large.txt"), &text).unwrap();
    let first = ok(call(
        &app,
        "read_file",
        json!({"path":"files/large.txt","max_bytes":129}),
    )
    .await);
    assert_eq!(first["next_byte"], 128);
    let second = ok(call(
        &app,
        "read_file",
        json!({"path":"files/large.txt","start_byte":first["next_byte"],"max_bytes":128}),
    )
    .await);
    assert_eq!(second["next_byte"], 256);
    assert_eq!(first["sha256"], second["sha256"]);
    std::fs::write(app.workspace.root.join("files/.env"), "PRIVATE_TEST_VALUE").unwrap();
    assert_eq!(
        call(&app, "read_file", json!({"path":"files/.env"}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(&app, "read_multiple_files", json!({"paths":["files/.env"]}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let tree = ok(call(&app, "file_tree", json!({"path":"files"})).await);
    assert!(!tree.to_string().contains(".env"));
    std::fs::write(app.workspace.root.join("files/empty.txt"), "").unwrap();
    ok(call(&app, "read_file", json!({"path":"files/empty.txt"})).await);

    let numbered = (0..20_000)
        .map(|n| format!("line{n:05}\n"))
        .collect::<String>();
    std::fs::write(app.workspace.root.join("files/many-lines.txt"), &numbered).unwrap();
    let page = call(
        &app,
        "read_file",
        json!({"path":"files/many-lines.txt","max_bytes":40_000}),
    )
    .await;
    assert_eq!(page.status, "ok");
    let output = page.output.unwrap();
    assert!(output.len() < 60_000);
    let displayed = output
        .lines()
        .filter_map(|line| {
            line.split_once('\t')
                .map(|(_, content)| format!("{content}\n"))
        })
        .collect::<String>();
    let offset = page.data.unwrap()["next_byte"].as_u64().unwrap() as usize;
    assert_eq!(
        displayed,
        numbered[..offset],
        "continuation must follow the last displayed byte, including line-number overhead"
    );
}
