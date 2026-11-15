use std::convert::Infallible;
use std::path::PathBuf;
use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response, StatusCode};
use tower::util::BoxCloneService;
use tower::{Layer, ServiceExt};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::info;

/// The platform UI service: static files, the SPA fallback, and 404 JSON for API misses.
pub type PlatformService = BoxCloneService<Request<Body>, Response<Body>, Infallible>;

/// API paths are never app routes. A miss there is a 404 JSON, not the SPA
/// shell: answering `200 text/html` made every client treat a route this
/// runtime lacks as a success. Over the phone relay that kept retrying
/// clients in a loop (Nov 5–9 2026, ~560k relay requests on one day).
fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/v1" || path.starts_with("/v1/")
}

fn api_not_found(path: &str) -> Response<Body> {
    let body = serde_json::json!({ "error": "not_found", "message": format!("No API route for {path}") });
    let mut res = Response::new(Body::from(body.to_string()));
    *res.status_mut() = StatusCode::NOT_FOUND;
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

/// Resolve the project/package root from the current executable path.
///
/// - Packaged desktop app: `.../Contents/MacOS/allternit-api` → `.../Contents`
/// - Repo dev build: `.../allternit/target/debug/allternit-api` → `.../allternit`
fn resolve_project_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut cur = exe.parent()?;

    // Packaged macOS desktop layout: Contents/MacOS/<binary>
    if cur.file_name()?.to_str()? == "MacOS" {
        return cur.parent().map(|p| p.to_path_buf());
    }

    // Repo build: walk up until we find the workspace Cargo.toml.
    loop {
        if cur.join("Cargo.toml").exists() {
            return Some(cur.to_path_buf());
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }

    None
}

pub fn resolve_static_path() -> PathBuf {
    if let Ok(p) = std::env::var("ALLTERNIT_PLATFORM_STATIC") {
        return PathBuf::from(p);
    }
    if let Some(root) = resolve_project_root() {
        // Packaged layout: <root>/platform (macOS: Contents/platform)
        let packaged = root.join("platform");
        if packaged.exists() {
            return packaged;
        }
        // Dev: sibling private checkout of Allternit/allternit-ai, or CI .hosted-ui
        for rel in ["../allternit-ai/dist", ".hosted-ui/dist"] {
            let p = root.join(rel);
            if p.exists() {
                return p;
            }
        }
    }
    PathBuf::from("./resources/platform")
}

/// Build a static-file service for the platform UI.
///
/// Returns `None` when no static export is available (e.g. development before the
/// Vite build has run). When available, the service is mounted at `/` so the UI
/// works offline with its original root-relative asset paths. Missing paths fall
/// back to `index.html` for Vite / React Router SPA behavior.
///
/// Every response carries `Cache-Control: no-cache`: without it Chromium
/// heuristically reuses a cached `index.html` after an app update and keeps
/// booting the previous build's hashed entry chunk (still in its disk cache)
/// until the heuristic expires. Revalidation on loopback is a cheap 304.
///
/// Misses under `/api` and `/v1` answer 404 JSON instead of `index.html`.
pub fn platform_service() -> Option<PlatformService> {
    let static_path = resolve_static_path();
    let index_path = static_path.join("index.html");

    if !index_path.exists() {
        // Only warn if user explicitly configured a path; default fallback is expected
        // to be missing in development before the Vite build step runs.
        let is_explicit = std::env::var("ALLTERNIT_PLATFORM_STATIC").is_ok();
        if is_explicit {
            tracing::warn!(
                "Platform static files not found at '{}'",
                static_path.display()
            );
        } else {
            tracing::info!(
                "Platform static files not found at '{}' (skipping — run Vite build to generate)",
                static_path.display()
            );
        }
        return None;
    }

    info!("Serving platform UI from: {}", static_path.display());

    let index = ServeFile::new(&index_path);
    let fallback = tower::service_fn(move |req: Request<Body>| {
        let index = index.clone();
        async move {
            let path = req.uri().path().to_owned();
            if is_api_path(&path) {
                return Ok::<_, Infallible>(api_not_found(&path));
            }
            let res = index.oneshot(req).await?;
            Ok(res.map(Body::new))
        }
    });
    let serve = ServeDir::new(&static_path)
        .append_index_html_on_directories(true)
        .fallback(fallback);
    let service = SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .layer(serve)
        .map_response(|res| res.map(Body::new));
    Some(BoxCloneService::new(service))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_responses_are_revalidated() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/main-abc.js"), "1").unwrap();
        std::env::set_var("ALLTERNIT_PLATFORM_STATIC", dir.path());
        let svc = platform_service().expect("service");

        for uri in ["/", "/assets/main-abc.js", "/some/spa/route"] {
            let res = svc
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), 200, "{uri}");
            assert_eq!(
                res.headers().get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
                Some("no-cache"),
                "{uri}"
            );
        }

        // API misses are 404 JSON, never the SPA shell.
        for uri in ["/api/factory/events", "/api/v1/not-a-route", "/v1/nope"] {
            let res = svc
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), 404, "{uri}");
            assert_eq!(
                res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
                Some("application/json"),
                "{uri}"
            );
        }
        std::env::remove_var("ALLTERNIT_PLATFORM_STATIC");
    }
}
