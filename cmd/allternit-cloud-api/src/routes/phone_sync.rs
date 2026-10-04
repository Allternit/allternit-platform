//! Phone numbers follow their runtime (cloud side).
//!
//! The runtime (Desktop or cloud computer) keeps its own copy of the numbers
//! assigned to it so an inbound call or text finds its bot. The runtime pulls
//! that list itself, authenticated as itself with its device credential, so no
//! user token is involved and a missed push heals on the next pull:
//!
//! `GET /api/v1/runtime-devices/me/phone-numbers`
//! (`Authorization: Bearer allternit_runtime_…`) →
//! `{ runtimeId, userId, numbers: [{ id, e164, botId, smsState, voiceState, messagingRef,
//!    carrier, type, portState, webhookPublicKey? }] }`
//!
//! Only live numbers (not released) assigned to that runtime are returned.
//! `webhookPublicKey` is the carrier's webhook verifying key (public by design;
//! Telnyx: `ALLTERNIT_TELNYX_PUBLIC_KEY`), which the runtime needs to re-check
//! relayed texts. Buy, port and release send the runtime a signed
//! `phone.numbers.changed` relay event ([`notify_changed`]) so the pull happens
//! at once instead of waiting for the next cycle.

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use super::phone::{NumberRow, NUMBER_COLS};
use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use super::runtime_relay::{relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

/// Where the runtime listens for the push (under `/webhooks`, which the relay allows).
pub const RUNTIME_NUMBERS_CHANGED_PATH: &str = "/webhooks/phone/numbers-changed";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/runtime-devices/me/phone-numbers", get(my_phone_numbers))
}

/// The carrier's webhook verifying key for `carrier`, when configured.
fn webhook_public_key(carrier: &str) -> Option<String> {
    match carrier {
        "telnyx" => std::env::var("ALLTERNIT_TELNYX_PUBLIC_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()),
        _ => None,
    }
}

fn number_json(n: &NumberRow) -> Value {
    let mut v = json!({
        "id": n.id, "e164": n.e164, "botId": n.bot_id, "smsState": n.sms_state, "voiceState": n.voice_state,
        "messagingRef": n.messaging_ref, "carrier": n.carrier, "type": n.kind, "portState": n.port_state,
    });
    if let Some(k) = webhook_public_key(&n.carrier) {
        v["webhookPublicKey"] = json!(k);
    }
    v
}

/// Live numbers assigned to `runtime_id` for `user_id`, oldest first.
pub async fn numbers_for_runtime(db: &PgPool, user_id: &str, runtime_id: &str) -> Result<Vec<NumberRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {NUMBER_COLS} FROM phone_numbers WHERE user_id = $1 AND runtime_id = $2 AND released_at IS NULL ORDER BY created_at"))
        .bind(user_id)
        .bind(runtime_id)
        .fetch_all(db)
        .await
}

async fn my_phone_numbers(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    // Only a device credential: a user token has no single runtime to answer for.
    let token = device_token_from_headers(&headers).ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    let device = runtime_device_for_token(&state.db, token, None).await?;
    let rows = numbers_for_runtime(&state.db, &device.user_id, &device.id).await?;
    Ok(Json(json!({ "runtimeId": device.id, "userId": device.user_id, "numbers": rows.iter().map(number_json).collect::<Vec<_>>() })).into_response())
}

