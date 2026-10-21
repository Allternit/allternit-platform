//! Fabric resources API — customer-facing create/get/terminate.
//!
//! Mounted under `/api/v1` so public paths land at `/api/v1/fabric/resources`.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    fabric::{
        credits::{CreditsError, CreditsLedger},
        os_client::{OsClientError, OsControlPlaneClient, OsLeaseIssueRequest},
        resources::{FabricPlacementSummary, FabricResource, ResourceManager},
        scheduler::{PlacementRecorder, SchedulerError, ScheduledResource},
    },
    AppState,
};
use allternit_computer_cloud::fabric::{
    CustomerConstraints, RegionPolicy, ReliabilityTier, ResourceKind, ResourceRequest,
};
use chrono::Utc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/fabric/resources", post(create_resource))
        .route("/fabric/resources/:id", get(get_resource))
        .route("/fabric/resources/:id/terminate", post(terminate_resource))
        .route("/fabric/resource-classes", get(list_resource_classes))
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
    tracing::warn!(error = %err, "fabric resource operation failed");
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

fn require_org(user: &AuthUser) -> Result<String, ApiError> {
    user.organization_id.clone().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct CreateResourceRequest {
    class: String,
    #[serde(default)]
    kind: Option<String>,
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

/// Build a Cloud `ResourceRequest` from a catalog class. Exported so the Model
/// Gateway can schedule inference capacity through the same path as the fabric
/// resources route.
pub(crate) fn fabric_request_from_class(
    resource_id: &str,
    class: &crate::fabric::sku::ResourceClass,
    display_name: Option<String>,
    region_policy: RegionPolicy,
    latency_slo_ms: Option<u64>,
    reliability_tier: Option<ReliabilityTier>,
) -> ResourceRequest {
    ResourceRequest {
        id: resource_id.to_string(),
        kind: class.kind,
        class: class.class.clone(),
        display_name,
        vcpu_min: class.vcpu,
        memory_mib_min: class.memory_mib,
        gpu_vram_mib_min: class.gpu_vram_mib,
        region_policy,
        latency_slo_ms,
        deadline: None,
        reliability_tier: reliability_tier.unwrap_or(class.reliability_tier),
        image: None,
        model: None,
        runtime: None,
        storage_mib: 0,
        egress_policy: None,
        constraints: CustomerConstraints::default(),
        labels: std::collections::HashMap::new(),
        user_data: None,
    }
}

async fn create_resource(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateResourceRequest>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;

    if req.class.is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "class_required", "Resource class is required."));
    }

    // Resolve kind from explicit input or from the class catalog.
    let full_class = match &req.kind {
        Some(kind) => format!("{}.{}", kind, req.class),
        None => {
            let classes = state.resource_class_catalog.classes();
            let class = classes
                .iter()
                .find(|c| c.class == req.class)
                .ok_or_else(|| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "unknown_class",
                        format!("Unknown resource class: {}", req.class),
                    )
                })?;
            format!("{}.{}", class.kind, class.class)
        }
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
    let fabric_req = fabric_request_from_class(&resource_id, &class, req.display_name.clone(), req.region_policy.clone().unwrap_or_default(), req.latency_slo_ms, req.reliability_tier);

    info!(
        resource_id = %resource_id,
        organization_id = %org,
        class = %full_class,
        "scheduling fabric resource"
    );

    // Route to the canonical OS control plane when one is configured; otherwise
    // fall back to the internal Cloud scheduler (preserves existing behavior and
    // local-only deployments).
    if let Some(os_client) = state.os_control_plane.as_ref() {
        create_resource_via_os(
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
        .await
    } else {
        let scheduled = state
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
            .map_err(scheduler_error)?;

        Ok(Json(scheduled_json(&scheduled)))
    }
}

/// Map a Cloud resource kind to the canonical capability/actions used for OS
/// lease validation. This is a product-side convention: the OS scheduler decides
/// *where* the capacity runs, while Cloud decides which capability gate the
/// lease grants.
fn capability_for_kind(kind: ResourceKind) -> (String, Vec<String>) {
    match kind {
        ResourceKind::Compute => ("compute.exec".to_string(), vec!["execute".to_string()]),
        ResourceKind::Gpu => ("model.generate".to_string(), vec!["generate".to_string()]),
        ResourceKind::Sandbox => ("sandbox.run".to_string(), vec!["run".to_string()]),
        ResourceKind::Inference => ("model.generate".to_string(), vec!["generate".to_string()]),
        ResourceKind::Agent => ("agent.run".to_string(), vec!["run".to_string()]),
        ResourceKind::Batch => ("batch.run".to_string(), vec!["run".to_string()]),
        ResourceKind::Cluster => ("cluster.run".to_string(), vec!["run".to_string()]),
        ResourceKind::Storage => (
            "storage.access".to_string(),
            vec!["read".to_string(), "write".to_string()],
        ),
        ResourceKind::Harness => ("harness.session".to_string(), vec!["create".to_string()]),
    }
}

