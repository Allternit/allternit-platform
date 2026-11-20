//! Tokens for "Sign in with Allternit"
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part A: A3, A4, A5, A8, A9, A10;
//! migration 150). `routes/oauth_server.rs` serves these over HTTP.
//!
//! | Token | Format | Lifetime |
//! |---|---|---|
//! | Access | `alt_oat_` + 64 hex | 3600 s |
//! | Refresh | `alt_ort_` + 64 hex | 90 days from issue, rotating; only with `offline_access` |
//!
//! Only `services::api_keys::hash_token` of a token (lowercase hex SHA-256) is
//! stored, in `oauth_tokens.token_hash`, plus `prefix` (the type prefix and 8
//! hex chars) for display (D1). Tokens carry their grant's `scopes`: a new
//! consent that changes the grant changes what every token on it may do.
//!
//! # Lock order
//! Every path takes row locks in this order: `oauth_auth_requests`, then
//! `oauth_grants`, then `oauth_tokens`. The consent approve (step 3) takes the
//! first two in the same order, so none of them can deadlock another.
//!
//! # Refresh rotation (A8)
//! A refresh locks the grant, then the token. A live refresh token is marked
//! `rotated_at = now()`, `replaced_by = <new refresh token id>`, and a new
//! access and refresh token are issued. The new pair is not random: each is
//! `HMAC-SHA256(key = the presented refresh token, "allternit-oauth-rotate\0"
//! || kind || "\0" || replaced_by)`. So the same old token presented again
//! within 30 s (a network retry, or a second request that lost the race) gets
//! the same pair back, and nothing but hashes is stored. Recomputing a pair
//! takes both the old token (held only by the client) and `replaced_by` (held
//! only by the database). After 30 s, or when the replacement was itself
//! rotated or revoked, the old token is reuse: the grant and every token on it
//! are revoked, audited `refresh_reuse` and `grant_revoked`, and the client
//! gets `invalid_grant`.
//!
//! # Authorization codes
//! Step 3 stores a code as `oauth_auth_requests.code_hash` (see the doc at the
//! top of `routes/oauth_server.rs`). [`exchange_code`] redeems it once
//! (`status` `approved` → `used`). A code presented again is reuse (A3): the
//! live grant for (app, user) and its tokens are revoked, audited `code_reuse`.
//! Each redemption also deletes up to 500 requests whose consent window ended
//! more than a day ago.
//!
//! # Bearer tokens for the API
//! [`authenticate_access_token`] is the one check every `alt_oat_` caller uses
//! (`/oauth2/userinfo`, `/api/v1/oauth/usage`, and the inference path).

use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use crate::services::api_keys::hash_token;
use crate::services::oauth_apps::{self, record_audit, AppState, AuditCtx, AuditEvent, OAuthApp};

type HmacSha256 = Hmac<Sha256>;

pub const ACCESS_TOKEN_PREFIX: &str = "alt_oat_";
pub const REFRESH_TOKEN_PREFIX: &str = "alt_ort_";
/// A3: access tokens live one hour.
pub const ACCESS_TOKEN_TTL_SECONDS: i64 = 3600;
/// A3: refresh tokens live 90 days from issue.
pub const REFRESH_TOKEN_TTL_DAYS: i64 = 90;
/// A8: a rotated refresh token presented again within this window gets the same new pair.
pub const REFRESH_GRACE_SECONDS: i64 = 30;
/// Hex characters kept after the type prefix, for display (A3).
const TOKEN_DISPLAY_CHARS: usize = 8;
/// Housekeeping: at most this many stale consent requests per redemption.
const PRUNE_BATCH: i64 = 500;
const OFFLINE_ACCESS: &str = "offline_access";

// ─── errors ───────────────────────────────────────────────────────────────────

/// RFC 6749 §5.2 errors of the token and revocation endpoints (A9).
#[derive(Debug)]
pub enum TokenError {
    InvalidRequest(String),
    /// 401 per A9.
    InvalidClient(String),
    InvalidGrant(String),
    UnauthorizedClient(String),
    UnsupportedGrantType(String),
    InvalidScope(String),
    /// 500 `server_error`; the cause is logged, never sent.
    Server(ApiError),
}

