//! A fresh workspace has no template folder. The API must still list and
//! serve the built-in templates, or a new install shows an empty Workflows
//! view and can't start a run.

use std::sync::Arc;

use allternit_factory_engine::api::service::{create_router, ServiceState};
use serde_json::Value;
use tempfile::TempDir;

#[tokio::test]
async fn fresh_workspace_lists_and_serves_builtin_templates() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(ServiceState::new(dir.path().to_path_buf()).await.unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, create_router(state)).await.unwrap() });

    let list: Value = reqwest::get(format!("{base}/api/factory/templates")).await.unwrap().json().await.unwrap();
    let ids: Vec<&str> = list["templates"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
    assert!(ids.contains(&"build-check-prove"), "built-ins missing: {list}");
    assert!(ids.contains(&"fact-check"), "built-ins missing: {list}");

    let one = reqwest::get(format!("{base}/api/factory/templates/fact-check")).await.unwrap();
    assert_eq!(one.status(), 200);

    let missing = reqwest::get(format!("{base}/api/factory/templates/no-such-template")).await.unwrap();
    assert_eq!(missing.status(), 404);
}
