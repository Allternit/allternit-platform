//! Per-user/org desktop quotas and usage tracking.
//!
//! Gated at provision time and reconciled at deprovision time.

use chrono::{DateTime, Datelike, Utc};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::sync::Arc;
use tracing::{info, warn};

use crate::auth::AuthUser;
use crate::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct QuotaCheck {
    pub allowed: bool,
    pub active: i64,
    pub active_limit: Option<i64>,
    pub monthly_minutes: i64,
    pub monthly_limit: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuotaLimits {
    pub max_concurrent: Option<i64>,
    pub max_monthly_minutes: Option<i64>,
}

/// Reason a quota check failed.
#[derive(Debug, Clone)]
pub enum QuotaError {
    ConcurrentLimit { active: i64, limit: i64 },
    MonthlyMinutesLimit { used: i64, limit: i64 },
}

impl std::fmt::Display for QuotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuotaError::ConcurrentLimit { active, limit } => {
                write!(f, "concurrent desktop limit reached ({}/{})", active, limit)
            }
            QuotaError::MonthlyMinutesLimit { used, limit } => {
                write!(f, "monthly desktop minutes limit reached ({}/{})", used, limit)
            }
        }
    }
}

impl QuotaError {
    pub fn to_check(&self) -> QuotaCheck {
        match self {
            QuotaError::ConcurrentLimit { active, limit } => QuotaCheck {
                allowed: false,
                active: *active,
                active_limit: Some(*limit),
                monthly_minutes: 0,
                monthly_limit: None,
                reason: Some(self.to_string()),
            },
            QuotaError::MonthlyMinutesLimit { used, limit } => QuotaCheck {
                allowed: false,
                active: 0,
                active_limit: None,
                monthly_minutes: *used,
                monthly_limit: Some(*limit),
                reason: Some(self.to_string()),
            },
        }
    }
}

/// Snapshot of a user's quota limits and current usage.
async fn load_snapshot(
    state: &Arc<AppState>,
    user: &AuthUser,
) -> Result<(QuotaLimits, i64, i64), String> {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| format!("db: {}", e))?;

        // Load user quota; fall back to org quota if present.
        let mut limits = QuotaLimits {
            max_concurrent: None,
            max_monthly_minutes: None,
        };
        if let Some(row) = conn
            .query_row(
                "SELECT max_concurrent, max_monthly_minutes FROM desktop_quotas WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| {
                    Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            limits.max_concurrent = row.0;
            limits.max_monthly_minutes = row.1;
        } else if let Some(org) = org_id.as_ref() {
            if let Some(row) = conn
                .query_row(
                    "SELECT max_concurrent, max_monthly_minutes FROM desktop_quotas WHERE org_id = ?1 AND user_id IS NULL",
                    rusqlite::params![org],
                    |row| {
                        Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
                    },
                )
                .optional()
                .map_err(|e| e.to_string())?
            {
                limits.max_concurrent = row.0;
                limits.max_monthly_minutes = row.1;
            }
        }

        // Active desktops for this user.
        let active: i64 = conn.query_row(
            "SELECT COUNT(*) FROM desktop_usage WHERE user_id = ?1 AND ended_at IS NULL",
            rusqlite::params![user_id],
            |row| row.get(0),
        ).map_err(|e| e.to_string())?;

        // Monthly minutes for this user (current calendar month).
        let now = Utc::now();
        let start_of_month = now
            .date_naive()
            .with_day(1)
            .unwrap_or_else(|| now.date_naive())
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let monthly_minutes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(minutes), 0) FROM desktop_usage \
             WHERE user_id = ?1 AND started_at >= ?2",
            rusqlite::params![user_id, start_of_month.to_string()],
            |row| row.get(0),
        ).map_err(|e| e.to_string())?;

        Ok::<_, String>((limits, active, monthly_minutes))
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(e) => Err(format!("task panicked checking desktop quota: {}", e)),
    }
}

