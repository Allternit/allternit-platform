//! Concurrency slots (live calls today; other kinds later).
//!
//! `acquire_slot(db, project_id, "call")` takes one slot or fails with a 429
//! `concurrency_limit`. The per-project cap comes from the plan (sandbox 1,
//! payg 5, growth 25, enterprise 100) unless the project has a
//! `call_cap_override`; the machine-wide cap is `ALLTERNIT_PLATFORM_MAX_CALLS`
//! (default 10). Slots expire on their own (`ALLTERNIT_PLATFORM_SLOT_TTL_SECS`,
//! default 2 h) so a crashed call can't pin capacity; `sweep_expired_slots`
//! also removes them. No public route yet: P3 voice calls this.

use sqlx::PgPool;

use super::{caller::Plan, PlatformError};

pub const KIND_CALL: &str = "call";
const DEFAULT_GLOBAL_CAP: i64 = 10;
const DEFAULT_TTL_SECS: i64 = 2 * 60 * 60;

fn global_cap() -> i64 {
    std::env::var("ALLTERNIT_PLATFORM_MAX_CALLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_GLOBAL_CAP)
}

fn ttl_secs() -> i64 {
    std::env::var("ALLTERNIT_PLATFORM_SLOT_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_TTL_SECS)
}

/// A held slot. Call [`SlotGuard::release`] when the call ends; if the guard is
/// dropped without that, it releases in the background.
pub struct SlotGuard {
    db: PgPool,
    project_id: String,
    slot_id: String,
    released: bool,
}

impl SlotGuard {
    pub fn slot_id(&self) -> &str {
        &self.slot_id
    }

    pub async fn release(mut self) -> Result<(), PlatformError> {
        self.released = true;
        release_slot(&self.db, &self.project_id, &self.slot_id).await
    }

    /// Give up ownership without releasing (the slot then lives until
    /// `release_slot` or expiry). Use when the call's lifetime outlives this
    /// request and a later handler releases by id.
    pub fn detach(mut self) -> String {
        self.released = true;
        self.slot_id.clone()
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let (db, project_id, slot_id) =
                (self.db.clone(), self.project_id.clone(), self.slot_id.clone());
            handle.spawn(async move {
                let _ = release_slot(&db, &project_id, &slot_id).await;
            });
        }
    }
}

pub async fn acquire_slot(db: &PgPool, project_id: &str, kind: &str) -> Result<SlotGuard, PlatformError> {
    acquire_slot_with_ttl(db, project_id, kind, ttl_secs()).await
}

pub async fn acquire_slot_with_ttl(
    db: &PgPool,
    project_id: &str,
    kind: &str,
    ttl_secs: i64,
) -> Result<SlotGuard, PlatformError> {
    let mut tx = db.begin().await?;
    // Serialize acquirers so the count-then-insert below can't oversubscribe.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('platform_call_slots'))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM platform_call_slots WHERE expires_at <= NOW()")
        .execute(&mut *tx)
        .await?;

    let project: Option<(String, Option<i32>)> = sqlx::query_as(
        "SELECT plan, call_cap_override FROM platform_projects WHERE id = $1 AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (plan, override_cap) = project
        .ok_or_else(|| PlatformError::not_found("project_not_found", "The project does not exist."))?;
    let project_cap = match override_cap {
        Some(n) if n > 0 => n as i64,
        _ => Plan::parse(&plan).call_cap() as i64,
    };

    let in_project: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM platform_call_slots WHERE project_id = $1 AND kind = $2",
    )
    .bind(project_id)
    .bind(kind)
    .fetch_one(&mut *tx)
    .await?;
    if in_project >= project_cap {
        return Err(PlatformError::rate_limit(
            "concurrency_limit",
            format!("This project is at its limit of {project_cap} concurrent {kind}s."),
        ));
    }
    let global: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_call_slots WHERE kind = $1")
        .bind(kind)
        .fetch_one(&mut *tx)
        .await?;
    if global >= global_cap() {
        return Err(PlatformError::rate_limit(
            "capacity_limit",
            format!("Allternit is at capacity for concurrent {kind}s right now. Retry shortly."),
        ));
    }

    let slot_id = format!("slot_{}", hex::encode(rand::random::<[u8; 12]>()));
    sqlx::query(
        "INSERT INTO platform_call_slots (project_id, slot_id, kind, expires_at) \
         VALUES ($1, $2, $3, NOW() + make_interval(secs => $4::float8))",
    )
    .bind(project_id)
    .bind(&slot_id)
    .bind(kind)
    .bind(ttl_secs as f64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(SlotGuard { db: db.clone(), project_id: project_id.to_string(), slot_id, released: false })
}

pub async fn release_slot(db: &PgPool, project_id: &str, slot_id: &str) -> Result<(), PlatformError> {
    sqlx::query("DELETE FROM platform_call_slots WHERE project_id = $1 AND slot_id = $2")
        .bind(project_id)
        .bind(slot_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Remove expired slots; returns how many were removed.
pub async fn sweep_expired_slots(db: &PgPool) -> Result<u64, PlatformError> {
    Ok(sqlx::query("DELETE FROM platform_call_slots WHERE expires_at <= NOW()")
        .execute(db)
        .await?
        .rows_affected())
}
