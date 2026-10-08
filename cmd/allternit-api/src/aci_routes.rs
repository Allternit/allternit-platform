//! ACI (Agent-Computer Interface) routes
//!
//! Proxy/adaptation layer over the ACU computer-use gateway
//! (`domains/computer-use/core`, FastAPI, default :8760, prefix
//! `/v1/computer-use`). Keeps the `/api/aci/*` contract the iOS and web
//! clients expect while the real planning loop runs in the ACU process.
//! Base URL: `AppConfig::acu_url()` (env `ALLTERNIT_ACU_URL`).

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures::StreamExt;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::warn;

use crate::{auth::AuthUser, AppState};

pub fn aci_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/aci/run", post(aci_run))
        .route("/aci/stream/:id", get(aci_stream))
        .route("/aci/stop/:id", post(aci_stop))
        .route("/aci/approve/:id", post(aci_approve))
        .route("/aci/approvals/:id", get(aci_approval_status))
        .route("/aci/handoff/:id", get(aci_handoff_status))
        .route("/aci/handoff/:id/approve", post(aci_handoff_approve))
        .route("/aci/handoff/:id/deny", post(aci_handoff_deny))
        .route("/aci/batch", post(crate::aci_batch::aci_batch_execute))
        .route(
            "/aci/batch/receipts/:id",
            get(crate::aci_batch::aci_batch_receipt),
        )
        .route("/aci/code", post(crate::aci_code::aci_code_execute))
        .route(
            "/aci/code/receipts/:id",
            get(crate::aci_code::aci_code_receipt),
        )
        .route("/aci/policy/audit", get(aci_policy_audit))
        .merge(crate::aci_credentials::credential_routes())
}

fn acu_base(state: &AppState) -> String {
    state.config.acu_url().trim_end_matches('/').to_string()
}

/// ACU could not be reached at all — a real 502, never a decoy 200.
fn acu_unavailable(e: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": "acu_unavailable",
            "message": format!("ACU computer-use gateway unavailable: {}", e),
        })),
    )
        .into_response()
}

/// Forward a non-success ACU response honestly (status + detail).
async fn forward_acu_error(resp: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = resp.text().await.unwrap_or_default();
    (
        status,
        Json(json!({
            "error": "acu_error",
            "message": format!("ACU gateway returned {}: {}", status, text),
        })),
    )
        .into_response()
}

// ─── Per-run event buffer ─────────────────────────────────────────────────────

/// ACU keeps one asyncio.Queue per run as a single-consumer replay buffer:
/// `/execute?stream=true` and `/runs/:id/events` read from that same queue,
/// so whichever consumer attaches first eats the history — and a short run
/// can be entirely over before `/api/aci/stream` attaches. To keep the client
/// contract (attach any time, see the full history, end with `done`), the run
/// handler drains the execute stream into this in-process buffer and the
/// stream endpoint serves from it.
///
/// Durability: each buffer is snapshot-throttled to disk (one JSON file per
/// run id under `<computer_use_dir>/run-buffers/`) and restored at boot, so
/// SSE late-join and run history survive a gateway restart. The upstream ACU
/// stream keeps running in the draining task and simply repopulates the
/// restored buffer.
struct RunEventBuffer {
    /// Mapped frames (the `/api/aci/stream` envelope), in arrival order.
    frames: Vec<serde_json::Value>,
    /// True once run.ended arrived or the upstream stream closed.
    done: bool,
    /// Snapshot throttle state.
    last_snapshot: Option<std::time::Instant>,
    /// Frames arrived since the last snapshot.
    dirty: bool,
}

/// On-disk snapshot shape for one run's buffer.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RunBufferSnapshot {
    frames: Vec<serde_json::Value>,
    done: bool,
}

/// Minimum interval between on-disk snapshots of one run's buffer.
const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

fn run_buffer_dir() -> std::path::PathBuf {
    crate::aci_approvals::computer_use_dir().join("run-buffers")
}

/// Write one run's buffer to its run-id-keyed snapshot file inside `dir`
/// (tmp + rename so a crash mid-write cannot corrupt the file).
fn snapshot_run_buffer_in(dir: &std::path::Path, run_id: &str, buf: &RunEventBuffer) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let snapshot = RunBufferSnapshot {
        frames: buf.frames.clone(),
        done: buf.done,
    };
    let Ok(json) = serde_json::to_string(&snapshot) else {
        return;
    };
    let path = dir.join(format!("{run_id}.json"));
    let tmp = dir.join(format!("{run_id}.json.tmp"));
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn snapshot_run_buffer(run_id: &str, buf: &RunEventBuffer) {
    snapshot_run_buffer_in(&run_buffer_dir(), run_id, buf);
}

/// Load every run-buffer snapshot written by a previous gateway process.
/// Corrupt files are skipped with a warning — a lost history buffer must
/// never take the gateway down.
fn restore_run_buffers_from(dir: &std::path::Path) -> HashMap<String, RunEventBuffer> {
    let mut store = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return store;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(run_id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<RunBufferSnapshot>(&text).ok())
        {
            Some(snapshot) => {
                store.insert(
                    run_id,
                    RunEventBuffer {
                        frames: snapshot.frames,
                        done: snapshot.done,
                        last_snapshot: Some(std::time::Instant::now()),
                        dirty: false,
                    },
                );
            }
            None => warn!("aci run-buffer restore: skipping unparseable {path:?}"),
        }
    }
    store
}

fn restore_run_buffers() -> HashMap<String, RunEventBuffer> {
    restore_run_buffers_from(&run_buffer_dir())
}

static ACI_RUN_EVENTS: Lazy<Mutex<HashMap<String, RunEventBuffer>>> =
    Lazy::new(|| Mutex::new(restore_run_buffers()));

/// Cap buffered frames per run so a long loop can't grow memory without
/// bound; the oldest frames drop first.
const MAX_BUFFERED_FRAMES: usize = 1000;

fn buffer_create(run_id: &str) {
    if let Ok(mut store) = ACI_RUN_EVENTS.lock() {
        store.entry(run_id.to_string()).or_insert_with(|| RunEventBuffer {
            frames: Vec::new(),
            done: false,
            last_snapshot: Some(std::time::Instant::now()),
            dirty: false,
        });
    }
}

/// Throttled snapshot: flush the buffer to disk at most once per
/// `SNAPSHOT_INTERVAL` while frames are arriving, always honoring the `done`
/// flag. Snapshots are best-effort; I/O failures are logged, never fatal.
fn snapshot_if_due(run_id: &str, buf: &mut RunEventBuffer, force: bool) {
    let now = std::time::Instant::now();
    let due = match buf.last_snapshot {
        None => true,
        Some(last) => now.duration_since(last) >= SNAPSHOT_INTERVAL,
    };
    if !(force || (buf.dirty && due)) {
        return;
    }
    snapshot_run_buffer(run_id, buf);
    buf.last_snapshot = Some(now);
    buf.dirty = false;
}

fn buffer_push(run_id: &str, mut frame: serde_json::Value, done: bool) {
    // Scrub credential values before a frame can enter the buffer (and its
    // on-disk snapshot): a sandbox that echoes its own environment must not
    // be able to smuggle a bound value into streamed/replayed output.
    if let Some(binding) = crate::aci_credentials::RUN_BINDINGS.get(run_id) {
        crate::aci_credentials::scrub_frame(&mut frame, binding.secrets());
    }
    if let Ok(mut store) = ACI_RUN_EVENTS.lock() {
        if let Some(buf) = store.get_mut(run_id) {
            if buf.frames.len() >= MAX_BUFFERED_FRAMES {
                buf.frames.remove(0);
            }
            buf.frames.push(frame);
            buf.dirty = true;
            if done {
                buf.done = true;
            }
            snapshot_if_due(run_id, buf, done);
        }
    }
}

