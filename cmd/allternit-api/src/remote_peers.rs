//! Remote peer fabric — BOT_TEAMMATES_SPEC Phase 3 (AD-1: direct peer model).
//!
//! Extends the local Rails peer concept (`.allternit/peers/`, `/api/rails/peers`)
//! across machines. Each node keeps:
//!
//! - `remote-peers.json` — registered remote peers `{ name, url, key_ref }`.
//!   Keys are NEVER stored here; `key_ref` names an env var whose value lives
//!   in `peers.env` (gitignored, chmod 600) or the process environment.
//! - `peer-runs.json` — light persistence of the in-memory run map so a
//!   restart does not orphan claimed runs.
//! - `peer-roster.json` — union roster: local peers + every remote peer's
//!   peer list, with ghost retention (`source_reachable: false`) on poll
//!   failure and reconciliation on reconnect.
//!
//! Ops (matching `hermes peer`):
//! - `dm`   — synchronous: holds the HTTP connection until the remote turn
//!   finishes (or the 900s envelope TTL expires) and returns the reply.
//! - `run`  — asynchronous: returns `run_id` immediately; `status`/`stop`
//!   poll/control it.
//! - Both accept `--idempotency-key` semantics: a repeated key replays the
//!   original result instead of re-running.
//!
//! Failure vocabulary is plain string codes aligned with the surface's
//! failure-reasons concept (see `src/lib/bots/failure-reasons.ts`):
//! `runtime_offline` (definitively unreachable — fail fast),
//! `delivery_timeout` (TTL / request timeout), `peer_rejected` (remote 401),
//! `server_error` (remote 5xx), `peer_not_found`, `unauthorized_peer`,
//! `expired`.
//!
//! These routes mount on the PUBLIC router (see `main.rs`) because inbound
//! cross-machine traffic carries a peer key, not a Clerk JWT. Every handler
//! gates itself: management endpoints accept the desktop access-token header,
//! a verified Clerk JWT, or a registered peer key; `/peers/fabric/*`
//! endpoints accept only the registered sender's key (401 otherwise). Key
//! material is never logged; error strings are run through [`redact`].

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::AppState;

// ============================================================================
// Constants
// ============================================================================

/// Envelope / run TTL: a run that makes no progress in 900s is expired.
const ENVELOPE_TTL_SECS: u64 = 900;
/// Slow-poll interval for union-roster refresh.
const ROSTER_POLL_SECS: u64 = 300;
/// Fail-fast ceiling for establishing a connection to a remote peer.
const CONNECT_TIMEOUT_SECS: u64 = 3;
/// Overall per-request timeout ceiling (TTL + slack for a synchronous dm).
const REQUEST_TIMEOUT_SECS: u64 = ENVELOPE_TTL_SECS + 60;
/// Bus recipient used to correlate fabric replies with pending runs.
const FABRIC_REPLY_PEER: &str = "fabric-replies";
/// Terminal run records are kept in `peer-runs.json` for at most this long.
const TERMINAL_RETENTION_SECS: u64 = 3600;
/// Hard cap on persisted run records.
const MAX_PERSISTED_RUNS: usize = 200;

// ============================================================================
// Types
// ============================================================================

/// A registered remote peer. Key material is referenced, never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePeerEntry {
    pub name: String,
    /// Base URL of the remote node, e.g. `http://100.x.y.z:8013`.
    pub url: String,
    /// Name of the env var holding this peer's shared key.
    pub key_ref: String,
    pub added_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RemoteRegistryFile {
    #[serde(default)]
    peers: Vec<RemotePeerEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerOp {
    Dm,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Stopping,
    Done,
    Failed,
    Stopped,
    Expired,
}

impl RunStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            RunStatus::Done | RunStatus::Failed | RunStatus::Stopped | RunStatus::Expired
        )
    }
}

/// One row of the run map (in-memory, lightly persisted to `peer-runs.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRun {
    pub run_id: String,
    pub op: PeerOp,
    /// True when this node received the envelope (mirror of a remote op).
    #[serde(default)]
    pub inbound: bool,
    /// Remote peer name (outbound) or sender name (inbound).
    pub peer: String,
    pub status: RunStatus,
    /// Terminal payload: the reply text for a finished turn.
    pub result: Option<String>,
    /// Plain-string failure code (see module docs).
    pub reason: Option<String>,
    /// For outbound runs: the run id on the remote node.
    pub mirror_run_id: Option<String>,
    /// For inbound runs: where to POST the result when the local turn ends.
    pub reply_url: Option<String>,
    pub from_peer: Option<String>,
    pub created_at: String,
    pub expires_at: String,
    pub updated_at: String,
}

/// One row of the union roster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RosterRow {
    pub name: String,
    /// `local-peer` (this machine's rails peers), `remote-peer` (a registered
    /// connection), or `peer` (a peer reported by a remote source).
    pub kind: String,
    /// Where this row came from: `local` or the remote peer's name.
    pub source: String,
    pub url: Option<String>,
    pub source_reachable: bool,
    pub last_seen_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RosterFile {
    #[serde(default)]
    rows: Vec<RosterRow>,
}

/// Captured response for idempotency-key replay.
#[derive(Debug, Clone)]
struct IdemRecord {
    status: StatusCode,
    body: Value,
    expires_at: Instant,
}

/// Reachability of a registered remote peer as of the last poll/attempt.
#[derive(Debug, Clone, Default)]
struct PeerLiveness {
    last_poll_ok: bool,
    last_attempt_at: Option<String>,
}

// ============================================================================
// State
// ============================================================================

pub struct RemotePeersState {
    root_dir: PathBuf,
    registry_path: PathBuf,
    env_path: PathBuf,
    runs_path: PathBuf,
    roster_path: PathBuf,
    registry: Mutex<RemoteRegistryFile>,
    runs: Mutex<HashMap<String, PeerRun>>,
    roster: Mutex<RosterFile>,
    /// (peer, idempotency key) -> captured response.
    idempotency: Mutex<HashMap<(String, String), IdemRecord>>,
    liveness: Mutex<HashMap<String, PeerLiveness>>,
    http: reqwest::Client,
    background_started: std::sync::atomic::AtomicBool,
    reply_watcher_started: std::sync::atomic::AtomicBool,
}

/// Per-data-dir state registry (AppState itself cannot be extended from this
/// module without editing shared files, so state is lazily created and keyed
/// off `AppState::data_dir` — the same `OnceLock` pattern as `APP_CONFIG`).
static STATES: OnceLock<RwLock<HashMap<PathBuf, Arc<RemotePeersState>>>> = OnceLock::new();

pub fn remote_peers_state(root_dir: &FsPath) -> Arc<RemotePeersState> {
    let states = STATES.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(state) = states.read().unwrap().get(root_dir) {
        return state.clone();
    }
    let state = Arc::new(RemotePeersState::load(root_dir));
    states
        .write()
        .unwrap()
        .insert(root_dir.to_path_buf(), state.clone());
    state.spawn_background();
    state
}

impl RemotePeersState {
    fn load(root_dir: &FsPath) -> Self {
        let dir = root_dir.join(".allternit");
        let _ = std::fs::create_dir_all(&dir);
        let registry_path = dir.join("remote-peers.json");
        let env_path = dir.join("peers.env");
        let runs_path = dir.join("peer-runs.json");
        let roster_path = dir.join("peer-roster.json");

        let registry: RemoteRegistryFile = read_json(&registry_path);
        let roster: RosterFile = read_json(&roster_path);
        let mut runs: HashMap<String, PeerRun> = read_json(&runs_path);

        // Restart reconciliation: outbound runs we were holding can never
        // complete after a restart (the result callback is gone), so fail
        // them fast with `runtime_offline` instead of orphaning a claim.
        // Inbound runs survive: the bus is durable and the reply watcher
        // restarts with us, so only genuinely past-TTL runs are expired.
        let now = chrono::Utc::now();
        for run in runs.values_mut() {
            let past_ttl = run
                .expires_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .map(|exp| exp <= now)
                .unwrap_or(false);
            if past_ttl {
                run.status = RunStatus::Expired;
                run.reason = Some("expired".to_string());
                run.updated_at = now.to_rfc3339();
            } else if !run.inbound
                && matches!(run.status, RunStatus::Running | RunStatus::Queued | RunStatus::Stopping)
            {
                run.status = RunStatus::Failed;
                run.reason = Some("runtime_offline".to_string());
                run.updated_at = now.to_rfc3339();
            }
        }

        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("remote peers http client");

        let state = Self {
            root_dir: root_dir.to_path_buf(),
            registry_path,
            env_path,
            runs_path,
            roster_path,
            registry: Mutex::new(registry),
            runs: Mutex::new(runs),
            roster: Mutex::new(roster),
            idempotency: Mutex::new(HashMap::new()),
            liveness: Mutex::new(HashMap::new()),
            http,
            background_started: std::sync::atomic::AtomicBool::new(false),
            reply_watcher_started: std::sync::atomic::AtomicBool::new(false),
        };
        state.save_runs();
        state
    }

