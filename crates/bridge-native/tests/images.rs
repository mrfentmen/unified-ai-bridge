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
use image::ImageEncoder;
use serde_json::{json, Value};
use std::sync::Arc;
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
fn png_bytes(alpha: bool) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(6, 4, |x, y| {
        image::Rgba([
            x as u8 * 30,
            y as u8 * 40,
            100,
            if alpha && x == 0 { 50 } else { 255 },
        ])
    });
    let mut output = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
fn save(app: &App, bytes: &[u8]) -> String {
    std::fs::write(app.workspace.root.join("files/input.png"), bytes).unwrap();
    sha256_bytes(bytes)
}
fn args(hash: &str, name: &str, operation: Value, format: &str) -> Value {
    json!({"path":"files/input.png","destination":format!("files/{name}"),"expected_sha256":hash,"operation":operation,"format":format})
}
fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(temp, Profile::General)
}

#[tokio::test]
async fn resize_crop_rotate_and_convert_real_images_reopen_with_expected_pixels() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let bytes = png_bytes(false);
    let hash = save(&app, &bytes);
    let root = &app.workspace.root;
    let info = ok(call(&app, &sid, "image_info", json!({"path":"files/input.png"})).await);
    assert_eq!(info["width"], 6);
    assert_eq!(info["height"], 4);
    assert_eq!(info["sha256"], hash);
    for (name, op, format, w, h) in [
        (
            "resized.png",
            json!({"kind":"resize","width":3,"height":2}),
            "png",
            3,
            2,
        ),
        (
            "cropped.png",
            json!({"kind":"crop","x":1,"y":1,"width":2,"height":2}),
            "png",
            2,
            2,
        ),
        (
            "rotated.png",
            json!({"kind":"rotate","degrees":90}),
            "png",
            4,
            6,
        ),
        ("converted.jpg", json!({"kind":"convert"}), "jpeg", 6, 4),
        ("converted.webp", json!({"kind":"convert"}), "webp", 6, 4),
    ] {
        let result = ok(decide(
            &app,
            &sid,
            "transform_image",
            args(&hash, name, op, format),
            true,
            || {},
        )
        .await);
        let encoded = std::fs::read(root.join("files").join(name)).unwrap();
        let decoded = image::load_from_memory(&encoded).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (w, h));
        assert_eq!(result["sha256"], sha256_bytes(&encoded));
        assert_eq!(result["state"], "completed");
        if name == "cropped.png" {
            assert_eq!(
                decoded.to_rgba8().get_pixel(0, 0),
                image::load_from_memory(&bytes)
                    .unwrap()
                    .to_rgba8()
                    .get_pixel(1, 1)
            );
        }
        if name == "rotated.png" {
            assert_eq!(
                decoded.to_rgba8().get_pixel(3, 0),
                image::load_from_memory(&bytes)
                    .unwrap()
                    .to_rgba8()
                    .get_pixel(0, 0)
            );
        }
    }
    assert_eq!(std::fs::read(root.join("files/input.png")).unwrap(), bytes);
    assert!(app.store.verify_audit().unwrap().valid);
}

#[tokio::test]
async fn denials_stale_hashes_locks_destinations_and_symlink_changes_never_clobber() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    for mode in ["deny", "source", "lock", "destination", "symlink"] {
        let hash = save(&app, &png_bytes(false));
        let name = format!("{mode}.png");
        let result = decide(
            &app,
            &sid,
            "transform_image",
            args(&hash, &name, json!({"kind":"convert"}), "png"),
            mode != "deny",
            || match mode {
                "source" => {
                    std::fs::write(root.join("files/input.png"), png_bytes(true)).unwrap();
                }
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
                "destination" => {
                    std::fs::write(root.join("files").join(&name), "user file").unwrap();
                }
                "symlink" => {
                    std::fs::remove_file(root.join("files/input.png")).unwrap();
                    std::os::unix::fs::symlink(
                        temp.path().join("outside.png"),
                        root.join("files/input.png"),
                    )
                    .unwrap();
                }
                _ => {}
            },
        )
        .await;
        assert_eq!(result.status, "error", "{result:?}");
        if mode == "destination" {
            assert_eq!(
                std::fs::read_to_string(root.join("files").join(&name)).unwrap(),
                "user file"
            );
        } else {
            assert!(!root.join("files").join(&name).exists());
        }
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

#[tokio::test]
async fn transparent_images_preserve_alpha_and_jpeg_refuses_silent_flattening() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let hash = save(&app, &png_bytes(true));
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "bad.jpg", json!({"kind":"convert"}), "jpeg")
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
    for (name, format) in [("alpha.png", "png"), ("alpha.webp", "webp")] {
        ok(decide(
            &app,
            &sid,
            "transform_image",
            args(&hash, name, json!({"kind":"convert"}), format),
            true,
            || {},
        )
        .await);
        let output = image::open(app.workspace.root.join("files").join(name)).unwrap();
        assert_eq!(output.to_rgba8().get_pixel(0, 0).0[3], 50);
    }
    assert!(!app.workspace.root.join("files/bad.jpg").exists());
}