/// Tell `runtime_id` its numbers changed so it pulls now. Best effort and off
/// the request path: a runtime that is asleep or offline syncs on its next boot
/// or timer pull instead.
pub fn notify_changed(state: &Arc<ApiState>, user_id: &str, runtime_id: &str) {
    let state = state.clone();
    let (user_id, runtime_id) = (user_id.to_string(), runtime_id.to_string());
    tokio::spawn(async move {
        let body = serde_json::to_vec(&json!({ "type": "phone.numbers.changed", "runtimeId": runtime_id })).unwrap_or_default();
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let sent = relay_signed_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            &user_id,
            &runtime_id,
            RelayRequest {
                method: "POST".to_string(),
                path: RUNTIME_NUMBERS_CHANGED_PATH.to_string(),
                headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
                body: STANDARD.encode(body),
                body_encoding: "base64".to_string(),
            },
            &["content-type"],
            HashMap::new(),
        )
        .await;
        match sent {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::info!(%runtime_id, status = r.status().as_u16(), "phone.numbers.changed not accepted; the runtime will sync on its next pull"),
            Err(e) => tracing::info!(%runtime_id, "phone.numbers.changed not delivered ({e}); the runtime will sync on its next pull"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{seed_runtime_device, test_pool};

    const USER: &str = "user_a";

    async fn pool() -> PgPool {
        let db = test_pool().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/024_phone_numbers.sql").replace("public.", "")).execute(&db).await.unwrap();
        sqlx::query("ALTER TABLE runtime_devices ADD COLUMN previous_credential_hash TEXT, ADD COLUMN previous_credential_expires_at TIMESTAMPTZ").execute(&db).await.unwrap();
        for id in ["rt1", "rt2"] {
            seed_runtime_device(&db, id, USER).await;
            let hash = crate::routes::runtime_pairing::sha256_hex(format!("allternit_runtime_{id}").as_bytes());
            sqlx::query("UPDATE runtime_devices SET credential_hash = $2 WHERE id = $1").bind(id).bind(hash).execute(&db).await.unwrap();
        }
        db
    }

    async fn add(db: &PgPool, id: &str, runtime: &str, e164: &str, carrier: &str) {
        sqlx::query("INSERT INTO phone_numbers (id, user_id, runtime_id, bot_id, e164, carrier, messaging_ref, sms_state) VALUES ($1, $2, $3, 'bot1', $4, $5, 'mp-1', 'active')")
            .bind(id)
            .bind(USER)
            .bind(runtime)
            .bind(e164)
            .bind(carrier)
            .execute(db)
            .await
            .unwrap();
    }

    fn bearer(id: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, format!("Bearer allternit_runtime_{id}").parse().unwrap());
        h
    }

    async fn listing(db: &PgPool, headers: &HeaderMap) -> Result<Value, ApiError> {
        // The route is a thin wrapper over this; build the same JSON without a full ApiState.
        let token = device_token_from_headers(headers).ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
        let device = runtime_device_for_token(db, token, None).await?;
        let rows = numbers_for_runtime(db, &device.user_id, &device.id).await?;
        Ok(json!({ "runtimeId": device.id, "userId": device.user_id, "numbers": rows.iter().map(number_json).collect::<Vec<_>>() }))
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_runtime_lists_exactly_its_own_live_numbers() {
        std::env::set_var("ALLTERNIT_TELNYX_PUBLIC_KEY", "cHVibGlj");
        let db = pool().await;
        add(&db, "n1", "rt1", "+16512686010", "telnyx").await;
        add(&db, "n2", "rt2", "+16512686011", "telnyx").await;
        add(&db, "n3", "rt1", "+16512686012", "telnyx").await;
        sqlx::query("UPDATE phone_numbers SET released_at = now() WHERE id = 'n3'").execute(&db).await.unwrap();
        add(&db, "n4", "rt1", "+16512686013", "other").await;
        let got = listing(&db, &bearer("rt1")).await.unwrap();
        assert_eq!((got["runtimeId"].as_str(), got["userId"].as_str()), (Some("rt1"), Some(USER)));
        let ids: Vec<&str> = got["numbers"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["n1", "n4"], "not rt2's, not the released one");
        let n1 = &got["numbers"][0];
        assert_eq!(n1["e164"], "+16512686010");
        assert_eq!((n1["botId"].as_str(), n1["smsState"].as_str(), n1["voiceState"].as_str(), n1["messagingRef"].as_str()), (Some("bot1"), Some("active"), Some("inactive"), Some("mp-1")));
        assert_eq!(n1["webhookPublicKey"], "cHVibGlj", "Telnyx numbers carry the carrier's public webhook key");
        assert!(got["numbers"][1].get("webhookPublicKey").is_none());
        std::env::remove_var("ALLTERNIT_TELNYX_PUBLIC_KEY");
        // A release shows up on the next pull.
        sqlx::query("UPDATE phone_numbers SET released_at = now() WHERE id = 'n1'").execute(&db).await.unwrap();
        assert_eq!(listing(&db, &bearer("rt1")).await.unwrap()["numbers"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn only_a_valid_device_credential_can_list() {
        let db = pool().await;
        add(&db, "n1", "rt1", "+16512686010", "telnyx").await;
        assert!(matches!(listing(&db, &HeaderMap::new()).await, Err(ApiError::Unauthorized(_))), "no credential");
        let mut user_token = HeaderMap::new();
        user_token.insert(axum::http::header::AUTHORIZATION, "Bearer eyJhbGciOi.user.jwt".parse().unwrap());
        assert!(matches!(listing(&db, &user_token).await, Err(ApiError::Unauthorized(_))), "a user token has no single runtime to answer for");
        assert!(matches!(listing(&db, &bearer("nobody")).await, Err(ApiError::Unauthorized(_))), "unknown device");
        sqlx::query("UPDATE runtime_devices SET revoked_at = now() WHERE id = 'rt1'").execute(&db).await.unwrap();
        assert!(listing(&db, &bearer("rt1")).await.is_err(), "revoked device");
    }

    #[test]
    fn the_push_path_is_one_the_relay_allows() {
        assert!(RUNTIME_NUMBERS_CHANGED_PATH.starts_with("/webhooks/"));
    }
}
