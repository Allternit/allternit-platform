//! Allternit Meetings calendar connections (docs/meetings/CONTRACT.md
//! section 4 and "Change 2026-11-13: ICS calendar links"; plan step 2.7).
//!
//! - `GET /api/v1/calendar/oauth/google/start?return_to=` (User, free ok):
//!   `200 { url }`, the Google consent URL (scope `calendar.events.readonly`
//!   plus `openid email`, offline access, PKCE S256). `state` is an
//!   HMAC-signed `{user, return_to, nonce, exp 10 min}`; the PKCE verifier is
//!   derived from the nonce with the server key, so it never travels.
//! - `GET /api/v1/calendar/oauth/google/callback` (no auth; state-verified,
//!   one use): exchanges the code, stores the tokens with credential_cipher
//!   and 302s to `return_to` with `calendar=connected` (+ `pick_rule=1` on
//!   the user's first calendar) or `calendar=error&reason=<code>`.
//! - `POST /api/v1/calendar/connections {provider:'ics', feed_url, label?}`:
//!   validates by one SSRF-guarded fetch that must parse as VCALENDAR, then
//!   syncs it at once. `GET` lists, `DELETE /:id` removes (Google token
//!   revoked best-effort; future meetings from it that Gizzi hasn't joined
//!   are removed).
//!
//! `return_to` allow-list: `https://ai.allternit.com/*`,
//! `https://m.allternit.com/*`, `allternit://meetings/*` (iOS), and
//! `http://localhost*` / `http://127.0.0.1*` only in development mode.
//! No Microsoft OAuth in v1.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use bytes::Bytes;
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::meetings::{self, MeetingError, Result};
use crate::services::meeting_calendar::{self as calendar, ics, ConnectionRow, GoogleConfig, CONNECTION_COLUMNS};
use crate::ApiState;

pub(crate) const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events.readonly";
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub(crate) const DEFAULT_RETURN_TO: &str = "https://ai.allternit.com/settings/meetings";
const STATE_TTL_SECS: i64 = 600;
const MAX_LABEL_CHARS: usize = 120;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/calendar/oauth/google/start", get(google_start))
        .route("/api/v1/calendar/oauth/google/callback", get(google_callback))
        .route("/api/v1/calendar/connections", get(list_connections).post(create_connection))
        .route("/api/v1/calendar/connections/:id", delete(delete_connection))
}

// ---------------------------------------------------------------------------
// return_to, state, PKCE (pure)
// ---------------------------------------------------------------------------

fn development_mode() -> bool {
    std::env::var("Allternit_API_DEVELOPMENT_MODE").map(|v| v == "true" || v == "1").unwrap_or(false)
}

/// The allow-listed `return_to`, or why it was refused. Absent = the web
/// settings page.
pub(crate) fn validate_return_to(raw: Option<&str>, dev: bool) -> std::result::Result<String, &'static str> {
    let Some(raw) = raw.map(str::trim).filter(|r| !r.is_empty()) else {
        return Ok(DEFAULT_RETURN_TO.to_string());
    };
    if raw.len() > 2048 || raw.contains('\\') {
        return Err("return_to is not an allowed address");
    }
    let url = url::Url::parse(raw).map_err(|_| "return_to must be an absolute URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("return_to is not an allowed address");
    }
    let host = url.host_str().unwrap_or_default();
    let ok = match url.scheme() {
        "https" => matches!(host, "ai.allternit.com" | "m.allternit.com") && url.port().is_none(),
        "http" => dev && matches!(host, "localhost" | "127.0.0.1"),
        "allternit" => host == "meetings",
        _ => false,
    };
    if ok {
        Ok(url.to_string())
    } else {
        Err("return_to is not an allowed address")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OAuthState {
    /// Clerk user id.
    pub u: String,
    /// Validated return_to.
    pub r: String,
    /// Nonce (one use; seeds the PKCE verifier).
    pub n: String,
    /// Expiry, unix seconds.
    pub e: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StateError {
    Malformed,
    BadSignature,
    Expired,
}

type HmacSha256 = Hmac<Sha256>;

fn mac(key: &[u8], domain: &str, data: &str) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac takes any key length");
    m.update(domain.as_bytes());
    m.update(b"\0");
    m.update(data.as_bytes());
    m
}

/// The signing key, derived from the Google client secret (server-only).
pub(crate) fn state_key(client_secret: &str) -> Vec<u8> {
    Sha256::digest(format!("allternit-calendar-oauth\0{client_secret}").as_bytes()).to_vec()
}

pub(crate) fn sign_state(key: &[u8], state: &OAuthState) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(state).expect("state serialises"));
    let sig = URL_SAFE_NO_PAD.encode(mac(key, "state", &payload).finalize().into_bytes());
    format!("{payload}.{sig}")
}