impl TokenError {
    pub fn code(&self) -> &'static str {
        match self {
            TokenError::InvalidRequest(_) => "invalid_request",
            TokenError::InvalidClient(_) => "invalid_client",
            TokenError::InvalidGrant(_) => "invalid_grant",
            TokenError::UnauthorizedClient(_) => "unauthorized_client",
            TokenError::UnsupportedGrantType(_) => "unsupported_grant_type",
            TokenError::InvalidScope(_) => "invalid_scope",
            TokenError::Server(_) => "server_error",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            TokenError::InvalidClient(_) => StatusCode::UNAUTHORIZED,
            TokenError::Server(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    pub fn description(&self) -> String {
        match self {
            TokenError::InvalidRequest(m)
            | TokenError::InvalidClient(m)
            | TokenError::InvalidGrant(m)
            | TokenError::UnauthorizedClient(m)
            | TokenError::UnsupportedGrantType(m)
            | TokenError::InvalidScope(m) => m.clone(),
            TokenError::Server(_) => "The server could not finish the request. Try again.".to_string(),
        }
    }
}

impl From<ApiError> for TokenError {
    fn from(e: ApiError) -> Self {
        TokenError::Server(e)
    }
}

impl From<sqlx::Error> for TokenError {
    fn from(e: sqlx::Error) -> Self {
        TokenError::Server(ApiError::from(e))
    }
}

fn invalid_grant(message: &str) -> TokenError {
    TokenError::InvalidGrant(message.to_string())
}

// ─── tokens ───────────────────────────────────────────────────────────────────

/// A token response (A1: `POST /oauth2/token`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedTokens {
    pub access_token: String,
    /// Only when the grant has `offline_access`.
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub scopes: Vec<String>,
}

impl IssuedTokens {
    pub fn to_json(&self) -> Value {
        let mut body = json!({
            "access_token": self.access_token,
            "token_type": "Bearer",
            "expires_in": self.expires_in,
            "scope": self.scopes.join(" "),
        });
        if let Some(refresh) = &self.refresh_token {
            body["refresh_token"] = json!(refresh);
        }
        body
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Access,
    Refresh,
}

impl TokenKind {
    fn as_str(self) -> &'static str {
        match self {
            TokenKind::Access => "access",
            TokenKind::Refresh => "refresh",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            TokenKind::Access => ACCESS_TOKEN_PREFIX,
            TokenKind::Refresh => REFRESH_TOKEN_PREFIX,
        }
    }

