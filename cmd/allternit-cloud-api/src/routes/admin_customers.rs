//! `GET /api/v1/admin/customers` — everyone who signed up (Clerk) with their
//! plan (Stripe mirror), cloud computer and the emails we sent them. Admin
//! only (`ALLTERNIT_ADMIN_USER_IDS`). Backs the platform Customers page.
//!
//! Also the daily summary emailed to the team (see
//! [`start_daily_customer_summary_task`]).

use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{error::ApiError, services::customer_emails::recipient_from_clerk_user, ApiState};

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/admin/customers", get(list_customers))
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CustomerRow {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub signed_up_at: Option<DateTime<Utc>>,
    /// plus / super / ultra, or None when the account has no paid plan.
    pub plan_id: Option<String>,
    /// Stripe status mirror: active, trialing, past_due, unpaid, canceled…
    pub plan_status: Option<String>,
    pub stripe_customer_id: Option<String>,
    pub computer_status: Option<String>,
    pub emails_sent: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomersResponse {
    pub total_signups: usize,
    pub paying: usize,
    pub customers: Vec<CustomerRow>,
}

/// Every Clerk user, newest first (paged, at most 2,000).
async fn clerk_users() -> Result<Vec<serde_json::Value>, ApiError> {
    let config = crate::services::user_trust::TrustConfig::from_env();
    let Some(secret) = config.clerk_secret_key.as_deref() else {
        return Err(ApiError::ServiceUnavailable("CLERK_SECRET_KEY is not set".to_string()));
    };
    let client = reqwest::Client::new();
    let mut users = Vec::new();
    for page in 0..4 {
        let url = format!(
            "{}/v1/users?limit=500&offset={}&order_by=-created_at",
            config.clerk_api_base.trim_end_matches('/'),
            page * 500
        );
        let batch: Vec<serde_json::Value> = client
            .get(url)
            .bearer_auth(secret)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|error| ApiError::ServiceUnavailable(format!("Clerk: {error}")))?
            .json()
            .await
            .map_err(|error| ApiError::ServiceUnavailable(format!("Clerk: {error}")))?;
        let done = batch.len() < 500;
        users.extend(batch);
        if done {
            break;
        }
    }
    Ok(users)
}

pub async fn customer_rows(db: &PgPool) -> Result<Vec<CustomerRow>, ApiError> {
    let users = clerk_users().await?;

    // Newest subscription per user (open ones first).
    let subscriptions: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (user_id) user_id, plan_id, status, stripe_customer_id
        FROM billing_subscriptions
        WHERE stripe_subscription_id LIKE 'sub\_%'
        ORDER BY user_id, (status IN ('active', 'trialing', 'past_due', 'unpaid')) DESC, updated_at DESC
        "#,
    )
    .fetch_all(db)
    .await?;
    let subscriptions: HashMap<String, (String, String, Option<String>)> = subscriptions
        .into_iter()
        .map(|(user, plan, status, customer)| (user, (plan, status, customer)))
        .collect();

    let computers: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (user_id) user_id, status
        FROM provisioned_instances
        WHERE tier = 'paid' AND status <> 'deleted' AND replaced_by IS NULL
        ORDER BY user_id, created_at DESC
        "#,
    )
    .fetch_all(db)
    .await?;
    let computers: HashMap<String, String> = computers.into_iter().collect();

    let emails: Vec<(String, String)> = sqlx::query_as(
        "SELECT user_id, kind FROM customer_emails WHERE status = 'sent' ORDER BY created_at",
    )
    .fetch_all(db)
    .await?;
    let mut emails_by_user: HashMap<String, Vec<String>> = HashMap::new();
    for (user, kind) in emails {
        emails_by_user.entry(user).or_default().push(kind);
    }

    Ok(users
        .iter()
        .filter_map(|user| {
            let user_id = user["id"].as_str()?.to_string();
            let recipient = recipient_from_clerk_user(user);
            let name = [user["first_name"].as_str(), user["last_name"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            let subscription = subscriptions.get(&user_id);
            Some(CustomerRow {
                email: recipient.map(|r| r.email),
                name: (!name.trim().is_empty()).then_some(name),
                signed_up_at: user["created_at"].as_i64().and_then(|ms| Utc.timestamp_millis_opt(ms).single()),
                plan_id: subscription.map(|(plan, _, _)| plan.clone()),
                plan_status: subscription.map(|(_, status, _)| status.clone()),
                stripe_customer_id: subscription.and_then(|(_, _, customer)| customer.clone()),
                computer_status: computers.get(&user_id).cloned(),
                emails_sent: emails_by_user.remove(&user_id).unwrap_or_default(),
                user_id,
            })
        })
        .collect())
}

fn is_paying(row: &CustomerRow) -> bool {
    matches!(row.plan_status.as_deref(), Some("active" | "trialing" | "past_due"))
}

async fn list_customers(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<CustomersResponse>, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "account").await?;
    if !crate::auth::is_admin_user(&user.id) {
        return Err(ApiError::Forbidden("Admins only".to_string()));
    }
    let customers = customer_rows(&state.db).await?;
    Ok(Json(CustomersResponse {
        total_signups: customers.len(),
        paying: customers.iter().filter(|row| is_paying(row)).count(),
        customers,
    }))
}

