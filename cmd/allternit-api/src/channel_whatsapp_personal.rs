//! WhatsApp personal number (linked device, QR) behind [`ChannelTransport`].
//!
//! Unofficial: the `services/wa-personal` sidecar drives the number through Baileys
//! on the user's own runtime. Inbound arrives at `/webhooks/channels/whatsapp-personal`
//! with `x-allternit-sidecar-token`; outbound goes to the sidecar's `POST /send`.
//! The whole provider is inert unless `ALLTERNIT_WA_PERSONAL=1`.
//!
//! Secret (sealed in `provider_account_bindings`): `{ sidecarUrl, sidecarToken, botId? }`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::HeaderMap;
use serde_json::{json, Value};

use crate::channel_gateway::*;
use crate::channel_transports::{pick, HttpReq, HttpSend};

pub const PROVIDER: &str = "whatsapp-personal";

/// Shown in the connect UI next to the QR.
pub const WARNING: &str = "Unofficial: this links your personal WhatsApp number as a linked device through an unofficial library. WhatsApp can ban numbers that automate messaging, and a ban cannot be undone by Allternit. Use a dedicated number you can afford to lose, not your main one.";

pub fn enabled() -> bool {
    std::env::var("ALLTERNIT_WA_PERSONAL").map(|v| v == "1").unwrap_or(false)
}

pub struct WhatsAppPersonalTransport {
    pub http: Arc<dyn HttpSend>,
    pub sidecar_url: Option<String>,
    pub sidecar_token: Option<String>,
    pub own_identity: Option<String>,
}

impl WhatsAppPersonalTransport {
    pub fn from_secret(secret: &str, http: Arc<dyn HttpSend>) -> Self {
        let f = |k: &str| Some(pick(secret, k)).filter(|s| !s.is_empty());
        Self { http, sidecar_url: f("sidecarUrl"), sidecar_token: f("sidecarToken"), own_identity: f("botId") }
    }
}

/// One sidecar `message` event -> one inbound. A chat (DM or group) is one conversation.
pub fn normalize(p: &Value) -> Vec<Inbound> {
    if p["event"].as_str() != Some("message") {
        return vec![];
    }
    let (Some(chat), Some(id)) = (p["chat"].as_str(), p["id"].as_str()) else { return vec![] };
    let Some(text) = p["text"].as_str().filter(|t| !t.trim().is_empty()) else { return vec![] };
    vec![Inbound {
        kind: InboundKind::Message,
        workspace: None,
        channel: chat.to_string(),
        conversation: format!("{PROVIDER}:{chat}"),
        thread: None,
        remote_id: id.to_string(),
        message_id: id.to_string(),
        text: Some(text.to_string()),
        user: p["from"].as_str().map(str::to_string),
        reaction: None,
        added: None,
        cursor: p["ts"].as_i64().map(|n| n.to_string()),
        own: false,
    }]
}