    /// A Postgres interval literal; constants only.
    fn ttl(self) -> String {
        match self {
            TokenKind::Access => format!("{ACCESS_TOKEN_TTL_SECONDS} seconds"),
            TokenKind::Refresh => format!("{REFRESH_TOKEN_TTL_DAYS} days"),
        }
    }
}

fn random_token(kind: TokenKind) -> String {
    format!("{}{}", kind.prefix(), hex::encode(rand::random::<[u8; 32]>()))
}

/// The pair a rotation issues; see "Refresh rotation" above.
fn rotated_token(presented_refresh: &str, replaced_by: Uuid, kind: TokenKind) -> String {
    let mut mac = HmacSha256::new_from_slice(presented_refresh.as_bytes()).expect("HMAC takes any key length");
    mac.update(b"allternit-oauth-rotate\0");
    mac.update(kind.as_str().as_bytes());
    mac.update(b"\0");
    mac.update(replaced_by.as_bytes());
    format!("{}{}", kind.prefix(), hex::encode(mac.finalize().into_bytes()))
}

fn display_prefix(token: &str) -> String {
    token.chars().take(ACCESS_TOKEN_PREFIX.len() + TOKEN_DISPLAY_CHARS).collect()
}

async fn insert_token(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    grant_id: Uuid,
    kind: TokenKind,
    token: &str,
) -> Result<(), ApiError> {
    sqlx::query(&format!(
        "INSERT INTO oauth_tokens (id, grant_id, kind, token_hash, prefix, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + interval '{}')",
        kind.ttl()
    ))
    .bind(id)
    .bind(grant_id)
    .bind(kind.as_str())
    .bind(hash_token(token))
    .bind(display_prefix(token))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn has_offline_access(scopes: &[String]) -> bool {
    scopes.iter().any(|s| s == OFFLINE_ACCESS)
}

/// A fresh random pair for a code exchange.
async fn issue_new(
    tx: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    scopes: &[String],
) -> Result<IssuedTokens, ApiError> {
    let access = random_token(TokenKind::Access);
    insert_token(tx, Uuid::new_v4(), grant_id, TokenKind::Access, &access).await?;
    let refresh = if has_offline_access(scopes) {
        let refresh = random_token(TokenKind::Refresh);
        insert_token(tx, Uuid::new_v4(), grant_id, TokenKind::Refresh, &refresh).await?;
        Some(refresh)
    } else {
        None
    };
    Ok(IssuedTokens {
        access_token: access,
        refresh_token: refresh,
        expires_in: ACCESS_TOKEN_TTL_SECONDS,
        scopes: scopes.to_vec(),
    })
}

/// Revokes a grant and every token on it, inside the caller's transaction.
/// The caller holds (or takes here first) the grant's row lock. Returns true
/// when the grant was live until now. Also for step 7's `DELETE /api/v1/oauth/grants/:id`.
pub async fn revoke_grant_in_tx(tx: &mut Transaction<'_, Postgres>, grant_id: Uuid) -> Result<bool, ApiError> {
    let revoked = sqlx::query("UPDATE oauth_grants SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(grant_id)
        .execute(&mut **tx)
        .await?
        .rows_affected()
        == 1;
    sqlx::query("UPDATE oauth_tokens SET revoked_at = now() WHERE grant_id = $1 AND revoked_at IS NULL")
        .bind(grant_id)
        .execute(&mut **tx)
        .await?;
    Ok(revoked)
}

// ─── PKCE and client authentication ───────────────────────────────────────────

/// RFC 7636 §4.1: 43 to 128 unreserved characters.
pub fn is_valid_code_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// `BASE64URL(SHA256(verifier))`, no padding (RFC 7636 §4.2).
pub fn s256_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// D2 / A5: a confidential app sends `client_id` + `client_secret` in the body
/// (`client_secret_post`); a public app sends only `client_id` and proves
/// itself with PKCE. Any mismatch is 401 `invalid_client`.
pub async fn authenticate_client(
    db: &PgPool,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<OAuthApp, TokenError> {
    let Some(client_id) = client_id else {
        return Err(TokenError::InvalidClient("Send client_id.".to_string()));
    };
    let Some(app) = oauth_apps::get_app_by_client_id(db, client_id).await? else {
        return Err(TokenError::InvalidClient("No app is registered with this client_id.".to_string()));
    };
    if app.is_public() {
        if client_secret.is_some() {
            return Err(TokenError::InvalidClient(
                "This is a public app: send no client_secret. PKCE is its proof.".to_string(),
            ));
        }
    } else {
        match client_secret {
            Some(secret) if app.verify_secret(secret) => {}
            Some(_) => return Err(TokenError::InvalidClient("The client secret is wrong.".to_string())),
            None => {
                return Err(TokenError::InvalidClient(
                    "This app is confidential: send client_secret in the body (client_secret_post).".to_string(),
                ))
            }
        }
    }
    Ok(app)
}

fn require_not_suspended(app: &OAuthApp) -> Result<(), TokenError> {
    if app.app_state() == AppState::Suspended {
        return Err(TokenError::UnauthorizedClient("This app is suspended.".to_string()));
    }
    Ok(())
}

// ─── grant_type=authorization_code ────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct CodeRow {
    id: String,
    app_id: Uuid,
    user_id: Option<String>,
    redirect_uri: String,
    code_challenge: String,
    status: String,
    code_live: bool,
}

/// Redeems an authorization code for tokens. `app` is the authenticated client.
pub async fn exchange_code(
    db: &PgPool,
    app: &OAuthApp,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
    ctx: &AuditCtx,
) -> Result<IssuedTokens, TokenError> {
    const BAD_CODE: &str = "The authorization code is not valid.";
    require_not_suspended(app)?;
    if !is_valid_code_verifier(code_verifier) {
        return Err(TokenError::InvalidRequest(
            "code_verifier must be 43 to 128 characters of A-Z, a-z, 0-9, '-', '.', '_' or '~' (RFC 7636).".to_string(),
        ));
    }

    let mut tx = db.begin().await?;
    let row: Option<CodeRow> = sqlx::query_as(
        "SELECT id, app_id, user_id, redirect_uri, code_challenge, status,
                COALESCE(code_expires_at > now(), false) AS code_live
         FROM oauth_auth_requests WHERE code_hash = $1 FOR UPDATE",
    )
    .bind(hash_token(code))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else { return Err(invalid_grant(BAD_CODE)) };
    if row.app_id != app.id {
        return Err(invalid_grant(BAD_CODE));
    }

    if row.status == "used" {
        // A3: a code used twice may be stolen. Revoke what was issued for it.
        let mut grant_id = None;
        if let Some(user_id) = row.user_id.as_deref() {
            grant_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM oauth_grants WHERE app_id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
            )
            .bind(app.id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
        }
        record_audit(&mut *tx, ctx, Some(app.id), grant_id, AuditEvent::CodeReuse, json!({ "request_id": row.id }))
            .await?;
        if let Some(grant_id) = grant_id {
            if revoke_grant_in_tx(&mut tx, grant_id).await? {
                record_audit(
                    &mut *tx,
                    ctx,
                    Some(app.id),
                    Some(grant_id),
                    AuditEvent::GrantRevoked,
                    json!({ "reason": "code_reuse", "request_id": row.id }),
                )
                .await?;
            }
        }
        tx.commit().await?;
        return Err(invalid_grant(
            "This authorization code was already used. Its tokens are revoked; ask the user to sign in again.",
        ));
    }
    if row.status != "approved" || !row.code_live {
        return Err(invalid_grant("The authorization code has expired or is not valid."));
    }
    if row.redirect_uri != redirect_uri {
        return Err(invalid_grant("redirect_uri does not match the one in the authorization request."));
    }
    if !constant_time_eq(&s256_challenge(code_verifier), &row.code_challenge) {
        return Err(invalid_grant("code_verifier does not match the code_challenge."));
    }
    let Some(user_id) = row.user_id.as_deref() else { return Err(invalid_grant(BAD_CODE)) };

    let grant: Option<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT id, scopes FROM oauth_grants WHERE app_id = $1 AND user_id = $2 AND revoked_at IS NULL FOR UPDATE",
    )
    .bind(app.id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("UPDATE oauth_auth_requests SET status = 'used' WHERE id = $1")
        .bind(&row.id)
        .execute(&mut *tx)
        .await?;
    let Some((grant_id, scopes)) = grant else {
        // The person revoked the app between consent and this call.
        tx.commit().await?;
        return Err(invalid_grant("This sign-in was revoked. Ask the user to sign in again."));
    };

    let issued = issue_new(&mut tx, grant_id, &scopes).await?;
    sqlx::query("UPDATE oauth_grants SET last_used_at = now() WHERE id = $1").bind(grant_id).execute(&mut *tx).await?;
    record_audit(
        &mut *tx,
        ctx,
        Some(app.id),
        Some(grant_id),
        AuditEvent::TokenIssued,
        json!({
            "grant_type": "authorization_code",
            "request_id": row.id,
            "scopes": scopes,
            "refresh_token": issued.refresh_token.is_some(),
        }),
    )
    .await?;
    tx.commit().await?;

    if let Err(e) = prune_expired_requests(db).await {
        tracing::warn!(error = %e, "oauth: could not prune old authorization requests");
    }
    Ok(issued)
}

/// Deletes up to 500 consent requests whose window ended over a day ago.
/// Their codes expired long before, so only reuse detection of a day-old code
/// is lost (it then reads as an unknown code: `invalid_grant`).
pub async fn prune_expired_requests(db: &PgPool) -> Result<u64, ApiError> {
    Ok(sqlx::query(&format!(
        "DELETE FROM oauth_auth_requests WHERE id IN (
             SELECT id FROM oauth_auth_requests WHERE expires_at < now() - interval '1 day'
             LIMIT {PRUNE_BATCH} FOR UPDATE SKIP LOCKED)"
    ))
    .execute(db)
    .await?
    .rows_affected())
}

// ─── grant_type=refresh_token ─────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct RefreshRow {
    live: bool,
    revoked: bool,
    rotated: bool,
    in_grace: bool,
    replaced_by: Option<Uuid>,
}

/// Rotates a refresh token (A8). `app` is the authenticated client. The
/// caller has already refused a `scope` parameter (`invalid_scope`).
pub async fn refresh(
    db: &PgPool,
    app: &OAuthApp,
    refresh_token: &str,
    ctx: &AuditCtx,
) -> Result<IssuedTokens, TokenError> {
    const BAD_TOKEN: &str = "The refresh token is not valid.";
    require_not_suspended(app)?;
    if !refresh_token.starts_with(REFRESH_TOKEN_PREFIX) {
        return Err(invalid_grant(BAD_TOKEN));
    }
    let found: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT t.id, t.grant_id, g.app_id FROM oauth_tokens t JOIN oauth_grants g ON g.id = t.grant_id
         WHERE t.token_hash = $1 AND t.kind = 'refresh'",
    )
    .bind(hash_token(refresh_token))
    .fetch_optional(db)
    .await?;
    let Some((token_id, grant_id, app_id)) = found else { return Err(invalid_grant(BAD_TOKEN)) };
    if app_id != app.id {
        return Err(invalid_grant(BAD_TOKEN));
    }

