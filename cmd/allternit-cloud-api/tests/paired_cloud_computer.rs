//! A cloud computer pairs itself as a Factory peer computer
//! (`POST /api/v1/computers/paired/self`, Factory phase 4).

mod common;

use allternit_cloud_api::routes::mesh::{HeadscaleAdmin, MeshError, MeshService};
use axum::{body::Body, http::Request, http::StatusCode};
use chrono::{DateTime, Utc};
use common::TestApp;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower::ServiceExt;

/// Headscale stand-in: one user per name, a fresh key per call.
#[derive(Default)]
struct FakeHeadscale {
    keys: std::sync::Mutex<u32>,
}

#[async_trait::async_trait]
impl HeadscaleAdmin for FakeHeadscale {
    async fn find_user_id(&self, _name: &str) -> Result<Option<u64>, MeshError> {
        Ok(Some(7))
    }
    async fn create_user(&self, _name: &str) -> Result<u64, MeshError> {
        Ok(7)
    }
    async fn create_preauth_key(&self, _user: u64, expiration: DateTime<Utc>) -> Result<(String, DateTime<Utc>), MeshError> {
        let mut n = self.keys.lock().unwrap();
        *n += 1;
        Ok((format!("key-{n}"), expiration))
    }
}

async fn app() -> TestApp {
    let mesh = MeshService::with_admin(Arc::new(FakeHeadscale::default()), "https://mesh.example.test");
    TestApp::with_mesh(Some(Arc::new(mesh))).await
}

async fn user(app: &TestApp, user: &str) {
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active') ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(&app.db)
        .await
        .unwrap();
}

async fn device(app: &TestApp, id: &str, owner: &str) -> String {
    user(app, owner).await;
    let token = format!("allternit_runtime_{id}_token");
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, credential_hash, credential_expires_at, status)
         VALUES ($1, $2, $1, $3, now() + interval '1 day', 'online')",
    )
    .bind(id)
    .bind(owner)
    .bind(hex::encode(Sha256::digest(token.as_bytes())))
    .execute(&app.db)
    .await
    .unwrap();
    token
}

async fn instance(app: &TestApp, id: &str, owner: &str, device: &str, status: &str) {
    sqlx::query(
        "INSERT INTO provisioned_instances (id, user_id, incus_name, status, device_id) VALUES ($1, $2, $1, $3, $4)",
    )
    .bind(id)
    .bind(owner)
    .bind(status)
    .bind(device)
    .execute(&app.db)
    .await
    .unwrap();
}

async fn call(app: &TestApp, method: &str, path: &str, headers: &[(&str, &str)], body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(path).method(method).header("Content-Type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app.router.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn pair_self(app: &TestApp, token: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {token}");
    call(app, "POST", "/api/v1/computers/paired/self", &[("Authorization", &bearer)], Value::Null).await
}

#[tokio::test]
async fn a_cloud_computer_pairs_itself_once_and_reports() {
    let app = app().await;
    let token = device(&app, "rd_cloud", "u_owner").await;
    instance(&app, "pi_1", "u_owner", "rd_cloud", "running").await;

    let (status, first) = pair_self(&app, &token).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let id = first["computerId"].as_str().unwrap().to_string();
    assert!(id.starts_with("pc_"));
    assert_eq!(first["name"], "Cloud computer");
    assert_eq!(first["controlUrl"], "https://mesh.example.test");
    assert_eq!(first["authKey"], "key-1");
    assert!(first["cloudUrl"].as_str().unwrap().starts_with("https://"));
    let owner: (String, Option<String>) =
        sqlx::query_as("SELECT user_id, provisioned_instance_id FROM paired_computers WHERE id = $1")
            .bind(&id)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(owner, ("u_owner".to_string(), Some("pi_1".to_string())));

    // Again (lost credentials): same computer, new secret, new mesh key.
    let (status, second) = pair_self(&app, &token).await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["computerId"], id.as_str());
    assert_ne!(second["secret"], first["secret"]);
    assert_eq!(second["authKey"], "key-2");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM paired_computers").fetch_one(&app.db).await.unwrap();
    assert_eq!(count, 1);

    // Reports work with the new secret only.
    let report = format!("/api/v1/computers/paired/{id}/report");
    let body = serde_json::json!({ "meshIp": "100.64.0.20" });
    let old = first["secret"].as_str().unwrap();
    let (status, _) = call(&app, "POST", &report, &[("x-allternit-computer-secret", old)], body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let new = second["secret"].as_str().unwrap();
    let (status, reply) = call(&app, "POST", &report, &[("x-allternit-computer-secret", new)], body).await;
    assert_eq!(status, StatusCode::OK, "{reply}");

    // The owner sees it among paired computers, marked as a cloud computer.
    let bearer = format!("Bearer {token}");
    let (_, list) = call(&app, "GET", "/api/v1/computers/paired", &[("Authorization", &bearer)], Value::Null).await;
    assert_eq!(list["computers"][0]["id"], id.as_str(), "{list}");
    assert_eq!(list["computers"][0]["cloud_computer"], true);
    assert_eq!(list["computers"][0]["mesh_ip"], "100.64.0.20");
}

#[tokio::test]
async fn only_live_cloud_computers_pair_themselves() {
    let app = app().await;
    // A Mac's Desktop (a runtime device with no instance): refused.
    let mac = device(&app, "rd_mac", "u_owner").await;
    let (status, body) = pair_self(&app, &mac).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A deleted cloud computer: refused.
    let gone = device(&app, "rd_gone", "u_owner").await;
    instance(&app, "pi_gone", "u_owner", "rd_gone", "deleted").await;
    let (status, body) = pair_self(&app, &gone).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // No device token at all: refused.
    let (status, _) = call(&app, "POST", "/api/v1/computers/paired/self", &[], Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM paired_computers").fetch_one(&app.db).await.unwrap();
    assert_eq!(count, 0);
}
