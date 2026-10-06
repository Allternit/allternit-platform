//! Web Push: ring and notify a person whose app is closed. Migration `039_push_subscriptions.sql`.
//!
//! Needs a VAPID keypair in env: `ALLTERNIT_VAPID_PUBLIC_KEY` (65-byte uncompressed P-256 point) and
//! `ALLTERNIT_VAPID_PRIVATE_KEY` (32-byte scalar), both base64url without padding, which is what
//! `npx web-push generate-vapid-keys` prints. `ALLTERNIT_VAPID_SUBJECT` (default
//! `mailto:support@allternit.com`) is the contact the push services see. With the keys unset every route
//! here answers 503 `push_not_configured` and nothing is ever sent.
//!
//! Routes (Clerk session or `compute`-scoped API key, except the public key):
//! - `GET    /api/v1/push/vapid-key`                                  → `{publicKey}` (public)
//! - `POST   /api/v1/push/subscriptions` {deviceId, subscription:{endpoint, keys:{p256dh, auth}}} → `{ok, id}`
//! - `DELETE /api/v1/push/subscriptions` {deviceId}                   → `{ok, removed}`
//!
//! Senders used by the rest of cloud-api: the backbone push sink (`routes::notifications`, every user event
//! on the event backbone, per the owner's notification preferences), and the direct senders that stay direct
//! because the backbone carries no event for them: [`spawn_notify`] for an in-app call ring, and
//! [`spawn_notify_pref`] for an in-app missed call (`call.ended` preference) and in-app message
//! (`message.received` preference). [`notify_channel_message`] (`channel_inbound`) pushes a relayed channel
//! message only for a runtime that doesn't forward its events yet; otherwise the sink sends it from the
//! forwarded `message.received`, so it is never pushed twice. Payloads are encrypted for the browser (RFC 8291, `aes128gcm`) and signed with VAPID
//! (RFC 8292). A push service answering 404/410 means the subscription is gone and its row is deleted.
//!
//! Payload the service worker receives (JSON): `{type: "call"|"missed_call"|"message"|"channel_message"|"event",
//! title, body, url, tag, data}` (`event` = a backbone event; `data.event` names it). `url` is a path on the app's own origin that opens the call or thread.

use aws_lc_rs::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_128_GCM},
    agreement::{self, PrivateKey, UnparsedPublicKey, ECDH_P256},
    hkdf::{Salt, HKDF_SHA256},
    rand::{SecureRandom, SystemRandom},
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use crate::ApiState;

/// Most subscriptions (devices) one person may register.
const MAX_SUBSCRIPTIONS_PER_USER: i64 = 10;
/// A message-type push to one person for one thread is dropped when another went out this recently.
const COLLAPSE_WINDOW_SECS: i64 = 20;
/// Message-type pushes one person receives per hour, across threads.
const MAX_MESSAGE_PUSHES_PER_HOUR: i64 = 40;
/// RFC 8291 record size we advertise; the whole payload is one record.
const RECORD_SIZE: u32 = 4096;
/// Plaintext cap so the single record plus padding delimiter and tag stays under `RECORD_SIZE`.
const MAX_PLAINTEXT: usize = 3000;
/// Drop a subscription after this many failures in a row that weren't a clean 404/410.
const MAX_FAILURES: i32 = 8;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/push/vapid-key", get(vapid_key_route))
        .route("/api/v1/push/subscriptions", post(subscribe_route).delete(unsubscribe_route))
}

// ---------------------------------------------------------------------------
// VAPID
// ---------------------------------------------------------------------------

pub struct Vapid {
    key: EcdsaKeyPair,
    public_b64: String,
    subject: String,
}

impl Vapid {
    /// `None` when the keys are unset or unusable (push stays off).
    pub fn from_env() -> Option<Self> {
        let public = std::env::var("ALLTERNIT_VAPID_PUBLIC_KEY").ok()?;
        let private = std::env::var("ALLTERNIT_VAPID_PRIVATE_KEY").ok()?;
        let subject = std::env::var("ALLTERNIT_VAPID_SUBJECT").unwrap_or_else(|_| "mailto:support@allternit.com".into());
        match Self::new(&public, &private, &subject) {
            Ok(vapid) => Some(vapid),
            Err(error) => {
                tracing::warn!("web push disabled, VAPID keys unusable: {error}");
                None
            }
        }
    }

