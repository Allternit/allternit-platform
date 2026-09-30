//! `/api/web-proxy` — fetches a public web page server-side so the browser
//! capsule can iframe it.
//!
//! This route is an SSRF surface (the server fetches attacker-chosen URLs), so
//! it is hardened in three layers:
//!
//! 1. **Access** — [`web_proxy_access`] only serves callers whose TCP peer is
//!    loopback (the Desktop UI on this machine, no forwarding headers) or who
//!    pass the normal [`crate::auth::auth_middleware`]. Anything else is 401,
//!    so a LAN host cannot use a Desktop user's machine as an open proxy.
//! 2. **Destination** — every address the proxy connects to must satisfy
//!    [`is_public_ip`]. Domain names go through [`PublicOnlyResolver`], which
//!    rejects the lookup if *any* resolved address is non-public; because the
//!    socket connects to exactly the addresses that were checked, DNS
//!    rebinding cannot swap in a private address between check and connect.
//!    Literal-IP hosts never reach a resolver, so they are checked directly.
//! 3. **Redirects** — [`redirect_policy`] re-validates scheme and literal-IP
//!    host on every hop (≤ [`MAX_REDIRECTS`]); domain hops go through the
//!    resolver again. System HTTP proxies are disabled so resolution can never
//!    be delegated to a proxy that would skip these checks.
//!
//! Behavioral reference: `cmd/gizzi-code/src/runtime/server/routes/web-proxy.ts`.

use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use reqwest::dns::{Name, Resolve, Resolving};
use reqwest::redirect::{Attempt, Policy};
use serde::Deserialize;
use std::{
    error::Error as StdError,
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use crate::AppState;
use std::sync::Arc;

/// Maximum redirect hops followed per request (matches the gizzi-code proxy).
pub const MAX_REDIRECTS: usize = 5;

/// Router for `/web-proxy` (nested under `/api`). Mount it with
/// [`web_proxy_access`] and the rate limiter applied — see `main.rs`.
pub fn web_proxy_router() -> Router<Arc<AppState>> {
    Router::new().route("/web-proxy", get(web_proxy))
}

// ── Access gate ──────────────────────────────────────────────────────────────

/// Serve direct loopback callers (the Desktop UI) without further checks;
/// everyone else must pass the normal auth middleware (401 otherwise).
///
/// A non-loopback caller that claims a localhost `Host`/`Origin`/`Referer` is
/// rejected outright: `auth_middleware`'s self-hosted fallback trusts those
/// headers, and a LAN peer spoofing them must not ride that fallback into an
/// SSRF-capable route.
pub async fn web_proxy_access(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if is_direct_loopback_peer(&request) {
        return next.run(request).await;
    }
    if crate::auth::is_localhost_origin(request.headers()) {
        return json_error(StatusCode::UNAUTHORIZED, "Authentication required");
    }
    crate::auth::auth_middleware(State(state), request, next).await
}

/// True only when the TCP peer is loopback AND the request was not relayed by
/// a reverse proxy on this host (which would make every public caller look
/// like loopback).
fn is_direct_loopback_peer(request: &Request) -> bool {
    const FORWARDING_HEADERS: [&str; 5] = [
        "forwarded",
        "x-forwarded-for",
        "x-real-ip",
        "cf-connecting-ip",
        "true-client-ip",
    ];
    let peer_is_loopback = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .is_some_and(|info| info.0.ip().to_canonical().is_loopback());
    peer_is_loopback
        && !FORWARDING_HEADERS
            .iter()
            .any(|name| request.headers().contains_key(*name))
}

// ── Destination policy ───────────────────────────────────────────────────────

pub use allternit_commrails::egress::{
    is_blocked_destination, is_public_ip, BlockedDestination, PublicOnlyResolver,
};

/// Scheme + literal-host validation shared by the initial URL and every
/// redirect hop. Domain hosts are left to the resolver (which sees every hop).
fn check_url(url: &reqwest::Url) -> Result<(), UrlRejection> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlRejection::Scheme);
    }
    match url.host() {
        None => Err(UrlRejection::Destination),
        Some(url::Host::Ipv4(ip)) if !is_public_ipv4(ip) => Err(UrlRejection::Destination),
        Some(url::Host::Ipv6(ip)) if !is_public_ipv6(ip) => Err(UrlRejection::Destination),
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            if domain.is_empty()
                || domain == "localhost"
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
            {
                Err(UrlRejection::Destination)
            } else {
                Ok(())
            }
        }
        Some(_) => Ok(()),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UrlRejection {
    Scheme,
    Destination,
}

