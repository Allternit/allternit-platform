//! Control-plane routes for the P2 per-subscription provisioning lane
//! (`/api/v1/provisioned-instances`, `/api/v1/provisioned-hosts`).
//!
//! Decision record: docs/architecture/2026-09-03-control-plane-data-plane-
//! decision.md items 7-10. These handlers are control-plane only: they
//! schedule, drive the Incus backend, and report status — the data plane
//! inside the container is reached through the runtime relay like any other
//! registered node, never through these routes.
//!
//! Auth: Clerk session per request (`resolve_user_scoped(.., "compute")`),
//! same convention as the other P1/P2 management namespaces. Access model:
//!
//! - Instance routes are owner-scoped: the caller must own the instance
//!   (admins pass any user check — `ALLTERNIT_ADMIN_USER_IDS`, a
//!   comma-separated user-id env list; there is no admin role in Clerk here).
//! - `create` is additionally subscription-gated: the caller must hold an
//!   active-or-trialing `billing_subscriptions` row for the requested
//!   subscription id (or be an admin). A subscription without a gate simply
//!   omits `subscriptionId`, which only admins may do.
//! - Host routes are admin-only; fleet hosts are operator-managed.
//!
//! ## Free sleeping computer (plan decision 16)
//!
//! - `POST /api/v1/provisioned-instances/free` (Clerk): ensure the account's
//!   one free computer. Idempotent (201 on first create, 200 after). 409 when
//!   the account has an active paid subscription, 403 until
//!   `ALLTERNIT_FREE_COMPUTERS=1`.
//! - `POST /api/v1/provisioned-instances/:id/wake` (owner): 202
//!   `{"status":"waking","instance":{…}}` while it starts, 200
//!   `{"status":"running",…}` when already awake; 429 over the hourly wake
//!   limit, 503 when the host's awake cap is full.
//! - `GET /api/v1/provisioned-instances/:id` → `instance.status` +
//!   `instance.runtimeOnline`. Ready = `runtimeOnline: true` (the runtime's
//!   relay connection is live).
//! - `POST /api/v1/runtime-devices/:id/activity` (device credential, called
//!   by the runtime inside the computer): `{"busy":true}` keeps it awake
//!   while a job runs; `{"nextWakeAt":"<RFC3339>"|null}` sets/clears the
//!   scheduled wake.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::services::InstanceView;
use crate::{ApiError, ApiState};

const ADMIN_USER_IDS_ENV: &str = "ALLTERNIT_ADMIN_USER_IDS";

fn is_admin(user_id: &str) -> bool {
    std::env::var(ADMIN_USER_IDS_ENV)
        .map(|value| {
            value
                .split(',')
                .any(|entry| entry.trim() == user_id && !entry.trim().is_empty())
        })
        .unwrap_or(false)
}

fn require_admin(user_id: &str) -> Result<(), ApiError> {
    if is_admin(user_id) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "Provisioned fleet hosts are operator-managed (admin only)".to_string(),
        ))
    }
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/provisioned-instances", post(create_instance))
        .route("/api/v1/provisioned-instances", get(list_instances))
        .route("/api/v1/provisioned-instances/free", post(create_free_instance))
        .route("/api/v1/provisioned-instances/:id/wake", post(wake_instance))
        .route("/api/v1/runtime-devices/:id/activity", post(runtime_activity))
        .route(
            "/api/v1/provisioned-instances/:id",
            get(get_instance_status),
        )
        .route(
            "/api/v1/provisioned-instances/:id/start",
            post(start_instance),
        )
        .route("/api/v1/provisioned-instances/:id/stop", post(stop_instance))
        .route(
            "/api/v1/provisioned-instances/:id/usage",
            get(instance_usage),
        )
        .route("/api/v1/provisioned-instances/:id", axum::routing::delete(delete_instance))
        .route("/api/v1/provisioned-hosts", post(register_host))
        .route("/api/v1/provisioned-hosts", get(list_hosts))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceRequest {
    /// Stripe subscription id to bill this instance against; required unless
    /// the caller is an admin.
    #[serde(default)]
    subscription_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterHostRequest {
    name: String,
    incus_endpoint: String,
    #[serde(default)]
    region: Option<String>,
    cpu_cores: i64,
    memory_mb: i64,
    disk_gb: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub id: String,
    pub name: String,
    pub incus_endpoint: String,
    pub region: Option<String>,
    pub cpu_cores_total: i64,
    pub memory_mb_total: i64,
    pub disk_gb_total: i64,
    pub cpu_cores_free: i64,
    pub memory_mb_free: i64,
    pub disk_gb_free: i64,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// RFC3339 lower bound; default 30 days ago.
    since: Option<String>,
}

async fn create_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Option<Json<CreateInstanceRequest>>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let subscription_id = body
        .and_then(|Json(request)| request.subscription_id)
        .filter(|value| !value.trim().is_empty());

    match subscription_id.as_deref() {
        Some(subscription_id) => ensure_active_subscription(
            &state.db,
            &user.id,
            subscription_id,
        )
        .await?,
        // Without a subscription id only admins may provision.
        None => require_admin(&user.id)?,
    }

    let view = state
        .provisioning_service
        .create(&user.id, subscription_id.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "instance": view }))).into_response())
}

