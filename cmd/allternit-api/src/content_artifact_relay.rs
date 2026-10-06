//! Content-artifact relay routes — A:// Artifacts API org relay tier
//! (docs/design/artifacts-api.md §6 relay tier, IMPLEMENTED 2026-09-12).
//!
//! An artifact travels between gateways as a portable relay bundle:
//! current-version HTML + metadata + the provenance (relay) chain. The
//! receiving gateway mints a NEW LOCAL id (§6 decision 5) — local ids stay
//! local — and records the origin id + relay path in provenance. Received
//! artifacts render under the standard sandbox policy with provenance
//! displayed (§6 decision 6): no stricter received sandbox, no policy
//! promotion flow.
//!
//! Transport: the bundle rides as a Factory-shaped envelope
//! (`allternit.content-artifact.relay/v1`) over an HTTP inbox POST between
//! gateway base URLs. The Factory UDS envelope path was rejected because
//! the receiving gateway runs no UDS listener; the Factory bus inbox is
//! keyed per data_dir (`.allternit/bus/queue.db`), so it cannot span two
//! gateway instances with separate data dirs — the org-mesh topology this
//! tier exists for. The Factory still provides the substrate pieces that
//! matter: the envelope/identity conventions and the durable local ledger —
//! every relayed bundle is recorded as a `ContentArtifactRelayed` ledger
//! event on the sending and receiving gateways.
//!
//! Gateway identity is `ALLTERNIT_GATEWAY_NAME` (default `local`). Peer
//! gateways are configured with `ALLTERNIT_RELAY_PEERS`, a comma-separated
//! `name=http(s)://host:port` list. Both are read at request time so an
//! operator can re-point peers without a restart.
//!
//! Endpoints:
//! - `POST /api/v1/content-artifacts/:id/relay` (Clerk auth) — package the
//!   current version and send it to `{target}`. Idempotent per bundle: a
//!   retried send with the same artifact+version+target replays the original
//!   receipt instead of re-delivering.
//! - `POST /api/v1/content-artifacts/relay/inbox` (public router; per-handler
//!   `internal_auth::require_internal_token`, with the same localhost
//!   local-dev bypass as other internal routes) — receive a bundle, mint a
//!   local id, store provenance. Idempotent by bundle hash: a replayed
//!   bundle returns the already-minted local artifact id (200, not 201).
//!
//! Read addressing: `GET /api/v1/content-artifacts/:id` accepts
//! `a://artifact/<id>@<gateway>`-style ids. A local hit on the prefix always
//! wins; on a local miss with a `@peer` suffix the gateway proxies the read
//! from the named peer (read-through — nothing is persisted locally) and
//! annotates the response with `resolvedVia: "relay"` + `resolvedFrom`.

