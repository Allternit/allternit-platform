//! LiveKit server API client (Apache-2.0 LiveKit) over HTTP/twirp.
//!
//! Used by the cloud voice pieces (voice_calls_cloud) and by the phone-number
//! program: per-number SIP inbound trunk + dispatch rule (`roomPrefix: "call-"`,
//! agent `allternit-voice`, participant attributes `{botId, ownerId, numberId,
//! to}`), `CreateSIPParticipant` for outbound calls — refused without a
//! consentRef from the phone/SMS consent gate — and `RoomService.SendData`
//! for call controls (topic `allternit.call.control`).
//!
//! Endpoint and field names follow the official LiveKit docs
//! (<https://docs.livekit.io/telephony/accepting-calls/dispatch-rule/> and the
//! SIP/Room service API references); requests are JSON twirp POSTs authorized
//! by a short-lived HS256 JWT minted from ALLTERNIT_LIVEKIT_KEY/SECRET.

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Room prefix for every call room (frozen HANDOFF §4.1).
pub const CALL_ROOM_PREFIX: &str = "call-";
/// LiveKit agent the dispatch rule dispatches into each call room.
pub const SIP_AGENT_NAME: &str = "allternit-voice";
/// LiveKit data-channel topic the worker listens on for call controls.
pub const CONTROL_TOPIC: &str = "allternit.call.control";

pub const LIVEKIT_URL_ENV: &str = "ALLTERNIT_LIVEKIT_URL";
/// Twirp service name of LiveKit's SIP API: the path is `/twirp/livekit.SIP/<Method>`
/// (protocol `livekit_sip.proto`, `service SIP`). `SIPService` 404s.
const SIP_SERVICE: &str = "SIP";
pub const LIVEKIT_KEY_ENV: &str = "ALLTERNIT_LIVEKIT_KEY";
pub const LIVEKIT_SECRET_ENV: &str = "ALLTERNIT_LIVEKIT_SECRET";
/// Browser/app-facing LiveKit URL (wss://livekit.allternit.com). Distinct from
/// ALLTERNIT_LIVEKIT_URL, which is the server API endpoint.
pub const LIVEKIT_PUBLIC_URL_ENV: &str = "ALLTERNIT_LIVEKIT_PUBLIC_URL";
/// Participant tokens for Listen / Take over live one hour.
const PARTICIPANT_TOKEN_TTL_SECS: i64 = 3600;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Minted server JWTs live five minutes; each request mints a fresh one.
const JWT_TTL_SECS: i64 = 300;

#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
    /// Public URL handed to clients with a participant token.
    pub public_url: Option<String>,
}

/// A room-scoped participant token plus the URL the client connects to.
#[derive(Debug, Clone)]
pub struct ParticipantAccess {
    pub token: String,
    pub url: String,
}

