//! Pairing a user's own machines as remote computers (ACI P4).
//!
//! 1. In the Computers list the user asks for a pairing code
//!    (`POST /api/v1/computers/pairing-codes`, signed in). Codes are single
//!    use, expire in 10 minutes, and are stored hashed.
//! 2. On the machine, `allternit computer pair <code>` redeems it
//!    (`POST /api/v1/computers/pair`, the code is the credential). It gets a
//!    mesh preauth key for the code's user (the same Headscale path as
//!    `/api/v1/mesh/enroll`), the computer's id, and a secret for its reports.
//! 3. Once the machine is on the mesh it reports its mesh address and whether
//!    its VNC server is up (`POST /api/v1/computers/paired/:id/report`, with
//!    the secret).
//! 4. Every one of the user's devices lists them
//!    (`GET /api/v1/computers/paired`) and mirrors them as `fabric` computers.
//!    Members of the organization that was active when the code was made see
//!    them too (`shared: true`).
//! 5. The Allternit Factory runs bots on a paired computer through its
//!    engine. `POST /api/v1/computers/paired/:id/peer-ticket` gives the owner
//!    or an organization member a short-lived data-plane JWT (`aud` = the
//!    computer id, scope `factory:peer`) that the computer's engine verifies
//!    against `GET /api/v1/auth/dp-jwks`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{ApiError, ApiState};

const CODE_TTL_MINUTES: i64 = 10;
/// Unambiguous characters (no 0/O, 1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const SECRET_HEADER: &str = "x-allternit-computer-secret";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/computers/pairing-codes", post(create_pairing_code))
        .route("/api/v1/computers/pair", post(redeem_pairing_code))
        .route("/api/v1/computers/paired", get(list_paired))
        .route("/api/v1/computers/paired/:id", delete(remove_paired))
        .route("/api/v1/computers/paired/:id/report", post(report))
        .route("/api/v1/computers/paired/:id/peer-ticket", post(peer_ticket))
}

/// Scope a peer ticket carries; the computer's engine accepts nothing else.
pub const PEER_SCOPE: &str = "factory:peer";
/// The port the computer's Factory engine takes peer calls on, forwarded over
/// the mesh by `allternit computers serve` beside VNC.
pub const PEER_PORT: u16 = 3019;

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// `XXXX-XXXX`: 31^8 ≈ 8.5e11 codes, single use, 10 minutes.
pub fn new_pairing_code() -> String {
    let mut rng = rand::thread_rng();
    let mut code = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            code.push('-');
        }
        code.push(CODE_ALPHABET[rng.gen_range(0..CODE_ALPHABET.len())] as char);
    }
    code
}

/// Codes are typed by hand: ignore case, spaces and dashes.
pub fn normalize_code(code: &str) -> String {
    let raw: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if raw.len() == 8 { format!("{}-{}", &raw[..4], &raw[4..]) } else { raw }
}

/// A mesh address is in the tailnet range 100.64.0.0/10.
pub fn is_mesh_ip(ip: &str) -> bool {
    match ip.parse::<std::net::Ipv4Addr>() {
        Ok(addr) => {
            let [a, b, ..] = addr.octets();
            a == 100 && (64..=127).contains(&b)
        }
        Err(_) => false,
    }
}

/// The signed-in user, or the owner of a paired runtime (desktop device
/// token), the same two credentials `/api/v1/mesh/enroll` accepts.
async fn signed_in_user(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    signed_in_caller(state, headers).await.map(|c| c.user_id)
}

/// Who is calling: the user, and the organization active for them (from the
/// Clerk session, or the one the paired runtime was approved under).
struct Caller {
    user_id: String,
    organization_id: Option<String>,
}

async fn signed_in_caller(state: &ApiState, headers: &HeaderMap) -> Result<Caller, ApiError> {
    if let Some(token) = crate::routes::runtime_pairing::device_token_from_headers(headers) {
        let device = crate::routes::runtime_pairing::runtime_device_for_token(&state.db, token, None).await?;
        let organization_id: Option<String> =
            sqlx::query_scalar("SELECT organization_id FROM runtime_devices WHERE id = $1")
                .bind(&device.id)
                .fetch_optional(&state.db)
                .await?
                .flatten();
        return Ok(Caller { user_id: device.user_id, organization_id });
    }
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "compute").await?;
    Ok(Caller { user_id: user.id, organization_id: user.organization_id })
}

