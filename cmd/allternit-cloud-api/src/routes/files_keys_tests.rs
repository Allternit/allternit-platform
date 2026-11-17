//! Files key sharing (SPEC section 13). DB-backed like the other Files route
//! tests (`routes::test_support`), with migration 145 applied to the test
//! schema. These pin the rules that keep the key with the user's own
//! devices: a grant answers one live request with its code, only a key holder
//! grants, a removed device forces a rotation, and a rotation reaches every
//! device left.

use base64::Engine as _;
use tower::ServiceExt;

use super::*;
use crate::routes::test_support::{authed_request, seed_runtime_device, test_state, MockGateway, DEV_USER};

// ---------------------------------------------------------------- pure ----

#[test]
fn the_confirm_code_is_derived_from_the_key_exactly_like_the_engine() {
    // contracts/files-drive/vectors/keys-v1.json, made with @noble/hashes.
    let v: serde_json::Value = serde_json::from_str(include_str!("../../../../contracts/files-drive/vectors/keys-v1.json")).unwrap();
    let public = parse_pub(v["confirm_code"]["device_pub"].as_str().unwrap()).unwrap();
    assert_eq!(confirm_code(&public), v["confirm_code"]["code"].as_str().unwrap());
    let e = parse_pub(v["envelope"]["device_pub"].as_str().unwrap()).unwrap();
    assert_eq!(confirm_code(&e), v["envelope"]["confirm_code"].as_str().unwrap());
}

fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

fn env(key_id: &str, device_id: &str) -> KeyEnvelope {
    KeyEnvelope {
        key_id: key_id.into(),
        device_id: device_id.into(),
        wrap_alg: WRAP_ALG.into(),
        ephemeral_pub: b64(&[9u8; 32]),
        nonce: b64(&[1u8; 24]),
        wrapped_key: b64(&[2u8; 48]),
        created_at: "2026-11-15T12:00:00Z".into(),
    }
}

#[test]
fn envelope_shape_refuses_what_could_never_open_or_leaks_the_key() {
    assert!(envelope_shape(&env("fk_1", "dev_phone1")).is_ok());
    assert!(envelope_shape(&env("fk_1", "rt_0123456789abcdef")).is_ok());
    let mut zero = env("fk_1", "dev_phone1");
    zero.ephemeral_pub = b64(&[0u8; 32]);
    assert!(envelope_shape(&zero).is_err(), "an all-zero ephemeral key makes the shared secret zero");
    let mut raw_key = env("fk_1", "dev_phone1");
    raw_key.wrapped_key = b64(&[2u8; 32]);
    assert!(envelope_shape(&raw_key).is_err(), "32 bytes is a bare key, not a sealed one");
    let mut alg = env("fk_1", "dev_phone1");
    alg.wrap_alg = "none".into();
    assert!(envelope_shape(&alg).is_err());
    assert!(envelope_shape(&env("k1", "dev_phone1")).is_err());
    assert!(envelope_shape(&env("fk_1", "pc_abc12345")).is_err());
    assert!(envelope_shape(&env("fk_1", "dev_../../x")).is_err());
}

#[test]
fn a_rotation_must_reach_every_device_exactly_once() {
    let devices: BTreeSet<String> = ["dev_phone1", "rt_mac00001"].iter().map(|s| s.to_string()).collect();
    assert!(rotation_covers(&[env("fk_2", "dev_phone1"), env("fk_2", "rt_mac00001")], &devices));
    assert!(!rotation_covers(&[env("fk_2", "dev_phone1")], &devices), "a device left out loses its files");
    assert!(!rotation_covers(&[env("fk_2", "dev_phone1"), env("fk_2", "rt_mac00001"), env("fk_2", "dev_gone01")], &devices), "the removed device must not get the new key");
    assert!(!rotation_covers(&[env("fk_2", "dev_phone1"), env("fk_2", "dev_phone1"), env("fk_2", "rt_mac00001")], &devices));
}

// ------------------------------------------------------------- with a db ----

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let sql = include_str!("../../migrations_pg/146_files_keys.sql").replace("public.", "");
    sqlx::raw_sql(&sql).execute(&state.db).await.expect("migration 145");
    // 147 before the test audit_log exists: its audit_log index is skipped here.
    let decline_sql = include_str!("../../migrations_pg/147_files_keys_decline.sql").replace("public.", "");
    sqlx::raw_sql(&decline_sql).execute(&state.db).await.expect("migration 147");
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS audit_log (id TEXT PRIMARY KEY, action TEXT, resource_type TEXT, resource_id TEXT, \
         user_id TEXT, user_email TEXT, details TEXT, success BOOLEAN, organization_id TEXT, project_id TEXT, ip_address TEXT)",
    )
    .execute(&state.db)
    .await
    .unwrap();
    state
}

