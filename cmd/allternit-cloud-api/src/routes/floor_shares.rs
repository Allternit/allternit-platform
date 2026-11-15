//! Office view live link (public floors).
//!
//! * `POST /api/v1/runtime/floor-share`: runtime -> cloud, relay-signed exactly
//!   like `/api/v1/runtime/events` ([`super::runtime_events::authenticate_runtime`]).
//!   Body `{"enabled": true, "projection": {...}}` upserts the owner's floor and
//!   answers `{"enabled": true, "token"}` (a new token when the share was off or
//!   missing); `{"enabled": false}` turns it off and clears the projection.
//! * `GET /api/v2/public/floors/:token`: no auth, rate limited with the other
//!   public routes. 404 when the token is unknown or the share is off. No-store,
//!   noindex, CORS for the 3D facility only.
//!
//! Privacy is enforced at write time: [`sanitize_projection`] rebuilds the floor
//! from an allowlist (`PublicFloor` in allternit-ai `src/lib/office/floor-types.ts`),
//! so task text, questions, titles and screens cannot be stored even if a runtime sends them.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use serde_json::{json, Value};

use super::runtime_events;
use crate::ApiState;

pub const PUSH_PATH: &str = "/api/v1/runtime/floor-share";
pub const FACILITY_ORIGIN: &str = "https://3dfacility.allternit.com";
const MAX_BODY: usize = 256 * 1024;
const MAX_BOTS: usize = 200;
const STATUSES: [&str; 4] = ["working", "waiting", "talking", "idle"];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route(PUSH_PATH, post(push)).route("/api/v2/public/floors/:token", get(public_floor))
}

fn clip(v: &Value, max: usize) -> Option<String> {
    v.as_str().map(|s| s.trim().chars().take(max).collect::<String>()).filter(|s| !s.is_empty())
}

fn safe_sheet(s: &str) -> bool {
    s.len() <= 600 && s.strip_prefix("https://").and_then(|r| r.split('/').next()).is_some_and(|h| h == "allternit.com" || h.ends_with(".allternit.com"))
}

fn safe_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or("");
    matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

/// Rebuild a `PublicFloor` from an allowlist. Anything not listed is dropped; a bot without a valid id, name or status is dropped.
pub fn sanitize_projection(input: &Value, now_ms: i64) -> Value {
    let bots: Vec<Value> = input["bots"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let id = clip(&b["id"], 128)?;
                    let name = clip(&b["name"], 80)?;
                    let status = b["status"].as_str().filter(|s| STATUSES.contains(s))?;
                    let mut out = json!({
                        "id": id,
                        "name": name,
                        "role": clip(&b["role"], 120).unwrap_or_default(),
                        "color": b["color"].as_str().filter(|c| safe_color(c)).unwrap_or("#9CA3AF"),
                        "status": status,
                        "updatedAt": b["updatedAt"].as_i64().filter(|n| *n >= 0).unwrap_or(0),
                    });
                    if let Some(sheet) = b["avatarSheet"].as_str().filter(|s| safe_sheet(s)) {
                        out["avatarSheet"] = json!(sheet);
                    }
                    Some(out)
                })
                .take(MAX_BOTS)
                .collect()
        })
        .unwrap_or_default();
    json!({ "updatedAt": now_ms, "bots": bots })
}

fn new_token() -> String {
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

async fn push(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    if body.len() > MAX_BODY {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "too_large");
    }
    let (user_id, _runtime) = match runtime_events::authenticate_runtime(&state.db, &headers, "POST", PUSH_PATH, &body, Utc::now().timestamp()).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Ok(parsed) = serde_json::from_slice::<Value>(&body) else { return json_error(StatusCode::BAD_REQUEST, "bad_body") };
    let Some(enabled) = parsed["enabled"].as_bool() else { return json_error(StatusCode::BAD_REQUEST, "bad_body") };

    let res = if enabled {
        let projection = sanitize_projection(&parsed["projection"], Utc::now().timestamp_millis());
        sqlx::query_scalar::<_, String>(
            "INSERT INTO floor_shares (token, user_id, enabled, projection, updated_at) VALUES ($1, $2, TRUE, $3, now()) \
             ON CONFLICT (user_id) DO UPDATE SET enabled = TRUE, projection = EXCLUDED.projection, updated_at = now(), \
             token = CASE WHEN floor_shares.enabled THEN floor_shares.token ELSE EXCLUDED.token END \
             RETURNING token",
        )
        .bind(new_token())
        .bind(&user_id)
        .bind(projection)
        .fetch_one(&state.db)
        .await
        .map(Some)
    } else {
        sqlx::query("UPDATE floor_shares SET enabled = FALSE, projection = '{}'::jsonb, updated_at = now() WHERE user_id = $1")
            .bind(&user_id)
            .execute(&state.db)
            .await
            .map(|_| None)
    };
    match res {
        Ok(token) => Json(json!({ "enabled": enabled, "token": token })).into_response(),
        Err(e) => {
            tracing::error!("floor share push: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    }
}

async fn public_floor(State(state): State<Arc<ApiState>>, Path(token): Path<String>) -> Response {
    let found = if (16..=128).contains(&token.len()) {
        sqlx::query_scalar::<_, Value>("SELECT projection FROM floor_shares WHERE token = $1 AND enabled")
            .bind(&token)
            .fetch_optional(&state.db)
            .await
    } else {
        Ok(None)
    };
    let mut response = match found {
        Ok(Some(floor)) => Json(floor).into_response(),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "not_found"),
        Err(e) => {
            tracing::error!("public floor: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "internal")
        }
    };
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static(FACILITY_ORIGIN));
    h.insert(header::VARY, HeaderValue::from_static("origin"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_drops_everything_outside_the_allowlist() {
        let input = json!({
            "task": "Pay Acme invoice 4411",
            "bots": [
                { "id": "a", "name": "Ada", "role": "Support", "color": "#112233", "status": "waiting", "updatedAt": 7,
                  "avatarSheet": "https://ai.allternit.com/s/a.png",
                  "task": "Refund Jo", "question": "Send it?", "threadTitle": "Acme", "lastText": "secret", "desk": { "kind": "screen", "image": "data:image/webp;base64,AAAA" } },
                { "id": "b", "name": "Bo", "status": "dreaming" },
                { "id": "c", "name": "Cy", "status": "idle", "color": "red; background:url(x)", "avatarSheet": "https://evil.example/x.png" },
            ],
        });
        let out = sanitize_projection(&input, 99);
        let text = out.to_string();
        for leaked in ["Acme", "Refund", "Send it", "secret", "desk", "task", "question"] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
        assert_eq!(out["updatedAt"], 99);
        let bots = out["bots"].as_array().unwrap();
        assert_eq!(bots.len(), 2, "a bot with an unknown status is dropped");
        assert_eq!(bots[0]["avatarSheet"], "https://ai.allternit.com/s/a.png");
        assert_eq!(bots[0]["status"], "waiting");
        assert_eq!(bots[1]["color"], "#9CA3AF");
        assert!(bots[1].get("avatarSheet").is_none());
        let mut keys: Vec<_> = bots[0].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["avatarSheet", "color", "id", "name", "role", "status", "updatedAt"]);
    }

    #[test]
    fn tokens_carry_at_least_128_bits() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 24);
    }
}