/// Subscription gate: the caller must own an active-or-trialing subscription
/// with exactly this Stripe subscription id.
async fn ensure_active_subscription(
    db: &sqlx::PgPool,
    user_id: &str,
    subscription_id: &str,
) -> Result<(), ApiError> {
    let status: Option<String> = sqlx::query_scalar(
        r#"
        SELECT status FROM billing_subscriptions
        WHERE stripe_subscription_id = $1 AND user_id = $2
        "#,
    )
    .bind(subscription_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    match status.as_deref() {
        Some("active" | "trialing") => Ok(()),
        Some(_) => Err(ApiError::Forbidden(
            "Subscription is not active — renew it to provision an instance".to_string(),
        )),
        None => Err(ApiError::Forbidden(
            "No such subscription on this account".to_string(),
        )),
    }
}

/// Fills `runtimeOnline` (the wake ready signal) from the relay hub.
async fn with_presence(mut view: InstanceView) -> InstanceView {
    if let Some(device_id) = &view.device_id {
        view.runtime_online = crate::routes::runtime_relay::connected_runtime_ids()
            .await
            .contains(device_id);
    }
    view
}

async fn list_instances(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
    let instances: Vec<InstanceView> = state
        .provisioning_service
        .list_for_user(&user.id)
        .await?
        .into_iter()
        .map(|mut view| {
            view.runtime_online = view
                .device_id
                .as_ref()
                .is_some_and(|device_id| connected.contains(device_id));
            view
        })
        .collect();
    Ok(Json(
        serde_json::json!({ "instances": instances }),
    ))
}

async fn get_instance_status(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let instance = state.provisioning_service.get_for_user(&id, &user.id).await?;
    Ok(Json(serde_json::json!({ "instance": with_presence(instance).await })))
}

/// `POST /api/v1/provisioned-instances/free` — see the module docs.
async fn create_free_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let service = &state.provisioning_service;
    if !service.free_defaults().enabled && !is_admin(&user.id) {
        return Err(ApiError::Forbidden(
            "Free cloud computers are not enabled yet".to_string(),
        ));
    }
    if service.has_active_subscription(&user.id).await? {
        return Err(ApiError::Conflict(
            "This account has a paid plan; it uses its paid cloud computer".to_string(),
        ));
    }
    let existed = service
        .list_for_user(&user.id)
        .await?
        .iter()
        .any(|view| {
            view.tier == crate::services::TIER_FREE
                && view.replaced_by.is_none()
                && view.status != "error"
        });
    let view = with_presence(service.create_free(&user.id).await?).await;
    let status = if existed { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, Json(serde_json::json!({ "instance": view }))).into_response())
}