async fn call(state: &Arc<ApiState>, method: &str, path: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let text = if body.is_null() { String::new() } else { body.to_string() };
    let res = routes().with_state(state.clone()).oneshot(authed_request(method, path, &text)).await.unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

/// A device key and its derived code.
fn device_key(seed: u8) -> (String, String) {
    let mut k = [seed; 32];
    k[0] = 1;
    (b64(&k), confirm_code(&k))
}

async fn ask(state: &Arc<ApiState>, device_id: &str, seed: u8) -> (StatusCode, serde_json::Value, String) {
    let (public, code) = device_key(seed);
    let (s, b) = call(state, "POST", "/api/v1/files-keys/requests", serde_json::json!({ "device_id": device_id, "device_pub": public, "device_name": device_id, "confirm_code": code })).await;
    (s, b, code)
}

async fn give(state: &Arc<ApiState>, key_id: &str, to: &str, by: &str, code: &str) -> (StatusCode, serde_json::Value) {
    call(state, "POST", "/api/v1/files-keys/grants", serde_json::json!({ "envelope": env(key_id, to), "approved_by_device_id": by, "confirm_code": code })).await
}

async fn audit_rows(state: &Arc<ApiState>, action: &str, success: bool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = $1 AND success = $2")
        .bind(action)
        .bind(success)
        .fetch_one(&state.db)
        .await
        .unwrap()
}

fn on() -> std::sync::MutexGuard<'static, ()> {
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    let g = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
    g
}

#[tokio::test]
#[serial_test::serial]
async fn routes_are_off_until_migration_145_is_applied() {
    let _g = on();
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    let (status, body) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "feature_disabled");
}

