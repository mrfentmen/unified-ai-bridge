//! Content script: provider adapters, conversation injection, completion
//! detection, tool dispatch, and the in-page connection panel.
//!
//! The model is never trusted for identity. It only emits a strict JSON tool
//! block; the background maps the tab to the host-issued session id.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use bridge_core::execution::{kimi_inserted_draft_matches, owned_draft_matches, request_key, x_grok_turn, DeliveryTurn};
use bridge_core::protocol::{self, format_tool_results, ParsedCall};
use bridge_core::provider::Provider;
use serde_json::{json, Value};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Document, Element, EventInit, HtmlElement, HtmlTextAreaElement, KeyboardEvent,
    KeyboardEventInit, ShadowRootInit, ShadowRootMode,
};

use crate::js;
use crate::model::{ActivityView, ApprovalView, BackgroundResponse, StatusView};
#[path = "chat_recovery.rs"]
mod recovery;
#[path = "generated_files.rs"]
mod generated_files;
#[path = "uploads.rs"]
mod uploads;
use bridge_core::recovery::{pending_result_state, DeliveryRetry, FormatRepairBudget, PendingResultState, WebsiteBlock};

const POLL_MS: i32 = 2_000;
const DETECT_MS: i32 = 1_500;
const INSTRUCTIONS_MARKER: &str = "Unified AI Bridge — local";

#[oxichrome::content_script(
    matches = [
        "https://chatgpt.com/*",
        "https://chat.openai.com/*",
        "https://claude.ai/*",
        "https://chat.deepseek.com/*",
        "https://gemini.google.com/*",
        "https://grok.com/*",
        "https://x.com/i/grok*",
        "https://chat.qwen.ai/*",
        "https://www.kimi.ai/*",
        "https://kimi.ai/*"
    ],
    run_at = DocumentIdle
)]
pub async fn bridge_content() {
    console_error_panic_hook::set_once();
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    if document
        .get_element_by_id("unified-ai-bridge-host")
        .is_some()
    {
        return;
    }
    let Ok(url) = document.url() else {
        return;
    };
    let Some(provider) = Provider::from_url(&url) else {
        return;
    };
    let conversation = provider.conversation_id(&url);
    let adapter = Adapter::for_url(provider, &url);
    let ui = match build_ui(&document) {
        Ok(ui) => ui,
        Err(err) => {
            web_sys::console::error_1(&err);
            return;
        }
    };
    let state = Rc::new(RefCell::new(ContentState {
        provider,
        adapter,
        conversation,
        url,
        ui,
        session_id: None,
        agent: None,
        workspace: String::new(),
        workspace_id: String::new(),
        pending_workspace_context: false,
        host_version: String::new(),
        yolo: Value::Null,
        instructions: String::new(),
        instructions_draft: Rc::default(),
        sending_instructions: false,
        connected: false,
        auto_connect: false,
        paused: false,
        auto_attempted: false,
        registering: false,
        in_flight: false,
        bridge: "disconnected".into(),
        error: None,
        bridge_error: None,
        adapter_warning: None,
        pending_result: None,
        sending_result: false,
        memory: ConversationMemory::default(),
        observed_user: None,
        session_approval: None,
        activity: Vec::new(),
        last_text: String::new(),
        stable_since_ms: 0.0,
        processed: HashSet::new(),
        last_register_ms: 0.0,
        route_epoch: 0,
        task_gate: None,
        continuation: None,
        continuation_loaded: false,
        recovering: false,
    }));

    install_buttons(&state);
    let updates = state.clone();
    let listener = Closure::<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>::new(
        move |message, _, respond| {
            if let Some(value) = js::from_js::<Value>(message) {
                if value["type"] == "browser_operation" {
                    if matches!(value["action"]["action"].as_str(),Some("inspect_upload" | "upload")) {
                        if let Some(respond)=js::function(&respond) {
                            let state=updates.clone();
                            spawn_local(async move {
                                let result=uploads::handle(state,value).await;
                                js::respond(&respond,&json!(result));
                            });
                            return JsValue::TRUE;
                        }
                        return JsValue::FALSE;
                    }
                    let outcome: Result<Value,String> = if value["expires_at_ms"].as_f64().unwrap_or(0.0)<=js::now_ms() {
                        Err("Browser operation expired; inspect downloads before retrying.".into())
                    } else { serde_json::from_value(value["scope"].clone()).map_err(|_|"Invalid browser scope.".to_string())
                        .and_then(|scope|serde_json::from_value(value["action"].clone()).map_err(|_|"Invalid browser action.".to_string())
                            .and_then(|action|generated_files::handle(&updates.borrow(),scope,action))) };
                    if let Some(respond) = js::function(&respond) { js::respond(&respond, &json!(outcome)); }
                    return JsValue::FALSE;
                }
                if value["type"] == "bridge_status_changed" {
                    spawn_local(poll_status(updates.clone()));
                    if let Some(respond) = js::function(&respond) {
                        js::respond(&respond, &json!({"ok":true}));
                    }
                }
                if value["type"] == "workspace_changed" {
                    update_workspace(
                        &updates,
                        value["workspace_id"].as_str().unwrap_or_default(),
                        value["root"].as_str().unwrap_or_default(),
                    );
                    if let Some(respond) = js::function(&respond) {
                        js::respond(&respond, &json!({"ok":true}));
                    }
                }
                if value["type"] == "provider_control" {
                    match value["action"].as_str().unwrap_or_default() {
                        "disconnect" => {
                            let mut state = updates.borrow_mut();
                            state.paused = true;
                            state.connected = false;
                            state.session_id = None;
                            state.error =
                                Some("Session disconnected by you. Use Connect to resume.".into());
                            render(&state);
                        }
                        "reconnect" => {
                            {
                                let mut state = updates.borrow_mut();
                                state.paused = false;
                                state.session_id = None;
                                state.registering = false;
                            }
                            spawn_local(register_session(updates.clone()));
                        }
                        "reinject" => {
                            let button = updates.borrow().ui.connect.clone();
                            button.click();
                        }
                        _ => {}
                    }
                    if let Some(respond) = js::function(&respond) {
                        js::respond(&respond, &json!({"ok":true}));
                    }
                }
            }
            JsValue::FALSE
        },
    );
    js::runtime_on_message_listener(&listener);
    listener.forget();
    render(&state.borrow());
    spawn_local(register_session(state.clone()));
    start_polling(state.clone());
    start_detection(state);
}

struct ContentState {
    provider: Provider,
    adapter: Adapter,
    conversation: Option<String>,
    url: String,
    ui: Ui,
    session_id: Option<String>,
    agent: Option<String>,
    workspace: String,
    workspace_id: String,
    pending_workspace_context: bool,
    host_version: String,
    yolo: Value,
    instructions: String,
    instructions_draft: Rc<RefCell<Option<String>>>,
    sending_instructions: bool,
    connected: bool,
    auto_connect: bool,
    paused: bool,
    auto_attempted: bool,
    registering: bool,
    in_flight: bool,
    bridge: String,
    error: Option<String>,
    bridge_error: Option<String>,
    adapter_warning: Option<String>,
    pending_result: Option<PendingResult>,
    sending_result: bool,
    memory: ConversationMemory,
    observed_user: Option<String>,
    session_approval: Option<ApprovalView>,
    activity: Vec<String>,
    last_text: String,
    stable_since_ms: f64,
    processed: HashSet<String>,
    last_register_ms: f64,
    route_epoch: u64,
    task_gate: Option<(String,String)>,
    continuation: Option<recovery::Continuation>,
    continuation_loaded: bool,
    recovering: bool,
}

