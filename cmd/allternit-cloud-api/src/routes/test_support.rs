//! Shared test scaffolding for P1 namespace-handler tests (routes::*).
//!
//! Each control-plane namespace test (agent-sessions, office, beta) mocks
//! the same gateway seam (routes::data_plane::DataPlaneGateway) and builds
//! the same minimal ApiState against a schema-per-test Postgres pool. Kept
//! here so the mocks and the `api_tokens` stub the auth fallback queries
//! live once; the production seam stays in routes::data_plane.

#![cfg(test)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::response::{IntoResponse, Response};

use super::data_plane::DataPlaneGateway;
use super::runtime_relay::RelayRequest;
use crate::services::ResolvedNode;
use crate::{ApiError, ApiState};

pub const DEV_USER: &str = "dev-user";

#[derive(Debug, Clone)]
pub struct RecordedRelay {
    pub user_id: String,
    pub device_id: String,
    pub method: String,
    pub path: String,
    pub body: String, // base64 as forwarded
}

/// Mock gateway: records every relay call, replays queued responses
/// (defaulting to 200 `{}`). Node resolution either returns the canned node
/// or fails with the canned 428 message.
pub struct MockGateway {
    node: Option<ResolvedNode>,
    resolve_error: Option<String>,
    responses: Mutex<VecDeque<Response>>,
    recorded: Mutex<Vec<RecordedRelay>>,
}