    let mut tx = db.begin().await?;
    let grant: Option<(bool, Vec<String>)> =
        sqlx::query_as("SELECT revoked_at IS NOT NULL, scopes FROM oauth_grants WHERE id = $1 FOR UPDATE")
            .bind(grant_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((grant_revoked, scopes)) = grant else { return Err(invalid_grant(BAD_TOKEN)) };
    let token: Option<RefreshRow> = sqlx::query_as(&format!(
        "SELECT expires_at > now() AS live,
                revoked_at IS NOT NULL AS revoked,
                rotated_at IS NOT NULL AS rotated,
                COALESCE(rotated_at > now() - interval '{REFRESH_GRACE_SECONDS} seconds', false) AS in_grace,
                replaced_by
         FROM oauth_tokens WHERE id = $1 FOR UPDATE"
    ))
    .bind(token_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(token) = token else { return Err(invalid_grant(BAD_TOKEN)) };

    if grant_revoked || token.revoked {
        return Err(invalid_grant("This sign-in was revoked. Ask the user to sign in again."));
    }
    // A new consent can drop offline_access; then no refresh token works.
    if !has_offline_access(&scopes) {
        return Err(invalid_grant("This sign-in no longer includes offline_access. Ask the user to sign in again."));
    }

    if token.rotated {
        if token.in_grace {
            if let Some(replaced_by) = token.replaced_by {
                if let Some(pair) = replay_rotation(&mut tx, grant_id, refresh_token, replaced_by, &scopes).await? {
                    tx.commit().await?;
                    return Ok(pair);
                }
            }
        }
        // A8: reuse after the grace. Revoke the whole grant.
        revoke_grant_in_tx(&mut tx, grant_id).await?;
        record_audit(
            &mut *tx,
            ctx,
            Some(app.id),
            Some(grant_id),
            AuditEvent::RefreshReuse,
            json!({ "refresh_token_id": token_id }),
        )
        .await?;
        record_audit(
            &mut *tx,
            ctx,
            Some(app.id),
            Some(grant_id),
            AuditEvent::GrantRevoked,
            json!({ "reason": "refresh_reuse" }),
        )
        .await?;
        tx.commit().await?;
        return Err(invalid_grant(
            "This refresh token was already used. The sign-in is revoked; ask the user to sign in again.",
        ));
    }
    if !token.live {
        return Err(invalid_grant("The refresh token has expired. Ask the user to sign in again."));
    }

    let replaced_by = Uuid::new_v4();
    let access = rotated_token(refresh_token, replaced_by, TokenKind::Access);
    let new_refresh = rotated_token(refresh_token, replaced_by, TokenKind::Refresh);
    insert_token(&mut tx, Uuid::new_v4(), grant_id, TokenKind::Access, &access).await?;
    insert_token(&mut tx, replaced_by, grant_id, TokenKind::Refresh, &new_refresh).await?;
    sqlx::query("UPDATE oauth_tokens SET rotated_at = now(), replaced_by = $2 WHERE id = $1")
        .bind(token_id)
        .bind(replaced_by)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE oauth_grants SET last_used_at = now() WHERE id = $1").bind(grant_id).execute(&mut *tx).await?;
    record_audit(
        &mut *tx,
        ctx,
        Some(app.id),
        Some(grant_id),
        AuditEvent::RefreshRotated,
        json!({ "refresh_token_id": token_id, "replaced_by": replaced_by }),
    )
    .await?;
    tx.commit().await?;
    Ok(IssuedTokens {
        access_token: access,
        refresh_token: Some(new_refresh),
        expires_in: ACCESS_TOKEN_TTL_SECONDS,
        scopes,
    })
}

/// The pair an earlier rotation of `presented` issued, when both of its
/// tokens are still live and the new refresh token has not been rotated in
/// turn. `None` means the presentation is reuse.
async fn replay_rotation(
    tx: &mut Transaction<'_, Postgres>,
    grant_id: Uuid,
    presented: &str,
    replaced_by: Uuid,
    scopes: &[String],
) -> Result<Option<IssuedTokens>, ApiError> {
    let new_refresh = rotated_token(presented, replaced_by, TokenKind::Refresh);
    let refresh_live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM oauth_tokens
                        WHERE id = $1 AND grant_id = $2 AND kind = 'refresh' AND token_hash = $3
                          AND revoked_at IS NULL AND rotated_at IS NULL AND expires_at > now())",
    )
    .bind(replaced_by)
    .bind(grant_id)
    .bind(hash_token(&new_refresh))
    .fetch_one(&mut **tx)
    .await?;
    if !refresh_live {
        return Ok(None);
    }
    let access = rotated_token(presented, replaced_by, TokenKind::Access);
    let remaining: Option<f64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM expires_at - now())::float8 FROM oauth_tokens
         WHERE token_hash = $1 AND grant_id = $2 AND kind = 'access' AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(hash_token(&access))
    .bind(grant_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(remaining) = remaining else { return Ok(None) };
    Ok(Some(IssuedTokens {
        access_token: access,
        refresh_token: Some(new_refresh),
        expires_in: (remaining.floor() as i64).max(1),
        scopes: scopes.to_vec(),
    }))
}

