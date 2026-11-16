use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
use bridge_native::{
    app::App,
    config::{AllowedCommand, Config, HomePaths},
    fsops::sha256_bytes,
    tools,
    wire::{BridgeEvent, ToolResultMessage},
    workspace::Workspace,
};
use serde_json::{json, Value};
use std::sync::Arc;
static PARSER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::Code)
}
#[tokio::test]
async fn rust_declarations_have_real_lines_and_skip_comments_strings_and_macro_expansion() {
    let _serial = PARSER_TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let source="// fn imaginary() {}\nconst TEXT: &str = \"fn fake() {}\";\nmod inner {\n pub struct Worker;\n impl Worker { pub fn run(&self) {} }\n pub fn café() {}\n}\nmacro_rules! make { () => { fn generated() {} } }\nmake!();\n";
    std::fs::write(app.workspace.root.join("src/lib.rs"), source).unwrap();
    let first = ok(call(
        &app,
        &sid,
        "list_symbols",
        json!({"path":"src/lib.rs","max_results":2}),
    )
    .await);
    assert_eq!(first["mode"], "syntax_declarations");
    assert_eq!(first["symbols"][0]["name"], "TEXT");
    assert_eq!(first["symbols"][0]["line"], 2);
    let second=ok(call(&app,&sid,"list_symbols",json!({"path":"src/lib.rs","offset":first["next_offset"],"expected_sha256":first["sha256"],"max_results":100})).await);
    let symbols = second["symbols"].as_array().unwrap();
    assert!(symbols.iter().any(|s| s["name"] == "run" && s["line"] == 5));
    assert!(symbols
        .iter()
        .any(|s| s["name"] == "café" && s["line"] == 6));
    assert!(!symbols
        .iter()
        .any(|s| ["fake", "generated", "imaginary"].contains(&s["name"].as_str().unwrap())));
    std::fs::write(app.workspace.root.join("src/lib.rs"), "fn later() {}\n").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "list_symbols",
            json!({"path":"src/lib.rs","offset":2,"expected_sha256":first["sha256"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
}
#[tokio::test]
async fn unsupported_invalid_private_and_nested_sources_fail_without_losing_host() {
    let _serial = PARSER_TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::write(app.workspace.root.join("src/app.ts"), "function main() {}").unwrap();
    assert_eq!(
        call(&app, &sid, "list_symbols", json!({"path":"src/app.ts"}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    for src in [
        "fn unfinished(",
        "fn f(){ ((( }",
        &format!("fn f(){{{}0{}}}", "(".repeat(30000), ")".repeat(30000)),
    ] {
        std::fs::write(app.workspace.root.join("src/bad.rs"), src).unwrap();
        assert_eq!(
            call(&app, &sid, "list_symbols", json!({"path":"src/bad.rs"}))
                .await
                .status,
            "error"
        );
    }
    std::fs::write(app.workspace.root.join(".env"), "fn secret() {}\n").unwrap();
    std::fs::write(
        app.workspace.root.join(".bridge/private.rs"),
        "fn private() {}\n",
    )
    .unwrap();
    for path in ["../outside.rs", ".env", ".bridge/private.rs"] {
        assert_eq!(
            call(&app, &sid, "list_symbols", json!({"path":path}))
                .await
                .status,
            "error"
        );
    }
    std::fs::write(app.workspace.root.join("src/good.rs"), "pub fn ok() {}\n").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "list_symbols",
            json!({"path":"src/good.rs","agent":"CLAUDE"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    ok(call(&app, &sid, "list_symbols", json!({"path":"src/good.rs"})).await);
    std::os::unix::fs::symlink(
        app.workspace.root.join("src/good.rs"),
        app.workspace.root.join("src/link.rs"),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "list_symbols", json!({"path":"src/link.rs"}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
}
#[tokio::test]
async fn search_pages_are_line_numbered_deduplicated_bounded_and_refuse_stale_cursors() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::write(
        app.workspace.root.join("src/a.rs"),
        "fn alpha() {}\n// ALPHA reference\nalpha();\n",
    )
    .unwrap();
    std::fs::write(app.workspace.root.join("src/b.rs"), "alpha();\n").unwrap();
    std::fs::write(app.workspace.root.join(".env"), "alpha SECRET").unwrap();
    let a = ok(call(
        &app,
        &sid,
        "search_files",
        json!({"path":".","query":"alpha","max_results":2}),
    )
    .await);
    assert_eq!(a["matches"].as_array().unwrap().len(), 2);
    assert_eq!(a["matches"][0]["line"], 1);
    assert_eq!(a["matches"][1]["line"], 2);
    let b = ok(call(
        &app,
        &sid,
        "search_files",
        json!({"path":".","query":"alpha","max_results":100,"cursor":a["next_cursor"]}),
    )
    .await);
    assert_eq!(b["matches"].as_array().unwrap().len(), 2);
    assert_eq!(b["matches"][0]["line"], 3);
    assert_eq!(b["matches"][1]["path"], "src/b.rs");
    assert_eq!(b["truncated"], false);
    std::fs::write(app.workspace.root.join("src/a.rs"), "changed").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "search_files",
            json!({"path":".","query":"alpha","cursor":a["next_cursor"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    assert_eq!(call(&app,&sid,"search_files",json!({"path":".","query":"alpha","cursor":{"path":"../outside","line":1,"sha256":null}})).await.status,"error");
    for i in 0..120 {
        std::fs::write(
            app.workspace.root.join(format!("src/file{i:03}.rs")),
            "no match",
        )
        .unwrap();
    }
    let bounded = ok(call(
        &app,
        &sid,
        "search_files",
        json!({"path":"src","query":"absent"}),
    )
    .await);
    assert_eq!(bounded["files_scanned"], 100);
    assert_eq!(bounded["truncated"], true);
    assert!(!bounded["next_cursor"].is_null());
    for i in 0..5010 {
        std::fs::write(
            app.workspace.root.join(format!(".env.{i}")),
            "private alpha",
        )
        .unwrap();
    }
    let excluded = ok(call(
        &app,
        &sid,
        "search_files",
        json!({"path":".","query":"private alpha"}),
    )
    .await);
    assert_eq!(excluded["tree_truncated"], true);
    assert!(excluded["matches"].as_array().unwrap().is_empty());
}
#[tokio::test]
async fn compiler_locations_are_filtered_and_point_to_actual_workspace_lines() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::write(
        app.workspace.root.join("src/main.rs"),
        "fn main(){ let _:u32 = true; }\n",
    )
    .unwrap();
    let ctx = tools::ToolContext::new(&app, Agent::ChatGpt, &sid);
    let source = json!({"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308"},"spans":[{"file_name":"src/main.rs","line_start":1,"column_start":23,"is_primary":true},{"file_name":"../secret","line_start":1,"column_start":1},{"file_name":".env","line_start":1,"column_start":1}]}});
    let result =
        tools::code_navigation::diagnostics(&ctx, &app.workspace.root, &source.to_string());
    assert_eq!(
        result["diagnostics"][0]["locations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        result["diagnostics"][0]["locations"][0]["path"],
        "src/main.rs"
    );
    assert_eq!(result["skipped_spans"], 2);
    assert_eq!(
        tools::code_navigation::diagnostics(&ctx, &app.workspace.root, "plain output")
            ["recognized"],
        0
    );
}
#[tokio::test]
async fn code_task_locates_updates_callers_and_runs_a_real_granted_compiler_check() {
    let _serial = PARSER_TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::create_dir_all(app.workspace.root.join("output")).unwrap();
    let lib = "pub fn value() -> u32 { 7 }\n";
    let main = "mod calc;\nfn main(){ assert_eq!(calc::value(), 7); }\n";
    std::fs::write(app.workspace.root.join("src/calc.rs"), lib).unwrap();
    std::fs::write(app.workspace.root.join("src/main.rs"), main).unwrap();
    let symbols = ok(call(&app, &sid, "list_symbols", json!({"path":"src/calc.rs"})).await);
    assert_eq!(symbols["symbols"][0]["name"], "value");
    let hits = ok(call(
        &app,
        &sid,
        "search_files",
        json!({"path":"src","query":"value"}),
    )
    .await);
    assert_eq!(hits["matches"].as_array().unwrap().len(), 2);
    let patch=ok(call(&app,&sid,"preview_patch",json!({"operations":[{"path":"src/calc.rs","content":"pub fn number() -> u32 { 7 }\n","expected_sha256":sha256_bytes(lib.as_bytes())},{"path":"src/main.rs","content":"mod calc;\nfn main(){ assert_eq!(calc::number(), 7); }\n","expected_sha256":sha256_bytes(main.as_bytes())}]})).await);
    ok(decide(
        &app,
        &sid,
        "apply_patch",
        json!({"patch_id":patch["patch_id"]}),
        true,
        || {},
    )
    .await);
    let rustc = std::env::var_os("RUSTC")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            let out = std::process::Command::new("rustup")
                .args(["which", "rustc"])
                .output()
                .unwrap();
            std::path::PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        });
    let app = app
        .grant_command(AllowedCommand {
            command_id: "compile-check".into(),
            program: rustc,
            fixed_args: vec![
                "--edition=2021".into(),
                "--emit=metadata".into(),
                "--error-format=json".into(),
                "src/main.rs".into(),
                "-o".into(),
                "output/check.rmeta".into(),
            ],
            allowed_args: vec![],
            cwd: ".".into(),
            timeout_secs: 15,
            max_output_bytes: 20000,
            writes: true,
            always_allow: false,
        })
        .unwrap();
    let result = ok(decide(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"compile-check"}),
        true,
        || {},
    )
    .await);
    assert_eq!(result["exit_code"], 0);
    assert!(app.workspace.root.join("output/check.rmeta").exists());
    std::fs::write(
        app.workspace.root.join("src/main.rs"),
        "fn main(){ let _:u32 = true; }\n",
    )
    .unwrap();
    let failed = decide(
        &app,
        &sid,
        "run_command",
        json!({"command_id":"compile-check"}),
        true,
        || {},
    )
    .await;
    assert_eq!(failed.status, "error");
    let data = failed.data.unwrap();
    assert!(data["diagnostics"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"] == "E0308"
            && d["locations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["path"] == "src/main.rs")));
    assert!(app.store.verify_audit().unwrap().valid);
    let audit=std::fs::read_to_string(app.home.base.join("audit.jsonl")).unwrap();
    assert!(audit.contains("tool.list_symbols")&&audit.contains("tool.search_files"));
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
    std::fs::copy(
        env!("CARGO_BIN_EXE_unified-ai-bridge-native"),
        home.installed_binary(),
    )
    .unwrap();
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
