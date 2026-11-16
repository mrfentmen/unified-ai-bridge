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
use std::sync::Arc;
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

fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
}
fn ok(r: ToolResultMessage) -> Value {
    assert_eq!(r.status, "ok", "{r:?}");
    r.data.unwrap()
}
#[tokio::test]
async fn quoted_multiline_unicode_tables_page_by_record_and_hash() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let path = app.workspace.root.join("files/people.csv");
    std::fs::write(&path,"name,note,amount\nAlice,\"first, quoted\",10\nΒήτα,\"two\nlines\",20.5\nCarol,=SUM(A1:A2),\n").unwrap();
    let first = ok(call(
        &app,
        &sid,
        "read_table",
        json!({"path":"files/people.csv","limit":1}),
    )
    .await);
    assert_eq!(first["rows"][0]["line"], 2);
    assert_eq!(first["rows"][0]["values"][1], "first, quoted");
    assert_eq!(first["next_offset"], 1);
    assert_eq!(first["total_rows"], 3);
    let second=ok(call(&app,&sid,"read_table",json!({"path":"files/people.csv","columns":["note","name"],"offset":1,"limit":1,"expected_sha256":first["sha256"]})).await);
    assert_eq!(second["rows"][0]["row"], 2);
    assert_eq!(second["rows"][0]["line"], 3);
    assert_eq!(second["rows"][0]["values"][0], "two\nlines");
    let last = ok(call(
        &app,
        &sid,
        "read_table",
        json!({"path":"files/people.csv","offset":2,"expected_sha256":first["sha256"]}),
    )
    .await);
    assert_eq!(last["rows"][0]["line"], 5);
    assert_eq!(last["rows"][0]["values"][1], "=SUM(A1:A2)");
    assert!(last["next_offset"].is_null());
    std::fs::write(&path, "name,note,amount\nChanged,x,1\n").unwrap();
    assert_eq!(
        call(
            &app,
            &sid,
            "read_table",
            json!({"path":"files/people.csv","offset":1,"expected_sha256":first["sha256"]})
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
            "read_table",
            json!({"path":"files/people.csv","offset":1})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn structured_filters_and_complete_aggregates_explain_excluded_values() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    std::fs::write(app.workspace.root.join("files/values.tsv"),"group\tamount\tlabel\na\t10\tfirst\na\t20\tsecond\na\t\tempty\na\tbad\tinvalid\na\tNaN\tnotfinite\nb\t999\texcluded\n").unwrap();
    let q=ok(call(&app,&sid,"query_table",json!({"path":"files/values.tsv","filters":[{"op":"equals","column":"group","value":"a"}],"aggregate_columns":["amount"],"limit":1})).await);
    assert_eq!(q["matched_rows"], 5);
    assert_eq!(q["aggregates"][0]["sum"], 30.0);
    assert_eq!(q["aggregates"][0]["mean"], 15.0);
    assert_eq!(q["aggregates"][0]["numeric_count"], 2);
    assert_eq!(q["aggregates"][0]["empty_count"], 1);
    assert_eq!(q["aggregates"][0]["non_numeric_count"], 2);
    let filtered=ok(call(&app,&sid,"query_table",json!({"path":"files/values.tsv","filters":[{"op":"number_range","column":"amount","min":10,"max":20},{"op":"contains","column":"label","value":"second"}],"aggregate_columns":["amount"]})).await);
    assert_eq!(filtered["matched_rows"], 1);
    assert_eq!(filtered["aggregates"][0]["sum"], 20.0);
    let empty=ok(call(&app,&sid,"query_table",json!({"path":"files/values.tsv","filters":[{"op":"is_empty","column":"amount"}],"aggregate_columns":["amount"]})).await);
    assert_eq!(empty["matched_rows"], 1);
    assert!(empty["aggregates"][0]["sum"].is_null());
    for bad in [
        json!({"op":"number_range","column":"amount"}),
        json!({"op":"number_range","column":"amount","min":2,"max":1}),
        json!({"op":"equals","column":"missing","value":"x"}),
        json!({"op":"equals","column":"group","value":"a","sql":"drop"}),
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "query_table",
                json!({"path":"files/values.tsv","filters":[bad]})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
}
#[tokio::test]
async fn malformed_or_oversized_tables_never_produce_partial_aggregates() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let file = app.workspace.root.join("files/data.csv");
    for content in [
        "a,a\n1,2\n".into(),
        "a,b\n1\n".into(),
        "a\n\0\n".into(),
        format!("a\n{}\n", "x".repeat(8193)),
        format!("a\n{}", "1\n".repeat(10001)),
        "a\n1e308\n1e308\n".into(),
    ] {
        std::fs::write(&file, content).unwrap();
        assert_eq!(
            call(
                &app,
                &sid,
                "query_table",
                json!({"path":"files/data.csv","aggregate_columns":["a"]})
            )
            .await
            .status,
            "error"
        );
    }
    std::fs::write(&file, b"a\n\xff\n").unwrap();
    assert_eq!(
        call(&app, &sid, "read_table", json!({"path":"files/data.csv"}))
            .await
            .status,
        "error"
    );
    std::fs::write(&file, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        call(&app, &sid, "read_table", json!({"path":"files/data.csv"}))
            .await
            .status,
        "error"
    );
}
#[tokio::test]
async fn result_bytes_are_bounded_and_projection_recovers_an_oversized_row() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let file = app.workspace.root.join("files/data.csv");
    let line = format!(
        "{},{},{},{}\n",
        "a".repeat(8000),
        "b".repeat(8000),
        "c".repeat(8000),
        "d".repeat(8000)
    );
    std::fs::write(&file, format!("a,b,c,d\n{line}{line}")).unwrap();
    assert_eq!(
        call(&app, &sid, "read_table", json!({"path":"files/data.csv"}))
            .await
            .code
            .as_deref(),
        Some("unsupported")
    );
    let first = ok(call(
        &app,
        &sid,
        "read_table",
        json!({"path":"files/data.csv","columns":["a","b"]}),
    )
    .await);
    assert_eq!(first["rows"].as_array().unwrap().len(), 1);
    assert_eq!(first["next_offset"], 1);
    assert!(first.to_string().len() < 25000);
    let next=ok(call(&app,&sid,"read_table",json!({"path":"files/data.csv","columns":["a","b"],"offset":1,"expected_sha256":first["sha256"]})).await);
    assert!(next["next_offset"].is_null());
}
#[tokio::test]
async fn private_traversal_linked_and_unknown_inputs_are_denied_in_all_profiles() {
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let root = &app.workspace.root;
    std::fs::write(root.join(".env"), "a\n1\n").unwrap();
    std::fs::write(t.path().join("outside.csv"), "a\n1\n").unwrap();
    std::os::unix::fs::symlink(t.path().join("outside.csv"), root.join("files/link.csv")).unwrap();
    std::fs::write(root.join("files/real.csv"), "a\n1\n").unwrap();
    std::fs::hard_link(root.join("files/real.csv"), root.join("files/hard.csv")).unwrap();
    for tool in ["read_table", "query_table"] {
        for path in [".env", "../outside.csv", "files/link.csv", "files/hard.csv"] {
            assert_eq!(
                call(&app, &sid, tool, json!({"path":path,"delimiter":"comma"}))
                    .await
                    .code
                    .as_deref(),
                Some("permission_denied")
            );
        }
        assert_eq!(
            call(
                &app,
                &sid,
                tool,
                json!({"path":"files/real.csv","agent":"fake"})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
    let r = tempfile::tempdir().unwrap();
    let (research, sid) = fixture_profile(&r, Profile::Research);
    std::fs::write(
        research
            .workspace
            .root
            .join("AGENTS/CLAUDE/notes/secret.csv"),
        "a\n1\n",
    )
    .unwrap();
    assert_eq!(
        call(
            &research,
            &sid,
            "read_table",
            json!({"path":"AGENTS/CLAUDE/notes/secret.csv"})
        )
        .await
        .code
        .as_deref(),
        Some("permission_denied")
    );
    let dataset = research.workspace.root.join("SHARED/datasets/test.csv");
    std::fs::write(&dataset, "a\n1\n").unwrap();
    ok(call(
        &research,
        &sid,
        "read_table",
        json!({"path":"SHARED/datasets/test.csv"}),
    )
    .await);
    assert_eq!(std::fs::read_to_string(dataset).unwrap(), "a\n1\n");
}
