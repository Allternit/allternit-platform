//! Usage summary / invoice-draft rendering — Clerk-authenticated. Aggregates
//! `usage_events` into line items on demand; no `invoices` table exists yet
//! (see billing.rs's NoopCharger doc comment for why persisting drafts isn't
//! worth it until a real charger exists).

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::auth::AuthUser;
use crate::billing::{self, InvoiceDraft, InvoiceLineItem};
use crate::AppState;

pub fn usage_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/usage/summary", get(usage_summary))
        .route("/usage/ledger", post(report_ledger))
}

#[derive(Debug, Deserialize)]
struct LedgerReport {
    calls: Vec<crate::usage_ledger::ReportedCall>,
}

/// Most calls accepted per report (gizzi sends one per model call).
const MAX_REPORTED_CALLS: usize = 200;

/// O15: gizzi-code (and S1 sidecars) report their own model calls / S1
/// decisions into the one ledger. Existing user auth; rows are owned by the
/// caller's user and active organization. Calls already metered by
/// allternit-api (same gizzi session) are skipped, re-deliveries update.
async fn report_ledger(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<LedgerReport>,
) -> impl IntoResponse {
    if body.calls.len() > MAX_REPORTED_CALLS {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "too_many_calls", "message": format!("At most {MAX_REPORTED_CALLS} calls per report.")})),
        )
            .into_response();
    }
    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<(usize, usize)> {
        let conn = db.connect()?;
        let (mut recorded, mut skipped) = (0, 0);
        for call in &body.calls {
            match crate::usage_ledger::reported_row(call, user.organization_id.as_deref(), &user.user_id) {
                Some(row) => match crate::usage_ledger::insert(&conn, &row)? {
                    Some(_) => recorded += 1,
                    None => skipped += 1,
                },
                None => skipped += 1,
            }
        }
        Ok((recorded, skipped))
    })
    .await;
    match result {
        Ok(Ok((recorded, skipped))) => (StatusCode::OK, Json(json!({"recorded": recorded, "skipped": skipped}))).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "db_error", "message": e.to_string()}))).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "task_join_error"}))).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct UsageSummaryQuery {
    organization_id: String,
    period_start: String,
    period_end: String,
    /// O15: `surface`, `tier`, `model`, `lane` (comma-separated). When set,
    /// the response adds a `ledger` object from `llm_usage_events`.
    group_by: Option<String>,
}

async fn usage_summary(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<UsageSummaryQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;
    let active_organization_id = user.organization_id;
    let organization_id = query.organization_id;
    let period_start = query.period_start;
    let period_end = query.period_end;
    let group_by = match query.group_by.as_deref().map(crate::usage_ledger::parse_group_by).transpose() {
        Ok(g) => g,
        Err(message) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid_group_by", "message": message}))).into_response()
        }
    };

    let result = tokio::task::spawn_blocking(move || -> Result<(InvoiceDraft, Option<serde_json::Value>), (StatusCode, serde_json::Value)> {
        let conn = db.connect().map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
        })?;

        if active_organization_id.as_deref() != Some(organization_id.as_str()) {
            return Err((
                StatusCode::FORBIDDEN,
                json!({"error": "inactive_organization", "message": "Select this organization before viewing its metered usage."}),
            ));
        }

        // Metered invoice data is organization-sensitive. Only active owners
        // and admins may view it, even if another member guesses the URL.
        let is_admin = crate::rbac::is_org_admin(&conn, &organization_id, &user_id).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
        })?;
        if !is_admin {
            return Err((
                StatusCode::FORBIDDEN,
                json!({"error": "insufficient_role", "message": "Only organization owners/admins can view metered billing."}),
            ));
        }

        let mut stmt = conn
            .prepare(
                "SELECT resource_type, unit, SUM(quantity) as total_quantity, SUM(computed_cost_cents) as total_cents
                 FROM usage_events
                 WHERE organization_id = ?1 AND started_at >= ?2 AND started_at < ?3
                 GROUP BY resource_type, unit
                 ORDER BY resource_type",
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()})))?;

        let rows = stmt
            .query_map(params![organization_id, period_start, period_end], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()})))?;

        let mut line_items = Vec::new();
        let mut total_cents = 0i64;
        for row in rows {
            let (resource_type, unit, quantity, cents) = row.map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
            })?;
            total_cents += cents;
            line_items.push(InvoiceLineItem {
                description: format!("{resource_type} ({unit})"),
                resource_type,
                quantity,
                unit,
                subtotal_cents: cents,
            });
        }

        let ledger = match &group_by {
            Some(dims) => Some(
                crate::usage_ledger::summary(&conn, &organization_id, &period_start, &period_end, dims).map_err(|e| {
                    (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "db_error", "message": e.to_string()}))
                })?,
            ),
            None => None,
        };

        Ok((InvoiceDraft {
            organization_id,
            period_start,
            period_end,
            line_items,
            total_cents,
            seller_legal_name: billing::SELLER_LEGAL_NAME,
            seller_address_lines: billing::SELLER_ADDRESS_LINES,
            payment_terms: billing::PAYMENT_TERMS,
        }, ledger))
    })
    .await;

    match result {
        Ok(Ok((draft, ledger))) => {
            let mut body = json!(draft);
            if let Some(ledger) = ledger {
                body["ledger"] = ledger;
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(Err((status, body))) => (status, Json(body)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "task_join_error"})),
        )
            .into_response(),
    }
}
