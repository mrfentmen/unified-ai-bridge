use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use bridge_core::provider::Agent;
use bridge_core::workspace::{PermissionMode, Profile, WorkspaceInfo};
use sha2::{Digest, Sha256};

pub const MARKER_FILE: &str = ".unified-ai-bridge-workspace.json";
pub const PROFILE_MARKER: &str = ".bridge/workspace.json";

pub async fn pick_folder() -> Result<PathBuf> {
    let output = tokio::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "activate",
            "-e",
            "POSIX path of (choose folder with prompt \"Choose a Unified AI Bridge workspace\")",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("open native folder picker")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        if detail.contains("(-128)") {
            bail!("folder selection cancelled");
        }
        bail!("native folder picker failed: {}", detail.trim());
    }
    let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    guard_project_root(&path)?;
    Ok(path.canonicalize()?)
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub audit_file: PathBuf,
    pub checkpoint_dir: PathBuf,
    pub lock_dir: PathBuf,
    pub profile: Profile,
    pub read_only: bool,
    pub permission_mode: PermissionMode,
}

#[derive(Serialize, Deserialize)]
struct ProfileMarker {
    profile: Profile,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    pub root: String,
    pub marker_written: bool,
    pub root_was_nonempty: bool,
    pub created_directories: usize,
    pub created_files: usize,
    pub existing_files_skipped: usize,
}

impl Workspace {
    pub fn id(&self) -> String {
        hex::encode(Sha256::digest(self.root.to_string_lossy().as_bytes()))[..24].to_string()
    }

    pub fn info(&self, label: String) -> WorkspaceInfo {
        let now = Utc::now().to_rfc3339();
        WorkspaceInfo {
            id: self.id(),
            label,
            root: self.root.display().to_string(),
            profile: self.profile,
            created_at: now.clone(),
            last_used_at: now,
            read_only: self.read_only,
            permission_mode: self.permission_mode,
        }
    }