impl LiveKitConfig {
    pub fn from_env() -> Option<Self> {
        let url = std::env::var(LIVEKIT_URL_ENV).ok()?;
        let api_key = std::env::var(LIVEKIT_KEY_ENV).ok()?;
        let api_secret = std::env::var(LIVEKIT_SECRET_ENV).ok()?;
        if url.is_empty() || api_key.is_empty() || api_secret.is_empty() {
            return None;
        }
        let public_url = std::env::var(LIVEKIT_PUBLIC_URL_ENV)
            .ok()
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty());
        Some(Self { url: url.trim_end_matches('/').to_string(), api_key, api_secret, public_url })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveKitError {
    #[error("livekit_not_configured")]
    NotConfigured,
    /// Outbound SIP participants may only be created with a consentRef from
    /// the phone/SMS consent gate (frozen HANDOFF §4.1).
    #[error("consent_ref_required")]
    ConsentRequired,
    #[error("livekit_public_url_not_configured")]
    PublicUrlMissing,
    #[error("livekit URL blocked: {0}")]
    Blocked(String),
    #[error("livekit request failed: {0}")]
    Http(String),
    #[error("livekit returned {0}: {1}")]
    Server(u16, String),
}

/// Outbound call leg. `consent_ref` is mandatory: the SIP participant is only
/// created when ao-phone-sms's TCPA consent gate has a reference on file.
pub struct CreateSipParticipantRequest {
    pub trunk_id: String,
    /// E.164 number to dial.
    pub call_to: String,
    pub room_name: String,
    pub participant_identity: String,
    pub participant_attributes: std::collections::HashMap<String, String>,
    pub consent_ref: Option<String>,
    /// Caller ID: the bot's own E.164 (`sipNumber`). Unset → the trunk's number.
    pub from_number: Option<String>,
}

/// Seam for the LiveKit server API. The production impl posts twirp JSON;
/// tests substitute a fake (no real LiveKit, no real credentials).
#[async_trait]
pub trait LiveKitAdminClient: Send + Sync {
    /// Create (or return the existing) SIP inbound trunk for one E.164 number.
    /// Returns the trunk id (`ST_…`).
    async fn ensure_inbound_trunk(&self, number_id: &str, e164: &str) -> Result<String, LiveKitError>;
    /// Delete the SIP inbound trunk created for this number.
    async fn delete_inbound_trunk(&self, trunk_id: &str) -> Result<(), LiveKitError>;
    /// Create the per-number dispatch rule: individual rooms prefixed `call-`,
    /// attributes `{botId, ownerId, numberId, to}`, agent `allternit-voice`.
    /// Returns the dispatch rule id (`SDR_…`).
    async fn ensure_dispatch_rule(
        &self,
        trunk_id: &str,
        number_id: &str,
        bot_id: &str,
        owner_id: &str,
        to_e164: &str,
    ) -> Result<String, LiveKitError>;
    async fn delete_dispatch_rule(&self, rule_id: &str) -> Result<(), LiveKitError>;
    /// Create a SIP participant (outbound leg). Refused without a consentRef.
    async fn create_sip_participant(
        &self,
        request: CreateSipParticipantRequest,
    ) -> Result<Value, LiveKitError>;
    /// Create a room that dispatches the named agent when it opens (outbound
    /// calls: the room and the agent exist before the SIP leg dials).
    /// `metadata` is the JSON string the agent receives as its job metadata.
    async fn create_room_with_agent(&self, _room: &str, _agent_name: &str, _metadata: &str) -> Result<(), LiveKitError> {
        Err(LiveKitError::NotConfigured)
    }
    /// Publish a data packet on a room's data channel (call controls).
    async fn send_data(&self, room: &str, topic: &str, payload: &[u8]) -> Result<(), LiveKitError>;
    /// Mint a client token for one room. Listen is receive-only
    /// (`can_publish` false → canSubscribe true, canPublish false,
    /// canPublishData false); takeover can publish audio and data.
    fn participant_access(
        &self,
        room: &str,
        identity: &str,
        can_publish: bool,
    ) -> Result<ParticipantAccess, LiveKitError>;
}

pub struct LiveKitHttpAdmin {
    config: LiveKitConfig,
    client: reqwest::Client,
}

impl LiveKitHttpAdmin {
    pub fn new(config: LiveKitConfig) -> Self {
        Self { config, client: reqwest::Client::new() }
    }

