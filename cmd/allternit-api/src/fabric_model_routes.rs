//! Fabric Model Gateway routes — OpenAI-shaped model catalog and inference.
//!
//! Paths are intentionally mounted under `/v1` so OpenAI-compatible clients can
//! discover models (`GET /v1/models`) and call the unified response endpoint
//! (`POST /v1/responses`). Authentication is applied by `main.rs` via the same
//! `auth_middleware` used for the protected `/api/v1` surface, so callers can
//! use Clerk JWTs or organization access tokens (`at-...`).

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::warn;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    fabric::credits::{CreditsError, CreditsLedger},
    fabric::inference_executor::execute_on_placement,
    fabric::model_catalog::ModelCatalog,
    fabric::model_gateway::ModelGateway,
    fabric::os_client::{OsClientError, OsControlPlaneClient, OsLeaseIssueRequest},
    fabric::os_mapping::ModelRequest,
    fabric::resources::ResourceManager,
    fabric::scheduler::{PlacementRecorder, SchedulerError},
    fabric::usage::UsageIngestor,
    AppState,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/*id", get(get_model))
        .route("/v1/responses", post(create_response))
}

#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) body: Json<Value>,
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: Json(json!({"error": code, "message": message.into()})),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, self.body).into_response()
    }
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, code, message)
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "fabric model gateway operation failed");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

fn require_org(user: &AuthUser) -> Result<String, ApiError> {
    user.organization_id.clone().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })
}

fn model_json(model: &crate::fabric::model_catalog::FabricModelRecord) -> Value {
    json!({
        "id": format!("{}/{}", model.provider_kind, model.model_id),
        "object": "model",
        "created": 0,
        "owned_by": model.provider_kind,
        "display_name": model.display_name,
        "context_window": model.context_tokens,
        "quality_tier": model.quality_tier,
        "pricing": {
            "input_cents_per_1m": model.input_cents_per_1m,
            "output_cents_per_1m": model.output_cents_per_1m,
        },
    })
}

async fn list_models(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let _org = require_org(&user)?;
    let db = state.db.clone();
    let models = tokio::task::spawn_blocking(move || {
        let catalog = ModelCatalog::new(db);
        catalog.seed_builtin()?;
        catalog.list()
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(json!({
        "object": "list",
        "data": models.iter().map(model_json).collect::<Vec<_>>(),
    })))
}

async fn get_model(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let _org = require_org(&user)?;
    let full_id = id.trim_start_matches('/').to_string();
    let db = state.db.clone();
    let model = tokio::task::spawn_blocking(move || {
        let catalog = ModelCatalog::new(db);
        catalog.seed_builtin()?;
        catalog.get_by_full_id(&full_id)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "model_not_found", format!("Model `{id}` not found.")))?;

    Ok(Json(model_json(&model)))
}

#[derive(Debug, Deserialize, Serialize)]
#[allow(dead_code)]
pub(crate) struct Message {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct ResponsesRequest {
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) messages: Vec<Message>,
    #[serde(default)]
    pub(crate) max_tokens: Option<u32>,
    #[serde(default)]
    pub(crate) temperature: Option<f32>,
}

async fn create_response(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<ResponsesRequest>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;

    // Seed the catalog before looking up the requested model.
    let db = state.db.clone();
    let model_full_id = req.model.clone();
    let model = tokio::task::spawn_blocking(move || {
        let catalog = ModelCatalog::new(db);
        catalog.seed_builtin()?;
        catalog.get_by_full_id(&model_full_id)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            "model_not_found",
            format!("Model `{}` not found.", req.model),
        )
    })?;

    let request_id = Uuid::new_v4().to_string();
    let (result, generated_text, os_resource_id) =
        run_completion(&state, &org, &user, &request_id, &model, &req).await?;

    let now = Utc::now().timestamp();
    let mut response = json!({
        "id": request_id,
        "object": "chat.completion",
        "created": now,
        "model": format!("{}/{}", model.provider_kind, model.model_id),
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": generated_text,
            },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": result.input_tokens,
            "completion_tokens": result.output_tokens,
            "total_tokens": result.input_tokens + result.output_tokens,
        },
        "cost_cents": result.cost_cents,
        "organization_id": org,
    });

    if let Some(resource_id) = os_resource_id {
        response["resource_id"] = json!(resource_id);
    }

    Ok(Json(response))
}

/// Run one chat completion through the existing model router/gateway
/// machinery: catalog-priced, credit-gated, OS-scheduled inference when a
/// control plane is configured, deterministic MVP fallback otherwise.
/// Shared by `POST /v1/responses` and the Al persona runtime (P-T5) — no new
/// LLM path is introduced.
pub(crate) async fn run_completion(
    state: &AppState,
    org: &str,
    user: &AuthUser,
    request_id: &str,
    model: &crate::fabric::model_catalog::FabricModelRecord,
    req: &ResponsesRequest,
) -> Result<(crate::fabric::model_gateway::ModelUsageResult, String, Option<String>), ApiError> {
    // Pre-dispatch gate: with no spendable balance the request is refused
    // upfront rather than accruing synchronous debt. (Overspend past zero
    // from a balance drained mid-request is still recorded as debt by the
    // metered charge — usage that already happened is never dropped.)
    let db = state.db.clone();
    let org_for_gate = org.to_string();
    let spendable = tokio::task::spawn_blocking(move || {
        CreditsLedger::new(db).available_cents(&org_for_gate)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    if spendable <= 0 {
        return Err(error(
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_credits",
            format!("Balance {spendable} cents available; top up organization credits to run inference."),
        ));
    }

    // Build the Cloud product ModelRequest. This is the input to the OS planner
    // and resource scheduler: it decides which canonical resource class is needed
    // to run the selected model.
    let model_request = ModelRequest::from_responses_request(request_id, req, Some(model));

    // Optional Phase-4 OS scheduling + execution hop: when an AllternitOS
    // control plane is configured, acquire a canonical lease, run inference on
    // the returned placement, and charge for actual token usage.
    if let Some(os_client) = state.os_control_plane.as_ref() {
        let (resource_id, placement_id, canonical_placement) = schedule_model_resource_via_os(
            state,
            org,
            user,
            request_id,
            &model_request,
            os_client,
        )
        .await?;

        let inference_result = execute_on_placement(&canonical_placement, req)
            .await
            .map_err(|e| {
                let (status, message) = e.to_api_error();
                error(status, "inference_execution_failed", message)
            })?;

        // Charge for actual token usage observed from the backend.
        let db = state.db.clone();
        let org_for_charge = org.to_string();
        let model_id_for_charge = req.model.clone();
        let request_id_for_charge = request_id.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let gateway = ModelGateway::new(db);
            gateway.charge_usage(
                &org_for_charge,
                &model_id_for_charge,
                inference_result.input_tokens,
                inference_result.output_tokens,
                &request_id_for_charge,
            )
        })
        .await
        .map_err(internal)?
        .map_err(map_gateway_error)?;

        // Tie actual token usage to the OS-scheduled resource/placement so the
        // Cloud usage ingestion pipeline can reconcile it into a canonical
        // UsageEvent.
        let resource_id_for_event = resource_id.clone();
        let db = state.db.clone();
        let total_tokens = inference_result.input_tokens + inference_result.output_tokens;
        tokio::task::spawn_blocking(move || {
            let ingestor = UsageIngestor::new(db);
            if let Err(e) = ingestor.record_usage_event(
                &resource_id_for_event,
                "model.inference",
                total_tokens as f64,
                "tokens",
                Some(Utc::now()),
                Some(&placement_id),
            ) {
                warn!(resource_id = %resource_id_for_event, error = %e, "failed to record model usage event");
            }
        })
        .await
        .map_err(internal)?;

        Ok((result, inference_result.generated_text, Some(resource_id)))
    } else {
        // No OS control plane configured: fall back to deterministic MVP
        // behavior so existing unit tests stay green.
        let input_tokens = estimate_input_tokens(&req.messages);
        let output_tokens = req.max_tokens.unwrap_or(150).clamp(1, model.context_tokens.max(1));

        let db = state.db.clone();
        let org_for_charge = org.to_string();
        let model_id_for_charge = req.model.clone();
        let request_id_for_charge = request_id.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let gateway = ModelGateway::new(db);
            gateway.charge_usage(
                &org_for_charge,
                &model_id_for_charge,
                input_tokens,
                output_tokens,
                &request_id_for_charge,
            )
        })
        .await
        .map_err(internal)?
        .map_err(map_gateway_error)?;

        let content = format!(
            "This is a deterministic MVP response from {}/{}. In production this route proxies to the provider and streams the real output.",
            model.provider_kind, model.model_id
        );
        Ok((result, content, None))
    }
}

/// Schedule inference capacity for a model request through the canonical OS
/// control plane. This is the Phase-4 OS hop: the Cloud Model Gateway decides
/// which resource class satisfies the model request, asks the OS resource
/// scheduler for a lease/placement, and records the result in Cloud's resource
/// tables so token usage can be attributed to real capacity.
async fn schedule_model_resource_via_os(
    state: &AppState,
    org: &str,
    user: &AuthUser,
    request_id: &str,
    model_request: &ModelRequest,
    os_client: &OsControlPlaneClient,
) -> Result<(String, String, allternitos_cloud_contracts::Placement), ApiError> {
    let ledger = CreditsLedger::new(state.db.clone());
    let recorder = PlacementRecorder::new(state.db.clone());

    // MVP: route every model request to the `compute.s` class. This satisfies
    // the Phase-4 integration test with the fake provider; in production the
    // class would be derived from the model's manifest/hardware profile.
    let full_class = "compute.s";
    let class = state
        .resource_class_catalog
        .get(full_class)
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "unknown_class",
                format!("Resource class {full_class} not available"),
            )
        })?;

    // Record the provisioning intent before placing a credit hold.
    let fabric_req = crate::fabric_resources_routes::fabric_request_from_class(
        request_id,
        &class,
        None,
        allternit_computer_cloud::fabric::RegionPolicy::Any,
        None,
        None,
    );
    recorder.record_resource(org, &fabric_req).map_err(|e| {
        warn!(resource_id = %request_id, error = %e, "failed to record model resource");
        map_scheduler_error(e)
    })?;

    let estimated_cents = class.retail_price_per_hour_cents;
    let hold = ledger.hold(org, request_id, estimated_cents).map_err(|e| {
        warn!(resource_id = %request_id, error = %e, "failed to place credit hold for model resource");
        map_scheduler_error(SchedulerError::Credits(e))
    })?;

    let lease_request = OsLeaseIssueRequest {
        requester_principal_id: format!("prn_{}", user.user_id),
        workload_id: request_id.to_string(),
        step_id: None,
        capability: "model.generate".to_string(),
        resource: Some(serde_json::to_string(model_request).unwrap_or_default()),
        resource_class_id: Some(full_class.to_string()),
        placement: None,
        actions: vec!["generate".to_string()],
        purpose: "managed inference model request".to_string(),
        not_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
    };

    let issue_response = match os_client.issue_lease(lease_request).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!(resource_id = %request_id, error = %e, "os control plane model lease issue failed");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(request_id, "os_lease_failed");
            return Err(map_os_client_error(e));
        }
    };

    let lease_record = match os_client.get_lease(&issue_response.lease_id).await {
        Ok(record) => record,
        Err(e) => {
            warn!(resource_id = %request_id, lease_id = %issue_response.lease_id, error = %e, "failed to fetch os model lease record");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(request_id, "os_lease_fetch_failed");
            return Err(map_os_client_error(e));
        }
    };

    let placement = match lease_record.placement {
        Some(p) => p,
        None => {
            warn!(resource_id = %request_id, lease_id = %issue_response.lease_id, "os model lease returned without placement");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(request_id, "os_lease_missing_placement");
            return Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "os_scheduling_error",
                "OS model lease did not include a placement",
            ));
        }
    };

    if let Err(e) = recorder.record_os_placement(
        org,
        request_id,
        &class.kind.to_string(),
        &class.class,
        None,
        Some(&issue_response.lease_id),
        &placement,
        &hold.id,
    ) {
        warn!(resource_id = %request_id, error = %e, "failed to record os model placement");
        let _ = ledger.release_hold(&hold.id);
        let _ = recorder.mark_terminated(request_id, "record_os_placement_failed");
        return Err(map_scheduler_error(e));
    }

    // Model capacity is billed per-token via ModelGateway::charge_usage, so
    // the provisioning hold is released instead of charged — charging the
    // estimate here would double-bill against the metered token charge.
    if let Err(e) = ledger.release_hold(&hold.id) {
        warn!(resource_id = %request_id, hold_id = %hold.id, error = %e, "failed to release hold after os model placement");
    }

    // Return the Cloud resource id, Cloud placement id, and the canonical OS
    // placement so the caller can execute inference on the leased capacity.
    let manager = ResourceManager::new(state.db.clone());
    let placement_id = manager
        .latest_placement(request_id)
        .map_err(internal)?
        .map(|p| p.id)
        .unwrap_or_default();
    Ok((request_id.to_string(), placement_id, placement))
}

fn map_scheduler_error(err: SchedulerError) -> ApiError {
    match err {
        SchedulerError::UnknownClass(class) => {
            error(StatusCode::BAD_REQUEST, "unknown_class", format!("Unknown resource class: {class}"))
        }
        SchedulerError::NoEligibleOffers => {
            error(StatusCode::SERVICE_UNAVAILABLE, "no_offers", "No eligible offers found")
        }
        SchedulerError::AllOffersUnprofitable => {
            error(StatusCode::SERVICE_UNAVAILABLE, "no_profitable_offers", "All offers are unprofitable")
        }
        SchedulerError::Provider(e) => internal(e),
        SchedulerError::Credits(CreditsError::InsufficientCredits { balance, required }) => {
            ApiError {
                status: StatusCode::PAYMENT_REQUIRED,
                body: Json(json!({
                    "error": "insufficient_credits",
                    "message": format!("Balance {balance} cents, required {required} cents"),
                    "balance_cents": balance,
                    "required_cents": required,
                })),
            }
        }
        SchedulerError::Credits(e) => error(StatusCode::PAYMENT_REQUIRED, "credits_error", e.to_string()),
        SchedulerError::Timeout => error(StatusCode::REQUEST_TIMEOUT, "timeout", "Provisioning timed out"),
    }
}

fn map_os_client_error(err: OsClientError) -> ApiError {
    match err {
        OsClientError::ControlPlane { status, body } => {
            error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", format!("OS control plane error {status}: {body}"))
        }
        other => error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", other.to_string()),
    }
}

fn estimate_input_tokens(messages: &[Message]) -> u32 {
    let mut total = 0u32;
    for msg in messages {
        // Very rough approximation: 1 token ~= 4 characters, plus a few per
        // message for the role/name wrapper. Good enough for MVP billing tests.
        total += (msg.content.len() / 4).max(1) as u32 + 3;
    }
    total.max(1)
}

fn map_gateway_error(err: crate::fabric::model_gateway::ModelGatewayError) -> ApiError {
    use crate::fabric::model_catalog::ModelCatalogError;
    use crate::fabric::model_gateway::ModelGatewayError;
    match err {
        ModelGatewayError::ModelNotFound(id) => {
            error(StatusCode::NOT_FOUND, "model_not_found", format!("Model `{id}` not found."))
        }
        ModelGatewayError::Catalog(ModelCatalogError::NotFound(id)) => {
            error(StatusCode::NOT_FOUND, "model_not_found", format!("Model `{id}` not found."))
        }
        ModelGatewayError::Catalog(ModelCatalogError::Db(e)) => internal(e),
        ModelGatewayError::Credits(CreditsError::InsufficientCredits { .. }) => {
            error(StatusCode::PAYMENT_REQUIRED, "insufficient_credits", err.to_string())
        }
        ModelGatewayError::Credits(CreditsError::InvalidAmount(_)) => {
            error(StatusCode::BAD_REQUEST, "invalid_amount", err.to_string())
        }
        ModelGatewayError::Credits(e) => internal(e),
        ModelGatewayError::Db(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fabric::credits::{CreditsLedger, TransactionType};
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    fn seed_org_user(conn: &rusqlite::Connection, org_id: &str, user_id: &str, role: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            rusqlite::params![org_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            rusqlite::params![user_id, format!("{}@test.local", user_id)],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![format!("{}:{}", org_id, user_id), org_id, user_id, role],
        )
        .unwrap();
    }

    fn auth_user(org_id: Option<&str>, user_id: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: Some(format!("{}@test.local", user_id)),
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: None,
            organization_slug: None,
        }
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::Null)
    }

    fn build_request(
        method: &str,
        uri: &str,
        user: AuthUser,
        body: Option<Value>,
    ) -> Request<Body> {
        let body = body
            .map(|b| Body::from(serde_json::to_string(&b).unwrap()))
            .unwrap_or_else(Body::empty);
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(body)
            .unwrap();
        req.extensions_mut().insert(user);
        req
    }

    #[tokio::test]
    async fn lists_catalog_models() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "GET",
                "/v1/models",
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let data = body["data"].as_array().unwrap();
        assert!(data.len() >= 10);
        assert!(data.iter().any(|m| m["id"] == "openai/gpt-4o"));
        assert!(data.iter().any(|m| m["id"] == "fireworks/accounts/fireworks/models/deepseek-r1"));
    }

    #[tokio::test]
    async fn gets_model_by_full_id() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "GET",
                "/v1/models/openai/gpt-4o-mini",
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["id"], "openai/gpt-4o-mini");
        assert_eq!(body["pricing"]["input_cents_per_1m"], 15);
    }

    #[tokio::test]
    async fn responses_charges_ledger() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        CreditsLedger::new(state.db.clone())
            .credit("org-1", 1000, TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/v1/responses",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "model": "openai/gpt-4o-mini",
                    "messages": [{"role": "user", "content": "Hello"}],
                    "max_tokens": 100,
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["model"], "openai/gpt-4o-mini");
        assert!(body["usage"]["total_tokens"].as_u64().unwrap() > 0);
        assert!(body["cost_cents"].as_i64().unwrap() > 0);

        let balance = CreditsLedger::new(state.db.clone())
            .balance_cents("org-1")
            .unwrap();
        assert!(balance < 1000);
    }

    #[tokio::test]
    async fn responses_returns_payment_required_when_insufficient_credits() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/v1/responses",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "model": "openai/gpt-4o",
                    "messages": [{"role": "user", "content": "Hello"}],
                    "max_tokens": 1000,
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn responses_returns_not_found_for_unknown_model() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/v1/responses",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "model": "openai/does-not-exist",
                    "messages": [{"role": "user", "content": "Hello"}],
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // Phase-4 real OS control-plane integration test.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;
    use tokio::io::AsyncBufReadExt;
    use tokio::process::Command;

    /// Spin up a tiny OpenAI-compatible HTTP server for the remote-endpoint
    /// journey test. Returns the bound port and a handle that joins when the
    /// server has handled its chat request.
    fn mock_openai_server(response_body: String) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let health_response = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
            let chat_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let mut handled_chat = false;
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                if request.starts_with("GET /health") {
                    let _ = stream.write_all(health_response.as_bytes());
                } else if request.starts_with("POST /v1/chat/completions") {
                    let _ = stream.write_all(chat_response.as_bytes());
                    handled_chat = true;
                }
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let _ = stream.read(&mut buf);
                if handled_chat {
                    break;
                }
            }
        });
        thread::sleep(Duration::from_millis(50));
        (port, handle)
    }

    async fn spawn_real_os_control_plane() -> (String, tokio::process::Child) {
        spawn_real_os_control_plane_with_endpoint(None).await
    }

    async fn spawn_real_os_control_plane_with_endpoint(
        fake_endpoint: Option<String>,
    ) -> (String, tokio::process::Child) {
        let temp = tempfile::tempdir().unwrap().keep();
        let bin = std::env::var("ALLTERNITOS_CONTROL_PLANE_BIN").unwrap_or_else(|_| {
            "~/Desktop/AllternitOS/target/debug/allternitos_control_plane".to_string()
        });
        let db_path = temp.join("cp.db");
        let mut cmd = Command::new(&bin);
        cmd.arg("--bind")
            .arg("127.0.0.1:0")
            .arg("--reconciler-path")
            .arg(&db_path)
            .arg("--fake-provider")
            .env("RUST_LOG", "info")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        if let Some(endpoint) = fake_endpoint {
            cmd.env("ALLTERNITOS_FAKE_PROVIDER_ENDPOINT", endpoint);
        }
        let mut child = cmd
            .spawn()
            .expect("failed to spawn allternitos_control_plane");

        let stdout = child.stdout.take().expect("stdout pipe");
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        let prefix = "control-plane HTTP server listening on 127.0.0.1:";
        let mut port: Option<String> = None;
        while let Some(line) = lines.next_line().await.expect("read stdout line") {
            if let Some(idx) = line.find(prefix) {
                port = Some(line[idx + prefix.len()..].trim().to_string());
                break;
            }
        }
        let port = port.expect("allternitos_control_plane did not log its listening port");
        let url = format!("http://127.0.0.1:{}", port);

        // Keep the child's stdout pipe open so its tracing subscriber does not
        // panic with a broken pipe after we stop reading.
        tokio::spawn(async move {
            while let Ok(Some(_)) = lines.next_line().await {}
        });

        let client = crate::net_client::client();
        for _ in 0..50 {
            if client.get(format!("{}/health", url)).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        (url, child)
    }

    // Ignored in CI: spawns the real AllternitOS control-plane binary
    // (ALLTERNITOS_CONTROL_PLANE_BIN), which this repo's CI does not build.
    // Run locally with:
    //   cargo test -p allternit-api --lib -- --ignored
    #[tokio::test]
    #[ignore = "requires AllternitOS workspace binaries not available in CI"]
    async fn responses_routes_through_real_os_control_plane() {
        let (url, _child) = spawn_real_os_control_plane().await;
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        CreditsLedger::new(state.db.clone())
            .credit("org-1", 10_000, TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/v1/responses",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "model": "openai/gpt-4o-mini",
                    "messages": [{"role": "user", "content": "Hello"}],
                    "max_tokens": 100,
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["model"], "openai/gpt-4o-mini");
        assert!(body["usage"]["total_tokens"].as_u64().unwrap() > 0);
        assert!(body["cost_cents"].as_i64().unwrap() > 0);
        assert!(
            body["resource_id"].as_str().map_or(false, |s| !s.is_empty()),
            "expected resource_id in response, got {:?}",
            body["resource_id"]
        );

        // Phase-4 execution loop: the response must contain real generated tokens
        // from the test double, not the old MVP deterministic string.
        let content = body["choices"][0]["message"]["content"].as_str().unwrap();
        assert_eq!(content, "Hello from AllternitOS mock backend");
        assert_eq!(body["usage"]["prompt_tokens"], 5);
        assert_eq!(body["usage"]["completion_tokens"], 7);
        assert_eq!(body["usage"]["total_tokens"], 12);

        // Verify the Cloud DB records the canonical OS placement (no off_unknown).
        let resource_id = body["resource_id"].as_str().unwrap();
        let manager = ResourceManager::new(state.db.clone());
        let resource = manager.get(resource_id).unwrap().expect("resource exists");
        assert_eq!(resource.provider_kind.as_deref(), Some("fake"));
        let placement = manager
            .latest_placement(resource_id)
            .unwrap()
            .expect("placement exists");
        assert!(
            !placement.offer_id.is_empty() && placement.offer_id.starts_with("off_"),
            "expected real offer_id, got {:?}",
            placement.offer_id
        );
        assert!(
            !placement.instance_type.is_empty(),
            "expected real instance_type, got {:?}",
            placement.instance_type
        );
        assert!(
            placement.provider_resource_id.as_deref().map_or(false, |id| id.starts_with("fake-")),
            "expected real provider_resource_id, got {:?}",
            placement.provider_resource_id
        );

        // Verify a usage event was recorded against the scheduled resource with
        // the actual token count from the backend (12 tokens, not the request's
        // max_tokens estimate).
        let events = manager
            .list_usage_events("org-1", Some(resource_id), 10)
            .unwrap();
        let inference_event = events
            .iter()
            .find(|e| e.event_type == "model.inference" && e.unit == "tokens")
            .expect("model.inference usage event");
        assert_eq!(inference_event.quantity, 12.0);
    }

    // Ignored in CI: spawns the real AllternitOS control-plane binary and
    // routes inference through allternitos-runtime (ALLTERNITOS_*_BIN), which
    // this repo's CI does not build. Run locally with:
    //   cargo test -p allternit-api --lib -- --ignored
    #[tokio::test]
    #[ignore = "requires AllternitOS workspace binaries not available in CI"]
    async fn responses_routes_through_real_os_control_plane_with_remote_endpoint() {
        // Full Phase-4 end-to-end journey: the OS fake provider reports a remote
        // endpoint, Cloud stores the canonical placement, and inference executes
        // through the allternitos-runtime remote-openai adapter against that URL.
        let body = r#"{"id":"chatcmpl-journey","object":"chat.completion","model":"remote-mock","choices":[{"index":0,"message":{"role":"assistant","content":"Cloud endpoint journey works"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#;
        let (port, server_handle) = mock_openai_server(body.to_string());
        let endpoint = format!("http://127.0.0.1:{port}");

        let (url, _child) = spawn_real_os_control_plane_with_endpoint(Some(endpoint.clone())).await;
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        CreditsLedger::new(state.db.clone())
            .credit("org-1", 10_000, TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/v1/responses",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({
                    "model": "openai/gpt-4o-mini",
                    "messages": [{"role": "user", "content": "Hello"}],
                    "max_tokens": 100,
                })),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["model"], "openai/gpt-4o-mini");

        // Response must come from the remote mock endpoint, not the local mock-llama-server.
        let content = body["choices"][0]["message"]["content"].as_str().unwrap();
        assert_eq!(content, "Cloud endpoint journey works");
        assert_eq!(body["usage"]["prompt_tokens"], 3);
        assert_eq!(body["usage"]["completion_tokens"], 4);
        assert_eq!(body["usage"]["total_tokens"], 7);

        // Cloud placement must carry the OS-populated endpoint.
        let resource_id = body["resource_id"].as_str().unwrap();
        let manager = ResourceManager::new(state.db.clone());
        let placement = manager
            .latest_placement(resource_id)
            .unwrap()
            .expect("placement exists");
        assert_eq!(
            placement.endpoint.as_deref(),
            Some(endpoint.as_str()),
            "expected placement endpoint to be populated from OS inspect"
        );
        assert_eq!(placement.status.as_str(), "running");

        // Verify the usage event carries the remote backend's token count.
        let events = manager
            .list_usage_events("org-1", Some(resource_id), 10)
            .unwrap();
        let inference_event = events
            .iter()
            .find(|e| e.event_type == "model.inference" && e.unit == "tokens")
            .expect("model.inference usage event");
        assert_eq!(inference_event.quantity, 7.0);

        let _ = server_handle.join();
    }
}
