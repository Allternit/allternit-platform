//! Clerk webhook receiver — syncs deletions into the central `users` table,
//! and sends the welcome email on `user.created` (services::customer_emails).
//!
//! Creation/update already happens via lazy per-request upsert (see
//! `runtime_pairing.rs`, `hosted_runtimes.rs`, `wizard.rs`, `gizzi_instances.rs`),
//! which pulls real email/name/avatar out of the verified Clerk JWT on every
//! authenticated request. There was no equivalent for deletion — a user who
//! deletes their Clerk account never had `status` updated here at all, since
//! nothing calls this service *after* they're gone to authenticate as them.
//! Only a push notification (a webhook) can close that gap.
//!
//! Clerk signs every delivery with the Svix scheme: `svix-id`, `svix-timestamp`,
//! and `svix-signature` headers, HMAC-SHA256 over `"${svix_id}.${svix_timestamp}.${body}"`,
//! using the secret's payload after stripping the `whsec_` prefix and
//! base64-decoding it, with the resulting signature itself base64-encoded
//! (see https://docs.svix.com/receiving/verifying-payloads/how-manual). The
//! secret comes from `CLERK_WEBHOOK_SECRET`; same fail-closed pattern as
//! `billing_webhooks.rs`'s Stripe handler — an unconfigured secret is a 503,
//! never a silent skip of verification.

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;

use crate::{error::ApiError, services, ApiState};

const SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route("/api/v1/webhooks/clerk", post(clerk_webhook))
}

fn webhook_not_configured_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "webhook_not_configured" })),
    )
        .into_response()
}

fn ignored_response(event_type: &str) -> Response {
    Json(json!({ "received": true, "ignored": event_type })).into_response()
}

/// Verify a Svix-signed Clerk webhook delivery. Fails closed: a missing
/// header, an out-of-tolerance timestamp, or no matching signature all
/// reject the same way as an outright bad signature.
fn verify_clerk_signature(
    secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    now_unix: i64,
) -> Result<(), ApiError> {
    let invalid = || ApiError::Unauthorized("Invalid Clerk webhook signature.".to_string());

    let svix_id = headers
        .get("svix-id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let svix_timestamp = headers
        .get("svix-timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let svix_signature = headers
        .get("svix-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;

    let timestamp: i64 = svix_timestamp.parse().map_err(|_| invalid())?;
    if (now_unix - timestamp).abs() > SIGNATURE_TOLERANCE_SECONDS {
        return Err(invalid());
    }

    // Secret is `whsec_<base64>`; the HMAC key is the decoded bytes, not the
    // raw prefixed string.
    let secret_b64 = secret.strip_prefix("whsec_").unwrap_or(secret);
    let secret_bytes = STANDARD
        .decode(secret_b64)
        .map_err(|_| ApiError::Internal("CLERK_WEBHOOK_SECRET is not valid whsec_<base64>".to_string()))?;

    let mut signed_content = Vec::with_capacity(svix_id.len() + svix_timestamp.len() + body.len() + 2);
    signed_content.extend_from_slice(svix_id.as_bytes());
    signed_content.push(b'.');
    signed_content.extend_from_slice(svix_timestamp.as_bytes());
    signed_content.push(b'.');
    signed_content.extend_from_slice(body);

    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret_bytes)
        .map_err(|_| ApiError::Internal("Failed to build webhook verifier".to_string()))?;
    mac.update(&signed_content);

    // svix-signature is space-delimited `v1,<base64>` entries (plural during
    // secret rotation) — any one matching is sufficient.
    for part in svix_signature.split(' ') {
        let Some(sig) = part.strip_prefix("v1,") else {
            continue;
        };
        let Ok(expected) = STANDARD.decode(sig) else {
            continue;
        };
        if mac.clone().verify_slice(&expected).is_ok() {
            return Ok(());
        }
    }
    Err(invalid())
}

async fn clerk_webhook(State(state): State<Arc<ApiState>>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(secret) = std::env::var("CLERK_WEBHOOK_SECRET") else {
        return webhook_not_configured_response();
    };
    if let Err(error) = verify_clerk_signature(&secret, &headers, &body, chrono::Utc::now().timestamp()) {
        return error.into_response();
    }

    let event: Value = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(_) => return ApiError::BadRequest("Invalid Clerk event payload".to_string()).into_response(),
    };

    let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");

    match event_type {
        "user.created" => {
            // Welcome email + a note to the team; answered at once so Clerk
            // never retries on a slow mail server.
            let db = state.db.clone();
            let user = event.get("data").cloned().unwrap_or(Value::Null);
            tokio::spawn(async move { services::customer_emails::on_user_created(&db, &user).await });
            Json(json!({ "received": true })).into_response()
        }
        "user.deleted" => handle_user_deleted(&state, &event).await,
        other => ignored_response(other),
    }
}

async fn handle_user_deleted(state: &ApiState, event: &Value) -> Response {
    let Some(user_id) = event
        .get("data")
        .and_then(|d| d.get("id"))
        .and_then(Value::as_str)
    else {
        return ApiError::BadRequest("user.deleted event missing data.id".to_string()).into_response();
    };

    match sqlx::query("UPDATE users SET status = 'inactive' WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
    {
        Ok(result) => {
            services::audit::write_audit_log(
                &state.db,
                services::audit::AuditEvent {
                    action: "user.deleted".to_string(),
                    resource_type: "user".to_string(),
                    resource_id: Some(user_id.to_string()),
                    user_id: Some(user_id.to_string()),
                    user_email: None,
                    details: Some(json!({ "matched": result.rows_affected() > 0 })),
                    success: true,
                },
            )
            .await;
            Json(json!({
                "received": true,
                "userId": user_id,
                "matched": result.rows_affected() > 0,
            }))
            .into_response()
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
