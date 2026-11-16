use bridge_core::{provider::Agent, workspace::Profile};
use bridge_native::{
    app::App,
    config::{Config, HomePaths},
    integrations::{self, ServerConfig, ToolGrant},
    tools,
    wire::BridgeEvent,
    workspace::Workspace,
};
use serde_json::json;
use std::sync::Arc;

fn fixture(temp: &tempfile::TempDir) -> Arc<App> {
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
    let workspace = Workspace::initialize_profile(&root, Profile::General).unwrap();
    let mut config = Config::default();
    config.project_root = workspace.root.clone();
    home.save_config(&config).unwrap();
    App::open_with_config(home, config, workspace, true).unwrap()
}

#[tokio::test]
async fn real_rust_stdio_server_discovery_grants_approval_and_result() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp);
    std::fs::write(
        app.workspace.root.join("files/proof.txt"),
        "real MCP file contents\n",
    )
    .unwrap();
    let program = std::path::PathBuf::from(env!("CARGO_BIN_EXE_unified-ai-bridge-native"));
    let server = ServerConfig {
        id: "rust-bridge".into(),
        executable_sha256: bridge_native::fsops::sha256_file(&program).unwrap().0,
        program,
        args: vec![
            "--home".into(),
            app.home.base.display().to_string(),
            "mcp".into(),
        ],
        tools: vec![],
        timeout_secs: 10,
    };
    let app = integrations::save(&app, server.clone()).unwrap();
    let discovered = integrations::discover(&app, &server.id).await.unwrap();
    let read = discovered["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "read_file")
        .unwrap();
    let args = json!({"server_id":"rust-bridge","tool":"read_file","arguments":{"path":"files/proof.txt"}});
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::ChatGpt,
            "test",
            "call_integration_tool",
            args.clone()
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let mut grant = server.clone();
    grant.tools.push(ToolGrant {
        name: "read_file".into(),
        schema_sha256: read["schema_sha256"].as_str().unwrap().into(),
        schema: read["schema"].clone(),
    });
    let app = integrations::save(&app, grant).unwrap();
    let list = tools::dispatch(&app, Agent::ChatGpt, "test", "list_integrations", json!({})).await;
    assert_eq!(list.data.as_ref().unwrap()["tools"][0]["tool"], "read_file");
    let schema = tools::dispatch(
        &app,
        Agent::ChatGpt,
        "test",
        "list_integrations",
        json!({"server_id":"rust-bridge","tool":"read_file"}),
    )
    .await;
    assert_eq!(schema.data.unwrap()["tool"]["schema"], read["schema"]);
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::ChatGpt,
            "test",
            "list_integrations",
            json!({"server_id":"rust-bridge","tool":"ungranted"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert_eq!(
        tools::dispatch(
            &app,
            Agent::ChatGpt,
            "test",
            "list_integrations",
            json!({"unknown":true})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    for allow in [false, true] {
        let mut events = app.events.subscribe();
        let a = app.clone();
        let args = args.clone();
        let task = tokio::spawn(async move {
            tools::dispatch(&a, Agent::ChatGpt, "test", "call_integration_tool", args).await
        });
        loop {
            if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                app.approvals.resolve(&approval.id, allow);
                break;
            }
        }
        let result = task.await.unwrap();
        if allow {
            assert_eq!(result.status, "ok", "{result:?}");
            assert!(result
                .data
                .unwrap()
                .to_string()
                .contains("real MCP file contents"));
        } else {
            assert_eq!(result.code.as_deref(), Some("approval_denied"));
        }
    }
    let mut workspace = app.workspace.clone();
    workspace.read_only = true;
    let locked = app.with_workspace(workspace);
    assert_eq!(
        tools::dispatch(
            &locked,
            Agent::ChatGpt,
            "test",
            "call_integration_tool",
            args.clone()
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let removed = integrations::remove(&app, "rust-bridge").unwrap();
    assert_eq!(
        tools::dispatch(
            &removed,
            Agent::ChatGpt,
            "test",
            "call_integration_tool",
            args
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    assert!(app.store.verify_audit().unwrap().valid);
    let mut bad = server;
    bad.program = "/bin/sh".into();
    bad.executable_sha256 = bridge_native::fsops::sha256_file(&bad.program).unwrap().0;
    assert!(bad.validate().is_err());
    println!("Real Rust MCP subprocess discovered; ungranted and denied calls blocked; approved read returned actual temp file; read-only and removal blocked calls; audit valid");
}

fn lifecycle_program() -> std::path::PathBuf {
    static PROGRAM: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            let result = std::process::Command::new(env!("CARGO"))
                .current_dir(root)
                .args([
                    "build",
                    "--locked",
                    "-p",
                    "bridge-native",
                    "--example",
                    "mcp_lifecycle_probe",
                ])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("examples/mcp_lifecycle_probe")
        })
        .clone()
}
async fn marker(path: &std::path::Path) {
    let end = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < end,
            "missing {}",
            path.display()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
async fn stopped(pid_file: &std::path::Path) {
    let pid: usize = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    for _ in 0..100 {
        // A killed zombie awaits reaping but cannot perform more work.
        let result = std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&result.stdout);
        if !result.status.success() || state.trim().is_empty() || state.contains('Z') {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("integration process {pid} still running");
}
fn probe(app: &Arc<App>, markers: &std::path::Path, mode: &str, timeout: u64) -> Arc<App> {
    let schema = json!({"type":"object","properties":{},"additionalProperties":false});
    integrations::save(
        app,
        ServerConfig {
            id: "probe".into(),
            program: lifecycle_program(),
            args: vec![markers.display().to_string(), mode.into()],
            executable_sha256: String::new(),
            tools: vec![ToolGrant {
                name: "wait".into(),
                schema_sha256: bridge_native::fsops::sha256_bytes(schema.to_string().as_bytes()),
                schema,
            }],
            timeout_secs: timeout,
        },
    )
    .unwrap()
}
#[tokio::test]
async fn removal_during_initialization_and_discovery_stops_the_process_before_any_call() {
    for mode in ["initialize", "discovery"] {
        let temp = tempfile::tempdir().unwrap();
        let app = fixture(&temp);
        let markers = temp.path().join("markers");
        std::fs::create_dir(&markers).unwrap();
        let app = probe(&app, &markers, mode, 10);
        let a = app.clone();
        let pending = tokio::spawn(async move { integrations::discover(&a, "probe").await });
        marker(&markers.join(if mode == "initialize" {
            "initializing"
        } else {
            "discovering"
        }))
        .await;
        integrations::remove(&app, "probe").unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(4), pending)
            .await
            .unwrap()
            .unwrap();
        assert!(result.is_err());
        assert!(!markers.join("called").exists());
        stopped(&markers.join("server.pid")).await;
    }
}
#[tokio::test]
async fn active_call_removal_lock_and_timeout_stop_inherited_process_groups_and_report_uncertainty()
{
    for action in ["remove", "lock", "timeout"] {
        let temp = tempfile::tempdir().unwrap();
        let app = fixture(&temp);
        let markers = temp.path().join("markers");
        std::fs::create_dir(&markers).unwrap();
        let app = probe(
            &app,
            &markers,
            "call",
            if action == "timeout" { 1 } else { 10 },
        );
        let mut events = app.events.subscribe();
        let a = app.clone();
        let pending = tokio::spawn(async move {
            tools::dispatch(
                &a,
                Agent::ChatGpt,
                "test",
                "call_integration_tool",
                json!({"server_id":"probe","tool":"wait","arguments":{}}),
            )
            .await
        });
        loop {
            if let BridgeEvent::ApprovalRequired { approval } = events.recv().await.unwrap() {
                assert!(app.approvals.resolve(&approval.id, true));
                break;
            }
        }
        marker(&markers.join("child.pid")).await;
        match action {
            "remove" => {
                integrations::remove(&app, "probe").unwrap();
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
        }
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.status, "error", "{result:?}");
        assert!(result.error.unwrap().contains("uncertain"));
        assert!(!markers.join("completed").exists());
        stopped(&markers.join("server.pid")).await;
        stopped(&markers.join("child.pid")).await;
        assert!(app.store.verify_audit().unwrap().valid);
    }
}
#[tokio::test]
async fn readiness_is_unprobed_and_invalid_grants_cannot_start_processes() {
    let temp = tempfile::tempdir().unwrap();
    let app = fixture(&temp);
    let markers = temp.path().join("markers");
    std::fs::create_dir(&markers).unwrap();
    let app = probe(&app, &markers, "call", 10);
    let state = tools::dispatch(
        &app,
        Agent::ChatGpt,
        "test",
        "integration_status",
        json!({"server_id":"probe"}),
    )
    .await;
    assert_eq!(state.status, "ok");
    assert_eq!(state.data.unwrap()["state"], "configured_unprobed");
    assert!(!markers.join("server.pid").exists());
    let oversized = tools::dispatch(
        &app,
        Agent::ChatGpt,
        "test",
        "call_integration_tool",
        json!({"server_id":"probe","tool":"wait","arguments":{"large":"x".repeat(32001)}}),
    )
    .await;
    assert_eq!(oversized.code.as_deref(), Some("invalid_arguments"));
    assert_eq!(app.approvals.pending_count(), 0);
    for args in [
        json!({"server_id":"missing"}),
        json!({"server_id":"probe","identity":"fake"}),
    ] {
        assert_eq!(
            tools::dispatch(&app, Agent::ChatGpt, "test", "integration_status", args)
                .await
                .status,
            "error"
        );
    }
    let mut config = app.home.load_config().unwrap();
    config
        .workspace_integrations
        .get_mut(&app.workspace.id())
        .unwrap()[0]
        .executable_sha256 = "0".repeat(64);
    app.home.save_config(&config).unwrap();
    assert_eq!(
        integrations::health(&app, "probe").unwrap()["state"],
        "blocked"
    );
    assert!(integrations::discover(&app, "probe").await.is_err());
    assert!(!markers.join("server.pid").exists());
}
