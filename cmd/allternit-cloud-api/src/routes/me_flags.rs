//! GET/PUT /api/v1/me/flags — small per-account UI flags.
//!
//! Web and the PWA write these straight into the Clerk user's
//! `unsafe_metadata` with the Clerk client. Allternit Desktop signs in
//! through the browser and has no Clerk client, only a session JWT, so it
//! reads and writes the same keys here and the flag follows the account
//! across Desktop, web and phone (first user: the "Meet your bots" card,
//! `botsIntroSeenAt`).
//!
//! Only the keys in [`ALLOWED_FLAGS`] are readable or writable; each value
//! is an RFC 3339 time. Writes go through Clerk's metadata endpoint, which
//! deep-merges, so other `unsafe_metadata` keys survive. A missing
//! `CLERK_SECRET_KEY` or a Clerk failure answers 503 (fail closed).

use axum::{
    extract::State,
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::{auth, error::ApiError, services::user_trust::TrustConfig, ApiState};

/// Flags a caller may read or write. Anything else in a PUT body is a 400.
pub(crate) const ALLOWED_FLAGS: &[&str] = &["botsIntroSeenAt"];

/// Longest accepted value (an ISO time with millis and offset is ~29).
const MAX_FLAG_VALUE_LEN: usize = 40;
const CLERK_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/me/flags", get(get_my_flags).put(put_my_flags))
}

async fn get_my_flags(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    read_flags_with(&TrustConfig::from_env(), &user.id).await.map(Json)
}

async fn put_my_flags(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    let updates = validate_updates(&body)?;
    write_flags_with(&TrustConfig::from_env(), &user.id, updates)
        .await
        .map(Json)
}

/// Check a PUT body: a non-empty object of allowlisted keys, each an
/// RFC 3339 time string of at most [`MAX_FLAG_VALUE_LEN`] chars.
pub(crate) fn validate_updates(body: &Value) -> Result<Map<String, Value>, ApiError> {
    let Some(object) = body.as_object() else {
        return Err(ApiError::BadRequest("Body must be a JSON object of flags".to_string()));
    };
    if object.is_empty() {
        return Err(ApiError::BadRequest("No flags given".to_string()));
    }
    let mut updates = Map::new();
    for (key, value) in object {
        if !ALLOWED_FLAGS.contains(&key.as_str()) {
            return Err(ApiError::BadRequest(format!("Unknown flag '{key}'")));
        }
        let Some(text) = value.as_str() else {
            return Err(ApiError::BadRequest(format!("Flag '{key}' must be an ISO-8601 time string")));
        };
        if text.len() > MAX_FLAG_VALUE_LEN || chrono::DateTime::parse_from_rfc3339(text).is_err() {
            return Err(ApiError::BadRequest(format!("Flag '{key}' must be an ISO-8601 time string")));
        }
        updates.insert(key.clone(), Value::String(text.to_string()));
    }
    Ok(updates)
}

/// The response shape for a Clerk user object: every allowlisted flag,
/// `null` when unset or not a string.
pub(crate) fn flags_response(clerk_user: &Value) -> Value {
    let metadata = clerk_user.get("unsafe_metadata");
    let flags: Map<String, Value> = ALLOWED_FLAGS
        .iter()
        .map(|key| {
            let value = metadata
                .and_then(|m| m.get(*key))
                .and_then(Value::as_str)
                .map(|s| Value::String(s.to_string()))
                .unwrap_or(Value::Null);
            (key.to_string(), value)
        })
        .collect();
    json!({ "flags": flags })
}

/// Clerk's `PATCH /v1/users/{id}/metadata` body; Clerk deep-merges it.
pub(crate) fn merge_body(updates: Map<String, Value>) -> Value {
    json!({ "unsafe_metadata": updates })
}

