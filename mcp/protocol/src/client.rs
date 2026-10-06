//! The client half of dual-era MCP (spec 2026-07-28, "Versioning and
//! Compatibility" + the Streamable HTTP "Backward Compatibility" section).
//!
//! Pure functions only — no I/O — so every Allternit MCP client (the
//! `mcp-client` crate's transports, proxies in allternit-api) makes the same
//! decisions:
//!
//! * a modern request carries `_meta` [`META_PROTOCOL_VERSION`],
//!   [`META_CLIENT_CAPABILITIES`] and [`META_CLIENT_INFO`] ([`with_modern_meta`]),
//!   and on HTTP the `MCP-Protocol-Version`, `Mcp-Method` and (for
//!   `tools/call`, `resources/read`, `prompts/get`) `Mcp-Name` headers
//!   ([`modern_headers`]);
//! * the first modern request to a server is the era probe; its failure is
//!   classified by [`classify_probe_failure`]: a recognized modern JSON-RPC
//!   error means "modern server, retry or surface", anything else in the
//!   fallback set means "legacy server, send `initialize`";
//! * a result without `resultType` is complete ([`check_result_type`]).

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

use crate::{codes, LATEST, LEGACY, META_CLIENT_CAPABILITIES, META_CLIENT_INFO, META_PROTOCOL_VERSION, SUPPORTED};

/// HTTP header carrying the protocol version (both eras after 2025-06-18).
pub const HEADER_PROTOCOL_VERSION: &str = "mcp-protocol-version";
/// HTTP header mirroring the JSON-RPC `method` (modern).
pub const HEADER_METHOD: &str = "mcp-method";
/// HTTP header mirroring `params.name` / `params.uri` (modern).
pub const HEADER_NAME: &str = "mcp-name";
/// Legacy (2025-03-26 .. 2025-11-25) Streamable HTTP session header.
pub const HEADER_SESSION_ID: &str = "mcp-session-id";

/// The legacy version a dual-era client offers in `initialize`.
pub const LEGACY_OFFER: &str = LEGACY[0];

/// Methods whose `Mcp-Name` header is required (2026-07-28 Streamable HTTP).
fn named_method(method: &str) -> bool {
    matches!(method, "tools/call" | "resources/read" | "prompts/get")
}

/// `params` with the modern per-request metadata merged into `_meta`
/// (existing `_meta` keys the caller set — progress tokens, app metadata —
/// are kept; the three protocol keys are always ours).
pub fn with_modern_meta(params: Option<Value>, version: &str, client_capabilities: &Value, client_info: &Value) -> Value {
    let mut params = match params {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let meta = params.entry("_meta").or_insert_with(|| Value::Object(Map::new()));
    if !meta.is_object() {
        *meta = Value::Object(Map::new());
    }
    let meta = meta.as_object_mut().expect("object ensured above");
    meta.insert(META_PROTOCOL_VERSION.into(), json!(version));
    meta.insert(META_CLIENT_CAPABILITIES.into(), client_capabilities.clone());
    meta.insert(META_CLIENT_INFO.into(), client_info.clone());
    Value::Object(params)
}

/// Remove the modern protocol keys from `params._meta` (used before handing
/// a request that arrived modern to a legacy server). Drops `_meta` entirely
/// when nothing else is left in it.
pub fn strip_modern_meta(params: &mut Value) {
    let Some(obj) = params.as_object_mut() else { return };
    let empty = match obj.get_mut("_meta").and_then(Value::as_object_mut) {
        Some(meta) => {
            meta.remove(META_PROTOCOL_VERSION);
            meta.remove(META_CLIENT_CAPABILITIES);
            meta.remove(META_CLIENT_INFO);
            meta.is_empty()
        }
        None => false,
    };
    if empty {
        obj.remove("_meta");
    }
}

/// The protocol version a request's `_meta` declares, if it is modern.
pub fn declared_version(params: &Value) -> Option<&str> {
    params
        .get("_meta")
        .and_then(|m| m.get(META_PROTOCOL_VERSION))
        .and_then(Value::as_str)
}

/// Encode a value for `Mcp-Name` / `Mcp-Param-*`: plain when it is
/// header-safe visible ASCII without surrounding whitespace, otherwise the
/// `=?base64?…?=` sentinel form (also used for plain values that already
/// look like the sentinel).
pub fn encode_header_value(value: &str) -> String {
    let safe = !value.is_empty()
        && value.bytes().all(|b| (0x20..=0x7e).contains(&b) || b == b'\t')
        && !value.starts_with([' ', '\t'])
        && !value.ends_with([' ', '\t'])
        && !(value.starts_with("=?base64?") && value.ends_with("?="));
    if safe {
        value.to_string()
    } else {
        format!("=?base64?{}?=", STANDARD.encode(value.as_bytes()))
    }
}

/// Decode a header value that may use the base64 sentinel form. `None` when
/// the sentinel is present but malformed.
pub fn decode_header_value(value: &str) -> Option<String> {
    match value.strip_prefix("=?base64?").and_then(|v| v.strip_suffix("?=")) {
        Some(encoded) => STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok()),
        None => Some(value.to_string()),
    }
}