/// Redirect policy: ≤ [`MAX_REDIRECTS`] hops, each re-validated by
/// [`check_url`] (domain hops are additionally re-resolved through
/// [`PublicOnlyResolver`] by the connector).
pub fn redirect_policy() -> Policy {
    Policy::custom(|attempt: Attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        match check_url(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(UrlRejection::Scheme) => attempt.error("redirect to non-http(s) scheme"),
            Err(UrlRejection::Destination) => {
                let target = attempt.url().to_string();
                attempt.error(BlockedDestination(target))
            }
        }
    })
}

/// Build the upstream client. Generic over the resolver so tests can pin a
/// fixture hostname to a local server while delegating everything else to
/// [`PublicOnlyResolver`].
pub fn build_proxy_client<R: Resolve + 'static>(
    resolver: Arc<R>,
) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .dns_resolver(resolver)
        .redirect(redirect_policy())
        // A system/env HTTP proxy would resolve names itself, bypassing
        // PublicOnlyResolver.
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
}

#[derive(Debug)]
enum FetchError {
    Scheme,
    Blocked,
    Upstream,
}

/// Validate `url` and GET it with `client` (built by [`build_proxy_client`]).
async fn fetch_upstream(
    client: &reqwest::Client,
    url: reqwest::Url,
) -> Result<reqwest::Response, FetchError> {
    match check_url(&url) {
        Ok(()) => {}
        Err(UrlRejection::Scheme) => return Err(FetchError::Scheme),
        Err(UrlRejection::Destination) => return Err(FetchError::Blocked),
    }
    client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
        )
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|err| {
            if is_blocked_destination(&err) {
                FetchError::Blocked
            } else {
                FetchError::Upstream
            }
        })
}

// ── Handler ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WebProxyQuery {
    url: Option<String>,
}

