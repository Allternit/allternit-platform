//! Dynamic model catalog routes.

use crate::catalog::CatalogSource;
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Catalog query parameters.
#[derive(Debug, Deserialize)]
pub struct CatalogQuery {
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_source() -> String {
    "all".to_string()
}

fn default_limit() -> usize {
    50
}

/// Catalog list response.
#[derive(Debug, Serialize)]
pub struct CatalogResponse {
    pub models: Vec<crate::catalog::CatalogEntry>,
    pub count: usize,
    /// Epoch seconds of the last successful HF poll; absent if never polled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
    /// Whether the cached poll is missing or older than 30 minutes.
    pub stale: bool,
}

/// Refresh response.
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub refreshed: bool,
    pub count: usize,
}

/// Error response.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/catalog", get(list_catalog))
        .route("/catalog/refresh", post(refresh_catalog))
        .with_state(state)
}

async fn list_catalog(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CatalogQuery>,
) -> Json<CatalogResponse> {
    let source = CatalogSource::parse(&query.source);
    let limit = query.limit.clamp(1, 200);
    // Stale-while-revalidate: kick off a background refresh when the cache is
    // older than 30 minutes, then serve whatever we have immediately. The
    // `stale` flag reflects the age of the data actually being served.
    state
        .catalog
        .spawn_refresh_if_stale(crate::catalog::STALE_AFTER)
        .await;
    let stale = state.catalog.is_stale(crate::catalog::STALE_AFTER).await;
    let models = state.catalog.catalog(source, limit).await;
    let count = models.len();
    let fetched_at = state.catalog.fetched_at().await;
    Json(CatalogResponse {
        models,
        count,
        fetched_at,
        stale,
    })
}

async fn refresh_catalog(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RefreshResponse>, (StatusCode, Json<ErrorResponse>)> {
    match state.catalog.refresh().await {
        Ok(count) => Ok(Json(RefreshResponse {
            refreshed: true,
            count,
        })),
        Err(err) => Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("failed to refresh catalog: {}", err),
            }),
        )),
    }
}
