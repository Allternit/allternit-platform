//! The one MCP server protocol layer every Allternit MCP server uses.
//!
//! MCP has two eras (spec 2026-07-28, "Versioning and Compatibility"):
//!
//! * **Legacy** (`2025-11-25` and earlier): the client opens with
//!   `initialize` and the server answers with the negotiated version.
//! * **Modern** (`2026-07-28`+): no handshake. Every request carries its
//!   version in `params._meta["io.modelcontextprotocol/protocolVersion"]`
//!   (and the `MCP-Protocol-Version` HTTP header); servers MUST answer
//!   `server/discover`, reject unknown versions with `-32022`, and every
//!   result carries `resultType`.
//!
//! Our servers are stateless already, so being dual-era is cheap: run
//! [`preflight`] before the server's own method match, and [`finish`] on
//! the response it produced. One version list lives here, so the next spec
//! revision is a one-line change instead of six disagreeing constants.

use serde_json::{json, Map, Value};

pub mod events;
pub mod servers;
pub mod webhooks;

/// Newest revision we speak. Modern clients get this unless they ask for
/// another supported modern version.
pub const LATEST: &str = "2026-07-28";

/// Handshake-era revisions we still answer `initialize` for, newest first.
pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Revision a legacy client gets when it asks for one we don't know.
pub const LEGACY_DEFAULT: &str = "2025-06-18";

/// Every revision this crate serves, newest first (the `supported` list in
/// `server/discover` and in `-32022` errors).
pub const SUPPORTED: [&str; 5] = [LATEST, "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// JSON-RPC error codes from the 2026-07-28 allocation policy.
pub mod codes {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// Legacy "resource not found"; modern uses INVALID_PARAMS.
    pub const LEGACY_RESOURCE_NOT_FOUND: i64 = -32002;
    pub const HEADER_MISMATCH: i64 = -32020;
    pub const MISSING_REQUIRED_CLIENT_CAPABILITY: i64 = -32021;
    pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;
}

/// How long a modern client may cache list/read results. Our lists are
/// per-user (OAuth principal), so they're always `private`.
pub const LIST_TTL_MS: u64 = 60_000;

/// Describes one server. Each Allternit MCP server builds one of these.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub name: &'static str,
    pub version: &'static str,
    /// Server capabilities as advertised to clients, e.g.
    /// `{"tools": {"listChanged": false}}`. `resources.subscribe` is a
    /// legacy-only key; [`ServerSpec::capabilities_for`] strips it for
    /// modern clients.
    pub capabilities: Value,
    pub instructions: Option<&'static str>,
}

impl ServerSpec {
    fn server_info(&self) -> Value {
        json!({ "name": self.name, "version": self.version })
    }

    pub fn capabilities_for(&self, era: &Era) -> Value {
        let mut caps = self.capabilities.clone();
        if era.is_modern() {
            // 2026-07-28 replaced resources/subscribe with subscriptions/listen.
            if let Some(res) = caps.get_mut("resources").and_then(Value::as_object_mut) {
                res.remove("subscribe");
            }
        }
        caps
    }
}

/// Which era a request belongs to, and the version it runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Era {
    /// `initialize` or a request without modern `_meta`. `version` is what
    /// the client asked for (or the header said), if anything.
    Legacy { version: Option<String> },
    /// A request carrying modern per-request metadata.
    Modern { version: String, client_capabilities: Value },
}

impl Era {
    /// Classify a request. `header_version` is the `MCP-Protocol-Version`
    /// HTTP header when the transport has one.
    pub fn of(method: &str, params: &Value, header_version: Option<&str>) -> Era {
        let meta = params.get("_meta");
        let meta_version = meta
            .and_then(|m| m.get(META_PROTOCOL_VERSION))
            .and_then(Value::as_str);
        if method != "initialize" {
            if let Some(v) = meta_version.or(header_version.filter(|v| is_modern_version(v))) {
                if is_modern_version(v) || meta_version.is_some() {
                    return Era::Modern {
                        version: v.to_string(),
                        client_capabilities: meta
                            .and_then(|m| m.get(META_CLIENT_CAPABILITIES))
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    };
                }
            }
        }
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .or(header_version);
        Era::Legacy { version: requested.map(str::to_string) }
    }

