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
    .expect("binary tool should finish or request approval promptly")
}

use std::io::{Cursor, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};
static PARSERS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const TYPES: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
fn fixture(t: &tempfile::TempDir) -> (Arc<App>, String) {
    fixture_profile(t, Profile::General)
}
fn ok(r: ToolResultMessage) -> Value {
    assert_eq!(r.status, "ok", "{r:?}");
    r.data.unwrap()
}
fn package(main: &str, parts: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(vec![]));
    for (name, text) in [("[Content_Types].xml", TYPES), ("word/document.xml", main)]
        .into_iter()
        .chain(parts.iter().copied())
    {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
fn body(inner: &str) -> String {
    format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><w:body>{inner}</w:body></w:document>"#
    )
}
fn save(app: &App, main: &str, parts: &[(&str, &str)]) {
    std::fs::write(
        app.workspace.root.join("files/example.docx"),
        package(main, parts),
    )
    .unwrap();
}
#[tokio::test]
async fn main_body_chunks_preserve_unicode_and_report_omitted_parts_without_inventing_pages() {
    let _serial = PARSERS.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let text = "Καλημέρα &amp; document ".repeat(80);
    save(&app,&body(&format!("<w:p><w:r><w:t>{text}</w:t><w:tab/><w:t>tail</w:t><w:br w:type=\"page\"/></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>")),&[("word/header1.xml","OMITTED HEADER")]);
    let info = ok(call(
        &app,
        &sid,
        "document_info",
        json!({"path":"files/example.docx"}),
    )
    .await);
    assert_eq!(info["body_paragraphs"], 2);
    assert_eq!(info["tables"], 1);
    assert_eq!(info["explicit_page_breaks"], 1);
    assert!(info["page_count"].is_null());
    assert_eq!(info["omitted_parts"][0], "word/header1.xml");
    let mut output = String::new();
    let mut offset = 0;
    loop {
        let args = if offset == 0 {
            json!({"path":"files/example.docx","max_bytes":256})
        } else {
            json!({"path":"files/example.docx","max_bytes":256,"offset":offset,"expected_sha256":info["sha256"]})
        };
        let page = ok(call(&app, &sid, "extract_document_text", args).await);
        output.push_str(page["text"].as_str().unwrap());
        if let Some(next) = page["next_offset"].as_u64() {
            assert!(next > offset);
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(
        output,
        format!("{}\ttail\n\ncell\n", "Καλημέρα & document ".repeat(80))
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn revisions_fields_alternatives_and_relationships_are_explicit_inert_text() {
    let _serial = PARSERS.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let content=body("<w:p><w:r><w:t>Visible</w:t></w:r><w:del><w:r><w:delText>Deleted secret</w:delText></w:r></w:del><w:ins><w:r><w:t>Inserted</w:t></w:r></w:ins><w:fldSimple w:instr=\"INCLUDETEXT /private/secret\"><w:r><w:t>Cached</w:t></w:r></w:fldSimple><w:hyperlink><w:r><w:t>Link label</w:t></w:r></w:hyperlink><mc:AlternateContent><mc:Choice Requires=\"w\"><w:r><w:t>Ambiguous</w:t></w:r></mc:Choice></mc:AlternateContent></w:p>");
    save(&app,&content,&[("word/_rels/document.xml.rels","<Relationships><Relationship Target=\"file:///private/secret\" TargetMode=\"External\"/></Relationships>")]);
    let data = ok(call(
        &app,
        &sid,
        "extract_document_text",
        json!({"path":"files/example.docx"}),
    )
    .await);
    assert_eq!(data["text"], "VisibleInsertedCachedLink label\n");
    assert_eq!(data["tracked_changes"], true);
    assert_eq!(data["cached_fields"], true);
    assert_eq!(data["alternate_content_skipped"], true);
    assert!(!data.to_string().contains("/private/secret"));
}
#[tokio::test]
async fn hostile_xml_and_packages_fail_without_losing_the_host() {
    let _serial = PARSERS.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let dtd = "<!DOCTYPE doc [<!ENTITY x SYSTEM 'file:///private/secret'>]>";
    for main in [
        format!("{dtd}{}", body("<w:p><w:r><w:t>&x;</w:t></w:r></w:p>")),
        body("<w:p>"),
        body(&format!("{}{}", "<w:p>".repeat(500), "</w:p>".repeat(500))),
        body(&format!(
            "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            "x".repeat(256001)
        )),
    ] {
        save(&app, &main, &[]);
        assert_eq!(
            call(
                &app,
                &sid,
                "document_info",
                json!({"path":"files/example.docx"})
            )
            .await
            .status,
            "error"
        );
    }
    for parts in [
        vec![("../outside", "escape")],
        vec![("word/vbaProject.bin", "macro")],
    ] {
        save(&app, &body("<w:p/>"), &parts);
        assert_eq!(
            call(
                &app,
                &sid,
                "document_info",
                json!({"path":"files/example.docx"})
            )
            .await
            .status,
            "error"
        );
    }
    save(
        &app,
        &body("<w:p><w:r><w:t>Still available</w:t></w:r></w:p>"),
        &[],
    );
    assert_eq!(
        ok(call(
            &app,
            &sid,
            "extract_document_text",
            json!({"path":"files/example.docx"})
        )
        .await)["text"],
        "Still available\n"
    );
}
#[tokio::test]
async fn stale_utf8_offsets_private_links_and_unsupported_formats_are_refused() {
    let _serial = PARSERS.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    save(&app, &body("<w:p><w:r><w:t>ééé</w:t></w:r></w:p>"), &[]);
    let info = ok(call(
        &app,
        &sid,
        "document_info",
        json!({"path":"files/example.docx"}),
    )
    .await);
    assert_eq!(
        call(
            &app,
            &sid,
            "extract_document_text",
            json!({"path":"files/example.docx","offset":1,"expected_sha256":info["sha256"]})
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
            json!({"path":"files/example.docx","offset":2})
        )
        .await
        .code
        .as_deref(),
        Some("invalid_arguments")
    );
    save(&app, &body("<w:p><w:r><w:t>changed</w:t></w:r></w:p>"), &[]);
    assert_eq!(
        call(
            &app,
            &sid,
            "extract_document_text",
            json!({"path":"files/example.docx","offset":2,"expected_sha256":info["sha256"]})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    std::fs::write(app.workspace.root.join(".env"), "private").unwrap();
    std::fs::write(app.workspace.root.join("files/example.pdf"), "%PDF").unwrap();
    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("files/escape")).unwrap();
    for tool in ["document_info", "extract_document_text"] {
        for path in [".env", "../outside.docx", "files/escape/outside.docx"] {
            assert_eq!(
                call(&app, &sid, tool, json!({"path":path}))
                    .await
                    .code
                    .as_deref(),
                Some("permission_denied")
            );
        }
        assert_eq!(
            call(&app, &sid, tool, json!({"path":"files/example.pdf"}))
                .await
                .code
                .as_deref(),
            Some("unsupported")
        );
        assert_eq!(
            call(
                &app,
                &sid,
                tool,
                json!({"path":"files/example.docx","identity":"fake"})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
}
#[tokio::test]
async fn control_heavy_text_stays_inside_serialized_response_budget() {
    let _serial = PARSERS.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let text = "&#9;".repeat(24000);
    save(
        &app,
        &body(&format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")),
        &[],
    );
    let result = call(
        &app,
        &sid,
        "extract_document_text",
        json!({"path":"files/example.docx","max_bytes":24000}),
    )
    .await;
    assert_eq!(result.status, "ok", "{result:?}");
    assert!(result.output.unwrap().len() <= 48000);
    let data = result.data.unwrap();
    assert!(data["next_offset"].is_u64());
    assert_eq!(data["truncated"], true);
}
