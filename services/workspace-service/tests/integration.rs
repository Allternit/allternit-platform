use allternit_workspace_service::{build_router, AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

fn app() -> axum::Router {
    build_router(AppState::new())
}

/// Serializes tests that mutate ALLTERNIT_FACTORY_PANE_SOCKET (process-global env).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn health_returns_ok() {
    let response = app()
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn create_and_get_session() {
    let app = app();

    let create_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "name": "test-session",
                        "workspace_id": "ws-test",
                        "metadata": { "owner": "agent-test" }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(create_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = created["session"]["id"].as_str().unwrap();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{}", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let fetched: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(fetched["session"]["name"], "test-session");
    assert_eq!(fetched["session"]["workspace_id"], "ws-test");
}

#[tokio::test]
async fn create_pane_and_capture_output() {
    let _guard = ENV_LOCK.lock().unwrap();
    // Force the metadata-only fallback: no pane engine at this path.
    std::env::set_var("ALLTERNIT_FACTORY_PANE_SOCKET", "/nonexistent/pane.sock");
    let app = app();

    let session_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "pane-test" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = axum::body::to_bytes(session_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let session_id = created["session"]["id"].as_str().unwrap();

    let pane_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{}/panes", session_id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "main" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(pane_response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(pane_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let pane: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let pane_id = pane["id"].as_str().unwrap();

    let send_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/panes/{}/send", pane_id))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "keys": "echo hello" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(send_response.status(), StatusCode::OK);

    let capture_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/panes/{}/capture", pane_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(capture_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(capture_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let captured: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(captured["output"], "$ echo hello");
}

#[tokio::test]
async fn register_and_list_skills() {
    let app = app();

    let register_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/skills")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "workspace_id": "ws-skills",
                        "name": "deploy",
                        "description": "Deploy the workspace",
                        "installed_by": "agent-test"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(register_response.status(), StatusCode::CREATED);

    let list_response = app
        .oneshot(
            Request::builder()
                .uri("/skills?workspace_id=ws-skills")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(list_response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(list_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let skills = listed["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "deploy");
}
