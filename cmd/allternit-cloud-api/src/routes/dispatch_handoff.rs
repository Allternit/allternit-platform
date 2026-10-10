//! Dispatch handoff — the production implementation of the dev-server-only
//! `/dispatch/handoff/*` endpoints (surfaces/ai.allternit.com/vite.config.ts).
//!
//! Flow (DispatchView.tsx ↔ iOS RuntimePairingView):
//! 1. The desktop/web mints a short-lived token bound to one of the user's
//!    paired runtimes (`POST /dispatch/handoff/mint`) and renders it as a QR.
//! 2. The phone claims the token (`POST /dispatch/handoff/claim`) and gets
//!    the runtime id back — iOS then pairs (`EnvironmentStore.pair`) and its
//!    cloud mode relays through that runtime, so local and cloud code
//!    sessions resolve to the same gizzi-code store on the same machine.
//! 3. The desktop polls `GET /dispatch/handoff/status` to detect the claim.
//!
//! All three endpoints verify the Clerk session themselves (like
//! runtime_pairing); they are mounted without the legacy API-token
//! middleware. The token carries no session state — it only ever means
//! "this phone, this user, that runtime".

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::sync::Arc;

use crate::{auth::clerk, auth::clerk::ClerkUser, ApiError, ApiState};

const HANDOFF_TTL_MINUTES: i64 = 10;

