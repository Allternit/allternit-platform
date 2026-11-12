//! `GET /v1/approvals`, `GET /v1/approvals/{id}`, `POST /v1/approvals/{id}`:
//! one approvals resource over the twin and computer hold systems (scope `twin`
//! or `computers`; table `platform_approvals`, migration 122). Owned by **lane 4
//! (Approvals)** of the Platform API parity program.
//!
//! Seam stub: registers nothing yet. [`pending_for_response`] is real so lane 2
//! can derive a response's `requires_action` / `required_actions` from it; lane 4
//! adds the routes inside `register` and the rest of the module.

use sqlx::PgPool;

use super::{PlatformError, RouteTable};

pub fn register(table: RouteTable) -> RouteTable {
    table
}

/// Ids (`apr_…`) of the approvals still pending for one response, oldest first.
pub async fn pending_for_response(db: &PgPool, response_id: &str) -> Result<Vec<String>, PlatformError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM platform_approvals WHERE response_id = $1 AND status = 'pending' ORDER BY created_at, id",
    )
    .bind(response_id)
    .fetch_all(db)
    .await?;
    Ok(ids)
}
