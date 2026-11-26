//! The person's status: GET/PUT /api/v1/me/status.
//!
//! `online`, `autopilot`, `dnd` (do not disturb) or `away`, saved in `account_prefs` under
//! [`USER_STATUS_KEY`] as `{ status, since }`. Nothing saved reads as `online` with `since: null`.
//! Every status but `online` holds non-urgent pushes ([`holds_pushes`], used by
//! `routes::notifications`), with the let-through level the person saved for quiet hours. The held
//! summary goes out from the sink loop once the status is back to `online` (and quiet hours are over).
//! Away is set by hand; nothing sets it on its own.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::error::ApiError;
use crate::routes::account_prefs::{read_pref, write_pref};
use crate::ApiState;

/// The `account_prefs.key` the status document is stored under.
pub const USER_STATUS_KEY: &str = "user_status";

/// Every status a person can pick.
pub const STATUSES: [&str; 4] = ["online", "autopilot", "dnd", "away"];

const BAD_STATUS: &str = "status must be online, autopilot, dnd or away";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/me/status", get(get_status).put(put_status))
}

/// The saved `{ status, since }`, or the default when nothing valid is saved.
fn normalize(saved: Option<Value>) -> Value {
    let status = saved
        .as_ref()
        .and_then(|v| v.get("status"))
        .and_then(Value::as_str)
        .filter(|s| STATUSES.contains(s));
    match status {
        Some(status) => json!({ "status": status, "since": saved.as_ref().and_then(|v| v.get("since")).cloned().unwrap_or(Value::Null) }),
        None => json!({ "status": "online", "since": null }),
    }
}

async fn read_status(db: &PgPool, user: &str) -> Result<Value, ApiError> {
    Ok(normalize(read_pref(db, user, USER_STATUS_KEY).await?))
}

/// True when `user`'s status holds non-urgent pushes: dnd, away or autopilot. A read error is false,
/// so pushes keep working.
pub async fn holds_pushes(db: &PgPool, user: &str) -> bool {
    match read_pref(db, user, USER_STATUS_KEY).await {
        Ok(saved) => normalize(saved)["status"].as_str().is_some_and(|s| s != "online"),
        Err(error) => {
            tracing::debug!("me_status: reading status failed: {error:?}");
            false
        }
    }
}

fn private(result: Result<Response, ApiError>) -> Response {
    let mut response = result.unwrap_or_else(IntoResponse::into_response);
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

async fn get_status(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    private(
        async {
            let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
            Ok(Json(read_status(&state.db, &user.id).await?).into_response())
        }
        .await,
    )
}

async fn put_status(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    private(
        async {
            let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
            let requested = serde_json::from_slice::<Value>(&body)
                .ok()
                .and_then(|v| v.get("status").and_then(Value::as_str).map(str::to_string))
                .filter(|s| STATUSES.contains(&s.as_str()));
            let Some(status) = requested else {
                return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": BAD_STATUS }))).into_response());
            };
            let current = read_status(&state.db, &user.id).await?;
            // `since` is when the status last changed; saving the same status keeps it.
            let since = if current["status"] == status.as_str() && !current["since"].is_null() {
                current["since"].clone()
            } else {
                json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true))
            };
            let document = json!({ "status": status, "since": since });
            write_pref(&state.db, &user.id, USER_STATUS_KEY, &document).await?;
            Ok(Json(document).into_response())
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, test_state, MockGateway};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const PATH: &str = "/api/v1/me/status";

    async fn router() -> (Router, PgPool) {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/145_account_prefs.sql")).execute(&state.db).await.expect("145 applies");
        let db = state.db.clone();
        (routes().with_state(state), db)
    }

    async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn me_status_needs_a_signed_in_caller() {
        let (router, _) = router().await;
        for method in ["GET", "PUT"] {
            let request = Request::builder()
                .method(method)
                .uri(PATH)
                .header("content-type", "application/json")
                .body(Body::from(json!({ "status": "dnd" }).to_string()))
                .unwrap();
            let (status, _) = send(&router, request).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn me_status_defaults_to_online_then_round_trips_and_holds_pushes() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let (router, db) = router().await;

        let (status, body) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({ "status": "online", "since": null }));

        let (status, saved) = send(&router, authed_request("PUT", PATH, &json!({ "status": "dnd" }).to_string())).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["status"], "dnd");
        assert!(saved["since"].as_str().is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()), "{saved}");

        let (status, read) = send(&router, authed_request("GET", PATH, "")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(read, saved);

        // The dev token's user is the one the PUT saved for.
        let user: String = sqlx::query_scalar("SELECT user_id FROM account_prefs WHERE key = $1")
            .bind(USER_STATUS_KEY)
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(holds_pushes(&db, &user).await, "dnd holds pushes");

        let (status, _) = send(&router, authed_request("PUT", PATH, &json!({ "status": "online" }).to_string())).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!holds_pushes(&db, &user).await, "online lets pushes through");
        assert!(!holds_pushes(&db, "someone_with_nothing_saved").await);
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn me_status_rejects_an_unknown_value() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let (router, _) = router().await;
        for body in [json!({ "status": "busy" }).to_string(), json!({}).to_string(), "not json".to_string()] {
            let (status, error) = send(&router, authed_request("PUT", PATH, &body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error, json!({ "error": BAD_STATUS }));
        }
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