    /// Spawn the slow roster poller. Idempotent. The fabric-reply watcher is
    /// spawned separately (see `spawn_reply_watcher`) because it needs the
    /// RailsState, which only route handlers can reach.
    fn spawn_background(self: &Arc<Self>) {
        if self
            .background_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(Duration::from_secs(ROSTER_POLL_SECS));
            ticker.tick().await; // first tick fires immediately; skip it
            loop {
                ticker.tick().await;
                state.refresh_roster().await;
            }
        });
    }

    /// Watch the durable bus for replies addressed to `fabric-replies` and
    /// complete matching inbound runs, then POST the result to the originator.
    pub fn spawn_reply_watcher(self: &Arc<Self>, app_state: Arc<AppState>) {
        if self
            .reply_watcher_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let state = self.clone();
        tokio::spawn(async move {
            let rails = app_state.rails.clone();
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                state.drain_fabric_replies_with(&rails).await;
            }
        });
    }

    // ------------------------------------------------------------------
    // Registry
    // ------------------------------------------------------------------

    fn registry_peers(&self) -> Vec<RemotePeerEntry> {
        self.registry.lock().unwrap().peers.clone()
    }

    fn registry_get(&self, name: &str) -> Option<RemotePeerEntry> {
        self.registry
            .lock()
            .unwrap()
            .peers
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    fn registry_save(&self, file: &RemoteRegistryFile) -> Result<(), String> {
        write_json(&self.registry_path, file)
    }

    /// Resolve a peer's key: process env first, then `peers.env`.
    /// Never log the returned value.
    fn resolve_key(&self, key_ref: &str) -> Option<String> {
        if let Ok(value) = std::env::var(key_ref) {
            if !value.is_empty() {
                return Some(value);
            }
        }
        let contents = std::fs::read_to_string(&self.env_path).ok()?;
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key_ref {
                    return Some(v.trim().to_string());
                }
            }
        }
        None
    }

    fn peer_key(&self, entry: &RemotePeerEntry) -> Option<String> {
        self.resolve_key(&entry.key_ref)
    }

    /// Persist `key` into `peers.env` under `key_ref` (replacing any existing
    /// line), chmod 600. Never logs the value.
    fn write_env_key(&self, key_ref: &str, key: &str) -> Result<(), String> {
        let mut lines: Vec<String> = std::fs::read_to_string(&self.env_path)
            .map(|c| c.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default();
        let prefix = format!("{key_ref}=");
        lines.retain(|l| !l.starts_with(&prefix));
        lines.push(format!("{key_ref}={key}"));
        let tmp = self.env_path.with_extension("env.tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.env_path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.env_path,
                std::fs::Permissions::from_mode(0o600),
            );
        }
        Ok(())
    }

    /// All known key values (registry-resolved + peers.env), for redaction
    /// only. Never logs them.
    fn all_key_values(&self) -> Vec<String> {
        let mut values: Vec<String> = self
            .registry_peers()
            .iter()
            .filter_map(|p| self.peer_key(p))
            .collect();
        if let Ok(contents) = std::fs::read_to_string(&self.env_path) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((_, v)) = line.split_once('=') {
                    let v = v.trim();
                    if !v.is_empty() {
                        values.push(v.to_string());
                    }
                }
            }
        }
        values
    }

    /// Replace any occurrence of known key material with `[redacted]`.
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for key in self.all_key_values() {
            if key.len() >= 8 && out.contains(&key) {
                out = out.replace(&key, "[redacted]");
            }
        }
        out
    }

    fn liveness_get(&self, name: &str) -> PeerLiveness {
        self.liveness
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    fn liveness_set(&self, name: &str, ok: bool) {
        self.liveness.lock().unwrap().insert(
            name.to_string(),
            PeerLiveness {
                last_poll_ok: ok,
                last_attempt_at: Some(chrono::Utc::now().to_rfc3339()),
            },
        );
    }

    // ------------------------------------------------------------------
    // Runs
    // ------------------------------------------------------------------

    fn now_pair(ttl_secs: u64) -> (String, String) {
        let now = chrono::Utc::now();
        let exp = now + chrono::Duration::seconds(ttl_secs as i64);
        (now.to_rfc3339(), exp.to_rfc3339())
    }

    fn create_run(&self, run: PeerRun) {
        self.runs.lock().unwrap().insert(run.run_id.clone(), run);
        self.save_runs();
    }

    fn get_run(&self, run_id: &str) -> Option<PeerRun> {
        self.runs.lock().unwrap().get(run_id).cloned()
    }

    fn update_run(&self, run_id: &str, f: impl FnOnce(&mut PeerRun)) -> Option<PeerRun> {
        let mut runs = self.runs.lock().unwrap();
        let run = runs.get_mut(run_id)?;
        f(run);
        run.updated_at = chrono::Utc::now().to_rfc3339();
        let cloned = run.clone();
        drop(runs);
        self.save_runs();
        Some(cloned)
    }

    fn expire_due_runs(&self) {
        let now = chrono::Utc::now();
        let mut changed = false;
        {
            let mut runs = self.runs.lock().unwrap();
            for run in runs.values_mut() {
                if run.status.is_terminal() {
                    continue;
                }
                let past = run
                    .expires_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map(|exp| exp <= now)
                    .unwrap_or(false);
                if past {
                    run.status = RunStatus::Expired;
                    run.reason = Some("expired".to_string());
                    run.updated_at = now.to_rfc3339();
                    changed = true;
                }
            }
        }
        if changed {
            self.save_runs();
        }
    }

    fn save_runs(&self) {
        let runs = self.runs.lock().unwrap();
        let now = chrono::Utc::now();
        let mut persisted: Vec<PeerRun> = runs
            .values()
            .filter(|r| {
                !r.status.is_terminal()
                    || r.updated_at
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .map(|u| (now - u).num_seconds() < TERMINAL_RETENTION_SECS as i64)
                        .unwrap_or(false)
            })
            .take(MAX_PERSISTED_RUNS)
            .cloned()
            .collect();
        persisted.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let map: HashMap<String, PeerRun> =
            persisted.into_iter().map(|r| (r.run_id.clone(), r)).collect();
        let _ = write_json(&self.runs_path, &map);
    }

    /// Check the idempotency cache; on a hit return the captured response.
    fn idem_lookup(&self, peer: &str, key: &str) -> Option<(StatusCode, Value)> {
        let mut idem = self.idempotency.lock().unwrap();
        let id = (peer.to_string(), key.to_string());
        match idem.get(&id) {
            Some(rec) if rec.expires_at > Instant::now() => Some((rec.status, rec.body.clone())),
            Some(_) => {
                idem.remove(&id);
                None
            }
            None => None,
        }
    }

    fn idem_store(&self, peer: &str, key: &str, status: StatusCode, body: Value) {
        self.idempotency.lock().unwrap().insert(
            (peer.to_string(), key.to_string()),
            IdemRecord {
                status,
                body,
                expires_at: Instant::now() + Duration::from_secs(ENVELOPE_TTL_SECS),
            },
        );
    }

    // ------------------------------------------------------------------
    // Outbound ops
    // ------------------------------------------------------------------

    fn classify_send_error(err: &reqwest::Error) -> (StatusCode, Value) {
        if err.is_connect() {
            (
                StatusCode::BAD_GATEWAY,
                json!({ "error": "peer unreachable", "reason": "runtime_offline" }),
            )
        } else if err.is_timeout() {
            (
                StatusCode::GATEWAY_TIMEOUT,
                json!({ "error": "peer request timed out", "reason": "delivery_timeout" }),
            )
        } else {
            (
                StatusCode::BAD_GATEWAY,
                json!({ "error": "peer request failed", "reason": "server_error" }),
            )
        }
    }

    /// POST an envelope to the remote's fabric inbox. Returns Err((status, body))
    /// with a typed failure code; never leaks key material.
    async fn post_to_peer(
        &self,
        entry: &RemotePeerEntry,
        path: &str,
        body: &Value,
    ) -> Result<Value, (StatusCode, Value)> {
        let key = self.peer_key(entry).ok_or_else(|| {
            (
                StatusCode::PRECONDITION_FAILED,
                json!({ "error": format!("no key configured for peer '{}'", entry.name), "reason": "missing_config" }),
            )
        })?;
        let url = format!("{}{}", entry.url.trim_end_matches('/'), path);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&key)
            .json(body)
            .send()
            .await
            .map_err(|e| Self::classify_send_error(&e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }));
        if status.is_success() {
            return Ok(parsed);
        }
        let reason = parsed
            .get("reason")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if status == StatusCode::UNAUTHORIZED {
                    "peer_rejected".to_string()
                } else if status.as_u16() == 404 {
                    "peer_not_found".to_string()
                } else {
                    "server_error".to_string()
                }
            });
        Err((
            status,
            json!({ "error": self.redact(&text), "reason": reason }),
        ))
    }

    fn new_run(&self, op: PeerOp, peer: &str) -> PeerRun {
        let (created_at, expires_at) = Self::now_pair(ENVELOPE_TTL_SECS);
        PeerRun {
            run_id: format!("run_{}", uuid::Uuid::new_v4()),
            op,
            inbound: false,
            peer: peer.to_string(),
            status: RunStatus::Queued,
            result: None,
            reason: None,
            mirror_run_id: None,
            reply_url: None,
            from_peer: None,
            created_at: created_at.clone(),
            expires_at,
            updated_at: created_at,
        }
    }

    fn run_view(run: &PeerRun) -> Value {
        json!({
            "run_id": run.run_id,
            "op": run.op,
            "inbound": run.inbound,
            "peer": run.peer,
            "status": run.status,
            "result": run.result,
            "reason": run.reason,
            "created_at": run.created_at,
            "expires_at": run.expires_at,
            "updated_at": run.updated_at,
        })
    }

    /// Execute `dm` (sync) or `run` (async) against a remote peer, with
    /// idempotency-key replay. For `dm` this holds until the remote turn
    /// finishes, the run fails, or the TTL expires.
    async fn execute_op(
        self: &Arc<Self>,
        entry: RemotePeerEntry,
        op: PeerOp,
        message: String,
        to: Option<String>,
        idempotency_key: Option<String>,
    ) -> (StatusCode, Value) {
        self.expire_due_runs();

        if let Some(key) = idempotency_key.as_deref() {
            if let Some(captured) = self.idem_lookup(&entry.name, key) {
                debug!(peer = %entry.name, "idempotency replay");
                return captured;
            }
        }

        let mut run = self.new_run(op, &entry.name);
        run.status = RunStatus::Running;
        self.create_run(run.clone());

        // Self-URL the remote uses to POST the result back to us (`run` ops
        // only; `dm` results ride the held connection back). Unset when the
        // operator has not configured ALLTERNIT_PEER_URL — status polling
        // still works via the mirror proxy.
        let reply_url = self.self_url().map(|base| {
            format!(
                "{}/api/peers/fabric/runs/{}/result",
                base.trim_end_matches('/'),
                run.run_id
            )
        });

        let envelope = json!({
            "op": op,
            "message": message,
            "run_id": run.run_id,
            "from": { "name": peer_self_name(self) },
            "reply_url": reply_url,
            "ttl_secs": ENVELOPE_TTL_SECS,
            "idempotency_key": idempotency_key,
            "to": to,
        });

        let fabric_path = match &to {
            Some(local) => format!("/api/peers/fabric/send/{local}"),
            None => "/api/peers/fabric/send".to_string(),
        };
        let fabric = self.post_to_peer(&entry, &fabric_path, &envelope).await;

        let (status, body) = match fabric {
            Ok(accepted) => {
                let mirror = accepted
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                self.update_run(&run.run_id, |r| {
                    r.mirror_run_id = mirror;
                });
                match op {
                    PeerOp::Run => (
                        StatusCode::ACCEPTED,
                        json!({ "run_id": run.run_id, "status": "running" }),
                    ),
                    // For `dm` the remote holds the connection until its local
                    // turn finishes, so this response IS the outcome.
                    PeerOp::Dm => {
                        let remote_status = accepted
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("failed");
                        match remote_status {
                            "done" => {
                                let reply = accepted
                                    .get("reply")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                self.update_run(&run.run_id, |r| {
                                    r.status = RunStatus::Done;
                                    r.result = reply.clone();
                                });
                                (
                                    StatusCode::OK,
                                    json!({ "run_id": run.run_id, "status": "done", "reply": reply }),
                                )
                            }
                            "stopped" => {
                                self.update_run(&run.run_id, |r| {
                                    r.status = RunStatus::Stopped;
                                });
                                (
                                    StatusCode::OK,
                                    json!({ "run_id": run.run_id, "status": "stopped" }),
                                )
                            }
                            "expired" => {
                                self.update_run(&run.run_id, |r| {
                                    r.status = RunStatus::Expired;
                                    r.reason = Some("delivery_timeout".to_string());
                                });
                                (
                                    StatusCode::GATEWAY_TIMEOUT,
                                    json!({ "run_id": run.run_id, "status": "expired", "reason": "delivery_timeout" }),
                                )
                            }
                            _ => {
                                let reason = accepted
                                    .get("reason")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("server_error")
                                    .to_string();
                                self.update_run(&run.run_id, |r| {
                                    r.status = RunStatus::Failed;
                                    r.reason = Some(reason.clone());
                                });
                                (
                                    StatusCode::BAD_GATEWAY,
                                    json!({ "run_id": run.run_id, "status": "failed", "reason": reason }),
                                )
                            }
                        }
                    }
                }
            }
            Err((status, failure)) => {
                let reason = failure
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("server_error")
                    .to_string();
                self.update_run(&run.run_id, |r| {
                    r.status = RunStatus::Failed;
                    r.reason = Some(reason.clone());
                });
                (status, json!({ "run_id": run.run_id, "status": "failed", "reason": reason }))
            }
        };

        if let Some(key) = idempotency_key.as_deref() {
            self.idem_store(&entry.name, key, status, body.clone());
        }
        (status, body)
    }

    /// This node's externally reachable base URL, if configured.
    fn self_url(&self) -> Option<String> {
        std::env::var("ALLTERNIT_PEER_URL")
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
    }

    /// Hold the connection for a synchronous `dm` until the run reaches a
    /// terminal state or the envelope TTL passes.
    async fn hold_for_dm(self: &Arc<Self>, run_id: &str) -> (StatusCode, Value) {
        let deadline = self
            .get_run(run_id)
            .and_then(|r| r.expires_at.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::seconds(ENVELOPE_TTL_SECS as i64));
        loop {
            self.expire_due_runs();
            match self.get_run(run_id) {
                Some(run) if run.status.is_terminal() => {
                    return match run.status {
                        RunStatus::Done => (
                            StatusCode::OK,
                            json!({ "run_id": run.run_id, "status": "done", "reply": run.result }),
                        ),
                        RunStatus::Stopped => (
                            StatusCode::OK,
                            json!({ "run_id": run.run_id, "status": "stopped" }),
                        ),
                        RunStatus::Expired => (
                            StatusCode::GATEWAY_TIMEOUT,
                            json!({ "run_id": run.run_id, "status": "expired", "reason": "delivery_timeout" }),
                        ),
                        _ => (
                            StatusCode::BAD_GATEWAY,
                            json!({ "run_id": run.run_id, "status": "failed", "reason": run.reason }),
                        ),
                    };
                }
                Some(_) if chrono::Utc::now() >= deadline => {
                    let _run = self.update_run(run_id, |r| {
                        r.status = RunStatus::Expired;
                        r.reason = Some("delivery_timeout".to_string());
                    });
                    return (
                        StatusCode::GATEWAY_TIMEOUT,
                        json!({ "run_id": run_id, "status": "expired", "reason": "delivery_timeout" }),
                    );
                }
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        json!({ "error": "run not found", "reason": "peer_not_found" }),
                    );
                }
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Forward a `stop` to the remote mirror run (best-effort) and mark the
    /// local run stopped.
    async fn stop_run(self: &Arc<Self>, run_id: &str) -> (StatusCode, Value) {
        self.expire_due_runs();
        let run = match self.get_run(run_id) {
            Some(r) => r,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    json!({ "error": "run not found", "reason": "peer_not_found" }),
                )
            }
        };
        if run.status.is_terminal() {
            return (StatusCode::OK, Self::run_view(&run));
        }
        if let (Some(mirror), Some(entry)) = (run.mirror_run_id.clone(), self.registry_get(&run.peer)) {
            let _ = self
                .post_to_peer(
                    &entry,
                    &format!("/api/peers/fabric/runs/{}/stop", mirror),
                    &json!({ "from": { "name": peer_self_name(self) } }),
                )
                .await;
        }
        let run = self
            .update_run(run_id, |r| {
                r.status = RunStatus::Stopped;
            })
            .expect("run exists");
        (StatusCode::OK, Self::run_view(&run))
    }

    // ------------------------------------------------------------------
    // Roster
    // ------------------------------------------------------------------

    /// Fetch one remote's peer list and reconcile its roster rows.
    /// On failure, keep last-known rows and mark them unreachable (ghosts).
    pub async fn refresh_roster_for(self: &Arc<Self>, entry: &RemotePeerEntry) -> bool {
        let result = self
            .post_to_peer(entry, "/api/peers/fabric/peers", &json!({}))
            .await;
        match result {
            Ok(body) => {
                let peers = body
                    .get("peers")
                    .and_then(|p| p.as_array())
                    .cloned()
                    .unwrap_or_default();
                let now = chrono::Utc::now().to_rfc3339();
                let mut roster = self.roster.lock().unwrap();
                roster.rows.retain(|r| r.source != entry.name);
                for p in peers {
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if name.is_empty() {
                        continue;
                    }
                    roster.rows.push(RosterRow {
                        name: name.to_string(),
                        kind: "peer".to_string(),
                        source: entry.name.clone(),
                        url: Some(entry.url.clone()),
                        source_reachable: true,
                        last_seen_at: now.clone(),
                    });
                }
                drop(roster);
                let _ = write_json(&self.roster_path, &*self.roster.lock().unwrap());
                self.liveness_set(&entry.name, true);
                true
            }
            Err(_) => {
                // Ghost retention: keep last-known rows, mark unreachable;
                // reconciliation happens naturally on the next successful poll.
                let mut roster = self.roster.lock().unwrap();
                for row in roster.rows.iter_mut().filter(|r| r.source == entry.name) {
                    row.source_reachable = false;
                }
                drop(roster);
                let _ = write_json(&self.roster_path, &*self.roster.lock().unwrap());
                self.liveness_set(&entry.name, false);
                false
            }
        }
    }

    pub async fn refresh_roster(self: &Arc<Self>) {
        let peers = self.registry_peers();
        for entry in peers {
            self.refresh_roster_for(&entry).await;
        }
    }

    /// Union roster: live local peers + remote connections + fetched rows.
    fn union_roster(&self, app_state: &AppState) -> Vec<RosterRow> {
        let mut rows: Vec<RosterRow> = Vec::new();

        for peer in app_state.rails.peers.list() {
            rows.push(RosterRow {
                name: peer.name,
                kind: "local-peer".to_string(),
                source: "local".to_string(),
                url: None,
                source_reachable: peer.status != allternit_factory_engine::PeerStatus::Dead,
                last_seen_at: peer.last_heartbeat_at,
            });
        }
        for entry in self.registry_peers() {
            let live = self.liveness_get(&entry.name);
            rows.push(RosterRow {
                name: entry.name.clone(),
                kind: "remote-peer".to_string(),
                source: "local".to_string(),
                url: Some(entry.url.clone()),
                source_reachable: live.last_poll_ok,
                last_seen_at: live
                    .last_attempt_at
                    .clone()
                    .unwrap_or_else(|| entry.added_at.clone()),
            });
        }
        rows.extend(self.roster.lock().unwrap().rows.clone());
        rows
    }

    // ------------------------------------------------------------------
    // Fabric replies (inbound runs completing on this node)
    // ------------------------------------------------------------------

    /// Watch the durable bus for replies addressed to `fabric-replies` and
    /// complete matching inbound runs, then POST the result to the originator.
    pub async fn drain_fabric_replies_with(self: &Arc<Self>, rails: &crate::rails::RailsState) {
        let messages = match rails
            .bus
            .poll_pending_for(&format!("peer:{FABRIC_REPLY_PEER}"), Some("http"), 20)
            .await
        {
            Ok(msgs) => msgs,
            Err(_) => return,
        };
        for msg in messages {
            let _ = rails.bus.mark_delivered(msg.id).await;
            let body = msg
                .payload
                .get("body")
                .and_then(|b| b.as_str())
                .unwrap_or("");
            // Reply contract: body starts with `@run <run_id>`.
            let Some(rest) = body.strip_prefix("@run ") else {
                continue;
            };
            let (run_id, reply) = match rest.split_once(' ') {
                Some((id, text)) => (id.to_string(), text.to_string()),
                None => (rest.to_string(), String::new()),
            };
            let run = match self.get_run(&run_id) {
                Some(r) => r,
                None => continue,
            };
            if !run.inbound || run.status.is_terminal() {
                continue;
            }
            let run = self
                .update_run(&run_id, |r| {
                    r.status = RunStatus::Done;
                    r.result = Some(reply.clone());
                })
                .expect("run exists");
            // Notify the originator (async `run` ops only — `dm` results ride
            // the held fabric connection back).
            if run.op != PeerOp::Run {
                continue;
            }
            if let (Some(reply_url), Some(from_peer)) = (run.reply_url.clone(), run.from_peer.clone())
            {
                if let Some(entry) = self.registry_get(&from_peer) {
                    let key = match self.peer_key(&entry) {
                        Some(k) => k,
                        None => continue,
                    };
                    let payload = json!({
                        "run_id": run.mirror_run_id,
                        "status": "done",
                        "result": reply,
                    });
                    let _ = self
                        .http
                        .post(&reply_url)
                        .bearer_auth(&key)
                        .json(&payload)
                        .send()
                        .await;
                }
            }
        }
    }
}

