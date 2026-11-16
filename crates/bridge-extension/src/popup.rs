use std::time::Duration;

use leptos::prelude::*;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

use bridge_core::provider::Provider;

use crate::js;
use crate::model::{BackgroundResponse, StatusView};

pub fn popup_view() -> impl IntoView {
    bridge_ui(false)
}

pub fn side_panel_view() -> impl IntoView {
    bridge_ui(true)
}

fn bridge_ui(wide: bool) -> impl IntoView {
    console_error_panic_hook::set_once();
    let status = RwSignal::new(StatusView::default());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    refresh(status, error);
    if let Some(window) = web_sys::window() {
        let callback = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            refresh(status, error);
        });
        let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
            callback.as_ref().unchecked_ref(),
            2_000,
        );
        callback.forget();
    }

    if wide {
        if let Some(window) = web_sys::window() {
            let callback = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
                move |event: web_sys::KeyboardEvent| {
                    if event.ctrl_key() || event.meta_key() || event.alt_key() {
                        return;
                    }
                    if event
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .is_some_and(|e| {
                            matches!(e.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                                || e.get_attribute("contenteditable").as_deref() == Some("true")
                        })
                    {
                        return;
                    }
                    match event.key().as_str() {
                        "r" => {
                            event.prevent_default();
                            refresh(status, error);
                        }
                        "a" | "d" => {
                            if let Some(approval) = status.get_untracked().approvals.first() {
                                event.prevent_default();
                                decide(approval.id.clone(), event.key() == "a", status, error);
                            }
                        }
                        _ => {}
                    }
                },
            );
            let _ = window
                .add_event_listener_with_callback("keydown", callback.as_ref().unchecked_ref());
            callback.forget();
        }
    }
    let width = if wide { "100%" } else { "376px" };
    let root_style = format!(
        "width:{width};max-width:100%;box-sizing:border-box;background:#0f171e;color:#e7eef4;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;font-size:12px;line-height:1.45;padding:14px;border-radius:0;min-height:{min_height}",
        min_height = if wide { "100vh" } else { "0" }
    );

    let toggle_auto = move |_| {
        let next = !status.get().auto_connect;
        busy.set(true);
        spawn_local(async move {
            let result = oxichrome::runtime::send_message(&json!({
                "op":"settings_set",
                "auto_connect": next,
            }))
            .await;
            if let Some(response) = result.ok().and_then(js::from_js::<BackgroundResponse>) {
                if !response.ok {
                    error.set(response.error);
                }
            } else {
                error.set(Some("Could not update the auto-connect setting.".into()));
            }
            busy.set(false);
            refresh(status, error);
        });
    };

    let reconnect = move |_| {
        busy.set(true);
        spawn_local(async move {
            let _ = oxichrome::runtime::send_message(&json!({"op":"reconnect"})).await;
            busy.set(false);
            refresh(status, error);
        });
    };

    view! {
        <div style=root_style>
            <div style="display:flex;align-items:center;gap:8px;margin-bottom:10px">
                <span style=move || {
                    let color = match status.get().bridge.as_str() {
                        "connected" => "#43bf7f",
                        _ => if status.get().last_error.is_some() { "#e05a5a" } else { "#7a8794" },
                    };
                    format!("width:8px;height:8px;border-radius:50%;background:{color};display:inline-block;flex:0 0 auto")
                }></span>
                <div style="flex:1">
                    <div style="font-weight:700;font-size:13px;letter-spacing:0.01em">"Unified AI Bridge"</div>
                    <div style="color:#8ea2b1;font-size:10px">"Rust native host · visible browser sessions"</div>
                </div>
                <span style=move || {
                    let color = match status.get().bridge.as_str() {
                        "connected" => "#43bf7f",
                        _ => "#8ea2b1",
                    };
                    format!("color:{color};font-size:10px;font-weight:600;text-transform:uppercase;letter-spacing:0.06em")
                }>{move || if status.get().bridge == "connected" { "connected".to_string() } else { format!("reconnecting ({})",status.get().reconnect_attempt) }}</span>
            </div>

            <div style=if wide {"display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:16px;align-items:start"}else{"display:block"}><div>
            <div style="background:#16222c;border:1px solid #293b47;border-radius:7px;padding:8px 10px;margin-bottom:10px">
                <div style="color:#8ea2b1;font-size:10px;text-transform:uppercase;letter-spacing:0.07em;margin-bottom:3px">"Approved workspace"</div>
                <div style="font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:10px;word-break:break-all;color:#cdd8e0">
                    {move || if status.get().workspace.is_empty() { "Waiting for the native host…".to_string() } else { status.get().workspace.clone() }}
                </div>
                <div style="color:#788d9b;font-size:10px;margin-top:3px">
                    {move || if status.get().project.is_empty() { String::new() } else { format!("project: {}", status.get().project) }}
                </div>
            </div>

            <WorkspaceControls status=status error=error/>
            <ReadinessControls status=status error=error/>
            <WriteGrantControls status=status error=error/>
            <CommandControls status=status error=error/>
            <ArtifactControls status=status error=error/>
            <ToolWorkbench status=status error=error/>
            <IntegrationControls status=status error=error/>

            <SectionTitle text="Providers"/>
            <div style="display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:5px;margin-bottom:11px">
                {Provider::ALL.into_iter().map(|provider| {
                    let session = move || status.get().sessions.into_iter().find(|session| session.provider == provider.slug());
                    view! {
                        <div style="background:#131f28;border:1px solid #273946;border-radius:6px;padding:6px 7px;min-width:0">
                            <div style="display:flex;align-items:center;gap:5px">
                                <span style=move || {
                                    let connected = session().is_some();
                                    format!("width:6px;height:6px;border-radius:50%;flex:0 0 auto;background:{}", if connected { "#43bf7f" } else { "#5c6b77" })
                                }></span>
                                <span style="font-weight:600;color:#dbe5ec;font-size:11px">{provider.display()}</span>
                            </div>
                            <div style="color:#8195a4;font-size:9px;margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">
                                {move || match session() {
                                    Some(session) => match session.conversation {
                                        Some(conversation) if !conversation.is_empty() => format!("tab {} · {}", session.tab_id, conversation),
                                        _ => format!("tab {}", session.tab_id),
                                    },
                                    None => "not open".to_string(),
                                }}
                            </div>
                            <details><summary>"Session details"</summary>
                                <div style="white-space:pre-wrap;word-break:break-all">{move||session().map(|s|format!("Session: {}\nConversation: {}\nLast seen: {}",s.id,s.conversation.unwrap_or_default(),s.last_seen_at)).unwrap_or_default()}</div>
                                <div>{move||status.get().activity.into_iter().find(|a|a.agent==provider.agent().dir()).map(|a|format!("{} · {} · {}",a.created_at,a.tool,a.summary)).unwrap_or_else(||"No recorded activity".into())}</div>
                                {[ ("focus","Focus tab"),("reconnect","Reconnect session"),("reinject","Re-inject instructions"),("disconnect","Disconnect session") ].into_iter().map(|(action,label)|view!{
                                    <button style=CONTROL_STYLE disabled=move||session().is_none() on:click=move |_|{if let Some(s)=session(){spawn_local(async move{if let Err(err)=workspace_request(json!({"op":"provider_control","session_id":s.id,"action":action})).await{error.set(Some(err));}refresh(status,error);});}}>{label}</button>
                                }).collect_view()}
                            </details>
                        </div>
                    }
                }).collect_view()}
            </div>

            <div style="display:flex;gap:6px;margin-bottom:9px">
                <button
                    on:click=toggle_auto
                    disabled=move || busy.get()
                    style=move || {
                        let enabled = status.get().auto_connect;
                        format!("flex:1;text-align:left;background:{};color:#e7eef4;border:1px solid #3a4e5c;border-radius:5px;padding:6px 8px;font-size:10px;font-family:inherit;cursor:pointer", if enabled { "#1d3b32" } else { "#18252e" })
                    }
                >
                    {move || if status.get().auto_connect { "Auto-connect: ON" } else { "Auto-connect: OFF" }}
                </button>
                <button
                    on:click=reconnect
                    disabled=move || busy.get()
                    style="background:#18252e;color:#c8d5de;border:1px solid #3a4e5c;border-radius:5px;padding:6px 8px;font-size:10px;font-family:inherit;cursor:pointer"
                >"Reconnect"</button>
            </div>

            </div><div>
            <Show when=move || !status.get().approvals.is_empty()>
                <div style="margin-bottom:10px;position:sticky;top:0;background:#0f171e;z-index:2">
                    <SectionTitle text="Approval required"/>
                    {move || status.get().approvals.into_iter().map(|approval| {
                        let id = approval.id.clone();
                        let id_deny = approval.id.clone();
                        let path = approval.path.clone().unwrap_or_default();
                        view! {
                            <div style="background:#241f14;border:1px solid #66532a;border-radius:6px;padding:7px 8px;margin-bottom:5px">
                                <div style="color:#f0d79c;font-weight:600;font-size:11px">{format!("{} · {}", approval.agent, approval.tool)}</div>
                                <div style="color:#cbb98e;font-size:10px;white-space:pre-wrap;overflow:auto;max-height:240px">{approval.reason.clone()}</div>
                                <div>{format!("Expires in {} seconds",((js_sys::Date::parse(&approval.expires_at)-js::now_ms())/1000.0).ceil().max(0.0) as i64)}</div>
                                <div style="color:#9d8c67;font-family:ui-monospace,Menlo,monospace;font-size:9px;word-break:break-all;margin-top:2px">{path}</div>
                                <div style="display:flex;gap:5px;margin-top:6px">
                                    <button
                                        on:click=move |_| { decide(id.clone(), true, status, error); }
                                        style="background:#3a4d2c;color:#e6f0dc;border:1px solid #607f47;border-radius:4px;padding:4px 8px;font-size:10px;font-family:inherit;cursor:pointer"
                                    >"Approve once"</button>
                                    <button
                                        on:click=move |_| { decide(id_deny.clone(), false, status, error); }
                                        style="background:#3a2626;color:#f0dcdc;border:1px solid #714444;border-radius:4px;padding:4px 8px;font-size:10px;font-family:inherit;cursor:pointer"
                                    >"Deny"</button>
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </Show>

            <SectionTitle text="Recent local activity"/>
            <JobControls status=status error=error/>
            <ActivityFeed status=status error=error/>

            <Show when=move || error.get().is_some() || status.get().last_error.is_some()>
                <div style="background:#2c1b1b;border:1px solid #6d3a36;border-radius:6px;padding:7px 8px;color:#ffb0a5;font-size:10px;margin-bottom:8px;word-break:break-word">
                    {move || error.get().or_else(|| status.get().last_error.clone()).unwrap_or_default()}
                </div>
            </Show>

            <div style="display:flex;gap:5px;margin-bottom:9px">
                <button
                    on:click=move |_| { refresh(status, error); }
                    style="flex:1;background:#1a2832;color:#c8d5de;border:1px solid #344957;border-radius:5px;padding:5px;font-size:10px;font-family:inherit;cursor:pointer"
                >"Refresh status"</button>
                <button
                    on:click=oxichrome::side_panel::open_on_click(|_| async {})
                    style="flex:1;background:#1a2832;color:#c8d5de;border:1px solid #344957;border-radius:5px;padding:5px;font-size:10px;font-family:inherit;cursor:pointer"
                >"Open side panel"</button>
            </div>

            </div></div>
            <div style="color:#627785;font-size:9px;border-top:1px solid #22333f;padding-top:7px">
                <div>{move || format!("Build {} · host {} · protocol {}", status.get().extension_build, status.get().host_version, status.get().protocol)}</div>
                <button style=CONTROL_STYLE on:click=move |_|{let _=js::create_tab(&js::to_js(&json!({"url":"chrome://extensions/"})));}>"Reload extension instructions"</button>
                <div>"After a build, open Extensions, click Reload for Unified AI Bridge, then refresh provider tabs. Verify the panel build number. Side panel shortcuts: r refresh, a approve first, d deny first."</div>
                "Each tab is bound to a host-issued session. Models never supply their own identity. Datasets are immutable; writes use checkpoints and the audit chain."
            </div>
        </div>
    }
}

const CONTROL_STYLE: &str = "background:#17242e;color:#dce6ed;border:1px solid #3a4e5c;padding:5px 7px;border-radius:4px;font:inherit;min-width:0";

#[component]
fn ActivityFeed(status: RwSignal<StatusView>, error: RwSignal<Option<String>>) -> impl IntoView {
    let provider = RwSignal::new(String::new());
    let decision = RwSignal::new(String::new());
    let tool = RwSignal::new(String::new());
    let cleared = RwSignal::new(0i64);
    let detail = RwSignal::new(String::new());
    view! {
        <div style="display:grid;gap:6px;margin-bottom:10px">
            <select aria-label="Filter activity provider" style=CONTROL_STYLE on:change=move|ev|provider.set(event_target_value(&ev))>
                <option value="">"All providers"</option>
                {Provider::ALL.into_iter().map(|provider|view!{<option value=provider.agent().dir()>{provider.display()}</option>}).collect_view()}
                <option value="USER">"Workspace controls"</option>
            </select>
            <select aria-label="Filter activity status" style=CONTROL_STYLE on:change=move|ev|decision.set(event_target_value(&ev))>
                <option value="">"All statuses"</option><option value="allowed">"Allowed"</option><option value="denied">"Denied"</option><option value="error">"Error"</option>
            </select>
            <input aria-label="Filter activity tool" placeholder="Filter by tool name" style=CONTROL_STYLE on:input=move|ev|tool.set(event_target_value(&ev))/>
            <button style=CONTROL_STYLE on:click=move |_|{cleared.set(status.get().activity.first().map(|a|a.id).unwrap_or_default());detail.set(String::new());}>"Clear view (keep audit)"</button>
            <div style="max-height:320px;overflow:auto;display:grid;gap:4px">
                {move || status.get().activity.into_iter().filter(|a|a.id>cleared.get()&&(provider.get().is_empty()||a.agent==provider.get())&&(decision.get().is_empty()||a.decision==decision.get())&&(tool.get().is_empty()||a.tool.contains(&tool.get()))).map(|activity|{
                    let id=activity.id;
                    view!{<button style=CONTROL_STYLE on:click=move |_|spawn_local(async move{match workspace_request(json!({"op":"activity_detail","activity_id":id})).await{Ok(result)=>detail.set(serde_json::to_string_pretty(&result).unwrap_or_default()),Err(err)=>error.set(Some(err))}})>
                        <div>{format!("{} · {} · {} · {} ms",activity.agent,activity.tool,activity.decision,activity.duration_ms)}</div>
                        <div>{activity.created_at}</div><div>{activity.path.unwrap_or_default()}</div><div>{activity.summary}</div>
                    </button>}
                }).collect_view()}
            </div>
            <Show when=move||!detail.get().is_empty()><pre style="white-space:pre-wrap;word-break:break-word;max-height:320px;overflow:auto">{move||detail.get()}</pre></Show>
        </div>
    }
}

#[component]
fn IntegrationControls(
    status: RwSignal<StatusView>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let id = RwSignal::new(String::new());
    let program = RwSignal::new(String::new());
    let args = RwSignal::new("[]".to_string());
    let servers = RwSignal::new(Vec::<serde_json::Value>::new());
    let discovery = RwSignal::new(serde_json::Value::Null);
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(String::new());
    let reload = move || {
        spawn_local(async move {
            match workspace_request(json!({"op":"integration_list"})).await {
                Ok(value) => servers.set(value["servers"].as_array().cloned().unwrap_or_default()),
                Err(err) => error.set(Some(err)),
            }
        })
    };
    let current = RwSignal::new(String::new());
    Effect::new(move |_| {
        let active = status.get().active_id;
        if !active.is_empty() && active != current.get_untracked() {
            current.set(active);
            discovery.set(serde_json::Value::Null);
            reload();
        }
    });
    view! {
        <details style="border:1px solid #304451;padding:8px;margin-bottom:10px"><summary>"MCP integrations"</summary>
            <div style="display:grid;gap:6px;margin-top:8px">
                <div>"Connect an installed native MCP executable. No packages are installed. External programs run with your OS permissions; they are not confined to the workspace. Each tool call requires approval."</div>
                <input style=CONTROL_STYLE aria-label="Integration ID" placeholder="server-id" prop:value=move||id.get() on:input=move|e|id.set(event_target_value(&e))/>
                <input style=CONTROL_STYLE aria-label="MCP executable" placeholder="/absolute/path/to/native-server" prop:value=move||program.get() on:input=move|e|program.set(event_target_value(&e))/>
                <input style=CONTROL_STYLE aria-label="MCP arguments" placeholder="[]" prop:value=move||args.get() on:input=move|e|args.set(event_target_value(&e))/>
                <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|{
                    let Ok(arguments)=serde_json::from_str::<Vec<String>>(&args.get())else{error.set(Some("Arguments must be a JSON string array.".into()));return;};
                    let server=json!({"id":id.get(),"program":program.get(),"args":arguments,"tools":[],"timeout_secs":60});busy.set(true);
                    spawn_local(async move{match workspace_request(json!({"op":"integration_save","server":server})).await{Ok(_)=>{notice.set("Registered with no tools enabled. Discover and review tools next.".into());reload();},Err(err)=>error.set(Some(err))};busy.set(false);});
                }>"Register server (no tools enabled)"</button>
                {move||servers.get().into_iter().map(|server|{
                    let server_id=server["id"].as_str().unwrap_or_default().to_string();let remove_id=server_id.clone();
                    view!{<div style="border-top:1px solid #304451;padding-top:6px">
                        <div>{format!("{} · {} · {} tools enabled",server_id,server["program"],server["tools"].as_array().map_or(0,Vec::len))}</div>
                        <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|{let id=server_id.clone();busy.set(true);spawn_local(async move{match workspace_request(json!({"op":"integration_discover","server_id":id})).await{Ok(value)=>{discovery.set(value);notice.set("Discovery succeeded. Select tools to grant; nothing is enabled automatically.".into());},Err(err)=>error.set(Some(err))};busy.set(false);});}>"Start and discover tools"</button>
                        <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|{let id=remove_id.clone();spawn_local(async move{match workspace_request(json!({"op":"integration_remove","server_id":id})).await{Ok(_)=>{reload();discovery.set(serde_json::Value::Null);},Err(err)=>error.set(Some(err))}});}>"Remove integration"</button>
                    </div>}
                }).collect_view()}
                {move||discovery.get()["tools"].as_array().cloned().unwrap_or_default().into_iter().map(|tool|{
                    let name=tool["name"].as_str().unwrap_or_default().to_string();let label=format!("Grant {}",name);
                    view!{<details><summary>{name.clone()}</summary><pre style="white-space:pre-wrap;max-height:180px;overflow:auto">{serde_json::to_string_pretty(&tool["schema"]).unwrap_or_default()}</pre>
                        <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|{
                            let selected=discovery.get()["server_id"].as_str().unwrap_or_default().to_string();let Some(mut server)=servers.get().into_iter().find(|s|s["id"]==selected)else{return;};
                            let mut grants=server["tools"].as_array().cloned().unwrap_or_default();grants.retain(|g|g["name"]!=name);grants.push(json!({"name":name,"schema_sha256":tool["schema_sha256"],"schema":tool["schema"]}));server["tools"]=json!(grants);busy.set(true);
                            spawn_local(async move{match workspace_request(json!({"op":"integration_save","server":server})).await{Ok(_)=>{reload();notice.set("Tool granted. The model can discover its schema using list_integrations.".into());},Err(err)=>error.set(Some(err))};busy.set(false);});
                        }>{label}</button>
                    </details>}
                }).collect_view()}
                <div>{move||notice.get()}</div>
            </div>
        </details>
    }
}

#[component]
fn ArtifactControls(
    status: RwSignal<StatusView>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = RwSignal::new(String::new());
    let download = RwSignal::new(String::new());
    let downloads = RwSignal::new(Vec::<serde_json::Value>::new());
    let shared = RwSignal::new(Vec::<serde_json::Value>::new());
    let selected = RwSignal::new(String::new());
    let path = RwSignal::new(String::new());
    let result = RwSignal::new(serde_json::Value::Null);
    let busy = RwSignal::new(false);
    let displayed_scope = RwSignal::new(String::new());
    Effect::new(move |_| {
        let scope = format!("{}:{}", status.get().active_id, session.get());
        if scope != displayed_scope.get_untracked() {
            displayed_scope.set(scope);
            shared.set(Vec::new());
            selected.set(String::new());
            path.set(String::new());
            result.set(serde_json::Value::Null);
        }
    });
    let action = move |op: &'static str| {
        let current = status.get_untracked();
        let sid = session.get_untracked();
        if op != "browser_downloads" && !current.sessions.iter().any(|s| s.id == sid) {
            error.set(Some(
                "Choose a connected conversation to share this file with.".into(),
            ));
            return;
        }
        let workspace = current.active_id;
        let request = match op {
            "browser_downloads" => json!({"op":op}),
            "share_download" => {
                let Ok(id) = download.get_untracked().parse::<i32>() else {
                    error.set(Some("Select a completed download first.".into()));
                    return;
                };
                json!({"op":op,"download_id":id,"workspace_id":workspace,"session_id":sid})
            }
            "revoke_artifact" => {
                json!({"op":op,"artifact_id":selected.get_untracked(),"workspace_id":workspace,"session_id":sid})
            }
            _ => {
                json!({"op":"run_tool","workspace_id":workspace,"session_id":sid,"tool":op,"arguments":if op=="import_artifact"{json!({"artifact_id":selected.get_untracked(),"path":path.get_untracked()})}else{json!({})}})
            }
        };
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            match workspace_request(request).await {
                Ok(value) => {
                    if op == "browser_downloads" {
                        downloads.set(value["downloads"].as_array().cloned().unwrap_or_default());
                        download.set(String::new());
                    } else if status.get_untracked().active_id == workspace
                        && session.get_untracked() == sid
                    {
                        if value["status"] == "error" {
                            error.set(Some(
                                value["error"]
                                    .as_str()
                                    .unwrap_or("File operation failed")
                                    .into(),
                            ));
                        }
                        if op == "share_download" {
                            selected.set(value["artifact_id"].as_str().unwrap_or_default().into());
                        }
                        result.set(value);
                        match workspace_request(json!({"op":"run_tool","workspace_id":workspace,"session_id":sid,"tool":"list_artifacts","arguments":{}})).await {
                            Ok(list) if status.get_untracked().active_id==workspace && session.get_untracked()==sid=>{
                                if list["status"]=="ok" {shared.set(list["data"]["artifacts"].as_array().cloned().unwrap_or_default());}
                                else {error.set(Some(list["error"].as_str().unwrap_or("Could not load shared files").into()));}
                            },
                            Err(err)=>error.set(Some(err)), _=>{},
                        }
                    }
                }
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
            refresh(status, error);
        });
    };
    view! {
        <details style="border:1px solid #304451;padding:8px;margin-bottom:10px">
            <summary>"Browser files → workspace"</summary>
            <div style="display:grid;gap:6px;margin-top:8px">
                <div>"Download from the chat first, then select the file here. Only your selected file is shared. Images, PDFs, office files, ZIPs and media keep their original bytes."</div>
                <div style="color:#8ea2b1">"Up to 64 MiB · completed safe downloads only · approval for every import · no overwrites, extraction or binary undo."</div>
                <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|action("browser_downloads")>"Refresh browser downloads"</button>
                <select aria-label="Browser download" style=CONTROL_STYLE prop:value=move||download.get() on:change=move|ev|download.set(event_target_value(&ev))>
                    <option value="">"Choose a downloaded file…"</option>
                    {move||downloads.get().into_iter().map(|d|view!{<option value=d["download_id"].to_string()>{format!("{} · {} bytes · {}",d["name"].as_str().unwrap_or("file"),d["bytes"],d["origin"].as_str().unwrap_or_default())}</option>}).collect_view()}
                </select>
                <select aria-label="File sharing session" style=CONTROL_STYLE prop:value=move||session.get() on:change=move|ev|session.set(event_target_value(&ev))>
                    <option value="">"Choose a conversation…"</option>
                    {move||status.get().sessions.into_iter().map(|s|view!{<option value=s.id>{format!("{} · tab {} · {}",s.agent,s.tab_id,s.conversation.unwrap_or_default())}</option>}).collect_view()}
                </select>
                <button style=CONTROL_STYLE disabled=move||busy.get()||download.get().is_empty()||session.get().is_empty() on:click=move |_|action("share_download")>"Share selected file with this session"</button>
                <div>"Sharing lasts 24 hours. The model can find the file with list_artifacts; sharing does not send a chat message."</div>
                <button style=CONTROL_STYLE disabled=move||busy.get()||session.get().is_empty() on:click=move |_|action("list_artifacts")>"Refresh shared files"</button>
                <select aria-label="Shared file" style=CONTROL_STYLE prop:value=move||selected.get() on:change=move|ev|selected.set(event_target_value(&ev))>
                    <option value="">"Choose a shared file…"</option>
                    {move||shared.get().into_iter().map(|a|{
                        let id=a["artifact_id"].as_str().unwrap_or_default().to_string();let selected_id=id.clone();
                        view!{<option value=id prop:selected=move||selected.get()==selected_id>{format!("{} · {} · {} bytes",a["name"].as_str().unwrap_or("file"),a["state"].as_str().unwrap_or_default(),a["bytes"])}</option>}
                    }).collect_view()}
                </select>
                <pre style="white-space:pre-wrap;word-break:break-all;font-size:10px">{move||shared.get().into_iter().find(|a|a["artifact_id"]==selected.get()).map(|a|format!("ID: {}\nType: {}\nSHA256: {}\nDestination: {}",a["artifact_id"].as_str().unwrap_or_default(),a["mime"].as_str().unwrap_or_default(),a["sha256"].as_str().unwrap_or_default(),a["destination"].as_str().unwrap_or("not imported"))).unwrap_or_default()}</pre>
                <input aria-label="Import destination" style=CONTROL_STYLE placeholder="files/generated-image.png (existing folder)" prop:value=move||path.get() on:input=move|ev|path.set(event_target_value(&ev))/>
                <div style="display:flex;gap:5px;flex-wrap:wrap">
                    <button style=CONTROL_STYLE disabled=move||busy.get()||path.get().trim().is_empty()||!shared.get().iter().any(|a|a["artifact_id"]==selected.get()&&a["state"]=="selected") on:click=move |_|action("import_artifact")>"Request import"</button>
                    <button style=CONTROL_STYLE disabled=move||busy.get()||!shared.get().iter().any(|a|a["artifact_id"]==selected.get()&&a["state"]=="selected") on:click=move |_|action("revoke_artifact")>"Revoke sharing"</button>
                </div>
                <div>{move||if busy.get(){"Working; check pending approvals below."}else{"The original download stays in place. Imported files are never opened or run automatically."}}</div>
                <pre style="white-space:pre-wrap;word-break:break-all;max-height:220px;overflow:auto;font-size:10px">{move||if result.get().is_null(){String::new()}else{serde_json::to_string_pretty(&result.get()).unwrap_or_default()}}</pre>
            </div>
        </details>
    }
}

#[component]
fn ToolWorkbench(status: RwSignal<StatusView>, error: RwSignal<Option<String>>) -> impl IntoView {
    let session = RwSignal::new(String::new());
    let path = RwSignal::new(String::new());
    let content = RwSignal::new(String::new());
    let expected = RwSignal::new(None::<String>);
    let preview_target = RwSignal::new(String::new());
    let preview_content = RwSignal::new(String::new());
    let preview_workspace = RwSignal::new(String::new());
    let patch_json = RwSignal::new(
        "{\"operations\":[{\"path\":\"files/example.txt\",\"content\":\"New text\\n\"}]}"
            .to_string(),
    );
    let patch_id = RwSignal::new(String::new());
    let patch_preview_json = RwSignal::new(String::new());
    let patch_session = RwSignal::new(String::new());
    let result = RwSignal::new(serde_json::Value::Null);
    let busy = RwSignal::new(false);
    let displayed_workspace = RwSignal::new(String::new());
    Effect::new(move |_| {
        let active = status.get().active_id;
        if active != displayed_workspace.get_untracked() {
            displayed_workspace.set(active);
            preview_target.set(String::new());
            preview_workspace.set(String::new());
            patch_id.set(String::new());
            patch_preview_json.set(String::new());
            patch_session.set(String::new());
            expected.set(None);
            result.set(serde_json::Value::Null);
        }
    });
    let tool = RwSignal::new("list_files".to_string());
    let arguments = RwSignal::new("{}".to_string());
    let execute = move |name: String, args: serde_json::Value| {
        let current = status.get_untracked();
        let selected = session.get_untracked();
        let selected = current
            .sessions
            .iter()
            .find(|s| s.id == selected)
            .or_else(|| {
                if selected.is_empty() {
                    current.sessions.first()
                } else {
                    None
                }
            })
            .map(|s| s.id.clone());
        let Some(selected) = selected else {
            error.set(Some("Connect a provider session first; its filesystem permissions will apply to this manual call.".into()));
            return;
        };
        let workspace = current.active_id;
        busy.set(true);
        spawn_local(async move {
            match workspace_request(json!({"op":"run_tool","workspace_id":workspace,"session_id":selected,"tool":name,"arguments":args})).await {
                Ok(value)=>{
                    if status.get_untracked().active_id!=workspace {busy.set(false);return;}
                    if value["status"]=="ok" && name=="preview_document" {
                        expected.set(value["data"]["expected_sha256"].as_str().map(str::to_owned));preview_target.set(args["path"].as_str().unwrap_or_default().into());preview_content.set(args["content"].as_str().unwrap_or_default().into());
                        preview_workspace.set(value["data"]["workspace_id"].as_str().unwrap_or_default().into());
                    }
                    if value["status"]=="ok" && name=="preview_patch" {
                        patch_id.set(value["data"]["patch_id"].as_str().unwrap_or_default().into());
                        patch_preview_json.set(serde_json::to_string(&args).unwrap_or_default());
                        patch_session.set(selected.clone());
                    }
                    if value["status"]!="ok" {error.set(Some(value["error"].as_str().unwrap_or("Tool failed").to_string()));}
                    else{error.set(None);}
                    result.set(value);
                },Err(err)=>error.set(Some(err)),
            }
            busy.set(false);
            refresh(status, error);
        });
    };
    view! {
        <details style="border:1px solid #304451;padding:8px;margin-bottom:10px">
            <summary>"Files, preview and restore"</summary>
            <div style="display:grid;gap:6px;margin-top:8px">
                <label>"Use session permissions "<select style=CONTROL_STYLE on:change=move|ev|{session.set(event_target_value(&ev));preview_target.set(String::new());patch_preview_json.set(String::new());}>
                    <option value="">"First connected session"</option>
                    {move||status.get().sessions.into_iter().map(|s|view!{<option value=s.id>{format!("{} · tab {}",s.agent,s.tab_id)}</option>}).collect_view()}
                </select></label>
                <input aria-label="File path" style=CONTROL_STYLE placeholder="notes/example.md" prop:value=move||path.get() on:input=move|ev|{path.set(event_target_value(&ev));preview_target.set(String::new());}/>
                <textarea aria-label="Replacement file content" style=CONTROL_STYLE rows="6" placeholder="New file contents" prop:value=move||content.get() on:input=move|ev|content.set(event_target_value(&ev))></textarea>
                <div style="display:flex;gap:5px;flex-wrap:wrap">
                    <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|execute("read_file".into(),json!({"path":path.get()}))>"Read"</button>
                    <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|execute("preview_document".into(),json!({"path":path.get(),"content":content.get()}))>"Preview change"</button>
                    <button style=CONTROL_STYLE disabled=move||busy.get()||preview_workspace.get()!=status.get().active_id||preview_target.get()!=path.get()||preview_content.get()!=content.get()||path.get().is_empty() on:click=move |_|{
                        let mut args=json!({"path":path.get(),"content":content.get(),"create_only":expected.get().is_none()});if let Some(hash)=expected.get(){args["expected_sha256"]=hash.into();}execute("write_document".into(),args);
                        preview_target.set(String::new());
                    }>"Apply preview"</button>
                    <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|execute("list_changes".into(),json!({"limit":30}))>"Recovery history"</button>
                </div>
                {move||result.get()["data"]["changes"].as_array().cloned().unwrap_or_default().into_iter().map(|change|{
                    let id=change["change_id"].as_str().unwrap_or_default().to_string();
                    view!{<div>{format!("{} · {} · {} · {}",change["created_at"],change["operation"],change["path"],change["state"])}<button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|execute("restore_change".into(),json!({"change_id":id}))>"Request restore"</button></div>}
                }).collect_view()}
                <details><summary>"Changes across files"</summary>
                    <p>"Preview up to 20 UTF-8 files. Include expected_sha256 for existing files. Apply and restore each require approval; files commit separately."</p>
                    <textarea aria-label="Patch operations JSON" style=CONTROL_STYLE rows="6" prop:value=move||patch_json.get() on:input=move|ev|{patch_json.set(event_target_value(&ev));patch_preview_json.set(String::new());}></textarea>
                    <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|match serde_json::from_str(&patch_json.get()){Ok(args)=>execute("preview_patch".into(),args),Err(err)=>error.set(Some(format!("Invalid patch JSON: {err}")))}>"Preview batch"</button>
                    <input aria-label="Patch ID" style=CONTROL_STYLE placeholder="Preview or saved patch ID" prop:value=move||patch_id.get() on:input=move|ev|{patch_id.set(event_target_value(&ev));patch_preview_json.set(String::new());}/>
                    <div style="display:flex;gap:5px;flex-wrap:wrap">
                        <button style=CONTROL_STYLE disabled=move||{busy.get()||patch_id.get().is_empty()||patch_preview_json.get().is_empty()||serde_json::from_str::<serde_json::Value>(&patch_json.get()).ok().map(|v|v.to_string()).unwrap_or_default()!=patch_preview_json.get()||!status.get().sessions.iter().any(|s|s.id==patch_session.get())} on:click=move |_|{execute("apply_patch".into(),json!({"patch_id":patch_id.get()}));patch_preview_json.set(String::new());}>"Request batch apply"</button>
                        <button style=CONTROL_STYLE disabled=move||busy.get()||patch_id.get().is_empty() on:click=move |_|execute("patch_status".into(),json!({"patch_id":patch_id.get()}))>"Batch status"</button>
                        <button style=CONTROL_STYLE disabled=move||busy.get()||patch_id.get().is_empty() on:click=move |_|execute("restore_patch".into(),json!({"patch_id":patch_id.get()}))>"Request batch restore"</button>
                    </div>
                </details>
                <details><summary>"Run any available tool"</summary>
                    <select aria-label="Tool" style=CONTROL_STYLE on:change=move|ev|tool.set(event_target_value(&ev))>
                        {move||status.get().capabilities["tools"].as_array().cloned().unwrap_or_default().into_iter().map(|entry|{
                            let name=entry["name"].as_str().unwrap_or_default().to_string();let label=format!("{} · {}{}",entry["category"].as_str().unwrap_or_default(),name,if entry["writes"]==true{" [writes]"}else{""});view!{<option value=name>{label}</option>}
                        }).collect_view()}
                    </select>
                    <div>{move||bridge_core::catalog::find(&tool.get()).map(|spec|format!("{} · Arguments: {}",spec.description,spec.arguments)).unwrap_or_default()}</div>
                    <textarea aria-label="Tool arguments JSON" style=CONTROL_STYLE rows="4" prop:value=move||arguments.get() on:input=move|ev|arguments.set(event_target_value(&ev))></textarea>
                    <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|match serde_json::from_str(&arguments.get()){Ok(args)=>execute(tool.get(),args),Err(err)=>error.set(Some(format!("Invalid argument JSON: {err}")))}>"Run tool"</button>
                </details>
                <div>{move||if busy.get(){"Running; check pending approvals below."}else{"Results stay here; manual calls do not send messages into chats."}}</div>
                <pre style="white-space:pre-wrap;overflow:auto;max-height:320px;font-size:10px">{move||if result.get().is_null(){String::new()}else{serde_json::to_string_pretty(&result.get()).unwrap_or_default()}}</pre>
            </div>
        </details>
    }
}

#[component]
fn CommandControls(status: RwSignal<StatusView>, error: RwSignal<Option<String>>) -> impl IntoView {
    let command_id = RwSignal::new(String::new());
    let program = RwSignal::new(String::new());
    let args = RwSignal::new("[]".to_string());
    let cwd = RwSignal::new(".".to_string());
    let timeout = RwSignal::new("120".to_string());
    let limit = RwSignal::new("20000".to_string());
    let writes = RwSignal::new(true);
    let always = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let grants = RwSignal::new(Vec::<serde_json::Value>::new());
    let notice = RwSignal::new(String::new());
    let presets = RwSignal::new(Vec::<serde_json::Value>::new());
    let reload = move || {
        spawn_local(async move {
            match workspace_request(json!({"op":"command_list"})).await {
                Ok(result) => {
                    grants.set(result["commands"].as_array().cloned().unwrap_or_default())
                }
                Err(err) => error.set(Some(err)),
            }
        })
    };
    let last_workspace = RwSignal::new(String::new());
    Effect::new(move |_| {
        let active = status.get().active_id;
        if !active.is_empty() && last_workspace.get_untracked() != active {
            last_workspace.set(active);
            reload();
        }
    });
    view! {
        <details style="border:1px solid #304451;padding:8px;margin-bottom:10px;background:#101d26">
            <summary>"Allowed commands"</summary>
            <div style="display:grid;gap:6px;margin-top:8px">
                <div>"Only commands you grant here can run. Approval is required each time unless you enable Always allow. Commands run as your macOS user."</div>
                <button style=CONTROL_STYLE on:click=move |_|spawn_local(async move{match workspace_request(json!({"op":"command_presets"})).await{Ok(result)=>{presets.set(result["presets"].as_array().cloned().unwrap_or_default());notice.set("Choose a detected command, review its settings, then Grant command. Build/test commands execute project code.".into());},Err(err)=>error.set(Some(err))}})>"Detect project commands"</button>
                {move||presets.get().into_iter().map(|preset|{
                    let label=format!("Use preset: {} {}",preset["program"],preset["fixed_args"]);
                    view!{<button style=CONTROL_STYLE on:click=move |_|{command_id.set(preset["command_id"].as_str().unwrap_or_default().into());program.set(preset["program"].as_str().unwrap_or_default().into());args.set(preset["fixed_args"].to_string());cwd.set(".".into());writes.set(preset["writes"]==true);always.set(false);}>{label}</button>}
                }).collect_view()}
                {move || grants.get().into_iter().map(|grant| {
                    let id=grant["command_id"].as_str().unwrap_or_default().to_string();
                    let description=format!("{} · {} {} · cwd {} · {}s · {} bytes",id,grant["program"].as_str().unwrap_or_default(),grant["fixed_args"],grant["cwd"].as_str().unwrap_or("."),grant["timeout_secs"],grant["max_output_bytes"]);
                    view!{<div style="font-family:monospace;word-break:break-all">{description}<button style=CONTROL_STYLE on:click=move |_|{let id=id.clone();spawn_local(async move{match workspace_request(json!({"op":"command_revoke","command_id":id})).await{Ok(_)=>{notice.set("Command grant removed.".into());error.set(None);reload();},Err(err)=>error.set(Some(err))}});}>"Remove grant"</button></div>}
                }).collect_view()}
                <input aria-label="Command ID" placeholder="cargo-test" style=CONTROL_STYLE prop:value=move||command_id.get() on:input=move|ev|command_id.set(event_target_value(&ev))/>
                <input aria-label="Program path" placeholder="/absolute/path/to/program" style=CONTROL_STYLE prop:value=move||program.get() on:input=move|ev|program.set(event_target_value(&ev))/>
                <input aria-label="Fixed command arguments" placeholder="[\"test\"]" style=CONTROL_STYLE prop:value=move||args.get() on:input=move|ev|args.set(event_target_value(&ev))/>
                <input aria-label="Command working directory" placeholder="." style=CONTROL_STYLE prop:value=move||cwd.get() on:input=move|ev|cwd.set(event_target_value(&ev))/>
                <label>"Timeout seconds "<input aria-label="Command timeout seconds" type="number" min="1" max="3600" style=CONTROL_STYLE prop:value=move||timeout.get() on:input=move|ev|timeout.set(event_target_value(&ev))/></label>
                <label>"Output byte limit "<input aria-label="Command output byte limit" type="number" min="128" max="50000" style=CONTROL_STYLE prop:value=move||limit.get() on:input=move|ev|limit.set(event_target_value(&ev))/></label>
                <label><input type="checkbox" prop:checked=move||writes.get() on:change=move|ev|writes.set(event_target_checked(&ev))/>" Command may write files"</label>
                <label><input type="checkbox" prop:checked=move||always.get() on:change=move|ev|always.set(event_target_checked(&ev))/>" Always allow this command in this workspace"</label>
                <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_| {
                    let parsed=serde_json::from_str::<Vec<String>>(&args.get());
                    let (Ok(fixed_args),Ok(seconds),Ok(bytes))=(parsed,timeout.get().parse::<u64>(),limit.get().parse::<usize>()) else {error.set(Some("Arguments must be a JSON array of strings; timeout and byte limit must be positive integers.".into()));return;};
                    let command=json!({"command_id":command_id.get(),"program":program.get(),"fixed_args":fixed_args,"cwd":cwd.get(),"timeout_secs":seconds,"max_output_bytes":bytes,"writes":writes.get(),"always_allow":always.get()});
                    busy.set(true);
                    spawn_local(async move{match workspace_request(json!({"op":"command_grant","command":command})).await{Ok(_)=>{notice.set("Command granted for the current workspace.".into());error.set(None);reload();},Err(err)=>error.set(Some(err))};busy.set(false);});
                }>"Grant command"</button>
                <div>{move||notice.get()}</div>
            </div>
        </details>
    }
}

async fn workspace_request(request: serde_json::Value) -> Result<serde_json::Value, String> {
    let response = oxichrome::runtime::send_message(&request)
        .await
        .map_err(|err| err.to_string())?;
    let response =
        js::from_js::<BackgroundResponse>(response).ok_or("Invalid workspace response")?;
    if response.ok {
        Ok(response.result.unwrap_or_default())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "Workspace operation failed".into()))
    }
}

