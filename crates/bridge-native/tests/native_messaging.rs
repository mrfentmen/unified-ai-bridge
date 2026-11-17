//! End-to-end Native Messaging framing test.
//!
//! Chrome communicates with the host over stdin/stdout using a 4-byte
//! little-endian length prefix, and on macOS it passes the calling extension's
//! origin as the first argument. This test speaks that same wire protocol to the
//! real compiled binary: hello, register a ChatGPT session, write a Markdown
//! document, read it back, and request status.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{json, Value};

struct HostProcess {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Result<Value, String>>,
    workspace_id: String,
}

impl HostProcess {
    fn register(&mut self) -> String {
        self.send(&json!({"type":"hello", "protocol":bridge_core::PROTOCOL_VERSION, "extension_version":"reliability-test"}));
        self.receive_until("hello_ack");
        self.send(
            &json!({"type":"register_session", "provider":"chatgpt", "tab_id":77,
            "url":"https://chatgpt.com/c/reliability", "conversation":"reliability"}),
        );
        let registered = self.receive_until("session_registered");
        self.workspace_id = registered["session"]["workspace_id"]
            .as_str()
            .unwrap()
            .to_string();
        registered["session"]["id"].as_str().unwrap().to_string()
    }

    fn tool(
        &mut self,
        session: &str,
        response: &str,
        index: u32,
        tool: &str,
        arguments: Value,
    ) -> Value {
        self.send(&json!({"type":"tool_request", "session_id":session,
            "workspace_id":self.workspace_id,
            "request_key":bridge_core::execution::request_key("chatgpt", "reliability", response, index),
            "response_id":response, "task_id":"user-task", "call_index":index, "tool":tool, "arguments":arguments}));
        self.receive_until("tool_result")["result"].clone()
    }

    fn start(binary: &Path, home: &Path, extra_args: &[&str]) -> Self {
        let mut child = Command::new(binary)
            .args(extra_args)
            .env("UAB_HOME", home)
            .env("UAB_NATIVE_HOSTS_DIR", home.join("NativeMessagingHosts"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn native host");
        let stdout = child.stdout.take().expect("host stdout");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdout = stdout;
            loop {
                let mut length = [0u8; 4];
                if stdout.read_exact(&mut length).is_err() {
                    break;
                }
                let length = u32::from_le_bytes(length) as usize;
                if length == 0 || length > 16 * 1024 * 1024 {
                    let _ = sender.send(Err(format!("invalid frame length {length}")));
                    break;
                }
                let mut body = vec![0u8; length];
                if stdout.read_exact(&mut body).is_err() {
                    break;
                }
                let value = serde_json::from_slice::<Value>(&body)
                    .map_err(|err| format!("host emitted invalid JSON: {err}"));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take().expect("host stdin");
        Self {
            child,
            stdin,
            messages: receiver,
            workspace_id: String::new(),
        }
    }

    fn send(&mut self, message: &Value) {
        let body = serde_json::to_vec(message).expect("serialize frame");
        self.stdin
            .write_all(&(body.len() as u32).to_le_bytes())
            .expect("write frame length");
        self.stdin.write_all(&body).expect("write frame body");
        self.stdin.flush().expect("flush frame");
    }

    fn receive_until(&mut self, expected: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("timed out waiting for host message `{expected}`");
            }
            match self.messages.recv_timeout(remaining) {
                Ok(Ok(value)) => {
                    if value.get("type").and_then(Value::as_str) == Some(expected) {
                        return value;
                    }
                    if value["type"] == "event" && value["event"]["type"] == expected {
                        return value["event"].clone();
                    }
                    if value.get("type").and_then(Value::as_str) == Some("error") {
                        panic!("host returned an error instead of {expected}: {value}");
                    }
                }
                Ok(Err(err)) => panic!("{err}"),
                Err(err) => panic!("failed waiting for `{expected}`: {err}"),
            }
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn selected_download_crosses_real_native_framing_and_requires_import_approval() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &root);
    let source = temp.path().canonicalize().unwrap().join("download.bin");
    let bytes = b"\x00\xff\x80Actual framed binary import\n";
    std::fs::write(&source, bytes).unwrap();
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    let share = json!({"type":"register_artifact","request_key":"panel-share","workspace_id":host.workspace_id,"session_id":session,
        "download":{"download_id":17,"filename":source,"bytes":bytes.len(),"mime":"application/octet-stream","origin":"files.example"}});
    host.send(&share);
    let registered = host.receive_until("control_result")["result"].clone();
    assert!(registered["artifact_id"].is_string(), "{registered}");
    host.send(&share);
    assert_eq!(host.receive_until("control_result")["result"], registered);
    let list = host.tool(&session, "artifacts-list", 0, "list_artifacts", json!({}));
    assert_eq!(list["data"]["artifacts"][0], registered);
    assert!(!list.to_string().contains(source.to_str().unwrap()));
    let destination = "AGENTS/CHATGPT/notes/imported.bin";
    host.send(&json!({"type":"run_tool","request_key":"panel-import","workspace_id":host.workspace_id,"session_id":session,
        "tool":"import_artifact","arguments":{"artifact_id":registered["artifact_id"],"path":destination}}));
    let prompt = host.receive_until("approval_required");
    assert!(!root.join(destination).exists());
    host.send(
        &json!({"type":"approval_decision","approval_id":prompt["approval"]["id"],"allow":true}),
    );
    let result = host.receive_until("control_result")["result"].clone();
    assert_eq!(result["status"], "ok", "{result}");
    assert_eq!(std::fs::read(root.join(destination)).unwrap(), bytes);
    assert_eq!(std::fs::read(source).unwrap(), bytes);
    assert_eq!(
        result["data"]["sha256"],
        bridge_native::fsops::sha256_bytes(bytes)
    );
}

#[test]
fn panel_tools_are_attributed_deduplicated_and_require_current_workspace_and_approval() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &root);
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    let path = "AGENTS/CHATGPT/notes/panel.txt";
    let message = json!({"type":"run_tool","request_key":"panel-create-1","workspace_id":host.workspace_id,"session_id":session,
        "tool":"write_document","arguments":{"path":path,"content":"Written from the panel\n","create_only":true}});
    host.send(&message);
    let first = host.receive_until("control_result")["result"].clone();
    assert_eq!(first["status"], "ok", "{first}");
    host.send(&message);
    let duplicate = host.receive_until("control_result")["result"].clone();
    assert_eq!(first, duplicate);
    assert_eq!(
        std::fs::read_to_string(root.join(path)).unwrap(),
        "Written from the panel\n"
    );

