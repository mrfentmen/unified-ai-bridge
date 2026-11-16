use bridge_core::{
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
use std::{path::Path, process::Command, sync::Arc};
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
}
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn init(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(root, &["config", "user.name", "Bridge verification"]);
    git(
        root,
        &["config", "user.email", "bridge-test@example.invalid"],
    );
}
fn seed(app: &App) {
    let root = &app.workspace.root;
    init(root);
    std::fs::write(root.join("files/allowed.txt"), "before\n").unwrap();
    std::fs::write(root.join(".env"), "PRIVATE_VALUE=original\n").unwrap();
    git(root, &["add", "--", "files/allowed.txt", ".env"]);
    git(root, &["commit", "-qm", "baseline"]);
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
async fn preview(app: &App, sid: &str, branch: &str) -> Value {
    ok(call(
        app,
        sid,
        "preview_git_commit",
        json!({"files":["files/allowed.txt"],"branch":branch,"message":"Reviewed local change"}),
    )
    .await)
}
async fn decide(
    app: &Arc<App>,
    sid: &str,
    id: &Value,
    allow: bool,
    change: impl FnOnce(),
) -> ToolResultMessage {
    let mut events = app.events.subscribe();
    let a = app.clone();
    let sid = sid.to_owned();
    let id = id.clone();
    let job =
        tokio::spawn(
            async move { call(&a, &sid, "apply_git_commit", json!({"review_id":id})).await },
        );
    loop {
        if let BridgeEvent::ApprovalRequired { approval } =
            tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            assert!(
                approval.reason.contains("Current") || approval.reason.contains("current branch")
            );
            change();
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    job.await.unwrap()
}
fn out(root: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
#[tokio::test]
async fn approved_working_files_create_a_new_branch_preserving_head_index_and_unrelated_edits() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/allowed.txt"), "staged content\n").unwrap();
    git(root, &["add", "--", "files/allowed.txt"]);
    std::fs::write(root.join("files/allowed.txt"), "approved working bytes\n").unwrap();
    std::fs::write(root.join("files/unrelated.txt"), "user draft\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let head = out(root, &["rev-parse", "HEAD"]);
    let p = preview(&app, &sid, "bridge/reviewed").await;
    assert_eq!(p["author"], "Unified AI Bridge <bridge@localhost>");
    let applied = ok(decide(&app, &sid, &p["review_id"], true, || {}).await);
    assert_eq!(applied["state"], "committed");
    assert_eq!(
        out(root, &["show", "bridge/reviewed:files/allowed.txt"]),
        "approved working bytes"
    );
    assert_eq!(out(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(out(root, &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        std::fs::read_to_string(root.join("files/unrelated.txt")).unwrap(),
        "user draft\n"
    );
    assert_eq!(
        out(
            root,
            &[
                "diff-tree",
                "--no-commit-id",
                "--name-only",
                "-r",
                "bridge/reviewed"
            ]
        ),
        "files/allowed.txt"
    );
    let replay = ok(call(
        &app,
        &sid,
        "apply_git_commit",
        json!({"review_id":p["review_id"]}),
    )
    .await);
    assert_eq!(replay["commit"], applied["commit"]);
    let mut saved = app
        .store
        .git_review(p["review_id"].as_str().unwrap(), &app.workspace.id())
        .unwrap()
        .unwrap();
    saved.state = "publishing".into();
    app.store.save_git_review(&saved).unwrap();
    let reopened = App::load(app.home.clone(), true).unwrap();
    assert_eq!(
        ok(call(
            &reopened,
            &sid,
            "git_commit_status",
            json!({"review_id":p["review_id"]})
        )
        .await)["state"],
        "committed"
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn denial_and_changed_file_head_branch_or_policy_during_approval_never_publish() {
    for case in ["deny", "file", "head", "branch", "lock", "symlink"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        seed(&app);
        let root = &app.workspace.root;
        std::fs::write(root.join("files/allowed.txt"), "approved\n").unwrap();
        let p = preview(&app, &sid, "bridge/rejected").await;
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let result = decide(&app, &sid, &p["review_id"], case != "deny", || match case {
            "file" => std::fs::write(root.join("files/allowed.txt"), "later edit\n").unwrap(),
            "head" => git(
                root,
                &["commit", "--allow-empty", "-qm", "Independent commit"],
            ),
            "branch" => git(root, &["branch", "bridge/rejected"]),
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
            "symlink" => {
                std::fs::remove_file(root.join("files/allowed.txt")).unwrap();
                std::os::unix::fs::symlink(
                    t.path().join("outside"),
                    root.join("files/allowed.txt"),
                )
                .unwrap();
            }
            _ => {}
        })
        .await;
        assert_eq!(result.status, "error", "{case}: {result:?}");
        if case != "branch" {
            assert!(!root.join(".git/refs/heads/bridge/rejected").exists());
        }
        if case != "head" {
            assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        }
    }
}
#[tokio::test]
async fn commit_hooks_filters_signing_and_external_diff_are_never_executed() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    let marker = t.path().join("must-not-exist");
    let trap = format!("/usr/bin/touch {}", marker.display());
    for key in [
        "filter.spy.clean",
        "filter.spy.process",
        "diff.spy.command",
        "diff.spy.textconv",
        "core.fsmonitor",
        "gpg.program",
    ] {
        git(root, &["config", key, &trap]);
    }
    git(root, &["config", "commit.gpgSign", "true"]);
    std::fs::write(root.join(".gitattributes"), "*.txt filter=spy diff=spy\n").unwrap();
    let hook = root.join(".git/hooks/reference-transaction");
    std::fs::write(&hook, format!("#!/bin/sh\n{trap}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        root.join("files/allowed.txt"),
        "approved without external commands\n",
    )
    .unwrap();
    let p = preview(&app, &sid, "bridge/no-hooks").await;
    ok(decide(&app, &sid, &p["review_id"], true, || {}).await);
    assert!(!marker.exists());
}
#[tokio::test]
async fn private_binary_unknown_fields_wrong_owner_and_expired_previews_are_denied() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    for files in [
        json!([".env"]),
        json!(["../outside"]),
        json!([".git/config"]),
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "preview_git_commit",
                json!({"files":files,"branch":"bridge/no","message":"x"})
            )
            .await
            .status,
            "error"
        );
    }
    std::fs::write(root.join("files/allowed.txt"), b"binary\0value").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "preview_git_commit",
            json!({"files":["files/allowed.txt"],"branch":"bridge/no","message":"x"})
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
    std::fs::write(root.join("files/allowed.txt"), "changed\n").unwrap();
    let p = preview(&app, &sid, "bridge/expire").await;
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
    for tool in ["apply_git_commit", "git_commit_status"] {
        assert_eq!(
            call(&app, &other, tool, json!({"review_id":p["review_id"]}))
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
                json!({"review_id":p["review_id"],"provider":"fake"})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
    let mut r = app
        .store
        .git_review(p["review_id"].as_str().unwrap(), &app.workspace.id())
        .unwrap()
        .unwrap();
    r.expires_at = 0;
    app.store.save_git_review(&r).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "apply_git_commit",
            json!({"review_id":p["review_id"]})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(app.approvals.pending_count(), 0);
}
#[tokio::test]
async fn interrupted_commit_is_reported_and_never_automatically_repeated() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    std::fs::write(app.workspace.root.join("files/allowed.txt"), "changed\n").unwrap();
    let p = preview(&app, &sid, "bridge/interrupted").await;
    let mut r = app
        .store
        .git_review(p["review_id"].as_str().unwrap(), &app.workspace.id())
        .unwrap()
        .unwrap();
    r.state = "preparing".into();
    app.store.save_git_review(&r).unwrap();
    let state = ok(call(
        &app,
        &sid,
        "git_commit_status",
        json!({"review_id":p["review_id"]}),
    )
    .await);
    assert_eq!(state["state"], "needs_reconciliation");
    assert_eq!(
        call(
            &app,
            &sid,
            "apply_git_commit",
            json!({"review_id":p["review_id"]})
        )
        .await
        .status,
        "error"
    );
    assert!(!app
        .workspace
        .root
        .join(".git/refs/heads/bridge/interrupted")
        .exists());
}

#[tokio::test]
async fn history_pages_and_selected_historical_files_preserve_private_path_policy() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/allowed.txt"), "second version\n").unwrap();
    git(root, &["add", "files/allowed.txt"]);
    git(root, &["commit", "-qm", "Second version"]);
    let first = ok(call(&app, &sid, "git_log", json!({"limit":1})).await);
    assert_eq!(first["commits"][0]["subject"], "Second version");
    assert_eq!(first["next_offset"], 1);
    let page = ok(call(
        &app,
        &sid,
        "git_log",
        json!({"commit":first["base_commit"],"offset":1,"limit":1}),
    )
    .await);
    assert_eq!(page["commits"][0]["subject"], "baseline");
    assert!(page["next_offset"].is_null());
    let id = page["commits"][0]["commit"].clone();
    let shown = call(
        &app,
        &sid,
        "git_show",
        json!({"commit":id,"file":"files/allowed.txt"}),
    )
    .await;
    assert_eq!(shown.status, "ok");
    assert_eq!(shown.output.as_deref(), Some("before\n"));
    for path in [".env", ".git/config", "../outside"] {
        assert_eq!(
            call(&app, &sid, "git_show", json!({"commit":id,"file":path}))
                .await
                .code
                .as_deref(),
            Some("permission_denied")
        );
    }
    for id in ["HEAD", "HEAD:path", "--help"] {
        assert_eq!(
            call(
                &app,
                &sid,
                "git_show",
                json!({"commit":id,"file":"files/allowed.txt"})
            )
            .await
            .status,
            "error"
        );
    }
    assert_eq!(
        call(&app, &sid, "git_log", json!({"limit":100}))
            .await
            .code
            .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(
        call(&app, &sid, "git_log", json!({"provider":"fake"}))
            .await
            .code
            .as_deref(),
        Some("invalid_arguments")
    );
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn failed_ref_publication_preserves_existing_branch_and_is_not_retried() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    git(root, &["branch", "bridge"]);
    let head = out(root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("files/allowed.txt"), "candidate\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let p = preview(&app, &sid, "bridge/blocked").await;
    let applied = decide(&app, &sid, &p["review_id"], true, || {}).await;
    assert_eq!(applied.status, "error");
    assert!(applied.error.unwrap().contains("Git inspection failed"));
    let status = ok(call(
        &app,
        &sid,
        "git_commit_status",
        json!({"review_id":p["review_id"]}),
    )
    .await);
    assert_eq!(status["state"], "needs_reconciliation");
    assert_eq!(out(root, &["rev-parse", "bridge"]), head);
    assert_eq!(out(root, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        call(
            &app,
            &sid,
            "apply_git_commit",
            json!({"review_id":p["review_id"]})
        )
        .await
        .status,
        "error"
    );
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn research_requires_own_repository_and_selected_file_checks_remain_mandatory() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    let root = &app.workspace.root;
    init(root);
    std::fs::write(root.join("AGENTS/CHATGPT/notes/a.md"), "before\n").unwrap();
    git(root, &["add", "AGENTS/CHATGPT/notes/a.md"]);
    git(root, &["commit", "-qm", "baseline"]);
    assert_eq!(call(&app,&sid,"preview_git_commit",json!({"path":".","files":["AGENTS/CHATGPT/notes/a.md"],"branch":"bridge/no","message":"x"})).await.code.as_deref(),Some("permission_denied"));
    let own = root.join("AGENTS/CHATGPT");
    init(&own);
    git(&own, &["add", "notes/a.md"]);
    git(&own, &["commit", "-qm", "baseline"]);
    std::fs::write(own.join("notes/a.md"), "after\n").unwrap();
    let p=ok(call(&app,&sid,"preview_git_commit",json!({"files":["AGENTS/CHATGPT/notes/a.md"],"branch":"bridge/own","message":"Reviewed own notes"})).await);
    ok(decide(&app, &sid, &p["review_id"], true, || {}).await);
    assert_eq!(out(&own, &["show", "bridge/own:notes/a.md"]), "after");
    for path in ["AGENTS/CLAUDE/notes/a.md", "SHARED/datasets/a.csv"] {
        std::fs::write(root.join(path), "protected\n").unwrap();
        assert_eq!(
            call(
                &app,
                &sid,
                "preview_git_commit",
                json!({"files":[path],"branch":"bridge/no","message":"x"})
            )
            .await
            .code
            .as_deref(),
            Some("permission_denied")
        );
    }
}

#[tokio::test]
async fn nested_commit_cannot_remove_an_unselected_parent_file() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::remove_file(root.join("files/allowed.txt")).unwrap();
    std::fs::create_dir(root.join("files/allowed.txt")).unwrap();
    std::fs::write(root.join("files/allowed.txt/new.txt"), "child\n").unwrap();
    let result=call(&app,&sid,"preview_git_commit",json!({"files":["files/allowed.txt/new.txt"],"branch":"bridge/no-parent-replace","message":"x"})).await;
    assert_eq!(result.code.as_deref(), Some("unsupported"));
    assert!(result.error.unwrap().contains("parent file"));
    assert!(!root
        .join(".git/refs/heads/bridge/no-parent-replace")
        .exists());
}

#[tokio::test]
async fn dangling_symbolic_branch_is_never_followed_or_replaced() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/allowed.txt"), "candidate\n").unwrap();
    let branch = root.join(".git/refs/heads/bridge/dangling");
    std::fs::create_dir_all(branch.parent().unwrap()).unwrap();
    std::fs::write(&branch, "ref: refs/heads/unrelated\n").unwrap();
    let result = call(
        &app,
        &sid,
        "preview_git_commit",
        json!({"files":["files/allowed.txt"],"branch":"bridge/dangling","message":"x"}),
    )
    .await;
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().contains("symbolic ref"));
    assert_eq!(
        std::fs::read_to_string(branch).unwrap(),
        "ref: refs/heads/unrelated\n"
    );
    assert!(!root.join(".git/refs/heads/unrelated").exists());
}
