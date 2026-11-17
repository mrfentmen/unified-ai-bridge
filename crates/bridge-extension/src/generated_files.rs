//! Visible DOM only. No React stores, provider endpoints, cookies or page JS.
use super::*;
use bridge_core::browser_files::{
    source_key, valid_name, Action, DownloadMethod, GeneratedFile, Scope,
};

struct Handle {
    scope: Scope,
    panel_user: Option<String>,
    file: GeneratedFile,
    node: Element,
    row: Element,
    url: Option<String>,
    expires: f64,
    used: bool,
}
thread_local! { static HANDLES: RefCell<Vec<Handle>> = const { RefCell::new(Vec::new()) }; }

pub(super) fn check_scope(state: &ContentState, scope: &Scope) -> Result<Option<String>, String> {
    let doc = document();
    if recovery::website_block(state.adapter, &doc) == Some(WebsiteBlock::AccountChanged) {
        return Err(WebsiteBlock::AccountChanged.message().into());
    }
    let url = doc.url().map_err(|_| "Cannot read conversation URL.")?;
    if !state.connected
        || state.paused
        || state.session_id.as_deref() != Some(&scope.session_id)
        || state.workspace_id != scope.workspace_id
        || state.provider.slug() != scope.provider
        || state.provider.conversation_id(&url).as_deref() != Some(&scope.conversation)
        || state.conversation.as_deref() != Some(&scope.conversation)
    {
        return Err("Generated-file scope changed. Refresh the connected conversation.".into());
    }
    if state.adapter.is_generating(&doc) {
        return Err("Wait for the website to finish its response before downloading files.".into());
    }
    let user = state
        .adapter
        .last_user(&doc)
        .ok_or("Cannot identify the current conversation turn.")?;
    let user_id = state.adapter.user_id(&user);
    if user_id.is_empty() {
        return Err("Cannot identify the current user message.".into());
    }
    if let Some(task) = &scope.task_id {
        if state.memory.task_id.as_ref() != Some(task)
            || state.observed_user.as_deref() != Some(&user_id)
            || state.task_gate.as_ref().is_some_and(|(id, _)| id == task)
        {
            return Err(
                "The task changed or stopped; this generated-file request is stale.".into(),
            );
        }
        Ok(None)
    } else {
        Ok(Some(user_id))
    }
}

fn visible(node: &Element) -> bool {
    let rect = node.get_bounding_client_rect();
    node.is_connected()
        && rect.width() > 0.0
        && rect.height() > 0.0
        && node
            .closest("[aria-hidden=\"true\"], [hidden]")
            .ok()
            .flatten()
            .is_none()
}

fn discover(
    state: &ContentState,
    scope: &Scope,
    panel_user: Option<String>,
) -> Result<Value, String> {
    let doc = document();
    let mut candidates = Vec::new();
    for selector in state.adapter.assistants() {
        let nodes = doc
            .query_selector_all(selector)
            .map_err(|_| "Assistant selector failed.")?;
        if nodes.length() == 0 {
            continue;
        }
        for i in nodes.length().saturating_sub(30)..nodes.length() {
            let Some(turn) = nodes.item(i).and_then(|n| n.dyn_into::<Element>().ok()) else {
                continue;
            };
            let Some(response) = state.adapter.response_id(&turn) else {
                continue;
            };
            if state.provider == Provider::ChatGpt {
                // Observed ChatGPT resource cards. Ordinary links and message
                // action menus are never treated as generated-file buttons.
                let rows = turn
                    .query_selector_all("span.group\\/resource-row")
                    .map_err(|_| "ChatGPT file-card selector failed.")?;
                for j in 0..rows.length().min(40) {
                    let Some(row) = rows.item(j).and_then(|n| n.dyn_into::<Element>().ok()) else {
                        continue;
                    };
                    let Some(name) = row
                        .query_selector("span[title]")
                        .ok()
                        .flatten()
                        .and_then(|n| n.get_attribute("title"))
                    else {
                        continue;
                    };
                    if !visible(&row) || !valid_name(&name) {
                        continue;
                    }
                    let preview = row
                        .query_selector("button[aria-label^=\"Open preview of \"]")
                        .ok()
                        .flatten();
                    if preview
                        .and_then(|p| p.get_attribute("aria-label"))
                        .as_deref()
                        != Some(&format!("Open preview of {name}"))
                    {
                        continue;
                    }
                    let Ok(buttons) =
                        row.query_selector_all("button[aria-label=\"Download file\"]")
                    else {
                        continue;
                    };
                    if buttons.length() != 1 {
                        continue;
                    }
                    let Some(button) = buttons.item(0).and_then(|n| n.dyn_into::<Element>().ok())
                    else {
                        continue;
                    };
                    candidates.push((response.clone(), name, button, row, None));
                }
            }
            let links = turn
                .query_selector_all("a[download][href]")
                .map_err(|_| "Download-link selector failed.")?;
            for j in 0..links.length().min(40) {
                let Some(link) = links.item(j).and_then(|n| n.dyn_into::<Element>().ok()) else {
                    continue;
                };
                let name = link.get_attribute("download").unwrap_or_default();
                let Some(raw) = link.get_attribute("href") else {
                    continue;
                };
                let Ok(url) =
                    url::Url::parse(&doc.url().unwrap_or_default()).and_then(|u| u.join(&raw))
                else {
                    continue;
                };
                if !visible(&link)
                    || !valid_name(&name)
                    || !crate::artifacts::allowed_file_url(state.provider, url.as_str())
                {
                    continue;
                }
                candidates.push((
                    response.clone(),
                    name,
                    link.clone(),
                    link,
                    Some(url.to_string()),
                ));
            }
        }
        break;
    }
    // Two same-named button cards in one response cannot be distinguished by
    // public metadata. Exclude both instead of guessing which file is meant.
    let mut name_counts = std::collections::HashMap::<(String, String), usize>::new();
    for (response, name, _, _, _) in &candidates {
        *name_counts
            .entry((response.clone(), name.clone()))
            .or_default() += 1;
    }
    let mut seen = HashSet::new();
    let files = HANDLES.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|h| {
            // Panel discovery and model discovery have different task scopes.
            // Refreshing one must not revoke the other's still-valid handles.
            check_scope(state, &h.scope).is_ok_and(|user| user == h.panel_user)
                && h.expires > js::now_ms()
                && h.node.is_connected()
        });
        let mut files = Vec::new();
        for (response, name, node, row, url) in candidates {
            if name_counts.get(&(response.clone(), name.clone())).copied() != Some(1) {
                continue;
            }
            if files.len() >= 40 {
                break;
            }
            let key = source_key(&[
                &scope.provider,
                &scope.conversation,
                &response,
                &name,
                if url.is_some() { "link" } else { "button" },
            ]);
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(old) = cache.iter().find(|h| {
                h.scope == *scope && h.panel_user == panel_user
                    && h.file.source_key == key && h.node.is_same_node(Some(&node)) && h.url == url
            }) {
                if !old.used {
                    files.push(old.file.clone());
                }
                continue;
            }
            let file = GeneratedFile {
                handle: format!("{:016x}{:016x}", js::random_u64(), js::random_u64()),
                name,
                response_id: response,
                source_key: key,
                method: if url.is_some() {
                    DownloadMethod::DirectLink
                } else {
                    DownloadMethod::WebsiteButton
                },
            };
            if !file.valid() {
                continue;
            }
            files.push(file.clone());
            cache.push(Handle {
                scope: scope.clone(),
                panel_user: panel_user.clone(),
                file,
                node,
                row,
                url,
                expires: js::now_ms() + 300_000.0,
                used: false,
            });
        }
        while cache.len() > 80 {
            cache.remove(0);
        }
        files
    });
    Ok(json!({"files":files}))
}

