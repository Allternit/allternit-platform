//! Usage meter (plan v3 lane 29): events move a link's bytes, a replay counts
//! once, and an account over 20 GB gets 429 cap_reached on a new live link.

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::routes::share_links::MAX_LIVE_BYTES_MONTH;
use crate::routes::test_support::{authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER};

const SECRET: &str = "test-usage-secret-0123456789abcdef-xyz";

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../migrations_pg/085_artifacts_v2.sql"),
        include_str!("../../migrations_pg/161_apps_mcp_2026.sql"),
        include_str!("../../migrations_pg/164_share_link_usage.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("migration");
    }
    // Live links need the relay admission secret (lane 17b).
    std::env::set_var(crate::routes::share_links::RELAY_SECRET_ENV, "test-relay-admission-secret-0123456789");
    state
}

async fn post_usage(state: &Arc<ApiState>, secret: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder().method("POST").uri("/api/v1/internal/share-links/usage").header("content-type", "application/json");
    if let Some(s) = secret {
        req = req.header(SECRET_HEADER, s);
    }
    let res = routes().with_state(state.clone()).oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn post_relay_batch(state: &Arc<ApiState>, secret: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/internal/share-links/usage")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {secret}"));
    let res = routes().with_state(state.clone()).oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn create_live(state: &Arc<ApiState>, device: &str, name: &str) -> (StatusCode, Value) {
    let body = json!({ "kind": "live", "subject": "mcp", "runtime_device_id": device, "subdomain": name }).to_string();
    let res = crate::routes::share_links::routes()
        .with_state(state.clone())
        .oneshot(authed_request("POST", "/api/v1/share-links", &body))
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
#[serial_test::serial]
async fn usage_events_move_a_links_bytes_once_and_over_20_gb_blocks_new_live_links() {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let _g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    let state = setup().await;
    let device = format!("rt_{}", uuid::Uuid::new_v4().simple());
    seed_runtime_device(&state.db, &device, DEV_USER).await;
    let name = format!("u{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let (status, link) = create_live(&state, &device, &name).await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    let id = link["id"].as_str().unwrap().to_string();
    let host = format!("{name}.live.allternitusercontent.com");

    // Auth: not configured 503, wrong secret 401.
    std::env::remove_var(SECRET_ENV);
    assert_eq!(post_usage(&state, Some(SECRET), json!({})).await.0, StatusCode::SERVICE_UNAVAILABLE);
    std::env::set_var(SECRET_ENV, SECRET);
    assert_eq!(post_usage(&state, Some("wrong-wrong-wrong-wrong-wrong-wrong-xx"), json!({})).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(post_usage(&state, None, json!({})).await.0, StatusCode::UNAUTHORIZED);

    // Events add bytes; the same event again adds nothing; an unknown host is unmatched.
    let ev = json!({ "id": "evt_1", "hostname": host, "bytes_in": 100, "bytes_out": 900 });
    let (status, out) = post_usage(&state, Some(SECRET), json!({ "events": [ev.clone(), ev.clone(), { "id": "evt_x", "hostname": "nobody.live.allternitusercontent.com", "bytes": 5 }] })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!((out["accepted"].as_i64(), out["duplicate"].as_i64(), out["unmatched"].as_i64()), (Some(1), Some(1), Some(1)));
    let (_, out) = post_usage(&state, Some(SECRET), ev).await;
    assert_eq!(out["duplicate"], 1, "a replay in a later request counts once");
    // A window-keyed event (no id) also dedupes on (tunnel, window).
    let w = json!({ "tunnel_id": "t1", "window_start": "2999-01-01T00:00:00Z", "hostname": host, "bytes": 50 });
    post_usage(&state, Some(SECRET), w.clone()).await;
    assert_eq!(post_usage(&state, Some(SECRET), w).await.1["duplicate"], 1);
    // OpenTunnel v0.6.2's real post: a bare array of envelopes, bearer auth; only connection.closed has bytes.
    let closed = json!({ "source": "opentunnel", "type": "connection.closed", "timestamp": "2999-02-01T00:00:01.000Z",
        "payload": { "schema_version": 1, "tunnel_id": name, "outcome": "closed", "duration_ms": 12, "bytes_in": 3, "bytes_out": 4 } });
    let created = json!({ "source": "opentunnel", "type": "tunnel.created", "timestamp": "2999-02-01T00:00:00.000Z",
        "payload": { "schema_version": 1, "tunnel_id": name } });
    let (status, out) = post_relay_batch(&state, SECRET, json!([created, closed.clone(), closed])).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!((out["accepted"].as_i64(), out["duplicate"].as_i64(), out["ignored"].as_i64()), (Some(1), Some(1), Some(1)), "{out}");

    let (month, total): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT bytes_month FROM share_links WHERE id = $1), \
                (SELECT COALESCE(SUM(bytes), 0)::BIGINT FROM share_link_usage WHERE link_id = $1)",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(month, 1000, "link bytes moved by exactly one event");
    assert_eq!(total, 1057, "future-month events (window + relay) are kept apart from this month");

    // Over 20 GB this month: a new live link is refused with cap_reached bytes.
    let big = json!({ "id": "evt_big", "hostname": host, "bytes": MAX_LIVE_BYTES_MONTH });
    assert_eq!(post_usage(&state, Some(SECRET), big).await.1["accepted"], 1);
    let (status, body) = create_live(&state, &device, &format!("{name}b")).await;
    assert_eq!((status, body["error"].as_str(), body["cap"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("cap_reached"), Some("bytes")), "{body}");
    assert!(body["used"].as_i64().unwrap() >= MAX_LIVE_BYTES_MONTH);
    std::env::remove_var(SECRET_ENV);
}