    pub fn is_modern(&self) -> bool {
        matches!(self, Era::Modern { .. })
    }

    /// Client capabilities: from `_meta` (modern) or `initialize` params
    /// (legacy — only present on the initialize request itself).
    pub fn client_capabilities<'a>(&'a self, params: &'a Value) -> Option<&'a Value> {
        match self {
            Era::Modern { client_capabilities, .. } => Some(client_capabilities),
            Era::Legacy { .. } => params.get("capabilities"),
        }
    }
}

/// A modern version is anything dated on or after the stateless revision.
/// Dates compare lexically (YYYY-MM-DD).
fn is_modern_version(v: &str) -> bool {
    v.len() == 10 && v >= LATEST
}

pub fn rpc_ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn rpc_err(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

pub fn rpc_err_data(id: &Value, code: i64, message: impl Into<String>, data: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into(), "data": data } })
}

/// Pick the version an `initialize` answer carries.
pub fn negotiate_legacy(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| LEGACY.iter().copied().find(|v| *v == r))
        .unwrap_or(LEGACY_DEFAULT)
}

/// Check the optional `Mcp-Method` / `Mcp-Name` request headers (required
/// for modern Streamable HTTP). We don't reject a request that omits them —
/// older proxies strip unknown headers — but a header that disagrees with
/// the body is a `-32020` HeaderMismatch, which the transport answers with
/// HTTP 400.
pub fn check_headers(
    id: &Value,
    method: &str,
    params: &Value,
    mcp_method: Option<&str>,
    mcp_name: Option<&str>,
) -> Option<Value> {
    if let Some(h) = mcp_method {
        if h != method {
            return Some(rpc_err(id, codes::HEADER_MISMATCH, format!("Mcp-Method header {h:?} does not match body method {method:?}")));
        }
    }
    if let Some(h) = mcp_name {
        let body_name = params
            .get("name")
            .or_else(|| params.get("uri"))
            .and_then(Value::as_str);
        if let Some(b) = body_name {
            if b != h {
                return Some(rpc_err(id, codes::HEADER_MISMATCH, format!("Mcp-Name header {h:?} does not match body {b:?}")));
            }
        }
    }
    None
}

/// Handle what's the same on every server: `initialize` (legacy),
/// `server/discover` (both eras), modern version checks, `ping`, and the
/// `notifications/*` a client may send. Returns `Some(response)` when the
/// request is fully handled here (`Some(Value::Null)` for a notification,
/// which gets no body), `None` to let the server's own method match run.
pub fn preflight(spec: &ServerSpec, era: &Era, id: &Value, method: &str) -> Option<Value> {
    if method.starts_with("notifications/") {
        return Some(Value::Null);
    }
    if let Era::Modern { version, .. } = era {
        if !SUPPORTED.contains(&version.as_str()) {
            return Some(rpc_err_data(
                id,
                codes::UNSUPPORTED_PROTOCOL_VERSION,
                "Unsupported protocol version",
                json!({ "supported": SUPPORTED, "requested": version }),
            ));
        }
    }
    match method {
        "initialize" => {
            let Era::Legacy { version } = era else { unreachable!("initialize is always legacy") };
            let negotiated = negotiate_legacy(version.as_deref());
            let mut result = json!({
                "protocolVersion": negotiated,
                "capabilities": spec.capabilities_for(era),
                "serverInfo": spec.server_info(),
            });
            if let Some(i) = spec.instructions {
                result["instructions"] = json!(i);
            }
            Some(rpc_ok(id, result))
        }
        "server/discover" => {
            let mut result = json!({
                "supportedVersions": SUPPORTED,
                "capabilities": spec.capabilities_for(era),
                "_meta": { META_SERVER_INFO: spec.server_info() },
                "ttlMs": LIST_TTL_MS,
                "cacheScope": "private",
            });
            if let Some(i) = spec.instructions {
                result["instructions"] = json!(i);
            }
            // Same shape for a legacy caller probing ahead of initialize.
            Some(rpc_ok(id, with_result_type(result)))
        }
        // Removed in 2026-07-28 but harmless; legacy clients still send it.
        "ping" => Some(finish(spec, era, method, rpc_ok(id, json!({})))),
        _ => None,
    }
}

