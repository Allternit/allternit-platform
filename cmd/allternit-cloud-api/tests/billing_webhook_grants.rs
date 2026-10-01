//! Credit-pack webhook grants (task G15): end-to-end through the real HTTP
//! route — Stripe-Signature verification on the raw body, a single grant for
//! `checkout.session.completed` (mode=payment, payment_status=paid), and
//! replay dedupe by Stripe event id.
//!
//! Harness follows tests/cost_params.rs (task G9): own pool against the
//! shared `public` schema, unique ids, rows deleted at test end. TEST MODE
//! ONLY — the Stripe secret here is a fixed test string used to sign and
//! verify locally; no real key is involved and no charge is created.

use allternit_cloud_api::{
    create_rate_limiter, create_router, model_router, runtime, routes, services, ApiState,
    RateLimitConfig,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

const TEST_WEBHOOK_SECRET: &str = "whsec_g15_test_secret";
const EVENT_ID: &str = "evt_g15_e2e_1";
const USER_ID: &str = "user_g15_e2e";

fn sign(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut signed_payload = timestamp.to_string().into_bytes();
    signed_payload.push(b'.');
    signed_payload.extend_from_slice(body);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(&signed_payload);
    format!(
        "t={},v1={}",
        timestamp,
        hex::encode(mac.finalize().into_bytes())
    )
}

fn paid_pack_event(event_id: &str) -> String {
    serde_json::json!({
        "id": event_id,
        "type": "checkout.session.completed",
        "data": {
            "object": {
                "id": "cs_test_g15",
                "mode": "payment",
                "payment_status": "paid",
                "metadata": {
                    "clerk_user_id": USER_ID,
                    "allternit_credits_usd": "25.00"
                }
            }
        }
    })
    .to_string()
}

struct WebhookApp {
    #[allow(dead_code)]
    temp: TempDir,
    db: PgPool,
    router: axum::Router,
}

impl WebhookApp {
    async fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string()
        });
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("connect to test database");
        sqlx::migrate!("./migrations_pg")
            .run(&db)
            .await
            .expect("run migrations_pg");

        // Test-mode only: a local secret signs and verifies the payload.
        // Hermetic bridge state — the wallet fallback is what this test
        // exercises; a stray operator env must not redirect grants at a real
        // allternit-api.
        std::env::set_var("STRIPE_WEBHOOK_SECRET", TEST_WEBHOOK_SECRET);
        std::env::remove_var("ALLTERNIT_FABRIC_LEDGER_URL");
        std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");

        std::env::set_var("Allternit_API_DEVELOPMENT_MODE", "true");

        let (event_tx, _event_rx) = broadcast::channel::<allternit_cloud_api::DeploymentEvent>(100);
        let event_store: Arc<dyn services::EventStore> =
            Arc::new(services::EventStoreImpl::new(db.clone()));
        let session_manager = Arc::new(runtime::session_manager::SessionManager::new(db.clone()));
        let run_service: Arc<dyn services::RunService> =
            Arc::new(services::RunServiceImpl::new(db.clone()).with_event_store(event_store.clone()));
        let cost_service: Arc<dyn services::CostService> =
            Arc::new(services::CostServiceImpl::new(db.clone()));
        let quota_service = Arc::new(services::QuotaService::new(db.clone()));

        let rate_limit_config = RateLimitConfig {
            requests_per_minute: 100_000,
            window: std::time::Duration::from_secs(60),
        };
        let rate_limiter = create_rate_limiter(rate_limit_config.clone());
        let public_rate_limiter = create_rate_limiter(rate_limit_config.clone());
        let free_inference_rate_limiter = create_rate_limiter(rate_limit_config);

        let state = Arc::new(ApiState {
            db: db.clone(),
            ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
            event_tx,
            event_store,
            run_service,
            session_manager,
            rate_limiter,
            public_rate_limiter,
            free_inference_rate_limiter,
            cost_service,
            quota_service: quota_service.clone(),
            contabo_runtime_service: Arc::new(services::ContaboRuntimeService::new(
                db.clone(),
                None,
                "https://api.allternit.com".to_string(),
            )),
            data_plane_gateway: Arc::new(routes::data_plane::PgDataPlaneGateway::new(
                db.clone(),
                Arc::new(services::ContaboRuntimeService::new(
                    db.clone(),
                    None,
                    "https://api.allternit.com".to_string(),
                )),
                quota_service.clone(),
                Arc::new(services::ProvisioningService::new(db.clone())),
            )),
            provisioning_service: Arc::new(services::ProvisioningService::new(db.clone())),
            mesh_service: None,
            credential_cipher: None,
            inference_key_service: None,
            metrics_state: Arc::new(allternit_cloud_api::middleware::metrics::MetricsState::new()),
            model_router: model_router::ModelRouter::disabled(
                model_router::catalog::starter_catalog(),
            ),
            inference_pool_service: Arc::new(services::InferencePoolService::new(db.clone())),
        });

        Self {
            temp,
            db,
            router: create_router(state),
        }
    }

    async fn post_stripe(&self, signature: &str, body: String) -> (StatusCode, serde_json::Value) {
        let response = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/stripe")
                    .header("content-type", "application/json")
                    .header("stripe-signature", signature)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }
}

