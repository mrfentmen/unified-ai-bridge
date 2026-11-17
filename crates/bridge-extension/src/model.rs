use bridge_core::workspace::WorkspaceInfo;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const AUTO_CONNECT_KEY: &str = "uab_auto_connect";
pub const EXTENSION_VERSION: &str = env!("CARGO_PKG_VERSION");

/// SQLite contains historical sessions. Only registrations observed by this
/// background connection may appear as connected in the provider rail.
pub fn refresh_live_sessions(live: &mut Vec<SessionView>, stored: Vec<SessionView>) {
    let ids: std::collections::HashSet<_> = live.iter().map(|s| s.id.clone()).collect();
    *live = stored.into_iter().filter(|s| ids.contains(&s.id)).collect();
}

pub fn session_missing(status: &StatusView, id: Option<&str>) -> bool {
    id.is_some_and(|id| !status.sessions.iter().any(|s|s.id == id))
}

#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn historical_sessions_never_become_live_from_a_status_reply() {
        let current = SessionView {
            id: "current".into(),
            tab_id: 1,
            ..Default::default()
        };
        let old = SessionView {
            id: "closed".into(),
            tab_id: 2,
            ..Default::default()
        };
        let mut live = vec![current.clone()];
        refresh_live_sessions(&mut live, vec![old, current]);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "current");
        live.clear();
        refresh_live_sessions(
            &mut live,
            vec![SessionView {
                id: "stored".into(),
                ..Default::default()
            }],
        );
        assert!(live.is_empty());
    }

    #[test]
    fn host_restart_invalidates_the_old_session_until_registered_again() {
        let mut status = StatusView::default();
        status.bridge = "connected".into();
        status.sessions = vec![SessionView {id:"old".into(),..Default::default()}];
        assert!(!session_missing(&status,Some("old")));
        status.sessions.clear();
        assert!(session_missing(&status,Some("old")));
        status.sessions.push(SessionView{id:"replacement".into(),..Default::default()});
        assert!(session_missing(&status,Some("old")));
        assert!(!session_missing(&status,Some("replacement")));
        assert!(!session_missing(&status,None));
    }
}

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
    pub yolo: Value,
    #[serde(default)]
    pub task_states: Vec<Value>,
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
