//! Step 17 on cloud-api: a computer pulls the guest drops kept while it
//! slept. DB-backed (`routes::test_support`); R2 is the only fake.

use std::collections::HashMap;
use std::time::Duration;

use tower::ServiceExt;

use super::*;
use crate::routes::runtime_relay::sign_runtime_request;
use crate::routes::test_support::{seed_runtime_device, test_state};
use crate::services::r2::{ObjectStore, R2Error};

const OWNER: &str = "user_drops_owner";

/// R2, faked: an in-memory bucket.
#[derive(Default)]
struct MemoryStore {
    objects: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait::async_trait]
impl ObjectStore for MemoryStore {
    fn presign_get(&self, _b: &str, _k: &str, _t: Duration) -> Result<String, R2Error> {
        Err(R2Error::Unavailable)
    }
    async fn head(&self, _b: &str, k: &str) -> Result<Option<u64>, R2Error> {
        Ok(self.objects.lock().unwrap().get(k).map(|v| v.len() as u64))
    }
    async fn put(&self, _b: &str, k: &str, bytes: Vec<u8>, _c: &str) -> Result<(), R2Error> {
        self.objects.lock().unwrap().insert(k.to_string(), bytes);
        Ok(())
    }
    async fn delete(&self, _b: &str, k: &str) -> Result<(), R2Error> {
        self.objects.lock().unwrap().remove(k);
        Ok(())
    }
    async fn get(&self, _b: &str, k: &str) -> Result<Option<Vec<u8>>, R2Error> {
        Ok(self.objects.lock().unwrap().get(k).cloned())
    }
}

/// A request signed by `runtime` the way allternit-api signs its heartbeat
/// (`seed_runtime_device` stores `sha256_hex("token-of-<id>")` as the key).
fn signed(runtime: &str, method: &str, path: &str, body: &[u8]) -> axum::http::Request<axum::body::Body> {
    let key = crate::routes::runtime_pairing::sha256_hex(format!("token-of-{runtime}").as_bytes());
    let ts = chrono::Utc::now().timestamp();
    axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("x-allternit-runtime-id", runtime)
        .header("x-allternit-runtime-ts", ts.to_string())
        .header("x-allternit-runtime-sig", sign_runtime_request(&key, ts, method, path, body))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_vec()))
        .unwrap()
}

