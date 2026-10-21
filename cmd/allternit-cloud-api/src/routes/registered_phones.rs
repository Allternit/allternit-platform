//! Registered phones (Phones 2.0): device identity for the phone stream relay.
//!
//! The account's own allternit-api (the Desktop's gateway, holding the runtime
//! device credential) upserts the phones its phone-link session serves and the
//! cloud keeps one row per (account, serial) with `last_seen_at`, so any
//! surface can list "linked but Desktop offline". Only identity + timestamps
//! are stored — never screen or input data.
//!
//! * `PUT /api/v1/runtime-devices/me/registered-phones` — body
//!   `{ phones: [{ serial, model?, name?, callBridge? }] }`; upserts each row for the
//!   device's account and touches `last_seen_at` (also the disconnect-time
//!   heartbeat: the runtime re-PUTs its phones when the session detaches).
//! * `GET /api/v1/runtime-devices/me/registered-phones` — the account's rows,
//!   `{ phones: [{ serial, model, name, lastSeenAt?, callBridge }] }`.
//!
//! `callBridge` (CONTRACT-linked-phone-voice-2026-10-18, pg 094) is the phone's
//! "bot answers calls" setting as the Desktop holds it, `{ enabled, botName }`,
//! so web and the PWA can show it read-only. Absent or null on an upsert keeps
//! what is stored (an older runtime or a heartbeat re-PUT never wipes it);
//! `null` in the list = never set.
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
    #[serde(default, rename = "callBridge")]
    pub call_bridge: Option<CallBridgeInput>,
}

/// A phone's call-bridge summary: whether a bot answers its calls, and which.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallBridgeInput {
    pub enabled: bool,
    #[serde(default)]
    pub bot_name: Option<String>,
}

/// Longest stored `callBridge.botName`.
const MAX_BOT_NAME: usize = 80;

impl CallBridgeInput {
    /// Trimmed, capped at [`MAX_BOT_NAME`] characters, empty = none.
    fn normalized(&self) -> CallBridgeInput {
        CallBridgeInput {
            enabled: self.enabled,
            bot_name: self
                .bot_name
                .as_deref()
                .map(|v| v.trim().chars().filter(|c| !c.is_control()).take(MAX_BOT_NAME).collect::<String>())
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        }
    }

    fn to_json(&self) -> Value {
        json!({ "enabled": self.enabled, "botName": self.bot_name })
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct RegisteredPhoneRow {
    pub serial: String,
    pub model: Option<String>,
    pub name: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub call_bridge: Option<Value>,
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
        call_bridge: input.call_bridge.as_ref().map(CallBridgeInput::normalized),
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
        "INSERT INTO registered_phones (account_id, serial, model, name, last_seen_at, call_bridge)
         VALUES ($1, $2, $3, $4, now(), $5)
         ON CONFLICT (account_id, serial) DO UPDATE
           SET model = EXCLUDED.model, name = EXCLUDED.name, last_seen_at = EXCLUDED.last_seen_at,
               call_bridge = COALESCE(EXCLUDED.call_bridge, registered_phones.call_bridge)",
    )
    .bind(account_id)
    .bind(phone.serial.trim())
    .bind(take(&phone.model))
    .bind(take(&phone.name))
    .bind(phone.call_bridge.as_ref().map(|b| b.normalized().to_json()))
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
        "SELECT serial, model, name, last_seen_at, call_bridge FROM registered_phones
         WHERE account_id = $1 ORDER BY created_at",
    )
    .bind(account_id)
    .fetch_all(db)
    .await
}

async fn device_account(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    Ok(device_of(state, headers).await?.0)
}