    let mut stale = message.clone();
    stale["request_key"] = json!("panel-stale-1");
    stale["workspace_id"] = json!("wrong-workspace");
    host.send(&stale);
    assert_eq!(host.receive_until("error")["code"], "workspace_changed");

    let mut conflict = message;
    conflict["arguments"]["content"] = json!("must not replace");
    host.send(&conflict);
    assert_eq!(
        host.receive_until("control_result")["result"]["code"],
        "request_conflict"
    );

    for allow in [false, true] {
        let path = format!("PROJECT/panel-approval-{allow}.md");
        host.send(&json!({"type":"run_tool","request_key":format!("panel-approval-{allow}"),"workspace_id":host.workspace_id,"session_id":session,
            "tool":"write_document","arguments":{"path":path,"content":"Reviewed write\n"}}));
        let pending = host.receive_until("approval_required");
        host.send(&json!({"type":"approval_decision","approval_id":pending["approval"]["id"],"allow":allow}));
        let result = host.receive_until("control_result")["result"].clone();
        if allow {
            assert_eq!(result["status"], "ok");
        } else {
            assert_eq!(result["code"], "approval_denied");
        }
        assert_eq!(root.join(path).is_file(), allow);
    }
    host.send(&json!({"type":"status"}));
    assert_eq!(host.receive_until("status_reply")["audit_valid"], true);
    let audit = std::fs::read_to_string(home.join("audit.jsonl")).unwrap();
    let events: Vec<Value> = audit
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(events
        .iter()
        .any(|event| event["action"] == "user.run_tool" && event["agent"] == "USER"));
    assert_eq!(
        events
            .iter()
            .filter(|event| event["action"] == "tool.write_document"
                && event["path"] == path
                && event["decision"] == "allowed")
            .count(),
        1
    );
    println!("panel: actual file created once; stale workspace and conflicting retry refused; approval deny/allow enforced; USER audit and chain verified");
}

