//! Cloud Android phones (`/api/v1/cloud-phones`) — redroid containers on
//! phone-enabled fleet hosts that the Desktop app adds to Phone Link by adb
//! address over the mesh. Service: `services::cloud_phones`; host scripts:
//! `infrastructure/cloud-phone/`.
//!
//! * `GET    /api/v1/cloud-phones`      — `{ phones }` owned by the caller.
//! * `POST   /api/v1/cloud-phones`      — `{ label? }` → 201 `{ phone }` with
//!   `phone.adbAddress` (`meshIp:port`) once running.
//! * `DELETE /api/v1/cloud-phones/:id`  — removes the container and its data.
//! * `GET    /api/v1/cloud-phones/acl-fragment` — admin only: `{ acls }`, one
//!   Headscale ACL rule per live phone (owner's mesh user → host mesh IP on
//!   that phone's two ports), to merge into
//!   infrastructure/mesh/headscale/policy.hujson. The production policy only
//!   allows same-owner traffic, so a phone is unreachable until its rule is in.
//!
//! Auth: Clerk session or `compute`-scoped API token
//! (`auth::resolve_user_scoped`), same as `/api/v1/provisioned-instances`,
//! or a runtime device token acting as its owner (how Allternit Desktop
//! calls, like `/api/v1/code-environments`). Rows are owner-scoped; another
//! user's id answers 404.
//!
//! Gate: every route answers 503 "Cloud phones are not available yet" unless
//! `CLOUD_PHONES_ENABLED=1`. Create is admin-only (`ALLTERNIT_ADMIN_USER_IDS`)
//! because there is no cloud phone plan or Stripe price yet; pricing needs
//! Eoj's approval before customers can create phones.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use super::runtime_pairing::{device_token_from_headers, runtime_device_for_token};
use crate::services::cloud_phones::{ensure_enabled, CloudPhoneService};
use crate::{ApiError, ApiState};

/// The calling user's id: a runtime device token acts as its owner, else a
/// Clerk session or `compute`-scoped API token.
async fn caller_user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, ApiError> {
    if let Some(token) = device_token_from_headers(headers) {
        return Ok(runtime_device_for_token(&state.db, token, None).await?.user_id);
    }
    Ok(crate::auth::resolve_user_scoped(&state.db, headers, "compute")
        .await?
        .id)
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/cloud-phones", get(list_phones).post(create_phone))
        .route("/api/v1/cloud-phones/acl-fragment", get(acl_fragment))
        .route("/api/v1/cloud-phones/:id", delete(delete_phone))
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePhoneRequest {
    #[serde(default)]
    label: Option<String>,
}

fn service(state: &ApiState) -> CloudPhoneService {
    CloudPhoneService::new(state.db.clone())
}

async fn list_phones(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_enabled()?;
    let user_id = caller_user_id(&state, &headers).await?;
    let phones = service(&state).list_for_user(&user_id).await?;
    Ok(Json(serde_json::json!({ "phones": phones })))
}

async fn create_phone(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    body: Option<Json<CreatePhoneRequest>>,
) -> Result<Response, ApiError> {
    ensure_enabled()?;
    let user_id = caller_user_id(&state, &headers).await?;
    if !crate::routes::provisioned_instances::is_admin(&user_id) {
        return Err(ApiError::Forbidden(
            "Cloud phones are in preview and not open to accounts yet".to_string(),
        ));
    }
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let phone = service(&state)
        .create(&user_id, request.label.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "phone": phone }))).into_response())
}

async fn delete_phone(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    ensure_enabled()?;
    let user_id = caller_user_id(&state, &headers).await?;
    service(&state).delete(&id, &user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn acl_fragment(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_enabled()?;
    let user_id = caller_user_id(&state, &headers).await?;
    if !crate::routes::provisioned_instances::is_admin(&user_id) {
        return Err(ApiError::Forbidden(
            "The mesh ACL fragment is operator-only".to_string(),
        ));
    }
    let acls = service(&state).acl_fragment().await?;
    Ok(Json(serde_json::json!({
        "acls": acls,
        "mergeInto": "infrastructure/mesh/headscale/policy.hujson",
    })))
}
