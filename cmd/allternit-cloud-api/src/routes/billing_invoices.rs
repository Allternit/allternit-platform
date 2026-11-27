//! The caller's Stripe invoices for the in-app billing page (Claude polish C3).
//!
//! `GET /api/v1/billing/invoices?limit=&cursor=` verifies the Clerk session
//! (or a `billing`-scoped API token) and lists invoices of the caller's own
//! Stripe customer (`user_billing_accounts`), newest first:
//!
//! ```json
//! { "invoices": [{ "id", "created_at", "total_usd", "currency", "status", "pdf_url" }],
//!   "cursor": "in_..." | null }
//! ```
//!
//! `cursor` is the id to pass back for the next page; null on the last page.
//! Drafts are left out (not sent to the customer yet). No Stripe customer
//! yet answers an empty list. 503 `billing_not_configured` without
//! `STRIPE_SECRET_KEY`; Stripe failures answer 502 `billing_upstream_error`.

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use super::billing_checkout::{
    billing_not_configured_response, billing_upstream_error_response, ReqwestStripeCheckout, StripeCheckout,
};
use crate::error::ApiError;
use crate::ApiState;

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/billing/invoices", get(list_invoices))
}

#[derive(Debug, Default, Deserialize)]
pub struct InvoiceQuery {
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
}

/// One invoice as the client shows it (`components/settings/BillingSummary.tsx` `Invoice`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InvoiceView {
    pub id: String,
    /// RFC 3339.
    pub created_at: String,
    pub total_usd: f64,
    pub currency: String,
    /// Stripe's status: `open`, `paid`, `void` or `uncollectible`.
    pub status: String,
    pub pdf_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InvoicePage {
    pub invoices: Vec<InvoiceView>,
    pub cursor: Option<String>,
}

/// A cursor must be a Stripe invoice id; anything else is a 400, so a
/// client cannot slip extra query text through to Stripe.
fn valid_cursor(cursor: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(cursor) = cursor.map(str::trim).filter(|cursor| !cursor.is_empty()) else {
        return Ok(None);
    };
    let ok = cursor.starts_with("in_")
        && cursor.len() <= 255
        && cursor.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(ApiError::BadRequest("cursor must be an invoice id from the previous page.".to_string()));
    }
    Ok(Some(cursor.to_string()))
}

fn invoice_view(invoice: &Value, customer_id: &str) -> Option<InvoiceView> {
    let id = invoice["id"].as_str().filter(|id| !id.is_empty())?;
    // Defence in depth: the Stripe query is already scoped to the customer.
    let owner = invoice["customer"].as_str().or_else(|| invoice["customer"]["id"].as_str());
    if owner != Some(customer_id) {
        return None;
    }
    let status = invoice["status"].as_str().unwrap_or("open");
    if status == "draft" {
        return None;
    }
    let created_at = invoice["created"]
        .as_i64()
        .and_then(|created| Utc.timestamp_opt(created, 0).single())
        .map(|created| created.to_rfc3339())?;
    Some(InvoiceView {
        id: id.to_string(),
        created_at,
        total_usd: invoice["total"].as_i64().unwrap_or(0) as f64 / 100.0,
        currency: invoice["currency"].as_str().unwrap_or("usd").to_string(),
        status: status.to_string(),
        pdf_url: invoice["invoice_pdf"].as_str().filter(|url| !url.is_empty()).map(str::to_string),
    })
}

