pub mod artifacts;
pub mod collaboration;
pub mod commands;
pub mod desktop;
pub mod development;
pub mod file_extra;
pub mod filesystem;
pub mod recovery;
pub mod patches;
pub mod research;
pub mod system;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::Value;

use bridge_core::catalog;
use bridge_core::provider::Agent;

use crate::app::App;
use crate::failure::ToolFailure;
use crate::fsops::{self, AtomicWrite};
use crate::sandbox::{Access, Decision, Existence, ResolvedPath};
use crate::wire::ToolResultMessage;

pub const MAX_TOOL_CONTENT: usize = 60_000;
pub const MAX_READ_BYTES: u64 = 1_048_576;

pub struct ToolContext<'a> {
    pub app: &'a App,
    pub agent: Agent,
    pub session_id: &'a str,
}

pub struct ToolOutcome {
    pub summary: String,
    pub output: String,
    pub data: Option<Value>,
    pub path: Option<String>,
    pub kind: &'static str,
    pub failure: Option<(&'static str, String)>,
}

impl ToolOutcome {
    pub fn read(
        summary: impl Into<String>,
        output: impl Into<String>,
        path: Option<String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            output: truncate_output(output.into()),
            data: None,
            path,
            kind: "read",
            failure: None,
        }
    }

    pub fn write(
        summary: impl Into<String>,
        output: impl Into<String>,
        path: Option<String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            output: truncate_output(output.into()),
            data: None,
            path,
            kind: "write",
            failure: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

pub fn truncate_output(mut output: String) -> String {
    if output.len() > MAX_TOOL_CONTENT {
        let mut boundary = MAX_TOOL_CONTENT;
        while !output.is_char_boundary(boundary) {
            boundary -= 1;
        }
        output.truncate(boundary);
        output.push_str("\n\n[bridge: output truncated at 60,000 bytes]");
    }
    output
}

pub fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, ToolFailure> {
    let value = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args
    };
    serde_json::from_value(value)
        .map_err(|err| ToolFailure::InvalidArguments(format!("invalid tool arguments: {err}")))
}

impl<'a> ToolContext<'a> {
    pub fn new(app: &'a App, agent: Agent, session_id: &'a str) -> Self {
        Self {
            app,
            agent,
            session_id,
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.app.workspace.root
    }

    pub fn display(&self, path: &ResolvedPath) -> String {
        path.display()
    }

    /// Read authorization is synchronous because reads never require approval.
    /// Denials are still written to the audit chain.
    pub fn check_read(&self, tool: &str, path: &ResolvedPath) -> Result<(), ToolFailure> {
        match self.app.decide(self.agent, Access::Read, path) {
            Decision::Allow => Ok(()),
            Decision::Deny(reason) => {
                let _ = self.app.audit_denial(
                    self.agent,
                    self.session_id,
                    tool,
                    Some(&path.display()),
                    &reason,
                    "denied",
                );
                Err(ToolFailure::PermissionDenied(reason))
            }
            Decision::Approval(reason) => {
                let _ = self.app.audit_denial(
                    self.agent,
                    self.session_id,
                    tool,
                    Some(&path.display()),
                    &reason,
                    "denied",
                );
                Err(ToolFailure::PermissionDenied(
                    "read approval is not supported by the bridge".into(),
                ))
            }
        }
    }

    pub fn resolve_existing(&self, raw: &str) -> Result<ResolvedPath, ToolFailure> {
        self.app.sandbox().resolve(raw, Existence::MustExist)
    }

    pub fn resolve_any(&self, raw: &str) -> Result<ResolvedPath, ToolFailure> {
        self.app.sandbox().resolve(raw, Existence::Any)
    }

    pub fn read_text(
        &self,
        tool: &str,
        raw: &str,
        max_bytes: u64,
    ) -> Result<(ResolvedPath, String), ToolFailure> {
        let path = self.resolve_existing(raw)?;
        self.check_read(tool, &path)?;
        if path.is_dir {
            return Err(ToolFailure::InvalidArguments(format!(
                "{} is a folder, not a file",
                path.display()
            )));
        }
        let meta = fs::metadata(&path.absolute)
            .map_err(|err| ToolFailure::from_io("read file metadata", err))?;
        if meta.len() > max_bytes {
            return Err(ToolFailure::InvalidArguments(format!(
                "file is {} bytes; limit for this tool is {} bytes",
                meta.len(),
                max_bytes
            )));
        }
        use std::io::Read;
        let file =
            fs::File::open(&path.absolute).map_err(|err| ToolFailure::from_io("open file", err))?;
        if !file
            .metadata()
            .map_err(|err| ToolFailure::from_io("inspect open file", err))?
            .is_file()
        {
            return Err(ToolFailure::Unsupported(
                "only regular files can be read".into(),
            ));
        }
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|err| ToolFailure::from_io(&format!("read {}", path.display()), err))?;
        if bytes.len() as u64 > max_bytes {
            return Err(ToolFailure::InvalidArguments(
                "file grew beyond the read byte limit".into(),
            ));
        }
        let text = String::from_utf8(bytes).map_err(|_| {
            ToolFailure::Unsupported(format!("{} is not valid UTF-8 text", path.display()))
        })?;
        Ok((path, text))
    }

