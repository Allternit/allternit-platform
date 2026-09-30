//! Agency API (WP11) + receipt JWKS forward: api.allternit.com → the node.
//!
//! The Agency API lives in allternit-api (`agency_api::agency_router`) on the
//! same host, bound on :8013. That node trusts loopback callers, so a blind
//! forward from cloud-api would hand every internet caller the node's local
//! identity. This module therefore:
//!
//! 1. **Authenticates first** with cloud-api's existing resolver
//!    (`auth::resolve_user_scoped(.., "compute")` — Clerk session or an
//!    `allternit_*` API token, the same gate the data-plane namespaces use).
//!    Unauthenticated calls get 401 and never reach the node.
//! 2. **Asserts the resolved identity** to the node through the node's
//!    existing trusted-peer path: `x-allternit-user-id` (+ organization id
//!    when the Clerk session carries one) plus the shared
//!    `x-allternit-desktop-access-token` secret. Caller-supplied identity
//!    headers and the caller's `Authorization` are never forwarded. With no
//!    access token configured the routes fail closed (503) instead of
//!    falling back to loopback trust, and `Host` is pinned to the public
//!    name so the node's localhost-origin fallback can never match.
//! 3. **Streams** the response (`Body::from_stream`), so `text/event-stream`
//!    run events flow through unbuffered; `Idempotency-Key` and
//!    `Last-Event-ID` pass through.
//!
//! `GET /.well-known/jwks.json` is public: receipts are verified offline by
//! anyone. It is cached briefly and re-sanitized (private JWK members are
//! stripped even if the node ever leaked one).
//!
//! Not forwarded: `/v1/models` (cloud-api's own public model catalog; the
//! node's Agency API does not serve it either).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Extension, Json, Router,
};
use futures_util::TryStreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::ApiState;

/// Env var for the node base URL (not a secret).
pub const NODE_URL_ENV: &str = "ALLTERNIT_AGENCY_NODE_URL";
/// Env var for the shared trusted-peer secret; must equal the node's
/// `ALLTERNIT_DESKTOP_ACCESS_TOKEN`.
pub const NODE_ACCESS_TOKEN_ENV: &str = "ALLTERNIT_AGENCY_NODE_ACCESS_TOKEN";
pub const DEFAULT_NODE_URL: &str = "http://127.0.0.1:8013";
/// Public host asserted to the node (never a loopback name).
pub const FORWARD_HOST: &str = "api.allternit.com";

const JWKS_TTL: Duration = Duration::from_secs(60);
const MAX_FORWARD_BODY: usize = 10 * 1024 * 1024;

/// Request headers that pass from the caller to the node, verbatim.
const PASSTHROUGH_REQUEST_HEADERS: &[&str] = &[
    "content-type",
    "accept",
    "idempotency-key",
    "last-event-id",
    "x-request-id",
    "cache-control",
];

/// Response headers never copied back (hop-by-hop, or recomputed by hyper).
const DROPPED_RESPONSE_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
    "proxy-connection",
    "upgrade",
    "trailer",
];

/// Private JWK members (RFC 7518 §6.2.2, §6.3.2, §6.4) — never served.
const PRIVATE_JWK_MEMBERS: &[&str] = &["d", "p", "q", "dp", "dq", "qi", "oth", "k"];

pub struct AgencyForward {
    node_url: String,
    access_token: Option<String>,
    client: reqwest::Client,
    jwks_cache: Mutex<Option<(Instant, Bytes)>>,
}

impl AgencyForward {
    pub fn new(node_url: impl Into<String>, access_token: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // No overall timeout: run event streams are long-lived.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client");
        Self {
            node_url: node_url.into().trim_end_matches('/').to_string(),
            access_token: access_token.filter(|t| !t.is_empty()),
            client,
            jwks_cache: Mutex::new(None),
        }
    }

    pub fn from_env() -> Self {
        let node_url = std::env::var(NODE_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_NODE_URL.to_string());
        let token = std::env::var(NODE_ACCESS_TOKEN_ENV).ok();
        if token.as_deref().map_or(true, str::is_empty) {
            tracing::warn!(
                "{NODE_ACCESS_TOKEN_ENV} unset: Agency API forward answers 503 (JWKS still served)"
            );
        }
        Self::new(node_url, token)
    }
}