#[test]
fn incompatible_protocol_revokes_a_previously_valid_handshake() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &root);
    let mut host = HostProcess::start(&binary, &home, &[]);
    host.register();
    host.send(&json!({"type":"hello","protocol":bridge_core::PROTOCOL_VERSION-1,"extension_version":"stale-extension"}));
    let error = host.receive_until("error");
    assert_eq!(error["code"], "protocol_mismatch");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains(&format!("host protocol {}", bridge_core::PROTOCOL_VERSION)));
    host.send(&json!({"type":"status"}));
    assert_eq!(host.receive_until("error")["code"], "hello_required");
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_unified-ai-bridge-native"))
}

#[test]
fn yolo_controls_cross_native_framing_and_restart_never_replays_a_write() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary,&home,&root);
    let mut host = HostProcess::start(&binary,&home,&[]);
    let session=host.register();
    host.send(&json!({"type":"set_workspace_policy","request_key":"strict","read_only":false,"permission_mode":"strict"}));
    host.receive_until("workspace_changed");
    host.send(&json!({"type":"yolo_control","request_key":"enable","workspace_id":host.workspace_id,"action":{"operation":"enable","seconds":60,"max_approvals":5}}));
    assert_eq!(host.receive_until("control_result")["result"]["yolo"]["active"],true);
    let args=json!({"path":"AGENTS/CHATGPT/notes/yolo.md","content":"Framed YOLO fixture\n","create_only":true});
    let result=host.tool(&session,"yolo-write",0,"write_document",args.clone());
    assert_eq!(result["status"],"ok","{result}");
    let first=std::fs::read(root.join("AGENTS/CHATGPT/notes/yolo.md")).unwrap();
    assert_eq!(first,b"Framed YOLO fixture\n");
    assert_eq!(host.tool(&session,"yolo-write",0,"write_document",args.clone()),result);
    host.send(&json!({"type":"status"}));
    assert_eq!(host.receive_until("status_reply")["yolo"]["used_approvals"],1);
    drop(host);
    let mut host=HostProcess::start(&binary,&home,&[]);
    let session=host.register();
    host.send(&json!({"type":"status"}));
    assert_eq!(host.receive_until("status_reply")["yolo"]["active"],false);
    assert_eq!(host.tool(&session,"yolo-write",0,"write_document",args),result);
    assert_eq!(std::fs::read(root.join("AGENTS/CHATGPT/notes/yolo.md")).unwrap(),first);
}