/// One page of `customer_id`'s invoices.
pub(crate) async fn invoice_page(
    stripe: &dyn StripeCheckout,
    secret_key: &str,
    customer_id: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<InvoicePage, ApiError> {
    let cursor = valid_cursor(cursor)?;
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut query = vec![
        ("customer".to_string(), customer_id.to_string()),
        ("limit".to_string(), limit.to_string()),
    ];
    if let Some(cursor) = cursor {
        query.push(("starting_after".to_string(), cursor));
    }
    let list = stripe.get_object(secret_key, "/v1/invoices", &query).await?;
    let data = list["data"].as_array().cloned().unwrap_or_default();
    let invoices = data.iter().filter_map(|invoice| invoice_view(invoice, customer_id)).collect();
    // The cursor is the last row Stripe returned (drafts included), so the
    // next page starts where this one ended.
    let cursor = if list["has_more"].as_bool() == Some(true) {
        data.last().and_then(|invoice| invoice["id"].as_str()).map(str::to_string)
    } else {
        None
    };
    Ok(InvoicePage { invoices, cursor })
}

async fn list_invoices(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Query(query): Query<InvoiceQuery>,
) -> Response {
    let user = match crate::auth::resolve_user_scoped(&state.db, &headers, "billing").await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let Ok(secret_key) = std::env::var("STRIPE_SECRET_KEY") else {
        return billing_not_configured_response();
    };
    let customer_id = match crate::services::auto_topup::linked_customer(&state.db, &user.id).await {
        Ok(Some(customer_id)) => customer_id,
        Ok(None) => return Json(InvoicePage { invoices: Vec::new(), cursor: None }).into_response(),
        Err(error) => return error.into_response(),
    };
    match invoice_page(&ReqwestStripeCheckout::new(), &secret_key, &customer_id, query.limit, query.cursor.as_deref())
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error @ ApiError::BadRequest(_)) => error.into_response(),
        Err(error) => billing_upstream_error_response(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::billing_payment_methods::test_support::FakeStripe;
    use serde_json::json;

    const KEY: &str = "sk_test_fake";

    fn invoice(id: &str, customer: &str, status: &str, total: i64) -> Value {
        json!({
            "id": id, "customer": customer, "status": status, "total": total,
            "currency": "usd", "created": 1_798_761_600i64,
            "invoice_pdf": format!("https://pay.stripe.com/invoice/{id}/pdf"),
        })
    }

    #[tokio::test]
    async fn only_the_callers_invoices_come_back() {
        let stripe = FakeStripe::default();
        stripe.invoices.lock().unwrap().extend([
            invoice("in_a1", "cus_a", "paid", 2000),
            invoice("in_b1", "cus_b", "paid", 9900),
            invoice("in_a2", "cus_a", "open", 1050),
        ]);
        let page = invoice_page(&stripe, KEY, "cus_a", None, None).await.unwrap();
        let ids: Vec<&str> = page.invoices.iter().map(|invoice| invoice.id.as_str()).collect();
        assert_eq!(ids, vec!["in_a1", "in_a2"]);
        assert_eq!(page.invoices[0].total_usd, 20.0);
        assert_eq!(page.invoices[1].total_usd, 10.5);
        assert_eq!(page.invoices[0].status, "paid");
        assert_eq!(page.invoices[0].pdf_url.as_deref(), Some("https://pay.stripe.com/invoice/in_a1/pdf"));
        assert!(page.invoices[0].created_at.starts_with("2027-01-01"));
        assert_eq!(page.cursor, None);
        let gets = stripe.gets.lock().unwrap();
        assert!(gets[0].1.contains(&("customer".to_string(), "cus_a".to_string())));
    }

    #[test]
    fn a_row_for_another_customer_or_a_draft_is_dropped() {
        assert!(invoice_view(&invoice("in_b1", "cus_b", "paid", 100), "cus_a").is_none());
        assert!(invoice_view(&invoice("in_d", "cus_a", "draft", 100), "cus_a").is_none());
    }

    #[tokio::test]
    async fn a_cursor_that_is_not_an_invoice_id_is_refused() {
        let stripe = FakeStripe::default();
        for cursor in ["cus_a", "in_1&customer=cus_b", "in_ 1"] {
            let result = invoice_page(&stripe, KEY, "cus_a", None, Some(cursor)).await;
            assert!(matches!(result, Err(ApiError::BadRequest(_))), "{cursor:?} must be refused");
        }
        assert!(stripe.gets.lock().unwrap().is_empty());
    }
}