/// `POST /api/v1/provisioned-instances/:id/wake` — see the module docs.
async fn wake_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let result = state
        .provisioning_service
        .wake(&id, Some(&user.id), crate::services::WakeReason::Owner)
        .await?;
    let view = with_presence(result.view).await;
    let (code, status) = match view.status.as_str() {
        "running" => (StatusCode::OK, "running"),
        _ => (StatusCode::ACCEPTED, "waking"),
    };
    Ok((
        code,
        Json(serde_json::json!({
            "status": status,
            "instance": view,
            "pollUrl": format!("/api/v1/provisioned-instances/{id}"),
            "pollIntervalSeconds": 2,
        })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeActivityRequest {
    #[serde(default)]
    busy: bool,
    /// Absent = unchanged; `null` = clear; RFC3339 = set.
    #[serde(default, deserialize_with = "double_option")]
    next_wake_at: Option<Option<DateTime<Utc>>>,
}

fn double_option<'de, D>(deserializer: D) -> Result<Option<Option<DateTime<Utc>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<DateTime<Utc>>::deserialize(deserializer).map(Some)
}

/// `POST /api/v1/runtime-devices/:id/activity` — the runtime inside a
/// provisioned computer reports activity with its device credential.
async fn runtime_activity(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<RuntimeActivityRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = crate::routes::runtime_pairing::device_token_from_headers(&headers)
        .ok_or_else(|| ApiError::Unauthorized("Runtime credential required".to_string()))?;
    crate::routes::runtime_pairing::authenticate_runtime_token(&state, token, &id).await?;
    let instance = state
        .provisioning_service
        .record_runtime_activity(&id, request.busy, request.next_wake_at)
        .await?
        .ok_or_else(|| ApiError::NotFound("No provisioned computer for this runtime".to_string()))?;
    Ok(Json(serde_json::json!({
        "instanceId": instance.id,
        "status": instance.status,
        "nextWakeAt": instance.next_wake_at,
        "lastActivityAt": instance.last_activity_at,
    })))
}

async fn start_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let instance = state.provisioning_service.start(&id, &user.id).await?;
    Ok(Json(serde_json::json!({ "instance": instance })))
}

async fn stop_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let instance = state.provisioning_service.stop(&id, &user.id).await?;
    Ok(Json(serde_json::json!({ "instance": instance })))
}

async fn delete_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let instance = state.provisioning_service.delete(&id, &user.id).await?;
    Ok(Json(serde_json::json!({ "instance": instance })))
}

async fn instance_usage(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let since = match query.since.as_deref() {
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map_err(|_| ApiError::BadRequest("since must be RFC3339".to_string()))?
            .with_timezone(&Utc),
        None => Utc::now() - chrono::Duration::days(30),
    };
    let total_seconds = state.provisioning_service.usage(&id, &user.id, since).await?;
    Ok(Json(serde_json::json!({
        "instanceId": id,
        "since": since,
        "totalRunningSeconds": total_seconds,
    })))
}

async fn register_host(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(request): Json<RegisterHostRequest>,
) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    require_admin(&user.id)?;

    if request.name.trim().is_empty() || request.incus_endpoint.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "name and incusEndpoint are required".to_string(),
        ));
    }
    if request.cpu_cores <= 0 || request.memory_mb <= 0 || request.disk_gb <= 0 {
        return Err(ApiError::BadRequest(
            "capacity must be positive in every dimension".to_string(),
        ));
    }
    let id = format!("ph_{}", Uuid::new_v4().simple());
    sqlx::query(
        r#"
        INSERT INTO provisioned_hosts (
            id, name, incus_endpoint, region,
            cpu_cores_total, memory_mb_total, disk_gb_total
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&id)
    .bind(request.name.trim())
    .bind(request.incus_endpoint.trim())
    .bind(request.region.as_deref())
    .bind(request.cpu_cores as i32)
    .bind(request.memory_mb)
    .bind(request.disk_gb)
    .execute(&state.db)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "status": "registered" })),
    )
        .into_response())
}

