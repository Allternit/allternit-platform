//! Cost endpoint tests (task G9): `month` + `group_by` params honored,
//! tenant scoping per the crate pattern (runs.tenant_id = authenticated user).
//!
//! These tests use their own pool against the shared `public` schema (which
//! the operator-provisioned migrations_pg DDL populates, since it is
//! `public.`-qualified). The crate-wide `tests/common/mod.rs` harness is
//! currently broken in this environment: it scopes connections to an empty
//! per-test schema, so every query there fails with "relation does not
//! exist" — all 35 of its tests fail identically with and without this
//! change. Rows here use unique ids and are deleted at test end.

use allternit_cloud_api::{
    create_rate_limiter, create_router, model_router, runtime, routes, services, ApiState,
    RateLimitConfig,
};
use axum::http::StatusCode;
use axum::response::Response;
use sqlx::PgPool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

const TEST_USER_ID: &str = "dev-user";

struct CostApp {
    db: PgPool,
    router: axum::Router,
    run_ids: Vec<String>,
}

impl CostApp {
    async fn new() -> Self {
        let database_url = std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| {
            "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string()
        });
        // Default search_path (public) — the migrations_pg DDL is
        // `public.`-qualified, so that is where the tables live.
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("connect to test database");
        sqlx::migrate!("./migrations_pg")
            .run(&db)
            .await
            .expect("run migrations_pg");

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
            event_tx: event_tx.clone(),
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
            db,
            router: create_router(state),
            run_ids: Vec::new(),
        }
    }

    /// Seed a run + run_cost row directly. `tenant_id`/`owner_id` model how
    /// the row was created; `started_at` drives month filtering.
    async fn seed_run_cost(
        &mut self,
        run_id: &str,
        tenant_id: Option<&str>,
        owner_id: Option<&str>,
        provider: &str,
        region: &str,
        instance_type: &str,
        total_cost: f64,
        started_at: &str,
    ) {
        // Idempotent so reruns after a panicked test don't collide on the
        // fixed ids.
        sqlx::query("DELETE FROM run_costs WHERE run_id = $1")
            .bind(run_id)
            .execute(&self.db)
            .await
            .unwrap();
        sqlx::query("DELETE FROM runs WHERE id = $1")
            .bind(run_id)
            .execute(&self.db)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO runs (id, name, mode, status, config, owner_id, tenant_id, created_at, updated_at)
             VALUES ($1, $2, 'cloud', 'completed', '{}'::json, $3, $4, NOW(), NOW())",
        )
        .bind(run_id)
        .bind(run_id)
        .bind(owner_id)
        .bind(tenant_id)
        .execute(&self.db)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO run_costs
             (id, run_id, instance_cost, storage_cost, transfer_cost, total_cost,
              provider, region, instance_type, started_at, ended_at, duration_seconds, created_at, updated_at)
             VALUES ($1, $2, $3, 0, 0, $3, $4, $5, $6, $7::timestamptz, $7::timestamptz, 3600, NOW(), NOW())",
        )
        .bind(format!("{run_id}_cost"))
        .bind(run_id)
        .bind(total_cost)
        .bind(provider)
        .bind(region)
        .bind(instance_type)
        .bind(started_at)
        .execute(&self.db)
        .await
        .unwrap();

        self.run_ids.push(run_id.to_string());
    }

    async fn get(&self, path: &str) -> Response {
        self.router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn cleanup(&self) {
        for id in &self.run_ids {
            sqlx::query("DELETE FROM run_costs WHERE run_id = $1")
                .bind(id)
                .execute(&self.db)
                .await
                .unwrap();
            sqlx::query("DELETE FROM runs WHERE id = $1")
                .bind(id)
                .execute(&self.db)
                .await
                .unwrap();
        }
        // Budget row created lazily by the summary endpoint.
        sqlx::query("DELETE FROM user_cost_budgets WHERE user_id = $1")
            .bind(TEST_USER_ID)
            .execute(&self.db)
            .await
            .unwrap();
    }
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn cost_summary_honors_month_filter() {
    let mut app = CostApp::new().await;
    // Dev-mode auth user id is `dev-user`.
    app.seed_run_cost("costp_run_may", Some(TEST_USER_ID), None, "contabo", "us-east", "small", 5.0, "2026-05-05 10:00:00+00").await;
    app.seed_run_cost("costp_run_apr", Some(TEST_USER_ID), None, "contabo", "us-east", "small", 7.0, "2026-04-15 10:00:00+00").await;
    app.seed_run_cost("costp_run_other", Some("user_2"), None, "contabo", "us-east", "small", 99.0, "2026-05-06 10:00:00+00").await;

    // Explicit month → only that month's row, never another tenant's spend.
    let resp = app.get("/api/v1/costs/summary?month=2026-05").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["run_count"], 1);
    assert!((body["current_month_cost"].as_f64().unwrap() - 5.0).abs() < 1e-6);

    // Second month → only that month's row.
    let resp = app.get("/api/v1/costs/summary?month=2026-04").await;
    let body = body_json(resp).await;
    assert_eq!(body["run_count"], 1);
    assert!((body["current_month_cost"].as_f64().unwrap() - 7.0).abs() < 1e-6);

    // Bad month → 400.
    let resp = app.get("/api/v1/costs/summary?month=september").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    app.cleanup().await;
}

