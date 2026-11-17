use anyhow::{bail, Result};
use chrono::{SecondsFormat, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use std::sync::Arc;
use tokio::sync::broadcast;

use bridge_core::instructions::build_profile_instructions;
use bridge_core::provider::{Agent, Provider};
use bridge_core::workspace::{Capabilities, WorkspaceInfo};

use crate::approvals::ApprovalManager;
use crate::config::{Config, HomePaths};
use crate::failure::ToolFailure;
use crate::sandbox::{Access, Sandbox};
use crate::store::{ActivityRecord, SessionRecord, Store};
use crate::wire::{ApprovalRequest, BridgeEvent};
use crate::workspace::Workspace;

pub struct App {
    pub home: HomePaths,
    pub config: Config,
    pub workspace: Workspace,
    pub store: Arc<Store>,
    pub approvals: Arc<ApprovalManager>,
    pub browser_files: Arc<crate::browser_files::Broker>,
    pub events: broadcast::Sender<BridgeEvent>,
    /// True only when a browser extension can answer approval prompts. MCP and
    /// headless runs fail closed for protected operations.
    pub interactive: bool,
}

pub struct SessionOpened {
    pub session: SessionRecord,
    pub instructions: String,
}

impl App {
    pub fn load(home: HomePaths, interactive: bool) -> Result<Arc<Self>> {
        let config = home.load_config()?;
        let workspace = Workspace::load(&config.project_root)?;
        Self::open_with_config(home, config, workspace, interactive)
    }

    pub fn open_with_config(
        home: HomePaths,
        mut config: Config,
        workspace: Workspace,
        interactive: bool,
    ) -> Result<Arc<Self>> {
        home.ensure()?;
        let audit_file = home.base.join("audit.jsonl");
        let store = Store::open_migrating(
            &home.database,
            &audit_file,
            &workspace.audit_file,
            &home.base.join("locks"),
        )?;
        store.prune_sessions_older_than(24 * 30)?;
        let mut workspace = workspace;
        if let Some(id) = store.active_workspace_id()? {
            let known = store
                .workspaces()?
                .into_iter()
                .find(|entry| entry.id == id)
                .ok_or_else(|| {
                    anyhow::anyhow!("active workspace {id} is missing from the registry")
                })?;
            workspace = Workspace::load(std::path::Path::new(&known.root))?;
            workspace.read_only = known.read_only;
            workspace.permission_mode = known.permission_mode;
        } else {
            store.save_workspace(&workspace.info(workspace.project_name()))?;
            store.set_active_workspace(&workspace.id())?;
        }
        config.project_root = workspace.root.clone();
        let (events, _) = broadcast::channel(512);
        Ok(Arc::new(Self {
            home,
            config,
            workspace,
            store: Arc::new(store),
            approvals: Arc::new(ApprovalManager::default()),
            browser_files: Arc::default(),
            events,
            interactive,
        }))
    }

    pub fn sandbox(&self) -> Sandbox {
        Sandbox::new(&self.workspace).with_exclusions(&self.config.private_file_patterns)
    }

    pub fn with_workspace(self: &Arc<Self>, workspace: Workspace) -> Arc<Self> {
        let mut config = self.config.clone();
        config.project_root = workspace.root.clone();
        Arc::new(Self {
            home: self.home.clone(),
            config,
            workspace,
            store: self.store.clone(),
            approvals: self.approvals.clone(),
            browser_files: self.browser_files.clone(),
            events: self.events.clone(),
            interactive: self.interactive,
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            profile: self.workspace.profile,
            tools: self.workspace.profile.tools().collect(),
            permissions: self.workspace.permission_mode,
            root: self.workspace.root.display().to_string(),
            read_only: self.workspace.read_only,
        }
    }

    pub fn grant_command(
        self: &Arc<Self>,
        command: crate::config::AllowedCommand,
    ) -> Result<Arc<Self>> {
        command.validate()?;
        let cwd = self.sandbox().resolve(
            &command.cwd.to_string_lossy(),
            crate::sandbox::Existence::MustExist,
        )?;
        if !cwd.is_dir {
            bail!("command cwd must be a directory inside the active workspace");
        }
        let mut config = self.config.clone();
        let commands = config
            .workspace_commands
            .entry(self.workspace.id())
            .or_default();
        if commands
            .iter()
            .any(|existing| existing.command_id == command.command_id)
        {
            bail!("command ID already exists; remove it before granting a replacement");
        }
        commands.push(command.clone());
        // The panel is the sole writer. Tool arguments cannot call this method.
        self.audit_workspace(
            "command.grant",
            serde_json::json!({"workspace_id":self.workspace.id(),"command":command}),
        )?;
        self.home.save_config(&config)?;
        Ok(Arc::new(Self {
            home: self.home.clone(),
            config,
            workspace: self.workspace.clone(),
            store: self.store.clone(),
            approvals: self.approvals.clone(),
            browser_files: self.browser_files.clone(),
            events: self.events.clone(),
            interactive: self.interactive,
        }))
    }

    pub fn revoke_command(self: &Arc<Self>, id: &str) -> Result<Arc<Self>> {
        let mut config = self.config.clone();
        let commands = config
            .workspace_commands
            .entry(self.workspace.id())
            .or_default();
        let before = commands.len();
        commands.retain(|command| command.command_id != id);
        if commands.len() == before {
            bail!("unknown command ID");
        }
        self.audit_workspace(
            "command.revoke",
            serde_json::json!({"workspace_id":self.workspace.id(),"command_id":id}),
        )?;
        self.home.save_config(&config)?;
        Ok(Arc::new(Self {
            home: self.home.clone(),
            config,
            workspace: self.workspace.clone(),
            store: self.store.clone(),
            approvals: self.approvals.clone(),
            browser_files: self.browser_files.clone(),
            events: self.events.clone(),
            interactive: self.interactive,
        }))
    }

    pub fn instructions(&self, agent: Agent, provider: &str) -> String {
        build_profile_instructions(
            agent,
            &self.workspace.root.display().to_string(),
            provider,
            self.workspace.profile,
            self.workspace.permission_mode,
            self.workspace.read_only,
        )
    }

    pub fn select_workspace(self: &Arc<Self>, id: &str) -> Result<Arc<Self>> {
        let mut info = self
            .store
            .workspaces()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown workspace {id}"))?;
        let mut workspace = Workspace::load(std::path::Path::new(&info.root))?;
        workspace.check_writable()?;
        workspace.read_only = info.read_only;
        workspace.permission_mode = info.permission_mode;
        info.last_used_at = Self::now();
        self.store.save_workspace(&info)?;
        let next = self.with_workspace(workspace);
        self.home.save_config(&next.config)?;
        let commit = (|| -> Result<()> {
            self.store.set_active_workspace(id)?;
            self.store.rebind_sessions(id)?;
            self.audit_workspace("workspace.switch", serde_json::json!({"from":self.workspace.id(),"from_root":self.workspace.root,"to":id,"to_root":next.workspace.root}))
        })();
        if let Err(err) = commit {
            let _ = self.store.set_active_workspace(&self.workspace.id());
            let _ = self.store.rebind_sessions(&self.workspace.id());
            let _ = self.home.save_config(&self.config);
            return Err(err);
        }
        // A switch never carries automatic approval into another workspace.
        if let Err(err) = crate::yolo::stop(self, "workspace switched") {
            eprintln!("YOLO stopped; audit failed: {err}");
        }
        // The switch is already committed and audited. An optional feed error
        // must not leave the running host on its previous App/root snapshot.
        match self.store.record_activity(
            "USER",
            "extension-panel",
            "workspace",
            "workspace",
            Some(&next.workspace.root.display().to_string()),
            &format!(
                "Workspace → {} · sessions updated",
                next.workspace.root.display()
            ),
            "allowed",
        ) {
            Ok(activity) => {
                let _ = self.events.send(BridgeEvent::Activity { activity });
            }
            Err(err) => eprintln!("workspace switch committed; activity feed failed: {err}"),
        }
        Ok(next)
    }

    pub fn create_workspace(
        &self,
        label: &str,
        path: &str,
        profile: bridge_core::workspace::Profile,
    ) -> Result<WorkspaceInfo> {
        if label.trim().is_empty() || label.len() > 100 {
            bail!("workspace label must be 1–100 bytes");
        }
        let workspace = Workspace::initialize_profile(std::path::Path::new(path), profile)?;
        workspace.check_writable()?;
        if let Some(existing) = self
            .store
            .workspaces()?
            .into_iter()
            .find(|entry| entry.id == workspace.id())
        {
            return Ok(existing);
        }
        let info = workspace.info(label.trim().into());
        self.audit_workspace("workspace.create", serde_json::to_value(&info)?)?;
        self.store.save_workspace(&info)?;
        Ok(info)
    }

    pub fn audit_workspace(&self, action: &str, detail: serde_json::Value) -> Result<()> {
        self.store.append_audit(crate::store::AuditInput {
            agent: "USER".into(),
            session_id: "extension-panel".into(),
            action: action.into(),
            path: None,
            decision: "allowed".into(),
            detail: detail.to_string(),
        })?;
        Ok(())
    }

    pub fn now() -> String {
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    pub fn register_session(
        &self,
        provider: Provider,
        tab_id: i64,
        window_id: i64,
        url: &str,
        conversation_hint: Option<String>,
        extension_version: Option<String>,
    ) -> Result<SessionOpened> {
        if tab_id <= 0 {
            bail!("tab_id must be a positive Chrome tab id");
        }
        if url.len() > 8192 {
            bail!("registration URL is unreasonably long");
        }
        provider
            .validate_url(url)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        let agent = provider.agent();
        let derived = provider.conversation_id(url);
        let conversation = derived.or_else(|| {
            conversation_hint.and_then(|hint| {
                let hint = hint
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(200)
                    .collect::<String>();
                (!hint.trim().is_empty()).then(|| hint.trim().to_string())
            })
        });
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        let now = Self::now();
        let session = SessionRecord {
            id: hex::encode(bytes),
            agent: agent.dir().to_string(),
            provider: provider.slug().to_string(),
            tab_id,
            window_id,
            url: url.to_string(),
            conversation,
            extension_version,
            workspace_id: self.workspace.id(),
            created_at: now.clone(),
            last_seen_at: now,
        };
        self.store.insert_session(&session)?;
        self.record_activity(
            agent,
            &session.id,
            "session",
            "session",
            Some(&agent.workspace_relative()),
            &format!("{} session opened for tab {}", provider.display(), tab_id),
            "allowed",
        )?;
        let instructions = self.instructions(agent, provider.display());
        self.store.append_audit(crate::store::AuditInput {
            agent: agent.dir().into(),
            session_id: session.id.clone(),
            action: "session.open".into(),
            path: Some(agent.workspace_relative()),
            decision: "allowed".into(),
            detail: serde_json::json!({
                "provider": provider.slug(),
                "tab_id": tab_id,
                "conversation": session.conversation,
                "url": session.url,
                "workspace_id": self.workspace.id(),
                "workspace_root": self.workspace.root
            })
            .to_string(),
        })?;
        Ok(SessionOpened {
            session,
            instructions,
        })
    }

    pub fn session_agent(
        &self,
        session_id: &str,
    ) -> Result<Option<(SessionRecord, Agent)>, ToolFailure> {
        let Some(session) = self
            .store
            .get_session(session_id)
            .map_err(|err| ToolFailure::Internal(err.to_string()))?
        else {
            return Ok(None);
        };
        let Some(agent) = Agent::from_dir(&session.agent) else {
            return Err(ToolFailure::Internal(format!(
                "session {} has an unknown agent identity",
                session.id
            )));
        };
        Ok(Some((session, agent)))
    }

    pub fn end_session(&self, session_id: &str) -> Result<()> {
        if let Some(session) = self.store.get_session(session_id)? {
            if let Some(agent) = Agent::from_dir(&session.agent) {
                self.record_activity(
                    agent,
                    session_id,
                    "session",
                    "session",
                    None,
                    "session closed",
                    "allowed",
                )?;
            }
        }
        self.store.delete_session(session_id)?;
        Ok(())
    }

    pub fn record_activity(
        &self,
        agent: Agent,
        session_id: &str,
        tool: &str,
        kind: &str,
        path: Option<&str>,
        summary: &str,
        decision: &str,
    ) -> Result<ActivityRecord> {
        let activity = self.store.record_activity(
            agent.dir(),
            session_id,
            tool,
            kind,
            path,
            summary,
            decision,
        )?;
        let _ = self.events.send(BridgeEvent::Activity {
            activity: activity.clone(),
        });
        Ok(activity)
    }

    pub async fn request_approval(
        &self,
        session_id: &str,
        agent: Agent,
        tool: &str,
        reason: &str,
        path: Option<String>,
    ) -> Result<(), ToolFailure> {
        crate::tasks::checkpoint(self)?;
        if let Some(path) = path.as_ref() {
            if crate::yolo::approve(self, session_id, agent, tool, std::slice::from_ref(path), false)? {
                return Ok(());
            }
        }
        if !self.interactive {
            self.audit_denial(
                agent,
                session_id,
                tool,
                path.as_deref(),
                "approval required but no browser UI is connected",
                "denied",
            )
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
            return Err(ToolFailure::PermissionDenied(format!(
                "{} requires user approval, which is unavailable in this headless session",
                tool
            )));
        }
        let request = self.approvals.build_request(
            session_id,
            agent.dir(),
            tool,
            reason,
            path.clone(),
            self.config.approval_timeout_secs,
        );
        self.store
            .append_audit(crate::store::AuditInput {
                agent: agent.dir().into(),
                session_id: session_id.into(),
                action: format!("approval.request:{tool}"),
                path: path.clone(),
                decision: "pending".into(),
                detail: format!("approval id {}", request.id),
            })
            .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        let tx = self.events.clone();
        let request_for_event = request.clone();
        let approval_id = request.id.clone();
        let ask = self
            .approvals
            .ask(request, self.config.approval_timeout_secs, move |_| {
                let _ = tx.send(BridgeEvent::ApprovalRequired {
                    approval: request_for_event.clone(),
                });
            });
        let result = tokio::select! {
            result = ask => result.and_then(|_|crate::tasks::checkpoint(self)),
            error = crate::tasks::wait_until_halted(self) => {
                let _ = self.events.send(BridgeEvent::ApprovalResolved {approval_id, allowed:false});
                Err(error)
            }
        };
        match &result {
            Ok(()) => self.audit_approval(agent, session_id, tool, path, "approved"),
            Err(_) => self.audit_approval(agent, session_id, tool, path, "denied"),
        }
        .map_err(|err| ToolFailure::Internal(err.to_string()))?;
        result
    }

    fn audit_approval(
        &self,
        agent: Agent,
        session_id: &str,
        tool: &str,
        path: Option<String>,
        decision: &str,
    ) -> Result<()> {
        self.store.append_audit(crate::store::AuditInput {
            agent: agent.dir().into(),
            session_id: session_id.into(),
            action: format!("approval.decision:{tool}"),
            path,
            decision: decision.into(),
            detail: String::new(),
        })?;
        Ok(())
    }

    pub fn audit_denial(
        &self,
        agent: Agent,
        session_id: &str,
        tool: &str,
        path: Option<&str>,
        reason: &str,
        decision: &str,
    ) -> Result<()> {
        self.store.append_audit(crate::store::AuditInput {
            agent: agent.dir().into(),
            session_id: session_id.into(),
            action: format!("tool.{tool}"),
            path: path.map(str::to_string),
            decision: decision.into(),
            detail: reason.to_string(),
        })?;
        Ok(())
    }

    pub fn audit_path_access(
        &self,
        agent: Agent,
        session_id: &str,
        action: &str,
        path: Option<String>,
        decision: &str,
        detail: &str,
    ) -> Result<()> {
        self.store.append_audit(crate::store::AuditInput {
            agent: agent.dir().into(),
            session_id: session_id.into(),
            action: action.into(),
            path,
            decision: decision.into(),
            detail: detail.into(),
        })?;
        Ok(())
    }

    pub fn decide(
        &self,
        agent: Agent,
        access: Access,
        path: &crate::sandbox::ResolvedPath,
    ) -> crate::sandbox::Decision {
        if access == Access::Read {
            self.sandbox().decide(agent, access, path)
        } else {
            self.current_write_decision(agent, access, path)
        }
    }

    /// In-flight work keeps its original root, but cannot bypass a newly locked
    /// workspace or a stricter write policy while awaiting user approval.
    pub fn current_write_decision(
        &self,
        agent: Agent,
        access: Access,
        path: &crate::sandbox::ResolvedPath,
    ) -> crate::sandbox::Decision {
        if let Err(err) = crate::yolo::checkpoint(self) {
            return crate::sandbox::Decision::Deny(err.to_string());
        }
        let original = self.sandbox().decide(agent, access, path);
        if matches!(original, crate::sandbox::Decision::Deny(_)) {
            return original;
        }
        match self.store.workspaces() {
            Ok(workspaces) => {
                if let Some(info) = workspaces.into_iter().find(|w| w.id == self.workspace.id()) {
                    if info.read_only {
                        return crate::sandbox::Decision::Deny(
                            "workspace was locked read-only".into(),
                        );
                    }
                    if info.permission_mode == bridge_core::workspace::PermissionMode::Strict
                        && matches!(original, crate::sandbox::Decision::Allow)
                    {
                        return crate::sandbox::Decision::Approval(
                            "Strict mode requires approval".into(),
                        );
                    }
                    original
                } else {
                    crate::sandbox::Decision::Deny("workspace grant was removed".into())
                }
            }
            Err(_) => {
                crate::sandbox::Decision::Deny("workspace policy could not be verified".into())
            }
        }
    }
}

pub fn clean_approval_reason(reason: &str) -> String {
    reason
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .take(65_000)
        .collect()
}

pub fn approval_event_path(request: &ApprovalRequest) -> Option<String> {
    request.path.clone()
}

pub fn workspace_context(app: &App) -> String {
    let project = app.workspace.project_name();
    format!(
        "{} ({})",
        app.workspace.root.display(),
        if project.is_empty() {
            "archaeology"
        } else {
            &project
        }
    )
}