/// Name this node presents to remote peers. Defaults to the machine hostname;
/// override with `ALLTERNIT_PEER_NAME`. Distinct from local rails peer names
/// (session-scoped) — this is the node's stable fabric identity.
fn peer_self_name(state: &RemotePeersState) -> String {
    if let Ok(name) = std::env::var("ALLTERNIT_PEER_NAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    format!("node-{}", short_id(&state.root_dir))
}

fn short_id(path: &FsPath) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hex::encode(&hasher.finalize())[..8].to_string()
}

// ============================================================================
// Persistence helpers
// ============================================================================

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &FsPath) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_json(path: &FsPath, value: &impl Serialize) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let serialized = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, serialized).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// ============================================================================
// Router
// ============================================================================

pub fn remote_peers_router() -> Router<Arc<AppState>> {
    Router::new()
        // Management (self-gated: desktop token / Clerk JWT / peer key).
        .route("/peers/remote", get(list_remote_peers).post(register_remote_peer))
        .route("/peers/remote/:name", delete(remove_remote_peer))
        .route("/peers/roster", get(get_roster))
        .route("/peers/remote/:name/dm", post(dm_remote_peer))
        .route("/peers/remote/:name/run", post(run_remote_peer))
        .route("/peers/runs/:run_id", get(get_run_status))
        .route("/peers/runs/:run_id/stop", post(stop_run_handler))
        // Fabric (registered-sender key only).
        .route("/peers/fabric/send/:local_name", post(fabric_send))
        .route("/peers/fabric/send", post(fabric_send_unnamed))
        .route("/peers/fabric/peers", get(fabric_peers).post(fabric_peers))
        .route("/peers/fabric/runs/:run_id", get(fabric_run_status).post(fabric_run_status))
        .route("/peers/fabric/runs/:run_id/stop", post(fabric_run_stop))
        .route("/peers/fabric/runs/:run_id/result", post(fabric_run_result))
}