    pub fn new(public_b64: &str, private_b64: &str, subject: &str) -> Result<Self, String> {
        let public = b64_decode(public_b64).ok_or("public key is not base64url")?;
        let private = b64_decode(private_b64).ok_or("private key is not base64url")?;
        let key = EcdsaKeyPair::from_private_key_and_public_key(&ECDSA_P256_SHA256_FIXED_SIGNING, &private, &public).map_err(|e| format!("{e}"))?;
        Ok(Self { key, public_b64: URL_SAFE_NO_PAD.encode(&public), subject: subject.to_string() })
    }

    pub fn public_key(&self) -> &str {
        &self.public_b64
    }

    /// `Authorization` header value for a push endpoint: `vapid t=<jwt>, k=<public key>`.
    pub fn authorization(&self, endpoint: &str) -> Result<String, String> {
        let audience = origin_of(endpoint).ok_or("endpoint has no origin")?;
        let exp = chrono::Utc::now().timestamp() + 12 * 3600;
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(json!({ "aud": audience, "exp": exp, "sub": self.subject }).to_string());
        let signing_input = format!("{header}.{claims}");
        let signature = self.key.sign(&SystemRandom::new(), signing_input.as_bytes()).map_err(|e| format!("{e}"))?;
        let jwt = format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        Ok(format!("vapid t={jwt}, k={}", self.public_b64))
    }

    #[cfg(test)]
    fn public_point(&self) -> Vec<u8> {
        use aws_lc_rs::signature::KeyPair;
        self.key.public_key().as_ref().to_vec()
    }
}

fn b64_decode(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value.trim().trim_end_matches('=')).ok()
}

/// `https://host[:port]` of a URL; push services require this as the JWT audience.
fn origin_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("https://{host}"))
}

/// Subscriptions must point at a public https host: no literal IPs, `localhost`, or single-label hosts.
fn endpoint_allowed(endpoint: &str) -> bool {
    let Some(origin) = origin_of(endpoint) else { return false };
    let host = origin.trim_start_matches("https://");
    if host.contains('@') || endpoint.len() > 2048 {
        return false;
    }
    let host = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let is_ip = host.chars().all(|c| c.is_ascii_digit() || c == '.') || host.contains('[');
    !is_ip && host.contains('.') && host != "localhost" && !host.ends_with(".local") && !host.ends_with(".internal")
}

// ---------------------------------------------------------------------------
// Payload encryption (RFC 8291, aes128gcm)
// ---------------------------------------------------------------------------

struct Len(usize);
impl aws_lc_rs::hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf(salt: &[u8], ikm: &[u8], info: &[&[u8]], out: &mut [u8]) -> Result<(), String> {
    let prk = Salt::new(HKDF_SHA256, salt).extract(ikm);
    prk.expand(info, Len(out.len())).map_err(|_| "hkdf expand")?.fill(out).map_err(|_| "hkdf fill".to_string())
}

