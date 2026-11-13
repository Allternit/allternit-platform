//! Allternit Self-Hosted Agents Server.
//!
//! The `allternit-agents-server` binary mounts only [`router`]. It never
//! builds the hosted `AppState`, never warms Clerk JWKS, and never touches
//! Stripe or `cloud_api_url`. Outbound HTTP goes through
//! [`egress_guard::EgressClient`] and nothing else.
//!
//! Contract: `openapi/selfhost-v1.yaml` (routes, errors), with
//! `openapi/selfhost-settings.schema.json` (settings) and
//! `docs/SELFHOST_WORKER_PROTOCOL.md` (workers). Every route mounted here is
//! checked against the OpenAPI paths by `tests/selfhost_routes.rs`.

pub mod admin;
pub mod auth;
pub mod egress_guard;
pub mod metrics_listener;
pub mod routes_agents;
pub mod routes_environments;
pub mod routes_info;
pub mod routes_keys;
pub mod routes_sessions;
pub mod secrets;
pub mod settings;
pub mod state;
pub mod store;
pub mod telemetry;

use std::borrow::Cow;
use std::time::Duration;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request, State};
use axum::handler::Handler;
use axum::http::header::{HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, MethodRouter};
use axum::{async_trait, Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use state::SelfHostState;
use store::StoreError;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// `{"error": {"type", "message", "code"}}`, per the spec's Error schema.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub code: Cow<'static, str>,
    pub message: String,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        kind: &'static str,
        code: impl Into<Cow<'static, str>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            kind,
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn invalid(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", code, message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "authentication_error", "unauthorized", message)
    }
    pub fn forbidden(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "permission_error", code, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found_error", "not_found", message)
    }
    pub fn conflict(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict_error", code, message)
    }
    pub fn too_large(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large_error", code, message)
    }
    pub fn unprocessable(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "unprocessable_error", code, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "api_error", "internal_error", message)
    }
    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_IMPLEMENTED, "api_error", "not_implemented", message)
    }
    pub fn unavailable(code: impl Into<Cow<'static, str>>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "api_error", code, message)
    }
    pub fn body(&self) -> Value {
        json!({ "error": { "type": self.kind, "message": self.message, "code": self.code } })
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body())).into_response()
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.message)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => ApiError::not_found("Not found."),
            StoreError::LeaseStale => ApiError::conflict(
                "lease_stale",
                "The lease generation is old. Another worker owns this item now.",
            ),
            StoreError::ResultTooLarge { size, max } => ApiError::too_large(
                "result_too_large",
                format!("Tool output is {size} bytes. The limit is {max}. Truncate it and post again."),
            ),
            StoreError::Conflict(code) => ApiError::conflict(code, conflict_message(code)),
            StoreError::InvalidCursor => {
                ApiError::invalid("invalid_cursor", "The `after` cursor does not name a row.")
            }
            StoreError::Invalid(m) => ApiError::invalid("invalid_request", m),
            StoreError::Backend(m) => {
                tracing::error!(error = %m, "selfhost store error");
                ApiError::internal("Storage error.")
            }
        }
    }
}

fn conflict_message(code: &str) -> &'static str {
    match code {
        "agent_in_use" => "The agent has a session that is not archived.",
        "environment_in_use" => "The environment has a session that is not archived.",
        "last_admin_key" => "This is the last admin key. Create another admin key first.",
        "setup_already_done" => "An API key already exists. Sign in with an admin key.",
        "tool_call_conflict" => "This tool_call_id belongs to another session.",
        _ => "Conflict.",
    }
}

// ─── Extractors with spec-shaped rejections ─────────────────────────────────

/// `Json<T>` whose rejection is a 400 in the spec's error shape.
pub struct ApiJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(ApiJson(v)),
            Err(rej) => Err(json_rejection(rej)),
        }
    }
}

fn json_rejection(rej: JsonRejection) -> ApiError {
    ApiError::invalid("invalid_body", rej.body_text())
}

/// `Query<T>` whose rejection is a 400 in the spec's error shape.
pub struct ApiQuery<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(v)) => Ok(ApiQuery(v)),
            Err(rej) => Err(query_rejection(rej)),
        }
    }
}

fn query_rejection(rej: QueryRejection) -> ApiError {
    ApiError::invalid("invalid_query", rej.body_text())
}

// ─── Shared response helpers ────────────────────────────────────────────────

pub fn rfc3339(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn rfc3339_opt(dt: Option<DateTime<Utc>>) -> Value {
    dt.map(|d| Value::String(rfc3339(d))).unwrap_or(Value::Null)
}

/// `limit` and `after`, shared by every list route.
#[derive(Debug, Default, Deserialize)]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub after: Option<String>,
}

pub const DEFAULT_PAGE: i64 = 20;
pub const MAX_PAGE: i64 = 100;

pub fn page_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    match limit {
        None => Ok(DEFAULT_PAGE),
        Some(n) if (1..=MAX_PAGE).contains(&n) => Ok(n),
        Some(_) => Err(ApiError::invalid(
            "invalid_limit",
            format!("limit must be between 1 and {MAX_PAGE}."),
        )),
    }
}

/// The ListEnvelope. `rows` holds up to `limit + 1` items; the extra one only
/// says there is another page.
pub fn list_envelope<T>(
    mut rows: Vec<T>,
    limit: i64,
    cursor: impl Fn(&T) -> String,
    render: impl Fn(&T) -> Value,
) -> Value {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit.max(0) as usize);
    let next_cursor = if has_more {
        rows.last().map(|r| Value::String(cursor(r))).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    json!({
        "data": rows.iter().map(render).collect::<Vec<_>>(),
        "has_more": has_more,
        "next_cursor": next_cursor,
    })
}

