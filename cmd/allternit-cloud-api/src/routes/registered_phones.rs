//! Registered phones (Phones 2.0): device identity for the phone stream relay.
//!
//! The account's own allternit-api (the Desktop's gateway, holding the runtime
//! device credential) upserts the phones its phone-link session serves and the
//! cloud keeps one row per (account, serial) with `last_seen_at`, so any
//! surface can list "linked but Desktop offline". Only identity + timestamps
//! are stored — never screen or input data.
//!
//! * `PUT /api/v1/runtime-devices/me/registered-phones` — body
//!   `{ phones: [{ serial, model?, name? }] }`; upserts each row for the
//!   device's account and touches `last_seen_at` (also the disconnect-time
//!   heartbeat: the runtime re-PUTs its phones when the session detaches).
//! * `GET /api/v1/runtime-devices/me/registered-phones` — the account's rows,
//!   `{ phones: [{ serial, model, name, lastSeenAt? }] }`.
//!
//! Device credential only, same as `phone_sync::my_phone_numbers`: a user
//! token has no single runtime to answer for, and the account is resolved
//! from the credential (`runtime_devices.user_id`).

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::{ApiError, ApiState};

/// A phone the runtime's phone-link session serves.
#[derive(Debug, Clone, Deserialize)]
pub struct RegisteredPhoneInput {
    pub serial: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
pub struct RegisteredPhoneRow {
    pub serial: String,
    pub model: Option<String>,
    pub name: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// At most this many phones per hello/upsert — a person links a handful.
const MAX_PHONES_PER_UPSERT: usize = 64;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(
            "/api/v1/runtime-devices/me/registered-phones",
            get(list_registered_phones).put(upsert_registered_phones),
        )
}

fn normalize(input: RegisteredPhoneInput) -> Result<RegisteredPhoneInput, ApiError> {
    let serial = input.serial.trim().to_string();
    if serial.is_empty()
        || serial.len() > 128
        || serial.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::BadRequest(
            "each phone needs a non-empty serial (<=128 chars, no whitespace)".into(),
        ));
    }
    let take = |value: Option<String>, max: usize| -> Option<String> {
        value
            .map(|v| v.trim().chars().take(max).collect::<String>())
            .filter(|v| !v.is_empty())
    };
    Ok(RegisteredPhoneInput {
        serial,
        model: take(input.model, 256),
        name: take(input.name, 256),
    })
}

/// Upsert one phone row for `account_id`, refreshing `last_seen_at`.
/// Identity fields are normalized here (not just in the handler) so every
/// write path stores trimmed, capped values.
pub async fn upsert_phone(
    db: &PgPool,
    account_id: &str,
    phone: &RegisteredPhoneInput,
) -> Result<(), sqlx::Error> {
    let take = |value: &Option<String>| -> Option<String> {
        value
            .as_deref()
            .map(|v| v.trim().chars().take(256).collect::<String>())
            .filter(|v| !v.is_empty())
    };
    sqlx::query(
        "INSERT INTO registered_phones (account_id, serial, model, name, last_seen_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (account_id, serial) DO UPDATE
           SET model = EXCLUDED.model, name = EXCLUDED.name, last_seen_at = EXCLUDED.last_seen_at",
    )
    .bind(account_id)
    .bind(phone.serial.trim())
    .bind(take(&phone.model))
    .bind(take(&phone.name))
    .execute(db)
    .await?;
    Ok(())
}

/// The account's registered phones, oldest first.
pub async fn phones_for_account(
    db: &PgPool,
    account_id: &str,
) -> Result<Vec<RegisteredPhoneRow>, sqlx::Error> {
    sqlx::query_as::<_, RegisteredPhoneRow>(
        "SELECT serial, model, name, last_seen_at FROM registered_phones
         WHERE account_id = $1 ORDER BY created_at",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

async fn device_account(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = device_token_from_headers(headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    let device = runtime_device_for_token(&state.db, token, None).await?;
    Ok(device.user_id)
}

async fn upsert_registered_phones(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let account_id = device_account(&state, &headers).await?;
    let phones: Vec<RegisteredPhoneInput> = serde_json::from_value(
        body.get("phones")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| ApiError::BadRequest("phones must be an array of { serial, model?, name? }".into()))?;
    if phones.is_empty() {
        return Err(ApiError::BadRequest("phones must not be empty".into()));
    }
    if phones.len() > MAX_PHONES_PER_UPSERT {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_PHONES_PER_UPSERT} phones per upsert"
        )));
    }
    for phone in &phones {
        let phone = normalize(phone.clone())?;
        upsert_phone(&state.db, &account_id, &phone).await?;
    }
    Ok(Json(json!({ "ok": true, "upserted": phones.len() })).into_response())
}

