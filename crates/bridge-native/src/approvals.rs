use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use tokio::sync::oneshot;

use crate::failure::ToolFailure;
use crate::wire::ApprovalRequest;

struct Pending {
    sender: oneshot::Sender<bool>,
    request: ApprovalRequest,
}

#[derive(Default)]
pub struct ApprovalManager {
    pub yolo: crate::yolo::Manager,
    pending: Mutex<HashMap<String, Pending>>,
}

impl ApprovalManager {
    pub fn new_id() -> String {
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    pub fn build_request(
        &self,
        session_id: &str,
        agent: &str,
        tool: &str,
        reason: &str,
        path: Option<String>,
        timeout_secs: u64,
    ) -> ApprovalRequest {
        let now = Utc::now();
        let expires = now + chrono::Duration::seconds(timeout_secs as i64);
        ApprovalRequest {
            id: Self::new_id(),
            session_id: session_id.to_string(),
            agent: agent.to_string(),
            tool: tool.to_string(),
            reason: reason.to_string(),
            path,
            requested_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            expires_at: expires.to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }

    /// Register a pending request and notify the extension. The tool call
    /// remains suspended until the user decides or the timeout expires.
    pub async fn ask<F>(
        &self,
        request: ApprovalRequest,
        timeout_secs: u64,
        notify: F,
    ) -> Result<(), ToolFailure>
    where
        F: FnOnce(&ApprovalRequest),
    {
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("approval mutex");
            pending.insert(
                request.id.clone(),
                Pending {
                    sender,
                    request: request.clone(),
                },
            );
        }
        let _cleanup = PendingCleanup {
            manager: self,
            id: request.id.clone(),
        };
        notify(&request);
        let timeout = Duration::from_secs(timeout_secs);
        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(ToolFailure::ApprovalDenied(format!(
                "the user denied permission for {}",
                request.tool
            ))),
            Ok(Err(_)) => Err(ToolFailure::ApprovalDenied(
                "the approval request was closed before a decision".into(),
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .expect("approval mutex")
                    .remove(&request.id);
                Err(ToolFailure::ApprovalDenied(format!(
                    "approval timed out after {timeout_secs}s"
                )))
            }
        }
    }

    /// Returns true if a pending request with that id existed.
    pub fn resolve(&self, id: &str, allow: bool) -> bool {
        let pending = self.pending.lock().expect("approval mutex").remove(id);
        match pending {
            Some(pending) => pending.sender.send(allow).is_ok(),
            None => false,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("approval mutex").len()
    }

    /// Snapshot used by the status channel so a reopened popup or a service
    /// worker restart can still display an approval that is blocked in the host.
    pub fn pending_requests(&self) -> Vec<ApprovalRequest> {
        self.pending
            .lock()
            .expect("approval mutex")
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }
}

/// Also remove the approval when an enclosing execution timeout cancels `ask`.
struct PendingCleanup<'a> {
    manager: &'a ApprovalManager,
    id: String,
}

impl Drop for PendingCleanup<'_> {
    fn drop(&mut self) {
        self.manager
            .pending
            .lock()
            .expect("approval mutex")
            .remove(&self.id);
    }
}
