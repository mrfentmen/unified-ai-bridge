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
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
fn parts(formula: bool) -> BTreeMap<String, String> {
    let f = if formula { "<f>A1+1</f>" } else { "" };
    [
        ("[Content_Types].xml".into(),"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/></Types>".into()),
        ("_rels/.rels".into(),"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".into()),
        ("xl/workbook.xml".into(),format!("<workbook xmlns=\"{S}\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Untouched\" sheetId=\"2\" r:id=\"rId2\" state=\"hidden\"/></sheets></workbook>")),
        ("xl/_rels/workbook.xml.rels".into(),"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet2.xml\"/></Relationships>".into()),
        ("xl/sharedStrings.xml".into(),format!("<sst xmlns=\"{S}\" count=\"1\" uniqueCount=\"1\"><si><t>Καλημέρα &amp; shared</t></si></sst>")),
        ("xl/styles.xml".into(),format!("<styleSheet xmlns=\"{S}\"><fonts count=\"2\"><font/><font><b/></font></fonts><fills count=\"1\"><fill/></fills><borders count=\"1\"><border/></borders><cellXfs count=\"2\"><xf numFmtId=\"0\" fontId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\"/></cellXfs></styleSheet>")),
        ("xl/worksheets/sheet1.xml".into(),format!("<worksheet xmlns=\"{S}\"><dimension ref=\"A1:C2\"/><sheetData><row r=\"1\"><c r=\"A1\" s=\"1\" t=\"s\"><v>0</v></c><c r=\"B1\">{f}<v>12.5</v></c><c r=\"C1\" t=\"b\"><v>1</v></c></row><row r=\"2\"><c r=\"A2\"/><c r=\"B2\" t=\"inlineStr\"><is><t>original</t></is></c><c r=\"C2\" t=\"b\"><v>0</v></c></row></sheetData></worksheet>")),
        ("xl/worksheets/sheet2.xml".into(),format!("<worksheet xmlns=\"{S}\"><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Untouched evidence</t></is></c></row></sheetData></worksheet>")),
    ].into_iter().collect()
}
fn zip(parts: &BTreeMap<String, String>) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in parts {
        z.start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        z.write_all(text.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}
fn payloads(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data).unwrap();
        out.insert(name, data);
    }
    out
}
fn save(app: &App, parts: &BTreeMap<String, String>) -> String {
    let b = zip(parts);
    std::fs::write(app.workspace.root.join("files/input.xlsx"), &b).unwrap();
    sha256_bytes(&b)
}
fn edit_args(hash: &str, name: &str, values: Value) -> Value {
    json!({"path":"files/input.xlsx","destination":format!("output/{name}.xlsx"),"sheet":"Data","range":"A1:C2","values":values,"expected_sha256":hash})
}
fn values() -> Value {
    json!([
        ["=literal, not a formula", 19.25, true],
        [null, "_x0041_ & <é>\r\n", false]
    ])
}

