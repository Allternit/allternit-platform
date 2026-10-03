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
            cloud_token: non_empty(pick(secret, "cloudToken")).or_else(|| env("ALLTERNIT_CLOUD_API_TOKEN")),
            phone_number_id: non_empty(pick(secret, "phoneNumberId")),
        }
    }
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
}
