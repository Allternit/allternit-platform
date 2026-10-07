//! Allternit Platform API (`/v1`): the developer-facing surface.
//!
//! One module per area (`accounts`, `agents`, `conversations`, `numbers`, `messages`, `calls` (voice and realtime), `webhooks`, `usage`).
//! Hosted agents run on a per-project runtime (`hosting`).
//! Each area exposes `register(RouteTable) -> RouteTable`; `build_table` below
//! is the single list. The route table records every `(method, path)` it
//! registers, and the test `openapi_matches_router` compares that list with
//! `openapi/platform-v1.yaml`, so a route can't ship without its spec entry.
//!
//! Everything here is inert until `ALLTERNIT_PLATFORM_API=1` (404
//! `platform_api_disabled`). Console routes for projects and keys live in
//! `console` under `/api/v1/platform/*` (Clerk session auth).
//!
//! The console also calls `/v1` itself: a Clerk session plus
//! `X-Allternit-Project: proj_…` acts on that project as its owner (see
//! [`caller::console_caller`]), so console pages use the same endpoints as
//! developers and need no parallel admin routes.
//!
//! Request pipeline for `/v1`: gate → authenticate (`PlatformCaller`) →
//! per-key and per-project rate limit → idempotency (POST + `Idempotency-Key`)
//! → handler.

pub mod accounts;
pub mod agent_tools;
pub mod agents;
pub mod caller;
pub mod calls;
pub mod console;
pub mod conversations;
pub mod error;
pub mod events;
pub mod hosting;
pub mod knowledge;
pub mod limits;
pub mod messages;
pub mod model_keys;
pub mod numbers;
pub mod page;
pub mod projects;
pub mod slots;
pub mod spend;
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

/// Header the console sends with its Clerk session to act on one project.
pub const CONSOLE_PROJECT_HEADER: &str = "x-allternit-project";

/// A principal tests put in place of a verified Clerk session.
#[cfg(test)]
#[derive(Clone)]
pub struct TestSession(pub projects::Principal);

fn console_project(request: &Request) -> Option<String> {
    request
        .headers()
        .get(CONSOLE_PROJECT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Console session → caller for the project named in [`CONSOLE_PROJECT_HEADER`].
/// Takes the headers (and a test principal) by value: a `&Request` held across
/// an await would make the middleware future non-`Send`.
async fn console_v1_caller(
    db: &sqlx::PgPool,
    headers: axum::http::HeaderMap,
    test_principal: Option<projects::Principal>,
    project_id: &str,
) -> Result<PlatformCaller, PlatformError> {
    let who = match test_principal {
        Some(who) => who,
        None => console::principal(&headers).await?.0,
    };
    caller::console_caller(db, &who, project_id).await
}

#[cfg(test)]
fn test_principal(request: &Request) -> Option<projects::Principal> {
    request.extensions().get::<TestSession>().map(|t| t.0.clone())
}

#[cfg(not(test))]
fn test_principal(_request: &Request) -> Option<projects::Principal> {
    None
}

/// Is the Platform API switched on for this process?
pub fn platform_api_enabled() -> bool {
    std::env::var("ALLTERNIT_PLATFORM_API")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Clerk user ids allowed to use the Platform API while it is switched off for
/// everyone else (`ALLTERNIT_PLATFORM_API_BETA_OWNERS`, comma separated): their
/// console and the projects they own work; every other caller still gets
/// `platform_api_disabled`. Used for live tests before launch.
pub fn beta_owners_from_env() -> Vec<String> {
    std::env::var("ALLTERNIT_PLATFORM_API_BETA_OWNERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Where the on/off switch comes from. `FromEnv` in production; tests force it.
#[derive(Clone, Debug)]
pub enum Gate {
    FromEnv,
    Forced(bool),
    /// Off, except for these owners (tests of the beta path).
    BetaOnly(Vec<String>),
}

impl Gate {
    fn enabled(&self) -> bool {
        match self {
            Gate::FromEnv => platform_api_enabled(),
            Gate::Forced(on) => *on,
            Gate::BetaOnly(_) => false,
        }
    }

    fn beta_owners(&self) -> Vec<String> {
        match self {
            Gate::FromEnv => beta_owners_from_env(),
            Gate::Forced(_) => Vec::new(),
            Gate::BetaOnly(owners) => owners.clone(),
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
    let table = knowledge::register(table);
    let table = conversations::register(table);
    let table = model_keys::register(table);
    let table = numbers::register(table);
    let table = messages::register(table);
    let table = calls::register(table);
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
    /// `/v1/agents` is also the Agency API's path: non-project credentials go there.
    agency: Arc<crate::routes::agency_forward::AgencyForward>,
}

/// The Platform API router (`/v1/*` plus the console routes). Mount once from
/// `lib.rs`: `.merge(routes::platform_v1::router(&state))`.
pub fn router(state: &Arc<ApiState>) -> Router<Arc<ApiState>> {
    router_gated(state, Gate::FromEnv)
}

pub fn router_gated(state: &Arc<ApiState>, gate: Gate) -> Router<Arc<ApiState>> {
    router_with_agency(state, gate, Arc::new(crate::routes::agency_forward::AgencyForward::from_env()))
}

pub fn router_with_agency(state: &Arc<ApiState>, gate: Gate, agency: Arc<crate::routes::agency_forward::AgencyForward>) -> Router<Arc<ApiState>> {
    let ps = PlatformState { api: state.clone(), gate, agency };

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
        let beta = ps.gate.beta_owners();
        let allowed = !beta.is_empty()
            && match console::principal(request.headers()).await {
                Ok((p, _)) => beta.contains(&p.user_id),
                Err(_) => false,
            };
        if !allowed {
            return PlatformError::disabled().into_response();
        }
    }
    next.run(request).await
}

async fn v1_middleware(
    axum::extract::State(ps): axum::extract::State<PlatformState>,
    mut request: Request,
    next: Next,
) -> Response {
    // The Agency API owns `/v1/agents` for everything but project keys, whether or not
    // the Platform API is switched on.
    let console_project = console_project(&request);
    if request.uri().path() == "/v1/agents" {
        let project_key = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|t| caller::is_project_key_token(t.trim()));
        if !project_key && console_project.is_none() {
            return crate::routes::agency_forward::forward(ps.api.clone(), ps.agency.clone(), request).await;
        }
    }
    let beta = if ps.gate.enabled() { None } else { Some(ps.gate.beta_owners()) };
    if beta.as_ref().is_some_and(Vec::is_empty) {
        return PlatformError::disabled().into_response();
    }
    let db = &ps.api.db;

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let authenticated = match (&console_project, token) {
        // A project key always wins; the console header only applies to a session.
        (Some(project_id), Some(t)) if !caller::is_project_key_token(t) => {
            let (headers, test) = (request.headers().clone(), test_principal(&request));
            console_v1_caller(db, headers, test, project_id).await
        }
        _ => caller::authenticate(db, token).await,
    };
    let caller = match authenticated {
        Ok(c) => c,
        // While switched off, nobody learns more than "disabled" from a bad key.
        Err(_) if beta.is_some() => return PlatformError::disabled().into_response(),
        Err(error) => return error.into_response(),
    };
    if beta.as_ref().is_some_and(|owners| !owners.contains(&caller.owner_user_id)) {
        return PlatformError::disabled().into_response();
    }

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
#[cfg(test)]
mod tests_p2_tools;
#[cfg(test)]
mod tests_p5_console;
#[cfg(test)]
mod tests_p3;
