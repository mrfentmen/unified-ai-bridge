use std::fs;
use std::path::{Component, Path, PathBuf};

use bridge_core::provider::Agent;
use bridge_core::workspace::{PermissionMode, Profile};

use crate::failure::ToolFailure;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existence {
    Any,
    MustExist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Approval(String),
    Deny(String),
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedPath {
    pub relative: PathBuf,
    pub absolute: PathBuf,
    pub exists: bool,
    pub is_dir: bool,
}

impl ResolvedPath {
    pub fn display(&self) -> String {
        if self.relative.as_os_str().is_empty() {
            ".".into()
        } else {
            self.relative.to_string_lossy().to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    root: PathBuf,
    profile: Profile,
    read_only: bool,
    permission_mode: PermissionMode,
    exclusions: Vec<globset::GlobMatcher>,
}

impl Sandbox {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            root: workspace.root.clone(),
            profile: workspace.profile,
            read_only: workspace.read_only,
            permission_mode: workspace.permission_mode,
            exclusions: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_exclusions(mut self, patterns: &[String]) -> Self {
        self.exclusions = patterns
            .iter()
            .filter_map(|p| {
                globset::GlobBuilder::new(p)
                    .case_insensitive(true)
                    .literal_separator(true)
                    .build()
                    .ok()
                    .map(|g| g.compile_matcher())
            })
            .collect();
        self
    }

    fn excluded(&self, path: &Path) -> bool {
        self.exclusions.iter().any(|pattern| {
            pattern.is_match(path)
                || path
                    .components()
                    .any(|component| pattern.is_match(std::path::Path::new(component.as_os_str())))
        })
    }

    /// Resolve a model-supplied path. Absolute paths, `..`, NULs, and symlinks
    /// in any existing component are rejected before any I/O is attempted.
    pub fn resolve(&self, raw: &str, existence: Existence) -> Result<ResolvedPath, ToolFailure> {
        let relative = if Path::new(raw).is_absolute() && self.profile != Profile::Research {
            let relative = Path::new(raw)
                .strip_prefix(&self.root)
                .map_err(|_| escape_error(raw))?;
            normalize_relative(&relative.to_string_lossy())?
        } else {
            normalize_relative(raw)?
        };
        if self.excluded(&relative) {
            return Err(ToolFailure::PermissionDenied(
                "path is excluded by the user's private-file policy".into(),
            ));
        }
        let absolute = self.root.join(&relative);
        let mut current = self.root.clone();
        let mut exists = true;
        let mut is_dir = true;

        for component in relative.components() {
            let Component::Normal(part) = component else {
                continue;
            };
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() {
                        return Err(ToolFailure::PermissionDenied(format!(
                            "symlinks are not allowed in tool paths: {}",
                            current.display()
                        )));
                    }
                    if !current.starts_with(&self.root) {
                        return Err(escape_error(raw));
                    }
                    is_dir = meta.is_dir();
                    exists = true;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    exists = false;
                    is_dir = false;
                    break;
                }
                Err(err) => {
                    return Err(ToolFailure::from_io(
                        &format!("inspect path {}", current.display()),
                        err,
                    ))
                }
            }
        }

        if existence == Existence::MustExist && !exists {
            return Err(ToolFailure::NotFound(format!(
                "no such file or folder in the workspace: {}",
                if raw.trim().is_empty() { "." } else { raw }
            )));
        }
        Ok(ResolvedPath {
            relative,
            absolute,
            exists,
            is_dir,
        })
    }

    /// Default permission matrix. The agent identity comes from the native
    /// host's registered session, never from a tool argument.
    pub fn decide(&self, agent: Agent, access: Access, path: &ResolvedPath) -> Decision {
        let decision = self.decide_profile(agent, access, path);
        if access != Access::Read
            && self.permission_mode == PermissionMode::Strict
            && matches!(decision, Decision::Allow)
        {
            Decision::Approval("Strict mode requires approval for writes".into())
        } else {
            decision
        }
    }

    fn decide_profile(&self, agent: Agent, access: Access, path: &ResolvedPath) -> Decision {
        if self.excluded(&path.relative) {
            return Decision::Deny("path is excluded by the user's private-file policy".into());
        }
        let parts = path_parts(&path.relative);
        if self.read_only && access != Access::Read {
            return Decision::Deny("workspace is locked read-only".into());
        }
        if parts.is_empty() {
            return match access {
                Access::Read => Decision::Allow,
                Access::Write | Access::Remove => {
                    Decision::Deny("the workspace root is protected".into())
                }
            };
        }
        if self.profile != Profile::Research {
            if parts.iter().any(|part| part == ".bridge") {
                return Decision::Deny(
                    "bridge metadata, checkpoints and trash are protected".into(),
                );
            }
            if access != Access::Read && parts.iter().any(|part| part == ".git") {
                return Decision::Deny(
                    "Git internals are protected; use approved git commands".into(),
                );
            }
            return match access {
                Access::Read => Decision::Allow,
                Access::Remove => Decision::Approval(
                    "deleting files requires approval and moves them to trash".into(),
                ),
                Access::Write => Decision::Allow,
            };
        }
        if parts[0].starts_with('.') {
            return Decision::Deny("workspace metadata files are protected".into());
        }
        match parts[0].as_str() {
            "PROJECT" => match access {
                Access::Read => Decision::Allow,
                Access::Write => {
                    Decision::Approval("editing a shared project document requires approval".into())
                }
                Access::Remove => {
                    Decision::Deny("project documents cannot be deleted by agents".into())
                }
            },
            "AGENTS" => self.decide_agents(agent, access, &parts),
            "SHARED" => self.decide_shared(access, &parts),
            "COLLABORATION" => self.decide_collaboration(access, &parts),
            "VERIFICATION" => match access {
                Access::Read => Decision::Allow,
                Access::Write => {
                    Decision::Approval("writing verification artifacts requires approval".into())
                }
                Access::Remove => {
                    Decision::Approval("deleting verification artifacts requires approval".into())
                }
            },
            "SYSTEM" => match access {
                Access::Read if parts.get(1).map(String::as_str) == Some("logs") => Decision::Allow,
                _ => Decision::Deny(
                    "SYSTEM is managed by the bridge and is not available to agents".into(),
                ),
            },
            other => Decision::Deny(format!("unknown or unapproved workspace area `{other}`")),
        }
    }

    fn decide_agents(&self, agent: Agent, access: Access, parts: &[String]) -> Decision {
        if parts.len() < 2 {
            return match access {
                Access::Read => Decision::Allow,
                _ => {
                    Decision::Deny("agent workspaces cannot be created or renamed by agents".into())
                }
            };
        }
        let Some(owner) = Agent::from_dir(&parts[1]) else {
            return Decision::Deny(format!("unknown agent workspace `{}`", parts[1]));
        };
        if owner == agent {
            return match access {
                Access::Read | Access::Write => Decision::Allow,
                Access::Remove => Decision::Approval(
                    "deleting a file from your own workspace requires approval".into(),
                ),
            };
        }
        match access {
            Access::Read => {
                if parts.len() == 2 || path_is_dir_only(parts) {
                    Decision::Allow
                } else if parts.get(2).map(String::as_str) == Some("findings") {
                    Decision::Allow
                } else {
                    Decision::Deny(format!(
                        "only {}'s published findings are readable from another agent's workspace",
                        owner.display()
                    ))
                }
            }
            Access::Write => Decision::Deny(format!(
                "{} cannot write to {}'s workspace",
                agent.display(),
                owner.display()
            )),
            Access::Remove => Decision::Deny(format!(
                "{} cannot delete from {}'s workspace",
                agent.display(),
                owner.display()
            )),
        }
    }

    fn decide_shared(&self, access: Access, parts: &[String]) -> Decision {
        let area = parts.get(1).map(String::as_str).unwrap_or("");
        match access {
            Access::Read => Decision::Allow,
            Access::Write => match area {
                "datasets" => Decision::Deny(
                    "original measurement datasets are immutable; write derived data to your own workspace"
                        .into(),
                ),
                "references" => Decision::Deny("shared reference material is immutable".into()),
                "documentation" | "figures" | "" => Decision::Approval(
                    "editing shared documentation or figures requires approval".into(),
                ),
                _ => Decision::Deny(format!("unknown shared area `{area}`")),
            },
            Access::Remove => match area {
                "datasets" => Decision::Deny("original measurement datasets cannot be deleted".into()),
                "references" => Decision::Deny("shared reference material cannot be deleted".into()),
                _ => Decision::Approval("deleting shared material requires approval".into()),
            },
        }
    }

    fn decide_collaboration(&self, access: Access, parts: &[String]) -> Decision {
        let area = parts.get(1).map(String::as_str).unwrap_or("");
        match access {
            Access::Read => Decision::Allow,
            Access::Write => match area {
                "proposals" | "peer_reviews" | "disagreements" => Decision::Allow,
                "accepted_findings" => Decision::Approval(
                    "accepted findings are protected; editing requires approval".into(),
                ),
                "" => {
                    Decision::Approval("writing at the collaboration root requires approval".into())
                }
                _ => Decision::Deny(format!("unknown collaboration area `{area}`")),
            },
            Access::Remove => {
                Decision::Approval("deleting collaboration material requires approval".into())
            }
        }
    }
}

fn path_is_dir_only(parts: &[String]) -> bool {
    // Callers only use this for read decisions; a later stat determines whether
    // the path is a directory. Files outside findings are denied separately by
    // the tools, which can require is_dir.
    parts.len() <= 2
}

pub fn normalize_relative(raw: &str) -> Result<PathBuf, ToolFailure> {
    if raw.contains('\0') {
        return Err(ToolFailure::InvalidArguments(
            "path contains a NUL byte".into(),
        ));
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(PathBuf::new());
    }
    let input = Path::new(trimmed);
    if input.is_absolute() {
        return Err(ToolFailure::InvalidArguments(
            "paths must be relative to the workspace root".into(),
        ));
    }
    let mut output = PathBuf::new();
    for component in input.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.len() > 255 {
                    return Err(ToolFailure::InvalidArguments(
                        "path component is too long".into(),
                    ));
                }
                output.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ToolFailure::PermissionDenied(
                    "parent-directory traversal is not allowed".into(),
                ))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ToolFailure::InvalidArguments(
                    "absolute paths are not allowed".into(),
                ))
            }
        }
    }
    if output.as_os_str().is_empty() {
        return Ok(PathBuf::new());
    }
    Ok(output)
}

