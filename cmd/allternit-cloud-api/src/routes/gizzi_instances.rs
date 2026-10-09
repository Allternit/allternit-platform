//! Self-registered gizzi instances (`gizzi serve --tunnel`).
//!
//! A user runs gizzi on their own machine behind an ephemeral public URL
//! (e.g. https://*.trycloudflare.com) and registers it here with their Clerk
//! session — or a paired runtime registers itself with its device credential —
//! so first-party apps can discover it. Unlike `runtime_devices`,
//! these records carry no credentials or relay lifecycle — they are a
//! lightweight, self-service presence registry keyed by (user_id, name).

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get, put},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use sqlx::{FromRow, PgPool};
use std::sync::Arc;
use uuid::Uuid;

use crate::{auth::clerk, routes::runtime_pairing, ApiError, ApiState};

/// An instance is reported as "online" while it has refreshed its
/// registration within this window; older records are "stale".
const ONLINE_WINDOW_MINUTES: i64 = 10;

/// Stale registrations are garbage-collected once their `updated_at` is
/// older than this; the registry is presence-only, so nothing references
/// the deleted rows.
const STALE_RETENTION_DAYS: i64 = 30;

/// How often the background GC sweep runs.
const GC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

const DEFAULT_INSTANCE_NAME: &str = "default";

#[derive(Debug, Deserialize)]
pub struct UpsertInstanceRequest {
    url: String,
    name: Option<String>,
}

#[derive(Debug, FromRow)]
pub(crate) struct GizziInstanceView {
    id: String,
    name: String,
    url: String,
    updated_at: DateTime<Utc>,
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/gizzi-instances", get(list_instances))
        .route("/api/v1/gizzi-instances/self", put(upsert_self_instance))
        .route("/api/v1/gizzi-instances/:id", delete(delete_instance))
}

fn derive_status(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> &'static str {
    if updated_at >= now - Duration::minutes(ONLINE_WINDOW_MINUTES) {
        "online"
    } else {
        "stale"
    }
}

fn instance_json(instance: &GizziInstanceView) -> serde_json::Value {
    serde_json::json!({
        "id": instance.id,
        "name": instance.name,
        "url": instance.url,
        "status": derive_status(instance.updated_at, Utc::now()),
        "updated_at": instance.updated_at,
    })
}

