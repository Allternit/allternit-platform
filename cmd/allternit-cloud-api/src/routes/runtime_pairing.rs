//! First-party device pairing for Allternit desktop and VPS runtimes.
//!
//! The browser approval is authorized by Clerk. The paired runtime proves
//! possession of its Ed25519 private key and receives an independently
//! revocable device credential. Provider credentials are never handled here.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use uuid::Uuid;

use crate::{
    auth::clerk::{self, ClerkUser},
    ApiError, ApiState,
};

const PAIRING_TTL_MINUTES: i64 = 10;
const CREDENTIAL_TTL_DAYS: i64 = 90;
/// How long a just-rotated-out device token stays valid. BYO boxes run two
/// components (gizzi-code, agent-daemon) off the same token and both rotate
/// independently; the grace lets the loser keep working until its own
/// rotation self-heals it instead of dying with 401s.
const ROTATION_GRACE_MINUTES: i64 = 15;
/// Device credentials minted here always carry this prefix; other routes use
/// it to tell a device token apart from a Clerk session JWT.
pub(crate) const DEVICE_TOKEN_PREFIX: &str = "allternit_runtime_";
const DEFAULT_CAPABILITIES: &[&str] = &[
    "runtime:connect",
    "runtime:execute",
    "runtime:files",
    "runtime:terminal",
    "runtime:remote_control",
    "providers:connect",
    "providers:use",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePairingRequest {
    name: String,
    runtime_type: String,
    hostname: Option<String>,
    platform: Option<String>,
    version: Option<String>,
    public_key: String,
    #[serde(default)]
    capabilities: Vec<String>,
    /// Set only by hosted runtimes auto-provisioned for a paying user.
    #[serde(default)]
    hosted_instance_id: Option<String>,
    /// One-time bootstrap secret issued to a hosted runtime during provisioning.
    #[serde(default)]
    hosted_bootstrap_token: Option<String>,
    /// Set only by provisioned per-subscription containers (P2 lane): the
    /// provisioned_instances row the init script registers against.
    #[serde(default)]
    provisioned_instance_id: Option<String>,
    /// One-time bootstrap code minted by the provisioning service and carried
    /// to the instance via cloud-init user-data; lets the init script
    /// self-approve its pairing (runtime_type "provisioned") without a
    /// Clerk session, mirroring the hosted bootstrap token.
    #[serde(default)]
    provisioned_bootstrap_token: Option<String>,
    /// One-time bootstrap secret minted by the BYO-VPS wizard and injected
    /// into the box's env file; lets a wizard-bootstrapped VPS self-approve
    /// its pairing (runtime_type "vps") without a Clerk session.
    #[serde(default)]
    byo_bootstrap_token: Option<String>,
    /// Cloud computer first-boot token (plan A2), read by the Desktop app
    /// from /etc/allternit/bootstrap.json. Also accepted as the
    /// `X-Allternit-Bootstrap-Token` header. When valid it pre-approves this
    /// same pairing for the instance's owner, forces runtimeType
    /// "provisioned" and the name "Allternit cloud computer", and links the
    /// provisioned_instances row; it is consumed at exchange, not here, so a
    /// crash between the two can retry within the token's expiry.
    #[serde(default)]
    bootstrap_token: Option<String>,
}

/// Header alternative to `CreatePairingRequest::bootstrap_token`.
pub const BOOTSTRAP_TOKEN_HEADER: &str = "x-allternit-bootstrap-token";
/// Device name every cloud computer pairs under.
pub const PROVISIONED_RUNTIME_NAME: &str = "Allternit cloud computer";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePairingResponse {
    pairing_id: String,
    device_code: String,
    user_code: String,
    challenge: String,
    verification_url: String,
    expires_at: DateTime<Utc>,
    poll_interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangePairingRequest {
    pairing_id: String,
    device_code: String,
    signature: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApprovePairingRequest {
    email: Option<String>,
    name: Option<String>,
    image_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionResponse {
    runtime_id: String,
    user_id: String,
    user_email: String,
    organization_id: Option<String>,
    device_token: String,
    token_type: &'static str,
    expires_at: DateTime<Utc>,
    capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingInfoResponse {
    pairing_id: String,
    user_code: String,
    name: String,
    runtime_type: String,
    hostname: Option<String>,
    platform: Option<String>,
    public_key_fingerprint: String,
    capabilities: Vec<String>,
    status: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct RuntimeDeviceView {
    id: String,
    name: String,
    runtime_type: String,
    hostname: Option<String>,
    platform: Option<String>,
    version: Option<String>,
    capabilities: String,
    public_key_fingerprint: String,
    status: String,
    last_seen_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    credential_expires_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct PairingRow {
    id: String,
    user_code: String,
    challenge: String,
    public_key: String,
    public_key_fingerprint: String,
    name: String,
    runtime_type: String,
    hostname: Option<String>,
    platform: Option<String>,
    version: Option<String>,
    capabilities: String,
    status: String,
    user_id: Option<String>,
    organization_id: Option<String>,
    hosted_instance_id: Option<String>,
    byo_bootstrap_token_id: Option<String>,
    provisioned_instance_id: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub(crate) struct RuntimeCredentialRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    credential_expires_at: DateTime<Utc>,
    status: String,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/runtime-pairings", post(create_pairing))
        .route("/api/v1/runtime-pairings/exchange", post(exchange_pairing))
        .route("/api/v1/runtime-pairings/code/:code", get(pairing_info))
        .route(
            "/api/v1/runtime-pairings/code/:code/approve",
            post(approve_pairing),
        )
        .route(
            "/api/v1/runtime-pairings/code/:code/deny",
            post(deny_pairing),
        )
        .route("/api/v1/runtime-devices", get(list_runtime_devices))
        // Registered before the parameterized :id route for clarity (axum
        // already prefers the static segment). Token introspection for peer
        // services — see verify_device_token.
        .route(
            "/api/v1/runtime-devices/verify-token",
            post(verify_device_token),
        )
        .route("/api/v1/runtime-devices/:id", delete(revoke_runtime_device))
        .route(
            "/api/v1/runtime-devices/:id/heartbeat",
            post(runtime_heartbeat),
        )
        .route(
            "/api/v1/runtime-devices/:id/rotate",
            post(rotate_runtime_credential),
        )
        .route("/api/v1/runtime-devices/:id/revoke-self", post(revoke_self))
}

/// The cloud computer bootstrap token from the body field, else the header;
/// blank values count as absent.
fn bootstrap_token_from(field: Option<String>, headers: &HeaderMap) -> Option<String> {
    field
        .filter(|token| !token.trim().is_empty())
        .or_else(|| {
            headers
                .get(BOOTSTRAP_TOKEN_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

async fn create_pairing(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(mut request): Json<CreatePairingRequest>,
) -> Result<(StatusCode, Json<CreatePairingResponse>), ApiError> {
    if let Some(token) = bootstrap_token_from(request.bootstrap_token.take(), &headers) {
        let instance_id =
            crate::services::provisioning::provisioned_instance_for_bootstrap_token(&state.db, &token)
                .await?;
        request.runtime_type = "provisioned".to_string();
        request.name = PROVISIONED_RUNTIME_NAME.to_string();
        request.provisioned_instance_id = Some(instance_id);
        request.provisioned_bootstrap_token = Some(token);
    }
    validate_pairing_request(&request)?;

    let pairing_id = Uuid::new_v4().to_string();
    let device_code = random_secret(32);
    let user_code = generate_user_code();
    let challenge = random_secret(32);
    let device_code_hash = sha256_hex(device_code.as_bytes());
    let public_key_bytes = decode_public_key(&request.public_key)?;
    let public_key_fingerprint = sha256_hex(&public_key_bytes);
    let capabilities = normalized_capabilities(request.capabilities)?;
    let capabilities_json = serde_json::to_string(&capabilities)?;
    let expires_at = Utc::now() + Duration::minutes(PAIRING_TTL_MINUTES);

    // Server-initiated runtimes carry a one-time bootstrap token. If valid,
    // we create an already-approved pairing for the owning user so the machine
    // can exchange it without a Clerk session: hosted runtimes validate
    // against hosted_runtime_instances, BYO-VPS boxes against the wizard-
    // minted byo_bootstrap_tokens table.
    let bootstrap_approval = match request.runtime_type.as_str() {
        "hosted" => Some(
            validate_hosted_bootstrap(
                &state,
                request.hosted_instance_id.as_deref(),
                request.hosted_bootstrap_token.as_deref(),
            )
            .await?,
        ),
        "vps" if request.byo_bootstrap_token.is_some() => Some(
            validate_byo_bootstrap(&state.db, request.byo_bootstrap_token.as_deref()).await?,
        ),
        // P2 provisioned lane: the init script self-approves with the one-time
        // pairing code minted at provisioning time (validated against
        // provisioned_instances.pairing_code_hash; services::provisioning).
        "provisioned" => Some(
            validate_provisioned_bootstrap(
                &state.db,
                request.provisioned_instance_id.as_deref(),
                request.provisioned_bootstrap_token.as_deref(),
            )
            .await?,
        ),
        _ => None,
    };

    let status = if bootstrap_approval.is_some() {
        "approved"
    } else {
        "pending"
    };
    let user_id = bootstrap_approval
        .as_ref()
        .map(|approval| approval.user_id.clone());
    let organization_id = bootstrap_approval
        .as_ref()
        .and_then(|approval| approval.organization_id.clone());
    let hosted_instance_id = bootstrap_approval
        .as_ref()
        .and_then(|approval| approval.hosted_instance_id.clone());
    let byo_bootstrap_token_id = bootstrap_approval
        .as_ref()
        .and_then(|approval| approval.byo_bootstrap_token_id.clone());
    let provisioned_instance_id = bootstrap_approval
        .as_ref()
        .and_then(|approval| approval.provisioned_instance_id.clone());

    if let Some(ref user_id) = user_id {
        let quota = state.quota_service.ensure_quota(user_id).await?;
        state.quota_service.check_spend_cap(user_id, &quota).await?;
        state
            .quota_service
            .record_pairing_created(user_id, &quota)
            .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO runtime_pairings (
            id, device_code_hash, user_code, challenge, public_key,
            public_key_fingerprint, name, runtime_type, hostname, platform,
            version, capabilities, status, user_id, organization_id,
            hosted_instance_id, byo_bootstrap_token_id, provisioned_instance_id,
            expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
        "#,
    )
    .bind(&pairing_id)
    .bind(device_code_hash)
    .bind(&user_code)
    .bind(&challenge)
    .bind(&request.public_key)
    .bind(public_key_fingerprint)
    .bind(request.name.trim())
    .bind(&request.runtime_type)
    .bind(request.hostname.as_deref())
    .bind(request.platform.as_deref())
    .bind(request.version.as_deref())
    .bind(capabilities_json)
    .bind(status)
    .bind(user_id.as_deref())
    .bind(organization_id.as_deref())
    .bind(hosted_instance_id.as_deref())
    .bind(byo_bootstrap_token_id.as_deref())
    .bind(provisioned_instance_id.as_deref())
    .bind(expires_at)
    .execute(&state.db)
    .await?;

    let platform_url = std::env::var("ALLTERNIT_PLATFORM_URL")
        .unwrap_or_else(|_| "https://ai.allternit.com".to_string());
    let verification_url = format!(
        "{}/pair?code={}",
        platform_url.trim_end_matches('/'),
        user_code
    );

    Ok((
        StatusCode::CREATED,
        Json(CreatePairingResponse {
            pairing_id,
            device_code,
            user_code,
            challenge,
            verification_url,
            expires_at,
            poll_interval_seconds: 2,
        }),
    ))
}

async fn pairing_info(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<PairingInfoResponse>, ApiError> {
    let _user = approver_from_headers(&state, &headers).await?;
    let pairing = pairing_by_code(&state, &normalize_user_code(&code)).await?;
    ensure_pairing_live(&state, &pairing).await?;
    Ok(Json(pairing_info_response(pairing)))
}

async fn approve_pairing(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
    profile: Option<Json<ApprovePairingRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let profile = profile.map(|Json(value)| value).unwrap_or_default();
    let user = approver_from_headers(&state, &headers).await?;
    let pairing = pairing_by_code(&state, &normalize_user_code(&code)).await?;
    ensure_pairing_live(&state, &pairing).await?;
    if pairing.status == "approved" {
        if pairing.user_id.as_deref() != Some(user.id.as_str()) {
            return Err(ApiError::Forbidden(
                "This pairing request was approved by another account".to_string(),
            ));
        }
        let callback_url = format!("allternit://pairing/complete?pairing_id={}", pairing.id);
        return Ok(Json(serde_json::json!({
            "status": "approved",
            "pairingId": pairing.id,
            "runtimeName": pairing.name,
            "desktopCallbackUrl": callback_url,
        })));
    }
    if pairing.status != "pending" {
        return Err(ApiError::BadRequest(
            "This pairing request cannot be approved".to_string(),
        ));
    }

    // Persist the Clerk-authenticated user before quota checks so that the
    // user_runtime_quotas foreign key (REFERENCES users(id)) is satisfied when
    // ensure_quota lazily creates the quota row.
    // Upsert the users row first. Both quota and usage tables FK-reference
    // users(id), so creating the user before touching them avoids a latent
    // foreign-key failure if SQLite foreign-key enforcement is ever enabled.
    let email = user
        .email
        .clone()
        .or_else(|| profile.email.filter(|value| value.contains('@')))
        .unwrap_or_else(|| format!("{}@users.allternit.local", user.id));
    let name = user.name.as_deref().or(profile.name.as_deref());
    let image_url = user.image_url.as_deref().or(profile.image_url.as_deref());
    sqlx::query(
        r#"
        INSERT INTO users (id, email, name, avatar_url, status, last_login_at)
        VALUES ($1, $2, $3, $4, 'active', CURRENT_TIMESTAMP)
        ON CONFLICT(id) DO UPDATE SET
            email = excluded.email,
            name = COALESCE(excluded.name, users.name),
            avatar_url = COALESCE(excluded.avatar_url, users.avatar_url),
            status = 'active',
            last_login_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&user.id)
    .bind(&email)
    .bind(name)
    .bind(image_url)
    .execute(&state.db)
    .await?;

    // Ensure the user has a quota row and enforce guardrails before approval.
    let quota = state.quota_service.ensure_quota(&user.id).await?;
    state
        .quota_service
        .check_spend_cap(&user.id, &quota)
        .await?;
    state
        .quota_service
        .record_pairing_approved(&user.id)
        .await?;

    sqlx::query(
        r#"
        UPDATE runtime_pairings
        SET status = 'approved', user_id = $1, organization_id = $2, approved_at = CURRENT_TIMESTAMP
        WHERE id = $3 AND status = 'pending'
        "#,
    )
    .bind(&user.id)
    .bind(user.organization_id.as_deref())
    .bind(&pairing.id)
    .execute(&state.db)
    .await?;

    let callback_url = format!("allternit://pairing/complete?pairing_id={}", pairing.id);
    Ok(Json(serde_json::json!({
        "status": "approved",
        "pairingId": pairing.id,
        "runtimeName": pairing.name,
        "desktopCallbackUrl": callback_url,
    })))
}

async fn deny_pairing(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _user = approver_from_headers(&state, &headers).await?;
    let code = normalize_user_code(&code);
    let affected = sqlx::query(
        "UPDATE runtime_pairings SET status = 'denied' WHERE user_code = $1 AND status = 'pending'",
    )
    .bind(code)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::NotFound("Pairing request not found".to_string()));
    }
    Ok(Json(serde_json::json!({ "status": "denied" })))
}

/// `runtime_devices.kind` is NOT NULL with CHECK (local|paired|provisioned).
/// Binding SQL NULL (the old `Option` map) violates that constraint even
/// though the column has a default — PostgreSQL does not apply DEFAULT when
/// NULL is supplied explicitly — and the exchange 500'd with DATABASE_ERROR
/// before a desktop handshake could finish.
fn runtime_device_kind(runtime_type: &str, provisioned_instance_id: Option<&str>) -> &'static str {
    if provisioned_instance_id.is_some() {
        crate::services::NodeKind::PROVISIONED
    } else if runtime_type == "desktop" {
        crate::services::NodeKind::LOCAL
    } else {
        crate::services::NodeKind::PAIRED
    }
}

async fn exchange_pairing(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<ExchangePairingRequest>,
) -> Result<Response, ApiError> {
    let pairing = sqlx::query_as::<_, PairingRow>(
        r#"
        SELECT id, user_code, challenge, public_key, public_key_fingerprint,
               name, runtime_type, hostname, platform, version, capabilities,
               status, user_id, organization_id, hosted_instance_id,
               byo_bootstrap_token_id, provisioned_instance_id, expires_at
        FROM runtime_pairings
        WHERE id = $1 AND device_code_hash = $2
        "#,
    )
    .bind(&request.pairing_id)
    .bind(sha256_hex(request.device_code.as_bytes()))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::Unauthorized("Invalid pairing credentials".to_string()))?;

    if pairing.expires_at <= Utc::now() {
        sqlx::query("UPDATE runtime_pairings SET status = 'expired' WHERE id = $1")
            .bind(&pairing.id)
            .execute(&state.db)
            .await?;
        return Ok((
            StatusCode::GONE,
            Json(serde_json::json!({ "error": "expired_token", "status": "expired" })),
        )
            .into_response());
    }

    match pairing.status.as_str() {
        "pending" => {
            return Ok((
                StatusCode::PRECONDITION_REQUIRED,
                Json(serde_json::json!({
                    "error": "authorization_pending",
                    "status": "pending"
                })),
            )
                .into_response());
        }
        "denied" => {
            return Ok((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "access_denied", "status": "denied" })),
            )
                .into_response());
        }
        "approved" => {}
        _ => {
            return Err(ApiError::BadRequest(
                "Pairing request has already been consumed".to_string(),
            ));
        }
    }

    verify_pairing_signature(&pairing, &request.signature)?;
    let user_id = pairing
        .user_id
        .clone()
        .ok_or_else(|| ApiError::Internal("Approved pairing has no user".to_string()))?;

    // Check the active-device cap before opening a transaction. The daily
    // pairing count is recorded *inside* the transaction, after the device
    // row exists: incrementing first meant a failed insert (or a desktop
    // poll retry) burned a slot even though the handshake never completed.
    let quota = state.quota_service.ensure_quota(&user_id).await?;
    state
        .quota_service
        .check_active_device_cap(&user_id, &quota, pairing.hostname.as_deref(), pairing.platform.as_deref())
        .await?;
    let count_daily_pairing = pairing.hosted_instance_id.is_none()
        && pairing.byo_bootstrap_token_id.is_none()
        && pairing.provisioned_instance_id.is_none();

    let runtime_id = format!("rt_{}", Uuid::new_v4().simple());
    let device_token = format!("{DEVICE_TOKEN_PREFIX}{}", random_secret(48));
    let credential_hash = sha256_hex(device_token.as_bytes());
    let credential_expires_at = Utc::now() + Duration::days(CREDENTIAL_TTL_DAYS);
    let kind = runtime_device_kind(
        &pairing.runtime_type,
        pairing.provisioned_instance_id.as_deref(),
    );

    let mut transaction = state.db.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO runtime_devices (
            id, user_id, organization_id, name, runtime_type, hostname, platform,
            version, capabilities, public_key, public_key_fingerprint,
            credential_hash, credential_expires_at, status, last_seen_at, kind
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'online', CURRENT_TIMESTAMP, $14)
        "#,
    )
    .bind(&runtime_id)
    .bind(&user_id)
    .bind(pairing.organization_id.as_deref())
    .bind(&pairing.name)
    .bind(&pairing.runtime_type)
    .bind(pairing.hostname.as_deref())
    .bind(pairing.platform.as_deref())
    .bind(pairing.version.as_deref())
    .bind(&pairing.capabilities)
    .bind(&pairing.public_key)
    .bind(&pairing.public_key_fingerprint)
    .bind(credential_hash)
    .bind(credential_expires_at)
    .bind(kind)
    .execute(&mut *transaction)
    .await?;
    let consumed = sqlx::query(
        r#"
        UPDATE runtime_pairings
        SET status = 'consumed', runtime_id = $1, consumed_at = CURRENT_TIMESTAMP
        WHERE id = $2 AND status = 'approved'
        "#,
    )
    .bind(&runtime_id)
    .bind(&pairing.id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if consumed != 1 {
        transaction.rollback().await?;
        return Err(ApiError::BadRequest(
            "Pairing request was already consumed".to_string(),
        ));
    }
    if let Some(hosted_instance_id) = pairing.hosted_instance_id.as_deref() {
        let linked = sqlx::query(
            r#"
            UPDATE hosted_runtime_instances
            SET runtime_device_id = $1, status = 'running',
                bootstrap_token_hash = NULL,
                active_since = COALESCE(active_since, CURRENT_TIMESTAMP),
                last_activity_at = CURRENT_TIMESTAMP,
                last_synced_at = CURRENT_TIMESTAMP,
                error_message = NULL
            WHERE id = $2 AND user_id = $3
              AND bootstrap_token_hash IS NOT NULL
              AND runtime_device_id IS NULL
            "#,
        )
        .bind(&runtime_id)
        .bind(hosted_instance_id)
        .bind(&user_id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        tracing::info!(%hosted_instance_id, %linked, "hosted instance link on pairing exchange");
        if linked != 1 {
            transaction.rollback().await?;
            return Err(ApiError::Unauthorized(
                "Hosted runtime bootstrap was already consumed or its ownership changed"
                    .to_string(),
            ));
        }
    }
    if let Some(provisioned_instance_id) = pairing.provisioned_instance_id.as_deref() {
        // Claims the instance's device slot; a racing second exchange rolls
        // the whole thing back (single-use pairing code semantics).
        crate::services::bind_device_slot(&mut transaction, provisioned_instance_id, &runtime_id)
            .await?;
        tracing::info!(%provisioned_instance_id, %runtime_id, "provisioned instance link on pairing exchange");
    }
    // Server-approved pairings already recorded the daily count at create,
    // when the owning user became known. Browser-approved desktop pairings
    // record it here, after the device row is in the same transaction, so a
    // failed insert or poll retry cannot consume a slot.
    if count_daily_pairing {
        crate::services::QuotaService::record_pairing_created_in_tx(
            &mut transaction,
            &user_id,
            &quota,
        )
        .await?;
    }
    transaction.commit().await?;

    crate::services::audit::write_audit_log(
        &state.db,
        crate::services::audit::AuditEvent {
            action: "device_pairing.token_issued".to_string(),
            resource_type: "runtime_device".to_string(),
            resource_id: Some(runtime_id.clone()),
            user_id: Some(user_id.clone()),
            user_email: None,
            details: Some(serde_json::json!({
                "pairingId": pairing.id,
                "runtimeName": pairing.name,
                "runtimeType": pairing.runtime_type,
                "hostname": pairing.hostname,
            })),
            success: true,
        },
    )
    .await;

    if let Some(hosted_instance_id) = pairing.hosted_instance_id.as_deref() {
        crate::services::record_runtime_started(&state.db, hosted_instance_id).await?;
    }
    if let Some(provisioned_instance_id) = pairing.provisioned_instance_id.as_deref() {
        // Post-commit: consume the one-time code, flip to running, open the
        // metering interval (services::provisioning).
        crate::services::activate_registered_device(&state.db, provisioned_instance_id).await?;
    }

    let user_email = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
        .bind(&user_id)
        .fetch_one(&state.db)
        .await?;
    let capabilities = serde_json::from_str(&pairing.capabilities).unwrap_or_default();
    Ok(Json(RuntimeSessionResponse {
        runtime_id,
        user_id,
        user_email,
        organization_id: pairing.organization_id,
        device_token,
        token_type: "Bearer",
        expires_at: credential_expires_at,
        capabilities,
    })
    .into_response())
}

/// Device id → provisioned instance id for the user's free cloud computers
/// the relay can wake (same states `wake_for_device` wakes from). A sleeping
/// one reads "offline", so clients need this to keep it as a runtime and let
/// the first relayed request wake it. Only each device's newest instance
/// counts; a free computer replaced by a paid one is left out.
async fn wakeable_instances_by_device(
    db: &PgPool,
    user_id: &str,
) -> Result<std::collections::HashMap<String, String>, ApiError> {
    let rows = sqlx::query_as::<_, (String, String, bool)>(
        r#"
        SELECT DISTINCT ON (device_id) device_id, id,
               (tier = 'free' AND replaced_by IS NULL
                AND status IN ('sleeping', 'waking', 'provisioning'))
        FROM provisioned_instances
        WHERE user_id = $1 AND device_id IS NOT NULL AND status <> 'deleted'
        ORDER BY device_id, created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(device_id, instance_id, wakeable)| wakeable.then_some((device_id, instance_id)))
        .collect())
}

async fn list_runtime_devices(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = approver_from_headers(&state, &headers).await?;
    let devices = sqlx::query_as::<_, RuntimeDeviceView>(
        r#"
        SELECT id, name, runtime_type, hostname, platform, version, capabilities,
               public_key_fingerprint, status, last_seen_at, created_at,
               credential_expires_at
        FROM runtime_devices
        WHERE user_id = $1 AND revoked_at IS NULL
        ORDER BY created_at DESC
        "#,
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await?;
    let wakeable = wakeable_instances_by_device(&state.db, &user.id).await?;
    let mut serialized = Vec::with_capacity(devices.len());
    for device in devices {
        let effective_status = if device.status == "online"
            && device.last_seen_at
                .map(|seen| seen < Utc::now() - Duration::minutes(10))
                .unwrap_or(true)
        {
            "offline".to_string()
        } else {
            device.status.clone()
        };
        // Live relay connections (multi-connection mode only; empty in
        // single-connection mode so the flag-off response is unchanged).
        let relay_connections =
            crate::routes::runtime_relay::relay_connection_presence(&device.id).await;
        let mut value = serde_json::json!({
            "id": device.id,
            "name": device.name,
            "runtimeType": device.runtime_type,
            "hostname": device.hostname,
            "platform": device.platform,
            "version": device.version,
            "capabilities": serde_json::from_str::<Vec<String>>(&device.capabilities).unwrap_or_default(),
            "publicKeyFingerprint": device.public_key_fingerprint,
            "status": effective_status,
            "lastSeenAt": device.last_seen_at,
            "createdAt": device.created_at,
            "credentialExpiresAt": device.credential_expires_at,
        });
        if !relay_connections.is_empty() {
            value["relayConnections"] = serde_json::Value::Array(relay_connections);
        }
        if let Some(instance_id) = wakeable.get(&device.id) {
            value["wakeable"] = serde_json::Value::Bool(true);
            value["provisionedInstanceId"] = serde_json::Value::String(instance_id.clone());
        }
        serialized.push(value);
    }
    Ok(Json(serde_json::json!({ "runtimes": serialized })))
}

async fn revoke_runtime_device(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let affected = sqlx::query(
        r#"
        UPDATE runtime_devices
        SET status = 'revoked', revoked_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(&id)
    .bind(&user_id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError::NotFound("Runtime not found".to_string()));
    }
    Ok(Json(serde_json::json!({ "id": id, "status": "revoked" })))
}

async fn runtime_heartbeat(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = authenticate_runtime(&state, &headers, &id).await?;
    sqlx::query(
        "UPDATE runtime_devices SET status = 'online', last_seen_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(&runtime.id)
    .execute(&state.db)
    .await?;
    Ok(Json(serde_json::json!({
        "runtimeId": runtime.id,
        "userId": runtime.user_id,
        "status": "online",
        "serverTime": Utc::now(),
    })))
}

async fn rotate_runtime_credential(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = authenticate_runtime(&state, &headers, &id).await?;
    let (device_token, expires_at) = rotate_credential(&state.db, &runtime.id).await?;
    Ok(Json(serde_json::json!({
        "runtimeId": runtime.id,
        "deviceToken": device_token,
        "tokenType": "Bearer",
        "expiresAt": expires_at,
    })))
}

/// Mints a fresh device credential for `runtime_id`, moving the replaced
/// hash into previous_* with a ROTATION_GRACE_MINUTES expiry. A rotation
/// authenticated via the previous hash takes the same path: the just-
/// replaced current becomes the new previous with a fresh grace, which lets
/// a stranded second component self-heal.
async fn rotate_credential(
    db: &PgPool,
    runtime_id: &str,
) -> Result<(String, DateTime<Utc>), ApiError> {
    let device_token = format!("{DEVICE_TOKEN_PREFIX}{}", random_secret(48));
    let expires_at = Utc::now() + Duration::days(CREDENTIAL_TTL_DAYS);
    sqlx::query(
        r#"
        UPDATE runtime_devices
        SET previous_credential_hash = credential_hash,
            previous_credential_expires_at = $1,
            credential_hash = $2,
            credential_expires_at = $3,
            last_seen_at = CURRENT_TIMESTAMP
        WHERE id = $4
        "#,
    )
    .bind(Utc::now() + Duration::minutes(ROTATION_GRACE_MINUTES))
    .bind(sha256_hex(device_token.as_bytes()))
    .bind(expires_at)
    .bind(runtime_id)
    .execute(db)
    .await?;
    Ok((device_token, expires_at))
}

async fn revoke_self(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let runtime = authenticate_runtime(&state, &headers, &id).await?;
    sqlx::query(
        "UPDATE runtime_devices SET status = 'revoked', revoked_at = CURRENT_TIMESTAMP WHERE id = $1",
    )
    .bind(&runtime.id)
    .execute(&state.db)
    .await?;
    Ok(Json(
        serde_json::json!({ "runtimeId": runtime.id, "status": "revoked" }),
    ))
}

/// Extracts a runtime device token (`Bearer allternit_runtime_…`) from the
/// Authorization header, or `None` when the header carries anything else
/// (e.g. a Clerk session JWT).
pub(crate) fn device_token_from_headers(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| value.starts_with(DEVICE_TOKEN_PREFIX))
}

/// Core credential check shared by the runtime routes and other registries
/// that accept a device token: token hash → device row, with expiry and
/// revocation enforced. `expected_id` scopes the lookup to one device when
/// the route path names it.
pub(crate) async fn runtime_device_for_token(
    db: &PgPool,
    token: &str,
    expected_id: Option<&str>,
) -> Result<RuntimeCredentialRow, ApiError> {
    let credential_hash = sha256_hex(token.as_bytes());
    let runtime = match expected_id {
        Some(expected_id) => sqlx::query_as::<_, RuntimeCredentialRow>(
            r#"
            SELECT id, user_id, name, credential_expires_at, status
            FROM runtime_devices
            WHERE credential_hash = $1 AND id = $2 AND revoked_at IS NULL
            "#,
        )
        .bind(&credential_hash)
        .bind(expected_id)
        .fetch_optional(db)
        .await?,
        None => sqlx::query_as::<_, RuntimeCredentialRow>(
            r#"
            SELECT id, user_id, name, credential_expires_at, status
            FROM runtime_devices
            WHERE credential_hash = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(&credential_hash)
        .fetch_optional(db)
        .await?,
    };
    let runtime = match runtime {
        Some(runtime) => runtime,
        // The just-rotated-out hash stays valid for a short grace window so a
        // second component holding the old token does not die while it
        // catches up. Authenticating this way never extends the grace.
        None => previous_credential_for_token(db, &credential_hash, expected_id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("Invalid runtime credential".to_string()))?,
    };
    if runtime.credential_expires_at <= Utc::now() {
        return Err(ApiError::TokenExpired(
            "Runtime credential expired".to_string(),
        ));
    }
    if runtime.status == "revoked" {
        return Err(ApiError::Unauthorized(
            "Runtime has been revoked".to_string(),
        ));
    }
    Ok(runtime)
}

/// Grace-window fallback for `runtime_device_for_token`: resolves a token
/// against the previous credential hash while its grace is still open. The
/// grace expiry is checked in Rust: sqlx stores DateTime<Utc> as RFC3339
/// text, which does not compare cleanly against CURRENT_TIMESTAMP in SQL
/// (same reason PairingRow.expires_at is checked in Rust). An expired grace
/// is treated as absent — the row is lazily cleared on the next rotation.
async fn previous_credential_for_token(
    db: &PgPool,
    credential_hash: &str,
    expected_id: Option<&str>,
) -> Result<Option<RuntimeCredentialRow>, ApiError> {
    type PreviousRow = (
        String,
        String,
        String,
        DateTime<Utc>,
        String,
        Option<DateTime<Utc>>,
    );
    let row: Option<PreviousRow> = match expected_id {
        Some(expected_id) => sqlx::query_as::<_, PreviousRow>(
            r#"
            SELECT id, user_id, name, credential_expires_at, status,
                   previous_credential_expires_at
            FROM runtime_devices
            WHERE previous_credential_hash = $1 AND id = $2 AND revoked_at IS NULL
            "#,
        )
        .bind(credential_hash)
        .bind(expected_id)
        .fetch_optional(db)
        .await?,
        None => sqlx::query_as::<_, PreviousRow>(
            r#"
            SELECT id, user_id, name, credential_expires_at, status,
                   previous_credential_expires_at
            FROM runtime_devices
            WHERE previous_credential_hash = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(credential_hash)
        .fetch_optional(db)
        .await?,
    };
    let Some((id, user_id, name, credential_expires_at, status, grace_expires_at)) = row else {
        return Ok(None);
    };
    if grace_expires_at.map(|expires| expires <= Utc::now()).unwrap_or(true) {
        return Ok(None);
    }
    Ok(Some(RuntimeCredentialRow {
        id,
        user_id,
        name,
        credential_expires_at,
        status,
    }))
}

async fn authenticate_runtime(
    state: &ApiState,
    headers: &HeaderMap,
    expected_id: &str,
) -> Result<RuntimeCredentialRow, ApiError> {
    let token = device_token_from_headers(headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    runtime_device_for_token(&state.db, token, Some(expected_id)).await
}

/// Token introspection for peer services (e.g. the local allternit-api
/// proxying a headless gizzi MCP client that holds a device token). Public
/// like the pairing exchange endpoint: possession of a valid device token is
/// itself the credential, and this reveals only the identity that token
/// already authenticates as — nothing else. Fails closed on unknown,
/// expired, or revoked tokens via `runtime_device_for_token`.
async fn verify_device_token(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = device_token_from_headers(&headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    let device = runtime_device_for_token(&state.db, token, None).await?;
    let email: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(&device.user_id)
        .fetch_optional(&state.db)
        .await?;
    Ok(Json(serde_json::json!({
        "runtimeId": device.id,
        "userId": device.user_id,
        "name": device.name,
        "status": device.status,
        "email": email,
    })))
}

pub(crate) async fn authenticate_runtime_token(
    state: &ApiState,
    token: &str,
    expected_id: &str,
) -> Result<String, ApiError> {
    let mut headers = HeaderMap::new();
    let value = format!("Bearer {token}")
        .parse()
        .map_err(|_| ApiError::Unauthorized("Invalid runtime credential".to_string()))?;
    headers.insert(axum::http::header::AUTHORIZATION, value);
    Ok(authenticate_runtime(state, &headers, expected_id)
        .await?
        .user_id)
}

/// Human-facing pairing routes are normally authorized by a Clerk session
/// from the browser. Allternit Desktop approves pairings in-app instead and
/// authenticates with its runtime device credential, which resolves to the
/// device's owner — the same trust decision as
/// `gizzi_instances::actor_from_headers`. A device token can only ever act on
/// pairings and devices belonging to its own owner.
async fn approver_from_headers(state: &ApiState, headers: &HeaderMap) -> Result<ClerkUser, ApiError> {
    if let Some(token) = device_token_from_headers(headers) {
        let device = runtime_device_for_token(&state.db, token, None).await?;
        return Ok(ClerkUser {
            id: device.user_id,
            email: None,
            name: None,
            image_url: None,
            organization_id: None,
        });
    }
    match clerk::user_from_headers(headers).await {
        Ok(user) => Ok(user),
        Err(_) => {
            // API-token approvers carry no profile — only the id is used
            // downstream (ownership checks on pairing approval).
            let user_id = crate::auth::resolve_user_id(&state.db, headers).await?;
            Ok(ClerkUser {
                id: user_id,
                email: None,
                name: None,
                image_url: None,
                organization_id: None,
            })
        }
    }
}

async fn pairing_by_code(state: &ApiState, code: &str) -> Result<PairingRow, ApiError> {
    sqlx::query_as::<_, PairingRow>(
        r#"
        SELECT id, user_code, challenge, public_key, public_key_fingerprint,
               name, runtime_type, hostname, platform, version, capabilities,
               status, user_id, organization_id, hosted_instance_id,
               byo_bootstrap_token_id, provisioned_instance_id, expires_at
        FROM runtime_pairings WHERE user_code = $1
        "#,
    )
    .bind(code)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Pairing request not found".to_string()))
}

async fn ensure_pairing_live(state: &ApiState, pairing: &PairingRow) -> Result<(), ApiError> {
    if pairing.expires_at > Utc::now() {
        return Ok(());
    }
    sqlx::query("UPDATE runtime_pairings SET status = 'expired' WHERE id = $1")
        .bind(&pairing.id)
        .execute(&state.db)
        .await?;
    Err(ApiError::TokenExpired(
        "Pairing request expired".to_string(),
    ))
}

fn pairing_info_response(pairing: PairingRow) -> PairingInfoResponse {
    PairingInfoResponse {
        pairing_id: pairing.id,
        user_code: pairing.user_code,
        name: pairing.name,
        runtime_type: pairing.runtime_type,
        hostname: pairing.hostname,
        platform: pairing.platform,
        public_key_fingerprint: pairing.public_key_fingerprint,
        capabilities: serde_json::from_str(&pairing.capabilities).unwrap_or_default(),
        status: pairing.status,
        expires_at: pairing.expires_at,
    }
}

fn verify_pairing_signature(pairing: &PairingRow, signature: &str) -> Result<(), ApiError> {
    let key_bytes = decode_public_key(&pairing.public_key)?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| ApiError::BadRequest("Runtime public key must be 32 bytes".to_string()))?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|_| ApiError::BadRequest("Invalid runtime public key".to_string()))?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| ApiError::Unauthorized("Invalid pairing signature".to_string()))?;
    let signature_array: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| ApiError::Unauthorized("Invalid pairing signature".to_string()))?;
    let signature = Signature::from_bytes(&signature_array);
    let message = pairing_signature_message(&pairing.id, &pairing.challenge);
    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|_| ApiError::Unauthorized("Runtime proof of possession failed".to_string()))
}

fn validate_pairing_request(request: &CreatePairingRequest) -> Result<(), ApiError> {
    if request.name.trim().is_empty() || request.name.len() > 100 {
        return Err(ApiError::BadRequest(
            "Runtime name must be 1-100 characters".to_string(),
        ));
    }
    if !matches!(
        request.runtime_type.as_str(),
        "desktop" | "vps" | "hosted" | "provisioned" | "ios" | "ao"
    ) {
        return Err(ApiError::BadRequest(
            "runtimeType must be desktop, vps, hosted, provisioned, ios, or ao".to_string(),
        ));
    }
    decode_public_key(&request.public_key)?;
    Ok(())
}

fn normalized_capabilities(requested: Vec<String>) -> Result<Vec<String>, ApiError> {
    if requested.is_empty() {
        return Ok(DEFAULT_CAPABILITIES
            .iter()
            .map(|value| value.to_string())
            .collect());
    }
    if requested
        .iter()
        .any(|capability| !DEFAULT_CAPABILITIES.contains(&capability.as_str()))
    {
        return Err(ApiError::BadRequest(
            "Unknown runtime capability requested".to_string(),
        ));
    }
    if !requested
        .iter()
        .any(|capability| capability == "runtime:connect")
    {
        return Err(ApiError::BadRequest(
            "runtime:connect is required for every paired runtime".to_string(),
        ));
    }
    let mut capabilities = requested;
    capabilities.sort();
    capabilities.dedup();
    Ok(capabilities)
}

fn decode_public_key(value: &str) -> Result<Vec<u8>, ApiError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ApiError::BadRequest("publicKey must use base64url encoding".to_string()))?;
    if bytes.len() != 32 {
        return Err(ApiError::BadRequest(
            "Ed25519 publicKey must be 32 bytes".to_string(),
        ));
    }
    Ok(bytes)
}

fn pairing_signature_message(pairing_id: &str, challenge: &str) -> String {
    format!("allternit-runtime-pairing:{pairing_id}:{challenge}")
}

/// A server-side pre-approval for a pairing, established by a one-time
/// bootstrap token: hosted runtimes carry `hosted_instance_id`, BYO-VPS boxes
/// carry `byo_bootstrap_token_id`; exactly one is set.
#[derive(Debug)]
struct BootstrapApproval {
    hosted_instance_id: Option<String>,
    byo_bootstrap_token_id: Option<String>,
    provisioned_instance_id: Option<String>,
    user_id: String,
    organization_id: Option<String>,
}

async fn validate_hosted_bootstrap(
    state: &ApiState,
    instance_id: Option<&str>,
    token: Option<&str>,
) -> Result<BootstrapApproval, ApiError> {
    let instance_id = instance_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("hosted_instance_id is required".to_string()))?;
    let token = token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("hosted_bootstrap_token is required".to_string()))?;

    let row: (String, Option<String>, Option<String>) = sqlx::query_as(
        r#"
        SELECT user_id, organization_id, bootstrap_token_hash
        FROM hosted_runtime_instances
        WHERE id = $1 AND status IN ('creating', 'starting', 'running', 'stopped')
        "#,
    )
    .bind(instance_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| {
        tracing::warn!(%instance_id, "hosted bootstrap rejected: no matching instance row");
        ApiError::Unauthorized("Invalid hosted instance".to_string())
    })?;

    let expected_hash = row.2.ok_or_else(|| {
        tracing::warn!(%instance_id, "hosted bootstrap rejected: instance row has no bootstrap token hash");
        ApiError::Unauthorized("Hosted instance has no bootstrap token".to_string())
    })?;
    if expected_hash != sha256_hex(token.as_bytes()) {
        tracing::warn!(%instance_id, "hosted bootstrap rejected: token hash mismatch");
        return Err(ApiError::Unauthorized(
            "Invalid hosted bootstrap token".to_string(),
        ));
    }

    Ok(BootstrapApproval {
        hosted_instance_id: Some(instance_id.to_string()),
        byo_bootstrap_token_id: None,
        provisioned_instance_id: None,
        user_id: row.0,
        organization_id: row.1,
    })
}

/// Validates the P2 provisioned lane's one-time bootstrap code against the
/// instance row (hash, expiry, unbound) and returns the pre-approval for the
/// owning user. The code itself is consumed later, at pairing exchange, when
/// `bind_device_slot` claims the instance's device slot — same single-use
/// shape as the hosted bootstrap token.
async fn validate_provisioned_bootstrap(
    db: &PgPool,
    instance_id: Option<&str>,
    token: Option<&str>,
) -> Result<BootstrapApproval, ApiError> {
    let user_id =
        crate::services::validate_provisioned_bootstrap(db, instance_id, token).await?;
    Ok(BootstrapApproval {
        hosted_instance_id: None,
        byo_bootstrap_token_id: None,
        provisioned_instance_id: instance_id.map(str::to_string),
        user_id,
        organization_id: None,
    })
}

/// Validates a wizard-minted BYO bootstrap token and consumes it. Consumption
/// happens here (at pairing-create), not at exchange: a valid-but-unconsumed
/// token could otherwise mint unlimited approved pairings. A bootstrap that
/// dies before the exchange simply re-runs — the wizard mints a fresh token
/// per attempt.
async fn validate_byo_bootstrap(
    db: &PgPool,
    token: Option<&str>,
) -> Result<BootstrapApproval, ApiError> {
    let token = token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::Unauthorized("byo_bootstrap_token is required".to_string()))?;

    let row: Option<(String, String, Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, user_id, consumed_at, expires_at
        FROM byo_bootstrap_tokens
        WHERE token_hash = $1
        "#,
    )
    .bind(sha256_hex(token.as_bytes()))
    .fetch_optional(db)
    .await?;

    // Expiry/consumption are checked in Rust: sqlx stores DateTime<Utc> as
    // RFC3339 text, which does not compare cleanly against CURRENT_TIMESTAMP
    // in SQL (same reason PairingRow.expires_at is checked in Rust).
    let (token_id, user_id, consumed_at, expires_at) = row.ok_or_else(|| {
        ApiError::Unauthorized("Invalid or expired BYO bootstrap token".to_string())
    })?;
    if consumed_at.is_some() || expires_at <= Utc::now() {
        return Err(ApiError::Unauthorized(
            "Invalid or expired BYO bootstrap token".to_string(),
        ));
    }

    let consumed = sqlx::query(
        "UPDATE byo_bootstrap_tokens SET consumed_at = CURRENT_TIMESTAMP WHERE id = $1 AND consumed_at IS NULL",
    )
    .bind(&token_id)
    .execute(db)
    .await?
    .rows_affected();
    if consumed != 1 {
        return Err(ApiError::Unauthorized(
            "Invalid or expired BYO bootstrap token".to_string(),
        ));
    }

    Ok(BootstrapApproval {
        hosted_instance_id: None,
        byo_bootstrap_token_id: Some(token_id),
        provisioned_instance_id: None,
        user_id,
        organization_id: None,
    })
}

fn normalize_user_code(value: &str) -> String {
    let compact = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_uppercase())
        .take(8)
        .collect::<String>();
    if compact.len() == 8 {
        format!("{}-{}", &compact[..4], &compact[4..])
    } else {
        compact
    }
}