async fn web_proxy(Query(query): Query<WebProxyQuery>) -> Response {
    let Some(target_url) = query.url else {
        return json_error(StatusCode::BAD_REQUEST, "Missing ?url= query parameter");
    };

    let parsed = match reqwest::Url::parse(&target_url) {
        Ok(url) => url,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "Invalid URL"),
    };

    let client = match build_proxy_client(Arc::new(PublicOnlyResolver)) {
        Ok(client) => client,
        Err(_) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build proxy client",
            )
        }
    };

    let upstream = match fetch_upstream(&client, parsed).await {
        Ok(response) => response,
        Err(FetchError::Scheme) => {
            return json_error(StatusCode::FORBIDDEN, "Only http/https URLs are allowed")
        }
        Err(FetchError::Blocked) => {
            return json_error(
                StatusCode::FORBIDDEN,
                "Requests to private/loopback addresses are blocked",
            )
        }
        Err(FetchError::Upstream) => {
            return json_error(StatusCode::BAD_GATEWAY, "Failed to fetch upstream URL")
        }
    };

    let status = upstream.status();
    let final_url = upstream.url().clone();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_html = content_type.contains("text/html") || content_type.is_empty();

    if !is_html {
        let mut headers = HeaderMap::new();
        if let Some(value) = upstream.headers().get(reqwest::header::CONTENT_TYPE) {
            if let Ok(value_str) = value.to_str() {
                if let Ok(header_value) = HeaderValue::from_str(value_str) {
                    headers.insert(axum::http::header::CONTENT_TYPE, header_value);
                }
            }
        }
        headers.insert(
            axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );

        let body = match upstream.bytes().await {
            Ok(bytes) => Body::from(bytes),
            Err(_) => return json_error(StatusCode::BAD_GATEWAY, "Failed to read response body"),
        };

        return (
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            headers,
            body,
        )
            .into_response();
    }

    let mut body_text = match upstream.text().await {
        Ok(body) => body,
        Err(_) => return json_error(StatusCode::BAD_GATEWAY, "Failed to read response body"),
    };

    let base_origin = format!(
        "{}://{}",
        final_url.scheme(),
        final_url.host_str().unwrap_or_default()
    );

    if final_url
        .host_str()
        .map(|host| host.ends_with("login.microsoftonline.com"))
        .unwrap_or(false)
    {
        body_text = body_text.replace("sso_reload=True", "");
        body_text = body_text.replace("\"reloadOnFailure\":true", "\"reloadOnFailure\":false");
        body_text = body_text.replace(
            "\"enabled\":true,\"type\":\"chrome\",\"reason\":\"Pull is needed\"",
            "\"enabled\":false,\"type\":\"chrome\",\"reason\":\"Disabled by embedded proxy\"",
        );
    }

    for pattern in [
        r#"(<iframe\b[^>]+\ssrc=)(["'])([^"']*)(["'])"#,
        r#"(<frame\b[^>]+\ssrc=)(["'])([^"']*)(["'])"#,
        r#"(<form\b[^>]+\saction=)(["'])([^"']*)(["'])"#,
        r#"(<a\b[^>]+\shref=)(["'])([^"']*)(["'])"#,
    ] {
        let regex = regex::Regex::new(pattern).expect("valid proxy rewrite regex");
        body_text = regex
            .replace_all(&body_text, |caps: &regex::Captures| {
                let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
                let opening_quote = caps.get(2).map(|m| m.as_str()).unwrap_or("\"");
                let raw = caps.get(3).map(|m| m.as_str()).unwrap_or_default();
                let closing_quote = caps.get(4).map(|m| m.as_str()).unwrap_or(opening_quote);
                format!(
                    "{prefix}{opening_quote}{}{closing_quote}",
                    proxify_url(raw, &final_url)
                )
            })
            .into_owned();
    }

    let injected_head = format!(
        r#"<base href="{base_origin}/"><script>
(function(){{
  var _proxyPrefix = '/api/web-proxy?url=';
  function toProxy(url) {{
    if (!url || url.charAt(0) === '#') return url;
    try {{
      var abs = new URL(url, '{final_url}').toString();
      if (abs.indexOf('/api/web-proxy?url=') !== -1) return abs;
      return _proxyPrefix + encodeURIComponent(abs);
    }} catch(e) {{ return url; }}
  }}
  document.addEventListener('click', function(event) {{
    var anchor = event.target && event.target.closest ? event.target.closest('a[href]') : null;
    if (!anchor) return;
    var href = anchor.getAttribute('href');
    if (!href || href.startsWith('javascript:') || href.startsWith('mailto:') || href.startsWith('tel:') || href.startsWith('#')) return;
    event.preventDefault();
    window.parent.postMessage({{ type: 'allternit-navigate', url: toProxy(href) }}, '*');
  }}, true);
  var _push = history.pushState.bind(history);
  var _replace = history.replaceState.bind(history);
  history.pushState = function(state, title, url) {{
    if (url) {{
      window.parent.postMessage({{ type: 'allternit-navigate', url: toProxy(String(url)) }}, '*');
      return;
    }}
    return _push(state, title, url);
  }};
  history.replaceState = function(state, title, url) {{
    if (url) {{
      window.parent.postMessage({{ type: 'allternit-navigate', url: toProxy(String(url)) }}, '*');
      return;
    }}
    return _replace(state, title, url);
  }};
}})();
</script>"#,
    );

    if let Some(idx) = body_text.find("<head>") {
        body_text.insert_str(idx + "<head>".len(), &injected_head);
    } else if let Some(idx) = body_text.find("<html>") {
        body_text.insert_str(
            idx + "<html>".len(),
            &format!("<head>{}</head>", injected_head),
        );
    } else {
        body_text = format!("<head>{}</head>{}", injected_head, body_text);
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );

    (
        StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
        headers,
        Body::from(body_text),
    )
        .into_response()
}