fn buffer_mark_done(run_id: &str) {
    if let Ok(mut store) = ACI_RUN_EVENTS.lock() {
        if let Some(buf) = store.get_mut(run_id) {
            buf.done = true;
            snapshot_if_due(run_id, buf, true);
        }
    }
}

/// Drain one ACU SSE response into the per-run buffer (runs detached).
async fn drain_acu_events(run_id: String, resp: reqwest::Response) {
    let mut byte_stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = byte_stream.next().await {
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                warn!("aci run {}: upstream read error: {}", run_id, e);
                break;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        // SSE blocks are separated by double newlines.
        while let Some(end) = buf.find("\n\n") {
            let block = buf[..end].to_string();
            buf = buf[end + 2..].to_string();
            let data = block
                .lines()
                .find(|l| l.starts_with("data:"))
                .and_then(|l| l.strip_prefix("data:"))
                .map(str::trim)
                .unwrap_or("");
            if data.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(data)
                .ok()
                .and_then(|frame| map_acu_frame(&frame))
            {
                Some(mapped) => {
                    let done =
                        mapped.get("type").and_then(|t| t.as_str()) == Some("done");
                    buffer_push(&run_id, mapped, done);
                }
                None => warn!("aci run {}: unparseable frame skipped", run_id),
            }
        }
    }
    buffer_mark_done(&run_id);
}

// ─── POST /api/aci/run ────────────────────────────────────────────────────────

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AciRunBody {
    goal: String,
    model: Option<String>,
    /// Optional bot id when the run is launched on behalf of a bot session.
    /// Threaded into the declarative policy seat descriptor (botId rule
    /// field) and the policy audit rows so per-bot filtering works.
    bot_id: Option<String>,
    allowed_sites: Option<serde_json::Value>,
    open_links_in_browser: Option<bool>,
    auto_verify: Option<bool>,
    /// The deleted TS route + both clients send a STRING ("dont-keep");
    /// accept any JSON and pass it through verbatim (contract fidelity —
    /// a bool here 422'd every real client).
    session_persistence: Option<serde_json::Value>,
    /// Grant id from a prior `handoff_required` response. When the safety
    /// policy flags the goal as sensitive, presenting a valid grant bound to
    /// this exact action payload (same hash) authorizes the run; a missing,
    /// expired, consumed, or hash-mismatched grant is denied.
    approval_id: Option<String>,
    /// Names of vault credentials (`POST /api/aci/credentials`) to bind to
    /// this run. Only names travel from the client: values are resolved
    /// server-side at provision time and injected into the sandbox
    /// environment (`sandbox_env`) — never into `task`/model context, never
    /// into logs, receipts, or stream frames (see `aci_credentials`).
    credential_names: Option<Vec<String>>,
}

/// Canonical action descriptor for the ACU loop route. Ephemeral ids
/// (session/run) are excluded so a client retrying the same body after a
/// handoff approval hashes identically. `approval_id` is never part of the
/// descriptor — it rides alongside, not inside, the hashed payload.
/// Credential NAMES (never values) are included so approving a run bound to
/// credentials {A, B} cannot be replayed with {A, C}.
fn aci_action_descriptor(body: &AciRunBody, goal: &str) -> serde_json::Value {
    let mut credential_names: Vec<&String> = body
        .credential_names
        .as_deref()
        .unwrap_or_default()
        .iter()
        .collect();
    credential_names.sort();
    credential_names.dedup();
    json!({
        "route": "aci.run",
        "goal": goal,
        "model": body.model,
        "allowedSites": body.allowed_sites,
        "openLinksInBrowser": body.open_links_in_browser,
        "autoVerify": body.auto_verify,
        "sessionPersistence": body.session_persistence,
        "credentialNames": credential_names,
    })
}

/// Extract the first host-like string from the `allowedSites` run metadata
/// for policy matching. Accepts a single string or an array of strings; URLs
/// are reduced to their host portion.
fn first_policy_host(allowed_sites: &Option<serde_json::Value>) -> Option<String> {
    let hosts = allowed_sites.as_ref()?;
    let mut candidates: Vec<&str> = Vec::new();
    match hosts {
        serde_json::Value::String(single) => candidates.push(single),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    candidates.push(s);
                }
            }
        }
        _ => return None,
    }
    candidates
        .into_iter()
        .find_map(|raw| {
            let without_scheme = raw
                .split_once("://")
                .map(|(_, rest)| rest)
                .unwrap_or(raw);
            let host = without_scheme
                .split('/')
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            (!host.is_empty()).then_some(host)
        })
}

