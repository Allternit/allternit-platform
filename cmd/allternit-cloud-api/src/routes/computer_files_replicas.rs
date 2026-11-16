//! Files drive replicas on cloud-api (plan step 15, SPEC sections 10 and 15).
//!
//! - `GET /api/v1/computer-files/:id/placement?root_id=…` (`PlacementResponse`):
//!   where to keep a replica of one root, ranked by rules over capacity,
//!   sensitivity and availability (plan D16), from the storage reports the
//!   user's computers sent in their heartbeats (`computer_files_storage`).
//!   Explainable and free: no model picks the device, and the user confirms.
//! - `/api/v1/computer-files/peer/:host_id/replicas/:replica_id/{have-need,head,blobs/:locator}`:
//!   the runtime-to-runtime relay. A cloud computer never joins the mesh, so
//!   a source computer pushes its sealed replica to it through cloud-api. The
//!   caller is a runtime (its device token, the same one it sends a host over
//!   the mesh); the host must belong to the same user. cloud-api forwards
//!   **only while the host is already connected**: it never wakes either
//!   computer and never touches either idle clock (the relayed path is under
//!   `/api/v1/files/v2/`, which `runtime_relay` never wakes or touches). An
//!   asleep host answers `503 asleep` and the source records `behind`.
//!
//! Both answer `feature_disabled` while the owner's `files_drive` flag is off.

use std::collections::HashMap;
use std::sync::Arc;

use allternit_files_core::{
    ComputerState, ErrorCode, PlacementOption, PlacementResponse, StorageDiscovery,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::computer_files::{self as cf, files_error, ResolvedComputer};
use super::computer_files_snapshot as snapshot;
use crate::ApiState;

/// A sealed blob is at most 4 MiB + 64 KiB; the relay takes 5 MiB.
const PEER_MAX_BODY: usize = 5 * 1024 * 1024;
/// A report older than this is not trusted for placement.
const STALE_REPORT_SECS: i64 = 7 * 24 * 3600;
const FRESH_REPORT_SECS: i64 = 24 * 3600;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computer-files/:id/placement", get(placement))
        .route(
            "/api/v1/computer-files/peer/:host_id/replicas/:replica_id/*rest",
            any(peer_relay).layer(DefaultBodyLimit::max(PEER_MAX_BODY)),
        )
}

// ------------------------------------------------------------- placement ----

/// One computer that could hold the replica, with what cloud-api knows of it.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub device_id: String,
    pub label: String,
    pub report: StorageDiscovery,
    pub state: ComputerState,
    /// Provisioned by cloud-api (relay only, sleeps when idle).
    pub cloud: bool,
    pub report_age_secs: i64,
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Space the candidate can give a replica: the freest volume, capped by what
/// it set aside for hosting (`capacity_bytes` 0 = no cap).
fn free_for_replica(r: &StorageDiscovery) -> u64 {
    let disk = r.volumes.iter().map(|v| v.free_bytes).max().unwrap_or(0);
    let host = &r.replica_host;
    if host.capacity_bytes == 0 {
        disk
    } else {
        disk.min(host.capacity_bytes.saturating_sub(host.used_bytes))
    }
}

