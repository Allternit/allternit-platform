//! End-to-end Contabo hosted-runtime proof: provision → data-plane pairing →
//! heartbeat, exercised for real against a scratch schema in the VPS's
//! `allternit_test` Postgres database.
//!
//! This test only runs on the Contabo VPS (mail), where docker and the test
//! database exist. It is gated behind `ALLTERNIT_E2E_CONTABO=1` and skips
//! cleanly (early return, not a failure) when the gate is unset or docker is
//! unreachable, so ordinary `cargo test` runs are unaffected.
//!
//! The chain under test:
//!   1. The API under test is bound on 0.0.0.0:<ephemeral port> backed by a
//!      scratch schema, with the Contabo service pointing containers at
//!      http://<docker-bridge-gateway>:<port> so they can reach it.
//!   2. `ContaboRuntimeService::provision` inserts the instance row and
//!      creates the workload container (Clerk auth cannot be satisfied in a
//!      harness, so the service is called directly — the same call the
//!      `POST /api/v1/hosted-runtimes/contabo` endpoint makes).
//!   3. The container's setup script pairs via /api/v1/runtime-pairings and
//!      the exchange, which flips the instance row to `running` and assigns
//!      its runtime device.
//!   4. The test polls the DB until the instance is `running` with a
//!      `runtime_devices` row.
//!   5. The device token is read from the container's identity file and used
//!      to POST /api/v1/runtime-devices/<id>/heartbeat (exactly what
//!      agent-daemon does every 30s), asserting 200 and a fresh
//!      `last_seen_at`.
//!   6. Cleanup destroys the container and drops the scratch schema, even on
//!      failure.

use allternit_cloud_api::{
    create_rate_limiter, create_router, model_router, runtime, routes, services, ApiState, RateLimitConfig,
};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

const TEST_DATABASE_URL: &str =
    "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
const TEST_USER_ID: &str = "user_e2e_contabo";
/// How long to wait for the container's data plane to finish installing and
/// pair. apt-get + the ~140MB gizzi-code download are slow on a cold box.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(300);
const PAIRING_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Best-effort cleanup that runs even when the chain fails midway.
struct E2eCleanup {
    db: PgPool,
    schema: String,
    container_name: Option<String>,
}

impl E2eCleanup {
    async fn run(&self) {
        if let Some(container) = &self.container_name {
            let _ = Command::new("docker")
                .args(["rm", "-f", container])
                .output()
                .await;
        }
        let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema))
            .execute(&self.db)
            .await;
    }
}

async fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Scratch-schema pool: every connection is pinned to the schema so the API
/// under test and the assertions share one isolated view of the database.
async fn scratch_pool(schema: &str) -> PgPool {
    let schema_for_hook = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let schema = schema_for_hook.clone();
            Box::pin(async move {
                sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                    .execute(&mut *conn)
                    .await?;
                sqlx::query(&format!("SET search_path TO {}", schema))
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(TEST_DATABASE_URL)
        .await
        .expect("connect to allternit_test")
}

/// Minimal schema for the provision → pair → heartbeat chain. Only the tables
/// and columns the route code on this path actually touches. (The legacy
/// SQLite-dialect `migrations/` tree was removed; the canonical schema is
/// `migrations_pg/`, applied via `sqlx::migrate!`.)
async fn create_chain_schema(db: &PgPool) {
    let statements = [
        r#"
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            email TEXT,
            name TEXT,
            avatar_url TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            last_login_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE hosted_runtime_nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            docker_host TEXT NOT NULL,
            tailnet_ip TEXT,
            total_memory_mb BIGINT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE hosted_runtime_instances (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            organization_id TEXT,
            name TEXT NOT NULL,
            provider TEXT NOT NULL,
            region TEXT NOT NULL,
            cpu_kind TEXT NOT NULL,
            cpus BIGINT NOT NULL,
            memory_mb BIGINT NOT NULL,
            status TEXT NOT NULL,
            runtime_device_id TEXT,
            bootstrap_token_hash TEXT,
            node_id TEXT,
            active_since TIMESTAMPTZ,
            last_activity_at TIMESTAMPTZ,
            last_synced_at TIMESTAMPTZ,
            error_message TEXT,
            cost_rate_provider TEXT,
            cost_rate_region TEXT,
            cost_rate_instance_type TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            destroyed_at TIMESTAMPTZ
        )
        "#,
        r#"
        CREATE TABLE runtime_pairings (
            id TEXT PRIMARY KEY,
            device_code_hash TEXT NOT NULL,
            user_code TEXT NOT NULL,
            challenge TEXT NOT NULL,
            public_key TEXT NOT NULL,
            public_key_fingerprint TEXT NOT NULL,
            name TEXT NOT NULL,
            runtime_type TEXT NOT NULL,
            hostname TEXT,
            platform TEXT,
            version TEXT,
            capabilities TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL,
            user_id TEXT,
            organization_id TEXT,
            hosted_instance_id TEXT,
            byo_bootstrap_token_id TEXT,
            runtime_id TEXT,
            expires_at TIMESTAMPTZ NOT NULL,
            approved_at TIMESTAMPTZ,
            consumed_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE runtime_devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            organization_id TEXT,
            name TEXT NOT NULL,
            runtime_type TEXT NOT NULL,
            hostname TEXT,
            platform TEXT,
            version TEXT,
            capabilities TEXT NOT NULL DEFAULT '[]',
            public_key TEXT NOT NULL,
            public_key_fingerprint TEXT NOT NULL,
            credential_hash TEXT NOT NULL,
            credential_expires_at TIMESTAMPTZ NOT NULL,
            previous_credential_hash TEXT,
            previous_credential_expires_at TIMESTAMPTZ,
            status TEXT NOT NULL DEFAULT 'offline',
            last_seen_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            revoked_at TIMESTAMPTZ
        )
        "#,
        r#"
        CREATE TABLE user_runtime_quotas (
            user_id TEXT PRIMARY KEY,
            plan_tier_id TEXT NOT NULL,
            max_active_devices BIGINT NOT NULL,
            max_pairings_per_day BIGINT NOT NULL,
            max_relay_sockets BIGINT NOT NULL,
            max_relay_mb_per_day BIGINT NOT NULL,
            max_hosted_runtime_hours_monthly BIGINT NOT NULL,
            can_create_hosted_runtime BOOLEAN NOT NULL,
            max_hosted_runtimes BIGINT NOT NULL,
            max_hosted_runtime_memory_mb BIGINT NOT NULL,
            hard_spend_cap_usd DOUBLE PRECISION
        )
        "#,
        r#"
        CREATE TABLE user_pairing_usage (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            usage_date DATE NOT NULL,
            pairings_created BIGINT NOT NULL DEFAULT 0,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (user_id, usage_date)
        )
        "#,
        r#"
        CREATE TABLE hosted_runtime_usage_sessions (
            id TEXT PRIMARY KEY,
            hosted_instance_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            ended_at TIMESTAMPTZ,
            duration_seconds BIGINT,
            cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
            estimated_cost_usd DOUBLE PRECISION,
            stop_reason TEXT
        )
        "#,
        r#"
        CREATE TABLE cost_rates (
            provider TEXT NOT NULL,
            region TEXT NOT NULL,
            instance_type TEXT NOT NULL,
            cost_per_hour DOUBLE PRECISION NOT NULL DEFAULT 0,
            PRIMARY KEY (provider, region, instance_type)
        )
        "#,
    ];
    for statement in statements {
        sqlx::query(statement)
            .execute(db)
            .await
            .expect("create chain schema");
    }

    // The pairing flow resolves the owning user's email and enforces quotas,
    // so the user and a generous quota row must already exist.
    sqlx::query(
        "INSERT INTO users (id, email, name, status, last_login_at) VALUES ($1, $2, $3, 'active', NOW())",
    )
    .bind(TEST_USER_ID)
    .bind("e2e-contabo@allternit.test")
    .bind("E2E Contabo")
    .execute(db)
    .await
    .expect("seed user");
    sqlx::query(
        r#"
        INSERT INTO user_runtime_quotas (
            user_id, plan_tier_id, max_active_devices, max_pairings_per_day,
            max_relay_sockets, max_relay_mb_per_day, max_hosted_runtime_hours_monthly,
            can_create_hosted_runtime, max_hosted_runtimes, max_hosted_runtime_memory_mb,
            hard_spend_cap_usd
        ) VALUES ($1, 'e2e', 100, 1000, 100, 10000, 1000, TRUE, 100, 8192, NULL)
        "#,
    )
    .bind(TEST_USER_ID)
    .execute(db)
    .await
    .expect("seed quota");
}

fn build_state(db: PgPool, container_api_url: &str) -> Arc<ApiState> {
    // Let requests through without API tokens, mirroring tests/common.
    std::env::set_var("Allternit_API_DEVELOPMENT_MODE", "true");

    let (event_tx, _event_rx) =
        tokio::sync::broadcast::channel::<allternit_cloud_api::DeploymentEvent>(100);
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
        window: Duration::from_secs(60),
    };
    let rate_limiter = create_rate_limiter(rate_limit_config.clone());
    let public_rate_limiter = create_rate_limiter(rate_limit_config.clone());
    let free_inference_rate_limiter = create_rate_limiter(rate_limit_config);

    Arc::new(ApiState {
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
            container_api_url.to_string(),
        )),
        data_plane_gateway: Arc::new(routes::data_plane::PgDataPlaneGateway::new(
            db.clone(),
            Arc::new(services::ContaboRuntimeService::new(
                db.clone(),
                None,
                container_api_url.to_string(),
            )),
            quota_service.clone(),
            Arc::new(services::ProvisioningService::new(db.clone())),
        )),
        provisioning_service: Arc::new(services::ProvisioningService::new(db.clone())),
        mesh_service: None,
        credential_cipher: None,
        inference_key_service: None,
        metrics_state: Arc::new(allternit_cloud_api::middleware::metrics::MetricsState::new()),
        model_router: model_router::ModelRouter::disabled(model_router::catalog::starter_catalog()),
        inference_pool_service: Arc::new(services::InferencePoolService::new(db)),
    })
}

