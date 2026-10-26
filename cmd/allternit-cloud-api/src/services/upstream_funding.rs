//! Upstream funding coverage.
//!
//! Customers prepay Allternit through Stripe (cloud wallet, `user_credits`).
//! Allternit prepays each inference provider (OpenRouter, Together,
//! Fireworks, DeepInfra, Groq); each provider refills itself from Allternit's
//! business card through its own auto-recharge setting. This module watches
//! those prepaid upstream balances so we never sell credits we cannot serve:
//!
//! * a poller ([`start_upstream_funding_task`], every 10 minutes) refreshes
//!   `upstream_balances` and appends `upstream_balance_history`;
//!   - OpenRouter: `GET {OPENROUTER_BASE_URL}/credits` with the production
//!     key (`OPENROUTER_CREDITS_KEY`, else `OPENROUTER_API_KEY`); balance =
//!     `total_credits - total_usage`;
//!   - every other provider (no balance API): the admin's last manual entry
//!     minus our metered upstream spend since that entry (`inference_usage`
//!     joined to `inference_pools`, wholesale cost when known, else the
//!     retail cost, which over-counts on purpose);
//!   - a failed refresh records `last_error` and keeps the last balance; it
//!     never stops the loop;
//! * an alert (`services::ops_alert`, once per provider per 6 h) when days
//!   left < `reserve_days` or balance < `floor_usd`;
//! * an in-memory set of providers under their floor, read by the model
//!   router through [`provider_below_floor`] to route around them when
//!   another provider serves the same model.
//!
//! Admin view: `routes/admin_funding.rs`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use crate::ApiError;

/// How often the poller refreshes every provider.
pub const REFRESH_EVERY: Duration = Duration::from_secs(10 * 60);

/// Inference providers cloud-api can route to, with the env var that turns
/// each on (same names `main.rs` uses to build the model router).
pub const KNOWN_PROVIDERS: &[(&str, &str)] = &[
    ("openrouter", "OPENROUTER_API_KEY"),
    ("together", "TOGETHER_API_KEY"),
    ("fireworks", "FIREWORKS_API_KEY"),
    ("deepinfra", "DEEPINFRA_API_KEY"),
    ("groq", "GROQ_API_KEY"),
];

const DEFAULT_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

// ---------------------------------------------------------------------------
// Router cache
// ---------------------------------------------------------------------------

fn below_floor_set() -> &'static RwLock<HashSet<String>> {
    static BELOW: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    BELOW.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Whether `provider`'s last known balance is under its funding floor.
/// Read by the model router on every request; refreshed by the poller.
/// Unknown balances (no manual entry yet, never refreshed) are never "below".
pub fn provider_below_floor(provider: &str) -> bool {
    below_floor_set()
        .read()
        .map(|set| set.contains(provider))
        .unwrap_or(false)
}

fn set_below_floor(provider: &str, below: bool) {
    let mut set = below_floor_set()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if below {
        set.insert(provider.to_string());
    } else {
        set.remove(provider);
    }
}

/// Key under which two catalog entries count as "the same model" across
/// providers: the upstream id's last path segment, lowercased, with `-` and
/// `_` dropped. `meta-llama/Llama-3.3-70B-Instruct-Turbo` (Together) and the
/// same id on DeepInfra match; `qwen/qwen-2.5-72b-instruct` (OpenRouter) and
/// `Qwen/Qwen2.5-72B-Instruct` match. Deliberately strict otherwise: a
/// wrong match would serve a different model.
pub fn same_model_key(upstream_id: &str) -> String {
    upstream_id
        .rsplit('/')
        .next()
        .unwrap_or(upstream_id)
        .to_lowercase()
        .chars()
        .filter(|c| *c != '-' && *c != '_')
        .collect()
}

// ---------------------------------------------------------------------------
// Math (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Days the balance lasts at the last 24 hours' burn. None = no burn (lasts
/// indefinitely) or unknown balance.
pub fn days_left(balance_usd: Option<f64>, burn_24h_usd: f64) -> Option<f64> {
    let balance = balance_usd?;
    if burn_24h_usd <= 0.0 {
        return None;
    }
    Some((balance.max(0.0)) / burn_24h_usd)
}

/// Upstream money on hand per dollar of customer wallet liability. None when
/// nobody holds credits (nothing owed).
pub fn coverage_ratio(upstream_total_usd: f64, wallet_liability_usd: f64) -> Option<f64> {
    if wallet_liability_usd <= 0.0 {
        return None;
    }
    Some(upstream_total_usd / wallet_liability_usd)
}