fn fabric_state(app_state: &Arc<AppState>) -> Arc<RemotePeersState> {
    let state = remote_peers_state(&app_state.data_dir);
    state.spawn_reply_watcher(app_state.clone());
    state
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

fn unauthorized(reason: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": reason, "reason": "unauthorized_peer" })),
    )
        .into_response()
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|v| v.to_string())
}

fn key_matches(provided: &str, expected: &str) -> bool {
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Fabric endpoints: the bearer must match the key registered for the named
/// sender. Unknown or mismatched senders get 401.
fn authorize_fabric(
    fp_state: &RemotePeersState,
    headers: &HeaderMap,
    from_name: &str,
) -> Result<RemotePeerEntry, Response> {
    let entry = fp_state.registry_get(from_name);
    let provided = extract_bearer(headers);
    match (entry, provided) {
        (Some(entry), Some(token)) => match fp_state.peer_key(&entry) {
            Some(key) if key_matches(&token, &key) => Ok(entry),
            _ => Err(unauthorized("unknown or invalid peer key")),
        },
        _ => Err(unauthorized("unknown sender")),
    }
}

/// Management endpoints: desktop access-token header, registered-peer key, or
/// verified Clerk JWT. Anything else is 401.
async fn authorize_management(
    app_state: &AppState,
    fp_state: &RemotePeersState,
    headers: &HeaderMap,
) -> Result<(), Response> {
    // 1. Desktop bootstrap shared secret (Electron-brokered loopback calls).
    if let (Some(configured), Some(provided)) = (
        app_state.config.desktop_access_token(),
        headers
            .get("x-allternit-desktop-access-token")
            .and_then(|v| v.to_str().ok()),
    ) {
        if key_matches(provided, &configured) {
            return Ok(());
        }
    }
    // 2. Registered-peer bearer key (node-to-node management).
    if let Some(token) = extract_bearer(headers) {
        for entry in fp_state.registry_peers() {
            if let Some(key) = fp_state.peer_key(&entry) {
                if key_matches(&token, &key) {
                    return Ok(());
                }
            }
        }
        // 3. Clerk JWT (web surface).
        if crate::auth::verify_token(&app_state.jwks, &token, &app_state.auth_config)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(unauthorized("management endpoint requires a user or peer credential"))
}

// ---------------------------------------------------------------------------
// Management handlers
// ---------------------------------------------------------------------------

async fn list_remote_peers(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let peers: Vec<Value> = fp
        .registry_peers()
        .iter()
        .map(|p| {
            let live = fp.liveness_get(&p.name);
            json!({
                "name": p.name,
                "url": p.url,
                "keyRef": p.key_ref,
                "addedAt": p.added_at,
                "reachable": live.last_poll_ok,
                "lastAttemptAt": live.last_attempt_at,
            })
        })
        .collect();
    (StatusCode::OK, Json(json!({ "peers": peers }))).into_response()
}

#[derive(Debug, Deserialize)]
struct RegisterRemotePeerRequest {
    name: String,
    url: String,
    #[serde(rename = "keyRef")]
    key_ref: Option<String>,
    /// Optional: the key value to persist into `peers.env` under `keyRef`.
    /// Accepted only from management-authenticated callers; never echoed back.
    key: Option<String>,
}

async fn register_remote_peer(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<RegisterRemotePeerRequest>,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let name = sanitize_peer_name(&request.name);
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "peer name has no usable characters" })),
        )
            .into_response();
    }
    let url = request.url.trim().trim_end_matches('/').to_string();
    if !valid_peer_url(&url) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "url must be http:// or https://" })),
        )
            .into_response();
    }
    let key_ref = request
        .key_ref
        .clone()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| format!("ALLTERNIT_PEER_{}_KEY", env_key_suffix(&name)));
    if let Some(key) = request.key.as_deref().filter(|k| !k.is_empty()) {
        if let Err(e) = fp.write_env_key(&key_ref, key) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": fp.redact(&e) })),
            )
                .into_response();
        }
    }
    if fp.resolve_key(&key_ref).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("key '{key_ref}' is not set (pass `key` or set the env var)"), "reason": "missing_config" })),
        )
            .into_response();
    }

    let entry = RemotePeerEntry {
        name: name.clone(),
        url,
        key_ref,
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    {
        let mut registry = fp.registry.lock().unwrap();
        registry.peers.retain(|p| p.name != name);
        registry.peers.push(entry.clone());
        if let Err(e) = fp.registry_save(&registry) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": fp.redact(&e) })),
            )
                .into_response();
        }
    }
    info!(peer = %name, url = %entry.url, "registered remote peer");

    // Kick a roster refresh so the union fills in immediately.
    let fp2 = fp.clone();
    let entry2 = entry.clone();
    tokio::spawn(async move {
        fp2.refresh_roster_for(&entry2).await;
    });

    (
        StatusCode::CREATED,
        Json(json!({
            "name": entry.name,
            "url": entry.url,
            "keyRef": entry.key_ref,
            "addedAt": entry.added_at,
        })),
    )
        .into_response()
}