pub(crate) async fn read_flags_with(config: &TrustConfig, user_id: &str) -> Result<Value, ApiError> {
    let (client, secret) = clerk_client(config)?;
    let request = client.get(user_url(config, user_id, "")).bearer_auth(secret);
    clerk_user(request, user_id).await.map(|user| flags_response(&user))
}

pub(crate) async fn write_flags_with(
    config: &TrustConfig,
    user_id: &str,
    updates: Map<String, Value>,
) -> Result<Value, ApiError> {
    let (client, secret) = clerk_client(config)?;
    let request = client
        .patch(user_url(config, user_id, "/metadata"))
        .bearer_auth(secret)
        .json(&merge_body(updates));
    clerk_user(request, user_id).await.map(|user| flags_response(&user))
}

fn clerk_client<'a>(config: &'a TrustConfig) -> Result<(reqwest::Client, &'a str), ApiError> {
    let Some(secret) = config.clerk_secret_key.as_deref().filter(|s| !s.is_empty()) else {
        return Err(ApiError::ServiceUnavailable(
            "Account flags are unavailable: CLERK_SECRET_KEY is not set".to_string(),
        ));
    };
    let client = reqwest::Client::builder()
        .timeout(CLERK_REQUEST_TIMEOUT)
        .build()
        .map_err(|e| ApiError::Internal(format!("Clerk client build failed: {e}")))?;
    Ok((client, secret))
}

fn user_url(config: &TrustConfig, user_id: &str, suffix: &str) -> String {
    format!(
        "{}/v1/users/{}{}",
        config.clerk_api_base.trim_end_matches('/'),
        user_id,
        suffix
    )
}