/// Agency routes (mirrors `agency_api::agency_router` in allternit-api) plus
/// the public JWKS. Auth is enforced per-request inside [`forward`], so the
/// router is merged with the public (Clerk-or-token) routes.
pub fn routes(forward: Arc<AgencyForward>) -> Router<Arc<ApiState>> {
    Router::new()
        .route("/.well-known/jwks.json", get(jwks))
        .route("/v1/agency", any(forward_handler))
        .route("/v1/runs", any(forward_handler))
        .route("/v1/runs/:run_id", any(forward_handler))
        .route("/v1/runs/:run_id/*rest", any(forward_handler))
        .route("/v1/receipts/:receipt_id", any(forward_handler))
        .route("/v1/attention", any(forward_handler))
        .route("/v1/attention/*rest", any(forward_handler))
        .route("/v1/campaigns", any(forward_handler))
        .route("/v1/campaigns/*rest", any(forward_handler))
        .route("/v1/replays", any(forward_handler))
        .route("/v1/replays/*rest", any(forward_handler))
        .route("/v1/decisions", any(forward_handler))
        .route("/v1/capabilities", any(forward_handler))
        .route("/v1/agents", any(forward_handler))
        .route("/v1/authority-profiles", any(forward_handler))
        .route("/v1/completion-criteria", any(forward_handler))
        .layer(Extension(forward))
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

async fn forward_handler(
    State(state): State<Arc<ApiState>>,
    Extension(fwd): Extension<Arc<AgencyForward>>,
    request: Request,
) -> Response {
    // 1. Auth before anything touches the node.
    let user = match crate::auth::resolve_user_scoped(&state.db, request.headers(), "compute").await
    {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };
    // 2. Fail closed: without the trusted-peer secret the node could only
    //    see us as a loopback caller.
    let Some(access_token) = fwd.access_token.clone() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "agency_not_configured",
            "The Agency API is not configured on this server",
        );
    };

    let (parts, body) = request.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| parts.uri.path().to_string());
    let body = match axum::body::to_bytes(body, MAX_FORWARD_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Request body too large",
            )
        }
    };

    let mut headers = reqwest::header::HeaderMap::new();
    for name in PASSTHROUGH_REQUEST_HEADERS {
        if let Some(v) = parts.headers.get(*name) {
            headers.insert(HeaderName::from_static(name), v.clone());
        }
    }
    headers.insert(header::HOST, HeaderValue::from_static(FORWARD_HOST));
    let identity = [
        ("x-allternit-user-id", Some(user.id.clone())),
        ("x-allternit-organization-id", user.organization_id.clone()),
        ("x-allternit-tenant-id", user.organization_id.clone()),
        ("x-allternit-user-email", user.email.clone()),
        ("x-allternit-desktop-access-token", Some(access_token)),
    ];
    for (name, value) in identity {
        if let Some(value) = value.and_then(|v| HeaderValue::from_str(&v).ok()) {
            headers.insert(HeaderName::from_static(name), value);
        }
    }

    let url = format!("{}{}", fwd.node_url, path_and_query);
    let upstream = match fwd
        .client
        .request(parts.method, &url)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, "agency forward: node unreachable");
            return error(
                StatusCode::BAD_GATEWAY,
                "agency_unavailable",
                "The Agency API node is unreachable",
            );
        }
    };
    stream_response(upstream)
}

fn stream_response(upstream: reqwest::Response) -> Response {
    let status = upstream.status();
    let mut out_headers = HeaderMap::new();
    for (name, value) in upstream.headers() {
        if !DROPPED_RESPONSE_HEADERS.contains(&name.as_str()) {
            out_headers.append(name.clone(), value.clone());
        }
    }
    let is_sse = out_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if is_sse {
        // Keep nginx from buffering the event stream in front of us.
        out_headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    }
    let body = Body::from_stream(upstream.bytes_stream().map_err(std::io::Error::other));
    let mut response = Response::new(body);
    *response.status_mut() = status;
    *response.headers_mut() = out_headers;
    response
}