#[derive(Debug, FromRow)]
struct HandoffRow {
    token: String,
    user_id: String,
    runtime_id: String,
    expires_at: DateTime<Utc>,
    claimed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintRequest {
    /// Which of the user's runtimes the phone should pair with. Optional
    /// when the user has exactly one unrevoked runtime.
    runtime_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MintResponse {
    token: String,
    runtime_id: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct ClaimRequest {
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimResponse {
    runtime_id: String,
    claimed: bool,
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    claimed: bool,
    runtime_id: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/dispatch/handoff/mint", post(mint_handoff))
        .route("/dispatch/handoff/claim", post(claim_handoff))
        .route("/dispatch/handoff/status", get(handoff_status))
}

/// POST /dispatch/handoff/mint — create a claim token bound to one of the
/// caller's runtimes. With multiple runtimes, `runtimeId` selects one.
async fn mint_handoff(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<MintRequest>,
) -> Result<Json<MintResponse>, ApiError> {
    let user = handoff_user(&state, &headers).await?;

    let runtime_id = match body.runtime_id {
        Some(id) => {
            let owned: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM runtime_devices WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
            )
            .bind(&id)
            .bind(&user.id)
            .fetch_optional(&state.db)
            .await?;
            if owned.is_none() {
                return Err(ApiError::NotFound("Runtime not found".to_string()));
            }
            id
        }
        None => {
            let runtimes: Vec<(String,)> = sqlx::query_as(
                "SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at DESC",
            )
            .bind(&user.id)
            .fetch_all(&state.db)
            .await?;
            match runtimes.len() {
                0 => {
                    return Err(ApiError::BadRequest(
                        "No paired runtime — pair a runtime device first.".to_string(),
                    ))
                }
                1 => runtimes.into_iter().next().unwrap().0,
                _ => {
                    return Err(ApiError::BadRequest(
                        "Multiple runtimes — pass runtimeId.".to_string(),
                    ))
                }
            }
        }
    };

    // The FK targets users(id); a Clerk user may not have a row yet.
    sqlx::query("INSERT INTO users (id, email, name, avatar_url) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.image_url)
        .execute(&state.db)
        .await?;

    let token = random_token(24);
    let expires_at = Utc::now() + Duration::minutes(HANDOFF_TTL_MINUTES);
    sqlx::query(
        "INSERT INTO dispatch_handoff_tokens (token, user_id, runtime_id, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(&token)
    .bind(&user.id)
    .bind(&runtime_id)
    .bind(expires_at)
    .execute(&state.db)
    .await?;

    Ok(Json(MintResponse {
        token,
        runtime_id,
        expires_at,
    }))
}

/// POST /dispatch/handoff/claim — the phone redeems the token and learns
/// which runtime to pair with. Claiming is idempotent for the same user.
async fn claim_handoff(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<ClaimRequest>,
) -> Result<Json<ClaimResponse>, ApiError> {
    let user = handoff_user(&state, &headers).await?;
    let row = fetch_token(&state, &body.token).await?;

    if row.user_id != user.id {
        return Err(ApiError::Forbidden(
            "This handoff belongs to a different account.".to_string(),
        ));
    }
    if row.expires_at < Utc::now() {
        return Err(ApiError::TokenExpired(
            "This handoff has expired — mint a new one.".to_string(),
        ));
    }
    if row.claimed_at.is_none() {
        sqlx::query("UPDATE dispatch_handoff_tokens SET claimed_at = CURRENT_TIMESTAMP WHERE token = $1")
            .bind(&row.token)
            .execute(&state.db)
            .await?;
    }

    Ok(Json(ClaimResponse {
        runtime_id: row.runtime_id,
        claimed: true,
    }))
}

/// GET /dispatch/handoff/status?token=… — the desktop polls to detect the
/// phone's claim (mirrors the dev plugin's `/status` shape).
async fn handoff_status(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<StatusQuery>,
) -> Result<Json<StatusResponse>, ApiError> {
    let user = handoff_user(&state, &headers).await?;
    let row = fetch_token(&state, &query.token).await?;

    if row.user_id != user.id {
        return Err(ApiError::Forbidden(
            "This handoff belongs to a different account.".to_string(),
        ));
    }

    Ok(Json(StatusResponse {
        claimed: row.claimed_at.is_some(),
        runtime_id: row.claimed_at.map(|_| row.runtime_id.clone()),
        expires_at: Some(row.expires_at),
    }))
}

async fn fetch_token(state: &Arc<ApiState>, token: &str) -> Result<HandoffRow, ApiError> {
    sqlx::query_as::<_, HandoffRow>(
        "SELECT token, user_id, runtime_id, expires_at, claimed_at FROM dispatch_handoff_tokens WHERE token = $1",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Unknown handoff token".to_string()))
}

/// Clerk verification with the same development shortcut the legacy auth
/// middleware honors (auth/middleware.rs) so the handoff loop can be
/// exercised against a local stack without a real Clerk session. The
/// shortcut authenticates only explicitly-enabled development overrides —
/// the hardcoded `dev-api-token` gated by `ALLTERNIT_ALLOW_DEV_TOKEN`
/// (default OFF, audit finding B1; see `auth::dev_token`), the
/// operator-configured `ALLTERNIT_DEV_BEARER` (with
/// `ALLTERNIT_DEV_MODE=true`), and the legacy literal (with
/// `ALLTERNIT_ALLOW_DEV_API_TOKEN=true`) — the latter two never in
/// production. No override is active by default.
async fn handoff_user(state: &ApiState, headers: &HeaderMap) -> Result<ClerkUser, ApiError> {
    let development_mode = std::env::var("Allternit_API_DEVELOPMENT_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if development_mode {
        let is_dev_token = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|t| {
                crate::auth::dev_token::is_allowed_dev_token(
                    t,
                    crate::auth::dev_token::dev_token_allowed(),
                ) || crate::auth::middleware::is_dev_api_token(t)
                    || crate::auth::middleware::is_legacy_dev_api_token(t)
            })
            .unwrap_or(false);
        if is_dev_token {
            return Ok(ClerkUser {
                id: "dev-user".to_string(),
                email: None,
                name: None,
                image_url: None,
                organization_id: None,
                org_role: None,
            });
        }
    }
    match clerk::user_from_headers(headers).await {
        Ok(user) => Ok(user),
        Err(_) => {
            // API-token callers carry no profile; the handoff insert binds
            // the optional fields as NULL (same as the dev-user path).
            let user_id = crate::auth::resolve_user_id(&state.db, headers).await?;
            Ok(ClerkUser {
                id: user_id,
                email: None,
                name: None,
                image_url: None,
                organization_id: None,
                org_role: None,
            })
        }
    }
}

fn random_token(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}
