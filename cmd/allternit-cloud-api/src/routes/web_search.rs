//! POST /api/v1/search/web — Allternit's own web search route.
//!
//! Gizzi Code's WebSearch (the TUI tool and the engine's `websearch`) calls
//! this for any model, signed in with the account or device token, instead of
//! the public keyless Exa MCP endpoint. The search runs on Allternit's Exa
//! account (`EXA_API_KEY`, server-side only); a missing key answers 503 so
//! clients fall back on their own.
//!
//! Body: `{ "query": "...", "num_results": 8 }` (1–10).
//! Answer: `{ "results": [{ "title", "url", "published", "text" }], "provider": "exa" }`.

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth, error::ApiError, ApiState};

const EXA_SEARCH_URL: &str = "https://api.exa.ai/search";
const MAX_QUERY_LEN: usize = 500;
const MAX_RESULTS: u64 = 10;
const DEFAULT_RESULTS: u64 = 8;
/// Text kept per result, so one search stays a small tool result.
const MAX_TEXT_CHARS: usize = 1_500;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/search/web", post(search_web))
}

async fn search_web(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    auth::resolve_user(&state.db, &headers).await?;
    let (query, num_results) = validate(&body)?;
    let key = std::env::var("EXA_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| ApiError::ServiceUnavailable("search_not_configured".into()))?;
    let response = crate::net_client::client()
        .post(EXA_SEARCH_URL)
        .header("x-api-key", key)
        .timeout(std::time::Duration::from_secs(25))
        .json(&json!({
            "query": query,
            "numResults": num_results,
            "type": "auto",
            "contents": { "text": { "maxCharacters": MAX_TEXT_CHARS } },
        }))
        .send()
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("search_unavailable: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError::ServiceUnavailable(format!(
            "search_unavailable: upstream {}",
            response.status().as_u16()
        )));
    }
    let upstream: Value = response
        .json()
        .await
        .map_err(|e| ApiError::ServiceUnavailable(format!("search_unavailable: {e}")))?;
    Ok(Json(json!({ "results": shape_results(&upstream), "provider": "exa" })))
}

/// The query (trimmed, 1–500 chars) and result count (1–10, default 8).
pub(crate) fn validate(body: &Value) -> Result<(String, u64), ApiError> {
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if query.is_empty() || query.chars().count() > MAX_QUERY_LEN {
        return Err(ApiError::BadRequest(format!(
            "query must be 1-{MAX_QUERY_LEN} characters"
        )));
    }
    let num = body
        .get("num_results")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_RESULTS)
        .clamp(1, MAX_RESULTS);
    Ok((query.to_string(), num))
}

/// Exa's `results[]` → our stable shape (title, url, published, text).
pub(crate) fn shape_results(upstream: &Value) -> Vec<Value> {
    upstream
        .get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|r| {
                    let url = r.get("url")?.as_str()?;
                    let text: String = r
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .chars()
                        .take(MAX_TEXT_CHARS)
                        .collect();
                    Some(json!({
                        "title": r.get("title").and_then(Value::as_str).unwrap_or(url),
                        "url": url,
                        "published": r.get("publishedDate").and_then(Value::as_str),
                        "text": text,
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_trims_and_clamps() {
        assert_eq!(validate(&json!({ "query": "  rust  " })).unwrap(), ("rust".into(), 8));
        assert_eq!(validate(&json!({ "query": "x", "num_results": 50 })).unwrap().1, 10);
        assert!(validate(&json!({ "query": "   " })).is_err());
        assert!(validate(&json!({ "query": "a".repeat(501) })).is_err());
    }

    #[test]
    fn shape_results_keeps_url_rows_and_caps_text() {
        let upstream = json!({ "results": [
            { "title": "A", "url": "https://a.dev", "publishedDate": "2026-11-01", "text": "x".repeat(5_000) },
            { "title": "no url" },
            { "url": "https://b.dev" },
        ]});
        let rows = shape_results(&upstream);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["text"].as_str().unwrap().len(), MAX_TEXT_CHARS);
        assert_eq!(rows[1]["title"], "https://b.dev");
    }
}