/// Strip private members from every key; `None` when it is not a JWK Set.
pub fn sanitize_jwks(raw: &[u8]) -> Option<Bytes> {
    let mut doc: Value = serde_json::from_slice(raw).ok()?;
    let keys = doc.get_mut("keys")?.as_array_mut()?;
    for key in keys.iter_mut() {
        let obj = key.as_object_mut()?;
        for member in PRIVATE_JWK_MEMBERS {
            obj.remove(*member);
        }
    }
    serde_json::to_vec(&doc).ok().map(Bytes::from)
}

async fn jwks(Extension(fwd): Extension<Arc<AgencyForward>>) -> Response {
    let mut cache = fwd.jwks_cache.lock().await;
    if let Some((at, body)) = cache.as_ref() {
        if at.elapsed() < JWKS_TTL {
            return jwks_response(body.clone());
        }
    }
    let fetched = async {
        let resp = fwd
            .client
            .get(format!("{}/.well-known/jwks.json", fwd.node_url))
            .header(header::HOST, FORWARD_HOST)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        sanitize_jwks(&resp.bytes().await.ok()?)
    }
    .await;
    match fetched {
        Some(body) => {
            *cache = Some((Instant::now(), body.clone()));
            jwks_response(body)
        }
        // Node hiccup: serve the last good set rather than break verifiers.
        None => match cache.as_ref() {
            Some((_, body)) => jwks_response(body.clone()),
            None => error(
                StatusCode::BAD_GATEWAY,
                "jwks_unavailable",
                "Receipt keys are temporarily unavailable",
            ),
        },
    }
}