pub(crate) fn verify_state(key: &[u8], raw: &str, now: i64) -> std::result::Result<OAuthState, StateError> {
    let (payload, sig) = raw.split_once('.').ok_or(StateError::Malformed)?;
    let sig = URL_SAFE_NO_PAD.decode(sig).map_err(|_| StateError::Malformed)?;
    mac(key, "state", payload).verify_slice(&sig).map_err(|_| StateError::BadSignature)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).map_err(|_| StateError::Malformed)?;
    let state: OAuthState = serde_json::from_slice(&bytes).map_err(|_| StateError::Malformed)?;
    if state.e < now {
        return Err(StateError::Expired);
    }
    Ok(state)
}

/// PKCE verifier for a nonce: 43 url-safe chars, recomputable only here.
pub(crate) fn pkce_verifier(key: &[u8], nonce: &str) -> String {
    URL_SAFE_NO_PAD.encode(mac(key, "pkce", nonce).finalize().into_bytes())
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Each state works once per process (Google's codes are single-use too).
fn consume_nonce(nonce: &str, exp: i64, now: i64) -> bool {
    static USED: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
    let mut used = USED.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|p| p.into_inner());
    used.retain(|_, e| *e >= now);
    used.insert(nonce.to_string(), exp).is_none()
}

fn redirect_uri() -> String {
    let base = std::env::var("ALLTERNIT_PUBLIC_API_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://api.allternit.com".to_string());
    format!("{base}/api/v1/calendar/oauth/google/callback")
}

fn with_params(return_to: &str, params: &[(&str, &str)]) -> String {
    match url::Url::parse(return_to) {
        Ok(mut url) => {
            {
                let mut q = url.query_pairs_mut();
                for (k, v) in params {
                    q.append_pair(k, v);
                }
            }
            url.to_string()
        }
        Err(_) => DEFAULT_RETURN_TO.to_string(),
    }
}

fn unavailable() -> MeetingError {
    MeetingError::new(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "Google Calendar isn't set up on this server yet.")
}

// ---------------------------------------------------------------------------
// Google OAuth
// ---------------------------------------------------------------------------

async fn google_start(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>> {
    let caller = meetings::caller(&state, &headers).await?;
    let return_to = validate_return_to(query.get("return_to").map(String::as_str), development_mode())
        .map_err(MeetingError::invalid)?;
    let config = GoogleConfig::from_env().ok_or_else(unavailable)?;
    let key = state_key(&config.client_secret);
    let oauth_state = OAuthState {
        u: caller.id,
        r: return_to,
        n: Uuid::new_v4().simple().to_string(),
        e: Utc::now().timestamp() + STATE_TTL_SECS,
    };
    let challenge = pkce_challenge(&pkce_verifier(&key, &oauth_state.n));
    let mut url = url::Url::parse(GOOGLE_AUTH_URL).expect("auth url parses");
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &redirect_uri())
        .append_pair("response_type", "code")
        .append_pair("scope", &format!("openid email {GOOGLE_SCOPE}"))
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &sign_state(&key, &oauth_state))
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(Json(json!({ "url": url.to_string() })))
}

