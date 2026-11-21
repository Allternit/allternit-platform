//! The node's `/v1/host/*` routes, shared by every runtime that serves them
//! (allternit-api on :8013 and the ao shim on :8014). Feature `http`.
//!
//! ```ignore
//! let api = host_probe::http::HostApi::new(credentials_fn);
//! let app = Router::new().merge(host_probe::http::router(api.clone()));
//! ```
//!
//! Routes (contract `surfaces/docs/dev/computers-console-contract.mdx`):
//!
//! | Route | Body |
//! |---|---|
//! | `GET /v1/host/snapshot` | `HostSnapshot` (one sampler kept per process, so net rates are real) |
//! | `GET /v1/host/services` | `{ services, providers }` |
//! | `GET /v1/host/ports` | `{ ports, sources }` |
//! | `GET /v1/host/updates` | `{ updates, sources, lastRefreshed?, stale? }` |
//! | `GET /v1/host/findings` | `{ findings, sources?, generatedAt, ageSec, refreshing }` from a cache |
//! | `GET /v1/host/logs/tail?unit=&lines=` | `{ lines: LogLine[] }` |
//! | `GET /v1/host/logs/stream?unit=` | SSE, one `LogLine` per `data:` |
//! | `POST /v1/host/actions` | `{ changeId, undo?, note? }` or `ActionError::body()` |
//! | `GET /v1/host/elevation` | `ElevationStatus` |
//! | `GET /v1/host/elevation/plan` | `SetupPlan` |
//! | `POST /v1/host/elevation/setup` | `ElevationStatus` after the one OS admin prompt |
//! | `GET /v1/host/forward?port=N` | WebSocket bridged to TCP `127.0.0.1:N` |
//!
//! Who may call: a request that came over the relay (`x-allternit-relayed`)
//! must carry the cloud's relay signature (`x-allternit-runtime-sig/ts/owner`,
//! keyed by this runtime's relay key). The cloud signs only after its own
//! caller check and the pairing's `runtime:execute` capability check
//! (`required_capability("/v1/host/*")`), so a valid signature is the proof
//! the pairing grants it. A local call follows the mounting app's own auth.
//! The forward socket is the exception: the relay opens it with no headers,
//! and the cloud's single-use ticket (checked against the same capability)
//! is its gate.
//!
//! Every host-probe call blocks, so it runs in `spawn_blocking`.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::actions::{run_action, ActionRequest, ApprovalVerifier};
use crate::approval::{DenyApprovals, RelayKeyApproval};
use crate::elevation::{self, ElevationError, SystemHost};
use crate::findings::FindingsReport;
use crate::logs::{LogError, LogOptions, LogTarget};
use crate::services::{ServiceOptions, ServiceProvider};
use crate::snapshot::Sampler;
use crate::updates::UpdateOptions;
use crate::{HostSnapshot, OsFamily, ProbeError};

/// Header every relay client (Desktop, agent-daemon, ao relay) sets on what
/// it replays from the cloud.
pub const RELAYED_HEADER: &str = "x-allternit-relayed";
pub const SIG_HEADER: &str = "x-allternit-runtime-sig";
pub const TS_HEADER: &str = "x-allternit-runtime-ts";
pub const OWNER_HEADER: &str = "x-allternit-owner";
/// Clock skew allowed on a relay signature (same as allternit-api relay_auth).
pub const MAX_SKEW_SECS: i64 = 300;
/// A findings report younger than this is served without a refresh.
pub const FINDINGS_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_ACTION_BODY: usize = 64 * 1024;
const MAX_TAIL_LINES: usize = 1000;
const FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What this runtime was paired with. `relay_key` = lowercase
/// `sha256_hex(device_token)`, `owner` = the paired user id, `runtime_id` =
/// the cloud's id for this runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCredentials {
    pub relay_key: String,
    pub owner: String,
    pub runtime_id: String,
}

/// Reads the current credentials (they can rotate, so read per request).
pub type CredentialsFn = Arc<dyn Fn() -> Option<NodeCredentials> + Send + Sync>;

/// Shared state of the routes: credentials, the snapshot sampler and the
/// findings cache.
pub struct HostApi {
    credentials: CredentialsFn,
    sampler: Mutex<Option<Sampler>>,
    findings: Mutex<FindingsCache>,
    scan: fn(&HostApi) -> Result<FindingsReport, ProbeError>,
}