    /// Egress guard, same rule as the channel transports: only http(s), no
    /// loopback/private/link-local literals. The URL is operator-configured,
    /// but a bad value must fail closed, not dial the cluster's own metadata
    /// service.
    fn guarded_url(&self, service: &str, method: &str) -> Result<String, LiveKitError> {
        let url = format!("{}/twirp/livekit.{service}/{method}", self.config.url);
        let parsed = reqwest::Url::parse(&url).map_err(|e| LiveKitError::Blocked(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(LiveKitError::Blocked("only http(s) destinations are allowed".into()));
        }
        let host = parsed.host_str().ok_or_else(|| LiveKitError::Blocked("URL has no host".into()))?;
        if is_forbidden_literal(host) {
            return Err(LiveKitError::Blocked(format!("{host} is not a public destination")));
        }
        Ok(url)
    }

    fn jwt(&self) -> Result<String, LiveKitError> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            sub: &'a str,
            iat: i64,
            nbf: i64,
            exp: i64,
            video: VideoGrant<'a>,
            sip: SipGrant,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct VideoGrant<'a> {
            room_admin: bool,
            room_create: bool,
            room_list: bool,
            room: &'a str,
        }
        // https://docs.livekit.io/home/get-started/authentication/#sip-grant
        // SIP admin manages trunks and dispatch rules; SIP call places
        // outbound calls (CreateSIPParticipant).
        #[derive(Serialize)]
        struct SipGrant {
            admin: bool,
            call: bool,
        }
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            iss: &self.config.api_key,
            sub: "allternit-cloud-api",
            iat: now,
            nbf: now - 10,
            exp: now + JWT_TTL_SECS,
            video: VideoGrant { room_admin: true, room_create: true, room_list: true, room: "*" },
            sip: SipGrant { admin: true, call: true },
        };
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        jsonwebtoken::encode(&header, &claims, &jsonwebtoken::EncodingKey::from_secret(self.config.api_secret.as_bytes()))
            .map_err(|e| LiveKitError::Http(format!("minting server JWT: {e}")))
    }

    fn participant_jwt(&self, room: &str, identity: &str, can_publish: bool) -> Result<String, LiveKitError> {
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            sub: &'a str,
            iat: i64,
            nbf: i64,
            exp: i64,
            video: Grant<'a>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Grant<'a> {
            room_join: bool,
            room: &'a str,
            can_subscribe: bool,
            can_publish: bool,
            can_publish_data: bool,
        }
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            iss: &self.config.api_key,
            sub: identity,
            iat: now,
            nbf: now - 10,
            exp: now + PARTICIPANT_TOKEN_TTL_SECS,
            video: Grant {
                room_join: true,
                room,
                can_subscribe: true,
                can_publish,
                can_publish_data: can_publish,
            },
        };
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        jsonwebtoken::encode(&header, &claims, &jsonwebtoken::EncodingKey::from_secret(self.config.api_secret.as_bytes()))
            .map_err(|e| LiveKitError::Http(format!("minting participant JWT: {e}")))
    }

    async fn post(&self, service: &str, method: &str, body: Value) -> Result<Value, LiveKitError> {
        let url = self.guarded_url(service, method)?;
        let token = self.jwt()?;
        let response = self
            .client
            .post(&url)
            .timeout(REQUEST_TIMEOUT)
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| LiveKitError::Http(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| LiveKitError::Http(e.to_string()))?;
        if !(200..=299).contains(&status) {
            return Err(LiveKitError::Server(status, text));
        }
        serde_json::from_str(&text).map_err(|e| LiveKitError::Http(format!("decoding {method} response: {e}")))
    }
}

/// Loopback / private / link-local literals are never dialed (defense in
/// depth; the env URL is operator input).
fn is_forbidden_literal(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower == "metadata.google.internal" {
        return true;
    }
    let ip = lower.parse::<std::net::IpAddr>().ok();
    match ip {
        Some(std::net::IpAddr::V4(v4)) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        Some(std::net::IpAddr::V6(v6)) => v6.is_loopback() || v6.is_unspecified(),
        None => false,
    }
}

#[async_trait]
impl LiveKitAdminClient for LiveKitHttpAdmin {
    async fn ensure_inbound_trunk(&self, number_id: &str, e164: &str) -> Result<String, LiveKitError> {
        let body = json!({
            "trunk": {
                "name": format!("allternit-number-{number_id}"),
                "numbers": [e164],
            }
        });
        let created = self.post(SIP_SERVICE, "CreateSIPInboundTrunk", body).await?;
        if let Some(id) = created.pointer("/sipTrunkId").or_else(|| created.pointer("/sip_trunk_id")).and_then(Value::as_str) {
            return Ok(id.to_string());
        }
        Err(LiveKitError::Http("CreateSIPInboundTrunk returned no sipTrunkId".into()))
    }

    async fn delete_inbound_trunk(&self, trunk_id: &str) -> Result<(), LiveKitError> {
        self.post(SIP_SERVICE, "DeleteSIPInboundTrunk", json!({ "sipTrunkId": trunk_id })).await?;
        Ok(())
    }

    async fn ensure_dispatch_rule(
        &self,
        trunk_id: &str,
        number_id: &str,
        bot_id: &str,
        owner_id: &str,
        to_e164: &str,
    ) -> Result<String, LiveKitError> {
        let body = json!({
            "dispatchRule": {
                "name": format!("allternit-number-{number_id}"),
                "trunkIds": [trunk_id],
                "rule": { "dispatchRuleIndividual": { "roomPrefix": CALL_ROOM_PREFIX } },
                "attributes": {
                    "botId": bot_id,
                    "ownerId": owner_id,
                    "numberId": number_id,
                    "to": to_e164,
                },
                "roomConfig": { "agents": [{ "agentName": SIP_AGENT_NAME }] },
            }
        });
        let created = self.post(SIP_SERVICE, "CreateSIPDispatchRule", body).await?;
        if let Some(id) = created.pointer("/sipDispatchRuleId").or_else(|| created.pointer("/sip_dispatch_rule_id")).and_then(Value::as_str) {
            return Ok(id.to_string());
        }
        Err(LiveKitError::Http("CreateSIPDispatchRule returned no sipDispatchRuleId".into()))
    }

