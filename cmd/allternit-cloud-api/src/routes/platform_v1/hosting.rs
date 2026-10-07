//! Where hosted agents run (spec §5): one hosted runtime per project.
//!
//! The runtime is a cloud computer from the free-tier provisioning lane
//! (small, sleeps when idle, wakes on relay traffic) owned by the synthetic
//! user `platform:<project_id>` (an internal `users` row, no email or login), so it is never the developer's own computer
//! and one project's agents never share a runtime with another's. API traffic
//! counts as owner activity, so an active project's runtime is kept; an idle
//! one may be removed and is created again on the next request (agents and
//! sessions are re-sent when the runtime id changes).
//!
//! Calls go through `runtime_relay::relay_signed_request_to_runtime_with`
//! (signed with the runtime's device token, wakes a sleeping computer). The
//! runtime side is `cmd/allternit-api/src/platform_agents.rs`.
//!
//! [`AgentHost`] is the seam: production is [`ProdHost`]; tests layer a fake as
//! an `Extension<Arc<dyn AgentHost>>`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::StatusCode;
use serde_json::Value;

use super::PlatformError;
use crate::routes::voice_calls_cloud::{CallRelay, ProdCallRelay, RelayStream};
use crate::ApiState;

/// The synthetic owner of a project's hosted runtime.
pub fn runtime_owner(project_id: &str) -> String {
    format!("platform:{project_id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRuntime {
    pub owner: String,
    pub runtime_id: String,
}

pub fn starting() -> PlatformError {
    PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "runtime_starting".into(),
        message: "The agent's hosted runtime is starting. Retry in a few seconds.".into(),
        param: None,
    }
}

fn unavailable(detail: &str) -> PlatformError {
    tracing::warn!("platform hosting: {detail}");
    // 503, not 502: Cloudflare replaces an origin 502 with its own page, so the
    // developer would never see this error's code or message.
    PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "runtime_unavailable".into(),
        message: "The agent's hosted runtime couldn't be reached. Retry shortly.".into(),
        param: None,
    }
}

#[async_trait]
pub trait AgentHost: Send + Sync {
    /// The project's runtime, created on first use. `runtime_starting` (503)
    /// while it is still being set up.
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError>;
    /// One buffered call; a warming or offline runtime is `runtime_starting`.
    async fn call(&self, rt: &HostRuntime, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError>;
    /// A streamed call (the SSE turn).
    async fn stream(&self, rt: &HostRuntime, path: &str, body: &Value) -> Result<RelayStream, PlatformError>;
}

pub struct ProdHost(pub Arc<ApiState>);

/// 503 from the relay means the computer is waking or offline.
fn map_status(status: u16, body: &Value) -> Result<(), PlatformError> {
    match status {
        503 => Err(starting()),
        s if s >= 500 => Err(unavailable(&format!("runtime answered {s}: {body}"))),
        _ => Ok(()),
    }
}

#[async_trait]
impl AgentHost for ProdHost {
    async fn runtime(&self, project_id: &str) -> Result<HostRuntime, PlatformError> {
        let owner = runtime_owner(project_id);
        // Cloud computers belong to a `users` row; the project's runtime owner is
        // an internal account with no email or login, created on first use.
        sqlx::query("INSERT INTO users (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING")
            .bind(&owner)
            .bind(format!("Platform project {project_id}"))
            .execute(&self.0.db)
            .await
            .map_err(|e| unavailable(&format!("runtime owner {owner}: {e}")))?;
        let view = self.0.provisioning_service.create_free(&owner).await.map_err(|e| unavailable(&format!("provision {owner}: {e}")))?;
        if matches!(view.status.as_str(), "error" | "deleted") {
            return Err(unavailable(&format!("runtime {} is {}", view.id, view.status)));
        }
        match view.device_id {
            Some(runtime_id) => Ok(HostRuntime { owner, runtime_id }),
            None => Err(starting()),
        }
    }

    async fn call(&self, rt: &HostRuntime, method: &str, path: &str, body: &Value) -> Result<(u16, Value), PlatformError> {
        let bytes = serde_json::to_vec(body).unwrap_or_default();
        let (status, out) = ProdCallRelay { state: &self.0 }
            .relay_with(method, &rt.owner, &rt.runtime_id, path, &bytes)
            .await
            .map_err(|e| unavailable(&e))?;
        let value: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
        map_status(status, &value)?;
        Ok((status, value))
    }

    async fn stream(&self, rt: &HostRuntime, path: &str, body: &Value) -> Result<RelayStream, PlatformError> {
        let bytes = serde_json::to_vec(body).unwrap_or_default();
        let (status, stream) = ProdCallRelay { state: &self.0 }
            .stream(&rt.owner, &rt.runtime_id, path, &bytes)
            .await
            .map_err(|e| unavailable(&e))?;
        if status >= 400 {
            map_status(status, &Value::Null)?;
            return Err(unavailable(&format!("turn refused with {status}")));
        }
        Ok(stream)
    }
}

/// The host for a request: a test's fake when one is layered, else production.
pub fn host_for(state: &Arc<ApiState>, layered: Option<axum::Extension<Arc<dyn AgentHost>>>) -> Arc<dyn AgentHost> {
    match layered {
        Some(axum::Extension(h)) => h,
        None => Arc::new(ProdHost(state.clone())),
    }
}

/// Splits runtime SSE bytes into the JSON of each `data:` line, keeping a
/// partial line for the next chunk.
#[derive(Default)]
pub struct SseLines {
    pending: String,
}

impl SseLines {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Value> {
        self.pending.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            if let Some(data) = line.trim_end_matches(['\r', '\n']).strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data.trim_start()) {
                    out.push(v);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use serde_json::json;

    #[test]
    fn sse_lines_survive_split_chunks() {
        let mut p = SseLines::default();
        assert!(p.push(b"data: {\"type\":\"text.de").is_empty());
        let got = p.push(b"lta\",\"text\":\"hi\"}\n\ndata: {\"type\":\"done\",\"text\":\"hi\"}\n\n");
        assert_eq!(got, vec![json!({ "type": "text.delta", "text": "hi" }), json!({ "type": "done", "text": "hi" })]);
    }

    #[test]
    fn owners_are_per_project() {
        assert_eq!(runtime_owner("proj_1"), "platform:proj_1");
        assert_ne!(runtime_owner("proj_1"), runtime_owner("proj_2"));
    }
}