/// The rules (plan D16). Pure, so the ranking is testable without a database.
/// `protected`: the root's policy says protected; such a folder never goes to
/// a cloud computer. Eligible options first, best score first.
pub(crate) fn rank(needed: u64, protected: bool, candidates: Vec<Candidate>) -> Vec<PlacementOption> {
    // 10% headroom: a replica grows with the folder.
    let need = needed.saturating_add(needed / 10);
    let mut out: Vec<PlacementOption> = candidates
        .into_iter()
        .map(|c| {
            let free = free_for_replica(&c.report);
            let ineligible = if !c.report.replica_host.enabled {
                Some("Replica hosting is turned off on this computer.".to_string())
            } else if protected && c.cloud {
                Some("Protected folders stay on your own devices.".to_string())
            } else if c.report_age_secs > STALE_REPORT_SECS {
                Some("It has not reported its storage for over a week.".to_string())
            } else if free < need {
                Some(format!("Not enough space: needs {}, has {} free.", human(need), human(free)))
            } else {
                None
            };
            let (score, reason) = match ineligible {
                Some(why) => (0, why),
                None => {
                    let mut score = 100u64;
                    let availability = match c.state {
                        ComputerState::Awake | ComputerState::Waking => {
                            score += 30;
                            "Online now"
                        }
                        ComputerState::Asleep => {
                            score += 10;
                            "Asleep; it catches up the next time it is on"
                        }
                        ComputerState::Offline => "Offline; it catches up when it is back",
                    };
                    score += if c.report_age_secs <= FRESH_REPORT_SECS { 20 } else { 5 };
                    let room = free / need.max(1);
                    score += match room {
                        r if r >= 10 => 30,
                        r if r >= 3 => 20,
                        _ => 10,
                    };
                    let removable_only = !c.report.volumes.is_empty() && c.report.volumes.iter().all(|v| v.removable);
                    if removable_only {
                        score = score.saturating_sub(20);
                    }
                    let mut reason = format!("{availability}, {} free.", human(free));
                    if removable_only {
                        reason.push_str(" Only on a removable disk.");
                    }
                    if c.cloud {
                        reason.push_str(" Encrypted copy on your cloud computer.");
                    }
                    (score, reason)
                }
            };
            PlacementOption {
                device_id: c.device_id,
                label: c.label,
                score,
                eligible: score > 0,
                reason,
                free_bytes: free,
                computer_state: c.state,
            }
        })
        .collect();
    out.sort_by(|a, b| b.eligible.cmp(&a.eligible).then(b.score.cmp(&a.score)).then(a.label.cmp(&b.label)));
    out
}

async fn placement(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let (user_id, computer) = match cf::owner_gate(&state, &headers, &id).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let Some(root_id) = query.get("root_id").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
        return files_error(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest, "root_id is required", None, None, None);
    };
    let ResolvedComputer::Runtime { runtime_id } = computer else {
        return files_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "This computer has not reported its storage yet", None, None, None);
    };
    let rows: Vec<(String, serde_json::Value, chrono::DateTime<chrono::Utc>, Option<String>)> = match sqlx::query_as(
        "SELECT s.computer_id, s.report, s.reported_at, d.name FROM computer_files_storage s \
         JOIN runtime_devices d ON d.id = s.computer_id AND d.user_id = s.user_id \
         WHERE s.user_id = $1 AND d.revoked_at IS NULL",
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => return cf::internal("placement", error),
    };
    let now = chrono::Utc::now();
    let mut source = None;
    let mut others = Vec::new();
    for (computer_id, report, reported_at, name) in rows {
        let Ok(report) = serde_json::from_value::<StorageDiscovery>(report) else { continue };
        if computer_id == runtime_id {
            source = Some(report);
        } else {
            others.push((computer_id, name, report, (now - reported_at).num_seconds()));
        }
    }
    let Some(source) = source else {
        return files_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "This computer has not reported its storage yet", None, None, None);
    };
    // The root's policy, from the asleep snapshot (the runtime checks again on create).
    let mut protected = false;
    if let Some(store) = snapshot::snapshot_store() {
        if let Some(root) = snapshot::load_manifest(store.as_ref(), &runtime_id)
            .await
            .and_then(|m| m.roots)
            .and_then(|roots| roots.into_iter().find(|r| r.id == root_id))
        {
            if !root.policy.replica {
                return files_error(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "This folder's policy does not allow replicas.", None, None, None);
            }
            protected = root.policy.protected;
        }
    }
    let mut candidates = Vec::with_capacity(others.len());
    for (device_id, name, report, age) in others {
        let cloud = cf::is_cloud_computer(&state.db, &device_id).await;
        let connected = super::runtime_relay::runtime_is_connected(&device_id).await;
        let computer_state = match (connected, cloud) {
            (true, _) => ComputerState::Awake,
            (false, true) => ComputerState::Asleep,
            (false, false) => ComputerState::Offline,
        };
        candidates.push(Candidate {
            label: name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| device_id.clone()),
            device_id,
            report,
            state: computer_state,
            cloud,
            report_age_secs: age,
        });
    }
    let needed_bytes = source.root_usage.get(&root_id).copied().unwrap_or(0);
    Json(PlacementResponse { computer_id: runtime_id, root_id, needed_bytes, options: rank(needed_bytes, protected, candidates) })
        .into_response()
}

