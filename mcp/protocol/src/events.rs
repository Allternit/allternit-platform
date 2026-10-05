//! MCP Events extension (draft: modelcontextprotocol/experimental-ext-triggers-events;
//! the webhook profile ChatGPT implements: developers.openai.com/plugins/build/mcp-events).
//!
//! Wire types and validation only. Storage, authorization and delivery live
//! with the server that owns the subscriptions (the `mcp.allternit.com` edge).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::IpAddr;

use crate::webhooks;

/// `events/*` error codes (draft extension; implementation-defined range).
pub mod codes {
    pub const NOT_FOUND: i64 = -32011;
    pub const FORBIDDEN: i64 = -32012;
    pub const RESOURCE_EXHAUSTED: i64 = -32013;
    pub const UNSUPPORTED: i64 = -32014;
    pub const CALLBACK_ENDPOINT_ERROR: i64 = -32015;
}

/// Largest event body a receiver must accept (ChatGPT: 256 KiB).
pub const MAX_EVENT_BYTES: usize = 262_144;

/// Header naming the subscription on every webhook delivery.
pub const HEADER_SUBSCRIPTION_ID: &str = "X-MCP-Subscription-Id";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    Webhook,
    Poll,
    Push,
}

/// One entry in `events/list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDef {
    pub name: &'static str,
    pub description: &'static str,
    pub delivery: Vec<DeliveryMode>,
    pub input_schema: Value,
    pub payload_schema: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Delivery {
    pub mode: DeliveryMode,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub secret: Option<String>,
}

/// `events/subscribe` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeParams {
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
    pub delivery: Delivery,
    #[serde(default)]
    pub cursor: Option<String>,
    /// Absent = server default; JSON `null` = asks for no expiry.
    #[serde(default, deserialize_with = "nullable")]
    pub ttl_ms: Option<Option<u64>>,
}

/// `events/unsubscribe` params.
#[derive(Debug, Clone, Deserialize)]
pub struct UnsubscribeParams {
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
    pub delivery: Delivery,
}

fn empty_object() -> Value {
    json!({})
}

fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<u64>>, D::Error> {
    Ok(Some(Option::<u64>::deserialize(d)?))
}

/// A validated webhook target.
#[derive(Debug, Clone)]
pub struct WebhookTarget {
    pub url: String,
    pub key: Vec<u8>,
}

/// Validate the webhook half of a subscribe/unsubscribe: https URL that
/// isn't an obviously private target, and (for subscribe) a `whsec_` secret
/// of 24–64 bytes. DNS resolution is re-checked at delivery time by the
/// sender — a literal check here is not enough on its own.
pub fn validate_webhook(delivery: &Delivery, need_secret: bool) -> Result<(String, Option<Vec<u8>>), String> {
    if delivery.mode != DeliveryMode::Webhook {
        return Err("only webhook delivery is offered".into());
    }
    let url = delivery.url.as_deref().ok_or("delivery.url is required")?;
    validate_callback_url(url)?;
    let key = match (&delivery.secret, need_secret) {
        (Some(s), _) => Some(webhooks::parse_secret(s).map_err(|e| e.to_string())?),
        (None, true) => return Err("delivery.secret is required".into()),
        (None, false) => None,
    };
    Ok((url.to_string(), key))
}

/// `https://` only, no credentials in the URL, no localhost / private /
/// link-local / loopback IP literals.
pub fn validate_callback_url(url: &str) -> Result<(), String> {
    let rest = url.strip_prefix("https://").ok_or("callback URL must use https://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err("callback URL has no host".into());
    }
    if authority.contains('@') {
        return Err("callback URL must not carry credentials".into());
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority)
    };
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") || lower.ends_with(".internal") {
        return Err("callback URL points at a private host".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_public_ip(&ip) {
            return Err("callback URL points at a private address".into());
        }
    }
    Ok(())
}