#[derive(Clone)]
struct PendingResult {
    text: String,
    turn: DeliveryTurn,
    draft: Rc<RefCell<Option<String>>>,
    retry: Rc<RefCell<DeliveryRetry>>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SavedResult {
    text: String,
    turn: DeliveryTurn,
    draft: Option<String>,
    retry: DeliveryRetry,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct InstructionSubmission {
    context_hash: String,
    preceding_user: Option<String>,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct ConversationMemory {
    #[serde(default)]
    context_hash: String,
    #[serde(default)]
    workspace_id: String,
    instructions_sent: bool,
    task_id: Option<String>,
    #[serde(default)]
    task_goal: String,
    #[serde(default)]
    auto_continue: bool,
    #[serde(default)]
    pending: Option<SavedResult>,
    #[serde(default)]
    instruction_submission: Option<InstructionSubmission>,
    #[serde(default)]
    format_repairs: FormatRepairBudget,
}

fn memory_key(state: &ContentState) -> Option<String> {
    state
        .conversation
        .as_ref()
        .map(|conversation| format!("uab-conversation:{}:{conversation}", state.provider.slug()))
}

fn save_memory(state: &ContentState) {
    if let Some(key) = memory_key(state) {
        let memory = memory_snapshot(state);
        spawn_local(async move {
            let _ = oxichrome::storage::set(&key, &memory).await;
        });
    }
}

fn memory_snapshot(state: &ContentState) -> ConversationMemory {
    let mut memory = state.memory.clone();
    memory.pending = state.pending_result.as_ref().map(|p|SavedResult {
        text:p.text.clone(),turn:p.turn.clone(),draft:p.draft.borrow().clone(),retry:p.retry.borrow().clone()
    });
    memory
}

fn update_workspace(shared: &Rc<RefCell<ContentState>>, id: &str, root: &str) {
    let mut state = shared.borrow_mut();
    state.task_gate = None;
    if state.paused {
        state.workspace_id = id.into();
        state.workspace = root.into();
        render(&state);
        return;
    }
    if let Some(composer) = state.adapter.composer(&document()) {
        if owned_draft_matches(state.provider, state.instructions_draft.borrow().as_deref(), &composer_text(&composer)) {
            let _ = set_composer(&document(), &composer, "");
        }
    }
    state.instructions_draft = Rc::default();
    state.sending_instructions = false;
    state.workspace_id = id.into();
    state.workspace = root.into();
    state.route_epoch += 1;
    state.session_id = None;
    state.connected = false;
    state.in_flight = false;
    state.pending_result = None;
    state.sending_result = false;
    state.pending_workspace_context = true;
    state.auto_attempted = false;
    state.memory.instructions_sent = false;
    state.memory.task_id = None;
    state.memory.workspace_id = id.into();
    state.memory.auto_continue = false;
    state.error =
        Some("Workspace changed. Updated instructions are queued for this conversation.".into());
    save_memory(&state);
    render(&state);
    drop(state);
    spawn_local(register_session(shared.clone()));
}

struct Ui {
    footer: HtmlElement,
    host: HtmlElement,
    root: HtmlElement,
    body: HtmlElement,
    dot: HtmlElement,
    title: HtmlElement,
    provider_line: HtmlElement,
    session_line: HtmlElement,
    state_line: HtmlElement,
    workspace_line: HtmlElement,
    bridge_line: HtmlElement,
    warning_line: HtmlElement,
    error_line: HtmlElement,
    approval_box: HtmlElement,
    approval_text: HtmlElement,
    approve: HtmlElement,
    deny: HtmlElement,
    activity_line: HtmlElement,
    connect: HtmlElement,
    send_pending: HtmlElement,
    continue_chat: HtmlElement,
    auto_continue: HtmlElement,
    toggle: HtmlElement,
}

fn build_ui(document: &Document) -> Result<Ui, JsValue> {
    let host = document.create_element("div")?;
    host.set_id("unified-ai-bridge-host");
    host.set_attribute("data-build", env!("CARGO_PKG_VERSION"))
        .ok();
    let host_html: HtmlElement = host.clone().dyn_into()?;
    style(
        &host_html,
        &[
            ("all", "initial"),
            ("position", "fixed"),
            ("right", "18px"),
            // Keep the panel clear of the providers' bottom-anchored composer.
            ("top", "18px"),
            ("z-index", "2147483646"),
            (
                "font-family",
                "-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
            ),
            ("color-scheme", "dark"),
        ],
    );
    let shadow = host.attach_shadow(&ShadowRootInit::new(ShadowRootMode::Open))?;
    let root: HtmlElement = document.create_element("div")?.dyn_into()?;
    style(
        &root,
        &[
            ("width", "356px"),
            ("max-width", "calc(100vw - 36px)"),
            ("background", "#101820"),
            ("border", "1px solid #2d3d49"),
            ("border-radius", "10px"),
            ("box-shadow", "0 10px 32px rgba(0,0,0,0.45)"),
            ("font-size", "12px"),
            ("line-height", "1.45"),
            ("overflow", "hidden"),
            ("text-align", "left"),
        ],
    );
    let header = make(document, "div", &[], &root)?;
    style(
        &header,
        &[
            ("display", "flex"),
            ("align-items", "center"),
            ("gap", "8px"),
            ("padding", "10px 12px"),
            ("background", "#16222c"),
            ("border-bottom", "1px solid #2d3d49"),
        ],
    );
    let dot = make(document, "span", &[], &header)?;
    style(
        &dot,
        &[
            ("width", "8px"),
            ("height", "8px"),
            ("border-radius", "50%"),
            ("background", "#7a8794"),
            ("flex", "0 0 auto"),
        ],
    );
    let title = make(document, "span", &[], &header)?;
    title.set_text_content(Some("Unified AI Bridge"));
    style(
        &title,
        &[
            ("font-weight", "650"),
            ("font-size", "12px"),
            ("color", "#eef3f7"),
            ("flex", "1"),
        ],
    );
    let toggle = make(document, "button", &[], &header)?;
    toggle.set_attribute("type", "button").ok();
    toggle
        .set_attribute("aria-label", "Collapse the bridge panel")
        .ok();
    toggle.set_text_content(Some("−"));
    style(
        &toggle,
        &[
            ("background", "transparent"),
            ("border", "0"),
            ("color", "#9fb2c0"),
            ("font-size", "16px"),
            ("line-height", "1"),
            ("cursor", "pointer"),
            ("padding", "0 2px"),
        ],
    );

    let body = make(document, "div", &[], &root)?;
    style(&body, &[("padding", "11px 12px 10px"), ("max-height", "60vh"), ("overflow-y", "auto")]);
    let provider_line = make(document, "div", &[], &body)?;
    style(
        &provider_line,
        &[
            ("font-weight", "600"),
            ("color", "#e6edf3"),
            ("margin-bottom", "3px"),
        ],
    );
    let state_line = make(document, "div", &[], &body)?;
    style(
        &state_line,
        &[("color", "#a8bac7"), ("margin-bottom", "6px")],
    );
    let session_line = make(document, "div", &[], &body)?;
    style(
        &session_line,
        &[
            ("font-family", "monospace"),
            ("font-size", "9px"),
            ("overflow-wrap", "anywhere"),
            ("margin-bottom", "6px"),
        ],
    );
    let workspace_line = make(document, "div", &[], &body)?;
    style(
        &workspace_line,
        &[
            ("color", "#8395a3"),
            (
                "font-family",
                "ui-monospace, SFMono-Regular, Menlo, monospace",
            ),
            ("font-size", "10px"),
            ("white-space", "nowrap"),
            ("overflow", "hidden"),
            ("text-overflow", "ellipsis"),
            ("margin-bottom", "6px"),
        ],
    );
    let bridge_line = make(document, "div", &[], &body)?;
    style(
        &bridge_line,
        &[("color", "#8395a3"), ("margin-bottom", "6px")],
    );
    let warning_line = make(document, "div", &[], &body)?;
    style(
        &warning_line,
        &[("color", "#e0b25a"), ("margin-bottom", "6px")],
    );
    let error_line = make(document, "div", &[], &body)?;
    style(
        &error_line,
        &[
            ("color", "#ff9f93"),
            ("margin-bottom", "6px"),
            ("word-break", "break-word"),
        ],
    );

    let buttons = make(document, "div", &[], &body)?;
    style(
        &buttons,
        &[
            ("display", "flex"),
            ("gap", "6px"),
            ("flex-wrap", "wrap"),
            ("margin", "8px 0 2px"),
        ],
    );
    let connect = make(document, "button", &[], &buttons)?;
    connect.set_attribute("type", "button").ok();
    connect.set_text_content(Some("Inject tool instructions"));
    style_button(&connect);
    let send_pending = make(document, "button", &[], &buttons)?;
    send_pending.set_attribute("type", "button").ok();
    send_pending.set_text_content(Some("Send pending result"));
    style_button(&send_pending);
    let continue_chat = make(document,"button",&[],&buttons)?;
    continue_chat.set_attribute("type","button").ok();
    continue_chat.set_text_content(Some("Continue in fresh chat"));
    style_button(&continue_chat);
    let auto_continue = make(document,"button",&[],&buttons)?;
    auto_continue.set_attribute("type","button").ok();
    auto_continue.set_text_content(Some("Auto handoff: OFF"));
    style_button(&auto_continue);
    let workspace_controls = make(document, "button", &[], &buttons)?;
    workspace_controls.set_attribute("type", "button").ok();
    workspace_controls.set_text_content(Some("Workspace controls…"));
    style_button(&workspace_controls);
    let open_controls = Closure::<dyn FnMut()>::new(move || {
        spawn_local(async {
            let _ = oxichrome::runtime::send_message(&json!({"op":"open_panel"})).await;
        });
    });
    workspace_controls
        .add_event_listener_with_callback("click", open_controls.as_ref().unchecked_ref())
        .ok();
    open_controls.forget();

    let approval_box = make(document, "div", &[], &body)?;
    style(
        &approval_box,
        &[
            ("display", "none"),
            ("margin-top", "8px"),
            ("padding", "8px"),
            ("background", "#241f14"),
            ("border", "1px solid #66532a"),
            ("border-radius", "6px"),
        ],
    );
    let approval_text = make(document, "div", &[], &approval_box)?;
    style(
        &approval_text,
        &[
            ("color", "#f0d79c"),
            ("margin-bottom", "6px"),
            ("word-break", "break-word"),
        ],
    );
    let approval_buttons = make(document, "div", &[], &approval_box)?;
    style(&approval_buttons, &[("display", "flex"), ("gap", "6px")]);
    let approve = make(document, "button", &[], &approval_buttons)?;
    approve.set_attribute("type", "button").ok();
    approve.set_text_content(Some("Approve once"));
    style_button(&approve);
    let deny = make(document, "button", &[], &approval_buttons)?;
    deny.set_attribute("type", "button").ok();
    deny.set_text_content(Some("Deny"));
    style_button(&deny);

    let activity_line = make(document, "div", &[], &body)?;
    style(
        &activity_line,
        &[
            ("margin-top", "9px"),
            ("padding-top", "8px"),
            ("border-top", "1px solid #24343f"),
            ("color", "#93a6b4"),
            ("font-size", "10px"),
            ("white-space", "pre-line"),
            ("max-height", "84px"),
            ("overflow", "hidden"),
        ],
    );
    let footer = make(document, "div", &[], &root)?;
    footer.set_text_content(Some(concat!(
        "Build ",
        env!("CARGO_PKG_VERSION"),
        " · local native bridge · no API keys"
    )));
    style(
        &footer,
        &[
            ("padding", "7px 12px"),
            ("border-top", "1px solid #24343f"),
            ("color", "#6f8390"),
            ("font-size", "9px"),
            ("background", "#0d141a"),
        ],
    );

    shadow.append_child(&root)?;
    let body_el = document
        .body()
        .ok_or_else(|| JsValue::from_str("document body unavailable"))?;
    body_el.append_child(&host)?;

    Ok(Ui {
        footer,
        host: host_html,
        root,
        body,
        dot,
        title,
        provider_line,
        session_line,
        state_line,
        workspace_line,
        bridge_line,
        warning_line,
        error_line,
        approval_box,
        approval_text,
        approve,
        deny,
        activity_line,
        connect,
        send_pending,
        continue_chat,
        auto_continue,
        toggle,
    })
}

fn make(
    document: &Document,
    tag: &str,
    _attrs: &[(&str, &str)],
    parent: &Element,
) -> Result<HtmlElement, JsValue> {
    let element: HtmlElement = document.create_element(tag)?.dyn_into()?;
    parent.append_child(&element)?;
    Ok(element)
}

fn style(element: &HtmlElement, properties: &[(&str, &str)]) {
    let style = element.style();
    for (name, value) in properties {
        let _ = style.set_property(name, value);
    }
}

fn style_button(button: &HtmlElement) {
    style(
        button,
        &[
            ("background", "#22323e"),
            ("color", "#e6edf3"),
            ("border", "1px solid #3b4f5d"),
            ("border-radius", "5px"),
            ("padding", "5px 9px"),
            ("font-size", "11px"),
            ("font-family", "inherit"),
            ("cursor", "pointer"),
        ],
    );
}

fn install_buttons(state: &Rc<RefCell<ContentState>>) {
    let toggle_state = state.clone();
    let toggle = state.borrow().ui.toggle.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        let ui = &toggle_state.borrow().ui;
        let hidden = ui
            .body
            .style()
            .get_property_value("display")
            .ok()
            .as_deref()
            == Some("none");
        let _ = ui
            .body
            .style()
            .set_property("display", if hidden { "block" } else { "none" });
        ui.toggle
            .set_text_content(Some(if hidden { "−" } else { "+" }));
    });
    toggle
        .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();

    let connect_state = state.clone();
    let connect = state.borrow().ui.connect.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        let state = connect_state.clone();
        {
            let mut current = state.borrow_mut();
            current.paused = false;
            if current.in_flight || current.pending_result.is_some() {
                current.error = Some(
                    "Finish or send the pending tool result before replacing instructions.".into(),
                );
                render(&current);
                return;
            }
            current.connected = false;
            current.pending_workspace_context = true;
            current.memory.task_id = None;
        }
        spawn_local(async move {
            connect_tools(state, false).await;
        });
    });
    connect
        .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();

    let pending_state = state.clone();
    let send_pending = state.borrow().ui.send_pending.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        let state = pending_state.clone();
        if let Some(pending) = &state.borrow().pending_result { *pending.retry.borrow_mut() = DeliveryRetry::default(); }
        spawn_local(async move {
            flush_pending_result(state).await;
        });
    });
    send_pending
        .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
        .ok();
    cb.forget();

    recovery::install_buttons(state);

    for allow in [true, false] {
        let approval_state = state.clone();
        let button = {
            let state = state.borrow();
            if allow {
                state.ui.approve.clone()
            } else {
                state.ui.deny.clone()
            }
        };
        let cb = Closure::<dyn FnMut()>::new(move || {
            let state = approval_state.clone();
            spawn_local(async move {
                decide_approval(state, allow).await;
            });
        });
        button
            .add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())
            .ok();
        cb.forget();
    }
}