/// Balance of a provider without a balance API: last manual entry minus
/// metered spend since then.
pub fn manual_balance(manual_usd: f64, spent_since_usd: f64) -> f64 {
    manual_usd - spent_since_usd.max(0.0)
}

/// OpenRouter `GET /credits` body → balance (total_credits − total_usage).
pub fn openrouter_balance(body: &serde_json::Value) -> Option<f64> {
    let data = body.get("data").unwrap_or(body);
    let credits = data.get("total_credits")?.as_f64()?;
    let usage = data.get("total_usage")?.as_f64()?;
    Some(credits - usage)
}

/// Why a provider is short, if it is.
#[derive(Debug, Clone, PartialEq)]
pub enum Shortfall {
    BelowFloor { balance: f64, floor: f64 },
    LowRunway { days_left: f64, reserve_days: i32 },
}

/// Below floor wins over low runway (it also changes routing).
pub fn shortfall(
    balance_usd: Option<f64>,
    burn_24h_usd: f64,
    reserve_days: i32,
    floor_usd: f64,
) -> Option<Shortfall> {
    let balance = balance_usd?;
    if floor_usd > 0.0 && balance < floor_usd {
        return Some(Shortfall::BelowFloor { balance, floor: floor_usd });
    }
    match days_left(Some(balance), burn_24h_usd) {
        Some(days) if days < reserve_days as f64 => Some(Shortfall::LowRunway {
            days_left: days,
            reserve_days,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One provider's funding state, as stored and as the admin route returns it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FundingRow {
    pub provider: String,
    pub balance_usd: Option<f64>,
    pub source: String,
    pub reserve_days: i32,
    pub floor_usd: f64,
    pub manual_balance_usd: Option<f64>,
    pub manual_set_at: Option<DateTime<Utc>>,
    pub burn_24h_usd: f64,
    pub last_error: Option<String>,
    pub refreshed_at: Option<DateTime<Utc>>,
}

const ROW_COLUMNS: &str = "provider, balance_usd, source, reserve_days, floor_usd, \
    manual_balance_usd, manual_set_at, burn_24h_usd, last_error, refreshed_at";

/// Providers with a production key set in this process.
pub fn configured_providers() -> Vec<&'static str> {
    KNOWN_PROVIDERS
        .iter()
        .filter(|(_, env)| std::env::var(env).is_ok_and(|v| !v.trim().is_empty()))
        .map(|(provider, _)| *provider)
        .collect()
}

pub fn is_known_provider(provider: &str) -> bool {
    KNOWN_PROVIDERS.iter().any(|(p, _)| *p == provider)
}

async fn ensure_row(db: &PgPool, provider: &str) -> Result<(), ApiError> {
    let source = if provider == "openrouter" { "api" } else { "manual" };
    sqlx::query(
        "INSERT INTO upstream_balances (provider, source) VALUES ($1, $2) ON CONFLICT (provider) DO NOTHING",
    )
    .bind(provider)
    .bind(source)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn load_row(db: &PgPool, provider: &str) -> Result<Option<FundingRow>, ApiError> {
    let sql = format!("SELECT {ROW_COLUMNS} FROM upstream_balances WHERE provider = $1");
    Ok(sqlx::query_as::<_, FundingRow>(&sql)
        .bind(provider)
        .fetch_optional(db)
        .await?)
}

pub async fn load_rows(db: &PgPool) -> Result<Vec<FundingRow>, ApiError> {
    let sql = format!("SELECT {ROW_COLUMNS} FROM upstream_balances ORDER BY provider");
    Ok(sqlx::query_as::<_, FundingRow>(&sql).fetch_all(db).await?)
}

/// Our metered upstream spend at `provider` since `since` (USD). Wholesale
/// cost when known, else the retail cost (over-counts, the safe direction).
/// BYOK usage has no pool and is never counted. A missing pools table
/// counts as zero spend rather than failing the refresh.
async fn spend_since(db: &PgPool, provider: &str, since: DateTime<Utc>) -> f64 {
    let result = sqlx::query_scalar::<_, f64>(
        r#"
        SELECT COALESCE(SUM(COALESCE(u.wholesale_cost_usd, u.cost_usd)), 0)::float8
        FROM inference_usage u
        JOIN inference_pools p ON p.id = u.pool_id
        WHERE p.provider_id = $1 AND u.created_at >= $2
        "#,
    )
    .bind(provider)
    .bind(since)
    .fetch_one(db)
    .await;
    match result {
        Ok(spend) => spend,
        Err(error) => {
            tracing::warn!(%provider, %error, "upstream spend query failed; counting zero");
            0.0
        }
    }
}

/// Total customer wallet liability: every positive `user_credits` balance.
pub async fn wallet_liability(db: &PgPool) -> Result<f64, ApiError> {
    Ok(sqlx::query_scalar::<_, f64>(
        "SELECT COALESCE(SUM(balance_usd), 0)::float8 FROM user_credits WHERE balance_usd > 0",
    )
    .fetch_one(db)
    .await?)
}

/// Read OpenRouter's prepaid balance with the production key.
async fn fetch_openrouter_balance(http: &reqwest::Client) -> Result<f64, String> {
    let key = std::env::var("OPENROUTER_CREDITS_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("OPENROUTER_API_KEY").ok())
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "OPENROUTER_API_KEY is not set".to_string())?;
    let base = std::env::var("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_OPENROUTER_BASE_URL.to_string());
    let url = format!("{}/credits", base.trim_end_matches('/'));
    let response = http
        .get(url)
        .bearer_auth(key.trim())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("OpenRouter /credits returned {status}"));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("bad JSON: {e}"))?;
    openrouter_balance(&body).ok_or_else(|| "no total_credits/total_usage in response".to_string())
}

/// Refresh one provider: read or compute its balance, store it plus a history
/// row, update the router cache, alert when short. A provider error is
/// recorded in `last_error` with the last balance kept; only a database
/// error is returned.
pub async fn refresh_provider(
    db: &PgPool,
    http: &reqwest::Client,
    provider: &str,
) -> Result<FundingRow, ApiError> {
    ensure_row(db, provider).await?;
    let row = load_row(db, provider)
        .await?
        .ok_or_else(|| ApiError::Internal(format!("upstream_balances row for {provider} vanished")))?;

    let now = Utc::now();
    let burn_24h = spend_since(db, provider, now - chrono::Duration::hours(24)).await;

    let api_capable = provider == "openrouter" && std::env::var("OPENROUTER_API_KEY").is_ok();
    let (balance, source, error) = if api_capable {
        match fetch_openrouter_balance(http).await {
            Ok(balance) => (Some(balance), "api".to_string(), None),
            Err(error) => {
                tracing::warn!(%provider, %error, "upstream balance refresh failed; keeping last value");
                (row.balance_usd, row.source.clone(), Some(error))
            }
        }
    } else {
        match (row.manual_balance_usd, row.manual_set_at) {
            (Some(manual), Some(set_at)) => {
                let spent = spend_since(db, provider, set_at).await;
                (Some(manual_balance(manual, spent)), "manual".to_string(), None)
            }
            _ => (None, "manual".to_string(), None),
        }
    };

    sqlx::query(
        r#"
        UPDATE upstream_balances
        SET balance_usd = $2, source = $3, burn_24h_usd = $4, last_error = $5,
            refreshed_at = $6, updated_at = $6
        WHERE provider = $1
        "#,
    )
    .bind(provider)
    .bind(balance)
    .bind(&source)
    .bind(burn_24h)
    .bind(&error)
    .bind(now)
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO upstream_balance_history (provider, balance_usd, burn_24h_usd, source, error, recorded_at)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(provider)
    .bind(balance)
    .bind(burn_24h)
    .bind(&source)
    .bind(&error)
    .bind(now)
    .execute(db)
    .await?;

    let short = shortfall(balance, burn_24h, row.reserve_days, row.floor_usd);
    set_below_floor(provider, matches!(short, Some(Shortfall::BelowFloor { .. })));
    if let Some(short) = &short {
        alert_short(provider, short, burn_24h);
    }

    Ok(FundingRow {
        balance_usd: balance,
        source,
        burn_24h_usd: burn_24h,
        last_error: error,
        refreshed_at: Some(now),
        ..row
    })
}

fn alert_short(provider: &str, short: &Shortfall, burn_24h: f64) {
    let (subject, detail) = match short {
        Shortfall::BelowFloor { balance, floor } => (
            format!("Upstream funding: {provider} is below its floor (${balance:.2} < ${floor:.2})"),
            format!(
                "{provider} balance is ${balance:.2}, under its floor of ${floor:.2}. The router now skips {provider} for models another provider serves."
            ),
        ),
        Shortfall::LowRunway { days_left, reserve_days } => (
            format!("Upstream funding: {provider} has {days_left:.1} days left"),
            format!(
                "{provider} lasts about {days_left:.1} days at the last 24 hours' burn (${burn_24h:.2}/day); the reserve is {reserve_days} days."
            ),
        ),
    };
    let body = format!(
        "{detail}\n\nCheck the auto-recharge setting and the card in the {provider} dashboard, then update the balance if it is entered by hand:\nPUT /api/v1/admin/funding/{provider}\n\nAll providers: GET /api/v1/admin/funding"
    );
    crate::services::ops_alert::send_once(&format!("upstream-funding:{provider}"), subject, body);
}

/// Refresh every configured provider plus any provider with a stored row
/// (a manual entry for a provider whose key is not set here still shows).
pub async fn refresh_all(db: &PgPool, http: &reqwest::Client) {
    let mut providers: Vec<String> = configured_providers().into_iter().map(str::to_string).collect();
    match sqlx::query_scalar::<_, String>("SELECT provider FROM upstream_balances")
        .fetch_all(db)
        .await
    {
        Ok(stored) => {
            for provider in stored {
                if !providers.contains(&provider) {
                    providers.push(provider);
                }
            }
        }
        Err(error) => tracing::warn!(%error, "upstream_balances unreadable (migration 104 applied?)"),
    }
    for provider in providers {
        if let Err(error) = refresh_provider(db, http, &provider).await {
            tracing::error!(%provider, %error, "upstream funding refresh failed");
        }
    }
}

/// Start the 10-minute funding poller. Never panics or exits on errors.
pub fn start_upstream_funding_task(db: PgPool) {
    tokio::spawn(async move {
        let http = reqwest::Client::new();
        let mut interval = tokio::time::interval(REFRESH_EVERY);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            refresh_all(&db, &http).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_left_divides_balance_by_daily_burn() {
        assert_eq!(days_left(Some(300.0), 100.0), Some(3.0));
        assert_eq!(days_left(Some(50.0), 200.0), Some(0.25));
    }

    #[test]
    fn days_left_is_none_without_burn_or_balance() {
        assert_eq!(days_left(Some(300.0), 0.0), None);
        assert_eq!(days_left(None, 100.0), None);
    }

    #[test]
    fn negative_balance_has_zero_days_left() {
        assert_eq!(days_left(Some(-20.0), 10.0), Some(0.0));
    }

    #[test]
    fn coverage_ratio_is_upstream_over_liability() {
        assert_eq!(coverage_ratio(500.0, 250.0), Some(2.0));
        assert_eq!(coverage_ratio(100.0, 400.0), Some(0.25));
        assert_eq!(coverage_ratio(100.0, 0.0), None);
    }

    #[test]
    fn manual_balance_subtracts_spend_since_entry() {
        assert_eq!(manual_balance(100.0, 30.0), 70.0);
        assert_eq!(manual_balance(100.0, -5.0), 100.0);
        assert_eq!(manual_balance(10.0, 25.0), -15.0);
    }

    #[test]
    fn openrouter_balance_reads_credits_minus_usage() {
        let body = serde_json::json!({ "data": { "total_credits": 150.0, "total_usage": 42.5 } });
        assert_eq!(openrouter_balance(&body), Some(107.5));
        assert_eq!(openrouter_balance(&serde_json::json!({ "data": {} })), None);
    }

    #[test]
    fn shortfall_prefers_floor_then_runway() {
        assert_eq!(
            shortfall(Some(40.0), 10.0, 3, 50.0),
            Some(Shortfall::BelowFloor { balance: 40.0, floor: 50.0 })
        );
        assert_eq!(
            shortfall(Some(100.0), 50.0, 3, 20.0),
            Some(Shortfall::LowRunway { days_left: 2.0, reserve_days: 3 })
        );
        assert_eq!(shortfall(Some(1000.0), 50.0, 3, 20.0), None);
        assert_eq!(shortfall(None, 50.0, 3, 20.0), None);
        // No floor set (0) never trips the floor.
        assert_eq!(shortfall(Some(5.0), 0.0, 3, 0.0), None);
    }

    #[test]
    fn same_model_key_matches_across_providers() {
        assert_eq!(
            same_model_key("meta-llama/Llama-3.3-70B-Instruct-Turbo"),
            same_model_key("meta-llama/Llama-3.3-70B-Instruct-Turbo")
        );
        assert_eq!(
            same_model_key("qwen/qwen-2.5-72b-instruct"),
            same_model_key("Qwen/Qwen2.5-72B-Instruct")
        );
        assert_eq!(
            same_model_key("moonshotai/Kimi-K3"),
            same_model_key("accounts/fireworks/models/kimi-k3")
        );
        assert_ne!(
            same_model_key("meta-llama/llama-3.1-70b-instruct"),
            same_model_key("meta-llama/Llama-3.3-70B-Instruct-Turbo")
        );
    }

    #[test]
    fn below_floor_cache_round_trips() {
        set_below_floor("test-provider-cache", true);
        assert!(provider_below_floor("test-provider-cache"));
        set_below_floor("test-provider-cache", false);
        assert!(!provider_below_floor("test-provider-cache"));
    }
}
