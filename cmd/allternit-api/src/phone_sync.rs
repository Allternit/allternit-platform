//! Phone numbers follow this runtime: pulled from the cloud as the runtime itself.
//!
//! The cloud (`phone_numbers`) says which numbers are assigned to this runtime; this
//! keeps `channel_phone_numbers` (and each number's `sms` connection) in step, so
//! an inbound call or text finds its bot without anyone re-attaching the number
//! by hand. No user token is involved: the pull is authenticated with the runtime's
//! own device credential ([`crate::relay_auth::RelaySecret`]).
//!
//! * Pull: `GET <cloud>/api/v1/runtime-devices/me/phone-numbers` →
//!   `{ runtimeId, userId, numbers: [{ id, e164, botId, smsState, voiceState, messagingRef,
//!   webhookPublicKey? }] }`. On boot, every [`PULL_INTERVAL`], and at once when the cloud
//!   pushes `phone.numbers.changed` (below), so a missed push heals itself.
//! * Push: `POST /webhooks/phone/numbers-changed` (signed by the cloud like every relayed
//!   request, [`crate::relay_auth::RelayedAuth`]) → 202 `{ ok: true }` and a pull now.
//! * Released: a number this sync created or updated that the cloud stops listing is removed
//!   here along with its `sms` connection. Numbers the sync never touched are left alone.
//! * Texts: a synced number's `sms` connection holds only `{ numberId, publicKey }`. Replies
//!   authenticate to the cloud with the runtime credential ([`runtime_bearer`]); a user
//!   token already sealed there by the UI flow is kept and still preferred.
//!
//! Inert until the runtime is paired (no device token, no pull).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::channel_phone::{is_e164, upsert_number};
use crate::db::DbHandle;
use crate::relay_auth::{RelayedAuth, RelaySecret};
use crate::AppState;

pub const NUMBERS_CHANGED_PATH: &str = "/webhooks/phone/numbers-changed";
pub const PULL_INTERVAL: Duration = Duration::from_secs(600);
/// After a failed pull (cloud unreachable, not paired yet) try again sooner.
const RETRY_INTERVAL: Duration = Duration::from_secs(60);

static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// The runtime's own bearer for cloud phone routes (`channels/sms/send`, `phone/calls/outbound`,
/// `phone/numbers/:id/consent`), or `None` when unpaired.
pub fn runtime_bearer() -> Option<String> {
    crate::relay_auth::process_secret().device_token()
}

pub fn cloud_base() -> String {
    std::env::var("ALLTERNIT_CLOUD_API_URL").ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "https://api.allternit.com".into())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteNumber {
    pub id: String,
    pub e164: String,
    pub bot_id: String,
    #[serde(default)]
    pub sms_state: String,
    #[serde(default)]
    pub voice_state: String,
    pub messaging_ref: Option<String>,
    /// The carrier's webhook verifying key (public), for re-checking relayed texts.
    pub webhook_public_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    user_id: Option<String>,
    #[serde(default)]
    numbers: Vec<RemoteNumber>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub synced: Vec<String>,
    pub removed: Vec<String>,
    /// `(number id, why)` for numbers that couldn't be applied (for instance the bot isn't on this runtime yet).
    pub skipped: Vec<(String, String)>,
}

/// Fetches the listing. A seam so tests don't need a network.
#[async_trait]
pub trait NumberSource: Send + Sync {
    async fn list(&self, cloud_url: &str, bearer: &str) -> Result<(u16, Value), String>;
}

pub struct HttpSource;

#[async_trait]
impl NumberSource for HttpSource {
    async fn list(&self, cloud_url: &str, bearer: &str) -> Result<(u16, Value), String> {
        let url = format!("{cloud_url}/api/v1/runtime-devices/me/phone-numbers");
        let resp = reqwest::Client::new().get(&url).bearer_auth(bearer).timeout(Duration::from_secs(15)).send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
    }
}

