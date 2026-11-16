use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use bridge_core::provider::Provider;
use bridge_core::workspace::{PermissionMode, Profile};
use bridge_core::{NATIVE_HOST_NAME, PROTOCOL_VERSION};
use serde::Deserialize;
use serde_json::{json, Value};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;

use crate::js;
use crate::model::{
    ActivityView, ApprovalView, BackgroundResponse, SessionView, StatusView, AUTO_CONNECT_KEY,
    EXTENSION_VERSION,
};

thread_local! {
    static KEEP: RefCell<Vec<JsValue>> = RefCell::new(Vec::new());
    static STATE: RefCell<Option<Rc<RefCell<BgState>>>> = RefCell::new(None);
}

struct BgState {
    details: Value,
    pending_controls: HashMap<String, js_sys::Function>,
    last_status_ms: f64,
    port: Option<JsValue>,
    ready: bool,
    queue: Vec<Value>,
    workspace: String,
    project: String,
    host_version: String,
    sessions: Vec<SessionView>,
    activity: Vec<ActivityView>,
    approvals: Vec<ApprovalView>,
    errors: Vec<String>,
    auto_connect: bool,
    pending_registers: HashMap<i64, js_sys::Function>,
    pending_tools: HashMap<String, PendingTool>,
    last_connect_ms: f64,
    reconnect_attempt: u32,
    connection_epoch: u64,
    audit_valid: bool,
    tool_timeout_secs: u64,
}

struct PendingTool {
    _tab_id: i32,
    session_id: String,
    respond: js_sys::Function,
}

impl BgState {
    fn new() -> Self {
        Self {
            details: json!({}),
            pending_controls: HashMap::new(),
            last_status_ms: 0.0,
            port: None,
            ready: false,
            queue: Vec::new(),
            workspace: String::new(),
            project: String::new(),
            host_version: String::new(),
            sessions: Vec::new(),
            activity: Vec::new(),
            approvals: Vec::new(),
            errors: Vec::new(),
            auto_connect: false,
            pending_registers: HashMap::new(),
            pending_tools: HashMap::new(),
            last_connect_ms: 0.0,
            reconnect_attempt: 0,
            connection_epoch: 0,
            audit_valid: false,
            tool_timeout_secs: 240,
        }
    }

    fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "[Unified AI Bridge] {message}"
        )));
        self.errors.push(message);
        if self.errors.len() > 10 {
            self.errors.remove(0);
        }
    }

    fn status(&self) -> Value {
        json!({
            "ok": true,
            "bridge": if self.ready { "connected" } else { "disconnected" },
            "reconnect_attempt": self.reconnect_attempt,
            "workspace": self.workspace.clone(),
            "project": self.project.clone(),
            "host_version": self.host_version.clone(),
            "protocol": PROTOCOL_VERSION,
            "extension_build": EXTENSION_VERSION,
            "active_id": self.details.get("active_id").cloned().unwrap_or_else(||json!("")),
            "workspaces": self.details.get("workspaces").cloned().unwrap_or_else(|| json!([])),
            "capabilities": self.details.get("capabilities").cloned().unwrap_or_default(),
            "in_flight": self.pending_tools.len(),
            "database_path": self.details.get("database_path").cloned().unwrap_or_else(||json!("")),
            "log_path": self.details.get("log_path").cloned().unwrap_or_else(||json!("")),
            "sessions": self.sessions.clone(),
            "activity": self.activity.clone(),
            "approvals": self.approvals.clone(),
            "auto_connect": self.auto_connect,
            "last_error": self.errors.last().cloned(),
            "audit_valid": self.audit_valid,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum BgRequest {
    Register {
        #[serde(default)]
        conversation: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
    Tool {
        session_id: String,
        workspace_id: String,
        request_key: String,
        response_id: String,
        task_id: String,
        call_index: u32,
        tool: String,
        #[serde(default)]
        arguments: Value,
    },
    Approval {
        approval_id: String,
        allow: bool,
    },
    Status,
    SettingsGet,
    SettingsSet {
        auto_connect: bool,
    },
    Reconnect,
    WorkspaceList,
    WorkspaceCreate {
        label: String,
        path: String,
        profile: Profile,
    },
    WorkspaceSelect {
        workspace_id: String,
        #[serde(default)]
        confirm_in_flight: bool,
    },
    WorkspaceForget {
        workspace_id: String,
    },
    WorkspacePolicy {
        read_only: bool,
        permission_mode: PermissionMode,
    },
    WorkspaceRescan,
    WorkspaceOpen,
    WorkspacePick,
    CommandGrant {
        command: Value,
    },
    CommandRevoke {
        command_id: String,
    },
    CommandList,
    ActivityDetail {
        activity_id: i64,
    },
    RunTool {
        workspace_id: String,
        session_id: String,
        tool: String,
        arguments: Value,
    },
    CommandPresets,
    IntegrationList,
    IntegrationSave {
        server: Value,
    },
    IntegrationRemove {
        server_id: String,
    },
    IntegrationDiscover {
        server_id: String,
    },
    ProviderControl {
        session_id: String,
        action: String,
    },
    OpenPanel,
}

#[oxichrome::background]
pub async fn start() {
    console_error_panic_hook::set_once();
    let state = Rc::new(RefCell::new(BgState::new()));
    STATE.with(|slot| *slot.borrow_mut() = Some(state.clone()));

    let listener_state = state.clone();
    let listener = Closure::<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>::new(
        move |message, sender, respond| {
            let Some(request) = js::from_js::<BgRequest>(message) else {
                return JsValue::FALSE;
            };
            handle_message(listener_state.clone(), request, sender, respond);
            JsValue::TRUE
        },
    );
    js::runtime_on_message_listener(&listener);
    KEEP.with(|keep| keep.borrow_mut().push(listener.into_js_value()));

    // Load the auto-connect preference without delaying listener registration.
    let settings_state = state.clone();
    spawn_local(async move {
        let value = oxichrome::storage::get::<bool>(AUTO_CONNECT_KEY)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
        settings_state.borrow_mut().auto_connect = value;
    });
}

fn handle_message(
    state: Rc<RefCell<BgState>>,
    request: BgRequest,
    sender: JsValue,
    respond: JsValue,
) {
    let Some(respond_fn) = js::function(&respond) else {
        return;
    };
    match request {
        BgRequest::Register { conversation, url } => {
            let tab_id = sender
                .clone()
                .dyn_into::<js_sys::Object>()
                .ok()
                .and_then(|sender| js_sys::Reflect::get(&sender, &JsValue::from_str("tab")).ok())
                .and_then(|tab| js_sys::Reflect::get(&tab, &JsValue::from_str("id")).ok())
                .and_then(|id| id.as_f64())
                .map(|id| id as i64);
            let Some(tab_id) = tab_id else {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Only a supported AI website tab can register a bridge session."}),
                );
                return;
            };
            let sender_tab_url = sender
                .clone()
                .dyn_into::<js_sys::Object>()
                .ok()
                .and_then(|sender| js_sys::Reflect::get(&sender, &JsValue::from_str("tab")).ok())
                .and_then(|tab| js_sys::Reflect::get(&tab, &JsValue::from_str("url")).ok())
                .and_then(|value| value.as_string());
            let Some(page_url) = sender_tab_url.or(url) else {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Chrome did not expose the tab URL. Reload the AI website tab."}),
                );
                return;
            };
            let Some(provider) = Provider::from_url(&page_url) else {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"This tab is not a supported AI website."}),
                );
                return;
            };
            let conversation = provider.conversation_id(&page_url).or(conversation);
            let settings_state = state.clone();
            let respond_fn = respond_fn.clone();
            spawn_local(async move {
                let auto_connect = oxichrome::storage::get::<bool>(AUTO_CONNECT_KEY)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                settings_state.borrow_mut().auto_connect = auto_connect;
                let previous = state
                    .borrow_mut()
                    .pending_registers
                    .insert(tab_id, respond_fn.clone());
                if let Some(previous) = previous {
                    js::respond(
                        &previous,
                        &json!({"ok":false,"error":"Registration superseded by a newer request."}),
                    );
                }
                let timeout_state = state.clone();
                let expected_response = respond_fn.clone();
                js::schedule(30_000, move || {
                    let matches = timeout_state
                        .borrow()
                        .pending_registers
                        .get(&tab_id)
                        .is_some_and(|f| f == &expected_response);
                    if matches {
                        if let Some(respond) =
                            timeout_state.borrow_mut().pending_registers.remove(&tab_id)
                        {
                            js::respond(
                                &respond,
                                &json!({"ok":false,"code":"registration_timeout","error":"Session registration timed out; retrying on the next status check."}),
                            );
                        }
                    }
                });
                let message = json!({
                    "type":"register_session",
                    "provider": provider.slug(),
                    "tab_id": tab_id,
                    "window_id": 0,
                    "url": page_url,
                    "conversation": conversation,
                    "auto_connect": auto_connect,
                });
                if let Err(err) = send_native(&settings_state, message) {
                    settings_state
                        .borrow_mut()
                        .pending_registers
                        .remove(&tab_id);
                    js::respond(&respond_fn, &json!({"ok":false,"error":err}));
                }
            });
        }
        BgRequest::Tool {
            session_id,
            workspace_id,
            request_key,
            response_id,
            task_id,
            call_index,
            tool,
            arguments,
        } => {
            if request_key.is_empty() || request_key.len() > 200 {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Invalid request key."}),
                );
                return;
            }
            let tab_id = sender
                .clone()
                .dyn_into::<js_sys::Object>()
                .ok()
                .and_then(|sender| js_sys::Reflect::get(&sender, &JsValue::from_str("tab")).ok())
                .and_then(|tab| js_sys::Reflect::get(&tab, &JsValue::from_str("id")).ok())
                .and_then(|id| id.as_f64())
                .map(|id| id as i32)
                .unwrap_or(-1);
            let sender_url = js_sys::Reflect::get(&sender, &JsValue::from_str("url"))
                .ok()
                .and_then(|v| v.as_string());
            let current_tab_url = js_sys::Reflect::get(&sender, &JsValue::from_str("tab"))
                .ok()
                .and_then(|tab| js_sys::Reflect::get(&tab, &JsValue::from_str("url")).ok())
                .and_then(|url| url.as_string());
            let frame_id = js_sys::Reflect::get(&sender, &JsValue::from_str("frameId"))
                .ok()
                .and_then(|id| id.as_f64())
                .map(|id| id as i32);
            let valid_session = state.borrow().sessions.iter().any(|session| {
                session.id == session_id
                    && bridge_core::execution::browser_sender_matches(
                        &session.provider,
                        session.tab_id,
                        session.conversation.as_deref(),
                        tab_id as i64,
                        frame_id,
                        sender_url.as_deref(),
                        current_tab_url.as_deref(),
                    )
            });
            if !valid_session {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"code":"session_mismatch","error":"The session does not belong to this tab and conversation. Reconnect the session."}),
                );
                return;
            }
            if state.borrow().pending_tools.contains_key(&request_key) {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"code":"request_in_progress","error":"This response block is already running."}),
                );
                return;
            }
            state.borrow_mut().pending_tools.insert(
                request_key.clone(),
                PendingTool {
                    _tab_id: tab_id,
                    session_id: session_id.clone(),
                    respond: respond_fn.clone(),
                },
            );
            let message = json!({
                "type":"tool_request",
                "session_id": session_id,
                "workspace_id": workspace_id,
                "request_key": request_key,
                "response_id": response_id,
                "task_id": task_id,
                "call_index": call_index,
                "tool": tool,
                "arguments": arguments,
            });
            if let Err(err) = send_native(&state, message) {
                state.borrow_mut().pending_tools.remove(&request_key);
                js::respond(&respond_fn, &json!({"ok":false,"error":err}));
            }
            let timeout_ms = (state.borrow().tool_timeout_secs.saturating_add(5) * 1000) as i32;
            js::schedule(timeout_ms, move || {
                let pending = state.borrow_mut().pending_tools.remove(&request_key);
                if let Some(pending) = pending {
                    state.borrow_mut().queue.retain(|message| {
                        message.get("request_key").and_then(Value::as_str) != Some(&request_key)
                    });
                    js::respond(
                        &pending.respond,
                        &json!({"ok":false,"code":"request_timeout","error":"The local bridge did not return a terminal result before its deadline. Check activity before repeating a write."}),
                    );
                }
            });
        }
        BgRequest::Approval { approval_id, allow } => {
            if state.borrow().ready {
                let message = json!({
                    "type":"approval_decision",
                    "approval_id": approval_id,
                    "allow": allow,
                });
                if let Err(err) = send_native(&state, message) {
                    js::respond(&respond_fn, &json!({"ok":false,"error":err}));
                    return;
                }
                state
                    .borrow_mut()
                    .approvals
                    .retain(|approval| approval.id != approval_id);
                js::respond(&respond_fn, &json!({"ok":true}));
            } else {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"The local bridge is disconnected."}),
                );
            }
        }
        BgRequest::Status => {
            let _ = ensure_native(&state, false);
            if state.borrow().ready && js::now_ms() - state.borrow().last_status_ms > 2_000.0 {
                state.borrow_mut().last_status_ms = js::now_ms();
                let _ = send_native(&state, json!({"type":"status"}));
            }
            let status = state.borrow().status();
            js::respond(&respond_fn, &status);
        }
        BgRequest::SettingsGet => {
            let state = state.clone();
            spawn_local(async move {
                let value = oxichrome::storage::get::<bool>(AUTO_CONNECT_KEY)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                state.borrow_mut().auto_connect = value;
                js::respond(&respond_fn, &json!({"ok":true,"auto_connect":value}));
            });
        }
        BgRequest::SettingsSet { auto_connect } => {
            let state = state.clone();
            let respond_fn = respond_fn.clone();
            spawn_local(async move {
                match oxichrome::storage::set(AUTO_CONNECT_KEY, &auto_connect).await {
                    Ok(()) => {
                        state.borrow_mut().auto_connect = auto_connect;
                        js::respond(&respond_fn, &json!({"ok":true,"auto_connect":auto_connect}));
                    }
                    Err(err) => js::respond(
                        &respond_fn,
                        &json!({"ok":false,"error":format!("Could not save the setting: {err}")}),
                    ),
                }
            });
        }
        BgRequest::Reconnect => {
            // Never open a second port while one is alive: the old port's
            // onDisconnect would race the new port and clear its state.
            match ensure_native(&state, true) {
                Ok(()) => {
                    if let Err(err) = send_native(&state, json!({"type":"status"})) {
                        js::respond(&respond_fn, &json!({"ok":false,"error":err}));
                    } else {
                        js::respond(&respond_fn, &json!({"ok":true,"bridge":"connecting"}));
                    }
                }
                Err(err) => js::respond(&respond_fn, &json!({"ok":false,"error":err})),
            }
        }
        BgRequest::OpenPanel => {
            match js::create_tab(&js::to_js(
                &json!({"url":js::extension_url("sidepanel.html")}),
            )) {
                Ok(_) => js::respond(&respond_fn, &json!({"ok":true})),
                Err(err) => js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":format!("Could not open controls: {err:?}")}),
                ),
            }
        }
        BgRequest::ProviderControl { session_id, action } => {
            let source = js_sys::Reflect::get(&sender, &JsValue::from_str("url"))
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            if !source.starts_with(&js::extension_url("")) {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Provider controls require the extension panel."}),
                );
                return;
            }
            let Some(session) = state
                .borrow()
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .cloned()
            else {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Session is no longer connected."}),
                );
                return;
            };
            if !["focus", "reconnect", "reinject", "disconnect"].contains(&action.as_str()) {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Unknown provider action."}),
                );
                return;
            }
            if action == "disconnect" {
                if let Err(err) = send_native(
                    &state,
                    json!({"type":"unregister_session","session_id":session_id}),
                ) {
                    js::respond(&respond_fn, &json!({"ok":false,"error":err}));
                    return;
                }
            }
            let promise = if action == "focus" {
                js::update_tab(session.tab_id as i32, &js::to_js(&json!({"active":true})))
            } else {
                js::tabs_send_message_raw(
                    session.tab_id as i32,
                    &js::to_js(&json!({"type":"provider_control","action":action})),
                )
            };
            spawn_local(async move {
                match promise {
                    Ok(promise) => match wasm_bindgen_futures::JsFuture::from(promise).await {
                        Ok(_) => {
                            js::respond(&respond_fn, &json!({"ok":true,"result":{"action":action}}))
                        }
                        Err(err) => js::respond(
                            &respond_fn,
                            &json!({"ok":false,"error":format!("Tab control failed: {err:?}")}),
                        ),
                    },
                    Err(err) => js::respond(
                        &respond_fn,
                        &json!({"ok":false,"error":format!("Tab control failed: {err:?}")}),
                    ),
                }
            });
        }
        request @ (BgRequest::WorkspaceList
        | BgRequest::WorkspaceCreate { .. }
        | BgRequest::WorkspaceSelect { .. }
        | BgRequest::WorkspaceForget { .. }
        | BgRequest::WorkspacePolicy { .. }
        | BgRequest::WorkspaceRescan
        | BgRequest::WorkspaceOpen
        | BgRequest::WorkspacePick
        | BgRequest::CommandGrant { .. }
        | BgRequest::CommandRevoke { .. }
        | BgRequest::CommandList
        | BgRequest::ActivityDetail { .. }
        | BgRequest::RunTool { .. }
        | BgRequest::CommandPresets
        | BgRequest::IntegrationList
        | BgRequest::IntegrationSave { .. }
        | BgRequest::IntegrationRemove { .. }
        | BgRequest::IntegrationDiscover { .. }) => {
            let source = js_sys::Reflect::get(&sender, &JsValue::from_str("url"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default();
            if !source.starts_with(&js::extension_url("")) {
                js::respond(
                    &respond_fn,
                    &json!({"ok":false,"error":"Workspace and command grants require the extension controls."}),
                );
                return;
            }
            let key = format!("panel-{:016x}", js::random_u64());
            let mut message = match request {
                BgRequest::WorkspaceList => json!({"type":"list_workspaces"}),
                BgRequest::WorkspaceCreate {
                    label,
                    path,
                    profile,
                } => json!({"type":"create_workspace","label":label,"path":path,"profile":profile}),
                BgRequest::WorkspaceSelect {
                    workspace_id,
                    confirm_in_flight,
                } => {
                    json!({"type":"set_active_workspace","workspace_id":workspace_id,"confirm_in_flight":confirm_in_flight})
                }
                BgRequest::WorkspaceForget { workspace_id } => {
                    json!({"type":"forget_workspace","workspace_id":workspace_id})
                }
                BgRequest::WorkspacePolicy {
                    read_only,
                    permission_mode,
                } => {
                    json!({"type":"set_workspace_policy","read_only":read_only,"permission_mode":permission_mode})
                }
                BgRequest::WorkspaceRescan => json!({"type":"rescan_workspace"}),
                BgRequest::WorkspaceOpen => json!({"type":"open_workspace"}),
                BgRequest::WorkspacePick => json!({"type":"pick_workspace_folder"}),
                BgRequest::CommandGrant { command } => {
                    json!({"type":"grant_command","command":command})
                }
                BgRequest::CommandRevoke { command_id } => {
                    json!({"type":"revoke_command","command_id":command_id})
                }
                BgRequest::CommandList => json!({"type":"list_command_grants"}),
                BgRequest::ActivityDetail { activity_id } => {
                    json!({"type":"activity_detail","activity_id":activity_id})
                }
                BgRequest::RunTool {
                    workspace_id,
                    session_id,
                    tool,
                    arguments,
                } => {
                    json!({"type":"run_tool","workspace_id":workspace_id,"session_id":session_id,"tool":tool,"arguments":arguments})
                }
                BgRequest::CommandPresets => json!({"type":"command_presets"}),
                BgRequest::IntegrationList => json!({"type":"integration_list"}),
                BgRequest::IntegrationSave { server } => {
                    json!({"type":"integration_save","server":server})
                }
                BgRequest::IntegrationRemove { server_id } => {
                    json!({"type":"integration_remove","server_id":server_id})
                }
                BgRequest::IntegrationDiscover { server_id } => {
                    json!({"type":"integration_discover","server_id":server_id})
                }
                _ => unreachable!(),
            };
            message["request_key"] = key.clone().into();
            state
                .borrow_mut()
                .pending_controls
                .insert(key.clone(), respond_fn.clone());
            if let Err(err) = send_native(&state, message) {
                state.borrow_mut().pending_controls.remove(&key);
                js::respond(&respond_fn, &json!({"ok":false,"error":err}));
            }
            let timeout_state = state.clone();
            js::schedule(245_000, move || {
                if let Some(respond) = timeout_state.borrow_mut().pending_controls.remove(&key) {
                    js::respond(
                        &respond,
                        &json!({"ok":false,"code":"request_timeout","error":"Operation timed out. Inspect activity and files before retrying a mutation."}),
                    );
                }
            });
        }
    }
}