    async fn delete_dispatch_rule(&self, rule_id: &str) -> Result<(), LiveKitError> {
        self.post(SIP_SERVICE, "DeleteSIPDispatchRule", json!({ "sipDispatchRuleId": rule_id })).await?;
        Ok(())
    }

    async fn create_sip_participant(
        &self,
        request: CreateSipParticipantRequest,
    ) -> Result<Value, LiveKitError> {
        let consent_ref = request.consent_ref.clone().unwrap_or_default();
        if consent_ref.trim().is_empty() {
            return Err(LiveKitError::ConsentRequired);
        }
        let mut attributes = request.participant_attributes;
        attributes.insert("direction".to_string(), "outbound".to_string());
        attributes.insert("consentRef".to_string(), consent_ref);
        let mut body = json!({
            "sipTrunkId": request.trunk_id,
            "sipCallTo": request.call_to,
            "roomName": request.room_name,
            "participantIdentity": request.participant_identity,
            "participantAttributes": attributes,
        });
        if let Some(from) = request.from_number.filter(|f| !f.is_empty()) {
            // https://docs.livekit.io/reference/telephony/sip-api/#createsipparticipant : sip_number = SIP From number
            body["sipNumber"] = json!(from);
        }
        self.post(SIP_SERVICE, "CreateSIPParticipant", body).await
    }

    async fn create_room_with_agent(&self, room: &str, agent_name: &str, metadata: &str) -> Result<(), LiveKitError> {
        // https://docs.livekit.io/reference/server/server-apis/#createroom : CreateRoomRequest.agents[] {agentName, metadata}
        let body = json!({
            "name": room,
            "emptyTimeout": 120,
            "agents": [{ "agentName": agent_name, "metadata": metadata }],
        });
        self.post("RoomService", "CreateRoom", body).await?;
        Ok(())
    }