fn render(state: &ContentState) {
    let document = document();
    ensure_host(&document, &state.ui.host);
    let ui = &state.ui;
    ui.footer.set_text_content(Some(&format!(
        "Build {} · host {} · protocol {}",
        env!("CARGO_PKG_VERSION"),
        if state.host_version.is_empty() {
            "pending"
        } else {
            &state.host_version
        },
        bridge_core::PROTOCOL_VERSION
    )));
    let provider_label = match &state.conversation {
        Some(conversation) => format!(
            "{} · conversation {}",
            state.provider.display(),
            short_id(conversation)
        ),
        None => state.provider.display().to_string(),
    };
    ui.provider_line.set_text_content(Some(&provider_label));
    ui.session_line.set_text_content(Some(&format!(
        "session: {}",
        state.session_id.as_deref().unwrap_or("registering")
    )));
    ui.host
        .set_attribute("data-session-id", state.session_id.as_deref().unwrap_or(""))
        .ok();
    ui.host
        .set_attribute(
            "data-conversation",
            state.conversation.as_deref().unwrap_or(""),
        )
        .ok();
    let title = match state.agent.as_deref() {
        Some(agent) => format!("Unified AI Bridge · {agent}"),
        None => "Unified AI Bridge".to_string(),
    };
    ui.title.set_text_content(Some(&title));

    let error = state.error.as_deref().or(state.bridge_error.as_deref());
    let (dot, state_text) = if error.is_some() {
        if state.connected {
            ("#e0a63a", "Connected with a reported error".to_string())
        } else if state.session_id.is_some() && state.bridge == "connected" {
            ("#e0a63a", "Session ready · chat setup needs attention".to_string())
        } else {
            ("#e05a5a", "Not connected".to_string())
        }
    } else if state.in_flight {
        ("#5aa9e6", "Executing local tool…".to_string())
    } else if state.pending_result.is_some() {
        ("#e0b25a", "Result ready · composer is busy".to_string())
    } else if state.session_id.is_none() || state.bridge != "connected" {
        ("#e0a63a", "Reconnecting local session…".to_string())
    } else if state.connected {
        (
            "#43bf7f",
            "Connected · watching for tool blocks".to_string(),
        )
    } else if state.session_id.is_some() {
        ("#e0b25a", "Session ready · inject instructions".to_string())
    } else {
        ("#7a8794", "Registering local session…".to_string())
    };
    let _ = ui.dot.style().set_property("background", dot);
    ui.state_line.set_text_content(Some(&state_text));

    let workspace = if state.workspace.is_empty() {
        "workspace: waiting for the native host".to_string()
    } else {
        format!("workspace: {}", state.workspace)
    };
    ui.workspace_line.set_text_content(Some(&workspace));
    ui.workspace_line.set_attribute("title", &workspace).ok();
    ui.bridge_line
        .set_text_content(Some(&format!("native bridge: {}{}", state.bridge,
            if state.yolo["active"] == true { format!(" · YOLO ON · {}s left",state.yolo["remaining_seconds"]) } else { String::new() })));
    recovery::render(state);

    let composer_present = state.adapter.composer(&document).is_some();
    let warning = state
        .adapter_warning
        .clone()
        .or_else(|| (!composer_present).then(|| "Composer not found: sign in or the website interface may have changed; this adapter needs an update.".to_string()));
    ui.warning_line.set_text_content(warning.as_deref());
    let _ = ui
        .warning_line
        .style()
        .set_property("display", if warning.is_some() { "block" } else { "none" });
    ui.error_line.set_text_content(error);
    let _ = ui.error_line.style().set_property(
        "display",
        if error.is_some() {
            "block"
        } else {
            "none"
        },
    );

    let approval_visible = state.session_approval.is_some();
    if let Some(approval) = &state.session_approval {
        let path = approval.path.as_deref().unwrap_or("");
        let text = if path.is_empty() {
            format!("Approval required: {}\n{}", approval.tool, approval.reason)
        } else {
            format!(
                "Approval required: {}\n{}\n{}",
                approval.tool, path, approval.reason
            )
        };
        ui.approval_text.set_text_content(Some(&text));
    } else {
        ui.approval_text.set_text_content(None);
    }
    let _ = ui
        .approval_box
        .style()
        .set_property("display", if approval_visible { "block" } else { "none" });

    let _ = ui.connect.style().set_property(
        "display",
        if state.session_id.is_none() {
            "none"
        } else {
            "inline-block"
        },
    );
    ui.connect.set_text_content(Some(if state.connected {
        "Re-inject instructions"
    } else {
        "Inject tool instructions"
    }));
    let _ = ui.send_pending.style().set_property(
        "display",
        if state.pending_result.is_some() {
            "inline-block"
        } else {
            "none"
        },
    );
    let activity = if state.activity.is_empty() {
        "No tool activity in this session yet.".to_string()
    } else {
        state.activity.join("\n")
    };
    ui.activity_line.set_text_content(Some(&activity));
    let _ = ui.root.style().set_property(
        "border-color",
        if state.error.is_some() {
            "#6d3a36"
        } else {
            "#2d3d49"
        },
    );
}

/// Single-page apps can replace the body after the content script mounts.
/// The shadow host is ours, but not React's; re-append it when the site
/// removes it so the panel does not silently disappear after navigation.
fn ensure_host(document: &Document, host: &HtmlElement) {
    if document
        .get_element_by_id("unified-ai-bridge-host")
        .is_none()
    {
        if let Some(body) = document.body() {
            let _ = body.append_child(host);
        }
    }
}

/// Route changes do not reload a content script. Invalidate the old binding
/// before detecting another block or delivering an asynchronous response.
fn sync_conversation(shared: &Rc<RefCell<ContentState>>) -> bool {
    let document = document();
    let Ok(url) = document.url() else {
        return false;
    };
    let mut state = shared.borrow_mut();
    let conversation = state.provider.conversation_id(&url);
    state.url = url;
    if state.conversation == conversation {
        return false;
    }
    state.conversation = conversation;
    state.continuation_loaded = false;
    state.task_gate = None;
    state.route_epoch += 1;
    state.session_id = None;
    state.connected = false;
    state.instructions.clear();
    state.instructions_draft = Rc::default();
    state.sending_instructions = false;
    state.auto_attempted = false;
    state.last_register_ms = 0.0;
    state.pending_result = None;
    state.sending_result = false;
    state.memory = ConversationMemory::default();
    state.pending_workspace_context = false;
    state.observed_user = None;
    state.in_flight = false;
    state.session_approval = None;
    state.activity.clear();
    state.last_text.clear();
    state.stable_since_ms = 0.0;
    state.error = None;
    // Preserve processed fingerprints across SPA navigation so returning to
    // a conversation cannot execute its completed blocks again.
    render(&state);
    true
}

fn route_current(shared: &Rc<RefCell<ContentState>>, epoch: u64) -> bool {
    sync_conversation(shared);
    shared.borrow().route_epoch == epoch
}

async fn register_session(shared: Rc<RefCell<ContentState>>) {
    let (conversation, url, epoch) = {
        let mut state = shared.borrow_mut();
        if state.registering || state.paused {
            return;
        }
        state.registering = true;
        state.last_register_ms = js::now_ms();
        (
            state.conversation.clone(),
            state.url.clone(),
            state.route_epoch,
        )
    };
    // Recovery must not replace newer in-memory task/delivery state with a
    // storage write that may still be pending from this same page.
    let key = if shared.borrow().memory.workspace_id.is_empty() { memory_key(&shared.borrow()) } else { None };
    if let Some(key) = key {
        if let Ok(Some(memory)) = oxichrome::storage::get::<ConversationMemory>(&key).await {
            if route_current(&shared, epoch) {
                if let Some(p)=&memory.pending {
                    shared.borrow_mut().pending_result=Some(PendingResult {
                        text:p.text.clone(),turn:p.turn.clone(),draft:Rc::new(RefCell::new(p.draft.clone())),retry:Rc::new(RefCell::new(p.retry.clone()))
                    });
                }
                shared.borrow_mut().memory = memory;
            }
        }
    }
    let response = oxichrome::runtime::send_message(&json!({
        "op": "register",
        "conversation": conversation,
        "url": url,
    }))
    .await;
    let parsed = response.ok().and_then(js::from_js::<BackgroundResponse>);
    if !route_current(&shared, epoch) {
        shared.borrow_mut().registering = false;
        return;
    }
    if shared.borrow().paused { shared.borrow_mut().registering = false; return; }
    {
        let mut state = shared.borrow_mut();
        state.registering = false;
        match parsed {
            Some(response) if response.ok => {
                let instructions = response.instructions.unwrap_or_default();
                let context_hash = request_key(
                    state.provider.slug(),
                    &response.workspace_id,
                    &instructions,
                    0,
                );
                if bridge_core::recovery::context_changed(&state.memory.workspace_id,&state.memory.context_hash,&response.workspace_id,&context_hash)
                {
                    if state.memory.workspace_id != response.workspace_id {
                        state.pending_result = None;
                        state.memory.auto_continue = false;
                    }
                    state.pending_workspace_context = true;
                    state.memory.instructions_sent = false;
                    state.memory.task_id = None;
                }
                state.workspace_id = response.workspace_id;
                state.memory.workspace_id = state.workspace_id.clone();
                state.session_id = response.session_id;
                state.agent = response.agent;
                state.workspace = response.workspace;
                state.instructions = instructions;
                state.auto_connect = response.auto_connect;
                state.bridge = if response.bridge.is_empty() {
                    "connected".into()
                } else {
                    response.bridge
                };
                state.error = None;
                state.adapter_warning = None;
            }
            Some(response) => {
                state.error = Some(
                    response
                        .error
                        .unwrap_or_else(|| "The local bridge refused this session.".into()),
                );
                state.bridge = "disconnected".into();
            }
            None => {
                state.error = Some(
                    "No response from the extension background worker. Reload the extension."
                        .into(),
                );
            }
        }
        render(&state);
    }
    if recovery::restore_and_drive(shared.clone()).await { return; }
    maybe_auto_connect(shared).await;
}

async fn connect_tools(shared: Rc<RefCell<ContentState>>, automatic: bool) {
    if shared.borrow().sending_instructions {
        return;
    }
    let epoch = shared.borrow().route_epoch;
    let (session_missing, instructions_empty, connected, adapter, instructions) = {
        let state = shared.borrow();
        (
            state.session_id.is_none(),
            state.instructions.is_empty(),
            state.connected,
            state.adapter,
            state.instructions.clone(),
        )
    };
    if session_missing || instructions_empty || connected {
        if session_missing || instructions_empty {
            let mut state = shared.borrow_mut();
            state.error = Some(if session_missing {
                "Wait for the local session to register before connecting.".into()
            } else {
                "The native host did not provide tool instructions. Re-register the tab.".into()
            });
            render(&state);
        }
        return;
    }
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let context_hash = {
        let s=shared.borrow();
        request_key(s.provider.slug(),&s.workspace_id,&instructions,0)
    };
    let submitted = shared.borrow().memory.instruction_submission.clone();
    if let Some(submitted) = submitted {
        let confirmed = submitted.context_hash == context_hash && adapter.last_user(&document).is_some_and(|user|
            adapter.user_text(&user).ok().is_some_and(|text| bridge_core::recovery::submitted_message_matches(submitted.preceding_user.as_deref(),&adapter.user_id(&user),&instructions,&text)));
        if confirmed {
            let mut s=shared.borrow_mut();
            s.connected=true;
            s.auto_attempted=true;
            s.pending_workspace_context=false;
            s.memory.instructions_sent=true;
            s.memory.context_hash=context_hash.clone();
            s.memory.instruction_submission=None;
            s.memory.task_id=None;
            s.error=None;
            save_memory(&s);
            render(&s);
            return;
        }
        if automatic {
            let mut s=shared.borrow_mut();
            s.auto_attempted=true;
            if s.error.is_none() {
                s.error=Some("Setup submission is unconfirmed. Automatic retries are paused. Inspect this conversation before clicking Inject tool instructions.".into());
            }
            render(&s);
            return;
        }
    }
    // A page reload starts a fresh content script. If the instruction message is
    // already in this conversation, reconnect without sending it a second time.
    if !shared.borrow().pending_workspace_context
        && (shared.borrow().memory.instructions_sent
            || instructions_already_sent(adapter, &document))
    {
        let mut state = shared.borrow_mut();
        state.connected = true;
        state.auto_attempted = true;
        state.error = None;
        state.adapter_warning = None;
        state.memory.instructions_sent = true;
        state.memory.context_hash = request_key(state.provider.slug(),&state.workspace_id,&state.instructions,0);
        save_memory(&state);
        render(&state);
        return;
    }
    if automatic {
        // Conversation DOM is hydrated after document_idle. Wait for the
        // already-sent instruction message to appear before injecting again.
        {
            let mut state = shared.borrow_mut();
            if state.auto_attempted {
                return;
            }
            state.auto_attempted = true;
        }
        // Workspace changes already establish that new context is required.
        // Avoid eight timer waits in background tabs, where Chrome throttles
        // each callback and can delay reinjection for minutes.
        let hydration_checks = if shared.borrow().pending_workspace_context {
            0
        } else {
            8
        };
        for _ in 0..hydration_checks {
            if !route_current(&shared, epoch) {
                return;
            }
            if !shared.borrow().pending_workspace_context
                && instructions_already_sent(adapter, &document)
            {
                let mut state = shared.borrow_mut();
                state.connected = true;
                state.error = None;
                state.adapter_warning = None;
                state.memory.instructions_sent = true;
                state.memory.context_hash = request_key(state.provider.slug(),&state.workspace_id,&state.instructions,0);
                save_memory(&state);
                render(&state);
                return;
            }
            sleep_ms(350).await;
        }
    }
    match adapter.composer(&document) {
        Some(composer)
            if composer_is_empty(&composer)
                || owned_draft_matches(adapter.provider, shared.borrow().instructions_draft.borrow().as_deref(), &composer_text(&composer)) => {}
        Some(_) => {
            let mut state = shared.borrow_mut();
            if state.pending_workspace_context {
                state.auto_attempted = false;
            }
            state.error = Some("The message box contains a draft. Clear it, then connect again; the bridge never overwrites your text.".into());
            render(&state);
            return;
        }
        None => {
            let mut state = shared.borrow_mut();
            if automatic {
                state.auto_attempted = false;
            }
            state.error = Some(
                "The provider's message box could not be found. The website may have changed."
                    .into(),
            );
            render(&state);
            return;
        }
    }
    sleep_ms(if automatic { 100 } else { 100 }).await;
    if !route_current(&shared, epoch) {
        return;
    }
    if adapter.is_generating(&document) {
        let mut state = shared.borrow_mut();
        state.auto_attempted = false;
        state.error = Some("Updated instructions are queued until the assistant finishes.".into());
        render(&state);
        return;
    }
    shared.borrow_mut().sending_instructions = true;
    shared.borrow_mut().memory.instruction_submission=Some(InstructionSubmission {
        context_hash,
        preceding_user:adapter.last_user(&document).map(|u|adapter.user_id(&u)),
    });
    let saved = { let s=shared.borrow();memory_key(&s).map(|key|(key,memory_snapshot(&s))) };
    if let Some((key,memory))=saved {
        if oxichrome::storage::set(&key,&memory).await.is_err() {
            let mut s=shared.borrow_mut();
            s.sending_instructions=false;
            s.auto_attempted=true;
            s.error=Some("Could not save setup delivery state. No setup message was submitted.".into());
            render(&s);
            return;
        }
    }
    let draft = shared.borrow().instructions_draft.clone();
    let context_current = || route_current(&shared, epoch);
    match adapter
        .send_guarded_message(
            &document,
            &instructions,
            None,
            Some(&draft),
            Some(&context_current),
        )
        .await
    {
        Ok(()) => {
            if !route_current(&shared, epoch) {
                return;
            }
            let mut state = shared.borrow_mut();
            state.connected = true;
            state.sending_instructions = false;
            state.instructions_draft = Rc::default();
            state.memory.instructions_sent = true;
            state.memory.instruction_submission = None;
            state.memory.context_hash = request_key(
                state.provider.slug(),
                &state.workspace_id,
                &state.instructions,
                0,
            );
            state.memory.task_id = None;
            state.pending_workspace_context = false;
            save_memory(&state);
            state.error = None;
            render(&state);
        }
        Err(err) => {
            if !route_current(&shared, epoch) {
                return;
            }
            let mut state = shared.borrow_mut();
            state.sending_instructions = false;
            state.auto_attempted = true;
            state.error = Some(format!("Could not confirm tool instructions: {err} Inspect the conversation before clicking Inject tool instructions; automatic retries are paused."));
            save_memory(&state);
            render(&state);
        }
    }
}