async fn aci_run(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(mut body): Json<AciRunBody>,
) -> impl IntoResponse {
    // Settings → Cowork → Allowed sites: when the client sends none, the run
    // uses the saved list. Filled before the action descriptor is hashed, so
    // a retried run (same body, same settings) hashes identically.
    if body.allowed_sites.is_none() {
        let db = state.db.clone();
        let user_id = user.user_id.clone();
        let saved = tokio::task::spawn_blocking(move || {
            db.connect()
                .map(|conn| crate::cowork_preferences_routes::load_prefs(&conn, &user_id).allowed_sites)
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !saved.is_empty() {
            body.allowed_sites = Some(json!(saved));
        }
    }
    let goal = body.goal.trim().to_string();
    if goal.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "goal is required"})),
        )
            .into_response();
    }

    let run_id = uuid::Uuid::new_v4().to_string();

    // Declarative policy gate (env-configured; entirely absent when
    // ALLTERNIT_ACI_POLICY_FILE is unset). Policy verdict first, existing
    // safety/grant flow second — a denial here never reaches aci_safety,
    // grant redemption, or ACU dispatch. The audit row is fsynced before
    // either outcome (audit-before-act).
    let policy_desc = crate::policy_config::PolicyDescriptor {
        tool: "aci.run".to_string(),
        intent: Some(goal.chars().take(200).collect()),
        bot_id: body
            .bot_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        session_id: None,
        mcp_tool: None,
        network_host: first_policy_host(&body.allowed_sites),
        file_path: None,
    };
    if let Some(verdict) = crate::policy_config::evaluate_descriptor(&policy_desc) {
        if let Err(e) = crate::policy_config::record_decision(
            &policy_desc,
            &verdict,
            Some(&user.user_id),
            Some(&run_id),
        ) {
            warn!(error = %e, "policy audit write failed; refusing to dispatch");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "policy_audit_unavailable"})),
            )
                .into_response();
        }
        if verdict.action == crate::permission_policy::PermissionAction::Deny {
            warn!(
                rule_id = ?verdict.rule_id,
                user_id = %user.user_id,
                "aci.run denied by declarative policy"
            );
            return (
                StatusCode::FORBIDDEN,
                Json(crate::policy_config::refusal_json(&verdict)),
            )
                .into_response();
        }
    }

    // Backend safety policy enforcement: host allowlist, sensitive-data
    // masking, and circuit-breaker rate limits. This mirrors the extension's
    // `browser-agent/safety/` layer so a rogue client cannot bypass it.
    let actor_key = format!(
        "{}:{}",
        user.organization_id.as_deref().unwrap_or("no-org"),
        &user.user_id
    );
    let decision = crate::aci_safety::evaluate_request(&goal, &actor_key);
    let mut granted = false;
    if !decision.allowed {
        crate::aci_safety::record_aci_error(&actor_key);

        if decision.handoff_required {
            // The action payload the grant is bound to: the goal the client
            // actually submitted, not the (possibly masked) variant.
            let action_hash =
                crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&body, &goal));

            // A client retrying after a handoff approval presents the grant
            // id. Redemption enforces approval state, ownership, expiry,
            // single-use, and — critically — that this payload hashes to the
            // granted action hash.
            if let Some(ref approval_id) = body.approval_id {
                match crate::aci_approvals::GRANTS.redeem(&user.user_id, approval_id, &action_hash)
                {
                    Ok(_) => granted = true,
                    Err(denial) => {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({
                                "error": "approval_denied",
                                "approval_id": approval_id,
                                "action_hash": action_hash,
                                "reason": denial.reason(),
                            })),
                        )
                            .into_response();
                    }
                }
            }

            if !granted {
                let approval_id = state.approval_store.create(
                    &user.user_id,
                    "aci.sensitive_action",
                    &json!({
                        "goal": decision.sanitized_goal,
                        "sensitive_actions": decision.sensitive_actions,
                        "reason": decision.reason,
                    }),
                );
                // Bind the grant to the same id and action hash so approving
                // the handoff authorizes exactly this payload — and only it.
                crate::aci_approvals::GRANTS.issue_with_id(
                    &approval_id,
                    &user.user_id,
                    &action_hash,
                    crate::aci_approvals::grant_ttl_secs(),
                );
                return (
                    StatusCode::ACCEPTED,
                    Json(json!({
                        "status": "handoff_required",
                        "approval_id": approval_id,
                        "action_hash": action_hash,
                        "message": decision.reason,
                        "sensitive_actions": decision.sensitive_actions,
                    })),
                )
                    .into_response();
            }
        } else {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": "aci_safety_violation",
                    "message": decision.reason.unwrap_or_else(|| "request blocked by safety policy".to_string()),
                })),
            )
                .into_response();
        }
    }

    let acu = acu_base(&state);

    // Credential binding: resolve names → plaintext material server-side.
    // The client never sees values; they flow only into the sandbox env of
    // the execute payload. Unknown names fail the run before ACU is called
    // (names only in the error — never values). TOTP seeds are resolved but
    // excluded from the sandbox env by CredentialStore::sandbox_env.
    let credential_names: Vec<String> = {
        let mut names: Vec<String> = body
            .credential_names
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let bound_credentials = if credential_names.is_empty() {
        Vec::new()
    } else {
        match crate::aci_credentials::CREDENTIALS.resolve(&user.user_id, &credential_names) {
            Ok(resolved) => resolved,
            Err(missing) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "unknown_credentials",
                        "message": "Some credential names do not exist in your vault.",
                        "missing": missing,
                    })),
                )
                    .into_response();
            }
        }
    };
    let sandbox_env = crate::aci_credentials::CredentialStore::sandbox_env(&bound_credentials);
    crate::aci_credentials::record_run_binding(&run_id, &user.user_id, &bound_credentials);

    // Start the run in ACU's streaming mode: the planning loop launches in
    // the gateway's background and the HTTP response is an SSE stream of
    // progress. We drain that stream into ACI_RUN_EVENTS (see the buffer
    // comment) so /api/aci/stream/:id can replay it to any number of clients
    // regardless of when they attach.
    //
    // `sandbox_env` is the credential-injection channel: the sandbox/VM
    // provision layer writes these variables into the run environment (the
    // `extra_env` → /etc/environment pattern from vm_session_routes). It is
    // deliberately a top-level field, separate from `options` and `task`, so
    // the planning loop's model context never carries credential values.
    let payload = json!({
        "mode": "intent",
        "task": decision.sanitized_goal,
        "session_id": run_id,
        "run_id": run_id,
        "target_scope": "browser",
        "sandbox_env": sandbox_env,
        "options": {
            "model": body.model,
            "allowedSites": body.allowed_sites,
            "autoVerify": body.auto_verify,
            "sessionPersistence": body.session_persistence,
            "openLinksInBrowser": body.open_links_in_browser,
            // The run's owner: the engine calls the computer toolset executor
            // (/computers/:id/toolset) on this user's behalf.
            "userId": user.user_id,
        },
    });

    let client = reqwest::Client::new();
    let resp = match client
        .post(format!("{}/v1/computer-use/execute?stream=true", acu))
        .json(&payload)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::aci_safety::record_aci_error(&actor_key);
            return acu_unavailable(e);
        }
    };
    if !resp.status().is_success() {
        crate::aci_safety::record_aci_error(&actor_key);
        return forward_acu_error(resp).await;
    }
    crate::aci_safety::record_aci_success(&actor_key);
    buffer_create(&run_id);
    tokio::spawn(drain_acu_events(run_id.clone(), resp));

    (
        StatusCode::OK,
        Json(json!({
            "sessionId": run_id,
            "adapterId": "browser",
        })),
    )
        .into_response()
}

// ─── GET /api/aci/policy/audit ────────────────────────────────────────────────

#[derive(Deserialize)]
struct PolicyAuditQuery {
    bot_id: Option<String>,
    limit: Option<usize>,
}

/// Read the policy decision audit log: newest-first rows, filtered by bot id
/// when given. Writes are audit-before-act on the policy seats, so a row
/// here predates the action it authorized (or the refusal that stopped it).
async fn aci_policy_audit(
    State(_state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Query(query): Query<PolicyAuditQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100).clamp(1, 1000);
    let rows = crate::policy_audit::read_rows(query.bot_id.as_deref(), limit);
    Json(json!({ "rows": rows }))
}

// ─── GET /api/aci/stream/:id ──────────────────────────────────────────────────

/// Pull a screenshot payload out of an ACU frame or nested `data`.
/// Planning-loop events use `screenshot_b64`; some adapters use `screenshot`
/// or an artifacts array.
fn screenshot_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value
        .get("screenshot_b64")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(s.to_string());
    }
    if let Some(s) = value
        .get("screenshot")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(s.to_string());
    }
    if let Some(s) = value
        .get("data_url")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("data:image"))
    {
        return Some(s.to_string());
    }
    if let Some(arr) = value.get("artifacts").and_then(|v| v.as_array()) {
        for art in arr {
            let ty = art.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if ty != "screenshot" {
                continue;
            }
            if let Some(c) = art
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(c.to_string());
            }
        }
    }
    value
        .get("data")
        .and_then(|inner| inner.is_object().then_some(inner))
        .and_then(screenshot_from_value)
}

/// Map one ACU SSE frame (`{event_type, run_id, message, data}`) to the
/// `/api/aci/stream` envelope the clients decode.
///
/// ACU planning-loop emits `screenshot.captured` with `screenshot_b64`. That
/// must become `type:"screenshot"` so Fabric Transport can show the live
/// computer. Other events with a human-readable `message` are `trace`; the
/// rest are `state`.
fn map_acu_frame(frame: &serde_json::Value) -> Option<serde_json::Value> {
    let event_type = frame.get("event_type").and_then(|v| v.as_str())?;
    let data = frame.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let ts = chrono::Utc::now().timestamp_millis();

    if event_type == "run.ended" {
        return Some(json!({ "type": "done", "data": data, "ts": ts }));
    }

    if let Some(screenshot) = screenshot_from_value(frame).or_else(|| screenshot_from_value(&data))
    {
        return Some(json!({
            "type": "screenshot",
            "data": { "screenshot": screenshot },
            "ts": ts,
        }));
    }

    let message = frame
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mapped = if !message.is_empty() {
        json!({
            "type": "trace",
            "data": { "message": message, "event_type": event_type, "data": data },
            "ts": ts,
        })
    } else {
        json!({ "type": "state", "data": data, "ts": ts })
    };
    Some(mapped)
}

