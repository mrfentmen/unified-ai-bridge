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
use std::sync::Arc;
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn plain_png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    use image::ImageEncoder;
    let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
    let mut bytes = vec![];
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}
#[cfg(target_os = "macos")]
async fn page(app: &Arc<App>, sid: &str) -> String {
    let data=ok(decide(app,sid,"create_document",json!({"destination":"output/ocr-source.pdf","format":"pdf","template":"report","title":"LOCAL FILE REVIEW","blocks":[{"type":"paragraph","text":"READ THE NOTES BEFORE WRITING."},{"type":"paragraph","text":"This is an OCR fixture, not a provider response."},{"type":"paragraph","text":"The local bridge reads permitted files. ".repeat(18)}]}),true,||{}).await);
    ok(decide(app,sid,"render_document_pages",json!({"path":"output/ocr-source.pdf","destination":"output/ocr-pages","expected_sha256":data["sha256"],"first_page":1,"max_dimension":1200}),true,||{}).await);
    "output/ocr-pages/page-0001.png".into()
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_rasterized_text_is_recognized_with_geometry_bounds_and_truncation() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let path = page(&app, &sid).await;
    let before = std::fs::read(app.workspace.root.join(&path)).unwrap();
    let data = ok(call(&app, &sid, "ocr_image", json!({"path":path})).await);
    assert_eq!(data["mode"], "fast_cpu");
    assert_eq!(data["revision"], 1);
    assert_eq!(data["language"], "en-US");
    assert_eq!(data["truncated"], false);
    let text = data["text"].as_str().unwrap();
    assert!(text.contains("LOCAL FILE REVIEW"), "{text}");
    assert!(text.contains("READ THE NOTES BEFORE"), "{text}");
    let actual = text.lines().take(2).collect::<Vec<_>>().join(" ");
    assert!(
        similar::TextDiff::from_chars("LOCAL FILE REVIEW READ THE NOTES BEFORE WRITING.", &actual)
            .ratio()
            > 0.90,
        "{actual}"
    );
    assert!(data.to_string().len() < 64000);
    for line in data["lines"].as_array().unwrap() {
        for key in ["x", "y", "width", "height"] {
            assert!((0.0..=1.0).contains(&line["box"][key].as_f64().unwrap()));
        }
        assert!((0.0..=1.0).contains(&line["confidence"].as_f64().unwrap()));
    }
    let small = ok(call(
        &app,
        &sid,
        "ocr_image",
        json!({"path":path,"max_bytes":256,"expected_sha256":data["sha256"]}),
    )
    .await);
    assert_eq!(small["truncated"], true);
    assert!(small["text"].as_str().unwrap().len() <= 256);
    assert!(small["continue_hint"].is_string());
    assert_eq!(
        std::fs::read(app.workspace.root.join(&path)).unwrap(),
        before
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn blank_transparent_oversized_corrupt_and_high_depth_images_are_explicit() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for color in [[255, 255, 255, 255], [0, 0, 0, 0]] {
        std::fs::write(
            app.workspace.root.join("files/blank.png"),
            plain_png(300, 200, color),
        )
        .unwrap();
        let d = ok(call(&app, &sid, "ocr_image", json!({"path":"files/blank.png"})).await);
        assert_eq!(d["text"], "");
        assert_eq!(d["observation_count"], 0);
        assert_eq!(d["truncated"], false);
    }
    for bytes in [
        plain_png(4097, 1, [255; 4]),
        plain_png(2001, 2000, [255; 4]),
        b"\x89PNG\r\n\x1a\ncorrupt".to_vec(),
        b"not a png".to_vec(),
    ] {
        std::fs::write(app.workspace.root.join("files/bad.png"), bytes).unwrap();
        assert_eq!(
            call(&app, &sid, "ocr_image", json!({"path":"files/bad.png"}))
                .await
                .code
                .as_deref(),
            Some("unsupported")
        );
    }
    let mut bytes = vec![];
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 255]).unwrap();
    }
    std::fs::write(app.workspace.root.join("files/deep.png"), bytes).unwrap();
    assert_eq!(
        call(&app, &sid, "ocr_image", json!({"path":"files/deep.png"}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
}
#[tokio::test]
async fn strict_arguments_hash_limits_private_paths_and_links_are_enforced() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let path = "files/blank.png";
    std::fs::write(app.workspace.root.join(path), plain_png(300, 200, [255; 4])).unwrap();
    for a in [
        json!({"path":path,"max_bytes":255}),
        json!({"path":path,"max_bytes":24001}),
        json!({"path":path,"agent":"Claude"}),
        json!({"path":path,"language":"ar"}),
    ] {
        assert_eq!(
            call(&app, &sid, "ocr_image", a).await.code.as_deref(),
            Some("invalid_arguments")
        );
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "ocr_image",
            json!({"path":path,"expected_sha256":"stale"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    for p in ["../outside.png", ".bridge/private.png", ".git/private.png"] {
        assert_eq!(
            call(&app, &sid, "ocr_image", json!({"path":p}))
                .await
                .status,
            "error"
        );
    }
    std::fs::hard_link(
        app.workspace.root.join(path),
        app.workspace.root.join("files/link.png"),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "ocr_image", json!({"path":path}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let f = std::fs::File::create(app.workspace.root.join("files/oversized.png")).unwrap();
    f.set_len(8 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "ocr_image",
            json!({"path":"files/oversized.png"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    let private = "AGENTS/CLAUDE/notes/private.png";
    std::fs::write(
        app.workspace.root.join(private),
        plain_png(300, 200, [255; 4]),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "ocr_image", json!({"path":private}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn unavailable_platform_reports_unsupported_after_path_checks() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::write(
        app.workspace.root.join("files/plain.png"),
        plain_png(300, 200, [255; 4]),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "ocr_image", json!({"path":"files/plain.png"}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
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