/// Check whether the user/org may provision another desktop.
pub async fn check_quota(state: &Arc<AppState>, user: &AuthUser) -> Result<QuotaCheck, QuotaError> {
    let (limits, active, monthly_minutes) = match load_snapshot(state, user).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to check desktop quota; allowing provision");
            return Ok(QuotaCheck {
                allowed: true,
                active: 0,
                active_limit: None,
                monthly_minutes: 0,
                monthly_limit: None,
                reason: None,
            });
        }
    };

    if let Some(limit) = limits.max_concurrent {
        if active >= limit {
            return Err(QuotaError::ConcurrentLimit { active, limit });
        }
    }

    if let Some(limit) = limits.max_monthly_minutes {
        if monthly_minutes >= limit {
            return Err(QuotaError::MonthlyMinutesLimit {
                used: monthly_minutes,
                limit,
            });
        }
    }

    Ok(QuotaCheck {
        allowed: true,
        active,
        active_limit: limits.max_concurrent,
        monthly_minutes,
        monthly_limit: limits.max_monthly_minutes,
        reason: None,
    })
}

/// Read-only quota status for UI surfaces (e.g. the Create Bot computer
/// step). Never fails: on a database error it reports an unconstrained
/// quota, mirroring check_quota's fail-open provision policy.
pub async fn quota_status(state: &Arc<AppState>, user: &AuthUser) -> QuotaCheck {
    let (limits, active, monthly_minutes) = match load_snapshot(state, user).await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to load desktop quota status");
            return QuotaCheck {
                allowed: true,
                active: 0,
                active_limit: None,
                monthly_minutes: 0,
                monthly_limit: None,
                reason: None,
            };
        }
    };

    let under_concurrent = limits.max_concurrent.map_or(true, |l| active < l);
    let under_monthly = limits
        .max_monthly_minutes
        .map_or(true, |l| monthly_minutes < l);
    let allowed = under_concurrent && under_monthly;
    let reason = if allowed {
        None
    } else if !under_concurrent {
        Some(format!(
            "concurrent desktop limit reached ({}/{})",
            active,
            limits.max_concurrent.unwrap_or(active)
        ))
    } else {
        Some(format!(
            "monthly desktop minutes limit reached ({}/{})",
            monthly_minutes,
            limits.max_monthly_minutes.unwrap_or(monthly_minutes)
        ))
    };

    QuotaCheck {
        allowed,
        active,
        active_limit: limits.max_concurrent,
        monthly_minutes,
        monthly_limit: limits.max_monthly_minutes,
        reason,
    }
}

/// Record that a desktop started.
pub async fn record_start(
    state: &Arc<AppState>,
    user: &AuthUser,
    bot_id: &str,
    sandbox_id: &str,
    provider: &str,
    os: &str,
) {
    let db = state.db.clone();
    let user_id = user.user_id.clone();
    let org_id = user.organization_id.clone();
    let bot_id = bot_id.to_string();
    let sandbox_id = sandbox_id.to_string();
    let provider = provider.to_string();
    let os = os.to_string();
    let started_at = Utc::now();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "INSERT INTO desktop_usage (user_id, org_id, bot_id, sandbox_id, provider, os, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                user_id,
                org_id,
                bot_id,
                sandbox_id,
                provider,
                os,
                started_at.to_rfc3339(),
            ],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await;

    match result {
        Err(e) => warn!(error = %e, "task panicked recording desktop usage start"),
        Ok(Err(e)) => warn!(error = %e, "failed to record desktop usage start"),
        Ok(Ok(())) => {}
    }
}