#[test]
fn chat_task_controls_persist_across_real_host_restart_without_replaying_writes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &root);
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    let args = json!({"path":"AGENTS/CHATGPT/notes/task-proof.md","content":"Real Native Messaging task receipt\n"});
    let written = host.tool(&session,"task-write",0,"write_document",args.clone());
    assert_eq!(written["status"],"ok","{written}");
    let id = {
        host.send(&json!({"type":"task_control","request_key":"task-list","workspace_id":host.workspace_id,"action":{"operation":"list"}}));
        host.receive_until("control_result")["result"]["tasks"][0]["id"].as_str().unwrap().to_owned()
    };
    host.send(&json!({"type":"task_control","request_key":"task-pause","workspace_id":host.workspace_id,"action":{"operation":"pause","task_id":id}}));
    assert_eq!(host.receive_until("control_result")["result"]["task"]["state"],"paused");
    let denied=host.tool(&session,"paused-write",0,"write_document",json!({"path":"AGENTS/CHATGPT/notes/not-written.md","content":"no"}));
    assert_eq!(denied["code"],"task_paused");
    assert!(!root.join("AGENTS/CHATGPT/notes/not-written.md").exists());
    drop(host);
    let mut host=HostProcess::start(&binary,&home,&[]);let session=host.register();
    host.send(&json!({"type":"task_control","request_key":"task-reopened","workspace_id":host.workspace_id,"action":{"operation":"detail","task_id":id}}));
    let detail=host.receive_until("control_result")["result"].clone();
    assert_eq!(detail["task"]["state"],"paused");
    assert_eq!(detail["task"]["counts"]["completed"],1);
    assert!(detail["steps"][0]["audit_seq"].as_i64().unwrap()>0);
    assert_eq!(host.tool(&session,"task-write",0,"write_document",args),written);
    host.send(&json!({"type":"task_control","request_key":"task-resume","workspace_id":host.workspace_id,"action":{"operation":"resume","task_id":id}}));
    assert_eq!(host.receive_until("control_result")["result"]["task"]["state"],"running");
    let read=host.tool(&session,"after-resume",0,"read_file",json!({"path":"AGENTS/CHATGPT/notes/task-proof.md"}));
    assert_eq!(read["status"],"ok");
    assert!(read["output"].as_str().unwrap().contains("Real Native Messaging task receipt"));
    host.send(&json!({"type":"tool_request","session_id":session,"workspace_id":host.workspace_id,
        "request_key":bridge_core::execution::request_key("chatgpt","reliability","pending-write",0),
        "response_id":"pending-write","task_id":"user-task","call_index":0,"tool":"write_document",
        "arguments":{"path":"SHARED/documentation/not-written.md","content":"stop cancels this approval"}}));
    let approval=host.receive_until("approval_required")["approval"]["id"].as_str().unwrap().to_owned();
    host.send(&json!({"type":"task_control","request_key":"task-stop","workspace_id":host.workspace_id,"action":{"operation":"stop","task_id":id}}));
    let mut stopped=false;let mut halted=false;
    while !stopped || !halted {
        let msg=host.messages.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        if msg["type"]=="control_result" {assert_eq!(msg["result"]["task"]["state"],"stopped");stopped=true;}
        if msg["type"]=="tool_result" {assert_eq!(msg["result"]["code"],"task_halted");halted=true;}
    }
    host.send(&json!({"type":"approval_decision","approval_id":approval,"allow":true}));
    assert_eq!(host.receive_until("approval_resolved")["allowed"],false);
    assert!(!root.join("SHARED/documentation/not-written.md").exists());
    host.send(&json!({"type":"task_control","request_key":"task-report","workspace_id":host.workspace_id,"action":{"operation":"export_report","task_id":id}}));
    let receipt=host.receive_until("control_result");
    let report:Value=serde_json::from_slice(&std::fs::read(receipt["result"]["report_path"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(report["task"]["state"],"stopped");
    assert_eq!(report["task"]["counts"]["completed"],2);
    assert_eq!(report["task"]["counts"]["cancelled"],2);
    assert_eq!(std::fs::read_to_string(root.join("AGENTS/CHATGPT/notes/task-proof.md")).unwrap(),"Real Native Messaging task receipt\n");
    host.send(&json!({"type":"status"}));assert_eq!(host.receive_until("status_reply")["audit_valid"],true);
    println!("Real framed task controls: write/read, pause, restart, replay without duplicate write, resume, stop during approval, late approval denial, private report, valid audit");
}

fn setup_workspace(binary: &Path, home: &Path, project: &Path) {
    let status = Command::new(binary)
        .args([
            "setup",
            "--project",
            project.to_str().expect("utf-8 project path"),
        ])
        .env("UAB_HOME", home)
        .env("UAB_NATIVE_HOSTS_DIR", home.join("NativeMessagingHosts"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .expect("run setup");
    assert!(status.success(), "setup failed");
}

#[test]
fn chrome_origin_argument_is_accepted() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = temp.path().join("home");
    let project = temp.path().join("archaeology");
    let binary = binary();
    setup_workspace(&binary, &home, &project);

    let mut host = HostProcess::start(
        &binary,
        &home,
        &["chrome-extension://gdoojijdmbacpolhfbgebeciagllgcop/"],
    );
    host.send(&json!({
        "type":"hello",
        "protocol":bridge_core::PROTOCOL_VERSION,
        "extension_version":"chrome-origin-test",
    }));
    let hello = host.receive_until("hello_ack");
    assert_eq!(hello["protocol"], bridge_core::PROTOCOL_VERSION);
}

#[test]
fn hello_register_write_read_and_status_over_chrome_framing() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let home = temp.path().join("home");
    let project = temp.path().join("archaeology");
    let binary = binary();
    setup_workspace(&binary, &home, &project);

    let mut host = HostProcess::start(&binary, &home, &[]);
    host.send(&json!({
        "type":"hello",
        "protocol":bridge_core::PROTOCOL_VERSION,
        "extension_version":"integration-test",
    }));
    let hello = host.receive_until("hello_ack");
    assert_eq!(hello["protocol"], bridge_core::PROTOCOL_VERSION);
    let canonical_project = std::fs::canonicalize(&project).expect("canonical project path");
    println!("framed hello_ack: {hello}");
    assert_eq!(
        hello["workspace_root"].as_str(),
        Some(canonical_project.to_string_lossy().as_ref())
    );

    host.send(&json!({
        "type":"register_session",
        "provider":"chatgpt",
        "tab_id":77,
        "window_id":9,
        "url":"https://chatgpt.com/c/native-framing-test",
        "conversation":"native-framing-test",
        "auto_connect":false,
    }));
    let registered = host.receive_until("session_registered");
    let workspace_id = registered["session"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let session_id = registered["session"]["id"]
        .as_str()
        .expect("session id")
        .to_string();
    assert_eq!(registered["session"]["agent"], "CHATGPT");
    assert_eq!(
        registered["session"]["extension_version"],
        "integration-test"
    );
    assert_eq!(registered["session"]["conversation"], "native-framing-test");
    let instructions = registered["instructions"]
        .as_str()
        .expect("tool instructions");
    let parsed = bridge_core::protocol::parse_tool_requests(instructions);
    assert!(
        parsed.problems.is_empty(),
        "invalid instruction example: {:?}",
        parsed.problems
    );
    assert!(
        parsed
            .calls
            .iter()
            .any(|call| call.call.tool == "list_files"
                && call.call.arguments == json!({"path":"PROJECT"})),
        "instructions must contain a usable tool example"
    );

    let relative = "AGENTS/CHATGPT/reports/native-framing.md";
    host.send(&json!({
        "type":"tool_request",
        "session_id":session_id,
        "workspace_id":workspace_id,
        "request_key":bridge_core::execution::request_key("chatgpt", "native-framing-test", "write-1", 0),
        "response_id":"write-1", "task_id":"task-1", "call_index":0,
        "tool":"write_document",
        "arguments":{
            "path":relative,
            "content":"# Native framing test\n\nCreated through Chrome's length-prefixed native messaging protocol.\n"
        }
    }));
    let write_result = host.receive_until("tool_result");
    assert_eq!(write_result["result"]["status"], "ok");
    assert!(
        project.join(relative).is_file(),
        "the real Markdown file exists"
    );

    host.send(&json!({
        "type":"tool_request",
        "session_id":session_id,
        "workspace_id":workspace_id,
        "request_key":bridge_core::execution::request_key("chatgpt", "native-framing-test", "read-1", 0),
        "response_id":"read-1", "task_id":"task-1", "call_index":0,
        "tool":"read_file",
        "arguments":{"path":relative}
    }));
    let read_result = host.receive_until("tool_result");
    assert_eq!(read_result["result"]["status"], "ok");
    assert!(read_result["result"]["output"]
        .as_str()
        .is_some_and(|text| text.contains("length-prefixed native messaging")));

    host.send(&json!({"type":"status"}));
    let status = host.receive_until("status_reply");
    assert_eq!(status["bridge"], "connected");
    assert!(status["sessions"]
        .as_array()
        .is_some_and(|sessions| sessions.len() == 1));
    assert!(status["audit_valid"].as_bool().unwrap_or(false));
    assert!(status["recent_activity"]
        .as_array()
        .is_some_and(|activity| !activity.is_empty()));
}

#[test]
fn retries_across_host_restart_are_cached_and_over_budget_never_writes() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &project);
    let relative = "AGENTS/CHATGPT/notes/retry.md";
    let args = json!({"path":relative, "content":"one real write\n"});
    let mut host = HostProcess::start(&binary, &home, &[]);
    let first_session = host.register();
    let first = host.tool(
        &first_session,
        "response-1",
        0,
        "write_document",
        args.clone(),
    );
    assert_eq!(first["status"], "ok");
    drop(host);
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    assert_ne!(session, first_session);
    let replay = host.tool(&session, "response-1", 0, "write_document", args);
    assert_eq!(
        replay, first,
        "cached result survives a native-host restart and new session"
    );
    let conflict = host.tool(
        &session,
        "response-1",
        0,
        "write_document",
        json!({"path":relative, "content":"must not replace"}),
    );
    assert_eq!(conflict["code"], "request_conflict");
    for index in 1..4 {
        assert_eq!(
            host.tool(
                &session,
                "response-1",
                index,
                "read_file",
                json!({"path":relative})
            )["status"],
            "ok"
        );
    }
    let overflow_path = "AGENTS/CHATGPT/notes/must-not-exist.md";
    let overflow = host.tool(
        &session,
        "response-1",
        4,
        "write_document",
        json!({"path":overflow_path,"content":"forbidden by budget"}),
    );
    assert_eq!(overflow["code"], "response_budget_exceeded");
    assert!(!project.join(overflow_path).exists());
    assert_eq!(
        std::fs::read_to_string(project.join(relative)).unwrap(),
        "one real write\n"
    );
    let denial = host.tool(
        &session,
        "response-2",
        0,
        "write_document",
        json!({"path":"../escape.md","content":"denied"}),
    );
    assert_eq!(denial["status"], "error");
    assert!(!temp.path().join("escape.md").exists());
    host.send(&json!({"type":"status"}));
    assert_eq!(host.receive_until("status_reply")["audit_valid"], true);
    let audit = std::fs::read_to_string(home.join("audit.jsonl")).unwrap();
    let writes = audit
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| {
            row["path"] == relative
                && row["decision"] == "allowed"
                && row["action"] == "tool.write_document"
        })
        .count();
    assert_eq!(writes, 1, "retry must not add a second write or checkpoint");
    println!(
        "real file={} cached_retry={} budget_denial={} traversal_denial={} audit_writes={writes}",
        project.join(relative).display(),
        replay["status"],
        overflow["code"],
        denial["code"]
    );
}