#[tokio::test]
async fn reads_typed_values_formula_caches_visibility_and_sparse_cells_without_recalculation() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let hash = save(&app, &parts(true));
    let meta = ok(call(
        &app,
        &sid,
        "inspect_workbook",
        json!({"path":"files/input.xlsx"}),
    )
    .await);
    assert_eq!(meta["sheets"][1]["visibility"], "hidden");
    assert_eq!(meta["sheets"][0]["stored_cells"], 6);
    assert_eq!(meta["edit_supported"], false);
    let read = ok(call(
        &app,
        &sid,
        "read_sheet_range",
        json!({"path":"files/input.xlsx","sheet":"Data","range":"A1:C3","expected_sha256":hash}),
    )
    .await);
    let cells = &read["rows"][0]["cells"];
    assert_eq!(cells[0]["value"], "Καλημέρα & shared");
    assert_eq!(cells[1]["value"], 12.5);
    assert_eq!(cells[1]["cached"], true);
    assert_eq!(cells[1]["formula"]["text"], "A1+1");
    assert_eq!(cells[2]["value"], true);
    assert_eq!(read["rows"][2]["cells"][0]["type"], "empty");
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/input.xlsx")).unwrap(),
        zip(&parts(true))
    );
}
#[tokio::test]
async fn approved_literal_edits_reopen_and_preserve_styles_original_and_untouched_parts() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let original = zip(&parts(false));
    let hash = save(&app, &parts(false));
    let changed = ok(decide(
        &app,
        &sid,
        "write_sheet_range",
        edit_args(&hash, "edited", values()),
        true,
        || {},
    )
    .await);
    assert_eq!(changed["changes"].as_array().unwrap().len(), 6);
    assert_eq!(changed["state"], "completed");
    let output = std::fs::read(app.workspace.root.join("output/edited.xlsx")).unwrap();
    assert_eq!(changed["sha256"], sha256_bytes(&output));
    let before = payloads(&original);
    let after = payloads(&output);
    for (name, bytes) in &before {
        if name != "xl/worksheets/sheet1.xml" {
            assert_eq!(after[name], *bytes, "{name}");
        }
    }
    let xml = std::str::from_utf8(&after["xl/worksheets/sheet1.xml"]).unwrap();
    let d = roxmltree::Document::parse(xml).unwrap();
    let a1 = d
        .descendants()
        .find(|n| n.attribute("r") == Some("A1"))
        .unwrap();
    assert_eq!(a1.attribute("s"), Some("1"));
    assert_eq!(a1.attribute("t"), Some("inlineStr"));
    assert!(!d.descendants().any(|n| n.tag_name().name() == "f"));
    let read = ok(call(
        &app,
        &sid,
        "read_sheet_range",
        json!({"path":"output/edited.xlsx","sheet":"Data","range":"A1:C2"}),
    )
    .await);
    for (row, vals) in values().as_array().unwrap().iter().enumerate() {
        for (col, v) in vals.as_array().unwrap().iter().enumerate() {
            assert_eq!(read["rows"][row]["cells"][col]["value"], *v);
        }
    }
    assert_eq!(
        std::fs::read(app.workspace.root.join("files/input.xlsx")).unwrap(),
        original
    );
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn denied_and_changed_approvals_never_publish_or_clobber_workbooks() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for mode in ["deny", "source", "destination", "lock", "symlink"] {
        let hash = save(&app, &parts(false));
        let dest = app.workspace.root.join(format!("output/{mode}.xlsx"));
        let r = decide(
            &app,
            &sid,
            "write_sheet_range",
            edit_args(&hash, mode, values()),
            mode != "deny",
            || match mode {
                "source" => {
                    save(&app, &parts(true));
                }
                "destination" => std::fs::write(&dest, "keep").unwrap(),
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
                "symlink" => std::os::unix::fs::symlink(t.path().join("outside"), &dest).unwrap(),
                _ => {}
            },
        )
        .await;
        assert_eq!(r.status, "error", "{mode}: {r:?}");
        if mode == "destination" {
            assert_eq!(std::fs::read(&dest).unwrap(), b"keep");
        } else {
            assert!(!dest.is_file());
        }
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
    assert!(!t.path().join("outside").exists());
    assert!(app.store.verify_audit().unwrap().valid);
}
#[tokio::test]
async fn unsupported_formulas_merges_links_rich_text_and_missing_cells_are_never_flattened() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for mode in ["formulas", "merges", "links", "rich", "missing", "metadata"] {
        let mut p = parts(mode == "formulas");
        match mode {
            "merges" => {
                let s = p.get_mut("xl/worksheets/sheet1.xml").unwrap();
                *s = s.replace(
                    "</worksheet>",
                    "<mergeCells><mergeCell ref=\"A1:B1\"/></mergeCells></worksheet>",
                );
            }
            "links" => {
                let s = p.get_mut("_rels/.rels").unwrap();
                *s=s.replace("</Relationships>","<Relationship Id=\"link\" TargetMode=\"External\" Target=\"http://127.0.0.1/never-opened\" Type=\"external\"/></Relationships>");
            }
            "rich" => {
                let s = p.get_mut("xl/sharedStrings.xml").unwrap();
                *s = s.replace(
                    "<t>Καλημέρα &amp; shared</t>",
                    "<r><rPr><b/></rPr><t>Rich text</t></r>",
                );
            }
            "missing" => {
                let s = p.get_mut("xl/worksheets/sheet1.xml").unwrap();
                *s = s.replace("<c r=\"A2\"/>", "");
            }
            "metadata" => {
                let s = p.get_mut("xl/worksheets/sheet1.xml").unwrap();
                *s = s.replace("<c r=\"A2\"/>", "<c r=\"A2\" cm=\"1\"/>");
            }
            _ => {}
        }
        let hash = save(&app, &p);
        let r = call(
            &app,
            &sid,
            "write_sheet_range",
            edit_args(&hash, mode, values()),
        )
        .await;
        assert_eq!(r.code.as_deref(), Some("unsupported"), "{mode}: {r:?}");
        assert!(!app
            .workspace
            .root
            .join(format!("output/{mode}.xlsx"))
            .exists());
        assert_eq!(
            sha256_bytes(&std::fs::read(app.workspace.root.join("files/input.xlsx")).unwrap()),
            hash
        );
    }
}
#[tokio::test]
async fn hostile_packages_declared_counts_duplicate_cells_and_result_budgets_fail_explicitly() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    for mode in [
        "traversal",
        "macro",
        "dtd",
        "count",
        "duplicate",
        "huge_text",
    ] {
        let mut p = parts(false);
        match mode {
            "traversal" => {
                p.insert("../escape".into(), "bad".into());
            }
            "macro" => {
                p.insert("xl/vbaProject.bin".into(), "bad".into());
            }
            "dtd" => {
                let s = p.get_mut("xl/sharedStrings.xml").unwrap();
                *s = format!("<!DOCTYPE sst [<!ENTITY x SYSTEM 'file:///private/secret'>]>{s}");
            }
            "count" => {
                let s = p.get_mut("xl/sharedStrings.xml").unwrap();
                *s = s.replace("uniqueCount=\"1\"", "uniqueCount=\"18446744073709551615\"");
            }
            "duplicate" => {
                let s = p.get_mut("xl/worksheets/sheet1.xml").unwrap();
                *s = s.replace("r=\"B1\"", "r=\"A1\"");
            }
            "huge_text" => {
                let s = p.get_mut("xl/sharedStrings.xml").unwrap();
                *s = s.replace("Καλημέρα &amp; shared", &"x".repeat(8193));
            }
            _ => {}
        }
        save(&app, &p);
        let r = call(
            &app,
            &sid,
            "read_sheet_range",
            json!({"path":"files/input.xlsx","sheet":"Data","range":"A1:C2"}),
        )
        .await;
        assert_eq!(r.status, "error", "{mode}: {r:?}");
    }
    let mut many = parts(false);
    let features = (0..1000)
        .map(|n| format!("<unsupported{n}{}/>", "x".repeat(200)))
        .collect::<String>();
    let sheet = many.get_mut("xl/worksheets/sheet1.xml").unwrap();
    *sheet = sheet.replace("</worksheet>", &format!("{features}</worksheet>"));
    save(&app, &many);
    let bounded = ok(call(
        &app,
        &sid,
        "inspect_workbook",
        json!({"path":"files/input.xlsx"}),
    )
    .await);
    assert!(!bounded["edit_supported"].as_bool().unwrap());
    assert!(bounded.to_string().len() < 10000);
    save(&app, &parts(false));
    ok(call(
        &app,
        &sid,
        "inspect_workbook",
        json!({"path":"files/input.xlsx"}),
    )
    .await);
    for range in [
        "a1",
        "$A$1",
        "XFE1",
        "A0",
        "A1:XFD1048576",
        "B2:A1",
        "A1:A401",
    ] {
        assert_eq!(
            call(
                &app,
                &sid,
                "read_sheet_range",
                json!({"path":"files/input.xlsx","sheet":"Data","range":range})
            )
            .await
            .code
            .as_deref(),
            Some("invalid_arguments")
        );
    }
}
#[tokio::test]
async fn private_link_research_stale_unknown_and_invalid_literal_requests_are_denied() {
    let _serial = TEST_LOCK.lock().await;
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture(&t);
    let hash = save(&app, &parts(false));
    std::fs::write(app.workspace.root.join(".env"), "private").unwrap();
    std::os::unix::fs::symlink(t.path(), app.workspace.root.join("files/escape")).unwrap();
    for tool in ["inspect_workbook", "read_sheet_range", "write_sheet_range"] {
        for path in [".env", "../outside.xlsx", "files/escape/out.xlsx"] {
            let mut args = match tool {
                "write_sheet_range" => edit_args(&hash, "bad", values()),
                "read_sheet_range" => json!({"sheet":"Data","range":"A1"}),
                _ => json!({}),
            };
            args["path"] = json!(path);
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
            "inspect_workbook",
            json!({"path":"files/input.xlsx","agent":"fake"})
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
            "read_sheet_range",
            json!({"path":"files/input.xlsx","sheet":"Data","range":"A1","expected_sha256":"stale"})
        )
        .await
        .code
        .as_deref(),
        Some("file_changed")
    );
    for vals in [
        json!([]),
        json!([[{}]]),
        json!([["\u{0}"]]),
        json!([[1e30]]),
    ] {
        let mut args = edit_args(&hash, "invalid", vals);
        args["range"] = json!("A1");
        assert_eq!(
            call(&app, &sid, "write_sheet_range", args).await.status,
            "error"
        );
    }
    let t = tempfile::tempdir().unwrap();
    let (app, sid) = fixture_profile(&t, Profile::Research);
    let source = "AGENTS/CHATGPT/notes/input.xlsx";
    let bytes = zip(&parts(false));
    std::fs::write(app.workspace.root.join(source), &bytes).unwrap();
    for dest in ["SHARED/datasets/out.xlsx", "AGENTS/CLAUDE/notes/out.xlsx"] {
        assert_eq!(call(&app,&sid,"write_sheet_range",json!({"path":source,"destination":dest,"sheet":"Data","range":"A1","values":[[1]],"expected_sha256":sha256_bytes(&bytes)})).await.code.as_deref(),Some("permission_denied"));
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
