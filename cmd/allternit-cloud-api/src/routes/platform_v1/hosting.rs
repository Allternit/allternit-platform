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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
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
        param: None, url: None, reason: None, refusal_status: None,
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
        param: None, url: None, reason: None, refusal_status: None,
    }
}

/// A provisioning failure: a full host is `computer_capacity` (503, with
/// `Retry-After`); anything else is `runtime_unavailable`.
fn provision_failed(owner: &str, error: crate::error::ApiError) -> PlatformError {
    let full = matches!(&error, crate::error::ApiError::ServiceUnavailable(m)
        if crate::services::provisioning::is_capacity_message(m));
    if full {
        PlatformError::from(error)
    } else {
        unavailable(&format!("provision {owner}: {error}"))
    }
}

/// A relay failure string that carries a capacity refusal (a relay wake that
/// found the host full) is `computer_capacity` too.
fn relay_failed(detail: &str) -> PlatformError {
    if crate::services::provisioning::mentions_capacity(detail) {
        return PlatformError::service_unavailable("computer_capacity", "No hosted runtime capacity right now. Retry shortly.");
    }
    unavailable(detail)
}

/// Replacing a runtime that never connected: at most [`REPLACE_AFTER_MINUTES`]`.len()`
/// times per project, each only once the stuck runtime is that old (5, 15, 45
/// minutes). The count is in memory (a restart forgets it: at most three more
/// replacements per project, never a loop); the clock is the stuck row's own
/// `created_at`, so it survives restarts.
const REPLACE_AFTER_MINUTES: [i64; 3] = [5, 15, 45];

#[derive(Default)]
struct ReplaceLedger(Mutex<HashMap<String, usize>>);

impl ReplaceLedger {
    /// True (and recorded) when the project's stuck runtime, created at
    /// `created_at`, may be replaced at `now`.
    fn may_replace(&self, project_id: &str, created_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let mut counts = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let done = counts.entry(project_id.to_string()).or_insert(0);
        let Some(minutes) = REPLACE_AFTER_MINUTES.get(*done) else {
            return false;
        };
        if now - created_at < Duration::minutes(*minutes) {
            return false;
        }
        *done += 1;
        true
    }
}

fn replace_ledger() -> &'static ReplaceLedger {
    static LEDGER: OnceLock<ReplaceLedger> = OnceLock::new();
    LEDGER.get_or_init(ReplaceLedger::default)
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
    /// The longest a streamed turn may run before the API gives up on it
    /// (`turn_timeout`). Tests shorten it; production keeps the default.
    fn turn_cap(&self) -> std::time::Duration {
        AGENT_TURN_CAP
    }
}

/// A hosted agent turn that hasn't finished in 10 minutes is failed
/// (`turn_timeout`) and its conversation freed. A cold computer wakes in well
/// under that.
pub const AGENT_TURN_CAP: std::time::Duration = std::time::Duration::from_secs(10 * 60);

pub struct ProdHost(pub Arc<ApiState>);

