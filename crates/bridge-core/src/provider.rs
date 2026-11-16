use serde::{Deserialize, Serialize};
use std::fmt;

use crate::CoreError;

/// Model websites supported by the single extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    ChatGpt,
    Claude,
    DeepSeek,
    Gemini,
    Grok,
    Qwen,
    Kimi,
}

/// The fixed agent identity that owns a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Agent {
    ChatGpt,
    Claude,
    DeepSeek,
    Gemini,
    Grok,
    Qwen,
    Kimi,
}

impl Provider {
    pub const ALL: [Provider; 7] = [
        Provider::ChatGpt,
        Provider::Claude,
        Provider::DeepSeek,
        Provider::Gemini,
        Provider::Grok,
        Provider::Qwen,
        Provider::Kimi,
    ];

    pub fn agent(self) -> Agent {
        match self {
            Provider::ChatGpt => Agent::ChatGpt,
            Provider::Claude => Agent::Claude,
            Provider::DeepSeek => Agent::DeepSeek,
            Provider::Gemini => Agent::Gemini,
            Provider::Grok => Agent::Grok,
            Provider::Qwen => Agent::Qwen,
            Provider::Kimi => Agent::Kimi,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Provider::ChatGpt => "chatgpt",
            Provider::Claude => "claude",
            Provider::DeepSeek => "deepseek",
            Provider::Gemini => "gemini",
            Provider::Grok => "grok",
            Provider::Qwen => "qwen",
            Provider::Kimi => "kimi",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Provider::ChatGpt => "ChatGPT",
            Provider::Claude => "Claude",
            Provider::DeepSeek => "DeepSeek",
            Provider::Gemini => "Gemini",
            Provider::Grok => "Grok",
            Provider::Qwen => "Qwen",
            Provider::Kimi => "Kimi",
        }
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatgpt" | "chat_gpt" | "chat-gpt" | "openai" => Some(Provider::ChatGpt),
            "claude" | "anthropic" => Some(Provider::Claude),
            "deepseek" | "deep_seek" => Some(Provider::DeepSeek),
            "gemini" | "google" => Some(Provider::Gemini),
            "grok" | "xai" => Some(Provider::Grok),
            "qwen" => Some(Provider::Qwen),
            "kimi" => Some(Provider::Kimi),
            _ => None,
        }
    }

    /// Canonical URLs that the extension may attach to. No wildcard subdomains:
    /// a lookalike host must never inherit a model's identity.
    pub fn allowed(&self, url: &str) -> bool {
        let Some((host, path)) = host_and_path(url) else {
            return false;
        };
        match self {
            Provider::ChatGpt => host_is(host, "chatgpt.com") || host_is(host, "chat.openai.com"),
            Provider::Claude => host_is(host, "claude.ai"),
            Provider::DeepSeek => host_is(host, "chat.deepseek.com"),
            Provider::Gemini => host_is(host, "gemini.google.com"),
            Provider::Grok => {
                host_is(host, "grok.com") || (host_is(host, "x.com") && is_x_grok_path(path))
            }
            Provider::Qwen => host_is(host, "chat.qwen.ai"),
            Provider::Kimi => host_is(host, "www.kimi.ai") || host_is(host, "kimi.ai"),
        }
    }

    /// Identify a provider from a page URL. `x.com` is only Grok on its Grok path.
    pub fn from_url(url: &str) -> Option<Self> {
        let (host, path) = host_and_path(url)?;
        if host_is(host, "chatgpt.com") || host_is(host, "chat.openai.com") {
            Some(Provider::ChatGpt)
        } else if host_is(host, "claude.ai") {
            Some(Provider::Claude)
        } else if host_is(host, "chat.deepseek.com") {
            Some(Provider::DeepSeek)
        } else if host_is(host, "gemini.google.com") {
            Some(Provider::Gemini)
        } else if host_is(host, "chat.qwen.ai") {
            Some(Provider::Qwen)
        } else if host_is(host, "www.kimi.ai") || host_is(host, "kimi.ai") {
            Some(Provider::Kimi)
        } else if host_is(host, "grok.com")
            || (host_is(host, "x.com") && is_x_grok_path(path))
        {
            Some(Provider::Grok)
        } else {
            None
        }
    }

    /// Validate that a registration's claimed provider owns this URL.
    pub fn validate_url(self, url: &str) -> Result<(), CoreError> {
        if self.allowed(url) {
            Ok(())
        } else {
            Err(CoreError::ProviderMismatch {
                provider: self.display().to_string(),
                url: url.to_string(),
            })
        }
    }

    /// Conversation identifier from the browser URL, used for session binding.
    /// X conversations are namespaced so they cannot collide with grok.com IDs.
    pub fn conversation_id(self, url: &str) -> Option<String> {
        self.validate_url(url).ok()?;
        let (host, path) = host_and_path(url)?;
        if self == Provider::Grok && host_is(host, "x.com") {
            let query = url.split('#').next()?.split_once('?')?.1;
            let mut ids = query.split('&').filter_map(|field| {
                let (key, value) = field.split_once('=').unwrap_or((field, ""));
                (key == "conversation").then_some(value)
            });
            let id = ids.next()?;
            // Do not guess between duplicate IDs or decode arbitrary path text.
            if ids.next().is_some() || !valid_conversation_id(id) || id.len() > 198 {
                return None;
            }
            return Some(format!("x:{id}"));
        }
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let picked = match self {
            Provider::ChatGpt => segments
                .windows(2)
                .find(|pair| matches!(pair[0], "c" | "g"))
                .map(|pair| pair[1])
                .or_else(|| {
                    segments
                        .windows(2)
                        .find(|pair| pair[0] == "c")
                        .map(|pair| pair[1])
                }),
            Provider::Claude => segments
                .windows(2)
                .find(|pair| pair[0] == "chat")
                .map(|pair| pair[1]),
            Provider::DeepSeek => segments
                .iter()
                .position(|part| *part == "s")
                .and_then(|index| segments.get(index + 1).copied()),
            Provider::Gemini => segments
                .windows(2)
                .find(|pair| pair[0] == "app")
                .map(|pair| pair[1]),
            Provider::Grok => segments
                .windows(2)
                .find(|pair| matches!(pair[0], "chat" | "c"))
                .map(|pair| pair[1]),
            Provider::Qwen => segments
                .windows(2)
                .find(|pair| pair[0] == "c" && pair[1] != "new-chat")
                .map(|pair| pair[1]),
            Provider::Kimi => segments
                .windows(2)
                .find(|pair| pair[0] == "chat" && pair[1] != "history")
                .map(|pair| pair[1]),
        }?;
        if !valid_conversation_id(picked) {
            return None;
        }
        Some(picked.to_string())
    }
}