async fn select_workspace(id: String, status: RwSignal<StatusView>) -> Result<(), String> {
    let mut confirm = false;
    if status.get_untracked().in_flight > 0 {
        confirm = web_sys::window().and_then(|window|window.confirm_with_message("Tools are still running. Switch workspace? Those calls will finish against the original folder.").ok()).unwrap_or(false);
        if !confirm {
            return Err("Workspace switch cancelled.".into());
        }
    }
    workspace_request(
        json!({"op":"workspace_select","workspace_id":id,"confirm_in_flight":confirm}),
    )
    .await?;
    Ok(())
}

#[component]
fn WorkspaceControls(
    status: RwSignal<StatusView>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let path = RwSignal::new(String::new());
    let label = RwSignal::new(String::new());
    let profile = RwSignal::new("general".to_string());
    let selected = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let notice = RwSignal::new(String::new());
    let perform = move |request: serde_json::Value| {
        busy.set(true);
        spawn_local(async move {
            match workspace_request(request).await {
                Ok(_) => {
                    notice.set("Workspace settings updated.".into());
                    error.set(None);
                }
                Err(err) => error.set(Some(err)),
            }
            busy.set(false);
            refresh(status, error);
        });
    };
    view! {
        <details style="border:1px solid #304451;padding:8px;margin-bottom:10px;background:#101d26">
            <summary style="cursor:pointer;color:#d8e6ef">"Workspace controls"</summary>
            <div style="display:grid;gap:6px;margin-top:8px">
                <div>{move || format!("{} · {}",status.get().capabilities["profile"].as_str().unwrap_or("pending"),if status.get().capabilities["read_only"].as_bool()==Some(true){"read-only"}else{"writable"})}</div>
                <select aria-label="Recent workspaces" style=CONTROL_STYLE on:change=move |event|selected.set(event_target_value(&event))>
                    <option value="">"Recent workspaces…"</option>
                    {move || status.get().workspaces.into_iter().map(|workspace|view!{<option value=workspace.id>{format!("{} · {}",workspace.label,workspace.profile.name())}</option>}).collect_view()}
                </select>
                <div style="display:flex;gap:5px;flex-wrap:wrap">
                    <button style=CONTROL_STYLE disabled=move || busy.get() || selected.get().is_empty() on:click=move |_| {
                        busy.set(true); let id=selected.get();
                        spawn_local(async move {match select_workspace(id,status).await{Ok(())=>{notice.set("Workspace switched; connected chats are receiving updated instructions.".into());error.set(None);},Err(err)=>error.set(Some(err))};busy.set(false);refresh(status,error);});
                    }>"Switch workspace"</button>
                    <button style=CONTROL_STYLE disabled=move || busy.get() || selected.get().is_empty() on:click=move |_|perform(json!({"op":"workspace_forget","workspace_id":selected.get()}))>"Forget selected"</button>
                </div>
                <input aria-label="Workspace path" placeholder="/Users/you/project" style=CONTROL_STYLE prop:value=move ||path.get() on:input=move |event|path.set(event_target_value(&event))/>
                <input aria-label="Workspace label" placeholder="Workspace label" style=CONTROL_STYLE prop:value=move ||label.get() on:input=move |event|label.set(event_target_value(&event))/>
                <select aria-label="Workspace profile" style=CONTROL_STYLE prop:value=move ||profile.get() on:change=move |event|profile.set(event_target_value(&event))>
                    <option value="general">"general · everyday files"</option><option value="code">"code · source projects"</option><option value="research">"research · agent workspaces"</option>
                </select>
                <div style="display:flex;gap:5px;flex-wrap:wrap">
                    <button style=CONTROL_STYLE disabled=move ||busy.get() on:click=move |_| {
                        busy.set(true);
                        spawn_local(async move {match workspace_request(json!({"op":"workspace_pick"})).await{Ok(result)=>{path.set(result["path"].as_str().unwrap_or_default().into());error.set(None);},Err(err)=>error.set(Some(err))};busy.set(false);});
                    }>"Choose folder…"</button>
                    <button style=CONTROL_STYLE disabled=move ||busy.get() ||path.get().trim().is_empty() on:click=move |_| {
                        busy.set(true); let root=path.get();let name=if label.get().trim().is_empty(){root.trim_end_matches('/').rsplit('/').next().unwrap_or("Workspace").to_string()}else{label.get()};let selected_profile=profile.get();
                        spawn_local(async move {
                            let outcome=async {let created=workspace_request(json!({"op":"workspace_create","label":name,"path":root,"profile":selected_profile})).await?;let id=created["id"].as_str().ok_or("Host did not return a workspace ID")?.to_string();select_workspace(id,status).await}.await;
                            match outcome{Ok(())=>{notice.set("Workspace created and selected; chats are receiving its context.".into());error.set(None);},Err(err)=>error.set(Some(err))};busy.set(false);refresh(status,error);
                        });
                    }>"Create / use folder"</button>
                </div>
                <div style="display:flex;gap:5px;flex-wrap:wrap">
                    <button style=CONTROL_STYLE on:click=move |_|perform(json!({"op":"workspace_open"}))>"Open in Finder"</button>
                    <button style=CONTROL_STYLE on:click=move |_| {
                        let root=status.get().workspace;
                        spawn_local(async move {if let Some(window)=web_sys::window(){match wasm_bindgen_futures::JsFuture::from(window.navigator().clipboard().write_text(&root)).await{Ok(_)=>notice.set("Workspace path copied.".into()),Err(_)=>error.set(Some("Could not copy workspace path.".into()))}}});
                    }>"Copy path"</button>
                    <button style=CONTROL_STYLE on:click=move |_|perform(json!({"op":"workspace_rescan"}))>"Rescan structure"</button>
                </div>
                <label><input type="checkbox" prop:checked=move ||status.get().capabilities["read_only"].as_bool().unwrap_or(false) on:change=move |event|perform(json!({"op":"workspace_policy","read_only":event_target_checked(&event),"permission_mode":status.get().capabilities["permissions"]}))/>" Lock workspace (read-only)"</label>
                <select aria-label="Permission mode" style=CONTROL_STYLE prop:value=move ||status.get().capabilities["permissions"].as_str().unwrap_or("standard").to_string() on:change=move |event|perform(json!({"op":"workspace_policy","read_only":status.get().capabilities["read_only"].as_bool().unwrap_or(false),"permission_mode":event_target_value(&event)}))>
                    <option value="strict">"Strict · approve every write"</option><option value="standard">"Standard · write within granted root"</option><option value="trusted">"Trusted · write within granted root"</option>
                </select>
                <div style="color:#93b5a3">{move ||notice.get()}</div>
            </div>
        </details>
        <details style="margin-bottom:10px;color:#94aabc">
            <summary>"Diagnostics"</summary>
            <div style="font-family:monospace;word-break:break-all">
                <div>{move ||format!("Audit: {}",if status.get().audit_valid{"valid"}else{"unverified / invalid"})}</div>
                <div>{move ||format!("DB: {}",status.get().database_path)}</div>
                <div>{move ||format!("Log: {}",status.get().log_path)}</div>
                <div>"Run unified-ai-bridge-native doctor for installation checks."</div>
            </div>
        </details>
    }
}

