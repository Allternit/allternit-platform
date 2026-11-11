//! Allternit Link boards through the cloud relay (hardware/link/README.md
//! section 4.2): relay tokens, the token-authenticated socket ticket, revoke,
//! two accounts holding the same board id, and account erase.

mod common;

use axum::{body::Body, http::Request, http::StatusCode};
use common::TestApp;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const BOARD: &str = "usba-0123456789ab";
const TOKEN_HEADER: &str = "x-allternit-link-board-token";

/// A user with a compute-capable API token and one paired runtime that may
/// accept relayed sockets (no runtime is connected in tests, so it is offline).
async fn account(app: &TestApp, user: &str) -> (String, String) {
    let api_token = format!("allternit_test_{user}");
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active') ON CONFLICT DO NOTHING")
        .bind(user)
        .bind(format!("{user}@example.test"))
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ($1, $2, 'test', $3, '[\"compute\"]')")
        .bind(format!("tok_{user}"))
        .bind(hex::encode(Sha256::digest(api_token.as_bytes())))
        .bind(user)
        .execute(&app.db)
        .await
        .unwrap();
    let runtime = format!("rd_{user}");
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, credential_hash, credential_expires_at, status, capabilities)
         VALUES ($1, $2, $1, $3, now() + interval '1 day', 'offline', '[\"runtime:connect\",\"runtime:execute\"]')",
    )
    .bind(&runtime)
    .bind(user)
    .bind(hex::encode(Sha256::digest(device_token(user).as_bytes())))
    .execute(&app.db)
    .await
    .unwrap();
    (api_token, runtime)
}

