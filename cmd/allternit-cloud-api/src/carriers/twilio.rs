//! Twilio adapter (fallback). Verified against Twilio's official OpenAPI specs
//! (https://github.com/twilio/twilio-oai, `spec/json/`) and
//! https://www.twilio.com/docs/usage/webhooks/webhooks-security :
//! - GET  api.twilio.com /2010-04-01/Accounts/{sid}/AvailablePhoneNumbers/{CC}/{Local|TollFree}.json
//! - POST …/IncomingPhoneNumbers.json (PhoneNumber), GET …?PhoneNumber=, DELETE …/IncomingPhoneNumbers/{sid}.json
//! - POST …/Messages.json (To, From | MessagingServiceSid, Body)
//! - POST messaging.twilio.com /v1/Services (FriendlyName, InboundRequestUrl, InboundMethod),
//!   POST /v1/Services/{sid} (InboundRequestUrl), POST /v1/Services/{sid}/PhoneNumbers (PhoneNumberSid)
//! - POST /v1/a2p/BrandRegistrations, POST/GET /v1/Services/{sid}/Compliance/Usa2p
//! - POST/GET /v1/Tollfree/Verifications
//! - Webhook: `X-Twilio-Signature` = base64(HMAC-SHA1(auth token, url + sorted key+value of form params)).
//!
//! Port-in is not offered here: Twilio's port-in API takes signed LOA documents,
//! so numbers are ported through the Twilio console or on Telnyx.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha1::Sha1;
use std::collections::HashMap;
use std::sync::Arc;

use super::*;

pub struct Twilio {
    http: Arc<dyn CarrierHttp>,
    sid: String,
    token: String,
}

impl Twilio {
    pub fn from_env(http: Arc<dyn CarrierHttp>) -> Result<Self, CarrierError> {
        let sid = std::env::var("ALLTERNIT_TWILIO_ACCOUNT_SID").unwrap_or_default();
        let token = std::env::var("ALLTERNIT_TWILIO_AUTH_TOKEN").unwrap_or_default();
        if sid.trim().is_empty() || token.trim().is_empty() {
            return Err(CarrierError::NotConfigured);
        }
        Ok(Self::new(http, sid, token))
    }

    pub fn new(http: Arc<dyn CarrierHttp>, sid: String, token: String) -> Self {
        Self { http, sid, token }
    }

    fn acct(&self, tail: &str) -> String {
        format!("https://api.twilio.com/2010-04-01/Accounts/{}/{}", self.sid, tail)
    }

