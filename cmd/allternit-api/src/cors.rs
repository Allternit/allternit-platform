//! CORS policy for the API.
//!
//! The API is served both to local/desktop callers (which carry no `Origin`
//! header and are unaffected by CORS) and to browsers on the public
//! `api.allternit.com` origin, where mirroring any origin is no longer
//! acceptable. The policy matrix:
//!
//! - `ALLTERNIT_LOCAL_DEV_BYPASS=true` — keep the legacy permissive behavior
//!   (`Access-Control-Allow-Origin` mirrors the request origin). Intended only
//!   for local development.
//! - Otherwise — an explicit origin allowlist: `ALLTERNIT_CORS_ORIGINS`
//!   (comma-separated) when set, else [`DEFAULT_ALLOWED_ORIGINS`]; plus the
//!   API's own loopback origins for its listen port (see [`self_origins`] —
//!   pages the API serves itself, e.g. the `/embed/computers/:id` viewer,
//!   send their own `Origin` on the VNC websocket upgrade); plus any exact
//!   origins ADDED via [`EXTRA_ALLOWED_ORIGINS_ENV`]
//!   (`ALLTERNIT_ALLOWED_ORIGINS`, see [`parse_extra_origins`]). Requests
//!   without an `Origin` header always pass (non-browser clients such as the
//!   packaged desktop launcher, git, curl, and service-to-service calls).
//!   Requests with a disallowed `Origin` are rejected with 403 by
//!   [`origin_gate`], including preflights; allowed origins get the full
//!   preflight response and `Vary: Origin` from `tower-http`.
//!
//! Self-hosted (VPS Desktop-Cloud) deployments do not need a separate mode:
//! the default allowlist already contains `platform.allternit.com` and
//! `ai.allternit.com`, whose browsers reach the VPS through the nginx proxy
//! with those origins intact.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::config::AppConfig;

/// Origins allowed to make cross-origin browser calls when
/// `ALLTERNIT_CORS_ORIGINS` is not set. Covers the public surfaces, local Vite
/// (5173) / Next.js (3000) dev servers, this API's own UI (8013), the
/// packaged desktop launcher UI (`http://127.0.0.1:3456`, see
/// `cmd/launcher/src/main.rs`), the desktop shell dev server (3014,
/// `devUiUrl` when the Electron app runs the UI from Vite in dev mode), the
/// ai.allternit.com web surface dev server (3013, see
/// `surfaces/ai.allternit.com/vite.config.ts`), and the hosted Microsoft
/// Office add-in task panes (Word/Excel/PowerPoint), which deploy to
/// Cloudflare Pages at `allternit-office-addins.pages.dev`
/// and will move to the `office-addins.allternit.com` custom domain.
pub const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[
    "https://platform.allternit.com",
    "https://ai.allternit.com",
    "https://allternit-office-addins.pages.dev",
    "https://office-addins.allternit.com",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    "http://localhost:3000",
    "http://127.0.0.1:3000",
    "http://localhost:8013",
    "http://127.0.0.1:8013",
    "http://localhost:3456",
    "http://127.0.0.1:3456",
    "http://localhost:3014",
    "http://127.0.0.1:3014",
    "http://localhost:3013",
    "http://127.0.0.1:3013",
];

/// Env var that ADDS exact origins to the allowlist (on top of the defaults
/// or `ALLTERNIT_CORS_ORIGINS`), e.g.
/// `ALLTERNIT_ALLOWED_ORIGINS=http://127.0.0.1:18013,http://127.0.0.1:18014`.
/// Entries must be exact `scheme://host[:port]` origins — no wildcards, no
/// paths; see [`normalize_origin`].
pub const EXTRA_ALLOWED_ORIGINS_ENV: &str = "ALLTERNIT_ALLOWED_ORIGINS";