/// Record that a desktop ended and compute minutes used. Writes a unified
/// `usage_events` row for the credit ledger while keeping the legacy
/// `desktop_usage` row for backward compatibility. When Stripe is configured,
/// emit a usage record for the consumed minutes.
pub async fn record_end(state: &Arc<AppState>, bot_id: &str) {
    let db = state.db.clone();
    let bot_id = bot_id.to_string();
    let ended_at = Utc::now();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        // Find the active usage row for this bot.
        let row: Option<(i64, String, String, String, Option<String>, String)> = conn
            .query_row(
                "SELECT id, started_at, provider, os, org_id, sandbox_id \
                 FROM desktop_usage WHERE bot_id = ?1 AND ended_at IS NULL ORDER BY id DESC LIMIT 1",
                rusqlite::params![bot_id],
                |row| {
                    let id: i64 = row.get(0)?;
                    let started: String = row.get(1)?;
                    let provider: String = row.get(2)?;
                    let os: String = row.get(3)?;
                    let org_id: Option<String> = row.get(4)?;
                    let sandbox_id: String = row.get(5)?;
                    Ok((id, started, provider, os, org_id, sandbox_id))
                },
            )
            .optional()?;

        if let Some((id, started, provider, os, org_id, sandbox_id)) = row {
            let started_at = DateTime::parse_from_rfc3339(&started)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| ended_at);
            let minutes = (ended_at - started_at).num_seconds().max(0) / 60;
            conn.execute(
                "UPDATE desktop_usage SET ended_at = ?1, minutes = ?2 WHERE id = ?3",
                rusqlite::params![ended_at.to_rfc3339(), minutes, id],
            )?;

            // Look up the computer's memory so pricing can scale by size/OS.
            let memory_mb: Option<i64> = conn
                .query_row(
                    "SELECT memory_mb FROM computers WHERE native_id = ?1 LIMIT 1",
                    [&sandbox_id],
                    |r| r.get(0),
                )
                .optional()?;

            // Emit a unified usage event for the credit ledger.
            if minutes > 0 {
                if let Some(ref org) = org_id {
                    let cost_cents = crate::pricing::compute_computer_minute_cost_cents(memory_mb, Some(&os), minutes as f64);
                    let event_id = uuid::Uuid::new_v4().to_string();
                    let idempotency_key = format!("desktop:{}:{}", sandbox_id, started);
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO usage_events \
                         (id, organization_id, environment_id, resource_type, quantity, unit, provider, started_at, ended_at, computed_cost_cents, idempotency_key) \
                         VALUES (?1, ?2, ?3, 'computer_minute', ?4, 'minutes', ?5, ?6, ?7, ?8, ?9)",
                        rusqlite::params![
                            event_id,
                            org,
                            sandbox_id,
                            minutes as f64,
                            provider,
                            started_at.to_rfc3339(),
                            ended_at.to_rfc3339(),
                            cost_cents,
                            idempotency_key,
                        ],
                    );

                    // Deduct the actual cost from the organization's pre-paid credits.
                    // A failure here is logged but does not block the deprovision flow;
                    // the usage_event remains the durable record.
                    if cost_cents > 0 {
                        let description = format!("desktop usage {} minutes ({} {} MB)", minutes, os, memory_mb.unwrap_or(4096));
                        if let Err(e) = crate::credits::consume(&db, org, cost_cents, &description, Some(&idempotency_key)) {
                            warn!(org_id = %org, error = %e, cost_cents, "failed to deduct credits for desktop usage");
                        }
                    }
                }
            }

            Ok::<_, rusqlite::Error>(Some((minutes, provider, os)))
        } else {
            Ok(None)
        }
    })
    .await;

    match result {
        Err(e) => warn!(error = %e, "task panicked recording desktop usage end"),
        Ok(Err(e)) => warn!(error = %e, "failed to record desktop usage end"),
        Ok(Ok(Some((minutes, provider, os)))) => {
            emit_stripe_usage_record(minutes, &provider, &os).await;
        }
        Ok(Ok(None)) => {}
    }
}