// ─── revocation (RFC 7009) ────────────────────────────────────────────────────

/// Revokes `token` when it belongs to `app`; anything else is a silent no-op
/// (the endpoint answers 200 either way). A refresh token revokes its grant
/// and every token on it; an access token revokes only itself.
pub async fn revoke_token(db: &PgPool, app: &OAuthApp, token: &str, ctx: &AuditCtx) -> Result<(), ApiError> {
    let kind = if token.starts_with(ACCESS_TOKEN_PREFIX) {
        TokenKind::Access
    } else if token.starts_with(REFRESH_TOKEN_PREFIX) {
        TokenKind::Refresh
    } else {
        return Ok(());
    };
    let found: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT t.id, t.grant_id, g.app_id FROM oauth_tokens t JOIN oauth_grants g ON g.id = t.grant_id
         WHERE t.token_hash = $1 AND t.kind = $2",
    )
    .bind(hash_token(token))
    .bind(kind.as_str())
    .fetch_optional(db)
    .await?;
    let Some((token_id, grant_id, app_id)) = found else { return Ok(()) };
    if app_id != app.id {
        return Ok(());
    }

    let mut tx = db.begin().await?;
    sqlx::query("SELECT 1 FROM oauth_grants WHERE id = $1 FOR UPDATE").bind(grant_id).execute(&mut *tx).await?;
    match kind {
        TokenKind::Refresh => {
            if revoke_grant_in_tx(&mut tx, grant_id).await? {
                record_audit(
                    &mut *tx,
                    ctx,
                    Some(app.id),
                    Some(grant_id),
                    AuditEvent::GrantRevoked,
                    json!({ "reason": "client_revoked_refresh_token", "refresh_token_id": token_id }),
                )
                .await?;
            }
        }
        TokenKind::Access => {
            sqlx::query("UPDATE oauth_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
                .bind(token_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

// ─── bearer access tokens ─────────────────────────────────────────────────────

/// Who an `alt_oat_` bearer acts for.
#[derive(Debug, Clone)]
pub struct OAuthAccess {
    pub token_id: Uuid,
    pub grant_id: Uuid,
    pub app_id: Uuid,
    pub client_id: String,
    /// The person who consented; their wallet pays (A8).
    pub user_id: String,
    /// The grant's scopes (D3).
    pub scopes: Vec<String>,
    pub monthly_cap_cents: i64,
    pub expires_at: DateTime<Utc>,
}

impl OAuthAccess {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Why a bearer was refused (A9).
#[derive(Debug)]
pub enum AccessTokenError {
    /// 401 `invalid_token`: missing, malformed or unknown.
    Invalid,
    /// 401 `oauth_token_expired`.
    Expired,
    /// 401 `oauth_grant_revoked`: token or grant revoked, or the app suspended.
    Revoked,
    /// 403 `app_not_approved_for_user`: the app is not live and the person is
    /// no longer its owner or a listed test user (A7).
    NotApprovedForUser,
    /// 500 `server_error`.
    Server(ApiError),
}

impl AccessTokenError {
    pub fn code(&self) -> &'static str {
        match self {
            AccessTokenError::Invalid => "invalid_token",
            AccessTokenError::Expired => "oauth_token_expired",
            AccessTokenError::Revoked => "oauth_grant_revoked",
            AccessTokenError::NotApprovedForUser => "app_not_approved_for_user",
            AccessTokenError::Server(_) => "server_error",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            AccessTokenError::NotApprovedForUser => StatusCode::FORBIDDEN,
            AccessTokenError::Server(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::UNAUTHORIZED,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            AccessTokenError::Invalid => "The access token is not valid.",
            AccessTokenError::Expired => "The access token has expired. Use the refresh token or sign in again.",
            AccessTokenError::Revoked => "This sign-in was revoked, or the app is suspended.",
            AccessTokenError::NotApprovedForUser => "This app is not live yet, and this person is not one of its test users.",
            AccessTokenError::Server(_) => "The server could not finish the request. Try again.",
        }
    }
}

impl From<sqlx::Error> for AccessTokenError {
    fn from(e: sqlx::Error) -> Self {
        AccessTokenError::Server(ApiError::from(e))
    }
}

#[derive(sqlx::FromRow)]
struct AccessRow {
    token_id: Uuid,
    grant_id: Uuid,
    app_id: Uuid,
    client_id: String,
    owner_user_id: String,
    app_state: String,
    user_id: String,
    scopes: Vec<String>,
    monthly_cap_cents: i32,
    expires_at: DateTime<Utc>,
    live: bool,
    revoked: bool,
}

/// The one check for an `alt_oat_` bearer (pass the token without `Bearer `).
/// Order: unknown → revoked (token, grant, or suspended app) → expired →
/// not approved for this person (a non-live app whose test-user list no
/// longer has them). Marks the grant used at most once a minute.
pub async fn authenticate_access_token(db: &PgPool, token: &str) -> Result<OAuthAccess, AccessTokenError> {
    if !token.starts_with(ACCESS_TOKEN_PREFIX) {
        return Err(AccessTokenError::Invalid);
    }
    let row: Option<AccessRow> = sqlx::query_as(
        "SELECT t.id AS token_id, t.grant_id, g.app_id, a.client_id, a.owner_user_id, a.state AS app_state,
                g.user_id, g.scopes, g.monthly_cap_cents, t.expires_at,
                t.expires_at > now() AS live,
                (t.revoked_at IS NOT NULL OR g.revoked_at IS NOT NULL) AS revoked
         FROM oauth_tokens t
         JOIN oauth_grants g ON g.id = t.grant_id
         JOIN oauth_apps a ON a.id = g.app_id
         WHERE t.token_hash = $1 AND t.kind = 'access'",
    )
    .bind(hash_token(token))
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Err(AccessTokenError::Invalid) };
    let app_state = AppState::parse(&row.app_state);
    if row.revoked || app_state == AppState::Suspended {
        return Err(AccessTokenError::Revoked);
    }
    if !row.live {
        return Err(AccessTokenError::Expired);
    }
    if app_state != AppState::Live && row.owner_user_id != row.user_id {
        let listed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM oauth_app_test_users
                            WHERE app_id = $1
                              AND (user_id = $2 OR email = (SELECT lower(email) FROM users WHERE id = $2)))",
        )
        .bind(row.app_id)
        .bind(&row.user_id)
        .fetch_one(db)
        .await?;
        if !listed {
            return Err(AccessTokenError::NotApprovedForUser);
        }
    }
    sqlx::query(
        "UPDATE oauth_grants SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
    )
    .bind(row.grant_id)
    .execute(db)
    .await?;
    Ok(OAuthAccess {
        token_id: row.token_id,
        grant_id: row.grant_id,
        app_id: row.app_id,
        client_id: row.client_id,
        user_id: row.user_id,
        scopes: row.scopes,
        monthly_cap_cents: i64::from(row.monthly_cap_cents),
        expires_at: row.expires_at,
    })
}

// ─── usage (A6, A8 monthly window) ────────────────────────────────────────────

/// The `oauth_grant_spend.month` key for `now`: `YYYY-MM` in UTC (A8).
pub fn spend_month(now: DateTime<Utc>) -> String {
    now.format("%Y-%m").to_string()
}

/// 00:00 UTC on the first of the month after `now`.
pub fn month_resets_at(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single().unwrap_or(now)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantUsage {
    pub month: String,
    pub spent_cents: i64,
    pub reserved_cents: i64,
    pub cap_cents: i64,
    pub resets_at: DateTime<Utc>,
}

impl GrantUsage {
    /// A6: `GET /api/v1/oauth/usage`.
    pub fn to_json(&self) -> Value {
        json!({
            "month": self.month,
            "spent_cents": self.spent_cents,
            "reserved_cents": self.reserved_cents,
            "cap_cents": self.cap_cents,
            "resets_at": self.resets_at.to_rfc3339(),
        })
    }
}

/// This month's spend for the grant behind `access`. No row yet = nothing spent.
pub async fn grant_usage(db: &PgPool, access: &OAuthAccess, now: DateTime<Utc>) -> Result<GrantUsage, ApiError> {
    let month = spend_month(now);
    let (spent_cents, reserved_cents): (i64, i64) =
        sqlx::query_as("SELECT spent_cents, reserved_cents FROM oauth_grant_spend WHERE grant_id = $1 AND month = $2")
            .bind(access.grant_id)
            .bind(&month)
            .fetch_optional(db)
            .await?
            .unwrap_or((0, 0));
    Ok(GrantUsage {
        month,
        spent_cents,
        reserved_cents,
        cap_cents: access.monthly_cap_cents,
        resets_at: month_resets_at(now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_matches_rfc7636_appendix_b() {
        assert_eq!(
            s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(!is_valid_code_verifier(&"a".repeat(42)));
        assert!(!is_valid_code_verifier(&"a".repeat(129)));
        assert!(!is_valid_code_verifier(&format!("{}+", "a".repeat(43))));
        assert!(is_valid_code_verifier(&"a-._~".repeat(9)));
    }

    #[test]
    fn month_window_rolls_over_in_december() {
        let dec = Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        assert_eq!(spend_month(dec), "2026-12");
        assert_eq!(month_resets_at(dec), Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap());
        let nov = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(spend_month(nov), "2026-11");
        assert_eq!(month_resets_at(nov), Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0).unwrap());
    }
}