#[tokio::test]
async fn signed_credit_pack_event_grants_once_and_replays_dedupe() {
    let app = WebhookApp::new().await;

    // FK target for the wallet grant.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(USER_ID)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM credit_transactions WHERE transaction_id = $1")
        .bind(format!("stripe-{EVENT_ID}"))
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM webhook_events WHERE id = $1")
        .bind(EVENT_ID)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active')")
        .bind(USER_ID)
        .bind(format!("{USER_ID}@test.local"))
        .execute(&app.db)
        .await
        .unwrap();

    let body = paid_pack_event(EVENT_ID);
    let now = chrono::Utc::now().timestamp();

    // A forged signature is rejected before any grant.
    let (status, _) = app
        .post_stripe("t=1,v1=deadbeef", body.clone())
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // First delivery: granted into the cloud wallet.
    let (status, json) = app.post_stripe(&sign(TEST_WEBHOOK_SECRET, now, body.as_bytes()), body.clone()).await;
    assert_eq!(status, StatusCode::OK, "response: {json}");
    assert_eq!(json["received"], true);
    assert_eq!(json["target"], "cloud_wallet");
    assert_eq!(json["idempotentReplay"], false);
    assert_eq!(json["balanceUsd"], 25.0);

    let grants: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM credit_transactions WHERE transaction_id = $1",
    )
    .bind(format!("stripe-{EVENT_ID}"))
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(grants, 1);

    // Stripe retry of the identical delivery: acknowledged as a replay,
    // still exactly one grant.
    let (status, json) = app
        .post_stripe(&sign(TEST_WEBHOOK_SECRET, now, body.as_bytes()), body.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "response: {json}");
    assert_eq!(json["idempotentReplay"], true);
    let grants: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM credit_transactions WHERE transaction_id = $1",
    )
    .bind(format!("stripe-{EVENT_ID}"))
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(grants, 1, "replay must not double-grant");

    // A tampered body under a (pre-tamper) valid signature is rejected.
    let tampered = body.replace("25.00", "2500.00");
    let (status, _) = app
        .post_stripe(&sign(TEST_WEBHOOK_SECRET, now, body.as_bytes()), tampered)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Cleanup (ids are unique to this test; delete so reruns stay clean).
    sqlx::query("DELETE FROM credit_transactions WHERE transaction_id = $1")
        .bind(format!("stripe-{EVENT_ID}"))
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM webhook_events WHERE id = $1")
        .bind(EVENT_ID)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM billing_purchase_trust WHERE user_id = $1")
        .bind(USER_ID)
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(USER_ID)
        .execute(&app.db)
        .await
        .unwrap();
}
