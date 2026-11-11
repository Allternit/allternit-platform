//! Service-to-service auth for internal-only routes.
//!
//! No such mechanism existed before this: every existing route is either
//! Clerk-JWT-authenticated (end users) or a public OAuth redirect target
//! (`connector_public_router`). The ACU (computer-use) Python gateway is
//! neither — it's a peer service with no Clerk session — so routes it calls
//! (cloud-credential resolution, usage-event ingestion) are gated by a shared
//! secret instead, following the same fail-closed posture as
//! `token_crypto::open`: no token configured means the route is unreachable,
//! not silently permissive, except for the same localhost-only local-dev
//! bypass `auth_middleware` already uses.

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};

use crate::auth::is_local_request;
use crate::AppState;

const INTERNAL_TOKEN_HEADER: &str = "x-allternit-internal-token";

/// Constant-time comparison -- avoids leaking the secret's length/prefix via
/// early-exit timing, same concern `token_crypto`'s AEAD tag check already
/// handles for encryption.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Gate for internal-only handlers. Call at the top of each handler (axum
/// doesn't make a clean per-route middleware layer worth it for two routes)
/// with the request's `HeaderMap`, its TCP peer (an
/// `Option<ConnectInfo<SocketAddr>>` extractor) and shared `AppState`. The
/// peer decides whether the no-token local-dev bypass applies; headers alone
/// never do.
pub fn require_internal_token(
    headers: &HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    state: &AppState,
) -> Result<(), StatusCode> {
    match state.config.internal_service_token() {
        Some(expected) => {
            let provided = headers
                .get(INTERNAL_TOKEN_HEADER)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if constant_time_eq(provided, &expected) {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        // No token configured: only allow same-machine local dev, same
        // posture as auth_middleware's Clerk bypass -- never permissive
        // against a real (non-localhost) caller.
        None if state.config.local_dev_bypass()
            && is_local_request(headers, peer.map(|ConnectInfo(addr)| addr)) =>
        {
            Ok(())
        }
        None => Err(StatusCode::UNAUTHORIZED),
    }
}
