//! Decisions shared by the browser adapter and native tests. Website notices
//! must come from visible UI outside the conversation, never assistant prose.
use serde::{Deserialize, Serialize};

pub fn context_changed(saved_workspace:&str,saved_hash:&str,workspace:&str,hash:&str)->bool {
    !saved_workspace.is_empty() && (saved_workspace!=workspace || (!saved_hash.is_empty() && saved_hash!=hash))
}

/// Long submitted messages may be collapsed by the visible UI. Require both
/// a new website-issued turn ID and a matching normalized opening fragment.
pub fn submitted_message_matches(before:Option<&str>,after:&str,expected:&str,observed:&str)->bool {
    if after.is_empty() || before==Some(after) {return false;}
    let normalize=|s:&str|s.chars().filter(|c|!c.is_whitespace()).take(512).collect::<String>();
    let expected=normalize(expected);
    let observed=normalize(observed);
    let prefix:String=expected.chars().take(180).collect();
    !prefix.is_empty() && observed.starts_with(&prefix)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebsiteBlock {
    ContextFull,
    Quota,
}

impl WebsiteBlock {
    pub fn from_notice(text: &str) -> Option<Self> {
        let text = text.to_lowercase();
        // Quotas take priority: a new conversation must never be used to retry
        // a request the account cannot currently make.
        if [
            "free tier limit reached",
            "daily limit reached",
            "usage limit reached",
            "you've reached your message limit",
            "you’ve reached your message limit",
            "you have reached your message limit",
            "rate limit exceeded",
        ]
        .iter()
        .any(|needle| text.contains(needle))
        {
            Some(Self::Quota)
        } else if [
            "context window full",
            "maximum length for this conversation",
            "conversation is too long",
        ]
        .iter()
        .any(|needle| text.contains(needle))
        {
            Some(Self::ContextFull)
        } else {
            None
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::ContextFull => "This conversation is full. Local results are saved. Continue in a fresh chat with a task handoff; no completed write will be rerun automatically.",
            Self::Quota => "The website reports an account usage limit. Local results are saved. Wait for the provider's limit to reset; new chats do not reset quota.",
        }
    }
}

/// A submission with an unknown outcome must not be clicked repeatedly.
/// Retries are only permitted before any submission was attempted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryRetry {
    pub attempts: u32,
    pub next_attempt_ms: f64,
    pub submission_uncertain: bool,
}
impl DeliveryRetry {
    pub fn ready(&self, now_ms: f64) -> bool {
        !self.submission_uncertain && self.attempts < 3 && now_ms >= self.next_attempt_ms
    }
    pub fn failed(&mut self, now_ms: f64, submitted: bool) {
        self.attempts = self.attempts.saturating_add(1);
        self.submission_uncertain |= submitted;
        self.next_attempt_ms = now_ms + 5_000.0 * (1u32 << self.attempts.min(4)) as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_empty_fingerprint_does_not_erase_a_restored_task() {
        assert!(!context_changed("root","","root","known"));
        assert!(!context_changed("root","known","root","known"));
        assert!(context_changed("root","","different-root","known"));
        assert!(context_changed("root","old","root","new"));
    }
    #[test]
    fn send_confirmation_requires_a_new_matching_user_message() {
        assert!(submitted_message_matches(Some("old"),"new","Result\nactual bytes","Result actual bytes"));
        assert!(!submitted_message_matches(Some("old"),"old","Result","Result"));
        assert!(!submitted_message_matches(Some("old"),"new","Result","User changed their task"));
        assert!(!submitted_message_matches(None,"","Result","Result"));
        let long="real submitted text ".repeat(80);
        let shown=format!("{} … Show more",long.chars().take(240).collect::<String>());
        assert!(submitted_message_matches(None,"new",&long,&shown));
    }
    #[test]
    fn context_and_quota_have_distinct_recovery() {
        assert_eq!(
            WebsiteBlock::from_notice("Context window full\nNew chat"),
            Some(WebsiteBlock::ContextFull)
        );
        assert_eq!(
            WebsiteBlock::from_notice("Free tier limit reached"),
            Some(WebsiteBlock::Quota)
        );
        assert_eq!(
            WebsiteBlock::from_notice("Context window full; usage limit reached"),
            Some(WebsiteBlock::Quota)
        );
        assert_eq!(
            WebsiteBlock::from_notice("Your model has a large context window"),
            None
        );
    }
    #[test]
    fn failed_delivery_backs_off_and_uncertain_submission_never_auto_retries() {
        let mut retry = DeliveryRetry::default();
        assert!(retry.ready(0.0));
        retry.failed(0.0, false);
        assert!(!retry.ready(9_999.0));
        assert!(retry.ready(10_000.0));
        retry.failed(10_000.0, true);
        assert!(!retry.ready(1_000_000.0));
        let restored: DeliveryRetry =
            serde_json::from_str(&serde_json::to_string(&retry).unwrap()).unwrap();
        assert!(!restored.ready(2_000_000.0));
        let mut retry = DeliveryRetry::default();
        for _ in 0..3 {
            retry.failed(0.0, false);
        }
        assert!(!retry.ready(2_000_000.0));
    }
}