#[component]
fn SectionTitle(text: &'static str) -> impl IntoView {
    view! {
        <div style="color:#8ea2b1;font-size:9px;text-transform:uppercase;letter-spacing:0.08em;margin:0 0 4px 0">{text}</div>
    }
}

fn refresh(status: RwSignal<StatusView>, error: RwSignal<Option<String>>) {
    spawn_local(async move {
        let result = oxichrome::runtime::send_message(&json!({"op":"status"})).await;
        match result.ok().and_then(js::from_js::<StatusView>) {
            Some(next) => {
                status.set(next);
            }
            None => error.set(Some("Cannot reach the extension background worker.".into())),
        }
    });
}

fn decide(id: String, allow: bool, status: RwSignal<StatusView>, error: RwSignal<Option<String>>) {
    spawn_local(async move {
        let result = oxichrome::runtime::send_message(&json!({
            "op":"approval",
            "approval_id": id,
            "allow": allow,
        }))
        .await;
        if let Some(response) = result.ok().and_then(js::from_js::<BackgroundResponse>) {
            if !response.ok {
                error.set(response.error);
            }
        } else {
            error.set(Some(
                "The background worker did not answer the approval.".into(),
            ));
        }
        refresh(status, error);
    });
}

// Suppress unused import warnings without a blanket allow.
const _: Duration = Duration::from_millis(1);