async fn maybe_auto_connect(state: Rc<RefCell<ContentState>>) {
    let (enabled, attempted, connected, has_session) = {
        let state = state.borrow();
        (
            state.auto_connect,
            state.auto_attempted,
            state.connected,
            state.session_id.is_some(),
        )
    };
    if (enabled || state.borrow().pending_workspace_context)
        && (!attempted || state.borrow().memory.instruction_submission.is_some())
        && !connected
        && has_session
    {
        connect_tools(state, true).await;
    }
}

fn start_polling(state: Rc<RefCell<ContentState>>) {
    let poll_state = state.clone();
    let poll = Closure::<dyn FnMut()>::new(move || {
        let state = poll_state.clone();
        spawn_local(async move {
            poll_status(state).await;
        });
    });
    if let Some(window) = web_sys::window() {
        let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
            poll.as_ref().unchecked_ref(),
            POLL_MS,
        );
    }
    poll.forget();

    let detect_state = state.clone();
    let detect = Closure::<dyn FnMut()>::new(move || detect_tool_request(detect_state.clone()));
    if let Some(window) = web_sys::window() {
        let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
            detect.as_ref().unchecked_ref(),
            DETECT_MS,
        );
    }
    detect.forget();
    // DOM changes wake background tabs without depending on throttled timers.
    // The timestamp stability check still requires three seconds of unchanged text.
    let observed = state.clone();
    let callback =
        Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |_, _| {
            // Repair our host in the observer itself: scheduling even a zero-delay
            // timer can leave a hidden tab without its panel for a full minute.
            // This only reattaches the existing UI; tool detection stays deferred.
            ensure_host(&document(), &observed.borrow().ui.host);
            let shared = observed.clone();
            js::schedule(0, move || {
                detect_tool_request(shared.clone());
                // Navigation and handoff acknowledgements must not wait for
                // Chrome's heavily throttled background-tab interval.
                let recover={let s=shared.borrow();s.continuation.is_some()&&!s.recovering};
                if recover {spawn_local(poll_status(shared));}
            });
        });
    if let Ok(observer) = web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()) {
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        options.set_character_data(true);
        let _ = observer.observe_with_options(&document(), &options);
        // The observer must remain alive for this content script's lifetime.
        std::mem::forget(observer);
        callback.forget();
    }
}

async fn poll_status(state: Rc<RefCell<ContentState>>) {
    if state.borrow().paused {
        return;
    }
    sync_conversation(&state);
    let snapshot = { let s=state.borrow(); (s.route_epoch,s.session_id.clone(),s.registering) };
    let response = oxichrome::runtime::send_message(&json!({"op":"status"})).await;
    let Some(status) = response.ok().and_then(js::from_js::<StatusView>) else {
        return;
    };
    // An earlier status response cannot invalidate a newer registration or route.
    { let s=state.borrow(); if s.paused || snapshot != (s.route_epoch,s.session_id.clone(),s.registering) { return; } }
    if !state.borrow().workspace_id.is_empty()
        && !status.active_id.is_empty()
        && state.borrow().workspace_id != status.active_id
    {
        update_workspace(&state, &status.active_id, &status.workspace);
        return;
    }
    let session_id = state.borrow().session_id.clone();
    let mut should_register = false;
    {
        let mut state = state.borrow_mut();
        state.bridge = if status.bridge != "connected" && status.reconnect_attempt > 0 {
            format!("reconnecting (attempt {})", status.reconnect_attempt)
        } else if status.bridge.is_empty() {
            "disconnected".into()
        } else {
            status.bridge.clone()
        };
        if !status.workspace.is_empty() {
            state.workspace = status.workspace.clone();
        }
        state.auto_connect = status.auto_connect;
        state.host_version = status.host_version.clone();
        state.yolo = status.yolo.clone();
        if !state.registering && crate::model::session_missing(&status,state.session_id.as_deref()) {
            state.session_id = None;
            state.session_approval = None;
            state.last_register_ms = 0.0;
            // Keep processed calls, task identity and pending results intact.
        }
        if let Some(task) = status.task_states.iter().find(|t|
            t["workspace_id"].as_str()==Some(&state.workspace_id)
            && t["provider"].as_str()==Some(state.provider.slug())
            && t["conversation"].as_str()==state.conversation.as_deref()
            && t["task_id"].as_str()==state.memory.task_id.as_deref()) {
            if task["state"] == "running" && state.task_gate.is_some() { state.adapter_warning=None; }
            state.task_gate = if task["state"] == "running" { None } else {
                Some((task["task_id"].as_str().unwrap_or_default().into(), task["state"].as_str().unwrap_or("halted").into()))
            };
        }
        if let Some(session_id) = session_id.as_deref() {
            state.session_approval = status
                .approvals
                .iter()
                .find(|approval| approval.session_id == session_id)
                .cloned();
            state.activity = status
                .activity
                .iter()
                .filter(|activity| {
                    activity.session_id == session_id || activity.tool == "workspace"
                })
                .take(3)
                .map(activity_label)
                .collect();
        }
        if state.session_id.is_none()
            && status.bridge == "connected"
            && !state.registering
            && js::now_ms() - state.last_register_ms > 12_000.0
        {
            should_register = true;
        }
        // Transport status is replaceable; local draft/tool errors retain
        // their own lifecycle and must not disappear when the host reconnects.
        state.bridge_error = status.last_error.clone();
        render(&state);
    }
    if should_register {
        spawn_local(register_session(state.clone()));
        return;
    }
    if recovery::restore_and_drive(state.clone()).await { return; }
    maybe_auto_connect(state.clone()).await;
    flush_pending_result(state).await;
}

fn activity_label(activity: &ActivityView) -> String {
    let decision = if activity.decision.is_empty() {
        String::new()
    } else {
        format!(" · {}", activity.decision)
    };
    format!("{} · {}{}", activity.tool, activity.summary, decision)
}

fn start_detection(state: Rc<RefCell<ContentState>>) {
    // The interval is installed by start_polling; this function exists to keep
    // initialization readable and to trigger one immediate check after load.
    detect_tool_request(state);
}