async fn list_hosts(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    require_admin(&user.id)?;

    #[derive(sqlx::FromRow)]
    struct HostRow {
        id: String,
        name: String,
        incus_endpoint: String,
        region: Option<String>,
        cpu_cores_total: i32,
        memory_mb_total: i64,
        disk_gb_total: i64,
        cpu_cores_allocated: i32,
        memory_mb_allocated: i64,
        disk_gb_allocated: i64,
        enabled: bool,
    }
    let rows = sqlx::query_as::<_, HostRow>(
        r#"
        SELECT id, name, incus_endpoint, region,
               cpu_cores_total, memory_mb_total, disk_gb_total,
               cpu_cores_allocated, memory_mb_allocated, disk_gb_allocated, enabled
        FROM provisioned_hosts
        ORDER BY id
        "#,
    )
    .fetch_all(&state.db)
    .await?;
    let hosts: Vec<HostView> = rows
        .into_iter()
        .map(|row| HostView {
            id: row.id,
            name: row.name,
            incus_endpoint: row.incus_endpoint,
            region: row.region,
            cpu_cores_total: i64::from(row.cpu_cores_total),
            memory_mb_total: row.memory_mb_total,
            disk_gb_total: row.disk_gb_total,
            cpu_cores_free: i64::from(row.cpu_cores_total) - i64::from(row.cpu_cores_allocated),
            memory_mb_free: row.memory_mb_total - row.memory_mb_allocated,
            disk_gb_free: row.disk_gb_total - row.disk_gb_allocated,
            enabled: row.enabled,
        })
        .collect();
    Ok(Json(serde_json::json!({ "hosts": hosts })))
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, MockGateway, DEV_USER};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tower::ServiceExt;

    // The provisioned route tests need tables beyond test_support's pool
    // (provisioned_*, billing_subscriptions, users, a runtime_devices row
    // shape with revoked_at), so this module builds its own schema-per-test
    // pool — same pattern, extra tables.
    async fn test_pool() -> sqlx::PgPool {
        let url = "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test";
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
            "CREATE TABLE users (id TEXT PRIMARY KEY)",
            r#"
            CREATE TABLE billing_subscriptions (
                stripe_subscription_id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                status TEXT NOT NULL
            )
            "#,
            r#"
            CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT,
                name TEXT,
                kind TEXT,
                status TEXT NOT NULL DEFAULT 'offline',
                revoked_at TIMESTAMPTZ,
                last_seen_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            r#"
            CREATE TABLE provisioned_hosts (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                incus_endpoint TEXT NOT NULL,
                region TEXT,
                cpu_cores_total INTEGER NOT NULL DEFAULT 0,
                memory_mb_total BIGINT NOT NULL DEFAULT 0,
                disk_gb_total BIGINT NOT NULL DEFAULT 0,
                cpu_cores_allocated INTEGER NOT NULL DEFAULT 0,
                memory_mb_allocated BIGINT NOT NULL DEFAULT 0,
                disk_gb_allocated BIGINT NOT NULL DEFAULT 0,
                enabled BOOLEAN NOT NULL DEFAULT TRUE,
                last_seen_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            r#"
            CREATE TABLE provisioned_instances (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id),
                subscription_id TEXT,
                host_id TEXT REFERENCES provisioned_hosts(id),
                incus_name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'provisioning',
                pairing_code_hash TEXT,
                pairing_expires_at TIMESTAMPTZ,
                device_id TEXT,
                cpu_cores INTEGER NOT NULL DEFAULT 2,
                memory_mb BIGINT NOT NULL DEFAULT 2048,
                disk_gb BIGINT NOT NULL DEFAULT 20,
                error_message TEXT,
                last_started_at TIMESTAMPTZ,
                last_stopped_at TIMESTAMPTZ,
                deleted_at TIMESTAMPTZ,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                plan_id TEXT,
                cancelled_at TIMESTAMPTZ,
                delete_after TIMESTAMPTZ,
                snapshot_image TEXT,
                snapshot_expires_at TIMESTAMPTZ,
                snapshot_deleted_at TIMESTAMPTZ,
                restored_from TEXT,
                tier TEXT NOT NULL DEFAULT 'paid',
                last_activity_at TIMESTAMPTZ,
                last_owner_activity_at TIMESTAMPTZ,
                next_wake_at TIMESTAMPTZ,
                sleep_count BIGINT NOT NULL DEFAULT 0,
                wake_count BIGINT NOT NULL DEFAULT 0,
                last_slept_at TIMESTAMPTZ,
                last_woken_at TIMESTAMPTZ,
                replaced_by TEXT
            )
            "#,
            r#"
            CREATE TABLE provisioned_instance_wakes (
                id TEXT PRIMARY KEY,
                instance_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                reason TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
            r#"
            CREATE TABLE provisioned_instance_usage_sessions (
                id TEXT PRIMARY KEY,
                instance_id TEXT NOT NULL REFERENCES provisioned_instances(id) ON DELETE CASCADE,
                user_id TEXT NOT NULL,
                started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                ended_at TIMESTAMPTZ,
                duration_seconds BIGINT,
                stop_reason TEXT
            )
            "#,
            r#"
            CREATE UNIQUE INDEX idx_provisioned_usage_one_open_session
                ON provisioned_instance_usage_sessions(instance_id) WHERE ended_at IS NULL
            "#,
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }
        sqlx::query("INSERT INTO users (id) VALUES ('dev-user'), ('other-user')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn insert_host(pool: &sqlx::PgPool, id: &str) {
        sqlx::query(
            r#"
            INSERT INTO provisioned_hosts (id, name, incus_endpoint, cpu_cores_total, memory_mb_total, disk_gb_total)
            VALUES ($1, $1, 'https://incus.example.com:8443', 8, 8192, 100)
            "#,
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_subscription(pool: &sqlx::PgPool, id: &str, user_id: &str, status: &str) {
        sqlx::query("INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, status) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(user_id)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
    }

    #[derive(Debug, Default)]
    struct MockBackend {
        calls: Mutex<Vec<String>>,
        statuses: Mutex<VecDeque<crate::services::BackendStatus>>,
    }

    #[async_trait]
    impl crate::services::ProvisionBackend for MockBackend {
        async fn create(
            &self,
            spec: &crate::services::ProvisionSpec,
        ) -> Result<(), crate::services::ProvisionError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("create:{}", spec.name));
            Ok(())
        }
        async fn start(&self, name: &str) -> Result<(), crate::services::ProvisionError> {
            self.calls.lock().unwrap().push(format!("start:{name}"));
            Ok(())
        }
        async fn stop(&self, name: &str) -> Result<(), crate::services::ProvisionError> {
            self.calls.lock().unwrap().push(format!("stop:{name}"));
            Ok(())
        }
        async fn status(
            &self,
            _name: &str,
        ) -> Result<crate::services::BackendStatus, crate::services::ProvisionError> {
            Ok(self
                .statuses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(crate::services::BackendStatus::Running))
        }
        async fn delete(&self, name: &str) -> Result<(), crate::services::ProvisionError> {
            self.calls.lock().unwrap().push(format!("delete:{name}"));
            Ok(())
        }
        async fn resize(
            &self,
            name: &str,
            _size: crate::services::ComputerSize,
            _storage_pool: &str,
        ) -> Result<(), crate::services::ProvisionError> {
            self.calls.lock().unwrap().push(format!("resize:{name}"));
            Ok(())
        }
        async fn snapshot_to_image(
            &self,
            _name: &str,
            _alias: &str,
            _compression: Option<&str>,
        ) -> Result<(), crate::services::ProvisionError> {
            Ok(())
        }
        async fn delete_image(&self, _alias: &str) -> Result<(), crate::services::ProvisionError> {
            Ok(())
        }
        async fn push_file(
            &self,
            _name: &str,
            _file: &crate::services::provisioning::InstanceFile,
        ) -> Result<(), crate::services::ProvisionError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct StaticRegistry {
        backend: Arc<MockBackend>,
    }

    #[async_trait]
    impl crate::services::BackendRegistry for StaticRegistry {
        async fn backend(
            &self,
            _host_id: &str,
            _endpoint: &str,
        ) -> Result<Arc<dyn crate::services::ProvisionBackend>, ApiError> {
            Ok(self.backend.clone())
        }
    }

    /// Minimal ApiState against our schema pool with the provisioning
    /// service bound to the mock backend registry (same wiring as
    /// test_support::test_state plus the provisioning service).
    async fn test_state(pool: sqlx::PgPool, backend: Arc<MockBackend>) -> Arc<ApiState> {
        let provisioning = Arc::new(crate::services::ProvisioningService::with_registry(
            pool.clone(),
            Arc::new(StaticRegistry { backend }),
        ));
        let event_store: Arc<dyn crate::services::EventStore> =
            Arc::new(crate::services::EventStoreImpl::new(pool.clone()));
        let session_manager = Arc::new(crate::runtime::session_manager::SessionManager::new(
            pool.clone(),
        ));
        let run_service: Arc<dyn crate::services::RunService> = Arc::new(
            crate::services::RunServiceImpl::new(pool.clone()).with_event_store(event_store.clone()),
        );
        let rate_limit_config = crate::RateLimitConfig {
            requests_per_minute: 100_000,
            window: std::time::Duration::from_secs(60),
        };
        let quota_service = Arc::new(crate::services::QuotaService::new(pool.clone()));
        Arc::new(ApiState {
            db: pool.clone(),
            ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
            event_tx: tokio::sync::broadcast::channel(16).0,
            event_store,
            run_service,
            session_manager,
            rate_limiter: crate::create_rate_limiter(rate_limit_config.clone()),
            public_rate_limiter: crate::create_rate_limiter(rate_limit_config.clone()),
            free_inference_rate_limiter: crate::create_rate_limiter(rate_limit_config),
            cost_service: Arc::new(crate::services::CostServiceImpl::new(pool.clone())),
            quota_service: quota_service.clone(),
            contabo_runtime_service: Arc::new(crate::services::ContaboRuntimeService::new(
                pool.clone(),
                None,
                "https://api.allternit.com".to_string(),
            )),
            data_plane_gateway: Arc::new(MockGateway::new(
                Some(MockGateway::healthy_node()),
                vec![],
            )),
            provisioning_service: provisioning,
            mesh_service: None,
            credential_cipher: None,
            inference_key_service: None,
            metrics_state: Arc::new(crate::middleware::metrics::MetricsState::new()),
            model_router: crate::model_router::ModelRouter::disabled(
                crate::model_router::catalog::starter_catalog(),
            ),
            inference_pool_service: Arc::new(crate::services::InferencePoolService::new(pool)),
        })
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn every_endpoint_requires_authentication() {
        let pool = test_pool().await;
        let state = test_state(pool, Arc::new(MockBackend::default())).await;
        let router = routes().with_state(state);

        for (method, path) in [
            ("POST", "/api/v1/provisioned-instances"),
            ("GET", "/api/v1/provisioned-instances"),
            ("GET", "/api/v1/provisioned-instances/pi_1"),
            ("POST", "/api/v1/provisioned-instances/pi_1/start"),
            ("POST", "/api/v1/provisioned-instances/pi_1/stop"),
            ("DELETE", "/api/v1/provisioned-instances/pi_1"),
            ("GET", "/api/v1/provisioned-instances/pi_1/usage"),
            ("POST", "/api/v1/provisioned-hosts"),
            ("GET", "/api/v1/provisioned-hosts"),
        ] {
            // Extraction precedes the handler's auth check; give Json
            // extractors a valid body so the assertion is about auth.
            let body = if path.ends_with("provisioned-hosts") {
                r#"{"name":"h","incusEndpoint":"https://incus:8443","cpuCores":8,"memoryMb":8192,"diskGb":100}"#
            } else {
                "{}"
            };
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            // Auth runs inside the handler after extraction; a well-formed
            // empty body keeps the assertion about auth, not JSON parsing.
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must require auth"
            );
            let body = body_json(response).await;
            assert_eq!(body["error"], "UNAUTHORIZED", "{method} {path}");
        }
    }

    #[tokio::test]
    async fn create_is_subscription_gated() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let pool = test_pool().await;
        insert_host(&pool, "host_a").await;
        let state = test_state(pool.clone(), Arc::new(MockBackend::default())).await;
        let router = routes().with_state(state);

        // Without a subscription id the caller must be an admin.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // A subscription the caller does not own does not gate.
        insert_subscription(&pool, "sub_other", "other-user", "active").await;
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_other"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // A non-active subscription does not gate.
        insert_subscription(&pool, "sub_pastdue", DEV_USER, "past_due").await;
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_pastdue"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // An active subscription provisions (201) and lands on the fleet.
        insert_subscription(&pool, "sub_ok", DEV_USER, "active").await;
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_ok"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        assert_eq!(body["instance"]["status"], "provisioning");
        assert_eq!(body["instance"]["subscriptionId"], "sub_ok");
        let first_id = body["instance"]["id"].clone();

        // A repeat for the same subscription is idempotent: the same
        // instance comes back, no error and no duplicate.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_ok"}"#,
            ))
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(body_json(response).await["instance"]["id"], first_id);

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn create_without_fleet_capacity_is_a_clean_503() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let pool = test_pool().await;
        insert_subscription(&pool, "sub_ok", DEV_USER, "active").await;
        let state = test_state(pool, Arc::new(MockBackend::default())).await;
        let router = routes().with_state(state);

        let response = router
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_ok"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn wake_endpoint_answers_202_with_the_poll_contract() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let pool = test_pool().await;
        insert_host(&pool, "host_a").await;
        let backend = Arc::new(MockBackend::default());
        let state = test_state(pool.clone(), backend.clone()).await;
        let view = state.provisioning_service.create_free(DEV_USER).await.unwrap();
        let other = state.provisioning_service.create_free("other-user").await.unwrap();
        sqlx::query("UPDATE provisioned_instances SET status = 'sleeping'")
            .execute(&pool)
            .await
            .unwrap();
        let router = routes().with_state(state);
        let id = view.id.clone();

        let response = router
            .clone()
            .oneshot(authed_request("POST", &format!("/api/v1/provisioned-instances/{id}/wake"), ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = body_json(response).await;
        assert_eq!(body["status"], "waking");
        assert_eq!(body["instance"]["runtimeOnline"], false);
        assert_eq!(body["pollUrl"], format!("/api/v1/provisioned-instances/{id}"));
        assert_eq!(body["pollIntervalSeconds"], 2);

        // Someone else's computer is not wakeable through this route.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/provisioned-instances/{}/wake", other.id),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let status: String =
            sqlx::query_scalar("SELECT status FROM provisioned_instances WHERE id = $1")
                .bind(&other.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "sleeping");

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn lifecycle_status_usage_and_ownership_are_owner_scoped() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let pool = test_pool().await;
        insert_host(&pool, "host_a").await;
        insert_subscription(&pool, "sub_ok", DEV_USER, "active").await;
        let backend = Arc::new(MockBackend::default());
        let state = test_state(pool.clone(), backend.clone()).await;
        let router = routes().with_state(state);

        // Create.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-instances",
                r#"{"subscriptionId":"sub_ok"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = body_json(response).await;
        let id = body["instance"]["id"].as_str().unwrap().to_string();

        // Status.
        let response = router
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/provisioned-instances/{id}"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["instance"]["status"], "provisioning");

        // List.
        let response = router
            .clone()
            .oneshot(authed_request("GET", "/api/v1/provisioned-instances", ""))
            .await
            .unwrap();
        let body = body_json(response).await;
        assert_eq!(body["instances"].as_array().unwrap().len(), 1);

        // Pairing bind flips to running (mirrors the exchange path).
        crate::services::activate_registered_device(&pool, &id).await.unwrap();

        // Start from running is a 400; stop works and closes metering.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/provisioned-instances/{id}/start"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/provisioned-instances/{id}/stop"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await["instance"]["status"],
            "stopped"
        );
        assert!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.starts_with("stop:"))
        );

        // Start again, then read usage.
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/provisioned-instances/{id}/start"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = router
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/provisioned-instances/{id}/usage"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["totalRunningSeconds"].as_i64().is_some());

        // Bad since is a 400.
        let response = router
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/provisioned-instances/{id}/usage?since=nope"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // Ownership: the other user cannot see or stop it.
        sqlx::query("UPDATE provisioned_instances SET user_id = 'other-user' WHERE id = $1")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        let response = router
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/provisioned-instances/{id}"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!("/api/v1/provisioned-instances/{id}/stop"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // Delete (as the owner again).
        sqlx::query("UPDATE provisioned_instances SET user_id = 'dev-user' WHERE id = $1")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        let response = router
            .oneshot(authed_request(
                "DELETE",
                &format!("/api/v1/provisioned-instances/{id}"),
                "",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["instance"]["status"], "deleted");
        assert!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.starts_with("delete:"))
        );

        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[tokio::test]
    async fn host_routes_are_admin_only() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap();
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let pool = test_pool().await;
        let state = test_state(pool, Arc::new(MockBackend::default())).await;
        let router = routes().with_state(state);

        // Non-admin: forbidden.
        let response = router
            .clone()
            .oneshot(authed_request("GET", "/api/v1/provisioned-hosts", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-hosts",
                r#"{"name":"h1","incusEndpoint":"https://incus:8443","cpuCores":8,"memoryMb":8192,"diskGb":100}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Admin: register + list with free capacity.
        std::env::set_var(ADMIN_USER_IDS_ENV, DEV_USER);
        let response = router
            .clone()
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-hosts",
                r#"{"name":"h1","incusEndpoint":"https://incus:8443","cpuCores":8,"memoryMb":8192,"diskGb":100}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let response = router
            .clone()
            .oneshot(authed_request("GET", "/api/v1/provisioned-hosts", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["hosts"][0]["name"], "h1");
        assert_eq!(body["hosts"][0]["memoryMbFree"], 8192);
        // Invalid capacity is a 400.
        let response = router
            .oneshot(authed_request(
                "POST",
                "/api/v1/provisioned-hosts",
                r#"{"name":"h2","incusEndpoint":"https://incus:8443","cpuCores":0,"memoryMb":8192,"diskGb":100}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        std::env::remove_var(ADMIN_USER_IDS_ENV);
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }
}