/// Encrypt `plaintext` for a browser subscription. `server_key` and `salt` are random in production and
/// fixed only by the RFC 8291 test vector.
fn encrypt_with(plaintext: &[u8], ua_public: &[u8], auth_secret: &[u8], server_key: &PrivateKey, salt: &[u8; 16]) -> Result<Vec<u8>, String> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err("payload too large".into());
    }
    let as_public = server_key.compute_public_key().map_err(|_| "public key")?;
    let as_public = as_public.as_ref().to_vec();
    let shared = agreement::agree(server_key, UnparsedPublicKey::new(&ECDH_P256, ua_public), "bad subscription key".to_string(), |secret| Ok(secret.to_vec()))?;

    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(&as_public);
    let mut ikm = [0u8; 32];
    hkdf(auth_secret, &shared, &[&key_info], &mut ikm)?;
    let mut cek = [0u8; 16];
    hkdf(salt, &ikm, &[b"Content-Encoding: aes128gcm\0"], &mut cek)?;
    let mut nonce = [0u8; 12];
    hkdf(salt, &ikm, &[b"Content-Encoding: nonce\0"], &mut nonce)?;

    let mut record = plaintext.to_vec();
    record.push(0x02); // last (only) record
    let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).map_err(|_| "cek")?);
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut record).map_err(|_| "seal")?;

    let mut body = Vec::with_capacity(21 + as_public.len() + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

pub fn encrypt_payload(plaintext: &[u8], ua_public_b64: &str, auth_b64: &str) -> Result<Vec<u8>, String> {
    let ua_public = b64_decode(ua_public_b64).filter(|k| k.len() == 65).ok_or("bad p256dh")?;
    let auth = b64_decode(auth_b64).filter(|a| a.len() == 16).ok_or("bad auth")?;
    let server_key = PrivateKey::generate(&ECDH_P256).map_err(|_| "keygen")?;
    let mut salt = [0u8; 16];
    SystemRandom::new().fill(&mut salt).map_err(|_| "rng")?;
    encrypt_with(plaintext, &ua_public, &auth, &server_key, &salt)
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait PushTransport: Send + Sync {
    /// POST `body` to the push service; the HTTP status, or an error string when it was unreachable.
    async fn post(&self, endpoint: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String>;
}

pub struct HttpTransport {
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new() -> Self {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).redirect(reqwest::redirect::Policy::none()).build().unwrap_or_default();
        Self { client }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PushTransport for HttpTransport {
    async fn post(&self, endpoint: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String> {
        let mut req = self.client.post(endpoint).body(body);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        req.send().await.map(|r| r.status().as_u16()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
pub(crate) static TEST_TRANSPORT: std::sync::Mutex<Option<Arc<dyn PushTransport>>> = std::sync::Mutex::new(None);

fn default_transport() -> Arc<dyn PushTransport> {
    #[cfg(test)]
    if let Some(t) = TEST_TRANSPORT.lock().unwrap().clone() {
        return t;
    }
    Arc::new(HttpTransport::new())
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PushMessage {
    /// `call`, `missed_call`, `message`, `channel_message` or `event` (a backbone event from the sink).
    pub kind: &'static str,
    pub title: String,
    pub body: String,
    /// Path on the app's origin the notification opens.
    pub url: String,
    /// Notifications with the same tag replace each other (also sent as the push `Topic`).
    pub tag: String,
    pub data: Value,
    pub ttl_secs: u32,
    /// `Urgency` header: `high` for calls, `normal` otherwise.
    pub high_urgency: bool,
}

impl PushMessage {
    fn json(&self) -> Vec<u8> {
        let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
        json!({ "type": self.kind, "title": clip(&self.title, 120), "body": clip(&self.body, 400), "url": self.url, "tag": self.tag, "data": self.data }).to_string().into_bytes()
    }
}

/// `Topic` must be at most 32 URL-safe base64 characters.
fn topic_for(tag: &str) -> String {
    hex::encode(Sha256::digest(tag.as_bytes()))[..32].to_string()
}

#[derive(sqlx::FromRow)]
struct Sub {
    id: String,
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// Push `msg` to every device `user` has registered. Returns how many pushes the services accepted.
pub async fn send_to_user(db: &PgPool, transport: &dyn PushTransport, vapid: &Vapid, user: &str, msg: &PushMessage) -> usize {
    let subs: Vec<Sub> = match sqlx::query_as("SELECT id, endpoint, p256dh, auth FROM push_subscriptions WHERE user_id = $1").bind(user).fetch_all(db).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!("push: reading subscriptions failed: {error}");
            return 0;
        }
    };
    let payload = msg.json();
    let mut delivered = 0;
    for sub in subs {
        let outcome = async {
            let body = encrypt_payload(&payload, &sub.p256dh, &sub.auth)?;
            let headers = vec![
                ("Authorization".to_string(), vapid.authorization(&sub.endpoint)?),
                ("Content-Encoding".into(), "aes128gcm".into()),
                ("Content-Type".into(), "application/octet-stream".into()),
                ("TTL".into(), msg.ttl_secs.to_string()),
                ("Urgency".into(), if msg.high_urgency { "high" } else { "normal" }.into()),
                ("Topic".into(), topic_for(&msg.tag)),
            ];
            transport.post(&sub.endpoint, headers, body).await
        }
        .await;
        match outcome {
            Ok(200..=299) => {
                delivered += 1;
                let _ = sqlx::query("UPDATE push_subscriptions SET last_ok_at = now(), failures = 0, last_error = NULL WHERE id = $1").bind(&sub.id).execute(db).await;
            }
            Ok(404 | 410) => {
                let _ = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1").bind(&sub.id).execute(db).await;
            }
            other => {
                let error = match other {
                    Ok(status) => format!("push service answered {status}"),
                    Err(e) => e,
                };
                tracing::warn!("push to {} failed: {error}", sub.id);
                let _ = sqlx::query("UPDATE push_subscriptions SET failures = failures + 1, last_error = $2 WHERE id = $1").bind(&sub.id).bind(error).execute(db).await;
                let _ = sqlx::query("DELETE FROM push_subscriptions WHERE id = $1 AND failures >= $2").bind(&sub.id).bind(MAX_FAILURES).execute(db).await;
            }
        }
    }
    delivered
}

/// Whether a message-type push for `collapse_key` may go out now, recording it when it may. One per
/// thread per 20 seconds, and 40 an hour per person.
pub async fn allow_message_push(db: &PgPool, user: &str, collapse_key: &str) -> bool {
    let run = async {
        let recent: i64 = sqlx::query_scalar("SELECT count(*) FROM push_log WHERE user_id = $1 AND collapse_key = $2 AND created_at > now() - make_interval(secs => $3)")
            .bind(user)
            .bind(collapse_key)
            .bind(COLLAPSE_WINDOW_SECS as f64)
            .fetch_one(db)
            .await?;
        if recent > 0 {
            return Ok::<bool, sqlx::Error>(false);
        }
        let hourly: i64 = sqlx::query_scalar("SELECT count(*) FROM push_log WHERE user_id = $1 AND created_at > now() - interval '1 hour'").bind(user).fetch_one(db).await?;
        if hourly >= MAX_MESSAGE_PUSHES_PER_HOUR {
            return Ok(false);
        }
        sqlx::query("INSERT INTO push_log (user_id, collapse_key) VALUES ($1, $2)").bind(user).bind(collapse_key).execute(db).await?;
        sqlx::query("DELETE FROM push_log WHERE created_at < now() - interval '1 day'").execute(db).await?;
        Ok(true)
    };
    run.await.unwrap_or(false)
}

/// True when push is configured and `user` has at least one device registered, so a ring can wake it.
pub async fn can_reach(db: &PgPool, user: &str) -> bool {
    if Vapid::from_env().is_none() {
        return false;
    }
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM push_subscriptions WHERE user_id = $1)").bind(user).fetch_one(db).await.unwrap_or(false)
}

/// Send `msg` to `user` in the background. Does nothing, and costs nothing, when push isn't configured.
/// `rate_limited` applies the per-thread collapse and hourly cap (messages; calls skip it).
pub fn spawn_notify(db: PgPool, user: String, msg: PushMessage, rate_limited: bool) {
    let Some(vapid) = Vapid::from_env() else { return };
    tokio::spawn(async move {
        if rate_limited && !allow_message_push(&db, &user, &msg.tag).await {
            return;
        }
        let transport = default_transport();
        send_to_user(&db, transport.as_ref(), &vapid, &user, &msg).await;
    });
}

/// Like [`spawn_notify`], but only when `user` wants pushes for the registry event type `event`
/// (`routes::notifications` preferences).
pub fn spawn_notify_pref(db: PgPool, user: String, event: &'static str, msg: PushMessage, rate_limited: bool) {
    let Some(vapid) = Vapid::from_env() else { return };
    tokio::spawn(async move {
        if !super::notifications::enabled(&db, &user, event).await {
            return;
        }
        if rate_limited && !allow_message_push(&db, &user, &msg.tag).await {
            return;
        }
        let transport = default_transport();
        send_to_user(&db, transport.as_ref(), &vapid, &user, &msg).await;
    });
}

/// Name the app shows for a channel provider.
pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "slack" => "Slack",
        "telegram" => "Telegram",
        "whatsapp" => "WhatsApp",
        "teams" => "Teams",
        "discord" | "discord_app" => "Discord",
        "sms" => "text",
        "email" => "email",
        _ => "channel",
    }
}

/// Whether the direct push for a relayed channel message may go out, recording it when it may: only for a
/// runtime that doesn't forward its events to the backbone (otherwise the push sink sends it from the
/// forwarded `message.received`), only when `user` wants message pushes, and one per address per 20 s.
pub async fn channel_push_allowed(db: &PgPool, user: &str, route_id: &str, runtime_id: &str) -> bool {
    if super::notifications::runtime_forwards_events(db, runtime_id).await {
        return false;
    }
    if !super::notifications::enabled(db, user, "message.received").await {
        return false;
    }
    allow_message_push(db, user, &format!("channel-{route_id}")).await
}

/// A message reached the user's runtime through a channel's relay address: tell their devices, unless the
/// runtime forwards its events (then the backbone push sink does, once, with the sender and text). The
/// message text isn't read here (the cloud only relays it), so this push just says where it came from.
pub fn notify_channel_message(db: &PgPool, user: &str, route_id: &str, provider: &str, runtime_id: &str) {
    let Some(vapid) = Vapid::from_env() else { return };
    let label = provider_label(provider);
    let msg = PushMessage {
        kind: "channel_message",
        title: "New message".to_string(),
        body: format!("You have a new {label} message for your bot"),
        url: format!("/?allternit_channel={provider}"),
        tag: format!("channel-{route_id}"),
        data: json!({ "provider": provider }),
        ttl_secs: 24 * 3600,
        high_urgency: false,
    };
    let (db, user, route_id, runtime_id) = (db.clone(), user.to_string(), route_id.to_string(), runtime_id.to_string());
    tokio::spawn(async move {
        if !channel_push_allowed(&db, &user, &route_id, &runtime_id).await {
            return;
        }
        let transport = default_transport();
        send_to_user(&db, transport.as_ref(), &vapid, &user, &msg).await;
    });
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "push_not_configured" }))).into_response()
}

fn bad_request(code: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
}

async fn vapid_key_route() -> Response {
    match Vapid::from_env() {
        Some(v) => Json(json!({ "publicKey": v.public_key() })).into_response(),
        None => not_configured(),
    }
}

#[derive(Deserialize)]
struct Keys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
struct Subscription {
    endpoint: String,
    keys: Keys,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeBody {
    device_id: String,
    subscription: Subscription,
}

async fn subscribe_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SubscribeBody>) -> Response {
    if Vapid::from_env().is_none() {
        return not_configured();
    }
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match subscribe(&state.db, &user, &body.device_id, &body.subscription.endpoint, &body.subscription.keys.p256dh, &body.subscription.keys.auth, header_str(&headers, "user-agent")).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "ok": true, "id": id }))).into_response(),
        Err(SubscribeError::Bad(code)) => bad_request(code),
        Err(SubscribeError::TooMany) => (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too_many_devices" }))).into_response(),
        Err(SubscribeError::Db(error)) => {
            tracing::error!("push subscribe: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
        }
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
}

#[derive(Debug)]
pub enum SubscribeError {
    Bad(&'static str),
    TooMany,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for SubscribeError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Register (or replace) the push endpoint of one device. A device keeps one row; an endpoint belongs to
/// one person, so re-subscribing on a shared browser moves it to whoever is signed in now.
pub async fn subscribe(db: &PgPool, user: &str, device_id: &str, endpoint: &str, p256dh: &str, auth: &str, user_agent: &str) -> Result<String, SubscribeError> {
    let device_id = device_id.trim();
    if device_id.is_empty() || device_id.len() > 100 {
        return Err(SubscribeError::Bad("bad_device_id"));
    }
    if !endpoint_allowed(endpoint) {
        return Err(SubscribeError::Bad("bad_endpoint"));
    }
    if b64_decode(p256dh).is_none_or(|k| k.len() != 65) || b64_decode(auth).is_none_or(|a| a.len() != 16) {
        return Err(SubscribeError::Bad("bad_keys"));
    }
    let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions WHERE user_id = $1 AND device_id <> $2").bind(user).bind(device_id).fetch_one(db).await?;
    if existing >= MAX_SUBSCRIPTIONS_PER_USER {
        return Err(SubscribeError::TooMany);
    }
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM push_subscriptions WHERE endpoint = $1 AND NOT (user_id = $2 AND device_id = $3)").bind(endpoint).bind(user).bind(device_id).execute(&mut *tx).await?;
    let id = format!("ps_{}", uuid::Uuid::new_v4().simple());
    let id: String = sqlx::query_scalar(
        "INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, user_agent) VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, device_id) DO UPDATE SET endpoint = $4, p256dh = $5, auth = $6, user_agent = $7, failures = 0, last_error = NULL
         RETURNING id",
    )
    .bind(&id)
    .bind(user)
    .bind(device_id)
    .bind(endpoint)
    .bind(p256dh)
    .bind(auth)
    .bind(user_agent.chars().take(200).collect::<String>())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnsubscribeBody {
    device_id: String,
}

async fn unsubscribe_route(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<UnsubscribeBody>) -> Response {
    // Signing out of a device must work even if push was switched off on the server since.
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await {
        Ok(u) => u.id,
        Err(e) => return e.into_response(),
    };
    match sqlx::query("DELETE FROM push_subscriptions WHERE user_id = $1 AND device_id = $2").bind(&user).bind(body.device_id.trim()).execute(&state.db).await {
        Ok(done) => Json(json!({ "ok": true, "removed": done.rows_affected() > 0 })).into_response(),
        Err(error) => {
            tracing::error!("push unsubscribe: {error}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::routes::test_support::test_pool;
    use std::sync::Mutex;

    /// Throwaway fixture keypair (fixed scalar), only ever used by tests. Returns (public, private) base64url.
    pub(crate) fn fixture_keys() -> (String, String) {
        let private = [7u8; 32];
        let public = PrivateKey::from_private_key(&ECDH_P256, &private).unwrap().compute_public_key().unwrap();
        (URL_SAFE_NO_PAD.encode(public.as_ref()), URL_SAFE_NO_PAD.encode(private))
    }

    #[derive(Default)]
    pub(crate) struct Recorder {
        pub sent: Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>,
        pub status: Mutex<Option<u16>>,
    }

    #[async_trait::async_trait]
    impl PushTransport for Recorder {
        async fn post(&self, endpoint: &str, headers: Vec<(String, String)>, body: Vec<u8>) -> Result<u16, String> {
            self.sent.lock().unwrap().push((endpoint.into(), headers, body));
            Ok(self.status.lock().unwrap().unwrap_or(201))
        }
    }

    async fn pool() -> PgPool {
        let db = test_pool().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/039_push_subscriptions.sql").replace("public.", "")).execute(&db).await.unwrap();
        db
    }

    /// A browser-side subscription: returns (p256dh, auth, private key for decrypting).
    pub(crate) fn browser_sub() -> (String, String, PrivateKey, [u8; 16]) {
        let key = PrivateKey::generate(&ECDH_P256).unwrap();
        let public = URL_SAFE_NO_PAD.encode(key.compute_public_key().unwrap().as_ref());
        let auth = [9u8; 16];
        (public, URL_SAFE_NO_PAD.encode(auth), key, auth)
    }

    /// Decrypt an aes128gcm body the way a browser does.
    pub(crate) fn decrypt(body: &[u8], ua: &PrivateKey, ua_public: &[u8], auth: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        let idlen = body[20] as usize;
        let as_public = &body[21..21 + idlen];
        let mut record = body[21 + idlen..].to_vec();
        let shared = agreement::agree(ua, UnparsedPublicKey::new(&ECDH_P256, as_public), (), |s| Ok(s.to_vec())).unwrap();
        let mut info = b"WebPush: info\0".to_vec();
        info.extend_from_slice(ua_public);
        info.extend_from_slice(as_public);
        let mut ikm = [0u8; 32];
        hkdf(auth, &shared, &[&info], &mut ikm).unwrap();
        let mut cek = [0u8; 16];
        hkdf(salt, &ikm, &[b"Content-Encoding: aes128gcm\0"], &mut cek).unwrap();
        let mut nonce = [0u8; 12];
        hkdf(salt, &ikm, &[b"Content-Encoding: nonce\0"], &mut nonce).unwrap();
        let key = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &cek).unwrap());
        let plain = key.open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut record).unwrap();
        let end = plain.iter().rposition(|b| *b != 0).unwrap();
        assert_eq!(plain[end], 0x02, "padding delimiter");
        plain[..end].to_vec()
    }

    /// RFC 8291 appendix A.
    #[test]
    fn matches_the_rfc_8291_test_vector() {
        let ua_public = b64_decode("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4").unwrap();
        let auth = b64_decode("BTBZMqHH6r4Tts7J_aSIgg").unwrap();
        let as_private = PrivateKey::from_private_key(&ECDH_P256, &b64_decode("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw").unwrap()).unwrap();
        let salt: [u8; 16] = b64_decode("DGv6ra1nlYgDCS1FRnbzlw").unwrap().try_into().unwrap();
        let body = encrypt_with(b"When I grow up, I want to be a watermelon", &ua_public, &auth, &as_private, &salt).unwrap();
        // the RFC prints the 86-byte header and the ciphertext separately
        let mut expected = b64_decode("DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8").unwrap();
        expected.extend(b64_decode("8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ").unwrap());
        assert_eq!(body, expected);
    }

    #[test]
    fn encrypted_payload_round_trips() {
        let (p256dh, auth_b64, key, auth) = browser_sub();
        let public = b64_decode(&p256dh).unwrap();
        let body = encrypt_payload(b"hello \xF0\x9F\x93\x9E", &p256dh, &auth_b64).unwrap();
        assert_eq!(decrypt(&body, &key, &public, &auth), b"hello \xF0\x9F\x93\x9E");
        assert!(encrypt_payload(&vec![b'x'; 5000], &p256dh, &auth_b64).is_err());
        assert!(encrypt_payload(b"x", "not-a-key", &auth_b64).is_err());
    }

    #[test]
    fn vapid_header_is_a_valid_es256_jwt() {
        let (public, private) = fixture_keys();
        let vapid = Vapid::new(&public, &private, "mailto:test@allternit.com").unwrap();
        let header = vapid.authorization("https://fcm.googleapis.com/fcm/send/abc").unwrap();
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (jwt, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, public);
        let parts: Vec<&str> = jwt.split('.').collect();
        let claims: Value = serde_json::from_slice(&b64_decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["sub"], "mailto:test@allternit.com");
        assert!(claims["exp"].as_i64().unwrap() > chrono::Utc::now().timestamp());
        let signature = b64_decode(parts[2]).unwrap();
        assert_eq!(signature.len(), 64);
        let verifier = aws_lc_rs::signature::UnparsedPublicKey::new(&aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED, vapid.public_point());
        verifier.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature).unwrap();
        assert!(Vapid::new("AAAA", &private, "mailto:x@y.z").is_err());
    }

    #[test]
    fn endpoints_must_be_public_https_hosts() {
        assert!(endpoint_allowed("https://fcm.googleapis.com/fcm/send/abc"));
        assert!(endpoint_allowed("https://updates.push.services.mozilla.com/wpush/v2/abc"));
        assert!(endpoint_allowed("https://web.push.apple.com/abc"));
        for bad in ["http://fcm.googleapis.com/x", "https://127.0.0.1/x", "https://localhost/x", "https://[::1]/x", "https://10.0.0.5:8443/x", "https://user@evil.com/x", "https://intranet/x", "ftp://a.b/x", ""] {
            assert!(!endpoint_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn topic_is_short_and_url_safe() {
        let t = topic_for("call-app-0123456789abcdef0123456789abcdef");
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    async fn sub_for(db: &PgPool, user: &str, device: &str, endpoint: &str) -> (PrivateKey, Vec<u8>, [u8; 16]) {
        let (p256dh, auth_b64, key, auth) = browser_sub();
        subscribe(db, user, device, endpoint, &p256dh, &auth_b64, "test-agent").await.unwrap();
        (key, b64_decode(&p256dh).unwrap(), auth)
    }

    fn call_msg() -> PushMessage {
        PushMessage {
            kind: "call",
            title: "Incoming call from Olive".into(),
            body: "Tap to answer".into(),
            url: "/?allternit_call=room1".into(),
            tag: "call-room1".into(),
            data: json!({ "room": "room1" }),
            ttl_secs: 45,
            high_urgency: true,
        }
    }

    #[tokio::test]
    async fn sends_an_encrypted_signed_push_to_each_device() {
        let db = pool().await;
        let (public, private) = fixture_keys();
        let vapid = Vapid::new(&public, &private, "mailto:test@allternit.com").unwrap();
        let (k1, pub1, auth1) = sub_for(&db, "u1", "phone", "https://push.example.com/a").await;
        sub_for(&db, "u1", "laptop", "https://push.example.com/b").await;
        sub_for(&db, "u2", "phone", "https://push.example.com/c").await;
        let rec = Recorder::default();
        assert_eq!(send_to_user(&db, &rec, &vapid, "u1", &call_msg()).await, 2);
        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 2, "u2's device gets nothing");
        let (endpoint, headers, body) = sent.iter().find(|s| s.0.ends_with("/a")).unwrap();
        assert_eq!(endpoint, "https://push.example.com/a");
        let h = |n: &str| headers.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str()).unwrap();
        assert!(h("Authorization").starts_with("vapid t="));
        assert_eq!((h("Content-Encoding"), h("TTL"), h("Urgency")), ("aes128gcm", "45", "high"));
        assert_eq!(h("Topic").len(), 32);
        let payload: Value = serde_json::from_slice(&decrypt(body, &k1, &pub1, &auth1)).unwrap();
        assert_eq!(payload["type"], "call");
        assert_eq!(payload["title"], "Incoming call from Olive");
        assert_eq!(payload["url"], "/?allternit_call=room1");
        assert_eq!(payload["data"]["room"], "room1");
    }

    #[tokio::test]
    async fn dead_subscriptions_are_removed_and_flaky_ones_counted() {
        let db = pool().await;
        let (public, private) = fixture_keys();
        let vapid = Vapid::new(&public, &private, "mailto:test@allternit.com").unwrap();
        sub_for(&db, "u1", "phone", "https://push.example.com/a").await;
        let rec = Recorder::default();
        *rec.status.lock().unwrap() = Some(500);
        assert_eq!(send_to_user(&db, &rec, &vapid, "u1", &call_msg()).await, 0);
        let failures: i32 = sqlx::query_scalar("SELECT failures FROM push_subscriptions WHERE user_id = 'u1'").fetch_one(&db).await.unwrap();
        assert_eq!(failures, 1);
        *rec.status.lock().unwrap() = Some(201);
        assert_eq!(send_to_user(&db, &rec, &vapid, "u1", &call_msg()).await, 1);
        let failures: i32 = sqlx::query_scalar("SELECT failures FROM push_subscriptions WHERE user_id = 'u1'").fetch_one(&db).await.unwrap();
        assert_eq!(failures, 0, "a good push resets the count");
        *rec.status.lock().unwrap() = Some(410);
        send_to_user(&db, &rec, &vapid, "u1", &call_msg()).await;
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM push_subscriptions").fetch_one(&db).await.unwrap();
        assert_eq!(left, 0, "410 Gone deletes the subscription");
    }

    #[tokio::test]
    async fn subscribing_replaces_per_device_and_moves_endpoints() {
        let db = pool().await;
        let first = sub_for(&db, "u1", "phone", "https://push.example.com/a").await;
        drop(first);
        sub_for(&db, "u1", "phone", "https://push.example.com/a2").await;
        let rows: Vec<String> = sqlx::query_scalar("SELECT endpoint FROM push_subscriptions WHERE user_id = 'u1'").fetch_all(&db).await.unwrap();
        assert_eq!(rows, ["https://push.example.com/a2"], "same device replaces its row");
        // someone else signs in on the same browser: the endpoint moves to them
        sub_for(&db, "u2", "browser", "https://push.example.com/a2").await;
        let owners: Vec<String> = sqlx::query_scalar("SELECT user_id FROM push_subscriptions").fetch_all(&db).await.unwrap();
        assert_eq!(owners, ["u2"]);
        // validation
        let (p, a, _, _) = browser_sub();
        assert!(matches!(subscribe(&db, "u1", "", "https://push.example.com/x", &p, &a, "").await, Err(SubscribeError::Bad("bad_device_id"))));
        assert!(matches!(subscribe(&db, "u1", "d", "http://push.example.com/x", &p, &a, "").await, Err(SubscribeError::Bad("bad_endpoint"))));
        assert!(matches!(subscribe(&db, "u1", "d", "https://push.example.com/x", "AAAA", &a, "").await, Err(SubscribeError::Bad("bad_keys"))));
        for i in 0..MAX_SUBSCRIPTIONS_PER_USER {
            sub_for(&db, "u3", &format!("d{i}"), &format!("https://push.example.com/m{i}")).await;
        }
        assert!(matches!(subscribe(&db, "u3", "one-too-many", "https://push.example.com/zz", &p, &a, "").await, Err(SubscribeError::TooMany)));
        assert!(subscribe(&db, "u3", "d0", "https://push.example.com/m0", &p, &a, "").await.is_ok(), "an existing device can still refresh");
    }

    #[tokio::test]
    async fn message_pushes_collapse_per_thread_and_cap_per_hour() {
        let db = pool().await;
        assert!(allow_message_push(&db, "u1", "dm-1").await);
        assert!(!allow_message_push(&db, "u1", "dm-1").await, "same thread inside the window");
        assert!(allow_message_push(&db, "u1", "dm-2").await, "another thread is fine");
        assert!(allow_message_push(&db, "u2", "dm-1").await, "another person is fine");
        sqlx::query("UPDATE push_log SET created_at = now() - interval '30 seconds'").execute(&db).await.unwrap();
        assert!(allow_message_push(&db, "u1", "dm-1").await, "window passed");
        for i in 0..MAX_MESSAGE_PUSHES_PER_HOUR {
            sqlx::query("INSERT INTO push_log (user_id, collapse_key, created_at) VALUES ('u9', $1, now() - interval '5 minutes')").bind(format!("k{i}")).execute(&db).await.unwrap();
        }
        assert!(!allow_message_push(&db, "u9", "fresh").await, "hourly cap");
    }
}