use axum::extract::Extension;
use axum::{
    extract::{Json, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::auth::AuthUser;
use crate::content_artifact_routes::{
    fetch_artifact_meta, insert_version, now_rfc3339, read_version_body, sha256_hex,
};
use crate::internal_auth::require_internal_token;
use crate::AppState;

/// Bundle envelope kind — the Factory-style type tag carried in every relay.
pub(crate) const RELAY_ENVELOPE: &str = "allternit.content-artifact.relay/v1";

/// Inbox path on the receiving gateway (same path this gateway serves).
const INBOX_PATH: &str = "/api/v1/content-artifacts/relay/inbox";

/// Outbound relay request timeout.
const RELAY_TIMEOUT: Duration = Duration::from_secs(30);

/// Read-through (`@peer`) request timeout.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(15);

/// The relay routers' route table. User-authenticated send side — merged
/// into the protected `/api/v1` nest by `main.rs`.
pub fn content_artifact_relay_router() -> Router<Arc<AppState>> {
    Router::new().route("/content-artifacts/:id/relay", post(relay_content_artifact))
}

/// The receiving side — merged into the PUBLIC router by `main.rs` because
/// peer gateways carry the internal service token, not a Clerk JWT. Every
/// handler gates itself with `require_internal_token`.
pub fn content_artifact_relay_inbox_router() -> Router<Arc<AppState>> {
    Router::new().route(INBOX_PATH, post(relay_inbox))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Gateway identity + peer map
// ═══════════════════════════════════════════════════════════════════════════════

/// This gateway's name in relay provenance and the peer map.
pub(crate) fn gateway_name() -> String {
    std::env::var("ALLTERNIT_GATEWAY_NAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

/// `ALLTERNIT_RELAY_PEERS` — comma-separated `name=http(s)://host:port`.
/// Read per request so operators can re-point peers without a restart.
pub(crate) fn relay_peers_from_env() -> Vec<(String, String)> {
    std::env::var("ALLTERNIT_RELAY_PEERS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|pair| {
                    let (name, url) = pair.split_once('=')?;
                    let name = name.trim();
                    let url = url.trim().trim_end_matches('/');
                    if name.is_empty() || !url.starts_with("http") {
                        return None;
                    }
                    Some((name.to_string(), url.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a peer name to its base URL (case-insensitive, trailing slash
/// already stripped by the parser).
pub(crate) fn resolve_peer(peers: &[(String, String)], name: &str) -> Option<String> {
    let needle = name.trim().to_ascii_lowercase();
    peers
        .iter()
        .find(|(p, _)| p.to_ascii_lowercase() == needle)
        .map(|(_, url)| url.clone())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Relay bundle
// ═══════════════════════════════════════════════════════════════════════════════

/// One hop of the relay chain — who held the artifact at each step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayHop {
    pub gateway: String,
    pub artifact_id: String,
    pub version: i64,
}

/// Origin metadata carried with the bundle. `sandbox_policy` is the ORIGIN's
/// policy — receivers force `standard` on the local row (decision 6) and keep
/// the origin policy here for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayOrigin {
    pub gateway: String,
    pub artifact_id: String,
    pub version: i64,
    pub user_id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub sandbox_policy: String,
    pub project_id: Option<String>,
    pub source_session_id: Option<String>,
    pub prompt: Option<String>,
    pub design_system_id: Option<String>,
    pub skill_id: Option<String>,
    pub skill_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayArtifact {
    pub title: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub body: String,
    pub body_sha256: String,
}

/// The portable bundle sent between gateways.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelayBundle {
    pub envelope: String,
    pub bundle_hash: String,
    pub sent_at: String,
    pub origin: RelayOrigin,
    pub relay_chain: Vec<RelayHop>,
    pub artifact: RelayArtifact,
}

/// Deterministic bundle hash: sha256 over the envelope kind, origin identity,
/// version, body hash, and relay chain. Two gateways computing the hash for
/// the same logical bundle always agree, which is what makes receive
/// idempotency by bundle hash work across machines.
pub(crate) fn relay_bundle_hash(
    origin_gateway: &str,
    artifact_id: &str,
    version: i64,
    body_sha256: &str,
    relay_chain: &[RelayHop],
) -> String {
    let chain = relay_chain
        .iter()
        .map(|h| format!("{}:{}:{}", h.gateway, h.artifact_id, h.version))
        .collect::<Vec<_>>()
        .join(",");
    sha256_hex(&format!(
        "{RELAY_ENVELOPE}\n{origin_gateway}\n{artifact_id}\n{version}\n{body_sha256}\n{chain}"
    ))
}

/// Build the bundle for sending `meta`'s current version to a peer.
/// `prior_chain` is the relay provenance chain this artifact already carries
/// (empty for a locally-created artifact); this gateway's hop is appended so
/// the chain grows one entry per relay.
pub(crate) fn build_bundle(
    gateway: &str,
    meta: &crate::content_artifact_routes::ArtifactMeta,
    body: &str,
    body_sha256: &str,
    user_id: &str,
    prior_chain: Vec<RelayHop>,
) -> RelayBundle {
    let mut relay_chain = prior_chain;
    relay_chain.push(RelayHop {
        gateway: gateway.to_string(),
        artifact_id: meta.id.clone(),
        version: meta.current_version,
    });
    let bundle_hash = relay_bundle_hash(
        gateway,
        &meta.id,
        meta.current_version,
        body_sha256,
        &relay_chain,
    );
    RelayBundle {
        envelope: RELAY_ENVELOPE.to_string(),
        bundle_hash,
        sent_at: now_rfc3339(),
        origin: RelayOrigin {
            gateway: gateway.to_string(),
            artifact_id: meta.id.clone(),
            version: meta.current_version,
            user_id: user_id.to_string(),
            title: meta.title.clone(),
            artifact_type: meta.artifact_type.clone(),
            sandbox_policy: meta.sandbox_policy.clone(),
            project_id: meta.project_id.clone(),
            source_session_id: meta.source_session_id.clone(),
            prompt: meta.prompt.clone(),
            design_system_id: meta.design_system_id.clone(),
            skill_id: meta.skill_id.clone(),
            skill_name: meta.skill_name.clone(),
        },
        relay_chain,
        artifact: RelayArtifact {
            title: meta.title.clone(),
            artifact_type: meta.artifact_type.clone(),
            body: body.to_string(),
            body_sha256: body_sha256.to_string(),
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Relay provenance (receive side storage) — read helpers shared with
// content_artifact_routes so reads can attach provenance.relay.
// ═══════════════════════════════════════════════════════════════════════════════

/// Relay provenance for one artifact, as attached to reads under
/// `provenance.relay`.
pub(crate) fn relay_provenance_json(
    origin_gateway: String,
    origin_artifact_id: String,
    origin_version: i64,
    origin_sandbox_policy: String,
    relay_chain: Vec<RelayHop>,
    bundle_hash: String,
    received_at: String,
) -> serde_json::Value {
    json!({
        "originGateway": origin_gateway,
        "originArtifactId": origin_artifact_id,
        "originVersion": origin_version,
        "originSandboxPolicy": origin_sandbox_policy,
        "relayPath": relay_chain,
        "bundleHash": bundle_hash,
        "receivedAt": received_at,
    })
}

fn parse_hops(raw: &str) -> Vec<RelayHop> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Fetch relay provenance for one artifact (None for locally-created rows).
pub(crate) fn fetch_relay_provenance(
    conn: &Connection,
    artifact_id: &str,
) -> Result<Option<serde_json::Value>, rusqlite::Error> {
    let row = conn
        .query_row(
            "SELECT origin_gateway, origin_artifact_id, origin_version,
                    origin_sandbox_policy, relay_chain, bundle_hash, received_at
             FROM content_artifact_relay_provenance WHERE artifact_id = ?1",
            params![artifact_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    Ok(row.map(
        |(gw, oid, ver, policy, chain, hash, at)| {
            relay_provenance_json(gw, oid, ver, policy, parse_hops(&chain), hash, at)
        },
    ))
}

/// Relay provenance for a set of artifact ids (list reads attach in one query).
pub(crate) fn fetch_relay_provenance_map(
    conn: &Connection,
    artifact_ids: &[String],
) -> Result<std::collections::HashMap<String, serde_json::Value>, rusqlite::Error> {
    let mut map = std::collections::HashMap::new();
    for id in artifact_ids {
        if let Some(v) = fetch_relay_provenance(conn, id)? {
            map.insert(id.clone(), v);
        }
    }
    Ok(map)
}

/// The prior relay chain an artifact carries (empty for locally-created rows) —
/// used when re-relaying a received artifact so the chain accumulates hops.
pub(crate) fn fetch_relay_chain(
    conn: &Connection,
    artifact_id: &str,
) -> Result<Vec<RelayHop>, rusqlite::Error> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT relay_chain FROM content_artifact_relay_provenance WHERE artifact_id = ?1",
            params![artifact_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.map(|r| parse_hops(&r)).unwrap_or_default())
}

// ═══════════════════════════════════════════════════════════════════════════════
// POST /content-artifacts/:id/relay — package + send
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Deserialize)]
struct RelayBody {
    target: String,
}

async fn relay_content_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<RelayBody>,
) -> impl IntoResponse {
    let target = body.target.trim().to_string();
    if target.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "target is required"})),
        )
            .into_response();
    }
    let peers = relay_peers_from_env();
    let Some(peer_base) = resolve_peer(&peers, &target) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("unknown relay peer '{target}' — configure ALLTERNIT_RELAY_PEERS"),
            })),
        )
            .into_response();
    };

    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let user_id = user.user_id.clone();
    let gw = gateway_name();
    let id_c = id.clone();
    let gw_c = gw.clone();

    // Package the current version inside spawn_blocking (DB + disk reads).
    let packaged = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let meta = match fetch_artifact_meta(&conn, &id_c, &user_id)? {
            Some(m) => m,
            None => return Ok(None),
        };
        let (body, sha, _, _) =
            read_version_body(&conn, &data_dir, &id_c, meta.current_version)?
                .expect("current_version row exists");
        let prior_chain = fetch_relay_chain(&conn, &id_c)?;
        let bundle = build_bundle(&gw_c, &meta, &body, &sha, &user_id, prior_chain);

        // Send dedupe: an identical artifact+version+target hashes to the same
        // bundle; a recorded receipt means we already delivered it.
        let existing: Option<String> = conn
            .query_row(
                "SELECT local_artifact_id FROM content_artifact_relay_receipts
                 WHERE bundle_hash = ?1 AND direction = 'sent'",
                params![&bundle.bundle_hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(Some((bundle, existing)))
    })
    .await;

    let (bundle, prior_receipt) = match packaged {
        Ok(Ok(Some(v))) => v,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "not found"})),
            )
                .into_response();
        }
        Ok(Err(e)) => return db_error("packaging relay bundle", e),
        Err(_) => return internal_error(),
    };

    if let Some(local_id) = prior_receipt {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "replayed": true,
                "target": target,
                "artifactId": local_id,
                "bundleHash": bundle.bundle_hash,
            })),
        )
            .into_response();
    }

    // Deliver over HTTP to the peer's inbox.
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(RELAY_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("relay http client build failed: {e}");
            return internal_error();
        }
    };
    let mut req = client
        .post(format!("{peer_base}{INBOX_PATH}"))
        .json(&bundle);
    if let Some(token) = state.config.internal_service_token() {
        req = req.header("x-allternit-internal-token", token);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("relay delivery to '{target}' failed: {e}")})),
            )
                .into_response();
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": format!("relay peer '{target}' rejected the bundle: {status}"),
                "detail": detail,
            })),
        )
            .into_response();
    }
    let received: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": format!("relay peer '{target}' returned an unreadable response: {e}")})),
            )
                .into_response();
        }
    };

    // Record the sent receipt (best-effort; the delivery itself already landed).
    let db = state.db.clone();
    let bundle_hash = bundle.bundle_hash.clone();
    let local_id = id.clone();
    let target_c = target.clone();
    let origin_version = bundle.origin.version;
    let chain_json = serde_json::to_string(&bundle.relay_chain).unwrap_or_else(|_| "[]".to_string());
    let recorded = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT OR IGNORE INTO content_artifact_relay_receipts
                 (bundle_hash, direction, peer_gateway, origin_gateway,
                  origin_artifact_id, origin_version, local_artifact_id,
                  relay_chain, created_at)
             VALUES (?1, 'sent', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                bundle_hash,
                target_c,
                gw,
                local_id,
                origin_version,
                local_id,
                chain_json,
                now_rfc3339(),
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    // Factory substrate record: the relay is a durable local ledger event.
    let _ = state
        .rails
        .ledger
        .append(allternit_factory_engine::core::types::AllternitEvent {
            event_id: allternit_factory_engine::core::ids::create_event_id(),
            ts: now_rfc3339(),
            actor: allternit_factory_engine::core::types::Actor {
                r#type: allternit_factory_engine::core::types::ActorType::Gate,
                id: gateway_name(),
            },
            scope: None,
            r#type: "ContentArtifactRelayed".to_string(),
            payload: json!({
                "direction": "sent",
                "peer": target,
                "originArtifactId": id,
                "originVersion": origin_version,
                "bundleHash": bundle.bundle_hash,
                "receivedAs": received["artifactId"],
            }),
            provenance: None,
        })
        .await;

    match recorded {
        Ok(Ok(())) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "target": target,
                "bundleHash": bundle.bundle_hash,
                "received": received,
            })),
        )
            .into_response(),
        _ => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POST /api/v1/content-artifacts/relay/inbox — receive + mint (public router,
// internal-token gated)
// ═══════════════════════════════════════════════════════════════════════════════

async fn relay_inbox(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(bundle): Json<RelayBundle>,
) -> impl IntoResponse {
    if let Err(status) = require_internal_token(&headers, &state) {
        return (
            status,
            Json(json!({"error": "unauthorized relay peer"})),
        )
            .into_response();
    }
    if bundle.envelope != RELAY_ENVELOPE {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unsupported relay envelope '{}'", bundle.envelope)})),
        )
            .into_response();
    }
    if bundle.artifact.body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "relay bundle has an empty body"})),
        )
            .into_response();
    }
    // Integrity: recompute the bundle hash from the payload (origin identity +
    // body hash + chain) and reject tampered or malformed bundles.
    let recomputed = relay_bundle_hash(
        &bundle.origin.gateway,
        &bundle.origin.artifact_id,
        bundle.origin.version,
        &bundle.artifact.body_sha256,
        &bundle.relay_chain,
    );
    if recomputed != bundle.bundle_hash {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bundle hash mismatch"})),
        )
            .into_response();
    }
    if sha256_hex(&bundle.artifact.body) != bundle.artifact.body_sha256 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "body sha256 mismatch"})),
        )
            .into_response();
    }

    let db = state.db.clone();
    let data_dir = state.data_dir.clone();
    let bundle_c = bundle.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        // Idempotent by bundle hash: a replayed delivery returns the
        // already-minted local artifact instead of duplicating it.
        let existing: Option<String> = tx
            .query_row(
                "SELECT local_artifact_id FROM content_artifact_relay_receipts
                 WHERE bundle_hash = ?1 AND direction = 'received'",
                params![&bundle_c.bundle_hash],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(local_id) = existing {
            tx.commit()?;
            return Ok((true, local_id, 1));
        }

        // Mint a NEW LOCAL id (decision 5). The receiving user is the origin
        // user id — the org mesh assumes shared user identity across org
        // gateways (documented in artifacts-api.md §6).
        let local_id = format!("art_{}", uuid::Uuid::new_v4());
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO content_artifacts
                 (id, user_id, title, type, project_id, source_session_id, prompt,
                  design_system_id, skill_id, skill_name, sandbox_policy, thumbnail,
                  current_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'standard', NULL, 1, ?11, ?11)",
            params![
                local_id,
                bundle_c.origin.user_id,
                bundle_c.artifact.title,
                bundle_c.artifact.artifact_type,
                bundle_c.origin.project_id,
                bundle_c.origin.source_session_id,
                bundle_c.origin.prompt,
                bundle_c.origin.design_system_id,
                bundle_c.origin.skill_id,
                bundle_c.origin.skill_name,
                &now,
            ],
        )?;
        // Received artifacts render under the standard policy (decision 6);
        // the origin policy is preserved in the provenance row for display.
        insert_version(
            &tx,
            &data_dir,
            &local_id,
            &bundle_c.artifact.artifact_type,
            1,
            &bundle_c.artifact.body,
        )
        .map_err(|e| rusqlite::Error::InvalidParameterName(e))?;
        let chain_json =
            serde_json::to_string(&bundle_c.relay_chain).unwrap_or_else(|_| "[]".to_string());
        tx.execute(
            "INSERT INTO content_artifact_relay_provenance
                 (artifact_id, origin_gateway, origin_artifact_id, origin_version,
                  origin_sandbox_policy, relay_chain, bundle_hash, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                local_id,
                bundle_c.origin.gateway,
                bundle_c.origin.artifact_id,
                bundle_c.origin.version,
                bundle_c.origin.sandbox_policy,
                chain_json,
                bundle_c.bundle_hash,
                &now,
            ],
        )?;
        tx.execute(
            "INSERT INTO content_artifact_relay_receipts
                 (bundle_hash, direction, peer_gateway, origin_gateway,
                  origin_artifact_id, origin_version, local_artifact_id,
                  relay_chain, created_at)
             VALUES (?1, 'received', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                bundle_c.bundle_hash,
                bundle_c.origin.gateway,
                bundle_c.origin.gateway,
                bundle_c.origin.artifact_id,
                bundle_c.origin.version,
                local_id,
                chain_json,
                &now,
            ],
        )?;
        tx.commit()?;
        Ok((false, local_id, 1))
    })
    .await;

    match result {
        Ok(Ok((replayed, local_id, version))) => {
            let _ = state
                .rails
                .ledger
                .append(allternit_factory_engine::core::types::AllternitEvent {
                    event_id: allternit_factory_engine::core::ids::create_event_id(),
                    ts: now_rfc3339(),
                    actor: allternit_factory_engine::core::types::Actor {
                        r#type: allternit_factory_engine::core::types::ActorType::Gate,
                        id: gateway_name(),
                    },
                    scope: None,
                    r#type: "ContentArtifactRelayed".to_string(),
                    payload: json!({
                        "direction": "received",
                        "peer": bundle.origin.gateway,
                        "originArtifactId": bundle.origin.artifact_id,
                        "originVersion": bundle.origin.version,
                        "bundleHash": bundle.bundle_hash,
                        "localArtifactId": local_id,
                        "replayed": replayed,
                    }),
                    provenance: None,
                })
                .await;
            let payload = json!({
                "artifactId": local_id,
                "version": version,
                "bundleHash": bundle.bundle_hash,
                "replayed": replayed,
            });
            if replayed {
                (StatusCode::OK, Json(payload)).into_response()
            } else {
                (StatusCode::CREATED, Json(payload)).into_response()
            }
        }
        Ok(Err(e)) => db_error("receiving relay bundle", e),
        Err(_) => internal_error(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Read-through relay resolution for a://artifact/<id>@<gateway> — called from
// content_artifact_routes::get_content_artifact on a local miss.
// ═══════════════════════════════════════════════════════════════════════════════

/// Split an artifact id into (local id, optional peer suffix). The suffix is
/// only treated as a gateway when both sides are non-empty.
pub(crate) fn split_relay_address(id: &str) -> (String, Option<String>) {
    match id.rsplit_once('@') {
        Some((local, peer)) if !local.is_empty() && !peer.is_empty() => {
            (local.to_string(), Some(peer.to_string()))
        }
        _ => (id.to_string(), None),
    }
}

/// Proxy-read an artifact from a peer gateway (local miss + `@peer` suffix).
/// Returns the peer's artifact JSON annotated with the resolution, or an HTTP
/// error response. Read-through only — nothing is persisted locally.
pub(crate) async fn resolve_from_peer(
    peer: &str,
    local_id: &str,
    headers: &HeaderMap,
) -> Result<serde_json::Value, axum::response::Response> {
    let peers = relay_peers_from_env();
    let Some(base) = resolve_peer(&peers, peer) else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("unknown peer gateway '{peer}' — configure ALLTERNIT_RELAY_PEERS")})),
        )
            .into_response());
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(RESOLVE_TIMEOUT)
        .build()
        .map_err(|_| internal_error())?;
    let mut req = client.get(format!("{base}/api/v1/content-artifacts/{local_id}"));
    // Forward the caller's credentials when present so the peer can authorize
    // the read; the internal token additionally satisfies local-dev gateways.
    if let Some(auth) = headers.get("authorization") {
        req = req.header("authorization", auth);
    }
    let resp = req.send().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("peer '{peer}' is unreachable: {e}")})),
        )
            .into_response()
    })?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not found on local gateway or peer"})),
        )
            .into_response());
    }
    if !resp.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("peer '{peer}' returned {}", resp.status())})),
        )
            .into_response());
    }
    let mut body: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("peer '{peer}' returned an unreadable response: {e}")})),
        )
            .into_response()
    })?;
    if let Some(artifact) = body.get_mut("artifact") {
        artifact["resolvedVia"] = json!("relay");
        artifact["resolvedFrom"] = json!(peer);
    }
    Ok(body)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Shared error helpers (mirroring content_artifact_routes)