async fn remove_remote_peer(
    State(app_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let removed = {
        let mut registry = fp.registry.lock().unwrap();
        let before = registry.peers.len();
        registry.peers.retain(|p| p.name != name);
        let removed = registry.peers.len() != before;
        if removed {
            let _ = fp.registry_save(&registry);
        }
        removed
    };
    if removed {
        // Drop the source's roster rows with it.
        let mut roster = fp.roster.lock().unwrap();
        roster.rows.retain(|r| r.source != name);
        let _ = write_json(&fp.roster_path, &*roster);
        (StatusCode::OK, Json(json!({ "removed": true }))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "peer not found", "reason": "peer_not_found" })),
        )
            .into_response()
    }
}

async fn get_roster(State(app_state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let rows = fp.union_roster(&app_state);
    (StatusCode::OK, Json(json!({ "roster": rows }))).into_response()
}

#[derive(Debug, Deserialize)]
struct OpRequest {
    message: String,
    /// Local rails peer on the REMOTE node that receives the envelope
    /// (`/api/rails/peers/<local-name>/send`-compatible). Defaults to the
    /// remote's first live local peer.
    to: Option<String>,
    #[serde(rename = "idempotencyKey")]
    idempotency_key: Option<String>,
}

async fn dm_remote_peer(
    State(app_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<OpRequest>,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let entry = match fp.registry_get(&name) {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "peer not found", "reason": "peer_not_found" })),
            )
                .into_response()
        }
    };
    let (status, body) = fp
        .execute_op(entry, PeerOp::Dm, request.message, request.to, request.idempotency_key)
        .await;
    (status, Json(body)).into_response()
}

async fn run_remote_peer(
    State(app_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<OpRequest>,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let entry = match fp.registry_get(&name) {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "peer not found", "reason": "peer_not_found" })),
            )
                .into_response()
        }
    };
    let (status, body) = fp
        .execute_op(entry, PeerOp::Run, request.message, request.to, request.idempotency_key)
        .await;
    (status, Json(body)).into_response()
}

async fn get_run_status(
    State(app_state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    fp.expire_due_runs();
    let run = match fp.get_run(&run_id) {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "run not found", "reason": "peer_not_found" })),
            )
                .into_response()
        }
    };
    // Proxy live status from the remote mirror while still running.
    if !run.status.is_terminal() {
        if let (Some(mirror), Some(entry)) = (run.mirror_run_id.clone(), fp.registry_get(&run.peer)) {
            if let Ok(view) = fp
                .post_to_peer(&entry, &format!("/api/peers/fabric/runs/{mirror}"), &json!({}))
                .await
            {
                return (StatusCode::OK, Json(view)).into_response();
            }
        }
    }
    (StatusCode::OK, Json(RemotePeersState::run_view(&run))).into_response()
}