fn generate_user_code() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let mut group = || {
        (0..4)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect::<String>()
    };
    format!("{}-{}", group(), group())
}

pub(crate) fn random_secret(bytes: usize) -> String {
    let mut data = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut data);
    URL_SAFE_NO_PAD.encode(data)
}

pub(crate) fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[cfg(test)]
mod tests {

    #[test]
    fn bootstrap_token_comes_from_the_field_or_the_header() {
        let mut headers = HeaderMap::new();
        assert_eq!(bootstrap_token_from(None, &headers), None);
        assert_eq!(bootstrap_token_from(Some("  ".to_string()), &headers), None);
        assert_eq!(bootstrap_token_from(Some(" tok ".to_string()), &headers).as_deref(), Some("tok"));
        headers.insert(BOOTSTRAP_TOKEN_HEADER, "hdr".parse().unwrap());
        assert_eq!(bootstrap_token_from(None, &headers).as_deref(), Some("hdr"));
        assert_eq!(bootstrap_token_from(Some("body".to_string()), &headers).as_deref(), Some("body"));
        // The camelCase body field deserializes.
        let request: CreatePairingRequest = serde_json::from_value(serde_json::json!({
            "name": "x", "runtimeType": "desktop", "publicKey": "k", "bootstrapToken": "t1"
        }))
        .unwrap();
        assert_eq!(request.bootstrap_token.as_deref(), Some("t1"));
    }

