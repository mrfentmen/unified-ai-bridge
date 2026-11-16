//! Browser response identity is independent of prose, session reconnects and
//! transport retries. These identifiers are supplied by the adapter, never the model.
use crate::provider::Provider;
use sha2::{Digest, Sha256};

/// Chrome's sender.url describes the document that loaded the content script;
/// sender.tab.url tracks SPA navigation. Both come from Chrome, never tool JSON.
/// Require the top frame, the bound tab/provider, and the current conversation.
pub fn browser_sender_matches(
    session_provider: &str,
    session_tab: i64,
    session_conversation: Option<&str>,
    sender_tab: i64,
    frame_id: Option<i32>,
    document_url: Option<&str>,
    current_tab_url: Option<&str>,
) -> bool {
    if sender_tab != session_tab || frame_id != Some(0) {
        return false;
    }
    let Some(document_provider) = document_url.and_then(Provider::from_url) else {
        return false;
    };
    let Some(current_url) = current_tab_url else {
        return false;
    };
    let Some(current_provider) = Provider::from_url(current_url) else {
        return false;
    };
    document_provider == current_provider
        && current_provider.slug() == session_provider
        && session_conversation.is_some()
        && current_provider.conversation_id(current_url).as_deref() == session_conversation
}

pub fn request_key(provider: &str, conversation: &str, response: &str, index: u32) -> String {
    let identity = serde_json::to_vec(&(provider, conversation, response, index)).unwrap();
    hex::encode(Sha256::digest(identity))
}

/// X exposes no persistent message IDs in its rendered transcript. Bind a
/// response slot to its preceding user text, not mutable assistant prose.
/// Regenerations and identical prompts deliberately share a slot: suppressing
/// an ambiguous repeat is safer than executing an old block a second time.
pub fn x_grok_turn(user_text: &str) -> Option<(String, String)> {
    conservative_turn("grok-x", user_text)
}

/// For rendered transcripts without stable message IDs, ambiguous repeats
/// share a slot. Provider namespaces keep independent adapters distinct.
pub fn conservative_turn(provider: &str, user_text: &str) -> Option<(String, String)> {
    let text = user_text.trim();
    if text.is_empty() {
        return None;
    }
    let key = request_key(&format!("{provider}-user"), "", text, 0);
    let prefix = if provider == "grok-x" { "x" } else { provider };
    Some((format!("{prefix}-user-{key}"), format!("{prefix}-reply-{key}")))
}

pub fn valid_turn_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 200 && !id.chars().any(char::is_control)
}

/// Kimi's Lexical editor commits insertText asynchronously and strips literal
/// line breaks in the rendered draft. Escaped JSON newlines are ordinary
/// backslash characters and must remain intact. Never accept other edits.
pub fn kimi_inserted_draft_matches(requested: &str, rendered: &str) -> bool {
    !rendered.trim().is_empty()
        && (rendered == requested || rendered == requested.replace(['\r', '\n'], ""))
}

