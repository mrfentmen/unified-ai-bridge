use bridge_core::{
    provider::{Agent, Provider},
    workspace::Profile,
};
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
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn args(destination: &str) -> Value {
    json!({"destination":destination,"format":"pdf","template":"report","title":"PDF test report","blocks":[
        {"type":"heading","text":"First section"},
        {"type":"paragraph","text":"Résumé naïve café Ångström. Literal content: <script>alert(1)</script> https://example.invalid/never-opened"},
        {"type":"bullet","text":"A reviewed, create-only file."},
        {"type":"page_break"},
        {"type":"heading","text":"Second section"},
        {"type":"paragraph","text":"After the deliberate page break."}
    ]})
}
#[tokio::test]
async fn approved_pdf_is_real_create_only_and_audited() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let data = ok(decide(
        &app,
        &sid,
        "create_document",
        args("output/report.pdf"),
        true,
        || {},
    )
    .await);
    let bytes = std::fs::read(app.workspace.root.join("output/report.pdf")).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"));
    assert_eq!(data["pages"], 2);
    assert_eq!(data["state"], "completed");
    assert_eq!(data["sha256"], sha256_bytes(&bytes));
    for forbidden in [
        b"/JavaScript".as_slice(),
        b"/OpenAction",
        b"/Annots",
        b"/EmbeddedFiles",
        b"/URI",
    ] {
        assert!(!bytes.windows(forbidden.len()).any(|b| b == forbidden));
    }
    let repeated = call(&app, &sid, "create_document", args("output/report.pdf")).await;
    assert_eq!(repeated.code.as_deref(), Some("file_changed"));
    assert_eq!(
        std::fs::read(app.workspace.root.join("output/report.pdf")).unwrap(),
        bytes
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn generated_pdf_reopens_extracts_unicode_and_renders_every_page() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let data = ok(decide(
        &app,
        &sid,
        "create_document",
        args("output/report.pdf"),
        true,
        || {},
    )
    .await);
    let meta = ok(call(
        &app,
        &sid,
        "document_info",
        json!({"path":"output/report.pdf"}),
    )
    .await);
    assert_eq!(meta["page_count"], 2);
    let text = ok(call(
        &app,
        &sid,
        "extract_document_text",
        json!({"path":"output/report.pdf"}),
    )
    .await);
    let extracted = text["text"].as_str().unwrap();
    for expected in [
        "PDF test report",
        "Résumé naïve café Ångström",
        "<script>alert(1)</script>",
        "https://example.invalid/never-opened",
        "After the deliberate page break.",
    ] {
        assert!(
            extracted.contains(expected),
            "missing {expected}: {extracted}"
        );
    }
    let render=ok(decide(&app,&sid,"render_document_pages",json!({"path":"output/report.pdf","destination":"output/pages","expected_sha256":data["sha256"],"first_page":1,"last_page":2,"max_dimension":842}),true,||{}).await);
    assert_eq!(render["pages"].as_array().unwrap().len(), 2);
    for page in 1..=2 {
        let png = std::fs::read(
            app.workspace
                .root
                .join(format!("output/pages/page-{page:04}.png")),
        )
        .unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(image.dimensions(), (595, 842));
        assert!(image.pixels().any(|p| p.0[0] < 100));
        // Margins are clear: text must not be clipped against page edges.
        assert!(image
            .enumerate_pixels()
            .filter(|(x, y, _)| *x < 40 || *x > 555 || *y < 35 || *y > 820)
            .all(|(_, _, p)| p.0 == [255, 255, 255]));
    }
}
#[tokio::test]
async fn approval_denial_and_destination_changes_leave_no_unreviewed_pdf() {
    let _serial = TEST_LOCK.lock().await;
    for mode in ["deny", "destination", "symlink", "parent", "lock"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let path = app.workspace.root.join("output/report.pdf");
        let result = decide(
            &app,
            &sid,
            "create_document",
            args("output/report.pdf"),
            mode != "deny",
            || match mode {
                "destination" => std::fs::write(&path, "preserve me").unwrap(),
                "symlink" => {
                    std::os::unix::fs::symlink(t.path().join("outside.pdf"), &path).unwrap()
                }
                "parent" => {
                    std::fs::rename(
                        app.workspace.root.join("output"),
                        app.workspace.root.join("original-output"),
                    )
                    .unwrap();
                    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("output"))
                        .unwrap();
                }
                "lock" => {
                    let mut ws = app
                        .store
                        .workspaces()
                        .unwrap()
                        .into_iter()
                        .find(|w| w.id == app.workspace.id())
                        .unwrap();
                    ws.read_only = true;
                    app.store.save_workspace(&ws).unwrap();
                }
                _ => {}
            },
        )
        .await;
        assert_eq!(result.status, "error", "{mode}: {result:?}");
        if mode == "deny" {
            assert_eq!(result.code.as_deref(), Some("approval_denied"));
        }
        if mode == "destination" {
            assert_eq!(std::fs::read(&path).unwrap(), b"preserve me");
        } else {
            assert!(!path.exists());
        }
        assert!(!t.path().join("outside.pdf").exists());
        assert!(!t.path().join("report.pdf").exists());
        assert!(app.store.verify_audit().unwrap().valid);
    }
}
#[tokio::test]
async fn invalid_content_and_unsupported_formats_fail_before_approval() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let mut cases = vec![];
    for (key, value) in [
        ("format", json!("docx")),
        ("template", json!("arbitrary")),
        ("destination", json!("output/file.docx")),
        ("agent", json!("Claude")),
        ("title", json!(" ")),
        ("title", json!("emoji 😀")),
        ("blocks", json!([])),
        (
            "blocks",
            json!([{"type":"paragraph","text":"safe","url":"http://example.invalid"}]),
        ),
    ] {
        let mut a = args("output/report.pdf");
        a[key] = value;
        cases.push(a);
    }
    for blocks in [
        json!([{"type":"page_break","text":"hidden"}]),
        json!([{"type":"paragraph","text":"a".repeat(4001)}]),
        json!(vec![json!({"type":"paragraph","text":"a"}); 201]),
    ] {
        let mut a = args("output/report.pdf");
        a["blocks"] = blocks;
        cases.push(a);
    }
    for a in cases {
        assert_eq!(
            call(&app, &sid, "create_document", a).await.code.as_deref(),
            Some("invalid_arguments")
        );
    }
    assert_eq!(
        std::fs::read_dir(app.workspace.root.join("output"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn protected_paths_and_research_boundaries_are_not_weakened() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for path in [
        "../outside.pdf",
        ".bridge/report.pdf",
        ".git/report.pdf",
        "files/.env/report.pdf",
    ] {
        assert_eq!(
            call(&app, &sid, "create_document", args(path)).await.status,
            "error"
        );
    }
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    for path in [
        "SHARED/datasets/report.pdf",
        "AGENTS/CLAUDE/notes/report.pdf",
    ] {
        assert_eq!(
            call(&app, &sid, "create_document", args(path))
                .await
                .code
                .as_deref(),
            Some("permission_denied")
        );
    }
    ok(decide(
        &app,
        &sid,
        "create_document",
        args("AGENTS/CHATGPT/notes/report.pdf"),
        true,
        || {},
    )
    .await);
    assert!(app
        .workspace
        .root
        .join("AGENTS/CHATGPT/notes/report.pdf")
        .is_file());
    assert!(app.store.verify_audit().unwrap().valid);
}
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
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
    std::fs::create_dir_all(&home.install_dir).unwrap();
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
            "https://chatgpt.com/c/document-create",
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
    .expect("document tool should complete or request an approval promptly")
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