/// 503 from the relay or the runtime means the computer (or its agent runtime)
/// is starting. So does a 502/504 with no body: the computer's own proxy
/// answering while its API boots after a start or wake. A 5xx carrying an
/// error body is the runtime's own failure.
fn map_status(status: u16, body: &Value) -> Result<(), PlatformError> {
    match status {
        503 => Err(starting()),
        502 | 504 if body.is_null() => Err(starting()),
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
        // Read the runtime first: this runs on every agent call, and only a
        // missing one is created (create_free would also return the live one,
        // but a stuck one is only replaced under the backoff below).
        let provisioning = &self.0.provisioning_service;
        let existing = provisioning
            .list_for_user(&owner)
            .await
            .map_err(|e| unavailable(&format!("runtime {owner}: {e}")))?
            .into_iter()
            .find(|v| v.tier == "free" && v.replaced_by.is_none() && !matches!(v.status.as_str(), "error" | "deleted"));
        let mut view = match existing {
            Some(view) => view,
            None => provisioning.create_free(&owner).await.map_err(|e| provision_failed(&owner, e))?,
        };
        // A runtime that paired but was never seen again (its pairing failed after
        // the one-time code was used) stays offline forever: replace it, with
        // backoff, and never while a new one would be refused for capacity.
        if let Some(device) = view.device_id.as_deref() {
            let never: Option<(bool,)> = sqlx::query_as(
                // Never seen after it was created (a sleeping runtime has heartbeats behind it;
                // `relay_connected_at` alone is cleared on every disconnect).
                "SELECT relay_connected_at IS NULL AND COALESCE(last_seen_at, created_at) <= created_at + INTERVAL '1 minute' \
                 AND created_at < NOW() - INTERVAL '10 minutes' FROM runtime_devices WHERE id = $1",
            )
            .bind(device)
            .fetch_optional(&self.0.db)
            .await
            .map_err(|e| unavailable(&format!("runtime {device}: {e}")))?;
            if never.is_some_and(|(stuck,)| stuck)
                && provisioning.platform_runtime_admissible(&owner).await.unwrap_or(false)
                && replace_ledger().may_replace(project_id, view.created_at, Utc::now())
            {
                tracing::warn!(%owner, instance = %view.id, %device, "platform hosting: runtime never connected; replacing it");
                provisioning.delete(&view.id, &owner).await.map_err(|e| unavailable(&format!("replace {}: {e}", view.id)))?;
                view = provisioning.create_free(&owner).await.map_err(|e| provision_failed(&owner, e))?;
            }
        }
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
            .map_err(|e| relay_failed(&e))?;
        let value: Value = serde_json::from_slice(&out).unwrap_or(Value::Null);
        map_status(status, &value)?;
        Ok((status, value))
    }

    async fn stream(&self, rt: &HostRuntime, path: &str, body: &Value) -> Result<RelayStream, PlatformError> {
        let bytes = serde_json::to_vec(body).unwrap_or_default();
        let (status, stream) = ProdCallRelay { state: &self.0 }
            .stream(&rt.owner, &rt.runtime_id, path, &bytes)
            .await
            .map_err(|e| relay_failed(&e))?;
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
    fn a_booting_runtime_is_starting_and_its_own_errors_are_not() {
        let code = |s, b: Value| map_status(s, &b).err().map(|e| e.code);
        assert_eq!(code(503, Value::Null).as_deref(), Some("runtime_starting"));
        assert_eq!(code(502, Value::Null).as_deref(), Some("runtime_starting"));
        assert_eq!(code(504, Value::Null).as_deref(), Some("runtime_starting"));
        assert_eq!(code(502, serde_json::json!({ "error": "gizzi runtime refused session create" })).as_deref(), Some("runtime_unavailable"));
        assert_eq!(code(500, Value::Null).as_deref(), Some("runtime_unavailable"));
        assert_eq!(code(200, Value::Null), None);
    }

    #[test]
    fn a_never_connecting_runtime_is_replaced_once_per_window_and_at_most_three_times() {
        let ledger = ReplaceLedger::default();
        let t0 = Utc::now();
        // 20 calls, 10 s apart, against a runtime that never connects: only the
        // first (stuck for 11 minutes) deletes; the new row is too young after.
        let mut created = t0;
        let mut deletes = 0;
        for i in 0..20 {
            let now = t0 + Duration::minutes(11) + Duration::seconds(10 * i);
            if ledger.may_replace("proj_1", created, now) {
                deletes += 1;
                created = now;
            }
        }
        assert_eq!(deletes, 1);
        // Later windows: 15 then 45 minutes of age, then never again.
        assert!(!ledger.may_replace("proj_1", created, created + Duration::minutes(14)));
        assert!(ledger.may_replace("proj_1", created, created + Duration::minutes(15)));
        assert!(ledger.may_replace("proj_1", created, created + Duration::minutes(45)));
        assert!(!ledger.may_replace("proj_1", created, created + Duration::days(1)));
        // Another project has its own count.
        assert!(ledger.may_replace("proj_2", created, created + Duration::minutes(5)));
    }

    #[test]
    fn a_capacity_refusal_is_computer_capacity_not_unavailable() {
        use crate::error::ApiError;
        let full = ApiError::ServiceUnavailable(format!("{}: host memory is full", crate::services::provisioning::FREE_POOL_FULL));
        assert_eq!(provision_failed("platform:p", full).code, "computer_capacity");
        let other = ApiError::ServiceUnavailable("Fleet host h no longer exists".into());
        assert_eq!(provision_failed("platform:p", other).code, "runtime_unavailable");
        assert_eq!(relay_failed("Service unavailable: Free computers are at capacity right now").code, "computer_capacity");
        // The Orgo pool (running cap / daily budget) is capacity too, on both paths.
        let orgo_full = ApiError::ServiceUnavailable(format!("{}; retry in 30 seconds", crate::services::provisioning::CLOUD_POOL_FULL));
        assert_eq!(provision_failed("platform:p", orgo_full).code, "computer_capacity");
        assert_eq!(relay_failed("Service unavailable: Cloud computers are at capacity; retry in 30 seconds").code, "computer_capacity");
    }

    #[test]
    fn owners_are_per_project() {
        assert_eq!(runtime_owner("proj_1"), "platform:proj_1");
        assert_ne!(runtime_owner("proj_1"), runtime_owner("proj_2"));
    }
}
