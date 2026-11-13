//! The compute gate (CONTRACT-compute-gate-2026-11-13; docs:
//! surfaces/docs/api/account-compute-status.mdx).
//!
//! - `GET /api/v1/account/compute-status`: which computer the account has
//!   (`none`, `paired`, `provisioning`, `hosted`, `suspended`), whether the
//!   gate applies, and the three paths (own Desktop, plan, credits) with
//!   prices read from the server, never hard-coded in clients.
//! - `POST /api/v1/account/compute-status/acknowledge` with
//!   `{"choice":"chat_only"}`: the user confirmed chat-only. Stored in
//!   `account_compute_acknowledgements` (migration 129); repeat calls keep the
//!   first time. It never lifts the gate for agent entry.
//! - [`require_compute`]: the server side of the gate. With
//!   `ALLTERNIT_COMPUTE_GATE=on`, agent entry (the runtime relay and the
//!   data-plane node resolution in front of it) answers 409
//!   `compute_required` for an account in `none` or `suspended`. With the
//!   flag off (the default) it does no database work at all. Chat with Cloud
//!   models, auth, billing, pairing and this route never call it. Synthetic
//!   owners (`platform:<project>`, `platform-drv:<id>`) are not accounts and
//!   are never gated.
//!
//! State rules (contract section 7). Own computers are the account's
//! non-revoked relay-capable `runtime_devices` that no provisioned computer
//! owns. Hosted computers are its `paid` and `credit` provisioned computers
//! (free computers are retired and do not count). The one reported is the
//! user's default node (`user_node_preferences`, surface `*`) when that one is
//! usable; otherwise a live hosted computer, then an own computer, then one
//! being created (or a paid subscription whose computer is not made yet),
//! then a suspended one, then a recent failed create (`none` with
//! `computer.status: "error"`).
//!
//! Auth: Clerk session or `allternit_*` token with the `account` scope, like
//! `me_flags`.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::services::provisioning::{provider_for_endpoint, CreditDefaults, CREDIT_SIZE_ID};
use crate::{auth, ApiError, ApiState};

pub const DOCS_URL: &str = "https://docs.allternit.com/api/account-compute-status";
pub const COMPUTE_REQUIRED_MESSAGE: &str = "Agents need a computer. Link your own, subscribe, or buy credits.";
const DEFAULT_DOWNLOAD_URL: &str = "https://www.allternit.com/download";
const ENV_GATE: &str = "ALLTERNIT_COMPUTE_GATE";
const ENV_DOWNLOAD_URL: &str = "ALLTERNIT_DOWNLOAD_URL";
const ENV_PLUS_INCLUDED_HOURS: &str = "ALLTERNIT_PLUS_INCLUDED_HOURS";
/// Same heartbeat grace as node resolution (`ALLTERNIT_NODE_STALE_AFTER_SECS`).
const ENV_STALE_AFTER_SECS: &str = "ALLTERNIT_NODE_STALE_AFTER_SECS";
/// A failed create is reported (as `none` + `status: "error"`) this long.
const ERROR_REPORT_HOURS: i64 = 24;

/// `ALLTERNIT_COMPUTE_GATE=on` (also `1`/`true`). Default off.
pub fn gate_enabled() -> bool {
    matches!(
        std::env::var(ENV_GATE).ok().map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("on") | Some("1") | Some("true")
    )
}

/// The flags and prices one request works with (env in production,
/// explicit in tests).
#[derive(Debug, Clone)]
pub struct GateConfig {
    pub gate: bool,
    pub credits: bool,
    pub credit: CreditDefaults,
    /// Running hours a month included with Plus (`ALLTERNIT_PLUS_INCLUDED_HOURS`, PENDING Eoj).
    pub plus_included_hours: i64,
    pub download_url: String,
}

