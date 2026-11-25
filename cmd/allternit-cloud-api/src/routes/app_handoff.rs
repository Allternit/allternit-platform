//! App-origin sessions (Apps + MCP 2026, plan v3 lane 32b, decision D15).
//! Child module of `artifacts_v2`.
//!
//! An app runs on its own origin, `https://<slug>.<APP_DOMAIN>`
//! (`allternitusercontent.com`), and never gets the Clerk session: no Clerk
//! cookie or token is ever set on, or accepted from, the app domain.
//!
//! * `POST /api/v1/apps/:slug/handoff` — signed in on ai.allternit.com (Clerk
//!   session, API token or device token; refused from an app origin). Needs
//!   view access to the app's artifact. Returns
//!   `{ token, expires_in, url, open_url }`: `token` (`aph_…`) is single use,
//!   valid [`HANDOFF_TTL_SECS`] seconds, bound to the user + artifact + slug;
//!   `url` is the app's hosted address and `open_url` is `url?handoff=<token>`.
//! * `POST /api/v1/apps/handoff/exchange` `{ token }` — called from the app
//!   origin, no other auth. Returns
//!   `{ session, expires_in, user: { id, name }, artifact_id, slug }`;
//!   `session` (`aps_…`) is a bearer for [`SESSION_TTL_SECS`] seconds. To
//!   refresh, the workspace mints a new handoff. A used, expired or unknown
//!   token -> 401 `handoff_invalid`.
//!
//! The page runtime (`/api/v2/artifact-runtime/:id/*`) takes
//! `Authorization: Bearer aps_…` through [`app_session_caller`] for that one
//! artifact only: another artifact -> 403 `app_session_wrong_app`; expired
//! or unknown -> 401 `app_session_invalid`. A request from an app origin
//! without an app session -> 401 `app_session_required`.
//!
//! `GET /api/v2/public/apps/:slug/content` (artifacts_v2) serves the app's
//! page: public for a `browser` app shared by link, else it needs this app's
//! `aps_` session (401 `app_session_required`, 403 `app_session_wrong_app`).
//! It returns `artifact_id` only to an authorized caller.
//!
//! CORS: [`is_app_origin`] + [`is_app_cors_path`] let app origins reach the
//! exchange, runtime and public app routes only (see `cors_origin_allowed`).

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use super::{caller, load_with_access, parse_json, Result};
use crate::artifacts::access::Caller;
use crate::artifacts::error::ArtifactError;
use crate::ApiState;

/// A handoff lives at most a minute (D15: "≤60 s").
pub const HANDOFF_TTL_SECS: i64 = 60;
/// An app session lives at most an hour; a new handoff refreshes it.
pub const SESSION_TTL_SECS: i64 = 3600;
/// The app domain bought 2026-11-24; `APP_DOMAIN` may name another one too.
pub const DEFAULT_APP_DOMAIN: &str = "allternitusercontent.com";
pub const EXCHANGE_PATH: &str = "/api/v1/apps/handoff/exchange";
const RUNTIME_PREFIX: &str = "/api/v2/artifact-runtime/";
/// The public app record and its content (`/api/v2/public/apps/:slug[/content]`).
const PUBLIC_APPS_PREFIX: &str = "/api/v2/public/apps/";
const HANDOFF_PREFIX: &str = "aph_";
const SESSION_PREFIX: &str = "aps_";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        // The path parameter is named `kind` (it holds the app slug) because
        // lane 9's `/api/v1/apps/:kind/:app_ref/install` shares this segment,
        // and the router refuses two parameter names in one position.
        .route("/api/v1/apps/:kind/handoff", post(mint_handoff))
        .route(EXCHANGE_PATH, post(exchange_handoff))
}

// --------------------------------------------------------------- origins ----

/// `https://<one label>.<app domain>`: the hosted app origins. Live links
/// (`<name>.live.<domain>`) run on the owner's machine, not this runtime.
pub fn is_app_origin(origin: &str) -> bool {
    let Some(host) = origin.strip_prefix("https://") else { return false };
    let host = host.to_ascii_lowercase();
    let mut domains = vec![DEFAULT_APP_DOMAIN.to_string()];
    if let Some(d) = crate::routes::share_links::app_domain() {
        domains.push(d);
    }
    domains.iter().any(|domain| {
        host.strip_suffix(domain.as_str())
            .and_then(|rest| rest.strip_suffix('.'))
            .is_some_and(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    && !label.starts_with('-')
                    && !label.ends_with('-')
            })
    })
}

/// The only routes an app origin may call cross-origin.
pub fn is_app_cors_path(path: &str) -> bool {
    path == EXCHANGE_PATH || path.starts_with(RUNTIME_PREFIX) || path.starts_with(PUBLIC_APPS_PREFIX)
}

pub(crate) fn from_app_origin(headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(is_app_origin)
}

/// Every Clerk-backed caller lookup refuses an app origin, so an app page can
/// never ride a workspace session (cookie or token) into the API.
pub(crate) fn refuse_app_origin(headers: &HeaderMap) -> Result<()> {
    if from_app_origin(headers) {
        return Err(app_session_required());
    }
    Ok(())
}

fn app_session_required() -> ArtifactError {
    ArtifactError::coded(
        StatusCode::UNAUTHORIZED,
        "app_session_required",
        "An app signs in with an app session, not a workspace session.",
    )
}

// ---------------------------------------------------------------- tokens ----