async fn create_resource_via_os(
    state: Arc<AppState>,
    org: &str,
    user: &AuthUser,
    req: &CreateResourceRequest,
    class: &crate::fabric::sku::ResourceClass,
    full_class: &str,
    resource_id: &str,
    os_client: &OsControlPlaneClient,
    fabric_req: &ResourceRequest,
) -> Result<Json<Value>, ApiError> {
    let ledger = CreditsLedger::new(state.db.clone());
    let recorder = PlacementRecorder::new(state.db.clone());

    // Record the provisioning intent before placing a credit hold.
    recorder.record_resource(org, fabric_req).map_err(|e| {
        warn!(resource_id = %resource_id, error = %e, "failed to record resource");
        scheduler_error(e)
    })?;

    let estimated_cents = class.retail_price_per_hour_cents;
    let hold = ledger.hold(org, resource_id, estimated_cents).map_err(|e| {
        warn!(resource_id = %resource_id, error = %e, "failed to place credit hold");
        scheduler_error(SchedulerError::Credits(e))
    })?;

    let (capability, actions) = capability_for_kind(class.kind);
    let lease_request = OsLeaseIssueRequest {
        requester_principal_id: format!("prn_{}", user.user_id),
        workload_id: resource_id.to_string(),
        step_id: None,
        capability,
        resource: None,
        resource_class_id: Some(full_class.to_string()),
        placement: None,
        actions,
        purpose: "fabric resource provisioning".to_string(),
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

    // The OS issue response does not include the placement; fetch the lease
    // record to obtain the canonical placement chosen by the OS scheduler.
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
        req.display_name.as_deref(),
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
    // terminate. Metered usage via the usage worker is the sole balance
    // charge — charging the provisioning estimate here would double-bill.
    info!(resource_id = %resource_id, hold_id = %hold.id, "hold open for resource lifetime; metered usage charges apply");

    let scheduled = ScheduledResource {
        resource_id: resource_id.to_string(),
        provider_kind: placement.provider_kind.clone(),
        provider_resource_id: placement.provider_resource_id.unwrap_or_default(),
        offer_id: placement.offer_id.clone(),
        region: placement.region.clone(),
        instance_type: placement.instance_type.clone(),
        ipv4: placement.ipv4.clone(),
        endpoint: placement.endpoint.clone(),
    };

    Ok(Json(scheduled_json(&scheduled)))
}

fn map_os_client_error(err: OsClientError) -> ApiError {
    match err {
        OsClientError::ControlPlane { status, body } => {
            error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", format!("OS control plane error {status}: {body}"))
        }
        other => error(StatusCode::SERVICE_UNAVAILABLE, "os_scheduling_error", other.to_string()),
    }
}

fn scheduled_json(scheduled: &ScheduledResource) -> Value {
    json!({
        "resource_id": scheduled.resource_id,
        "provider_kind": scheduled.provider_kind,
        "provider_resource_id": scheduled.provider_resource_id,
        "region": scheduled.region,
        "instance_type": scheduled.instance_type,
        "ipv4": scheduled.ipv4,
        "endpoint": scheduled.endpoint,
        "status": "active",
    })
}

async fn get_resource(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;

    let db = state.db.clone();
    let id_for_manager = id.clone();
    let resource = tokio::task::spawn_blocking(move || {
        let manager = ResourceManager::new(db);
        manager.get(&id_for_manager)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "resource_not_found", "No such resource"))?;

    if resource.organization_id != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "wrong_organization",
            "Resource belongs to a different organization.",
        ));
    }

    let db = state.db.clone();
    let id_for_placement = id.clone();
    let placement = tokio::task::spawn_blocking(move || {
        let manager = ResourceManager::new(db);
        manager.latest_placement(&id_for_placement)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    Ok(Json(resource_json(&resource, placement.as_ref())))
}

fn resource_json(resource: &FabricResource, placement: Option<&FabricPlacementSummary>) -> Value {
    let mut value = json!({
        "id": resource.id,
        "organization_id": resource.organization_id,
        "kind": resource.kind,
        "class": resource.class,
        "display_name": resource.display_name,
        "status": resource.status,
        "provider_kind": resource.provider_kind,
        "provider_resource_id": resource.provider_resource_id,
        "region": resource.region,
        "requested_at": resource.requested_at.to_rfc3339(),
        "provisioned_at": resource.provisioned_at.map(|d| d.to_rfc3339()),
        "terminated_at": resource.terminated_at.map(|d| d.to_rfc3339()),
    });

    if let Some(p) = placement {
        fn cents(maybe: &Option<allternitos_cloud_contracts::Money>) -> Option<i64> {
            maybe.as_ref().map(|m| m.minor_units as i64)
        }
        value["placement"] = json!({
            "id": p.id,
            "resource_id": p.resource_id,
            "provider_kind": p.provider_kind,
            "provider_resource_id": p.provider_resource_id,
            "offer_id": p.offer_id,
            "instance_type": p.instance_type,
            "region": p.region,
            "node_id": p.node_id,
            "ipv4": p.ipv4,
            "endpoint": p.endpoint,
            "retail_price_per_hour_cents": cents(&p.retail_price_per_hour),
            "provider_cost_per_hour_cents": cents(&p.provider_cost_per_hour),
            "retail_price_per_request_cents": cents(&p.retail_price_per_request),
            "provider_cost_per_request_cents": cents(&p.provider_cost_per_request),
            "retail_price_per_token_cents": cents(&p.retail_price_per_token),
            "provider_cost_per_token_cents": cents(&p.provider_cost_per_token),
            "status": p.status,
            "started_at": p.started_at.to_rfc3339(),
            "ended_at": p.ended_at.map(|d| d.to_rfc3339()),
            "termination_reason": p.termination_reason,
        });
    }

    value
}

async fn terminate_resource(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let org = require_org(&user)?;

    let db = state.db.clone();
    let id_for_check = id.clone();
    let belongs = tokio::task::spawn_blocking(move || {
        let manager = ResourceManager::new(db);
        manager.belongs_to_org(&id_for_check, &org)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    if !belongs {
        return Err(error(
            StatusCode::FORBIDDEN,
            "wrong_organization",
            "Resource belongs to a different organization.",
        ));
    }

    // Find the provider resource id from the latest open placement so we can
    // attempt best-effort provider-side termination.
    let db = state.db.clone();
    let id_for_placement = id.clone();
    let provider_resource_id = tokio::task::spawn_blocking(move || {
        let manager = ResourceManager::new(db);
        manager.latest_placement(&id_for_placement)
    })
    .await
    .map_err(internal)?
    .map_err(internal)?
    .and_then(|p| p.provider_resource_id);

    let db = state.db.clone();
    let id_for_terminate = id.clone();
    tokio::task::spawn_blocking(move || {
        let manager = ResourceManager::new(db);
        manager.terminate(&id_for_terminate, "user_request")
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    // Release any credit holds still open for this resource. Metered usage
    // charges remain on the ledger; the hold only reserved availability.
    let db = state.db.clone();
    let id_for_holds = id.clone();
    let id_for_hold_warn = id.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let ledger = CreditsLedger::new(db);
        let mut stmt = conn.prepare(
            "SELECT id FROM fabric_credit_holds WHERE resource_id = ?1 AND status = 'held'",
        )?;
        let hold_ids: Vec<String> = stmt
            .query_map(rusqlite::params![id_for_holds], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for hold_id in hold_ids {
            if let Err(e) = ledger.release_hold(&hold_id) {
                warn!(resource_id = %id_for_hold_warn, hold_id = %hold_id, error = %e, "failed to release hold on terminate");
            }
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;

    if let Some(provider_resource_id) = provider_resource_id {
        let registry = state.fabric_provider_registry.clone();
        let resource_id_for_warn = id.clone();
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

    Ok(Json(json!({
        "id": id,
        "status": "terminated",
    })))
}

async fn list_resource_classes(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<Value>, ApiError> {
    let _org = require_org(&user)?;
    let classes = state.resource_class_catalog.classes();
    Ok(Json(json!({ "classes": classes })))
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

    async fn app_state_with_fake_provider_impl(
        temp: &Path,
        os_control_plane: Option<crate::fabric::os_client::OsControlPlaneClient>,
    ) -> Arc<AppState> {
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
            os_control_plane,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn app_state_with_fake_provider(temp: &Path) -> Arc<AppState> {
        app_state_with_fake_provider_impl(temp, None).await
    }

    #[tokio::test]
    async fn create_resource_schedules_and_returns_placement() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "active");
        assert_eq!(body["provider_kind"], "fake");
        assert!(
            body["provider_resource_id"]
                .as_str()
                .unwrap_or("")
                .starts_with("fake-")
        );
    }

    #[tokio::test]
    async fn create_resource_rejects_unknown_class() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "zzzzz"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_resource_rejects_insufficient_credits() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn get_resource_returns_resource_and_placement() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .clone()
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s", "display_name": "my-server"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let create_body = body_json(resp.into_body()).await;
        let resource_id = create_body["resource_id"].as_str().unwrap();

        let resp = app
            .oneshot(build_request(
                "GET",
                &format!("/fabric/resources/{}", resource_id),
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["id"], resource_id);
        assert_eq!(body["display_name"], "my-server");
        assert_eq!(body["status"], "active");
        assert!(body["placement"].is_object());
    }

    #[tokio::test]
    async fn get_resource_forbidden_for_other_org() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        seed_org_user(&conn, "org-2", "owner-2", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .clone()
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let create_body = body_json(resp.into_body()).await;
        let resource_id = create_body["resource_id"].as_str().unwrap();

        let resp = app
            .oneshot(build_request(
                "GET",
                &format!("/fabric/resources/{}", resource_id),
                auth_user(Some("org-2"), "owner-2"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn terminate_resource_closes_placement() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .clone()
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let create_body = body_json(resp.into_body()).await;
        let resource_id = create_body["resource_id"].as_str().unwrap();

        let resp = app
            .oneshot(build_request(
                "POST",
                &format!("/fabric/resources/{}/terminate", resource_id),
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["id"], resource_id);
        assert_eq!(body["status"], "terminated");

        let manager = ResourceManager::new(state.db.clone());
        let resource = manager.get(resource_id).unwrap().expect("resource exists");
        assert_eq!(resource.status, "terminated");
        let placement = manager.latest_placement(resource_id).unwrap().expect("placement exists");
        assert!(placement.ended_at.is_some());

        // Terminate must release the provisioning hold so available credits
        // are restored without charging the balance.
        assert_eq!(ledger.held_cents("org-1").unwrap(), 0);
        assert_eq!(ledger.balance_cents("org-1").unwrap(), 10_000);
        assert_eq!(ledger.available_cents("org-1").unwrap(), 10_000);
    }

    #[tokio::test]
    async fn list_resource_classes_returns_classes() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider(&temp).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "GET",
                "/fabric/resource-classes",
                auth_user(Some("org-1"), "owner-1"),
                None,
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let classes = body["classes"].as_array().unwrap();
        assert!(!classes.is_empty());
        assert!(classes.iter().any(|c| c["class"] == "s"));
    }

    // OS-backed resource creation test helpers.
    use crate::fabric::os_client::{OsLeaseIssueResponse, OsLeaseRecord};
    use axum::{routing::get, Json};
    use std::net::SocketAddr;
    use tokio::io::AsyncBufReadExt;
    use tokio::net::TcpListener;
    use tokio::process::Command;

    async fn mock_os_issue_lease(
        Json(req): Json<OsLeaseIssueRequest>,
    ) -> Json<OsLeaseIssueResponse> {
        assert_eq!(req.resource_class_id, Some("compute.s".to_string()));
        Json(OsLeaseIssueResponse {
            lease_id: "lease_os_123".to_string(),
            state: "active".to_string(),
            issued_at: Utc::now().to_rfc3339(),
            not_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            token: "tok_os_123".to_string(),
        })
    }

    async fn mock_os_get_lease(axum::extract::Path(lease_id): axum::extract::Path<String>) -> Json<OsLeaseRecord> {
        Json(OsLeaseRecord {
            lease_id,
            state: "active".to_string(),
            requester_principal_id: "prn_owner-1".to_string(),
            workload_id: "res_test".to_string(),
            step_id: None,
            capability: "compute.exec".to_string(),
            resource: None,
            resource_class_id: Some("compute.s".to_string()),
            placement: Some(allternitos_cloud_contracts::Placement {
                id: "plc_os_123".to_string(),
                resource_id: "res_test".to_string(),
                node_id: None,
                offer_id: "off_os_abc".to_string(),
                provider_kind: "os_fake".to_string(),
                provider_resource_id: Some("os-res-1".to_string()),
                region: "us-east".to_string(),
                instance_type: "os-fake-small".to_string(),
                ipv4: Some("10.0.0.1".to_string()),
                endpoint: Some("http://10.0.0.1".to_string()),
                retail_price_per_hour: Some(allternitos_cloud_contracts::Money {
                    currency: "USD".to_string(),
                    minor_units: 5,
                }),
                provider_cost_per_hour: Some(allternitos_cloud_contracts::Money {
                    currency: "USD".to_string(),
                    minor_units: 3,
                }),
                retail_price_per_request: None,
                provider_cost_per_request: None,
                retail_price_per_token: None,
                provider_cost_per_token: None,
                hold_id: None,
                status: "active".to_string(),
                started_at: Utc::now(),
                ended_at: None,
                termination_reason: None,
                created_at: Some(Utc::now()),
                updated_at: None,
                labels: std::collections::HashMap::new(),
            }),
            actions: vec!["execute".to_string()],
            purpose: "fabric resource provisioning".to_string(),
            issued_at: Utc::now().to_rfc3339(),
            not_after: (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            token: "tok_os_123".to_string(),
        })
    }

    fn mock_os_app() -> axum::Router {
        axum::Router::new()
            .route("/v1/leases/issue", post(mock_os_issue_lease))
            .route("/v1/leases/:lease_id", get(mock_os_get_lease))
    }

    async fn start_mock_os_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, mock_os_app()).await.unwrap();
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn create_resource_routes_through_os_control_plane() {
        let (addr, _handle) = start_mock_os_server().await;
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(format!("http://{}", addr));

        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider_impl(&temp, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "active");
        assert_eq!(body["provider_kind"], "os_fake");
        assert_eq!(body["instance_type"], "os-fake-small");
        assert_eq!(body["provider_resource_id"], "os-res-1");

        // Verify the Cloud DB records the canonical OS placement.
        let manager = ResourceManager::new(state.db.clone());
        let resource_id = body["resource_id"].as_str().unwrap();
        let resource = manager.get(resource_id).unwrap().expect("resource exists");
        assert_eq!(resource.provider_kind.as_deref(), Some("os_fake"));
        let placement = manager.latest_placement(resource_id).unwrap().expect("placement exists");
        assert_eq!(placement.offer_id, "off_os_abc");
        assert_eq!(placement.instance_type, "os-fake-small");
    }

    /// Spawn the real AllternitOS control-plane binary with the fake provider
    /// and return its HTTP origin plus the child handle.
    async fn spawn_real_os_control_plane() -> (String, tokio::process::Child) {
        let temp = tempfile::tempdir().unwrap().keep();
        let bin = std::env::var("ALLTERNITOS_CONTROL_PLANE_BIN").unwrap_or_else(|_| {
            "~/Desktop/AllternitOS/target/debug/allternitos_control_plane".to_string()
        });
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

        // Wait until /health is reachable; the log line is emitted just before
        // the accept loop starts, so a tiny poll prevents 503 race conditions.
        let client = reqwest::Client::new();
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
    async fn create_resource_routes_through_real_os_control_plane() {
        let (url, _child) = spawn_real_os_control_plane().await;
        let os_client = crate::fabric::os_client::OsControlPlaneClient::new(url);

        let temp = tempfile::tempdir().unwrap().keep();
        let state = app_state_with_fake_provider_impl(&temp, Some(os_client)).await;
        let conn = state.db.connect().unwrap();
        seed_org_user(&conn, "org-1", "owner-1", "owner");
        drop(conn);

        let ledger = CreditsLedger::new(state.db.clone());
        ledger
            .credit("org-1", 10_000, fabric::credits::TransactionType::Purchase, None, None, None, None)
            .unwrap();

        let app = router().with_state(state.clone());
        let resp = app
            .oneshot(build_request(
                "POST",
                "/fabric/resources",
                auth_user(Some("org-1"), "owner-1"),
                Some(json!({"class": "s"})),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], "active");
        assert_eq!(body["provider_kind"], "fake");
        assert!(
            body["provider_resource_id"]
                .as_str()
                .unwrap_or("")
                .starts_with("fake-"),
            "expected fake provider_resource_id, got {:?}",
            body["provider_resource_id"]
        );
        assert!(
            !body["instance_type"].as_str().unwrap_or("").is_empty(),
            "expected real instance_type"
        );

        // Verify the Cloud DB records the canonical OS placement (no off_unknown).
        let manager = ResourceManager::new(state.db.clone());
        let resource_id = body["resource_id"].as_str().unwrap();
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
    }
}
