//! The whole prepaid money loop against **real Stripe test mode**:
//!
//!   1. A signed `checkout.session.completed` credit-pack event grants $10.
//!   2. Usage debits the wallet below the auto top-up threshold.
//!   3. The auto top-up trigger charges a real test card off-session
//!      (Stripe test mode `pm_card_visa`), and Stripe answers `succeeded`.
//!   4. The `payment_intent.succeeded` event for that real PaymentIntent
//!      grants the top-up once; a replay grants nothing.
//!   5. A real test-mode charge made with `pm_card_createDispute` opens a
//!      dispute; its `charge.dispute.created` event holds the credits and
//!      turns auto top-up off.
//!
//! Gated behind `ALLTERNIT_E2E_STRIPE=1` and `STRIPE_TEST_SECRET_KEY`
//! (must start with `sk_test_`; a live key is refused). Needs the local
//! `allternit_test` Postgres. Webhook events are built from the real Stripe
//! objects and signed with a local test secret, as Stripe would sign them.

use allternit_cloud_api::{
    create_rate_limiter, create_router, model_router, runtime, routes, services, ApiState,
    RateLimitConfig,
};
use allternit_cloud_api::routes::billing_checkout::ReqwestStripeCheckout;
use allternit_cloud_api::services::auto_topup;
use allternit_cloud_api::services::CostService;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;

const WEBHOOK_SECRET: &str = "whsec_e2e_money_loop";

fn sign(body: &[u8]) -> String {
    let timestamp = chrono::Utc::now().timestamp();
    let mut payload = timestamp.to_string().into_bytes();
    payload.push(b'.');
    payload.extend_from_slice(body);
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(&payload);
    format!("t={},v1={}", timestamp, hex::encode(mac.finalize().into_bytes()))
}

struct Stripe {
    key: String,
    http: reqwest::Client,
}

impl Stripe {
    async fn post(&self, path: &str, form: &[(&str, &str)]) -> Value {
        let response = self
            .http
            .post(format!("https://api.stripe.com{path}"))
            .basic_auth(&self.key, Some(""))
            .form(form)
            .send()
            .await
            .expect("stripe post");
        let status = response.status();
        let body: Value = response.json().await.expect("stripe json");
        assert!(status.is_success(), "Stripe {path} failed: {body}");
        body
    }

    async fn get(&self, path: &str) -> Value {
        let response = self
            .http
            .get(format!("https://api.stripe.com{path}"))
            .basic_auth(&self.key, Some(""))
            .send()
            .await
            .expect("stripe get");
        let status = response.status();
        let body: Value = response.json().await.expect("stripe json");
        assert!(status.is_success(), "Stripe {path} failed: {body}");
        body
    }

    async fn delete(&self, path: &str) {
        let _ = self
            .http
            .delete(format!("https://api.stripe.com{path}"))
            .basic_auth(&self.key, Some(""))
            .send()
            .await;
    }
}

async fn app(db: &PgPool) -> axum::Router {
    let (event_tx, _rx) = broadcast::channel::<allternit_cloud_api::DeploymentEvent>(100);
    let event_store: Arc<dyn services::EventStore> =
        Arc::new(services::EventStoreImpl::new(db.clone()));
    let session_manager = Arc::new(runtime::session_manager::SessionManager::new(db.clone()));
    let run_service: Arc<dyn services::RunService> =
        Arc::new(services::RunServiceImpl::new(db.clone()).with_event_store(event_store.clone()));
    let cost_service: Arc<dyn services::CostService> =
        Arc::new(services::CostServiceImpl::new(db.clone()));
    let quota_service = Arc::new(services::QuotaService::new(db.clone()));
    let limits = RateLimitConfig {
        requests_per_minute: 100_000,
        window: std::time::Duration::from_secs(60),
    };
    let contabo = || {
        Arc::new(services::ContaboRuntimeService::new(
            db.clone(),
            None,
            "https://api.allternit.com".to_string(),
        ))
    };
    let state = Arc::new(ApiState {
        db: db.clone(),
        ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
        event_tx,
        event_store,
        run_service,
        session_manager,
        rate_limiter: create_rate_limiter(limits.clone()),
        public_rate_limiter: create_rate_limiter(limits.clone()),
        free_inference_rate_limiter: create_rate_limiter(limits),
        cost_service,
        quota_service: quota_service.clone(),
        contabo_runtime_service: contabo(),
        data_plane_gateway: Arc::new(routes::data_plane::PgDataPlaneGateway::new(
            db.clone(),
            contabo(),
            quota_service.clone(),
            Arc::new(services::ProvisioningService::new(db.clone())),
        )),
        provisioning_service: Arc::new(services::ProvisioningService::new(db.clone())),
        mesh_service: None,
        credential_cipher: None,
        inference_key_service: None,
        metrics_state: Arc::new(allternit_cloud_api::middleware::metrics::MetricsState::new()),
        model_router: model_router::ModelRouter::disabled(model_router::catalog::starter_catalog()),
        inference_pool_service: Arc::new(services::InferencePoolService::new(db.clone())),
    });
    create_router(state)
}