pub fn reconnect_delay_ms(attempt: u32) -> u32 {
    1_000u32
        .saturating_mul(1u32 << attempt.saturating_sub(1).min(5))
        .min(30_000)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryTurn {
    pub conversation: String,
    pub user: String,
    pub assistant: String,
}

impl DeliveryTurn {
    pub fn is_current(&self, current: Option<&Self>) -> bool {
        current == Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_editor_normalization_preserves_payload_and_rejects_draft_changes() {
        let message = "Result\n[[BRIDGE_RESULT]]\n{\"output\":\"first\\nsecond\"}\n[[/BRIDGE_RESULT]]";
        let rendered = message.replace('\n', "");
        assert!(kimi_inserted_draft_matches(message, message));
        assert!(kimi_inserted_draft_matches(message, &rendered));
        assert!(!kimi_inserted_draft_matches(message, ""));
        assert!(!kimi_inserted_draft_matches(message, "my unsent draft"));
        assert!(!kimi_inserted_draft_matches(message, &format!("{rendered} extra")));
        assert!(!kimi_inserted_draft_matches(message, &rendered.replace("first", "changed")));
        assert!(!kimi_inserted_draft_matches(message, &rendered.replace("\\n", "")));
        assert!(!kimi_inserted_draft_matches(message, &rendered.replace("second", "")));
    }

    #[test]
    fn spa_sender_uses_current_chrome_tab_without_weakening_identity() {
        let valid = |tab, frame, document, current| {
            browser_sender_matches(
                "grok",
                17,
                Some("conversation-1234"),
                tab,
                frame,
                document,
                current,
            )
        };
        let current = Some("https://grok.com/c/conversation-1234?rid=response-5678");
        // The content script was loaded on the home page before New chat.
        assert!(valid(17, Some(0), Some("https://grok.com/"), current));
        assert!(valid(17, Some(0), current, current));
        assert!(!valid(18, Some(0), current, current));
        assert!(!valid(17, Some(1), current, current));
        assert!(!valid(17, None, current, current));
        assert!(!valid(17, Some(0), Some("https://evil.example/"), current));
        assert!(!valid(
            17,
            Some(0),
            Some("https://claude.ai/chat/conversation-1234"),
            current
        ));
        assert!(!valid(17, Some(0), None, current));
        assert!(!valid(17, Some(0), current, None));
        assert!(!valid(
            17,
            Some(0),
            current,
            Some("https://grok.com/c/other-conversation")
        ));
        assert!(!valid(17, Some(0), current, Some("https://grok.com/")));
    }

    #[test]
    fn reconnect_delay_is_bounded() {
        assert_eq!(
            (1..=7).map(reconnect_delay_ms).collect::<Vec<_>>(),
            vec![1000, 2000, 4000, 8000, 16000, 30000, 30000]
        );
        assert_eq!(reconnect_delay_ms(u32::MAX), 30_000);
    }

    #[test]
    fn x_grok_sender_rejects_changed_query_and_cross_surface_collision() {
        let current = Some("https://x.com/i/grok?conversation=12345678");
        let valid = |url| browser_sender_matches(
            "grok", 17, Some("x:12345678"), 17, Some(0),
            Some("https://x.com/i/grok"), url,
        );
        assert!(valid(current));
        assert!(!valid(Some("https://x.com/i/grok?conversation=87654321")));
        assert!(!valid(Some("https://grok.com/c/12345678")));
        assert!(!valid(Some("https://grok.com/c/x:12345678")));
        assert!(!valid(Some("https://grok.com/c/x-12345678")));
        assert!(!valid(Some("https://x.com/i/grok?conversation=12345678&conversation=87654321")));
        assert!(!valid(Some("https://x.com/home?conversation=12345678")));
    }

    #[test]
    fn x_grok_response_slot_survives_rerender_and_suppresses_ambiguous_repeats() {
        let (user, response) = x_grok_turn("read files/proof.md").unwrap();
        assert_eq!(x_grok_turn("\nread files/proof.md  "), Some((user.clone(), response.clone())));
        let turn = DeliveryTurn { conversation: "x-1234".into(), user, assistant: response.clone() };
        let (next_user, next_response) = x_grok_turn("read files/another.md").unwrap();
        assert!(!turn.is_current(Some(&DeliveryTurn {
            conversation: turn.conversation.clone(), user: next_user, assistant: next_response,
        })));
        assert_eq!(x_grok_turn("  "), None);
        assert_ne!(request_key("grok", "x-1234", &response, 0), request_key("grok", "x-5678", &response, 0));
    }

    #[test]
    fn new_providers_require_the_registered_tab_conversation_and_provider() {
        for (provider, home, current) in [
            ("qwen", "https://chat.qwen.ai/", "https://chat.qwen.ai/c/test-1234"),
            ("kimi", "https://www.kimi.ai/en", "https://www.kimi.ai/chat/test-1234"),
        ] {
            let check = |tab, frame, url| browser_sender_matches(provider, 71, Some("test-1234"), tab, frame, Some(home), Some(url));
            assert!(check(71, Some(0), current));
            assert!(!check(72, Some(0), current));
            assert!(!check(71, Some(1), current));
            assert!(!check(71, Some(0), home));
            assert!(!check(71, Some(0), "https://chatgpt.com/c/test-1234"));
        }
        assert_ne!(conservative_turn("kimi", "same task"), conservative_turn("grok-x", "same task"));
    }

    #[test]
    fn retries_share_key_but_distinct_responses_and_slots_do_not() {
        let key = request_key("chatgpt", "conversation", "response", 0);
        assert_eq!(key, request_key("chatgpt", "conversation", "response", 0));
        assert_ne!(key, request_key("chatgpt", "conversation", "response2", 0));
        assert_ne!(key, request_key("chatgpt", "conversation", "response", 1));
        assert_ne!(key, request_key("claude", "conversation", "response", 0));
    }

    #[test]
    fn delivery_rejects_new_user_turn_response_route_and_missing_dom() {
        let original = DeliveryTurn {
            conversation: "c".into(),
            user: "u".into(),
            assistant: "a".into(),
        };
        assert!(original.is_current(Some(&original)));
        assert!(!original.is_current(None));
        for changed in [
            DeliveryTurn {
                conversation: "c2".into(),
                ..original.clone()
            },
            DeliveryTurn {
                user: "u2".into(),
                ..original.clone()
            },
            DeliveryTurn {
                assistant: "a2".into(),
                ..original.clone()
            },
        ] {
            assert!(!original.is_current(Some(&changed)));
        }
    }
}