async fn stop_run_handler(
    State(app_state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    if let Err(resp) = authorize_management(&app_state, &fp, &headers).await {
        return resp;
    }
    let (status, body) = fp.stop_run(&run_id).await;
    (status, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Fabric handlers (inbound, registered-sender key only)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FabricSendRequest {
    op: PeerOp,
    message: String,
    run_id: String,
    from: FabricFrom,
    reply_url: Option<String>,
    #[serde(rename = "ttlSecs")]
    ttl_secs: Option<u64>,
    #[serde(rename = "idempotencyKey")]
    idempotency_key: Option<String>,
    /// Fallback local peer name when the path parameter is omitted.
    to: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FabricFrom {
    name: String,
}

async fn fabric_send_unnamed(
    state: State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<FabricSendRequest>,
) -> Response {
    fabric_send_inner(state, None, headers, request).await
}

async fn fabric_send(
    State(app_state): State<Arc<AppState>>,
    Path(local_name): Path<String>,
    headers: HeaderMap,
    Json(request): Json<FabricSendRequest>,
) -> Response {
    fabric_send_inner(State(app_state), Some(local_name), headers, request).await
}

async fn fabric_send_inner(
    State(app_state): State<Arc<AppState>>,
    local_name: Option<String>,
    headers: HeaderMap,
    request: FabricSendRequest,
) -> Response {
    let fp = fabric_state(&app_state);
    let entry = match authorize_fabric(&fp, &headers, &request.from.name) {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    let _ = entry;

    // Idempotency replay at the fabric edge too: a retried envelope returns
    // the original acceptance instead of re-delivering to the local peer.
    if let Some(key) = request.idempotency_key.as_deref() {
        if let Some((status, body)) = fp.idem_lookup(&request.from.name, key) {
            return (status, Json(body)).into_response();
        }
    }

    // Resolve the target local peer (rails inbox delivery, send-compatible):
    // explicit name, else the first live local peer.
    let local_name = match local_name
        .or_else(|| request.to.clone())
        .or_else(|| {
            app_state
                .rails
                .peers
                .list()
                .into_iter()
                .find(|p| p.status != allternit_factory_engine::PeerStatus::Dead)
                .map(|p| p.name)
        }) {
        Some(name) => name,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no local peer available", "reason": "peer_not_found" })),
            )
                .into_response()
        }
    };
    let peer = match app_state.rails.peers.resolve(&local_name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "peer not found", "reason": "peer_not_found" })),
            )
                .into_response()
        }
    };

    let ttl = request.ttl_secs.unwrap_or(ENVELOPE_TTL_SECS).min(ENVELOPE_TTL_SECS);
    let (created_at, expires_at) = RemotePeersState::now_pair(ttl);
    let inbound_op = request.op;
    let inbound = PeerRun {
        run_id: format!("in_{}", uuid::Uuid::new_v4()),
        op: inbound_op,
        inbound: true,
        peer: request.from.name.clone(),
        status: RunStatus::Running,
        result: None,
        reason: None,
        mirror_run_id: Some(request.run_id.clone()),
        reply_url: request.reply_url.clone(),
        from_peer: Some(request.from.name.clone()),
        created_at: created_at.clone(),
        expires_at,
        updated_at: created_at,
    };
    let inbound_id = inbound.run_id.clone();
    fp.create_run(inbound);

    // Deliver to the local peer's durable inbox. The payload carries the
    // reply contract: reply via SendMessage to peer `fabric-replies` with
    // body `@run <id> <reply>`.
    let delivered_body = format!(
        "{}\n\n[fabric:{} run {} — reply via SendMessage to peer '{}' with body `@run {} <reply>`]",
        request.message,
        request.from.name,
        inbound_id,
        FABRIC_REPLY_PEER,
        inbound_id,
    );
    let bus_msg = allternit_factory_engine::bus::NewBusMessage {
        correlation_id: uuid::Uuid::new_v4().to_string(),
        to: format!("peer:{}", peer.name),
        from: format!("remote:{}", request.from.name),
        kind: "peer_envelope".to_string(),
        payload: json!({ "body": delivered_body, "from": format!("remote:{}", request.from.name) }),
        transport: "http".to_string(),
    };
    let mut delivered = false;
    match app_state.rails.bus.send_message(bus_msg).await {
        Ok(_) => delivered = true,
        Err(e) => {
            warn!(peer = %peer.name, "fabric envelope bus persist failed: {}", fp.redact(&e.to_string()));
        }
    }
    if !delivered {
        fp.update_run(&inbound_id, |r| {
            r.status = RunStatus::Failed;
            r.reason = Some("server_error".to_string());
        });
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to persist envelope", "reason": "server_error" })),
        )
            .into_response();
    }

    match inbound_op {
        // Synchronous op: hold the fabric connection until the local turn
        // finishes (the reply watcher completes the run when the local peer
        // replies) or the envelope TTL expires — this response IS the reply.
        PeerOp::Dm => {
            let (status, body) = fp.hold_for_dm(&inbound_id).await;
            if let Some(key) = request.idempotency_key.as_deref() {
                fp.idem_store(&request.from.name, key, status, body.clone());
            }
            (status, Json(body)).into_response()
        }
        // Asynchronous op: accept immediately; the result arrives later via
        // the reply watcher and the originator's `reply_url` callback.
        PeerOp::Run => {
            let body = json!({ "accepted": true, "run_id": inbound_id });
            if let Some(key) = request.idempotency_key.as_deref() {
                fp.idem_store(&request.from.name, key, StatusCode::ACCEPTED, body.clone());
            }
            (StatusCode::ACCEPTED, Json(body)).into_response()
        }
    }
}

async fn fabric_peers(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    // Any registered peer key may read the peer list (roster propagation).
    let authorized = match extract_bearer(&headers) {
        Some(token) => fp.registry_peers().iter().any(|entry| {
            fp.peer_key(entry)
                .map(|key| key_matches(&token, &key))
                .unwrap_or(false)
        }),
        None => false,
    };
    if !authorized {
        return unauthorized("unknown sender");
    }
    let peers: Vec<Value> = app_state
        .rails
        .peers
        .list()
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "vendor": p.vendor,
                "status": p.status,
            })
        })
        .collect();
    (StatusCode::OK, Json(json!({ "peers": peers }))).into_response()
}