/// Whether `caller` may use a computer owned by `owner` under `org`: the
/// owner, or a member of the computer's organization.
fn may_use(caller: &Caller, owner: &str, org: Option<&str>) -> bool {
    caller.user_id == owner || (org.is_some() && caller.organization_id.as_deref() == org)
}

async fn create_pairing_code(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let code = new_pairing_code();
    let expires_at = Utc::now() + Duration::minutes(CODE_TTL_MINUTES);
    sqlx::query("INSERT INTO computer_pairing_codes (code_hash, user_id, expires_at, organization_id) VALUES ($1, $2, $3, $4)")
        .bind(sha256_hex(&code))
        .bind(&caller.user_id)
        .bind(expires_at)
        .bind(&caller.organization_id)
        .execute(&state.db)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "code": code,
            "expiresAt": expires_at.to_rfc3339(),
            "command": format!("allternit computer pair {code}"),
        })),
    )
        .into_response())
}

#[derive(Deserialize)]
struct RedeemBody {
    code: String,
    name: String,
    #[serde(default)]
    os: Option<String>,
}

async fn redeem_pairing_code(State(state): State<Arc<ApiState>>, Json(body): Json<RedeemBody>) -> Result<Response, ApiError> {
    let code_hash = sha256_hex(&normalize_code(&body.code));
    // Claim the code atomically: only an unused, unexpired code works, once.
    let claimed: Option<(String, Option<String>)> = sqlx::query_as(
        "UPDATE computer_pairing_codes SET used_at = now()
         WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()
         RETURNING user_id, organization_id",
    )
    .bind(&code_hash)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, organization_id)) = claimed else {
        return Err(ApiError::Unauthorized("That pairing code is wrong, used, or expired. Make a new one in Computers.".into()));
    };
    let Some(mesh) = &state.mesh_service else {
        return Err(ApiError::ServiceUnavailable("The Allternit mesh isn't configured on this server.".into()));
    };
    let enrollment = mesh
        .enroll(&user_id)
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("Couldn't get a mesh key: {e}")))?;

    let name: String = body.name.trim().chars().take(80).collect();
    let name = if name.is_empty() { "Remote computer".to_string() } else { name };
    let os = body.os.map(|o| o.trim().chars().take(32).collect::<String>()).filter(|o| !o.is_empty());
    let id = format!("pc_{}", uuid::Uuid::new_v4().simple());
    let secret = hex::encode(rand::thread_rng().gen::<[u8; 32]>());
    sqlx::query("INSERT INTO paired_computers (id, user_id, name, os, secret_hash, organization_id) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(&id)
        .bind(&user_id)
        .bind(&name)
        .bind(&os)
        .bind(sha256_hex(&secret))
        .bind(&organization_id)
        .execute(&state.db)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "computerId": id,
            "secret": secret,
            "controlUrl": enrollment.control_url,
            "authKey": enrollment.auth_key,
            "authKeyExpiresAt": enrollment.expires_at.to_rfc3339(),
        })),
    )
        .into_response())
}

#[derive(Deserialize)]
struct ReportBody {
    #[serde(rename = "meshIp")]
    mesh_ip: String,
    #[serde(rename = "vncReady", default)]
    vnc_ready: bool,
    #[serde(default)]
    name: Option<String>,
}

async fn report(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ReportBody>,
) -> Result<Response, ApiError> {
    let secret = headers.get(SECRET_HEADER).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if secret.is_empty() {
        return Err(ApiError::Unauthorized("missing computer secret".into()));
    }
    if !is_mesh_ip(&body.mesh_ip) {
        return Err(ApiError::BadRequest("meshIp must be an Allternit mesh address (100.64.0.0/10)".into()));
    }
    let name = body.name.map(|n| n.trim().chars().take(80).collect::<String>()).filter(|n| !n.is_empty());
    let updated = sqlx::query(
        "UPDATE paired_computers SET mesh_ip = $3, vnc_ready = $4, name = COALESCE($5, name), last_seen_at = now()
         WHERE id = $1 AND secret_hash = $2",
    )
    .bind(&id)
    .bind(sha256_hex(secret))
    .bind(&body.mesh_ip)
    .bind(body.vnc_ready)
    .bind(&name)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::Unauthorized("unknown computer or wrong secret".into()));
    }
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

