//! WhatsApp business number through Allternit's Embedded Signup (cloud-api
//! `routes/whatsapp_es.rs`).
//!
//! The Meta app secret and the business token never reach this runtime. Inbound
//! requests arrive from the cloud relay re-signed with a per-address relay
//! secret (the account's `appSecret`); outbound goes back through
//! `POST {cloud}/api/v1/channels/whatsapp/send`, which holds the token and
//! enforces the 24-hour window.
//!
//! Account secret (`mode: "business"` selects this transport):
//! `{ mode, appSecret (relay secret), phoneNumberId, cloudToken? }`.
//! `cloudToken` is an `allternit_*` API token with the `compute` scope, falling
//! back to env `ALLTERNIT_CLOUD_API_TOKEN`. The cloud base is
//! `ALLTERNIT_CLOUD_API_URL` (default https://api.allternit.com).
//!
//! One number has one display name, so every message carries the speaking
//! bot's name in bold (WhatsApp bold is `*text*`).

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::channel_gateway::*;
use crate::channel_transports::{pick, whatsapp_normalize, HttpReq, HttpResp, HttpSend, WhatsAppTransport};

pub struct WhatsAppBusinessTransport {
    pub http: Arc<dyn HttpSend>,
    pub cloud_url: String,
    pub cloud_token: Option<String>,
    pub phone_number_id: Option<String>,
}

/// True when the account secret selects the cloud-sent business transport.
pub fn is_business(secret: &str) -> bool {
    pick(secret, "mode") == "business"
}

impl WhatsAppBusinessTransport {
    pub fn from_secret(secret: &str, http: Arc<dyn HttpSend>) -> Self {
        let non_empty = |s: String| Some(s).filter(|s| !s.is_empty());
        let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Self {
            http,
            cloud_url: env("ALLTERNIT_CLOUD_API_URL").unwrap_or_else(|| "https://api.allternit.com".into()).trim_end_matches('/').to_string(),
            // A signed-in computer sends with its own device credential; no token to paste.
            cloud_token: non_empty(pick(secret, "cloudToken")).or_else(|| env("ALLTERNIT_CLOUD_API_TOKEN")).or_else(crate::phone_sync::runtime_bearer),
            phone_number_id: non_empty(pick(secret, "phoneNumberId")),
        }
    }
}

/// `POST /gateway/channel-accounts/whatsapp {runtimeId?} | {relaySecret, phoneNumberId, displayName?}`:
/// record the business numbers Embedded Signup connected, so bots can be switched on for them.
/// Without a relay secret in the body it asks the cloud (`GET /api/v1/channels/whatsapp/accounts`)
/// for the owner's numbers routed to `runtimeId`. 404 `not_installed` until a signup lands. The relay
/// secret is sealed; the Meta token never leaves the cloud.
pub fn whatsapp_business_connect_router() -> axum::Router<Arc<crate::AppState>> {
    axum::Router::new().route("/gateway/channel-accounts/whatsapp", axum::routing::post(connect_h))
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConnectBody {
    relay_secret: Option<String>,
    phone_number_id: Option<String>,
    display_name: Option<String>,
    runtime_id: Option<String>,
}

async fn connect_h(
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::Extension(user): axum::Extension<crate::auth::AuthUser>,
    headers: HeaderMap,
    body: Option<axum::Json<ConnectBody>>,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse, Json};
    let body = body.map(|axum::Json(b)| b).unwrap_or_default();
    if let (Some(secret), Some(pnid)) = (body.relay_secret.as_deref(), body.phone_number_id.as_deref()) {
        return match upsert_business_account(&state.db, &user.user_id, secret, pnid, body.display_name.as_deref()) {
            Ok(v) => Json(v).into_response(),
            Err((code, msg)) => (code, Json(json!({ "error": msg }))).into_response(),
        };
    }
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| crate::phone_sync::runtime_bearer().map(|t| format!("Bearer {t}")));
    let Some(auth) = auth else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "not_signed_in" }))).into_response();
    };
    let url = format!("{}/api/v1/channels/whatsapp/accounts", crate::phone_sync::cloud_base().trim_end_matches('/'));
    let resp = match reqwest::Client::new().get(url).header("authorization", auth).timeout(std::time::Duration::from_secs(10)).send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud unreachable: {e}") }))).into_response(),
    };
    if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "whatsapp_not_configured" }))).into_response();
    }
    if !resp.status().is_success() {
        return (StatusCode::BAD_GATEWAY, Json(json!({ "error": format!("cloud returned {}", resp.status()) }))).into_response();
    }
    let listed: Value = resp.json().await.unwrap_or(Value::Null);
    match record_from_cloud(&state.db, &user.user_id, &listed, body.runtime_id.as_deref()) {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_installed" }))).into_response(),
        Err((code, msg)) => (code, Json(json!({ "error": msg }))).into_response(),
    }
}

