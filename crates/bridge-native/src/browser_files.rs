//! A bounded request/reply bridge back to the owning browser tab. Pending
//! requests are removed on cancellation; late and mismatched replies fail closed.
use crate::{app::App, failure::ToolFailure, wire::BridgeEvent};
use bridge_core::browser_files::{Action, Scope};
use serde_json::Value;
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::sync::oneshot;

type Pending = (Scope, oneshot::Sender<Result<Value, String>>);
tokio::task_local! { static DEADLINE_MS: i64; }

/// Enter before waiting for the native execution semaphore, so a queued
/// browser mutation never outlives the host's enclosing tool timeout.
pub async fn deadline<F: std::future::Future>(seconds: u64, future: F) -> F::Output {
    DEADLINE_MS
        .scope(
            chrono::Utc::now().timestamp_millis() + seconds.min(86_400) as i64 * 1_000 - 250,
            future,
        )
        .await
}
#[derive(Default)]
pub struct Broker {
    pending: Mutex<HashMap<String, Pending>>,
}

struct Cleanup<'a>(&'a Broker, String);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        self.0
            .pending
            .lock()
            .expect("browser requests")
            .remove(&self.1);
    }
}

impl Broker {
    pub async fn request(
        &self,
        app: &App,
        scope: Scope,
        action: Action,
    ) -> Result<Value, ToolFailure> {
        self.request_timeout(app, scope, action, Duration::from_secs(60))
            .await
    }
    async fn request_timeout(
        &self,
        app: &App,
        scope: Scope,
        action: Action,
        timeout: Duration,
    ) -> Result<Value, ToolFailure> {
        use rand::RngCore;
        if !app.interactive {
            return Err(ToolFailure::Unsupported(
                "Generated files require a connected browser extension.".into(),
            ));
        }
        crate::tasks::checkpoint(app)?;
        let expires_at_ms = (chrono::Utc::now().timestamp_millis() + timeout.as_millis() as i64)
            .min(DEADLINE_MS.try_with(|d| *d).unwrap_or(i64::MAX));
        if expires_at_ms <= chrono::Utc::now().timestamp_millis() + 1_000 {
            return Err(ToolFailure::Conflict("Tool deadline is too close to start a browser action. No new download was requested.".into()));
        }
        let mut nonce = [0; 16];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let id = hex::encode(nonce);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("browser requests");
            if pending.len() >= 16 {
                return Err(ToolFailure::Conflict(
                    "Too many pending browser operations.".into(),
                ));
            }
            pending.insert(id.clone(), (scope.clone(), tx));
        }
        let _cleanup = Cleanup(self, id.clone());
        app.events
            .send(BridgeEvent::BrowserOperation {
                id,
                scope,
                action,
                expires_at_ms,
                execute_before_ms: expires_at_ms.min(chrono::Utc::now().timestamp_millis() + 5_000),
            })
            .map_err(|_| {
                ToolFailure::Unsupported("Browser operation has no connected receiver.".into())
            })?;
        let reply = tokio::select! {
            reply = tokio::time::timeout(timeout, rx) => reply,
            error = crate::tasks::wait_until_halted(app) => return Err(error),
        };
        match reply {
            Ok(Ok(Ok(value))) => { crate::tasks::checkpoint(app)?; Ok(value) },
            Ok(Ok(Err(error))) => Err(ToolFailure::Conflict(error.chars().take(600).collect())),
            _ => Err(ToolFailure::Conflict("Browser operation timed out or disconnected. A file transfer may have started; inspect the chat composer and browser downloads before doing anything else. This action is not retried.".into())),
        }
    }
    pub fn resolve(&self, id: &str, scope: &Scope, result: Result<Value, String>) -> bool {
        let mut pending = self.pending.lock().expect("browser requests");
        if !pending
            .get(id)
            .is_some_and(|(expected, _)| expected == scope)
        {
            return false;
        }
        pending
            .remove(id)
            .is_some_and(|(_, tx)| tx.send(result).is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn replies_require_exact_scope_and_are_consumed_once() {
        let broker = Broker::default();
        let scope = Scope {
            session_id: "one".into(),
            workspace_id: "root".into(),
            provider: "chatgpt".into(),
            tab_id: 1,
            conversation: "chat".into(),
            task_id: Some("task".into()),
        };
        let (tx, rx) = oneshot::channel();
        broker
            .pending
            .lock()
            .unwrap()
            .insert("nonce".into(), (scope.clone(), tx));
        for wrong in [
            Scope {
                session_id: "two".into(),
                ..scope.clone()
            },
            Scope {
                workspace_id: "other".into(),
                ..scope.clone()
            },
            Scope {
                task_id: Some("later".into()),
                ..scope.clone()
            },
            Scope {
                tab_id: 2,
                ..scope.clone()
            },
        ] {
            assert!(!broker.resolve("nonce", &wrong, Ok(Value::Null)));
        }
        assert!(broker.resolve("nonce", &scope, Ok(Value::Bool(true))));
        assert!(!broker.resolve("nonce", &scope, Ok(Value::Null)));
        assert_eq!(rx.await.unwrap().unwrap(), Value::Bool(true));
        let (tx, _) = oneshot::channel();
        broker
            .pending
            .lock()
            .unwrap()
            .insert("cancelled".into(), (scope.clone(), tx));
        drop(Cleanup(&broker, "cancelled".into()));
        assert!(!broker.resolve("cancelled", &scope, Ok(Value::Null)));
    }
}