async fn aci_stream(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    // Runs started through this API process are served from the in-process
    // buffer (replay + live tail, ending at done). Anything else — a run
    // created before this process booted, or by another ACU client — falls
    // back to proxying ACU's own events endpoint, which shares ACU's
    // single-consumer queue (see the RunEventBuffer comment).
    let known = ACI_RUN_EVENTS
        .lock()
        .map(|store| store.contains_key(&id))
        .unwrap_or(false);
    if !known {
        return aci_stream_passthrough(state, id).await;
    }

    let stream = async_stream::stream! {
        let mut cursor = 0usize;
        loop {
            let (batch, done) = match ACI_RUN_EVENTS.lock() {
                Ok(store) => match store.get(&id) {
                    Some(buf) => (buf.frames[cursor.min(buf.frames.len())..].to_vec(), buf.done),
                    None => (Vec::new(), true),
                },
                Err(_) => (Vec::new(), true),
            };
            cursor += batch.len();
            for frame in batch {
                yield Ok::<Event, std::convert::Infallible>(
                    Event::default().data(frame.to_string()),
                );
            }
            if done {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Direct proxy of ACU's `/runs/:id/events` for runs this process did not
/// start. Only one consumer can ever see such a run's history (ACU's queue is
/// single-consumer) — first attached client wins.
async fn aci_stream_passthrough(state: Arc<AppState>, id: String) -> Response {
    let acu = acu_base(&state);
    let client = reqwest::Client::new();
    let resp = match client
        .get(format!("{}/v1/computer-use/runs/{}/events", acu, id))
        .header("Accept", "text/event-stream")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return acu_unavailable(e),
    };
    if !resp.status().is_success() {
        return forward_acu_error(resp).await;
    }

    let stream = async_stream::stream! {
        let mut byte_stream = resp.bytes_stream();
        let mut buf = String::new();
        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(&String::from_utf8_lossy(&bytes));
                    // SSE blocks are separated by double newlines.
                    while let Some(end) = buf.find("\n\n") {
                        let block = buf[..end].to_string();
                        buf = buf[end + 2..].to_string();
                        let data = block
                            .lines()
                            .find(|l| l.starts_with("data:"))
                            .and_then(|l| l.strip_prefix("data:"))
                            .map(str::trim)
                            .unwrap_or("");
                        if data.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<serde_json::Value>(data)
                            .ok()
                            .and_then(|frame| map_acu_frame(&frame))
                        {
                            Some(mapped) => {
                                let is_done = mapped.get("type").and_then(|t| t.as_str()) == Some("done");
                                yield Ok::<Event, std::convert::Infallible>(
                                    Event::default().data(mapped.to_string()),
                                );
                                if is_done {
                                    return;
                                }
                            }
                            None => warn!("aci stream: unparseable frame skipped"),
                        }
                    }
                }
                Err(e) => {
                    warn!("aci stream: upstream read error: {}", e);
                    break;
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ─── POST /api/aci/stop/:id ───────────────────────────────────────────────────

async fn aci_stop(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let acu = acu_base(&state);
    let client = reqwest::Client::new();
    match client
        .post(format!("{}/v1/computer-use/runs/{}/cancel", acu, id))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => Json(v).into_response(),
            Err(_) => Json(json!({"run_id": id, "status": "cancelled"})).into_response(),
        },
        Ok(r) => forward_acu_error(r).await,
        Err(e) => acu_unavailable(e),
    }
}

// ─── POST /api/aci/approve/:id[?deny=true] ────────────────────────────────────

#[derive(Deserialize)]
struct AciApproveQuery {
    deny: Option<bool>,
}

async fn aci_approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<AciApproveQuery>,
) -> Response {
    let decision = if query.deny.unwrap_or(false) {
        "deny"
    } else {
        "approve"
    };
    let acu = acu_base(&state);
    let client = reqwest::Client::new();
    match client
        .post(format!("{}/v1/computer-use/runs/{}/approve", acu, id))
        .json(&json!({ "decision": decision, "comment": "" }))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => Json(v).into_response(),
            Err(_) => Json(json!({"run_id": id, "decision": decision})).into_response(),
        },
        Ok(r) => forward_acu_error(r).await,
        Err(e) => acu_unavailable(e),
    }
}

// ─── Handoff endpoints for sensitive actions ──────────────────────────────────

/// One user-facing approval status model (see `aci_approvals` for the
/// vocabulary). Resolution order: Rust hash grant → handoff record → proxied
/// ACU run. Grant and handoff state are in-memory, so after a gateway restart
/// only the ACU-run lookup can still resolve (matching the Python side, where
/// `approval_future`s also die with their process).
#[derive(Debug, Serialize)]
struct UnifiedApprovalResponse {
    approval_id: String,
    status: &'static str,
    /// Where the answer came from: `hash_grant`, `handoff`, or `acu_run`.
    source: &'static str,
    expires_at: Option<i64>,
    message: Option<String>,
}

/// Map an ACU run record (GET /v1/computer-use/runs/{id}) to the unified
/// status. A run awaiting a decision is `pending`; a timed-out
/// `approval_future` is `expired`; anything else means the approval gate was
/// resolved (or never needed one) and the run moved on — `consumed`.
fn unified_status_from_acu_run(run: &serde_json::Value) -> UnifiedApprovalResponse {
    let status = run.get("status").and_then(|s| s.as_str()).unwrap_or("");
    let timed_out = run
        .get("approval_timed_out")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let unified = if status == "awaiting_approval" {
        crate::aci_approvals::UnifiedApprovalStatus::Pending
    } else if timed_out {
        crate::aci_approvals::UnifiedApprovalStatus::Expired
    } else {
        crate::aci_approvals::UnifiedApprovalStatus::Consumed
    };
    UnifiedApprovalResponse {
        approval_id: run
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: unified.as_str(),
        source: "acu_run",
        expires_at: None,
        message: if status == "awaiting_approval" {
            Some(format!("run is awaiting approval (run status: {status})"))
        } else {
            Some(format!("run status: {status}"))
        },
    }
}

async fn aci_approval_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    // 1. Rust hash grant (covers handoffs issued by aci_run: grant and
    //    handoff record share the same id).
    if let Some(grant) = crate::aci_approvals::GRANTS.get(&id) {
        let unified = crate::aci_approvals::unified_status(
            &grant,
            chrono::Utc::now().timestamp_millis(),
        );
        return Json(UnifiedApprovalResponse {
            approval_id: id,
            status: unified.as_str(),
            source: "hash_grant",
            expires_at: Some(grant.expires_at),
            message: None,
        })
            .into_response();
    }

    // 2. Handoff record without a live grant (e.g. grant swept by the
    //    retention cutoff). Handoff records carry no TTL of their own.
    if let Some(req) = state.approval_store.get(&id) {
        let status = match req.status {
            crate::permission_policy::ApprovalStatus::Pending => {
                crate::aci_approvals::UnifiedApprovalStatus::Pending
            }
            crate::permission_policy::ApprovalStatus::Approved => {
                crate::aci_approvals::UnifiedApprovalStatus::Approved
            }
            crate::permission_policy::ApprovalStatus::Denied => {
                crate::aci_approvals::UnifiedApprovalStatus::Denied
            }
        };
        return Json(UnifiedApprovalResponse {
            approval_id: id,
            status: status.as_str(),
            source: "handoff",
            expires_at: None,
            message: None,
        })
            .into_response();
    }

    // 3. Proxied Python approval_future: the id is an ACU run id.
    let acu = acu_base(&state);
    let client = reqwest::Client::new();
    match client
        .get(format!("{}/v1/computer-use/runs/{}", acu, id))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(run) => Json(unified_status_from_acu_run(&run)).into_response(),
            Err(_) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "acu_error", "message": "ACU returned an unparseable run record"})),
            )
                .into_response(),
        },
        // ACU answered but doesn't know this run either — the approval id is
        // unknown to every backend.
        Ok(r) if r.status().as_u16() == 404 => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "approval_not_found",
                "message": "No approval with this id in the Rust grant store, handoff store, or ACU run store.",
            })),
        )
            .into_response(),
        Ok(r) => forward_acu_error(r).await,
        Err(e) => acu_unavailable(e),
    }
}

async fn aci_handoff_status(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match state.approval_store.get(&id) {
        Some(req) => Json(req).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "handoff_not_found", "message": "No such handoff request."})),
        )
            .into_response(),
    }
}

async fn aci_handoff_approve(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if state.approval_store.approve(&id) {
        // Mirror the human decision into the action-hash grant so a
        // subsequent run carrying this approval_id can redeem it.
        crate::aci_approvals::GRANTS.approve(&id);
        Json(json!({"approval_id": id, "status": "approved"})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "handoff_not_found", "message": "No such handoff request."})),
        )
            .into_response()
    }
}

