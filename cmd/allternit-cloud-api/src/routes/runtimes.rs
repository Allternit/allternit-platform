//! `GET /api/v1/runtimes`: the runtime registry (Harness & Bots 2.0, contract
//! `CONTRACT-harness-bots-2.md`, "Routes"). Web and phones read it here, so
//! the picker works with no computer. It is the `runtimes[]` array of the same
//! `cli-catalog.json` allternit-api embeds (`cmd/allternit-api/src/cli_catalog.rs`),
//! included at build time the way `gateway_catalog.rs` includes its JSON.
//!
//! Public (no sign-in) and static: `{version, runtimes}` with a strong ETag
//! (SHA-256 of the file, the same value allternit-api sends) and
//! `Cache-Control: public, max-age=3600`; `If-None-Match` answers 304. There
//! is no `installed` here: what is installed is known only on the user's
//! computer, which adds it on its own gateway for its owner.

use std::sync::{Arc, OnceLock};

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ApiState;

const CATALOG_JSON: &str =
    include_str!("../../../gizzi-code/src/runtime/providers/discovery/cli-catalog.json");

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtimes", get(get_runtimes))
}

fn body() -> &'static Value {
    static BODY: OnceLock<Value> = OnceLock::new();
    BODY.get_or_init(|| {
        let raw: Value = serde_json::from_str(CATALOG_JSON).unwrap_or(Value::Null);
        json!({
            "version": raw.get("version").cloned().unwrap_or(Value::Null),
            "runtimes": raw.get("runtimes").cloned().unwrap_or_else(|| json!([])),
        })
    })
}

fn etag_for(catalog_json: &str) -> String {
    let digest = Sha256::digest(catalog_json.as_bytes());
    format!("\"rt-{}\"", hex::encode(&digest[..16]))
}

fn etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| etag_for(CATALOG_JSON))
}

fn etag_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().trim_start_matches("W/"))
        .any(|v| v == "*" || v == etag)
}

async fn get_runtimes(headers: HeaderMap) -> Response {
    let etag = etag();
    let cache = [
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "public, max-age=3600"),
    ];
    if etag_matches(&headers, etag) {
        return (StatusCode::NOT_MODIFIED, cache).into_response();
    }
    (cache, Json(body().clone())).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn router() -> Router {
        Router::new().route("/api/v1/runtimes", get(get_runtimes))
    }

    #[test]
    fn registry_is_v3_with_runtimes() {
        assert!(body()["version"].as_u64().is_some_and(|v| v >= 3), "{}", body()["version"]);
        let runtimes = body()["runtimes"].as_array().expect("runtimes[]");
        assert!(runtimes.len() >= 30, "{} runtimes", runtimes.len());
        assert!(runtimes.iter().any(|r| r["id"] == "allternit"));
    }

    #[tokio::test]
    async fn runtimes_route_is_public_cached_and_has_no_installed() {
        let response = router()
            .oneshot(Request::get("/api/v1/runtimes").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=3600");
        let tag = response.headers()[header::ETAG].to_str().unwrap().to_string();
        assert_eq!(tag, etag_for(CATALOG_JSON));
        let payload: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert!(payload.get("installed").is_none());
        assert!(payload["runtimes"].as_array().is_some_and(|r| r.len() >= 30));

        let response = router()
            .oneshot(
                Request::get("/api/v1/runtimes")
                    .header(header::IF_NONE_MATCH, tag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }
}
