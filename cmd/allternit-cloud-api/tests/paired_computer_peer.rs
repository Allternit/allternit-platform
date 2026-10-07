//! Paired computers shared with the owner's organization, and the Factory
//! peer tickets their engines accept (design agreed 2026-10-07).

mod common;

use axum::{body::Body, http::Request, http::StatusCode};
use common::TestApp;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// A runtime device (paired runtime) for `user` in `org`, returning its token.
async fn device(app: &TestApp, id: &str, user: &str, org: Option<&str>) -> String {
    let token = format!("allternit_runtime_{id}_token");
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active') ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, organization_id, name, credential_hash, credential_expires_at, status)
         VALUES ($1, $2, $3, $1, $4, now() + interval '1 day', 'online')",
    )
    .bind(id)
    .bind(user)
    .bind(org)
    .bind(hex::encode(Sha256::digest(token.as_bytes())))
    .execute(&app.db)
    .await
    .unwrap();
    token
}

async fn computer(app: &TestApp, id: &str, owner: &str, org: Option<&str>, mesh_ip: Option<&str>) {
    sqlx::query(
        "INSERT INTO paired_computers (id, user_id, name, secret_hash, organization_id, mesh_ip)
         VALUES ($1, $2, $1, 'x', $3, $4)",
    )
    .bind(id)
    .bind(owner)
    .bind(org)
    .bind(mesh_ip)
    .execute(&app.db)
    .await
    .unwrap();
}

async fn call(app: &TestApp, method: &str, path: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn organization_members_see_and_reach_a_paired_computer() {
    std::env::set_var("ALLTERNIT_DP_JWT_SEED", "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
    let app = TestApp::new().await;
    let owner = device(&app, "rd_owner", "u_owner", Some("org_1")).await;
    let mate = device(&app, "rd_mate", "u_mate", Some("org_1")).await;
    let outsider = device(&app, "rd_out", "u_out", Some("org_2")).await;
    computer(&app, "pc_team", "u_owner", Some("org_1"), Some("100.64.0.9")).await;
    computer(&app, "pc_solo", "u_out", None, Some("100.64.0.10")).await;
    computer(&app, "pc_off", "u_owner", Some("org_1"), None).await;

    // The teammate lists the organization's computers, marked shared.
    let (status, list) = call(&app, "GET", "/api/v1/computers/paired", &mate).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let ids: Vec<&str> = list["computers"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert_eq!(ids, ["pc_team", "pc_off"], "{list}");
    assert_eq!(list["computers"][0]["shared"], true);
    let (_, own) = call(&app, "GET", "/api/v1/computers/paired", &owner).await;
    assert_eq!(own["computers"][0]["shared"], false, "{own}");

    // The teammate gets a ticket for the team computer, addressed to it.
    let (status, ticket) = call(&app, "POST", "/api/v1/computers/paired/pc_team/peer-ticket", &mate).await;
    assert_eq!(status, StatusCode::OK, "{ticket}");
    assert_eq!(ticket["meshIp"], "100.64.0.9");
    assert_eq!(ticket["peerPort"], 3019);
    let keys = allternit_cloud_api::auth::dataplane_jwt::key_pair_from_env().unwrap();
    let claims =
        allternit_cloud_api::auth::dataplane_jwt::verify(ticket["ticket"].as_str().unwrap(), keys.verifying_key()).unwrap();
    assert_eq!((claims.sub.as_str(), claims.aud.as_str(), claims.scope.as_str()), ("u_mate", "pc_team", "factory:peer"));

    // Outside the organization, or someone's private computer: not found.
    let (status, _) = call(&app, "POST", "/api/v1/computers/paired/pc_team/peer-ticket", &outsider).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "POST", "/api/v1/computers/paired/pc_solo/peer-ticket", &mate).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Not on the mesh yet: says so.
    let (status, off) = call(&app, "POST", "/api/v1/computers/paired/pc_off/peer-ticket", &owner).await;
    assert_eq!(status, StatusCode::CONFLICT, "{off}");
}