#[tokio::test]
async fn oriented_png_normalizes_pixels_but_embedded_icc_and_high_bit_depth_are_refused() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut bytes = vec![];
    // TIFF EXIF: one SHORT Orientation tag, value 6 (rotate 90 clockwise).
    let exif = vec![
        b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder.set_exif_metadata(exif).unwrap();
    encoder
        .write_image(
            &[255, 0, 0, 0, 255, 0],
            2,
            1,
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    let hash = save(&app, &bytes);
    ok(decide(
        &app,
        &sid,
        "transform_image",
        args(&hash, "oriented.png", json!({"kind":"convert"}), "png"),
        true,
        || {},
    )
    .await);
    let output = image::open(app.workspace.root.join("files/oriented.png")).unwrap();
    assert_eq!((output.width(), output.height()), (1, 2));
    assert_eq!(output.to_rgb8().get_pixel(0, 0).0, [255, 0, 0]);
    let mut bytes = vec![];
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder.set_icc_profile(vec![1; 128]).unwrap();
    encoder
        .write_image(&[255, 0, 0], 1, 1, image::ExtendedColorType::Rgb8)
        .unwrap();
    let hash = save(&app, &bytes);
    let info = ok(call(&app, &sid, "image_info", json!({"path":"files/input.png"})).await);
    assert_eq!(info["has_icc_profile"], true);
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "icc.png", json!({"kind":"convert"}), "png")
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
    let mut bytes = std::io::Cursor::new(vec![]);
    image::DynamicImage::ImageLuma16(image::ImageBuffer::from_pixel(
        2,
        1,
        image::Luma([12345u16]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    let hash = save(&app, &bytes.into_inner());
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "16bit.png", json!({"kind":"convert"}), "png")
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
}

#[tokio::test]
async fn animation_corruption_oversize_and_invalid_operations_fail_before_approval() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let mut animated = vec![];
    {
        let mut encoder = png::Encoder::new(&mut animated, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[255, 0, 0]).unwrap();
        writer.write_image_data(&[0, 255, 0]).unwrap();
        writer.finish().unwrap();
    }
    let hash = save(&app, &animated);
    let info = ok(call(&app, &sid, "image_info", json!({"path":"files/input.png"})).await);
    assert_eq!(info["animated"], true);
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "anim.png", json!({"kind":"convert"}), "png")
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
    let mut huge = vec![];
    {
        let mut encoder = png::Encoder::new(&mut huge, 100000, 100000);
        encoder.set_color(png::ColorType::Rgb);
        let _ = encoder.write_header().unwrap().finish();
    }
    save(&app, &huge);
    assert_eq!(
        call(&app, &sid, "image_info", json!({"path":"files/input.png"}))
            .await
            .status,
        "error"
    );
    let hash = save(&app, b"corrupt image");
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "bad.png", json!({"kind":"convert"}), "png")
        )
        .await
        .status,
        "error"
    );
    let hash = save(&app, &png_bytes(false));
    for op in [
        json!({"kind":"resize","width":0,"height":2}),
        json!({"kind":"resize","width":8192,"height":8192}),
        json!({"kind":"crop","x":5,"y":3,"width":2,"height":2}),
        json!({"kind":"rotate","degrees":45}),
        json!({"kind":"convert","identity":"fake"}),
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "transform_image",
                args(&hash, "bad.png", op, "png")
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_image",
            args(&hash, "bad.jpg", json!({"kind":"convert"}), "png")
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(app.approvals.pending_count(), 0);
    assert!(!app.workspace.root.join("files/bad.png").exists());
}

#[tokio::test]
async fn private_paths_unknown_fields_and_research_policy_remain_denied() {
    let _serial = TEST_LOCK.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let hash = save(&app, &png_bytes(false));
    for path in ["../outside.png", ".bridge/private.png", ".env"] {
        assert_eq!(
            call(&app, &sid, "image_info", json!({"path":path}))
                .await
                .status,
            "error"
        );
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "image_info",
            json!({"path":"files/input.png","identity":"fake"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    let mut request = args(&hash, "unused.png", json!({"kind":"convert"}), "png");
    request["destination"] = json!(".git/private.png");
    assert_eq!(
        call(&app, &sid, "transform_image", request)
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&temp, Profile::Research);
    let bytes = png_bytes(false);
    std::fs::write(
        app.workspace.root.join("AGENTS/CHATGPT/notes/input.png"),
        &bytes,
    )
    .unwrap();
    for destination in ["SHARED/datasets/image.png", "AGENTS/CLAUDE/notes/image.png"] {
        assert_eq!(call(&app,&sid,"transform_image",json!({"path":"AGENTS/CHATGPT/notes/input.png","destination":destination,"expected_sha256":sha256_bytes(&bytes),"operation":{"kind":"convert"}})).await.code.as_deref(),Some("permission_denied"));
    }
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
    tokio::time::timeout(std::time::Duration::from_secs(5),tools::dispatch(app, Agent::ChatGpt, sid, tool, args)).await.expect("image tool should complete or request an approval promptly")
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
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            change();
            assert!(app.approvals.resolve(&approval.id, allow));
            break;
        }
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap()
}