pub(super) fn handle(state: &ContentState, scope: Scope, action: Action) -> Result<Value, String> {
    let panel_user = check_scope(state, &scope)?;
    if matches!(action, Action::List) {
        return discover(state, &scope, panel_user);
    }
    let id = match &action {
        Action::Inspect { handle } => handle,
        Action::Download { file } => &file.handle,
        _ => unreachable!(),
    };
    HANDLES.with(|cache| {
        let mut cache = cache.borrow_mut();
        let h = cache.iter_mut().find(|h|h.file.handle==*id).ok_or("Unknown file handle; rediscover rendered files.")?;
        if h.scope!=scope || h.panel_user!=panel_user || h.expires<=js::now_ms() || h.used
            || !visible(&h.row) || !h.node.is_connected() || !h.row.contains(Some(&h.node)) {
            return Err("File handle expired, changed or was already used. Inspect downloads before retrying.".into());
        }
        if h.node.has_attribute("disabled") || h.node.get_attribute("aria-disabled").as_deref()==Some("true") {
            return Err("The website's file download control is disabled.".into());
        }
        let response = state.adapter.assistants().iter().find_map(|selector|
            h.row.closest(selector).ok().flatten().and_then(|turn|state.adapter.response_id(&turn)));
        if response.as_deref()!=Some(&h.file.response_id) {return Err("File moved to another response; rediscover files.".into());}
        // Recheck the actual rendered control rather than trusting our old node.
        if let Some(expected) = &h.url {
            let raw = h.node.get_attribute("href").ok_or("Download link disappeared.")?;
            let url = url::Url::parse(&document().url().unwrap_or_default()).and_then(|u|u.join(&raw)).map_err(|_|"Download link changed.")?;
            if url.as_str()!=expected || h.node.get_attribute("download").as_deref()!=Some(&h.file.name) {return Err("Download link changed; rediscover files.".into());}
        } else if h.node.get_attribute("aria-label").as_deref()!=Some("Download file")
            || h.row.query_selector("span[title]").ok().flatten().and_then(|n|n.get_attribute("title")).as_deref()!=Some(&h.file.name) {
            return Err("Website file card changed; rediscover files.".into());
        }
        if let Action::Download{file} = &action {
            if file!=&h.file {return Err("Approved file metadata changed.".into());}
            // Mark used before any side effect. Native durable claims protect
            // the same source across reloads and new sessions as well.
            h.used = true;
            if let Some(url) = &h.url {return Ok(json!({"state":"direct_link","url":url}));}
            let button = h.node.dyn_ref::<HtmlElement>().ok_or("Download button is not clickable.")?;
            button.click();
            Ok(json!({"state":"manual_share_required"}))
        } else { serde_json::to_value(&h.file).map_err(|_|"Cannot describe generated file.".into()) }
    })
}
