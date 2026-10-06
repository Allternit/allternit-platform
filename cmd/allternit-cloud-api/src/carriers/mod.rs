//! Phone carriers (Telnyx primary, Twilio fallback) behind one interface.
//!
//! Everything the phone routes need from a carrier goes through [`Carrier`]:
//! number search/buy/release, the messaging webhook, SMS send, inbound webhook
//! verification, US registration (10DLC or toll-free verification) and port-in.
//! Adapters talk to the carrier through [`CarrierHttp`], so tests fake the wire
//! and never touch a real account.
//!
//! Env (unset means the phone routes answer 503 `phone_not_configured`):
//! - `ALLTERNIT_PHONE_CARRIER` = `telnyx` (default) | `twilio`
//! - Telnyx: `ALLTERNIT_TELNYX_API_KEY`, `ALLTERNIT_TELNYX_PUBLIC_KEY` (base64
//!   ed25519 webhook key).
//! - Twilio: `ALLTERNIT_TWILIO_ACCOUNT_SID`, `ALLTERNIT_TWILIO_AUTH_TOKEN`.

pub mod telnyx;
pub mod twilio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarrierError {
    /// A required env var is unset.
    NotConfigured,
    /// The carrier doesn't offer this operation through its API.
    Unsupported(&'static str),
    /// The caller's input is unusable for this carrier.
    Invalid(String),
    /// The webhook failed signature verification (or is stale).
    BadSignature,
    /// The carrier refused or failed (HTTP status, message with secrets removed).
    Upstream(u16, String),
    /// Network failure.
    Transport(String),
}

impl std::fmt::Display for CarrierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "carrier not configured"),
            Self::Unsupported(what) => write!(f, "{what} is not supported on this carrier"),
            Self::Invalid(m) => write!(f, "{m}"),
            Self::BadSignature => write!(f, "bad webhook signature"),
            Self::Upstream(status, m) => write!(f, "carrier error {status}: {m}"),
            Self::Transport(m) => write!(f, "carrier unreachable: {m}"),
        }
    }
}

/// One HTTP call to a carrier.
#[derive(Debug, Clone, Default)]
pub struct HttpReq {
    pub method: &'static str,
    pub url: String,
    pub bearer: Option<String>,
    pub basic: Option<(String, String)>,
    pub json: Option<Value>,
    pub form: Option<Vec<(String, String)>>,
}

#[derive(Debug, Clone)]
pub struct HttpResp {
    pub status: u16,
    pub body: Value,
}

#[async_trait]
pub trait CarrierHttp: Send + Sync {
    async fn send(&self, req: HttpReq) -> Result<HttpResp, CarrierError>;
}

/// Production transport.
pub struct ReqwestHttp(reqwest::Client);

impl ReqwestHttp {
    pub fn new() -> Self {
        Self(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default(),
        )
    }
}

