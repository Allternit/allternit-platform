//! Spend caps (spec §6, §7): every project has a monthly cap
//! (`platform_projects.spend_cap_cents`, $100 by default). Anything that starts
//! billable work it can't take back (a phone call, a realtime session) asks
//! [`spend_allowed`] first and refuses with [`cap_reached`] when it says no.
//!
//! The check itself belongs to billing (P5): it compares the month's priced
//! usage with the cap. Until billing prices usage, nothing has a cost to
//! compare, so every project with a cap row is allowed. Callers depend only on
//! this signature.

use sqlx::PgPool;

use super::PlatformError;

/// May this project start more billable work this month?
pub async fn spend_allowed(db: &PgPool, project_id: &str) -> Result<bool, PlatformError> {
    let cap: Option<(i64,)> = sqlx::query_as("SELECT spend_cap_cents FROM platform_projects WHERE id = $1 AND archived_at IS NULL")
        .bind(project_id)
        .fetch_optional(db)
        .await?;
    // A project without a row can't start anything; one with a cap is allowed
    // until billing (P5) prices its usage against the cap.
    Ok(cap.is_some())
}

/// 402 `spend_cap_reached`.
pub fn cap_reached() -> PlatformError {
    PlatformError {
        status: axum::http::StatusCode::PAYMENT_REQUIRED,
        kind: "invalid_request_error",
        code: "spend_cap_reached".into(),
        message: "This project reached its monthly spend cap. Raise the cap in the console to continue.".into(),
        param: None,
    }
}