    async fn send_data(&self, room: &str, topic: &str, payload: &[u8]) -> Result<(), LiveKitError> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let body = json!({
            "room": room,
            "topic": topic,
            "data": STANDARD.encode(payload),
            "kind": "RELIABLE",
        });
        self.post("RoomService", "SendData", body).await?;
        Ok(())
    }

    fn participant_access(
        &self,
        room: &str,
        identity: &str,
        can_publish: bool,
    ) -> Result<ParticipantAccess, LiveKitError> {
        let url = self.config.public_url.clone().ok_or(LiveKitError::PublicUrlMissing)?;
        Ok(ParticipantAccess { token: self.participant_jwt(room, identity, can_publish)?, url })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_literal_hosts_are_blocked() {
        assert!(is_forbidden_literal("localhost"));
        assert!(is_forbidden_literal("127.0.0.1"));
        assert!(is_forbidden_literal("10.0.0.1"));
        assert!(is_forbidden_literal("192.168.1.1"));
        assert!(is_forbidden_literal("169.254.1.1"));
        assert!(is_forbidden_literal("0.0.0.0"));
        assert!(is_forbidden_literal("::1"));
        assert!(!is_forbidden_literal("livekit.example.com"));
        assert!(!is_forbidden_literal("203.0.113.10"));
    }

    #[test]
    fn config_from_env_requires_all_three() {
        // Env is process-global; keep this the only test that mutates the
        // LIVEKIT_* vars and restore the originals.
        let saved = [
            (LIVEKIT_URL_ENV, std::env::var(LIVEKIT_URL_ENV).ok()),
            (LIVEKIT_KEY_ENV, std::env::var(LIVEKIT_KEY_ENV).ok()),
            (LIVEKIT_SECRET_ENV, std::env::var(LIVEKIT_SECRET_ENV).ok()),
        ];
        std::env::remove_var(LIVEKIT_URL_ENV);
        std::env::remove_var(LIVEKIT_KEY_ENV);
        std::env::remove_var(LIVEKIT_SECRET_ENV);
        assert!(LiveKitConfig::from_env().is_none(), "unset env means not configured");

        std::env::set_var(LIVEKIT_URL_ENV, "https://livekit.example.com");
        std::env::set_var(LIVEKIT_KEY_ENV, "key");
        std::env::remove_var(LIVEKIT_SECRET_ENV);
        assert!(LiveKitConfig::from_env().is_none(), "missing secret means not configured");

        std::env::set_var(LIVEKIT_SECRET_ENV, "secret");
        let config = LiveKitConfig::from_env().expect("all three set");
        assert_eq!(config.url, "https://livekit.example.com");
        assert!(config.url.ends_with("example.com"));

        for (name, value) in saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }

    #[tokio::test]
    async fn outbound_participant_without_consent_ref_is_refused() {
        let admin = LiveKitHttpAdmin::new(LiveKitConfig {
            url: "https://livekit.example.com".to_string(),
            api_key: "key".to_string(),
            api_secret: "secret".to_string(),
            public_url: None,
        });
        let result = admin
            .create_sip_participant(CreateSipParticipantRequest {
                trunk_id: "ST_x".to_string(),
                call_to: "+15551234567".to_string(),
                room_name: "call-abc".to_string(),
                participant_identity: "sip-out".to_string(),
                participant_attributes: std::collections::HashMap::new(),
                consent_ref: None,
                from_number: None,
            })
            .await;
        assert!(matches!(result, Err(LiveKitError::ConsentRequired)));
    }

    #[test]
    fn server_jwt_carries_admin_grant_and_short_ttl() {
        let admin = LiveKitHttpAdmin::new(LiveKitConfig {
            url: "https://livekit.example.com".to_string(),
            api_key: "testkey".to_string(),
            api_secret: "testsecret".to_string(),
            public_url: Some("wss://livekit.example.com".to_string()),
        });
        let token = admin.jwt().expect("jwt mints");
        let mut parts = token.split('.');
        let _header = parts.next().unwrap();
        let payload = parts.next().unwrap();
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(claims["iss"], "testkey");
        assert_eq!(claims["sub"], "allternit-cloud-api");
        assert_eq!(claims["video"]["roomAdmin"], true);
        assert_eq!(claims["video"]["room"], "*");
        assert_eq!(claims["video"]["roomCreate"], true, "CreateRoom needs roomCreate");
        assert_eq!(claims["sip"]["admin"], true, "trunks and dispatch rules need sip.admin");
        assert_eq!(claims["sip"]["call"], true, "CreateSIPParticipant needs sip.call");
        let ttl = claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap();
        assert!((250..=300).contains(&ttl), "ttl ~5m, got {ttl}");
    }

    #[test]
    fn sip_calls_use_the_twirp_sip_service_path() {
        let admin = LiveKitHttpAdmin::new(LiveKitConfig {
            url: "https://livekit.example.com".to_string(),
            api_key: "k".to_string(),
            api_secret: "s".to_string(),
            public_url: None,
        });
        assert_eq!(
            admin.guarded_url(SIP_SERVICE, "CreateSIPParticipant").unwrap(),
            "https://livekit.example.com/twirp/livekit.SIP/CreateSIPParticipant"
        );
    }

    fn decode_claims(token: &str) -> Value {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let payload = token.split('.').nth(1).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
    }

    fn test_admin(public_url: Option<&str>) -> LiveKitHttpAdmin {
        LiveKitHttpAdmin::new(LiveKitConfig {
            url: "https://livekit.example.com".to_string(),
            api_key: "k".to_string(),
            api_secret: "s".to_string(),
            public_url: public_url.map(str::to_string),
        })
    }

    #[test]
    fn listen_token_is_receive_only_and_takeover_can_publish() {
        let admin = test_admin(Some("wss://livekit.allternit.com"));
        let listen = admin.participant_access("call-1", "listener-u", false).unwrap();
        assert_eq!(listen.url, "wss://livekit.allternit.com");
        let g = &decode_claims(&listen.token)["video"];
        assert_eq!(g["room"], "call-1");
        assert_eq!(g["roomJoin"], true);
        assert_eq!(g["canSubscribe"], true);
        assert_eq!(g["canPublish"], false);
        assert_eq!(g["canPublishData"], false);
        let talk = admin.participant_access("call-1", "human-u", true).unwrap();
        let g = &decode_claims(&talk.token)["video"];
        assert_eq!(g["canPublish"], true);
        assert_eq!(g["canPublishData"], true);
    }

    #[test]
    fn participant_access_needs_the_public_url() {
        let admin = test_admin(None);
        assert!(matches!(
            admin.participant_access("call-1", "x", false),
            Err(LiveKitError::PublicUrlMissing)
        ));
    }
}