/// The email claim of a Google id_token. It came straight from Google's
/// token endpoint over TLS, so the signature needn't be re-checked (OIDC
/// core 3.1.3.7).
fn id_token_email(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    if claims.get("email_verified").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    claims.get("email").and_then(Value::as_str).map(|e| e.trim().to_ascii_lowercase()).filter(|e| e.contains('@'))
}

async fn google_callback(State(state): State<Arc<ApiState>>, Query(query): Query<HashMap<String, String>>) -> Response {
    let fail = |return_to: &str, reason: &str| {
        Redirect::to(&with_params(return_to, &[("calendar", "error"), ("reason", reason)])).into_response()
    };
    let Some(config) = GoogleConfig::from_env() else { return fail(DEFAULT_RETURN_TO, "unavailable") };
    let key = state_key(&config.client_secret);
    let now = Utc::now().timestamp();
    let oauth_state = match query.get("state").map(|s| verify_state(&key, s, now)) {
        Some(Ok(s)) => s,
        Some(Err(StateError::Expired)) => return fail(DEFAULT_RETURN_TO, "expired"),
        _ => return fail(DEFAULT_RETURN_TO, "invalid_state"),
    };
    let return_to = oauth_state.r.clone();
    if !consume_nonce(&oauth_state.n, oauth_state.e, now) {
        return fail(&return_to, "invalid_state");
    }
    if let Some(error) = query.get("error") {
        let reason = if error == "access_denied" { "access_denied" } else { "provider_error" };
        return fail(&return_to, reason);
    }
    let Some(code) = query.get("code").filter(|c| !c.is_empty()) else { return fail(&return_to, "invalid_request") };

    let verifier = pkce_verifier(&key, &oauth_state.n);
    let redirect = redirect_uri();
    let exchanged = calendar::http()
        .post(calendar::GOOGLE_TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await;
    let token: calendar::TokenResponse = match exchanged {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(t) => t,
            Err(_) => return fail(&return_to, "provider_error"),
        },
        Ok(r) => {
            tracing::warn!(status = %r.status(), "calendar oauth: code exchange refused");
            return fail(&return_to, "provider_error");
        }
        Err(e) => {
            tracing::warn!("calendar oauth: code exchange failed: {e}");
            return fail(&return_to, "provider_error");
        }
    };
    let scopes: Vec<String> = token.scope.as_deref().unwrap_or_default().split_whitespace().map(str::to_string).collect();
    if !scopes.iter().any(|s| s == GOOGLE_SCOPE) {
        return fail(&return_to, "scope_denied");
    }
    let (Some(access), Some(email)) = (token.access_token.as_deref(), token.id_token.as_deref().and_then(id_token_email))
    else {
        return fail(&return_to, "provider_error");
    };
    match store_google(&state, &oauth_state.u, &email, access, token.expires_in, token.refresh_token.as_deref(), &scopes).await {
        Ok((id, first)) => {
            let sync_state = state.clone();
            tokio::spawn(async move { calendar::sync_connection_by_id(&sync_state, id).await });
            let mut params = vec![("calendar", "connected")];
            if first {
                params.push(("pick_rule", "1"));
            }
            Redirect::to(&with_params(&return_to, &params)).into_response()
        }
        Err(reason) => fail(&return_to, reason),
    }
}