/// Read the device token out of the container's runtime identity file (the
/// file agent-daemon heartbeats with). Retries briefly: the file appears the
/// moment the exchange succeeds.
async fn container_device_token(container_name: &str) -> Result<String, String> {
    for _ in 0..12 {
        let output = Command::new("docker")
            .args([
                "exec",
                container_name,
                "cat",
                "/data/.local/share/gizzi-code/runtime-device.json",
            ])
            .output()
            .await
            .map_err(|e| format!("docker exec failed: {e}"))?;
        if output.status.success() {
            let body: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|e| format!("identity file is not valid JSON: {e}"))?;
            // Current shape is the agent-daemon RuntimeIdentity document
            // (deviceToken); tolerate the legacy snake_case shape too.
            if let Some(token) = body
                .get("deviceToken")
                .or_else(|| body.get("device_token"))
                .and_then(|value| value.as_str())
            {
                return Ok(token.to_string());
            }
            return Err(format!("identity file has no device token: {body}"));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    Err("identity file never appeared in the container".to_string())
}

/// Tail of the container logs, for failure diagnostics on the VPS.
async fn container_logs(container_name: &str) -> String {
    let output = Command::new("docker")
        .args(["logs", "--tail", "80", container_name])
        .output()
        .await;
    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!("{stdout}{stderr}")
        }
        Err(e) => format!("<docker logs failed: {e}>"),
    }
}

#[tokio::test]
async fn contabo_provision_pair_heartbeat() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "allternit_cloud_api=info".into()),
        )
        .with_test_writer()
        .try_init();
    if std::env::var("ALLTERNIT_E2E_CONTABO").ok().as_deref() != Some("1") {
        eprintln!(
            "skipping e2e_contabo_provision_heartbeat: set ALLTERNIT_E2E_CONTABO=1 \
             (runs only on the Contabo VPS with docker)"
        );
        return;
    }
    if !docker_available().await {
        eprintln!("skipping e2e_contabo_provision_heartbeat: docker daemon is not reachable");
        return;
    }

    // 1. Scratch schema + API under test on an ephemeral port. The listener is
    // bound first so the port can be baked into the URL handed to containers.
    let schema = format!("e2e_contabo_{}", uuid::Uuid::new_v4().simple());
    let db = scratch_pool(&schema).await;
    create_chain_schema(&db).await;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind ephemeral port");
    let port = listener.local_addr().expect("local addr").port();
    let gateway = std::env::var("ALLTERNIT_E2E_DOCKER_GATEWAY")
        .unwrap_or_else(|_| "172.17.0.1".to_string());
    let container_api_url = format!("http://{}:{}", gateway, port);
    let state = build_state(db.clone(), &container_api_url);
    let router = create_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    eprintln!("API under test on port {port}; containers reach it at {container_api_url}");

    let mut cleanup = E2eCleanup {
        db: db.clone(),
        schema: schema.clone(),
        container_name: None,
    };
    let result = run_chain(&state, &db, port, &mut cleanup.container_name).await;
    cleanup.run().await;
    if let Err(error) = result {
        panic!("contabo e2e chain failed: {error}");
    }
}

