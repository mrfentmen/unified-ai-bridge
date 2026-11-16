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
fn wav() -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut cursor,
            hound::WavSpec {
                channels: 2,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for i in 0..16000 {
            writer.write_sample((i as i16) - 8000).unwrap();
        }
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}
fn save(app: &App) -> (Vec<u8>, String) {
    let bytes = wav();
    std::fs::write(app.workspace.root.join("files/input.wav"), &bytes).unwrap();
    let hash = sha256_bytes(&bytes);
    (bytes, hash)
}
fn args(hash: &str, destination: &str) -> Value {
    json!({"path":"files/input.wav","destination":destination,"expected_sha256":hash,"operation":{"kind":"trim","start_frame":1000,"end_frame":3000}})
}
#[tokio::test]
async fn approved_trim_reopens_with_exact_samples_and_keeps_source_and_audit() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (input, hash) = save(&app);
    let meta = ok(call(&app, &sid, "media_info", json!({"path":"files/input.wav"})).await);
    assert_eq!(meta["frames"], 8000);
    assert_eq!(meta["duration_seconds"], 1.0);
    assert_eq!(meta["trim_supported"], true);
    let data = ok(decide(
        &app,
        &sid,
        "transform_media",
        args(&hash, "output/clip.wav"),
        true,
        || {},
    )
    .await);
    assert_eq!(data["frames"], 2000);
    assert_eq!(data["duration_seconds"], 0.25);
    let output = std::fs::read(app.workspace.root.join("output/clip.wav")).unwrap();
    assert_eq!(data["sha256"], sha256_bytes(&output));
    let mut reader = hound::WavReader::new(std::io::Cursor::new(&output)).unwrap();
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        samples,
        (2000..6000).map(|i| (i as i16) - 8000).collect::<Vec<_>>()
    );
    let reopened = ok(call(&app, &sid, "media_info", json!({"path":"output/clip.wav"})).await);
    assert_eq!(reopened["sha256"], data["sha256"]);
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/input.wav")).unwrap(),
        input
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn approval_denial_changes_and_lock_prevent_publication() {
    let _serial = TEST_LOCK.lock().await;
    for mode in ["deny", "source", "destination", "parent", "lock"] {
        let t = tempfile::tempdir().unwrap();
        let (app, sid) = fixture(&t);
        let (_, hash) = save(&app);
        let out = app.workspace.root.join("output/clip.wav");
        let r = decide(
            &app,
            &sid,
            "transform_media",
            args(&hash, "output/clip.wav"),
            mode != "deny",
            || match mode {
                "source" => {
                    std::fs::write(app.workspace.root.join("files/input.wav"), "changed").unwrap()
                }
                "destination" => std::fs::write(&out, "preserve me").unwrap(),
                "parent" => {
                    std::fs::rename(
                        app.workspace.root.join("output"),
                        app.workspace.root.join("old-output"),
                    )
                    .unwrap();
                    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("output"))
                        .unwrap();
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
                _ => {}
            },
        )
        .await;
        assert_eq!(r.status, "error", "{mode}: {r:?}");
        if mode == "deny" {
            assert_eq!(r.code.as_deref(), Some("approval_denied"));
        }
        if mode == "source" {
            assert_eq!(r.code.as_deref(), Some("file_changed"));
        }
        if mode == "destination" {
            assert_eq!(std::fs::read(&out).unwrap(), b"preserve me");
        } else {
            assert!(!out.exists());
        }
        assert!(!t.path().join("clip.wav").exists());
        assert!(app.store.verify_audit().unwrap().valid);
    }
}
#[tokio::test]
async fn strict_schema_invalid_ranges_metadata_and_stale_hash_never_write() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (bytes, hash) = save(&app);
    for operation in [
        json!({"kind":"trim","start_frame":3000,"end_frame":3000}),
        json!({"kind":"trim","start_frame":0,"end_frame":8001}),
        json!({"kind":"trim","start_frame":-1,"end_frame":1}),
        json!({"kind":"trim","start_frame":0,"end_frame":1,"shell":"never"}),
        json!({"kind":"convert"}),
    ] {
        let mut a = args(&hash, "output/invalid.wav");
        a["operation"] = operation;
        assert_eq!(
            call(&app, &sid, "transform_media", a).await.code.as_deref(),
            Some("invalid_arguments")
        );
    }
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_media",
            args("stale", "output/stale.wav")
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    assert_eq!(
        call(
            &app,
            &sid,
            "media_info",
            json!({"path":"files/input.wav","agent":"Claude"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    let mut tagged = bytes.clone();
    tagged.extend(b"cue \x04\0\0\0\0\0\0\0");
    let len = (tagged.len() - 8) as u32;
    tagged[4..8].copy_from_slice(&len.to_le_bytes());
    std::fs::write(app.workspace.root.join("files/input.wav"), &tagged).unwrap();
    let meta = ok(call(&app, &sid, "media_info", json!({"path":"files/input.wav"})).await);
    assert_eq!(meta["trim_supported"], false);
    assert_eq!(
        call(
            &app,
            &sid,
            "transform_media",
            args(&sha256_bytes(&tagged), "output/tagged.wav")
        )
        .await
        .code
        .as_deref(),
        Some("unsupported")
    );
    std::fs::write(
        app.workspace.root.join("files/input.wav"),
        b"ID3 not a supported file",
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "media_info", json!({"path":"files/input.wav"}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    let oversized = std::fs::File::create(app.workspace.root.join("files/oversized.wav")).unwrap();
    oversized.set_len(32 * 1024 * 1024 + 1).unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "media_info",
            json!({"path":"files/oversized.wav"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(
        std::fs::read_dir(app.workspace.root.join("output"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn private_paths_links_and_research_boundaries_are_checked_for_read_and_write() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let (bytes, hash) = save(&app);
    for path in [
        "../outside.wav",
        ".bridge/a.wav",
        ".git/a.wav",
        "files/.env",
    ] {
        assert_eq!(
            call(&app, &sid, "media_info", json!({"path":path}))
                .await
                .status,
            "error"
        );
    }
    assert_eq!(
        call(&app, &sid, "transform_media", args(&hash, "../outside.wav"))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    std::fs::hard_link(
        app.workspace.root.join("files/input.wav"),
        app.workspace.root.join("files/hard.wav"),
    )
    .unwrap();
    assert_eq!(
        call(&app, &sid, "media_info", json!({"path":"files/input.wav"}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    let src = "AGENTS/CHATGPT/notes/input.wav";
    std::fs::write(app.workspace.root.join(src), &bytes).unwrap();
    for destination in ["SHARED/datasets/clip.wav", "AGENTS/CLAUDE/notes/clip.wav"] {
        let mut a = args(&hash, destination);
        a["path"] = json!(src);
        assert_eq!(
            call(&app, &sid, "transform_media", a).await.code.as_deref(),
            Some("permission_denied")
        );
    }
    let mut a = args(&hash, "AGENTS/CHATGPT/notes/clip.wav");
    a["path"] = json!(src);
    ok(decide(&app, &sid, "transform_media", a, true, || {}).await);
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
    .expect("media tool should complete or request an approval promptly")
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