async fn fabric_run_status(
    State(app_state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let fp = fabric_state(&app_state);
    // Status polling is part of the op the sender already authenticated; any
    // registered peer key may poll (the run id is an unguessable UUIDv4).
    let authorized = match extract_bearer(&headers) {
        Some(token) => fp.registry_peers().iter().any(|entry| {
            fp.peer_key(entry)
                .map(|key| key_matches(&token, &key))
                .unwrap_or(false)
        }),
        None => false,
    };
    if !authorized {
        return unauthorized("unknown sender");
    }
    fp.expire_due_runs();
    match fp.get_run(&run_id) {
        Some(run) => (StatusCode::OK, Json(RemotePeersState::run_view(&run))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "run not found", "reason": "peer_not_found" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct FabricStopRequest {
    from: Option<FabricFrom>,
}

async fn fabric_run_stop(
    State(app_state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<FabricStopRequest>>,
) -> Response {
    let fp = fabric_state(&app_state);
    let from_name = body
        .and_then(|Json(req)| req.from)
        .map(|f| f.name)
        .unwrap_or_default();
    if authorize_fabric(&fp, &headers, &from_name).is_err() {
        return unauthorized("unknown sender");
    }
    fp.expire_due_runs();
    match fp.update_run(&run_id, |r| {
        if !r.status.is_terminal() {
            r.status = RunStatus::Stopped;
        }
    }) {
        Some(run) => (StatusCode::OK, Json(RemotePeersState::run_view(&run))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "run not found", "reason": "peer_not_found" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct FabricResultRequest {
    run_id: Option<String>,
    status: Option<String>,
    result: Option<String>,
    reason: Option<String>,
}

async fn fabric_run_result(
    State(app_state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<FabricResultRequest>,
) -> Response {
    let fp = fabric_state(&app_state);
    // The result callback names the run it is completing; authenticate the
    // sender the same way as status polling (unguessable run id + peer key).
    let authorized = match extract_bearer(&headers) {
        Some(token) => fp.registry_peers().iter().any(|entry| {
            fp.peer_key(entry)
                .map(|key| key_matches(&token, &key))
                .unwrap_or(false)
        }),
        None => false,
    };
    if !authorized {
        return unauthorized("unknown sender");
    }
    if let Some(id) = request.run_id.as_deref() {
        if id != run_id {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "run id mismatch", "reason": "invalid_response" })),
            )
                .into_response();
        }
    }
    let status = match request.status.as_deref() {
        Some("done") => RunStatus::Done,
        Some("stopped") => RunStatus::Stopped,
        Some("failed") => RunStatus::Failed,
        _ => RunStatus::Done,
    };
    let reason = request.reason.clone();
    match fp.update_run(&run_id, |r| {
        if !r.status.is_terminal() {
            r.status = status;
            r.result = request.result.clone();
            r.reason = reason.clone();
        }
    }) {
        Some(run) => (StatusCode::OK, Json(RemotePeersState::run_view(&run))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "run not found", "reason": "peer_not_found" })),
        )
            .into_response(),
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn sanitize_peer_name(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
}

fn env_key_suffix(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

fn valid_peer_url(url: &str) -> bool {
    match url::Url::parse(url) {
        Ok(u) => matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        Err(_) => false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "allternit-remote-peers-{}-{}",
            tag,
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Direct (non-global) state for pure state-level tests.
    fn load_state(dir: &FsPath) -> RemotePeersState {
        RemotePeersState::load(dir)
    }

    fn sample_entry(name: &str, url: &str, key_ref: &str) -> RemotePeerEntry {
        RemotePeerEntry {
            name: name.to_string(),
            url: url.to_string(),
            key_ref: key_ref.to_string(),
            added_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn post(
        app: &Router,
        uri: &str,
        body: Value,
        bearer: Option<&str>,
    ) -> axum::response::Response {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = bearer {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn get(app: &Router, uri: &str, bearer: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri(uri);
        if let Some(token) = bearer {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn delete(app: &Router, uri: &str, bearer: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().method("DELETE").uri(uri);
        if let Some(token) = bearer {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_json(resp: axum::response::Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
        (status, value)
    }

    /// Write a remote-peer registry + peers.env under `dir` so the global
    /// handler-created state picks it up on first access.
    fn seed_peer_files(dir: &FsPath, entries: &[RemotePeerEntry], keys: &[(&str, &str)]) {
        let allternit = dir.join(".allternit");
        std::fs::create_dir_all(&allternit).unwrap();
        let file = RemoteRegistryFile {
            peers: entries.to_vec(),
        };
        std::fs::write(
            allternit.join("remote-peers.json"),
            serde_json::to_string_pretty(&file).unwrap(),
        )
        .unwrap();
        let env = keys
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(allternit.join("peers.env"), env + "\n").unwrap();
    }

    // ------------------------------------------------------------------
    // State-level tests
    // ------------------------------------------------------------------

    #[test]
    fn registry_env_roundtrip_and_redaction() {
        let dir = temp_dir("redact");
        let state = load_state(&dir);
        let entry = sample_entry("bob", "http://127.0.0.1:9999", "ALLTERNIT_PEER_BOB_KEY");
        state
            .write_env_key(&entry.key_ref, "super-secret-key-material")
            .unwrap();

        // Key resolves from peers.env, never stored in the registry.
        assert_eq!(
            state.resolve_key(&entry.key_ref).as_deref(),
            Some("super-secret-key-material")
        );
        let persisted: RemoteRegistryFile = read_json(&state.registry_path);
        assert!(persisted.peers.is_empty()); // registry untouched by env write

        // Redaction scrubs key material from arbitrary strings.
        let leaked = format!("auth failed for bob with key super-secret-key-material");
        assert_eq!(state.redact(&leaked), "auth failed for bob with key [redacted]");
    }

    #[test]
    fn idempotency_record_replays_and_expires() {
        let dir = temp_dir("idem");
        let state = load_state(&dir);
        let body = json!({ "run_id": "run_x", "status": "done", "reply": "hi" });
        state.idem_store("bob", "key-1", StatusCode::OK, body.clone());
        let (status, replayed) = state.idem_lookup("bob", "key-1").expect("replay hit");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replayed, body);
        // Different peer/key: miss.
        assert!(state.idem_lookup("alice", "key-1").is_none());
        assert!(state.idem_lookup("bob", "key-2").is_none());
        // Expired records are evicted on lookup.
        state.idempotency.lock().unwrap().insert(
            ("bob".to_string(), "stale".to_string()),
            IdemRecord {
                status: StatusCode::OK,
                body: json!({}),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert!(state.idem_lookup("bob", "stale").is_none());
    }

    #[test]
    fn ttl_expiry_marks_run_expired() {
        let dir = temp_dir("ttl");
        let state = load_state(&dir);
        let past = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
        let run = PeerRun {
            run_id: "run_ttl".to_string(),
            op: PeerOp::Run,
            inbound: false,
            peer: "bob".to_string(),
            status: RunStatus::Running,
            result: None,
            reason: None,
            mirror_run_id: None,
            reply_url: None,
            from_peer: None,
            created_at: past.clone(),
            expires_at: past.clone(),
            updated_at: past,
        };
        state.create_run(run);
        state.expire_due_runs();
        let expired = state.get_run("run_ttl").unwrap();
        assert_eq!(expired.status, RunStatus::Expired);
        assert_eq!(expired.reason.as_deref(), Some("expired"));
    }

    #[test]
    fn restart_reconciles_orphaned_outbound_runs() {
        let dir = temp_dir("restart");
        let allternit = dir.join(".allternit");
        std::fs::create_dir_all(&allternit).unwrap();
        let (_, exp) = RemotePeersState::now_pair(ENVELOPE_TTL_SECS);
        let run = PeerRun {
            run_id: "run_orphan".to_string(),
            op: PeerOp::Dm,
            inbound: false,
            peer: "bob".to_string(),
            status: RunStatus::Running,
            result: None,
            reason: None,
            mirror_run_id: Some("in_x".to_string()),
            reply_url: None,
            from_peer: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            expires_at: exp,
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let mut map = HashMap::new();
        map.insert(run.run_id.clone(), run);
        std::fs::write(
            allternit.join("peer-runs.json"),
            serde_json::to_string_pretty(&map).unwrap(),
        )
        .unwrap();

        let state = load_state(&dir);
        let reconciled = state.get_run("run_orphan").unwrap();
        assert_eq!(reconciled.status, RunStatus::Failed);
        assert_eq!(reconciled.reason.as_deref(), Some("runtime_offline"));
    }

    // ------------------------------------------------------------------
    // Handler-level tests
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn fabric_unknown_sender_rejected_401() {
        let dir = temp_dir("unknown-sender");
        let app_state = crate::test_helpers::app_state(&dir).await;
        seed_peer_files(
            &dir,
            &[sample_entry("alice", "http://127.0.0.1:1", "K_ALICE")],
            &[("K_ALICE", "alice-key")],
        );
        let app = remote_peers_router().with_state(app_state);
        let envelope = json!({
            "op": "dm",
            "message": "hello",
            "run_id": "run_1",
            "from": { "name": "mallory" },
        });
        // Unknown sender name.
        let (status, _) = body_json(post(&app, "/peers/fabric/send/gizmo", envelope.clone(), Some("alice-key")).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Known sender, wrong key.
        let envelope_ok = json!({
            "op": "dm",
            "message": "hello",
            "run_id": "run_1",
            "from": { "name": "alice" },
        });
        let (status, body) = body_json(post(&app, "/peers/fabric/send/gizmo", envelope_ok.clone(), Some("wrong-key")).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["reason"], "unauthorized_peer");
        // No bearer at all.
        let (status, _) = body_json(post(&app, "/peers/fabric/send/gizmo", envelope_ok, None).await).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remote_registry_crud_and_missing_key_rejected() {
        let dir = temp_dir("crud");
        let app_state = crate::test_helpers::app_state(&dir).await;
        seed_peer_files(&dir, &[], &[]);
        let app = remote_peers_router().with_state(app_state);
        // Management calls authenticate with any registered peer key; seed one.
        seed_peer_files(
            &dir,
            &[sample_entry("authpeer", "http://127.0.0.1:1", "K_AUTH")],
            &[("K_AUTH", "auth-key")],
        );
        let auth = Some("auth-key");

        // Missing key: 400 missing_config.
        let (status, body) = body_json(
            post(
                &app,
                "/peers/remote",
                json!({ "name": "bob", "url": "http://127.0.0.1:9999", "keyRef": "K_BOB" }),
                auth,
            )
            .await,
        ).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["reason"], "missing_config");

        // Register with inline key (written to peers.env, never echoed).
        let (status, body) = body_json(
            post(
                &app,
                "/peers/remote",
                json!({ "name": "bob", "url": "http://127.0.0.1:9999", "key": "shared-bob-key" }),
                auth,
            )
            .await,
        ).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["keyRef"], "ALLTERNIT_PEER_BOB_KEY");
        assert!(body.get("key").is_none());

        // List: keyRef only, no key material.
        let (status, body) = body_json(get(&app, "/peers/remote", auth).await).await;
        assert_eq!(status, StatusCode::OK);
        let bob = body["peers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "bob")
            .expect("bob listed");
        assert_eq!(bob["keyRef"], "ALLTERNIT_PEER_BOB_KEY");
        assert!(bob.to_string().find("shared-bob-key").is_none());

        // Roster includes the connection.
        let (status, body) = body_json(get(&app, "/peers/roster", auth).await).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["roster"].as_array().unwrap().iter().any(|r| r["name"] == "bob"));

        // Delete.
        let (status, _) = body_json(delete(&app, "/peers/remote/bob", auth).await).await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = body_json(get(&app, "/peers/remote", auth).await).await;
        assert!(!body["peers"].as_array().unwrap().iter().any(|p| p["name"] == "bob"));
    }

    #[tokio::test]
    async fn dm_unreachable_peer_fails_fast_runtime_offline() {
        let dir = temp_dir("offline");
        let app_state = crate::test_helpers::app_state(&dir).await;
        // 127.0.0.1:1 is reserved/unbound — connection refused → fail fast.
        seed_peer_files(
            &dir,
            &[sample_entry("bob", "http://127.0.0.1:1", "K_BOB")],
            &[("K_BOB", "bob-key")],
        );
        let app = remote_peers_router().with_state(app_state);
        let (status, body) = body_json(
            post(
                &app,
                "/peers/remote/bob/dm",
                json!({ "message": "ping", "idempotencyKey": "dm-1" }),
                Some("bob-key"),
            )
            .await,
        ).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["reason"], "runtime_offline");
        assert_eq!(body["status"], "failed");

        // Idempotency replay: same key returns the original failure verbatim.
        let (status2, body2) = body_json(
            post(
                &app,
                "/peers/remote/bob/dm",
                json!({ "message": "ping", "idempotencyKey": "dm-1" }),
                Some("bob-key"),
            )
            .await,
        ).await;
        assert_eq!(status2, StatusCode::BAD_GATEWAY);
        assert_eq!(body2, body);
    }

    #[tokio::test]
    async fn ghost_retention_then_reconcile() {
        let dir_a = temp_dir("ghost-a");
        let dir_b = temp_dir("ghost-b");

        // Node B: live server with a registered local peer + registry entry for A.
        let app_state_b = crate::test_helpers::app_state(&dir_b).await;
        app_state_b
            .rails
            .peers
            .register("gizmo", dir_b.clone(), "test")
            .unwrap();
        let app_b = Router::new()
            .nest("/api", remote_peers_router())
            .with_state(app_state_b.clone());
        let url_b = serve(app_b).await;

        // Node A knows B (key K), B knows A (key K) — shared pair key.
        seed_peer_files(
            &dir_a,
            &[sample_entry("bob", &url_b, "K_BOB")],
            &[("K_BOB", "shared-key")],
        );
        seed_peer_files(
            &dir_b,
            &[sample_entry("alice", "http://127.0.0.1:1", "K_ALICE")],
            &[("K_ALICE", "shared-key")],
        );

        // First refresh succeeds → union row for gizmo, reachable.
        let fp_a = remote_peers_state(&dir_a);
        let entry = fp_a.registry_get("bob").unwrap();
        assert!(fp_a.clone().refresh_roster_for(&entry).await);
        let rows = fp_a.roster.lock().unwrap().rows.clone();
        let gizmo = rows.iter().find(|r| r.name == "gizmo").expect("gizmo row");
        assert!(gizmo.source_reachable);
        assert_eq!(gizmo.source, "bob");

        // Poll failure keeps last-known rows as ghosts.
        let entry_dead = RemotePeerEntry {
            url: "http://127.0.0.1:1".to_string(),
            ..entry.clone()
        };
        assert!(!fp_a.clone().refresh_roster_for(&entry_dead).await);
        let rows = fp_a.roster.lock().unwrap().rows.clone();
        let ghost = rows.iter().find(|r| r.name == "gizmo").expect("ghost row kept");
        assert!(!ghost.source_reachable);

        // Reconnect reconciles: reachable again.
        assert!(fp_a.clone().refresh_roster_for(&entry).await);
        let rows = fp_a.roster.lock().unwrap().rows.clone();
        assert!(rows.iter().find(|r| r.name == "gizmo").unwrap().source_reachable);
    }

    #[tokio::test]
    async fn dm_round_trip_across_two_nodes() {
        let dir_a = temp_dir("e2e-a");
        let dir_b = temp_dir("e2e-b");

        let app_state_a = crate::test_helpers::app_state(&dir_a).await;
        let app_state_b = crate::test_helpers::app_state(&dir_b).await;
        app_state_b
            .rails
            .peers
            .register("gizmo", dir_b.clone(), "test")
            .unwrap();

        let app_a = Router::new()
            .nest("/api", remote_peers_router())
            .with_state(app_state_a.clone());
        let app_b = Router::new()
            .nest("/api", remote_peers_router())
            .with_state(app_state_b.clone());
        let url_a = serve(app_a.clone()).await;
        let url_b = serve(app_b).await;

        // Pair the nodes with a shared key. Each side names the OTHER node by
        // its fabric self-name (`node-<hash>` unless ALLTERNIT_PEER_URL is set).
        seed_peer_files(
            &dir_a,
            &[sample_entry("bob", &url_b, "K_BOB")],
            &[("K_BOB", "pair-key")],
        );
        let fp_a = remote_peers_state(&dir_a);
        let a_name = peer_self_name(&fp_a);
        seed_peer_files(
            &dir_b,
            &[sample_entry(&a_name, &url_a, "K_ALICE")],
            &[("K_ALICE", "pair-key")],
        );

        // Fire the synchronous dm from A (held in the background).
        let app_a2 = app_a.clone();
        let dm_task = tokio::spawn(async move {
            post(
                &app_a2,
                "/api/peers/remote/bob/dm",
                json!({ "message": "status?", "to": "gizmo" }),
                Some("pair-key"),
            )
            .await
        });

        // Wait for B to register the inbound run, then simulate gizmo's reply
        // via the fabric-reply contract (SendMessage to peer `fabric-replies`).
        let fp_b = remote_peers_state(&dir_b);
        let inbound_id = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let runs = fp_b.runs.lock().unwrap().clone();
                if let Some(run) = runs.values().find(|r| r.inbound) {
                    return run.run_id.clone();
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("inbound run appears");

        app_state_b
            .rails
            .bus
            .send_message(allternit_factory_engine::bus::NewBusMessage {
                correlation_id: uuid::Uuid::new_v4().to_string(),
                to: format!("peer:{FABRIC_REPLY_PEER}"),
                from: "gizmo".to_string(),
                kind: "peer_envelope".to_string(),
                payload: json!({ "body": format!("@run {inbound_id} all quiet on the western front"), "from": "gizmo" }),
                transport: "http".to_string(),
            })
            .await
            .unwrap();
        fp_b.drain_fabric_replies_with(&app_state_b.rails).await;

        // A's dm hold resolves with the reply.
        let resp = tokio::time::timeout(Duration::from_secs(10), dm_task)
            .await
            .expect("dm completes")
            .unwrap();
        let (status, body) = body_json(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "done");
        assert_eq!(body["reply"], "all quiet on the western front");

        // B's inbound run is done with the result recorded.
        let inbound = fp_b.get_run(&inbound_id).unwrap();
        assert_eq!(inbound.status, RunStatus::Done);
        assert_eq!(
            inbound.result.as_deref(),
            Some("all quiet on the western front")
        );
    }
}