// ------------------------------------------------------------ peer relay ----

/// The replica-host routes a peer may reach (`rest` after `/replicas/:id/`).
pub(crate) fn peer_route_allowed(method: &Method, replica_id: &str, rest: &str) -> bool {
    let id_ok = !replica_id.is_empty()
        && replica_id.len() <= 128
        && replica_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !id_ok {
        return false;
    }
    let segs: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    match (method.as_str(), segs.as_slice()) {
        ("POST", ["have-need"]) => true,
        ("GET" | "PUT", ["head"]) => true,
        ("GET" | "PUT", ["blobs", locator]) => {
            !locator.is_empty() && locator.len() <= 128 && locator.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }
        _ => false,
    }
}

async fn peer_relay(
    State(state): State<Arc<ApiState>>,
    method: Method,
    headers: HeaderMap,
    Path((host_id, replica_id, rest)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    let Some(token) = super::runtime_pairing::device_token_from_headers(&headers) else {
        return files_error(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, "A runtime device token is required", None, None, None);
    };
    let caller = match super::runtime_pairing::runtime_device_for_token(&state.db, token, None).await {
        Ok(device) => device,
        Err(error) => return error.into_response(),
    };
    if !cf::files_drive_on(&caller.user_id) {
        return cf::disabled();
    }
    if !peer_route_allowed(&method, &replica_id, &rest) {
        return files_error(StatusCode::FORBIDDEN, ErrorCode::Forbidden, "Only replica host routes go through the peer relay", None, None, None);
    }
    if host_id == caller.id {
        return files_error(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest, "A computer does not relay to itself", None, None, None);
    }
    match cf::owned_runtime(&state.db, &caller.user_id, &host_id).await {
        Ok(true) => {}
        Ok(false) => return cf::not_found(),
        Err(error) => return cf::internal("peer owner", error),
    }
    // Never a wake: a host with no relay connection is asleep or offline.
    if !super::runtime_relay::runtime_is_connected(&host_id).await {
        let cloud = cf::is_cloud_computer(&state.db, &host_id).await;
        let (code, computer_state, message) = if cloud {
            (ErrorCode::Asleep, ComputerState::Asleep, "The replica's computer is asleep; it catches up the next time it is on")
        } else {
            (ErrorCode::Offline, ComputerState::Offline, "The replica's computer is offline")
        };
        return files_error(StatusCode::SERVICE_UNAVAILABLE, code, message, Some(computer_state), None, None);
    }
    let mut relayed = HashMap::new();
    // The host introspects the source's device token and sees the same owner,
    // exactly as when the source reaches it over the mesh.
    relayed.insert("authorization".to_string(), format!("Bearer {token}"));
    if let Some(ct) = headers.get(axum::http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        relayed.insert("content-type".to_string(), ct.to_string());
    }
    let request = super::runtime_relay::RelayRequest {
        method: method.as_str().to_string(),
        path: format!("/api/v1/files/v2/replicas/{replica_id}/{}", rest.trim_start_matches('/')),
        headers: relayed,
        body: STANDARD.encode(&body),
        body_encoding: "base64".to_string(),
    };
    super::runtime_relay::relay_request_to_runtime(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &caller.user_id,
        &host_id,
        request,
    )
    .await
    .unwrap_or_else(IntoResponse::into_response)
}

#[cfg(test)]
#[path = "computer_files_replicas_tests.rs"]
mod tests;