    async fn call(&self, method: &'static str, url: String, form: Option<Vec<(String, String)>>) -> Result<Value, CarrierError> {
        let resp = self.http.send(HttpReq { method, url, basic: Some((self.sid.clone(), self.token.clone())), form, ..Default::default() }).await?;
        if resp.status == 204 {
            return Ok(Value::Null);
        }
        ok_or_upstream(resp)
    }
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_form(body: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(body)
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

/// base64(HMAC-SHA1(token, url + params sorted by key, each key then value)).
pub fn twilio_signature(token: &str, url: &str, params: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut data = url.to_string();
    for (k, v) in sorted {
        data.push_str(k);
        data.push_str(v);
    }
    let mut mac = Hmac::<Sha1>::new_from_slice(token.as_bytes()).expect("hmac takes any key length");
    mac.update(data.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

fn state_of_campaign(status: &str) -> RegState {
    match status {
        "VERIFIED" => RegState::Approved,
        "FAILED" => RegState::Rejected,
        _ => RegState::Pending,
    }
}

#[async_trait]
impl Carrier for Twilio {
    fn name(&self) -> &'static str {
        "twilio"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<AvailableNumber>, CarrierError> {
        let kind = if q.kind == NumberType::TollFree { "TollFree" } else { "Local" };
        let mut url = format!("{}?SmsEnabled=true&PageSize={}", self.acct(&format!("AvailablePhoneNumbers/{}/{kind}.json", urlencoding::encode(&q.country))), q.limit.clamp(1, 50));
        if let Some(area) = q.area_code.as_deref().filter(|a| !a.is_empty()) {
            url.push_str(&format!("&AreaCode={}", urlencoding::encode(area)));
        }
        if let Some(locality) = q.locality.as_deref().filter(|l| !l.is_empty()) {
            url.push_str(&format!("&InLocality={}", urlencoding::encode(locality)));
        }
        let body = self.call("GET", url, None).await?;
        Ok(body
            .get("available_phone_numbers")
            .and_then(|a| a.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|n| {
                        Some(AvailableNumber {
                            e164: str_at(n, &["phone_number"])?.to_string(),
                            kind: q.kind.as_str().to_string(),
                            locality: str_at(n, &["locality"]).map(str::to_string),
                            monthly_cost: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn buy(&self, req: &BuyRequest) -> Result<BoughtNumber, CarrierError> {
        let service = self
            .call(
                "POST",
                "https://messaging.twilio.com/v1/Services".into(),
                Some(pairs(&[("FriendlyName", &format!("allternit-{}", req.reference)), ("InboundRequestUrl", &req.webhook_url), ("InboundMethod", "POST"), ("UseInboundWebhookOnNumber", "false")])),
            )
            .await?;
        let service_sid = str_at(&service, &["sid"]).ok_or_else(|| CarrierError::Upstream(502, "messaging service has no sid".into()))?.to_string();
        let cleanup = |sid: String| async move {
            let _ = self.call("DELETE", format!("https://messaging.twilio.com/v1/Services/{sid}"), None).await;
        };
        let number = match self.call("POST", self.acct("IncomingPhoneNumbers.json"), Some(pairs(&[("PhoneNumber", &req.e164), ("FriendlyName", &format!("allternit-{}", req.reference))]))).await {
            Ok(n) => n,
            Err(e) => {
                cleanup(service_sid).await;
                return Err(e);
            }
        };
        let number_sid = str_at(&number, &["sid"]).ok_or_else(|| CarrierError::Upstream(502, "number has no sid".into()))?.to_string();
        if let Err(e) = self.call("POST", format!("https://messaging.twilio.com/v1/Services/{service_sid}/PhoneNumbers"), Some(pairs(&[("PhoneNumberSid", &number_sid)]))).await {
            let _ = self.release(&req.e164, Some(&number_sid), None).await;
            cleanup(service_sid).await;
            return Err(e);
        }
        Ok(BoughtNumber { carrier_number_id: Some(number_sid), messaging_ref: Some(service_sid) })
    }

    async fn release(&self, e164: &str, carrier_number_id: Option<&str>, messaging_ref: Option<&str>) -> Result<(), CarrierError> {
        let sid = match carrier_number_id {
            Some(s) => Some(s.to_string()),
            None => self
                .call("GET", format!("{}?PhoneNumber={}", self.acct("IncomingPhoneNumbers.json"), urlencoding::encode(e164)), None)
                .await?
                .get("incoming_phone_numbers")
                .and_then(|n| n.get(0))
                .and_then(|n| str_at(n, &["sid"]))
                .map(str::to_string),
        };
        if let Some(sid) = sid {
            match self.call("DELETE", self.acct(&format!("IncomingPhoneNumbers/{sid}.json")), None).await {
                Ok(_) | Err(CarrierError::Upstream(404, _)) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(service) = messaging_ref {
            let _ = self.call("DELETE", format!("https://messaging.twilio.com/v1/Services/{service}"), None).await;
        }
        Ok(())
    }

    async fn set_messaging_webhook(&self, messaging_ref: &str, url: &str) -> Result<(), CarrierError> {
        self.call("POST", format!("https://messaging.twilio.com/v1/Services/{messaging_ref}"), Some(pairs(&[("InboundRequestUrl", url), ("InboundMethod", "POST")]))).await?;
        Ok(())
    }

    async fn send_sms(&self, from: &str, to: &str, text: &str, _messaging_ref: Option<&str>) -> Result<SentMessage, CarrierError> {
        let resp = self.call("POST", self.acct("Messages.json"), Some(pairs(&[("To", to), ("From", from), ("Body", text)]))).await?;
        let id = str_at(&resp, &["sid"]).ok_or_else(|| CarrierError::Upstream(502, "message has no sid".into()))?.to_string();
        let parts = str_at(&resp, &["num_segments"]).and_then(|n| n.parse().ok()).unwrap_or(1);
        Ok(SentMessage { id, parts })
    }

    fn parse_inbound(&self, headers: &HashMap<String, String>, url: &str, body: &[u8]) -> Result<InboundEvent, CarrierError> {
        let given = headers.get("x-twilio-signature").ok_or(CarrierError::BadSignature)?;
        let params = parse_form(body);
        let expected = twilio_signature(&self.token, url, &params);
        // Constant-time compare via the MAC would be ideal; both are fixed-length base64 of a keyed hash.
        let (a, b) = (expected.as_bytes(), given.trim().as_bytes());
        if a.len() != b.len() || a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) != 0 {
            return Err(CarrierError::BadSignature);
        }
        let get = |k: &str| params.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        match (get("MessageSid"), get("From"), get("To"), get("Body")) {
            (Some(id), Some(from), Some(to), Some(text)) if get("SmsStatus").map_or(true, |s| s == "received") => Ok(InboundEvent::Message { id, from, to, text }),
            _ => Ok(InboundEvent::Ignored),
        }
    }

    async fn submit_registration(
        &self,
        kind: RegistrationKind,
        e164: &str,
        carrier_number_id: Option<&str>,
        messaging_ref: Option<&str>,
        f: &RegistrationForm,
    ) -> Result<RegistrationHandle, CarrierError> {
        match kind {
            RegistrationKind::TenDlc => {
                let service = messaging_ref.ok_or_else(|| CarrierError::Invalid("number has no messaging service".into()))?;
                let (Some(profile), Some(a2p)) = (f.twilio_customer_profile_sid.as_deref(), f.twilio_a2p_profile_sid.as_deref()) else {
                    return Err(CarrierError::Invalid("missing fields: twilioCustomerProfileSid, twilioA2pProfileSid".into()));
                };
                if f.sample_messages.len() < 2 || f.use_case.is_empty() || f.use_case_summary.is_empty() {
                    return Err(CarrierError::Invalid("missing fields: useCase, useCaseSummary, sampleMessages (two or more)".into()));
                }
                let brand = self
                    .call("POST", "https://messaging.twilio.com/v1/a2p/BrandRegistrations".into(), Some(pairs(&[("CustomerProfileBundleSid", profile), ("A2PProfileBundleSid", a2p)])))
                    .await?;
                let brand_sid = str_at(&brand, &["sid"]).ok_or_else(|| CarrierError::Upstream(502, "brand has no sid".into()))?.to_string();
                let flow = super::message_flow(f);
                let mut form = pairs(&[
                    ("BrandRegistrationSid", &brand_sid),
                    ("Description", &f.use_case_summary),
                    ("MessageFlow", &flow),
                    ("UsAppToPersonUsecase", &f.use_case),
                    ("HasEmbeddedLinks", "false"),
                    ("HasEmbeddedPhone", "false"),
                ]);
                for sample in &f.sample_messages {
                    form.push(("MessageSamples".into(), sample.clone()));
                }
                let campaign = self.call("POST", format!("https://messaging.twilio.com/v1/Services/{service}/Compliance/Usa2p"), Some(form)).await?;
                Ok(RegistrationHandle { brand_id: Some(brand_sid), campaign_id: str_at(&campaign, &["sid"]).map(str::to_string), tfv_id: None })
            }
            RegistrationKind::TollFree => {
                let number_sid = carrier_number_id.ok_or_else(|| CarrierError::Invalid("number has no carrier id".into()))?;
                let _ = e164;
                for (name, value) in [("legalName", &f.legal_name), ("website", &f.website), ("contactEmail", &f.contact_email), ("useCaseSummary", &f.use_case_summary), ("messageVolume", &f.message_volume), ("useCase", &f.use_case)] {
                    if value.trim().is_empty() {
                        return Err(CarrierError::Invalid(format!("missing fields: {name}")));
                    }
                }
                if f.sample_messages.is_empty() || f.opt_in_image_urls.is_empty() {
                    return Err(CarrierError::Invalid("missing fields: sampleMessages, optInImageUrls".into()));
                }
                let mut form = pairs(&[
                    ("BusinessName", &f.legal_name),
                    ("BusinessWebsite", &f.website),
                    ("NotificationEmail", &f.contact_email),
                    ("UseCaseSummary", &f.use_case_summary),
                    ("ProductionMessageSample", &f.sample_messages[0]),
                    ("OptInType", "VIA_TEXT"),
                    ("MessageVolume", &f.message_volume),
                    ("TollfreePhoneNumberSid", number_sid),
                    ("BusinessStreetAddress", &f.street),
                    ("BusinessCity", &f.city),
                    ("BusinessStateProvinceRegion", &f.state),
                    ("BusinessPostalCode", &f.postal_code),
                    ("BusinessCountry", &f.country),
                    ("BusinessContactFirstName", &f.contact_first_name),
                    ("BusinessContactLastName", &f.contact_last_name),
                    ("BusinessContactEmail", &f.contact_email),
                    ("BusinessContactPhone", &f.contact_phone),
                ]);
                form.push(("UseCaseCategories".into(), f.use_case.clone()));
                for url in &f.opt_in_image_urls {
                    form.push(("OptInImageUrls".into(), url.clone()));
                }
                let resp = self.call("POST", "https://messaging.twilio.com/v1/Tollfree/Verifications".into(), Some(form)).await?;
                Ok(RegistrationHandle { tfv_id: str_at(&resp, &["sid"]).map(str::to_string), ..Default::default() })
            }
        }
    }

    async fn registration_status(&self, kind: RegistrationKind, _e164: &str, h: &RegistrationHandle, messaging_ref: Option<&str>) -> Result<RegistrationStatus, CarrierError> {
        match kind {
            RegistrationKind::TenDlc => {
                if let Some(brand) = h.brand_id.as_deref() {
                    let b = self.call("GET", format!("https://messaging.twilio.com/v1/a2p/BrandRegistrations/{brand}"), None).await?;
                    if str_at(&b, &["status"]) == Some("FAILED") {
                        return Ok(RegistrationStatus { state: RegState::Rejected, reason: Some(str_at(&b, &["failure_reason"]).unwrap_or("brand registration failed").to_string()) });
                    }
                }
                let service = messaging_ref.ok_or_else(|| CarrierError::Invalid("number has no messaging service".into()))?;
                let r = self.call("GET", format!("https://messaging.twilio.com/v1/Services/{service}/Compliance/Usa2p"), None).await?;
                let status = r.get("compliance").and_then(|c| c.get(0)).and_then(|c| str_at(c, &["campaign_status"])).unwrap_or("IN_PROGRESS");
                let state = state_of_campaign(status);
                let reason = (state == RegState::Rejected).then(|| r.get("compliance").and_then(|c| c.get(0)).and_then(|c| c.get("errors")).map(|e| e.to_string()).unwrap_or_else(|| "campaign failed".into()));
                Ok(RegistrationStatus { state, reason })
            }
            RegistrationKind::TollFree => {
                let id = h.tfv_id.as_deref().ok_or_else(|| CarrierError::Invalid("no verification id".into()))?;
                let r = self.call("GET", format!("https://messaging.twilio.com/v1/Tollfree/Verifications/{id}"), None).await?;
                Ok(match str_at(&r, &["status"]).unwrap_or("") {
                    "TWILIO_APPROVED" => RegistrationStatus { state: RegState::Approved, reason: None },
                    "TWILIO_REJECTED" => RegistrationStatus { state: RegState::Rejected, reason: Some(str_at(&r, &["rejection_reason"]).unwrap_or("rejected").to_string()) },
                    _ => RegistrationStatus { state: RegState::Pending, reason: None },
                })
            }
        }
    }

    async fn port_in_create(&self, _e164s: &[String], _reference: &str, _webhook_url: &str) -> Result<PortStatus, CarrierError> {
        Err(CarrierError::Unsupported("port-in"))
    }

    async fn port_in_status(&self, _order_id: &str) -> Result<PortStatus, CarrierError> {
        Err(CarrierError::Unsupported("port-in"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriers::fake::FakeHttp;
    use serde_json::json;

    /// Twilio's documented example: https://www.twilio.com/docs/usage/security#validating-requests
    #[test]
    fn signature_matches_twilios_published_example() {
        let params = vec![
            ("CallSid".to_string(), "CA1234567890ABCDE".to_string()),
            ("Caller".to_string(), "+14158675310".to_string()),
            ("Digits".to_string(), "1234".to_string()),
            ("From".to_string(), "+14158675310".to_string()),
            ("To".to_string(), "+18005551212".to_string()),
        ];
        assert_eq!(twilio_signature("12345", "https://mycompany.com/myapp.php?foo=1&bar=2", &params), "GvWf1cFY/Q7PnoempGyD5oXAezc=");
    }

    #[test]
    fn verifies_inbound_form() {
        let t = Twilio::new(FakeHttp::with(vec![]), "AC1".into(), "tok".into());
        let body = "MessageSid=SM1&From=%2B15551230000&To=%2B15559870000&Body=hello+there&SmsStatus=received";
        let url = "https://api.allternit.com/channels/in/abc";
        let sig = twilio_signature("tok", url, &parse_form(body.as_bytes()));
        let headers = HashMap::from([("x-twilio-signature".to_string(), sig)]);
        assert_eq!(
            t.parse_inbound(&headers, url, body.as_bytes()).unwrap(),
            InboundEvent::Message { id: "SM1".into(), from: "+15551230000".into(), to: "+15559870000".into(), text: "hello there".into() }
        );
        assert_eq!(t.parse_inbound(&headers, url, b"MessageSid=SM1&Body=tampered"), Err(CarrierError::BadSignature));
        assert_eq!(t.parse_inbound(&HashMap::new(), url, body.as_bytes()), Err(CarrierError::BadSignature));
    }

    #[tokio::test]
    async fn buy_makes_service_number_and_link() {
        let http = FakeHttp::with(vec![(201, json!({"sid":"MG1"})), (201, json!({"sid":"PN1"})), (201, json!({"sid":"PN1"}))]);
        let t = Twilio::new(http.clone(), "AC1".into(), "tok".into());
        let bought = t.buy(&BuyRequest { e164: "+14155550101".into(), kind: NumberType::Local, webhook_url: "https://api/x".into(), reference: "n1".into() }).await.unwrap();
        assert_eq!(bought, BoughtNumber { carrier_number_id: Some("PN1".into()), messaging_ref: Some("MG1".into()) });
        let reqs = http.requests();
        assert!(reqs[0].form.as_ref().unwrap().contains(&("InboundRequestUrl".to_string(), "https://api/x".to_string())));
        assert_eq!(reqs[0].basic, Some(("AC1".to_string(), "tok".to_string())));
        assert!(reqs[2].url.ends_with("/Services/MG1/PhoneNumbers"));
    }

    #[tokio::test]
    async fn send_and_port_unsupported() {
        let http = FakeHttp::with(vec![(201, json!({"sid":"SM9","num_segments":"2"}))]);
        let t = Twilio::new(http, "AC1".into(), "tok".into());
        let sent = t.send_sms("+1555", "+1666", "hi", None).await.unwrap();
        assert_eq!(sent, SentMessage { id: "SM9".into(), parts: 2 });
        assert_eq!(t.port_in_create(&[], "x", "u").await, Err(CarrierError::Unsupported("port-in")));
    }
}