async fn aci_handoff_deny(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if state.approval_store.deny(&id) {
        crate::aci_approvals::GRANTS.deny(&id);
        Json(json!({"approval_id": id, "status": "denied"})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "handoff_not_found", "message": "No such handoff request."})),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_buffer_snapshot_and_restore_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut buf = RunEventBuffer {
            frames: vec![
                json!({"type": "trace", "data": {"message": "step 1"}}),
                json!({"type": "screenshot", "data": {"screenshot": "aGk="}}),
            ],
            done: true,
            last_snapshot: None,
            dirty: true,
        };
        snapshot_run_buffer_in(dir.path(), "run-abc", &buf);
        // Snapshot resets throttle bookkeeping (caller side), so simulate the
        // caller: the on-disk copy is what matters.
        let mut store = HashMap::new();
        store.insert("run-abc".to_string(), std::mem::replace(&mut buf, RunEventBuffer {
            frames: vec![],
            done: false,
            last_snapshot: None,
            dirty: false,
        }));
        let restored = restore_run_buffers_from(dir.path());
        let restored = restored.get("run-abc").expect("run-abc restored");
        assert_eq!(restored.frames.len(), 2);
        assert_eq!(restored.frames[0]["type"], "trace");
        assert_eq!(restored.frames[1]["type"], "screenshot");
        assert!(restored.done);
        assert!(!restored.dirty);
    }

    #[test]
    fn run_buffer_restore_skips_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("run-good.json"), r#"{"frames":[{"type":"state"}],"done":false}"#).unwrap();
        std::fs::write(dir.path().join("run-bad.json"), "{not json").unwrap();
        std::fs::write(dir.path().join("run-bad2.json"), r#"{"frames":"nope","done":false}"#).unwrap();
        std::fs::write(dir.path().join("ignore.txt"), "not a snapshot").unwrap();

        let restored = restore_run_buffers_from(dir.path());
        assert_eq!(restored.len(), 1);
        assert!(restored.contains_key("run-good"));
    }

    #[test]
    fn unified_status_from_acu_run_maps_python_approval_lifecycle() {
        // awaiting_approval → pending
        let awaiting = json!({"run_id": "r1", "status": "awaiting_approval", "approval_timed_out": false});
        assert_eq!(unified_status_from_acu_run(&awaiting).status, "pending");

        // Timed-out approval_future → expired
        let timed_out = json!({"run_id": "r1", "status": "running", "approval_timed_out": true});
        assert_eq!(unified_status_from_acu_run(&timed_out).status, "expired");

        // Resolved / completed run → consumed
        let completed = json!({"run_id": "r1", "status": "completed", "approval_timed_out": false});
        assert_eq!(unified_status_from_acu_run(&completed).status, "consumed");
    }

    #[test]
    fn snapshot_throttle_writes_immediately_when_due_and_on_done() {
        // This test mutates ALLTERNIT_COMPUTER_USE_DIR, which the policy-seat
        // tests read at request time (audit log, approvals). Take the same
        // env-var lock they do, or a parallel policy test sees this test's
        // temp dir (or the unset fallback) mid-request and flakes.
        let _guard = crate::policy_config::POLICY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prior_dir = std::env::var("ALLTERNIT_COMPUTER_USE_DIR").ok();
        let dir = tempfile::tempdir().unwrap();
        // Point the snapshot writer at a temp dir for this test.
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", dir.path());
        let mut buf = RunEventBuffer {
            frames: vec![json!({"type": "state"})],
            done: false,
            last_snapshot: None,
            dirty: true,
        };
        // First push: last_snapshot is None → snapshot fires.
        snapshot_if_due("run-throttle", &mut buf, false);
        assert!(dir.path().join("run-buffers/run-throttle.json").exists());
        assert!(!buf.dirty);

        // Immediate second push: within the throttle window, not forced → no
        // rewrite (dirty stays queued).
        buf.frames.push(json!({"type": "trace"}));
        buf.dirty = true;
        snapshot_if_due("run-throttle", &mut buf, false);
        let on_disk = std::fs::read_to_string(dir.path().join("run-buffers/run-throttle.json")).unwrap();
        let parsed: RunBufferSnapshot = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed.frames.len(), 1, "throttled snapshot must not have been rewritten");
        assert!(buf.dirty, "unsnapshotted frames stay dirty");

        // done=true forces a flush.
        buf.done = true;
        snapshot_if_due("run-throttle", &mut buf, true);
        let on_disk = std::fs::read_to_string(dir.path().join("run-buffers/run-throttle.json")).unwrap();
        let parsed: RunBufferSnapshot = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed.frames.len(), 2);
        assert!(parsed.done);
        assert!(!buf.dirty);
        // Restore the prior value (don't leave the var unset for tests that
        // expect the policy-seat default resolution).
        match prior_dir {
            Some(value) => std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", value),
            None => std::env::remove_var("ALLTERNIT_COMPUTER_USE_DIR"),
        }
    }

    #[test]
    fn map_acu_frame_emits_screenshot_from_planning_loop() {
        let frame = json!({
            "event_type": "screenshot.captured",
            "run_id": "run-1",
            "message": "screenshot.captured",
            "data": {
                "type": "screenshot.captured",
                "run_id": "run-1",
                "step": 0,
                "phase": "initial",
                "screenshot_b64": "iVBORw0KGgo="
            }
        });
        let mapped = map_acu_frame(&frame).expect("mapped");
        assert_eq!(mapped["type"], "screenshot");
        assert_eq!(mapped["data"]["screenshot"], "iVBORw0KGgo=");
    }

    #[test]
    fn map_acu_frame_ends_on_run_ended() {
        let frame = json!({
            "event_type": "run.ended",
            "run_id": "run-1",
            "message": "completed",
            "data": { "status": "completed" }
        });
        let mapped = map_acu_frame(&frame).expect("mapped");
        assert_eq!(mapped["type"], "done");
    }

    #[test]
    fn map_acu_frame_keeps_trace_when_no_screenshot() {
        let frame = json!({
            "event_type": "plan.created",
            "run_id": "run-1",
            "message": "plan.created",
            "data": { "step": 1 }
        });
        let mapped = map_acu_frame(&frame).expect("mapped");
        assert_eq!(mapped["type"], "trace");
        assert_eq!(mapped["data"]["event_type"], "plan.created");
    }

    #[test]
    fn aci_action_descriptor_is_deterministic_and_excludes_ephemeral_ids() {
        let body = AciRunBody {
            goal: "buy tickets example.com".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            bot_id: None,
            allowed_sites: Some(json!(["example.com"])),
            open_links_in_browser: Some(true),
            auto_verify: Some(false),
            session_persistence: Some(json!("dont-keep")),
            approval_id: Some("grant-from-a-prior-attempt".to_string()),
            credential_names: None,
        };
        let h1 = crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&body, &body.goal));

        // Same body re-submitted after a handoff decision, with a different
        // (or absent) approval_id, must hash identically.
        let mut retry = AciRunBody {
            goal: "buy tickets example.com".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            allowed_sites: Some(json!(["example.com"])),
            open_links_in_browser: Some(true),
            auto_verify: Some(false),
            session_persistence: Some(json!("dont-keep")),
            bot_id: None,
            approval_id: None,
            credential_names: None,
        };
        let h2 = crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&retry, &retry.goal));
        assert_eq!(h1, h2);

        // Any payload change changes the hash.
        retry.goal = "buy tickets evil-example.com".to_string();
        let h3 = crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&retry, &retry.goal));
        assert_ne!(h1, h3);
    }

    #[test]
    fn handoff_grant_redeems_once_for_matching_hash_only() {
        // Issue a grant exactly as aci_run does when flagging a sensitive
        // goal, then exercise the redemption contract the run handler relies
        // on: approved + same hash → ok; repeat → single-use denial; other
        // payload → hash-mismatch denial.
        let store = crate::permission_policy::ApprovalStore::new();
        let body = AciRunBody {
            goal: "checkout with credit card on example.com".to_string(),
            model: None,
            bot_id: None,
            allowed_sites: None,
            open_links_in_browser: None,
            auto_verify: None,
            session_persistence: None,
            approval_id: None,
            credential_names: None,
        };
        let action_hash = crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&body, &body.goal));
        let approval_id = store.create("user-1", "aci.sensitive_action", &json!({}));
        crate::aci_approvals::GRANTS.issue_with_id(
            &approval_id,
            "user-1",
            &action_hash,
            crate::aci_approvals::grant_ttl_secs(),
        );

        // Not yet approved by the human.
        assert_eq!(
            crate::aci_approvals::GRANTS.redeem("user-1", &approval_id, &action_hash),
            Err(crate::aci_approvals::GrantDenial::NotApproved)
        );

        crate::aci_approvals::GRANTS.approve(&approval_id);
        assert!(crate::aci_approvals::GRANTS
            .redeem("user-1", &approval_id, &action_hash)
            .is_ok());
        assert_eq!(
            crate::aci_approvals::GRANTS.redeem("user-1", &approval_id, &action_hash),
            Err(crate::aci_approvals::GrantDenial::AlreadyConsumed)
        );

        let mut tampered = body.clone();
        tampered.goal = "checkout with credit card on evil.com".to_string();
        let other_hash =
            crate::aci_approvals::hash_action_payload(&aci_action_descriptor(&tampered, &tampered.goal));
        assert_eq!(
            crate::aci_approvals::GRANTS.redeem("user-1", &approval_id, &other_hash),
            Err(crate::aci_approvals::GrantDenial::AlreadyConsumed)
        );
    }
}