/// Record every number the cloud lists for this computer (all of them when no runtime is named);
/// answers with the newest one recorded, or `None` when there are none yet.
pub fn record_from_cloud(db: &crate::db::DbHandle, owner: &str, listed: &Value, runtime_id: Option<&str>) -> Result<Option<Value>, (axum::http::StatusCode, String)> {
    let mut last = None;
    for a in listed.get("accounts").and_then(Value::as_array).into_iter().flatten() {
        let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or_default();
        if runtime_id.is_some_and(|rt| s("runtimeId") != rt) {
            continue;
        }
        last = Some(upsert_business_account(db, owner, s("relaySecret"), s("phoneNumberId"), None)?);
    }
    Ok(last)
}

pub fn upsert_business_account(db: &crate::db::DbHandle, owner: &str, relay_secret: &str, phone_number_id: &str, display_name: Option<&str>) -> Result<Value, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let (relay_secret, pnid) = (relay_secret.trim(), phone_number_id.trim());
    if relay_secret.is_empty() || pnid.is_empty() || !pnid.bytes().all(|b| b.is_ascii_digit()) {
        return Err((StatusCode::BAD_REQUEST, "relaySecret and a numeric phoneNumberId are required".into()));
    }
    let keys = json!({ "mode": "business", "appSecret": relay_secret, "phoneNumberId": pnid }).to_string();
    let Some(sealed) = crate::agent_gateway_routes::seal_strict(&keys) else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no encryption key is configured; the connection was not stored".into()));
    };
    let conn = db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let name = display_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("WhatsApp business");
    let t = crate::agent_gateway_routes::now();
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM provider_account_bindings WHERE owner = ?1 AND vendor = 'whatsapp' AND external_account_id = ?2",
            rusqlite::params![owner, pnid],
            |r| r.get(0),
        )
        .ok();
    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE provider_account_bindings SET display_name = ?1, secret_ref = ?2, state = 'CONNECTED', verified_at = ?3, updated_at = ?3 WHERE id = ?4 AND owner = ?5",
                rusqlite::params![name, sealed, t, id, owner],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
        None => {
            let id = crate::agent_gateway_routes::id("acct");
            conn.execute(
                "INSERT INTO provider_account_bindings (id, owner, vendor, auth_type, external_account_id, display_name, secret_ref, scopes_json, state, verified_at, created_at, updated_at)
                 VALUES (?1, ?2, 'whatsapp', 'channel_oauth', ?3, ?4, ?5, '[\"messages\"]', 'CONNECTED', ?6, ?6, ?6)",
                rusqlite::params![id, owner, pnid, name, sealed, t],
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            id
        }
    };
    Ok(json!({ "account": { "id": id, "vendor": "whatsapp", "displayName": name, "handle": pnid, "state": "CONNECTED" } }))
}

/// The bot's name in bold ahead of its text.
pub fn speaker_text(speaker: Option<&str>, text: &str) -> String {
    match speaker.map(|s| s.replace('*', "")).filter(|s| !s.trim().is_empty()) {
        Some(name) => format!("*{}*: {text}", name.trim()),
        None => text.to_string(),
    }
}

