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
fn fixture(temp: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(temp, Profile::General)
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

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn save_zip(app: &App, entries: &[(&str, &[u8])]) -> String {
    let bytes = zip_bytes(entries);
    std::fs::write(app.workspace.root.join("files/input.zip"), &bytes).unwrap();
    sha256_bytes(&bytes)
}
fn no_staging(app: &App) {
    for entry in walkdir::WalkDir::new(&app.workspace.root) {
        let entry = entry.unwrap();
        assert!(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".bridge-archive-"),
            "leftover: {}",
            entry.path().display()
        );
    }
}

#[tokio::test]
async fn zip32_limits_encryption_nested_paths_and_declared_expansion_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let normal = zip_bytes(&[("a", b"data")]);
    let central = normal.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    for kind in ["encrypted", "zip64", "count", "ambiguous"] {
        let mut bytes = normal.clone();
        let end = bytes.len() - 22;
        match kind {
            "encrypted" => {
                bytes[6] |= 1;
                bytes[central + 8] |= 1;
            }
            "zip64" => {
                bytes[end + 10] = 255;
                bytes[end + 11] = 255;
                bytes[end + 8] = 255;
                bytes[end + 9] = 255;
            }
            "count" => {
                bytes[end + 10] = 255;
                bytes[end + 11] = 254;
                bytes[end + 8] = 255;
                bytes[end + 9] = 254;
            }
            _ => {
                bytes.extend_from_slice(b"PK\x05\x06");
            }
        }
        std::fs::write(app.workspace.root.join("files/input.zip"), bytes).unwrap();
        assert_eq!(
            call(
                &app,
                &sid,
                "list_archive",
                json!({"path":"files/input.zip"})
            )
            .await
            .status,
            "error",
            "{kind}"
        );
    }
    let name = vec!["nested"; 13].join("/");
    save_zip(&app, &[(&name, b"x")]);
    assert_eq!(
        call(
            &app,
            &sid,
            "list_archive",
            json!({"path":"files/input.zip"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    // Use real highly-compressible data to prove metadata expansion is capped
    // before extraction. Five 14 MiB entries exceed the 64 MiB total budget.
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let bytes = vec![0; 14 * 1024 * 1024];
    for i in 0..5 {
        writer
            .start_file(
                format!("part{i}"),
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .compression_level(Some(1)),
            )
            .unwrap();
        writer.write_all(&bytes).unwrap();
    }
    std::fs::write(
        app.workspace.root.join("files/input.zip"),
        writer.finish().unwrap().into_inner(),
    )
    .unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "list_archive",
            json!({"path":"files/input.zip"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    no_staging(&app);
}

#[tokio::test]
async fn real_source_and_binary_files_round_trip_without_execution_or_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    std::fs::write(
        root.join("files/main.rs"),
        "fn main() { println!(\"project\"); }\n",
    )
    .unwrap();
    std::fs::write(root.join("files/data.bin"), [0, 255, 1, 128]).unwrap();
    let data = ok(decide(
        &app,
        &sid,
        "create_archive",
        json!({"paths":["files/main.rs","files/data.bin"],"destination":"files/project.zip"}),
        true,
        || {},
    )
    .await);
    let bytes = std::fs::read(root.join("files/project.zip")).unwrap();
    assert_eq!(data["sha256"], sha256_bytes(&bytes));
    let listed = ok(call(
        &app,
        &sid,
        "list_archive",
        json!({"path":"files/project.zip"}),
    )
    .await);
    assert_eq!(listed["entries"].as_array().unwrap().len(), 2);
    let extracted=ok(decide(&app,&sid,"extract_archive",json!({"path":"files/project.zip","destination":"files/unpacked","expected_sha256":listed["sha256"]}),true,||{}).await);
    assert_eq!(extracted["state"], "completed");
    for name in ["main.rs", "data.bin"] {
        let original = std::fs::read(root.join("files").join(name)).unwrap();
        let output = root.join("files/unpacked/files").join(name);
        assert_eq!(std::fs::read(&output).unwrap(), original);
        assert_eq!(
            std::fs::metadata(output).unwrap().permissions().mode() & 0o111,
            0
        );
    }
    assert_eq!(call(&app,&sid,"extract_archive",json!({"path":"files/project.zip","destination":"files/unpacked","expected_sha256":listed["sha256"]})).await.code.as_deref(),Some("file_changed"));
    assert_eq!(
        std::fs::read(root.join("files/project.zip")).unwrap(),
        bytes
    );
    assert!(app.store.verify_audit().unwrap().valid);
    no_staging(&app);
}

#[tokio::test]
async fn denied_approvals_changed_sources_new_destinations_and_locked_workspace_do_not_write() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    std::fs::write(root.join("files/source.txt"), "original").unwrap();
    for change in ["deny", "source", "destination", "lock"] {
        let dest = format!("files/{change}.zip");
        let result = decide(
            &app,
            &sid,
            "create_archive",
            json!({"paths":["files/source.txt"],"destination":dest}),
            change != "deny",
            || match change {
                "source" => std::fs::write(root.join("files/source.txt"), "changed").unwrap(),
                "destination" => std::fs::write(root.join(&dest), "keep user file").unwrap(),
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
        assert_eq!(result.status, "error", "{result:?}");
        if change == "destination" {
            assert_eq!(
                std::fs::read_to_string(root.join(dest)).unwrap(),
                "keep user file"
            );
        } else {
            assert!(!root.join(dest).exists());
        }
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
    let hash = save_zip(&app, &[("file.txt", b"data")]);
    let args =
        json!({"path":"files/input.zip","destination":"files/denied","expected_sha256":hash});
    assert_eq!(
        decide(&app, &sid, "extract_archive", args.clone(), false, || {})
            .await
            .code
            .as_deref(),
        Some("approval_denied")
    );
    assert!(!root.join("files/denied").exists());
    assert_eq!(
        decide(&app, &sid, "extract_archive", args, true, || {
            std::fs::write(root.join("files/input.zip"), b"changed").unwrap();
        })
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    no_staging(&app);
}

#[tokio::test]
async fn unsafe_zip_paths_links_collisions_private_files_and_oversized_entries_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    for name in [
        "../escape",
        "/absolute",
        "a/../escape",
        "C:/drive",
        "a\\backslash",
        "a//b",
        "a./b",
        ".bridge/trash/item",
        ".GIT/config",
    ] {
        let hash = save_zip(&app, &[(name, b"data")]);
        assert_eq!(
            call(
                &app,
                &sid,
                "list_archive",
                json!({"path":"files/input.zip"})
            )
            .await
            .status,
            "error",
            "{name}"
        );
        assert_eq!(
            call(
                &app,
                &sid,
                "extract_archive",
                json!({"path":"files/input.zip","destination":"files/out","expected_sha256":hash})
            )
            .await
            .status,
            "error"
        );
        assert!(!app.workspace.root.join("files/out").exists());
    }
    for names in [
        ["A.txt", "a.txt"],
        ["Caf\u{e9}.txt", "Cafe\u{301}.txt"],
        ["Foo/a", "foo/b"],
        ["parent", "parent/child"],
    ] {
        save_zip(&app, &[(names[0], b"a"), (names[1], b"b")]);
        assert_eq!(
            call(
                &app,
                &sid,
                "list_archive",
                json!({"path":"files/input.zip"})
            )
            .await
            .status,
            "error"
        );
    }
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.add_symlink(
        "link",
        "/tmp/outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    std::fs::write(
        app.workspace.root.join("files/input.zip"),
        zip.finish().unwrap().into_inner(),
    )
    .unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "list_archive",
            json!({"path":"files/input.zip"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let hash = save_zip(&app, &[(".env", b"SECRET=not-for-export")]);
    assert_eq!(
        call(
            &app,
            &sid,
            "extract_archive",
            json!({"path":"files/input.zip","destination":"files/out","expected_sha256":hash})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let large = vec![0; 16 * 1024 * 1024 + 1];
    save_zip(&app, &[("huge", &large)]);
    assert_eq!(
        call(
            &app,
            &sid,
            "list_archive",
            json!({"path":"files/input.zip"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert_eq!(app.approvals.pending_count(), 0);
    no_staging(&app);
}

#[tokio::test]
async fn corrupt_second_entry_cleans_only_its_own_incomplete_output() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    let mut bytes = zip_bytes(&[
        ("first.txt", b"first-content"),
        ("nested/second.txt", b"second-content"),
    ]);
    let i = bytes
        .windows(14)
        .position(|b| b == b"second-content")
        .unwrap();
    bytes[i] = b'X';
    std::fs::write(root.join("files/input.zip"), &bytes).unwrap();
    std::fs::write(root.join("files/keep.txt"), "keep").unwrap();
    // Listing checks metadata; extraction must detect the real CRC failure
    // after it has already staged the first entry.
    ok(call(
        &app,
        &sid,
        "list_archive",
        json!({"path":"files/input.zip"}),
    )
    .await);
    let result=decide(&app,&sid,"extract_archive",json!({"path":"files/input.zip","destination":"files/partial","expected_sha256":sha256_bytes(&bytes)}),true,||{}).await;
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().to_lowercase().contains("checksum"));
    assert!(!root.join("files/partial").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("files/keep.txt")).unwrap(),
        "keep"
    );
    no_staging(&app);
}

#[tokio::test]
async fn extraction_rechecks_destination_policy_and_ancestor_symlinks_after_approval() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    let root = &app.workspace.root;
    for mode in ["collision", "lock", "symlink"] {
        let hash = save_zip(&app, &[("notes/readme.txt", b"keep bytes")]);
        let dest = format!("files/{mode}/out");
        std::fs::create_dir(root.join("files").join(mode)).unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let result = decide(
            &app,
            &sid,
            "extract_archive",
            json!({"path":"files/input.zip","destination":dest,"expected_sha256":hash}),
            true,
            || match mode {
                "collision" => {
                    std::fs::create_dir(root.join(&dest)).unwrap();
                    std::fs::write(root.join(&dest).join("user.txt"), "keep").unwrap();
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
                _ => {
                    std::fs::remove_dir(root.join("files/symlink")).unwrap();
                    std::os::unix::fs::symlink(&outside, root.join("files/symlink")).unwrap();
                }
            },
        )
        .await;
        assert_eq!(result.status, "error");
        assert!(!root.join(&dest).join("notes/readme.txt").exists());
        assert!(!outside.join("out").exists());
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
    no_staging(&app);
}

#[tokio::test]
async fn malformed_arguments_research_protections_symlink_sources_and_count_limits() {
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&temp);
    for (tool, args) in [
        ("list_archive", json!({"path":"../outside.zip"})),
        (
            "list_archive",
            json!({"path":"files/input.zip","agent":"fake"}),
        ),
        (
            "create_archive",
            json!({"paths":[],"destination":"files/out.zip"}),
        ),
        (
            "create_archive",
            json!({"paths":["files"],"destination":"files/out.zip"}),
        ),
    ] {
        assert_eq!(call(&app, &sid, tool, args).await.status, "error");
    }
    let outside = temp.path().join("outside.zip");
    std::fs::write(&outside, zip_bytes(&[("file", b"data")])).unwrap();
    std::os::unix::fs::symlink(outside, app.workspace.root.join("files/link.zip")).unwrap();
    assert_eq!(
        call(&app, &sid, "list_archive", json!({"path":"files/link.zip"}))
            .await
            .code
            .as_deref(),
        Some("permission_denied")
    );
    let names: Vec<_> = (0..257).map(|i| format!("f{i}")).collect();
    let entries: Vec<_> = names
        .iter()
        .map(|s| (s.as_str(), b"x".as_slice()))
        .collect();
    save_zip(&app, &entries);
    assert_eq!(
        call(
            &app,
            &sid,
            "list_archive",
            json!({"path":"files/input.zip"})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    let temp = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&temp, Profile::Research);
    let bytes = zip_bytes(&[("file.txt", b"data")]);
    std::fs::write(
        app.workspace.root.join("AGENTS/CHATGPT/notes/input.zip"),
        &bytes,
    )
    .unwrap();
    for destination in ["SHARED/datasets/out", "AGENTS/CLAUDE/notes/out"] {
        assert_eq!(call(&app,&sid,"extract_archive",json!({"path":"AGENTS/CHATGPT/notes/input.zip","destination":destination,"expected_sha256":sha256_bytes(&bytes)})).await.code.as_deref(),Some("permission_denied"));
        assert!(!app.workspace.root.join(destination).exists());
    }
    no_staging(&app);
}

async fn call(app: &App, sid: &str, tool: &str, args: Value) -> ToolResultMessage {
    tools::dispatch(app, Agent::ChatGpt, sid, tool, args).await
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
