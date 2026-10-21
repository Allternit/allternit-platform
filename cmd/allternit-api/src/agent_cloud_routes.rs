//! Agent Cloud API — persistent agent creation + Fabric runtime integration.
//!
//! Mounted under `/api/v1` so public paths land at `/api/v1/agents/:id/runtime/*`.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    fabric::{
        credits::CreditsLedger,
        os_client::{OsClientError, OsControlPlaneClient, OsLeaseIssueRequest},
        resources::ResourceManager,
        scheduler::{PlacementRecorder, ScheduledResource, SchedulerError},
    },
    AppState,
};
use allternit_computer_cloud::fabric::{
    CustomerConstraints, RegionPolicy, ReliabilityTier, ResourceKind, ResourceRequest,
};
use chrono::Utc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agents/:id/runtime/provision", post(provision_runtime))
        .route("/agents/:id/runtime/terminate", post(terminate_runtime))
        .route("/agents/:id/harness/provision", post(provision_harness))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: Json<Value>,
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
    tracing::warn!(error = %err, "agent cloud operation failed");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

fn scheduler_error(err: SchedulerError) -> ApiError {
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
        SchedulerError::Credits(crate::fabric::credits::CreditsError::InsufficientCredits { balance, required }) => {
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

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProvisionRuntimeRequest {
    #[serde(default = "default_runtime_class")]
    class: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    region_policy: Option<RegionPolicy>,
    #[serde(default)]
    reliability_tier: Option<ReliabilityTier>,
    #[serde(default)]
    latency_slo_ms: Option<u64>,
    #[serde(default)]
    constraints: Option<CustomerConstraints>,
}

fn default_runtime_class() -> String {
    "s".to_string()
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ProvisionHarnessRequest {
    #[serde(default = "default_harness_class")]
    class: String,
    #[serde(default)]
    display_name: Option<String>,
}

fn default_harness_class() -> String {
    "gizzi".to_string()
}

async fn provision_runtime(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Json(req): Json<ProvisionRuntimeRequest>,
) -> Result<Json<Value>, ApiError> {
    let org = user.organization_id.clone().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })?;

    // Verify agent exists and belongs to the calling user.
    let db = state.db.clone();
    let agent_id_for_lookup = agent_id.clone();
    let user_id = user.user_id.clone();
    let agent = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, config FROM agents WHERE id = ?1",
        )?;
        let row: Option<(String, String, Option<String>)> = stmt
            .query_row(rusqlite::params![agent_id_for_lookup], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    let (_owner_id, existing_config): (String, Option<String>) = match agent {
        Some((id, owner_id, config)) if owner_id == user_id => (id, config),
        Some(_) => {
            return Err(error(
                StatusCode::FORBIDDEN,
                "agent_access_denied",
                "Agent belongs to a different user.",
            ));
        }
        None => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "agent_not_found",
                "Agent not found.",
            ));
        }
    };

    // Resolve the resource class. Default to compute.s for agent runtimes.
    let full_class = match state.resource_class_catalog.classes().iter().find(|c| c.class == req.class) {
        Some(class) => format!("{}.{}", class.kind, class.class),
        None => format!("compute.{}", req.class),
    };

    let class = state
        .resource_class_catalog
        .get(&full_class)
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "unknown_class",
                format!("Unknown resource class: {full_class}"),
            )
        })?;

    let resource_id = Uuid::new_v4().to_string();
    let fabric_req = ResourceRequest {
        id: resource_id.clone(),
        kind: ResourceKind::Compute,
        class: class.class.clone(),
        display_name: req.display_name.clone().or_else(|| Some(format!("agent-{}", agent_id))),
        vcpu_min: class.vcpu,
        memory_mib_min: class.memory_mib,
        gpu_vram_mib_min: class.gpu_vram_mib,
        region_policy: req.region_policy.clone().unwrap_or_default(),
        latency_slo_ms: req.latency_slo_ms,
        deadline: None,
        reliability_tier: req.reliability_tier.unwrap_or(class.reliability_tier),
        image: None,
        model: None,
        runtime: None,
        storage_mib: 0,
        egress_policy: None,
        constraints: req.constraints.clone().unwrap_or_default(),
        labels: {
            let mut labels = std::collections::HashMap::new();
            labels.insert("agent_id".to_string(), agent_id.clone());
            labels.insert("managed_by".to_string(), "agent_cloud".to_string());
            labels
        },
        user_data: None,
    };

    info!(
        agent_id = %agent_id,
        resource_id = %resource_id,
        organization_id = %org,
        class = %full_class,
        "scheduling agent runtime"
    );

    // Route to the canonical OS control plane when one is configured; otherwise
    // fall back to the internal Cloud scheduler (preserves existing behavior and
    // local-only deployments).
    let scheduled = if let Some(os_client) = state.os_control_plane.as_ref() {
        provision_runtime_via_os(
            Arc::clone(&state),
            &org,
            &user,
            &req,
            &class,
            &full_class,
            &resource_id,
            os_client,
            &fabric_req,
        )
        .await?
    } else {
        state
            .fabric_scheduler
            .schedule(
                &org,
                &fabric_req,
                &state.resource_class_catalog,
                &state.fabric_provider_registry,
                &CreditsLedger::new(state.db.clone()),
                &PlacementRecorder::new(state.db.clone()),
            )
            .await
            .map_err(scheduler_error)?
    };

    // Update agent config with the provisioned Fabric resource id.
    let mut config: Value = existing_config
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    config["fabric_resource_id"] = json!(resource_id);
    config["runtime_status"] = json!("active");
    config["runtime_class"] = json!(full_class);

    let db = state.db.clone();
    let agent_id_for_update = agent_id.clone();
    let config_string = config.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE agents SET config = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            rusqlite::params![config_string, agent_id_for_update],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(json!({
        "agent_id": agent_id,
        "resource_id": resource_id,
        "provider_kind": scheduled.provider_kind,
        "provider_resource_id": scheduled.provider_resource_id,
        "region": scheduled.region,
        "instance_type": scheduled.instance_type,
        "ipv4": scheduled.ipv4,
        "endpoint": scheduled.endpoint,
        "status": "active",
        "runtime_status": "active",
    })))
}