/// One number has one display name, so each bot's message starts with its bold name.
pub fn speaker_prefixed(who: Option<&str>, text: &str) -> String {
    match who.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => format!("*{w}*\n{text}"),
        None => text.to_string(),
    }
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[async_trait]
impl ChannelTransport for WhatsAppPersonalTransport {
    fn provider(&self) -> &'static str {
        PROVIDER
    }
    fn verify(&self, secret: &str, headers: &HeaderMap, _body: &[u8]) -> Result<(), String> {
        if !enabled() {
            return Err("whatsapp_personal_disabled".into());
        }
        let want = pick(secret, "sidecarToken");
        let given = headers.get("x-allternit-sidecar-token").and_then(|v| v.to_str().ok()).unwrap_or_default();
        if want.is_empty() || !same_secret(&want, given) {
            return Err("bad sidecar token".into());
        }
        Ok(())
    }
    fn normalize(&self, payload: &Value) -> Vec<Inbound> {
        normalize(payload)
    }
    /// The bot's name always travels in the message (never the number's display name).
    fn identity(&self, requested: Option<&str>) -> Identity {
        Identity { id: requested.map(str::to_string), exact: requested.is_none() }
    }
    async fn post(&self, out: &Outbound) -> Result<Receipt, PostError> {
        if !enabled() {
            return Err(PostError::Rejected("whatsapp_personal_disabled".into()));
        }
        let (Some(base), Some(token)) = (&self.sidecar_url, &self.sidecar_token) else {
            return Err(PostError::Rejected("the WhatsApp personal bridge is not set up".into()));
        };
        let relayed = !self.identity(out.identity.as_deref()).exact;
        let body = json!({ "to": out.channel, "text": speaker_prefixed(out.identity.as_deref(), &out.text) });
        let url = format!("{}/send", base.trim_end_matches('/'));
        let resp = self.http.post_json(HttpReq { url, headers: vec![("Authorization".into(), format!("Bearer {token}"))], body }).await;
        match resp {
            // Connection failure: the sidecar may or may not have sent it.
            Err(e) => Err(PostError::Uncertain(e)),
            Ok(r) => match r.status {
                200..=299 => Ok(Receipt { remote_id: r.body["id"].as_str().map(str::to_string).unwrap_or_default(), relayed }),
                409 => Err(PostError::Rejected("the WhatsApp number is not linked right now (scan the QR again)".into())),
                500..=599 => Err(PostError::Uncertain(format!("sidecar returned {}", r.status))),
                s => Err(PostError::Rejected(format!("sidecar returned {s}: {}", r.body))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel_transports::HttpResp;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        sent: Mutex<Vec<HttpReq>>,
        reply: Mutex<Option<Result<HttpResp, String>>>,
    }
    #[async_trait]
    impl HttpSend for Fake {
        async fn post_json(&self, req: HttpReq) -> Result<HttpResp, String> {
            self.sent.lock().unwrap().push(req);
            self.reply.lock().unwrap().clone().unwrap_or(Ok(HttpResp { status: 200, body: json!({ "id": "WAID1" }) }))
        }
    }

    fn on<R>(f: impl FnOnce() -> R) -> R {
        // Process-global env; every test that needs it sets the same value.
        std::env::set_var("ALLTERNIT_WA_PERSONAL", "1");
        f()
    }

    fn tx(http: Arc<Fake>) -> WhatsAppPersonalTransport {
        WhatsAppPersonalTransport::from_secret(r#"{"sidecarUrl":"http://127.0.0.1:8791/","sidecarToken":"tok"}"#, http)
    }

    #[test]
    fn normalizes_a_sidecar_message_into_one_conversation_per_chat() {
        let e = normalize(&json!({ "event": "message", "id": "A1", "chat": "1555@s.whatsapp.net", "from": "1555", "text": "hi", "ts": 1700000000 }));
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].conversation, "whatsapp-personal:1555@s.whatsapp.net");
        assert_eq!(e[0].channel, "1555@s.whatsapp.net");
        assert_eq!(e[0].text.as_deref(), Some("hi"));
        assert!(!e[0].own);
        assert!(normalize(&json!({ "event": "message", "id": "A1", "chat": "c", "text": "  " })).is_empty());
        assert!(normalize(&json!({ "event": "qr" })).is_empty());
    }

    #[test]
    fn verify_checks_the_sidecar_token_and_the_flag() {
        on(|| {
            let t = tx(Arc::new(Fake::default()));
            let secret = r#"{"sidecarToken":"tok"}"#;
            let mut h = HeaderMap::new();
            assert!(t.verify(secret, &h, b"{}").is_err());
            h.insert("x-allternit-sidecar-token", "wrong".parse().unwrap());
            assert!(t.verify(secret, &h, b"{}").is_err());
            h.insert("x-allternit-sidecar-token", "tok".parse().unwrap());
            assert!(t.verify(secret, &h, b"{}").is_ok());
            assert!(t.verify(r#"{"sidecarToken":""}"#, &h, b"{}").is_err());
        });
    }

    #[tokio::test]
    async fn post_sends_through_the_sidecar_with_a_bold_speaker_prefix() {
        on(|| ());
        let http = Arc::new(Fake::default());
        let r = tx(http.clone()).post(&Outbound { workspace: None, channel: "1555@s.whatsapp.net".into(), thread: None, text: "On it.".into(), identity: Some("Finance Bot".into()) }).await.unwrap();
        assert_eq!(r, Receipt { remote_id: "WAID1".into(), relayed: true });
        let sent = http.sent.lock().unwrap();
        assert_eq!(sent[0].url, "http://127.0.0.1:8791/send");
        assert_eq!(sent[0].headers, vec![("Authorization".to_string(), "Bearer tok".to_string())]);
        assert_eq!(sent[0].body, json!({ "to": "1555@s.whatsapp.net", "text": "*Finance Bot*\nOn it." }));
    }

    #[tokio::test]
    async fn post_classifies_sidecar_failures() {
        on(|| ());
        let out = Outbound { workspace: None, channel: "c".into(), thread: None, text: "x".into(), identity: None };
        for (reply, want_uncertain) in [(Ok(HttpResp { status: 409, body: json!({}) }), false), (Ok(HttpResp { status: 502, body: json!({}) }), true), (Err("refused".into()), true)] {
            let http = Arc::new(Fake { reply: Mutex::new(Some(reply)), ..Default::default() });
            let err = tx(http).post(&out).await.unwrap_err();
            assert_eq!(matches!(err, PostError::Uncertain(_)), want_uncertain, "{err:?}");
        }
        let unset = WhatsAppPersonalTransport::from_secret("{}", Arc::new(Fake::default()));
        assert!(matches!(unset.post(&out).await, Err(PostError::Rejected(_))));
    }

    #[test]
    fn speaker_prefix_is_bold_only_when_a_bot_speaks() {
        assert_eq!(speaker_prefixed(Some("Ada"), "hi"), "*Ada*\nhi");
        assert_eq!(speaker_prefixed(None, "hi"), "hi");
        assert_eq!(speaker_prefixed(Some(" "), "hi"), "hi");
    }

    #[test]
    fn warning_names_the_risks() {
        assert!(WARNING.contains("Unofficial") && WARNING.contains("ban") && WARNING.contains("dedicated number"));
    }
}