impl GateConfig {
    pub fn from_env() -> Self {
        Self {
            gate: gate_enabled(),
            credits: crate::services::credit_computers_enabled(),
            credit: CreditDefaults::from_env(),
            plus_included_hours: std::env::var(ENV_PLUS_INCLUDED_HOURS)
                .ok()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|v| *v >= 0)
                .unwrap_or(200),
            download_url: std::env::var(ENV_DOWNLOAD_URL)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| DEFAULT_DOWNLOAD_URL.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ComputeState {
    None,
    Paired,
    Provisioning,
    Hosted,
    Suspended,
}

impl ComputeState {
    /// Contract: the gate covers `none` and `suspended` (when the flag is on).
    pub fn gated(self) -> bool {
        matches!(self, ComputeState::None | ComputeState::Suspended)
    }
}

/// `computer` in the status response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComputerView {
    pub id: String,
    /// `own`, `plan` or `credits`.
    pub kind: &'static str,
    /// `incus`, `orgo`, or null for the user's own machine.
    pub provider: Option<String>,
    /// `running`, `sleeping`, `starting`, `stopped` or `error`.
    pub status: &'static str,
    pub sleeps_when_idle: bool,
    /// Credit computers: hours of running time the balance buys.
    pub runway_hours: Option<f64>,
    /// Suspended computers: when the stopped computer is deleted (RFC 3339).
    pub retention_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputeSnapshot {
    pub state: ComputeState,
    pub online: bool,
    pub computer: Option<ComputerView>,
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Not an account: Platform API project runtimes and hosted-driver
/// computers. Account ids (Clerk `user_…`) never contain a colon.
fn is_service_owner(user_id: &str) -> bool {
    user_id.contains(':')
}

fn instance_status(status: &str) -> &'static str {
    match status {
        "running" => "running",
        "sleeping" => "sleeping",
        "provisioning" | "waking" => "starting",
        "error" => "error",
        _ => "stopped",
    }
}

#[derive(sqlx::FromRow)]
struct OwnDeviceRow {
    id: String,
    status: String,
    last_seen_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct InstanceRow {
    id: String,
    tier: String,
    status: String,
    device_id: Option<String>,
    delete_after: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    incus_endpoint: Option<String>,
}

/// Relay-capable devices of the account that no provisioned computer owns.
const OWN_DEVICES_SQL: &str = r#"
    SELECT d.id, d.status, d.last_seen_at
    FROM runtime_devices d
    WHERE d.user_id = $1 AND d.revoked_at IS NULL
      AND d.capabilities LIKE '%"runtime:connect"%'
      AND NOT EXISTS (SELECT 1 FROM provisioned_instances i WHERE i.device_id = d.id)
    ORDER BY d.last_seen_at DESC NULLS LAST, d.id
"#;

/// The account's state, for compute-status and the refusal body.
/// `connected` = runtime ids with a live relay connection (the online signal).
pub async fn compute_snapshot(
    db: &sqlx::PgPool,
    user_id: &str,
    connected: &HashSet<String>,
    credit: &CreditDefaults,
    now: DateTime<Utc>,
) -> Result<ComputeSnapshot, ApiError> {
    let own: Vec<OwnDeviceRow> = sqlx::query_as(OWN_DEVICES_SQL).bind(user_id).fetch_all(db).await?;
    let instances: Vec<InstanceRow> = sqlx::query_as(
        r#"
        SELECT i.id, i.tier, i.status, i.device_id, i.delete_after, i.updated_at, h.incus_endpoint
        FROM provisioned_instances i
        LEFT JOIN provisioned_hosts h ON h.id = i.host_id
        WHERE i.user_id = $1 AND i.tier IN ('paid', 'credit') AND i.status <> 'deleted'
          AND i.replaced_by IS NULL
        ORDER BY i.created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    let preferred: Option<String> = sqlx::query_scalar(
        "SELECT node_id FROM user_node_preferences WHERE user_id = $1 AND surface = '*'",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    let subscribed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing'))",
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    let has_credit = instances.iter().any(|row| row.tier == "credit");
    let balance_usd: f64 = if has_credit {
        sqlx::query_scalar::<_, f64>("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .unwrap_or(0.0)
    } else {
        0.0
    };

    let stale_after = Duration::seconds(
        std::env::var(ENV_STALE_AFTER_SECS)
            .ok()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(600),
    );
    let own_view = |row: &OwnDeviceRow| -> (ComputerView, bool) {
        let online = connected.contains(&row.id)
            || (row.status == "online" && row.last_seen_at.is_some_and(|seen| now - seen <= stale_after));
        (
            ComputerView {
                id: row.id.clone(),
                kind: "own",
                provider: None,
                status: if online { "running" } else { "stopped" },
                sleeps_when_idle: false,
                runway_hours: None,
                retention_until: None,
            },
            online,
        )
    };
    let instance_view = |row: &InstanceRow| -> (ComputerView, bool) {
        let provider = row.incus_endpoint.as_deref().map(provider_for_endpoint);
        let credits = row.tier == "credit";
        let online = row.status == "running"
            && row.device_id.as_ref().is_some_and(|device| connected.contains(device));
        (
            ComputerView {
                id: row.id.clone(),
                kind: if credits { "credits" } else { "plan" },
                sleeps_when_idle: provider.as_deref() == Some("orgo"),
                provider,
                status: instance_status(&row.status),
                runway_hours: if credits && row.status != "suspended" {
                    credit.runway_hours(balance_usd)
                } else {
                    None
                },
                retention_until: (row.status == "suspended").then_some(row.delete_after).flatten().map(rfc3339),
            },
            online,
        )
    };
    let snapshot = |state: ComputeState, (computer, online): (ComputerView, bool)| ComputeSnapshot {
        state,
        online: online && matches!(state, ComputeState::Paired | ComputeState::Hosted),
        computer: Some(computer),
    };
    let live = |row: &&InstanceRow| matches!(row.status.as_str(), "running" | "sleeping" | "waking" | "stopped");

    // 1. The user's default node, when it is usable.
    if let Some(node) = &preferred {
        if let Some(row) = own.iter().find(|row| &row.id == node) {
            return Ok(snapshot(ComputeState::Paired, own_view(row)));
        }
        if let Some(row) = instances.iter().filter(live).find(|row| row.device_id.as_ref() == Some(node)) {
            return Ok(snapshot(ComputeState::Hosted, instance_view(row)));
        }
    }
    // 2. A live hosted computer, 3. an own computer.
    if let Some(row) = instances.iter().find(live) {
        return Ok(snapshot(ComputeState::Hosted, instance_view(row)));
    }
    if let Some(row) = own.first() {
        return Ok(snapshot(ComputeState::Paired, own_view(row)));
    }
    // 4. One being created, or a paid plan whose computer is not made yet.
    if let Some(row) = instances.iter().find(|row| row.status == "provisioning") {
        return Ok(snapshot(ComputeState::Provisioning, instance_view(row)));
    }
    if subscribed {
        return Ok(ComputeSnapshot { state: ComputeState::Provisioning, online: false, computer: None });
    }
    // 5. Suspended (credits at $0, or a lapsed plan).
    if let Some(row) = instances.iter().find(|row| row.status == "suspended") {
        return Ok(snapshot(ComputeState::Suspended, instance_view(row)));
    }
    // 6. A recent failed create: none, with the error so clients offer a retry.
    let recent_error = instances
        .first()
        .filter(|row| row.status == "error" && now - row.updated_at <= Duration::hours(ERROR_REPORT_HOURS));
    Ok(ComputeSnapshot {
        state: ComputeState::None,
        online: false,
        computer: recent_error.map(|row| instance_view(row).0),
    })
}

/// True when the account can reach agent work: an own computer, a live or
/// starting hosted computer, or an active plan. One query; the same rule as
/// [`compute_snapshot`] reporting `paired`, `hosted` or `provisioning`.
pub async fn has_compute(db: &sqlx::PgPool, user_id: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM runtime_devices d
            WHERE d.user_id = $1 AND d.revoked_at IS NULL
              AND d.capabilities LIKE '%"runtime:connect"%'
              AND NOT EXISTS (SELECT 1 FROM provisioned_instances i WHERE i.device_id = d.id)
        ) OR EXISTS (
            SELECT 1 FROM provisioned_instances
            WHERE user_id = $1 AND tier IN ('paid', 'credit') AND replaced_by IS NULL
              AND status IN ('provisioning', 'running', 'stopped', 'sleeping', 'waking')
        ) OR EXISTS (
            SELECT 1 FROM billing_subscriptions WHERE user_id = $1 AND status IN ('active', 'trialing')
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await?)
}

/// `paths` (contract section 2): own Desktop, plans, credits (null when off).
pub async fn paths_for(db: &sqlx::PgPool, user_id: &str, config: &GateConfig) -> Result<Value, ApiError> {
    use crate::routes::billing_subscriptions::find_plan;
    // (contract id, billing plan id, placeholder cpu / GB / GB)
    const PLANS: [(&str, &str, i64, i64, i64); 3] = [
        ("plus", "plus", 2, 8, 50),
        ("max", "super", 4, 16, 100),
        ("ultra", "ultra", 8, 32, 200),
    ];
    let sizes: Vec<(String, Option<i32>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT id, computer_base_vcpu, computer_base_memory_mb, computer_base_disk_gb FROM plan_tiers WHERE id = ANY($1)",
    )
    .bind(PLANS.iter().map(|plan| plan.1.to_string()).collect::<Vec<_>>())
    .fetch_all(db)
    .await?;
    let plans: Vec<Value> = PLANS
        .iter()
        .filter_map(|&(id, billing_id, cpu, memory_gb, disk_gb)| {
            let plan = find_plan(billing_id)?;
            let (cpu, memory_gb, disk_gb) = match sizes.iter().find(|row| row.0 == billing_id) {
                Some((_, Some(cpu), Some(memory_mb), Some(disk_gb))) => (i64::from(*cpu), memory_mb / 1024, *disk_gb),
                _ => (cpu, memory_gb, disk_gb),
            };
            Some(json!({
                "id": id,
                "billing_plan_id": billing_id,
                "name": plan.label,
                "price_usd_month": plan.price_usd,
                "computer": { "cpu": cpu, "memory_gb": memory_gb, "disk_gb": disk_gb },
                "included_hours": (id == "plus").then_some(config.plus_included_hours),
            }))
        })
        .collect();
    let credits = if config.credits {
        let balance_usd: f64 = sqlx::query_scalar::<_, f64>("SELECT balance_usd FROM user_credits WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?
            .unwrap_or(0.0);
        let credit = &config.credit;
        json!({
            "price_per_hour_usd": credit.price_per_hour_usd,
            "stopped_price_per_hour_usd": credit.stopped_price_per_hour_usd,
            "balance_usd": balance_usd,
            "min_start_usd": credit.min_start_usd,
            "sizes": [{
                "id": CREDIT_SIZE_ID,
                "cpu": credit.cpu_cores,
                "memory_gb": credit.memory_mb / 1024,
                "disk_gb": credit.disk_gb,
                "price_per_hour_usd": credit.price_per_hour_usd,
                "stopped_price_per_hour_usd": credit.stopped_price_per_hour_usd,
            }],
        })
    } else {
        Value::Null
    };
    Ok(json!({
        "own": { "download_url": config.download_url, "pairing_supported": true },
        "plan": { "plans": plans },
        "credits": credits,
    }))
}

/// The 409 body (contract section 4).
pub fn compute_required_body(paths: Value) -> Value {
    json!({
        "error": {
            "code": "compute_required",
            "message": COMPUTE_REQUIRED_MESSAGE,
            "docs_url": DOCS_URL,
            "paths": paths,
        }
    })
}

/// Server enforcement for agent entry. A no-op unless
/// `ALLTERNIT_COMPUTE_GATE=on`.
pub async fn require_compute(db: &sqlx::PgPool, user_id: &str) -> Result<(), ApiError> {
    if !gate_enabled() {
        return Ok(());
    }
    require_compute_with(db, user_id, &GateConfig::from_env()).await
}

/// [`require_compute`] with explicit flags.
pub async fn require_compute_with(db: &sqlx::PgPool, user_id: &str, config: &GateConfig) -> Result<(), ApiError> {
    if !config.gate || is_service_owner(user_id) || has_compute(db, user_id).await? {
        return Ok(());
    }
    Err(ApiError::ComputeRequired(compute_required_body(paths_for(db, user_id, config).await?)))
}

/// The compute-status response body.
pub async fn status_body(
    db: &sqlx::PgPool,
    user_id: &str,
    config: &GateConfig,
    connected: &HashSet<String>,
    now: DateTime<Utc>,
) -> Result<Value, ApiError> {
    let snapshot = compute_snapshot(db, user_id, connected, &config.credit, now).await?;
    let acknowledged_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT acknowledged_at FROM account_compute_acknowledgements WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(json!({
        "state": snapshot.state,
        "online": snapshot.online,
        "gate": config.gate && snapshot.state.gated(),
        "acknowledged_at": acknowledged_at.map(rfc3339),
        "computer": snapshot.computer,
        "paths": paths_for(db, user_id, config).await?,
        "docs_url": DOCS_URL,
    }))
}

/// Record the chat-only choice (idempotent: the first time is kept).
pub async fn acknowledge(db: &sqlx::PgPool, user_id: &str, body: Option<&Value>) -> Result<Response, ApiError> {
    if body.and_then(|body| body["choice"].as_str()) != Some("chat_only") {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": {
                "code": "invalid_choice",
                "message": "choice must be \"chat_only\"",
            }})),
        )
            .into_response());
    }
    let at: DateTime<Utc> = sqlx::query_scalar(
        r#"
        INSERT INTO account_compute_acknowledgements (user_id, choice)
        VALUES ($1, 'chat_only')
        ON CONFLICT (user_id) DO UPDATE SET choice = account_compute_acknowledgements.choice
        RETURNING acknowledged_at
        "#,
    )
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(Json(json!({ "acknowledged_at": rfc3339(at) })).into_response())
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/account/compute-status", get(get_compute_status))
        .route("/api/v1/account/compute-status/acknowledge", post(post_acknowledge))
}

