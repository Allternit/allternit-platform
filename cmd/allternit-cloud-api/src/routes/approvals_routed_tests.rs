//! `GET /api/v1/approvals?approver=me` (Island bar + Team map, lane 14).
//! Real Postgres (schema-per-test, see `routes::test_support`): the Platform
//! chain through migrations 122 + 165 for `platform_approvals`, rows written
//! by the production `platform_v1::approvals::insert`; a column subset of
//! migration 130 for `meeting_tool_approvals` and of 001 for
//! `approval_requests` (the default list).

use axum::{body::Body, http::Request, routing::get, Router};
use tower::ServiceExt;

use super::*;
use crate::auth::models::AuthenticatedUser;
use crate::routes::platform_v1::approvals::{insert, NewApproval};
use crate::routes::test_support::{platform_schema, test_state, MockGateway};

const OWNER: &str = "user_owner";
const OTHER: &str = "user_other";

async fn setup() -> (Arc<ApiState>, Router) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    platform_schema(&state.db).await;
    sqlx::raw_sql(
        "CREATE TABLE meeting_tool_approvals (
            id UUID PRIMARY KEY, session_id UUID NOT NULL, user_id TEXT NOT NULL, tool TEXT NOT NULL,
            args JSONB NOT NULL DEFAULT '{}'::jsonb, args_summary TEXT NOT NULL, asked_by TEXT,
            status TEXT NOT NULL DEFAULT 'pending', requested_at TIMESTAMPTZ NOT NULL,
            resolved_at TIMESTAMPTZ, expires_at TIMESTAMPTZ NOT NULL);
         CREATE TABLE approval_requests (
            id TEXT NOT NULL, run_id TEXT, step_cursor TEXT, status TEXT DEFAULT 'pending',
            priority TEXT DEFAULT 'normal', title TEXT, description TEXT, action_type TEXT,
            action_params JSON, reasoning TEXT, requested_by TEXT, responded_by TEXT,
            response_message TEXT, timeout_seconds BIGINT,
            created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP, responded_at TIMESTAMPTZ);
         INSERT INTO platform_projects (id, owner_user_id, name, env) VALUES ('prj_1', 'user_owner', 'Test', 'sandbox');",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let auth = AuthContext {
        user: AuthenticatedUser { user_id: OWNER.into(), token_id: "tok_owner".into(), permissions: vec!["*".into()] },
        is_development: false,
    };
    let app = Router::new()
        .route("/api/v1/approvals", get(list_approvals))
        .layer(Extension(auth))
        .with_state(state.clone());
    (state, app)
}

