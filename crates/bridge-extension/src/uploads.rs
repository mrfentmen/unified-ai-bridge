//! ChatGPT attachment controls observed in the visible composer. No website
//! endpoints or framework internals. Uncertain uploads stay for user inspection.
use super::*;
use base64::Engine;
use bridge_core::browser_files::{Action, Scope, UploadFile, MAX_UPLOAD_BYTES};
use web_sys::{DataTransfer, File, FilePropertyBag, HtmlInputElement};

const INPUT: &str = "input[type=file][aria-label=\"Attach files\"]";
const CARDS: &str = "[data-composer-attachments] .group\\/composer-attachment";
struct Owned {
    id: String,
    sha256: String,
    turn: String,
    conversation: String,
    row: Element,
    name: String,
}
thread_local! { static OWNED: RefCell<Option<Owned>> = const { RefCell::new(None) }; }

fn user(adapter: Adapter) -> Result<String, String> {
    adapter
        .last_user(&document())
        .map(|e| adapter.user_id(&e))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Cannot identify the current user message for upload.".into())
}
fn composer_root(adapter: Adapter) -> Result<Element, String> {
    let doc = document();
    let input = doc
        .query_selector(INPUT)
        .map_err(|_| "Attachment selector failed.")?
        .ok_or("ChatGPT attachment input missing; this website's adapter needs an update.")?;
    let root = input
        .parent_element()
        .ok_or("Attachment container missing.")?;
    let editor = adapter.composer(&doc).ok_or("Composer not found.")?;
    if !root.contains(Some(&editor)) {
        return Err("Attachment input does not belong to the active composer.".into());
    }
    Ok(root)
}
fn preflight(state: &ContentState, scope: &Scope) -> Result<String, String> {
    generated_files::check_scope(state, scope)?;
    upload_quota()?;
    if state.provider != Provider::ChatGpt {
        return Err("Automatic attachments currently support ChatGPT only.".into());
    }
    let root = composer_root(state.adapter)?;
    let editor = state
        .adapter
        .composer(&document())
        .ok_or("Composer not found.")?;
    if !composer_is_empty(&editor) {
        return Err("The composer contains a draft; upload was not started.".into());
    }
    if root
        .query_selector_all(CARDS)
        .map_err(|_| "Cannot inspect attachments.")?
        .length()
        != 0
        || root
            .query_selector("[data-composer-attachments][data-visible-attachments]")
            .ok()
            .flatten()
            .is_some()
    {
        return Err("The composer already contains attachments. Send or remove them before requesting another file.".into());
    }
    user(state.adapter)
}
fn upload_quota() -> Result<(), String> {
    let nodes = document()
        .query_selector_all("[role=status], [role=alert]")
        .map_err(|_| "Cannot inspect upload status.")?;
    for i in 0..nodes.length() {
        let Some(node) = nodes.item(i).and_then(|n| n.dyn_into::<HtmlElement>().ok()) else {
            continue;
        };
        if !visible_control(&node)
            || node
                .closest("article, [data-message-author-role], [data-content-search-unit-key]")
                .ok()
                .flatten()
                .is_some()
        {
            continue;
        }
        let text = node.inner_text();
        if text.len() <= 2048 && text.to_lowercase().contains("out of attachments") {
            return Err(format!(
                "ChatGPT attachment limit: {text}. No upload retry; wait for the website's reset."
            ));
        }
    }
    Ok(())
}
fn ready(row: &Element, name: &str) -> bool {
    if !row.is_connected()
        || row
            .query_selector("[role=progressbar], [role=alert], [aria-busy=true]")
            .ok()
            .flatten()
            .is_some()
    {
        return false;
    }
    // Images use a preview div[role=button], while documents use a nested
    // button. Both are actual observed ChatGPT controls, not filename matches.
    if row.get_attribute("role").as_deref() == Some("button")
        && row.get_attribute("aria-label").as_deref() == Some(name)
        && row.get_attribute("aria-haspopup").as_deref() == Some("dialog")
        && row.get_attribute("aria-disabled").as_deref() != Some("true")
    {
        return row
            .query_selector("img")
            .ok()
            .flatten()
            .and_then(|e| e.dyn_into::<web_sys::HtmlImageElement>().ok())
            .is_some_and(|img| img.complete() && img.natural_width() > 0);
    }
    // A completed upload acquires a file preview button. The uploading card
    // has only its Remove control. Exact label comparison avoids CSS escaping.
    row.query_selector_all("button").ok().is_some_and(|nodes| {
        (0..nodes.length()).any(|i| {
            nodes
                .item(i)
                .and_then(|n| n.dyn_into::<Element>().ok())
                .is_some_and(|e| {
                    e.get_attribute("aria-label").as_deref() == Some(name)
                        && !e.has_attribute("disabled")
                })
        })
    })
}