async fn get_compute_status(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    let connected = crate::routes::runtime_relay::connected_runtime_ids().await;
    status_body(&state.db, &user.id, &GateConfig::from_env(), &connected, Utc::now())
        .await
        .map(Json)
}

async fn post_acknowledge(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let user = auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    acknowledge(&state.db, &user.id, body.as_ref().map(|Json(value)| value)).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{authed_request, test_state, MockGateway};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// The tables the gate reads, from the real migrations (014, 018, 019,
    /// 128, 129) on top of test_support's pool (api_tokens, runtime_devices).
    pub(crate) async fn add_gate_schema(db: &sqlx::PgPool) {
        for statement in [
            "CREATE TABLE users (id TEXT PRIMARY KEY)",
            "CREATE TABLE billing_subscriptions (stripe_subscription_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, plan_id TEXT, plan_tier TEXT, status TEXT NOT NULL)",
            "CREATE TABLE runtime_pairings (id TEXT PRIMARY KEY)",
            "CREATE TABLE plan_tiers (id TEXT PRIMARY KEY, display_name TEXT)",
            "CREATE TABLE user_node_preferences (user_id TEXT NOT NULL, surface TEXT NOT NULL DEFAULT '*', node_id TEXT NOT NULL, PRIMARY KEY (user_id, surface))",
            "CREATE TABLE user_credits (user_id TEXT PRIMARY KEY, balance_usd DOUBLE PRECISION NOT NULL DEFAULT 0, updated_at TIMESTAMPTZ DEFAULT NOW())",
            "CREATE TABLE customer_emails (id BIGSERIAL PRIMARY KEY, user_id TEXT NOT NULL, kind TEXT NOT NULL, ref TEXT NOT NULL DEFAULT '', email TEXT, status TEXT NOT NULL CHECK (status IN ('sent', 'failed', 'skipped')), error TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(), UNIQUE (user_id, kind, ref))",
            "INSERT INTO users (id) VALUES ('dev-user'), ('user_2')",
        ] {
            sqlx::query(statement).execute(db).await.unwrap();
        }
        for sql in [
            include_str!("../../migrations_pg/014_provisioned_fleet.sql"),
            include_str!("../../migrations_pg/018_cloud_computer_provisioning.sql"),
            include_str!("../../migrations_pg/019_free_sleeping_computer.sql"),
            include_str!("../../migrations_pg/128_credit_computers.sql"),
            include_str!("../../migrations_pg/129_compute_gate.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(db).await.expect("migration applies");
        }
        sqlx::query("INSERT INTO provisioned_hosts (id, name, incus_endpoint) VALUES ('host_o', 'orgo', 'orgo://ws_test')")
            .execute(db)
            .await
            .unwrap();
    }

    async fn insert_instance(db: &sqlx::PgPool, id: &str, user: &str, tier: &str, status: &str, device: Option<&str>) {
        sqlx::query(
            "INSERT INTO provisioned_instances (id, user_id, host_id, incus_name, status, tier, device_id) VALUES ($1, $2, 'host_o', $1, $3, $4, $5)",
        )
        .bind(id)
        .bind(user)
        .bind(status)
        .bind(tier)
        .bind(device)
        .execute(db)
        .await
        .unwrap();
    }

    async fn insert_device(db: &sqlx::PgPool, id: &str, user: &str, last_seen_secs_ago: i64) {
        sqlx::query(
            "INSERT INTO runtime_devices (id, user_id, name, capabilities, status, last_seen_at) VALUES ($1, $2, 'mac', '[\"runtime:connect\",\"runtime:execute\"]', 'online', NOW() - make_interval(secs => $3))",
        )
        .bind(id)
        .bind(user)
        .bind(last_seen_secs_ago as f64)
        .execute(db)
        .await
        .unwrap();
    }

    fn config(gate: bool, credits: bool) -> GateConfig {
        GateConfig {
            gate,
            credits,
            credit: CreditDefaults::default(),
            plus_included_hours: 200,
            download_url: DEFAULT_DOWNLOAD_URL.to_string(),
        }
    }

    async fn gate_db() -> sqlx::PgPool {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        add_gate_schema(&state.db).await;
        state.db.clone()
    }

    async fn status(db: &sqlx::PgPool, user: &str, gate: bool) -> Value {
        status_body(db, user, &config(gate, true), &HashSet::new(), Utc::now()).await.unwrap()
    }

    #[tokio::test]
    async fn state_moves_none_to_provisioning_to_hosted_and_gate_follows() {
        let db = gate_db().await;
        let none = status(&db, "user_2", true).await;
        assert_eq!(none["state"], "none");
        assert_eq!(none["gate"], true);
        assert!(none["computer"].is_null());
        assert!(!has_compute(&db, "user_2").await.unwrap());

        insert_instance(&db, "pi_c", "user_2", "credit", "provisioning", None).await;
        let provisioning = status(&db, "user_2", true).await;
        assert_eq!(provisioning["state"], "provisioning");
        assert_eq!(provisioning["gate"], false);
        assert_eq!(provisioning["computer"]["kind"], "credits");
        assert_eq!(provisioning["computer"]["status"], "starting");
        assert_eq!(provisioning["computer"]["provider"], "orgo");
        assert!(has_compute(&db, "user_2").await.unwrap());

        sqlx::query("UPDATE provisioned_instances SET status = 'sleeping' WHERE id = 'pi_c'").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_2', 1.0)").execute(&db).await.unwrap();
        let hosted = status(&db, "user_2", true).await;
        assert_eq!(hosted["state"], "hosted");
        assert_eq!(hosted["computer"]["status"], "sleeping");
        assert_eq!(hosted["computer"]["sleeps_when_idle"], true);
        // $1 at $0.10/h running = 10 hours of runway.
        assert!((hosted["computer"]["runway_hours"].as_f64().unwrap() - 10.0).abs() < 1e-9);

        // Stopped at $0: suspended, gated again, retention date shown.
        sqlx::query("UPDATE provisioned_instances SET status = 'suspended', delete_after = '2026-11-27T12:00:00Z' WHERE id = 'pi_c'")
            .execute(&db)
            .await
            .unwrap();
        let suspended = status(&db, "user_2", true).await;
        assert_eq!(suspended["state"], "suspended");
        assert_eq!(suspended["gate"], true);
        assert_eq!(suspended["computer"]["status"], "stopped");
        assert_eq!(suspended["computer"]["retention_until"], "2026-11-27T12:00:00Z");
        assert!(!has_compute(&db, "user_2").await.unwrap());
    }

    #[tokio::test]
    async fn a_paired_desktop_lifts_the_gate_and_revoking_it_restores_none() {
        let db = gate_db().await;
        insert_device(&db, "rd_mac", "user_2", 30).await;
        let paired = status(&db, "user_2", true).await;
        assert_eq!(paired["state"], "paired");
        assert_eq!(paired["gate"], false);
        assert_eq!(paired["online"], true);
        assert_eq!(paired["computer"]["kind"], "own");
        assert!(paired["computer"]["provider"].is_null());

        // A stale heartbeat keeps the state, flips online.
        sqlx::query("UPDATE runtime_devices SET last_seen_at = NOW() - INTERVAL '2 hours' WHERE id = 'rd_mac'")
            .execute(&db)
            .await
            .unwrap();
        let stale = status(&db, "user_2", true).await;
        assert_eq!(stale["state"], "paired");
        assert_eq!(stale["online"], false);

        sqlx::query("UPDATE runtime_devices SET revoked_at = NOW() WHERE id = 'rd_mac'").execute(&db).await.unwrap();
        assert_eq!(status(&db, "user_2", true).await["state"], "none");
    }

    #[tokio::test]
    async fn a_hosted_computers_own_device_is_not_an_own_computer_and_the_default_node_wins() {
        let db = gate_db().await;
        // The provisioned computer's runtime device is hosted, not "paired".
        insert_device(&db, "rd_cloud", "user_2", 30).await;
        insert_instance(&db, "pi_p", "user_2", "paid", "suspended", Some("rd_cloud")).await;
        assert_eq!(status(&db, "user_2", true).await["state"], "suspended");

        // Desktop paired and a running plan computer: the default node decides.
        sqlx::query("UPDATE provisioned_instances SET status = 'running' WHERE id = 'pi_p'").execute(&db).await.unwrap();
        insert_device(&db, "rd_mac", "user_2", 30).await;
        assert_eq!(status(&db, "user_2", true).await["computer"]["id"], "pi_p");
        sqlx::query("INSERT INTO user_node_preferences (user_id, surface, node_id) VALUES ('user_2', '*', 'rd_mac')")
            .execute(&db)
            .await
            .unwrap();
        let preferred = status(&db, "user_2", true).await;
        assert_eq!(preferred["state"], "paired");
        assert_eq!(preferred["computer"]["id"], "rd_mac");
    }

    #[tokio::test]
    async fn an_active_plan_without_its_computer_is_provisioning_and_free_computers_do_not_count() {
        let db = gate_db().await;
        insert_instance(&db, "pi_f", "user_2", "free", "running", None).await;
        assert_eq!(status(&db, "user_2", true).await["state"], "none");
        sqlx::query("INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status) VALUES ('sub_1', 'user_2', 'plus', 'pro', 'active')")
            .execute(&db)
            .await
            .unwrap();
        let pending = status(&db, "user_2", true).await;
        assert_eq!(pending["state"], "provisioning");
        assert!(pending["computer"].is_null());
        assert_eq!(pending["gate"], false);
    }

    #[tokio::test]
    async fn gate_off_never_gates_but_reports_the_real_state() {
        let db = gate_db().await;
        let body = status(&db, "user_2", false).await;
        assert_eq!(body["state"], "none");
        assert_eq!(body["gate"], false);
        require_compute_with(&db, "user_2", &config(false, true)).await.expect("gate off refuses nothing");
    }

    #[tokio::test]
    async fn gate_on_refuses_agent_entry_with_the_contract_409() {
        let db = gate_db().await;
        let error = require_compute_with(&db, "user_2", &config(true, false)).await.unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["error"]["code"], "compute_required");
        assert_eq!(body["error"]["message"], COMPUTE_REQUIRED_MESSAGE);
        assert_eq!(body["error"]["docs_url"], DOCS_URL);
        assert_eq!(body["error"]["paths"]["own"]["download_url"], DEFAULT_DOWNLOAD_URL);
        // Credits path off: null, as in compute-status.
        assert!(body["error"]["paths"]["credits"].is_null());
        // Plans come from the server: Max is billing plan "super", sized by plan_tiers (migration 018).
        let max = &body["error"]["paths"]["plan"]["plans"][1];
        assert_eq!(max["id"], "max");
        assert_eq!(max["billing_plan_id"], "super");
        assert_eq!(max["price_usd_month"], 100.0);
        assert_eq!(max["computer"]["cpu"], 4);
        assert_eq!(max["computer"]["memory_gb"], 8);
        assert_eq!(body["error"]["paths"]["plan"]["plans"][0]["included_hours"], 200);
        assert!(max["included_hours"].is_null());

        // Platform project runtimes are not accounts: never gated.
        require_compute_with(&db, "platform:proj_1", &config(true, false)).await.unwrap();
        // A paired Desktop passes.
        insert_device(&db, "rd_mac", "user_2", 30).await;
        require_compute_with(&db, "user_2", &config(true, false)).await.unwrap();
    }

    #[tokio::test]
    async fn credits_path_carries_server_prices_and_the_wallet_balance() {
        let db = gate_db().await;
        sqlx::query("INSERT INTO user_credits (user_id, balance_usd) VALUES ('user_2', 12.5)").execute(&db).await.unwrap();
        let credits = &status(&db, "user_2", true).await["paths"]["credits"];
        assert_eq!(credits["balance_usd"], 12.5);
        assert_eq!(credits["price_per_hour_usd"], 0.10);
        assert_eq!(credits["stopped_price_per_hour_usd"], 0.005);
        assert_eq!(credits["min_start_usd"], 5.0);
        assert_eq!(credits["sizes"][0]["id"], "medium");
        assert_eq!(credits["sizes"][0]["memory_gb"], 8);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn acknowledge_is_idempotent_and_rejects_other_choices() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        add_gate_schema(&state.db).await;
        let router = routes().with_state(state.clone());
        let read = |response: Response| async move {
            let status = response.status();
            let body: Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
            (status, body)
        };

        let (code, bad) = read(
            router.clone().oneshot(authed_request("POST", "/api/v1/account/compute-status/acknowledge", r#"{"choice":"plan"}"#)).await.unwrap(),
        )
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert_eq!(bad["error"]["code"], "invalid_choice");

        let path = "/api/v1/account/compute-status/acknowledge";
        let (code, first) = read(router.clone().oneshot(authed_request("POST", path, r#"{"choice":"chat_only"}"#)).await.unwrap()).await;
        assert_eq!(code, StatusCode::OK);
        let (_, second) = read(router.clone().oneshot(authed_request("POST", path, r#"{"choice":"chat_only"}"#)).await.unwrap()).await;
        assert_eq!(first["acknowledged_at"], second["acknowledged_at"]);
        assert!(!first["acknowledged_at"].as_str().unwrap().contains('.'), "whole seconds, as in the contract");

        let (code, status) = read(router.oneshot(authed_request("GET", "/api/v1/account/compute-status", "")).await.unwrap()).await;
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
        assert_eq!(code, StatusCode::OK);
        assert_eq!(status["acknowledged_at"], first["acknowledged_at"]);
        assert_eq!(status["state"], "none");
        // Default env: the gate flag is off, so gate is false even for none.
        assert_eq!(status["gate"], false);
    }
}