fn jwks_response(body: Bytes) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{test_state, MockGateway, DEV_USER};
    use axum::http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    const PEER_TOKEN: &str = "peer-secret-for-tests";

    type Seen = Arc<StdMutex<Vec<axum::http::HeaderMap>>>;

    /// A fake node: JWKS (with a leaked `d`), a JSON route that records the
    /// headers it received, and an SSE route that emits its second event
    /// only after the test signals it (proves no buffering).
    async fn spawn_node(release: Arc<tokio::sync::Notify>) -> (String, Seen) {
        let seen: Seen = Arc::new(StdMutex::new(Vec::new()));
        let seen_c = seen.clone();
        let app = Router::new()
            .route(
                "/.well-known/jwks.json",
                get(|| async {
                    Json(json!({"keys":[{"kty":"OKP","crv":"Ed25519","kid":"k1","x":"pub","d":"SECRET"}]}))
                }),
            )
            .route(
                "/v1/agency",
                any(move |headers: axum::http::HeaderMap| {
                    let seen = seen_c.clone();
                    async move {
                        seen.lock().unwrap().push(headers);
                        (StatusCode::ACCEPTED, Json(json!({"object":"run","id":"run_1"})))
                    }
                }),
            )
            .route(
                "/v1/runs/:id/events",
                get(move || {
                    let release = release.clone();
                    async move {
                        let stream = async_stream(release);
                        (
                            [(header::CONTENT_TYPE, "text/event-stream")],
                            Body::from_stream(stream),
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    fn async_stream(
        release: Arc<tokio::sync::Notify>,
    ) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> {
        futures_util::stream::unfold(0u8, move |step| {
            let release = release.clone();
            async move {
                match step {
                    0 => Some((Ok(Bytes::from_static(b"id: 1\ndata: first\n\n")), 1)),
                    1 => {
                        release.notified().await;
                        Some((Ok(Bytes::from_static(b"id: 2\ndata: second\n\n")), 2))
                    }
                    _ => None,
                }
            }
        })
    }

    async fn router(node_url: &str, token: Option<&str>) -> Router {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        routes(Arc::new(AgencyForward::new(node_url, token.map(String::from)))).with_state(state)
    }

    fn req(method: &str, path: &str) -> axum::http::request::Builder {
        HttpRequest::builder().method(method).uri(path)
    }

    #[tokio::test]
    async fn jwks_is_public_cached_and_strips_private_members() {
        let (url, _) = spawn_node(Arc::new(tokio::sync::Notify::new())).await;
        let app = router(&url, None).await;
        let resp = app
            .clone()
            .oneshot(req("GET", "/.well-known/jwks.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("max-age=60"));
        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["keys"][0]["kid"], "k1");
        assert_eq!(body["keys"][0]["x"], "pub");
        assert!(body["keys"][0].get("d").is_none(), "private key leaked: {body}");
    }

    #[tokio::test]
    async fn agency_routes_require_cloud_api_auth() {
        let (url, seen) = spawn_node(Arc::new(tokio::sync::Notify::new())).await;
        let app = router(&url, Some(PEER_TOKEN)).await;
        for (method, path) in [
            ("POST", "/v1/agency"),
            ("GET", "/v1/runs"),
            ("GET", "/v1/runs/run_1/events"),
            ("GET", "/v1/receipts/rcpt_1"),
            ("GET", "/v1/attention"),
            ("POST", "/v1/campaigns"),
            ("GET", "/v1/replays/rp_1"),
            ("GET", "/v1/capabilities"),
            ("GET", "/v1/agents"),
            ("GET", "/v1/authority-profiles"),
            ("GET", "/v1/completion-criteria"),
            ("POST", "/v1/decisions"),
        ] {
            // A spoofed identity header must not substitute for auth.
            let resp = app
                .clone()
                .oneshot(
                    req(method, path)
                        .header("x-allternit-user-id", "victim")
                        .header("x-allternit-desktop-access-token", PEER_TOKEN)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
        }
        assert!(seen.lock().unwrap().is_empty(), "unauthenticated call reached the node");
    }

    #[tokio::test]
    async fn forwards_identity_and_passthrough_headers_only() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let (url, seen) = spawn_node(Arc::new(tokio::sync::Notify::new())).await;
        let app = router(&url, Some(PEER_TOKEN)).await;
        let resp = app
            .oneshot(
                req("POST", "/v1/agency?x=1")
                    .header("authorization", "Bearer dev-api-token")
                    .header("content-type", "application/json")
                    .header("idempotency-key", "idem-123")
                    .header("last-event-id", "42")
                    .header("x-allternit-user-id", "spoofed")
                    .header("x-allternit-organization-id", "org_spoofed")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let seen = seen.lock().unwrap();
        let h = &seen[0];
        assert_eq!(h["idempotency-key"], "idem-123");
        assert_eq!(h["last-event-id"], "42");
        assert_eq!(h["x-allternit-user-id"], DEV_USER);
        assert_eq!(h["x-allternit-desktop-access-token"], PEER_TOKEN);
        assert_eq!(h[header::HOST], FORWARD_HOST);
        assert!(h.get("authorization").is_none(), "caller token forwarded");
        assert!(h.get("x-allternit-organization-id").is_none(), "spoofed org forwarded");
    }

    #[tokio::test]
    async fn fails_closed_without_peer_token() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let (url, seen) = spawn_node(Arc::new(tokio::sync::Notify::new())).await;
        let app = router(&url, None).await;
        let resp = app
            .oneshot(
                req("GET", "/v1/runs")
                    .header("authorization", "Bearer dev-api-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn sse_streams_without_buffering() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let release = Arc::new(tokio::sync::Notify::new());
        let (url, _) = spawn_node(release.clone()).await;
        let app = router(&url, Some(PEER_TOKEN)).await;
        let resp = app
            .oneshot(
                req("GET", "/v1/runs/run_1/events")
                    .header("authorization", "Bearer dev-api-token")
                    .header("accept", "text/event-stream")
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(resp.headers()["x-accel-buffering"], "no");
        let mut body = resp.into_body();
        // First event arrives while the node is still holding the second.
        let first = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("first event must stream before the node finishes")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(std::str::from_utf8(&first).unwrap().contains("data: first"));
        release.notify_one();
        let rest = body.collect().await.unwrap().to_bytes();
        assert!(std::str::from_utf8(&rest).unwrap().contains("data: second"));
    }
}
