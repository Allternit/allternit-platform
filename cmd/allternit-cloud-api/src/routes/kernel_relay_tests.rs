//! Step 18: the kernel approval relay. Real Postgres (schema-per-test, `routes::test_support`)
//! with migration 171; the computer on the other end of the relay is a test connection that
//! answers like allternit-api's `/api/v1/kernel/approvals/resolve` (the network is the edge).

use super::*;
use crate::routes::runtime_relay::{answer_test_request, register_test_connection, CloudMessage};
use crate::routes::test_support::{seed_runtime_device, test_state, MockGateway};
use axum::http::HeaderValue;
use base64::Engine as _;

const OWNER: &str = "user_owner";
const RUNTIME: &str = "kr-rt-1";

async fn setup() -> Arc<ApiState> {
    // The cloud kernel key comes from the secret env; a fixed test seed here.
    std::env::set_var(crate::kernel::SEED_ENV, base64::engine::general_purpose::STANDARD.encode([9u8; 32]));
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(include_str!("../../migrations_pg/171_kernel_approval_relay.sql")).execute(&state.db).await.unwrap();
    seed_runtime_device(&state.db, RUNTIME, OWNER).await;
    sqlx::query("UPDATE runtime_devices SET capabilities = $2 WHERE id = $1")
        .bind(RUNTIME)
        .bind(r#"["runtime:connect","runtime:execute"]"#)
        .execute(&state.db)
        .await
        .unwrap();
    state
}

fn body(request_id: &str, deadline_secs: i64) -> ReceiveBody {
    let deadline = (Utc::now() + chrono::Duration::seconds(deadline_secs)).to_rfc3339();
    ReceiveBody {
        kernel: json!({
            "requestId": request_id, "runId": "run_1", "assignee": "owner", "actionClass": "money",
            "actionSummary": "Send invoice in_1 for $40.00", "actionHash": "sha256:abc", "deadline": deadline, "humanOnly": true,
            "actionPayload": { "secret": "never relayed" },
        }),
        attention_id: Some("att_1".into()),
        status: None,
    }
}

fn phone(extra: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("x-test-session-user", HeaderValue::from_static(OWNER));
    h.insert("authorization", HeaderValue::from_static("Bearer clerk-session-jwt"));
    for (k, v) in extra {
        h.insert(*k, HeaderValue::from_static(v));
    }
    h
}

async fn status_of(state: &ApiState, request_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM kernel_approval_relay WHERE request_id = $1").bind(request_id).fetch_one(&state.db).await.unwrap()
}

async fn receipt_kinds(state: &ApiState) -> Vec<String> {
    sqlx::query_scalar("SELECT kind FROM kernel_receipts WHERE principal = $1 ORDER BY idx").bind(OWNER).fetch_all(&state.db).await.unwrap()
}

async fn read(resp: Response) -> (StatusCode, Value) {
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn new_request_is_stored_pushed_once_and_listed_with_the_kernel_object() {
    let state = setup().await;
    let first = upsert(&state.db, OWNER, RUNTIME, body("apr_1", 300)).await.ok().unwrap();
    let push = first.push.expect("first sight of a pending request pushes");
    assert_eq!(push.data["category"], PUSH_CATEGORY);
    assert_eq!(push.data["requestId"], "apr_1");
    assert!(push.data.to_string().find("never relayed").is_none(), "action payload never reaches a phone");
    // Sent again (device retry): no second push.
    assert!(upsert(&state.db, OWNER, RUNTIME, body("apr_1", 300)).await.ok().unwrap().push.is_none());
    let items = pending_items(&state.db, OWNER).await.unwrap();
    assert_eq!(items.len(), 1);
    let k = &items[0]["kernel"];
    assert_eq!((k["requestId"].as_str(), k["actionClass"].as_str(), k["humanOnly"].as_bool()), (Some("apr_1"), Some("money"), Some(true)));
    assert_eq!(items[0]["runtimeId"], RUNTIME);
    // Someone else's computer cannot see it.
    assert!(pending_items(&state.db, "user_other").await.unwrap().is_empty());
}

#[tokio::test]
async fn phone_approve_forwards_to_the_computer_and_returns_its_signed_token() {
    let state = setup().await;
    upsert(&state.db, OWNER, RUNTIME, body("apr_2", 300)).await.ok().unwrap();
    let (connection, mut outgoing) = register_test_connection(RUNTIME).await;
    let seen = Arc::new(tokio::sync::Mutex::new(None::<(String, HashMap<String, String>, Value)>));
    let seen_w = seen.clone();
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            if let CloudMessage::Request { request_id, path, headers, body, body_encoding, .. } = message {
                let raw = if body_encoding == "base64" {
                    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(body).unwrap()).unwrap()
                } else {
                    body
                };
                *seen_w.lock().await = Some((path, headers, serde_json::from_str(&raw).unwrap()));
                // What allternit-api answers on approve: the signed ApprovalTokenV1.
                answer_test_request(&connection, &request_id, 200, r#"{"ok":true,"body":{"request_id":"apr_2","token":"eyJ.signed","key_id":"kid_device","approved_by":"user_owner"}}"#).await;
            }
        }
    });
    let decision = PhoneDecision { decision: "approve".into(), note: None };
    let (code, answer) = read(resolve_for(&state, RUNTIME, "apr_2", &phone(&[]), decision).await).await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    assert_eq!(answer["body"]["token"], "eyJ.signed");
    let (path, headers, fwd) = seen.lock().await.clone().expect("forwarded to the computer");
    assert_eq!(path, DEVICE_RESOLVE_PATH);
    assert_eq!(headers.get("x-allternit-human-proof").map(String::as_str), Some("clerk-session-jwt"));
    assert_eq!((fwd["request_id"].as_str(), fwd["decision"].as_str()), (Some("apr_2"), Some("approve")));
    assert_eq!(status_of(&state, "apr_2").await, "approved");
    assert_eq!(receipt_kinds(&state).await, ["relay_received", "relay_forwarded", "relay_result"]);
}

