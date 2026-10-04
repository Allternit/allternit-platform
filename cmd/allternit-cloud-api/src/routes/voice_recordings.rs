//! Call recording playback.
//!
//! * `GET /api/v1/voice/calls/{callId}/recording` (Clerk user): a 10-minute
//!   presigned GET for the call's audio in the private R2 bucket
//!   (`allternit-call-recordings`, 90-day auto-delete).
//!
//! The voice service records with LiveKit Egress to `calls/<callId>.ogg` and
//! reports the key as `recordingRef` in `call.ended`. Voicemail left by the
//! bot is part of the same room recording, so it plays from this route too.
//!
//! Answers: 200 `{url, expiresAt, durationSec?, contentType}`; 403 not the
//! caller's own call; 404 no such call or no recording; 410 the recording has
//! expired (the object is gone); 503 R2 is not configured here.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

use crate::{
    auth,
    error::ApiError,
    services::r2::{ObjectStore, R2Client, R2Error},
    ApiState,
};

pub const RECORDINGS_BUCKET: &str = "allternit-call-recordings";
pub const URL_TTL: Duration = Duration::from_secs(600);
const RETENTION_DAYS: i64 = 90;
const CONTENT_TYPE: &str = "audio/ogg";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/voice/calls/:call_id/recording", get(get_recording))
}

fn coded(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "code": code, "message": message }))).into_response()
}

async fn get_recording(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(call_id): Path<String>) -> Result<Response, ApiError> {
    let user_id = auth::resolve_user_id(&state.db, &headers).await?;
    let r2 = R2Client::from_env();
    let store = r2.as_ref().ok().map(|c| c as &dyn ObjectStore);
    recording_for(&state, &user_id, &call_id, store, Utc::now()).await
}

/// Same convention as the voice service's `recording::object_key`.
fn default_key(call_id: &str) -> String {
    format!("calls/{call_id}.ogg")
}

fn safe_key(key: &str) -> bool {
    key.starts_with("calls/") && !key.contains("..") && key.len() < 200
}