fn detect_tool_request(state: Rc<RefCell<ContentState>>) {
    if sync_conversation(&state) {
        return;
    }
    let (connected, busy, session_id) = {
        let state = state.borrow();
        (
            state.connected,
            state.in_flight || state.pending_result.is_some() || state.sending_result || state.recovering || state.continuation.is_some(),
            state.session_id.clone(),
        )
    };
    if !connected || busy || session_id.is_none() {
        return;
    }
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let adapter = state.borrow().adapter;
    if recovery::website_block(adapter, &document) == Some(WebsiteBlock::AccountChanged) {
        let mut current = state.borrow_mut();
        let message = WebsiteBlock::AccountChanged.message();
        if current.error.as_deref() != Some(message) {
            current.error = Some(message.into());
            render(&current);
        }
        return;
    }
    let Some(turn) = adapter.delivery_turn(&document) else {
        // A streaming reply may not have its permanent message ID yet. This
        // is normal on Kimi; do not report an adapter failure while it streams.
        if adapter.is_generating(&document) {
            return;
        }
        let mut current = state.borrow_mut();
        if adapter.grok_on_x && adapter.last_assistant(&document).is_none()
            && !adapter.is_generating(&document)
        {
            current.adapter_warning = Some("Grok on X: waiting for a completed reply with Regenerate and Copy text controls. If a reply is already visible, the assistant selector needs an update; tool execution is paused.".into());
            render(&current);
        }
        if adapter
            .last_assistant(&document)
            .is_some_and(|node| adapter.response_id(&node).is_none())
            || adapter.last_user(&document).is_some_and(|node| adapter.user_id(&node).is_empty())
        {
            current.adapter_warning = Some(format!("This website's interface changed; the adapter needs an update. {} message ID selector failed{}.", current.provider.display(), if adapter.provider == Provider::Kimi { " (.chat-content-item[data-archer-id])" } else { "" }));
            render(&current);
        }
        return;
    };
    let Some(user) = adapter.last_user(&document) else {
        return;
    };
    let user_text = match adapter.user_text(&user) {
        Ok(text) => text,
        Err(warning) => {
            let mut current = state.borrow_mut();
            current.adapter_warning = Some(warning);
            render(&current);
            return;
        }
    };
    if user_text.trim().is_empty() {
        let mut current = state.borrow_mut();
        current.adapter_warning = Some("Waiting for the latest user message to render. Bring this chat into view; empty or hidden message text cannot authorize tool execution.".into());
        render(&current);
        return;
    }
    let task_id = {
        let mut current = state.borrow_mut();
        let first_observation = current.observed_user.is_none();
        let changed = current.observed_user.as_deref() != Some(&turn.user);
        current.observed_user = Some(turn.user.clone());
        if user_text.contains(INSTRUCTIONS_MARKER) && !user_text.contains(recovery::HANDOFF_MARKER) {
            current.memory.task_id = None;
        } else if !user_text.contains(protocol::RESULT_OPEN) && changed && !first_observation {
            current.memory.task_id = Some(turn.user.clone());
            current.memory.task_goal = user_text.clone();
            current.error = None;
        }
        if changed {
            save_memory(&current);
        }
        let Some(task) = current.memory.task_id.clone() else {
            current.adapter_warning = Some(
                "Connected. Waiting for a new user task; setup does not execute tools.".into(),
            );
            render(&current);
            return;
        };
        task
    };
    let task_gate = state.borrow().task_gate.clone();
    if let Some((id, gate)) = task_gate {
        if id == task_id {
            let mut current = state.borrow_mut();
            current.adapter_warning = Some(format!("Chat task {gate}. Use the extension's Chat task controls. Already started commands have separate Stop controls."));
            current.processed.insert(format!("{}:{}",turn.conversation,turn.assistant));
            render(&current);
            return;
        }
    }
    if adapter.is_generating(&document) {
        return;
    }
    let Some(node) = adapter.last_assistant(&document) else {
        let mut state = state.borrow_mut();
        state.adapter_warning = Some(if adapter.composer(&document).is_some() {
            "No assistant reply found yet. Send a prompt after connecting.".into()
        } else {
            "Composer not found: sign in or the website interface may have changed.".into()
        });
        render(&state);
        return;
    };
    let text = match adapter.reply_text(&node) {
        Ok(text) => text,
        Err(error) => {
            if adapter.qwen_tool_editor_complete(&node) {
                // The editor may omit lines. Ask for a complete plain-text
                // rendering; never interpret or execute its visible prefix.
                let text = assistant_text(&node);
                if state.borrow().last_text != text {
                    let mut current = state.borrow_mut();
                    current.last_text = text;
                    current.stable_since_ms = js::now_ms();
                    current.adapter_warning = Some(error);
                    render(&current);
                    drop(current);
                    let wake = state.clone();
                    js::schedule(3_100, move || detect_tool_request(wake));
                    return;
                }
                if js::now_ms() - state.borrow().stable_since_ms < 3_000.0 { return; }
                let mut current = state.borrow_mut();
                if current.memory.format_repairs.reserve(&task_id, &turn.assistant) {
                    current.pending_result = Some(PendingResult {
                        text: format_tool_results(&[json!({
                            "status":"error", "code":"adapter_incomplete_render", "executed":false,
                            "error":"Qwen displayed the requested tool JSON in a code editor that may hide lines. The bridge did not execute any call from that reply. Re-emit the same intended request as ONE plain-text line: [[BRIDGE_TOOL]] followed by the complete strict JSON object followed by [[/BRIDGE_TOOL]]. No Markdown fence, backticks, code editor, or list. Preserve every argument exactly. This is a formatting correction for the original task, not a new task or extra call allowance. Wait for the actual result. At most two automatic formatting corrections are available per task."
                        })]),
                        turn: turn.clone(), draft: Rc::default(), retry: Rc::default(),
                    });
                    current.adapter_warning = Some("Qwen hid the tool request in a code editor. A formatting correction is queued; no local operation from that reply was executed.".into());
                    save_memory(&current);
                    render(&current);
                    drop(current);
                    let shared = state.clone();
                    spawn_local(async move { flush_pending_result(shared).await; });
                    return;
                }
            }
            let mut current = state.borrow_mut();
            current.adapter_warning = Some(error);
            render(&current);
            return;
        }
    };
    if text.trim().is_empty() {
        return;
    }
    if state.borrow().last_text != text {
        let mut current = state.borrow_mut();
        current.last_text = text.clone();
        current.stable_since_ms = js::now_ms();
        current.adapter_warning = None;
        drop(current);
        let wake = state.clone();
        js::schedule(3_100, move || detect_tool_request(wake));
        return;
    }
    let ready = js::now_ms() - state.borrow().stable_since_ms >= 3_000.0;
    if !ready {
        return;
    }
    let fingerprint = format!("{}:{}", turn.conversation, turn.assistant);
    if state.borrow().processed.contains(&fingerprint) {
        if adapter.grok_on_x {
            let mut current = state.borrow_mut();
            current.adapter_warning = Some("This Grok on X reply was already handled. Identical prompts and regenerated replies are not rerun; send a new, distinct task.".into());
            render(&current);
        }
        return;
    }
    let parsed = protocol::parse_tool_requests(&text);
    if parsed.calls.is_empty() {
        if let Some(problem) = parsed.problems.first() {
            state.borrow_mut().error = Some(format!("Malformed tool block: {}", problem.message));
            render(&state.borrow());
        } else {
            let mut current = state.borrow_mut();
            current.memory.task_id = None;
            save_memory(&current);
        }
        state.borrow_mut().processed.insert(fingerprint);
        return;
    }
    {
        let mut state = state.borrow_mut();
        state.in_flight = true;
        state.processed.insert(fingerprint);
        state.error = None;
        render(&state);
    }
    let session_id = session_id.unwrap();
    let epoch = state.borrow().route_epoch;
    spawn_local(async move {
        let mut payloads = Vec::new();
        for (index, ParsedCall { call, .. }) in parsed.calls.iter().enumerate() {
            if !delivery_current(&state, epoch, &turn) {
                discard_stale_result(&state, epoch);
                return;
            }
            let tool = call.tool.clone();
            let arguments = call.arguments.clone();
            let key = request_key(
                state.borrow().provider.slug(),
                &turn.conversation,
                &turn.assistant,
                index as u32,
            );
            let workspace_id = state.borrow().workspace_id.clone();
            let result = call_bridge(
                &session_id,
                &workspace_id,
                &key,
                &turn.assistant,
                &task_id,
                index as u32,
                &tool,
                arguments,
            )
            .await;
            match result {
                Ok(response) if response.ok => {
                    payloads.push(response.result.unwrap_or_else(|| json!({"status":"ok"})));
                }
                Ok(response) => {
                    payloads.push(json!({
                        "status":"error",
                        "tool": tool,
                        "error": response.error.unwrap_or_else(|| "The bridge rejected this request.".into()),
                        "code": response.code,
                    }));
                }
                Err(err) => {
                    payloads.push(json!({"status":"error","tool":tool,"error":err}));
                }
            }
            if payloads
                .last()
                .and_then(|p| p.get("code"))
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("budget_exceeded") || matches!(code,"task_halted"|"task_paused"|"task_stopped"))
            {
                let mut current = state.borrow_mut();
                if payloads.last().and_then(|p|p["code"].as_str()).is_some_and(|c|c.starts_with("task_")&&!c.ends_with("budget_exceeded")) {
                    current.task_gate=Some((task_id.clone(),"halted".into()));
                } else { current.memory.task_id = None; }
                save_memory(&current);
                break;
            }
        }
        let result_text = format_tool_results(&payloads);
        if !delivery_current(&state, epoch, &turn) {
            discard_stale_result(&state, epoch);
            return;
        }
        let mut state_ref = state.borrow_mut();
        state_ref.pending_result = Some(PendingResult {
            text: result_text,
            turn,
            draft: Rc::default(),
            retry: Rc::default(),
        });
        state_ref.in_flight = false;
        save_memory(&state_ref);
        render(&state_ref);
        drop(state_ref);
        flush_pending_result(state.clone()).await;
    });
}

async fn call_bridge(
    session_id: &str,
    workspace_id: &str,
    request_key: &str,
    response_id: &str,
    task_id: &str,
    call_index: u32,
    tool: &str,
    arguments: Value,
) -> Result<BackgroundResponse, String> {
    let response = oxichrome::runtime::send_message(&json!({
        "op":"tool",
        "session_id": session_id,
        "workspace_id": workspace_id,
        "request_key": request_key,
        "response_id": response_id,
        "task_id": task_id,
        "call_index": call_index,
        "tool": tool,
        "arguments": arguments,
    }))
    .await
    .map_err(|err| format!("Extension background unavailable: {err}"))?;
    js::from_js::<BackgroundResponse>(response)
        .ok_or_else(|| "The bridge returned an invalid response.".to_string())
}

async fn flush_pending_result(state: Rc<RefCell<ContentState>>) {
    if sync_conversation(&state) {
        return;
    }
    let epoch = state.borrow().route_epoch;
    let pending = state.borrow().pending_result.clone();
    let Some(pending) = pending else {
        return;
    };
    if state.borrow().in_flight || state.borrow().sending_result || state.borrow().recovering
        || state.borrow().task_gate.as_ref().is_some_and(|(_,s)|matches!(s.as_str(),"transferring"|"stopped")) {
        return;
    }
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let adapter = state.borrow().adapter;
    // Account changes can also change rendered turn IDs. Preserve the saved
    // result and its draft before considering stale-turn reconciliation.
    if recovery::website_block(adapter, &document) == Some(WebsiteBlock::AccountChanged) {
        state.borrow_mut().error = Some(WebsiteBlock::AccountChanged.message().into());
        render(&state.borrow());
        return;
    }
    let latest_user = match adapter.last_user(&document) {
        Some(user) => match adapter.user_text(&user) {
            Ok(text) => Some((adapter.user_id(&user), text)),
            Err(warning) => {
                state.borrow_mut().adapter_warning = Some(warning);
                render(&state.borrow());
                return;
            }
        },
        None => None,
    };
    match pending_result_state(
        &pending.turn,
        adapter.delivery_turn(&document).as_ref(),
        latest_user.as_ref().map(|(id, text)| (id.as_str(), text.as_str())),
        &pending.text,
        pending.retry.borrow().submission_uncertain,
    ) {
        PendingResultState::WaitingForTranscript => {
            state.borrow_mut().adapter_warning = Some("A tool result is saved while this website renders its latest message. Bring the chat into view; an unconfirmed submission will not be sent twice.".into());
            render(&state.borrow());
            return;
        }
        PendingResultState::Delivered => {
            let mut current = state.borrow_mut();
            current.pending_result = None;
            current.error = None;
            current.adapter_warning = None;
            save_memory(&current);
            render(&current);
            return;
        }
        PendingResultState::Stale => {
            discard_stale_result(&state, epoch);
            return;
        }
        PendingResultState::Current => {}
    }
    if let Some(block)=recovery::website_block(adapter,&document) {
        state.borrow_mut().error=Some(block.message().into());
        render(&state.borrow());
        if block==WebsiteBlock::ContextFull && state.borrow().memory.auto_continue {
            recovery::begin(state.clone()).await;
        }
        return;
    }
    if !pending.retry.borrow().ready(js::now_ms()) { return; }
    if adapter.is_generating(&document) {
        state.borrow_mut().error = Some(
            "Result queued while the assistant is busy. It will send when the composer is ready."
                .into(),
        );
        render(&state.borrow());
        return;
    }
    let composer_ready = adapter
        .composer(&document)
        .map(|composer| {
            composer_is_empty(&composer)
                || owned_draft_matches(adapter.provider, pending.draft.borrow().as_deref(), &composer_text(&composer))
        })
        .unwrap_or(false);
    if !composer_ready {
        state.borrow_mut().error = Some("A tool result is ready, but the message box is not empty. Clear it and press “Send pending result”.".into());
        render(&state.borrow());
        return;
    }
    state.borrow_mut().sending_result = true;
    // Persist the uncertain-delivery boundary before touching the submit
    // control. A reload between clicking and acknowledgement must not resend.
    pending.retry.borrow_mut().submission_uncertain = true;
    let saved = {let s=state.borrow(); memory_key(&s).map(|key|(key,memory_snapshot(&s)))};
    if let Some((key,memory))=saved {
        if oxichrome::storage::set(&key,&memory).await.is_err() {
            state.borrow_mut().sending_result=false;
            state.borrow_mut().error=Some("Could not save pending delivery. No message was submitted; use Send pending result after storage recovers.".into());
            render(&state.borrow());
            return;
        }
    }
    let context_current = || route_current(&state, epoch);
    match adapter
        .send_guarded_message(
            &document,
            &pending.text,
            Some(&pending.turn),
            Some(&pending.draft),
            Some(&context_current),
        )
        .await
    {
        Ok(()) => {
            if !route_current(&state, epoch) {
                return;
            }
            let mut state = state.borrow_mut();
            state.pending_result = None;
            state.sending_result = false;
            state.error = None;
            state.connected = true;
            save_memory(&state);
            render(&state);
        }
        Err(err) => {
            if !route_current(&state, epoch) {
                return;
            }
            let mut state = state.borrow_mut();
            state.sending_result = false;
            let rejected_by_context = recovery::website_block(adapter,&document)==Some(WebsiteBlock::ContextFull)
                && pending.turn.is_current(adapter.delivery_turn(&document).as_ref());
            if rejected_by_context {pending.retry.borrow_mut().submission_uncertain=false;}
            pending.retry.borrow_mut().failed(js::now_ms(), !rejected_by_context);
            state.error = Some(format!("Could not return the tool result: {err}"));
            save_memory(&state);
            render(&state);
        }
    }
}