/// Upsert the Google connection. Returns its id and whether it is the
/// user's first calendar (the UI then asks the auto-join rule).
async fn store_google(
    state: &ApiState,
    user_id: &str,
    email: &str,
    access: &str,
    expires_in: Option<i64>,
    refresh: Option<&str>,
    scopes: &[String],
) -> std::result::Result<(Uuid, bool), &'static str> {
    let db = &state.db;
    let internal = |e: sqlx::Error| {
        tracing::error!("calendar oauth: store failed: {e}");
        "internal"
    };
    let others: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calendar_connections WHERE user_id = $1 AND NOT (provider = 'google' AND account_email = $2)")
        .bind(user_id)
        .bind(email)
        .fetch_one(db)
        .await
        .map_err(internal)?;
    let refresh_sealed = match refresh {
        Some(r) => Some(calendar::seal(state, r).map_err(|_| "internal")?),
        None => {
            // Google sends a refresh token on each prompt=consent grant;
            // without one, an existing row must already have it.
            let has: Option<bool> = sqlx::query_scalar(
                "SELECT encrypted_refresh_token IS NOT NULL FROM calendar_connections \
                 WHERE user_id = $1 AND provider = 'google' AND account_email = $2",
            )
            .bind(user_id)
            .bind(email)
            .fetch_optional(db)
            .await
            .map_err(internal)?;
            if has != Some(true) {
                return Err("no_refresh_token");
            }
            None
        }
    };
    let access_sealed = calendar::seal(state, access).map_err(|_| "internal")?;
    let expires = Utc::now() + chrono::Duration::seconds(expires_in.unwrap_or(3600));
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO calendar_connections (user_id, provider, account_email, encrypted_refresh_token, \
             encrypted_access_token, access_expires_at, scopes, status) \
         VALUES ($1, 'google', $2, $3, $4, $5, $6, 'active') \
         ON CONFLICT (user_id, provider, COALESCE(account_email, feed_url_hash)) DO UPDATE SET \
             encrypted_refresh_token = COALESCE(EXCLUDED.encrypted_refresh_token, calendar_connections.encrypted_refresh_token), \
             encrypted_access_token = EXCLUDED.encrypted_access_token, access_expires_at = EXCLUDED.access_expires_at, \
             scopes = EXCLUDED.scopes, status = 'active', sync_token = NULL, last_synced_at = NULL, updated_at = now() \
         RETURNING id",
    )
    .bind(user_id)
    .bind(email)
    .bind(refresh_sealed)
    .bind(access_sealed)
    .bind(expires)
    .bind(scopes)
    .fetch_one(db)
    .await
    .map_err(internal)?;
    Ok((id, others == 0))
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

async fn list_connections(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Json<Value>> {
    let caller = meetings::caller(&state, &headers).await?;
    meetings::require_paid(&state.db, &caller).await?;
    let rows = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {CONNECTION_COLUMNS} FROM calendar_connections WHERE user_id = $1 ORDER BY created_at"
    ))
    .bind(&caller.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "connections": rows.iter().map(calendar::connection_json).collect::<Vec<_>>() })))
}

