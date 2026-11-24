//! Native Android push (FCM HTTP v1) for the Allternit Android app. Migration `152_device_token_platform.sql`.
//!
//! The Android twin of [`super::apns`]. Tokens are registered through the same route
//! (`POST /api/v1/device-tokens` with `platform: "android"`, handled in `apns.rs`) and stored in the same
//! `apns_device_tokens` table with `platform = 'android'`. Every sender that ends in [`super::apns::send_to_user`]
//! also calls [`send_to_user`] here, so preferences, quiet hours, collapse and rate limits apply once, upstream.
//!
//! Env:
//! - `FCM_SERVICE_ACCOUNT_JSON`  the Firebase service account key (project `allternit-6b8e1`): the JSON itself,
//!   or a path to the JSON file. Literal `\n` escapes in `private_key` are accepted.
//!
//! With it unset, Android sends are skipped (one warning per process) and registration answers 503
//! `fcm_not_configured`. Nothing crashes and nothing is sent.
//!
//! Auth: an OAuth2 access token for the `firebase.messaging` scope, from a JWT (RS256) signed with the service
//! account's key (the JWT bearer grant). The token is cached until five minutes before it expires.
//!
//! The push is a data-only message, so the app builds the notification itself (`AllternitMessagingService` in
//! `surfaces/allternit-mobile/android/feature/push`): channel from `category`, the iOS category buttons, and the
//! tap opens `url` / `inboxId`. Data keys (all strings): `type`, `title`, `body`, `url`, `tag`, `event`,
//! `category`, `level`, `botId`, `inboxId`, `sessionId`, `runtimeId`, `contentHash`, `private`, and `data`
//! (the sender's whole `data` object as JSON). A revoke is `{event: "notification.revoke", tags: "a,b"}`.

use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use super::web_push::PushMessage;

/// The env var holding the service account key (JSON or a path to it).
pub const ENV_SERVICE_ACCOUNT: &str = "FCM_SERVICE_ACCOUNT_JSON";
/// Drop a token after this many failures in a row that weren't a clean "gone" answer (as APNs).
const MAX_FAILURES: i32 = 8;
/// Refresh the cached access token this long before it expires.
const TOKEN_MARGIN_SECS: i64 = 5 * 60;
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// The `data` keys copied from the sender's `data` object when they are strings.
const DATA_KEYS: [&str; 8] = ["event", "category", "level", "botId", "inboxId", "sessionId", "runtimeId", "contentHash"];

// ---------------------------------------------------------------------------
// Config + access token
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
    #[serde(default)]
    token_uri: Option<String>,
}

pub struct Fcm {
    project_id: String,
    client_email: String,
    key: jsonwebtoken::EncodingKey,
    token_uri: String,
}

static MISSING_WARNED: AtomicBool = AtomicBool::new(false);
static TOKEN_CACHE: Mutex<Option<(String, i64, String)>> = Mutex::new(None);

impl Fcm {
    /// `None` when the env is unset or the key is unusable (Android push stays off). Silent: the senders log.
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var(ENV_SERVICE_ACCOUNT).ok().filter(|v| !v.trim().is_empty())?;
        let json = if raw.trim_start().starts_with('{') {
            raw
        } else {
            match std::fs::read_to_string(raw.trim()) {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!("fcm disabled, {ENV_SERVICE_ACCOUNT} is not JSON and not a readable file: {error}");
                    return None;
                }
            }
        };
        match Self::from_json(&json) {
            Ok(fcm) => Some(fcm),
            Err(error) => {
                tracing::warn!("fcm disabled, service account unusable: {error}");
                None
            }
        }
    }

    /// [`Self::from_env`] for a sender: logs once per process that Android sends are skipped when it is unset.
    fn for_send() -> Option<Self> {
        let fcm = Self::from_env();
        if fcm.is_none() && !MISSING_WARNED.swap(true, Ordering::Relaxed) {
            tracing::warn!("fcm: {ENV_SERVICE_ACCOUNT} is unset or unusable, Android pushes are skipped");
        }
        fcm
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let account: ServiceAccount = serde_json::from_str(json).map_err(|e| format!("not a service account: {e}"))?;
        let pem = account.private_key.replace("\\n", "\n");
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).map_err(|e| format!("private_key: {e}"))?;
        if account.project_id.trim().is_empty() || account.client_email.trim().is_empty() {
            return Err("project_id and client_email are required".into());
        }
        Ok(Self {
            project_id: account.project_id.trim().into(),
            client_email: account.client_email.trim().into(),
            key,
            token_uri: account.token_uri.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_TOKEN_URI.into()),
        })
    }

    fn sign_assertion(&self, iat: i64) -> Result<String, String> {
        let claims = json!({ "iss": self.client_email, "scope": SCOPE, "aud": self.token_uri, "iat": iat, "exp": iat + 3600 });
        jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256), &claims, &self.key).map_err(|e| format!("{e}"))
    }

    /// The cached OAuth2 access token, fetched again five minutes before it expires (or after a 401).
    async fn access_token(&self) -> Result<String, String> {
        let now = chrono::Utc::now().timestamp();
        let cached = TOKEN_CACHE
            .lock()
            .ok()
            .and_then(|cache| cache.as_ref().filter(|(email, exp, _)| email == &self.client_email && exp - TOKEN_MARGIN_SECS > now).map(|(_, _, token)| token.clone()));
        if let Some(token) = cached {
            return Ok(token);
        }
        let assertion = self.sign_assertion(now)?;
        let body = format!("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}");
        let resp = client()
            .post(&self.token_uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        let status = resp.status().as_u16();
        let answer: Value = resp.json().await.unwrap_or(Value::Null);
        let Some(token) = answer.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()) else {
            return Err(format!("token endpoint answered {status} {}", answer.get("error").and_then(Value::as_str).unwrap_or("")));
        };
        let expires = now + answer.get("expires_in").and_then(Value::as_i64).unwrap_or(3600);
        if let Ok(mut cache) = TOKEN_CACHE.lock() {
            *cache = Some((self.client_email.clone(), expires, token.to_string()));
        }
        Ok(token.to_string())
    }

    fn send_url(&self) -> String {
        format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", self.project_id)
    }
}