/// Drop a number's `sms` connection(s) and its row.
fn remove_number(conn: &rusqlite::Connection, owner: &str, number_id: &str) {
    let accounts: Vec<String> = conn
        .prepare("SELECT id FROM provider_account_bindings WHERE vendor = 'sms' AND owner = ?1 AND external_account_id = ?2")
        .and_then(|mut q| q.query_map(params![owner, number_id], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for a in &accounts {
        let _ = conn.execute("DELETE FROM channel_account_bots WHERE account_id = ?1", params![a]);
        let _ = conn.execute("DELETE FROM provider_account_bindings WHERE id = ?1", params![a]);
    }
    let _ = conn.execute("DELETE FROM channel_phone_numbers WHERE number_id = ?1 AND owner = ?2", params![number_id, owner]);
}

/// The number's `sms` connection: created on first sync, otherwise updated in place with
/// whatever the cloud says, keeping a user token the UI flow may have sealed there.
fn ensure_connection(conn: &rusqlite::Connection, owner: &str, n: &RemoteNumber, cloud_url: &str) -> Result<String, String> {
    let existing: Option<(String, String)> = conn
        .query_row("SELECT id, COALESCE(secret_ref, '') FROM provider_account_bindings WHERE vendor = 'sms' AND owner = ?1 AND external_account_id = ?2", params![owner, n.id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let mut secret: serde_json::Map<String, Value> = existing
        .as_ref()
        .and_then(|(_, sealed)| serde_json::from_str::<Value>(&crate::token_crypto::open(sealed)).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    secret.insert("numberId".into(), json!(n.id));
    if let Some(k) = n.webhook_public_key.as_deref().filter(|k| !k.trim().is_empty()) {
        secret.insert("publicKey".into(), json!(k));
    }
    if !secret.contains_key("cloudUrl") && std::env::var("ALLTERNIT_CLOUD_API_URL").is_ok() {
        secret.insert("cloudUrl".into(), json!(cloud_url));
    }
    let sealed = crate::token_crypto::seal(&Value::Object(secret).to_string());
    let t = crate::agent_gateway_routes::now();
    let account = match existing {
        Some((id, _)) => {
            conn.execute("UPDATE provider_account_bindings SET secret_ref = ?1, display_name = ?2, state = 'CONNECTED', updated_at = ?3 WHERE id = ?4", params![sealed, n.e164, t, id]).map_err(|e| e.to_string())?;
            id
        }
        None => {
            let id = crate::agent_gateway_routes::id("acct");
            conn.execute(
                "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, 'sms', 'channel_oauth', ?3, ?4, ?5, '[\"messages\"]', 'CONNECTED', ?6, ?6, ?6)",
                params![id, owner, n.id, n.e164, sealed, t],
            )
            .map_err(|e| e.to_string())?;
            id
        }
    };
    // One number answers as one bot: it is the connection's only member.
    conn.execute("DELETE FROM channel_account_bots WHERE account_id = ?1 AND bot_id <> ?2", params![account, n.bot_id]).map_err(|e| e.to_string())?;
    conn.execute("INSERT OR IGNORE INTO channel_account_bots (account_id, bot_id, owner, is_default, created_at) VALUES (?1, ?2, ?3, 1, ?4)", params![account, n.bot_id, owner, t]).map_err(|e| e.to_string())?;
    Ok(account)
}

/// Make this runtime's numbers match the cloud's list for `owner`.
pub fn apply(db: &DbHandle, owner: &str, remote: &[RemoteNumber], cloud_url: &str) -> Result<Report, String> {
    let conn = db.connect().map_err(|e| e.to_string())?;
    let mut report = Report::default();
    for n in remote {
        if !is_e164(&n.e164) {
            report.skipped.push((n.id.clone(), format!("{} is not an E.164 number", n.e164)));
            continue;
        }
        // The cloud keeps one live number per e164: an older row for the same number under another id is stale.
        let stale: Vec<String> = conn
            .prepare("SELECT number_id FROM channel_phone_numbers WHERE owner = ?1 AND e164 = ?2 AND number_id <> ?3")
            .and_then(|mut q| q.query_map(params![owner, n.e164, n.id], |r| r.get(0))?.collect())
            .unwrap_or_default();
        for s in &stale {
            remove_number(&conn, owner, s);
        }
        let applied = upsert_number(db, &n.id, owner, &n.bot_id, &n.e164, None).and_then(|_| ensure_connection(&conn, owner, n, cloud_url));
        match applied {
            Ok(account) => {
                conn.execute("UPDATE channel_phone_numbers SET account_id = ?2, synced_at = ?3 WHERE number_id = ?1 AND owner = ?4", params![n.id, account, chrono::Utc::now().to_rfc3339(), owner]).map_err(|e| e.to_string())?;
                report.synced.push(n.id.clone());
            }
            Err(why) => report.skipped.push((n.id.clone(), why)),
        }
    }
    let live: std::collections::HashSet<&str> = remote.iter().map(|n| n.id.as_str()).collect();
    let gone: Vec<String> = conn
        .prepare("SELECT number_id FROM channel_phone_numbers WHERE owner = ?1 AND synced_at IS NOT NULL")
        .and_then(|mut q| q.query_map(params![owner], |r| r.get(0))?.collect())
        .unwrap_or_default();
    for id in gone.into_iter().filter(|id| !live.contains(id.as_str())) {
        remove_number(&conn, owner, &id);
        report.removed.push(id);
    }
    Ok(report)
}

/// One pull: list the numbers assigned to this runtime and apply them.
pub async fn sync_once(db: &DbHandle, secret: &dyn RelaySecret, source: &dyn NumberSource) -> Result<Report, String> {
    let (bearer, owner) = (secret.device_token().ok_or("this runtime isn't paired")?, secret.paired_owner().ok_or("this runtime isn't paired")?);
    let cloud = cloud_base();
    let (status, body) = source.list(&cloud, &bearer).await?;
    match status {
        200..=299 => {}
        401 | 403 => return Err(format!("the cloud refused this runtime's credential ({status})")),
        s => return Err(format!("the cloud answered {s}: {}", body["error"].as_str().unwrap_or("no listing"))),
    }
    let listing: Listing = serde_json::from_value(body).map_err(|e| format!("unreadable phone number listing: {e}"))?;
    if listing.user_id.as_deref().is_some_and(|u| u != owner) {
        return Err("the cloud listed another account's numbers; not applying them".into());
    }
    apply(db, &owner, &listing.numbers, &cloud)
}

/// Pull on boot, every [`PULL_INTERVAL`], and whenever the cloud pushes a change.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let secret = crate::relay_auth::process_secret();
        let mut wait = Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = CHANGED.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
            wait = match sync_once(&state.db, secret.as_ref(), &HttpSource).await {
                Ok(r) => {
                    if !r.synced.is_empty() || !r.removed.is_empty() || !r.skipped.is_empty() {
                        tracing::info!(synced = r.synced.len(), removed = r.removed.len(), skipped = ?r.skipped, "phone numbers synced from the cloud");
                    }
                    PULL_INTERVAL
                }
                Err(e) => {
                    tracing::debug!("phone number sync: {e}");
                    RETRY_INTERVAL
                }
            };
        }
    });
}

pub fn phone_sync_router() -> Router<Arc<AppState>> {
    phone_sync_router_with(crate::relay_auth::process_secret())
}

pub fn phone_sync_router_with(secret: Arc<dyn RelaySecret>) -> Router<Arc<AppState>> {
    Router::new().route(NUMBERS_CHANGED_PATH, post(changed_h)).layer(crate::relay_auth::secret_layer(secret))
}

async fn changed_h(_auth: RelayedAuth) -> axum::response::Response {
    CHANGED.notify_one();
    (StatusCode::ACCEPTED, Json(json!({ "ok": true }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_auth::StaticRelaySecret;
    use std::sync::Mutex;

    struct Fake(Mutex<Vec<(String, String)>>, u16, Value);
    #[async_trait]
    impl NumberSource for Fake {
        async fn list(&self, cloud_url: &str, bearer: &str) -> Result<(u16, Value), String> {
            self.0.lock().unwrap().push((cloud_url.into(), bearer.into()));
            Ok((self.1, self.2.clone()))
        }
    }
    fn fake(status: u16, body: Value) -> Fake {
        Fake(Mutex::new(vec![]), status, body)
    }
    fn listing(numbers: Value) -> Value {
        json!({ "runtimeId": "rt_1", "userId": "user-a", "numbers": numbers })
    }
    fn num(id: &str, e164: &str, bot: &str) -> Value {
        json!({ "id": id, "e164": e164, "botId": bot, "smsState": "active", "voiceState": "active", "messagingRef": null, "webhookPublicKey": "cHVibGlj" })
    }
    fn secret() -> StaticRelaySecret {
        StaticRelaySecret { token: "allternit_runtime_tok".into(), owner: "user-a".into() }
    }

    async fn setup(tag: &str) -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-psync-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        let conn = st.db.connect().unwrap();
        for bot in ["bot-1", "bot-2"] {
            conn.execute("INSERT INTO agents (id, user_id, name, model, provider, is_bot, config) VALUES (?1,'user-a','b','m','p',1,'{}')", params![bot]).unwrap();
        }
        st
    }

    fn rows(st: &AppState) -> Vec<(String, String, Option<String>)> {
        let conn = st.db.connect().unwrap();
        let mut q = conn.prepare("SELECT number_id, bot_id, account_id FROM channel_phone_numbers ORDER BY number_id").unwrap();
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().filter_map(Result::ok).collect()
    }

    #[tokio::test]
    async fn a_number_the_cloud_assigns_appears_with_its_connection() {
        let st = setup("appear").await;
        let src = fake(200, listing(json!([num("n1", "+16512686010", "bot-1")])));
        let r = sync_once(&st.db, &secret(), &src).await.unwrap();
        assert_eq!(r.synced, vec!["n1"]);
        // It authenticated as the runtime.
        assert_eq!(src.0.lock().unwrap()[0].1, "allternit_runtime_tok");
        let got = rows(&st);
        assert_eq!((got[0].0.as_str(), got[0].1.as_str()), ("n1", "bot-1"));
        let n = crate::channel_phone::number(&st.db, "n1").unwrap();
        assert_eq!((n.owner.as_str(), n.e164.as_str()), ("user-a", "+16512686010"));
        // The sms connection carries the carrier key, no user token, and bot-1 as its default.
        let acct = crate::channel_transports::accounts(&st.db, "sms", got[0].2.as_deref()).remove(0);
        assert_eq!(crate::channel_transports::pick(&acct.secret, "publicKey"), "cHVibGlj");
        assert_eq!(crate::channel_transports::pick(&acct.secret, "numberId"), "n1");
        assert_eq!(crate::channel_transports::pick(&acct.secret, "token"), "");
        // Calls resolve their thread now (the bug: "that phone number isn't set up on this runtime").
        assert!(crate::channel_phone::number(&st.db, "n1").is_some());
    }

    #[tokio::test]
    async fn syncing_twice_changes_nothing_and_follows_a_bot_change() {
        let st = setup("idem").await;
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-1")])))).await.unwrap();
        let first = rows(&st);
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-1")])))).await.unwrap();
        assert_eq!(rows(&st), first);
        assert_eq!(st.db.connect().unwrap().query_row::<i64, _, _>("SELECT count(*) FROM provider_account_bindings WHERE vendor='sms'", [], |r| r.get(0)).unwrap(), 1);
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-2")])))).await.unwrap();
        assert_eq!(rows(&st)[0].1, "bot-2");
        let members: Vec<String> = {
            let conn = st.db.connect().unwrap();
            let mut q = conn.prepare("SELECT bot_id FROM channel_account_bots").unwrap();
            q.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
        };
        assert_eq!(members, vec!["bot-2"], "one number answers as one bot");
    }

    #[tokio::test]
    async fn a_user_token_the_ui_sealed_survives_the_sync() {
        let st = setup("token").await;
        let conn = st.db.connect().unwrap();
        upsert_number(&st.db, "n1", "user-a", "bot-1", "+16512686010", None).unwrap();
        let sealed = crate::token_crypto::seal(&json!({ "token": "user-bearer", "numberId": "n1", "publicKey": "old" }).to_string());
        conn.execute(
            "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at) VALUES ('acct-1','user-a','sms','channel_oauth','n1','x',?1,'[]','CONNECTED','t','t','t')",
            params![sealed],
        )
        .unwrap();
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-1")])))).await.unwrap();
        let acct = crate::channel_transports::accounts(&st.db, "sms", Some("acct-1")).remove(0);
        assert_eq!(crate::channel_transports::pick(&acct.secret, "token"), "user-bearer");
        assert_eq!(crate::channel_transports::pick(&acct.secret, "publicKey"), "cHVibGlj", "the carrier key is refreshed");
    }

    #[tokio::test]
    async fn released_numbers_are_removed_but_hand_made_ones_stay() {
        let st = setup("release").await;
        upsert_number(&st.db, "manual", "user-a", "bot-1", "+14155550100", None).unwrap();
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-1"), num("n2", "+16512686011", "bot-2")])))).await.unwrap();
        assert_eq!(rows(&st).len(), 3);
        let r = sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n2", "+16512686011", "bot-2")])))).await.unwrap();
        assert_eq!(r.removed, vec!["n1"]);
        assert_eq!(rows(&st).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["manual", "n2"]);
        assert_eq!(st.db.connect().unwrap().query_row::<i64, _, _>("SELECT count(*) FROM provider_account_bindings WHERE vendor='sms' AND external_account_id='n1'", [], |r| r.get(0)).unwrap(), 0);
        // Everything released: only the hand-made number remains.
        sync_once(&st.db, &secret(), &fake(200, listing(json!([])))).await.unwrap();
        assert_eq!(rows(&st).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["manual"]);
    }

    #[tokio::test]
    async fn a_rebought_number_replaces_its_stale_row() {
        let st = setup("rebuy").await;
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("old", "+16512686010", "bot-1")])))).await.unwrap();
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("new", "+16512686010", "bot-1")])))).await.unwrap();
        assert_eq!(rows(&st).iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["new"]);
    }

    #[tokio::test]
    async fn a_number_whose_bot_isnt_here_is_skipped_not_fatal() {
        let st = setup("nobot").await;
        let r = sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "ghost"), num("n2", "+16512686011", "bot-1")])))).await.unwrap();
        assert_eq!(r.synced, vec!["n2"]);
        assert_eq!(r.skipped[0].0, "n1");
        assert!(r.skipped[0].1.contains("bot doesn't exist"));
    }

    #[tokio::test]
    async fn failures_change_nothing() {
        let st = setup("fail").await;
        sync_once(&st.db, &secret(), &fake(200, listing(json!([num("n1", "+16512686010", "bot-1")])))).await.unwrap();
        for (status, body) in [(401, json!({})), (404, json!({ "error": "x" })), (503, json!({}))] {
            assert!(sync_once(&st.db, &secret(), &fake(status, body)).await.is_err());
        }
        // Another account's list is refused.
        let other = json!({ "userId": "user-b", "numbers": [] });
        assert!(sync_once(&st.db, &secret(), &fake(200, other)).await.is_err());
        assert_eq!(rows(&st).len(), 1);
        // Unpaired: no request at all.
        let unpaired = crate::relay_auth::UnconfiguredRelaySecret;
        let src = fake(200, listing(json!([])));
        assert!(sync_once(&st.db, &unpaired, &src).await.is_err());
        assert!(src.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_push_route_needs_the_cloud_signature() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let st = setup("push").await;
        let app = phone_sync_router_with(Arc::new(secret())).with_state(st);
        let body = br#"{"type":"phone.numbers.changed"}"#;
        let status = |req: Request<Body>| {
            let app = app.clone();
            async move { app.oneshot(req).await.unwrap().status() }
        };
        assert_eq!(status(crate::relay_auth::relayed_post(NUMBERS_CHANGED_PATH, body, None)).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(crate::relay_auth::relayed_post(NUMBERS_CHANGED_PATH, body, Some(("wrong", "user-a")))).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(crate::relay_auth::relayed_post(NUMBERS_CHANGED_PATH, body, Some(("allternit_runtime_tok", "user-a")))).await, StatusCode::ACCEPTED);
    }
}