/// The daily summary text for the 24 hours before `now`.
pub fn summary_text(rows: &[CustomerRow], cancelled_24h: &[(String, String)], now: DateTime<Utc>) -> (String, String) {
    let since = now - Duration::hours(24);
    let new_signups: Vec<&CustomerRow> = rows.iter().filter(|row| row.signed_up_at.is_some_and(|at| at >= since)).collect();
    let paying: Vec<&CustomerRow> = rows.iter().filter(|row| is_paying(row)).collect();
    let mut by_plan: HashMap<&str, usize> = HashMap::new();
    for row in &paying {
        *by_plan.entry(row.plan_id.as_deref().unwrap_or("?")).or_default() += 1;
    }
    let payment_problems = rows
        .iter()
        .filter(|row| matches!(row.plan_status.as_deref(), Some("past_due" | "unpaid")))
        .count();
    let waiting = paying.iter().filter(|row| row.computer_status.is_none()).count();

    let label = |row: &CustomerRow| row.email.clone().unwrap_or_else(|| row.user_id.clone());
    let mut lines = vec![format!("Allternit, last 24 hours (to {} UTC)", now.format("%Y-%m-%d %H:%M")), String::new()];
    lines.push(format!("New sign-ups: {}", new_signups.len()));
    lines.extend(new_signups.iter().map(|row| format!("  - {}", label(row))));
    lines.push(format!("Cancelled plans: {}", cancelled_24h.len()));
    lines.extend(cancelled_24h.iter().map(|(user, plan)| format!("  - {user} ({plan})")));
    lines.push(String::new());
    lines.push(format!("Total sign-ups: {}", rows.len()));
    lines.push(format!(
        "Paying: {} (Plus {}, Super {}, Ultra {})",
        paying.len(),
        by_plan.get("plus").copied().unwrap_or(0),
        by_plan.get("super").copied().unwrap_or(0),
        by_plan.get("ultra").copied().unwrap_or(0),
    ));
    lines.push(format!("Payment problems (past due / unpaid): {payment_problems}"));
    lines.push(format!("Paying without a cloud computer yet: {waiting}"));
    lines.push(String::new());
    lines.push("All customers: https://platform.allternit.com/admin/customers".to_string());
    let subject = format!(
        "Allternit daily: {} new sign-up{}, {} paying",
        new_signups.len(),
        if new_signups.len() == 1 { "" } else { "s" },
        paying.len()
    );
    (subject, lines.join("\n"))
}

/// Hour (UTC) the daily summary goes out: 13:00 UTC = 8:00 Central (CDT).
const SUMMARY_HOUR_UTC: u32 = 13;

/// Emails the team a daily customer summary at [`SUMMARY_HOUR_UTC`].
pub fn start_daily_customer_summary_task(state: Arc<ApiState>) {
    tokio::spawn(async move {
        loop {
            let now = Utc::now();
            let mut next = now
                .with_hour(SUMMARY_HOUR_UTC)
                .and_then(|t| t.with_minute(0))
                .and_then(|t| t.with_second(0))
                .unwrap_or(now);
            if next <= now {
                next += Duration::days(1);
            }
            let wait = (next - now).to_std().unwrap_or(std::time::Duration::from_secs(3600));
            tokio::time::sleep(wait).await;
            if let Err(error) = send_daily_summary(&state.db).await {
                tracing::error!("daily customer summary failed: {}", error);
            }
        }
    });
}

async fn send_daily_summary(db: &PgPool) -> Result<(), ApiError> {
    let rows = customer_rows(db).await?;
    let cancelled: Vec<(String, String)> = sqlx::query_as(
        r#"
        SELECT user_id, plan_id FROM billing_subscriptions
        WHERE status = 'canceled' AND updated_at >= now() - interval '24 hours'
          AND stripe_subscription_id LIKE 'sub\_%'
        "#,
    )
    .fetch_all(db)
    .await?;
    let emails: HashMap<String, String> = rows
        .iter()
        .filter_map(|row| Some((row.user_id.clone(), row.email.clone()?)))
        .collect();
    let cancelled: Vec<(String, String)> = cancelled
        .into_iter()
        .map(|(user, plan)| (emails.get(&user).cloned().unwrap_or(user), plan))
        .collect();
    let now = Utc::now();
    let (subject, body) = summary_text(&rows, &cancelled, now);
    crate::services::ops_alert::send_once(&format!("daily:{}", now.format("%Y-%m-%d")), subject, body);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(email: &str, hours_ago: i64, plan: Option<(&str, &str)>, computer: Option<&str>, now: DateTime<Utc>) -> CustomerRow {
        CustomerRow {
            user_id: format!("user_{email}"),
            email: Some(email.to_string()),
            name: None,
            signed_up_at: Some(now - Duration::hours(hours_ago)),
            plan_id: plan.map(|(p, _)| p.to_string()),
            plan_status: plan.map(|(_, s)| s.to_string()),
            stripe_customer_id: None,
            computer_status: computer.map(str::to_string),
            emails_sent: vec![],
        }
    }

    #[test]
    fn daily_summary_counts_the_right_people() {
        let now = Utc.with_ymd_and_hms(2026, 10, 2, 13, 0, 0).unwrap();
        let rows = vec![
            row("new@x.com", 2, None, None, now),
            row("buyer@x.com", 30, Some(("plus", "active")), Some("running"), now),
            row("late@x.com", 100, Some(("super", "past_due")), Some("running"), now),
            row("waiting@x.com", 5, Some(("ultra", "active")), None, now),
        ];
        let (subject, body) = summary_text(&rows, &[("gone@x.com".into(), "plus".into())], now);
        assert_eq!(subject, "Allternit daily: 2 new sign-ups, 3 paying");
        assert!(body.contains("New sign-ups: 2\n  - new@x.com\n  - waiting@x.com"));
        assert!(body.contains("Cancelled plans: 1\n  - gone@x.com (plus)"));
        assert!(body.contains("Paying: 3 (Plus 1, Super 1, Ultra 1)"));
        assert!(body.contains("Payment problems (past due / unpaid): 1"));
        assert!(body.contains("Paying without a cloud computer yet: 1"));
    }
}