fn with_result_type(mut result: Value) -> Value {
    if let Some(obj) = result.as_object_mut() {
        obj.entry("resultType").or_insert_with(|| json!("complete"));
    }
    result
}

/// Methods whose results are `CacheableResult` in 2026-07-28.
fn is_cacheable(method: &str) -> bool {
    matches!(
        method,
        "tools/list" | "prompts/list" | "resources/list" | "resources/read" | "resources/templates/list"
    )
}

/// Decorate a response the server built for the request's era. Legacy
/// responses pass through untouched. Modern results get `resultType`,
/// `_meta.serverInfo` and, for list/read methods, `ttlMs` + `cacheScope`;
/// modern errors get the renumbered resource-not-found code.
pub fn finish(spec: &ServerSpec, era: &Era, method: &str, mut response: Value) -> Value {
    if !era.is_modern() {
        return response;
    }
    if let Some(err) = response.get_mut("error").and_then(Value::as_object_mut) {
        if err.get("code").and_then(Value::as_i64) == Some(codes::LEGACY_RESOURCE_NOT_FOUND) {
            err.insert("code".into(), json!(codes::INVALID_PARAMS));
        }
        return response;
    }
    if let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) {
        result.entry("resultType").or_insert_with(|| json!("complete"));
        let meta = result
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(meta) = meta.as_object_mut() {
            meta.entry(META_SERVER_INFO).or_insert_with(|| spec.server_info());
        }
        if is_cacheable(method) {
            result.entry("ttlMs").or_insert_with(|| json!(LIST_TTL_MS));
            result.entry("cacheScope").or_insert_with(|| json!("private"));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServerSpec {
        ServerSpec {
            name: "test",
            version: "1.0",
            capabilities: json!({ "tools": { "listChanged": false }, "resources": { "subscribe": false, "listChanged": false } }),
            instructions: Some("hello"),
        }
    }

    fn modern_params() -> Value {
        json!({ "_meta": { META_PROTOCOL_VERSION: LATEST, META_CLIENT_CAPABILITIES: { "extensions": { "io.modelcontextprotocol/ui": {} } } } })
    }

    #[test]
    fn initialize_negotiates_known_and_falls_back() {
        let s = spec();
        for (asked, got) in [
            ("2025-11-25", "2025-11-25"),
            ("2025-03-26", "2025-03-26"),
            ("2024-11-05", "2024-11-05"),
            ("1999-01-01", LEGACY_DEFAULT),
        ] {
            let p = json!({ "protocolVersion": asked });
            let era = Era::of("initialize", &p, None);
            let r = preflight(&s, &era, &json!(1), "initialize").unwrap();
            assert_eq!(r["result"]["protocolVersion"], got);
            assert_eq!(r["result"]["capabilities"]["resources"]["subscribe"], false);
            assert_eq!(r["result"]["instructions"], "hello");
            assert!(r["result"].get("resultType").is_none());
        }
    }

    #[test]
    fn initialize_with_modern_version_still_legacy() {
        // A client that sends initialize wants the handshake era.
        let p = json!({ "protocolVersion": LATEST });
        assert!(!Era::of("initialize", &p, None).is_modern());
    }

    #[test]
    fn discover_lists_versions_and_strips_legacy_keys() {
        let s = spec();
        let p = modern_params();
        let era = Era::of("server/discover", &p, None);
        assert!(era.is_modern());
        let r = preflight(&s, &era, &json!("d"), "server/discover").unwrap();
        let res = &r["result"];
        assert_eq!(res["resultType"], "complete");
        assert_eq!(res["supportedVersions"][0], LATEST);
        assert!(res["capabilities"]["resources"].get("subscribe").is_none());
        assert_eq!(res["_meta"][META_SERVER_INFO]["name"], "test");
        assert_eq!(res["cacheScope"], "private");
    }

    #[test]
    fn unknown_modern_version_is_32022() {
        let p = json!({ "_meta": { META_PROTOCOL_VERSION: "2099-01-01" } });
        let era = Era::of("tools/list", &p, None);
        let r = preflight(&spec(), &era, &json!(7), "tools/list").unwrap();
        assert_eq!(r["error"]["code"], codes::UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(r["error"]["data"]["requested"], "2099-01-01");
        assert_eq!(r["error"]["data"]["supported"][0], LATEST);
    }

    #[test]
    fn header_alone_selects_modern_only_for_modern_versions() {
        assert!(Era::of("tools/list", &json!({}), Some(LATEST)).is_modern());
        assert!(!Era::of("tools/list", &json!({}), Some("2025-06-18")).is_modern());
        assert!(!Era::of("tools/list", &json!({}), None).is_modern());
    }

    #[test]
    fn finish_decorates_modern_only() {
        let s = spec();
        let legacy = Era::of("tools/list", &json!({}), None);
        let raw = rpc_ok(&json!(1), json!({ "tools": [] }));
        assert_eq!(finish(&s, &legacy, "tools/list", raw.clone()), raw);

        let modern = Era::of("tools/list", &modern_params(), None);
        let r = finish(&s, &modern, "tools/list", raw);
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["ttlMs"], LIST_TTL_MS);
        assert_eq!(r["result"]["_meta"][META_SERVER_INFO]["version"], "1.0");

        let call = finish(&s, &modern, "tools/call", rpc_ok(&json!(2), json!({ "content": [] })));
        assert!(call["result"].get("ttlMs").is_none());
        assert_eq!(call["result"]["resultType"], "complete");

        let nf = finish(&s, &modern, "resources/read", rpc_err(&json!(3), codes::LEGACY_RESOURCE_NOT_FOUND, "nf"));
        assert_eq!(nf["error"]["code"], codes::INVALID_PARAMS);
    }

    #[test]
    fn client_capabilities_from_either_era() {
        let p = modern_params();
        let era = Era::of("tools/list", &p, None);
        assert!(era.client_capabilities(&p).unwrap()["extensions"].get("io.modelcontextprotocol/ui").is_some());
        let lp = json!({ "protocolVersion": "2025-06-18", "capabilities": { "roots": {} } });
        let le = Era::of("initialize", &lp, None);
        assert!(le.client_capabilities(&lp).unwrap().get("roots").is_some());
    }

    #[test]
    fn notifications_get_no_body() {
        let era = Era::of("notifications/initialized", &json!({}), None);
        assert_eq!(preflight(&spec(), &era, &Value::Null, "notifications/initialized"), Some(Value::Null));
    }

    #[test]
    fn header_mismatch() {
        let id = json!(1);
        assert!(check_headers(&id, "tools/call", &json!({ "name": "a" }), Some("tools/call"), Some("a")).is_none());
        assert!(check_headers(&id, "tools/call", &json!({}), None, None).is_none());
        let e = check_headers(&id, "tools/call", &json!({ "name": "a" }), Some("tools/list"), None).unwrap();
        assert_eq!(e["error"]["code"], codes::HEADER_MISMATCH);
        let e = check_headers(&id, "tools/call", &json!({ "name": "a" }), None, Some("b")).unwrap();
        assert_eq!(e["error"]["code"], codes::HEADER_MISMATCH);
    }
}