fn validate_instance_url(url: &str) -> Result<String, ApiError> {
    let trimmed = url.trim();
    let valid = (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && trimmed.len() > "https://".len()
        && !trimmed.chars().any(char::is_whitespace);
    if !valid {
        return Err(ApiError::BadRequest(
            "url must be an http(s) URL".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Upsert a gizzi instance row for `user_id`. `pub(crate)` so the BYO-VPS
/// wizard registrar can write the row directly after bootstrap — the API
/// knows the user id, the mesh IP, and the port, so no credentials are ever
/// handed to the VPS for registration.
pub(crate) async fn upsert_instance(
    db: &PgPool,
    user_id: &str,
    name: &str,
    url: &str,
) -> Result<GizziInstanceView, ApiError> {
    let id = format!("gi_{}", Uuid::new_v4().simple());
    sqlx::query(
        r#"
        INSERT INTO gizzi_instances (id, user_id, name, url)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT(user_id, name)
        DO UPDATE SET url = excluded.url, updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(&id)
    .bind(user_id)
    .bind(name)
    .bind(url)
    .execute(db)
    .await?;
    // The explicit updated_at keeps conflict refreshes observable; the table
    // trigger covers any other UPDATE path. Re-read so the response carries
    // the stored id and timestamps either way.
    let instance = sqlx::query_as::<_, GizziInstanceView>(
        "SELECT id, name, url, updated_at FROM gizzi_instances WHERE user_id = $1 AND name = $2",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(db)
    .await?;
    // Keep the migrations_pg/013 backfill row (id 'gi_' || instance id,
    // kind 'local') alive: re-registration refreshes its liveness so node
    // resolution keeps considering it. No-op when the row does not exist
    // (e.g. 013 not applied); the UPDATE targets at most one row.
    sqlx::query(
        r#"
        UPDATE runtime_devices
        SET status = 'online', last_seen_at = CURRENT_TIMESTAMP, endpoint_url = $1
        WHERE id = 'gi_' || $2
        "#,
    )
    .bind(&instance.url)
    .bind(&instance.id)
    .execute(db)
    .await?;
    Ok(instance)
}

/// Who is registering: a paired runtime device registers under its owner
/// and defaults the instance name to the device name; anything else falls
/// back to the Clerk session path. The variant also records which profile
/// data is available for ensuring the `users` row (see [`ensure_user_row`]).
#[derive(Debug)]
enum Actor {
    Device { user_id: String, device_name: String },
    Clerk(clerk::ClerkUser),
    /// Authenticated by an `allternit_*` API token: the user id is known but
    /// no profile claims are available (same backfill contract as Device).
    ApiToken { user_id: String },
}

impl Actor {
    fn user_id(&self) -> &str {
        match self {
            Actor::Device { user_id, .. } => user_id,
            Actor::Clerk(user) => &user.id,
            Actor::ApiToken { user_id } => user_id,
        }
    }

    fn default_name(&self) -> &str {
        match self {
            Actor::Device { device_name, .. } => device_name,
            Actor::Clerk(_) | Actor::ApiToken { .. } => DEFAULT_INSTANCE_NAME,
        }
    }
}

/// Resolves who is registering: a paired runtime device token registers
/// under the device's owner; a Clerk session or `allternit_*` API token
/// falls through to the user identity.
async fn actor_from_headers(db: &PgPool, headers: &HeaderMap) -> Result<Actor, ApiError> {
    if let Some(token) = runtime_pairing::device_token_from_headers(headers) {
        let device = runtime_pairing::runtime_device_for_token(db, token, None).await?;
        // The registry PUT doubles as a lightweight heartbeat.
        sqlx::query(
            "UPDATE runtime_devices SET status = 'online', last_seen_at = CURRENT_TIMESTAMP WHERE id = $1",
        )
        .bind(&device.id)
        .execute(db)
        .await?;
        return Ok(Actor::Device {
            user_id: device.user_id,
            device_name: device.name,
        });
    }
    match clerk::user_from_headers(headers).await {
        Ok(user) => Ok(Actor::Clerk(user)),
        Err(_) => Ok(Actor::ApiToken {
            user_id: crate::auth::resolve_user_id(db, headers).await?,
        }),
    }
}

/// `gizzi_instances.user_id` references `users(id)`, so a `users` row must
/// exist before the upsert — a first registration for a user who never went
/// through the pairing/hosted flows otherwise fails on the FK. Mirrors the
/// pairing-approve pattern (runtime_pairing.rs): a Clerk session upserts
/// its profile claims; a device token only backfills a placeholder row if
/// missing (its owner row was already created by the pairing approve flow
/// and must not be clobbered).
async fn ensure_user_row(db: &PgPool, actor: &Actor) -> Result<(), ApiError> {
    match actor {
        Actor::Clerk(user) => {
            let email = user
                .email
                .clone()
                .unwrap_or_else(|| format!("{}@users.allternit.local", user.id));
            sqlx::query(
                r#"
                INSERT INTO users (id, email, name, avatar_url, status, last_login_at)
                VALUES ($1, $2, $3, $4, 'active', CURRENT_TIMESTAMP)
                ON CONFLICT(id) DO UPDATE SET
                    email = excluded.email,
                    name = COALESCE(excluded.name, users.name),
                    avatar_url = COALESCE(excluded.avatar_url, users.avatar_url),
                    status = 'active',
                    last_login_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(&user.id)
            .bind(&email)
            .bind(user.name.as_deref())
            .bind(user.image_url.as_deref())
            .execute(db)
            .await?;
        }
        Actor::Device { user_id, .. } | Actor::ApiToken { user_id } => {
            let email = format!("{}@users.allternit.local", user_id.replace('@', "_"));
            sqlx::query(
                r#"
                INSERT INTO users (id, email, status, last_login_at)
                VALUES ($1, $2, 'active', CURRENT_TIMESTAMP)
                ON CONFLICT(id) DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(email)
            .execute(db)
            .await?;
        }
    }
    Ok(())
}

async fn upsert_self_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Json(body): Json<UpsertInstanceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let url = validate_instance_url(&body.url)?;
    let actor = actor_from_headers(&state.db, &headers).await?;
    ensure_user_row(&state.db, &actor).await?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| actor.default_name());
    let instance = upsert_instance(&state.db, actor.user_id(), name, &url).await?;
    Ok(Json(instance_json(&instance)))
}

async fn list_instances(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let instances = sqlx::query_as::<_, GizziInstanceView>(
        r#"
        SELECT id, name, url, updated_at
        FROM gizzi_instances
        WHERE user_id = $1
        ORDER BY updated_at DESC
        "#,
    )
    .bind(&user_id)
    .fetch_all(&state.db)
    .await?;
    let instances = instances.iter().map(instance_json).collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "instances": instances })))
}

/// Deletes instance rows whose `updated_at` is older than
/// [`STALE_RETENTION_DAYS`]. Returns the number of rows removed.
pub async fn collect_stale_instances(db: &PgPool) -> Result<u64, ApiError> {
    let deleted = sqlx::query(
        "DELETE FROM gizzi_instances WHERE updated_at < NOW() + $1::INTERVAL",
    )
    .bind(format!("-{STALE_RETENTION_DAYS} days"))
    .execute(db)
    .await?
    .rows_affected();
    Ok(deleted)
}

/// Background GC for stale gizzi instances. Runs a sweep immediately at
/// startup (the first interval tick fires without delay), then hourly.
pub fn start_gizzi_instance_gc_task(db: PgPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(GC_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(
            retention_days = STALE_RETENTION_DAYS,
            "Gizzi instance GC task started"
        );
        loop {
            interval.tick().await;
            match collect_stale_instances(&db).await {
                Ok(deleted) => tracing::info!(deleted, "Gizzi instance GC sweep complete"),
                Err(error) => tracing::error!("Gizzi instance GC sweep failed: {}", error),
            }
        }
    });
}

async fn delete_instance(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?.id;
    let affected = sqlx::query("DELETE FROM gizzi_instances WHERE id = $1 AND user_id = $2")
        .bind(&id)
        .bind(&user_id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if affected == 0 {
        return Err(ApiError::NotFound("Gizzi instance not found".to_string()));
    }
    Ok(Json(serde_json::json!({ "id": id, "status": "deleted" })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table DDL only — the trigger is left out so tests can backdate
    /// updated_at to exercise stale derivation.
    async fn test_pool() -> PgPool {
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
        // Minimal users shape; gizzi_instances.user_id references it, and
        // sqlx enables PRAGMA foreign_keys by default, so the FK is really
        // exercised here.
        sqlx::query("DROP TABLE IF EXISTS users CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE users (
                id TEXT PRIMARY KEY,
                email TEXT NOT NULL UNIQUE,
                name TEXT,
                avatar_url TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                last_login_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DROP TABLE IF EXISTS gizzi_instances CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE gizzi_instances (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                UNIQUE(user_id, name)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Minimal runtime_devices shape for the device-token auth path
        // (including the migration-022 rotation-grace columns
        // runtime_device_for_token falls back to, and the migration-012
        // endpoint_url upsert_instance stamps the backfill row with).
        sqlx::query("DROP TABLE IF EXISTS runtime_devices CASCADE").execute(&pool).await.unwrap();
        sqlx::query(r#"
        CREATE TABLE runtime_devices (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                credential_hash TEXT NOT NULL UNIQUE,
                credential_expires_at TIMESTAMPTZ NOT NULL,
                previous_credential_hash TEXT,
                previous_credential_expires_at TIMESTAMPTZ,
                endpoint_url TEXT,
                status TEXT NOT NULL DEFAULT 'offline',
                last_seen_at TIMESTAMPTZ,
                revoked_at TIMESTAMPTZ
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    async fn insert_device(pool: &PgPool, token: &str, status: &str, revoked: bool) {
        sqlx::query(
            r#"
            INSERT INTO runtime_devices (
                id, user_id, name, credential_hash, credential_expires_at, status, revoked_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind("rd_1")
        .bind("user_9")
        .bind("joes-macbook")
        .bind(runtime_pairing::sha256_hex(token.as_bytes()))
        .bind(Utc::now() + Duration::days(1))
        .bind(status)
        .bind(if revoked { Some(Utc::now()) } else { None })
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_user(pool: &PgPool, id: &str) {
        sqlx::query("INSERT INTO users (id, email) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("{id}@users.allternit.local"))
            .execute(pool)
            .await
            .unwrap();
    }

    #[test]
    fn validate_instance_url_accepts_http_and_https() {
        assert!(validate_instance_url("https://xyz.trycloudflare.com").is_ok());
        assert!(validate_instance_url("http://localhost:8080").is_ok());
    }

    #[test]
    fn validate_instance_url_rejects_non_http() {
        for url in ["", "ftp://host", "xyz.trycloudflare.com", "https://", "https://a b"] {
            assert!(validate_instance_url(url).is_err(), "should reject {url:?}");
        }
    }

    #[test]
    fn derive_status_marks_recent_online_and_old_stale() {
        let now = Utc::now();
        assert_eq!(derive_status(now, now), "online");
        assert_eq!(
            derive_status(now - Duration::minutes(ONLINE_WINDOW_MINUTES - 1), now),
            "online"
        );
        assert_eq!(
            derive_status(now - Duration::minutes(ONLINE_WINDOW_MINUTES + 1), now),
            "stale"
        );
    }

    #[tokio::test]
    async fn upsert_inserts_then_refreshes_url_and_timestamp() {
        let pool = test_pool().await;
        insert_user(&pool, "user_1").await;

        let first = upsert_instance(&pool, "user_1", "my-macbook", "https://a.trycloudflare.com")
            .await
            .unwrap();
        assert_eq!(first.name, "my-macbook");
        assert_eq!(first.url, "https://a.trycloudflare.com");

        // Backdate so a refresh must move updated_at forward.
        sqlx::query("UPDATE gizzi_instances SET updated_at = NOW() - INTERVAL '1 hour'")
            .execute(&pool)
            .await
            .unwrap();

        let second = upsert_instance(&pool, "user_1", "my-macbook", "https://b.trycloudflare.com")
            .await
            .unwrap();
        assert_eq!(second.id, first.id, "upsert keeps the same row");
        assert_eq!(second.url, "https://b.trycloudflare.com");
        assert!(
            second.updated_at > Utc::now() - Duration::minutes(1),
            "upsert refreshes updated_at"
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gizzi_instances")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "repeated PUTs must not duplicate the row");
    }

    #[tokio::test]
    async fn list_is_per_user_with_derived_status() {
        let pool = test_pool().await;
        insert_user(&pool, "user_1").await;
        insert_user(&pool, "user_2").await;
        upsert_instance(&pool, "user_1", "default", "https://a.trycloudflare.com")
            .await
            .unwrap();
        upsert_instance(&pool, "user_1", "stale-one", "https://b.trycloudflare.com")
            .await
            .unwrap();
        upsert_instance(&pool, "user_2", "default", "https://c.trycloudflare.com")
            .await
            .unwrap();
        sqlx::query(
            "UPDATE gizzi_instances SET updated_at = NOW() - INTERVAL '1 hour' WHERE name = 'stale-one'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let rows = sqlx::query_as::<_, GizziInstanceView>(
            "SELECT id, name, url, updated_at FROM gizzi_instances WHERE user_id = $1 ORDER BY updated_at DESC",
        )
        .bind("user_1")
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "default", "newest first");
        let statuses: Vec<&str> = rows
            .iter()
            .map(|row| derive_status(row.updated_at, Utc::now()))
            .collect();
        assert_eq!(statuses, vec!["online", "stale"]);
    }

    #[tokio::test]
    async fn device_token_upsert_resolves_owner_and_device_name() {
        let pool = test_pool().await;
        let token = format!("{}testsecret", runtime_pairing::DEVICE_TOKEN_PREFIX);
        insert_device(&pool, &token, "offline", false).await;

        let actor = actor_from_headers(&pool, &bearer_headers(&token))
            .await
            .unwrap();
        assert_eq!(actor.user_id(), "user_9", "registration lands on the device owner");
        assert_eq!(
            actor.default_name(),
            "joes-macbook",
            "name defaults to the device name"
        );

        ensure_user_row(&pool, &actor).await.unwrap();
        let instance = upsert_instance(&pool, actor.user_id(), actor.default_name(), "https://a.trycloudflare.com")
            .await
            .unwrap();
        assert_eq!(instance.name, "joes-macbook");

        // The registry PUT doubles as a heartbeat: last_seen_at moves forward.
        let (status, last_seen_at): (String, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT status, last_seen_at FROM runtime_devices WHERE id = 'rd_1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "online");
        assert!(
            last_seen_at.unwrap() > Utc::now() - Duration::minutes(1),
            "device registration bumps last_seen_at"
        );
    }

    #[tokio::test]
    async fn gc_deletes_only_instances_older_than_retention() {
        let pool = test_pool().await;
        insert_user(&pool, "user_1").await;

        // Insert the old row directly with a backdated updated_at; any UPDATE
        // would refresh the TIMESTAMPTZ (the production table has a trigger for
        // exactly that), so seed with INSERT only.
        sqlx::query(
            r#"
            INSERT INTO gizzi_instances (id, user_id, name, url, updated_at)
            VALUES ('gi_old', 'user_1', 'old-one', 'https://old.trycloudflare.com',
                    NOW() - INTERVAL '40 days')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        upsert_instance(&pool, "user_1", "fresh-one", "https://fresh.trycloudflare.com")
            .await
            .unwrap();

        let deleted = collect_stale_instances(&pool).await.unwrap();
        assert_eq!(deleted, 1, "only the 40-day-old row should be collected");

        let names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM gizzi_instances ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(names, vec!["fresh-one".to_string()]);

        // A second sweep on a clean registry deletes nothing.
        let deleted = collect_stale_instances(&pool).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn revoked_device_token_is_rejected() {
        let pool = test_pool().await;
        let token = format!("{}testsecret", runtime_pairing::DEVICE_TOKEN_PREFIX);
        insert_device(&pool, &token, "revoked", true).await;

        let error = actor_from_headers(&pool, &bearer_headers(&token))
            .await
            .unwrap_err();
        assert!(
            matches!(error, ApiError::Unauthorized(_)),
            "revoked device credential must be a 401, got {error:?}"
        );

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM gizzi_instances")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "rejected tokens must not register anything");
    }

    #[tokio::test]
    async fn clerk_upsert_creates_users_row_for_brand_new_user() {
        let pool = test_pool().await;
        let actor = Actor::Clerk(clerk::ClerkUser {
            id: "user_brandnew".to_string(),
            email: Some("joe@example.com".to_string()),
            name: Some("Joe".to_string()),
            image_url: None,
            organization_id: None,
            org_role: None,
        });

        // First registration for a user with no users row must not hit the
        // gizzi_instances.user_id FK.
        ensure_user_row(&pool, &actor).await.unwrap();
        let instance = upsert_instance(&pool, actor.user_id(), actor.default_name(), "https://a.trycloudflare.com")
            .await
            .unwrap();
        assert_eq!(instance.name, "default");

        let (email, name): (String, Option<String>) =
            sqlx::query_as("SELECT email, name FROM users WHERE id = 'user_brandnew'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(email, "joe@example.com");
        assert_eq!(name.as_deref(), Some("Joe"));

        // A repeated PUT refreshes the instance and keeps a single users row.
        ensure_user_row(&pool, &actor).await.unwrap();
        upsert_instance(&pool, actor.user_id(), actor.default_name(), "https://b.trycloudflare.com")
            .await
            .unwrap();
        let users_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(users_count, 1);
    }

    #[tokio::test]
    async fn clerk_upsert_without_email_claim_uses_placeholder() {
        let pool = test_pool().await;
        let actor = Actor::Clerk(clerk::ClerkUser {
            id: "user_noemail".to_string(),
            email: None,
            name: None,
            image_url: None,
            organization_id: None,
            org_role: None,
        });
        ensure_user_row(&pool, &actor).await.unwrap();

        let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = 'user_noemail'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(email, "user_noemail@users.allternit.local");
    }

    #[tokio::test]
    async fn device_ensure_user_row_backfills_placeholder_without_clobbering() {
        let pool = test_pool().await;
        let actor = Actor::Device {
            user_id: "user_9".to_string(),
            device_name: "joes-macbook".to_string(),
        };

        // No users row yet (owner predates the pairing approve flow):
        // backfill a placeholder so the FK is satisfied.
        ensure_user_row(&pool, &actor).await.unwrap();
        let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = 'user_9'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(email, "user_9@users.allternit.local");

        // A DOUBLE PRECISION profile row (created by the pairing approve flow) must not
        // be clobbered by the device-token path.
        sqlx::query("UPDATE users SET email = 'joe@example.com', name = 'Joe' WHERE id = 'user_9'")
            .execute(&pool)
            .await
            .unwrap();
        ensure_user_row(&pool, &actor).await.unwrap();
        let (email, name): (String, Option<String>) =
            sqlx::query_as("SELECT email, name FROM users WHERE id = 'user_9'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(email, "joe@example.com");
        assert_eq!(name.as_deref(), Some("Joe"));
    }
}