async fn create_connection(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Result<Response> {
    let caller = meetings::caller(&state, &headers).await?;
    meetings::require_paid(&state.db, &caller).await?;
    let body: Value = serde_json::from_slice(&body).map_err(|_| MeetingError::invalid("Body must be JSON."))?;
    match body.get("provider").and_then(Value::as_str) {
        Some("ics") => {}
        Some("google") => {
            return Err(MeetingError::invalid("Connect Google Calendar with /api/v1/calendar/oauth/google/start."))
        }
        _ => return Err(MeetingError::invalid("provider must be 'ics'.")),
    }
    let raw = body.get("feed_url").and_then(Value::as_str).ok_or_else(|| MeetingError::invalid("feed_url is required."))?;
    let label = match body.get("label") {
        None | Some(Value::Null) => None,
        Some(Value::String(l)) if l.chars().count() <= MAX_LABEL_CHARS => Some(l.trim().to_string()).filter(|l| !l.is_empty()),
        Some(_) => return Err(MeetingError::invalid(format!("label must be text up to {MAX_LABEL_CHARS} characters."))),
    };
    let feed_url = calendar::normalise_feed_url(raw)
        .ok_or_else(|| MeetingError::invalid(calendar::FeedError::InvalidUrl.message()))?;
    let hash = hex::encode(Sha256::digest(feed_url.as_bytes()));

    let existing = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {CONNECTION_COLUMNS} FROM calendar_connections WHERE user_id = $1 AND provider = 'ics' AND feed_url_hash = $2"
    ))
    .bind(&caller.id)
    .bind(&hash)
    .fetch_optional(&state.db)
    .await?;
    if let Some(row) = existing {
        return Ok((StatusCode::OK, Json(calendar::connection_json(&row))).into_response());
    }

    let fetched = calendar::fetch_feed(&feed_url, None).await.map_err(|e| MeetingError::invalid(e.message()))?;
    if !ics::is_vcalendar(&fetched.body) {
        return Err(MeetingError::invalid("That link didn't return a calendar (.ics). Copy the ICS or webcal link."));
    }
    let now = Utc::now();
    let name = ics::parse_feed(&fetched.body, now, now).and_then(|f| f.name);
    let host = url::Url::parse(&feed_url).ok().and_then(|u| u.host_str().map(str::to_string));
    let label = label.or(name).or(host);
    let inserted = sqlx::query_as::<_, ConnectionRow>(&format!(
        "INSERT INTO calendar_connections (user_id, provider, label, encrypted_feed_url, feed_url_hash, status, last_synced_at, sync_token) \
         VALUES ($1, 'ics', $2, $3, $4, 'active', now(), $5) RETURNING {CONNECTION_COLUMNS}"
    ))
    .bind(&caller.id)
    .bind(label.as_deref())
    .bind(calendar::seal(&state, &feed_url)?)
    .bind(&hash)
    .bind(calendar::IcsValidators::from_fetch(&fetched, now).to_json())
    .fetch_one(&state.db)
    .await;
    let row = match inserted {
        Ok(row) => row,
        Err(e) if meetings::is_unique_violation(&e) => {
            return Err(MeetingError::conflict("That calendar was just connected."));
        }
        Err(e) => return Err(e.into()),
    };
    // First sync from the body already in hand.
    calendar::sync_connection(&state, &row, Some(fetched)).await;
    Ok((StatusCode::CREATED, Json(calendar::connection_json(&row))).into_response())
}