/// The real findings scan: a snapshot, then services, ports, updates and
/// (on a nearly full disk) clean-up estimates. Slow; blocking.
fn real_scan(api: &HostApi) -> Result<FindingsReport, ProbeError> {
    let snap = api.snapshot_blocking()?;
    crate::findings::findings_report_with(&snap)
}

#[derive(Default)]
struct FindingsCache {
    report: Option<FindingsReport>,
    generated_at: Option<String>,
    at: Option<Instant>,
    refreshing: bool,
    last_error: Option<String>,
}

impl HostApi {
    pub fn new(credentials: CredentialsFn) -> Arc<Self> {
        Arc::new(HostApi {
            credentials,
            sampler: Mutex::new(None),
            findings: Mutex::new(FindingsCache::default()),
            scan: real_scan,
        })
    }

    pub fn credentials(&self) -> Option<NodeCredentials> {
        (self.credentials)()
    }

    /// The approval check for this runtime: the cloud's signed ids, or refuse
    /// everything when the runtime is not paired.
    pub fn verifier(&self) -> Box<dyn ApprovalVerifier> {
        match self.credentials() {
            Some(c) if !c.relay_key.is_empty() && !c.runtime_id.is_empty() => {
                Box::new(RelayKeyApproval::new(c.relay_key, c.runtime_id))
            }
            _ => Box::new(DenyApprovals),
        }
    }

    /// One snapshot from the process-wide sampler. Blocking.
    pub fn snapshot_blocking(&self) -> Result<HostSnapshot, ProbeError> {
        let mut guard = self.sampler.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let mut s = Sampler::new()?;
            // The first reading needs a short gap for a real CPU figure.
            std::thread::sleep(Duration::from_millis(250));
            let snap = s.sample()?;
            *guard = Some(s);
            return Ok(snap);
        }
        guard.as_mut().expect("set above").sample()
    }

    /// [`HostApi::snapshot_blocking`] off the async runtime. Used by the
    /// presence heartbeat (`hostSample`) and `GET /v1/host/snapshot`.
    pub async fn snapshot(self: &Arc<Self>) -> Result<HostSnapshot, ProbeError> {
        let api = self.clone();
        tokio::task::spawn_blocking(move || api.snapshot_blocking())
            .await
            .unwrap_or_else(|e| Err(ProbeError::Unsupported(format!("snapshot task: {e}"))))
    }

    /// The `hostSample` for a heartbeat: the snapshot as JSON, or `None`
    /// when this host cannot be sampled. Never fails the heartbeat.
    pub async fn host_sample(self: &Arc<Self>) -> Option<Value> {
        self.snapshot()
            .await
            .ok()
            .and_then(|s| serde_json::to_value(s).ok())
    }

    /// Mark the findings stale (after an action changed the host).
    pub fn invalidate_findings(&self) {
        let mut c = self.findings.lock().unwrap_or_else(|p| p.into_inner());
        c.at = None;
    }

    /// Start a background refresh unless one is running. Returns whether one
    /// was started.
    fn refresh_findings(self: &Arc<Self>) -> bool {
        {
            let mut c = self.findings.lock().unwrap_or_else(|p| p.into_inner());
            if c.refreshing {
                return false;
            }
            c.refreshing = true;
        }
        let api = self.clone();
        tokio::task::spawn_blocking(move || {
            let result = (api.scan)(&api);
            let mut c = api.findings.lock().unwrap_or_else(|p| p.into_inner());
            c.refreshing = false;
            match result {
                Ok(report) => {
                    c.report = Some(report);
                    c.generated_at = Some(chrono::Utc::now().to_rfc3339());
                    c.at = Some(Instant::now());
                    c.last_error = None;
                }
                Err(e) => c.last_error = Some(e.to_string()),
            }
        });
        true
    }

    /// The findings body: the cached report with its age, refreshed in the
    /// background when older than [`FINDINGS_TTL`]. Never waits for a scan
    /// (the updates read alone takes 45-60 s on dnf and Windows). Before the
    /// first scan finishes: 202 with `generatedAt: null` and no findings,
    /// which clients must show as "checking", never as "no problems".
    fn findings_body(self: &Arc<Self>) -> (StatusCode, Value) {
        let (fresh, has_report) = {
            let c = self.findings.lock().unwrap_or_else(|p| p.into_inner());
            (
                c.at.is_some_and(|t| t.elapsed() < FINDINGS_TTL),
                c.report.is_some(),
            )
        };
        if !fresh {
            self.refresh_findings();
        }
        let c = self.findings.lock().unwrap_or_else(|p| p.into_inner());
        let age = c.at.map(|t| t.elapsed().as_secs());
        let mut body = match &c.report {
            Some(r) => serde_json::to_value(r).unwrap_or_else(|_| json!({ "findings": [] })),
            None => json!({ "findings": [] }),
        };
        body["generatedAt"] = json!(c.generated_at);
        body["ageSec"] = json!(age);
        body["refreshing"] = json!(c.refreshing);
        if let Some(e) = &c.last_error {
            body["error"] = json!(e);
        }
        let status = if has_report {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        (status, body)
    }
}