#[cfg(test)]
mod approval_http_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
    }

    /// HTTP smoke of the unified approval surface: a handoff record resolves
    /// to pending -> approved via GET /api/aci/approvals/{id}, and an id
    /// unknown to every backend surfaces a real gateway error (not a decoy
    /// 200) when ACU is unreachable.
    #[tokio::test]
    async fn unified_approval_status_over_http_via_handoff_store() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;

        // A handoff record without a live hash grant (source: "handoff").
        let approval_id = state
            .approval_store
            .create("user-1", "aci.sensitive_action", &json!({"goal": "buy tickets"}));

        let app = aci_router().with_state(state.clone());
        let get_status = |id: &str| {
            let app = app.clone();
            let id = id.to_string();
            async move {
                app.oneshot(
                    Request::get(format!("/aci/approvals/{id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let resp = get_status(&approval_id).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["status"], "pending");
        assert_eq!(json["source"], "handoff");
        assert_eq!(json["approval_id"], approval_id);

        state.approval_store.approve(&approval_id);
        let resp = get_status(&approval_id).await;
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["status"], "approved");

        state.approval_store.deny(&approval_id);
        let resp = get_status(&approval_id).await;
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["status"], "denied");

        // Unknown id: the proxy leg must fail honestly — 502 acu_unavailable
        // when ACU is down, or 404 approval_not_found when ACU is up but
        // doesn't know the run. Never a fabricated 200.
        let resp = get_status("00000000-0000-0000-0000-000000000000").await;
        let status = resp.status();
        assert!(
            status == StatusCode::BAD_GATEWAY || status == StatusCode::NOT_FOUND,
            "unknown approval must surface a real error, got {status}"
        );
        let json = body_json(resp.into_body()).await;
        assert!(
            json["error"] == "acu_unavailable" || json["error"] == "approval_not_found",
            "unexpected error body: {json}"
        );
    }
}

#[cfg(test)]
mod policy_seat_tests {
    //! Policy seats on `/aci/run`: deny refusals, the audit-before-act
    //! ordering guarantee (audit row durable BEFORE the executor is called,
    //! even when the executor then fails), and the `/aci/policy/audit` read
    //! API. Policy state and the env vars it rides on are process-global, so
    //! every test here serializes on POLICY_LOCK and restores the prior
    //! policy on the way out.

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex as StdMutex};
    use tower::ServiceExt;

    const POLICY_USER: &str = "user-policy-seat";

    /// Serializes the env-var + global-policy mutations in this module
    /// against every other test that touches a policy seat (see
    /// `policy_config::POLICY_TEST_LOCK`).
    fn policy_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::policy_config::POLICY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct PolicyGuard {
        prior: Option<crate::permission_policy::PermissionPolicy>,
    }

    impl PolicyGuard {
        fn install(rules: Vec<crate::permission_policy::PermissionRule>) -> Self {
            let prior = crate::policy_config::active_policy();
            crate::policy_config::install(Some(crate::permission_policy::PermissionPolicy {
                name: "seat-tests".to_string(),
                rules,
            }));
            Self { prior }
        }
    }

    impl Drop for PolicyGuard {
        fn drop(&mut self) {
            crate::policy_config::install(self.prior.take());
        }
    }

    fn allow_all() -> crate::permission_policy::PermissionRule {
        crate::permission_policy::PermissionRule {
            id: Some("default-allow".to_string()),
            tool: Some("*".to_string()),
            action: crate::permission_policy::PermissionAction::Allow,
            ..Default::default()
        }
    }

    fn policy_user() -> crate::auth::AuthUser {
        crate::auth::AuthUser {
            user_id: POLICY_USER.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
    }

    fn audit_lines() -> Vec<serde_json::Value> {
        let Ok(text) = std::fs::read_to_string(crate::policy_audit::policy_audit_path()) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Mock ACU whose execute handler always records the hit and also checks
    /// whether the `allowed` audit row is already durable at hit time — then
    /// fails with a 500, the forced-executor failure the ordering guarantee
    /// must survive.
    async fn start_failing_mock_acu(
        hit: Arc<std::sync::atomic::AtomicBool>,
        audit_seen_before_hit: Arc<std::sync::atomic::AtomicBool>,
    ) -> String {
        let app = axum::Router::new()
            .route(
                "/v1/computer-use/execute",
                axum::routing::post(move || {
                    let hit = hit.clone();
                    let flag = audit_seen_before_hit.clone();
                    async move {
                        hit.store(true, std::sync::atomic::Ordering::SeqCst);
                        let lines = audit_lines();
                        let seen = lines.iter().any(|row| {
                            row["tool"] == "aci.run"
                                && row["decision"] == "allowed"
                                && row["actor"] == POLICY_USER
                        });
                        flag.store(seen, std::sync::atomic::Ordering::SeqCst);
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                }),
            )
            .fallback(|| async { StatusCode::NOT_FOUND });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    async fn post_goal(app: &axum::Router, goal: &str) -> axum::response::Response {
        post_goal_as(app, goal, None).await
    }

    async fn post_goal_as(
        app: &axum::Router,
        goal: &str,
        bot_id: Option<&str>,
    ) -> axum::response::Response {
        let mut body = serde_json::json!({"goal": goal});
        if let Some(bot_id) = bot_id {
            body["botId"] = serde_json::Value::String(bot_id.to_string());
        }
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/aci/run")
                    .extension(policy_user())
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_audit(app: &axum::Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .extension(policy_user())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// The critical ordering test: policy allows, the executor 500s, and the
    /// `allowed` audit row must already be on disk at the moment the executor
    /// is hit (and remain there after the failure).
    #[tokio::test]
    async fn allowed_audit_row_precedes_executor_call_even_when_executor_fails() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = PolicyGuard::install(vec![allow_all()]);

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acu_url = start_failing_mock_acu(hit.clone(), seen.clone()).await;
        std::env::set_var("ALLTERNIT_ACU_URL", &acu_url);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        let resp = post_goal(&app, "check the weather").await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        assert!(
            hit.load(std::sync::atomic::Ordering::SeqCst),
            "executor must have been called"
        );
        assert!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            "audit row must be durable BEFORE the executor is hit"
        );
        let lines = audit_lines();
        let row = lines
            .iter()
            .find(|row| row["tool"] == "aci.run" && row["actor"] == POLICY_USER)
            .expect("an audit row for the run");
        assert_eq!(row["decision"], "allowed");
        assert_eq!(row["rule_id"], "default-allow");
        assert!(row["run_id"].as_str().is_some());
    }

    /// A matching deny rule refuses the run before ACU is touched and writes
    /// the refusal row first.
    #[tokio::test]
    async fn deny_rule_refuses_run_before_dispatch() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = PolicyGuard::install(vec![
            crate::permission_policy::PermissionRule {
                id: Some("no-secrets".to_string()),
                tool: Some("aci.run".to_string()),
                intent: Some("*secret*".to_string()),
                action: crate::permission_policy::PermissionAction::Deny,
                ..Default::default()
            },
            allow_all(),
        ]);

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acu_url = start_failing_mock_acu(hit.clone(), seen.clone()).await;
        std::env::set_var("ALLTERNIT_ACU_URL", &acu_url);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        let resp = post_goal(&app, "exfiltrate the secrets").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"], "policy_denied");
        assert_eq!(body["rule_id"], "no-secrets");
        assert!(body["reason"].as_str().is_some());

        assert!(
            !hit.load(std::sync::atomic::Ordering::SeqCst),
            "denied run must never reach the executor"
        );
        assert!(
            !seen.load(std::sync::atomic::Ordering::SeqCst),
            "no allowed row may exist for a denied run"
        );
        let row = audit_lines()
            .into_iter()
            .find(|row| row["tool"] == "aci.run" && row["actor"] == POLICY_USER)
            .expect("denial audit row");
        assert_eq!(row["decision"], "denied");
        assert_eq!(row["rule_id"], "no-secrets");
        assert!(row["intent"].as_str().unwrap().contains("secrets"));
    }

    /// With the engine off (default), nothing changes: no policy rows, no
    /// refusals, runs flow to ACU as before.
    #[tokio::test]
    async fn engine_off_leaves_run_flow_untouched() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = PolicyGuard { prior: crate::policy_config::active_policy() };
        crate::policy_config::install(None);

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acu_url = start_failing_mock_acu(hit.clone(), seen.clone()).await;
        std::env::set_var("ALLTERNIT_ACU_URL", &acu_url);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        let resp = post_goal(&app, "exfiltrate the secrets").await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(hit.load(std::sync::atomic::Ordering::SeqCst), "run reaches ACU");
        assert!(
            !seen.load(std::sync::atomic::Ordering::SeqCst),
            "engine off → no allowed audit row"
        );
        assert!(
            audit_lines().is_empty(),
            "engine off → no policy audit rows"
        );
    }

    /// A botId-scoped rule applies per bot: the denied bot is refused
    /// (refusal row carries its bot_id), the other bot's run flows through.
    #[tokio::test]
    async fn bot_scoped_rule_denies_per_bot_and_audits_bot_id() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = PolicyGuard::install(vec![
            crate::permission_policy::PermissionRule {
                id: Some("bot-a-off".to_string()),
                tool: Some("aci.run".to_string()),
                bot_id: Some("bot-a".to_string()),
                action: crate::permission_policy::PermissionAction::Deny,
                ..Default::default()
            },
            allow_all(),
        ]);

        let hit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let acu_url = start_failing_mock_acu(hit.clone(), seen.clone()).await;
        std::env::set_var("ALLTERNIT_ACU_URL", &acu_url);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        // bot-a: denied before dispatch, row carries bot_id.
        let resp = post_goal_as(&app, "check the weather", Some("bot-a")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"], "policy_denied");
        assert_eq!(body["rule_id"], "bot-a-off");
        assert!(
            !hit.load(std::sync::atomic::Ordering::SeqCst),
            "bot-a's run must not reach the executor"
        );
        let row = audit_lines()
            .into_iter()
            .find(|row| row["tool"] == "aci.run" && row["actor"] == POLICY_USER)
            .expect("bot-a denial audit row");
        assert_eq!(row["decision"], "denied");
        assert_eq!(row["bot_id"], "bot-a");

        // bot-b: allow-all matches, run reaches the executor (which 500s).
        let resp = post_goal_as(&app, "check the weather", Some("bot-b")).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            hit.load(std::sync::atomic::Ordering::SeqCst),
            "bot-b's run must reach the executor"
        );
        let row = audit_lines()
            .into_iter()
            .find(|row| {
                row["tool"] == "aci.run" && row["actor"] == POLICY_USER && row["bot_id"] == "bot-b"
            })
            .expect("bot-b allowed audit row");
        assert_eq!(row["decision"], "allowed");

        // Audit API filter: per-bot rows retrievable.
        let resp = get_audit(&app, "/aci/policy/audit?bot_id=bot-a").await;
        let body = body_json(resp.into_body()).await;
        let rows = body["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["decision"], "denied");
    }

    /// The read API returns newest-first rows and the bot_id filter works.
    #[tokio::test]
    async fn audit_api_returns_rows_with_bot_filter() {
        let _guard = policy_lock();
        let temp = tempfile::tempdir().unwrap().keep();
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);
        let _policy = PolicyGuard { prior: crate::policy_config::active_policy() };
        crate::policy_config::install(None);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        for (bot, decision) in [
            ("bot-a", crate::policy_audit::PolicyDecision::Allowed),
            ("bot-b", crate::policy_audit::PolicyDecision::Denied),
            ("bot-a", crate::policy_audit::PolicyDecision::Allowed),
        ] {
            let mut row = crate::policy_audit::PolicyAuditRow::new(decision, "aci.run");
            row.bot_id = Some(bot.to_string());
            crate::policy_audit::record(&row).unwrap();
        }

        let resp = get_audit(&app, "/aci/policy/audit").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let rows = body["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 3);
        // Newest first.
        assert_eq!(rows[0]["bot_id"], "bot-a");
        assert_eq!(rows[2]["bot_id"], "bot-a");

        let resp = get_audit(&app, "/aci/policy/audit?bot_id=bot-b").await;
        let body = body_json(resp.into_body()).await;
        let rows = body["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["bot_id"], "bot-b");
        assert_eq!(rows[0]["decision"], "denied");

        let resp = get_audit(&app, "/aci/policy/audit?limit=2").await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["rows"].as_array().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod credential_binding_http_tests {
    //! End-to-end run binding: `/aci/run` with `credentialNames` resolves
    //! values server-side, injects them into the sandbox env of the ACU
    //! execute payload, and guarantees the value never reaches the model
    //! context (`task`), the run-event buffer, its on-disk snapshot, or
    //! approval receipts. A mock ACU gateway stands in for the Python
    //! planning loop via `ALLTERNIT_ACU_URL`.

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::sync::{Arc, Mutex as StdMutex};
    use tower::ServiceExt;

    const E2E_USER: &str = "user-cu17-e2e";
    const E2E_CRED: &str = "session-tok";
    const E2E_VALUE: &str = "e2e-sekrit-token-value";

    fn e2e_user() -> crate::auth::AuthUser {
        crate::auth::AuthUser {
            user_id: E2E_USER.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    fn ensure_e2e_key() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if std::env::var("ALLTERNIT_ENCRYPTION_KEY").is_err() {
                std::env::set_var(
                    "ALLTERNIT_ENCRYPTION_KEY",
                    "aci-credentials-test-key-0123456789abcdef",
                );
            }
        });
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::Value::Null)
    }

    type Capture = Arc<StdMutex<Vec<serde_json::Value>>>;

    /// Minimal mock of the ACU gateway: captures the execute payload and
    /// streams back one trace frame that deliberately echoes a bound
    /// credential value (a sandbox leaking its own env), then run.ended.
    async fn start_mock_acu() -> (String, Capture) {
        let capture: Capture = Arc::new(StdMutex::new(Vec::new()));
        let state = capture.clone();
        let app = axum::Router::new()
            .route(
                "/v1/computer-use/execute",
                axum::routing::post(
                    move |body: axum::body::Bytes| {
                        let state = state.clone();
                        async move {
                            if let Ok(value) =
                                serde_json::from_slice::<serde_json::Value>(&body)
                            {
                                state.lock().unwrap().push(value);
                            }
                            let sse = format!(
                                "data: {}\n\ndata: {}\n\n",
                                serde_json::json!({
                                    "event_type": "plan.created",
                                    "run_id": "r",
                                    "message": format!("sandbox env echo: {E2E_VALUE}"),
                                    "data": {},
                                }),
                                serde_json::json!({
                                    "event_type": "run.ended",
                                    "run_id": "r",
                                    "message": "completed",
                                    "data": {"status": "completed"},
                                }),
                            );
                            (
                                [("content-type", "text/event-stream")],
                                sse,
                            )
                        }
                    },
                ),
            )
            .fallback(|| async { StatusCode::NOT_FOUND });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), capture)
    }

    async fn post_run(
        app: &axum::Router,
        goal: &str,
        credential_names: &[&str],
        approval_id: Option<&str>,
    ) -> axum::response::Response {
        let mut body = serde_json::json!({
            "goal": goal,
            "credentialNames": credential_names,
        });
        if let Some(id) = approval_id {
            body["approvalId"] = serde_json::Value::String(id.to_string());
        }
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/aci/run")
                    .extension(e2e_user())
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn wait_for_done(run_id: &str) -> Vec<serde_json::Value> {
        for _ in 0..100 {
            if let Ok(store) = ACI_RUN_EVENTS.lock() {
                if let Some(buf) = store.get(run_id) {
                    if buf.done {
                        return buf.frames.clone();
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("run {run_id} did not finish in time");
    }

    #[tokio::test]
    async fn run_binds_credentials_into_sandbox_env_and_leaks_nowhere() {
        // Serializes against policy-seat tests (this test POSTs /aci/run,
        // which a concurrently installed policy could deny) AND against every
        // other test that reads/writes the process-wide
        // ALLTERNIT_COMPUTER_USE_DIR. Both purposes share ONE guard:
        // `computer_use_dir_test_lock()` is the same mutex
        // (`policy_config::POLICY_TEST_LOCK`), and std `Mutex` is not
        // reentrant — acquiring both on this thread self-deadlocks.
        let _guard = crate::test_helpers::computer_use_dir_test_lock();
        ensure_e2e_key();
        let temp = tempfile::tempdir().unwrap().keep();
        // Redirect gateway state (run buffers, credential vault) at the temp
        // dir before the run. Left in place for the whole test.
        std::env::set_var("ALLTERNIT_COMPUTER_USE_DIR", &temp);

        // The global store persists across test processes on this machine;
        // delete first so re-runs don't trip DuplicateName.
        let _ = crate::aci_credentials::CREDENTIALS.delete(E2E_USER, E2E_CRED);
        crate::aci_credentials::CREDENTIALS
            .create(E2E_USER, E2E_CRED, crate::aci_credentials::CredentialType::Token, E2E_VALUE)
            .expect("seed credential");

        let (acu_url, capture) = start_mock_acu().await;
        std::env::set_var("ALLTERNIT_ACU_URL", &acu_url);

        let state = crate::test_helpers::app_state(&temp).await;
        let app = aci_router().with_state(state);

        // ── Plain run: value goes to sandbox_env, never the model context.
        let resp = post_run(&app, "check my order status", &[E2E_CRED], None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let run_id = body["sessionId"].as_str().unwrap().to_string();

        // Give the drain task a beat, then inspect what ACU received.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let payloads = capture.lock().unwrap().clone();
        assert_eq!(payloads.len(), 1, "mock ACU should have seen one execute");
        let payload = &payloads[0];
        assert_eq!(
            payload["sandbox_env"]["ACI_CRED_SESSION_TOK"].as_str(),
            Some(E2E_VALUE),
            "value must be injected as sandbox env material"
        );
        let task = payload["task"].as_str().unwrap_or("");
        assert!(
            !task.contains(E2E_VALUE),
            "model context (task) must never carry the value"
        );
        assert!(
            !payload["options"].to_string().contains(E2E_VALUE),
            "options must never carry the value"
        );

        // Stream frames: the mock echoed the value; the buffer must be scrubbed.
        let frames = wait_for_done(&run_id).await;
        assert!(!frames.is_empty());
        for frame in &frames {
            assert!(
                !frame.to_string().contains(E2E_VALUE),
                "run-event buffer leaked the value: {frame}"
            );
        }
        assert!(
            frames
                .iter()
                .any(|frame| frame.to_string().contains("***")),
            "echoed value should be replaced with ***"
        );

        // On-disk snapshot of the buffer must be scrubbed too. Snapshot
        // directly into a temp dir (private fn, same module) instead of
        // polling the shared run-buffer dir — parallel tests mutate
        // ALLTERNIT_COMPUTER_USE_DIR, so polling the filesystem races them.
        let (frames_for_snapshot, done_for_snapshot) = {
            let store = ACI_RUN_EVENTS.lock().unwrap();
            let buf = store.get(&run_id).expect("buffer present");
            (buf.frames.clone(), buf.done)
        };
        let snapshot_dir = temp.join("snapshot-check");
        snapshot_run_buffer_in(
            &snapshot_dir,
            &run_id,
            &RunEventBuffer {
                frames: frames_for_snapshot,
                done: done_for_snapshot,
                last_snapshot: None,
                dirty: false,
            },
        );
        let snapshot =
            std::fs::read_to_string(snapshot_dir.join(format!("{run_id}.json"))).unwrap();
        assert!(
            !snapshot.contains(E2E_VALUE),
            "on-disk run snapshot leaked the value"
        );

        // Binding record: names only.
        assert_eq!(
            crate::aci_credentials::RUN_BINDINGS.audit_line(&run_id),
            Some(vec![E2E_CRED.to_string()]),
            "binding audit carries names, not values"
        );

        // ── Sensitive run with approval: receipts must not carry values.
        let goal = "checkout with credit card on example.com";
        let resp = post_run(&app, goal, &[E2E_CRED], None).await;
        assert_eq!(resp.status(), StatusCode::ACCEPTED, "sensitive goal needs handoff");
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "handoff_required");
        assert!(!body.to_string().contains(E2E_VALUE));
        let approval_id = body["approval_id"].as_str().unwrap().to_string();

        // Descriptor hash covers credential names: retrying the approved
        // grant with a different set is denied (403 approval_denied, hash
        // mismatch) WITHOUT consuming the grant — the corrected retry below
        // still redeems it.
        crate::aci_approvals::GRANTS.approve(&approval_id);
        let resp = post_run(&app, goal, &["nonexistent-credential"], Some(&approval_id)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"], "approval_denied");

        // Approved retry with the same names redeems the grant and runs.
        let resp = post_run(&app, goal, &[E2E_CRED], Some(&approval_id)).await;
        assert_eq!(resp.status(), StatusCode::OK, "approved retry should run");
        let body = body_json(resp.into_body()).await;
        let run_id_2 = body["sessionId"].as_str().unwrap().to_string();
        let frames_2 = wait_for_done(&run_id_2).await;
        for frame in &frames_2 {
            assert!(!frame.to_string().contains(E2E_VALUE));
        }

        // Every redemption receipt for this grant: hash + names vocabulary
        // only, value nowhere.
        let receipts = crate::aci_approvals::GRANTS
            .receipts()
            .into_iter()
            .filter(|receipt| receipt.grant_id == approval_id)
            .collect::<Vec<_>>();
        assert!(!receipts.is_empty(), "redemption attempts must be receipted");
        for receipt in &receipts {
            let text = serde_json::to_string(receipt).unwrap();
            assert!(
                !text.contains(E2E_VALUE),
                "receipt leaked the value: {text}"
            );
        }
    }
}
