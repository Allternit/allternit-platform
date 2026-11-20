//! Our issuer for "Link your Claude organization" (contract B1): the JWKS
//! and discovery document Anthropic reads to verify the client assertions and
//! the Workload Identity Federation JWT that `auth::anthropic_client` signs.
//!
//! - `GET /oauth2/anthropic-client/jwks.json`
//! - `GET /oauth2/anthropic-client/.well-known/openid-configuration`
//!   (`issuer` and `jwks_uri` only)
//!
//! Public, no credential. They need only `ALLTERNIT_ANTHROPIC_CLIENT_ES256_PEM`
//! (not `ALLTERNIT_CLAUDE_LINK`), so the issuer can be registered at
//! Anthropic before the feature is turned on; without the key, 503
//! `not_configured`. The `/oauth2/anthropic-client/` prefix is its own:
//! `managed_auth` owns `/oauth2/org/…` and `edge_auth` `/oauth2/edge/…`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::auth::anthropic_client;

pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/oauth2/anthropic-client/jwks.json", get(jwks))
        .route("/oauth2/anthropic-client/.well-known/openid-configuration", get(discovery))
}

fn not_configured() -> Response {
    super::managed_auth::coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "not_configured",
        "Link your Claude organization is not set up on this server.",
    )
}

async fn jwks() -> Response {
    match anthropic_client::signing_key() {
        Some(key) => Json(key.jwks()).into_response(),
        None => not_configured(),
    }
}

async fn discovery() -> Response {
    if anthropic_client::signing_key().is_none() {
        return not_configured();
    }
    let issuer = anthropic_client::issuer();
    Json(json!({ "issuer": issuer, "jwks_uri": format!("{issuer}/jwks.json") })).into_response()
}
