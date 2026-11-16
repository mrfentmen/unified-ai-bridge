use bridge_core::provider::Agent;
use bridge_core::workspace::{PermissionMode, Profile};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    tools,
    workspace::Workspace,
};
use serde_json::json;
use std::sync::Arc;

fn app(temp: &tempfile::TempDir) -> Arc<App> {
    let base = temp.path().join("home");
    let root = temp.path().join("research");
    let home = HomePaths {
        base: base.clone(),
        config_file: base.join("config.toml"),
        database: base.join("bridge.sqlite3"),
        logs_dir: base.join("logs"),
        default_project: root.clone(),
        native_hosts_dir: base.join("hosts"),
        install_dir: base.join("bin"),
    };
    let (workspace, _) = Workspace::initialize(&root).unwrap();
    let mut config = Config::default();
    config.project_root = workspace.root.clone();
    home.save_config(&config).unwrap();
    App::open_with_config(home, config, workspace, true).unwrap()
}

#[tokio::test]
async fn detected_command_presets_are_suggestions_and_revoked_pending_grants_never_run() {
    let temp = tempfile::tempdir().unwrap();
    let research = app(&temp);
    let workspace = research
        .create_workspace(
            "Code",
            temp.path().join("code").to_str().unwrap(),
            Profile::Code,
        )
        .unwrap();
    let app = research.select_workspace(&workspace.id).unwrap();
    std::fs::create_dir(app.workspace.root.join(".git")).unwrap();
    let suggestions = tools::commands::presets(&app);
    let git = suggestions
        .iter()
        .find(|c| c.command_id == "git-status")
        .expect("git executable available on test host");
    assert!(!git.always_allow);
    assert!(app
        .config
        .workspace_commands
        .get(&app.workspace.id())
        .is_none());
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::ChatGpt,
            "test",
            "run_command",
            json!({"command_id":"git-status"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let app = app.grant_command(git.clone()).unwrap();
    let mut events = app.events.subscribe();
    let pending = app.clone();
    let request = tokio::spawn(async move {
        tools::dispatch(
            &pending,
            Agent::ChatGpt,
            "test",
            "run_command",
            json!({"command_id":"git-status"}),
        )
        .await
    });
    let approval = loop {
        if let bridge_native::wire::BridgeEvent::ApprovalRequired { approval } =
            events.recv().await.unwrap()
        {
            break approval;
        }
    };
    let _revoked = app.revoke_command("git-status").unwrap();
    app.approvals.resolve(&approval.id, true);
    let result = request.await.unwrap();
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert!(result.error.unwrap().contains("grant changed"));
    assert!(app.store.verify_audit().unwrap().valid);
    let audit = std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    assert!(
        !audit.contains("command.start"),
        "revoked command must never launch"
    );
}

#[test]
fn committed_switch_survives_activity_feed_failure() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let general = old
        .create_workspace(
            "Scratch",
            temp.path().join("scratch").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let conn = rusqlite::Connection::open(&old.home.database).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_feed BEFORE INSERT ON activity BEGIN SELECT RAISE(FAIL, 'feed unavailable'); END;").unwrap();
    let active = old.select_workspace(&general.id).unwrap();
    assert_eq!(active.workspace.id(), general.id);
    assert_eq!(
        active.store.active_workspace_id().unwrap().as_deref(),
        Some(general.id.as_str())
    );
    assert!(active.store.verify_audit().unwrap().valid);
    assert_eq!(
        App::load(active.home.clone(), false)
            .unwrap()
            .workspace
            .id(),
        general.id
    );
}

#[test]
fn migration_preserves_legacy_audit_and_rejects_corruption() {
    use bridge_native::store::{AuditInput, Store};
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("db.sqlite3");
    let legacy_path = temp.path().join("legacy.jsonl");
    let destination = temp.path().join("home-audit.jsonl");
    let locks = temp.path().join("locks");
    let legacy = Store::open(&database, &legacy_path, &locks).unwrap();
    legacy
        .append_audit(AuditInput {
            agent: "USER".into(),
            session_id: "test".into(),
            action: "test".into(),
            path: None,
            decision: "allowed".into(),
            detail: "migration test".into(),
        })
        .unwrap();
    let original = std::fs::read(&legacy_path).unwrap();
    std::fs::write(&legacy_path, b"corrupt\n").unwrap();
    assert!(Store::open_migrating(&database, &destination, &legacy_path, &locks).is_err());
    assert!(!destination.exists());
    std::fs::write(&legacy_path, &original).unwrap();
    let migrated = Store::open_migrating(&database, &destination, &legacy_path, &locks).unwrap();
    assert!(migrated.verify_audit().unwrap().valid);
    assert_eq!(std::fs::read(&destination).unwrap(), original);
    assert_eq!(std::fs::read(&legacy_path).unwrap(), original);
    let reopened = Store::open_migrating(&database, &destination, &legacy_path, &locks).unwrap();
    assert!(reopened.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn switching_preserves_inflight_root_and_general_edits_real_code() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let old_root = old.workspace.root.clone();
    let original = old_root.join("PROJECT/PROJECT_CONTEXT.md");
    let original_bytes = std::fs::read(&original).unwrap();
    let root = temp.path().join("ordinary-folder");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("existing.rs"), "fn main() {}\n").unwrap();
    let general = old
        .create_workspace("Ordinary folder", root.to_str().unwrap(), Profile::General)
        .unwrap();
    let mut events = old.events.subscribe();
    let running = old.clone();
    let write = tokio::spawn(async move {
        tools::dispatch(
            &running,
            Agent::ChatGpt,
            "original-session",
            "write_document",
            json!({"path":"PROJECT/inflight.md","content":"old workspace\n"}),
        )
        .await
    });
    let pending = loop {
        if let bridge_native::wire::BridgeEvent::ApprovalRequired { approval } =
            events.recv().await.unwrap()
        {
            break approval;
        }
    };
    let active = old.select_workspace(&general.id).unwrap();
    assert!(active.approvals.resolve(&pending.id, true));
    assert_eq!(write.await.unwrap().status, "ok");
    assert_eq!(
        std::fs::read_to_string(old_root.join("PROJECT/inflight.md")).unwrap(),
        "old workspace\n"
    );
    assert!(!root.join("PROJECT/inflight.md").exists());
    let changed = tools::dispatch(&active, Agent::ChatGpt, "updated-session", "edit_document",
        json!({"path":"existing.rs","expected_sha256":bridge_native::fsops::sha256_bytes(b"fn main() {}\n"),"old_text":"fn main() {}","new_text":"fn main() { println!(\"hello\"); }"})).await;
    assert_eq!(changed.status, "ok", "{changed:?}");
    let activity = active.store.recent_activity(1).unwrap().remove(0);
    let stored = active.store.activity_result(activity.id).unwrap();
    assert_eq!(stored["tool"], "edit_document");
    assert_eq!(stored["status"], "ok");
    assert_eq!(
        stored["data"]["workspace_root"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(changed.data.unwrap()["checkpoint"].as_str().is_some());
    assert!(std::fs::read_to_string(root.join("existing.rs"))
        .unwrap()
        .contains("println!"));
    let absolute = root.canonicalize().unwrap().join("files/new.py");
    let created = tools::dispatch(
        &active,
        Agent::Claude,
        "updated-session",
        "write_document",
        json!({"path":absolute,"content":"print('real code file')\n","create_only":true}),
    )
    .await;
    assert_eq!(created.status, "ok", "{created:?}");
    assert!(absolute.is_file());
    let searched = tools::dispatch(
        &active,
        Agent::Claude,
        "updated-session",
        "search_files",
        json!({"query":"real code file"}),
    )
    .await;
    assert_eq!(searched.status, "ok", "{searched:?}");
    assert!(searched.output.unwrap().contains("files/new.py"));
    let denied = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "updated-session",
        "write_document",
        json!({"path":old_root.join("PROJECT/escape.md"),"content":"must be denied"}),
    )
    .await;
    assert_eq!(denied.code.as_deref(), Some("permission_denied"));
    assert!(!old_root.join("PROJECT/escape.md").exists());
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    assert!(!active
        .instructions(Agent::ChatGpt, "ChatGPT")
        .contains("submit_peer_review"));
    let unavailable = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "updated-session",
        "list_agents",
        json!({}),
    )
    .await;
    assert_eq!(unavailable.code.as_deref(), Some("permission_denied"));
    assert!(active.store.verify_audit().unwrap().valid);
    let audit = std::fs::read_to_string(active.home.base.join("audit.jsonl")).unwrap();
    assert!(audit.contains("workspace.switch"));
    let reloaded = App::load(active.home.clone(), false).unwrap();
    assert_eq!(reloaded.workspace.id(), general.id);
    println!("workspace switch: old in-flight write completed; new code edited with checkpoint; outside-root write denied; audit valid; restart restored {}", general.id);
}

#[tokio::test]
async fn profiles_are_additive_and_locks_and_strict_mode_deny_writes() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let root = temp.path().join("code");
    let code = old
        .create_workspace("Code", root.to_str().unwrap(), Profile::Code)
        .unwrap();
    assert!(root.join("src").is_dir());
    assert!(old
        .create_workspace("Mismatch", root.to_str().unwrap(), Profile::General)
        .is_err());
    assert!(!root.join("notes").exists());
    let active = old.select_workspace(&code.id).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/config"), "protected\n").unwrap();
    for path in [".git/config", ".bridge/workspace.json", "../outside.rs"] {
        let result = tools::dispatch(
            &active,
            Agent::ChatGpt,
            "s",
            "write_document",
            json!({"path":path,"content":"not allowed"}),
        )
        .await;
        assert_eq!(
            result.code.as_deref(),
            Some("permission_denied"),
            "{result:?}"
        );
    }
    let mut workspace = active.workspace.clone();
    workspace.read_only = true;
    let locked = active.with_workspace(workspace);
    assert_eq!(
        tools::dispatch(
            &locked,
            Agent::ChatGpt,
            "s",
            "write_document",
            json!({"path":"src/no.rs","content":"no"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let mut workspace = active.workspace.clone();
    workspace.permission_mode = PermissionMode::Strict;
    let strict = active.with_workspace(workspace);
    let mut events = strict.events.subscribe();
    let cloned = strict.clone();
    let call = tokio::spawn(async move {
        tools::dispatch(
            &cloned,
            Agent::ChatGpt,
            "s",
            "write_document",
            json!({"path":"src/denied.rs","content":"no"}),
        )
        .await
    });
    loop {
        if let bridge_native::wire::BridgeEvent::ApprovalRequired { approval } =
            events.recv().await.unwrap()
        {
            strict.approvals.resolve(&approval.id, false);
            break;
        }
    }
    assert_eq!(call.await.unwrap().code.as_deref(), Some("approval_denied"));
    assert!(!root.join("src/denied.rs").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path(), root.join("escape-link")).unwrap();
        assert!(active
            .sandbox()
            .resolve("escape-link/other", bridge_native::sandbox::Existence::Any)
            .is_err());
    }
    let before = active.workspace.id();
    assert!(active.select_workspace("does-not-exist").is_err());
    assert_eq!(
        active.store.active_workspace_id().unwrap().as_deref(),
        Some(before.as_str())
    );
}

fn grant(id: &str, program: &str, args: &[&str]) -> bridge_native::config::AllowedCommand {
    bridge_native::config::AllowedCommand {
        command_id: id.into(),
        program: program.into(),
        fixed_args: args.iter().map(|value| (*value).into()).collect(),
        allowed_args: Vec::new(),
        cwd: ".".into(),
        timeout_secs: 5,
        max_output_bytes: 256,
        writes: false,
        always_allow: false,
    }
}

#[tokio::test]
async fn commands_require_workspace_grant_and_user_approval() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let general = old
        .create_workspace(
            "Commands",
            temp.path().join("commands").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let active = old.select_workspace(&general.id).unwrap();
    let active = active
        .grant_command(grant("print", "/usr/bin/printf", &["approved command\\n"]))
        .unwrap();
    let list = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "list_allowed_commands",
        json!({}),
    )
    .await;
    assert_eq!(list.status, "ok");
    assert_eq!(list.data.unwrap()["commands"][0]["command_id"], "print");
    let bad_list = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "list_allowed_commands",
        json!({"path":"../escape"}),
    )
    .await;
    assert_eq!(bad_list.code.as_deref(), Some("invalid_arguments"));
    for request in [
        json!({"command_id":"not-granted"}),
        json!({"command_id":"print","args":["; touch /tmp/escape"]}),
    ] {
        let denied = tools::dispatch(&active, Agent::ChatGpt, "s", "run_command", request).await;
        assert_eq!(denied.code.as_deref(), Some("permission_denied"));
    }
    for allow in [false, true] {
        let mut events = active.events.subscribe();
        let running = active.clone();
        let task = tokio::spawn(async move {
            tools::dispatch(
                &running,
                Agent::ChatGpt,
                "s",
                "run_command",
                json!({"command_id":"print"}),
            )
            .await
        });
        loop {
            if let bridge_native::wire::BridgeEvent::ApprovalRequired { approval } =
                events.recv().await.unwrap()
            {
                active.approvals.resolve(&approval.id, allow);
                break;
            }
        }
        let result = task.await.unwrap();
        if allow {
            assert_eq!(result.status, "ok", "{result:?}");
            assert_eq!(result.data.unwrap()["stdout"], "approved command\n");
        } else {
            assert_eq!(result.code.as_deref(), Some("approval_denied"));
        }
    }
    let other = active
        .create_workspace(
            "Other",
            temp.path().join("other").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let switched = active.select_workspace(&other.id).unwrap();
    assert_eq!(
        tools::dispatch(
            &switched,
            Agent::ChatGpt,
            "s",
            "run_command",
            json!({"command_id":"print"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert!(active.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn command_output_is_bounded_and_exit_and_timeout_are_results() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let general = old
        .create_workspace(
            "Commands",
            temp.path().join("commands").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let mut active = old.select_workspace(&general.id).unwrap();
    let large = "é".repeat(10000);
    let false_program = if std::path::Path::new("/usr/bin/false").exists() {
        "/usr/bin/false"
    } else {
        "/bin/false"
    };
    let sleep_program = if std::path::Path::new("/bin/sleep").exists() {
        "/bin/sleep"
    } else {
        "/usr/bin/sleep"
    };
    for mut entry in [
        grant("large", "/usr/bin/printf", &["%s", &large]),
        grant("failure", false_program, &[]),
        grant("slow", sleep_program, &["5"]),
    ] {
        if entry.command_id == "large" {
            entry.fixed_args = vec![
                "%s%s%s".into(),
                "é".repeat(2000),
                "x".repeat(2000),
                "x".repeat(2000),
            ];
        }
        entry.always_allow = true;
        entry.timeout_secs = 1;
        active = active.grant_command(entry).unwrap();
    }
    let large = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "run_command",
        json!({"command_id":"large"}),
    )
    .await;
    assert_eq!(large.status, "ok", "{large:?}");
    assert!(large.output.unwrap().contains("output truncated"));
    let data = large.data.unwrap();
    assert_eq!(data["truncated"], true);
    assert!(data["stdout"].as_str().unwrap().len() + data["stderr"].as_str().unwrap().len() <= 256);
    let failed = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "run_command",
        json!({"command_id":"failure"}),
    )
    .await;
    assert_eq!(failed.status, "error");
    assert_eq!(failed.code.as_deref(), Some("command_exit"));
    assert_ne!(failed.data.unwrap()["exit_code"], 0);
    let start = std::time::Instant::now();
    let timed = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "run_command",
        json!({"command_id":"slow"}),
    )
    .await;
    assert_eq!(timed.code.as_deref(), Some("command_timeout"));
    assert!(start.elapsed().as_secs() < 3);
    let revoked = active.revoke_command("large").unwrap();
    assert_eq!(
        tools::dispatch(
            &revoked,
            Agent::ChatGpt,
            "s",
            "run_command",
            json!({"command_id":"large"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert!(active.store.verify_audit().unwrap().valid);
}

#[test]
fn shell_and_traversing_command_grants_are_rejected() {
    assert!(grant("shell", "/bin/sh", &["-c", "echo prohibited"])
        .validate()
        .is_err());
    let mut bad = grant("traverse", "/usr/bin/printf", &["no"]);
    bad.cwd = "../outside".into();
    assert!(bad.validate().is_err());
    bad.cwd = ".".into();
    bad.max_output_bytes = usize::MAX;
    assert!(bad.validate().is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn approved_command_keeps_proxy_invocation_name() {
    let temp = tempfile::tempdir().unwrap();
    let old = app(&temp);
    let general = old
        .create_workspace(
            "Proxy",
            temp.path().join("proxy-root").to_str().unwrap(),
            Profile::General,
        )
        .unwrap();
    let active = old.select_workspace(&general.id).unwrap();
    let source = temp.path().join("proxy.rs");
    let binary = temp.path().join("multi-call-program");
    std::fs::write(&source,r#"fn main() { let name=std::env::args().next().unwrap(); if !name.ends_with("approved-proxy") { std::process::exit(7); } println!("proxy invocation preserved"); }"#).unwrap();
    let compiled =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap();
    assert!(compiled.success());
    let alias = temp.path().join("approved-proxy");
    std::os::unix::fs::symlink(&binary, &alias).unwrap();
    let mut entry = grant("proxy", alias.to_str().unwrap(), &[]);
    entry.always_allow = true;
    let active = active.grant_command(entry).unwrap();
    let result = tools::dispatch(
        &active,
        Agent::ChatGpt,
        "s",
        "run_command",
        json!({"command_id":"proxy"}),
    )
    .await;
    assert_eq!(result.status, "ok", "{result:?}");
    assert_eq!(
        result.data.unwrap()["stdout"],
        "proxy invocation preserved\n"
    );
}
