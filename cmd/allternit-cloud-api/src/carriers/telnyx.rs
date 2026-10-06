//! Telnyx adapter (API v2). Verified against the official OpenAPI spec
//! (https://github.com/team-telnyx/openapi, `openapi/spec3.json`) and
//! https://developers.telnyx.com/docs/messaging/messages/receiving-webhooks :
//! - GET  /available_phone_numbers (filter[country_code|national_destination_code|locality|phone_number_type|limit])
//! - POST /messaging_profiles, PATCH /messaging_profiles/{id}, DELETE /messaging_profiles/{id}
//! - POST /number_orders {phone_numbers:[{phone_number}], messaging_profile_id, customer_reference}
//! - GET  /phone_numbers?filter[phone_number]=, DELETE /phone_numbers/{id}
//! - POST /messages {from,to,text,type,messaging_profile_id}
//! - POST /10dlc/brand, POST /10dlc/campaignBuilder, GET /10dlc/brand/{id},
//!   GET /10dlc/campaign/{id}, POST /10dlc/phone_number_campaigns
//! - POST /messaging_tollfree/verification/requests, GET …/requests/{id}
//! - POST /porting_orders, GET /porting_orders/{id}
//! - Webhook: `telnyx-signature-ed25519` (base64) over `{telnyx-timestamp}|{raw body}`,
//!   public key base64; event `data.event_type = message.received`,
//!   `data.payload.{id, direction, from.phone_number, to[0].phone_number, text}`.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use super::*;

const BASE: &str = "https://api.telnyx.com/v2";
/// A signed webhook older than this is a replay.
const MAX_SKEW_SECS: i64 = 300;

pub struct Telnyx {
    http: Arc<dyn CarrierHttp>,
    api_key: String,
    public_key: Option<VerifyingKey>,
    base: String,
}

impl Telnyx {
    pub fn from_env(http: Arc<dyn CarrierHttp>) -> Result<Self, CarrierError> {
        let api_key = std::env::var("ALLTERNIT_TELNYX_API_KEY").unwrap_or_default();
        if api_key.trim().is_empty() {
            return Err(CarrierError::NotConfigured);
        }
        let public_key = std::env::var("ALLTERNIT_TELNYX_PUBLIC_KEY").ok().filter(|k| !k.trim().is_empty());
        Self::new(http, api_key, public_key.as_deref())
    }

    pub fn new(http: Arc<dyn CarrierHttp>, api_key: String, public_key_b64: Option<&str>) -> Result<Self, CarrierError> {
        let public_key = match public_key_b64 {
            Some(b64) => {
                let bytes = STANDARD.decode(b64.trim()).map_err(|_| CarrierError::Invalid("ALLTERNIT_TELNYX_PUBLIC_KEY is not base64".into()))?;
                let bytes: [u8; 32] = bytes.try_into().map_err(|_| CarrierError::Invalid("ALLTERNIT_TELNYX_PUBLIC_KEY is not a 32-byte key".into()))?;
                Some(VerifyingKey::from_bytes(&bytes).map_err(|_| CarrierError::Invalid("ALLTERNIT_TELNYX_PUBLIC_KEY is not a valid ed25519 key".into()))?)
            }
            None => None,
        };
        Ok(Self { http, api_key, public_key, base: BASE.to_string() })
    }

    async fn call(&self, method: &'static str, path: &str, json: Option<Value>) -> Result<Value, CarrierError> {
        let resp = self
            .http
            .send(HttpReq { method, url: format!("{}{}", self.base, path), bearer: Some(self.api_key.clone()), json, ..Default::default() })
            .await?;
        ok_or_upstream(resp)
    }
}

fn enc(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn registration_state_from_campaign(campaign: &Value) -> RegistrationStatus {
    let status = str_at(campaign, &["campaignStatus"]).unwrap_or("");
    let reason = || {
        campaign.get("failureReasons").and_then(|r| match r {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        })
    };
    match status {
        "MNO_PROVISIONED" | "MNO_ACCEPTED" => RegistrationStatus { state: RegState::Approved, reason: None },
        "TCR_FAILED" | "TELNYX_FAILED" | "MNO_REJECTED" | "MNO_PROVISIONING_FAILED" | "TCR_SUSPENDED" | "TCR_EXPIRED" => {
            RegistrationStatus { state: RegState::Rejected, reason: reason().or_else(|| Some(status.to_string())) }
        }
        _ => RegistrationStatus { state: RegState::Pending, reason: None },
    }
}

/// The campaignBuilder body for a 10DLC registration.
fn campaign_body(brand_id: &str, f: &RegistrationForm) -> Value {
    let name = super::sender_name(f);
    let mut campaign = json!({
        "brandId": brand_id, "usecase": f.use_case, "description": f.use_case_summary,
        "messageFlow": super::message_flow(f),
        "optinKeywords": "START", "optoutKeywords": "STOP,STOPALL,UNSUBSCRIBE,CANCEL,END,QUIT", "helpKeywords": "HELP",
        "optinMessage": super::opt_in_text(&name),
        "optoutMessage": super::opt_out_text(&name),
        "helpMessage": super::help_text(&name, &f.contact_email),
        "subscriberOptin": true, "subscriberOptout": true, "subscriberHelp": true,
        // Assistant replies can carry links (shared files) and phone numbers.
        "embeddedLink": true, "embeddedPhone": true,
        "numberPool": false, "ageGated": false, "directLending": false,
        "termsAndConditions": true, "autoRenewal": true,
        "webhookURL": super::status_webhook_url("telnyx"),
    });
    for (i, sample) in f.sample_messages.iter().take(5).enumerate() {
        campaign[format!("sample{}", i + 1)] = json!(sample);
    }
    if let Some(url) = &f.privacy_policy_url {
        campaign["privacyPolicyLink"] = json!(url);
    }
    if let Some(url) = &f.terms_url {
        campaign["termsAndConditionsLink"] = json!(url);
    }
    campaign
}

/// Telnyx refuses a campaign while its brand is still being verified
/// (campaignBuilder 400, code 527 "Brand registration status pending").
fn brand_not_ready(e: &CarrierError) -> bool {
    matches!(e, CarrierError::Upstream(400, msg) if {
        let m = msg.to_ascii_lowercase();
        m.contains("registration status pending") || m.contains("otp") || m.contains("not verified")
    })
}

fn require(form: &RegistrationForm, pairs: &[(&str, &str)]) -> Result<(), CarrierError> {
    let _ = form;
    let missing: Vec<&str> = pairs.iter().filter(|(_, v)| v.trim().is_empty()).map(|(k, _)| *k).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(CarrierError::Invalid(format!("missing fields: {}", missing.join(", "))))
    }
}

