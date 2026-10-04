//! `record_usage`: the one way billable usage enters `platform_usage_events`.

use sqlx::PgPool;

use super::PlatformError;

/// Exact meter strings (spec §7 / CONTRACTS).
pub const METERS: [&str; 11] = [
    "voice_min_allternit",
    "voice_min_byok",
    "agent_month",
    "tokens_in",
    "tokens_out",
    "number_local_month",
    "number_tollfree_month",
    "sms_segment",
    "mms",
    "registration_passthrough_cents",
    "recording_min_month",
];

#[derive(Debug, Clone)]
pub struct UsageEvent {
    pub project_id: String,
    pub account_id: Option<String>,
    pub key_id: Option<String>,
    pub meter: String,
    pub quantity: f64,
    pub unit: Option<String>,
    pub ref_id: Option<String>,
    /// Unique per project: a repeat with the same value records nothing.
    pub idempotency: Option<String>,
}

/// Insert one usage event. Returns the new event id, or `None` when
/// `idempotency` was already recorded for this project (a safe retry).
pub async fn record_usage(db: &PgPool, event: UsageEvent) -> Result<Option<String>, PlatformError> {
    if !METERS.contains(&event.meter.as_str()) {
        return Err(PlatformError::invalid_request(
            "unknown_meter",
            format!("'{}' is not a usage meter.", event.meter),
        ));
    }
    if !event.quantity.is_finite() || event.quantity < 0.0 {
        return Err(PlatformError::invalid_request(
            "invalid_quantity",
            "Usage quantity must be a non-negative number.",
        ));
    }
    let id = format!("use_{}", hex::encode(rand::random::<[u8; 12]>()));
    let inserted: Option<String> = sqlx::query_scalar(
        r#"
        INSERT INTO platform_usage_events
            (id, project_id, account_id, key_id, meter, quantity, unit, ref_id, idempotency)
        VALUES ($1, $2, $3, $4, $5, ($6::float8)::numeric, $7, $8, $9)
        ON CONFLICT (project_id, idempotency) WHERE idempotency IS NOT NULL DO NOTHING
        RETURNING id
        "#,
    )
    .bind(&id)
    .bind(&event.project_id)
    .bind(&event.account_id)
    .bind(&event.key_id)
    .bind(&event.meter)
    .bind(event.quantity)
    .bind(&event.unit)
    .bind(&event.ref_id)
    .bind(&event.idempotency)
    .fetch_optional(db)
    .await?;
    Ok(inserted)
}