/// Send a Clerk user request and return the user object. Clerk 404 is a
/// 404 (no such account); any other failure is a 503.
async fn clerk_user(request: reqwest::RequestBuilder, user_id: &str) -> Result<Value, ApiError> {
    let unavailable = || ApiError::ServiceUnavailable("Account flags could not be reached — retry in a moment".to_string());
    let response = request.send().await.map_err(|error| {
        tracing::warn!(user_id = %user_id, "me/flags: Clerk request failed: {error}");
        unavailable()
    })?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ApiError::NotFound("Account not found".to_string()));
    }
    if !status.is_success() {
        tracing::warn!(user_id = %user_id, status = %status, "me/flags: Clerk answered non-success");
        return Err(unavailable());
    }
    response.json::<Value>().await.map_err(|error| {
        tracing::warn!(user_id = %user_id, "me/flags: Clerk response unparseable: {error}");
        unavailable()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn config(base: &str) -> TrustConfig {
        TrustConfig {
            skip: false,
            clerk_api_base: base.to_string(),
            clerk_secret_key: Some("sk_test_mock".to_string()),
            cache_ttl: chrono::Duration::hours(24),
        }
    }

    /// (method, path, authorization, body) per Clerk call.
    type Calls = Arc<Mutex<Vec<(String, String, String, Value)>>>;

    /// Mock Clerk: `GET /v1/users/:id` and `PATCH /v1/users/:id/metadata`
    /// answer with a user whose `unsafe_metadata` holds `stored` (the PATCH
    /// merges its body's `unsafe_metadata` first, like Clerk). Any id other
    /// than `user_1` is a 404; `fail` answers 500 to everything.
    async fn spawn_mock_clerk(stored: Value, fail: bool) -> (String, Calls) {
        use axum::{
            extract::Path,
            http::{HeaderMap, Method, StatusCode},
            response::IntoResponse,
            routing::{get, patch},
        };

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let stored = Arc::new(Mutex::new(stored));
        let handler = {
            let calls = calls.clone();
            let stored = stored.clone();
            move |method: Method, Path(id): Path<String>, headers: HeaderMap, body: axum::body::Bytes| {
                let calls = calls.clone();
                let stored = stored.clone();
                async move {
                    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    let path = if method == Method::PATCH {
                        format!("/v1/users/{id}/metadata")
                    } else {
                        format!("/v1/users/{id}")
                    };
                    calls.lock().unwrap().push((method.to_string(), path, auth, body.clone()));
                    if fail {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
                    }
                    if id != "user_1" {
                        return (StatusCode::NOT_FOUND, "{}").into_response();
                    }
                    let mut metadata = stored.lock().unwrap();
                    if let Some(updates) = body.get("unsafe_metadata").and_then(Value::as_object) {
                        let object = metadata.as_object_mut().unwrap();
                        for (k, v) in updates {
                            object.insert(k.clone(), v.clone());
                        }
                    }
                    Json(json!({ "id": id, "unsafe_metadata": metadata.clone() })).into_response()
                }
            }
        };
        let app = Router::new()
            .route("/v1/users/:id", get(handler.clone()))
            .route("/v1/users/:id/metadata", patch(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), calls)
    }

    #[test]
    fn only_allowlisted_keys_are_accepted() {
        let ok = validate_updates(&json!({ "botsIntroSeenAt": "2026-10-20T12:00:00.000Z" })).unwrap();
        assert_eq!(ok.get("botsIntroSeenAt").unwrap(), "2026-10-20T12:00:00.000Z");
        for body in [
            json!({ "isAdmin": "2026-10-20T12:00:00Z" }),
            json!({ "botsIntroSeenAt": "2026-10-20T12:00:00Z", "plan": "2026-10-20T12:00:00Z" }),
            json!({}),
            json!([]),
            json!("botsIntroSeenAt"),
        ] {
            assert!(
                matches!(validate_updates(&body), Err(ApiError::BadRequest(_))),
                "{body} must be a 400"
            );
        }
    }

    #[test]
    fn values_must_be_short_rfc3339_times() {
        for good in ["2026-10-20T12:00:00Z", "2026-10-20T12:00:00.123+02:00"] {
            assert!(validate_updates(&json!({ "botsIntroSeenAt": good })).is_ok(), "{good}");
        }
        let too_long = format!("2026-10-20T12:00:00.{}Z", "1".repeat(30));
        for bad in [
            json!(null),
            json!(true),
            json!(1_760_000_000),
            json!(""),
            json!("yesterday"),
            json!("2026-10-20"),
            json!(too_long),
        ] {
            assert!(
                matches!(validate_updates(&json!({ "botsIntroSeenAt": bad })), Err(ApiError::BadRequest(_))),
                "{bad} must be a 400"
            );
        }
    }

    #[test]
    fn response_maps_only_allowlisted_string_flags() {
        let user = json!({
            "id": "user_1",
            "unsafe_metadata": { "botsIntroSeenAt": "2026-10-20T12:00:00Z", "other": "x" },
            "private_metadata": { "secret": "no" }
        });
        assert_eq!(
            flags_response(&user),
            json!({ "flags": { "botsIntroSeenAt": "2026-10-20T12:00:00Z" } })
        );
        for user in [
            json!({ "id": "user_1" }),
            json!({ "unsafe_metadata": {} }),
            json!({ "unsafe_metadata": { "botsIntroSeenAt": 5 } }),
        ] {
            assert_eq!(flags_response(&user), json!({ "flags": { "botsIntroSeenAt": null } }));
        }
    }

    #[test]
    fn merge_body_nests_updates_under_unsafe_metadata() {
        let updates = validate_updates(&json!({ "botsIntroSeenAt": "2026-10-20T12:00:00Z" })).unwrap();
        assert_eq!(
            merge_body(updates),
            json!({ "unsafe_metadata": { "botsIntroSeenAt": "2026-10-20T12:00:00Z" } })
        );
    }

    #[tokio::test]
    async fn read_then_write_round_trips_through_clerk_and_keeps_other_keys() {
        let (base, calls) = spawn_mock_clerk(json!({ "theme": "dark" }), false).await;
        let cfg = config(&base);

        let read = read_flags_with(&cfg, "user_1").await.unwrap();
        assert_eq!(read, json!({ "flags": { "botsIntroSeenAt": null } }));

        let updates = validate_updates(&json!({ "botsIntroSeenAt": "2026-10-20T12:00:00Z" })).unwrap();
        let written = write_flags_with(&cfg, "user_1", updates).await.unwrap();
        assert_eq!(written, json!({ "flags": { "botsIntroSeenAt": "2026-10-20T12:00:00Z" } }));

        let read = read_flags_with(&cfg, "user_1").await.unwrap();
        assert_eq!(read, written);

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        assert_eq!((calls[0].0.as_str(), calls[0].1.as_str()), ("GET", "/v1/users/user_1"));
        assert_eq!((calls[1].0.as_str(), calls[1].1.as_str()), ("PATCH", "/v1/users/user_1/metadata"));
        assert_eq!(
            calls[1].3,
            json!({ "unsafe_metadata": { "botsIntroSeenAt": "2026-10-20T12:00:00Z" } }),
            "only the flag is sent; Clerk merges it"
        );
        assert!(calls.iter().all(|c| c.2 == "Bearer sk_test_mock"));
    }

    #[tokio::test]
    async fn clerk_errors_map_to_404_and_503() {
        let (base, _) = spawn_mock_clerk(json!({}), false).await;
        assert!(matches!(
            read_flags_with(&config(&base), "user_missing").await,
            Err(ApiError::NotFound(_))
        ));

        let (base, _) = spawn_mock_clerk(json!({}), true).await;
        assert!(matches!(
            read_flags_with(&config(&base), "user_1").await,
            Err(ApiError::ServiceUnavailable(_))
        ));
        let updates = validate_updates(&json!({ "botsIntroSeenAt": "2026-10-20T12:00:00Z" })).unwrap();
        assert!(matches!(
            write_flags_with(&config(&base), "user_1", updates).await,
            Err(ApiError::ServiceUnavailable(_))
        ));

        // Unreachable Clerk is a 503 too.
        assert!(matches!(
            read_flags_with(&config("http://127.0.0.1:1"), "user_1").await,
            Err(ApiError::ServiceUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn missing_secret_key_fails_closed_without_calling_clerk() {
        let (base, calls) = spawn_mock_clerk(json!({}), false).await;
        let mut cfg = config(&base);
        cfg.clerk_secret_key = None;
        let error = read_flags_with(&cfg, "user_1").await.unwrap_err();
        assert!(matches!(&error, ApiError::ServiceUnavailable(m) if m.contains("CLERK_SECRET_KEY")));
        cfg.clerk_secret_key = Some(String::new());
        let updates = validate_updates(&json!({ "botsIntroSeenAt": "2026-10-20T12:00:00Z" })).unwrap();
        assert!(matches!(
            write_flags_with(&cfg, "user_1", updates).await,
            Err(ApiError::ServiceUnavailable(_))
        ));
        assert!(calls.lock().unwrap().is_empty());
    }

    /// The full app router builds (axum panics on overlapping routes) and
    /// `/api/v1/me/flags` is served by this module, not the data-plane
    /// fallback: unauthenticated GET/PUT are 401, not 404.
    #[tokio::test]
    async fn full_router_serves_me_flags_without_route_collisions() {
        use crate::routes::test_support::{test_state, MockGateway};
        use axum::{body::Body, http::Request, http::StatusCode};
        use tower::ServiceExt;

        let gateway = Arc::new(MockGateway::new(Some(MockGateway::healthy_node()), vec![]));
        let app = crate::create_router(test_state(gateway.clone()).await);
        for method in ["GET", "PUT"] {
            let request = Request::builder()
                .method(method)
                .uri("/api/v1/me/flags")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"botsIntroSeenAt":"2026-10-20T12:00:00Z"}"#))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} /api/v1/me/flags");
        }
        assert!(gateway.recorded().is_empty(), "never relayed to a runtime");
    }
}
