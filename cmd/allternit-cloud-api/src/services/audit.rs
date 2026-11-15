//! Best-effort audit log writes.
//!
//! The `audit_log` table (migrations_pg/001_initial.sql) previously had zero
//! writers — security-relevant events (API key lifecycle, Clerk account
//! deletions, billing grants, device pairing) were invisible to compliance
//! review. Writes here are synchronous-with-request and best-effort: a failed
//! audit insert is logged with `tracing::warn!`, never surfaced as an error
//! to the caller (a webhook must still 2xx, a key creation must still return
//! its one-time token).

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// A single audit log entry. Fields mirror the `audit_log` table columns.
pub struct AuditEvent {
    /// Dotted action name, e.g. `api_key.create`, `user.deleted`,
    /// `billing.subscription.granted`, `device_pairing.token_issued`.
    pub action: String,
    /// Table-ish resource kind, e.g. `api_key`, `user`, `billing_entitlement`.
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub user_id: Option<String>,
    pub user_email: Option<String>,
    /// Structured details; serialized to the `details` text column as JSON.
    pub details: Option<Value>,
    pub success: bool,
}

/// Insert an entry into `audit_log`. Never fails the caller.
pub async fn write_audit_log(pool: &PgPool, event: AuditEvent) {
    // The `details` column is `text` (not jsonb), so serialize manually.
    let details = event.details.map(|value| value.to_string());
    let result = sqlx::query(
        r#"
        INSERT INTO audit_log (id, action, resource_type, resource_id, user_id, user_email, details, success)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(format!("al_{}", Uuid::new_v4()))
    .bind(&event.action)
    .bind(&event.resource_type)
    .bind(event.resource_id.as_deref())
    .bind(event.user_id.as_deref())
    .bind(event.user_email.as_deref())
    .bind(details)
    .bind(event.success)
    .execute(pool)
    .await;

    if let Err(error) = result {
        tracing::warn!(action = %event.action, %error, "failed to write audit_log entry");
    }
}

/// Who an entry belongs to for the console audit page
/// (`GET /api/v1/platform/audit-log`, migrations_pg/122): the Clerk org and
/// the project it concerns, plus the caller's trusted IP when known.
#[derive(Debug, Clone, Default)]
pub struct AuditScope {
    pub organization_id: Option<String>,
    pub project_id: Option<String>,
    pub ip_address: Option<String>,
}

/// [`write_audit_log`] with the org, project and IP columns filled. Never
/// fails the caller.
pub async fn write_scoped_audit_log(pool: &PgPool, event: AuditEvent, scope: AuditScope) {
    let details = event.details.map(|value| value.to_string());
    let result = sqlx::query(
        r#"
        INSERT INTO audit_log (id, action, resource_type, resource_id, user_id, user_email, details, success,
                               organization_id, project_id, ip_address)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
    )
    .bind(format!("al_{}", Uuid::new_v4()))
    .bind(&event.action)
    .bind(&event.resource_type)
    .bind(event.resource_id.as_deref())
    .bind(event.user_id.as_deref())
    .bind(event.user_email.as_deref())
    .bind(details)
    .bind(event.success)
    .bind(scope.organization_id.as_deref())
    .bind(scope.project_id.as_deref())
    .bind(scope.ip_address.as_deref())
    .execute(pool)
    .await;

    if let Err(error) = result {
        tracing::warn!(action = %event.action, %error, "failed to write audit_log entry");
    }
}