#[async_trait]
impl ChannelTransport for WhatsAppBusinessTransport {
    fn provider(&self) -> &'static str {
        "whatsapp"
    }
    fn verify(&self, secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
        WhatsAppTransport { http: self.http.clone(), access_token: None, own_identity: None }.verify(secret, headers, body)
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        whatsapp_normalize(payload)
    }
    fn identity(&self, requested: Option<&str>) -> Identity {
        // Never exact: the number's display name is the user's business, not the bot's.
        Identity { id: requested.map(str::to_string), exact: false }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        let token = self.cloud_token.clone().ok_or_else(|| PostError::Rejected("no Allternit cloud token configured for WhatsApp".into()))?;
        let to = out.thread.clone().ok_or_else(|| PostError::Rejected("no WhatsApp recipient for this conversation".into()))?;
        let body = json!({ "phoneNumberId": out.channel, "to": to, "text": speaker_text(out.identity.as_deref(), &out.text) });
        let url = format!("{}/api/v1/channels/whatsapp/send", self.cloud_url);
        let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await.map_err(PostError::Uncertain)?;
        match resp.status {
            200..=299 => resp
                .body
                .get("messageId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(|id| Receipt { remote_id: id.to_string(), relayed: true })
                .ok_or_else(|| PostError::Uncertain("the cloud accepted the send but returned no message id".into())),
            409 => Err(PostError::Rejected("outside_24h_window: WhatsApp only allows a template message after 24 hours of silence".into())),
            503 => Err(PostError::Rejected("whatsapp_not_configured".into())),
            500..=599 => Err(PostError::Uncertain(format!("cloud returned {}", resp.status))),
            s => Err(PostError::Rejected(format!("cloud returned {s}: {}", resp.body))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeHttp {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<Option<HttpResp>>,
    }
    #[async_trait]
    impl HttpSend for FakeHttp {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            Ok(self.reply.lock().unwrap().clone().unwrap_or(HttpResp { status: 200, body: json!({}) }))
        }
    }

    fn transport(http: Arc<FakeHttp>) -> WhatsAppBusinessTransport {
        WhatsAppBusinessTransport { http, cloud_url: "https://cloud.test".into(), cloud_token: Some("allternit_tok".into()), phone_number_id: Some("PN1".into()) }
    }
    fn out(identity: Option<&str>) -> Outbound {
        Outbound { workspace: None, channel: "PN1".into(), thread: Some("15551234567".into()), text: "hello".into(), identity: identity.map(str::to_string) }
    }

    #[test]
    fn the_speaker_is_bold_and_asterisks_in_names_cannot_break_it() {
        assert_eq!(speaker_text(Some("Muse"), "hi"), "*Muse*: hi");
        assert_eq!(speaker_text(Some("a*b"), "hi"), "*ab*: hi");
        assert_eq!(speaker_text(None, "hi"), "hi");
        assert_eq!(speaker_text(Some("  "), "hi"), "hi");
    }

    #[test]
    fn the_account_secret_selects_the_business_transport() {
        assert!(is_business(r#"{"mode":"business","appSecret":"s"}"#));
        assert!(!is_business(r#"{"appSecret":"s","accessToken":"t"}"#));
    }

    #[tokio::test]
    async fn sends_through_the_cloud_with_the_prefix() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = Some(HttpResp { status: 200, body: json!({ "messageId": "wamid.OK", "windowOpen": true }) });
        let receipt = transport(http.clone()).post(&out(Some("Muse"))).await.unwrap();
        assert_eq!(receipt, Receipt { remote_id: "wamid.OK".into(), relayed: true });
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].url, "https://cloud.test/api/v1/channels/whatsapp/send");
        assert_eq!(sent[0].headers, vec![("Authorization".to_string(), "Bearer allternit_tok".to_string())]);
        assert_eq!(sent[0].body, json!({ "phoneNumberId": "PN1", "to": "15551234567", "text": "*Muse*: hello" }));
    }

    #[tokio::test]
    async fn the_24h_refusal_is_a_definite_rejection() {
        let http = Arc::new(FakeHttp::default());
        *http.reply.lock().unwrap() = Some(HttpResp { status: 409, body: json!({ "error": "outside_24h_window" }) });
        match transport(http).post(&out(Some("Muse"))).await {
            Err(PostError::Rejected(why)) => assert!(why.starts_with("outside_24h_window")),
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_cloud_token_means_nothing_is_sent() {
        let http = Arc::new(FakeHttp::default());
        let mut t = transport(http.clone());
        t.cloud_token = None;
        assert!(matches!(t.post(&out(None)).await, Err(PostError::Rejected(_))));
        assert!(http.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn embedded_signup_records_the_business_number_once() {
        let dir = std::env::temp_dir().join(format!("allternit-wa-biz-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let st = crate::test_helpers::app_state(&dir).await;
        assert!(upsert_business_account(&st.db, "u1", "", "123", None).is_err());
        assert!(upsert_business_account(&st.db, "u1", "rs", "12a", None).is_err());
        let a = upsert_business_account(&st.db, "u1", "relay-1", "1555", Some("Acme Support")).unwrap();
        let b = upsert_business_account(&st.db, "u1", "relay-2", "1555", None).unwrap();
        assert_eq!(a["account"]["id"], b["account"]["id"], "re-onboarding the same number updates it");
        let accts = crate::channel_transports::accounts(&st.db, "whatsapp", None);
        assert_eq!(accts.len(), 1);
        assert!(is_business(&accts[0].secret));
        assert_eq!(crate::channel_transports::pick(&accts[0].secret, "appSecret"), "relay-2");

        // Pulled from the cloud: only the numbers routed to this computer are recorded.
        let listed = json!({ "accounts": [
            { "accountId": "a1", "runtimeId": "rt-other", "phoneNumberId": "1777", "relaySecret": "r-other" },
            { "accountId": "a2", "runtimeId": "rt-1", "phoneNumberId": "1888", "relaySecret": "r-1" },
        ] });
        assert!(record_from_cloud(&st.db, "u1", &json!({ "accounts": [] }), Some("rt-1")).unwrap().is_none());
        let got = record_from_cloud(&st.db, "u1", &listed, Some("rt-1")).unwrap().unwrap();
        assert_eq!(got["account"]["handle"], "1888");
        assert_eq!(crate::channel_transports::accounts(&st.db, "whatsapp", None).len(), 2, "1555 from before + 1888; 1777 lives on another computer");
    }
}