fn path_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

fn escape_error(raw: &str) -> ToolFailure {
    ToolFailure::PermissionDenied(format!("path escapes the workspace: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, Workspace, Sandbox) {
        let temp = tempdir().unwrap();
        let (workspace, _) = Workspace::initialize(&temp.path().join("p")).unwrap();
        let sandbox = Sandbox::new(&workspace);
        (temp, workspace, sandbox)
    }

    #[test]
    fn rejects_traversal_absolute_and_nul() {
        let (_temp, _workspace, sandbox) = setup();
        assert!(sandbox.resolve("../etc/passwd", Existence::Any).is_err());
        assert!(sandbox.resolve("/etc/passwd", Existence::Any).is_err());
        assert!(sandbox.resolve("a\0b", Existence::Any).is_err());
    }

    #[test]
    fn rejects_symlink_escape() {
        let (temp, workspace, sandbox) = setup();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, workspace.root.join("SHARED/datasets/link")).unwrap();
        let err = sandbox
            .resolve("SHARED/datasets/link/secret.txt", Existence::Any)
            .unwrap_err();
        assert!(matches!(err, ToolFailure::PermissionDenied(_)));
    }

    #[test]
    fn permission_matrix_matches_defaults() {
        let (_temp, _workspace, sandbox) = setup();
        let own = sandbox
            .resolve("AGENTS/CHATGPT/notes/a.md", Existence::Any)
            .unwrap();
        assert!(sandbox
            .decide(Agent::ChatGpt, Access::Write, &own)
            .is_allow());
        let other = sandbox
            .resolve("AGENTS/CLAUDE/notes/a.md", Existence::Any)
            .unwrap();
        assert!(!sandbox
            .decide(Agent::ChatGpt, Access::Write, &other)
            .is_allow());
        let finding = sandbox
            .resolve("AGENTS/CLAUDE/findings/a.md", Existence::Any)
            .unwrap();
        assert!(sandbox
            .decide(Agent::ChatGpt, Access::Read, &finding)
            .is_allow());
        let dataset = sandbox
            .resolve("SHARED/datasets/raw.csv", Existence::Any)
            .unwrap();
        assert!(matches!(
            sandbox.decide(Agent::ChatGpt, Access::Write, &dataset),
            Decision::Deny(_)
        ));
        let shared_doc = sandbox
            .resolve("SHARED/documentation/a.md", Existence::Any)
            .unwrap();
        assert!(matches!(
            sandbox.decide(Agent::ChatGpt, Access::Write, &shared_doc),
            Decision::Approval(_)
        ));
        let project = sandbox
            .resolve("PROJECT/RESEARCH_STATUS.md", Existence::Any)
            .unwrap();
        assert!(matches!(
            sandbox.decide(Agent::ChatGpt, Access::Write, &project),
            Decision::Approval(_)
        ));
    }
}
