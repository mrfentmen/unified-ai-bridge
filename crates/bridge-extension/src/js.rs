use js_sys::{Function, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["chrome", "tabs"], js_name = query, catch)]
    pub fn query_tabs(query: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_namespace = ["chrome", "downloads"], js_name = search, catch)]
    pub fn downloads_search(query: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_namespace = ["chrome", "runtime"], js_name = connectNative)]
    fn connect_native_port(application: &str) -> JsValue;

    #[wasm_bindgen(js_namespace = ["chrome", "runtime", "onMessage"], js_name = addListener)]
    pub fn runtime_on_message_listener(
        callback: &Closure<dyn FnMut(JsValue, JsValue, JsValue) -> JsValue>,
    );
    #[wasm_bindgen(js_namespace = ["chrome", "tabs", "onRemoved"], js_name = addListener)]
    pub fn tabs_on_removed_listener(callback: &Closure<dyn FnMut(JsValue)>);

    #[wasm_bindgen(js_namespace = ["chrome", "runtime"], js_name = getURL)]
    pub fn extension_url(path: &str) -> String;

    #[wasm_bindgen(js_namespace = ["chrome", "tabs"], js_name = create, catch)]
    pub fn create_tab(properties: &JsValue) -> Result<js_sys::Promise, JsValue>;
    #[wasm_bindgen(js_namespace = ["chrome", "tabs"], js_name = update, catch)]
    pub fn update_tab(tab_id: i32, properties: &JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["chrome", "tabs"], js_name = sendMessage, catch)]
    pub fn tabs_send_message_raw(
        tab_id: i32,
        message: &JsValue,
    ) -> Result<js_sys::Promise, JsValue>;
}

/// Chrome native messaging and runtime messaging JSON-serialize their payloads.
/// `serde_wasm_bindgen::to_value` serializes maps as JavaScript `Map` objects,
/// which Chrome turns into `{}` on the wire. Round-trip through JSON instead so
/// every payload is a plain object with enumerable string keys.
fn json_to_js<T: Serialize + ?Sized>(value: &T) -> Result<JsValue, String> {
    let text = serde_json::to_string(value).map_err(|err| err.to_string())?;
    js_sys::JSON::parse(&text).map_err(|err| format!("parse serialized JSON: {err:?}"))
}

pub fn to_js<T: Serialize + ?Sized>(value: &T) -> JsValue {
    json_to_js(value).unwrap_or(JsValue::NULL)
}

/// `chrome.runtime.connectNative` returns a Port object. The wasm-bindgen
/// `catch` wrapper is deliberately not used here: in this build it catches the
/// call but drops the returned Port, leaving `undefined` even though Chrome has
/// already launched the host. A plain extern preserves the Port; failures are
/// surfaced by the missing-object check below.
pub fn connect_native(application: &str) -> Result<JsValue, JsValue> {
    let port = connect_native_port(application);
    if !port.is_object() {
        return Err(JsValue::from_str(
            "chrome.runtime.connectNative did not return a Port object",
        ));
    }
    Ok(port)
}

pub fn from_js<T: DeserializeOwned>(value: JsValue) -> Option<T> {
    serde_wasm_bindgen::from_value(value).ok()
}

pub fn add_listener_one(
    target: &JsValue,
    event: &str,
    callback: &Closure<dyn FnMut(JsValue)>,
) -> Result<(), JsValue> {
    let event = Reflect::get(target, &JsValue::from_str(event))?;
    let add = Reflect::get(&event, &JsValue::from_str("addListener"))?
        .dyn_into::<Function>()
        .map_err(|_| JsValue::from_str("event has no addListener"))?;
    let cb: &Function = callback.as_ref().unchecked_ref();
    add.call1(&event, cb.as_ref())?;
    Ok(())
}

pub fn port_post(port: &JsValue, message: &serde_json::Value) -> Result<(), String> {
    let post = Reflect::get(port, &JsValue::from_str("postMessage"))
        .ok()
        .and_then(|value| value.dyn_into::<Function>().ok())
        .ok_or_else(|| "native port has no postMessage".to_string())?;
    let value = json_to_js(message).map_err(|err| format!("serialize native message: {err}"))?;
    post.call1(port, &value)
        .map(|_| ())
        .map_err(|err| format!("native port postMessage failed: {err:?}"))
}

pub fn port_disconnect(port: &JsValue) {
    if let Ok(disconnect) = Reflect::get(port, &JsValue::from_str("disconnect")) {
        if let Some(disconnect) = disconnect.dyn_ref::<Function>() {
            let _ = disconnect.call0(port);
        }
    }
}

pub fn send_to_tab(tab_id: i32, message: &serde_json::Value) {
    let value = to_js(message);
    let _ = tabs_send_message_raw(tab_id, &value);
}

pub fn respond(function: &Function, value: &serde_json::Value) {
    let _ = function.call1(&JsValue::NULL, &to_js(value));
}

pub fn function(value: &JsValue) -> Option<Function> {
    value.clone().dyn_into::<Function>().ok()
}

pub fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// Works in the service worker as well as in a document.
pub fn schedule(delay_ms: i32, callback: impl FnOnce() + 'static) {
    let callback = Closure::once_into_js(callback);
    if let Ok(timer) = Reflect::get(&js_sys::global(), &JsValue::from_str("setTimeout")) {
        if let Some(timer) = timer.dyn_ref::<Function>() {
            let _ = timer.call2(&js_sys::global(), &callback, &JsValue::from(delay_ms));
        }
    }
}

pub fn random_u64() -> u64 {
    // Chrome exposes a cryptographic RNG; avoid Math.random for request keys.
    let crypto = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("crypto")).ok();
    if let Some(crypto) = crypto {
        let get_random = Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))
            .ok()
            .and_then(|value| value.dyn_into::<Function>().ok());
        if let Some(get_random) = get_random {
            let array = js_sys::Uint32Array::new_with_length(2);
            if get_random.call1(&crypto, &array).is_ok() {
                let values = array.to_vec();
                if values.len() == 2 {
                    return ((values[0] as u64) << 32) | values[1] as u64;
                }
            }
        }
    }
    now_ms() as u64
}