async fn terminate_runtime(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    // Verify agent exists and belongs to the calling user.
    let db = state.db.clone();
    let agent_id_for_lookup = agent_id.clone();
    let user_id = user.user_id.clone();
    let agent = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, config FROM agents WHERE id = ?1",
        )?;
        let row: Option<(String, String, Option<String>)> = stmt
            .query_row(rusqlite::params![agent_id_for_lookup], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    let existing_config: Option<String> = match agent {
        Some((_, owner_id, config)) if owner_id == user_id => config,
        Some(_) => {
            return Err(error(
                StatusCode::FORBIDDEN,
                "agent_access_denied",
                "Agent belongs to a different user.",
            ));
        }
        None => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "agent_not_found",
                "Agent not found.",
            ));
        }
    };

    let config: Value = existing_config
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let resource_id = config
        .get("fabric_resource_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(resource_id) = resource_id {
        // Best-effort provider-side termination before closing the placement.
        let db = state.db.clone();
        let resource_id_for_placement = resource_id.clone();
        let provider_resource_id = tokio::task::spawn_blocking(move || {
            let manager = ResourceManager::new(db);
            manager.latest_placement(&resource_id_for_placement)
        })
        .await
        .map_err(internal)?
        .map_err(internal)?
        .and_then(|p| p.provider_resource_id);

        let db = state.db.clone();
        let resource_id_for_terminate = resource_id.clone();
        tokio::task::spawn_blocking(move || {
            let manager = ResourceManager::new(db);
            manager.terminate(&resource_id_for_terminate, "agent_runtime_terminated")
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;

        if let Some(provider_resource_id) = provider_resource_id {
            let registry = state.fabric_provider_registry.clone();
            let resource_id_for_warn = resource_id.clone();
            tokio::spawn(async move {
                for provider in registry.providers() {
                    if let Err(e) = provider.terminate(&provider_resource_id).await {
                        warn!(
                            resource_id = %resource_id_for_warn,
                            provider_resource_id = %provider_resource_id,
                            provider = %provider.kind(),
                            error = %e,
                            "provider termination failed"
                        );
                    }
                }
            });
        }
    }

    // Always update the agent config to mark runtime as terminated.
    let mut config = config;
    config["runtime_status"] = json!("terminated");

    let db = state.db.clone();
    let agent_id_for_update = agent_id.clone();
    let config_string = config.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE agents SET config = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            rusqlite::params![config_string, agent_id_for_update],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(json!({
        "agent_id": agent_id,
        "status": "terminated",
    })))
}

