//! Shared data-plane gateway seam for P1 control-plane namespaces (tranche 1+).
//!
//! Every node-stateful namespace handler (agent-sessions, office, beta, later
//! jobs/rails) follows the same four-step design from
//! docs/architecture/2026-09-04-p1-route-inventory.md §3:
//!
//! 1. **Auth** — `auth::resolve_user_scoped(.., "compute")`, the same
//!    Clerk-first resolver the runtime relay uses.
//! 2. **Node resolution** — the caller's default data-plane node via
//!    [`DataPlaneGateway::resolve_default_node`] (services::node_resolution);
//!    428 "pair a device" when the account has no healthy node.
//! 3. **Relay** — the request is forwarded through the EXISTING outbound-WS
//!    relay (`runtime_relay::relay_request_to_runtime`): same allow-list,
//!    same wake-on-demand, same `Body::from_stream` chunked responses. SSE
//!    is never buffered; `text/event-stream` passes the response header
//!    filter.
//! 4. **Cache nothing, transform nothing** — v1 is a faithful proxy.
//!
//! The gateway trait is the one seam all namespace handlers share, so handler
//! tests mock node resolution + relay once and verify auth gating and
//! path/method wiring without a runtime on the other end. The production
//! implementation ([`PgDataPlaneGateway`]) holds the same clones `ApiState`
//! holds so it can be constructed before the state and stored in it without
//! circular references.

use async_trait::async_trait;
use axum::{extract::RawQuery, http::HeaderMap, response::Response};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::sync::Arc;

use super::runtime_relay::{relay_headers_from_http, RelayRequest};
use crate::{
    services::{resolve_default_node_preferring, ContaboRuntimeService, PgNodeStore, ResolvedNode, SharedQuotaService},
    ApiError, ApiState,
};

/// The service boundary every P1 namespace handler depends on: node
/// resolution + one relay invocation.
#[async_trait]
pub trait DataPlaneGateway: Send + Sync {
    async fn resolve_default_node(&self, user_id: &str) -> Result<ResolvedNode, ApiError>;
    async fn relay(
        &self,
        user_id: &str,
        device_id: &str,
        request: RelayRequest,
    ) -> Result<Response, ApiError>;
}

/// Production gateway: resolves nodes from `runtime_devices` and relays
/// through the existing `runtime_relay` machinery.
pub struct PgDataPlaneGateway {
    db: sqlx::PgPool,
    contabo_runtime_service: Arc<ContaboRuntimeService>,
    quota_service: SharedQuotaService,
}

impl PgDataPlaneGateway {
    pub fn new(
        db: sqlx::PgPool,
        contabo_runtime_service: Arc<ContaboRuntimeService>,
        quota_service: SharedQuotaService,
    ) -> Self {
        Self {
            db,
            contabo_runtime_service,
            quota_service,
        }
    }
}

#[async_trait]
impl DataPlaneGateway for PgDataPlaneGateway {
    async fn resolve_default_node(&self, user_id: &str) -> Result<ResolvedNode, ApiError> {
        let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
        resolve_default_node_preferring(&PgNodeStore::new(&self.db), user_id, &connected).await
    }

    async fn relay(
        &self,
        user_id: &str,
        device_id: &str,
        request: RelayRequest,
    ) -> Result<Response, ApiError> {
        super::runtime_relay::relay_request_to_runtime(
            &self.db,
            &self.contabo_runtime_service,
            &self.quota_service,
            user_id,
            device_id,
            request,
        )
        .await
    }
}

/// Universal P1 handler core: Clerk auth → default-node resolution → relay.
/// `path` is the exact :8013 path to proxy (e.g. `/api/v1/office/bindings`),
/// including any query string.
pub(crate) async fn relay_data_plane_request(
    state: &ApiState,
    headers: &HeaderMap,
    method: &str,
    path: String,
    body: &[u8],
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, headers, "compute").await?;
    let node = state.data_plane_gateway.resolve_default_node(&user.id).await?;
    let request = RelayRequest {
        method: method.to_string(),
        path,
        headers: relay_headers_from_http(headers),
        body: STANDARD.encode(body),
        body_encoding: "base64".to_string(),
    };
    state
        .data_plane_gateway
        .relay(&user.id, &node.device_id, request)
        .await
}

/// Append a request's raw query string to a relay path, when present.
pub(crate) fn with_query(base: &str, query: &RawQuery) -> String {
    match &query.0 {
        Some(query) if !query.is_empty() => format!("{base}?{query}"),
        _ => base.to_string(),
    }
}