    pub async fn authorize_write(
        &self,
        tool: &str,
        path: &ResolvedPath,
    ) -> Result<bool, ToolFailure> {
        self.authorize_access(tool, Access::Write, path).await
    }

    pub async fn authorize_remove(
        &self,
        tool: &str,
        path: &ResolvedPath,
    ) -> Result<bool, ToolFailure> {
        self.authorize_access(tool, Access::Remove, path).await
    }

    async fn authorize_access(
        &self,
        tool: &str,
        access: Access,
        path: &ResolvedPath,
    ) -> Result<bool, ToolFailure> {
        match self.app.decide(self.agent, access, path) {
            Decision::Allow => Ok(false),
            Decision::Deny(reason) => {
                let _ = self.app.audit_denial(
                    self.agent,
                    self.session_id,
                    tool,
                    Some(&path.display()),
                    &reason,
                    "denied",
                );
                Err(ToolFailure::PermissionDenied(reason))
            }
            Decision::Approval(reason) => {
                self.app
                    .request_approval(
                        self.session_id,
                        self.agent,
                        tool,
                        &reason,
                        Some(path.display()),
                    )
                    .await?;
                Ok(true)
            }
        }
    }

    /// Revalidate after all approval waits, using the original workspace grant.
    pub fn recheck_access(
        &self,
        access: Access,
        path: &ResolvedPath,
        approved: bool,
    ) -> Result<(), ToolFailure> {
        match self.app.decide(self.agent, access, path) {
            Decision::Deny(reason) => Err(ToolFailure::PermissionDenied(reason)),
            Decision::Approval(_) if !approved => Err(ToolFailure::PermissionDenied(
                "write policy changed; request approval again".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Authorize and atomically create/replace a text file, checkpointing any
    /// previous contents, then append the consequential operation to the audit
    /// chain with before/after hashes.
    pub async fn write_text(
        &self,
        tool: &str,
        raw: &str,
        content: &str,
        create_only: bool,
    ) -> Result<(ResolvedPath, AtomicWrite), ToolFailure> {
        let initial = self.resolve_any(raw)?;
        self.check_read(tool, &initial)?;
        let before = recovery::snapshot(&initial)?;
        if create_only && before.is_some() {
            return Err(ToolFailure::Conflict("destination already exists".into()));
        }
        let expected = recovery::hash(&before);
        let (path, result, _) = recovery::mutate(
            self,
            tool,
            raw,
            expected.as_deref(),
            Some(content.as_bytes()),
            false,
        )
        .await?;
        Ok((path, result))
    }

    /// Exact-string edit performed while holding the path lock across read,
    /// checkpoint, and atomic replacement.
    pub async fn edit_text(
        &self,
        tool: &str,
        raw: &str,
        old_text: &str,
        new_text: &str,
        replace_all: bool,
    ) -> Result<(ResolvedPath, AtomicWrite), ToolFailure> {
        if old_text.is_empty() {
            return Err(ToolFailure::InvalidArguments(
                "old_text cannot be empty".into(),
            ));
        }
        let (initial, original) = self.read_text(tool, raw, MAX_READ_BYTES)?;
        let matches = original.matches(old_text).count();
        if matches == 0 || (matches > 1 && !replace_all) {
            return Err(ToolFailure::InvalidArguments(format!(
                "old_text has {matches} matches; provide a unique fragment or replace_all"
            )));
        }
        let updated = if replace_all {
            original.replace(old_text, new_text)
        } else {
            original.replacen(old_text, new_text, 1)
        };
        let expected = fsops::sha256_bytes(original.as_bytes());
        let (path, result, _) = recovery::mutate(
            self,
            tool,
            &initial.display(),
            Some(&expected),
            Some(updated.as_bytes()),
            false,
        )
        .await?;
        Ok((path, result))
    }

    pub async fn create_folder(&self, tool: &str, raw: &str) -> Result<ResolvedPath, ToolFailure> {
        let path = self.resolve_any(raw)?;
        if path.exists && !path.is_dir {
            return Err(ToolFailure::InvalidArguments(format!(
                "{} already exists as a file",
                path.display()
            )));
        }
        let scoped = crate::grants::applicable(self, tool, &path, 0)?;
        let approved = if scoped {
            true
        } else {
            self.authorize_write(tool, &path).await?
        };
        let _lock = fsops::PathLock::acquire(&self.app.workspace.lock_dir, &path.display())
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        let current = self.resolve_any(raw)?;
        if current.absolute != path.absolute || (current.exists && !current.is_dir) {
            return Err(ToolFailure::Conflict(
                "folder target changed during approval".into(),
            ));
        }
        self.recheck_access(Access::Write, &current, approved)?;
        let _grant = crate::grants::reserve(self, tool, &current, 0)?;
        fs::create_dir_all(&path.absolute)
            .map_err(|err| ToolFailure::Io(format!("create folder {}: {err}", path.display())))?;
        self.app
            .audit_path_access(
                self.agent,
                self.session_id,
                &format!("tool.{tool}"),
                Some(path.display()),
                "allowed",
                "directory created",
            )
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        Ok(path)
    }
}

pub fn require_text_extension(path: &Path, allowed: &[&str]) -> Result<(), ToolFailure> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if allowed
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
    {
        Ok(())
    } else {
        Err(ToolFailure::InvalidArguments(format!(
            "unsupported document extension .{extension}; allowed: {}",
            allowed.join(", ")
        )))
    }
}

pub fn path_string(path: &Path) -> Option<String> {
    Some(path.to_string_lossy().to_string())
}

pub fn parse_agent_arg(value: &str) -> Result<Agent, ToolFailure> {
    Agent::from_dir(value)
        .ok_or_else(|| ToolFailure::InvalidArguments(format!("unknown agent `{value}`")))
}

pub async fn dispatch(
    app: &App,
    agent: Agent,
    session_id: &str,
    tool: &str,
    arguments: Value,
) -> ToolResultMessage {
    let started = Instant::now();
    let Some(spec) = catalog::find(tool) else {
        return ToolResultMessage::error(tool, "unknown_tool", format!("unknown tool `{tool}`"));
    };
    if !app.workspace.profile.allows(spec)
        || (app.workspace.read_only && spec.writes && tool != "cancel_job")
    {
        let reason = format!(
            "tool {tool} is unavailable in profile {} (read_only={})",
            app.workspace.profile.name(),
            app.workspace.read_only
        );
        let _ = app.audit_denial(agent, session_id, tool, None, &reason, "denied");
        let _ = app.record_activity(agent, session_id, tool, "denial", None, &reason, "denied");
        return ToolResultMessage::error(tool, "permission_denied", reason);
    }
    let context = ToolContext::new(app, agent, session_id);
    // A scoped grant cannot be bypassed by choosing another filesystem writer.
    // Trash/restore/import already have their own mandatory approval path.
    if spec.writes
        && matches!(
            spec.category,
            catalog::Category::Filesystem
                | catalog::Category::Research
                | catalog::Category::Collaboration
        )
        && !crate::grants::TOOLS.contains(&tool)
        && !matches!(tool, "delete_file" | "restore_change" | "import_artifact" | "apply_patch" | "restore_patch")
    {
        match app.store.write_grant(&app.workspace.id(), session_id) {
            Ok(None) => {}
            Ok(Some(_)) => {
                let reason = "this operation is outside the session's scoped file-write grant";
                let _ = app.audit_denial(agent, session_id, tool, None, reason, "denied");
                return ToolResultMessage::error(tool, "permission_denied", reason.into());
            }
            Err(e) => return ToolResultMessage::error(tool, "grant_error", e.to_string()),
        }
    }
    let result = match tool {
        "preview_patch" => patches::preview(&context, arguments).await,
        "apply_patch" => patches::apply(&context, arguments).await,
        "patch_status" => patches::status(&context, arguments).await,
        "restore_patch" => patches::restore(&context, arguments).await,
        "list_jobs" => crate::jobs::list(&context, arguments).await,
        "job_status" => crate::jobs::status(&context, arguments).await,
        "read_job_output" => crate::jobs::output(&context, arguments).await,
        "cancel_job" => crate::jobs::cancel(&context, arguments).await,
        "inspect_processes" => crate::jobs::processes(&context, arguments).await,
        "workspace_info" => system::workspace_info(&context, arguments).await,
        "system_info" => system::system_info(&context, arguments).await,
        "list_artifacts" => artifacts::list(&context, arguments).await,
        "import_artifact" => artifacts::import(&context, arguments).await,
        "list_files" => filesystem::list_files(&context, arguments).await,
        "read_file" => filesystem::read_file(&context, arguments).await,
        "preview_document" => recovery::preview(&context, arguments).await,
        "list_changes" => recovery::list(&context, arguments).await,
        "restore_change" => recovery::restore(&context, arguments).await,
        "delete_file" => recovery::delete(&context, arguments).await,
        "append_document" => file_extra::append(&context, arguments).await,
        "copy_file" => file_extra::copy(&context, arguments).await,
        "read_multiple_files" => file_extra::read_multiple(&context, arguments).await,
        "glob_files" => file_extra::glob(&context, arguments).await,
        "file_tree" => file_extra::tree(&context, arguments).await,
        "diff_files" => file_extra::diff(&context, arguments).await,
        "list_integrations" => crate::integrations::list(&context, arguments).await,
        "call_integration_tool" => crate::integrations::call(&context, arguments).await,
        "search_files" => filesystem::search_files(&context, arguments).await,
        "write_document" => filesystem::write_document(&context, arguments).await,
        "edit_document" => filesystem::edit_document(&context, arguments).await,
        "create_folder" => filesystem::create_folder(&context, arguments).await,
        "move_file" => filesystem::move_file(&context, arguments).await,
        "inspect_file" => filesystem::inspect_file(&context, arguments).await,
        "inspect_file_metadata" => filesystem::inspect_file_metadata(&context, arguments).await,
        "inspect_dataset" => research::inspect_dataset(&context, arguments).await,
        "calculate_statistics" => research::calculate_statistics(&context, arguments).await,
        "generate_visualization" => research::generate_visualization(&context, arguments).await,
        "create_research_report" => research::create_research_report(&context, arguments).await,
        "inspect_research_manifest" => {
            research::inspect_research_manifest(&context, arguments).await
        }
        "calculate_sha256" => research::calculate_sha256(&context, arguments).await,
        "verify_file_integrity" => research::verify_file_integrity(&context, arguments).await,
        "list_agents" => collaboration::list_agents(&context).await,
        "inspect_agent_workspace" => {
            collaboration::inspect_agent_workspace(&context, arguments).await
        }
        "read_agent_findings" => collaboration::read_agent_findings(&context, arguments).await,
        "submit_research_proposal" => {
            collaboration::submit_research_proposal(&context, arguments).await
        }
        "submit_peer_review" => collaboration::submit_peer_review(&context, arguments).await,
        "record_disagreement" => collaboration::record_disagreement(&context, arguments).await,
        "git_status" => development::git_status(&context, arguments).await,
        "git_diff" => development::git_diff(&context, arguments).await,
        "inspect_repository" => development::inspect_repository(&context, arguments).await,
        "run_approved_script" => development::run_approved_script(&context, arguments).await,
        "run_command" => commands::run_command(&context, arguments).await,
        "list_allowed_commands" => commands::list_allowed_commands(&context, arguments).await,
        "inspect_test_results" => development::inspect_test_results(&context, arguments).await,
        "desktop_screenshot" => desktop::desktop_screenshot(&context, arguments).await,
        "active_application" => desktop::active_application(&context).await,
        "list_open_applications" => desktop::list_open_applications(&context).await,
        "inspect_accessible_window" => {
            desktop::inspect_accessible_window(&context, arguments).await
        }
        _ => Err(ToolFailure::Unsupported(format!(
            "tool `{tool}` is catalogued but not implemented"
        ))),
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(outcome) => {
            if matches!(
                tool,
                "preview_document"
                    | "patch_status"
                    | "list_jobs"
                    | "job_status"
                    | "read_job_output"
                    | "inspect_processes"
                    | "workspace_info"
                    | "system_info"
                    | "list_artifacts"
                    | "list_changes"
                    | "file_tree"
                    | "glob_files"
                    | "diff_files"
                    | "read_multiple_files"
                    | "list_integrations"
            ) {
                if let Err(err) = app.audit_path_access(
                    agent,
                    session_id,
                    &format!("tool.{tool}"),
                    outcome.path.clone(),
                    "allowed",
                    &outcome.summary,
                ) {
                    return ToolResultMessage::error(tool, "audit_error", err.to_string());
                }
            }
            let activity_kind = outcome.kind;
            let activity_path = outcome.path.clone();
            let failure = outcome.failure;
            let mut message =
                ToolResultMessage::success(tool, outcome.summary, outcome.output, outcome.data);
            if let Some((code, error)) = failure {
                message.status = "error".into();
                message.code = Some(code.into());
                message.error = Some(error);
            }
            if message.data.is_none() {
                message.data = Some(serde_json::json!({}));
            }
            if let Some(data) = &mut message.data {
                if let Some(object) = data.as_object_mut() {
                    object.insert("duration_ms".into(), Value::from(elapsed_ms));
                    object.insert("workspace_id".into(), Value::from(app.workspace.id()));
                    object.insert(
                        "workspace_root".into(),
                        Value::from(app.workspace.root.display().to_string()),
                    );
                }
            }
            if let Ok(mut activity) = app.store.record_activity(
                agent.dir(),
                session_id,
                tool,
                activity_kind,
                activity_path.as_deref(),
                message.summary.as_deref().unwrap_or_default(),
                if message.status == "ok" {
                    "allowed"
                } else {
                    "error"
                },
            ) {
                let _ = app
                    .store
                    .save_activity_result(activity.id, elapsed_ms, &message);
                activity.duration_ms = elapsed_ms;
                let _ = app
                    .events
                    .send(crate::wire::BridgeEvent::Activity { activity });
            }
            message
        }
        Err(failure) => {
            let decision = match failure {
                ToolFailure::PermissionDenied(_) | ToolFailure::ApprovalDenied(_) => "denied",
                _ => "error",
            };
            let _ = app.audit_denial(
                agent,
                session_id,
                tool,
                None,
                &failure.to_string(),
                decision,
            );
            let message = ToolResultMessage::error(tool, failure.code(), failure.to_string());
            if let Ok(mut activity) = app.store.record_activity(
                agent.dir(),
                session_id,
                tool,
                "error",
                None,
                &failure.to_string(),
                decision,
            ) {
                let _ = app
                    .store
                    .save_activity_result(activity.id, elapsed_ms, &message);
                activity.duration_ms = elapsed_ms;
                let _ = app
                    .events
                    .send(crate::wire::BridgeEvent::Activity { activity });
            }
            message
        }
    }
}

/// Convert an owned result to the JSON payload embedded in a conversation.
pub fn result_payload(message: &ToolResultMessage) -> Value {
    serde_json::to_value(message).unwrap_or_else(|_| {
        serde_json::json!({"tool":"unknown","status":"error","error":"result serialization failed"})
    })
}

/// Create a directory and a file in a single test-friendly helper used by the
/// self-test and future integration tests.
pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

pub fn relative_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

pub fn lock_key(path: &ResolvedPath) -> String {
    path.relative.to_string_lossy().to_string()
}

pub fn absolute(root: &Path, relative: &str) -> PathBuf {
    root.join(relative)
}

/// Escape text used in generated Markdown tables and report bodies.
pub fn markdown_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}