/// Provision a managed harness runtime for an agent.
///
/// When the agent's `harness_config.mode` is `cloud`, this route schedules a
/// canonical harness resource (e.g. `harness.gizzi`) through the OS Resource
/// Scheduler if one is configured, or through the internal Cloud scheduler
/// otherwise. The resulting resource id is stored in the agent's config so
/// subsequent session/chat routes can target the leased harness worker.
async fn provision_harness(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(agent_id): Path<String>,
    Json(req): Json<ProvisionHarnessRequest>,
) -> Result<Json<Value>, ApiError> {
    let org = user.organization_id.clone().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })?;

    // Verify agent exists and belongs to the calling user, and read its harness
    // config to confirm cloud-managed harness mode.
    let db = state.db.clone();
    let agent_id_for_lookup = agent_id.clone();
    let user_id = user.user_id.clone();
    let agent = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, config, harness_config FROM agents WHERE id = ?1",
        )?;
        let row: Option<(String, String, Option<String>, Option<String>)> = stmt
            .query_row(rusqlite::params![agent_id_for_lookup], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?;
        Ok::<_, rusqlite::Error>(row)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    let (existing_config, harness_config_json): (Option<String>, Option<String>) = match agent {
        Some((_id, owner_id, config, harness_config)) if owner_id == user_id => (config, harness_config),
        Some(_) => {
            return Err(error(
                StatusCode::FORBIDDEN,
                "agent_access_denied",
                "Agent belongs to a different user.",
            ));
        }
        None => {
            return Err(error(
                StatusCode::NOT_FOUND,
                "agent_not_found",
                "Agent not found.",
            ));
        }
    };

    let harness_config: Value = harness_config_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({"mode": "cloud"}));
    let harness_mode = harness_config.get("mode").and_then(|v| v.as_str()).unwrap_or("cloud");
    if harness_mode != "cloud" {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "harness_mode_unsupported",
            "Agent harness mode must be 'cloud' to provision a managed harness runtime.",
        ));
    }
    let harness_type = harness_config
        .get("harness")
        .or_else(|| harness_config.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("gizzi");
    // AllternitOS owns the canonical harness model; Cloud is the product layer
    // that provisions/sells harness runtimes. Gizzi and OpenCode are both
    // first-class harness backends today; additional harness types can be added
    // by extending the canonical `harness.<type>.session` capability contract.
    let harness_session_capability = match harness_type {
        "gizzi" => "harness.gizzi.session",
        "opencode" => "harness.opencode.session",
        "aider" => "harness.aider.session",
        "claude" => "harness.claude.session",
        "codex" => "harness.codex.session",
        "kimi" => "harness.kimi.session",
        "antigravity" => "harness.antigravity.session",
        "hermes" => "harness.hermes.session",
        "oh_my_pi" => "harness.oh_my_pi.session",
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "harness_type_unsupported",
                format!("Unsupported managed harness type: {harness_type}"),
            ));
        }
    };

    // Resolve the resource class. When the request does not explicitly specify
    // a class, derive it from the configured harness type so that an opencode
    // harness leases `harness.opencode` rather than defaulting to `harness.gizzi`.
    let harness_class = if req.class == default_harness_class() && harness_type != "gizzi" {
        harness_type.to_string()
    } else {
        req.class.clone()
    };
    let full_class = match state
        .resource_class_catalog
        .classes()
        .iter()
        .find(|c| c.class == harness_class)
    {
        Some(class) => format!("{}.{}", class.kind, class.class),
        None => format!("harness.{}", harness_class),
    };

    let class = state
        .resource_class_catalog
        .get(&full_class)
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "unknown_class",
                format!("Unknown resource class: {full_class}"),
            )
        })?;

    let resource_id = Uuid::new_v4().to_string();
    let fabric_req = ResourceRequest {
        id: resource_id.clone(),
        kind: ResourceKind::Harness,
        class: class.class.clone(),
        display_name: req
            .display_name
            .clone()
            .or_else(|| Some(format!("harness-{}-{}", harness_type, agent_id))),
        vcpu_min: class.vcpu,
        memory_mib_min: class.memory_mib,
        gpu_vram_mib_min: class.gpu_vram_mib,
        region_policy: RegionPolicy::Any,
        latency_slo_ms: None,
        deadline: None,
        reliability_tier: class.reliability_tier,
        image: None,
        model: None,
        runtime: Some(harness_type.to_string()),
        storage_mib: 0,
        egress_policy: None,
        constraints: CustomerConstraints::default(),
        labels: {
            let mut labels = std::collections::HashMap::new();
            labels.insert("agent_id".to_string(), agent_id.clone());
            labels.insert("managed_by".to_string(), "agent_cloud".to_string());
            labels.insert("harness_type".to_string(), harness_type.to_string());
            labels
        },
        user_data: None,
    };

    info!(
        agent_id = %agent_id,
        resource_id = %resource_id,
        organization_id = %org,
        class = %full_class,
        "scheduling managed harness runtime"
    );

    let scheduled = if let Some(os_client) = state.os_control_plane.as_ref() {
        provision_resource_via_os(
            Arc::clone(&state),
            &org,
            &user,
            req.display_name.as_deref(),
            &class,
            &full_class,
            &resource_id,
            os_client,
            &fabric_req,
            harness_session_capability,
            &["create".to_string()],
            "agent cloud harness provisioning",
        )
        .await?
    } else {
        state
            .fabric_scheduler
            .schedule(
                &org,
                &fabric_req,
                &state.resource_class_catalog,
                &state.fabric_provider_registry,
                &CreditsLedger::new(state.db.clone()),
                &PlacementRecorder::new(state.db.clone()),
            )
            .await
            .map_err(scheduler_error)?
    };

    let mut config: Value = existing_config
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    config["harness_resource_id"] = json!(resource_id);
    config["harness_status"] = json!("active");
    config["harness_class"] = json!(full_class);

    let db = state.db.clone();
    let agent_id_for_update = agent_id.clone();
    let config_string = config.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        conn.execute(
            "UPDATE agents SET config = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            rusqlite::params![config_string, agent_id_for_update],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(json!({
        "agent_id": agent_id,
        "resource_id": resource_id,
        "provider_kind": scheduled.provider_kind,
        "provider_resource_id": scheduled.provider_resource_id,
        "region": scheduled.region,
        "instance_type": scheduled.instance_type,
        "ipv4": scheduled.ipv4,
        "endpoint": scheduled.endpoint,
        "status": "active",
        "harness_status": "active",
        "harness_type": harness_type,
    })))
}