pub(crate) async fn recording_for(state: &ApiState, user_id: &str, call_id: &str, store: Option<&dyn ObjectStore>, now: DateTime<Utc>) -> Result<Response, ApiError> {
    let call: Option<(String, DateTime<Utc>)> = sqlx::query_as("SELECT user_id, started_at FROM voice_calls WHERE call_id = $1")
        .bind(call_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((owner, started_at)) = call else {
        return Ok(coded(StatusCode::NOT_FOUND, "not-found", "No such call."));
    };
    if owner != user_id {
        return Ok(coded(StatusCode::FORBIDDEN, "forbidden", "This call belongs to another account."));
    }

    // The call.ended payload has the key and the duration. Events are pruned a
    // week after delivery, so an older call falls back to the fixed key.
    let ended: Option<Value> = sqlx::query_scalar("SELECT payload FROM voice_call_events WHERE call_id = $1 AND event_type = 'call.ended' ORDER BY n DESC LIMIT 1")
        .bind(call_id)
        .fetch_optional(&state.db)
        .await?;
    let duration = ended.as_ref().and_then(|p| p.get("durationSec")).and_then(Value::as_u64);
    let key = match &ended {
        Some(p) => match p.get("recordingRef").and_then(Value::as_str) {
            Some(k) => k.to_string(),
            None => return Ok(coded(StatusCode::NOT_FOUND, "no-recording", "This call was not recorded.")),
        },
        None => default_key(call_id),
    };
    if !safe_key(&key) {
        return Ok(coded(StatusCode::NOT_FOUND, "no-recording", "This call was not recorded."));
    }
    let expired = || coded(StatusCode::GONE, "recording-expired", "This recording has been deleted (recordings are kept for 90 days).");
    let Some(store) = store else {
        return Ok(coded(StatusCode::SERVICE_UNAVAILABLE, "recordings-unavailable", "Call recordings are not available right now."));
    };
    match store.head(RECORDINGS_BUCKET, &key).await {
        Ok(Some(_)) => {}
        Ok(None) if ended.is_some() => return Ok(expired()),
        Ok(None) if now - started_at > ChronoDuration::days(RETENTION_DAYS) => return Ok(expired()),
        Ok(None) => return Ok(coded(StatusCode::NOT_FOUND, "no-recording", "This call was not recorded.")),
        Err(R2Error::Unavailable) => return Ok(coded(StatusCode::SERVICE_UNAVAILABLE, "recordings-unavailable", "Call recordings are not available right now.")),
        Err(e) => {
            tracing::warn!(call_id, "recording lookup failed: {e}");
            return Ok(coded(StatusCode::BAD_GATEWAY, "recordings-error", "Could not reach the recording store. Try again."));
        }
    }
    let url = match store.presign_get(RECORDINGS_BUCKET, &key, URL_TTL) {
        Ok(u) => u,
        Err(_) => return Ok(coded(StatusCode::SERVICE_UNAVAILABLE, "recordings-unavailable", "Call recordings are not available right now.")),
    };
    let expires_at = now + ChronoDuration::seconds(URL_TTL.as_secs() as i64);
    let mut body = json!({ "url": url, "expiresAt": expires_at, "contentType": CONTENT_TYPE });
    if let Some(d) = duration {
        body["durationSec"] = json!(d);
    }
    Ok((StatusCode::OK, [(axum::http::header::CACHE_CONTROL, "no-store")], Json(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};
    use async_trait::async_trait;
    use axum::body::to_bytes;
    use serial_test::serial;

    struct Fake(Option<u64>);
    #[async_trait]
    impl ObjectStore for Fake {
        fn presign_get(&self, bucket: &str, key: &str, _ttl: Duration) -> Result<String, R2Error> {
            Ok(format!("https://r2.test/{bucket}/{key}?sig=x"))
        }
        async fn head(&self, _b: &str, _k: &str) -> Result<Option<u64>, R2Error> {
            Ok(self.0)
        }
    }

    async fn state() -> Arc<ApiState> {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        for sql in [
            "CREATE TABLE voice_calls (call_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, started_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
            "CREATE TABLE voice_call_events (id BIGSERIAL PRIMARY KEY, call_id TEXT NOT NULL REFERENCES voice_calls(call_id) ON DELETE CASCADE, event_type TEXT NOT NULL, n INTEGER NOT NULL, payload JSONB NOT NULL)",
        ] {
            sqlx::query(sql).execute(&state.db).await.unwrap();
        }
        state
    }

    async fn call(state: &ApiState, id: &str, user: &str, ended: Option<Value>, age_days: i64) {
        sqlx::query("INSERT INTO voice_calls (call_id, user_id, started_at) VALUES ($1, $2, now() - make_interval(days => $3::int))")
            .bind(id).bind(user).bind(age_days as i32).execute(&state.db).await.unwrap();
        if let Some(p) = ended {
            sqlx::query("INSERT INTO voice_call_events (call_id, event_type, n, payload) VALUES ($1, 'call.ended', 1, $2)")
                .bind(id).bind(p).execute(&state.db).await.unwrap();
        }
    }

    async fn run(state: &ApiState, user: &str, id: &str, store: Option<&dyn ObjectStore>) -> (StatusCode, Value) {
        let r = recording_for(state, user, id, store, Utc::now()).await.unwrap();
        let s = r.status();
        (s, serde_json::from_slice(&to_bytes(r.into_body(), 1 << 20).await.unwrap()).unwrap_or(json!(null)))
    }

    #[tokio::test]
    #[serial]
    async fn owner_gets_a_url_and_others_get_403() {
        let state = state().await;
        call(&state, "c1", "u1", Some(json!({"durationSec": 42, "reason": "caller_hangup", "recordingRef": "calls/c1.ogg"})), 0).await;
        let store = Fake(Some(1000));
        let (s, b) = run(&state, "u1", "c1", Some(&store)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["url"], "https://r2.test/allternit-call-recordings/calls/c1.ogg?sig=x");
        assert_eq!((b["durationSec"].as_u64(), b["contentType"].as_str()), (Some(42), Some("audio/ogg")));
        assert!(b["expiresAt"].is_string());
        assert_eq!(run(&state, "u2", "c1", Some(&store)).await.0, StatusCode::FORBIDDEN);
        assert_eq!(run(&state, "u1", "nope", Some(&store)).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    #[serial]
    async fn no_recording_is_404_and_a_missing_object_is_410() {
        let state = state().await;
        call(&state, "c2", "u1", Some(json!({"durationSec": 5, "reason": "caller_hangup"})), 0).await;
        assert_eq!(run(&state, "u1", "c2", Some(&Fake(Some(1))))  .await.1["code"], "no-recording");
        call(&state, "c3", "u1", Some(json!({"durationSec": 5, "recordingRef": "calls/c3.ogg"})), 100).await;
        let (s, b) = run(&state, "u1", "c3", Some(&Fake(None))).await;
        assert_eq!((s, b["code"].as_str()), (StatusCode::GONE, Some("recording-expired")));
        // Events pruned: fall back to the fixed key. Recent + missing = never recorded; old + missing = expired.
        call(&state, "c4", "u1", None, 1).await;
        assert_eq!(run(&state, "u1", "c4", Some(&Fake(None))).await.0, StatusCode::NOT_FOUND);
        assert_eq!(run(&state, "u1", "c4", Some(&Fake(Some(9)))).await.0, StatusCode::OK);
        call(&state, "c5", "u1", None, 120).await;
        assert_eq!(run(&state, "u1", "c5", Some(&Fake(None))).await.0, StatusCode::GONE);
    }

    #[tokio::test]
    #[serial]
    async fn unconfigured_r2_is_503_and_bad_keys_are_refused() {
        let state = state().await;
        call(&state, "c6", "u1", Some(json!({"recordingRef": "calls/c6.ogg"})), 0).await;
        assert_eq!(run(&state, "u1", "c6", None).await.0, StatusCode::SERVICE_UNAVAILABLE);
        call(&state, "c7", "u1", Some(json!({"recordingRef": "../secrets/x"})), 0).await;
        assert_eq!(run(&state, "u1", "c7", Some(&Fake(Some(1)))).await.0, StatusCode::NOT_FOUND);
    }
}