/// Normalize one configured origin to the exact serialization browsers send
/// in the `Origin` header (`scheme://host[:port]`, lowercase host, default
/// port omitted). Only `http`/`https` origins with a host are accepted;
/// wildcards, userinfo, paths (other than a bare trailing `/`), queries, and
/// fragments are rejected so an entry can never widen into a pattern.
pub fn normalize_origin(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.contains('*') {
        return Err("wildcards are not allowed; list exact origins".into());
    }
    let url = url::Url::parse(raw).map_err(|e| format!("not a URL: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!("scheme must be http or https, got {:?}", url.scheme()));
    }
    if url.host_str().map_or(true, str::is_empty) {
        return Err("missing host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("userinfo is not allowed in an origin".into());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err("an origin has no path, query, or fragment".into());
    }
    Ok(url.origin().ascii_serialization())
}

/// Parse the comma-separated [`EXTRA_ALLOWED_ORIGINS_ENV`] value into
/// normalized exact origins. Malformed entries are skipped with a warning —
/// they never fail startup and never widen the allowlist.
pub fn parse_extra_origins(raw: Option<&str>) -> Vec<HeaderValue> {
    let Some(raw) = raw else { return Vec::new() };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|entry| {
            match normalize_origin(entry).and_then(|o| {
                HeaderValue::from_str(&o).map_err(|e| format!("invalid header value: {e}"))
            }) {
                Ok(v) => Some(v),
                Err(reason) => {
                    tracing::warn!(
                        env = EXTRA_ALLOWED_ORIGINS_ENV,
                        origin = entry,
                        %reason,
                        "ignoring malformed allowed origin"
                    );
                    None
                }
            }
        })
        .collect()
}

/// The API's own loopback origins for its listen port. Pages the API serves
/// itself (the `/embed/computers/:id` viewer) open a websocket back to the
/// API, and browsers always attach `Origin` to websocket upgrades — so on a
/// non-default port (the dev default is 18013, not the listed 8013) the
/// viewer was rejected by its own server without these.
pub fn self_origins(api_port: u16) -> Vec<HeaderValue> {
    ["localhost", "127.0.0.1"]
        .iter()
        .filter_map(|host| HeaderValue::from_str(&format!("http://{host}:{api_port}")).ok())
        .collect()
}

/// Compose the effective allowlist: `base` (defaults or
/// `ALLTERNIT_CORS_ORIGINS`), then the API's own origins, then the extra
/// env-configured origins — deduplicated, order preserved.
pub fn effective_allowed_origins(
    base: Vec<HeaderValue>,
    api_port: u16,
    extra_raw: Option<&str>,
) -> Vec<HeaderValue> {
    let mut out: Vec<HeaderValue> = Vec::new();
    for origin in base
        .into_iter()
        .chain(self_origins(api_port))
        .chain(parse_extra_origins(extra_raw))
    {
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out
}

/// Parse a comma-separated origin list (as stored in `ALLTERNIT_CORS_ORIGINS`)
/// into header values. Empty entries and values that are not valid header
/// values are skipped; an empty result falls back to
/// [`DEFAULT_ALLOWED_ORIGINS`].
pub fn parse_allowed_origins(raw: Option<&str>) -> Vec<HeaderValue> {
    let parsed: Vec<HeaderValue> = raw
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter_map(|s| match HeaderValue::from_str(s) {
                    Ok(v) => Some(v),
                    Err(err) => {
                        tracing::warn!(origin = s, %err, "ignoring invalid CORS origin");
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    if parsed.is_empty() {
        return DEFAULT_ALLOWED_ORIGINS
            .iter()
            .map(|s| HeaderValue::from_static(s))
            .collect();
    }
    parsed
}

/// The CORS layer for the whole app, from the resolved config. See the module
/// docs for the policy.
///
/// `Access-Control-Allow-Credentials` stays enabled (the local UIs use
/// `credentials: 'include'`); a concrete origin list composes with credentials,
/// unlike a wildcard.
pub fn cors_layer_from_config(cfg: &AppConfig) -> CorsLayer {
    cors_layer(cfg.local_dev_bypass(), cfg.cors_origins())
}

/// Build the CORS layer from explicit policy inputs.
///
/// `bypass` mirrors any origin (local-dev mode); otherwise `origins` is the
/// allowlist used for `Access-Control-Allow-Origin`.
pub fn cors_layer(bypass: bool, origins: Vec<HeaderValue>) -> CorsLayer {
    let allow_origin = if bypass {
        AllowOrigin::mirror_request()
    } else {
        AllowOrigin::list(origins)
    };
    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(allowed_request_headers())
}

/// Request headers permitted on cross-origin calls. Keep in sync with the
/// custom `x-allternit-*` auth/bootstrap headers, the OfficeCLI taskpane
/// upload headers, and the LLM gateway surface.
fn allowed_request_headers() -> Vec<HeaderName> {
    [
        header::ACCEPT,
        header::AUTHORIZATION,
        header::CONTENT_TYPE,
        header::ORIGIN,
        HeaderName::from_static("x-client-version"),
        HeaderName::from_static("x-allternit-desktop-access-token"),
        HeaderName::from_static("x-allternit-self-hosted-token"),
        HeaderName::from_static("x-allternit-user-id"),
        HeaderName::from_static("x-allternit-user-email"),
        HeaderName::from_static("x-allternit-user-name"),
        HeaderName::from_static("x-allternit-tenant-id"),
        HeaderName::from_static("x-office-filename"),
        HeaderName::from_static("x-office-host"),
        HeaderName::from_static("x-office-binding-id"),
        HeaderName::from_static("idempotency-key"),
        HeaderName::from_static("x-allternit-session-id"),
    ]
    .to_vec()
}

/// Shared state for [`origin_gate`]: the configured allowlist.
#[derive(Clone)]
pub struct CorsGateState {
    allowed: Arc<Vec<HeaderValue>>,
}

impl CorsGateState {
    pub fn new(origins: Vec<HeaderValue>) -> Self {
        Self {
            allowed: Arc::new(origins),
        }
    }
}

/// Reject requests whose `Origin` header is present but not in the allowlist
/// with a 403. Applied only when the dev bypass is off; requests without an
/// `Origin` header (non-browser clients) always pass. Runs outside the
/// [`CorsLayer`] (installed after it) so disallowed preflights are rejected
/// before the layer would answer them itself; rejections set `Vary: Origin`
/// manually, matching what the layer adds to allowed responses.
pub async fn origin_gate(State(state): State<CorsGateState>, req: Request, next: Next) -> Response {
    if let Some(origin) = req.headers().get(&header::ORIGIN) {
        if !state.allowed.contains(origin) {
            let mut res = StatusCode::FORBIDDEN.into_response();
            res.headers_mut()
                .insert(header::VARY, HeaderValue::from_static("origin"));
            return res;
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tower::ServiceExt;

    fn app(bypass: bool, origins: Vec<HeaderValue>) -> Router {
        let mut router = Router::new()
            .route("/health", get(|| async { "ok" }))
            .layer(cors_layer(bypass, origins.clone()));
        if !bypass {
            // Mirror main.rs: the gate is installed outside the CORS layer.
            router = router.layer(axum::middleware::from_fn_with_state(
                CorsGateState::new(origins),
                origin_gate,
            ));
        }
        router
    }

    #[test]
    fn parse_defaults_when_unset() {
        let origins = parse_allowed_origins(None);
        assert_eq!(origins.len(), DEFAULT_ALLOWED_ORIGINS.len());
    }

    #[test]
    fn default_allowlist_includes_office_addin_origins() {
        let origins = parse_allowed_origins(None);
        let values: Vec<&str> = origins.iter().map(|v| v.to_str().unwrap()).collect();
        for required in [
            "https://allternit-office-addins.pages.dev",
            "https://office-addins.allternit.com",
        ] {
            assert!(values.contains(&required), "missing {required}");
        }
    }

    #[test]
    fn default_allowlist_includes_web_surface_dev_origins() {
        let origins = parse_allowed_origins(None);
        let values: Vec<&str> = origins.iter().map(|v| v.to_str().unwrap()).collect();
        for required in ["http://localhost:3013", "http://127.0.0.1:3013"] {
            assert!(values.contains(&required), "missing {required}");
        }
    }

    #[test]
    fn parse_trims_and_skips_empty_entries() {
        let origins = parse_allowed_origins(Some(" https://a.com , ,https://b.com "));
        let values: Vec<&str> = origins.iter().map(|v| v.to_str().unwrap()).collect();
        assert_eq!(values, vec!["https://a.com", "https://b.com"]);
    }

    #[test]
    fn parse_falls_back_when_only_whitespace() {
        let origins = parse_allowed_origins(Some(" , ,"));
        assert_eq!(origins.len(), DEFAULT_ALLOWED_ORIGINS.len());
    }

    #[tokio::test]
    async fn allowed_origin_simple_request_succeeds() {
        let res = app(false, parse_allowed_origins(Some("https://platform.allternit.com")))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(header::ORIGIN, "https://platform.allternit.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://platform.allternit.com"
        );
        assert!(res
            .headers()
            .get_all(header::VARY)
            .iter()
            .any(|v| v.to_str().unwrap().contains("origin")));
    }

    #[tokio::test]
    async fn disallowed_origin_simple_request_is_rejected() {
        let res = app(false, parse_allowed_origins(Some("https://platform.allternit.com")))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
        assert_eq!(res.headers().get(header::VARY).unwrap(), "origin");
    }

    #[tokio::test]
    async fn disallowed_origin_preflight_is_rejected() {
        let res = app(false, parse_allowed_origins(Some("https://platform.allternit.com")))
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/health")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }

    #[tokio::test]
    async fn allowed_origin_preflight_succeeds() {
        let res = app(false, parse_allowed_origins(Some("https://ai.allternit.com")))
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/health")
                    .header(header::ORIGIN, "https://ai.allternit.com")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://ai.allternit.com"
        );
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .is_some());
        assert!(res
            .headers()
            .get_all(header::VARY)
            .iter()
            .any(|v| v.to_str().unwrap().contains("origin")));
    }

    #[tokio::test]
    async fn request_without_origin_passes() {
        let res = app(false, parse_allowed_origins(Some("https://platform.allternit.com")))
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    fn strs(values: &[HeaderValue]) -> Vec<&str> {
        values.iter().map(|v| v.to_str().unwrap()).collect()
    }

    #[test]
    fn defaults_unchanged_without_extra_env() {
        let effective = effective_allowed_origins(parse_allowed_origins(None), 8013, None);
        // 8013 is already a default, so nothing is appended.
        assert_eq!(strs(&effective), DEFAULT_ALLOWED_ORIGINS.to_vec());
        assert_eq!(DEFAULT_ALLOWED_ORIGINS.len(), 16);
    }

    #[test]
    fn extra_env_adds_exact_origins() {
        let effective = effective_allowed_origins(
            parse_allowed_origins(None),
            8013,
            Some("http://127.0.0.1:18013, http://127.0.0.1:18014/"),
        );
        let values = strs(&effective);
        assert!(values.contains(&"http://127.0.0.1:18013"));
        assert!(values.contains(&"http://127.0.0.1:18014"));
        // Additive: every default is still present.
        for d in DEFAULT_ALLOWED_ORIGINS {
            assert!(values.contains(d), "missing default {d}");
        }
        assert_eq!(values.len(), DEFAULT_ALLOWED_ORIGINS.len() + 2);
        // Exact, not "any loopback port".
        assert!(!values.contains(&"http://127.0.0.1:18015"));
    }

    #[test]
    fn extra_env_normalizes_origins() {
        let values = parse_extra_origins(Some("HTTP://LocalHost:18013,https://Example.com:443"));
        assert_eq!(strs(&values), vec!["http://localhost:18013", "https://example.com"]);
    }

    #[test]
    fn malformed_extra_entries_are_ignored() {
        let values = parse_extra_origins(Some(concat!(
            "*, http://*:18013, http://127.0.0.1:*, 127.0.0.1:18013, ftp://127.0.0.1:21, ",
            "http://127.0.0.1:18013/embed, http://127.0.0.1:18013?x=1, http://u:p@127.0.0.1:1, ",
            "not a url, http://127.0.0.1:18014",
        )));
        assert_eq!(strs(&values), vec!["http://127.0.0.1:18014"]);
        assert!(normalize_origin("http://127.0.0.1:18013/embed").is_err());
        assert!(normalize_origin("*").is_err());
        assert!(parse_extra_origins(None).is_empty());
        assert!(parse_extra_origins(Some(" , ")).is_empty());
    }

    #[test]
    fn api_own_origin_is_allowed_on_non_default_port() {
        let effective = effective_allowed_origins(parse_allowed_origins(None), 18013, None);
        let values = strs(&effective);
        assert!(values.contains(&"http://127.0.0.1:18013"));
        assert!(values.contains(&"http://localhost:18013"));
        // Also when ALLTERNIT_CORS_ORIGINS replaces the defaults.
        let replaced = effective_allowed_origins(
            parse_allowed_origins(Some("https://platform.allternit.com")),
            18013,
            None,
        );
        assert_eq!(
            strs(&replaced),
            vec![
                "https://platform.allternit.com",
                "http://localhost:18013",
                "http://127.0.0.1:18013",
            ]
        );
    }

    fn origin_request(origin: &str) -> Request {
        Request::builder()
            .uri("/health")
            .header(header::ORIGIN, origin)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn embed_viewer_ws_origin_passes_gate_on_non_default_port() {
        // The embed page served by an API on :18013 opens its VNC websocket
        // with `Origin: http://127.0.0.1:18013`; its own server must admit it.
        let origins = effective_allowed_origins(parse_allowed_origins(None), 18013, None);
        let res = app(false, origins.clone())
            .oneshot(origin_request("http://127.0.0.1:18013"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // Before the fix (defaults only) the same request was rejected.
        let res = app(false, parse_allowed_origins(None))
            .oneshot(origin_request("http://127.0.0.1:18013"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        // A different loopback port stays rejected.
        let res = app(false, origins)
            .oneshot(origin_request("http://127.0.0.1:18099"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn extra_env_origin_passes_gate() {
        let origins = effective_allowed_origins(
            parse_allowed_origins(None),
            8013,
            Some("http://127.0.0.1:18014"),
        );
        let res = app(false, origins)
            .oneshot(origin_request("http://127.0.0.1:18014"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dev_bypass_mode_mirrors_any_origin() {
        let res = app(true, vec![])
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header(header::ORIGIN, "https://anything.example.com")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://anything.example.com"
        );
    }
}