#[test]
fn timeout_is_terminal_and_removes_pending_approval_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &project);
    let config_path = home.join("config.toml");
    let mut config: bridge_native::config::Config =
        toml::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config.tool_timeout_secs = 1;
    std::fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    let path = "PROJECT/approval-timeout.md";
    let result = host.tool(
        &session,
        "approval-response",
        0,
        "write_document",
        json!({"path":path, "content":"must not be written"}),
    );
    assert_eq!(result["code"], "request_timeout");
    assert!(!project.join(path).exists());
    let retry = host.tool(
        &session,
        "approval-response",
        0,
        "write_document",
        json!({"path":path, "content":"must not be written"}),
    );
    assert_eq!(retry, result);
    host.send(&json!({"type":"status"}));
    let status = host.receive_until("status_reply");
    assert!(status["approvals"].as_array().unwrap().is_empty());
    println!(
        "timeout={} retry={} approvals={} file_exists={}",
        result["code"],
        retry["code"],
        status["approvals"],
        project.join(path).exists()
    );
}

#[test]
fn workspace_wire_switch_rejects_stale_context_and_returns_general_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let research = temp.path().join("research");
    let general = temp.path().join("general");
    let binary = binary();
    setup_workspace(&binary, &home, &research);
    let mut host = HostProcess::start(&binary, &home, &[]);
    let session = host.register();
    let previous = host.workspace_id.clone();
    host.send(&json!({"type":"create_workspace","request_key":"create-general","label":"General","path":general,"profile":"general"}));
    let created = host.receive_until("control_result");
    let id = created["result"]["id"].as_str().unwrap().to_string();
    host.send(
        &json!({"type":"set_active_workspace","request_key":"switch-general","workspace_id":id}),
    );
    let changed = host.receive_until("workspace_changed");
    assert_eq!(changed["profile"], "general");
    assert!(changed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|session| session["workspace_id"] == id));
    assert!(!changed["capabilities"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "list_agents"));
    host.send(&json!({"type":"tool_request","session_id":session,"workspace_id":previous,
        "request_key":bridge_core::execution::request_key("chatgpt","reliability","stale-workspace",0),
        "response_id":"stale-workspace","task_id":"task","call_index":0,"tool":"write_document","arguments":{"path":"files/stale.txt","content":"must not write"}}));
    assert_eq!(host.receive_until("error")["code"], "workspace_changed");
    host.workspace_id = id;
    let result = host.tool(
        &session,
        "fresh-workspace",
        0,
        "write_document",
        json!({"path":"files/main.rs","content":"fn main() {}\n","create_only":true}),
    );
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"]["workspace_id"], host.workspace_id);
    assert_eq!(
        std::fs::read_to_string(general.join("files/main.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(!general.join("files/stale.txt").exists());
    assert!(!research.join("files/main.rs").exists());
    host.send(&json!({"type":"status"}));
    let status = host.receive_until("status_reply");
    assert_eq!(status["active_id"], host.workspace_id);
    assert_eq!(status["audit_valid"], true);
    println!(
        "workspace wire: profile=general; stale-context write denied; real file={}; audit=true",
        general.join("files/main.rs").display()
    );
}

#[test]
fn background_command_survives_native_host_kill_and_returns_same_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let root = temp.path().join("workspace");
    let binary = binary();
    setup_workspace(&binary, &home, &root);
    let mut host = HostProcess::start(&binary, &home, &[]);
    let sid = host.register();
    let sleep = ["/usr/bin/sleep", "/bin/sleep"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .unwrap();
    host.send(&json!({"type":"grant_command","request_key":"grant-sleep","command":{"command_id":"durable-sleep","program":sleep,"fixed_args":["3"],"allowed_args":[],"cwd":"AGENTS/CHATGPT","timeout_secs":15,"max_output_bytes":128,"writes":false,"always_allow":true}}));
    host.receive_until("control_result");
    let result = host.tool(
        &sid,
        "start-durable-job",
        0,
        "run_command",
        json!({"command_id":"durable-sleep","background":true}),
    );
    assert_eq!(result["status"], "ok", "{result}");
    let id = result["data"]["job_id"].as_str().unwrap().to_string();
    host.send(&json!({"type":"job_control","request_key":"job-detail","workspace_id":host.workspace_id,"action":{"operation":"detail","job_id":id}}));
    host.receive_until("control_result");
    drop(host); // SIGKILL the real native-messaging process; the worker must survive.
    let mut next = HostProcess::start(&binary, &home, &[]);
    let next_sid = next.register();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        next.send(&json!({"type":"job_control","request_key":"job-after-restart","workspace_id":next.workspace_id,"action":{"operation":"detail","job_id":id}}));
        let value = next.receive_until("control_result")["result"].clone();
        if value["job"]["state"] == "completed" {
            assert_eq!(value["job"]["result"]["data"]["exit_code"], 0);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{value}");
        std::thread::sleep(Duration::from_millis(200));
    }
    let list = next.tool(&next_sid, "list-after-restart", 0, "list_jobs", json!({}));
    assert_eq!(list["data"]["jobs"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["data"]["jobs"][0]["job_id"], id);
}

#[test]
fn framed_continuation_moves_only_unused_budget_and_never_replays_original_write() {
    let temp=tempfile::tempdir().unwrap();
    let home=temp.path().join("home");
    let root=temp.path().join("workspace");
    let binary=binary();
    setup_workspace(&binary,&home,&root);
    let mut config:bridge_native::config::Config=toml::from_str(&std::fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    config.max_calls_per_task=2;
    std::fs::write(home.join("config.toml"),toml::to_string_pretty(&config).unwrap()).unwrap();
    let mut host=HostProcess::start(&binary,&home,&[]);
    let session=host.register();
    let path="AGENTS/CHATGPT/notes/handoff.txt";
    assert_eq!(host.tool(&session,"source-write",0,"write_document",json!({"path":path,"content":"written once before handoff\n"}))["status"],"ok");
    host.send(&json!({"type":"continue_task","request_key":"prepare","session_id":session,"workspace_id":host.workspace_id,"action":{"operation":"prepare","task_id":"user-task"}}));
    let prepared=host.receive_until("control_result")["result"].clone();
    assert_eq!(prepared["remaining_calls"],1);
    assert_eq!(host.tool(&session,"source-blocked",0,"write_document",json!({"path":"AGENTS/CHATGPT/notes/forbidden.txt","content":"must not run"}))["code"],"task_paused");
    // Real host restart between the two halves of the handoff.
    drop(host);
    let mut host=HostProcess::start(&binary,&home,&[]);
    host.register();
    host.send(&json!({"type":"register_session","provider":"chatgpt","tab_id":77,"url":"https://chatgpt.com/c/continued-task","conversation":"continued-task"}));
    let target=host.receive_until("session_registered")["session"]["id"].as_str().unwrap().to_string();
    host.send(&json!({"type":"continue_task","request_key":"bind","session_id":target,"workspace_id":host.workspace_id,"action":{"operation":"bind","handoff_id":prepared["handoff_id"],"task_id":"handoff-user"}}));
    assert_eq!(host.receive_until("control_result")["result"]["task"]["call_limit"],1);
    for (response,expected) in [("read-proof","ok"),("exhausted","error")] {
        host.send(&json!({"type":"tool_request","session_id":target,"workspace_id":host.workspace_id,
            "request_key":bridge_core::execution::request_key("chatgpt","continued-task",response,0),"response_id":response,"task_id":"handoff-user","call_index":0,"tool":"read_file","arguments":{"path":path}}));
        let result=host.receive_until("tool_result")["result"].clone();
        assert_eq!(result["status"],expected,"{result}");
        if expected=="error" {assert_eq!(result["code"],"task_budget_exceeded");}
    }
    assert_eq!(std::fs::read_to_string(root.join(path)).unwrap(),"written once before handoff\n");
    assert!(!root.join("AGENTS/CHATGPT/notes/forbidden.txt").exists());
    let db=rusqlite::Connection::open(home.join("bridge.sqlite3")).unwrap();
    let writes:u32=db.query_row("SELECT COUNT(*) FROM response_requests WHERE json_extract(payload,'$[0]')='write_document' AND status='complete'",[],|r|r.get(0)).unwrap();
    assert_eq!(writes,1);
}

#[test]
fn framed_workspace_budgets_enforce_new_limits_without_refilling_old_tasks() {
    let temp=tempfile::tempdir().unwrap();
    let home=temp.path().join("home");
    let root=temp.path().join("workspace");
    let binary=binary();
    setup_workspace(&binary,&home,&root);
    let mut host=HostProcess::start(&binary,&home,&[]);
    let session=host.register();
    let set = |host: &mut HostProcess, task, response| {
        host.send(&json!({"type":"task_budget_control","request_key":"limits","workspace_id":host.workspace_id,
            "action":{"operation":"save","limits":{"calls_per_task":task,"calls_per_response":response}}}));
        host.receive_until("control_result")["result"].clone()
    };
    assert_eq!(set(&mut host,2,1)["limits"]["calls_per_task"],2);
    let path="AGENTS/CHATGPT/notes/budget.txt";
    assert_eq!(host.tool(&session,"budget-write",0,"write_document",json!({"path":path,"content":"budget survives settings and restart\n"}))["status"],"ok");
    assert_eq!(host.tool(&session,"budget-write",1,"read_file",json!({"path":path}))["code"],"response_budget_exceeded");
    // An explicit increase changes future tasks, never this observed task.
    assert_eq!(set(&mut host,64,8)["limits"]["calls_per_task"],64);
    assert_eq!(host.tool(&session,"budget-read",0,"read_file",json!({"path":path}))["status"],"ok");
    assert_eq!(host.tool(&session,"budget-exhausted",0,"read_file",json!({"path":path}))["code"],"task_budget_exceeded");
    drop(host);
    let mut host=HostProcess::start(&binary,&home,&[]);
    let session=host.register();
    host.send(&json!({"type":"task_budget_control","request_key":"status","workspace_id":host.workspace_id,"action":{"operation":"status"}}));
    assert_eq!(host.receive_until("control_result")["result"]["limits"]["calls_per_task"],64);
    assert_eq!(host.tool(&session,"after-restart",0,"read_file",json!({"path":path}))["code"],"task_budget_exceeded");
    host.send(&json!({"type":"tool_request","session_id":session,"workspace_id":host.workspace_id,
        "request_key":bridge_core::execution::request_key("chatgpt","reliability","new-task-read",0),
        "response_id":"new-task-read","task_id":"new-explicit-user-task","call_index":0,
        "tool":"read_file","arguments":{"path":path}}));
    assert_eq!(host.receive_until("tool_result")["result"]["status"],"ok");
    let db=rusqlite::Connection::open(home.join("bridge.sqlite3")).unwrap();
    let limit:u32=db.query_row("SELECT json_extract(record,'$.call_limit') FROM chat_tasks WHERE task_id='new-explicit-user-task'",[],|r|r.get(0)).unwrap();
    assert_eq!(limit,64);
    let writes:u32=db.query_row("SELECT COUNT(*) FROM response_requests WHERE json_extract(payload,'$[0]')='write_document' AND status='complete'",[],|r|r.get(0)).unwrap();
    assert_eq!(writes,1);
    assert_eq!(std::fs::read_to_string(root.join(path)).unwrap(),"budget survives settings and restart\n");
}