fn delivery_current(state: &Rc<RefCell<ContentState>>, epoch: u64, turn: &DeliveryTurn) -> bool {
    route_current(state, epoch)
        && turn.is_current(state.borrow().adapter.delivery_turn(&document()).as_ref())
}

fn discard_stale_result(state: &Rc<RefCell<ContentState>>, epoch: u64) {
    if !route_current(state, epoch) {
        return;
    }
    let mut state = state.borrow_mut();
    if let Some(pending) = &state.pending_result {
        if let Some(composer) = state.adapter.composer(&document()) {
            if owned_draft_matches(state.provider, pending.draft.borrow().as_deref(), &composer_text(&composer)) {
                let _ = set_composer(&document(), &composer, "");
            }
        }
    }
    state.pending_result = None;
    state.in_flight = false;
    state.sending_result = false;
    save_memory(&state);
    state.error = Some("A result belongs to an older conversation turn. It was kept in local activity and was not sent to this turn.".into());
    render(&state);
}

async fn decide_approval(state: Rc<RefCell<ContentState>>, allow: bool) {
    let approval_id = {
        let state = state.borrow();
        match &state.session_approval {
            Some(approval) => approval.id.clone(),
            None => return,
        }
    };
    let response = oxichrome::runtime::send_message(&json!({
        "op":"approval",
        "approval_id": approval_id,
        "allow": allow,
    }))
    .await;
    match response.ok().and_then(js::from_js::<BackgroundResponse>) {
        Some(response) if response.ok => {
            let mut state = state.borrow_mut();
            state.session_approval = None;
            state.error = None;
            render(&state);
        }
        Some(response) => {
            state.borrow_mut().error = Some(
                response
                    .error
                    .unwrap_or_else(|| "Approval could not be recorded.".into()),
            );
            render(&state.borrow());
        }
        None => {
            state.borrow_mut().error =
                Some("The extension background did not answer the approval.".into());
            render(&state.borrow());
        }
    }
}

fn composer_text(composer: &HtmlElement) -> String {
    if let Some(textarea) = composer.dyn_ref::<HtmlTextAreaElement>() {
        return textarea.value();
    }
    composer.text_content().unwrap_or_default()
}

fn composer_is_empty(composer: &HtmlElement) -> bool {
    composer_text(composer).trim().is_empty()
}

fn instructions_already_sent(adapter: Adapter, document: &Document) -> bool {
    let composer = adapter.composer(document);
    let draft = composer.as_ref().map(composer_text).unwrap_or_default();
    if draft.contains(INSTRUCTIONS_MARKER) {
        return false;
    }
    adapter.users(document).iter().any(|user| {
        adapter.user_text(user).ok().is_some_and(|text|
            text.contains(INSTRUCTIONS_MARKER) || text.contains(protocol::RESULT_OPEN))
    })
}

