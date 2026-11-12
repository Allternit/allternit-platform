//! Usage report + request log tests (console Wave 3, lane L3B). Real Postgres
//! (schema-per-test, see `routes::test_support`); migrations 003, 050 and 121
//! are applied for real on a minimal `inference_usage` (no users FK).

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use tower::ServiceExt;

use super::*;
use crate::{
    routes::test_support::{test_state, MockGateway},
    services::inference_settlement::{record_request_bodies, InferenceAttribution, MAX_CAPTURED_BODY_BYTES},
};

async fn setup() -> (Arc<ApiState>, Router) {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    sqlx::raw_sql(
        "CREATE TABLE inference_usage (
            id TEXT PRIMARY KEY, user_id TEXT NOT NULL, model TEXT NOT NULL,
            prompt_tokens BIGINT NOT NULL, completion_tokens BIGINT NOT NULL,
            cost_usd DOUBLE PRECISION NOT NULL, wholesale_cost_usd DOUBLE PRECISION,
            estimated BOOLEAN NOT NULL DEFAULT FALSE, pool_id TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
    )
    .execute(&state.db)
    .await
    .unwrap();
    for sql in [
        include_str!("../../migrations_pg/003_api_keys.sql"),
        include_str!("../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../migrations_pg/125_inference_attribution.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", ""))
            .execute(&state.db)
            .await
            .expect("migration applies");
    }
    let app = routes().with_state(state.clone());
    (state, app)
}

/// An `allternit_*` API token for `user` (the non-Clerk path of resolve_user).
async fn token(state: &ApiState, user: &str) -> String {
    let token = format!("allternit_usage_test_{user}");
    sqlx::query(
        "INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) \
         VALUES ($1, $2, 'usage test', $3, '[\"*\"]')",
    )
    .bind(format!("tok_{user}"))
    .bind(crate::auth::middleware::hash_api_token(&token))
    .bind(user)
    .execute(&state.db)
    .await
    .unwrap();
    token
}

async fn project(state: &ApiState, id: &str, owner: &str, org: Option<&str>) {
    sqlx::query("INSERT INTO platform_projects (id, owner_user_id, org_id, name, env) VALUES ($1, $2, $3, 'Test', 'sandbox')")
        .bind(id)
        .bind(owner)
        .bind(org)
        .execute(&state.db)
        .await
        .unwrap();
}

fn at(raw: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
}

async fn usage_row(
    state: &ApiState,
    id: &str,
    user: &str,
    project_id: Option<&str>,
    created_at: &str,
    cost_usd: f64,
    request_id: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO inference_usage (id, user_id, model, prompt_tokens, completion_tokens, cost_usd, \
            created_at, project_id, client, request_id, latency_ms) \
         VALUES ($1, $2, 'kimi-k3', 100, 50, $3, $4, $5, 'api', $6, 120)",
    )
    .bind(id)
    .bind(user)
    .bind(cost_usd)
    .bind(at(created_at))
    .bind(project_id)
    .bind(request_id)
    .execute(&state.db)
    .await
    .unwrap();
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

async fn get(app: &Router, path: &str, token: Option<&str>) -> Reply {
    let mut builder = Request::builder().method("GET").uri(path);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app.clone().oneshot(builder.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 22).await.unwrap();
    Reply { status, headers, body: String::from_utf8(bytes.to_vec()).unwrap() }
}