async fn list_registered_phones(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let account_id = device_account(&state, &headers).await?;
    let rows = phones_for_account(&state.db, &account_id).await?;
    Ok(Json(json!({
        "phones": rows.iter().map(|r| json!({
            "serial": r.serial,
            "model": r.model,
            "name": r.name,
            "lastSeenAt": r.last_seen_at.map(|t| t.to_rfc3339()),
        })).collect::<Vec<_>>()
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::{seed_runtime_device, test_pool};

    const USER: &str = "user_a";

    async fn pool() -> PgPool {
        let db = test_pool().await;
        sqlx::raw_sql(&include_str!("../../migrations_pg/093_registered_phones.sql").replace("public.", ""))
            .execute(&db)
            .await
            .unwrap();
        for id in ["rt1", "rt2"] {
            seed_runtime_device(&db, id, USER).await;
            let hash = crate::routes::runtime_pairing::sha256_hex(format!("allternit_runtime_{id}").as_bytes());
            sqlx::query("UPDATE runtime_devices SET credential_hash = $2 WHERE id = $1")
                .bind(id)
                .bind(hash)
                .execute(&db)
                .await
                .unwrap();
        }
        db
    }

    async fn upsert(db: &PgPool, serial: &str, model: Option<&str>, name: Option<&str>) {
        upsert_phone(
            db,
            USER,
            &RegisteredPhoneInput {
                serial: serial.to_string(),
                model: model.map(str::to_string),
                name: name.map(str::to_string),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn upsert_inserts_then_updates_and_touches_last_seen() {
        let db = pool().await;
        upsert(&db, "R5CRA1A2LBA", Some("Pixel 9"), Some("Joe's Pixel")).await;
        let rows = phones_for_account(&db, USER).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].serial, "R5CRA1A2LBA");
        assert_eq!(rows[0].model.as_deref(), Some("Pixel 9"));
        assert!(rows[0].last_seen_at.is_some());

        // Same (account, serial): one row, identity refreshed.
        upsert(&db, "R5CRA1A2LBA", Some("Pixel 9 Pro"), None).await;
        let rows = phones_for_account(&db, USER).await.unwrap();
        assert_eq!(rows.len(), 1, "upsert must not duplicate the row");
        assert_eq!(rows[0].model.as_deref(), Some("Pixel 9 Pro"));

        // Another serial: a second row.
        upsert(&db, "emulator-5554", None, Some(" Emulator ")).await;
        let rows = phones_for_account(&db, USER).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].serial, "emulator-5554");
        assert_eq!(rows[1].name.as_deref(), Some("Emulator"), "name is trimmed");
        assert!(rows[1].model.is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn phones_are_scoped_per_account() {
        let db = pool().await;
        upsert(&db, "serial-x", None, None).await;
        sqlx::query("INSERT INTO registered_phones (account_id, serial) VALUES ('user_b', 'serial-y')")
            .execute(&db)
            .await
            .unwrap();
        let mine: Vec<String> = phones_for_account(&db, USER)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.serial)
            .collect();
        assert_eq!(mine, vec!["serial-x".to_string()], "user_b's phone stays invisible");
    }

    #[test]
    fn serial_validation() {
        let ok = |serial: &str| normalize(RegisteredPhoneInput { serial: serial.into(), model: None, name: None });
        assert!(ok("R5CRA1A2LBA").is_ok());
        assert!(ok("emulator-5554").is_ok());
        assert!(ok("5200f6d3e687c8a9").is_ok());
        assert!(ok("  spaced-serial  ").is_ok(), "trimmed");
        assert!(ok("").is_err());
        assert!(ok("   ").is_err());
        assert!(ok("has space").is_err());
        assert!(ok(&"x".repeat(129)).is_err());
        assert!(ok(&"x".repeat(128)).is_ok());
        let long = normalize(RegisteredPhoneInput {
            serial: "s".into(),
            model: Some(("m".repeat(300))),
            name: Some("".into()),
        })
        .unwrap();
        assert_eq!(long.model.unwrap().len(), 256, "model is capped");
        assert!(long.name.is_none(), "empty name becomes NULL");
    }
}