/// The standard request headers for a modern Streamable HTTP POST, as
/// `(lower-case name, value)` pairs.
pub fn modern_headers(version: &str, method: &str, params: &Value) -> Vec<(&'static str, String)> {
    let mut headers = vec![(HEADER_PROTOCOL_VERSION, version.to_string()), (HEADER_METHOD, method.to_string())];
    if named_method(method) {
        if let Some(name) = params.get("name").or_else(|| params.get("uri")).and_then(Value::as_str) {
            headers.push((HEADER_NAME, encode_header_value(name)));
        }
    }
    headers
}

/// Was this error a recognized modern JSON-RPC error (a modern server
/// speaking, as opposed to a legacy server rejecting a request it does not
/// understand)?
pub fn is_modern_error_code(code: i64) -> bool {
    matches!(
        code,
        codes::UNSUPPORTED_PROTOCOL_VERSION | codes::HEADER_MISMATCH | codes::MISSING_REQUIRED_CLIENT_CAPABILITY
    )
}

/// Newest modern version both sides support, from a `supported` list (an
/// `UnsupportedProtocolVersionError`'s `data.supported` or a discover
/// result's `supportedVersions`).
pub fn pick_modern_version(supported: &Value) -> Option<&'static str> {
    let theirs: Vec<&str> = supported.as_array()?.iter().filter_map(Value::as_str).collect();
    SUPPORTED
        .iter()
        .copied()
        .filter(|v| *v >= LATEST)
        .find(|v| theirs.contains(v))
}

/// Newest legacy version both sides support (what `initialize` should offer
/// when a server said which versions it speaks), else [`LEGACY_OFFER`].
pub fn pick_legacy_version(supported: Option<&Value>) -> &'static str {
    let theirs: Vec<&str> = supported
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    LEGACY
        .iter()
        .copied()
        .find(|v| theirs.contains(v))
        .unwrap_or(LEGACY_OFFER)
}