fn forget_access_token() {
    if let Ok(mut cache) = TOKEN_CACHE.lock() {
        *cache = None;
    }
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default())
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// FCM `collapse_key`: same rule as `apns-collapse-id` (at most 64 bytes, on a char boundary).
fn collapse_key(tag: &str) -> String {
    let mut out = String::new();
    for c in tag.chars() {
        if out.len() + c.len_utf8() > 64 {
            break;
        }
        out.push(c);
    }
    out
}

/// The FCM v1 request body for `msg` to one Android `token`. Data-only (the app builds the notification), every
/// value a string (FCM rejects anything else). Same copy, link and keys as the APNs [`super::apns::payload`].
pub fn message(msg: &PushMessage, token: &str) -> Value {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let d = &msg.data;
    let mut data = Map::new();
    data.insert("type".into(), json!(msg.kind));
    data.insert("title".into(), json!(clip(&msg.title, 120)));
    data.insert("body".into(), json!(clip(&msg.body, 400)));
    data.insert("url".into(), json!(msg.url));
    data.insert("tag".into(), json!(msg.tag));
    for key in DATA_KEYS {
        if let Some(value) = d.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
            data.insert(key.into(), json!(value));
        }
    }
    if let Some(private) = d.get("private").and_then(Value::as_bool) {
        data.insert("private".into(), json!(private.to_string()));
    }
    data.insert("data".into(), json!(d.to_string()));
    json!({
        "message": {
            "token": token,
            "data": data,
            "android": {
                "priority": if msg.high_urgency || matches!(d.get("level").and_then(Value::as_str), Some("needs_you" | "urgent")) { "HIGH" } else { "NORMAL" },
                "ttl": format!("{}s", msg.ttl_secs),
                "collapse_key": collapse_key(&msg.tag),
            },
        },
    })
}

/// The data message that removes delivered notifications `tags` from an Android phone (APNs [`super::apns::revoke_payload`]).
pub fn revoke_message(tags: &[String], token: &str) -> Value {
    json!({
        "message": {
            "token": token,
            "data": { "event": "notification.revoke", "tags": tags.join(",") },
            "android": { "priority": "NORMAL" },
        },
    })
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct AndroidRow {
    id: String,
    token: String,
}

async fn android_targets(db: &PgPool, user: &str) -> Result<Vec<AndroidRow>, sqlx::Error> {
    sqlx::query_as("SELECT id, token FROM apns_device_tokens WHERE user_id = $1 AND platform = 'android'").bind(user).fetch_all(db).await
}

/// One FCM send. Returns true when FCM accepted it. A gone token (404 / `UNREGISTERED` / `SENDER_ID_MISMATCH`) is
/// deleted; other failures count up and drop the row after [`MAX_FAILURES`] (as APNs `deliver`).
async fn deliver(db: &PgPool, fcm: &Fcm, row_id: &str, body: &Value) -> bool {
    let token = match fcm.access_token().await {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!("fcm: getting an access token failed: {error}");
            return false;
        }
    };
    let outcome = match client().post(fcm.send_url()).bearer_auth(token).json(body).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let answer: Value = resp.json().await.unwrap_or(Value::Null);
            let code = answer["error"]["details"]
                .as_array()
                .and_then(|details| details.iter().find_map(|d| d.get("errorCode").and_then(Value::as_str)))
                .or_else(|| answer["error"]["status"].as_str())
                .unwrap_or("")
                .to_string();
            Ok((status, code))
        }
        Err(error) => Err(error.without_url().to_string()),
    };
    match outcome {
        Ok((200, _)) => {
            let _ = sqlx::query("UPDATE apns_device_tokens SET last_ok_at = now(), failures = 0, last_error = NULL WHERE id = $1").bind(row_id).execute(db).await;
            true
        }
        Ok((status, code)) if status == 404 || matches!(code.as_str(), "UNREGISTERED" | "SENDER_ID_MISMATCH") => {
            let _ = sqlx::query("DELETE FROM apns_device_tokens WHERE id = $1").bind(row_id).execute(db).await;
            false
        }
        other => {
            let error = match other {
                Ok((status, code)) => {
                    if status == 401 {
                        forget_access_token();
                    }
                    format!("fcm answered {status} {code}")
                }
                Err(e) => e,
            };
            tracing::warn!("fcm push to {row_id} failed: {error}");
            let _ = sqlx::query("UPDATE apns_device_tokens SET failures = failures + 1, last_error = $2 WHERE id = $1").bind(row_id).bind(&error).execute(db).await;
            let _ = sqlx::query("DELETE FROM apns_device_tokens WHERE id = $1 AND failures >= $2").bind(row_id).bind(MAX_FAILURES).execute(db).await;
            false
        }
    }
}

