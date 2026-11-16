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