/// The `/v1/host/*` routes with the relay gate. Merge into any router.
pub fn router<S>(api: Arc<HostApi>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let gated = Router::new()
        .route("/v1/host/snapshot", get(snapshot_route))
        .route("/v1/host/services", get(services_route))
        .route("/v1/host/ports", get(ports_route))
        .route("/v1/host/updates", get(updates_route))
        .route("/v1/host/findings", get(findings_route))
        .route("/v1/host/logs/tail", get(logs_tail_route))
        .route("/v1/host/logs/stream", get(logs_stream_route))
        .route("/v1/host/actions", post(actions_route))
        .route("/v1/host/elevation", get(elevation_status_route))
        .route("/v1/host/elevation/plan", get(elevation_plan_route))
        .route("/v1/host/elevation/setup", post(elevation_setup_route))
        .route_layer(middleware::from_fn_with_state(api.clone(), relay_gate));
    Router::new()
        .merge(gated)
        // No relay headers on a socket_open; the cloud's ticket gates it.
        .route("/v1/host/forward", get(forward_route))
        .with_state(api)
}

// ---------------------------------------------------------------------------
// Relay gate
// ---------------------------------------------------------------------------

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_relayed(headers: &HeaderMap) -> bool {
    headers
        .get(RELAYED_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| matches!(v.trim(), "1" | "true"))
        .unwrap_or(false)
}

/// Why a relayed request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReject {
    Unauthorized(&'static str),
    NotPaired,
}

/// Check the cloud's signature on a relayed request:
/// `v1=<hex HMAC-SHA256(relay_key, "<ts>.<METHOD>.<path>.<sha256_hex(body)>")>`,
/// `path` without the query string, owner = the paired owner, ±300 s.
pub fn verify_relay_signature(
    creds: Option<&NodeCredentials>,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    body: &[u8],
    now: i64,
) -> Result<(), RelayReject> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let sig = header(SIG_HEADER).ok_or(RelayReject::Unauthorized("missing relay signature"))?;
    let ts = header(TS_HEADER).ok_or(RelayReject::Unauthorized("missing relay timestamp"))?;
    let owner = header(OWNER_HEADER).ok_or(RelayReject::Unauthorized("missing relay owner"))?;
    let sig = sig
        .strip_prefix("v1=")
        .ok_or(RelayReject::Unauthorized("unsupported signature version"))?;
    let sig = hex::decode(sig).map_err(|_| RelayReject::Unauthorized("malformed signature"))?;
    let ts: i64 = ts
        .parse()
        .map_err(|_| RelayReject::Unauthorized("malformed timestamp"))?;
    let creds = creds
        .filter(|c| !c.relay_key.is_empty())
        .ok_or(RelayReject::NotPaired)?;
    if (now - ts).abs() > MAX_SKEW_SECS {
        return Err(RelayReject::Unauthorized("stale relay timestamp"));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(creds.relay_key.as_bytes())
        .expect("hmac takes any key length");
    mac.update(format!("{ts}.{method}.{path}.{}", hex::encode(Sha256::digest(body))).as_bytes());
    mac.verify_slice(&sig)
        .map_err(|_| RelayReject::Unauthorized("bad relay signature"))?;
    if owner != creds.owner {
        return Err(RelayReject::Unauthorized(
            "relay owner does not match this runtime",
        ));
    }
    Ok(())
}