/// Push `msg` to every Android device `user` registered. Returns how many FCM accepted. Records the inbox tag in
/// `notification_delivered` like APNs, so [`revoke`] can take it back. Does nothing when FCM isn't configured.
pub async fn send_to_user(db: &PgPool, user: &str, msg: &PushMessage) -> usize {
    let Some(fcm) = Fcm::for_send() else { return 0 };
    let targets = match android_targets(db, user).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!("fcm: reading tokens failed: {error}");
            return 0;
        }
    };
    let mut delivered = 0;
    for target in &targets {
        let body = message(msg, &target.token);
        // Never the text or the token: the keys that decide how the phone shows it.
        tracing::info!(
            "fcm alert to {}: event={} category={} priority={}",
            target.id,
            body["message"]["data"]["event"].as_str().unwrap_or(msg.kind),
            body["message"]["data"]["category"].as_str().unwrap_or("-"),
            body["message"]["android"]["priority"].as_str().unwrap_or("NORMAL"),
        );
        if deliver(db, &fcm, &target.id, &body).await {
            delivered += 1;
        }
    }
    if delivered > 0 {
        if let Some(inbox) = msg.data.get("inboxId").and_then(Value::as_str).filter(|v| !v.is_empty()) {
            super::apns::record_delivered(db, user, inbox, &msg.tag, msg.data.get("eventId").and_then(Value::as_str)).await;
        }
    }
    delivered
}

/// Send [`revoke_message`] to every Android device of `user`. Returns how many FCM accepted.
pub async fn revoke(db: &PgPool, user: &str, tags: &[String]) -> usize {
    if tags.is_empty() {
        return 0;
    }
    let Some(fcm) = Fcm::for_send() else { return 0 };
    let targets = match android_targets(db, user).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!("fcm: reading tokens for a revoke failed: {error}");
            return 0;
        }
    };
    let mut accepted = 0;
    for target in &targets {
        if deliver(db, &fcm, &target.id, &revoke_message(tags, &target.token)).await {
            accepted += 1;
        }
    }
    accepted
}

/// True when FCM is configured and `user` has at least one Android device registered.
pub async fn can_reach(db: &PgPool, user: &str) -> bool {
    if std::env::var(ENV_SERVICE_ACCOUNT).map(|v| v.trim().is_empty()).unwrap_or(true) {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM apns_device_tokens WHERE user_id = $1 AND platform = 'android')").bind(user).fetch_one(db).await.unwrap_or(false)
}

/// An FCM registration token: 32-4096 characters of `[A-Za-z0-9_:-]` (case matters, never lowercased).
pub fn token_ok(token: &str) -> bool {
    (32..=4096).contains(&token.len()) && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_is_data_only_with_string_values() {
        let msg = crate::routes::notifications::message_for(
            "approval.requested",
            &json!({ "botId": "b1", "botName": "Raven", "approvalId": "ap1", "summary": "send the invoice to Acme", "inboxId": "approval:ap1", "inboxKind": "held_send", "approveInline": true, "contentHash": "ab12", "sessionId": "s1", "runtimeId": "rt_1" }),
        )
        .unwrap();
        let body = message(&msg, "fcm-token");
        let m = &body["message"];
        assert_eq!(m["token"], "fcm-token");
        assert!(m.get("notification").is_none(), "data-only so the app adds the buttons: {body}");
        let data = m["data"].as_object().unwrap();
        assert!(data.values().all(Value::is_string), "FCM takes string values only: {body}");
        assert_eq!(data["category"], "APPROVAL_INLINE");
        assert_eq!(data["inboxId"], "approval:ap1");
        assert_eq!(data["url"], "/?allternit_inbox=approval%3Aap1");
        assert_eq!(m["android"]["priority"], "HIGH", "approvals are needs-you");
        let inner: Value = serde_json::from_str(data["data"].as_str().unwrap()).unwrap();
        assert_eq!(inner["botId"], "b1");
    }

    #[test]
    fn token_rules() {
        assert!(token_ok(&format!("dGhpc0lz:APA91b{}", "x_-".repeat(40))));
        assert!(!token_ok("short"));
        assert!(!token_ok(&format!("{} space", "a".repeat(40))));
    }

    #[test]
    fn rejects_garbage_service_account() {
        assert!(Fcm::from_json("{}").is_err());
        assert!(Fcm::from_json(r#"{"project_id":"p","client_email":"e","private_key":"nope"}"#).is_err());
    }
}