async fn wait_for_change(root: &Element) {
    // MutationObserver completion also works when background tab timers are
    // throttled. The fallback only bounds a quiet page; it is not the signal
    // that an upload succeeded.
    let resolver = Rc::new(RefCell::new(None::<js_sys::Function>));
    let promise = js_sys::Promise::new(&mut |resolve, _| *resolver.borrow_mut() = Some(resolve));
    let wake = resolver.clone();
    let callback =
        Closure::<dyn FnMut(js_sys::Array, web_sys::MutationObserver)>::new(move |_, _| {
            if let Some(resolve) = wake.borrow_mut().take() {
                let _ = resolve.call0(&JsValue::NULL);
            }
        });
    let observer = web_sys::MutationObserver::new(callback.as_ref().unchecked_ref()).ok();
    if let Some(observer) = &observer {
        let options = web_sys::MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        options.set_attributes(true);
        let _ = observer.observe_with_options(root, &options);
    }
    js::schedule(1000, move || {
        if let Some(resolve) = resolver.borrow_mut().take() {
            let _ = resolve.call0(&JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    if let Some(observer) = observer {
        observer.disconnect();
    }
}
fn validate(file: &UploadFile) -> Result<Vec<u8>, String> {
    if !bridge_core::browser_files::valid_name(&file.name)
        || file.bytes > MAX_UPLOAD_BYTES
        || file.base64.len() > MAX_UPLOAD_BYTES.div_ceil(3) * 4
    {
        return Err("Attachment exceeds the 512 KiB transfer limit or has an unsafe name.".into());
    }
    if !matches!(
        file.mime.as_str(),
        "text/plain" | "image/png" | "image/jpeg" | "image/webp"
    ) {
        return Err("Unsupported attachment type.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&file.base64)
        .map_err(|_| "Invalid attachment encoding.")?;
    if bytes.len() != file.bytes {
        return Err("Attachment byte count changed.".into());
    }
    // Hash in Rust, matching the native byte commitment before entering DOM.
    use sha2::{Digest, Sha256};
    if hex::encode(Sha256::digest(&bytes)) != file.sha256 {
        return Err("Attachment hash changed in transit.".into());
    }
    Ok(bytes)
}
pub(super) async fn handle(
    state: Rc<RefCell<ContentState>>,
    message: Value,
) -> Result<Value, String> {
    let expires = message["expires_at_ms"].as_f64().unwrap_or_default();
    if expires <= js::now_ms() {
        return Err("Upload authorization expired before browser dispatch.".into());
    }
    let scope: Scope =
        serde_json::from_value(message["scope"].clone()).map_err(|_| "Invalid upload scope.")?;
    let action: Action =
        serde_json::from_value(message["action"].clone()).map_err(|_| "Invalid upload action.")?;
    let turn = preflight(&state.borrow(), &scope)?;
    let Action::Upload {
        file,
        turn_id,
        attempt_id,
    } = action
    else {
        return Ok(json!({"turn_id":turn}));
    };
    if turn != turn_id
        || attempt_id.len() != 32
        || !attempt_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Conversation turn or upload authorization changed.".into());
    }
    let bytes = validate(&file)?;
    let adapter = state.borrow().adapter;
    let root = composer_root(adapter)?;
    let input = root
        .query_selector(INPUT)
        .map_err(|_| "Attachment selector failed.")?
        .ok_or("Attachment input disappeared.")?
        .dyn_into::<HtmlInputElement>()
        .map_err(|_| "Invalid file input.")?;
    let array = js_sys::Array::new();
    array.push(&js_sys::Uint8Array::from(bytes.as_slice()));
    let options = FilePropertyBag::new();
    options.set_type(&file.mime);
    let web_file = File::new_with_u8_array_sequence_and_options(&array, &file.name, &options)
        .map_err(|_| "Could not construct the approved attachment.")?;
    let transfer = DataTransfer::new().map_err(|_| "Browser does not support file attachments.")?;
    transfer
        .items()
        .add_with_file(&web_file)
        .map_err(|_| "Cannot prepare file selection.")?;
    if preflight(&state.borrow(), &scope)? != turn || expires <= js::now_ms() {
        return Err("Upload authorization changed before dispatch.".into());
    }
    input.set_files(transfer.files().as_ref());
    let event = web_sys::EventInit::new();
    event.set_bubbles(true);
    input
        .dispatch_event(
            &web_sys::Event::new_with_event_init_dict("change", &event)
                .map_err(|_| "Cannot create file event.")?,
        )
        .map_err(|_| "Website did not accept the file event. Inspect the composer; no retry.")?;
    let deadline = js::now_ms() + 40_000.0;
    while js::now_ms() < deadline {
        upload_quota()?;
        generated_files::check_scope(&state.borrow(), &scope).map_err(|e| {
            format!("{e} The upload may have completed; inspect the website, no retry.")
        })?;
        if user(adapter)? != turn {
            return Err(
                "Conversation changed during upload. Inspect the website; no retry.".into(),
            );
        }
        let cards = root
            .query_selector_all(CARDS)
            .map_err(|_| "Cannot inspect uploaded attachments.")?;
        if cards.length() > 1 {
            return Err(
                "Another attachment appeared. Nothing will be submitted automatically.".into(),
            );
        }
        if let Some(row) = cards.item(0).and_then(|n| n.dyn_into::<Element>().ok()) {
            if ready(&row, &file.name) && adapter.send_button(&document()).is_some() {
                OWNED.with(|o| {
                    *o.borrow_mut() = Some(Owned {
                        id: attempt_id.clone(),
                        sha256: file.sha256.clone(),
                        turn: turn.clone(),
                        conversation: scope.conversation,
                        row,
                        name: file.name.clone(),
                    })
                });
                return Ok(json!({"state":"attached","upload_id":attempt_id,"sha256":file.sha256}));
            }
        }
        wait_for_change(&root).await;
    }
    Err("The website did not confirm the attachment within 40 seconds. Inspect the composer for progress or an error; the upload will not be retried.".into())
}

/// Every automated message, including setup, must preserve user attachments.
/// Only the result carrying our exact receipt may submit our completed card.
pub(super) fn check_send(adapter: Adapter, doc: &Document, text: &str) -> Result<(), String> {
    if adapter.provider != Provider::ChatGpt {
        return Ok(());
    }
    // Upload inputs appear later than the text editor during page hydration
    // and may disappear when the upload quota is exhausted. Text-only setup
    // and tool results still work; inspect the actual editor's attachment area.
    let composer = adapter.composer(doc).ok_or("Composer not found.")?;
    let root = composer
        .closest("[data-composer-surface-variant], form")
        .map_err(|_| "Cannot inspect the composer.")?
        .ok_or("Composer container changed; inspect attachments before sending manually.")?;
    let cards = root
        .query_selector_all(CARDS)
        .map_err(|_| "Cannot inspect composer attachments.")?;
    if cards.length() == 0 {
        if bridge_core::browser_files::upload_receipt(text).is_some() {
            return Err("The approved attachment is missing. The upload receipt was preserved; inspect the chat before sending manually.".into());
        }
        if root
            .query_selector("[data-composer-attachments][data-visible-attachments]")
            .ok()
            .flatten()
            .is_some()
        {
            return Err("Unrecognized attachments are present; submit them manually.".into());
        }
        return Ok(());
    }
    let authorized = OWNED.with(|o| {
        o.borrow().as_ref().is_some_and(|o| {
            cards.length() == 1
                && cards.item(0).is_some_and(|n| n.is_same_node(Some(&o.row)))
                && ready(&o.row, &o.name)
                && bridge_core::browser_files::has_upload_receipt(text, &o.id, &o.sha256)
                && user(adapter).ok().as_deref() == Some(&o.turn)
                && doc
                    .url()
                    .ok()
                    .and_then(|url| adapter.provider.conversation_id(&url))
                    .as_deref()
                    == Some(&o.conversation)
        })
    });
    if authorized {
        Ok(())
    } else {
        Err("Composer has an attachment not owned by this result. The bridge will not submit it; inspect and send it manually.".into())
    }
}