async fn run_chain(
    state: &Arc<ApiState>,
    db: &PgPool,
    port: u16,
    cleanup_container: &mut Option<String>,
) -> Result<(), String> {
    // 2. Provision: inserts the instance row, creates the container, and runs
    // the data-plane setup script inside it (this blocks until the script —
    // including the pairing attempt — finishes).
    let provisioned = state
        .contabo_runtime_service
        .provision(TEST_USER_ID, "e2e-contabo-proof", 512)
        .await
        .map_err(|e| format!("provision failed: {e}"))?;
    let container_name = services::ContaboRuntimeService::container_name(&provisioned.instance_id);
    *cleanup_container = Some(container_name.clone());
    eprintln!(
        "provisioned instance {} (container {})",
        provisioned.instance_id, container_name
    );

    // 3+4. The setup script pairs through the API under test; poll the DB
    // until the exchange has flipped the instance to running and assigned its
    // runtime device.
    let deadline = Instant::now() + PAIRING_TIMEOUT;
    let (status, runtime_device_id) = loop {
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT status, runtime_device_id FROM hosted_runtime_instances WHERE id = $1",
        )
        .bind(&provisioned.instance_id)
        .fetch_one(db)
        .await
        .map_err(|e| format!("failed to poll instance row: {e}"))?;
        if row.0 == "running" && row.1.is_some() {
            break (row.0, row.1.expect("checked above"));
        }
        if Instant::now() >= deadline {
            let logs = container_logs(&container_name).await;
            return Err(format!(
                "instance never paired (last status: {}, timeout {:?}). container logs:\n{}",
                row.0, PAIRING_TIMEOUT, logs
            ));
        }
        tokio::time::sleep(PAIRING_POLL_INTERVAL).await;
    };
    assert_eq!(status, "running");

    let device_status: String =
        sqlx::query_scalar("SELECT status FROM runtime_devices WHERE id = $1")
            .bind(&runtime_device_id)
            .fetch_optional(db)
            .await
            .map_err(|e| format!("failed to read runtime device: {e}"))?
            .ok_or_else(|| {
                format!("no runtime_devices row for paired device {runtime_device_id}")
            })?;
    eprintln!("paired as runtime device {runtime_device_id} (status {device_status})");

    // 5. Heartbeat: same request agent-daemon makes every 30s, authenticated
    // with the device token the exchange issued to the container.
    let device_token = container_device_token(&container_name).await?;
    let response = reqwest::Client::new()
        .post(format!(
            "http://127.0.0.1:{}/api/v1/runtime-devices/{}/heartbeat",
            port, runtime_device_id
        ))
        .bearer_auth(&device_token)
        .send()
        .await
        .map_err(|e| format!("heartbeat request failed: {e}"))?;
    if response.status() != reqwest::StatusCode::OK {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("heartbeat returned {status}: {body}"));
    }

    let (device_status, last_seen_at): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT status, last_seen_at FROM runtime_devices WHERE id = $1")
            .bind(&runtime_device_id)
            .fetch_one(db)
            .await
            .map_err(|e| format!("failed to re-read runtime device: {e}"))?;
    if device_status != "online" {
        return Err(format!("device status after heartbeat: {device_status}"));
    }
    let last_seen_at = last_seen_at.ok_or("last_seen_at still NULL after heartbeat")?;
    let age = chrono::Utc::now() - last_seen_at;
    if age > chrono::Duration::seconds(60) {
        return Err(format!("last_seen_at is stale after heartbeat: {age}"));
    }
    eprintln!("heartbeat verified: device online, last_seen_at {age} ago");

    Ok(())
}