async fn provision_runtime_via_os(
    state: Arc<AppState>,
    org: &str,
    user: &AuthUser,
    req: &ProvisionRuntimeRequest,
    class: &crate::fabric::sku::ResourceClass,
    full_class: &str,
    resource_id: &str,
    os_client: &OsControlPlaneClient,
    fabric_req: &ResourceRequest,
) -> Result<ScheduledResource, ApiError> {
    provision_resource_via_os(
        state,
        org,
        user,
        req.display_name.as_deref(),
        class,
        full_class,
        resource_id,
        os_client,
        fabric_req,
        "agent.run",
        &["run".to_string()],
        "agent cloud runtime provisioning",
    )
    .await
}

/// Issue a canonical OS lease for a resource, record the placement in the Cloud
/// ledger, and place a credit hold (released on terminate; metered usage is the
/// sole balance charge). Shared by agent runtimes and managed harness runtimes.
async fn provision_resource_via_os(
    state: Arc<AppState>,
    org: &str,
    user: &AuthUser,
    display_name: Option<&str>,
    class: &crate::fabric::sku::ResourceClass,
    full_class: &str,
    resource_id: &str,
    os_client: &OsControlPlaneClient,
    fabric_req: &ResourceRequest,
    capability: &str,
    actions: &[String],
    purpose: &str,
) -> Result<ScheduledResource, ApiError> {
    let ledger = CreditsLedger::new(state.db.clone());
    let recorder = PlacementRecorder::new(state.db.clone());

    // Record the provisioning intent before placing a credit hold.
    recorder.record_resource(org, fabric_req).map_err(|e| {
        warn!(resource_id = %resource_id, error = %e, "failed to record resource intent");
        scheduler_error(e)
    })?;

    let estimated_cents = class.retail_price_per_hour_cents;
    let hold = ledger.hold(org, resource_id, estimated_cents).map_err(|e| {
        warn!(resource_id = %resource_id, error = %e, "failed to place credit hold for resource");
        scheduler_error(SchedulerError::Credits(e))
    })?;

    let lease_request = OsLeaseIssueRequest {
        requester_principal_id: format!("prn_{}", user.user_id),
        workload_id: resource_id.to_string(),
        step_id: None,
        capability: capability.to_string(),
        resource: None,
        resource_class_id: Some(full_class.to_string()),
        placement: None,
        actions: actions.to_vec(),
        purpose: purpose.to_string(),
        not_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
    };

    let issue_response = match os_client.issue_lease(lease_request).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!(resource_id = %resource_id, error = %e, "os control plane lease issue failed");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(resource_id, "os_lease_failed");
            return Err(map_os_client_error(e));
        }
    };

    let lease_record = match os_client.get_lease(&issue_response.lease_id).await {
        Ok(record) => record,
        Err(e) => {
            warn!(resource_id = %resource_id, lease_id = %issue_response.lease_id, error = %e, "failed to fetch os lease record");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(resource_id, "os_lease_fetch_failed");
            return Err(map_os_client_error(e));
        }
    };

    let placement = match lease_record.placement {
        Some(p) => p,
        None => {
            warn!(resource_id = %resource_id, lease_id = %issue_response.lease_id, "os lease returned without placement");
            let _ = ledger.release_hold(&hold.id);
            let _ = recorder.mark_terminated(resource_id, "os_lease_missing_placement");
            return Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "os_scheduling_error",
                "OS lease did not include a placement",
            ));
        }
    };

    if let Err(e) = recorder.record_os_placement(
        org,
        resource_id,
        &class.kind.to_string(),
        &class.class,
        display_name,
        Some(&issue_response.lease_id),
        &placement,
        &hold.id,
    ) {
        warn!(resource_id = %resource_id, error = %e, "failed to record os placement");
        let _ = ledger.release_hold(&hold.id);
        let _ = recorder.mark_terminated(resource_id, "record_os_placement_failed");
        return Err(scheduler_error(e));
    }

    // The hold stays open for the resource's lifetime (reduces
    // available_cents, blocking new provisioning) and is released on
    // terminate. Metered usage is the sole balance charge — charging the
    // provisioning estimate here would double-bill.
    info!(resource_id = %resource_id, hold_id = %hold.id, "hold open for resource lifetime; metered usage charges apply");

    Ok(ScheduledResource {
        resource_id: resource_id.to_string(),
        provider_kind: placement.provider_kind.clone(),
        provider_resource_id: placement.provider_resource_id.unwrap_or_default(),
        offer_id: placement.offer_id.clone(),
        region: placement.region.clone(),
        instance_type: placement.instance_type.clone(),
        ipv4: placement.ipv4.clone(),
        endpoint: placement.endpoint.clone(),
    })
}

