//! Native iOS push (APNs) for the Allternit iPhone app. Migration `110_apns_device_tokens.sql`.
//!
//! Rides on the same pipeline as Web Push: every sender in [`super::web_push`] (the backbone push sink,
//! rings, missed calls, in-app messages, channel messages, quiet-hours summaries, page repeats) ends in
//! `web_push::send_to_user`, which also calls [`send_to_user`] here. Preferences, quiet hours, collapse and
//! rate limits are therefore applied once, upstream, for both transports.
//!
//! Token-based auth over HTTP/2 (a `.p8` signing key). Env:
//! - `APNS_KEY_P8`   the `.p8` file contents (PEM; literal `\n` escapes are accepted)
//! - `APNS_KEY_ID`   the key's 10-character id
//! - `APNS_TEAM_ID`  the Apple Developer team id
//! - `APNS_TOPIC`    the app bundle id (default `com.allternit.mobile`)
//! - `APNS_ENV`      `production` (default) or `sandbox`: used for a token whose app didn't say which
//!
//! With the key, key id or team id unset, the routes answer 503 `apns_not_configured` and nothing is sent.
//!
//! Routes (Clerk session or `compute`-scoped API key):
//! - `POST   /api/v1/device-tokens` {token, platform:"ios", environment?, deviceId?} → `{ok, id}`
//! - `DELETE /api/v1/device-tokens` {token}                                         → `{ok, removed}`
//!
//! The notification carries `{type, url, tag, data}` next to `aps`; the app opens `url` (the same app path
//! the Web Push service worker opens) when the notification is tapped.

use aws_lc_rs::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use super::web_push::PushMessage;
use crate::ApiState;

/// Most iOS devices one person may register.
const MAX_TOKENS_PER_USER: i64 = 10;
/// Drop a token after this many failures in a row that weren't a clean "gone" answer.
const MAX_FAILURES: i32 = 8;
/// Apple accepts a provider token for an hour and rejects refreshing it more than every 20 minutes.
const JWT_REFRESH_SECS: i64 = 40 * 60;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/device-tokens", post(register_route).delete(unregister_route))
}

// ---------------------------------------------------------------------------
// Config + provider token
// ---------------------------------------------------------------------------

pub struct Apns {
    key: EcdsaKeyPair,
    key_id: String,
    team_id: String,
    topic: String,
    default_env: &'static str,
}

impl Apns {
    /// `None` when the env is unset or the key is unusable (APNs stays off).
    pub fn from_env() -> Option<Self> {
        let pem = std::env::var("APNS_KEY_P8").ok().filter(|v| !v.trim().is_empty())?;
        let key_id = std::env::var("APNS_KEY_ID").ok().filter(|v| !v.trim().is_empty())?;
        let team_id = std::env::var("APNS_TEAM_ID").ok().filter(|v| !v.trim().is_empty())?;
        let topic = std::env::var("APNS_TOPIC").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "com.allternit.mobile".into());
        let env = std::env::var("APNS_ENV").unwrap_or_default();
        match Self::new(&pem, &key_id, &team_id, &topic, &env) {
            Ok(apns) => Some(apns),
            Err(error) => {
                tracing::warn!("apns disabled, key unusable: {error}");
                None
            }
        }
    }

    pub fn new(pem: &str, key_id: &str, team_id: &str, topic: &str, env: &str) -> Result<Self, String> {
        let der = pem_to_der(pem).ok_or("APNS_KEY_P8 is not a PEM private key")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der).map_err(|e| format!("{e}"))?;
        Ok(Self { key, key_id: key_id.trim().into(), team_id: team_id.trim().into(), topic: topic.trim().into(), default_env: normalize_env(env).unwrap_or("production") })
    }

    fn sign_jwt(&self, iat: i64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "ES256", "kid": self.key_id }).to_string());
        let claims = URL_SAFE_NO_PAD.encode(json!({ "iss": self.team_id, "iat": iat }).to_string());
        let input = format!("{header}.{claims}");
        let sig = self.key.sign(&SystemRandom::new(), input.as_bytes()).map_err(|e| format!("{e}"))?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
    }

    /// The cached provider token, re-signed every 40 minutes (or after Apple says it expired).
    fn provider_token(&self) -> Result<String, String> {
        let now = chrono::Utc::now().timestamp();
        let mut cache = JWT_CACHE.lock().map_err(|_| "jwt cache poisoned")?;
        if let Some((kid, iat, jwt)) = cache.as_ref() {
            if kid == &self.key_id && now - iat < JWT_REFRESH_SECS {
                return Ok(jwt.clone());
            }
        }
        let jwt = self.sign_jwt(now)?;
        *cache = Some((self.key_id.clone(), now, jwt.clone()));
        Ok(jwt)
    }
}

static JWT_CACHE: Mutex<Option<(String, i64, String)>> = Mutex::new(None);