    /// Add only missing directories and a profile marker. Never replace a
    /// user's file, follow a skeleton symlink, or silently change profiles.
    pub fn initialize_profile(root: &Path, profile: Profile) -> Result<Self> {
        guard_project_root(root)?;
        fs::create_dir_all(root)?;
        let root = root.canonicalize()?;
        guard_project_root(&root)?;
        if root.join(PROFILE_MARKER).exists() || root.join(MARKER_FILE).exists() {
            let existing = Self::load(&root)?;
            if existing.profile != profile {
                bail!("profile_mismatch: folder already uses {}; choose that profile or a different folder before adding structure", existing.profile.name());
            }
        }
        if profile == Profile::Research {
            let (workspace, _) = Self::initialize(&root)?;
            let metadata = root.join(".bridge");
            if fs::symlink_metadata(&metadata).is_ok_and(|meta| meta.file_type().is_symlink()) {
                bail!("workspace metadata must not be a symlink");
            }
            fs::create_dir_all(&metadata)?;
            if !root.join(PROFILE_MARKER).exists() {
                write_new_file(
                    &root.join(PROFILE_MARKER),
                    &serde_json::to_vec_pretty(&ProfileMarker { profile })?,
                )?;
            }
            return Ok(workspace);
        }
        let names: &[&str] = if profile == Profile::Code {
            &[
                "src",
                "docs",
                "tests",
                ".bridge",
                ".bridge/checkpoints",
                ".bridge/locks",
            ]
        } else {
            &[
                "notes",
                "files",
                "output",
                ".bridge",
                ".bridge/checkpoints",
                ".bridge/locks",
            ]
        };
        for name in names {
            let path = root.join(name);
            if path.exists() {
                let meta = fs::symlink_metadata(&path)?;
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    bail!("skeleton entry {} is not a plain directory", path.display());
                }
            } else {
                fs::create_dir(&path)?;
            }
        }
        let marker = root.join(PROFILE_MARKER);
        if !marker.exists() {
            write_new_file(
                &marker,
                &serde_json::to_vec_pretty(&ProfileMarker { profile })?,
            )?;
        }
        Self::load(&root)
    }

    pub fn check_writable(&self) -> Result<()> {
        tempfile::NamedTempFile::new_in(&self.root)
            .with_context(|| format!("workspace is not writable: {}", self.root.display()))?;
        Ok(())
    }

    /// Initialize a user-selected project directory. Existing files are never
    /// overwritten; only missing bridge structure is added.
    pub fn initialize(root: &Path) -> Result<(Self, InitReport)> {
        guard_project_root(root)?;
        fs::create_dir_all(root)
            .with_context(|| format!("create project root {}", root.display()))?;
        let root = root
            .canonicalize()
            .with_context(|| format!("canonicalize project root {}", root.display()))?;
        guard_project_root(&root)?;

        let root_was_nonempty = fs::read_dir(&root)?.next().is_some();
        let marker_path = root.join(MARKER_FILE);
        let marker_written = !marker_path.exists();

        let mut created_directories = 0usize;
        for relative in directory_layout() {
            let path = root.join(&relative);
            if !path.exists() {
                fs::create_dir_all(&path)
                    .with_context(|| format!("create workspace directory {}", path.display()))?;
                created_directories += 1;
            }
        }

        let mut created_files = 0usize;
        let mut existing_files_skipped = 0usize;
        for (relative, content) in initial_files() {
            let path = root.join(&relative);
            if path.exists() {
                existing_files_skipped += 1;
                continue;
            }
            write_new_file(&path, content.as_bytes())
                .with_context(|| format!("create initial file {}", path.display()))?;
            created_files += 1;
        }

        if marker_written {
            let marker = serde_json::json!({
                "application": "unified-ai-bridge",
                "workspace_version": 1,
                "initialized_at": Utc::now().to_rfc3339(),
                "note": "This directory is managed by Unified AI Bridge. Original datasets under SHARED/datasets are immutable to agents."
            });
            write_new_file(
                &marker_path,
                serde_json::to_string_pretty(&marker)?.as_bytes(),
            )?;
        }

        let workspace = Self::from_canonical_root(root);
        let report = InitReport {
            root: workspace.root.to_string_lossy().to_string(),
            marker_written,
            root_was_nonempty,
            created_directories,
            created_files,
            existing_files_skipped,
        };
        Ok((workspace, report))
    }

    /// Open an already initialized workspace. This is the only mode used by
    /// native sessions and MCP, so the bridge cannot silently adopt an
    /// arbitrary directory at runtime.
    pub fn load(root: &Path) -> Result<Self> {
        let canonical = root
            .canonicalize()
            .with_context(|| format!("project root {} does not exist", root.display()))?;
        let marker = canonical.join(MARKER_FILE);
        let profile_marker = canonical.join(PROFILE_MARKER);
        if profile_marker.exists() {
            let meta = fs::symlink_metadata(canonical.join(".bridge"))?;
            if meta.file_type().is_symlink()
                || fs::symlink_metadata(&profile_marker)?
                    .file_type()
                    .is_symlink()
            {
                bail!("workspace metadata must not be a symlink");
            }
            let profile: ProfileMarker = serde_json::from_slice(&fs::read(profile_marker)?)?;
            let mut workspace = Self::from_canonical_root(canonical);
            workspace.profile = profile.profile;
            if profile.profile != Profile::Research {
                workspace.audit_file = workspace.root.join(".bridge/audit.jsonl");
                workspace.checkpoint_dir = workspace.root.join(".bridge/checkpoints");
                workspace.lock_dir = workspace.root.join(".bridge/locks");
            }
            return Ok(workspace);
        }
        if !marker.is_file() {
            bail!(
                "{} is not an initialized Unified AI Bridge workspace (missing {MARKER_FILE}); run setup",
                canonical.display()
            );
        }
        Ok(Self::from_canonical_root(canonical))
    }

    fn from_canonical_root(root: PathBuf) -> Self {
        let checkpoint_dir = root.join("SYSTEM/checkpoints");
        let lock_dir = checkpoint_dir.join(".locks");
        let audit_file = root.join("SYSTEM/audit/audit.jsonl");
        Self {
            root,
            audit_file,
            checkpoint_dir,
            lock_dir,
            profile: Profile::Research,
            read_only: false,
            permission_mode: PermissionMode::Standard,
        }
    }

    pub fn project_name(&self) -> String {
        self.root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Archaeology")
            .to_string()
    }
}

