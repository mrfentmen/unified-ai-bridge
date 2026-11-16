use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::{ActivityRecord, SessionRecord};
use bridge_core::workspace::{PermissionMode, Profile, WorkspaceInfo};

/// Extension -> native host. The extension is trusted only for its registration
/// data; every tool call is bound to a host-issued session id.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtToHost {
    Hello {
        protocol: u32,
        extension_version: String,
    },
    RegisterSession {
        provider: String,
        tab_id: i64,
        #[serde(default)]
        window_id: i64,
        url: String,
        #[serde(default)]
        conversation: Option<String>,
        #[serde(default)]
        auto_connect: bool,
    },
    ToolRequest {
        session_id: String,
        workspace_id: String,
        request_key: String,
        response_id: String,
        task_id: String,
        call_index: u32,
        tool: String,
        #[serde(default)]
        arguments: Value,
    },
    ApprovalDecision {
        approval_id: String,
        allow: bool,
    },
    UnregisterSession {
        session_id: String,
    },
    Status,
    RecoveryControl {
        request_key: String,
        workspace_id: String,
        action: crate::tools::large_recovery::Control,
    },
    JobControl {
        request_key: String,
        workspace_id: String,
        action: crate::jobs::Control,
    },
    Preflight {
        request_key: String,
    },
    CreateWriteGrant {
        request_key: String,
        workspace_id: String,
        grant: crate::grants::Request,
    },
    RevokeWriteGrant {
        request_key: String,
        workspace_id: String,
        session_id: String,
    },
    ListWriteGrants {
        request_key: String,
    },
    ListWorkspaces {
        request_key: String,
    },
    SetActiveWorkspace {
        request_key: String,
        workspace_id: String,
        #[serde(default)]
        confirm_in_flight: bool,
    },
    CreateWorkspace {
        request_key: String,
        label: String,
        path: String,
        profile: Profile,
    },
    ForgetWorkspace {
        request_key: String,
        workspace_id: String,
    },
    SetWorkspacePolicy {
        request_key: String,
        read_only: bool,
        permission_mode: PermissionMode,
    },
    RescanWorkspace {
        request_key: String,
    },
    OpenWorkspace {
        request_key: String,
    },
    PickWorkspaceFolder {
        request_key: String,
    },
    GrantCommand {
        request_key: String,
        command: crate::config::AllowedCommand,
    },
    RevokeCommand {
        request_key: String,
        command_id: String,
    },
    ListCommandGrants {
        request_key: String,
    },
    ActivityDetail {
        request_key: String,
        activity_id: i64,
    },
    RunTool {
        request_key: String,
        workspace_id: String,
        session_id: String,
        tool: String,
        arguments: Value,
    },
    RegisterArtifact {
        request_key: String,
        workspace_id: String,
        session_id: String,
        download: crate::tools::artifacts::SelectedDownload,
    },
    RevokeArtifact {
        request_key: String,
        workspace_id: String,
        session_id: String,
        artifact_id: String,
    },
    CommandPresets {
        request_key: String,
    },
    IntegrationList {
        request_key: String,
    },
    IntegrationStatus {request_key:String,server_id:String},
    IntegrationSave {
        request_key: String,
        server: crate::integrations::ServerConfig,
    },
    IntegrationRemove {
        request_key: String,
        server_id: String,
    },
    IntegrationDiscover {
        request_key: String,
        server_id: String,
    },
    Ping {
        #[serde(default)]
        nonce: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToExt {
    HelloAck {
        protocol: u32,
        host_version: String,
        interactive: bool,
        workspace_root: String,
        project_name: String,
        native_host_name: String,
        tool_timeout_secs: u64,
        workspace_id: String,
        profile: Profile,
    },
    SessionRegistered {
        session: SessionRecord,
        instructions: String,
        auto_connect: bool,
        capabilities: Value,
    },
    ToolResult {
        session_id: String,
        request_key: String,
        result: ToolResultMessage,
        duration_ms: u64,
    },
    ApprovalRequired {
        approval: ApprovalRequest,
    },
    ApprovalResolved {
        approval_id: String,
        allowed: bool,
    },
    StatusReply {
        bridge: String,
        host_version: String,
        workspace_root: String,
        sessions: Vec<SessionRecord>,
        recent_activity: Vec<ActivityRecord>,
        approvals: Vec<ApprovalRequest>,
        audit_valid: bool,
        protocol: u32,
        extension_build: String,
        active_id: String,
        workspaces: Vec<WorkspaceInfo>,
        capabilities: Value,
        in_flight: usize,
        database_path: String,
        log_path: String,
    },
    WorkspaceList {
        request_key: String,
        active_id: String,
        workspaces: Vec<WorkspaceInfo>,
    },
    WorkspaceChanged {
        request_key: String,
        workspace_id: String,
        root: String,
        profile: Profile,
        migrated: bool,
        workspaces: Vec<WorkspaceInfo>,
        capabilities: Value,
        sessions: Vec<SessionRecord>,
    },
    ControlResult {
        request_key: String,
        result: Value,
    },
    Event {
        event: BridgeEvent,
    },
    Pong {
        nonce: u64,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_key: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BridgeEvent {
    Activity { activity: ActivityRecord },
    ApprovalRequired { approval: ApprovalRequest },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: String,
    pub agent: String,
    pub tool: String,
    pub reason: String,
    pub path: Option<String>,
    pub requested_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResultMessage {
    pub fn success(tool: &str, summary: String, output: String, data: Option<Value>) -> Self {
        Self {
            tool: tool.to_string(),
            status: "ok".into(),
            summary: Some(summary),
            output: Some(output),
            data,
            code: None,
            error: None,
        }
    }

    pub fn error(tool: &str, code: &str, error: String) -> Self {
        Self {
            tool: tool.to_string(),
            status: "error".into(),
            summary: None,
            output: None,
            data: None,
            code: Some(code.to_string()),
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_messages_are_strictly_tagged() {
        let message: ExtToHost = serde_json::from_value(serde_json::json!({
            "type":"hello","protocol":1,"extension_version":"0.1.0"
        }))
        .unwrap();
        assert!(matches!(message, ExtToHost::Hello { protocol: 1, .. }));
        assert!(serde_json::from_value::<ExtToHost>(serde_json::json!({"type":"shell"})).is_err());
    }
}