#[tokio::test]
async fn cost_breakdown_group_by_dimensions() {
    let mut app = CostApp::new().await;
    app.seed_run_cost("costp_g_p1", Some(TEST_USER_ID), None, "contabo", "us-east", "small", 2.0, "2026-06-01 10:00:00+00").await;
    app.seed_run_cost("costp_g_p2", Some(TEST_USER_ID), None, "hetzner", "eu-central", "large", 3.0, "2026-06-02 10:00:00+00").await;
    app.seed_run_cost("costp_g_p3", Some(TEST_USER_ID), None, "contabo", "us-west", "small", 4.0, "2026-06-03 10:00:00+00").await;

    // group_by=provider → one row per provider, other dims report "all".
    let resp = app.get("/api/v1/costs/breakdown?month=2026-06&group_by=provider").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["provider"], "contabo"); // 6.0 > 3.0, DESC
    assert_eq!(rows[0]["region"], "all");
    assert_eq!(rows[0]["instance_type"], "all");
    assert!((rows[0]["total_cost"].as_f64().unwrap() - 6.0).abs() < 1e-6);
    assert_eq!(rows[1]["provider"], "hetzner");

    // group_by=region → one row per region.
    let resp = app.get("/api/v1/costs/breakdown?month=2026-06&group_by=region").await;
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r["provider"] == "all"));

    // Legacy no-group_by mode still works (three-dimension grouping).
    let resp = app.get("/api/v1/costs/breakdown?month=2026-06").await;
    let body = body_json(resp).await;
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 3);

    // Unknown dimension → 400 (tag grouping intentionally lives only on the
    // allternit-api gateway usage endpoint — a different cost surface).
    let resp = app.get("/api/v1/costs/breakdown?group_by=tag").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    app.cleanup().await;
}

#[tokio::test]
async fn run_cost_tenant_scoping() {
    let mut app = CostApp::new().await;
    // Crate pattern: tenant_id = authenticated user.
    app.seed_run_cost("costp_sc_mine", Some(TEST_USER_ID), None, "contabo", "us-east", "small", 1.5, "2026-03-04 10:00:00+00").await;
    // Legacy owner_id rows still resolve.
    app.seed_run_cost("costp_sc_legacy", None, Some(TEST_USER_ID), "contabo", "us-east", "small", 2.5, "2026-03-04 10:00:00+00").await;
    // Another tenant's run is denied.
    app.seed_run_cost("costp_sc_theirs", Some("user_2"), None, "contabo", "us-east", "small", 9.0, "2026-03-04 10:00:00+00").await;
    // A run with neither tenant nor owner is denied, not world-readable.
    app.seed_run_cost("costp_sc_orphan", None, None, "contabo", "us-east", "small", 9.0, "2026-03-04 10:00:00+00").await;

    let resp = app.get("/api/v1/runs/costp_sc_mine/cost").await;
    if resp.status() != StatusCode::OK {
        panic!("scoping debug: status={} body={}", resp.status(), body_json(resp).await);
    }
    let body = body_json(resp).await;
    assert!((body["total_cost"].as_f64().unwrap() - 1.5).abs() < 1e-6);

    let resp = app.get("/api/v1/runs/costp_sc_legacy/cost").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app.get("/api/v1/runs/costp_sc_theirs/cost").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app.get("/api/v1/runs/costp_sc_orphan/cost").await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app.get("/api/v1/runs/costp_sc_missing/cost").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    app.cleanup().await;
}