async fn platform_row(state: &ApiState, source_ref: &str, routing: Value, status: &str) {
    let mut action = json!({ "action": "send_email", "summary": format!("ask {source_ref}") });
    action.as_object_mut().unwrap().extend(routing.as_object().unwrap().clone());
    insert(
        &state.db,
        NewApproval {
            project_id: "prj_1",
            source: "twin",
            source_ref,
            account_id: None,
            conversation_id: None,
            response_id: None,
            kind: "send_email".into(),
            proposed_action: action,
            status,
            expires_at: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
}

async fn meeting_row(state: &ApiState, user: &str, status: &str, expires_in_mins: i64) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO meeting_tool_approvals (id, session_id, user_id, tool, args_summary, asked_by, status, requested_at, expires_at) \
         VALUES ($1, $2, $3, 'calendar.create', 'Lunch Friday 12:00', 'Sam', $4, NOW(), NOW() + make_interval(mins => $5::int))",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(user)
    .bind(status)
    .bind(expires_in_mins as i32)
    .execute(&state.db)
    .await
    .unwrap();
    id
}

async fn get_json(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = app.clone().oneshot(Request::builder().uri(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn approver_me_lists_only_routed_pending_asks_and_the_default_list_is_unchanged() {
    let (state, app) = setup().await;
    platform_row(&state, "gap_mine", json!({ "approver_user_id": OWNER, "rule_id": "arl_1", "actor_ref": "b:agent_7" }), "pending").await;
    platform_row(&state, "gap_theirs", json!({ "approver_user_id": OTHER }), "pending").await;
    platform_row(&state, "gap_unrouted", json!({}), "pending").await;
    platform_row(&state, "gap_done", json!({ "approver_user_id": OWNER }), "approved").await;
    let mine = meeting_row(&state, OWNER, "pending", 5).await;
    meeting_row(&state, OTHER, "pending", 5).await;
    meeting_row(&state, OWNER, "pending", -1).await; // past its deadline
    meeting_row(&state, OWNER, "denied", 5).await;

    let (status, body) = get_json(&app, "/api/v1/approvals?approver=me").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["object"], "list");
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2, "{body}");

    let platform = data.iter().find(|a| a["store"] == "platform").expect("routed platform ask");
    assert_eq!(platform["approver_user_id"], OWNER);
    assert_eq!(platform["rule_id"], "arl_1");
    assert_eq!(platform["actor"], "b:agent_7");
    assert_eq!(platform["summary"], "ask gap_mine");
    assert_eq!(platform["state"], "pending");
    assert_eq!(platform["source"]["source_ref"], "gap_mine");
    assert!(platform["id"].as_str().unwrap().starts_with("platform:apr_"));
    assert_eq!(platform["resolve"], Value::Null);

    let meeting = data.iter().find(|a| a["store"] == "meeting").expect("meeting ask");
    assert_eq!(meeting["id"], format!("meeting:{mine}"));
    assert_eq!(meeting["approver_user_id"], OWNER);
    assert_eq!(meeting["summary"], "Lunch Friday 12:00");
    assert_eq!(meeting["resolve"]["method"], "POST");
    let path = meeting["resolve"]["path"].as_str().unwrap();
    assert!(path.starts_with("/api/v1/meeting-sessions/") && path.ends_with(&format!("/approvals/{mine}")), "{path}");

    // The stored routing columns are what the filter read.
    let stored: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT approver_user_id, rule_id, actor_ref FROM platform_approvals WHERE source_ref = 'gap_mine'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(stored, (Some(OWNER.into()), Some("arl_1".into()), Some("b:agent_7".into())));

    // No parameter: still the run approvals list (`approval_requests`, empty
    // here), a bare JSON array with none of the routed asks in it.
    let (status, body) = get_json(&app, "/api/v1/approvals").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!([]));

    let (status, _) = get_json(&app, "/api/v1/approvals?approver=someone").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn routed_path_answers_a_runtime_device_token_and_a_session_but_not_a_stranger() {
    use crate::routes::runtime_events::{RUNTIME_ID_HEADER, SIG_HEADER, TS_HEADER};
    use crate::routes::runtime_pairing::sha256_hex;
    use crate::routes::runtime_relay::sign_runtime_request;
    use crate::routes::test_support::seed_runtime_device;

    let (state, _) = setup().await;
    platform_row(&state, "gap_mine", json!({ "approver_user_id": OWNER, "actor_ref": "b:agent_7" }), "pending").await;
    platform_row(&state, "gap_theirs", json!({ "approver_user_id": OTHER }), "pending").await;
    seed_runtime_device(&state.db, "rt-1", OWNER).await;
    let app = Router::new().merge(routed_routes()).with_state(state.clone());
    let path = "/api/v1/approvals/routed";

    // The runtime signs the path without the query; it acts as its owner.
    let key = sha256_hex(b"token-of-rt-1");
    let ts = Utc::now().timestamp();
    let signed = Request::builder()
        .uri(format!("{path}?limit=10"))
        .header(RUNTIME_ID_HEADER, "rt-1")
        .header(TS_HEADER, ts.to_string())
        .header(SIG_HEADER, sign_runtime_request(&key, ts, "GET", path, b""))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(signed).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap()).unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1, "{body}");
    assert_eq!(data[0]["source"]["source_ref"], "gap_mine");

    // A bad signature is 401 (never falls through to a session).
    let forged = Request::builder()
        .uri(path)
        .header(RUNTIME_ID_HEADER, "rt-1")
        .header(TS_HEADER, ts.to_string())
        .header(SIG_HEADER, sign_runtime_request(&sha256_hex(b"nope"), ts, "GET", path, b""))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(forged).await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // A signed-in session works too (test stand-in header).
    let session = Request::builder().uri(path).header("x-test-session-user", OTHER).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(session).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap()).unwrap();
    assert_eq!(body["data"][0]["source"]["source_ref"], "gap_theirs");
}