// ═══════════════════════════════════════════════════════════════════════════════

fn db_error(context: &str, e: rusqlite::Error) -> axum::response::Response {
    warn!("DB error {}: {}", context, e);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": e.to_string()})),
    )
        .into_response()
}

fn internal_error() -> axum::response::Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal error"})),
    )
        .into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_artifact_routes::tests::{
        body_json, create_artifact, test_app_state, test_user,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const RELAY_TOKEN: &str = "relay-test-token";

    /// Serializes tests that mutate the shared relay env vars
    /// (ALLTERNIT_RELAY_PEERS / ALLTERNIT_GATEWAY_NAME /
    /// ALLTERNIT_INTERNAL_SERVICE_TOKEN) so parallel tests in this module
    /// cannot stomp each other's configuration. tokio's async Mutex guard is
    /// Send, unlike std's.
    static RELAY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn inbox_app(temp: &std::path::Path) -> (axum::Router, Arc<AppState>) {
        let state = test_app_state(temp).await;
        (
            content_artifact_relay_inbox_router().with_state(state.clone()),
            state,
        )
    }

    fn bundle_for(body: &str, title: &str, gateway: &str, artifact_id: &str, version: i64) -> RelayBundle {
        let body_sha = sha256_hex(body);
        let chain = vec![RelayHop {
            gateway: gateway.to_string(),
            artifact_id: artifact_id.to_string(),
            version,
        }];
        let hash = relay_bundle_hash(gateway, artifact_id, version, &body_sha, &chain);
        RelayBundle {
            envelope: RELAY_ENVELOPE.to_string(),
            bundle_hash: hash,
            sent_at: now_rfc3339(),
            origin: RelayOrigin {
                gateway: gateway.to_string(),
                artifact_id: artifact_id.to_string(),
                version,
                user_id: "user-1".to_string(),
                title: title.to_string(),
                artifact_type: "text/html".to_string(),
                sandbox_policy: "standard".to_string(),
                project_id: None,
                source_session_id: None,
                prompt: Some("relayed".to_string()),
                design_system_id: None,
                skill_id: None,
                skill_name: None,
            },
            relay_chain: chain,
            artifact: RelayArtifact {
                title: title.to_string(),
                artifact_type: "text/html".to_string(),
                body: body.to_string(),
                body_sha256: body_sha,
            },
        }
    }

    fn inbox_request(bundle: &RelayBundle, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri(INBOX_PATH)
            .header("Content-Type", "application/json");
        if let Some(t) = token {
            b = b.header("x-allternit-internal-token", t);
        }
        b.body(Body::from(serde_json::to_string(bundle).unwrap()))
            .unwrap()
    }

    #[test]
    fn bundle_hash_is_deterministic_and_chain_sensitive() {
        let h1 = relay_bundle_hash("gw-a", "art_1", 2, "abc", &[]);
        let h2 = relay_bundle_hash("gw-a", "art_1", 2, "abc", &[]);
        assert_eq!(h1, h2);
        let hop = RelayHop {
            gateway: "gw-a".to_string(),
            artifact_id: "art_1".to_string(),
            version: 2,
        };
        let h3 = relay_bundle_hash("gw-a", "art_1", 2, "abc", std::slice::from_ref(&hop));
        assert_ne!(h1, h3, "chain hop changes the hash");
        assert_ne!(h1, relay_bundle_hash("gw-b", "art_1", 2, "abc", &[]));
        assert_ne!(h1, relay_bundle_hash("gw-a", "art_1", 3, "abc", &[]));
    }

    #[tokio::test]
    async fn peer_map_parsing_and_resolution() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var(
            "ALLTERNIT_RELAY_PEERS",
            "gw-a=http://127.0.0.1:8013/, GW-B=https://b.example.com, broken, =http://x, no-scheme",
        );
        let peers = relay_peers_from_env();
        assert_eq!(peers.len(), 2);
        assert_eq!(resolve_peer(&peers, "gw-a").as_deref(), Some("http://127.0.0.1:8013"));
        assert_eq!(resolve_peer(&peers, "gw-b").as_deref(), Some("https://b.example.com"));
        assert!(resolve_peer(&peers, "gw-c").is_none());
        std::env::remove_var("ALLTERNIT_RELAY_PEERS");
    }

    #[test]
    fn relay_address_splitting() {
        assert_eq!(
            split_relay_address("art_abc@gateway-b"),
            ("art_abc".to_string(), Some("gateway-b".to_string()))
        );
        assert_eq!(split_relay_address("art_abc"), ("art_abc".to_string(), None));
        assert_eq!(split_relay_address("art_@"), ("art_@".to_string(), None));
        assert_eq!(split_relay_address("@gw"), ("@gw".to_string(), None));
    }

    #[tokio::test]
    async fn inbox_requires_internal_token() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN", RELAY_TOKEN);
        let temp = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let (app, _state) = inbox_app(&temp).await;
        let bundle = bundle_for("<html>relayed</html>", "Relayed", "gw-a", "art_origin", 1);

        // No token at all: the test config has no local-dev bypass, so 401.
        let resp = app
            .clone()
            .oneshot(inbox_request(&bundle, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Wrong token: 401.
        let resp = app
            .clone()
            .oneshot(inbox_request(&bundle, Some("wrong")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Correct token: 201 with a fresh local id.
        let resp = app.oneshot(inbox_request(&bundle, Some(RELAY_TOKEN))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let received = body_json(resp.into_body()).await;
        let local_id = received["artifactId"].as_str().unwrap().to_string();
        assert!(local_id.starts_with("art_"));
        assert_ne!(local_id, "art_origin", "receive mints a NEW local id");

        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn inbox_mints_provenance_and_is_idempotent_by_bundle_hash() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN", RELAY_TOKEN);
        let temp = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let (app, state) = inbox_app(&temp).await;

        let bundle = bundle_for(
            "<html><body><h1>Hello from gw-a</h1></body></html>",
            "Relayed deck",
            "gw-a",
            "art_origin123",
            2,
        );
        let resp = app
            .clone()
            .oneshot(inbox_request(&bundle, Some(RELAY_TOKEN)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let first = body_json(resp.into_body()).await;
        let local_id = first["artifactId"].as_str().unwrap().to_string();

        // Replay the same bundle: same local id, 200 not 201, no duplicate.
        let resp = app
            .clone()
            .oneshot(inbox_request(&bundle, Some(RELAY_TOKEN)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let replayed = body_json(resp.into_body()).await;
        assert_eq!(replayed["artifactId"], local_id);
        assert_eq!(replayed["replayed"], true);

        // Read back through the user-facing router: body round-trips and
        // provenance.relay carries origin id + chain.
        let user_app = crate::content_artifact_routes::content_artifact_router()
            .merge(crate::content_artifact_relay::content_artifact_relay_router())
            .with_state(state.clone());
        let user = test_user("user-1", None);
        let resp = user_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{local_id}"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let read = body_json(resp.into_body()).await;
        assert_eq!(
            read["artifact"]["body"],
            "<html><body><h1>Hello from gw-a</h1></body></html>"
        );
        assert_eq!(read["artifact"]["version"], 1);
        assert_eq!(read["artifact"]["sandboxPolicy"], "standard");
        let relay = &read["artifact"]["provenance"]["relay"];
        assert_eq!(relay["originGateway"], "gw-a");
        assert_eq!(relay["originArtifactId"], "art_origin123");
        assert_eq!(relay["originVersion"], 2);
        assert_eq!(relay["relayPath"][0]["gateway"], "gw-a");
        assert_eq!(relay["relayPath"][0]["artifactId"], "art_origin123");

        // Exactly one artifact exists despite the replayed delivery.
        let resp = user_app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts")
                    .extension(user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);

        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn inbox_rejects_tampered_bundles() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN", RELAY_TOKEN);
        let temp = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let (app, _state) = inbox_app(&temp).await;

        // Hash that does not match the contents.
        let mut bad = bundle_for("<html>x</html>", "X", "gw-a", "art_o", 1);
        bad.bundle_hash = "deadbeef".to_string();
        let resp = app
            .clone()
            .oneshot(inbox_request(&bad, Some(RELAY_TOKEN)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Body swapped after hashing.
        let mut swapped = bundle_for("<html>x</html>", "X", "gw-a", "art_o", 1);
        swapped.artifact.body = "<html>evil</html>".to_string();
        let resp = app.clone().oneshot(inbox_request(&swapped, Some(RELAY_TOKEN))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Unknown envelope kind.
        let mut alien = bundle_for("<html>x</html>", "X", "gw-a", "art_o", 1);
        alien.envelope = "something-else/v9".to_string();
        let resp = app.oneshot(inbox_request(&alien, Some(RELAY_TOKEN))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
        std::fs::remove_dir_all(&temp).ok();
    }

    #[tokio::test]
    async fn relay_send_end_to_end_between_two_gateways() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN", RELAY_TOKEN);
        std::env::set_var("ALLTERNIT_GATEWAY_NAME", "gw-a");
        // Receiver gateway "gw-b" runs in-process on an ephemeral port.
        let temp_b = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_b).unwrap();
        let (inbox, state_b) = inbox_app(&temp_b).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, inbox).await.unwrap();
        });
        std::env::set_var("ALLTERNIT_RELAY_PEERS", format!("gw-b=http://127.0.0.1:{port}"));

        // Sender gateway: create an artifact, relay it to gw-b.
        let temp_a = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_a).unwrap();
        let state_a = test_app_state(&temp_a).await;
        let app_a = crate::content_artifact_routes::content_artifact_router()
            .merge(content_artifact_relay_router())
            .with_state(state_a);
        let user = test_user("user-1", None);
        let (_s, created) = create_artifact(
            &app_a,
            &user,
            "Origin deck",
            "<html><body><p>from gw-a</p></body></html>",
            json!({}),
        )
        .await;
        let origin_id = created["artifact"]["id"].as_str().unwrap().to_string();

        let resp = app_a
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/content-artifacts/{origin_id}/relay"))
                    .extension(user.clone())
                    .header("Content-Type", "application/json")
                    .body(Body::from(json!({"target": "gw-b"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let sent = body_json(resp.into_body()).await;
        assert_eq!(sent["ok"], true);
        let received_id = sent["received"]["artifactId"].as_str().unwrap().to_string();
        assert!(received_id.starts_with("art_"));
        assert_ne!(received_id, origin_id, "receiver minted its own id");

        // The received artifact is renderable on gateway B with provenance.
        let app_b_user = crate::content_artifact_routes::content_artifact_router()
            .with_state(state_b);
        let resp = app_b_user
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{received_id}"))
                    .extension(user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let read = body_json(resp.into_body()).await;
        assert_eq!(read["artifact"]["body"], "<html><body><p>from gw-a</p></body></html>");
        let relay = &read["artifact"]["provenance"]["relay"];
        assert_eq!(relay["originGateway"], "gw-a");
        assert_eq!(relay["originArtifactId"], origin_id);
        assert_eq!(relay["relayPath"].as_array().unwrap().len(), 1);

        // Send dedupe: re-POSTing the same artifact+target replays the
        // original receipt instead of delivering a second copy.
        let resp = app_a
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/content-artifacts/{origin_id}/relay"))
                    .extension(test_user("user-1", None))
                    .header("Content-Type", "application/json")
                    .body(Body::from(json!({"target": "gw-b"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resent = body_json(resp.into_body()).await;
        assert_eq!(resent["replayed"], true);
        let resp = app_b_user
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts")
                    .extension(test_user("user-1", None))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1, "no duplicate on resend");

        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
        std::env::remove_var("ALLTERNIT_GATEWAY_NAME");
        std::env::remove_var("ALLTERNIT_RELAY_PEERS");
        std::fs::remove_dir_all(&temp_a).ok();
        std::fs::remove_dir_all(&temp_b).ok();
    }

    #[tokio::test]
    async fn at_peer_read_resolves_through_peer_gateway() {
        let _guard = RELAY_ENV_LOCK.lock().await;
        std::env::set_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN", RELAY_TOKEN);
        // "Gateway B" holds an artifact; "gateway A" has none.
        let temp_b = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_b).unwrap();
        let state_b = test_app_state(&temp_b).await;
        let app_b = crate::content_artifact_routes::content_artifact_router().with_state(state_b.clone());
        let user = test_user("user-1", None);
        let (_s, created) = create_artifact(&app_b, &user, "Peer deck", "<html>peer</html>", json!({})).await;
        let peer_artifact_id = created["artifact"]["id"].as_str().unwrap().to_string();

        // The TCP-served peer has no auth middleware, so inject the user the
        // same way auth_middleware's local-dev bypass does. Nested under
        // /api/v1 exactly like the real gateway in main.rs.
        let app_b = Router::new()
            .nest("/api/v1", crate::content_artifact_routes::content_artifact_router().with_state(state_b))
            .layer(axum::middleware::from_fn(|mut req: axum::extract::Request, next: axum::middleware::Next| async move {
                req.extensions_mut().insert(test_user("user-1", None));
                next.run(req).await
            }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app_b).await.unwrap();
        });
        std::env::set_var("ALLTERNIT_RELAY_PEERS", format!("gw-b=http://127.0.0.1:{port}"));

        let temp_a = std::env::temp_dir().join(format!("relay-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_a).unwrap();
        let state_a = test_app_state(&temp_a).await;
        let app_a = crate::content_artifact_routes::content_artifact_router().with_state(state_a);

        // Local miss + @peer → proxied read, annotated.
        let resp = app_a
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{peer_artifact_id}@gw-b"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resolved = body_json(resp.into_body()).await;
        assert_eq!(resolved["artifact"]["body"], "<html>peer</html>");
        assert_eq!(resolved["artifact"]["resolvedVia"], "relay");
        assert_eq!(resolved["artifact"]["resolvedFrom"], "gw-b");

        // Unknown peer → 404 naming the peer.
        let resp = app_a
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/content-artifacts/{peer_artifact_id}@nope"))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Miss on both sides → 404.
        let resp = app_a
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts/art_missing@gw-b")
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Nothing was persisted locally by the read-through.
        let resp = app_a
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/content-artifacts")
                    .extension(user)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = body_json(resp.into_body()).await;
        assert_eq!(listed["artifacts"].as_array().unwrap().len(), 0);

        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
        std::env::remove_var("ALLTERNIT_RELAY_PEERS");
        std::fs::remove_dir_all(&temp_a).ok();
        std::fs::remove_dir_all(&temp_b).ok();
    }
}