/// The runtime device token Desktop's main process holds for `user`'s runtime.
fn device_token(user: &str) -> String {
    format!("allternit_runtime_{user}_token")
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

async fn register(app: &TestApp, api_token: &str, runtime: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {api_token}");
    call(app, "POST", "/api/v1/link-boards", &[("Authorization", &bearer)], json!({ "boardId": BOARD, "name": " Keychain\u{7} bot ", "runtimeDeviceId": runtime })).await
}

async fn ticket(app: &TestApp, board: &str, token: &str, body: Value) -> (StatusCode, Value) {
    call(app, "POST", &format!("/api/v1/link-boards/{board}/socket-ticket"), &[(TOKEN_HEADER, token)], body).await
}

#[tokio::test]
async fn a_relay_token_buys_a_ticket_until_the_board_is_revoked() {
    let app = TestApp::new().await;
    let (api, runtime) = account(&app, "u_link_a").await;
    let bearer = format!("Bearer {api}");

    // Registration answers the token once; only its hash is stored.
    let (status, reg) = register(&app, &api, &runtime).await;
    assert_eq!(status, StatusCode::OK, "{reg}");
    assert_eq!((reg["boardId"].as_str(), reg["runtimeDeviceId"].as_str()), (Some(BOARD), Some(runtime.as_str())));
    let token = reg["relayToken"].as_str().unwrap().to_string();
    assert!(token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{token}");
    let (stored, name): (String, Option<String>) =
        sqlx::query_as("SELECT token_hash, name FROM link_boards WHERE account = 'u_link_a' AND board_id = $1")
            .bind(BOARD)
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(stored, hex::encode(Sha256::digest(hex::decode(&token).unwrap())));
    assert_ne!(stored, token);
    assert_eq!(name.as_deref(), Some("Keychain bot"), "trimmed, control characters removed");
    let (_, list) = call(&app, "GET", "/api/v1/link-boards", &[("Authorization", &bearer)], Value::Null).await;
    assert_eq!(list["boards"][0]["serial"], format!("usb:{BOARD}"));
    assert!(list["boards"][0].get("relayToken").is_none() && !list.to_string().contains(&token), "the token is shown once");

    // A bad board id is refused at registration.
    let (status, _) = call(&app, "POST", "/api/v1/link-boards", &[("Authorization", &bearer)], json!({ "boardId": "usba-0123456789AB", "runtimeDeviceId": runtime })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A wrong, short or missing token is 401 with one message.
    let wrong = "ab".repeat(32);
    for (board, presented) in [(BOARD, wrong.as_str()), (BOARD, &token[..62]), (BOARD, ""), ("usba-0123456789ac", token.as_str())] {
        let (status, body) = ticket(&app, board, presented, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{board} {body}");
    }

    // The right token gets past auth: the answer is the runtime being offline.
    let (status, body) = ticket(&app, BOARD, &token, json!({})).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.to_string().contains("offline"), "{body}");
    // Any other path is 403, after auth.
    let (status, _) = ticket(&app, BOARD, &token, json!({ "path": "/api/v1/terminal" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = ticket(&app, BOARD, &token, json!({ "path": format!("/api/v1/phones/usba?board={BOARD}") })).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    // Revoked: the same token is 401. A second revoke (Desktop retries at its
    // next start) is safe and still 200.
    let (status, body) = call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[("Authorization", &bearer)], Value::Null).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "revoked": true })));
    let (status, _) = ticket(&app, BOARD, &token, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[("Authorization", &bearer)], Value::Null).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "revoked": true })));
    let (status, _) = call(&app, "DELETE", "/api/v1/link-boards/usba-ffffffffffff", &[("Authorization", &bearer)], Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Registering again rotates the token and clears the revoke.
    let (_, again) = register(&app, &api, &runtime).await;
    let rotated = again["relayToken"].as_str().unwrap();
    assert_ne!(rotated, token);
    assert_eq!(ticket(&app, BOARD, &token, json!({})).await.0, StatusCode::UNAUTHORIZED, "the old token stays dead");
    assert_eq!(ticket(&app, BOARD, rotated, json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn two_accounts_with_the_same_board_id_each_keep_their_own_row() {
    let app = TestApp::new().await;
    let (api_a, runtime_a) = account(&app, "u_link_one").await;
    let (api_b, runtime_b) = account(&app, "u_link_two").await;
    let (_, a) = register(&app, &api_a, &runtime_a).await;
    // Registering someone else's runtime is not found.
    let (status, _) = register(&app, &api_b, &runtime_a).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, b) = register(&app, &api_b, &runtime_b).await;
    assert_eq!(status, StatusCode::OK, "a guessed board id does not block the second account: {b}");
    let (token_a, token_b) = (a["relayToken"].as_str().unwrap(), b["relayToken"].as_str().unwrap());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM link_boards WHERE board_id = $1").bind(BOARD).fetch_one(&app.db).await.unwrap();
    assert_eq!(rows, 2);

    // B revokes its row: A's token still works, B's does not, and B never
    // reaches A's row with its own token.
    let bearer_b = format!("Bearer {api_b}");
    call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[("Authorization", &bearer_b)], Value::Null).await;
    assert_eq!(ticket(&app, BOARD, token_b, json!({})).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(ticket(&app, BOARD, token_a, json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE);
    let revoked_a: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM link_boards WHERE account = 'u_link_one'").fetch_one(&app.db).await.unwrap();
    assert!(revoked_a.is_none(), "B's revoke left A's row alone");
    let (_, list_b) = call(&app, "GET", "/api/v1/link-boards", &[("Authorization", &bearer_b)], Value::Null).await;
    assert_eq!(list_b["boards"].as_array().unwrap().len(), 1);
    assert_eq!(list_b["boards"][0]["runtimeDeviceId"], runtime_b.as_str());

    // Erasing account A removes its board row and nothing of B's.
    allternit_cloud_api::services::account_erase::erase_account(&app.state, "u_link_one").await.unwrap();
    let left: Vec<String> = sqlx::query_scalar("SELECT account FROM link_boards WHERE board_id = $1").bind(BOARD).fetch_all(&app.db).await.unwrap();
    assert_eq!(left, vec!["u_link_two".to_string()]);
    assert_eq!(ticket(&app, BOARD, token_a, json!({})).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn desktops_runtime_device_token_acts_as_its_owner_and_only_its_owner() {
    let app = TestApp::new().await;
    let (_, runtime) = account(&app, "u_link_desk").await;
    account(&app, "u_link_else").await;
    let owner = format!("Bearer {}", device_token("u_link_desk"));
    let other = format!("Bearer {}", device_token("u_link_else"));
    let board = |rt: &str| json!({ "boardId": BOARD, "runtimeDeviceId": rt });

    // No token at all: 401 on every account route.
    assert_eq!(call(&app, "POST", "/api/v1/link-boards", &[], board(&runtime)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/api/v1/link-boards", &[], Value::Null).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[], Value::Null).await.0, StatusCode::UNAUTHORIZED);
    // A device token that matches no runtime: 401.
    let forged = "Bearer allternit_runtime_forged_token";
    assert_eq!(call(&app, "GET", "/api/v1/link-boards", &[("Authorization", forged)], Value::Null).await.0, StatusCode::UNAUTHORIZED);

    // The owner's device token registers, lists and revokes.
    let (status, reg) = call(&app, "POST", "/api/v1/link-boards", &[("Authorization", &owner)], board(&runtime)).await;
    assert_eq!(status, StatusCode::OK, "{reg}");
    let token = reg["relayToken"].as_str().unwrap().to_string();
    let (_, list) = call(&app, "GET", "/api/v1/link-boards", &[("Authorization", &owner)], Value::Null).await;
    assert_eq!(list["boards"].as_array().unwrap().len(), 1, "{list}");

    // Another account's device token cannot reach it: its runtime is not
    // found, the board is not found, and its list is empty.
    let (status, _) = call(&app, "POST", "/api/v1/link-boards", &[("Authorization", &other)], board(&runtime)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[("Authorization", &other)], Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, theirs) = call(&app, "GET", "/api/v1/link-boards", &[("Authorization", &other)], Value::Null).await;
    assert_eq!(theirs, json!({ "boards": [] }));
    assert_eq!(ticket(&app, BOARD, &token, json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE, "the owner's board still works");

    // Revoke twice from Desktop: 200 both times; an unknown board is 404.
    for _ in 0..2 {
        let (status, body) = call(&app, "DELETE", &format!("/api/v1/link-boards/{BOARD}"), &[("Authorization", &owner)], Value::Null).await;
        assert_eq!((status, body), (StatusCode::OK, json!({ "revoked": true })));
    }
    let (status, _) = call(&app, "DELETE", "/api/v1/link-boards/usba-ffffffffffff", &[("Authorization", &owner)], Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(ticket(&app, BOARD, &token, json!({})).await.0, StatusCode::UNAUTHORIZED);
}