fn proxify_url(raw_url: &str, final_url: &reqwest::Url) -> String {
    if raw_url.is_empty()
        || raw_url.starts_with("data:")
        || raw_url.starts_with("blob:")
        || raw_url.starts_with("javascript:")
        || raw_url.starts_with('#')
        || raw_url.starts_with("mailto:")
        || raw_url.starts_with("tel:")
    {
        return raw_url.to_string();
    }
    if raw_url.starts_with("/api/web-proxy?url=") {
        return raw_url.to_string();
    }
    match final_url.join(raw_url) {
        Ok(abs) => format!("/api/web-proxy?url={}", urlencoding::encode(abs.as_str())),
        Err(_) => raw_url.to_string(),
    }
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        axum::Json(serde_json::json!({
            "error": message,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request as HttpRequest, response::Redirect};
    use tower::ServiceExt;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn check_url_rejects_literal_private_hosts_and_bad_schemes() {
        for url in [
            "http://127.0.0.1/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://169.254.169.254/latest/meta-data/",
            "http://100.64.0.1/",
            "http://localhost:8013/",
            "http://localhost./",
            "http://api.localhost/",
            "http://printer.local/",
        ] {
            let parsed = reqwest::Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed), Err(UrlRejection::Destination), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/",
            "gopher://example.com/",
        ] {
            let parsed = reqwest::Url::parse(url).unwrap();
            assert_eq!(check_url(&parsed), Err(UrlRejection::Scheme), "{url}");
        }
        assert_eq!(
            check_url(&reqwest::Url::parse("https://example.com/").unwrap()),
            Ok(())
        );
    }

    // ── Live client tests against a local fixture server ──────────────────

    /// `fixture.test` is pinned to the local fixture server (standing in for a
    /// public site); `rebind.test` behaves like a public DNS name that resolves
    /// to loopback (e.g. `127.0.0.1.nip.io`) and goes through the real
    /// PublicOnlyResolver check. Everything else is PublicOnlyResolver.
    struct FixtureResolver {
        addr: SocketAddr,
    }

    impl Resolve for FixtureResolver {
        fn resolve(&self, name: Name) -> Resolving {
            let host = name.as_str().to_string();
            let addr = self.addr;
            Box::pin(async move {
                let addrs = match host.as_str() {
                    "fixture.test" => vec![addr],
                    "rebind.test" => PublicOnlyResolver::resolve_public("localhost").await?,
                    other => PublicOnlyResolver::resolve_public(other).await?,
                };
                Ok(Box::new(addrs.into_iter()) as Addrs)
            })
        }
    }

    async fn spawn_fixture() -> (SocketAddr, reqwest::Client) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        let app =
            Router::new()
                .route("/ok", get(|| async { "ok" }))
                .route(
                    "/to-fixture",
                    get(move || async move {
                        Redirect::temporary(&format!("http://fixture.test:{port}/ok"))
                    }),
                )
                .route(
                    "/to-ip",
                    get(move || async move {
                        Redirect::temporary(&format!("http://127.0.0.1:{port}/ok"))
                    }),
                )
                .route(
                    "/to-mapped",
                    get(move || async move {
                        Redirect::temporary(&format!("http://[::ffff:127.0.0.1]:{port}/ok"))
                    }),
                )
                .route(
                    "/to-localhost",
                    get(move || async move {
                        Redirect::temporary(&format!("http://localhost:{port}/ok"))
                    }),
                )
                .route(
                    "/to-rebind",
                    get(move || async move {
                        Redirect::temporary(&format!("http://rebind.test:{port}/ok"))
                    }),
                )
                .route(
                    "/to-metadata",
                    get(|| async {
                        Redirect::temporary("http://169.254.169.254/latest/meta-data/")
                    }),
                )
                .route(
                    "/to-file",
                    get(|| async { Redirect::temporary("file:///etc/passwd") }),
                )
                .route("/loop", get(|| async { Redirect::temporary("/loop") }))
                .route(
                    "/hop/:n",
                    get(
                        |axum::extract::Path(n): axum::extract::Path<u32>| async move {
                            if n == 0 {
                                "ok".into_response()
                            } else {
                                Redirect::temporary(&format!("/hop/{}", n - 1)).into_response()
                            }
                        },
                    ),
                );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = build_proxy_client(Arc::new(FixtureResolver { addr })).unwrap();
        (addr, client)
    }

    fn fixture_url(addr: SocketAddr, path: &str) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://fixture.test:{}{path}", addr.port())).unwrap()
    }

    #[tokio::test]
    async fn fixture_is_reachable_and_public_redirects_are_followed() {
        let (addr, client) = spawn_fixture().await;
        let resp = fetch_upstream(&client, fixture_url(addr, "/ok"))
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");
        let resp = fetch_upstream(&client, fixture_url(addr, "/to-fixture"))
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn redirect_to_loopback_or_metadata_is_blocked() {
        let (addr, client) = spawn_fixture().await;
        for path in [
            "/to-ip",
            "/to-mapped",
            "/to-localhost",
            "/to-rebind",
            "/to-metadata",
        ] {
            let result = fetch_upstream(&client, fixture_url(addr, path)).await;
            assert!(
                matches!(result, Err(FetchError::Blocked)),
                "{path}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn hostname_resolving_to_loopback_is_blocked_at_connect() {
        let (addr, client) = spawn_fixture().await;
        let url = reqwest::Url::parse(&format!("http://rebind.test:{}/ok", addr.port())).unwrap();
        let result = fetch_upstream(&client, url).await;
        assert!(matches!(result, Err(FetchError::Blocked)), "{result:?}");
    }

    #[tokio::test]
    async fn redirect_to_other_scheme_and_redirect_loops_fail() {
        let (addr, client) = spawn_fixture().await;
        // reqwest never follows a redirect to a non-http(s) scheme: the 3xx
        // comes back unfollowed (the handler relays it without Location).
        let resp = fetch_upstream(&client, fixture_url(addr, "/to-file"))
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(resp.url().path(), "/to-file");
        let result = fetch_upstream(&client, fixture_url(addr, "/loop")).await;
        assert!(matches!(result, Err(FetchError::Upstream)), "{result:?}");
    }

    #[tokio::test]
    async fn follows_at_most_max_redirects() {
        let (addr, client) = spawn_fixture().await;
        let path = format!("/hop/{MAX_REDIRECTS}");
        let resp = fetch_upstream(&client, fixture_url(addr, &path))
            .await
            .unwrap();
        assert_eq!(resp.text().await.unwrap(), "ok");
        let path = format!("/hop/{}", MAX_REDIRECTS + 1);
        let result = fetch_upstream(&client, fixture_url(addr, &path)).await;
        assert!(matches!(result, Err(FetchError::Upstream)), "{result:?}");
    }

    #[tokio::test]
    async fn production_client_blocks_literal_loopback() {
        let client = build_proxy_client(Arc::new(PublicOnlyResolver)).unwrap();
        for url in [
            "http://127.0.0.1:9/",
            "http://[::1]:9/",
            "http://localhost:9/",
        ] {
            let result = fetch_upstream(&client, reqwest::Url::parse(url).unwrap()).await;
            assert!(
                matches!(result, Err(FetchError::Blocked)),
                "{url}: {result:?}"
            );
        }
    }

    // ── Access gate ───────────────────────────────────────────────────────

    async fn gated_app() -> (Router, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let app = Router::new()
            .route("/api/web-proxy", get(|| async { StatusCode::NO_CONTENT }))
            .layer(axum::middleware::from_fn_with_state(
                state,
                web_proxy_access,
            ));
        (app, temp)
    }

    fn from_peer(ip: IpAddr, headers: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder()
            .method("GET")
            .uri("/api/web-proxy?url=https%3A%2F%2Fexample.com")
            .extension(axum::extract::ConnectInfo(SocketAddr::new(ip, 50000)));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn loopback_peer_is_served_without_auth() {
        let (app, _temp) = gated_app().await;
        for ip in [v4("127.0.0.1"), v6("::1"), v6("::ffff:127.0.0.1")] {
            let resp = app.clone().oneshot(from_peer(ip, &[])).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT, "{ip}");
        }
    }

    #[tokio::test]
    async fn non_loopback_unauthenticated_is_401() {
        let (app, _temp) = gated_app().await;
        let lan = v4("192.168.1.20");
        let cases: [&[(&str, &str)]; 4] = [
            &[],
            &[("host", "localhost:8013")],
            &[("origin", "http://127.0.0.1:8013")],
            &[("referer", "http://localhost:8013/")],
        ];
        for headers in cases {
            let resp = app.clone().oneshot(from_peer(lan, headers)).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "LAN peer with {headers:?}"
            );
        }
        // No ConnectInfo at all (not served with connect info) is not loopback.
        let req = HttpRequest::builder()
            .uri("/api/web-proxy?url=https%3A%2F%2Fexample.com")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn reverse_proxied_loopback_is_not_trusted() {
        let (app, _temp) = gated_app().await;
        for header in [
            "x-forwarded-for",
            "forwarded",
            "x-real-ip",
            "cf-connecting-ip",
        ] {
            let resp = app
                .clone()
                .oneshot(from_peer(v4("127.0.0.1"), &[(header, "203.0.113.9")]))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{header}");
        }
    }
}
