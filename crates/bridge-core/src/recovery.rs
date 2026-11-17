//! Decisions shared by the browser adapter and native tests. Website notices
//! must come from visible UI outside the conversation, never assistant prose.
use serde::{Deserialize, Serialize};
use crate::execution::DeliveryTurn;

/// Formatting repairs do not create tasks or refill the host's call allowance.
/// Persist the reservation together with the queued notice before submission.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FormatRepairBudget {
    task: String,
    responses: Vec<String>,
}

impl FormatRepairBudget {
    pub fn reserve(&mut self, task: &str, response: &str) -> bool {
        if task.is_empty() || response.is_empty() { return false; }
        if self.task != task {
            self.task = task.into();
            self.responses.clear();
        }
        if self.responses.len() >= 2 || self.responses.iter().any(|id| id == response) {
            return false;
        }
        self.responses.push(response.into());
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingResultState {
    Current,
    WaitingForTranscript,
    Delivered,
    Stale,
}

/// Reconcile a saved result before considering another submission. A provider
/// may create a new user node while its text remains unrendered in a hidden tab.
/// A delayed acknowledgement requires the entire result, not the setup prefix.
pub fn pending_result_state(
    saved: &DeliveryTurn,
    current: Option<&DeliveryTurn>,
    latest_user: Option<(&str, &str)>,
    expected: &str,
    submission_uncertain: bool,
) -> PendingResultState {
    if current.is_some_and(|turn| turn.conversation != saved.conversation) {
        return PendingResultState::Stale;
    }
    let Some((id, text)) = latest_user else {
        return PendingResultState::WaitingForTranscript;
    };
    if id.is_empty() || text.trim().is_empty() {
        return PendingResultState::WaitingForTranscript;
    }
    if id != saved.user {
        let normalize = |s: &str| s.chars().filter(|c| !c.is_whitespace() && *c != '`').collect::<String>();
        if submission_uncertain && !expected.trim().is_empty()
            && normalize(expected) == normalize(text) {
            return PendingResultState::Delivered;
        }
        return PendingResultState::Stale;
    }
    match current {
        None => PendingResultState::WaitingForTranscript,
        Some(turn) if saved == turn => PendingResultState::Current,
        Some(_) => PendingResultState::Stale,
    }
}

pub fn context_changed(saved_workspace:&str,saved_hash:&str,workspace:&str,hash:&str)->bool {
    !saved_workspace.is_empty() && (saved_workspace!=workspace || (!saved_hash.is_empty() && saved_hash!=hash))
}

/// Long submitted messages may be collapsed by the visible UI. Require both
/// a new website-issued turn ID and a matching normalized opening fragment.
pub fn submitted_message_matches(before:Option<&str>,after:&str,expected:&str,observed:&str)->bool {
    if after.is_empty() || before==Some(after) {return false;}
    // ChatGPT renders inline-code backticks even inside a user message. This
    // acknowledgement comparison is separate from tool parsing/authorization.
    let normalize=|s:&str|s.chars().filter(|c|!c.is_whitespace() && *c!='`').take(512).collect::<String>();
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
    fn format_repair_is_bounded_persistent_and_does_not_repeat_a_response() {
        let mut budget = FormatRepairBudget::default();
        assert!(!budget.reserve("", "reply"));
        assert!(!budget.reserve("task", ""));
        assert!(budget.reserve("task", "reply1"));
        assert!(!budget.reserve("task", "reply1"));
        let mut restored: FormatRepairBudget = serde_json::from_str(&serde_json::to_string(&budget).unwrap()).unwrap();
        assert!(!restored.reserve("task", "reply1"));
        assert!(restored.reserve("task", "reply2"));
        assert!(!restored.reserve("task", "reply3"));
        assert!(restored.reserve("explicit-new-task", "reply4"));
    }
    #[test]
    fn pending_result_survives_delayed_rendering_without_resubmission() {
        let saved = DeliveryTurn { conversation: "chat".into(), user: "task".into(), assistant: "call".into() };
        let expected = "[[BRIDGE_RESULT]]\n{\"output\":\"verified bytes\"}\n[[/BRIDGE_RESULT]]";
        let check = |current, latest, uncertain| pending_result_state(&saved, current, latest, expected, uncertain);
        assert_eq!(check(None, None, true), PendingResultState::WaitingForTranscript);
        assert_eq!(check(None, Some(("result", "")), true), PendingResultState::WaitingForTranscript);
        assert_eq!(check(Some(&saved), Some(("result", "  \n")), true), PendingResultState::WaitingForTranscript);
        assert_eq!(check(None, Some(("result", expected)), true), PendingResultState::Delivered);
        let next = DeliveryTurn { user: "result".into(), assistant: "next".into(), ..saved.clone() };
        let normalized = expected.replace('\n', "");
        assert_eq!(check(Some(&next), Some(("result", &normalized)), true), PendingResultState::Delivered);
        assert_eq!(check(Some(&saved), Some(("task", "original task")), true), PendingResultState::Current);
        // An uncertain Current result still cannot be sent by DeliveryRetry.
        assert!(!DeliveryRetry { submission_uncertain: true, ..Default::default() }.ready(99_000.0));
    }

    #[test]
    fn pending_result_rejects_changed_turn_context_and_partial_matches() {
        let saved = DeliveryTurn { conversation: "chat".into(), user: "task".into(), assistant: "call".into() };
        let expected = format!("[[BRIDGE_RESULT]]{}correct tail", "shared data ".repeat(30));
        let check = |current, latest, uncertain| pending_result_state(&saved, current, latest, &expected, uncertain);
        assert_eq!(check(None, Some(("new-task", "do something else")), true), PendingResultState::Stale);
        assert_eq!(check(None, Some(("result", &expected)), false), PendingResultState::Stale);
        let different = expected.replace("correct tail", "different result");
        assert_eq!(check(None, Some(("result", &different)), true), PendingResultState::Stale);
        assert_eq!(check(None, Some(("result", &expected[..200])), true), PendingResultState::Stale);
        let other_chat = DeliveryTurn { conversation: "other".into(), ..saved.clone() };
        assert_eq!(check(Some(&other_chat), Some(("result", &expected)), true), PendingResultState::Stale);
        let regenerated = DeliveryTurn { assistant: "regenerated".into(), ..saved.clone() };
        assert_eq!(check(Some(&regenerated), Some(("task", "original task")), true), PendingResultState::Stale);
    }
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
        assert!(submitted_message_matches(Some("old"),"new","Workspace is `/Users/test/project`. Read `files/input.txt`.","Workspace is /Users/test/project. Read files/input.txt."));
        assert!(!submitted_message_matches(Some("old"),"new","Workspace is `/Users/test/project`.","Workspace is /Users/test/different."));
    }
    #[test]
    fn delivery_confirmation_does_not_search_past_accessibility_announcements() {
        let expected = "Local bridge execution result. This is tool output, not a user claim.\n\n[[BRIDGE_RESULT]]\n{\"status\":\"ok\",\"tool\":\"read_file\"}\n[[/BRIDGE_RESULT]]";
        let wrapper = format!("You said Local bridge execution result. This is tool output… {expected}");
        assert!(!submitted_message_matches(Some("before"), "after", expected, &wrapper));
        // The provider adapter must select actual prompt lines, keeping this
        // comparison strict rather than accepting an arbitrary matching suffix.
        assert!(submitted_message_matches(Some("before"), "after", expected, expected));
        assert!(!submitted_message_matches(Some("after"), "after", expected, expected));
        assert!(!submitted_message_matches(Some("before"), "after", expected, ""));
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