async fn relay_gate(State(api): State<Arc<HostApi>>, req: Request, next: Next) -> Response {
    if !is_relayed(req.headers()) {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_ACTION_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return fail(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "body too large",
            )
        }
    };
    let path = parts
        .extensions
        .get::<axum::extract::OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());
    let creds = api.credentials();
    match verify_relay_signature(
        creds.as_ref(),
        &parts.headers,
        parts.method.as_str(),
        &path,
        &bytes,
        unix_now(),
    ) {
        Ok(()) => {
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Err(RelayReject::Unauthorized(why)) => {
            fail(StatusCode::UNAUTHORIZED, "relay_unauthorized", why)
        }
        Err(RelayReject::NotPaired) => fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "not_paired",
            "this runtime is not paired",
        ),
    }
}

// ---------------------------------------------------------------------------
// Read routes
// ---------------------------------------------------------------------------

fn fail(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

fn probe_error(e: ProbeError) -> Response {
    match e {
        ProbeError::Unsupported(m) => fail(StatusCode::NOT_IMPLEMENTED, "unsupported", &m),
        ProbeError::Io(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

/// Run a blocking read and answer its JSON.
async fn blocking_json<T, F>(f: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce() -> Result<T, ProbeError> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => probe_error(e),
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

async fn snapshot_route(State(api): State<Arc<HostApi>>) -> Response {
    match api.snapshot().await {
        Ok(s) => Json(s).into_response(),
        Err(e) => probe_error(e),
    }
}

async fn services_route() -> Response {
    blocking_json(|| {
        let mut report = crate::services::services_report(&ServiceOptions::default())?;
        if let Ok(ports) = crate::ports::ports() {
            crate::ports::attach_ports(&mut report.services, &ports);
        }
        Ok(report)
    })
    .await
}

async fn ports_route() -> Response {
    blocking_json(crate::ports::ports_report).await
}

async fn updates_route() -> Response {
    blocking_json(|| crate::updates::updates_report(&UpdateOptions::default())).await
}

async fn findings_route(State(api): State<Arc<HostApi>>) -> Response {
    let (status, body) = api.findings_body();
    (status, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LogQuery {
    unit: Option<String>,
    /// systemd | supervisord | docker | launchd | scm (optional; looked up
    /// from the service list when absent).
    provider: Option<String>,
    lines: Option<usize>,
    #[serde(default, rename = "errorsOnly", alias = "errors_only")]
    errors_only: Option<bool>,
}

fn target_for(
    provider: Option<ServiceProvider>,
    unit: &str,
    name: &str,
    os: Option<OsFamily>,
) -> LogTarget {
    match provider {
        Some(ServiceProvider::Systemd) => LogTarget::Systemd { unit: unit.into() },
        Some(ServiceProvider::Supervisord) => LogTarget::Supervisord {
            program: unit.into(),
        },
        Some(ServiceProvider::Docker) => LogTarget::Docker {
            container: unit.into(),
        },
        Some(ServiceProvider::Launchd) => LogTarget::Macos {
            process: name.into(),
        },
        Some(ServiceProvider::Scm) => LogTarget::Windows {
            provider: unit.into(),
        },
        None => match os {
            Some(OsFamily::Macos) => LogTarget::Macos {
                process: unit.into(),
            },
            Some(OsFamily::Windows) => LogTarget::Windows {
                provider: unit.into(),
            },
            _ => LogTarget::Systemd { unit: unit.into() },
        },
    }
}

/// `unit` (+ optional `provider`) to a log target. Blocking (may read the
/// service list).
fn resolve_target(q: &LogQuery) -> Result<LogTarget, Response> {
    let unit = q
        .unit
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            fail(
                StatusCode::BAD_REQUEST,
                "invalid_target",
                "unit is required",
            )
        })?;
    if !crate::logs::valid_name(unit) {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "unit is not a valid name",
        ));
    }
    if let Some(p) = q.provider.as_deref().filter(|p| !p.is_empty()) {
        let provider: ServiceProvider = serde_json::from_value(json!(p.to_ascii_lowercase()))
            .map_err(|_| {
                fail(
                    StatusCode::BAD_REQUEST,
                    "invalid_target",
                    "unknown provider",
                )
            })?;
        return Ok(target_for(Some(provider), unit, unit, OsFamily::current()));
    }
    let found = crate::services::services()
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.unit == unit || s.name == unit));
    Ok(match found {
        Some(s) => target_for(Some(s.provider), &s.unit, &s.name, OsFamily::current()),
        None => target_for(None, unit, unit, OsFamily::current()),
    })
}

fn log_error(e: LogError) -> Response {
    match e {
        LogError::InvalidTarget(m) => fail(StatusCode::BAD_REQUEST, "invalid_target", &m),
        LogError::NotAllowed(m) => fail(StatusCode::FORBIDDEN, "not_allowed", &m),
        LogError::Unsupported(m) => fail(StatusCode::NOT_IMPLEMENTED, "unsupported", &m),
        LogError::Unavailable(m) => fail(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &m),
        LogError::Io(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

async fn logs_tail_route(Query(q): Query<LogQuery>) -> Response {
    let lines = q.lines.unwrap_or(100).clamp(1, MAX_TAIL_LINES);
    let errors_only = q.errors_only.unwrap_or(false);
    let res = tokio::task::spawn_blocking(move || {
        let target = resolve_target(&q)?;
        let opts = LogOptions {
            errors_only,
            ..LogOptions::default()
        };
        crate::logs::tail_with(target, lines, &opts).map_err(log_error)
    })
    .await;
    match res {
        Ok(Ok(lines)) => Json(json!({ "lines": lines })).into_response(),
        Ok(Err(r)) => r,
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

async fn logs_stream_route(Query(q): Query<LogQuery>) -> Response {
    let errors_only = q.errors_only.unwrap_or(false);
    let opened = tokio::task::spawn_blocking(move || {
        let target = resolve_target(&q)?;
        let opts = LogOptions {
            errors_only,
            ..LogOptions::default()
        };
        crate::logs::stream(target, opts).map_err(log_error)
    })
    .await;
    let stream = match opened {
        Ok(Ok(s)) => s,
        Ok(Err(r)) => return r,
        Err(e) => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "probe_error",
                &e.to_string(),
            )
        }
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<crate::logs::LogLine>(256);
    // The pump owns the LogStream; dropping it (client gone) stops the source.
    tokio::task::spawn_blocking(move || {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            match stream.recv_timeout(Duration::from_secs(1)) {
                Ok(line) => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if tx.is_closed() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    let events = futures_util::stream::unfold(rx, |mut rx| async move {
        let line = rx.recv().await?;
        let data = serde_json::to_string(&line).unwrap_or_else(|_| "{}".into());
        Some((Ok::<Event, Infallible>(Event::default().data(data)), rx))
    });
    let mut res = Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response();
    let h = res.headers_mut();
    h.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    h.insert(
        "x-accel-buffering",
        axum::http::HeaderValue::from_static("no"),
    );
    res
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

async fn actions_route(State(api): State<Arc<HostApi>>, body: Bytes) -> Response {
    let req: ActionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return fail(
                StatusCode::BAD_REQUEST,
                "invalid_body",
                &format!("body must be {{type, target, approval_id}}: {e}"),
            )
        }
    };
    let verifier = api.verifier();
    match tokio::task::spawn_blocking(move || run_action(&req, verifier)).await {
        Ok(Ok(result)) => {
            api.invalidate_findings();
            Json(result).into_response()
        }
        Ok(Err(e)) => {
            let status =
                StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            (status, Json(e.body())).into_response()
        }
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "action_failed",
            &e.to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Elevation (one-time admin setup)
// ---------------------------------------------------------------------------

fn elevation_error(e: ElevationError) -> Response {
    let (status, code) = match &e {
        ElevationError::Unsupported(_) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
        ElevationError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid"),
        ElevationError::NeedsTerminal { .. } => (StatusCode::CONFLICT, "needs_terminal"),
        ElevationError::Cancelled => (StatusCode::CONFLICT, "cancelled"),
        ElevationError::Failed(_) => (StatusCode::INTERNAL_SERVER_ERROR, "setup_failed"),
    };
    let mut body = json!({ "error": code, "message": e.to_string() });
    if let ElevationError::NeedsTerminal { command } = &e {
        body["command"] = json!(command);
    }
    (status, Json(body)).into_response()
}

fn helper_source() -> Result<std::path::PathBuf, Response> {
    elevation::bundled_helper().ok_or_else(|| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "helper_missing",
            "the admin helper is not installed next to this runtime; update Allternit",
        )
    })
}

async fn elevation_status_route() -> Response {
    match tokio::task::spawn_blocking(|| elevation::status(&SystemHost)).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

async fn elevation_plan_route() -> Response {
    let res = tokio::task::spawn_blocking(|| {
        let src = helper_source()?;
        elevation::plan(&SystemHost, &src).map_err(elevation_error)
    })
    .await;
    match res {
        Ok(Ok(plan)) => Json(plan).into_response(),
        Ok(Err(r)) => r,
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

async fn elevation_setup_route() -> Response {
    let res = tokio::task::spawn_blocking(|| {
        let src = helper_source()?;
        elevation::install(&SystemHost, &src).map_err(elevation_error)
    })
    .await;
    match res {
        Ok(Ok(status)) => Json(status).into_response(),
        Ok(Err(r)) => r,
        Err(e) => fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "probe_error",
            &e.to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Port forward: WebSocket <-> TCP 127.0.0.1:N
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ForwardQuery {
    port: Option<String>,
}

/// `port` as 1-65535, or `None`.
pub fn parse_forward_port(raw: Option<&str>) -> Option<u16> {
    let n: u32 = raw?.trim().parse().ok()?;
    (1..=65535).contains(&n).then_some(n as u16)
}

/// The relay client sends `socket_ready` when this upgrade completes, so the
/// TCP dial happens first: a refused port answers 502 and the relay client
/// sends `socket_close` instead.
async fn forward_route(Query(q): Query<ForwardQuery>, ws: Option<WebSocketUpgrade>) -> Response {
    let Some(port) = parse_forward_port(q.port.as_deref()) else {
        return fail(
            StatusCode::BAD_REQUEST,
            "invalid_port",
            "port must be 1-65535",
        );
    };
    let Some(ws) = ws else {
        return fail(
            StatusCode::UPGRADE_REQUIRED,
            "websocket_required",
            "open this path as a WebSocket",
        );
    };
    let tcp = match tokio::time::timeout(
        FORWARD_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return fail(StatusCode::BAD_GATEWAY, "connect_failed", &e.to_string()),
        Err(_) => {
            return fail(
                StatusCode::GATEWAY_TIMEOUT,
                "connect_timeout",
                "connect timed out",
            )
        }
    };
    let _ = tcp.set_nodelay(true);
    ws.on_upgrade(move |socket| pump_forward(socket, tcp))
}

async fn pump_forward(socket: WebSocket, tcp: tokio::net::TcpStream) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (mut tcp_rx, mut tcp_tx) = tcp.into_split();
    let to_tcp = async {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let data = match msg {
                Message::Binary(b) => b,
                Message::Text(t) => t.into_bytes(),
                Message::Close(_) => break,
                _ => continue,
            };
            if tcp_tx.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = tcp_tx.shutdown().await;
    };
    let to_ws = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tcp_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.send(Message::Close(None)).await;
    };
    // Either side ending closes both.
    tokio::select! {
        _ = to_tcp => {}
        _ = to_ws => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    const TOKEN: &str = "allternit_runtime_test_token";

    fn creds() -> NodeCredentials {
        NodeCredentials {
            relay_key: crate::approval::relay_key_from_device_token(TOKEN),
            owner: "user-a".into(),
            runtime_id: "rt_box".into(),
        }
    }

    /// A scan that takes a moment and reads nothing from the host.
    fn stub_scan(_: &HostApi) -> Result<FindingsReport, ProbeError> {
        std::thread::sleep(Duration::from_millis(300));
        Ok(FindingsReport {
            findings: vec![],
            sources: vec![],
        })
    }

    fn api(paired: bool) -> Arc<HostApi> {
        Arc::new(HostApi {
            credentials: Arc::new(move || paired.then(creds)),
            sampler: Mutex::new(None),
            findings: Mutex::new(FindingsCache::default()),
            scan: stub_scan,
        })
    }

    fn sign(path: &str, method: &str, body: &[u8], ts: i64) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(creds().relay_key.as_bytes()).unwrap();
        mac.update(
            format!("{ts}.{method}.{path}.{}", hex::encode(Sha256::digest(body))).as_bytes(),
        );
        format!("v1={}", hex::encode(mac.finalize().into_bytes()))
    }

    async fn status_of(app: Router, req: HttpRequest<Body>) -> (StatusCode, Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn relayed_action(body: &str, signed: Option<(&str, i64)>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder()
            .method("POST")
            .uri("/v1/host/actions")
            .header("content-type", "application/json")
            .header(RELAYED_HEADER, "1");
        if let Some((owner, ts)) = signed {
            b = b
                .header(
                    SIG_HEADER,
                    sign("/v1/host/actions", "POST", body.as_bytes(), ts),
                )
                .header(TS_HEADER, ts.to_string())
                .header(OWNER_HEADER, owner);
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn relayed_requests_need_the_cloud_signature() {
        let body = r#"{"type":"service.delete","target":"x","approval_id":"ap-x"}"#;
        let app = || router::<()>(api(true));
        // Unsigned relayed call: refused before any handler runs.
        let (s, v) = status_of(app(), relayed_action(body, None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(v["error"], "relay_unauthorized");
        // Wrong owner.
        let (s, _) = status_of(app(), relayed_action(body, Some(("user-b", unix_now())))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // Stale timestamp.
        let (s, _) = status_of(
            app(),
            relayed_action(body, Some(("user-a", unix_now() - 3600))),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // Good signature reaches the handler, which refuses the unknown type
        // (400), proving the body survived the gate intact.
        let (s, v) = status_of(app(), relayed_action(body, Some(("user-a", unix_now())))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"], "unknown_type");
        // Unpaired runtime: 503.
        let (s, v) = status_of(
            router::<()>(api(false)),
            relayed_action(body, Some(("user-a", unix_now()))),
        )
        .await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(v["error"], "not_paired");
    }

    #[tokio::test]
    async fn a_local_action_without_a_cloud_approval_is_403() {
        // Not relayed: the mounting app's auth applies; the approval check
        // still refuses anything the cloud did not sign.
        let body =
            r#"{"type":"service.stop","target":"nginx.service","approval_id":"ap-anything"}"#;
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/host/actions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let (s, v) = status_of(router::<()>(api(true)), req).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], "not_approved");
    }

    #[test]
    fn forward_port_is_1_to_65535() {
        assert_eq!(parse_forward_port(Some("22")), Some(22));
        assert_eq!(parse_forward_port(Some("65535")), Some(65535));
        for bad in [
            None,
            Some(""),
            Some("0"),
            Some("65536"),
            Some("-1"),
            Some("ssh"),
        ] {
            assert_eq!(parse_forward_port(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn forward_rejects_a_bad_port_before_upgrading() {
        let req = HttpRequest::builder()
            .uri("/v1/host/forward?port=0")
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Body::empty())
            .unwrap();
        let (s, v) = status_of(router::<()>(api(true)), req).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["error"], "invalid_port");
    }

    #[tokio::test]
    async fn findings_never_wait_for_a_scan() {
        let a = api(true);
        // Before any scan: 202, no findings, generatedAt null, refreshing.
        let (s, body) = a.findings_body();
        assert_eq!(s, StatusCode::ACCEPTED);
        assert_eq!(body["findings"], json!([]));
        assert!(body["generatedAt"].is_null());
        assert_eq!(body["refreshing"], true);
        // Let the first (stub) scan finish, then pin a known report.
        tokio::time::sleep(Duration::from_millis(600)).await;
        // A cached report is served with its age.
        {
            let mut c = a.findings.lock().unwrap();
            c.report = Some(FindingsReport {
                findings: vec![],
                sources: vec![],
            });
            c.generated_at = Some("2026-11-21T00:00:00Z".into());
            c.at = Some(Instant::now());
            c.refreshing = false;
        }
        let (s, body) = a.findings_body();
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["generatedAt"], "2026-11-21T00:00:00Z");
        assert_eq!(body["ageSec"], 0);
        assert_eq!(body["refreshing"], false);
        // After an action the cache is stale: still served, refresh started.
        a.invalidate_findings();
        let (s, body) = a.findings_body();
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["refreshing"], true);
    }

    #[test]
    fn verifier_follows_pairing() {
        assert!(api(false)
            .verifier()
            .verify("ap1.x", crate::ActionType::PortFree, "80")
            .is_err());
        let id = crate::approval::mint_for_test(
            &creds().relay_key,
            "rt_box",
            "port.free",
            "80",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            unix_now() + 60,
        );
        assert_eq!(
            api(true)
                .verifier()
                .verify(&id, crate::ActionType::PortFree, "80"),
            Ok(())
        );
    }
}
