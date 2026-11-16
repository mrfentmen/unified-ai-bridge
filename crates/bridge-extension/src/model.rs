use bridge_core::workspace::WorkspaceInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const AUTO_CONNECT_KEY: &str = "uab_auto_connect";
pub const EXTENSION_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionView {
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub tab_id: i64,
    #[serde(default)]
    pub window_id: i64,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(default)]
    pub extension_version: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActivityView {
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub decision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApprovalView {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub requested_at: String,
    #[serde(default)]
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusView {
    #[serde(default)]
    pub reconnect_attempt: u32,
    #[serde(default)]
    pub active_id: String,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub in_flight: usize,
    #[serde(default)]
    pub database_path: String,
    #[serde(default)]
    pub log_path: String,
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub extension_build: String,
    #[serde(default)]
    pub bridge: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub host_version: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub sessions: Vec<SessionView>,
    #[serde(default)]
    pub activity: Vec<ActivityView>,
    #[serde(default)]
    pub approvals: Vec<ApprovalView>,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub audit_valid: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BackgroundResponse {
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub bridge: String,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub status: Option<StatusView>,
}

pub fn json_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
