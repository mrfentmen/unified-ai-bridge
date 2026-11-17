//! Browser file controls are capabilities minted by the content script, never
//! model-supplied URLs or local paths. Native authority supplies their scope.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub session_id: String,
    pub workspace_id: String,
    pub provider: String,
    pub tab_id: i64,
    pub conversation: String,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List,
    Inspect { handle: String },
    Download { file: GeneratedFile },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedFile {
    pub handle: String,
    pub name: String,
    pub response_id: String,
    pub source_key: String,
    pub method: DownloadMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadMethod {
    DirectLink,
    WebsiteButton,
}

impl GeneratedFile {
    pub fn valid(&self) -> bool {
        [(&self.handle, 32), (&self.source_key, 64)]
            .iter()
            .all(|(s, n)| s.len() == *n && s.bytes().all(|b| b.is_ascii_hexdigit()))
            && valid_name(&self.name)
            && !self.response_id.is_empty()
            && self.response_id.len() <= 200
            && !self.response_id.chars().any(char::is_control)
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.len() <= 240
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
}

pub fn source_key(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(serde_json::to_vec(parts).expect("strings")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_capabilities_are_strict_and_bound_to_the_source() {
        let file = GeneratedFile {
            handle: "a".repeat(32),
            name: "image.svg".into(),
            response_id: "response".into(),
            source_key: "b".repeat(64),
            method: DownloadMethod::WebsiteButton,
        };
        assert!(file.valid());
        for name in ["../secret", "/tmp/a", "a\\b", "a\n.svg", "..", ""] {
            assert!(!GeneratedFile {
                name: name.into(),
                ..file.clone()
            }
            .valid());
        }
        let mut value = serde_json::to_value(&file).unwrap();
        value["url"] = "https://example.invalid".into();
        assert!(serde_json::from_value::<GeneratedFile>(value).is_err());
        assert_ne!(source_key(&["chat1", "a"]), source_key(&["chat2", "a"]));
        assert_ne!(source_key(&["a", "bc"]), source_key(&["ab", "c"]));
    }
}