#[component]
fn ReadinessControls(
    status: RwSignal<StatusView>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let report = RwSignal::new(String::new());
    view! {
        <details style="margin:8px 0"><summary>"Readiness check"</summary>
            <div>{move||{
                let s=status.get();
                if s.bridge!="connected" {"BLOCKED: native host is offline".into()}
                else if s.extension_build!=crate::model::EXTENSION_VERSION || s.host_version!=crate::model::EXTENSION_VERSION {format!("BLOCKED: loaded builds differ — panel {}, worker {}, host {}",crate::model::EXTENSION_VERSION,s.extension_build,s.host_version)}
                else if s.capabilities["read_only"]==true {"BLOCKED for writes: workspace is locked".into()}
                else {format!("Connected · {} registered sessions · account/composer readiness must be checked in each tab",s.sessions.len())}
            }}</div>
            <button style=CONTROL_STYLE on:click=move |_|spawn_local(async move{
                match workspace_request(json!({"op":"preflight"})).await{Ok(value)=>report.set(serde_json::to_string_pretty(&value).unwrap_or_default()),Err(e)=>error.set(Some(e))}
            })>"Check host and workspace"</button>
            <pre style="white-space:pre-wrap;overflow:auto;max-height:340px">{move||report.get()}</pre>
        </details>
    }
}

