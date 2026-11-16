use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    tools,
    wire::ToolResultMessage,
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
#[tokio::test]
async fn status_and_diff_filter_private_files_and_never_modify_the_index() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/allowed.txt"), "after\n").unwrap();
    std::fs::write(root.join(".env"), "PRIVATE_VALUE=never return this\n").unwrap();
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let state = call(&app, &sid, "git_status", json!({})).await;
    assert_eq!(state.status, "ok");
    let encoded = serde_json::to_string(&state).unwrap();
    assert!(encoded.contains("files/allowed.txt"));
    assert!(!encoded.contains(".env"));
    let diff = call(&app, &sid, "git_diff", json!({})).await;
    assert_eq!(diff.status, "ok", "{diff:?}");
    let output = diff.output.unwrap();
    assert!(output.contains("+after"));
    assert!(!output.contains("PRIVATE_VALUE"));
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    let denied = call(&app, &sid, "git_diff", json!({"files":[".env"]})).await;
    assert_eq!(denied.code.as_deref(), Some("permission_denied"));
    assert_eq!(
        call(&app, &sid, "git_diff", json!({"files":["../outside"]}))
            .await
            .status,
        "error"
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn staged_selection_preserves_unrelated_changes_and_literal_filenames() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/allowed.txt"), "staged\n").unwrap();
    std::fs::write(root.join("files/other.txt"), "unrelated\n").unwrap();
    std::fs::write(root.join("files/[literal].txt"), "literal\n").unwrap();
    git(
        root,
        &["add", "--", "files/allowed.txt", "files/[literal].txt"],
    );
    let diff = call(
        &app,
        &sid,
        "git_diff",
        json!({"staged":true,"files":["files/[literal].txt"]}),
    )
    .await;
    assert_eq!(diff.status, "ok", "{diff:?}");
    let text = diff.output.unwrap();
    assert!(text.contains("[literal].txt"));
    assert!(!text.contains("allowed.txt"));
    assert!(!text.contains("other.txt"));
    assert_eq!(
        std::fs::read_to_string(root.join("files/other.txt")).unwrap(),
        "unrelated\n"
    );
}
#[tokio::test]
async fn repositories_are_explicit_no_parent_search_worktree_pointers_or_alternates() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    init(t.path());
    assert_eq!(
        call(&app, &sid, "git_status", json!({}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    std::fs::write(
        app.workspace.root.join(".git"),
        format!("gitdir: {}\n", t.path().join(".git").display()),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "git_status", json!({})).await.status,
        "error"
    );
    std::fs::remove_file(app.workspace.root.join(".git")).unwrap();
    seed(&app);
    std::fs::write(
        app.workspace.root.join(".git/objects/info/alternates"),
        t.path().join(".git/objects").to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "git_status", json!({}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
}
#[tokio::test]
async fn inspection_does_not_execute_filters_fsmonitor_hooks_or_external_diff() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    let marker = t.path().join("must-not-exist");
    let trap = format!("/usr/bin/touch {}", marker.display());
    for key in [
        "filter.spy.clean",
        "filter.spy.smudge",
        "filter.spy.process",
        "core.fsmonitor",
        "diff.spy.command",
        "diff.spy.textconv",
    ] {
        git(root, &["config", key, &trap]);
    }
    git(root, &["config", "filter.spy.required", "true"]);
    std::fs::write(
        root.join(".gitattributes"),
        "files/*.txt filter=spy diff=spy\n",
    )
    .unwrap();
    std::fs::write(
        root.join("files/allowed.txt"),
        "changed without external code\n",
    )
    .unwrap();
    let result = call(&app, &sid, "git_diff", json!({})).await;
    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result
        .output
        .unwrap()
        .contains("changed without external code"));
    assert!(!marker.exists());
}
#[tokio::test]
async fn config_includes_metadata_symlinks_and_readme_escapes_are_refused() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    std::fs::write(t.path().join("private.txt"), "NEVER RETURN THIS").unwrap();
    std::os::unix::fs::symlink(t.path().join("private.txt"), root.join("README.md")).unwrap();
    let inspected = call(&app, &sid, "inspect_repository", json!({})).await;
    assert_eq!(inspected.status, "ok");
    assert!(!serde_json::to_string(&inspected)
        .unwrap()
        .contains("NEVER RETURN"));
    git(
        root,
        &[
            "config",
            "include.path",
            t.path().join("outside-config").to_str().unwrap(),
        ],
    );
    assert_eq!(
        call(&app, &sid, "git_status", json!({}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    git(root, &["config", "--unset", "include.path"]);
    std::os::unix::fs::symlink(t.path().join("private.txt"), root.join(".git/info/link")).unwrap();
    assert_eq!(
        call(&app, &sid, "git_status", json!({}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(&app, &sid, "git_status", json!({"agent":"fake"}))
            .await
            .code
            .as_deref(),
        Some("invalid_arguments")
    );
}
#[tokio::test]
async fn oversized_diffs_are_errors_instead_of_partial_reviews() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    std::fs::write(
        app.workspace.root.join("files/allowed.txt"),
        "changed line\n".repeat(10000),
    )
    .unwrap();
    let result = call(&app, &sid, "git_diff", json!({})).await;
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().contains("exceeded"));
}
#[tokio::test]
async fn long_filename_metadata_is_bounded_without_claiming_a_complete_review() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    seed(&app);
    let root = &app.workspace.root;
    let deep = root
        .join("files")
        .join("a".repeat(200))
        .join("b".repeat(200));
    std::fs::create_dir_all(&deep).unwrap();
    for i in 0..90 {
        std::fs::write(
            deep.join(format!("{i:03}{}.txt", "c".repeat(190))),
            "content\n",
        )
        .unwrap();
        std::fs::write(
            root.join(format!("{i:03}{}.txt", "d".repeat(240))),
            "content\n",
        )
        .unwrap();
    }
    git(root, &["add", "--", "."]);
    let status = call(&app, &sid, "git_status", json!({})).await;
    assert_eq!(status.status, "ok", "{status:?}");
    let data = status.data.unwrap();
    assert_eq!(data["truncated"], true);
    assert!(serde_json::to_vec(&data["entries"]).unwrap().len() < 24500);
    let inspected = call(&app, &sid, "inspect_repository", json!({})).await;
    assert_eq!(inspected.status, "ok", "{inspected:?}");
    assert!(
        serde_json::to_vec(&inspected.data.unwrap()["entries"])
            .unwrap()
            .len()
            < 24500
    );
    let selection: Vec<_> = (0..40)
        .map(|i| {
            deep.join(format!("{i:03}{}.txt", "c".repeat(190)))
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let diff = call(
        &app,
        &sid,
        "git_diff",
        json!({"staged":true,"files":selection}),
    )
    .await;
    assert_eq!(diff.status, "error");
    assert!(diff.error.unwrap().contains("metadata exceeds"));
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