fn map_os_client_error(err: OsClientError) -> ApiError {
    match err {
        OsClientError::ControlPlane { status, body } => {
            error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", format!("OS control plane error {status}: {body}"))
        }
        other => error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::path::Path;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::{
        auth::{AuthConfig, AuthUser},
        config::AppConfig,
        db::DbHandle,
        fabric,
        rails::RailsState,
    };
    use allternit_computer_cloud::providers::fake::fake_cpu_provider;

    fn seed_org_user(conn: &rusqlite::Connection, org_id: &str, user_id: &str, role: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, ?2)",
            rusqlite::params![org_id, "Test Org"],
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
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .extension(user)
            .body(body)
            .unwrap()
    }

    async fn app_state_with_fake_provider(temp: &Path) -> Arc<AppState> {
        let config = AppConfig {
            company: crate::config::CompanyConfig::default(),
            user: crate::config::UserConfig::default(),
        };
        let db = DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        let resource_class_catalog = fabric::sku::ResourceClassCatalog::from_db(&db)
            .expect("initialize resource class catalog");
        let fabric_node_pool = std::sync::Arc::new(
            allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
        );
        let fabric_node_provider =
            allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                fabric_node_pool,
                "__system".to_string(),
            );
        let mut fabric_provider_registry = allternit_computer_cloud::fabric::FabricProviderRegistry::empty();
        fabric_provider_registry.register(std::sync::Arc::new(fake_cpu_provider()));

        let fabric_price_cache = fabric::PriceCache::new(db.clone());
        let fabric_scheduler = fabric::Scheduler::new(fabric::CostEngine::default_engine())
            .with_price_cache(fabric_price_cache.clone());

        Arc::new(AppState {
            config,
            db,
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(RwLock::new(HashMap::new())),
            computer_guest_tokens: Arc::new(RwLock::new(HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog,
            fabric_node_provider,
            fabric_provider_registry,
            fabric_scheduler,
            fabric_price_cache,
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    fn seed_agent(db: &DbHandle, user_id: &str, name: &str) -> String {
        seed_agent_with_harness(db, user_id, name, r#"{"mode":"cloud"}"#)
    }

    fn seed_agent_with_harness(db: &DbHandle, user_id: &str, name: &str, harness_config: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, description, type, model, provider,
                                capabilities, system_prompt, tools, max_iterations, temperature,
                                config, status, trust_tier, harness_config, enabled_modes, mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            rusqlite::params![
                &id,
                user_id,
                name,
                "Test agent for runtime provisioning",
                "worker",
                "gpt-4o-mini",
                "openai",
                "[]",
                None::<String>,
                "[]",
                10,
                0.7,
                "{}",
                "idle",
                "standard",
                harness_config,
                "[\"chat\"]",
                "primary",
            ],
        )
        .unwrap();
        id
    }

    #[tokio::test]
    async fn provision_runtime_schedules_fabric_resource() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent(&state.db, "owner-1", "Test Agent");

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["runtime_status"], "active");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());

        // Config should now hold the resource id and status.
        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["runtime_status"], "active");
        assert!(!config["fabric_resource_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provision_runtime_rejects_missing_org() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let agent_id = seed_agent(&state.db, "owner-1", "Test Agent");

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/provision", agent_id),
                auth_user(None, "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provision_runtime_rejects_other_users_agent() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        seed_org_user(&conn, "org-1", "owner-2", "owner");
        drop(conn);

        let agent_id = seed_agent(&state.db, "owner-1", "Test Agent");

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/provision", agent_id),
                auth_user(Some("org-1"), "owner-2"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn terminate_runtime_closes_placement_and_updates_config() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent(&state.db, "owner-1", "Test Agent");

        let app = router().with_state(state.clone());
        let resp = app
            .clone()
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/terminate", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "terminated");

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["runtime_status"], "terminated");
    }

    // OS-backed integration test: agent runtime provisioning routes through the
    // canonical AllternitOS control plane and records the canonical placement.
    use tokio::io::AsyncBufReadExt;
    use tokio::process::Command;

    /// Returns None (and the test should skip) when the sidecar binary is not
    /// built — e.g. CI runners without the AllternitOS workspace checkout.
    async fn spawn_real_os_control_plane() -> Option<(String, tokio::process::Child)> {
        let temp = tempfile::tempdir().unwrap().keep();
        let bin = std::env::var("ALLTERNITOS_CONTROL_PLANE_BIN").unwrap_or_else(|_| {
            "~/Desktop/AllternitOS/target/debug/allternitos_control_plane".to_string()
        });
        if !std::path::Path::new(&bin).exists() {
            eprintln!(
                "skipping: allternitos_control_plane binary not found at {bin} \
                 (set ALLTERNITOS_CONTROL_PLANE_BIN to run this test)"
            );
            return None;
        }
        let db_path = temp.join("cp.db");
        let mut child = Command::new(&bin)
            .arg("--bind")
            .arg("127.0.0.1:0")
            .arg("--reconciler-path")
            .arg(&db_path)
            .arg("--fake-provider")
            .env("RUST_LOG", "info")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
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

        let client = reqwest::Client::new();
        for _ in 0..50 {
            if client.get(format!("{}/health", url)).send().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        Some((url, child))
    }

    #[tokio::test]
    async fn provision_runtime_routes_through_real_os_control_plane() {
        let Some((url, _child)) = spawn_real_os_control_plane().await else {
            return;
        };
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        fabric::credits::CreditsLedger::new(state.db.clone())
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent(&state.db, "owner-1", "Test Agent");

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/runtime/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());
        assert!(
            body["provider_kind"].as_str().map_or(false, |s| !s.is_empty()),
            "expected provider_kind in response, got {:?}",
            body["provider_kind"]
        );

        // Verify the Cloud DB records the canonical OS placement.
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

        // Agent config should hold the OS-scheduled resource id.
        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["runtime_status"], "active");
        assert_eq!(config["fabric_resource_id"], resource_id);
    }

    #[tokio::test]
    async fn provision_harness_schedules_fabric_resource() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"gizzi"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["harness_status"], "active");
        assert_eq!(body["harness_type"], "gizzi");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["harness_status"], "active");
        assert_eq!(config["harness_class"], "harness.gizzi");
        assert!(!config["harness_resource_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provision_harness_rejects_non_cloud_mode() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"local"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn provision_harness_routes_through_real_os_control_plane() {
        let Some((url, _child)) = spawn_real_os_control_plane().await else {
            return;
        };
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        fabric::credits::CreditsLedger::new(state.db.clone())
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"gizzi"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["harness_status"], "active");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());
        assert!(
            body["provider_kind"].as_str().map_or(false, |s| !s.is_empty()),
            "expected provider_kind in response, got {:?}",
            body["provider_kind"]
        );

        // Verify the Cloud DB records the canonical OS placement.
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

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["harness_status"], "active");
        assert_eq!(config["harness_class"], "harness.gizzi");
        assert_eq!(config["harness_resource_id"], resource_id);
    }

    #[tokio::test]
    async fn provision_harness_reconciles_os_usage_event_to_credits() {
        use crate::fabric::usage::UsageIngestor;

        let Some((url, _child)) = spawn_real_os_control_plane().await else {
            return;
        };
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        fabric::sku::ResourceClassCatalog::seed_builtin(&state.db).unwrap();
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"gizzi"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let resource_id = body["resource_id"].as_str().unwrap();

        // Look up the canonical OS placement id recorded by Cloud.
        let manager = ResourceManager::new(state.db.clone());
        let placement = manager
            .latest_placement(resource_id)
            .unwrap()
            .expect("placement exists");

        // Simulate the OS node-agent pushing a harness usage event for the
        // provisioned resource and placement.
        let ingestor = UsageIngestor::new(state.db.clone());
        let event_id = ingestor
            .record_usage_event(
                resource_id,
                "harness.gizzi.session",
                1.0,
                "request",
                Some(Utc::now()),
                Some(&placement.id),
            )
            .unwrap();
        ingestor.process_event(&event_id, &ledger).unwrap();

        // Provisioning only opens a hold (no balance charge); the harness
        // request consumed 5 cents of metered usage.
        // 10_000 - 5 = 9_995, and the hold remains open.
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 9_995);
        assert!(ledger.held_cents("org-1").unwrap() > 0);

        // The usage event should be marked processed and linked to a cost event.
        let conn = state.db.connect().unwrap();
        let cost_event_id: Option<String> = conn
            .query_row(
                "SELECT cost_event_id FROM fabric_usage_events WHERE id = ?1",
                rusqlite::params![&event_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cost_event_id.is_some());
    }

    #[tokio::test]
    async fn provision_harness_opencode_schedules_fabric_resource() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"opencode"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["harness_status"], "active");
        assert_eq!(body["harness_type"], "opencode");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["harness_status"], "active");
        assert_eq!(config["harness_class"], "harness.opencode");
        assert!(!config["harness_resource_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn provision_harness_opencode_routes_through_real_os_control_plane() {
        let Some((url, _child)) = spawn_real_os_control_plane().await else {
            return;
        };
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state_with_driver_and_os(&temp, None, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        fabric::credits::CreditsLedger::new(state.db.clone())
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"opencode"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["harness_status"], "active");
        assert_eq!(body["harness_type"], "opencode");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());
        assert!(
            body["provider_kind"].as_str().map_or(false, |s| !s.is_empty()),
            "expected provider_kind in response, got {:?}",
            body["provider_kind"]
        );

        // Verify the Cloud DB records the canonical OS placement.
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

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["harness_status"], "active");
        assert_eq!(config["harness_class"], "harness.opencode");
        assert_eq!(config["harness_resource_id"], resource_id);
    }

    #[tokio::test]
    async fn provision_harness_aider_schedules_fabric_resource() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = fabric::credits::CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let agent_id = seed_agent_with_harness(&state.db, "owner-1", "Test Agent", r#"{"mode":"cloud","harness":"aider"}"#);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/agents/{}/harness/provision", agent_id),
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agent_id"], agent_id);
        assert_eq!(body["status"], "active");
        assert_eq!(body["harness_status"], "active");
        assert_eq!(body["harness_type"], "aider");
        assert!(!body["resource_id"].as_str().unwrap().is_empty());

        let conn = state.db.connect().unwrap();
        let config: String = conn
            .query_row(
                "SELECT config FROM agents WHERE id = ?1",
                rusqlite::params![agent_id],
                |row| row.get(0),
            )
            .unwrap();
        let config: Value = serde_json::from_str(&config).unwrap();
        assert_eq!(config["harness_status"], "active");
        assert_eq!(config["harness_class"], "harness.aider");
        assert!(!config["harness_resource_id"].as_str().unwrap().is_empty());
    }
}