fn new_token(prefix: &str) -> String {
    let bytes: [u8; 32] = rand::random();
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

#[derive(Debug, FromRow)]
struct Grant {
    user_id: String,
    email: Option<String>,
    name: Option<String>,
    org_id: Option<String>,
    org_role: Option<String>,
    artifact_id: String,
    app_slug: String,
}

impl Grant {
    fn caller(self) -> Caller {
        Caller {
            id: self.user_id,
            email: self.email,
            name: self.name,
            image_url: None,
            org_id: self.org_id,
            org_role: self.org_role,
        }
    }
}

// ----------------------------------------------------------------- mint ----

async fn mint_handoff(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Result<(StatusCode, Json<Value>)> {
    // `caller` already refuses app origins (refuse_app_origin).
    let who = caller(&state, &headers).await?;
    let slug = slug.to_ascii_lowercase();
    let artifact_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM artifacts WHERE app_slug = $1 AND app_permission_tier IS NOT NULL",
    )
    .bind(&slug)
    .fetch_optional(&state.db)
    .await?;
    let Some(artifact_id) = artifact_id else { return Err(ArtifactError::not_found()) };
    // View access or 404 (existence isn't revealed).
    let (row, _access) = load_with_access(&state.db, &artifact_id, &who).await?;

    // Tidy this user's dead rows on the way (no scheduled job).
    sqlx::query("DELETE FROM app_handoffs WHERE user_id = $1 AND expires_at < now() - interval '1 hour'")
        .bind(&who.id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM app_sessions WHERE user_id = $1 AND expires_at < now()")
        .bind(&who.id)
        .execute(&state.db)
        .await?;

    let token = new_token(HANDOFF_PREFIX);
    sqlx::query(
        "INSERT INTO app_handoffs (token_hash, user_id, email, name, org_id, org_role, artifact_id, app_slug, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(secs => $9))",
    )
    .bind(token_hash(&token))
    .bind(&who.id)
    .bind(who.email.as_deref())
    .bind(who.name.as_deref())
    .bind(who.org_id.as_deref())
    .bind(who.org_role.as_deref())
    .bind(&row.id)
    .bind(&slug)
    .bind(HANDOFF_TTL_SECS as f64)
    .execute(&state.db)
    .await?;

    let url = crate::routes::share_links::link_url("hosted", &slug);
    let open_url = url.as_ref().map(|u| format!("{u}?handoff={token}"));
    Ok((
        StatusCode::CREATED,
        Json(json!({ "token": token, "expires_in": HANDOFF_TTL_SECS, "url": url, "open_url": open_url })),
    ))
}

// ------------------------------------------------------------- exchange ----

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeRequest {
    token: String,
}

async fn exchange_handoff(State(state): State<Arc<ApiState>>, body: Bytes) -> Result<Json<Value>> {
    let req: ExchangeRequest = parse_json(&body)?;
    let invalid = || {
        ArtifactError::coded(
            StatusCode::UNAUTHORIZED,
            "handoff_invalid",
            "This sign-in link was used or has expired. Open the app again.",
        )
    };
    if !req.token.starts_with(HANDOFF_PREFIX) {
        return Err(invalid());
    }
    // Single use: the first exchange marks it used, atomically.
    let grant = sqlx::query_as::<_, Grant>(
        "UPDATE app_handoffs SET used_at = now() \
         WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING user_id, email, name, org_id, org_role, artifact_id, app_slug",
    )
    .bind(token_hash(&req.token))
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(invalid)?;

    let session = new_token(SESSION_PREFIX);
    sqlx::query(
        "INSERT INTO app_sessions (token_hash, user_id, email, name, org_id, org_role, artifact_id, app_slug, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(secs => $9))",
    )
    .bind(token_hash(&session))
    .bind(&grant.user_id)
    .bind(grant.email.as_deref())
    .bind(grant.name.as_deref())
    .bind(grant.org_id.as_deref())
    .bind(grant.org_role.as_deref())
    .bind(&grant.artifact_id)
    .bind(&grant.app_slug)
    .bind(SESSION_TTL_SECS as f64)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({
        "session": session,
        "expires_in": SESSION_TTL_SECS,
        "user": { "id": grant.user_id, "name": grant.name },
        "artifact_id": grant.artifact_id,
        "slug": grant.app_slug,
    })))
}

// --------------------------------------------------------- runtime auth ----

/// The page runtime's caller for an app session, bound to `artifact_id`.
/// `Ok(None)` = no app session on this request (the normal workspace caller
/// applies), unless it comes from an app origin, which must carry one.
pub(crate) async fn app_session_caller(db: &PgPool, headers: &HeaderMap, artifact_id: &str) -> Result<Option<Caller>> {
    let Some(token) = bearer(headers).filter(|t| t.starts_with(SESSION_PREFIX)) else {
        refuse_app_origin(headers)?;
        return Ok(None);
    };
    let grant = sqlx::query_as::<_, Grant>(
        "SELECT user_id, email, name, org_id, org_role, artifact_id, app_slug \
         FROM app_sessions WHERE token_hash = $1 AND expires_at > now()",
    )
    .bind(token_hash(token))
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        ArtifactError::coded(
            StatusCode::UNAUTHORIZED,
            "app_session_invalid",
            "The app session has expired. Open the app again.",
        )
    })?;
    if grant.artifact_id != artifact_id {
        return Err(ArtifactError::coded(
            StatusCode::FORBIDDEN,
            "app_session_wrong_app",
            "This app session belongs to another app.",
        ));
    }
    Ok(Some(grant.caller()))
}

#[cfg(test)]
#[path = "app_handoff_tests.rs"]
mod tests;