fn forget_provider_token() {
    if let Ok(mut cache) = JWT_CACHE.lock() {
        *cache = None;
    }
}

fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let pem = pem.replace("\\n", "\n");
    let body: String = pem.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("-----")).collect();
    STANDARD.decode(body).ok().filter(|d| !d.is_empty())
}

fn normalize_env(env: &str) -> Option<&'static str> {
    match env.trim().to_ascii_lowercase().as_str() {
        "production" | "prod" => Some("production"),
        "sandbox" | "development" | "dev" => Some("sandbox"),
        _ => None,
    }
}

fn host_for(env: &str) -> &'static str {
    if env == "sandbox" {
        "https://api.sandbox.push.apple.com"
    } else {
        "https://api.push.apple.com"
    }
}

/// APNs only speaks HTTP/2; reqwest negotiates it over ALPN (the `http2` feature is on in Cargo.toml).
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

pub fn payload(msg: &PushMessage) -> Value {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    json!({
        "aps": {
            "alert": { "title": clip(&msg.title, 120), "body": clip(&msg.body, 400) },
            "sound": "default",
            // The server keeps no unread count; 1 marks "something new" and the app clears it when opened.
            "badge": 1,
            "thread-id": clip(&msg.tag, 64),
            "mutable-content": 1,
        },
        "type": msg.kind,
        "url": msg.url,
        "tag": msg.tag,
        "data": msg.data,
    })
}