/// Removing a calendar works on any plan (it only deletes the user's data).
async fn delete_connection(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let caller = meetings::caller(&state, &headers).await?;
    let not_found = || MeetingError::new(StatusCode::NOT_FOUND, "not_found", "Calendar connection not found.");
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let row = sqlx::query_as::<_, ConnectionRow>(&format!(
        "SELECT {CONNECTION_COLUMNS} FROM calendar_connections WHERE id = $1 AND user_id = $2"
    ))
    .bind(id)
    .bind(&caller.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(not_found)?;
    if row.provider == "google" {
        if let Some(token) = row.encrypted_refresh_token.as_deref().and_then(|s| calendar::open(&state, s)) {
            if let Err(e) = calendar::http().post(calendar::GOOGLE_REVOKE_URL).form(&[("token", token.as_str())]).send().await {
                tracing::info!("calendar: Google revoke failed (row deleted anyway): {e}");
            }
        }
    }
    // Gizzi shouldn't join meetings from a calendar the user disconnected.
    let upcoming: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM meetings WHERE calendar_connection_id = $1 AND (starts_at IS NULL OR starts_at >= now() - interval '1 hour')",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    for meeting in upcoming {
        calendar::remove_calendar_meeting(&state.db, meeting).await?;
    }
    sqlx::query("DELETE FROM calendar_connections WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn sample(exp: i64) -> OAuthState {
        OAuthState { u: "user_1".into(), r: DEFAULT_RETURN_TO.into(), n: "nonce-1".into(), e: exp }
    }

    #[test]
    fn calendar_oauth_state_round_trips_and_rejects_tampering_and_expiry() {
        let now = 1_800_000_000;
        let signed = sign_state(KEY, &sample(now + 600));
        assert_eq!(verify_state(KEY, &signed, now), Ok(sample(now + 600)));
        assert_eq!(verify_state(KEY, &signed, now + 601), Err(StateError::Expired));
        assert_eq!(verify_state(b"another key", &signed, now), Err(StateError::BadSignature));

        // Swap in another user's payload under the old signature.
        let (_, sig) = signed.split_once('.').unwrap();
        let forged_payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&OAuthState { u: "attacker".into(), ..sample(now + 600) }).unwrap(),
        );
        assert_eq!(verify_state(KEY, &format!("{forged_payload}.{sig}"), now), Err(StateError::BadSignature));
        assert_eq!(verify_state(KEY, "no-dot", now), Err(StateError::Malformed));
        assert_eq!(verify_state(KEY, "a.b.c", now), Err(StateError::Malformed));
    }

    #[test]
    fn calendar_oauth_pkce_verifier_is_secret_derived_and_s256() {
        let verifier = pkce_verifier(KEY, "nonce-1");
        assert_eq!(verifier.len(), 43, "RFC 7636: 43..128 chars");
        assert_eq!(verifier, pkce_verifier(KEY, "nonce-1"), "recomputable at the callback");
        assert_ne!(verifier, pkce_verifier(KEY, "nonce-2"));
        assert_ne!(verifier, pkce_verifier(b"other", "nonce-1"));
        // RFC 7636 appendix B test vector for S256.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let signed = sign_state(KEY, &sample(2_000_000_000));
        assert!(!signed.contains(&verifier), "the verifier never travels in the state");
    }

    #[test]
    fn calendar_oauth_nonce_is_single_use() {
        let nonce = Uuid::new_v4().to_string();
        assert!(consume_nonce(&nonce, 2_000, 1_000));
        assert!(!consume_nonce(&nonce, 2_000, 1_000), "a replayed callback is refused");
    }

    #[test]
    fn calendar_oauth_return_to_allow_list() {
        let ok = |raw: &str, dev: bool| validate_return_to(Some(raw), dev).is_ok();
        assert_eq!(validate_return_to(None, false).unwrap(), DEFAULT_RETURN_TO);
        assert_eq!(validate_return_to(Some("  "), false).unwrap(), DEFAULT_RETURN_TO);
        assert!(ok("https://ai.allternit.com/settings/meetings", false));
        assert!(ok("https://m.allternit.com/meetings?tab=1", false));
        assert!(ok("allternit://meetings/calendar-connected", false));
        assert!(ok("http://localhost:5173/settings/meetings", true));
        assert!(ok("http://127.0.0.1:3000/x", true));
        for bad in [
            "http://localhost:5173/settings/meetings",
            "http://ai.allternit.com/settings",
            "https://ai.allternit.com.evil.com/x",
            "https://evil.com/?https://ai.allternit.com/",
            "https://ai.allternit.com@evil.com/x",
            "https://user@ai.allternit.com/x",
            "https://ai.allternit.com:8443/x",
            "https://allternit.com/x",
            "allternit://settings/x",
            "javascript:alert(1)",
            "//ai.allternit.com/x",
            "https:\\\\evil.com",
        ] {
            assert!(!ok(bad, false), "{bad} must be refused");
        }
        assert!(!ok("http://evil.com/x", true), "dev mode only opens localhost");
    }

    #[test]
    fn calendar_oauth_callback_params_keep_the_existing_query() {
        assert_eq!(
            with_params("https://ai.allternit.com/settings/meetings?tab=cal", &[("calendar", "connected"), ("pick_rule", "1")]),
            "https://ai.allternit.com/settings/meetings?tab=cal&calendar=connected&pick_rule=1"
        );
        assert_eq!(
            with_params("allternit://meetings/calendar-connected", &[("calendar", "error"), ("reason", "access_denied")]),
            "allternit://meetings/calendar-connected?calendar=error&reason=access_denied"
        );
    }
}
