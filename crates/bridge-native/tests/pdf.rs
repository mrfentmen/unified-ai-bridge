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
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn research_datasets_other_agent_folders_and_hardlinks_remain_protected() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    let bytes = pdf(false, 3, 200);
    let hash = sha256_bytes(&bytes);
    let source = "AGENTS/CHATGPT/notes/input.pdf";
    std::fs::write(app.workspace.root.join(source), &bytes).unwrap();
    for destination in ["SHARED/datasets/pages", "AGENTS/CLAUDE/notes/pages"] {
        assert_eq!(call(&app,&sid,"render_document_pages",json!({"path":source,"expected_sha256":hash,"destination":destination,"first_page":1})).await.code.as_deref(),Some("permission_denied"));
    }
    std::fs::hard_link(
        app.workspace.root.join(source),
        app.workspace.root.join("AGENTS/CHATGPT/notes/linked.pdf"),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "document_info", json!({"path":source}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        std::fs::read(app.workspace.root.join(source)).unwrap(),
        bytes
    );
}

// Minimal valid PDF fixtures, generated here solely for parser/policy tests.
// Public-document and browser acceptance are separate evidence.
fn pdf(encrypted: bool, page_count: usize, width: u32) -> Vec<u8> {
    let mut objects=vec![
        "<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /JavaScript /JS (app.launchURL('http://127.0.0.1/forbidden')) >> >>".to_string(),
        format!("<< /Type /Pages /Count {page_count} /Kids [{}] >>",(0..page_count).map(|i|format!("{} 0 R",4+i*2)).collect::<Vec<_>>().join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for i in 0..page_count {
        let rotate = if i == 1 { 90 } else { 0 };
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} 120] /Rotate {rotate} /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R /Annots [<< /Type /Annot /Subtype /Link /Rect [0 0 100 10] /A << /S /URI /URI (https://example.invalid/not-opened) >> >>] >>",5+i*2));
        let stream = if i == 2 {
            String::new()
        } else {
            format!(
                "1 0 0 rg 10 90 20 20 re f\n0 0 0 rg BT /F1 12 Tf 10 60 Td (Page {} {}) Tj ET\n",
                i + 1,
                "readable words ".repeat(30)
            )
        };
        objects.push(format!(
            "<< /Length {} >>\nstream\n{stream}endstream",
            stream.len()
        ));
    }
    if encrypted {
        objects.push(format!(
            "<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <{}> /U <{}> >>",
            "00".repeat(32),
            "00".repeat(32)
        ));
    }
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let encrypt = if encrypted {
        format!(" /Encrypt {} 0 R /ID [<1234567890abcdef1234567890abcdef><1234567890abcdef1234567890abcdef>]",objects.len())
    } else {
        String::new()
    };
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R{encrypt} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    bytes
}
fn save(app: &App, bytes: &[u8]) -> String {
    std::fs::write(app.workspace.root.join("files/input.pdf"), bytes).unwrap();
    sha256_bytes(bytes)
}
fn render_args(hash: &str, name: &str) -> Value {
    json!({"path":"files/input.pdf","destination":format!("output/{name}"),"expected_sha256":hash,"first_page":1,"last_page":3,"max_dimension":400})
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn embedded_text_chunks_keep_physical_page_ranges_and_empty_page_information() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let hash = save(&app, &pdf(false, 3, 200));
    let info = ok(call(
        &app,
        &sid,
        "document_info",
        json!({"path":"files/input.pdf"}),
    )
    .await);
    assert_eq!(info["page_count"], 3);
    assert_eq!(info["pages"][1]["rotation_degrees"], 90);
    let full = ok(call(
        &app,
        &sid,
        "extract_document_text",
        json!({"path":"files/input.pdf"}),
    )
    .await);
    assert_eq!(full["pages_without_text"], json!([3]));
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let part=ok(call(&app,&sid,"extract_document_text",json!({"path":"files/input.pdf","offset":offset,"max_bytes":256,"expected_sha256":hash})).await);
        let s = part["text"].as_str().unwrap();
        assert!(part.to_string().len() <= 48000);
        for range in part["page_ranges"].as_array().unwrap() {
            assert!((1..=2).contains(&range["page"].as_u64().unwrap()));
            assert!(range["end"].as_u64().unwrap() <= s.len() as u64);
            assert!(range["start"].as_u64().unwrap() < range["end"].as_u64().unwrap());
        }
        text.push_str(s);
        match part["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(text, full["text"].as_str().unwrap());
    assert!(text.contains("Page 1"));
    assert!(text.contains("Page 2"));
    assert!(!text.contains("example.invalid"));
    assert!(!text.contains("launchURL"));
    assert!(app.store.verify_audit().unwrap().valid);
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn approved_previews_preserve_rotation_pixels_source_and_audit() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let bytes = pdf(false, 3, 200);
    let hash = save(&app, &bytes);
    let data = ok(decide(
        &app,
        &sid,
        "render_document_pages",
        render_args(&hash, "pages"),
        true,
        || {},
    )
    .await);
    assert_eq!(data["state"], "completed");
    for (i, (w, h)) in [(400, 240), (240, 400), (400, 240)].into_iter().enumerate() {
        let path = app
            .workspace
            .root
            .join(format!("output/pages/page-{:04}.png", i + 1));
        let png = std::fs::read(&path).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (w, h));
        assert_eq!(data["pages"][i]["sha256"], sha256_bytes(&png));
        if i == 0 {
            assert_eq!(image.get_pixel(30, 40).0, [255, 0, 0, 255]);
            assert_eq!(image.get_pixel(370, 210).0, [255, 255, 255, 255]);
        }
        if i == 1 {
            assert_eq!(image.get_pixel(200, 30).0, [255, 0, 0, 255]);
        }
        if i == 2 {
            assert!(image.pixels().all(|p| p.0 == [255, 255, 255, 255]));
        }
    }
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/input.pdf")).unwrap(),
        bytes
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn denied_and_changed_approvals_publish_nothing_and_never_clobber() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for mode in ["deny", "source", "destination", "lock", "symlink"] {
        let hash = save(&app, &pdf(false, 3, 200));
        let path = app.workspace.root.join(format!("output/{mode}"));
        let result = decide(
            &app,
            &sid,
            "render_document_pages",
            render_args(&hash, mode),
            mode != "deny",
            || match mode {
                "source" => {
                    save(&app, &pdf(false, 3, 220));
                }
                "destination" => std::fs::write(&path, "keep me").unwrap(),
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
                "symlink" => std::os::unix::fs::symlink(t.path(), &path).unwrap(),
                _ => {}
            },
        )
        .await;
        assert_eq!(result.status, "error", "{mode}: {result:?}");
        if mode == "deny" {
            assert_eq!(result.code.as_deref(), Some("approval_denied"));
        }
        if mode == "destination" {
            assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
        }
        assert!(!path.join("page-0001.png").exists());
        if mode == "lock" {
            let mut w = app
                .store
                .workspaces()
                .unwrap()
                .into_iter()
                .find(|w| w.id == app.workspace.id())
                .unwrap();
            w.read_only = false;
            app.store.save_workspace(&w).unwrap();
        }
    }
    assert!(std::fs::read_dir(app.workspace.root.join("output"))
        .unwrap()
        .all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".bridge-archive-")));
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn protected_paths_unknown_arguments_and_unavailable_platforms_are_explicit() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let hash = save(&app, &pdf(false, 3, 200));
    std::fs::write(app.workspace.root.join(".env"), "secret").unwrap();
    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("files/escape")).unwrap();
    for tool in [
        "document_info",
        "extract_document_text",
        "render_document_pages",
    ] {
        for path in [".env", "../outside.pdf", "files/escape/outside.pdf"] {
            let args = if tool == "render_document_pages" {
                json!({"path":path,"destination":"output/denied","first_page":1,"expected_sha256":hash})
            } else {
                json!({"path":path})
            };
            assert_eq!(
                call(&app, &sid, tool, args).await.code.as_deref(),
                Some("permission_denied")
            );
        }
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "document_info",
            json!({"path":"files/input.pdf","identity":"fake"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    #[cfg(not(target_os = "macos"))]
    for tool in [
        "document_info",
        "extract_document_text",
        "render_document_pages",
    ] {
        let args = if tool == "render_document_pages" {
            render_args(&hash, "unsupported")
        } else {
            json!({"path":"files/input.pdf"})
        };
        assert_eq!(
            call(&app, &sid, tool, args).await.code.as_deref(),
            Some("unsupported")
        );
    }
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn encrypted_oversized_stale_malformed_and_bad_ranges_fail_without_losing_host() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for bytes in [
        pdf(true, 3, 200),
        pdf(false, 201, 200),
        pdf(false, 3, 20001),
        b"%PDF-1.4\nbroken".to_vec(),
    ] {
        save(&app, &bytes);
        let result = call(
            &app,
            &sid,
            "document_info",
            json!({"path":"files/input.pdf"}),
        )
        .await;
        assert_eq!(result.code.as_deref(), Some("unsupported"), "{result:?}");
        if bytes.windows(8).any(|w| w == b"/Encrypt") {
            assert!(result.error.unwrap().contains("encrypted"));
        }
    }
    let hash = save(&app, &pdf(false, 3, 200));
    for (key, value) in [
        ("first_page", json!(0)),
        ("last_page", json!(9)),
        ("max_dimension", json!(1601)),
        ("first_page", json!(4)),
    ] {
        let mut args = render_args(&hash, "invalid");
        args[key] = value;
        assert_eq!(
            call(&app, &sid, "render_document_pages", args)
                .await
                .code
                .as_deref(),
            Some("invalid_arguments")
        );
    }
    let mut args = render_args("stale", "stale");
    assert_eq!(
        call(&app, &sid, "render_document_pages", args.clone())
            .await
            .code
            .as_deref(),
        Some("file_changed")
    );
    args["expected_sha256"] = json!(hash);
    args["destination"] = json!("../outside");
    assert_eq!(
        call(&app, &sid, "render_document_pages", args)
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "extract_document_text",
            json!({"path":"files/input.pdf","offset":1})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "extract_document_text",
            json!({"path":"files/input.pdf","offset":99999,"expected_sha256":hash})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    ok(call(
        &app,
        &sid,
        "document_info",
        json!({"path":"files/input.pdf"}),
    )
    .await);
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
    .expect("image tool should complete or request an approval promptly")
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