/// `apns-collapse-id` must be at most 64 bytes.
fn collapse_id(tag: &str) -> String {
    let mut out = String::new();
    for c in tag.chars() {
        if out.len() + c.len_utf8() > 64 {
            break;
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct TokenRow {
    id: String,
    token: String,
    environment: String,
}

/// Push `msg` to every iOS device `user` registered. Returns how many APNs accepted. Does nothing, and
/// costs nothing, when APNs isn't configured.
pub async fn send_to_user(db: &PgPool, user: &str, msg: &PushMessage) -> usize {
    let Some(apns) = Apns::from_env() else { return 0 };
    let rows: Vec<TokenRow> = match sqlx::query_as("SELECT id, token, environment FROM apns_device_tokens WHERE user_id = $1").bind(user).fetch_all(db).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!("apns: reading tokens failed: {error}");
            return 0;
        }
    };
    if rows.is_empty() {
        return 0;
    }
    let body = payload(msg).to_string();
    let expiration = chrono::Utc::now().timestamp() + i64::from(msg.ttl_secs);
    let mut delivered = 0;
    for row in rows {
        let jwt = match apns.provider_token() {
            Ok(jwt) => jwt,
            Err(error) => {
                tracing::warn!("apns: signing provider token failed: {error}");
                return delivered;
            }
        };
        let url = format!("{}/3/device/{}", host_for(&row.environment), row.token);
        let req = client()
            .post(url)
            .header("authorization", format!("bearer {jwt}"))
            .header("apns-topic", &apns.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", if msg.high_urgency { "10" } else { "5" })
            .header("apns-expiration", expiration.to_string())
            .header("apns-collapse-id", collapse_id(&msg.tag))
            .header("content-type", "application/json")
            .body(body.clone());
        let outcome = match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let reason = resp.json::<Value>().await.ok().and_then(|v| v.get("reason").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
                Ok((status, reason))
            }
            Err(error) => Err(error.to_string()),
        };
        match outcome {
            Ok((200, _)) => {
                delivered += 1;
                let _ = sqlx::query("UPDATE apns_device_tokens SET last_ok_at = now(), failures = 0, last_error = NULL WHERE id = $1").bind(&row.id).execute(db).await;
            }
            Ok((status, reason)) if status == 410 || matches!(reason.as_str(), "BadDeviceToken" | "Unregistered" | "DeviceTokenNotForTopic") => {
                let _ = sqlx::query("DELETE FROM apns_device_tokens WHERE id = $1").bind(&row.id).execute(db).await;
            }
            other => {
                let error = match other {
                    Ok((status, reason)) => {
                        if status == 403 && reason == "ExpiredProviderToken" {
                            forget_provider_token();
                        }
                        format!("apns answered {status} {reason}")
                    }
                    Err(e) => e,
                };
                tracing::warn!("apns push to {} failed: {error}", row.id);
                let _ = sqlx::query("UPDATE apns_device_tokens SET failures = failures + 1, last_error = $2 WHERE id = $1").bind(&row.id).bind(&error).execute(db).await;
                let _ = sqlx::query("DELETE FROM apns_device_tokens WHERE id = $1 AND failures >= $2").bind(&row.id).bind(MAX_FAILURES).execute(db).await;
            }
        }
    }
    delivered
}

/// True when APNs is configured and `user` has at least one iOS device registered.
pub async fn can_reach(db: &PgPool, user: &str) -> bool {
    if Apns::from_env().is_none() {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM apns_device_tokens WHERE user_id = $1)").bind(user).fetch_one(db).await.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterBody {
    token: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
}

fn token_ok(token: &str) -> bool {
    (32..=200).contains(&token.len()) && token.chars().all(|c| c.is_ascii_hexdigit())
}

async fn register_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<RegisterBody>) -> Response {
    let Some(apns) = Apns::from_env() else { return error(StatusCode::SERVICE_UNAVAILABLE, "apns_not_configured") };
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    if body.platform.as_deref().is_some_and(|p| p != "ios") {
        return error(StatusCode::BAD_REQUEST, "unsupported_platform");
    }
    let token = body.token.trim().to_ascii_lowercase();
    if !token_ok(&token) {
        return error(StatusCode::BAD_REQUEST, "bad_token");
    }
    let env = body.environment.as_deref().and_then(normalize_env).unwrap_or(apns.default_env);
    let device_id: String = body.device_id.unwrap_or_default().trim().chars().take(100).collect();
    match register(&state.db, &user, &token, env, &device_id).await {
        Ok(Some(id)) => (StatusCode::CREATED, Json(json!({ "ok": true, "id": id }))).into_response(),
        Ok(None) => error(StatusCode::TOO_MANY_REQUESTS, "too_many_devices"),
        Err(e) => {
            tracing::error!("apns register: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

/// Register (or move) one device token. A token belongs to one person: registering it while signed in as
/// someone else moves it to them. `None` when the person is at the device cap.
pub async fn register(db: &PgPool, user: &str, token: &str, env: &str, device_id: &str) -> Result<Option<String>, sqlx::Error> {
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM apns_device_tokens WHERE user_id = $1 AND token <> $2").bind(user).bind(token).fetch_one(db).await?;
    if existing >= MAX_TOKENS_PER_USER {
        return Ok(None);
    }
    let mut tx = db.begin().await?;
    if !device_id.is_empty() {
        // A reinstall hands the same device a new token; keep one row per device.
        sqlx::query("DELETE FROM apns_device_tokens WHERE user_id = $1 AND device_id = $2 AND token <> $3").bind(user).bind(device_id).bind(token).execute(&mut *tx).await?;
    }
    let id = format!("apns_{}", uuid::Uuid::new_v4().simple());
    let id: String = sqlx::query_scalar(
        "INSERT INTO apns_device_tokens (id, user_id, token, environment, device_id) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (token) DO UPDATE SET user_id = $2, environment = $4, device_id = $5, failures = 0, last_error = NULL
         RETURNING id",
    )
    .bind(&id)
    .bind(user)
    .bind(token)
    .bind(env)
    .bind(device_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

#[derive(Deserialize)]
struct UnregisterBody {
    token: String,
}

async fn unregister_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<UnregisterBody>) -> Response {
    // Signing out must work even if APNs was switched off on the server since.
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    let token = body.token.trim().to_ascii_lowercase();
    match sqlx::query("DELETE FROM apns_device_tokens WHERE user_id = $1 AND token = $2").bind(&user).bind(&token).execute(&state.db).await {
        Ok(done) => Json(json!({ "ok": true, "removed": done.rows_affected() > 0 })).into_response(),
        Err(e) => {
            tracing::error!("apns unregister: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway P-256 PKCS#8 key generated for this test only.
    fn fixture_pem() -> String {
        let rng = SystemRandom::new();
        let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let b64 = STANDARD.encode(doc.as_ref());
        format!("-----BEGIN PRIVATE KEY-----\\n{b64}\\n-----END PRIVATE KEY-----")
    }

    #[test]
    fn parses_escaped_pem_and_signs_a_three_part_jwt() {
        let apns = Apns::new(&fixture_pem(), "KEY1234567", "TEAM123456", "com.allternit.mobile", "").unwrap();
        assert_eq!(apns.default_env, "production");
        let jwt = apns.sign_jwt(1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["kid"], "KEY1234567");
        let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "TEAM123456");
        assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), 64);
    }

    #[test]
    fn rejects_garbage_key() {
        assert!(Apns::new("not a key", "K", "T", "com.allternit.mobile", "sandbox").is_err());
    }

    #[test]
    fn payload_carries_alert_and_app_url() {
        let msg = PushMessage { kind: "message", title: "Raven".into(), body: "Done".into(), url: "/b/bot_1".into(), tag: "thread-1".into(), data: json!({"event": "message.received"}), ttl_secs: 60, high_urgency: false };
        let p = payload(&msg);
        assert_eq!(p["aps"]["alert"]["title"], "Raven");
        assert_eq!(p["aps"]["thread-id"], "thread-1");
        assert_eq!(p["url"], "/b/bot_1");
        assert_eq!(p["type"], "message");
    }

    #[test]
    fn collapse_id_stays_under_64_bytes_on_a_char_boundary() {
        let id = collapse_id(&"é".repeat(40));
        assert!(id.len() <= 64);
        assert_eq!(id.chars().count(), 32);
    }
}