fn is_x_grok_path(path: &str) -> bool {
    path == "/i/grok" || path.starts_with("/i/grok/")
}

fn valid_conversation_id(id: &str) -> bool {
    (4..=200).contains(&id.len())
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

impl Agent {
    pub const ALL: [Agent; 7] = [
        Agent::ChatGpt,
        Agent::Claude,
        Agent::DeepSeek,
        Agent::Gemini,
        Agent::Grok,
        Agent::Qwen,
        Agent::Kimi,
    ];

    /// Uppercase directory name under `AGENTS/`.
    pub fn dir(self) -> &'static str {
        match self {
            Agent::ChatGpt => "CHATGPT",
            Agent::Claude => "CLAUDE",
            Agent::DeepSeek => "DEEPSEEK",
            Agent::Gemini => "GEMINI",
            Agent::Grok => "GROK",
            Agent::Qwen => "QWEN",
            Agent::Kimi => "KIMI",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Agent::ChatGpt => "ChatGPT",
            Agent::Claude => "Claude",
            Agent::DeepSeek => "DeepSeek",
            Agent::Gemini => "Gemini",
            Agent::Grok => "Grok",
            Agent::Qwen => "Qwen",
            Agent::Kimi => "Kimi",
        }
    }

    pub fn provider(self) -> Provider {
        match self {
            Agent::ChatGpt => Provider::ChatGpt,
            Agent::Claude => Provider::Claude,
            Agent::DeepSeek => Provider::DeepSeek,
            Agent::Gemini => Provider::Gemini,
            Agent::Grok => Provider::Grok,
            Agent::Qwen => Provider::Qwen,
            Agent::Kimi => Provider::Kimi,
        }
    }

    pub fn from_dir(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "CHATGPT" => Some(Agent::ChatGpt),
            "CLAUDE" => Some(Agent::Claude),
            "DEEPSEEK" => Some(Agent::DeepSeek),
            "GEMINI" => Some(Agent::Gemini),
            "GROK" => Some(Agent::Grok),
            "QWEN" => Some(Agent::Qwen),
            "KIMI" => Some(Agent::Kimi),
            _ => None,
        }
    }

    pub fn workspace_relative(self) -> String {
        format!("AGENTS/{}", self.dir())
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display())
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.dir())
    }
}

