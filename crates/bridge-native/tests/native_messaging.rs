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