/// What a dual-era client does after its modern probe failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// A modern server that does not speak our preferred version: retry the
    /// request with this one.
    RetryModern(&'static str),
    /// A legacy server: send `initialize` offering this version.
    Legacy(&'static str),
    /// A modern server rejected the request itself (header mismatch, a
    /// required client capability): surface the error, do not fall back.
    ModernError,
    /// Not an era signal (auth, rate limit, server error, network): surface
    /// the error unchanged.
    Propagate,
}

/// Classify a failed modern probe.
///
/// * `http_status` — the HTTP status when the transport has one (`None` for
///   stdio, or for a JSON-RPC error that arrived in a 2xx body).
/// * `rpc_error` — the JSON-RPC `error` object, when the body had one.
/// * `timed_out` — the probe got no answer in time (stdio legacy servers may
///   ignore unknown methods).
///
/// Rules (spec 2026-07-28): a recognized modern error identifies a modern
/// server; 400/404/405 — and any other 4xx except auth / rate limiting —
/// without one identifies a legacy server, as does any non-modern JSON-RPC
/// error (a modern server MUST implement `server/discover`, so e.g. `-32601`
/// on the probe means legacy). On stdio a timeout also means legacy.
pub fn classify_probe_failure(http_status: Option<u16>, rpc_error: Option<&Value>, timed_out: bool) -> ProbeVerdict {
    if let Some(err) = rpc_error {
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code == codes::UNSUPPORTED_PROTOCOL_VERSION {
            let supported = err.pointer("/data/supported");
            return match supported.and_then(pick_modern_version) {
                Some(v) if v != LATEST => ProbeVerdict::RetryModern(v),
                // Our preferred version is in their list yet they refused it:
                // nothing to retry with.
                Some(_) => ProbeVerdict::ModernError,
                None => ProbeVerdict::Legacy(pick_legacy_version(supported)),
            };
        }
        if is_modern_error_code(code) {
            return ProbeVerdict::ModernError;
        }
    }
    if timed_out {
        return if http_status.is_none() { ProbeVerdict::Legacy(LEGACY_OFFER) } else { ProbeVerdict::Propagate };
    }
    match http_status {
        Some(401 | 403 | 407 | 408 | 429) => ProbeVerdict::Propagate,
        Some(s) if (400..500).contains(&s) => ProbeVerdict::Legacy(LEGACY_OFFER),
        Some(s) if (200..300).contains(&s) && rpc_error.is_some() => ProbeVerdict::Legacy(LEGACY_OFFER),
        Some(_) => ProbeVerdict::Propagate,
        // stdio / in-body JSON-RPC error with no HTTP status
        None if rpc_error.is_some() => ProbeVerdict::Legacy(LEGACY_OFFER),
        None => ProbeVerdict::Propagate,
    }
}

/// `resultType` semantics: missing means `complete` (legacy servers and
/// modern ones that omit it). Anything else (`input_required`, …) is a
/// multi-round-trip result this client does not continue; the error names
/// the type.
pub fn check_result_type(result: &Value) -> Result<(), String> {
    match result.get("resultType").and_then(Value::as_str) {
        None | Some("complete") => Ok(()),
        Some(other) => Err(format!("server returned resultType {other:?}, which this client does not support")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_meta_merges_and_keeps_caller_keys() {
        let caps = json!({ "extensions": { "io.modelcontextprotocol/ui": {} } });
        let info = json!({ "name": "c", "version": "1" });
        let p = with_modern_meta(Some(json!({ "name": "t", "_meta": { "progressToken": 7 } })), LATEST, &caps, &info);
        assert_eq!(p["name"], "t");
        assert_eq!(p["_meta"]["progressToken"], 7);
        assert_eq!(p["_meta"][META_PROTOCOL_VERSION], LATEST);
        assert_eq!(p["_meta"][META_CLIENT_CAPABILITIES], caps);
        assert_eq!(p["_meta"][META_CLIENT_INFO], info);
        let none = with_modern_meta(None, LATEST, &caps, &info);
        assert_eq!(declared_version(&none), Some(LATEST));

        let mut back = p.clone();
        strip_modern_meta(&mut back);
        assert_eq!(back, json!({ "name": "t", "_meta": { "progressToken": 7 } }));
        let mut bare = none;
        strip_modern_meta(&mut bare);
        assert_eq!(bare, json!({}));
    }

    #[test]
    fn header_values_use_the_base64_sentinel_when_unsafe() {
        assert_eq!(encode_header_value("get_weather"), "get_weather");
        assert_eq!(encode_header_value("Hello, 世界"), "=?base64?SGVsbG8sIOS4lueVjA==?=");
        assert_eq!(encode_header_value(" padded "), "=?base64?IHBhZGRlZCA=?=");
        assert_eq!(encode_header_value("line1\nline2"), "=?base64?bGluZTEKbGluZTI=?=");
        assert_eq!(encode_header_value("=?base64?literal?="), "=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?=");
        for v in ["get_weather", "Hello, 世界", " padded ", "=?base64?literal?="] {
            assert_eq!(decode_header_value(&encode_header_value(v)).as_deref(), Some(v));
        }
        assert_eq!(decode_header_value("=?base64?!!?="), None);
    }

    #[test]
    fn modern_headers_name_only_named_methods() {
        let h = modern_headers(LATEST, "tools/call", &json!({ "name": "echo" }));
        assert_eq!(h, vec![(HEADER_PROTOCOL_VERSION, LATEST.into()), (HEADER_METHOD, "tools/call".into()), (HEADER_NAME, "echo".into())]);
        let h = modern_headers(LATEST, "resources/read", &json!({ "uri": "ui://x/y" }));
        assert_eq!(h[2], (HEADER_NAME, "ui://x/y".into()));
        assert_eq!(modern_headers(LATEST, "tools/list", &json!({ "name": "x" })).len(), 2);
    }

    #[test]
    fn probe_classification() {
        use ProbeVerdict::*;
        // legacy servers: plain 400/404/405, or a non-modern JSON-RPC error
        assert_eq!(classify_probe_failure(Some(400), None, false), Legacy(LEGACY_OFFER));
        assert_eq!(classify_probe_failure(Some(404), None, false), Legacy(LEGACY_OFFER));
        assert_eq!(classify_probe_failure(Some(405), None, false), Legacy(LEGACY_OFFER));
        let nf = json!({ "code": -32601, "message": "Method not found" });
        assert_eq!(classify_probe_failure(Some(200), Some(&nf), false), Legacy(LEGACY_OFFER));
        assert_eq!(classify_probe_failure(Some(404), Some(&nf), false), Legacy(LEGACY_OFFER));
        assert_eq!(classify_probe_failure(None, Some(&nf), false), Legacy(LEGACY_OFFER));
        let bad_session = json!({ "code": -32000, "message": "Bad Request: No valid session ID provided" });
        assert_eq!(classify_probe_failure(Some(400), Some(&bad_session), false), Legacy(LEGACY_OFFER));
        // stdio silence
        assert_eq!(classify_probe_failure(None, None, true), Legacy(LEGACY_OFFER));
        assert_eq!(classify_probe_failure(Some(0), None, true), Propagate);
        // not era signals
        for s in [401, 403, 429, 500, 502] {
            assert_eq!(classify_probe_failure(Some(s), None, false), Propagate, "{s}");
        }
        // modern servers
        let mismatch = json!({ "code": -32020, "message": "x" });
        assert_eq!(classify_probe_failure(Some(400), Some(&mismatch), false), ModernError);
        let only_legacy = json!({ "code": -32022, "message": "u", "data": { "supported": ["2025-06-18", "2025-03-26"] } });
        assert_eq!(classify_probe_failure(Some(400), Some(&only_legacy), false), Legacy("2025-06-18"));
        let refused_ours = json!({ "code": -32022, "message": "u", "data": { "supported": [LATEST] } });
        assert_eq!(classify_probe_failure(Some(400), Some(&refused_ours), false), ModernError);
    }

    #[test]
    fn version_picking() {
        assert_eq!(pick_modern_version(&json!(["2099-01-01", LATEST, "2025-11-25"])), Some(LATEST));
        assert_eq!(pick_modern_version(&json!(["2025-11-25"])), None);
        assert_eq!(pick_legacy_version(Some(&json!(["2025-03-26", "2024-11-05"]))), "2025-03-26");
        assert_eq!(pick_legacy_version(None), LEGACY_OFFER);
    }

    #[test]
    fn result_type_defaults_to_complete() {
        assert!(check_result_type(&json!({ "tools": [] })).is_ok());
        assert!(check_result_type(&json!({ "resultType": "complete" })).is_ok());
        assert!(check_result_type(&json!({ "resultType": "input_required" })).unwrap_err().contains("input_required"));
    }
}