/// `(account, runtime device id)` of the credential.
async fn device_of(state: &ApiState, headers: &HeaderMap) -> Result<(String, String), ApiError> {
    let token = device_token_from_headers(headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    let device = runtime_device_for_token(&state.db, token, None).await?;
    Ok((device.user_id, device.id))
}

/// Remember which runtime serves the phone (pg 096), for the Platform API
/// relay (`platform_v1::phones`). Best effort: a schema without the column
/// keeps the upsert working.
pub async fn set_phone_runtime(db: &PgPool, account_id: &str, serial: &str, runtime_id: &str) {
    let result = sqlx::query("UPDATE registered_phones SET runtime_id = $3 WHERE account_id = $1 AND serial = $2")
        .bind(account_id)
        .bind(serial.trim())
        .bind(runtime_id)
        .execute(db)
        .await;
    if let Err(e) = result {
        tracing::debug!("registered phone runtime not stored: {e}");
    }
}

/// The runtime that most recently served any of the account's phones (pg 096).
pub async fn latest_phone_runtime(db: &PgPool, account_id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT runtime_id FROM registered_phones WHERE account_id = $1 AND runtime_id IS NOT NULL \
         ORDER BY last_seen_at DESC NULLS LAST LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

/// The runtime a registered phone was last seen on (pg 096), when known.
pub async fn phone_runtime(db: &PgPool, account_id: &str, serial: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT runtime_id FROM registered_phones WHERE account_id = $1 AND serial = $2")
        .bind(account_id)
        .bind(serial.trim())
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .flatten()
}

async fn upsert_registered_phones(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let (account_id, runtime_id) = device_of(&state, &headers).await?;
    let phones: Vec<RegisteredPhoneInput> = serde_json::from_value(
        body.get("phones")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|_| {
        ApiError::BadRequest(
            "phones must be an array of { serial, model?, name?, callBridge?: { enabled, botName? } }".into(),
        )
    })?;
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
        set_phone_runtime(&state.db, &account_id, &phone.serial, &runtime_id).await;
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
            "callBridge": r.call_bridge.clone().unwrap_or(Value::Null),
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
        // pg 094 adds call_bridge (its voice_calls part needs tables this test has no use for).
        sqlx::raw_sql("ALTER TABLE registered_phones ADD COLUMN IF NOT EXISTS call_bridge jsonb")
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
                call_bridge: None,
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
        let ok = |serial: &str| normalize(RegisteredPhoneInput { serial: serial.into(), model: None, name: None, call_bridge: None });
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
            call_bridge: None,
        })
        .unwrap();
        assert_eq!(long.model.unwrap().len(), 256, "model is capped");
        assert!(long.name.is_none(), "empty name becomes NULL");
    }

    #[test]
    fn call_bridge_shape_is_checked_and_bot_name_capped() {
        let parse = |v: Value| serde_json::from_value::<RegisteredPhoneInput>(v);
        let phone = parse(json!({ "serial": "s", "callBridge": { "enabled": true, "botName": format!("  {}  ", "B".repeat(100)) } })).unwrap();
        let bridge = normalize(phone).unwrap().call_bridge.unwrap();
        assert!(bridge.enabled);
        assert_eq!(bridge.bot_name.unwrap().chars().count(), 80);
        let off = normalize(parse(json!({ "serial": "s", "callBridge": { "enabled": false, "botName": " " } })).unwrap()).unwrap();
        assert_eq!(off.call_bridge.unwrap().to_json(), json!({ "enabled": false, "botName": null }));
        assert!(parse(json!({ "serial": "s", "callBridge": null })).unwrap().call_bridge.is_none());
        assert!(parse(json!({ "serial": "s" })).unwrap().call_bridge.is_none());
        assert!(parse(json!({ "serial": "s", "callBridge": { "botName": "x" } })).is_err(), "enabled is required");
        assert!(parse(json!({ "serial": "s", "callBridge": { "enabled": "yes" } })).is_err());
        assert!(parse(json!({ "serial": "s", "callBridge": { "enabled": true, "inputDeviceId": "x" } })).is_err(), "no device ids in the cloud");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn call_bridge_is_stored_returned_and_kept_when_absent() {
        let db = pool().await;
        let put = |bridge: Option<CallBridgeInput>| RegisteredPhoneInput { serial: "moto".into(), model: None, name: Some("Moto".into()), call_bridge: bridge };
        upsert_phone(&db, USER, &put(None)).await.unwrap();
        assert!(phones_for_account(&db, USER).await.unwrap()[0].call_bridge.is_none(), "never set reads null");
        upsert_phone(&db, USER, &put(Some(CallBridgeInput { enabled: true, bot_name: Some(" Front desk ".into()) }))).await.unwrap();
        let stored = phones_for_account(&db, USER).await.unwrap()[0].call_bridge.clone();
        assert_eq!(stored, Some(json!({ "enabled": true, "botName": "Front desk" })));
        // A heartbeat re-PUT without callBridge keeps it.
        upsert_phone(&db, USER, &put(None)).await.unwrap();
        assert_eq!(phones_for_account(&db, USER).await.unwrap()[0].call_bridge, stored);
        upsert_phone(&db, USER, &put(Some(CallBridgeInput { enabled: false, bot_name: None }))).await.unwrap();
        assert_eq!(phones_for_account(&db, USER).await.unwrap()[0].call_bridge, Some(json!({ "enabled": false, "botName": null })));
    }
}