async fn send(state: &Arc<ApiState>, req: axum::http::Request<axum::body::Body>) -> (StatusCode, Vec<u8>) {
    let res = routes().with_state(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    (status, http_body_util::BodyExt::collect(res.into_body()).await.unwrap().to_bytes().to_vec())
}

async fn queue(db: &sqlx::PgPool, store: &MemoryStore, computer: &str, drop_id: &str, bytes: &[u8]) {
    let key = format!("files-drops/{computer}/{drop_id}");
    store.objects.lock().unwrap().insert(key.clone(), bytes.to_vec());
    sqlx::query(
        "INSERT INTO files_queued_drops (drop_id, share_id, guest_id, owner_user_id, computer_id, parent_id, name, size, content_hash, r2_key) \
         VALUES ($1, 'sh_drops', 'gst_dana', $2, $3, 'it_shared', $4, $5, $6, $7)",
    )
    .bind(drop_id)
    .bind(OWNER)
    .bind(computer)
    .bind(format!("{drop_id}.pdf"))
    .bind(bytes.len() as i64)
    .bind(blake3::hash(bytes).to_hex().to_string())
    .bind(&key)
    .execute(db)
    .await
    .unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn a_computer_pulls_only_its_own_drops_and_an_ack_clears_the_r2_copy() {
    std::env::set_var("ALLTERNIT_FILES_DRIVE_ENABLED", "1");
    let state = test_state(Arc::new(crate::routes::test_support::MockGateway::new(None, vec![]))).await;
    seed_runtime_device(&state.db, "rt_drops_mac", OWNER).await;
    seed_runtime_device(&state.db, "rt_drops_other", OWNER).await;

    // Before migration 144: feature_disabled, nothing read.
    let (status, _) = send(&state, signed("rt_drops_mac", "GET", DROPS_PATH, b"")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    sqlx::raw_sql(include_str!("../../migrations_pg/144_files_shares.sql")).execute(&state.db).await.expect("144 applies");
    sqlx::query(
        "INSERT INTO files_shares (share_id, owner_user_id, computer_id, root_id, item_id, role, link_token, created_by) \
         VALUES ('sh_drops', $1, 'rt_drops_mac', 'rt_1', 'it_shared', 'upload_only', 'lk_drops', 'owner')",
    )
    .bind(OWNER)
    .execute(&state.db)
    .await
    .unwrap();
    let store = Arc::new(MemoryStore::default());
    snapshot::test_store::set(Some(store.clone() as Arc<dyn ObjectStore>));
    queue(&state.db, &store, "rt_drops_mac", "drp_1", b"W-9 form").await;
    queue(&state.db, &store, "rt_drops_mac", "drp_2", b"lease").await;
    queue(&state.db, &store, "rt_drops_other", "drp_3", b"not yours").await;

    // Unsigned: refused.
    let unsigned = axum::http::Request::builder().uri(DROPS_PATH).body(axum::body::Body::empty()).unwrap();
    assert_eq!(send(&state, unsigned).await.0, StatusCode::UNAUTHORIZED);

    let (status, body) = send(&state, signed("rt_drops_mac", "GET", DROPS_PATH, b"")).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let listed: QueuedDropsResponse = serde_json::from_slice(&body).unwrap();
    let ids: Vec<&str> = listed.drops.iter().map(|d| d.drop_id.as_str()).collect();
    assert_eq!(ids, ["drp_1", "drp_2"], "only this computer's drops, oldest first");

    let content = format!("{DROPS_PATH}/drp_1/content");
    let (status, bytes) = send(&state, signed("rt_drops_mac", "GET", &content, b"")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, b"W-9 form".as_slice()));
    // Another computer of the same owner can't read it.
    assert_eq!(send(&state, signed("rt_drops_other", "GET", &content, b"")).await.0, StatusCode::NOT_FOUND);

    let ack_path = format!("{DROPS_PATH}/drp_1/ack");
    let ack = br#"{"state":"delivered","item_id":"it_inbox_drop1"}"#;
    assert_eq!(send(&state, signed("rt_drops_mac", "POST", &ack_path, ack)).await.0, StatusCode::NO_CONTENT);
    // Repeated ack (the first answer was lost): still fine.
    assert_eq!(send(&state, signed("rt_drops_mac", "POST", &ack_path, ack)).await.0, StatusCode::NO_CONTENT);
    assert!(!store.objects.lock().unwrap().contains_key("files-drops/rt_drops_mac/drp_1"), "the R2 copy is gone");
    let state_of: String = sqlx::query_scalar("SELECT state FROM files_queued_drops WHERE drop_id = 'drp_1'").fetch_one(&state.db).await.unwrap();
    assert_eq!(state_of, "delivered");
    // Delivered drops are not listed again, and their content is gone.
    let (_, body) = send(&state, signed("rt_drops_mac", "GET", DROPS_PATH, b"")).await;
    let listed: QueuedDropsResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed.drops.iter().map(|d| d.drop_id.as_str()).collect::<Vec<_>>(), ["drp_2"]);
    assert_eq!(send(&state, signed("rt_drops_mac", "GET", &content, b"")).await.0, StatusCode::NOT_FOUND);

    // A signature over another body is refused.
    let mut forged = signed("rt_drops_mac", "POST", &format!("{DROPS_PATH}/drp_2/ack"), br#"{"state":"delivered"}"#);
    *forged.body_mut() = axum::body::Body::from(br#"{"state":"rejected"}"#.to_vec());
    assert_eq!(send(&state, forged).await.0, StatusCode::UNAUTHORIZED);
    snapshot::test_store::set(None);
}
