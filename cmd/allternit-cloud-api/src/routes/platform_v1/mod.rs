//! Allternit Platform API (`/v1`): the developer-facing surface.
//!
//! One module per area (`accounts`, `agents`, `numbers`, `messages`, `webhooks`, `usage`).
//! Each area exposes `register(RouteTable) -> RouteTable`; `build_table` below
//! is the single list. The route table records every `(method, path)` it
//! registers, and the test `openapi_matches_router` compares that list with
//! `openapi/platform-v1.yaml`, so a route can't ship without its spec entry.
//!
//! Everything here is inert until `ALLTERNIT_PLATFORM_API=1` (404
//! `platform_api_disabled`). Console routes for projects and keys live in
//! `console` under `/api/v1/platform/*` (Clerk session auth).
//!
//! Request pipeline for `/v1`: gate → authenticate (`PlatformCaller`) →
//! per-key and per-project rate limit → idempotency (POST + `Idempotency-Key`)
//! → handler.

pub mod accounts;
pub mod agents;
pub mod caller;
pub mod console;
pub mod error;
pub mod events;
pub mod limits;
pub mod messages;
pub mod numbers;
pub mod page;
pub mod projects;
pub mod slots;
pub mod usage;
pub mod usage_events;
pub mod webhooks;

use std::sync::Arc;

use axum::{
    async_trait,
    extract::{rejection::JsonRejection, FromRequest, FromRequestParts, Query, Request},
    http::{header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::MethodRouter,
    Router,
};
use serde::de::DeserializeOwned;

use crate::ApiState;

pub use caller::{PlatformCaller, ProjectEnv};
pub use error::PlatformError;
pub use page::{build_page, Page, PageParams};
pub use slots::{acquire_slot, release_slot, SlotGuard};
pub use usage_events::{record_usage, UsageEvent};

/// Is the Platform API switched on for this process?
pub fn platform_api_enabled() -> bool {
    std::env::var("ALLTERNIT_PLATFORM_API")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Where the on/off switch comes from. `FromEnv` in production; tests force it.
#[derive(Clone, Copy, Debug)]
pub enum Gate {
    FromEnv,
    Forced(bool),
}

impl Gate {
    fn enabled(self) -> bool {
        match self {
            Gate::FromEnv => platform_api_enabled(),
            Gate::Forced(on) => on,
        }
    }
}

/// Collects `/v1` routes and remembers `(METHOD, "/v1/x/{id}")` for the
/// OpenAPI check.
pub struct RouteTable {
    router: Router<Arc<ApiState>>,
    routes: Vec<(&'static str, String)>,
}

impl RouteTable {
    fn new() -> Self {
        Self { router: Router::new(), routes: Vec::new() }
    }

    /// Register `path` (axum `:param` syntax) with the HTTP methods in `verbs`
    /// (the verbs `handlers` actually serves; they are what the spec lists).
    pub fn add(
        mut self,
        path: &'static str,
        verbs: &[&'static str],
        handlers: MethodRouter<Arc<ApiState>>,
    ) -> Self {
        self.router = self.router.route(path, handlers);
        let spec_path = path
            .split('/')
            .map(|seg| match seg.strip_prefix(':') {
                Some(name) => format!("{{{name}}}"),
                None => seg.to_string(),
            })
            .collect::<Vec<_>>()
            .join("/");
        for verb in verbs {
            self.routes.push((verb, spec_path.clone()));
        }
        self
    }
}

fn build_table() -> RouteTable {
    // One line per area. Later phases add theirs here.
    let table = RouteTable::new();
    let table = accounts::register(table);
    let table = agents::register(table);
    let table = numbers::register(table);
    let table = messages::register(table);
    let table = webhooks::register(table);
    usage::register(table)
}

/// `(METHOD, path)` for every registered `/v1` route, with `{param}` paths.
pub fn registered_routes() -> Vec<(&'static str, String)> {
    build_table().routes
}

#[derive(Clone)]
struct PlatformState {
    api: Arc<ApiState>,
    gate: Gate,
}

/// The Platform API router (`/v1/*` plus the console routes). Mount once from
/// `lib.rs`: `.merge(routes::platform_v1::router(&state))`.
pub fn router(state: &Arc<ApiState>) -> Router<Arc<ApiState>> {
    router_gated(state, Gate::FromEnv)
}

pub fn router_gated(state: &Arc<ApiState>, gate: Gate) -> Router<Arc<ApiState>> {
    let ps = PlatformState { api: state.clone(), gate };

    let v1 = build_table()
        .router
        .layer(axum::middleware::from_fn_with_state(ps.clone(), v1_middleware));

    let console = console::routes()
        .layer(axum::middleware::from_fn_with_state(ps, console_gate));

    Router::new().merge(v1).merge(console)
}

async fn console_gate(
    axum::extract::State(ps): axum::extract::State<PlatformState>,
    request: Request,
    next: Next,
) -> Response {
    if !ps.gate.enabled() {
        return PlatformError::disabled().into_response();
    }
    next.run(request).await
}

async fn v1_middleware(
    axum::extract::State(ps): axum::extract::State<PlatformState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !ps.gate.enabled() {
        return PlatformError::disabled().into_response();
    }
    let db = &ps.api.db;

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let caller = match caller::authenticate(db, token).await {
        Ok(c) => c,
        Err(error) => return error.into_response(),
    };

    let rate = match limits::check_rate_limit(db, &caller).await {
        Ok(info) => info,
        Err((error, info)) => {
            let mut response = error.into_response();
            info.apply(response.headers_mut());
            if let Ok(v) = header::HeaderValue::from_str(
                &(info.reset_unix - chrono::Utc::now().timestamp()).max(1).to_string(),
            ) {
                response.headers_mut().insert(header::RETRY_AFTER, v);
            }
            return response;
        }
    };

    request.extensions_mut().insert(caller.clone());
    let mut response = if request.method() == axum::http::Method::POST {
        limits::run_idempotent(db, &caller, request, next).await
    } else {
        next.run(request).await
    };
    rate.apply(response.headers_mut());
    response
}

/// JSON body extractor that reports malformed input in the `/v1` error format.
pub struct ApiJson<T>(pub T);

#[async_trait]
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = PlatformError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(request, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(JsonRejection::MissingJsonContentType(_)) => Err(PlatformError::invalid_request(
                "invalid_content_type",
                "Send a JSON body with 'Content-Type: application/json'.",
            )),
            Err(rejection) => Err(PlatformError::invalid_request(
                "invalid_json",
                rejection.body_text(),
            )),
        }
    }
}

/// Query-string extractor that reports malformed input in the `/v1` error format.
pub struct ApiQuery<T>(pub T);

#[async_trait]
impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = PlatformError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(v)| ApiQuery(v))
            .map_err(|r| PlatformError::invalid_request("invalid_query", r.body_text()))
    }
}

/// `{id}` path params: a missing/foreign row is always 404, never a hint.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", hex::encode(rand::random::<[u8; 12]>()))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_p1;
#[cfg(test)]
mod tests_p2;