fn send_native(state: &Rc<RefCell<BgState>>, message: Value) -> Result<(), String> {
    ensure_native(state, false)?;
    let (port, ready) = {
        let state = state.borrow();
        (state.port.clone(), state.ready)
    };
    let Some(port) = port else {
        return Err("The local bridge is not connected.".into());
    };
    if ready {
        js::port_post(&port, &message)
    } else {
        state.borrow_mut().queue.push(message);
        Ok(())
    }
}

fn ensure_native(state: &Rc<RefCell<BgState>>, force: bool) -> Result<(), String> {
    let now = js::now_ms();
    {
        let state = state.borrow();
        if state.port.is_some() {
            return Ok(());
        }
        if !force
            && now - state.last_connect_ms
                < bridge_core::execution::reconnect_delay_ms(state.reconnect_attempt) as f64
        {
            return Err(state
                .errors
                .last()
                .cloned()
                .unwrap_or_else(|| "The local bridge is disconnected.".into()));
        }
    }
    state.borrow_mut().last_connect_ms = now;
    {
        let mut current = state.borrow_mut();
        current.reconnect_attempt = current.reconnect_attempt.saturating_add(1);
    }
    let epoch = {
        let mut current = state.borrow_mut();
        current.connection_epoch = current.connection_epoch.wrapping_add(1);
        current.connection_epoch
    };
    let port = match js::connect_native(NATIVE_HOST_NAME) {
        Ok(port) => port,
        Err(err) => {
            let message = format!(
                "Cannot start the local native host. Build and install it, then reload Chrome. ({err:?})"
            );
            stop_native(state, &message, true);
            return Err(message);
        }
    };
    state.borrow_mut().port = Some(port.clone());
    state.borrow_mut().ready = false;

    let message_state = state.clone();
    let on_message = Closure::<dyn FnMut(JsValue)>::new(move |message| {
        if message_state.borrow().connection_epoch == epoch {
            handle_native_message(message_state.clone(), message);
        }
    });
    if let Err(err) = js::add_listener_one(&port, "onMessage", &on_message) {
        stop_native(
            state,
            &format!("Could not attach native message listener: {err:?}"),
            true,
        );
        return Err("Could not attach the native host message listener.".into());
    }
    KEEP.with(|keep| keep.borrow_mut().push(on_message.into_js_value()));

    let disconnect_state = state.clone();
    let on_disconnect = Closure::<dyn FnMut(JsValue)>::new(move |_port| {
        if disconnect_state.borrow().connection_epoch == epoch {
            stop_native(&disconnect_state,"The local bridge disconnected. An in-flight operation may have completed; inspect activity before retrying.",true);
        }
    });
    if let Err(err) = js::add_listener_one(&port, "onDisconnect", &on_disconnect) {
        stop_native(
            state,
            &format!("Could not attach native disconnect listener: {err:?}"),
            true,
        );
        return Err("Could not attach the native host disconnect listener.".into());
    }
    KEEP.with(|keep| keep.borrow_mut().push(on_disconnect.into_js_value()));

    let hello = json!({
        "type":"hello",
        "protocol": PROTOCOL_VERSION,
        "extension_version": EXTENSION_VERSION,
    });
    if let Err(err) = js::port_post(&port, &hello) {
        stop_native(state, &err, true);
        return Err(err);
    }
    let timeout_state = state.clone();
    js::schedule(15_000, move || {
        let expired = {
            let state = timeout_state.borrow();
            state.connection_epoch == epoch && state.port.is_some() && !state.ready
        };
        if expired {
            stop_native(
                &timeout_state,
                "Native host handshake timed out after 15 seconds.",
                true,
            );
        }
    });
    Ok(())
}