fn request_ids(reply: &Reply) -> Vec<String> {
    reply.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["request_id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_user_reads_only_their_own_usage_and_requests() {
    let (state, app) = setup().await;
    project(&state, "proj_a", "user_a", None).await;
    project(&state, "proj_b", "user_b", None).await;
    usage_row(&state, "u_a1", "user_a", Some("proj_a"), "2026-11-01T10:00:00Z", 0.25, Some("req_a1")).await;
    usage_row(&state, "u_a2", "user_a", None, "2026-11-01T11:00:00Z", 0.50, Some("req_a2")).await;
    usage_row(&state, "u_b1", "user_b", Some("proj_b"), "2026-11-01T12:00:00Z", 9.0, Some("req_b1")).await;
    let a = token(&state, "user_a").await;
    let b = token(&state, "user_b").await;

    let report = get(&app, "/api/v1/me/usage/report?from=2026-11-01&to=2026-11-02&group_by=model", Some(a.as_str())).await;
    assert_eq!(report.status, StatusCode::OK, "{}", report.body);
    assert_eq!(
        report.json()["rows"],
        json!([{ "group": "kimi-k3", "requests": 2, "prompt_tokens": 200, "completion_tokens": 100, "cost_usd": 0.75 }])
    );

    let list = get(&app, "/api/v1/me/usage/requests", Some(a.as_str())).await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    assert_eq!(request_ids(&list), ["req_a2", "req_a1"], "newest first, only A's");
    assert_eq!(list.json()["data"][1]["project_id"], json!("proj_a"));
    assert_eq!(list.json()["data"][1]["latency_ms"], json!(120));

    // B's request is a 404 to A and readable by B.
    assert_eq!(get(&app, "/api/v1/me/usage/requests/req_b1", Some(a.as_str())).await.status, StatusCode::NOT_FOUND);
    let own = get(&app, "/api/v1/me/usage/requests/req_b1", Some(b.as_str())).await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.body);
    assert_eq!(own.json()["request_id"], json!("req_b1"));
    assert_eq!(request_ids(&get(&app, "/api/v1/me/usage/requests", Some(b.as_str())).await), ["req_b1"]);

    // B's project as a filter is a 404, never an empty report.
    assert_eq!(
        get(&app, "/api/v1/me/usage/report?project_id=proj_b", Some(a.as_str())).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/v1/me/usage/requests?project_id=proj_b", Some(a.as_str())).await.status,
        StatusCode::NOT_FOUND
    );
    let own_project = get(&app, "/api/v1/me/usage/requests?project_id=proj_a", Some(a.as_str())).await;
    assert_eq!(request_ids(&own_project), ["req_a1"]);

    // A token never carries an org, so scope=org is refused; no caller is a 401.
    assert_eq!(get(&app, "/api/v1/me/usage/report?scope=org", Some(a.as_str())).await.status, StatusCode::FORBIDDEN);
    assert_eq!(get(&app, "/api/v1/me/usage/report", None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(get(&app, "/api/v1/me/usage/requests", None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn org_scope_is_admin_only_and_covers_only_that_org() {
    let (state, _) = setup().await;
    project(&state, "proj_org_a", "user_1", Some("org_a")).await;
    project(&state, "proj_org_b", "user_2", Some("org_b")).await;
    project(&state, "proj_personal", "user_1", None).await;
    usage_row(&state, "u1", "user_1", Some("proj_org_a"), "2026-11-01T10:00:00Z", 1.0, Some("req_u1")).await;
    usage_row(&state, "u2", "user_3", Some("proj_org_a"), "2026-11-01T11:00:00Z", 2.0, Some("req_u2")).await;
    usage_row(&state, "u3", "user_2", Some("proj_org_b"), "2026-11-01T12:00:00Z", 4.0, Some("req_u3")).await;
    usage_row(&state, "u4", "user_1", Some("proj_personal"), "2026-11-01T13:00:00Z", 8.0, Some("req_u4")).await;
    usage_row(&state, "u5", "user_1", None, "2026-11-01T14:00:00Z", 16.0, Some("req_u5")).await;

    let admin = UsageViewer { user_id: "admin_a".into(), org_id: Some("org_a".into()), org_admin: true };
    let scope = resolve_scope(&admin, Some("org")).unwrap();
    assert_eq!(scope, UsageScope::Org("org_a".into()));
    let filter = UsageFilter::scoped(scope.clone());

    let rows = usage_report_rows(&state.db, &filter, GroupBy::Project).await.unwrap();
    assert_eq!(
        rows,
        vec![ReportRow {
            group: Some("proj_org_a".into()),
            requests: 2,
            prompt_tokens: 200,
            completion_tokens: 100,
            cost_usd: 3.0,
        }],
        "org a's projects only: no org b, no personal project, no projectless rows"
    );
    let (page, next) = usage_request_page(&state.db, &filter, None, 50).await.unwrap();
    let ids: Vec<&str> = page.iter().map(|row| row.request_id.as_str()).collect();
    assert_eq!(ids, ["req_u2", "req_u1"]);
    assert!(next.is_none());
    assert!(matches!(
        usage_request_detail(&state.db, scope.clone(), "req_u3").await,
        Err(ApiError::NotFound(_))
    ));
    assert_eq!(usage_request_detail(&state.db, scope, "req_u2").await.unwrap()["cost_usd"], json!(2.0));

    // Another org's project is a 404 even to an admin; their own org's passes.
    assert!(matches!(check_project(&state.db, &admin, Some("proj_org_b")).await, Err(ApiError::NotFound(_))));
    assert!(check_project(&state.db, &admin, Some("proj_org_a")).await.is_ok());

    // scope=me for the admin is their own rows (none here).
    let me = UsageFilter::scoped(resolve_scope(&admin, None).unwrap());
    assert!(usage_report_rows(&state.db, &me, GroupBy::Day).await.unwrap().is_empty());

    // A member who is not an admin, and a token caller, cannot use scope=org.
    let member = UsageViewer { user_id: "user_3".into(), org_id: Some("org_a".into()), org_admin: false };
    assert!(matches!(resolve_scope(&member, Some("org")), Err(ApiError::Forbidden(_))));
    let token_caller = UsageViewer { user_id: "user_1".into(), org_id: None, org_admin: false };
    assert!(matches!(resolve_scope(&token_caller, Some("org")), Err(ApiError::Forbidden(_))));
    assert!(matches!(resolve_scope(&admin, Some("everyone")), Err(ApiError::BadRequest(_))));
}

#[tokio::test]
async fn csv_report_has_a_header_row() {
    let (state, app) = setup().await;
    usage_row(&state, "u1", "user_a", None, "2026-11-01T10:00:00Z", 0.25, Some("req_1")).await;
    usage_row(&state, "u2", "user_a", None, "2026-11-02T10:00:00Z", 0.5, Some("req_2")).await;
    let a = token(&state, "user_a").await;

    let reply = get(&app, "/api/v1/me/usage/report?from=2026-11-01&to=2026-11-03&group_by=day&format=csv", Some(a.as_str())).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(reply.headers["content-type"].to_str().unwrap().starts_with("text/csv"));
    let disposition = reply.headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(disposition.contains("allternit-usage-day-2026-11-01-2026-11-03.csv"), "{disposition}");
    let lines: Vec<&str> = reply.body.lines().collect();
    assert_eq!(lines[0], "group,requests,prompt_tokens,completion_tokens,cost_usd");
    assert_eq!(lines[1..], ["2026-11-01,1,100,50,0.250000", "2026-11-02,1,100,50,0.500000"]);

    // No rows still answers the header.
    let empty = get(&app, "/api/v1/me/usage/report?from=2026-10-01&to=2026-10-02&format=csv", Some(a.as_str())).await;
    assert_eq!(empty.body, format!("{CSV_HEADER}\n"));

    assert_eq!(get(&app, "/api/v1/me/usage/report?format=xml", Some(a.as_str())).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(get(&app, "/api/v1/me/usage/report?group_by=week", Some(a.as_str())).await.status, StatusCode::BAD_REQUEST);
}

#[test]
fn csv_fields_are_quoted_and_formulas_defused() {
    assert_eq!(csv_field("kimi-k3"), "kimi-k3");
    assert_eq!(csv_field("a,b"), "\"a,b\"");
    assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_field("=SUM(A1)"), "'=SUM(A1)");
    assert_eq!(csv_field("@cmd"), "'@cmd");
}

#[tokio::test]
async fn to_is_exclusive_and_from_inclusive() {
    let (state, app) = setup().await;
    usage_row(&state, "u1", "user_a", None, "2026-11-01T00:00:00Z", 1.0, Some("req_start")).await;
    usage_row(&state, "u2", "user_a", None, "2026-11-01T23:59:59.999999Z", 2.0, Some("req_late")).await;
    usage_row(&state, "u3", "user_a", None, "2026-11-02T00:00:00Z", 4.0, Some("req_next")).await;
    let a = token(&state, "user_a").await;

    let day = get(&app, "/api/v1/me/usage/report?from=2026-11-01&to=2026-11-02&group_by=day", Some(a.as_str())).await;
    assert_eq!(day.status, StatusCode::OK, "{}", day.body);
    assert_eq!(
        day.json()["rows"],
        json!([{ "group": "2026-11-01", "requests": 2, "prompt_tokens": 200, "completion_tokens": 100, "cost_usd": 3.0 }])
    );

    // RFC 3339 bounds work too, with the same edges.
    let rfc = get(
        &app,
        "/api/v1/me/usage/report?from=2026-11-01T00:00:00Z&to=2026-11-01T12:00:00Z&group_by=day",
        Some(a.as_str()),
    )
    .await;
    assert_eq!(rfc.json()["rows"][0]["requests"], json!(1));

    let list = get(&app, "/api/v1/me/usage/requests?from=2026-11-01&to=2026-11-02", Some(a.as_str())).await;
    assert_eq!(request_ids(&list), ["req_late", "req_start"]);
    let next_day = get(&app, "/api/v1/me/usage/requests?from=2026-11-02", Some(a.as_str())).await;
    assert_eq!(request_ids(&next_day), ["req_next"]);

    assert_eq!(
        get(&app, "/api/v1/me/usage/report?from=2026-11-02&to=2026-11-02", Some(a.as_str())).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn report_range_defaults_and_bounds() {
    let now = at("2026-11-12T15:00:00Z");
    assert_eq!(
        report_range(None, None, now).unwrap(),
        (at("2026-10-14T00:00:00Z"), at("2026-11-13T00:00:00Z")),
        "30 UTC days, today included"
    );
    assert_eq!(
        report_range(None, Some("2026-11-05"), now).unwrap(),
        (at("2026-10-06T00:00:00Z"), at("2026-11-05T00:00:00Z"))
    );
    assert!(matches!(report_range(Some("2026-11-05"), Some("2026-11-01"), now), Err(ApiError::BadRequest(_))));
    assert!(matches!(report_range(Some("2024-01-01"), Some("2026-01-01"), now), Err(ApiError::BadRequest(_))));
    assert!(matches!(report_range(Some("yesterday"), None, now), Err(ApiError::BadRequest(_))));
    // A `+00:00` offset decoded from a query string arrives as a space.
    assert_eq!(parse_bound("2026-11-01T12:00:00 00:00", "to").unwrap(), at("2026-11-01T12:00:00Z"));
    assert_eq!(parse_bound("2026-11-01T12:00:00+02:00", "to").unwrap(), at("2026-11-01T10:00:00Z"));
}

#[tokio::test]
async fn bodies_are_returned_only_when_captured() {
    let (state, app) = setup().await;
    project(&state, "proj_a", "user_a", None).await;
    usage_row(&state, "u_cap", "user_a", Some("proj_a"), "2026-11-01T10:00:00Z", 0.1, Some("req_cap")).await;
    usage_row(&state, "u_exp", "user_a", Some("proj_a"), "2026-11-01T10:01:00Z", 0.1, Some("req_exp")).await;
    usage_row(&state, "u_none", "user_a", Some("proj_a"), "2026-11-01T10:02:00Z", 0.1, Some("req_none")).await;
    // A row written before request ids existed is found by its usage id.
    usage_row(&state, "u_old", "user_a", None, "2026-10-01T10:00:00Z", 0.1, None).await;
    let a = token(&state, "user_a").await;
    let b = token(&state, "user_b").await;

    let request = json!({ "model": "kimi-k3", "messages": [{ "role": "user", "content": "hi" }] });
    let response = json!({ "choices": [{ "message": { "role": "assistant", "content": "hello" } }] });
    let captured = InferenceAttribution {
        project_id: Some("proj_a".into()),
        request_id: Some("req_cap".into()),
        capture_request: Some(Arc::new(request.clone())),
        ..Default::default()
    };
    record_request_bodies(&state.db, &captured, Some(response.clone())).await;
    // No captured request (flag off or project not opted in): nothing kept.
    let plain = InferenceAttribution {
        project_id: Some("proj_a".into()),
        request_id: Some("req_none".into()),
        ..Default::default()
    };
    record_request_bodies(&state.db, &plain, Some(json!({ "x": 1 }))).await;
    // Over the cap: nothing kept.
    let big = InferenceAttribution {
        project_id: Some("proj_a".into()),
        request_id: Some("req_big".into()),
        capture_request: Some(Arc::new(json!({ "blob": "x".repeat(MAX_CAPTURED_BODY_BYTES) }))),
        ..Default::default()
    };
    record_request_bodies(&state.db, &big, None).await;
    // Expired bodies are never served.
    sqlx::query(
        "INSERT INTO inference_request_bodies (request_id, project_id, request, response, expires_at) \
         VALUES ('req_exp', 'proj_a', '{\"a\":1}', '{\"b\":2}', NOW() - INTERVAL '1 day')",
    )
    .execute(&state.db)
    .await
    .unwrap();

    let kept: Vec<(String, bool)> = sqlx::query_as(
        "SELECT request_id, expires_at > NOW() + INTERVAL '29 days' FROM inference_request_bodies ORDER BY request_id",
    )
    .fetch_all(&state.db)
    .await
    .unwrap();
    assert_eq!(kept, [("req_cap".to_string(), true), ("req_exp".to_string(), false)]);

    let detail = get(&app, "/api/v1/me/usage/requests/req_cap", Some(a.as_str())).await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.body);
    assert_eq!(detail.json()["request"], request);
    assert_eq!(detail.json()["response"], response);
    assert_eq!(detail.json()["project_id"], json!("proj_a"));

    for id in ["req_exp", "req_none", "u_old"] {
        let reply = get(&app, &format!("/api/v1/me/usage/requests/{id}"), Some(a.as_str())).await;
        assert_eq!(reply.status, StatusCode::OK, "{id}: {}", reply.body);
        assert_eq!(reply.json()["request_id"], json!(id));
        assert_eq!(reply.json()["request"], Value::Null, "{id}");
        assert_eq!(reply.json()["response"], Value::Null, "{id}");
    }

    // Another user never sees the captured bodies.
    assert_eq!(get(&app, "/api/v1/me/usage/requests/req_cap", Some(b.as_str())).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn requests_page_with_a_keyset_cursor() {
    let (state, app) = setup().await;
    usage_row(&state, "u_p1", "user_a", None, "2026-11-01T10:00:00Z", 0.1, Some("req_p1")).await;
    usage_row(&state, "u_p2", "user_a", None, "2026-11-01T11:00:00Z", 0.1, Some("req_p2")).await;
    usage_row(&state, "u_p3", "user_a", None, "2026-11-01T11:00:00Z", 0.1, Some("req_p3")).await;
    let a = token(&state, "user_a").await;

    let first = get(&app, "/api/v1/me/usage/requests?limit=2", Some(a.as_str())).await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(request_ids(&first), ["req_p3", "req_p2"], "ties order by id");
    assert_eq!(first.json()["has_more"], json!(true));
    let cursor = first.json()["next_cursor"].as_str().unwrap().to_string();

    let second = get(&app, &format!("/api/v1/me/usage/requests?limit=2&cursor={cursor}"), Some(a.as_str())).await;
    assert_eq!(request_ids(&second), ["req_p1"]);
    assert_eq!(second.json()["next_cursor"], Value::Null);

    for bad in ["limit=0", "limit=201", "limit=ten", "cursor=zz"] {
        let reply = get(&app, &format!("/api/v1/me/usage/requests?{bad}"), Some(a.as_str())).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(get(&app, "/api/v1/me/usage/requests?limit=200", Some(a.as_str())).await.status, StatusCode::OK);
}

#[tokio::test]
async fn log_bodies_is_set_by_the_project_owner_only() {
    let (state, _) = setup().await;
    project(&state, "proj_a", "user_a", None).await;
    let owner = Principal { user_id: "user_a".into(), org_id: None, org_admin: false };
    let stranger = Principal { user_id: "user_b".into(), org_id: None, org_admin: false };

    let mut loaded = projects::get_project(&state.db, &owner, "proj_a").await.unwrap();
    assert!(!loaded.log_bodies, "off by default");
    assert!(projects::set_log_bodies(&state.db, &stranger, "proj_a", true).await.is_err());
    projects::set_log_bodies(&state.db, &owner, "proj_a", true).await.unwrap();
    projects::load_log_bodies(&state.db, &mut loaded).await;
    assert!(loaded.log_bodies);
}
