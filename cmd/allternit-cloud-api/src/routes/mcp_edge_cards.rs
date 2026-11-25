//! MCP edge: Server Card (SEP-2127) and AI catalog for mcp.allternit.com.
//!
//! * `GET /.well-known/mcp` (alias `/.well-known/mcp/server-card.json`): the
//!   Server Card, `application/mcp-server-card+json`. One card, three remotes
//!   (agents, connectors, bots by id), all streamable-http.
//! * `GET /.well-known/ai-catalog.json`: the AI catalog listing the card,
//!   `application/ai-catalog+json`.
//!
//! Static JSON built from constants (no env, no state, so it answers on a
//! deploy whose `MCP_PUBLIC_URL` is unset too). `Cache-Control: public,
//! max-age=3600`, `Access-Control-Allow-Origin: *`.
//!
//! Shape notes from the SEP schema: the card has no top-level capabilities or
//! authentication field and a remote has no `authentication`, so OAuth is
//! advertised under `_meta["com.allternit/auth"]` (pointing at the RFC 9728
//! protected-resource metadata the edge already serves) and the MCP
//! extensions under each remote's `extensionsInfo`.

use axum::{
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

const ORIGIN: &str = "https://mcp.allternit.com";
const CARD_SCHEMA: &str = "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json";
const CARD_TYPE: &str = "application/mcp-server-card+json";
const CATALOG_TYPE: &str = "application/ai-catalog+json";
const CARD_PATH: &str = "/.well-known/mcp";
const CATALOG_PATH: &str = "/.well-known/ai-catalog.json";
const CARD_NAME: &str = "com.allternit/allternit";
const DESCRIPTION: &str = "Your Allternit agents, bots and connected apps as MCP tools.";

pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(CARD_PATH, get(card))
        .route("/.well-known/mcp/server-card.json", get(card))
        .route(CATALOG_PATH, get(catalog))
}

/// The Server Card for `origin` (no trailing slash).
pub fn server_card(origin: &str) -> Value {
    let extensions = json!({
        "tasks": { "description": "Long-running tool calls as tasks" },
        "events": { "description": "Event catalog and subscriptions" },
        "ui": { "description": "MCP Apps: tools return interactive UI" },
        "skills": { "description": "Skills served as resources" }
    });
    let remote = |url: String, variables: Value| {
        let mut r = json!({
            "type": "streamable-http",
            "url": url,
            "supportedProtocolVersions": mcp_protocol::SUPPORTED,
            "extensionsInfo": extensions,
        });
        if !variables.is_null() {
            r["variables"] = variables;
        }
        r
    };
    json!({
        "$schema": CARD_SCHEMA,
        "name": CARD_NAME,
        "title": "Allternit",
        "version": env!("CARGO_PKG_VERSION"),
        "description": DESCRIPTION,
        "websiteUrl": "https://allternit.com",
        "remotes": [
            remote(format!("{origin}/mcp"), Value::Null),
            remote(format!("{origin}/mcp/connectors"), Value::Null),
            remote(
                format!("{origin}/mcp/bots/{{vendor_bot_id}}"),
                json!({ "vendor_bot_id": { "description": "The id of the vendor bot to talk to", "isRequired": true, "format": "string" } }),
            ),
        ],
        "_meta": {
            "com.allternit/auth": {
                "type": "oauth2",
                "protectedResourceMetadata": format!("{origin}/.well-known/oauth-protected-resource"),
            }
        }
    })
}

/// The AI catalog (specVersion 1.0) listing the Server Card.
pub fn ai_catalog(origin: &str) -> Value {
    json!({
        "specVersion": "1.0",
        "host": {
            "displayName": "Allternit",
            "identifier": "mcp.allternit.com",
            "documentationUrl": "https://docs.allternit.com",
        },
        "entries": [{
            "identifier": "urn:air:mcp.allternit.com:mcp:allternit",
            "displayName": "Allternit MCP Server",
            "type": CARD_TYPE,
            "url": format!("{origin}{CARD_PATH}"),
            "description": DESCRIPTION,
            "tags": ["mcp", "agents", "connectors", "allternit"],
        }]
    })
}

fn respond(content_type: &'static str, body: Value) -> Response {
    let mut resp = Json(body).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    resp
}

async fn card() -> Response {
    respond(CARD_TYPE, server_card(ORIGIN))
}

async fn catalog() -> Response {
    respond(CATALOG_TYPE, ai_catalog(ORIGIN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use tower::ServiceExt;

    async fn get_json(uri: &str) -> (StatusCode, axum::http::HeaderMap, Value) {
        let app = routes::<()>();
        let req = axum::http::Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn server_card_answers_200_with_the_sep_2127_keys() {
        for uri in ["/.well-known/mcp", "/.well-known/mcp/server-card.json"] {
            let (status, headers, card) = get_json(uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert_eq!(headers[header::CONTENT_TYPE], CARD_TYPE);
            assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=3600");
            assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            for k in ["$schema", "name", "version", "description", "remotes", "_meta"] {
                assert!(card.get(k).is_some(), "{uri} missing {k}");
            }
            assert!(card["description"].as_str().unwrap().len() <= 100);
            let remotes = card["remotes"].as_array().unwrap();
            assert_eq!(remotes.len(), 3);
            assert!(remotes.iter().all(|r| r["type"] == "streamable-http"));
            assert_eq!(remotes[0]["url"], "https://mcp.allternit.com/mcp");
            assert_eq!(remotes[1]["url"], "https://mcp.allternit.com/mcp/connectors");
            assert_eq!(remotes[2]["url"], "https://mcp.allternit.com/mcp/bots/{vendor_bot_id}");
            assert_eq!(remotes[0]["supportedProtocolVersions"], json!(mcp_protocol::SUPPORTED));
            for e in ["tasks", "events", "ui", "skills"] {
                assert!(remotes[0]["extensionsInfo"].get(e).is_some(), "{e}");
            }
            assert_eq!(card["_meta"]["com.allternit/auth"]["protectedResourceMetadata"], "https://mcp.allternit.com/.well-known/oauth-protected-resource");
        }
    }

    #[tokio::test]
    async fn ai_catalog_answers_200_and_lists_the_card() {
        let (status, headers, doc) = get_json("/.well-known/ai-catalog.json").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CONTENT_TYPE], CATALOG_TYPE);
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        for k in ["specVersion", "host", "entries"] {
            assert!(doc.get(k).is_some(), "missing {k}");
        }
        let entry = &doc["entries"][0];
        for k in ["identifier", "displayName", "type", "url", "description"] {
            assert!(entry.get(k).is_some(), "entry missing {k}");
        }
        assert_eq!((entry["type"].as_str(), entry["url"].as_str()), (Some(CARD_TYPE), Some("https://mcp.allternit.com/.well-known/mcp")));
    }
}