pub fn deleted(id: &str) -> Value {
    json!({ "id": id, "deleted": true })
}

/// Lengths count characters, like JSON Schema `maxLength`.
pub fn check_len(field: &str, value: &str, min: usize, max: usize) -> Result<(), ApiError> {
    let n = value.chars().count();
    if n < min || n > max {
        return Err(ApiError::invalid(
            "invalid_field",
            format!("{field} must be {min} to {max} characters."),
        ));
    }
    Ok(())
}

/// Exact hostname, optional port; no scheme, path, whitespace or wildcard.
pub fn check_hostname(field: &str, host: &str) -> Result<(), ApiError> {
    if host.is_empty()
        || host.len() > 255
        || host.contains('/')
        || host.contains('*')
        || host.chars().any(char::is_whitespace)
    {
        return Err(ApiError::invalid(
            "invalid_host",
            format!("{field}: {host:?} must be an exact hostname, optionally with a port."),
        ));
    }
    Ok(())
}

/// `Some(None)` for an explicit `null`, `None` for an absent field. Use with
/// `#[serde(default, deserialize_with = "double_option")]`.
pub fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

// ─── Router ─────────────────────────────────────────────────────────────────

type Mr = MethodRouter<SelfHostState>;

/// Routes with their methods recorded, so the route list can be checked
/// against the OpenAPI document. The method label comes from the same helper
/// that builds the handler, so it cannot drift.
pub struct RouteBook {
    router: Router<SelfHostState>,
    routes: Vec<(&'static str, &'static str)>,
}

pub fn on_get<H, T>(h: H) -> (&'static str, Mr)
where
    H: Handler<T, SelfHostState>,
    T: 'static,
{
    ("GET", get(h))
}
pub fn on_post<H, T>(h: H) -> (&'static str, Mr)
where
    H: Handler<T, SelfHostState>,
    T: 'static,
{
    ("POST", post(h))
}
pub fn on_patch<H, T>(h: H) -> (&'static str, Mr)
where
    H: Handler<T, SelfHostState>,
    T: 'static,
{
    ("PATCH", patch(h))
}
pub fn on_delete<H, T>(h: H) -> (&'static str, Mr)
where
    H: Handler<T, SelfHostState>,
    T: 'static,
{
    ("DELETE", delete(h))
}

impl RouteBook {
    pub fn new() -> Self {
        Self {
            router: Router::new(),
            routes: Vec::new(),
        }
    }

    pub fn add(mut self, path: &'static str, handlers: Vec<(&'static str, Mr)>) -> Self {
        let mut combined: Option<Mr> = None;
        for (method, mr) in handlers {
            self.routes.push((method, path));
            combined = Some(match combined {
                None => mr,
                Some(c) => c.merge(mr),
            });
        }
        let mr = combined.expect("a route needs at least one handler");
        self.router = self.router.route(path, mr);
        self
    }

    /// `(METHOD, axum path)` pairs, e.g. `("GET", "/v1/agents/:id")`.
    pub fn routes(&self) -> &[(&'static str, &'static str)] {
        &self.routes
    }
}

impl Default for RouteBook {
    fn default() -> Self {
        Self::new()
    }
}

/// Every selfhost route. Later steps add their modules here.
pub fn route_book() -> RouteBook {
    let book = RouteBook::new()
        .add("/health/live", vec![on_get(health_live)])
        .add("/health/ready", vec![on_get(health_ready)]);
    let book = routes_info::mount(book);
    let book = routes_agents::mount(book);
    let book = routes_environments::mount(book);
    let book = routes_sessions::mount(book);
    routes_keys::mount(book)
}

/// `(METHOD, axum path)` of every mounted route.
pub fn route_table() -> Vec<(&'static str, &'static str)> {
    route_book().routes().to_vec()
}

/// The selfhost router, ready to serve. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()` so the internal
/// routes can see the peer address.
pub fn router(state: SelfHostState) -> Router {
    let cors = cors_layer(&state.settings.server.cors_origins);
    let mut router = route_book()
        .router
        .layer(axum::middleware::from_fn(log_requests));
    if let Some(cors) = cors {
        // Outermost, so a preflight is answered before any auth runs.
        router = router.layer(cors);
    }
    router.with_state(state)
}

/// CORS for exact origins from `server.cors_origins`. Empty list: no layer,
/// no CORS headers. `null` is allowed only when listed literally. No
/// wildcard and no credentials mode: clients send the key in Authorization.
pub fn cors_layer(origins: &[String]) -> Option<CorsLayer> {
    if origins.is_empty() {
        return None;
    }
    let list: Vec<HeaderValue> = origins
        .iter()
        .filter(|o| o.as_str() != "*")
        .filter_map(|o| HeaderValue::from_str(o.trim_end_matches('/')).ok())
        .collect();
    if list.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(list))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                AUTHORIZATION,
                CONTENT_TYPE,
                HeaderName::from_static("idempotency-key"),
                HeaderName::from_static("last-event-id"),
                HeaderName::from_static("x-session-secret"),
            ])
            .expose_headers([HeaderName::from_static("last-event-id")])
            .max_age(Duration::from_secs(600)),
    )
}

async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let started = std::time::Instant::now();
    let res = next.run(req).await;
    tracing::info!(
        method = %method,
        path = %path,
        status = res.status().as_u16(),
        ms = started.elapsed().as_millis() as u64,
        "request"
    );
    res
}

async fn health_live() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn health_ready(State(st): State<SelfHostState>) -> Response {
    match st.store.health().await {
        Ok(()) => Json(json!({ "status": "ok", "store": st.store.backend() })).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "readiness check failed");
            ApiError::unavailable("store_unavailable", "The database is not reachable.")
                .into_response()
        }
    }
}