#[component]
fn JobControls(status: RwSignal<StatusView>, error: RwSignal<Option<String>>) -> impl IntoView {
    let jobs = RwSignal::new(Vec::<serde_json::Value>::new());
    let selected = RwSignal::new(String::new());
    let detail = RwSignal::new(String::new());
    let cursor = RwSignal::new(0u64);
    let output = RwSignal::new(String::new());
    let seen_workspace = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    Effect::new(move |_| {
        let root = status.get().active_id;
        if root != seen_workspace.get_untracked() {
            seen_workspace.set(root);
            jobs.set(vec![]);
            selected.set(String::new());
            cursor.set(0);
            output.set(String::new());
            detail.set(String::new());
        }
    });
    let request = move |action: serde_json::Value| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        let workspace = status.get_untracked().active_id;
        spawn_local(async move {
            let result = workspace_request(
                json!({"op":"job_control","workspace_id":workspace,"action":action}),
            )
            .await;
            if status.get_untracked().active_id == workspace {
                match result {
                    Ok(value) => {
                        error.set(None);
                        if let Some(rows) = value["jobs"].as_array() {
                            jobs.set(rows.clone());
                            detail.set(format!("Loaded {} jobs for this workspace.", rows.len()));
                            if selected.get_untracked().is_empty() {
                                selected.set(
                                    rows.first()
                                        .and_then(|j| j["job_id"].as_str())
                                        .unwrap_or_default()
                                        .into(),
                                );
                            }
                        } else if let Some(chunks) = value["chunks"].as_array() {
                            detail.set(
                                serde_json::to_string_pretty(&value["job"]).unwrap_or_default(),
                            );
                            let text = chunks
                                .iter()
                                .map(|c| {
                                    format!(
                                        "[{}] {}",
                                        c["stream"].as_str().unwrap_or_default(),
                                        c["text"].as_str().unwrap_or_default()
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            output.update(|v| {
                                v.push_str(&text);
                                if v.len() > 100_000 {
                                    let mut start = v.len() - 100_000;
                                    while !v.is_char_boundary(start) {
                                        start += 1;
                                    }
                                    v.drain(..start);
                                }
                            });
                            cursor.set(value["next_cursor"].as_u64().unwrap_or(0));
                        } else {
                            detail.set(serde_json::to_string_pretty(&value).unwrap_or_default());
                        }
                    }
                    Err(e) => error.set(Some(e)),
                }
            }
            busy.set(false);
        });
    };
    view! {
        <details style="margin:10px 0"><summary>"Command jobs"</summary>
            <div>"Approved background commands survive reconnects. Stop ends the process; it does not undo files already written. Refresh for the latest receipt."</div>
            <button style=CONTROL_STYLE disabled=move||busy.get() on:click=move |_|request(json!({"operation":"list"}))>"Refresh jobs"</button>
            <div style="display:grid;gap:4px;max-height:220px;overflow:auto">{move||jobs.get().into_iter().map(|j|{
                let id=j["job_id"].as_str().unwrap_or_default().to_string();
                let label=format!("{} · {} · {}",j["command_id"].as_str().unwrap_or_default(),j["state"].as_str().unwrap_or_default(),&id[..id.len().min(8)]);
                view!{<button style=CONTROL_STYLE on:click=move |_|{selected.set(id.clone());cursor.set(0);output.set(String::new());request(json!({"operation":"detail","job_id":id,"after":0}));}>{label}</button>}
            }).collect_view()}</div>
            <div style="font-family:monospace;word-break:break-all">{move||format!("Selected job: {}",selected.get())}</div>
            <button style=CONTROL_STYLE disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|request(json!({"operation":"detail","job_id":selected.get(),"after":cursor.get()}))>"Read status / next output"</button>
            <button style=CONTROL_STYLE disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|request(json!({"operation":"cancel","job_id":selected.get()}))>"Stop job"</button>
            <button style=CONTROL_STYLE disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|request(json!({"operation":"keep_awake","job_id":selected.get(),"seconds":3600}))>"Keep awake for job (up to 1 hour)"</button>
            <button style=CONTROL_STYLE disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|request(json!({"operation":"keep_awake","job_id":selected.get(),"seconds":0}))>"Release keep-awake"</button>
            <button style=CONTROL_STYLE disabled=move||busy.get()||selected.get().is_empty() on:click=move |_|request(json!({"operation":"export_report","job_id":selected.get()}))>"Save local job receipt"</button>
            <pre style="white-space:pre-wrap;overflow:auto;max-height:260px">{move||detail.get()}</pre>
            <pre style="white-space:pre-wrap;overflow:auto;max-height:260px">{move||output.get()}</pre>
        </details>
    }
}

#[component]
fn WriteGrantControls(
    status: RwSignal<StatusView>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let session = RwSignal::new(String::new());
    let directory = RwSignal::new(String::from("files"));
    let seconds = RwSignal::new(String::from("3600"));
    let calls = RwSignal::new(String::from("50"));
    let bytes = RwSignal::new(String::from("1048576"));
    let files = RwSignal::new(String::from("20"));
    let selected = RwSignal::new(vec![String::from("write_document")]);
    let grants = RwSignal::new(Vec::<serde_json::Value>::new());
    let notice = RwSignal::new(String::new());
    let request = move |message: serde_json::Value| {
        let root = status.get_untracked().active_id;
        spawn_local(async move {
            match workspace_request(message).await {
                Ok(value) => {
                    if status.get_untracked().active_id == root {
                        if let Some(list) = value["grants"].as_array() {
                            grants.set(list.clone());
                        }
                        notice.set(serde_json::to_string_pretty(&value).unwrap_or_default());
                        error.set(None);
                    }
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };
    view! {
        <details style="margin:8px 0"><summary>"Scoped file-write grants"</summary>
            <div>"Grant selected file operations to one registered session in an existing directory. Expiry, revocation and exhausted budgets stop further granted writes. Each attempted write reserves its full resulting byte size. Other capabilities require their own approval. A reconnect may require a new session grant."</div>
            <select aria-label="Write grant session" style=CONTROL_STYLE prop:value=move||session.get() on:change=move|ev|session.set(event_target_value(&ev))>
                <option value="">"Choose session"</option>{move||status.get().sessions.into_iter().filter(|s|s.workspace_id==status.get().active_id).map(|s|view!{<option value=s.id>{format!("{} · tab {} · {}",s.provider,s.tab_id,s.conversation.unwrap_or_default())}</option>}).collect_view()}
            </select>
            <input aria-label="Write grant directory" style=CONTROL_STYLE prop:value=move||directory.get() on:input=move|ev|directory.set(event_target_value(&ev))/>
            { ["write_document","edit_document","append_document","copy_file","create_folder"].into_iter().map(|tool|view!{<label><input type="checkbox" prop:checked=move||selected.get().iter().any(|t|t==tool) on:change=move|ev|{let checked=event_target_checked(&ev);selected.update(|v|{v.retain(|t|t!=tool);if checked{v.push(tool.into());}});}/>{tool}</label>}).collect_view()}
            <label>"Duration (seconds)"<input style=CONTROL_STYLE prop:value=move||seconds.get() on:input=move|ev|seconds.set(event_target_value(&ev))/></label>
            <label>"Write calls"<input style=CONTROL_STYLE prop:value=move||calls.get() on:input=move|ev|calls.set(event_target_value(&ev))/></label>
            <label>"Total bytes"<input style=CONTROL_STYLE prop:value=move||bytes.get() on:input=move|ev|bytes.set(event_target_value(&ev))/></label>
            <label>"Distinct destination paths"<input style=CONTROL_STYLE prop:value=move||files.get() on:input=move|ev|files.set(event_target_value(&ev))/></label>
            <button style=CONTROL_STYLE on:click=move |_|{
                match (seconds.get().parse::<u64>(),calls.get().parse::<u64>(),bytes.get().parse::<u64>(),files.get().parse::<usize>()){
                    (Ok(seconds),Ok(max_calls),Ok(max_bytes),Ok(max_files))=>request(json!({"op":"create_write_grant","workspace_id":status.get().active_id,"grant":{"session_id":session.get(),"directory":directory.get(),"tools":selected.get(),"seconds":seconds,"max_calls":max_calls,"max_bytes":max_bytes,"max_files":max_files}})),
                    _=>error.set(Some("Grant budgets must be positive whole numbers.".into())),
                }
            }>"Grant selected file operations"</button>
            <button style=CONTROL_STYLE on:click=move |_|request(json!({"op":"list_write_grants"}))>"Refresh grants"</button>
            {move||grants.get().into_iter().filter(|g|g["workspace_id"]==status.get().active_id).map(|g|{
                let sid=g["session_id"].as_str().unwrap_or_default().to_string();let summary=format!("{} · {} · calls {}/{} · bytes {}/{} · revoked {} · expires {}",sid,g["directory"],g["used_calls"],g["max_calls"],g["used_bytes"],g["max_bytes"],g["revoked"],g["expires_at"]);
                view!{<div style="word-break:break-all">{summary}<button style=CONTROL_STYLE on:click=move |_|request(json!({"op":"revoke_write_grant","workspace_id":status.get().active_id,"session_id":sid}))>"Revoke"</button></div>}
            }).collect_view()}
            <pre style="white-space:pre-wrap;max-height:200px;overflow:auto">{move||notice.get()}</pre>
        </details>
    }
}