#[derive(Serialize, sqlx::FromRow)]
struct PairedComputer {
    id: String,
    name: String,
    os: Option<String>,
    mesh_ip: Option<String>,
    vnc_ready: bool,
    created_at: DateTime<Utc>,
    last_seen_at: Option<DateTime<Utc>>,
    /// Paired by someone else in the caller's organization.
    shared: bool,
}

async fn list_paired(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let computers: Vec<PairedComputer> = sqlx::query_as(
        "SELECT id, name, os, mesh_ip, vnc_ready, created_at, last_seen_at, user_id <> $1 AS shared
         FROM paired_computers
         WHERE user_id = $1 OR (organization_id IS NOT NULL AND organization_id = $2)
         ORDER BY created_at",
    )
    .bind(&caller.user_id)
    .bind(&caller.organization_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "computers": computers })).into_response())
}

/// A short-lived ticket for the computer's Factory engine (see the module
/// doc, step 5). Only the owner or a member of the computer's organization
/// gets one, and only once the computer has reported a mesh address.
async fn peer_ticket(State(state): State<Arc<ApiState>>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let caller = signed_in_caller(&state, &headers).await?;
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT user_id, organization_id, mesh_ip FROM paired_computers WHERE id = $1")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
    // Someone else's computer answers like a missing one.
    let Some((_, _, mesh_ip)) = row.filter(|(owner, org, _)| may_use(&caller, owner, org.as_deref())) else {
        return Err(ApiError::NotFound("no such paired computer".into()));
    };
    let Some(mesh_ip) = mesh_ip else {
        return Err(ApiError::Conflict(
            "That computer hasn't joined the mesh yet. Check that `allternit computers serve` is running on it.".into(),
        ));
    };
    let ticket = crate::auth::dataplane_jwt::mint(&caller.user_id, &id, PEER_SCOPE)?;
    let expires_in = crate::auth::dataplane_jwt::ttl_secs_from_env();
    Ok(Json(serde_json::json!({
        "ticket": ticket,
        "computerId": id,
        "meshIp": mesh_ip,
        "peerPort": PEER_PORT,
        "expiresIn": expires_in,
    }))
    .into_response())
}

async fn remove_paired(State(state): State<Arc<ApiState>>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user_id = signed_in_user(&state, &headers).await?;
    let removed = sqlx::query("DELETE FROM paired_computers WHERE id = $1 AND user_id = $2")
        .bind(&id)
        .bind(&user_id)
        .execute(&state.db)
        .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::NotFound("no such paired computer".into()));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_readable_and_normalize() {
        let code = new_pairing_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(code.chars().filter(|c| *c != '-').all(|c| CODE_ALPHABET.contains(&(c as u8))));
        assert_eq!(normalize_code(" abcd efgh "), "ABCD-EFGH");
        assert_eq!(normalize_code("abcd-efgh"), "ABCD-EFGH");
    }

    #[test]
    fn owner_and_organization_members_may_use_a_computer() {
        let me = |org: Option<&str>| Caller { user_id: "u1".into(), organization_id: org.map(str::to_string) };
        assert!(may_use(&me(None), "u1", None));
        assert!(may_use(&me(Some("o1")), "u2", Some("o1")));
        assert!(!may_use(&me(Some("o2")), "u2", Some("o1")));
        assert!(!may_use(&me(None), "u2", Some("o1")));
        // A computer with no organization is the owner's alone.
        assert!(!may_use(&me(Some("o1")), "u2", None));
    }

    #[test]
    fn only_mesh_addresses() {
        assert!(is_mesh_ip("100.64.3.9"));
        assert!(!is_mesh_ip("100.128.0.1"));
        assert!(!is_mesh_ip("192.168.1.10"));
        assert!(!is_mesh_ip("not-an-ip"));
    }
}