#[tokio::test]
async fn agent_credential_on_the_phone_route_is_refused_with_a_receipt() {
    let state = setup().await;
    upsert(&state.db, OWNER, RUNTIME, body("apr_3", 300)).await.ok().unwrap();
    for marker in ["x-allternit-agent-id", "x-allternit-run-id", "x-allternit-runtime-id"] {
        let decision = PhoneDecision { decision: "approve".into(), note: None };
        let (code, answer) = read(resolve_for(&state, RUNTIME, "apr_3", &phone(&[(marker, "agent_1")]), decision).await).await;
        assert_eq!(code, StatusCode::FORBIDDEN, "{marker}");
        assert_eq!(answer["error"]["code"], "forbidden_caller");
    }
    assert_eq!(status_of(&state, "apr_3").await, "pending", "nothing was forwarded or decided");
    assert_eq!(receipt_kinds(&state).await.iter().filter(|k| *k == "relay_refused").count(), 3);
}

#[tokio::test]
async fn relay_row_expires_with_the_request() {
    let state = setup().await;
    let out = upsert(&state.db, OWNER, RUNTIME, body("apr_4", -1)).await.ok().unwrap();
    assert!(out.push.is_none(), "an already-expired request is not pushed");
    assert!(pending_items(&state.db, OWNER).await.unwrap().is_empty());
    assert_eq!(status_of(&state, "apr_4").await, "expired");
    let decision = PhoneDecision { decision: "approve".into(), note: None };
    let (code, answer) = read(resolve_for(&state, RUNTIME, "apr_4", &phone(&[]), decision).await).await;
    assert_eq!((code, answer["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("token_expired")));
    // A decision made on the device closes the row; a later pending resend does not reopen it.
    upsert(&state.db, OWNER, RUNTIME, body("apr_5", 300)).await.ok().unwrap();
    let mut closed = body("apr_5", 300);
    closed.status = Some("closed".into());
    upsert(&state.db, OWNER, RUNTIME, closed).await.ok().unwrap();
    upsert(&state.db, OWNER, RUNTIME, body("apr_5", 300)).await.ok().unwrap();
    assert_eq!(status_of(&state, "apr_5").await, "closed");
}
