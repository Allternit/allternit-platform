//! Udemy public-catalog proxy routes.
//!
//! Surfaces call `POST /api/v1/udemy/search`; this handler forwards the query
//! to Udemy's public `api-2.0/courses/` endpoint and returns the raw JSON
//! (`count`, `next`, `previous`, `results`) so the web/iOS/gizzi-code models
//! can stay identical.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

pub fn udemy_router() -> Router<Arc<AppState>> {
    Router::new().route("/udemy/search", post(search_udemy))
}

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_page_size")]
    page_size: u32,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    level: Option<String>,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

async fn search_udemy(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<SearchRequest>,
) -> impl IntoResponse {
    let client = crate::net_client::client();

    let mut url = reqwest::Url::parse("https://www.udemy.com/api-2.0/courses/")
        .expect("static Udemy URL is valid");

    let query = body.query.trim();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "query is required" })),
        );
    }

    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("search", query)
            .append_pair("page", &body.page.to_string())
            .append_pair("page_size", &body.page_size.to_string())
            .append_pair("ratings", "4.0,4.5,5.0")
            .append_pair("closed_captions", "en");

        match body.price.as_deref() {
            Some("free") => { qp.append_pair("price", "price-free"); }
            Some("paid") => { qp.append_pair("price", "price-paid"); }
            _ => {}
        }

        match body.level.as_deref() {
            Some("beginner") => { qp.append_pair("instructional_level", "beginner"); }
            Some("intermediate") => { qp.append_pair("instructional_level", "intermediate"); }
            Some("expert") => { qp.append_pair("instructional_level", "expert"); }
            _ => {}
        }
    }

    let response = client
        .get(url)
        .header("Accept", "application/json, text/plain, */*")
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "Udemy request failed", "details": text, "status": status.as_u16() })),
                );
            }

            match resp.json::<serde_json::Value>().await {
                Ok(value) => (StatusCode::OK, Json(value)),
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": "Failed to decode Udemy response", "details": e.to_string() })),
                ),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "Udemy request failed", "details": e.to_string() })),
        ),
    }
}