/// False for loopback, private, link-local, CGNAT, multicast, unspecified,
/// documentation and unique-local ranges.
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (18..=19).contains(&o[1]))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(&IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

/// Deterministic subscription id for the identity key
/// `(principal, url, name, arguments)` — the same key always maps to the
/// same id, which makes `events/subscribe` idempotent.
pub fn subscription_id(principal: &str, url: &str, name: &str, arguments: &Value) -> String {
    let mut h = Sha256::new();
    for part in [principal, url, name, &canonical_json(arguments)] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    let digest = h.finalize();
    let hex: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    format!("sub_{hex}")
}

/// JSON with object keys sorted, so `{"a":1,"b":2}` and `{"b":2,"a":1}`
/// identify the same subscription.
pub fn canonical_json(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<_> = m.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sort(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

/// Body of one event delivery.
pub fn event_envelope(event_id: &str, name: &str, timestamp_rfc3339: &str, data: Value, cursor: Option<&str>) -> Value {
    json!({ "eventId": event_id, "name": name, "timestamp": timestamp_rfc3339, "data": data, "cursor": cursor })
}

/// Control envelope sent before activating an unverified callback; the
/// receiver must echo `challenge`.
pub fn verification_envelope(challenge: &str) -> Value {
    json!({ "type": "verification", "challenge": challenge })
}

/// Control envelope ending a subscription (auth revoked, event removed).
pub fn terminated_envelope(subscription_id: &str, code: i64, message: &str) -> Value {
    json!({ "type": "terminated", "subscriptionId": subscription_id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    #[test]
    fn callback_url_rules() {
        for ok in ["https://receiver.example.com/cb/1", "https://chatgpt.com:443/x?y=1", "https://[2606:4700::1]/x"] {
            assert!(validate_callback_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://example.com/x",
            "https://localhost/x",
            "https://a.localhost/x",
            "https://127.0.0.1/x",
            "https://10.1.2.3/x",
            "https://192.168.1.1:8443/x",
            "https://169.254.169.254/latest",
            "https://100.64.0.3/x",
            "https://[::1]/x",
            "https://[fd00::1]/x",
            "https://[::ffff:10.0.0.1]/x",
            "https://user:pw@example.com/x",
            "https:///x",
        ] {
            assert!(validate_callback_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn subscribe_params_and_ttl_null() {
        let secret = format!("whsec_{}", B64.encode([1u8; 32]));
        let p: SubscribeParams = serde_json::from_value(json!({
            "name": "approval.requested",
            "arguments": { "agent_id": "a1" },
            "delivery": { "mode": "webhook", "url": "https://cb.example.com/1", "secret": secret },
            "cursor": null,
            "ttlMs": null
        }))
        .unwrap();
        assert_eq!(p.ttl_ms, Some(None));
        let (url, key) = validate_webhook(&p.delivery, true).unwrap();
        assert_eq!(url, "https://cb.example.com/1");
        assert_eq!(key.unwrap().len(), 32);

        let p2: SubscribeParams = serde_json::from_value(json!({
            "name": "x", "delivery": { "mode": "webhook", "url": "https://cb.example.com/1" }
        }))
        .unwrap();
        assert_eq!(p2.ttl_ms, None);
        assert_eq!(p2.arguments, json!({}));
        assert!(validate_webhook(&p2.delivery, true).is_err());
        assert!(validate_webhook(&p2.delivery, false).is_ok());
    }

    #[test]
    fn subscription_id_is_stable_and_key_order_free() {
        let a = subscription_id("u1", "https://x/cb", "n", &json!({ "a": 1, "b": { "d": 2, "c": 3 } }));
        let b = subscription_id("u1", "https://x/cb", "n", &json!({ "b": { "c": 3, "d": 2 }, "a": 1 }));
        assert_eq!(a, b);
        assert!(a.starts_with("sub_") && a.len() == 28);
        assert_ne!(a, subscription_id("u2", "https://x/cb", "n", &json!({ "a": 1, "b": { "d": 2, "c": 3 } })));
        // Length-prefixing stops ("ab","c") colliding with ("a","bc").
        assert_ne!(subscription_id("ab", "c", "n", &json!({})), subscription_id("a", "bc", "n", &json!({})));
    }

    #[test]
    fn event_def_serializes_camel_case() {
        let d = EventDef {
            name: "approval.requested",
            description: "d",
            delivery: vec![DeliveryMode::Webhook],
            input_schema: json!({ "type": "object" }),
            payload_schema: json!({ "type": "object" }),
        };
        let v = serde_json::to_value(d).unwrap();
        assert_eq!(v["delivery"][0], "webhook");
        assert!(v.get("inputSchema").is_some() && v.get("payloadSchema").is_some());
    }
}
