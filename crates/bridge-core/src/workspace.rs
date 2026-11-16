use crate::catalog::{self, Category, ToolSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    #[default]
    Research,
    General,
    Code,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::General => "general",
            Self::Code => "code",
        }
    }

    pub fn tools(self) -> impl Iterator<Item = &'static ToolSpec> {
        catalog::TOOLS.iter().filter(move |tool| self.allows(tool))
    }

    pub fn allows(self, tool: &ToolSpec) -> bool {
        self == Self::Research
            || !matches!(tool.category, Category::Research | Category::Collaboration)
            || matches!(tool.name, "calculate_sha256" | "verify_file_integrity")
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Strict,
    #[default]
    Standard,
    Trusted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: String,
    pub label: String,
    pub root: String,
    pub profile: Profile,
    pub created_at: String,
    pub last_used_at: String,
    pub read_only: bool,
    pub permission_mode: PermissionMode,
}

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub profile: Profile,
    pub tools: Vec<&'static ToolSpec>,
    pub permissions: PermissionMode,
    pub root: String,
    pub read_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn general_and_code_hide_research_tools_but_keep_integrity_tools() {
        for profile in [Profile::General, Profile::Code] {
            let names: Vec<_> = profile.tools().map(|tool| tool.name).collect();
            assert!(names.contains(&"write_document"));
            assert!(names.contains(&"calculate_sha256"));
            assert!(!names.contains(&"inspect_dataset"));
            assert!(!names.contains(&"submit_peer_review"));
        }
        assert_eq!(Profile::Research.tools().count(), catalog::TOOLS.len());
    }
}