impl MockGateway {
    pub fn json(status: StatusCode, body: &str) -> Response {
        (
            status,
            [(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body.to_string(),
        )
            .into_response()
    }

    pub fn healthy_node() -> ResolvedNode {
        ResolvedNode {
            device_id: "rt_default".to_string(),
            name: "default node".to_string(),
            kind: crate::services::NodeKind("local".to_string()),
            last_seen_at: Some(chrono::Utc::now()),
        }
    }

    pub fn new(node: Option<ResolvedNode>, responses: Vec<Response>) -> Self {
        Self {
            node,
            resolve_error: None,
            responses: Mutex::new(responses.into()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    pub fn failing(message: &str) -> Self {
        Self {
            node: None,
            resolve_error: Some(message.to_string()),
            responses: Mutex::new(VecDeque::new()),
            recorded: Mutex::new(Vec::new()),
        }
    }

    pub fn recorded(&self) -> Vec<RecordedRelay> {
        self.recorded.lock().unwrap().clone()
    }
}

#[async_trait]
impl DataPlaneGateway for MockGateway {
    async fn resolve_default_node(&self, _user_id: &str) -> Result<ResolvedNode, ApiError> {
        match (&self.node, &self.resolve_error) {
            (Some(node), _) => Ok(node.clone()),
            (None, Some(message)) => Err(ApiError::PreconditionRequired(message.clone())),
            (None, None) => Err(ApiError::PreconditionRequired("no node".to_string())),
        }
    }

    async fn relay(
        &self,
        user_id: &str,
        device_id: &str,
        request: RelayRequest,
    ) -> Result<Response, ApiError> {
        self.recorded.lock().unwrap().push(RecordedRelay {
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            method: request.method,
            path: request.path,
            body: request.body,
        });
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Self::json(StatusCode::OK, "{}")))
    }
}

/// Schema-per-test pool; `api_tokens` must exist because the auth fallback
/// queries it before the dev-token gate (same pattern as auth::resolve::tests).
/// The remaining tables back the socket-ticket/WS-upgrade path the beta
/// events tests exercise: runtime ownership/capabilities (runtime_devices)
/// and relay-socket quotas (user_runtime_quotas / user_relay_usage /
/// runtime_relay_sockets), all minimal column subsets of migrations_pg/001.
pub async fn test_pool() -> sqlx::PgPool {
    // CI's Postgres service; TEST_DATABASE_URL points a local run elsewhere.
    let url_owned = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string());
    let url = url_owned.as_str();
    let schema = format!("test_{}", uuid::Uuid::new_v4().simple());
    let schema_for_hook = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
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
        .connect(url)
        .await
        .unwrap();
    for statement in [
        r#"
        CREATE TABLE api_tokens (
            id TEXT PRIMARY KEY,
            token_hash TEXT NOT NULL,
            name TEXT NOT NULL,
            user_id TEXT NOT NULL,
            permissions TEXT NOT NULL DEFAULT '[]',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            expires_at TIMESTAMPTZ,
            last_used_at TIMESTAMPTZ,
            is_revoked BOOLEAN NOT NULL DEFAULT FALSE
        )
        "#,
        r#"
        CREATE TABLE runtime_devices (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            name TEXT,
            capabilities TEXT NOT NULL DEFAULT '[]',
            status TEXT NOT NULL DEFAULT 'offline',
            credential_expires_at TIMESTAMPTZ,
            credential_hash TEXT,
            revoked_at TIMESTAMPTZ,
            last_seen_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE user_runtime_quotas (
            user_id TEXT PRIMARY KEY,
            plan_tier_id TEXT,
            max_active_devices BIGINT,
            max_pairings_per_day BIGINT,
            max_relay_sockets BIGINT,
            max_relay_mb_per_day BIGINT,
            max_hosted_runtime_hours_monthly BIGINT,
            can_create_hosted_runtime BOOLEAN,
            max_hosted_runtimes BIGINT DEFAULT 0,
            max_hosted_runtime_memory_mb BIGINT DEFAULT 0,
            hard_spend_cap_usd REAL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
        r#"
        CREATE TABLE user_relay_usage (
            id TEXT PRIMARY KEY,
            user_id TEXT,
            usage_date DATE,
            sockets_opened BIGINT DEFAULT 0,
            peak_concurrent_sockets BIGINT DEFAULT 0,
            egress_bytes BIGINT DEFAULT 0,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(user_id, usage_date)
        )
        "#,
        r#"
        CREATE TABLE runtime_relay_sockets (
            id TEXT PRIMARY KEY,
            runtime_id TEXT,
            socket_path TEXT,
            opened_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            closed_at TIMESTAMPTZ,
            egress_bytes BIGINT DEFAULT 0,
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )
        "#,
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool
}

/// Insert a runtime_devices row owned by `user_id` with the capabilities the
/// relay requires (`runtime:connect` plus `runtime:execute` for beta paths).
pub async fn seed_runtime_device(db: &sqlx::PgPool, device_id: &str, user_id: &str) {
    sqlx::query(
        r#"
        INSERT INTO runtime_devices (id, user_id, name, capabilities, status, credential_expires_at, credential_hash)
        VALUES ($1, $2, 'test node', $3, 'online', '2999-01-01', $4)
        "#,
    )
    .bind(device_id)
    .bind(user_id)
    .bind(r#"["runtime:connect","runtime:execute"]"#)
    .bind(super::runtime_pairing::sha256_hex(format!("token-of-{device_id}").as_bytes()))
    .execute(db)
    .await
    .unwrap();
}

/// Minimal ApiState with the gateway mocked; mirrors the wiring in
/// tests/common/mod.rs.
pub async fn test_state(gateway: Arc<dyn DataPlaneGateway>) -> Arc<ApiState> {
    test_state_with_provisioning(gateway, None).await
}

/// Like [`test_state`], but lets the caller substitute the provisioning
/// service (routes::provisioned_instances tests inject a mock backend
/// registry; everyone else gets the default Incus registry, which these
/// tests never invoke).
pub async fn test_state_with_provisioning(
    gateway: Arc<dyn DataPlaneGateway>,
    provisioning_service: Option<Arc<crate::services::ProvisioningService>>,
) -> Arc<ApiState> {
    let db = test_pool().await;
    let event_store: Arc<dyn crate::services::EventStore> =
        Arc::new(crate::services::EventStoreImpl::new(db.clone()));
    let session_manager = Arc::new(crate::runtime::session_manager::SessionManager::new(
        db.clone(),
    ));
    let run_service: Arc<dyn crate::services::RunService> = Arc::new(
        crate::services::RunServiceImpl::new(db.clone()).with_event_store(event_store.clone()),
    );
    let rate_limit_config = crate::RateLimitConfig {
        requests_per_minute: 100_000,
        window: std::time::Duration::from_secs(60),
    };
    let quota_service = Arc::new(crate::services::QuotaService::new(db.clone()));
    Arc::new(ApiState {
        db: db.clone(),
        ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
        event_tx: tokio::sync::broadcast::channel(16).0,
        event_store,
        run_service,
        session_manager,
        rate_limiter: crate::create_rate_limiter(rate_limit_config.clone()),
        public_rate_limiter: crate::create_rate_limiter(rate_limit_config.clone()),
        free_inference_rate_limiter: crate::create_rate_limiter(rate_limit_config),
        cost_service: Arc::new(crate::services::CostServiceImpl::new(db.clone())),
        quota_service: quota_service.clone(),
        contabo_runtime_service: Arc::new(crate::services::ContaboRuntimeService::new(
            db.clone(),
            None,
            "https://api.allternit.com".to_string(),
        )),
        data_plane_gateway: gateway,
        provisioning_service: provisioning_service
            .unwrap_or_else(|| Arc::new(crate::services::ProvisioningService::new(db.clone()))),
        mesh_service: None,
        credential_cipher: None,
        inference_key_service: None,
        metrics_state: Arc::new(crate::middleware::metrics::MetricsState::new()),
        model_router: crate::model_router::ModelRouter::disabled(
            crate::model_router::catalog::starter_catalog(),
        ),
        inference_pool_service: Arc::new(crate::services::InferencePoolService::new(db.clone())),
    })
}

pub fn authed_request(method: &str, path: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", "Bearer dev-api-token")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// The event backbone's tables (Platform API 050/051 + 057) in a test schema.
pub async fn events_backbone_schema(db: &sqlx::PgPool) {
    for sql in [
        include_str!("../../migrations_pg/003_api_keys.sql"),
        include_str!("../../migrations_pg/020_channel_inbound_queue.sql"),
        include_str!("../../migrations_pg/024_phone_numbers.sql"),
        include_str!("../../migrations_pg/050_platform_api_foundation.sql"),
        include_str!("../../migrations_pg/051_platform_numbers_messaging.sql"),
        include_str!("../../migrations_pg/057_allternit_events_backbone.sql"),
    ] {
        sqlx::raw_sql(&sql.replace("public.", "")).execute(db).await.expect("migration applies");
    }
}