async fn post_event(router: &axum::Router, event: Value) -> (StatusCode, Value) {
    let body = event.to_string();
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/stripe")
                .header("content-type", "application/json")
                .header("stripe-signature", sign(body.as_bytes()))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn balance(db: &PgPool, user_id: &str) -> f64 {
    sqlx::query_scalar::<_, f64>("SELECT balance_usd::float8 FROM user_credits WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(db)
        .await
        .expect("balance")
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.005
}

#[tokio::test]
async fn real_stripe_test_mode_money_loop() {
    if std::env::var("ALLTERNIT_E2E_STRIPE").as_deref() != Ok("1") {
        eprintln!("skipped: set ALLTERNIT_E2E_STRIPE=1 and STRIPE_TEST_SECRET_KEY");
        return;
    }
    let key = std::env::var("STRIPE_TEST_SECRET_KEY").expect("STRIPE_TEST_SECRET_KEY");
    assert!(key.starts_with("sk_test_"), "refusing to run with a non-test Stripe key");
    std::env::set_var("STRIPE_SECRET_KEY", &key);
    std::env::set_var("STRIPE_WEBHOOK_SECRET", WEBHOOK_SECRET);
    std::env::remove_var("ALLTERNIT_FABRIC_LEDGER_URL");
    std::env::remove_var("ALLTERNIT_INTERNAL_SERVICE_TOKEN");
    std::env::remove_var("ALLTERNIT_OPS_ALERT_URL");
    std::env::set_var("Allternit_API_DEVELOPMENT_MODE", "true");

    let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string()
    });
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect");
    sqlx::migrate!("./migrations_pg").run(&db).await.expect("migrations");

    let user_id = format!("user_e2e_money_{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active')")
        .bind(&user_id)
        .bind(format!("{user_id}@example.test"))
        .execute(&db)
        .await
        .expect("user");
    let router = app(&db).await;
    let costs = services::CostServiceImpl::new(db.clone());
    let stripe = Stripe { key: key.clone(), http: reqwest::Client::new() };

    // 1. Credit pack: signed checkout.session.completed grants $10 once.
    let pack_event = json!({
        "id": format!("evt_e2e_pack_{}", uuid::Uuid::new_v4().simple()),
        "type": "checkout.session.completed",
        "data": { "object": {
            "id": format!("cs_test_e2e_{}", uuid::Uuid::new_v4().simple()),
            "object": "checkout.session",
            "mode": "payment",
            "payment_status": "paid",
            "metadata": { "clerk_user_id": user_id, "allternit_credits_usd": "10.00" }
        }}
    });
    let (status, body) = post_event(&router, pack_event.clone()).await;
    assert_eq!(status, StatusCode::OK, "pack grant: {body}");
    assert!(close(balance(&db, &user_id).await, 10.0));
    let (status, _) = post_event(&router, pack_event).await;
    assert_eq!(status, StatusCode::OK);
    assert!(close(balance(&db, &user_id).await, 10.0), "pack replay must not grant twice");

    // 2. Real Stripe customer with a saved test card; auto top-up on.
    let customer = stripe
        .post("/v1/customers", &[("email", &format!("{user_id}@example.test")), ("metadata[clerk_user_id]", &user_id)])
        .await;
    let customer_id = customer["id"].as_str().unwrap().to_string();
    let pm = stripe
        .post("/v1/payment_methods/pm_card_visa/attach", &[("customer", &customer_id)])
        .await;
    let pm_id = pm["id"].as_str().unwrap().to_string();
    auto_topup::save_card(
        &db,
        &user_id,
        &customer_id,
        &auto_topup::SavedCard { payment_method_id: pm_id.clone(), brand: "visa".into(), last4: Some("4242".into()) },
    )
    .await
    .expect("save card");
    auto_topup::save_settings(
        &db,
        &user_id,
        &auto_topup::Settings { enabled: true, threshold_usd: 5.0, amount_usd: 10.0, monthly_cap_usd: 50.0 },
    )
    .await
    .expect("settings");

    // 3. Usage takes the wallet under the threshold; the trigger charges the card.
    let after = costs
        .deduct_credits_for_usage(&user_id, &format!("e2e-usage-{}", uuid::Uuid::new_v4()), 7.0, "inference")
        .await
        .expect("debit");
    assert!(close(after, 3.0));
    // The debit itself spawns the trigger (production path): wait for its real PaymentIntent.
    let mut intent_id = String::new();
    for _ in 0..60 {
        let row: Option<Option<String>> = sqlx::query_scalar(
            "SELECT payment_intent_id FROM billing_auto_topup_attempts WHERE user_id = $1",
        )
        .bind(&user_id)
        .fetch_optional(&db)
        .await
        .expect("attempt row");
        if let Some(Some(id)) = row {
            intent_id = id;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(!intent_id.is_empty(), "the debit should start a charge under the threshold");
    let intent = stripe.get(&format!("/v1/payment_intents/{intent_id}")).await;
    assert_eq!(intent["status"], "succeeded", "off-session charge: {intent}");
    assert_eq!(intent["metadata"]["kind"], "auto_topup");
    assert_eq!(intent["amount"], 1000);

    // A second trigger while one is pending starts nothing.
    let again = auto_topup::run_check(&db, &ReqwestStripeCheckout::new(), &key, &user_id, after)
        .await
        .expect("run_check again");
    assert!(again.is_none(), "one pending charge at a time");

    // 4. The real PaymentIntent's success event grants $10 once.
    let succeeded = json!({
        "id": format!("evt_e2e_pi_{}", uuid::Uuid::new_v4().simple()),
        "type": "payment_intent.succeeded",
        "data": { "object": intent }
    });
    let (status, body) = post_event(&router, succeeded.clone()).await;
    assert_eq!(status, StatusCode::OK, "top-up grant: {body}");
    assert!(close(balance(&db, &user_id).await, 13.0));
    let (status, _) = post_event(&router, succeeded).await;
    assert_eq!(status, StatusCode::OK);
    assert!(close(balance(&db, &user_id).await, 13.0), "top-up replay must not grant twice");
    let row = auto_topup::load(&db, &user_id).await.expect("load").expect("row");
    assert_eq!(row.status, "ok");
    assert!(close(row.month_spent_usd, 10.0));

    // 5. A real charge that Stripe test mode disputes automatically.
    let disputed = stripe
        .post(
            "/v1/payment_intents",
            &[
                ("amount", "500"),
                ("currency", "usd"),
                ("customer", &customer_id),
                ("payment_method", "pm_card_createDispute"),
                ("confirm", "true"),
                ("payment_method_types[]", "card"),
                ("metadata[kind]", "credit_pack"),
                ("metadata[clerk_user_id]", &user_id),
                ("metadata[allternit_credits_usd]", "5.00"),
            ],
        )
        .await;
    assert_eq!(disputed["status"], "succeeded", "dispute charge: {disputed}");
    let charge_id = disputed["latest_charge"].as_str().unwrap().to_string();
    let mut dispute = Value::Null;
    for _ in 0..20 {
        let list = stripe.get(&format!("/v1/disputes?charge={charge_id}")).await;
        if let Some(found) = list["data"].as_array().and_then(|d| d.first()) {
            dispute = found.clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    assert!(!dispute.is_null(), "Stripe test mode should open a dispute for pm_card_createDispute");
    let before = balance(&db, &user_id).await;
    let opened = json!({
        "id": format!("evt_e2e_dispute_{}", uuid::Uuid::new_v4().simple()),
        "type": "charge.dispute.created",
        "data": { "object": dispute }
    });
    let (status, body) = post_event(&router, opened.clone()).await;
    assert_eq!(status, StatusCode::OK, "dispute: {body}");
    assert!(close(balance(&db, &user_id).await, before - 5.0), "dispute holds the disputed amount");
    let (status, _) = post_event(&router, opened).await;
    assert_eq!(status, StatusCode::OK);
    assert!(close(balance(&db, &user_id).await, before - 5.0), "dispute replay must not hold twice");
    let row = auto_topup::load(&db, &user_id).await.expect("load").expect("row");
    assert!(!row.enabled, "a dispute turns auto top-up off");

    stripe.delete(&format!("/v1/customers/{customer_id}")).await;
    eprintln!("money loop passed: pack {intent_id} dispute {}", dispute["id"]);
}