fn assistant_text(node: &Element) -> String {
    // Claude's outer assistant wrapper contains an sr-only announcement that
    // repeats and truncates the response. Mixing it with the visible reply can
    // turn valid JSON into an unterminated string, or execute an announcement
    // instead of the displayed text. Read the observed transcript roots only;
    // keep the outer node for stable turn identity and completion checks.
    let text = if node.get_attribute("data-cds").as_deref() == Some("AssistantMessage") {
        node.query_selector_all("[data-transcript-engine-root]")
            .ok()
            .map(|roots| {
                (0..roots.length())
                    .filter_map(|i| roots.item(i)?.text_content())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    } else {
        node.text_content().unwrap_or_default()
    };
    let trimmed = text.trim();
    if trimmed.len() > 240_000 {
        let mut boundary = trimmed.len() - 240_000;
        while !trimmed.is_char_boundary(boundary) {
            boundary += 1;
        }
        trimmed[boundary..].to_string()
    } else {
        trimmed.to_string()
    }
}

fn short_id(value: &str) -> String {
    if value.len() <= 12 {
        value.to_string()
    } else {
        format!("{}…", &value[..12])
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn document() -> Document {
    web_sys::window()
        .and_then(|window| window.document())
        .expect("content script document")
}

async fn sleep_ms(milliseconds: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, milliseconds);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[derive(Clone, Copy)]
struct Adapter {
    provider: Provider,
    grok_on_x: bool,
}

impl Adapter {
    fn for_url(provider: Provider, url: &str) -> Self {
        Self {
            provider,
            grok_on_x: provider == Provider::Grok && url.starts_with("https://x.com/"),
        }
    }

    fn composers(self) -> &'static [&'static str] {
        match self.provider {
            Provider::ChatGpt => &[
                "#prompt-textarea",
                "div[contenteditable=\"true\"].ProseMirror",
                "div[contenteditable=\"true\"]",
                "textarea",
            ],
            Provider::Claude => &[
                "div[contenteditable=\"true\"].ProseMirror",
                "div[contenteditable=\"true\"]",
                "fieldset textarea",
                "textarea",
            ],
            Provider::DeepSeek => &[
                "textarea#chat-input",
                "textarea[placeholder]",
                "textarea",
                "div[contenteditable=\"true\"]",
            ],
            Provider::Gemini => &[
                "rich-textarea div[contenteditable=\"true\"]",
                "div.ql-editor[contenteditable=\"true\"]",
                "div[contenteditable=\"true\"]",
                "textarea",
            ],
            Provider::Grok => &[
                "div[contenteditable=\"true\"][aria-label=\"Message Grok\"]",
                "div[contenteditable=\"true\"][role=\"textbox\"]",
                "textarea[placeholder]",
                "textarea",
                "div[contenteditable=\"true\"]",
            ],
            Provider::Qwen => &["textarea.message-input-textarea"],
            Provider::Kimi => &[".chat-editor .chat-input-editor[contenteditable=\"true\"]"],
        }
    }

    fn send_buttons(self) -> &'static [&'static str] {
        if self.grok_on_x {
            return &["button[aria-label=\"Grok something\"]", "button[aria-label=\"Send\"]"];
        }
        match self.provider {
            Provider::ChatGpt => &[
                "button[data-testid=\"send-button\"]",
                "button[aria-label=\"Send prompt\"]",
                "button[aria-label=\"Send message\"]",
                "button[aria-label*=\"Send\"]",
                "button[aria-label*=\"send\"]",
                "button[type=\"submit\"]",
            ],
            Provider::Claude => &[
                "button[aria-label=\"Send Message\"]",
                "button[aria-label*=\"Send\"]",
                "button[aria-label*=\"send\"]",
            ],
            Provider::DeepSeek => &[
                // DeepSeek's send control is a div in its design system, not a
                // <button>. It only appears after the controlled textarea sees
                // a real input event, so this must be paired with the textarea
                // insertion path in set_composer().
                "div.ds-button--primary.ds-button--circle",
                "div[class*=\"ds-button--primary\"][class*=\"ds-button--circle\"]",
                ".ds-button--primary",
                "button[data-testid=\"send-button\"]",
                "button[aria-label*=\"Send\"]",
                "div[role=\"button\"][aria-label*=\"Send\"]",
                "button[type=\"submit\"]",
            ],
            Provider::Gemini => &[
                "button[aria-label*=\"Send message\"]",
                "button[aria-label*=\"Send\"]",
                "button.send-button",
            ],
            Provider::Grok => &[
                "button[data-composer-action=\"send\"]",
                "button[data-testid=\"send-button\"]",
                "button[aria-label*=\"Send\"]",
                "button[type=\"submit\"]",
            ],
            Provider::Qwen => &["button.send-button:not(.disabled)"],
            Provider::Kimi => &[".chat-editor .send-button-container:not(.disabled):not(.stop)"],
        }
    }

    fn assistants(self) -> &'static [&'static str] {
        if self.grok_on_x {
            // Observed on X, 2026-11-16. Require completed-response controls
            // and select only the text container, excluding follow-up prompts.
            return &["[data-testid=\"primaryColumn\"] [aria-label=\"Grok\"] div.r-16lk18l:has(button[aria-label=\"Regenerate\"]):has(button[aria-label=\"Copy text\"]) > div:first-child"];
        }
        match self.provider {
            Provider::ChatGpt => &[
                // Current ChatGPT transcript UI (2026): one wrapper per turn,
                // tagged ...:assistant and containing data-response-body.
                "div[data-content-search-unit-key$=\":assistant\"]",
                "div:has(> h4[data-conversation-role=\"assistant\"])",
                "[data-message-author-role=\"assistant\"]",
                "article[data-testid^=\"conversation-turn\"] [data-message-author-role=\"assistant\"]",
            ],
            Provider::Claude => &["[data-testid=\"assistant-message\"]", "div.font-claude-response", "div[data-is-streaming] .font-claude-response"],
            // The generic .ds-markdown also includes DeepSeek's reasoning.
            Provider::DeepSeek => &[".ds-assistant-message-main-content"],
            Provider::Gemini => &["model-response", "message-content", ".model-response-text"],
            Provider::Grok => &["[data-testid=\"assistant-message\"]", "div[class*=\"response-content\"]", "div[class*=\"message-content\"]"],
            Provider::Qwen => &[".qwen-chat-message-assistant .response-message-content.phase-answer"],
            Provider::Kimi => &[".segment-assistant .segment-content-box"],
        }
    }

    fn generating(self) -> &'static [&'static str] {
        match self.provider {
            Provider::ChatGpt => &[
                "button[data-testid=\"stop-button\"]",
                "button[aria-label*=\"Stop\"]",
            ],
            Provider::Claude => &[
                "[data-testid=\"assistant-message\"][data-is-streaming=\"true\"]",
                "button[aria-label*=\"Stop\"]",
                "button[aria-label*=\"stop\"]",
            ],
            Provider::DeepSeek => &[
                ".ds-loading",
                "button[aria-label*=\"Stop\"]",
                "div[role=\"button\"][aria-label*=\"Stop\"]",
            ],
            Provider::Gemini => &[
                "button[aria-label*=\"Stop\"]",
                "button[aria-label*=\"stop\"]",
            ],
            Provider::Grok => &[
                "button[aria-label*=\"Stop\"]",
                "button[aria-label*=\"stop\"]",
            ],
            Provider::Qwen => &["button.stop-button"],
            Provider::Kimi => &[".send-button-container.stop"],
        }
    }

    fn composer(self, document: &Document) -> Option<HtmlElement> {
        for selector in self.composers() {
            if let Ok(elements) = document.query_selector_all(selector) {
                for index in 0..elements.length() {
                    if let Some(element) = elements.item(index) {
                        if let Ok(composer) = element.dyn_into::<HtmlElement>() {
                            if visible_control(&composer) {
                                return Some(composer);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn last_assistant(self, document: &Document) -> Option<Element> {
        for selector in self.assistants() {
            let Ok(nodes) = document.query_selector_all(selector) else {
                continue;
            };
            if nodes.length() == 0 {
                continue;
            }
            let mut last = None;
            for index in 0..nodes.length() {
                if let Some(node) = nodes.item(index) {
                    last = node.dyn_into::<Element>().ok();
                }
            }
            if last.is_some() {
                return last;
            }
        }
        None
    }

    fn users(self, document: &Document) -> Vec<Element> {
        let selector = if self.grok_on_x {
            // The combined bubble classes exclude follow-up suggestions and
            // model controls, which share individual classes with user turns.
            "[data-testid=\"primaryColumn\"] [aria-label=\"Grok\"] div.r-obd0qt.r-1cmwbt1"
        } else { match self.provider {
            Provider::ChatGpt => {
                "[data-content-search-unit-key$=\":user\"], [data-message-author-role=\"user\"]"
            }
            Provider::Claude | Provider::Grok => "[data-testid=\"user-message\"]",
            Provider::DeepSeek => ".ds-virtual-list-visible-items > [data-virtual-list-item-key]",
            Provider::Gemini => "user-query",
            Provider::Qwen => ".qwen-chat-message-user",
            Provider::Kimi => ".segment-user .user-content",
        }};
        let Ok(nodes) = document.query_selector_all(selector) else {
            return Vec::new();
        };
        (0..nodes.length())
            .filter_map(|i| nodes.item(i)?.dyn_into::<Element>().ok())
            .filter(|node| {
                self.provider != Provider::DeepSeek
                    || node.query_selector(".ds-markdown").ok().flatten().is_none()
            })
            .collect()
    }

    fn last_user(self, document: &Document) -> Option<Element> {
        self.users(document).pop()
    }

    fn user_text(self, node: &Element) -> Result<String, String> {
        if self.provider == Provider::Grok && !self.grok_on_x {
            // Grok.com prefixes its wrapper with an sr-only "You" heading.
            // The actual bubble has a semantic marker independent of classes.
            return node.query_selector("[data-bubble-text]").ok().flatten()
                .and_then(|bubble| bubble.text_content())
                .map(|text| text.trim().to_string())
                .ok_or_else(|| "This website's interface changed; the adapter needs an update. Grok user prompt selector [data-bubble-text] failed; hidden announcements cannot confirm delivery or authorize tools.".into());
        }
        if self.provider != Provider::Gemini {
            return Ok(assistant_text(node));
        }
        // Gemini's user-query wrapper includes a hidden "You said" heading
        // with a truncated duplicate. Only the actual prompt lines can confirm
        // delivery or authorize a task. Do not fall back to that announcement
        // if the website changes its markup.
        let warning = "This website's interface changed; the adapter needs an update. Gemini user prompt selector .query-text > .query-text-line failed; hidden announcements cannot confirm delivery or authorize tools.";
        let lines = node.query_selector_all(".query-text > .query-text-line")
            .map_err(|_| warning.to_string())?;
        if lines.length() == 0 {
            return Err(warning.into());
        }
        Ok((0..lines.length())
            .filter_map(|i| lines.item(i)?.text_content())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string())
    }

    fn response_id(self, node: &Element) -> Option<String> {
        if self.grok_on_x {
            let user = self.preceding_user(node)?;
            return x_grok_turn(&assistant_text(&user)).map(|(_, response)| response);
        }
        match self.provider {
            Provider::ChatGpt => node
                .closest("[data-chatgpt-search-message-ids]")
                .ok()
                .flatten()
                .and_then(|node| node.get_attribute("data-chatgpt-search-message-ids"))
                .and_then(|ids| ids.split_whitespace().next().map(str::to_owned))
                .or_else(|| {
                    node.query_selector("[data-response-body]")
                        .ok()
                        .flatten()?
                        .get_attribute("data-response-body")
                }),
            Provider::Claude => node.get_attribute("data-turn-key").or_else(|| {
                node.closest("[data-turn-key]")
                    .ok()
                    .flatten()?
                    .get_attribute("data-turn-key")
            }),
            Provider::DeepSeek => node
                .closest("[data-virtual-list-item-key]")
                .ok()
                .flatten()?
                .get_attribute("data-virtual-list-item-key"),
            Provider::Gemini => node
                .closest(".conversation-container[id]")
                .ok()
                .flatten()?
                .get_attribute("id"),
            Provider::Grok => node.get_attribute("aria-labelledby").or_else(|| {
                node.closest("[id^=\"response-\"]")
                    .ok()
                    .flatten()?
                    .get_attribute("id")
            }),
            Provider::Qwen => node.closest("[id^=\"chat-response-message-\"]")
                .ok().flatten().and_then(|n| n.get_attribute("id"))
                .or_else(|| node.closest("[id^=\"qwen-chat-message-assistant-\"]")
                    .ok().flatten().and_then(|n| n.get_attribute("id"))),
            Provider::Kimi => node.closest(".chat-content-item-assistant[data-archer-id]")
                .ok().flatten().and_then(|n| n.get_attribute("data-archer-id"))
                .filter(|id| !id.is_empty()),
        }
    }

    fn user_id(self, node: &Element) -> String {
        if self.provider == Provider::Kimi {
            // These outer-wrapper IDs persist through reloads. Text hashes
            // collide when independent tool calls return identical content.
            return node.closest(".chat-content-item-user[data-archer-id]")
                .ok().flatten().and_then(|n| n.get_attribute("data-archer-id"))
                .unwrap_or_default();
        }
        if self.provider == Provider::Qwen {
            if let Some(id) = node.query_selector("[data-msg-id]").ok().flatten()
                .and_then(|n| n.get_attribute("data-msg-id")).filter(|id| !id.is_empty()) {
                return format!("qwen-user-{id}");
            }
            if let Some(id) = node.get_attribute("id").filter(|id| !id.is_empty()) {
                return id;
            }
        }
        if self.grok_on_x {
            return x_grok_turn(&assistant_text(node)).map(|(user, _)| user).unwrap_or_default();
        }
        if self.provider == Provider::Claude {
            if let Some(id) = node
                .get_attribute("id")
                .and_then(|id| id.get(id.len().saturating_sub(36)..).map(str::to_owned))
            {
                return id;
            }
        }
        self.response_id(node)
            .unwrap_or_else(|| format!("user-{:016x}", fnv1a(self.user_text(node).unwrap_or_default().as_bytes())))
    }

    fn delivery_turn(self, document: &Document) -> Option<DeliveryTurn> {
        let assistant = self.last_assistant(document)?;
        let user = self.last_user(document)?;
        // A newly submitted user turn supersedes the old assistant response.
        if assistant.compare_document_position(&user) & web_sys::Node::DOCUMENT_POSITION_FOLLOWING
            != 0
        {
            return None;
        }
        if (self.grok_on_x || self.provider == Provider::Kimi)
            && !self.preceding_user(&assistant)?.is_same_node(Some(&user)) {
            return None;
        }
        let user_id = self.user_id(&user);
        if user_id.is_empty() { return None; }
        Some(DeliveryTurn {
            conversation: self.provider.conversation_id(&document.url().ok()?)?,
            user: user_id,
            assistant: self.response_id(&assistant)?,
        })
    }

    fn preceding_user(self, assistant: &Element) -> Option<Element> {
        let document = assistant.owner_document()?;
        self.users(&document).into_iter().rev().find(|user| {
            user.compare_document_position(assistant) & web_sys::Node::DOCUMENT_POSITION_FOLLOWING != 0
        })
    }

    fn reply_text(self, node: &Element) -> Result<String, String> {
        if self.provider == Provider::DeepSeek {
            // Observed on the actual site: loading ends before streaming does;
            // the per-turn Read aloud control appears only after completion.
            // A three-second pause alone can otherwise finalize partial JSON.
            let complete = node.closest("[data-virtual-list-item-key]").ok().flatten()
                .and_then(|turn| turn.query_selector("[role=\"button\"][aria-label=\"Read aloud\"]").ok().flatten())
                .is_some();
            if !complete {
                return Err("Waiting for DeepSeek's completed response. Completion selector [data-virtual-list-item-key] [role=button][aria-label=\"Read aloud\"] is missing; if the reply has finished, this website's interface changed and the adapter needs an update.".into());
            }
        }
        if self.provider == Provider::Claude
            && node.get_attribute("data-cds").as_deref() == Some("AssistantMessage")
            && node.query_selector("[data-transcript-engine-root]").ok().flatten().is_none()
        {
            return Err("This website's interface changed; the adapter needs an update. Claude visible reply selector [data-transcript-engine-root] failed; hidden announcements were not executed.".into());
        }
        if self.provider == Provider::Qwen {
            if node.closest(".qwen-chat-message-assistant").ok().flatten()
                .and_then(|turn| turn.query_selector_all(".response-message-content.phase-answer").ok())
                .is_some_and(|answers| answers.length() > 1) {
                return Err("Qwen displayed multiple candidate answers. Choose one response before using local tools; no candidate was executed.".into());
            }
            // A virtualized editor can omit arbitrary code lines. Never execute
            // an apparently valid prefix or read the site's network responses.
            if node.query_selector(".monaco-editor, pre.qwen-markdown-code").ok().flatten().is_some() {
                return Err("Qwen rendered a code editor that may hide lines. Ask it to emit the tool call as one plain-text [[BRIDGE_TOOL]] JSON [[/BRIDGE_TOOL]] line without a code fence; this reply was not executed.".into());
            }
        }
        if self.provider != Provider::Kimi {
            return Ok(assistant_text(node));
        }
        // Work on a detached clone, never remove content from the user's page.
        // K3.1 wraps BOTH reasoning and the final answer in process-flow.
        // Only its completed text sections are answer content. Retain their
        // whole subtree so sibling prose and fenced code stay together.
        let clean = node.clone_node_with_deep(true).ok()
            .and_then(|n| n.dyn_into::<Element>().ok())
            .ok_or_else(|| "Kimi reply could not be inspected safely.".to_string())?;
        let flows = clean.query_selector_all(".process-flow")
            .map_err(|_| "Kimi process-flow selector failed.".to_string())?;
        for index in 0..flows.length() {
            let Some(flow) = flows.item(index).and_then(|n| n.dyn_into::<Element>().ok()) else { continue; };
            if flow.get_attribute("data-process-phase").as_deref() != Some("completed") {
                return Err("Kimi is still completing its response; local tools are waiting.".into());
            }
            if flow.query_selector(":scope > .process-flow__text-section").ok().flatten().is_none() {
                return Err("This website's interface changed; the adapter needs an update. Kimi completed-answer selector .process-flow__text-section failed; no thinking text was executed.".into());
            }
            let non_answers = flow.query_selector_all(":scope > :not(.process-flow__text-section)")
                .map_err(|_| "Kimi completed-answer selector failed.".to_string())?;
            for child in 0..non_answers.length() {
                if let Some(n) = non_answers.item(child) { let _ = flow.remove_child(&n); }
            }
        }
        let excluded = clean.query_selector_all(".thinking-container, [data-process-section], .process-flow__header, .segment-assistant-actions")
            .map_err(|_| "Kimi thinking selector failed.".to_string())?;
        for index in 0..excluded.length() {
            if let Some(n) = excluded.item(index) {
                if let Some(parent) = n.parent_node() { let _ = parent.remove_child(&n); }
            }
        }
        Ok(assistant_text(&clean))
    }

    fn qwen_tool_editor_complete(self, node: &Element) -> bool {
        if self.provider != Provider::Qwen { return false; }
        let Some(turn) = node.closest(".qwen-chat-message-assistant").ok().flatten() else { return false; };
        if turn.query_selector_all(".response-message-content.phase-answer").ok().is_none_or(|answers| answers.length() != 1)
            || turn.query_selector(".response-message-footer button[aria-label=\"Regenerate\"]").ok().flatten().is_none() {
            return false;
        }
        let Ok(blocks) = node.query_selector_all("pre.qwen-markdown-code") else { return false; };
        (0..blocks.length()).any(|i| {
            let Some(block) = blocks.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else { return false; };
            let label = block.query_selector(".qwen-markdown-code-header > div:first-child").ok().flatten()
                .and_then(|n| n.text_content()).unwrap_or_default();
            label.trim() == protocol::BLOCK_TAG
                || block.query_selector(".view-lines").ok().flatten().and_then(|n| n.text_content())
                    .is_some_and(|text| text.contains(protocol::TOOL_OPEN))
        })
    }

    fn is_generating(self, document: &Document) -> bool {
        self.generating()
            .iter()
            .any(|selector| document.query_selector(selector).ok().flatten().is_some())
    }

    async fn send_guarded_message(
        self,
        document: &Document,
        text: &str,
        expected: Option<&DeliveryTurn>,
        draft: Option<&Rc<RefCell<Option<String>>>>,
        context_current: Option<&dyn Fn() -> bool>,
    ) -> Result<(), String> {
        let conversation = document
            .url()
            .ok()
            .and_then(|url| self.provider.conversation_id(&url));
        let check_route = || -> Result<(), String> {
            uploads::check_send(self, document, text)?;
            if context_current.is_some_and(|current| !current()) {
                return Err("workspace or conversation changed before submission".into());
            }
            let current = document
                .url()
                .ok()
                .and_then(|url| self.provider.conversation_id(&url));
            if current != conversation {
                return Err("conversation changed before the message was sent".into());
            }
            if expected
                .is_some_and(|expected| !expected.is_current(self.delivery_turn(document).as_ref()))
            {
                return Err("conversation turn changed before the result was sent".into());
            }
            Ok(())
        };
        let composer = self
            .composer(document)
            .ok_or_else(|| "message box not found; the website may have changed".to_string())?;
        let preceding_user = self.last_user(document).map(|user|self.user_id(&user));
        if let Some(block)=recovery::website_block(self,document) { return Err(block.message().into()); }
        let own_draft = draft.is_some_and(|draft| {
            owned_draft_matches(self.provider, draft.borrow().as_deref(), &composer_text(&composer))
        });
        if !composer_is_empty(&composer) && !own_draft {
            return Err("message box contains a draft; the bridge will not overwrite it".into());
        }
        check_route()?;
        if !own_draft {
            set_composer(document, &composer, text)?;
        }
        if matches!(self.provider, Provider::Kimi | Provider::Gemini) && !own_draft {
            // Lexical and Quill normalize paragraph breaks asynchronously.
            // Wait before claiming a draft, then verify the complete payload.
            // Never adopt changed content merely because it became stable.
            sleep_ms(280).await;
            for _ in 0..20 {
                check_route()?;
                let rendered = composer_text(&composer);
                if kimi_inserted_draft_matches(text, &rendered) { break; }
                if !rendered.is_empty() {
                    return Err("The result draft differs from the bridge message; it was not sent.".into());
                }
                sleep_ms(100).await;
            }
            check_route()?;
            if !kimi_inserted_draft_matches(text, &composer_text(&composer)) {
                return Err(format!("{} did not finish inserting the bridge message; it was not sent.", self.provider.display()));
            }
        }
        // Contenteditable editors normalize line breaks differently. Compare
        // with the actual inserted draft, not its source string, to detect edits.
        let inserted = composer_text(&composer);
        if let Some(draft) = draft {
            *draft.borrow_mut() = Some(inserted.clone());
        }
        sleep_ms(280).await;
        // ChatGPT enables its submit button asynchronously after the editor
        // state changes. Poll for an enabled control instead of clicking a
        // still-disabled button and reporting a false success.
        let mut clicked = false;
        for _ in 0..30 {
            if let Some(block)=recovery::website_block(self,document) { return Err(block.message().into()); }
            check_route()?;
            if !owned_draft_matches(self.provider, Some(&inserted), &composer_text(&composer)) {
                return Err("The result draft changed before submission; it was not sent.".into());
            }
            if let Some(button) = self.send_button(document) {
                button.click();
                clicked = true;
                break;
            }
            sleep_ms(100).await;
        }
        if clicked {
            if wait_for_send(self,document,text,preceding_user.clone()).await {
                return Ok(());
            }
            if let Some(block)=recovery::website_block(self,document) { return Err(block.message().into()); }
            return Err("The website has not acknowledged submission. The result is saved; automatic resubmission is paused to avoid duplicates. Inspect the chat before using Send pending result.".into());
        }
        if let Some(block)=recovery::website_block(self,document) { return Err(block.message().into()); }
        check_route()?;
        press_enter(&composer)?;
        if wait_for_send(self,document,text,preceding_user).await {
            return Ok(());
        }
        if let Some(block)=recovery::website_block(self,document) { return Err(block.message().into()); }
        Err("the website did not accept the message (composer never cleared); the adapter may need an update".into())
    }

    fn send_button(self, document: &Document) -> Option<HtmlElement> {
        for selector in self.send_buttons() {
            let Ok(elements) = document.query_selector_all(selector) else {
                continue;
            };
            for index in 0..elements.length() {
                let Some(element) = elements.item(index) else {
                    continue;
                };
                let Ok(button) = element.dyn_into::<HtmlElement>() else {
                    continue;
                };
                if !visible_control(&button)
                    || button.get_attribute("disabled").is_some()
                    || button.get_attribute("aria-disabled").as_deref() == Some("true")
                {
                    continue;
                }
                return Some(button);
            }
        }
        None
    }
}

fn visible_control(element: &HtmlElement) -> bool {
    let rect = element.get_bounding_client_rect();
    rect.width() > 0.0
        && rect.height() > 0.0
        && element
            .closest("[hidden], [aria-hidden=\"true\"]")
            .ok()
            .flatten()
            .is_none()
}

fn set_composer(document: &Document, composer: &HtmlElement, text: &str) -> Result<(), String> {
    composer
        .focus()
        .map_err(|_| "could not focus the message box".to_string())?;
    if let Some(textarea) = composer.dyn_ref::<HtmlTextAreaElement>() {
        // DeepSeek (and other React-controlled textareas) ignore a plain
        // `.value =` assignment: React's value tracker stays empty, so the
        // send control never appears. Selecting the textarea and issuing a
        // real insertText command emits the input path the site expects.
        textarea.select();
        if !insert_text(document, text) {
            textarea.set_value(text);
        }
        dispatch_input(document, composer);
        return Ok(());
    }
    let Some(window) = web_sys::window() else {
        return Err("window unavailable".into());
    };
    let Some(selection) = window.get_selection().ok().flatten() else {
        composer.set_text_content(Some(text));
        dispatch_input(document, composer);
        return Ok(());
    };
    if let Some(range) = document.create_range().ok() {
        if range.select_node_contents(composer).is_ok() {
            selection.remove_all_ranges().ok();
            selection.add_range(&range).ok();
            let document_value: &JsValue = document.as_ref();
            let inserted = js_sys::Reflect::get(document_value, &JsValue::from_str("execCommand"))
                .ok()
                .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
                .and_then(|exec| {
                    exec.call3(
                        document_value,
                        &JsValue::from_str("insertText"),
                        &JsValue::FALSE,
                        &JsValue::from_str(text),
                    )
                    .ok()
                })
                .and_then(|result| result.as_bool())
                .unwrap_or(false);
            if !inserted {
                composer.set_text_content(Some(text));
            }
            dispatch_input(document, composer);
            return Ok(());
        }
    }
    composer.set_text_content(Some(text));
    dispatch_input(document, composer);
    Ok(())
}

fn insert_text(document: &Document, text: &str) -> bool {
    let document_value: &JsValue = document.as_ref();
    js_sys::Reflect::get(document_value, &JsValue::from_str("execCommand"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok())
        .and_then(|exec| {
            exec.call3(
                document_value,
                &JsValue::from_str("insertText"),
                &JsValue::FALSE,
                &JsValue::from_str(text),
            )
            .ok()
        })
        .and_then(|result| result.as_bool())
        .unwrap_or(false)
}

fn dispatch_input(document: &Document, composer: &HtmlElement) {
    let _ = document;
    let init = EventInit::new();
    init.set_bubbles(true);
    if let Ok(event) = web_sys::Event::new_with_event_init_dict("input", &init) {
        let _ = composer.dispatch_event(&event);
    }
    if let Ok(event) = web_sys::Event::new_with_event_init_dict("change", &init) {
        let _ = composer.dispatch_event(&event);
    }
}

async fn wait_for_send(adapter:Adapter, document:&Document, text:&str, preceding_user:Option<String>) -> bool {
    let watched_document=document.clone();
    let document=document.clone();
    let text=text.to_string();
    let confirmed=Rc::new(move||adapter.last_user(&document).is_some_and(|user|
        adapter.user_text(&user).ok().is_some_and(|observed| bridge_core::recovery::submitted_message_matches(preceding_user.as_deref(),&adapter.user_id(&user),&text,&observed))));
    if confirmed() {return true;}
    let resolver:Rc<RefCell<Option<js_sys::Function>>>=Rc::default();
    let promise=js_sys::Promise::new(&mut |resolve,_|*resolver.borrow_mut()=Some(resolve));
    let resolve=resolver.borrow().as_ref().unwrap().clone();
    let check=confirmed.clone();
    let callback=Closure::<dyn FnMut(js_sys::Array,web_sys::MutationObserver)>::new(move|_,_|{
        if check() {let _=resolve.call1(&JsValue::UNDEFINED,&JsValue::TRUE);}
    });
    let observer=web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()).ok();
    if let Some(observer)=&observer {
        let options=web_sys::MutationObserverInit::new();options.set_child_list(true);options.set_subtree(true);options.set_character_data(true);
        observer.observe_with_options(&watched_document,&options).ok();
    }
    let resolve=resolver.borrow().as_ref().unwrap().clone();
    let check=confirmed.clone();
    js::schedule(12_000,move||{let _=resolve.call1(&JsValue::UNDEFINED,&JsValue::from_bool(check()));});
    let sent=wasm_bindgen_futures::JsFuture::from(promise).await.ok().and_then(|v|v.as_bool()).unwrap_or(false);
    if let Some(observer)=observer {observer.disconnect();}
    sent
}


fn press_enter(composer: &HtmlElement) -> Result<(), String> {
    for event_type in ["keydown", "keypress", "keyup"] {
        let init = KeyboardEventInit::new();
        init.set_key("Enter");
        init.set_code("Enter");
        init.set_key_code(13);
        init.set_which(13);
        init.set_char_code(0);
        init.set_bubbles(true);
        init.set_cancelable(true);
        let event = KeyboardEvent::new_with_keyboard_event_init_dict(event_type, &init)
            .map_err(|_| "could not create keyboard event".to_string())?;
        composer
            .dispatch_event(&event)
            .map_err(|_| "could not send Enter to the message box".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod recovery_memory_tests {
    use super::*;
    #[test]
    fn pending_result_roundtrip_keeps_turn_draft_and_uncertain_boundary() {
        let memory=ConversationMemory {
            context_hash:"known".into(),workspace_id:"workspace".into(),instructions_sent:true,
            task_id:Some("original-user".into()),task_goal:"Build a game".into(),auto_continue:true,
            pending:Some(SavedResult {text:"actual saved result".into(),turn:DeliveryTurn {conversation:"chat".into(),user:"user".into(),assistant:"response".into()},draft:Some("exact editor draft".into()),retry:DeliveryRetry {submission_uncertain:true,..Default::default()}}),
            instruction_submission:Some(InstructionSubmission{context_hash:"known".into(),preceding_user:Some("before-setup".into())}),
            format_repairs:FormatRepairBudget::default(),
        };
        let restored:ConversationMemory=serde_json::from_str(&serde_json::to_string(&memory).unwrap()).unwrap();
        assert_eq!(restored.task_id.as_deref(),Some("original-user"));
        assert_eq!(restored.instruction_submission.unwrap().preceding_user.as_deref(),Some("before-setup"));
        let p=restored.pending.unwrap();
        assert_eq!(p.text,"actual saved result");
        assert_eq!(p.draft.as_deref(),Some("exact editor draft"));
        assert_eq!(p.turn.conversation,"chat");
        assert!(!p.retry.ready(1_000_000.0));
        let legacy:ConversationMemory=serde_json::from_str(r#"{"instructions_sent":true,"task_id":"old"}"#).unwrap();
        assert_eq!(legacy.task_id.as_deref(),Some("old"));
        assert!(!legacy.auto_continue);
        assert!(legacy.pending.is_none());
        assert!(legacy.instruction_submission.is_none());
    }
}
