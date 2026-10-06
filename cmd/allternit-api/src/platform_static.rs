use std::path::PathBuf;
use axum::http::{header, HeaderValue};
use tower::Layer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::{SetResponseHeader, SetResponseHeaderLayer};
use tracing::info;

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
pub fn platform_service() -> Option<SetResponseHeader<ServeDir<ServeFile>, HeaderValue>> {
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

    let serve = ServeDir::new(&static_path)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(&index_path));
    Some(
        SetResponseHeaderLayer::overriding(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .layer(serve),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

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
        std::env::remove_var("ALLTERNIT_PLATFORM_STATIC");
    }
}