fn stop_native(state: &Rc<RefCell<BgState>>, message: &str, retry: bool) {
    let (port, responders, epoch, delay) = {
        let mut current = state.borrow_mut();
        current.connection_epoch = current.connection_epoch.wrapping_add(1);
        let port = current.port.take();
        current.ready = false;
        current.queue.clear();
        current.sessions.clear();
        current.error(message);
        let mut responders: Vec<js_sys::Function> = current
            .pending_tools
            .drain()
            .map(|(_, pending)| pending.respond)
            .collect();
        responders.extend(
            current
                .pending_registers
                .drain()
                .map(|(_, respond)| respond),
        );
        responders.extend(current.pending_controls.drain().map(|(_, respond)| respond));
        (
            port,
            responders,
            current.connection_epoch,
            bridge_core::execution::reconnect_delay_ms(current.reconnect_attempt) as i32,
        )
    };
    if let Some(port) = port {
        js::port_disconnect(&port);
    }
    for respond in responders {
        js::respond(&respond, &json!({"ok":false,"error":message}));
    }
    if retry {
        let state = state.clone();
        js::schedule(delay, move || {
            if state.borrow().connection_epoch == epoch {
                let _ = ensure_native(&state, false);
            }
        });
    }
}

fn handle_native_message(state: Rc<RefCell<BgState>>, message: JsValue) {
    let Some(value) = js::from_js::<Value>(message) else {
        state
            .borrow_mut()
            .error("The native host sent a message that was not valid JSON.");
        return;
    };
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match kind {
        "hello_ack" => {
            if value.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
                stop_native(&state,&format!("Protocol mismatch: extension {PROTOCOL_VERSION}, host {}. Rebuild and reload the extension and host together.",value.get("protocol").unwrap_or(&Value::Null)),false);
                return;
            }
            {
                let mut state = state.borrow_mut();
                state.ready = true;
                state.reconnect_attempt = 0;
                // A successful new handshake resolves errors from the old
                // connection. Keep their console/log history, not an active banner.
                state.errors.clear();
                state.tool_timeout_secs = value
                    .get("tool_timeout_secs")
                    .and_then(Value::as_u64)
                    .unwrap_or(240);
                state.details["active_id"] = value["workspace_id"].clone();
                state.workspace = value
                    .get("workspace_root")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                state.project = value
                    .get("project_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                state.host_version = value
                    .get("host_version")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
            }
            // Refresh host state and flush registrations queued before the ack.
            let (port, queue) = {
                let mut state = state.borrow_mut();
                let queue = std::mem::take(&mut state.queue);
                (state.port.clone(), queue)
            };
            if let Some(port) = port {
                for message in queue {
                    let _ = js::port_post(&port, &message);
                }
            }
            if let Err(err) = send_native(&state, json!({"type":"status"})) {
                state.borrow_mut().error(err);
            }
        }
        "session_registered" => {
            let session: Option<SessionView> = value
                .get("session")
                .cloned()
                .and_then(|session| serde_json::from_value(session).ok());
            let Some(session) = session else {
                state
                    .borrow_mut()
                    .error("Native host returned an invalid session registration.");
                return;
            };
            let tab_id = session.tab_id;
            {
                let mut state = state.borrow_mut();
                if !state
                    .sessions
                    .iter()
                    .any(|existing| existing.id == session.id)
                {
                    state.sessions.push(session.clone());
                }
            }
            let Some(respond) = state.borrow_mut().pending_registers.remove(&tab_id) else {
                return;
            };
            let (workspace, auto_connect) = {
                let state = state.borrow();
                (state.workspace.clone(), state.auto_connect)
            };
            let host_auto = value
                .get("auto_connect")
                .and_then(Value::as_bool)
                .unwrap_or(auto_connect);
            js::respond(
                &respond,
                &json!({
                    "ok": true,
                    "bridge": "connected",
                    "session_id": session.id,
                    "workspace_id": session.workspace_id,
                    "capabilities": value["capabilities"],
                    "agent": session.agent,
                    "provider": session.provider,
                    "conversation": session.conversation,
                    "instructions": value.get("instructions").and_then(Value::as_str).unwrap_or(""),
                    "auto_connect": host_auto,
                    "workspace": workspace,
                    "session": session,
                }),
            );
        }
        "tool_result" => {
            let request_key = value
                .get("request_key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !state
                .borrow()
                .pending_tools
                .get(&request_key)
                .is_some_and(|pending| {
                    Some(pending.session_id.as_str())
                        == value.get("session_id").and_then(Value::as_str)
                })
            {
                return;
            }
            let pending = state.borrow_mut().pending_tools.remove(&request_key);
            if let Some(pending) = pending {
                js::respond(
                    &pending.respond,
                    &json!({
                        "ok": true,
                        "result": value.get("result").cloned().unwrap_or(Value::Null),
                        "duration_ms": value.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                    }),
                );
            }
        }
        "error" => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Unknown native host error")
                .to_string();
            let code = value
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("native_error")
                .to_string();
            if code == "protocol_mismatch" {
                stop_native(&state, &message, false);
                return;
            }
            let request_key = value
                .get("request_key")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(request_key) = request_key {
                let control = state.borrow_mut().pending_controls.remove(&request_key);
                if let Some(respond) = control {
                    js::respond(&respond, &json!({"ok":false,"error":message,"code":code}));
                    return;
                }
                let pending = state.borrow_mut().pending_tools.remove(&request_key);
                if let Some(pending) = pending {
                    js::respond(
                        &pending.respond,
                        &json!({"ok":false,"error":message,"code":code}),
                    );
                }
            } else {
                state.borrow_mut().error(message.clone());
                let responders: Vec<js_sys::Function> = state
                    .borrow_mut()
                    .pending_registers
                    .drain()
                    .map(|(_, respond)| respond)
                    .collect();
                for respond in responders {
                    js::respond(&respond, &json!({"ok":false,"error":message,"code":code}));
                }
            }
        }
        "event" => {
            let Some(event) = value.get("event") else {
                return;
            };
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "activity" => {
                    if let Some(activity) = event
                        .get("activity")
                        .cloned()
                        .and_then(|activity| serde_json::from_value::<ActivityView>(activity).ok())
                    {
                        let mut state = state.borrow_mut();
                        state.activity.insert(0, activity);
                        state.activity.truncate(40);
                    }
                }
                "approval_required" => {
                    if let Some(approval) = event
                        .get("approval")
                        .cloned()
                        .and_then(|approval| serde_json::from_value::<ApprovalView>(approval).ok())
                    {
                        let mut state = state.borrow_mut();
                        state
                            .approvals
                            .retain(|existing| existing.id != approval.id);
                        state.approvals.insert(0, approval);
                        state.approvals.truncate(20);
                    }
                }
                _ => {}
            }
        }
        "approval_resolved" => {
            if let Some(id) = value.get("approval_id").and_then(Value::as_str) {
                state
                    .borrow_mut()
                    .approvals
                    .retain(|approval| approval.id != id);
            }
        }
        "approval_required" => {
            if let Some(approval) = value
                .get("approval")
                .cloned()
                .and_then(|approval| serde_json::from_value::<ApprovalView>(approval).ok())
            {
                let mut state = state.borrow_mut();
                state
                    .approvals
                    .retain(|existing| existing.id != approval.id);
                state.approvals.insert(0, approval);
                state.approvals.truncate(20);
            }
        }
        "status_reply" => {
            let mut state = state.borrow_mut();
            state.details = value.clone();
            state.ready = true;
            state.workspace = value
                .get("workspace_root")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            state.project = state
                .workspace
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            if let Some(host_version) = value.get("host_version").and_then(Value::as_str) {
                state.host_version = host_version.to_string();
            }
            if let Some(sessions) = value
                .get("sessions")
                .cloned()
                .and_then(|sessions| serde_json::from_value::<Vec<SessionView>>(sessions).ok())
            {
                state.sessions = sessions;
            }
            if let Some(activity) = value
                .get("recent_activity")
                .cloned()
                .and_then(|activity| serde_json::from_value::<Vec<ActivityView>>(activity).ok())
            {
                state.activity = activity.into_iter().take(40).collect();
            }
            if let Some(approvals) = value
                .get("approvals")
                .cloned()
                .and_then(|approvals| serde_json::from_value::<Vec<ApprovalView>>(approvals).ok())
            {
                state.approvals = approvals;
            }
            state.audit_valid = value
                .get("audit_valid")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }
        "workspace_changed" | "workspace_list" | "control_result" => {
            let key = value["request_key"].as_str().unwrap_or_default();
            if kind == "workspace_changed" {
                let sessions =
                    serde_json::from_value::<Vec<SessionView>>(value["sessions"].clone())
                        .unwrap_or_default();
                {
                    let mut state = state.borrow_mut();
                    state.workspace = value["root"].as_str().unwrap_or_default().into();
                    state.details["active_id"] = value["workspace_id"].clone();
                    state.details["workspaces"] = value["workspaces"].clone();
                    state.details["capabilities"] = value["capabilities"].clone();
                    state.sessions = sessions.clone();
                }
                let mut tabs = std::collections::HashSet::new();
                for session in sessions {
                    if tabs.insert(session.tab_id) {
                        js::send_to_tab(
                            session.tab_id as i32,
                            &json!({"type":"workspace_changed","workspace_id":value["workspace_id"],"root":value["root"]}),
                        );
                    }
                }
            }
            let respond = state.borrow_mut().pending_controls.remove(key);
            if let Some(respond) = respond {
                js::respond(
                    &respond,
                    &json!({"ok":true,"result":if kind == "control_result" { value["result"].clone() } else { value.clone() }}),
                );
            }
            let _ = send_native(&state, json!({"type":"status"}));
        }
        "pong" => {}
        _ => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[Unified AI Bridge] unhandled native message type: {kind}"
            )));
        }
    }
}

pub fn status_snapshot() -> StatusView {
    STATE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|state| {
                let value = state.borrow().status();
                js::from_js::<StatusView>(js::to_js(&value)).unwrap_or_default()
            })
            .unwrap_or_default()
    })
}

pub fn request_snapshot() -> BackgroundResponse {
    BackgroundResponse::default()
}