async fn emit_stripe_usage_record(minutes: i64, provider: &str, os: &str) {
    let secret = match std::env::var("STRIPE_SECRET_KEY").ok().filter(|s| !s.is_empty()) {
        Some(s) => s,
        None => return,
    };
    let item = match std::env::var("STRIPE_DESKTOP_USAGE_SUBSCRIPTION_ITEM")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(s) => s,
        None => return,
    };

    let client = crate::net_client::client();
    let params = [
        ("quantity", minutes.to_string()),
        ("timestamp", Utc::now().timestamp().to_string()),
        ("action", "increment".to_string()),
    ];
    match client
        .post(&format!(
            "https://api.stripe.com/v1/subscription_items/{}/usage_records",
            item
        ))
        .basic_auth(&secret, Some(""))
        .form(&params)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!(minutes, provider, os, "Stripe desktop usage record created");
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Stripe desktop usage record failed");
        }
        Err(e) => warn!(error = %e, "Stripe desktop usage record request failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use crate::test_helpers::app_state;

    fn test_user(user_id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn test_state() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!(
            "allternit-desktop-quotas-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        app_state(&dir).await
    }

    async fn set_user_quota(state: &AppState, user_id: &str, max_concurrent: i64) {
        let db = state.db.clone();
        let user_id = user_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO desktop_quotas (user_id, max_concurrent, max_monthly_minutes) VALUES (?1, ?2, ?3)",
                rusqlite::params![user_id, max_concurrent, 10000i64],
            )
            .unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn allows_provision_when_no_quota_set() {
        let state = test_state().await;
        let user = test_user("free-user", None);
        let check = check_quota(&state, &user).await.unwrap();
        assert!(check.allowed);
    }

    #[tokio::test]
    async fn enforces_concurrent_limit() {
        let state = test_state().await;
        let user = test_user("limited-user", None);
        set_user_quota(&state, &user.user_id, 1).await;

        record_start(&state, &user, "bot-1", "sb-1", "incus", "linux").await;
        let check = check_quota(&state, &user).await;
        assert!(matches!(check, Err(QuotaError::ConcurrentLimit { active: 1, limit: 1 })));

        record_end(&state, "bot-1").await;
        let check = check_quota(&state, &user).await.unwrap();
        assert!(check.allowed);
        assert_eq!(check.active, 0);
    }

    #[tokio::test]
    async fn quota_status_reports_usage_and_limits() {
        let state = test_state().await;
        let user = test_user("status-user", None);
        set_user_quota(&state, &user.user_id, 2).await;

        let status = quota_status(&state, &user).await;
        assert!(status.allowed);
        assert_eq!(status.active, 0);
        assert_eq!(status.active_limit, Some(2));
        assert_eq!(status.monthly_limit, Some(10000));

        record_start(&state, &user, "bot-s1", "sb-s1", "incus", "linux").await;
        record_start(&state, &user, "bot-s2", "sb-s2", "incus", "linux").await;
        let status = quota_status(&state, &user).await;
        assert!(!status.allowed);
        assert_eq!(status.active, 2);
        assert!(status.reason.unwrap().contains("concurrent desktop limit"));
    }

    #[tokio::test]
    async fn record_end_computes_minutes() {
        let state = test_state().await;
        let user = test_user("usage-user", None);
        record_start(&state, &user, "bot-2", "sb-2", "tart", "macos").await;
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect().unwrap();
            let start = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
            conn.execute(
                "UPDATE desktop_usage SET started_at = ?1 WHERE bot_id = 'bot-2'",
                rusqlite::params![start],
            )
            .unwrap();
        })
        .await
        .unwrap();
        record_end(&state, "bot-2").await;

        let db = state.db.clone();
        let minutes: i64 = tokio::task::spawn_blocking(move || {
            let conn = db.connect().unwrap();
            conn.query_row(
                "SELECT minutes FROM desktop_usage WHERE bot_id = 'bot-2'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap();
        assert!(minutes >= 4 && minutes <= 6);
    }
}