#[tokio::test]
#[serial_test::serial]
async fn a_new_device_gets_the_key_only_through_a_holder_approving_its_code() {
    let _g = on();
    let state = setup().await;

    // No key yet: the first device creates it by granting its own request.
    let (s, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(s, StatusCode::OK);
    assert!(info.get("key_id").is_none() && info["devices"].as_array().unwrap().is_empty(), "{info}");
    let (s, _, phone_code) = ask(&state, "dev_phone01", 3).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(give(&state, "fk_first", "dev_phone01", "dev_phone01", &phone_code).await.0, StatusCode::NO_CONTENT);
    assert_eq!(audit_rows(&state, "files_key.create", true).await, 1);

    // A second device asks. Its request shows its own code.
    let (s, pending, laptop_code) = ask(&state, "dev_laptop1", 4).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{pending}");
    assert_eq!(pending["confirm_code"], laptop_code.as_str());

    // Wrong code: refused and audited, the request stays.
    let wrong = if laptop_code == "000000" { "000001" } else { "000000" };
    let (s, b) = give(&state, "fk_first", "dev_laptop1", "dev_phone01", wrong).await;
    assert_eq!((s, b["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{b}");
    assert_eq!(audit_rows(&state, "files_key.grant", false).await, 1);
    // A device without the key can't approve, and nobody approves themselves.
    assert_eq!(give(&state, "fk_first", "dev_laptop1", "dev_stranger", &laptop_code).await.0, StatusCode::FORBIDDEN);
    assert_eq!(give(&state, "fk_first", "dev_laptop1", "dev_laptop1", &laptop_code).await.0, StatusCode::FORBIDDEN);
    // A grant for any key but the current one is refused.
    assert_eq!(give(&state, "fk_other", "dev_laptop1", "dev_phone01", &laptop_code).await.0, StatusCode::CONFLICT);

    // The holder approves with the matching code.
    assert_eq!(give(&state, "fk_first", "dev_laptop1", "dev_phone01", &laptop_code).await.0, StatusCode::NO_CONTENT);
    let (s, e) = call(&state, "GET", "/api/v1/files-keys/envelopes/dev_laptop1", serde_json::Value::Null).await;
    assert_eq!((s, e["key_id"].as_str(), e["device_id"].as_str()), (StatusCode::OK, Some("fk_first"), Some("dev_laptop1")));
    // The request is used up: the same grant again finds nothing waiting.
    assert_eq!(give(&state, "fk_first", "dev_laptop1", "dev_phone01", &laptop_code).await.0, StatusCode::NOT_FOUND);

    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(info["key_id"], "fk_first");
    assert_eq!(info["device_ids"], serde_json::json!(["dev_phone01", "dev_laptop1"]));
    assert!(info["pending_requests"].as_array().unwrap().is_empty());
    assert_eq!(info["rotation_needed"], false);
}

#[tokio::test]
#[serial_test::serial]
async fn requests_are_checked_expire_and_are_rate_limited() {
    let _g = on();
    let state = setup().await;

    // A code that is not the one derived from the key is refused.
    let (public, code) = device_key(5);
    let other = if code == "123456" { "654321" } else { "123456" };
    let (s, _) = call(&state, "POST", "/api/v1/files-keys/requests", serde_json::json!({ "device_id": "dev_tablet1", "device_pub": public, "device_name": "Tablet", "confirm_code": other })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A computer id that is not one of the user's runtimes is refused.
    seed_runtime_device(&state.db, "rt_theirs0001", "user_b").await;
    assert_eq!(ask(&state, "rt_theirs0001", 6).await.0, StatusCode::NOT_FOUND);

    // Two waiting devices showing one code could be mixed up: the second is refused.
    let (s, _, c) = ask(&state, "dev_tablet1", 5).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    sqlx::query(
        "INSERT INTO files_key_requests (request_id, user_id, device_id, device_pub, confirm_code, expires_at) \
         VALUES ('fkr_twin', $1, 'dev_tablet2', 'another-key', $2, NOW() + INTERVAL '5 minutes')",
    )
    .bind(DEV_USER)
    .bind(device_key(7).1)
    .execute(&state.db)
    .await
    .unwrap();
    let (s, b, _) = ask(&state, "dev_tablet3", 7).await;
    assert_eq!((s, b["code"].as_str()), (StatusCode::CONFLICT, Some("already_exists")), "{b}");

    // An expired request can't be granted.
    sqlx::query("UPDATE files_key_requests SET expires_at = NOW() - INTERVAL '1 second' WHERE device_id = 'dev_tablet1'")
        .execute(&state.db)
        .await
        .unwrap();
    assert_eq!(give(&state, "fk_first", "dev_tablet1", "dev_tablet1", &c).await.0, StatusCode::NOT_FOUND);

    // 20 requests an hour per user.
    for i in 0..30u8 {
        let (s, b, _) = ask(&state, &format!("dev_flood{i:03}"), 100 + i).await;
        if s == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(b["code"], "rate_limited");
            return;
        }
    }
    panic!("requests were never rate limited");
}

#[tokio::test]
#[serial_test::serial]
async fn removing_a_device_marks_rotation_and_the_rotation_reaches_everyone_left() {
    let _g = on();
    let state = setup().await;
    seed_runtime_device(&state.db, "rt_mac0000001", DEV_USER).await;
    let (_, _, a) = ask(&state, "dev_phone01", 3).await;
    give(&state, "fk_first", "dev_phone01", "dev_phone01", &a).await;
    let (_, _, b) = ask(&state, "dev_laptop1", 4).await;
    give(&state, "fk_first", "dev_laptop1", "dev_phone01", &b).await;
    let (_, _, c) = ask(&state, "rt_mac0000001", 8).await;
    assert_eq!(give(&state, "fk_first", "rt_mac0000001", "dev_phone01", &c).await.0, StatusCode::NO_CONTENT);

    // The laptop is lost.
    assert_eq!(call(&state, "DELETE", "/api/v1/files-keys/devices/dev_laptop1", serde_json::Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(info["rotation_needed"], true, "{info}");
    assert_eq!(call(&state, "GET", "/api/v1/files-keys/envelopes/dev_laptop1", serde_json::Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(audit_rows(&state, "files_key.device_removed", true).await, 1);

    let rotation = |envs: Vec<KeyEnvelope>| serde_json::json!({ "key_id": "fk_second", "rotated_from": "fk_first", "rotated_by_device_id": "dev_phone01", "envelopes": envs });
    // Sealing the new key to the removed laptop is refused; so is leaving the Mac out.
    let (s, _) = call(&state, "POST", "/api/v1/files-keys/rotations", rotation(vec![env("fk_second", "dev_phone01"), env("fk_second", "rt_mac0000001"), env("fk_second", "dev_laptop1")])).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = call(&state, "POST", "/api/v1/files-keys/rotations", rotation(vec![env("fk_second", "dev_phone01")])).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, b) = call(&state, "POST", "/api/v1/files-keys/rotations", rotation(vec![env("fk_second", "dev_phone01"), env("fk_second", "rt_mac0000001")])).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");

    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!((info["key_id"].as_str(), info["rotated_from"].as_str(), info["rotation_needed"].as_bool()), (Some("fk_second"), Some("fk_first"), Some(false)));
    // Old envelopes stay until every replica is re-sealed.
    let (s, old) = call(&state, "GET", "/api/v1/files-keys/envelopes/rt_mac0000001?key_id=fk_first", serde_json::Value::Null).await;
    assert_eq!((s, old["key_id"].as_str()), (StatusCode::OK, Some("fk_first")));

    // A revoked computer stops being a holder the next time the list is read.
    sqlx::query("UPDATE runtime_devices SET revoked_at = NOW() WHERE id = 'rt_mac0000001'").execute(&state.db).await.unwrap();
    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(info["device_ids"], serde_json::json!(["dev_phone01"]));
    assert_eq!(info["rotation_needed"], true);

    // Removing the last holder retires the key, so a device can start a new one.
    assert_eq!(call(&state, "DELETE", "/api/v1/files-keys/devices/dev_phone01", serde_json::Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert!(info.get("key_id").is_none(), "{info}");
}

#[tokio::test]
#[serial_test::serial]
async fn only_a_key_holder_declines_and_only_a_waiting_request() {
    let _g = on();
    let state = setup().await;
    let (_, _, phone_code) = ask(&state, "dev_phone01", 3).await;
    assert_eq!(give(&state, "fk_first", "dev_phone01", "dev_phone01", &phone_code).await.0, StatusCode::NO_CONTENT);
    let (s, _, laptop_code) = ask(&state, "dev_laptop1", 4).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let decline = |who: &str, by: &str| format!("/api/v1/files-keys/requests/{who}?declined_by_device_id={by}");

    // A device without the key can't decline, and the request stays.
    let (s, b) = call(&state, "DELETE", &decline("dev_laptop1", "dev_stranger"), serde_json::Value::Null).await;
    assert_eq!((s, b["code"].as_str()), (StatusCode::FORBIDDEN, Some("forbidden")), "{b}");
    assert_eq!(audit_rows(&state, "files_key.request_declined", false).await, 1);
    // The waiting device can't decline itself out either: it has no key.
    assert_eq!(call(&state, "DELETE", &decline("dev_laptop1", "dev_laptop1"), serde_json::Value::Null).await.0, StatusCode::FORBIDDEN);
    // Missing or malformed decliner.
    assert_eq!(call(&state, "DELETE", "/api/v1/files-keys/requests/dev_laptop1", serde_json::Value::Null).await.0, StatusCode::BAD_REQUEST);

    // A device with no waiting request: not found.
    let (s, b) = call(&state, "DELETE", &decline("dev_nobody1", "dev_phone01"), serde_json::Value::Null).await;
    assert_eq!((s, b["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")), "{b}");

    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert_eq!(info["pending_requests"].as_array().unwrap().len(), 1, "{info}");

    // The holder declines: the request is gone everywhere and can't be granted.
    assert_eq!(call(&state, "DELETE", &decline("dev_laptop1", "dev_phone01"), serde_json::Value::Null).await.0, StatusCode::NO_CONTENT);
    assert_eq!(audit_rows(&state, "files_key.request_declined", true).await, 1);
    let (_, info) = call(&state, "GET", "/api/v1/files-keys", serde_json::Value::Null).await;
    assert!(info["pending_requests"].as_array().unwrap().is_empty(), "{info}");
    assert_eq!(give(&state, "fk_first", "dev_laptop1", "dev_phone01", &laptop_code).await.0, StatusCode::NOT_FOUND);
    // Declining it again finds nothing waiting.
    assert_eq!(call(&state, "DELETE", &decline("dev_laptop1", "dev_phone01"), serde_json::Value::Null).await.0, StatusCode::NOT_FOUND);
    // The declined device may ask again.
    assert_eq!(ask(&state, "dev_laptop1", 4).await.0, StatusCode::ACCEPTED);
}