impl Default for ReqwestHttp {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CarrierHttp for ReqwestHttp {
    async fn send(&self, req: HttpReq) -> Result<HttpResp, CarrierError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes()).map_err(|e| CarrierError::Invalid(e.to_string()))?;
        let mut builder = self.0.request(method, &req.url);
        if let Some(token) = &req.bearer {
            builder = builder.bearer_auth(token);
        }
        if let Some((user, pass)) = &req.basic {
            builder = builder.basic_auth(user, Some(pass));
        }
        if let Some(json) = &req.json {
            builder = builder.json(json);
        }
        if let Some(form) = &req.form {
            builder = builder.form(form);
        }
        // reqwest's error text can include the URL; the URL never carries a secret.
        let response = builder.send().await.map_err(|e| CarrierError::Transport(e.without_url().to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(HttpResp { status, body })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberType {
    Local,
    TollFree,
}

impl NumberType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::TollFree => "toll_free",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "toll_free" | "tollfree" => Some(Self::TollFree),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub country: String,
    pub area_code: Option<String>,
    pub locality: Option<String>,
    pub kind: NumberType,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableNumber {
    pub e164: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub locality: Option<String>,
    pub monthly_cost: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BuyRequest {
    pub e164: String,
    pub kind: NumberType,
    /// Where the carrier posts inbound texts (the number's relay address).
    pub webhook_url: String,
    /// Our number id, kept on the carrier's side as a reference.
    pub reference: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoughtNumber {
    /// The carrier's id for the number, when it is known at purchase time.
    pub carrier_number_id: Option<String>,
    /// What messages are sent through (Telnyx messaging profile, Twilio messaging service).
    pub messaging_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    pub id: String,
    pub parts: u32,
}

/// A verified inbound webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundEvent {
    Message { id: String, from: String, to: String, text: String },
    /// Delivery receipts and other events that need no action.
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationKind {
    TenDlc,
    TollFree,
}

impl RegistrationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TenDlc => "10dlc",
            Self::TollFree => "tollfree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegState {
    Pending,
    Approved,
    Rejected,
}

impl RegState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// The business form behind a registration. Camel-case JSON from the app.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RegistrationForm {
    pub entity_type: String,
    pub legal_name: String,
    pub display_name: String,
    pub ein: Option<String>,
    pub website: String,
    pub vertical: String,
    pub street: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub country: String,
    pub contact_first_name: String,
    pub contact_last_name: String,
    pub contact_email: String,
    pub contact_phone: String,
    /// Sole proprietors: the mobile number the carrier texts a verification code to.
    pub mobile_phone: Option<String>,
    pub use_case: String,
    pub use_case_summary: String,
    pub sample_messages: Vec<String>,
    /// Free-text notes from the business. Not sent as the carrier message flow:
    /// carriers reject flows that list several opt-in paths (see `message_flow`).
    pub opt_in_workflow: String,
    pub opt_in_image_urls: Vec<String>,
    /// Public page that shows the number and the texting disclosure (the call to
    /// action). Filled with Allternit's hosted page for the number when empty.
    pub opt_in_page_url: Option<String>,
    pub message_volume: String,
    pub privacy_policy_url: Option<String>,
    pub terms_url: Option<String>,
    /// Twilio only: the Trust Hub bundles an A2P brand is built from.
    pub twilio_customer_profile_sid: Option<String>,
    pub twilio_a2p_profile_sid: Option<String>,
}

/// Carrier-side ids of a submitted registration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationHandle {
    pub brand_id: Option<String>,
    pub campaign_id: Option<String>,
    pub tfv_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationStatus {
    pub state: RegState,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortStatus {
    pub id: String,
    /// The carrier's own status word.
    pub status: String,
    pub done: bool,
    pub failed: bool,
    /// The messaging profile the ported number will use (set at order creation).
    pub messaging_ref: Option<String>,
}

#[async_trait]
pub trait Carrier: Send + Sync {
    fn name(&self) -> &'static str;

    async fn search(&self, query: &SearchQuery) -> Result<Vec<AvailableNumber>, CarrierError>;
    async fn buy(&self, req: &BuyRequest) -> Result<BoughtNumber, CarrierError>;
    /// Give the number back. Idempotent: an already-gone number is success.
    async fn release(&self, e164: &str, carrier_number_id: Option<&str>, messaging_ref: Option<&str>) -> Result<(), CarrierError>;
    /// Point inbound messages at `url`.
    async fn set_messaging_webhook(&self, messaging_ref: &str, url: &str) -> Result<(), CarrierError>;
    async fn send_sms(&self, from: &str, to: &str, text: &str, messaging_ref: Option<&str>) -> Result<SentMessage, CarrierError>;

    /// Verify an inbound webhook and decode it. `url` is the public address the
    /// carrier posted to (Twilio signs it).
    fn parse_inbound(&self, headers: &HashMap<String, String>, url: &str, body: &[u8]) -> Result<InboundEvent, CarrierError>;

    async fn submit_registration(
        &self,
        kind: RegistrationKind,
        e164: &str,
        carrier_number_id: Option<&str>,
        messaging_ref: Option<&str>,
        form: &RegistrationForm,
    ) -> Result<RegistrationHandle, CarrierError>;
    async fn registration_status(&self, kind: RegistrationKind, e164: &str, handle: &RegistrationHandle, messaging_ref: Option<&str>) -> Result<RegistrationStatus, CarrierError>;
    /// File the 10DLC campaign for a brand that was still being verified at
    /// submit time. `Ok(None)` means the brand isn't ready yet; try again later.
    async fn file_pending_campaign(&self, brand_id: &str, form: &RegistrationForm) -> Result<Option<String>, CarrierError> {
        let _ = (brand_id, form);
        Ok(None)
    }

    /// Sole proprietor brands: (re)send the verification code to the brand's mobile number.
    async fn send_brand_otp(&self, brand_id: &str) -> Result<(), CarrierError> {
        let _ = brand_id;
        Err(CarrierError::Unsupported("sole proprietor verification"))
    }
    /// Sole proprietor brands: check the code the person received. `Ok(false)` = wrong code.
    async fn verify_brand_otp(&self, brand_id: &str, pin: &str) -> Result<bool, CarrierError> {
        let _ = (brand_id, pin);
        Err(CarrierError::Unsupported("sole proprietor verification"))
    }

    async fn port_in_create(&self, e164s: &[String], reference: &str, webhook_url: &str) -> Result<PortStatus, CarrierError>;
    async fn port_in_status(&self, order_id: &str) -> Result<PortStatus, CarrierError>;
}

/// Telnyx/TCR entity type for an individual with no EIN.
pub const SOLE_PROPRIETOR: &str = "SOLE_PROPRIETOR";

/// The configured carrier, or `NotConfigured`.
pub fn from_env(http: Arc<dyn CarrierHttp>) -> Result<Arc<dyn Carrier>, CarrierError> {
    match std::env::var("ALLTERNIT_PHONE_CARRIER").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "" | "telnyx" => telnyx::Telnyx::from_env(http).map(|c| Arc::new(c) as Arc<dyn Carrier>),
        "twilio" => twilio::Twilio::from_env(http).map(|c| Arc::new(c) as Arc<dyn Carrier>),
        other => Err(CarrierError::Invalid(format!("unknown carrier {other}"))),
    }
}

/// Public address carrier status webhooks (registration, port) are posted to.
/// The texts a number sends for opt-in, STOP, START and HELP. The campaign filed
/// with the carrier declares these same strings, so what people receive always
/// matches what was registered.
pub fn sender_name(f: &RegistrationForm) -> String {
    let n = if f.display_name.trim().is_empty() { f.legal_name.trim() } else { f.display_name.trim() };
    if n.is_empty() { "Allternit".to_string() } else { n.to_string() }
}

pub fn opt_in_text(name: &str) -> String {
    format!("{name}: Thanks for your message! You're now subscribed to replies from {name}'s AI assistant. Msg frequency varies. Msg & data rates may apply. Reply STOP to opt out, HELP for help. We will not share or sell your mobile information for marketing/promotional purposes.")
}

pub fn opt_out_text(name: &str) -> String {
    format!("{name}: You're unsubscribed and will get no more messages. Reply START to resubscribe.")
}

pub fn resubscribe_text(name: &str) -> String {
    format!("{name}: You're subscribed again. Msg frequency varies. Msg & data rates may apply. Reply STOP to opt out, HELP for help.")
}

pub fn help_text(name: &str, contact_email: &str) -> String {
    let contact = contact_email.trim();
    let reach = if contact.is_empty() { "visit allternit.com".to_string() } else { format!("email {contact}") };
    format!("{name}: This number is answered by an AI assistant. For help {reach}. Msg & data rates may apply. Reply STOP to opt out.")
}

/// Allternit's public opt-in page for a number: `GET /sms/{number_id}`.
pub fn hosted_opt_in_page_url(number_id: &str) -> String {
    let base = std::env::var("ALLTERNIT_CLOUD_API_URL").unwrap_or_else(|_| "https://api.allternit.com".to_string());
    format!("{}/sms/{number_id}", base.trim_end_matches('/'))
}

/// The carrier "message flow" (opt-in workflow): exactly one opt-in path, the
/// person texting the number first after seeing it on a public page. Telnyx
/// rejected a flow that also listed signed forms and verbal consent without a
/// form or script to verify (2026-10-05), so only this path is ever filed.
pub fn message_flow(f: &RegistrationForm) -> String {
    let name = sender_name(f);
    let page = f.opt_in_page_url.as_deref().map(str::trim).filter(|u| !u.is_empty()).unwrap_or("(opt-in page)");
    let mut flow = format!(
        "Opt-in is mobile-originated only. {name} publishes this number on a public page, {page}, which states that texting the number \
         starts a conversation with {name}'s AI assistant, that message frequency varies, that message and data rates may apply, \
         and that replying STOP opts out and HELP gets help. A person opts in by texting the number first; no one is texted before \
         they do. The first reply confirms the subscription with the opt-in message. STOP, STOPALL, UNSUBSCRIBE, CANCEL, END and QUIT \
         opt out immediately with one confirmation; START opts back in; HELP returns contact details."
    );
    if let Some(url) = f.terms_url.as_deref().filter(|u| !u.trim().is_empty()) {
        flow.push_str(&format!(" Terms: {}.", url.trim()));
    }
    if let Some(url) = f.privacy_policy_url.as_deref().filter(|u| !u.trim().is_empty()) {
        flow.push_str(&format!(" Privacy: {}.", url.trim()));
    }
    flow
}

pub fn status_webhook_url(carrier: &str) -> String {
    let base = std::env::var("ALLTERNIT_CLOUD_API_URL").unwrap_or_else(|_| "https://api.allternit.com".to_string());
    format!("{}/api/v1/phone/webhooks/{carrier}", base.trim_end_matches('/'))
}

/// `+` and 8–15 digits, no leading zero.
pub fn is_e164(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('+') else { return false };
    (8..=15).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_digit()) && !rest.starts_with('0')
}

/// Pull an error message out of a carrier error body without echoing request data.
pub(crate) fn error_message(body: &Value) -> String {
    let pick = |v: &Value| -> Option<String> {
        v.get("detail").or_else(|| v.get("title")).or_else(|| v.get("message")).or_else(|| v.get("description")).and_then(|m| m.as_str()).map(str::to_string)
    };
    body.get("errors")
        .and_then(|e| e.get(0))
        // Telnyx 10DLC answers some errors with a bare array.
        .or_else(|| body.get(0))
        .and_then(pick)
        .or_else(|| pick(body))
        .unwrap_or_else(|| "request failed".to_string())
}

pub(crate) fn ok_or_upstream(resp: HttpResp) -> Result<Value, CarrierError> {
    if (200..300).contains(&resp.status) {
        Ok(resp.body)
    } else {
        Err(CarrierError::Upstream(resp.status, error_message(&resp.body)))
    }
}

pub(crate) fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_str()
}

/// Test double: records requests, replays queued responses (default 200 `{}`).
#[cfg(test)]
pub mod fake {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeHttp {
        pub sent: Mutex<Vec<HttpReq>>,
        pub replies: Mutex<VecDeque<HttpResp>>,
    }

    impl FakeHttp {
        pub fn with(replies: Vec<(u16, Value)>) -> Arc<Self> {
            Arc::new(Self {
                sent: Mutex::new(vec![]),
                replies: Mutex::new(replies.into_iter().map(|(status, body)| HttpResp { status, body }).collect()),
            })
        }
        pub fn requests(&self) -> Vec<HttpReq> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CarrierHttp for FakeHttp {
        async fn send(&self, req: HttpReq) -> Result<HttpResp, CarrierError> {
            self.sent.lock().unwrap().push(req);
            Ok(self.replies.lock().unwrap().pop_front().unwrap_or(HttpResp { status: 200, body: serde_json::json!({}) }))
        }
    }
}