    use super::*;

    #[test]
    fn desktop_handshake_gets_local_kind_never_null() {
        assert_eq!(runtime_device_kind("desktop", None), crate::services::NodeKind::LOCAL);
        assert_eq!(runtime_device_kind("vps", None), crate::services::NodeKind::PAIRED);
        assert_eq!(
            runtime_device_kind("desktop", Some("pi_1")),
            crate::services::NodeKind::PROVISIONED
        );
    }

    /// Minimal schema for the BYO bootstrap token path: the token table plus
    /// the users table its user_id FK references.
    async fn test_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS users CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                status TEXT NOT NULL DEFAULT 'active'
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS byo_bootstrap_tokens CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE byo_bootstrap_tokens (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                instance_name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                expires_at TIMESTAMPTZ NOT NULL,
                consumed_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (id, email) VALUES ('user_1', 'user_1@users.allternit.local')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn insert_token(pool: &PgPool, id: &str, token: &str, expires_in: Duration) {
        sqlx::query(
            r#"
            INSERT INTO byo_bootstrap_tokens (id, user_id, instance_name, token_hash, expires_at)
            VALUES ($1, 'user_1', 'byo-vps-1', $2, $3)
            "#,
        )
        .bind(id)
        .bind(sha256_hex(token.as_bytes()))
        .bind(Utc::now() + expires_in)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wakeable_lists_only_live_unreplaced_free_computers() {
        let pool = test_pool().await;
        sqlx::query("DROP TABLE IF EXISTS provisioned_instances CASCADE").execute(&pool).await.unwrap();
        sqlx::query(
            r#"
            CREATE TABLE provisioned_instances (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                device_id TEXT,
                tier TEXT NOT NULL DEFAULT 'paid',
                status TEXT NOT NULL,
                replaced_by TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let rows = [
            // (id, user, device, tier, status, replaced_by, age_minutes)
            ("pi_sleep", "user_1", Some("dev_sleep"), "free", "sleeping", None, 0),
            ("pi_waking", "user_1", Some("dev_waking"), "free", "waking", None, 0),
            ("pi_running", "user_1", Some("dev_running"), "free", "running", None, 0),
            ("pi_paid", "user_1", Some("dev_paid"), "paid", "stopped", None, 0),
            ("pi_replaced", "user_1", Some("dev_replaced"), "free", "sleeping", Some("pi_paid"), 0),
            ("pi_nodev", "user_1", None, "free", "sleeping", None, 0),
            ("pi_other", "user_2", Some("dev_other"), "free", "sleeping", None, 0),
            // An older deleted-then-recreated device: only the newest row counts.
            ("pi_old", "user_1", Some("dev_reused"), "free", "sleeping", None, 60),
            ("pi_new", "user_1", Some("dev_reused"), "paid", "running", None, 0),
        ];
        for (id, user, device, tier, status, replaced_by, age) in rows {
            sqlx::query(
                "INSERT INTO provisioned_instances (id, user_id, device_id, tier, status, replaced_by, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, NOW() - make_interval(mins => $7))",
            )
            .bind(id)
            .bind(user)
            .bind(device)
            .bind(tier)
            .bind(status)
            .bind(replaced_by)
            .bind(age)
            .execute(&pool)
            .await
            .unwrap();
        }

        let wakeable = wakeable_instances_by_device(&pool, "user_1").await.unwrap();
        let mut got: Vec<(&str, &str)> =
            wakeable.iter().map(|(d, i)| (d.as_str(), i.as_str())).collect();
        got.sort();
        assert_eq!(got, vec![("dev_sleep", "pi_sleep"), ("dev_waking", "pi_waking")]);
    }

    #[tokio::test]
    async fn byo_bootstrap_token_validates_once_then_is_consumed() {
        let pool = test_pool().await;
        insert_token(&pool, "bt_1", "byo-secret", Duration::hours(1)).await;

        let approval = validate_byo_bootstrap(&pool, Some("byo-secret"))
            .await
            .unwrap();
        assert_eq!(approval.user_id, "user_1");
        assert_eq!(approval.byo_bootstrap_token_id.as_deref(), Some("bt_1"));
        assert!(approval.hosted_instance_id.is_none());

        // Single-use: the same token can never approve a second pairing.
        let error = validate_byo_bootstrap(&pool, Some("byo-secret"))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn byo_bootstrap_token_rejects_expired_unknown_and_missing() {
        let pool = test_pool().await;
        insert_token(&pool, "bt_expired", "old-secret", Duration::minutes(-5)).await;

        for token in [Some("old-secret"), Some("never-issued"), Some("")] {
            let error = validate_byo_bootstrap(&pool, token).await.unwrap_err();
            assert!(
                matches!(error, ApiError::Unauthorized(_)),
                "token {token:?} must be rejected, got {error:?}"
            );
        }
        let error = validate_byo_bootstrap(&pool, None).await.unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)));

        // The expired token must not be consumed by the failed attempt...
        let consumed: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT consumed_at FROM byo_bootstrap_tokens WHERE id = 'bt_expired'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(consumed.is_none());
    }

    /// Minimal runtime_devices shape for the rotation-grace tests (mirrors
    /// the gizzi_instances/mesh device-token tests, plus the grace columns
    /// from migration 022).
    async fn device_pool() -> PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
        let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url)
            .await
            .unwrap();
        sqlx::query("DROP TABLE IF EXISTS runtime_devices CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                credential_hash TEXT NOT NULL UNIQUE,
                credential_expires_at TIMESTAMPTZ NOT NULL,
                previous_credential_hash TEXT,
                previous_credential_expires_at TIMESTAMPTZ,
                status TEXT NOT NULL DEFAULT 'offline',
                last_seen_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_device(pool: &PgPool, token: &str) {
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (
                id, user_id, name, credential_hash, credential_expires_at, status
            )
            VALUES ('rd_1', 'user_9', 'byo-vps-1', $1, $2, 'offline')
            "#,
        )
        .bind(sha256_hex(token.as_bytes()))
        .bind(Utc::now() + Duration::days(1))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rotated_out_token_works_within_grace_alongside_new_token() {
        let pool = device_pool().await;
        let old_token = format!("{DEVICE_TOKEN_PREFIX}oldsecret");
        insert_device(&pool, &old_token).await;

        let (new_token, _) = rotate_credential(&pool, "rd_1").await.unwrap();

        // Both the fresh token and the just-replaced one authenticate.
        let via_new = runtime_device_for_token(&pool, &new_token, Some("rd_1"))
            .await
            .unwrap();
        assert_eq!(via_new.id, "rd_1");
        let via_old = runtime_device_for_token(&pool, &old_token, Some("rd_1"))
            .await
            .unwrap();
        assert_eq!(via_old.id, "rd_1");
        let via_old = runtime_device_for_token(&pool, &old_token, None)
            .await
            .unwrap();
        assert_eq!(via_old.user_id, "user_9");
    }

    #[tokio::test]
    async fn authenticating_via_previous_token_does_not_extend_grace() {
        let pool = device_pool().await;
        let old_token = format!("{DEVICE_TOKEN_PREFIX}oldsecret");
        insert_device(&pool, &old_token).await;
        rotate_credential(&pool, "rd_1").await.unwrap();

        let grace_before: DateTime<Utc> =
            sqlx::query_scalar("SELECT previous_credential_expires_at FROM runtime_devices WHERE id = 'rd_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        runtime_device_for_token(&pool, &old_token, Some("rd_1"))
            .await
            .unwrap();
        let grace_after: DateTime<Utc> =
            sqlx::query_scalar("SELECT previous_credential_expires_at FROM runtime_devices WHERE id = 'rd_1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(grace_before, grace_after);
    }

    #[tokio::test]
    async fn rotation_with_previous_token_succeeds_and_mints_fresh_current() {
        let pool = device_pool().await;
        let token_a = format!("{DEVICE_TOKEN_PREFIX}tokena");
        insert_device(&pool, &token_a).await;
        let (token_b, _) = rotate_credential(&pool, "rd_1").await.unwrap();

        // The stranded component still holds token_a: it authenticates via
        // the grace window and rotates. token_b moves to previous with a
        // fresh grace; token_a is gone for good.
        let runtime = runtime_device_for_token(&pool, &token_a, Some("rd_1"))
            .await
            .unwrap();
        let (token_c, _) = rotate_credential(&pool, &runtime.id).await.unwrap();

        runtime_device_for_token(&pool, &token_c, Some("rd_1"))
            .await
            .unwrap();
        runtime_device_for_token(&pool, &token_b, Some("rd_1"))
            .await
            .unwrap();
        let error = runtime_device_for_token(&pool, &token_a, Some("rd_1"))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)));
    }

    #[tokio::test]
    async fn previous_token_is_rejected_after_grace_expires() {
        let pool = device_pool().await;
        let old_token = format!("{DEVICE_TOKEN_PREFIX}oldsecret");
        insert_device(&pool, &old_token).await;
        let (new_token, _) = rotate_credential(&pool, "rd_1").await.unwrap();

        sqlx::query(
            "UPDATE runtime_devices SET previous_credential_expires_at = $1 WHERE id = 'rd_1'",
        )
        .bind(Utc::now() - Duration::minutes(1))
        .execute(&pool)
        .await
        .unwrap();

        let error = runtime_device_for_token(&pool, &old_token, Some("rd_1"))
            .await
            .unwrap_err();
        assert!(matches!(error, ApiError::Unauthorized(_)));
        // The current token is unaffected by the lapsed grace.
        runtime_device_for_token(&pool, &new_token, Some("rd_1"))
            .await
            .unwrap();
    }
}