impl std::str::FromStr for Provider {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_slug(s).ok_or_else(|| CoreError::UnsupportedWebsite(s.to_string()))
    }
}

impl std::str::FromStr for Agent {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_dir(s).ok_or_else(|| CoreError::UnsupportedWebsite(s.to_string()))
    }
}

fn host_and_path(url: &str) -> Option<(&str, &str)> {
    let rest = url.split_once("://")?.1;
    let split_at = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..split_at];
    if authority.contains('@') {
        return None;
    }
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(authority)
        .trim_end_matches('.');
    let path = if split_at < rest.len() {
        &rest[split_at..]
    } else {
        "/"
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    Some((host, path))
}

fn host_is(actual: &str, expected: &str) -> bool {
    actual.eq_ignore_ascii_case(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_canonical_hosts() {
        assert_eq!(
            Provider::from_url("https://chatgpt.com/c/abc-123"),
            Some(Provider::ChatGpt)
        );
        assert_eq!(
            Provider::from_url("https://claude.ai/chat/abc12345"),
            Some(Provider::Claude)
        );
        assert_eq!(
            Provider::from_url("https://chat.deepseek.com/a/chat/s/abc12345"),
            Some(Provider::DeepSeek)
        );
        assert_eq!(
            Provider::from_url("https://gemini.google.com/app/abc12345"),
            Some(Provider::Gemini)
        );
        assert_eq!(
            Provider::from_url("https://grok.com/chat/abc12345"),
            Some(Provider::Grok)
        );
        assert_eq!(
            Provider::from_url("https://x.com/i/grok?conversation=1"),
            Some(Provider::Grok)
        );
        assert_eq!(
            Provider::from_url("https://chatgpt.com.evil.example/c/x"),
            None
        );
        assert_eq!(Provider::from_url("https://evilchatgpt.com/c/x"), None);
        assert_eq!(Provider::from_url("https://x.com/home"), None);
    }

    #[test]
    fn rejects_provider_url_mismatch() {
        assert!(Provider::Claude
            .validate_url("https://chatgpt.com/")
            .is_err());
        assert!(Provider::Grok.validate_url("https://x.com/i/grok").is_ok());
    }

    #[test]
    fn extracts_conversation_ids() {
        assert_eq!(
            Provider::Grok.conversation_id(
                "https://grok.com/c/00000000-0000-4000-8000-000000000001?rid=another-response"
            ),
            Some("00000000-0000-4000-8000-000000000001".into())
        );
        assert_eq!(
            Provider::Grok.conversation_id("https://grok.com/chat/legacy-chat"),
            Some("legacy-chat".into())
        );
        assert_eq!(Provider::Grok.conversation_id("https://grok.com/"), None);
        assert_eq!(
            Provider::ChatGpt.conversation_id("https://chatgpt.com/c/68b1a2c3-1111-2222"),
            Some("68b1a2c3-1111-2222".into())
        );
        assert_eq!(
            Provider::DeepSeek.conversation_id("https://chat.deepseek.com/a/chat/s/1234-abcd"),
            Some("1234-abcd".into())
        );
        assert_eq!(
            Provider::Gemini.conversation_id("https://gemini.google.com/app"),
            None
        );
    }

    #[test]
    fn x_grok_conversations_are_bound_to_the_query_and_host() {
        assert_eq!(
            Provider::Grok.conversation_id("https://x.com/i/grok?conversation=1234567890123456789"),
            Some("x:1234567890123456789".into())
        );
        assert_eq!(
            Provider::Grok.conversation_id("https://x.com/i/grok?mode=fast&conversation=chat_1234#view"),
            Some("x:chat_1234".into())
        );
        for url in [
            "https://x.com/i/grok",
            "https://x.com/i/grok#?conversation=1234",
            "https://x.com/i/grok?conversation=",
            "https://x.com/i/grok?conversation=1234&conversation=5678",
            "https://x.com/i/grok?conversation=..%2Fother",
            "https://x.com/home?conversation=1234",
            "https://x.com/i/grokish?conversation=1234",
            "https://x.com.evil.example/i/grok?conversation=1234",
            "https://claude.ai/chat/1234",
        ] {
            assert_eq!(Provider::Grok.conversation_id(url), None, "{url}");
        }
        assert_eq!(Provider::from_url("https://x.com/i/grokish"), None);
        assert!(!Provider::Grok.allowed("https://x.com/i/grokish"));
    }

    #[test]
    fn agents_map_to_directories() {
        for provider in Provider::ALL {
            assert_eq!(
                Agent::from_dir(provider.agent().dir()),
                Some(provider.agent())
            );
        }
    }

    #[test]
    fn qwen_and_kimi_use_canonical_hosts_and_conversation_routes() {
        for (provider, url, id) in [
            (Provider::Qwen, "https://chat.qwen.ai/c/00000000-0000-4000-8000-000000000002", "00000000-0000-4000-8000-000000000002"),
            (Provider::Kimi, "https://www.kimi.ai/chat/00000000-0000-4000-8000-000000000003?chat_enter_method=home", "00000000-0000-4000-8000-000000000003"),
            (Provider::Kimi, "https://kimi.ai/en/chat/test-1234", "test-1234"),
        ] {
            assert_eq!(Provider::from_url(url), Some(provider));
            assert_eq!(provider.conversation_id(url).as_deref(), Some(id));
            assert_eq!(Agent::from_dir(provider.agent().dir()), Some(provider.agent()));
            assert_eq!(Provider::from_slug(provider.slug()), Some(provider));
            for other in Provider::ALL.into_iter().filter(|p| *p != provider) {
                assert!(other.validate_url(url).is_err());
                assert_eq!(other.conversation_id(url), None);
            }
        }
        for url in ["https://chat.qwen.ai.evil.example/c/1234", "https://evil.kimi.ai/chat/1234", "https://www.kimi.ai@evil.example/chat/1234"] {
            assert_eq!(Provider::from_url(url), None);
        }
        assert_eq!(Provider::Qwen.conversation_id("https://chat.qwen.ai/"), None);
        assert_eq!(Provider::Qwen.conversation_id("https://chat.qwen.ai/c/new-chat"), None);
        assert_eq!(Provider::Kimi.conversation_id("https://www.kimi.ai/en"), None);
        assert_eq!(Provider::Kimi.conversation_id("https://www.kimi.ai/chat/history"), None);
    }
}
