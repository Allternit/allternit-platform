//! The user's default computer: `PUT /api/v1/runtime-devices/:id/default`
//! pins it, and `GET /api/v1/runtime-devices` marks it `preferred`.

mod common;

use axum::{body::Body, http::Request, http::StatusCode};
use common::TestApp;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn device(app: &TestApp, id: &str, owner: &str) -> String {
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active') ON CONFLICT DO NOTHING")
        .bind(owner)
        .bind(format!("{owner}@example.test"))
        .execute(&app.db)
        .await
        .unwrap();
    let token = format!("allternit_runtime_{id}_token");
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, capabilities, public_key_fingerprint,
                                      credential_hash, credential_expires_at, status, last_seen_at)
         VALUES ($1, $2, $1, 'desktop', '[]', 'fp', $3, now() + interval '1 day', 'online', now())",
    )
    .bind(id)
    .bind(owner)
    .bind(hex::encode(Sha256::digest(token.as_bytes())))
    .execute(&app.db)
    .await
    .unwrap();
    token
}

async fn call(app: &TestApp, method: &str, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn preferred(list: &Value) -> Vec<String> {
    list["runtimes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["preferred"] == true)
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn picking_a_default_moves_the_one_pin_and_stays_in_the_account() {
    let app = TestApp::new().await;
    let mac = device(&app, "rt_mac", "u_owner").await;
    device(&app, "rt_cloud", "u_owner").await;
    let other = device(&app, "rt_other", "u_other").await;

    let (status, list) = call(&app, "GET", "/api/v1/runtime-devices", &mac).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(preferred(&list).is_empty(), "{list}");

    let (status, body) = call(&app, "PUT", "/api/v1/runtime-devices/rt_cloud/default", &mac).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, list) = call(&app, "GET", "/api/v1/runtime-devices", &mac).await;
    assert_eq!(preferred(&list), ["rt_cloud"]);

    // A second pick replaces the pin; there is only ever one default.
    let (status, _) = call(&app, "PUT", "/api/v1/runtime-devices/rt_mac/default", &mac).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&app, "GET", "/api/v1/runtime-devices", &mac).await;
    assert_eq!(preferred(&list), ["rt_mac"]);

    // Someone else's runtime, or a revoked one, can't be made the default.
    let (status, _) = call(&app, "PUT", "/api/v1/runtime-devices/rt_mac/default", &other).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE runtime_devices SET revoked_at = now() WHERE id = 'rt_cloud'")
        .execute(&app.db)
        .await
        .unwrap();
    let (status, _) = call(&app, "PUT", "/api/v1/runtime-devices/rt_cloud/default", &mac).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
