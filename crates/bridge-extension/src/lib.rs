//! Unified AI Bridge browser extension.
//!
//! Everything here is Rust compiled to WebAssembly. Oxichrome generates the
//! MV3 manifest, service-worker shim, content-script loader, popup HTML, and
//! side-panel HTML. No JavaScript, TypeScript, or CSS files are maintained.

use leptos::prelude::*;
use oxichrome::prelude::*;

mod artifacts;
mod background;
mod content;
mod js;
mod model;
mod popup;

#[extension(
    name = "Unified AI Bridge",
    description = "Permission-checked local tools for ChatGPT, Claude, DeepSeek, Gemini, Grok, Qwen, and Kimi through one Rust native backend.",
    version = "0.5.8",
    permissions = [Storage, NativeMessaging, Downloads],
    host_permissions = [
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
    icons = { 16 = "icons/16.png", 32 = "icons/32.png", 48 = "icons/48.png", 128 = "icons/128.png" },
    action = { default_title = "Unified AI Bridge" }
)]
pub struct UnifiedAiBridge;

#[oxichrome::popup]
fn Popup() -> impl IntoView {
    popup::popup_view()
}

#[oxichrome::side_panel(title = "Unified AI Bridge")]
fn SidePanel() -> impl IntoView {
    popup::side_panel_view()
}