#[async_trait]
impl Carrier for Telnyx {
    fn name(&self) -> &'static str {
        "telnyx"
    }

    async fn search(&self, q: &SearchQuery) -> Result<Vec<AvailableNumber>, CarrierError> {
        let mut path = format!(
            "/available_phone_numbers?filter[country_code]={}&filter[phone_number_type]={}&filter[features][]=sms&filter[limit]={}",
            enc(&q.country),
            q.kind.as_str(),
            q.limit.clamp(1, 50)
        );
        if let Some(area) = q.area_code.as_deref().filter(|a| !a.is_empty()) {
            path.push_str(&format!("&filter[national_destination_code]={}", enc(area)));
        }
        if let Some(locality) = q.locality.as_deref().filter(|l| !l.is_empty()) {
            path.push_str(&format!("&filter[locality]={}", enc(locality)));
        }
        let body = self.call("GET", &path, None).await?;
        Ok(body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|n| {
                        Some(AvailableNumber {
                            e164: str_at(n, &["phone_number"])?.to_string(),
                            kind: q.kind.as_str().to_string(),
                            locality: n.get("region_information").and_then(|r| r.as_array()).and_then(|r| {
                                r.iter().find(|x| str_at(x, &["region_type"]) == Some("rate_center")).and_then(|x| str_at(x, &["region_name"]).map(str::to_string))
                            }),
                            monthly_cost: str_at(n, &["cost_information", "monthly_cost"]).map(str::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn buy(&self, req: &BuyRequest) -> Result<BoughtNumber, CarrierError> {
        let profile = self
            .call(
                "POST",
                "/messaging_profiles",
                Some(json!({
                    "name": format!("allternit-{}", req.reference),
                    "enabled": true,
                    "whitelisted_destinations": ["US", "CA"],
                    "webhook_url": req.webhook_url,
                    "webhook_api_version": "2",
                })),
            )
            .await?;
        let profile_id = str_at(&profile, &["data", "id"]).ok_or_else(|| CarrierError::Upstream(502, "messaging profile has no id".into()))?.to_string();
        let order = self
            .call(
                "POST",
                "/number_orders",
                Some(json!({
                    "phone_numbers": [{ "phone_number": req.e164 }],
                    "messaging_profile_id": profile_id,
                    "customer_reference": req.reference,
                })),
            )
            .await;
        let failed = match &order {
            Ok(body) => str_at(body, &["data", "status"]) == Some("failure"),
            Err(_) => true,
        };
        if failed {
            let _ = self.call("DELETE", &format!("/messaging_profiles/{profile_id}"), None).await;
            return Err(order.err().unwrap_or_else(|| CarrierError::Upstream(409, "number order failed".into())));
        }
        // The number's own id (needed to release it). It may not exist yet if the order is still pending.
        let carrier_number_id = self
            .call("GET", &format!("/phone_numbers?filter[phone_number]={}", enc(&req.e164)), None)
            .await
            .ok()
            .and_then(|b| b.get("data").and_then(|d| d.get(0)).and_then(|n| str_at(n, &["id"])).map(str::to_string));
        Ok(BoughtNumber { carrier_number_id, messaging_ref: Some(profile_id) })
    }

    async fn release(&self, e164: &str, carrier_number_id: Option<&str>, messaging_ref: Option<&str>) -> Result<(), CarrierError> {
        let id = match carrier_number_id {
            Some(id) => Some(id.to_string()),
            None => self
                .call("GET", &format!("/phone_numbers?filter[phone_number]={}", enc(e164)), None)
                .await?
                .get("data")
                .and_then(|d| d.get(0))
                .and_then(|n| str_at(n, &["id"]))
                .map(str::to_string),
        };
        if let Some(id) = id {
            match self.call("DELETE", &format!("/phone_numbers/{id}"), None).await {
                Ok(_) | Err(CarrierError::Upstream(404, _)) => {}
                Err(e) => return Err(e),
            }
        }
        if let Some(profile) = messaging_ref {
            let _ = self.call("DELETE", &format!("/messaging_profiles/{profile}"), None).await;
        }
        Ok(())
    }

    async fn set_messaging_webhook(&self, messaging_ref: &str, url: &str) -> Result<(), CarrierError> {
        self.call("PATCH", &format!("/messaging_profiles/{messaging_ref}"), Some(json!({ "webhook_url": url, "webhook_api_version": "2" }))).await?;
        Ok(())
    }

    async fn send_sms(&self, from: &str, to: &str, text: &str, messaging_ref: Option<&str>) -> Result<SentMessage, CarrierError> {
        let mut body = json!({ "from": from, "to": to, "text": text, "type": "SMS" });
        if let Some(profile) = messaging_ref {
            body["messaging_profile_id"] = json!(profile);
        }
        let resp = self.call("POST", "/messages", Some(body)).await?;
        let id = str_at(&resp, &["data", "id"]).ok_or_else(|| CarrierError::Upstream(502, "message has no id".into()))?.to_string();
        let parts = resp.get("data").and_then(|d| d.get("parts")).and_then(|p| p.as_u64()).unwrap_or(1) as u32;
        Ok(SentMessage { id, parts })
    }

    fn parse_inbound(&self, headers: &HashMap<String, String>, _url: &str, body: &[u8]) -> Result<InboundEvent, CarrierError> {
        let key = self.public_key.as_ref().ok_or(CarrierError::NotConfigured)?;
        let sig = headers.get("telnyx-signature-ed25519").ok_or(CarrierError::BadSignature)?;
        let timestamp = headers.get("telnyx-timestamp").ok_or(CarrierError::BadSignature)?;
        let sig_bytes = STANDARD.decode(sig.trim()).map_err(|_| CarrierError::BadSignature)?;
        let sig = Signature::from_slice(&sig_bytes).map_err(|_| CarrierError::BadSignature)?;
        let mut signed = Vec::with_capacity(timestamp.len() + 1 + body.len());
        signed.extend_from_slice(timestamp.as_bytes());
        signed.push(b'|');
        signed.extend_from_slice(body);
        key.verify(&signed, &sig).map_err(|_| CarrierError::BadSignature)?;
        let at: i64 = timestamp.trim().parse().map_err(|_| CarrierError::BadSignature)?;
        if (chrono::Utc::now().timestamp() - at).abs() > MAX_SKEW_SECS {
            return Err(CarrierError::BadSignature);
        }
        let event: Value = serde_json::from_slice(body).map_err(|_| CarrierError::Invalid("webhook body is not JSON".into()))?;
        if str_at(&event, &["data", "event_type"]) != Some("message.received") {
            return Ok(InboundEvent::Ignored);
        }
        let payload = event.get("data").and_then(|d| d.get("payload")).ok_or_else(|| CarrierError::Invalid("no payload".into()))?;
        if let Some(direction) = str_at(payload, &["direction"]) {
            if direction != "inbound" {
                return Ok(InboundEvent::Ignored);
            }
        }
        let field = |v: Option<&str>, what: &str| v.map(str::to_string).ok_or_else(|| CarrierError::Invalid(format!("webhook has no {what}")));
        Ok(InboundEvent::Message {
            id: field(str_at(payload, &["id"]), "message id")?,
            from: field(str_at(payload, &["from", "phone_number"]), "sender")?,
            to: field(payload.get("to").and_then(|t| t.get(0)).and_then(|t| str_at(t, &["phone_number"])), "recipient")?,
            text: str_at(payload, &["text"]).unwrap_or("").to_string(),
        })
    }

    async fn submit_registration(
        &self,
        kind: RegistrationKind,
        e164: &str,
        _carrier_number_id: Option<&str>,
        _messaging_ref: Option<&str>,
        f: &RegistrationForm,
    ) -> Result<RegistrationHandle, CarrierError> {
        match kind {
            RegistrationKind::TenDlc => {
                require(
                    f,
                    &[
                        ("entityType", &f.entity_type),
                        ("displayName", &f.display_name),
                        ("contactEmail", &f.contact_email),
                        ("vertical", &f.vertical),
                        ("country", &f.country),
                        ("useCase", &f.use_case),
                        ("useCaseSummary", &f.use_case_summary),
                    ],
                )?;
                if f.sample_messages.is_empty() {
                    return Err(CarrierError::Invalid("missing fields: sampleMessages".into()));
                }
                let sole = f.entity_type == super::SOLE_PROPRIETOR;
                let mobile = f.mobile_phone.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| f.contact_phone.clone());
                if sole {
                    require(f, &[("contactFirstName", &f.contact_first_name), ("contactLastName", &f.contact_last_name)])?;
                    if !super::is_e164(&mobile) {
                        return Err(CarrierError::Invalid("missing fields: mobilePhone (a mobile number in +1… form; the carrier texts it a code)".into()));
                    }
                }
                let mut brand_body = json!({
                    "entityType": f.entity_type, "displayName": f.display_name, "companyName": f.legal_name,
                    "phone": f.contact_phone, "street": f.street, "city": f.city, "state": f.state,
                    "postalCode": f.postal_code, "country": f.country, "email": f.contact_email,
                    "website": f.website, "vertical": f.vertical, "webhookURL": super::status_webhook_url("telnyx"),
                });
                if sole {
                    // An individual: no EIN; the carrier verifies the person by a code to their mobile.
                    brand_body["firstName"] = json!(f.contact_first_name);
                    brand_body["lastName"] = json!(f.contact_last_name);
                    brand_body["mobilePhone"] = json!(mobile);
                } else {
                    brand_body["ein"] = json!(f.ein);
                }
                let brand = self.call("POST", "/10dlc/brand", Some(brand_body)).await?;
                let brand_id = str_at(&brand, &["brandId"]).ok_or_else(|| CarrierError::Upstream(502, "brand has no id".into()))?.to_string();
                if sole {
                    // The campaign waits for the code; the status refresh files it once verified.
                    self.send_brand_otp(&brand_id).await?;
                    return Ok(RegistrationHandle { brand_id: Some(brand_id), campaign_id: None, tfv_id: None });
                }
                let _ = e164;
                // A new brand is still being verified, and the carrier refuses
                // campaigns until it is. Keep the brand and file the campaign
                // from the status refresh once it clears.
                let campaign_id = match self.call("POST", "/10dlc/campaignBuilder", Some(campaign_body(&brand_id, f))).await {
                    Ok(campaign) => str_at(&campaign, &["campaignId"]).map(str::to_string),
                    Err(e) if brand_not_ready(&e) => None,
                    Err(e) => return Err(e),
                };
                Ok(RegistrationHandle { brand_id: Some(brand_id), campaign_id, tfv_id: None })
            }
            RegistrationKind::TollFree => {
                require(
                    f,
                    &[
                        ("legalName", &f.legal_name),
                        ("website", &f.website),
                        ("street", &f.street),
                        ("city", &f.city),
                        ("state", &f.state),
                        ("postalCode", &f.postal_code),
                        ("contactFirstName", &f.contact_first_name),
                        ("contactLastName", &f.contact_last_name),
                        ("contactEmail", &f.contact_email),
                        ("contactPhone", &f.contact_phone),
                        ("messageVolume", &f.message_volume),
                        ("useCase", &f.use_case),
                        ("useCaseSummary", &f.use_case_summary),
                    ],
                )?;
                if f.sample_messages.is_empty() || f.opt_in_image_urls.is_empty() {
                    return Err(CarrierError::Invalid("missing fields: sampleMessages, optInImageUrls".into()));
                }
                let mut body = json!({
                    "businessName": f.legal_name, "corporateWebsite": f.website,
                    "businessAddr1": f.street, "businessCity": f.city, "businessState": f.state, "businessZip": f.postal_code,
                    "businessContactFirstName": f.contact_first_name, "businessContactLastName": f.contact_last_name,
                    "businessContactEmail": f.contact_email, "businessContactPhone": f.contact_phone,
                    "messageVolume": f.message_volume, "phoneNumbers": [{ "phoneNumber": e164 }],
                    "useCase": f.use_case, "useCaseSummary": f.use_case_summary,
                    "productionMessageContent": f.sample_messages.join("\n"),
                    "optInWorkflow": super::message_flow(f),
                    "optInWorkflowImageURLs": f.opt_in_image_urls.iter().map(|u| json!({ "url": u })).collect::<Vec<_>>(),
                    "additionalInformation": f.use_case_summary, "webhookUrl": super::status_webhook_url("telnyx"),
                });
                if !f.entity_type.is_empty() {
                    body["entityType"] = json!(f.entity_type);
                }
                if let Some(ein) = &f.ein {
                    body["businessRegistrationNumber"] = json!(ein);
                    body["businessRegistrationType"] = json!("EIN");
                    body["businessRegistrationCountry"] = json!(f.country);
                }
                if let Some(url) = &f.privacy_policy_url {
                    body["privacyPolicyURL"] = json!(url);
                }
                if let Some(url) = &f.terms_url {
                    body["termsAndConditionURL"] = json!(url);
                }
                let resp = self.call("POST", "/messaging_tollfree/verification/requests", Some(body)).await?;
                let id = str_at(&resp, &["id"]).or_else(|| str_at(&resp, &["verificationRequestId"])).ok_or_else(|| CarrierError::Upstream(502, "verification has no id".into()))?;
                Ok(RegistrationHandle { tfv_id: Some(id.to_string()), ..Default::default() })
            }
        }
    }

    async fn registration_status(&self, kind: RegistrationKind, e164: &str, h: &RegistrationHandle, _messaging_ref: Option<&str>) -> Result<RegistrationStatus, CarrierError> {
        match kind {
            RegistrationKind::TenDlc => {
                let brand_id = h.brand_id.as_deref().ok_or_else(|| CarrierError::Invalid("no brand id".into()))?;
                let brand = self.call("GET", &format!("/10dlc/brand/{brand_id}"), None).await?;
                if str_at(&brand, &["status"]) == Some("REGISTRATION_FAILED") {
                    return Ok(RegistrationStatus { state: RegState::Rejected, reason: Some(brand.get("failureReasons").map(|r| r.to_string()).unwrap_or_else(|| "brand registration failed".into())) });
                }
                let Some(campaign_id) = h.campaign_id.as_deref() else {
                    return Ok(RegistrationStatus { state: RegState::Pending, reason: None });
                };
                let campaign = self.call("GET", &format!("/10dlc/campaign/{campaign_id}"), None).await?;
                let status = registration_state_from_campaign(&campaign);
                if status.state == RegState::Approved {
                    // Link the number to the approved campaign; a repeat assignment is harmless.
                    match self.call("POST", "/10dlc/phone_number_campaigns", Some(json!({ "phoneNumber": e164, "campaignId": campaign_id }))).await {
                        Ok(_) | Err(CarrierError::Upstream(400..=409, _)) => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(status)
            }
            RegistrationKind::TollFree => {
                let id = h.tfv_id.as_deref().ok_or_else(|| CarrierError::Invalid("no verification id".into()))?;
                let resp = self.call("GET", &format!("/messaging_tollfree/verification/requests/{id}"), None).await?;
                Ok(match str_at(&resp, &["verificationStatus"]).unwrap_or("") {
                    "Verified" => RegistrationStatus { state: RegState::Approved, reason: None },
                    "Rejected" => RegistrationStatus { state: RegState::Rejected, reason: str_at(&resp, &["reason"]).map(str::to_string).or_else(|| Some("rejected".into())) },
                    _ => RegistrationStatus { state: RegState::Pending, reason: None },
                })
            }
        }
    }

    async fn file_pending_campaign(&self, brand_id: &str, form: &RegistrationForm) -> Result<Option<String>, CarrierError> {
        match self.call("POST", "/10dlc/campaignBuilder", Some(campaign_body(brand_id, form))).await {
            Ok(campaign) => Ok(str_at(&campaign, &["campaignId"]).map(str::to_string)),
            Err(e) if brand_not_ready(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `PUT /10dlc/campaign/{id}` with the corrected texts. A Telnyx-rejected campaign goes back
    /// into review on the update (TELNYX_FAILED → TCR_ACCEPTED, seen 2026-10-06) with no new
    /// brand or campaign fee. The brand, use case and keywords stay as filed.
    async fn update_campaign(&self, campaign_id: &str, form: &RegistrationForm) -> Result<bool, CarrierError> {
        let mut body = campaign_body("", form);
        if let Some(o) = body.as_object_mut() {
            for k in ["brandId", "usecase", "optinKeywords", "optoutKeywords", "helpKeywords", "webhookURL"] {
                o.remove(k);
            }
        }
        self.call("PUT", &format!("/10dlc/campaign/{campaign_id}"), Some(body)).await?;
        Ok(true)
    }

    async fn send_brand_otp(&self, brand_id: &str) -> Result<(), CarrierError> {
        self.call(
            "POST",
            &format!("/10dlc/brand/{brand_id}/smsOtp"),
            Some(json!({
                "pinSms": "Allternit: your texting verification code is @OTP_PIN@. Reply STOP to opt out.",
                "successSms": "Allternit: you're verified. Your number's texting registration continues now.",
            })),
        )
        .await?;
        Ok(())
    }

    async fn verify_brand_otp(&self, brand_id: &str, pin: &str) -> Result<bool, CarrierError> {
        match self.call("PUT", &format!("/10dlc/brand/{brand_id}/smsOtp"), Some(json!({ "otpPin": pin.trim() }))).await {
            Ok(_) => Ok(true),
            Err(CarrierError::Upstream(400..=422, _)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn port_in_create(&self, e164s: &[String], reference: &str, webhook_url: &str) -> Result<PortStatus, CarrierError> {
        let profile = self
            .call(
                "POST",
                "/messaging_profiles",
                Some(json!({
                    "name": format!("allternit-{reference}"), "enabled": true, "whitelisted_destinations": ["US", "CA"],
                    "webhook_url": webhook_url, "webhook_api_version": "2",
                })),
            )
            .await?;
        let profile_id = str_at(&profile, &["data", "id"]).ok_or_else(|| CarrierError::Upstream(502, "messaging profile has no id".into()))?.to_string();
        let created = self.call("POST", "/porting_orders", Some(json!({ "phone_numbers": e164s, "customer_reference": reference }))).await;
        let resp = match created {
            Ok(r) => r,
            Err(e) => {
                let _ = self.call("DELETE", &format!("/messaging_profiles/{profile_id}"), None).await;
                return Err(e);
            }
        };
        // The order comes back as `data: [order]`.
        let order = resp.get("data").and_then(|d| d.get(0)).or_else(|| resp.get("data")).ok_or_else(|| CarrierError::Upstream(502, "porting order missing".into()))?;
        let mut status = port_status_of(order)?;
        // The ported number keeps texting through our messaging profile, and the order reports to us.
        self.call(
            "PATCH",
            &format!("/porting_orders/{}", status.id),
            Some(json!({
                "phone_number_configuration": { "messaging_profile_id": profile_id },
                "messaging": { "enable_messaging": true },
                "webhook_url": super::status_webhook_url("telnyx"),
            })),
        )
        .await?;
        status.messaging_ref = Some(profile_id);
        Ok(status)
    }

    async fn port_in_status(&self, order_id: &str) -> Result<PortStatus, CarrierError> {
        let resp = self.call("GET", &format!("/porting_orders/{order_id}"), None).await?;
        port_status_of(resp.get("data").unwrap_or(&resp))
    }
}

fn port_status_of(order: &Value) -> Result<PortStatus, CarrierError> {
    let id = str_at(order, &["id"]).ok_or_else(|| CarrierError::Upstream(502, "porting order has no id".into()))?.to_string();
    let status = str_at(order, &["status", "value"]).unwrap_or("draft").to_string();
    Ok(PortStatus { done: status == "ported", failed: status == "cancelled", id, status, messaging_ref: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carriers::fake::FakeHttp;
    use ed25519_dalek::{Signer, SigningKey};

    fn signing() -> (SigningKey, String) {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pub_b64 = STANDARD.encode(key.verifying_key().to_bytes());
        (key, pub_b64)
    }

    fn signed_headers(key: &SigningKey, ts: i64, body: &str) -> HashMap<String, String> {
        let sig = key.sign(format!("{ts}|{body}").as_bytes());
        HashMap::from([
            ("telnyx-signature-ed25519".to_string(), STANDARD.encode(sig.to_bytes())),
            ("telnyx-timestamp".to_string(), ts.to_string()),
        ])
    }

    const INBOUND: &str = r#"{"data":{"event_type":"message.received","payload":{"id":"m-1","direction":"inbound","from":{"phone_number":"+15551230000"},"to":[{"phone_number":"+15559870000"}],"text":"hi"}}}"#;

    #[test]
    fn verifies_and_parses_inbound() {
        let (key, pub_b64) = signing();
        let t = Telnyx::new(FakeHttp::with(vec![]), "k".into(), Some(&pub_b64)).unwrap();
        let now = chrono::Utc::now().timestamp();
        let event = t.parse_inbound(&signed_headers(&key, now, INBOUND), "", INBOUND.as_bytes()).unwrap();
        assert_eq!(event, InboundEvent::Message { id: "m-1".into(), from: "+15551230000".into(), to: "+15559870000".into(), text: "hi".into() });
    }

    #[test]
    fn rejects_bad_tampered_and_stale_signatures() {
        let (key, pub_b64) = signing();
        let t = Telnyx::new(FakeHttp::with(vec![]), "k".into(), Some(&pub_b64)).unwrap();
        let now = chrono::Utc::now().timestamp();
        let good = signed_headers(&key, now, INBOUND);
        assert_eq!(t.parse_inbound(&good, "", INBOUND.replace("hi", "yo").as_bytes()), Err(CarrierError::BadSignature));
        assert_eq!(t.parse_inbound(&HashMap::new(), "", INBOUND.as_bytes()), Err(CarrierError::BadSignature));
        let stale = signed_headers(&key, now - 4000, INBOUND);
        assert_eq!(t.parse_inbound(&stale, "", INBOUND.as_bytes()), Err(CarrierError::BadSignature));
        let other = SigningKey::from_bytes(&[9u8; 32]);
        assert_eq!(t.parse_inbound(&signed_headers(&other, now, INBOUND), "", INBOUND.as_bytes()), Err(CarrierError::BadSignature));
    }

    #[test]
    fn receipts_and_outbound_echoes_are_ignored() {
        let (key, pub_b64) = signing();
        let t = Telnyx::new(FakeHttp::with(vec![]), "k".into(), Some(&pub_b64)).unwrap();
        let now = chrono::Utc::now().timestamp();
        let receipt = r#"{"data":{"event_type":"message.finalized","payload":{"id":"m"}}}"#;
        assert_eq!(t.parse_inbound(&signed_headers(&key, now, receipt), "", receipt.as_bytes()), Ok(InboundEvent::Ignored));
    }

    #[test]
    fn no_public_key_means_not_configured_never_unverified() {
        let t = Telnyx::new(FakeHttp::with(vec![]), "k".into(), None).unwrap();
        assert_eq!(t.parse_inbound(&HashMap::new(), "", b"{}"), Err(CarrierError::NotConfigured));
    }

    #[tokio::test]
    async fn search_maps_numbers() {
        let http = FakeHttp::with(vec![(200, json!({"data":[{"phone_number":"+14155550101","cost_information":{"monthly_cost":"1.00"},"region_information":[{"region_type":"rate_center","region_name":"SAN FRANCISCO"}]}]}))]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let found = t.search(&SearchQuery { country: "US".into(), area_code: Some("415".into()), locality: None, kind: NumberType::Local, limit: 5 }).await.unwrap();
        assert_eq!(found[0].e164, "+14155550101");
        assert_eq!(found[0].locality.as_deref(), Some("SAN FRANCISCO"));
        let req = &http.requests()[0];
        assert!(req.url.contains("filter[national_destination_code]=415") && req.url.contains("filter[phone_number_type]=local"));
        assert_eq!(req.bearer.as_deref(), Some("k"));
    }

    #[tokio::test]
    async fn buy_creates_profile_then_orders_and_cleans_up_on_failure() {
        let http = FakeHttp::with(vec![
            (200, json!({"data":{"id":"prof-1"}})),
            (200, json!({"data":{"id":"ord-1","status":"pending"}})),
            (200, json!({"data":[{"id":"num-1"}]})),
        ]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let bought = t.buy(&BuyRequest { e164: "+14155550101".into(), kind: NumberType::Local, webhook_url: "https://api/x".into(), reference: "n1".into() }).await.unwrap();
        assert_eq!(bought, BoughtNumber { carrier_number_id: Some("num-1".into()), messaging_ref: Some("prof-1".into()) });
        let reqs = http.requests();
        assert_eq!(reqs[0].json.as_ref().unwrap()["webhook_url"], "https://api/x");
        assert_eq!(reqs[1].json.as_ref().unwrap()["messaging_profile_id"], "prof-1");

        let http = FakeHttp::with(vec![(200, json!({"data":{"id":"prof-2"}})), (422, json!({"errors":[{"detail":"number unavailable"}]}))]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let err = t.buy(&BuyRequest { e164: "+14155550101".into(), kind: NumberType::Local, webhook_url: "u".into(), reference: "n2".into() }).await.unwrap_err();
        assert_eq!(err, CarrierError::Upstream(422, "number unavailable".into()));
        assert_eq!(http.requests()[2].method, "DELETE", "the orphan messaging profile is removed");
    }

    #[tokio::test]
    async fn toll_free_registration_submits_and_maps_status() {
        let http = FakeHttp::with(vec![(200, json!({"id":"tfv-1"})), (200, json!({"verificationStatus":"Rejected","reason":"opt-in unclear"}))]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let form = RegistrationForm {
            legal_name: "Acme".into(), website: "https://acme.test".into(), street: "1 Main".into(), city: "X".into(), state: "CA".into(), postal_code: "94000".into(),
            contact_first_name: "A".into(), contact_last_name: "B".into(), contact_email: "a@acme.test".into(), contact_phone: "+14155550100".into(),
            message_volume: "1,000".into(), use_case: "Account Notifications".into(), use_case_summary: "bot replies".into(), opt_in_workflow: "web form".into(),
            sample_messages: vec!["Hi from Acme. Reply STOP to opt out.".into()], opt_in_image_urls: vec!["https://acme.test/optin.png".into()], ..Default::default()
        };
        let handle = t.submit_registration(RegistrationKind::TollFree, "+18005550101", None, None, &form).await.unwrap();
        assert_eq!(handle.tfv_id.as_deref(), Some("tfv-1"));
        assert_eq!(http.requests()[0].json.as_ref().unwrap()["phoneNumbers"][0]["phoneNumber"], "+18005550101");
        let status = t.registration_status(RegistrationKind::TollFree, "+18005550101", &handle, None).await.unwrap();
        assert_eq!(status, RegistrationStatus { state: RegState::Rejected, reason: Some("opt-in unclear".into()) });
        assert!(matches!(t.submit_registration(RegistrationKind::TollFree, "+1", None, None, &RegistrationForm::default()).await, Err(CarrierError::Invalid(_))));
    }

    #[tokio::test]
    async fn ten_dlc_status_assigns_number_once_approved() {
        let http = FakeHttp::with(vec![
            (200, json!({"status":"OK"})),
            (200, json!({"campaignStatus":"MNO_PROVISIONED"})),
            (200, json!({})),
        ]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let h = RegistrationHandle { brand_id: Some("b".into()), campaign_id: Some("c".into()), tfv_id: None };
        let s = t.registration_status(RegistrationKind::TenDlc, "+14155550101", &h, None).await.unwrap();
        assert_eq!(s.state, RegState::Approved);
        let last = http.requests().pop().unwrap();
        assert!(last.url.ends_with("/10dlc/phone_number_campaigns"));
        assert_eq!(last.json.unwrap()["campaignId"], "c");
    }

    #[tokio::test]
    async fn port_order_status_words() {
        let http = FakeHttp::with(vec![
            (200, json!({"data":{"id":"prof-9"}})),
            (201, json!({"data":[{"id":"po-1","status":{"value":"draft"}}]})),
            (200, json!({})),
            (200, json!({"data":{"id":"po-1","status":{"value":"ported"}}})),
        ]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let created = t.port_in_create(&["+14155550101".into()], "n1", "https://api/x").await.unwrap();
        assert!(!created.done && created.id == "po-1");
        assert_eq!(created.messaging_ref.as_deref(), Some("prof-9"));
        assert_eq!(http.requests()[2].json.as_ref().unwrap()["phone_number_configuration"]["messaging_profile_id"], "prof-9");
        assert!(t.port_in_status("po-1").await.unwrap().done);
    }

    #[tokio::test]
    async fn campaign_waits_for_a_pending_brand() {
        let pending = json!([{ "code": 527, "fields": null, "description": "Brand registration status pending" }]);
        let http = FakeHttp::with(vec![(200, json!({ "brandId": "b-1" })), (400, pending.clone())]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let form = RegistrationForm {
            entity_type: "PRIVATE_PROFIT".into(), display_name: "Acme".into(), contact_email: "ops@acme.test".into(), vertical: "TECHNOLOGY".into(),
            country: "US".into(), use_case: "CUSTOMER_CARE".into(), use_case_summary: "assistant".into(), opt_in_workflow: "texts us first".into(),
            sample_messages: vec!["hi".into()], ..Default::default()
        };
        let h = t.submit_registration(RegistrationKind::TenDlc, "+15550001111", None, None, &form).await.unwrap();
        assert_eq!((h.brand_id.as_deref(), h.campaign_id.as_deref()), (Some("b-1"), None), "the brand is kept, the campaign waits");
        let body = http.requests()[1].json.clone().unwrap();
        assert_eq!((body["helpMessage"].as_str().map(|m| m.contains("ops@acme.test")), body["optoutMessage"].is_string(), body["sample1"].as_str()), (Some(true), true, Some("hi")));

        let http = FakeHttp::with(vec![(400, pending), (200, json!({ "campaignId": "c-1" })), (422, json!({ "errors": [{ "detail": "bad sample" }] }))]);
        let t = Telnyx::new(http, "k".into(), None).unwrap();
        assert_eq!(t.file_pending_campaign("b-1", &form).await.unwrap(), None);
        assert_eq!(t.file_pending_campaign("b-1", &form).await.unwrap().as_deref(), Some("c-1"));
        assert!(matches!(t.file_pending_campaign("b-1", &form).await, Err(CarrierError::Upstream(422, m)) if m == "bad sample"));
    }


    #[tokio::test]
    async fn a_sole_proprietor_brand_has_no_ein_and_gets_a_code_first() {
        let http = FakeHttp::with(vec![(200, json!({ "brandId": "b-sp" })), (200, json!({}))]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        let form = RegistrationForm {
            entity_type: "SOLE_PROPRIETOR".into(), display_name: "Dana Lee Tutoring".into(), contact_email: "dana@example.test".into(), vertical: "EDUCATION".into(),
            country: "US".into(), use_case: "SOLE_PROPRIETOR".into(), use_case_summary: "lesson reminders".into(), opt_in_workflow: "students text first".into(),
            sample_messages: vec!["Reminder: lesson at 4".into()], contact_first_name: "Dana".into(), contact_last_name: "Lee".into(),
            mobile_phone: Some("+16125550188".into()), ein: Some("should-not-be-sent".into()), ..Default::default()
        };
        let h = t.submit_registration(RegistrationKind::TenDlc, "+16125550100", None, None, &form).await.unwrap();
        assert_eq!((h.brand_id.as_deref(), h.campaign_id.as_deref()), (Some("b-sp"), None), "the campaign waits for the code");
        let reqs = http.requests();
        let brand = reqs[0].json.clone().unwrap();
        assert_eq!((brand["firstName"].as_str(), brand["mobilePhone"].as_str(), brand.get("ein")), (Some("Dana"), Some("+16125550188"), None));
        assert!(reqs[1].url.ends_with("/10dlc/brand/b-sp/smsOtp") && reqs[1].method == "POST");
        assert!(reqs[1].json.as_ref().unwrap()["pinSms"].as_str().unwrap().contains("@OTP_PIN@"));
        // A sole proprietor without a mobile number is refused before anything is filed.
        let bad = RegistrationForm { mobile_phone: None, contact_phone: String::new(), ..form.clone() };
        assert!(matches!(t.submit_registration(RegistrationKind::TenDlc, "+16125550100", None, None, &bad).await, Err(CarrierError::Invalid(m)) if m.contains("mobilePhone")));

        let http = FakeHttp::with(vec![(200, json!({})), (400, json!({ "errors": [{ "detail": "invalid otp" }] }))]);
        let t = Telnyx::new(http.clone(), "k".into(), None).unwrap();
        assert!(t.verify_brand_otp("b-sp", " 123456 ").await.unwrap());
        assert!(!t.verify_brand_otp("b-sp", "000000").await.unwrap());
        assert_eq!(http.requests()[0].json.clone().unwrap()["otpPin"], "123456");
    }


    #[test]
    fn campaign_message_flow_is_one_opt_in_path_with_its_page() {
        let f = RegistrationForm {
            display_name: "Allternit".into(),
            opt_in_workflow: "texts first, signed forms, verbal consent".into(),
            opt_in_page_url: Some("https://api.allternit.com/sms/n1".into()),
            terms_url: Some("https://www.allternit.com/terms".into()),
            ..Default::default()
        };
        let flow = campaign_body("b1", &f)["messageFlow"].as_str().unwrap().to_string();
        assert!(flow.contains("https://api.allternit.com/sms/n1") && flow.contains("mobile-originated only"));
        assert!(flow.contains("Terms: https://www.allternit.com/terms."));
        let lower = flow.to_lowercase();
        assert!(!lower.contains("signed") && !lower.contains("verbal"), "the business's notes never add a second opt-in path");
    }

    #[test]
    fn a_campaign_update_sends_only_the_corrected_texts() {
        let f = RegistrationForm { display_name: "Acme".into(), use_case: "CUSTOMER_CARE".into(), use_case_summary: "help".into(), ..Default::default() };
        let mut body = campaign_body("", &f);
        for k in ["brandId", "usecase", "optinKeywords", "optoutKeywords", "helpKeywords", "webhookURL"] {
            body.as_object_mut().unwrap().remove(k);
        }
        assert!(body.get("messageFlow").is_some() && body.get("optinMessage").is_some() && body.get("description").is_some());
        assert!(body.get("brandId").is_none() && body.get("usecase").is_none());
    }

}