fn guard_project_root(root: &Path) -> Result<()> {
    if !root.is_absolute() {
        bail!("project root must be an absolute path");
    }
    if root.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("project root must not contain ..");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let home_canon = home.as_ref().and_then(|path| path.canonicalize().ok());
    let resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if resolved == Path::new("/") {
        bail!("refusing to use / as the project root");
    }
    if let Some(home) = &home_canon {
        if &resolved == home {
            bail!("refusing to use the home directory itself as the project root");
        }
        if home.starts_with(&resolved) {
            bail!("refusing to use an ancestor of the home directory as the project root");
        }
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("open {} with create_new", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn directory_layout() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = vec![
        "PROJECT".into(),
        "SHARED/references".into(),
        "SHARED/datasets".into(),
        "SHARED/documentation".into(),
        "SHARED/figures".into(),
        "COLLABORATION/proposals".into(),
        "COLLABORATION/peer_reviews".into(),
        "COLLABORATION/disagreements".into(),
        "COLLABORATION/accepted_findings".into(),
        "VERIFICATION/tests".into(),
        "VERIFICATION/evidence".into(),
        "VERIFICATION/reports".into(),
        "SYSTEM/logs".into(),
        "SYSTEM/sessions".into(),
        "SYSTEM/checkpoints".into(),
        "SYSTEM/audit".into(),
    ];
    for agent in Agent::ALL {
        let base = PathBuf::from("AGENTS").join(agent.dir());
        dirs.push(base.clone());
        for sub in ["notes", "findings", "experiments", "reports", "logs"] {
            dirs.push(base.join(sub));
        }
    }
    dirs
}

fn initial_files() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    files.push((
        PathBuf::from("PROJECT/PROJECT_CONTEXT.md"),
        r#"# Project context

This file describes the archaeology project that the independent agents are investigating.

## Status

No project context has been supplied yet. Replace the placeholders below with verified information. Do not treat this template as research evidence.

## Research question

_To be defined by the project owner._

## Materials and their provenance

_List approved materials, collection methods, and custodial information._

## Constraints

_List legal, ethical, conservation, and data-handling constraints._
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("PROJECT/RESEARCH_OBJECTIVES.md"),
        r#"# Research objectives

## Primary objective

_To be defined._

## Secondary objectives

_To be defined._

## Out of scope

_To be defined._

## Completion criteria

An objective is complete only when its evidence, analysis, limitations, and verification are recorded in the workspace.
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("PROJECT/RESEARCH_STATUS.md"),
        r#"# Research status

## Current state

No investigations have been recorded. This file is not evidence of completed research.

## Open questions

- _None recorded yet._

## Recent changes

- Workspace initialized.
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("PROJECT/DATASET_MANIFEST.md"),
        r#"# Dataset manifest

Original measurement datasets belong in `SHARED/datasets/` and are immutable to agents.
Their machine-readable integrity manifest is `SHARED/datasets/DATA_MANIFEST.csv`.

## Entries

_No datasets registered yet. Add entries only by explicit user action, with provenance and SHA-256 hashes._
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("SHARED/datasets/DATA_MANIFEST.csv"),
        "path,sha256,bytes,description\n".to_string(),
    ));
    files.push((
        PathBuf::from("SHARED/datasets/README.md"),
        r#"# Original datasets

This directory is read-only to every agent. Store original measurement data here; derived tables, cleaned copies, and analysis outputs belong in an agent's own workspace or `SHARED/figures/`.

Record integrity hashes in `DATA_MANIFEST.csv`. Use `verify_file_integrity` to check them.
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("SHARED/documentation/README.md"),
        "Shared documentation. Agent edits require explicit user approval.\n".to_string(),
    ));
    files.push((
        PathBuf::from("COLLABORATION/README.md"),
        r#"# Collaboration

- `proposals/` — collaboration proposals with attribution.
- `peer_reviews/` — reviews of published findings.
- `disagreements/` — preserved competing explanations.
- `accepted_findings/` — protected; editing requires approval.

Agents remain independent. Agreement is not proof.
"#
        .to_string(),
    ));
    files.push((
        PathBuf::from("VERIFICATION/README.md"),
        "Verification artifacts live here. Writes require approval.\n".to_string(),
    ));
    for agent in Agent::ALL {
        files.push((
            PathBuf::from("AGENTS").join(agent.dir()).join("README.md"),
            format!(
                "# {name} workspace\n\nThis is the permanent workspace for **{name}** ({dir}).\n\n\
- `notes/` — working notes\n- `findings/` — published findings readable by the other agents\n- `experiments/` — methods and run records\n- `reports/` — agent-authored reports\n- `logs/` — local notes about tool activity\n\n\
Only this agent may write here. Other agents may read `findings/` only. Never fabricate data, and distinguish observation from interpretation.\n",
                name = agent.display(),
                dir = agent.dir()
            ),
        ));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn initializes_structure_without_overwriting() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("PROJECT_CONTEXT.md"), "user file").unwrap();

        let (workspace, report) = Workspace::initialize(&root).unwrap();
        assert!(workspace.root.join("AGENTS/CHATGPT/findings").is_dir());
        assert!(workspace.root.join("SHARED/datasets/README.md").is_file());
        assert_eq!(report.created_directories, directory_layout().len());
        assert!(report.root_was_nonempty);

        let (_, second) = Workspace::initialize(&root).unwrap();
        assert_eq!(second.created_directories, 0);
        assert!(second.existing_files_skipped > 0);
        assert_eq!(
            fs::read_to_string(root.join("PROJECT_CONTEXT.md")).unwrap(),
            "user file"
        );
    }
}
