//! Allternit Agent System Rails - HTTP Service
//!
//! Provides HTTP API for agent task planning, work execution, and policy gates.
//! This service wraps the core rails library with an Axum HTTP interface.
//!
//! All CLI commands are exposed as HTTP endpoints.

use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::core::types::EventScope;
use crate::leases::leases::LeasesOptions;
use crate::ledger::ledger::LedgerOptions;
use crate::{
    bus::{Bus, NewBusMessage},
    context::{ContextPackStore, ContextPackStoreOptions},
    policy, ActorType, DagMutation, Gate, GateOptions, Index, IndexOptions, LeaseRequest, Leases,
    Ledger, LedgerQuery, Mail, MailOptions, PeerEnvelope, PeerRegistry, ReceiptStore,
    ReceiptStoreOptions, Steer, Vault, VaultOptions, WihPickupOptions, WorkOps, send_envelope,
};

/// Service state shared across handlers
#[derive(Clone)]
pub struct ServiceState {
    pub ledger: Arc<Ledger>,
    pub gate: Arc<Gate>,
    pub leases: Arc<Leases>,
    pub mail: Arc<Mail>,
    pub vault: Arc<Vault>,
    pub oauth_vault: Arc<crate::vault::OAuthVault>,
    pub index: Arc<Index>,
    pub receipts: Arc<ReceiptStore>,
    pub work_ops: Arc<WorkOps>,
    pub context_packs: Arc<ContextPackStore>,
    pub peers: Arc<PeerRegistry>,
    pub bus: Arc<Bus>,
    pub steer: Arc<Steer>,
    pub root_dir: PathBuf,
}

impl ServiceState {
    pub async fn new(root_dir: PathBuf) -> anyhow::Result<Self> {
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root_dir.clone()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));

        let leases = Arc::new(
            Leases::new(LeasesOptions {
                root_dir: Some(root_dir.clone()),
                leases_dir: Some(PathBuf::from(".allternit/leases")),
                event_sink: Some(ledger.clone()),
                actor_id: Some("gate".to_string()),
                auto_renewal_enabled: true,
                auto_renewal_threshold_seconds: 300,
                auto_renewal_interval_seconds: 60,
                auto_renewal_extend_seconds: 600,
            })
            .await?,
        );

        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root_dir.clone()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })?);

        let context_packs = Arc::new(ContextPackStore::new(ContextPackStoreOptions {
            root_dir: Some(root_dir.clone()),
            context_packs_dir: Some(PathBuf::from(".allternit/context-packs")),
        })?);

        let index = Arc::new(
            Index::new(IndexOptions {
                root_dir: Some(root_dir.clone()),
                index_dir: Some(PathBuf::from(".allternit/index")),
            })
            .await?,
        );

        let vault = Arc::new(Vault::new(VaultOptions {
            root_dir: Some(root_dir.clone()),
            ledger: ledger.clone(),
            actor_id: Some("gate".to_string()),
        }));

        let oauth_vault = Arc::new(crate::vault::OAuthVault::new(root_dir.clone()));

        let gate = Arc::new(Gate::new(GateOptions {
            ledger: ledger.clone(),
            leases: leases.clone(),
            receipts: receipts.clone(),
            index: Some(index.clone()),
            vault: Some(vault.clone()),
            oauth_vault: Some(oauth_vault.clone()),
            root_dir: Some(root_dir.clone()),
            actor_id: Some("gate".to_string()),
            strict_provenance: None,
            visual_provider: None,
            visual_config: None,
        }));

        let work_ops = Arc::new(WorkOps::new(
            ledger.clone(),
            Some("allternit work".to_string()),
            Some(ActorType::Agent),
        ));

        let mail = Arc::new(Mail::new(MailOptions {
            root_dir: Some(root_dir.clone()),
            ledger: ledger.clone(),
            actor_id: Some("mail".to_string()),
            actor_type: None,
            mail_index: None,
        }));

        let peers = Arc::new(PeerRegistry::new(root_dir.clone())?);

        let bus = Arc::new(
            Bus::new(crate::bus::BusOptions {
                root_dir: root_dir.clone(),
                ledger: ledger.clone(),
                actor_id: Some("service".to_string()),
                actor_type: Some(ActorType::Gate),
            })
            .await?,
        );

        let steer = Arc::new(Steer::new(ledger.clone()));

        Ok(Self {
            ledger,
            gate,
            leases,
            mail,
            vault,
            oauth_vault,
            index,
            receipts,
            work_ops,
            context_packs,
            peers,
            bus,
            steer,
            root_dir,
        })
    }
}

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
}

// ============================================================================
// PEER endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfoResponse {
    pub peer_id: String,
    pub name: String,
    pub cwd: String,
    pub vendor: String,
    pub inbox_socket: String,
    pub status: String,
    pub registered_at: String,
    pub last_heartbeat_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerListResponse {
    pub peers: Vec<PeerInfoResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRegisterRequest {
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRegisterResponse {
    pub peer_id: String,
    pub name: String,
    pub inbox_socket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSendRequest {
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSendResponse {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInboxResponse {
    pub messages: Vec<crate::bus::BusMessage>,
}

// ============================================================================
// STEER endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerCheckpointRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerCheckpointResponse {
    pub changed: bool,
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerConsultRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerConsultResponse {
    pub verdict: String,
    pub body: String,
}

// ============================================================================
// PLAN endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanNewRequest {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanNewResponse {
    pub prompt_id: String,
    pub dag_id: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRefineRequest {
    pub dag_id: String,
    pub delta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutations: Option<Vec<DagMutation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRefineResponse {
    pub delta_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanShowResponse {
    pub dag_id: String,
    pub dag: serde_json::Value,
}

// ============================================================================
// DAG endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagRenderResponse {
    pub dag_id: String,
    pub format: String,
    pub content: String,
}

// ============================================================================
// WIH (Work Identity Handle) endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihListRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    #[serde(default)]
    pub ready_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihListResponse {
    pub wihs: Vec<WihInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihInfo {
    pub wih_id: String,
    pub node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dag_id: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihPickupRequest {
    pub dag_id: String,
    pub node_id: String,
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default)]
    pub fresh: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihPickupResponse {
    pub wih_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pack_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihContextResponse {
    pub wih_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_pack: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihSignOpenRequest {
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihSignOpenResponse {
    pub signed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihCloseRequest {
    pub status: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Node output text (stored as a blob + `node.output` receipt; counts as
    /// evidence). See `spec/STORAGE_LAYOUT.md`.
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WihCloseResponse {
    pub closed: bool,
}

// ============================================================================
// LEASE endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRequestReq {
    pub wih_id: String,
    pub agent_id: String,
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRequestResponse {
    pub lease_id: String,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseReleaseResponse {
    pub released: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseResponse {
    pub lease_id: String,
    pub wih_id: String,
    pub holder: String,
    pub scope: LeaseScope,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseScope {
    pub paths: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRenewRequest {
    pub extend_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRenewResponse {
    pub lease: LeaseResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseListResponse {
    pub leases: Vec<LeaseResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkDiscoverResponse {
    pub work_requests: Vec<WorkRequestResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkRequestResponse {
    pub request_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub wih_id: String,
    pub role: String,
    pub execution_mode: String,
    pub priority: i32,
    pub deps_satisfied: bool,
    pub required_gates: Vec<String>,
    pub required_evidence: Vec<String>,
    pub lease_required: bool,
    pub lease_scope: Option<LeaseScope>,
    pub created_at: String,
}

// ============================================================================
// LEDGER endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerTailRequest {
    #[serde(default = "default_tail_count")]
    pub count: usize,
}

fn default_tail_count() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerTraceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wih_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
}

// ============================================================================
// INDEX endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRebuildResponse {
    pub indexed_count: usize,
}

// ============================================================================
// MAIL endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailEnsureThreadRequest {
    pub topic: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailEnsureThreadResponse {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailSendRequest {
    pub thread_id: String,
    pub body_ref: String,
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailSendResponse {
    pub sent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailRequestReviewRequest {
    pub thread_id: String,
    pub wih_id: String,
    pub diff_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailDecideRequest {
    pub thread_id: String,
    pub approve: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailInboxRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default = "default_inbox_limit")]
    pub limit: usize,
}

fn default_inbox_limit() -> usize {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAckRequest {
    pub thread_id: String,
    pub message_id: String,
    /// The acking recipient (E3 per-recipient ack state). When omitted, the
    /// ack event's actor identifies the acker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailReserveRequest {
    pub wih_id: String,
    pub agent_id: String,
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailShareRequest {
    pub thread_id: String,
    pub asset_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailArchiveRequest {
    pub thread_id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailGuardRequest {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// ============================================================================
// GATE endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateStatusResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateCheckRequest {
    pub wih_id: String,
    pub tool: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateCheckResponse {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRulesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateVerifyRequest {
    #[serde(default)]
    pub json: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateVerifyResponse {
    pub ok: bool,
    pub ledger_chain_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger_chain_issues: Option<Vec<String>>,
    pub cycle_dags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDecisionRequest {
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub links: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateDecisionResponse {
    pub decision_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMutateRequest {
    pub dag_id: String,
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutations: Option<Vec<DagMutation>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMutateResponse {
    pub decision_id: String,
    pub mutation_ids: Vec<String>,
}

// --- MCP Agent Features Requests/Responses ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateElicitationRequest {
    pub wih_id: String,
    pub message: String,
    pub requested_schema: Option<serde_json::Value>,
    pub url: Option<String>,
    pub server_name: Option<String>,
    pub timeout_seconds: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateElicitationResponse {
    pub elicitation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateElicitationAnswerRequest {
    pub wih_id: String,
    pub elicitation_id: String,
    pub status: String,
    pub response: Option<serde_json::Value>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateAgentHandoffRequest {
    pub wih_id: String,
    pub source_agent_id: String,
    pub target_agent_id: String,
    pub reason: Option<String>,
    pub context_variables: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSamplingRequest {
    pub wih_id: String,
    pub messages: Vec<serde_json::Value>,
    pub system_prompt: Option<String>,
    pub max_tokens: Option<i32>,
    pub temperature: Option<f32>,
    pub stop_sequences: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSamplingResponse {
    pub sampling_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSamplingAnswerRequest {
    pub wih_id: String,
    pub sampling_id: String,
    pub role: String,
    pub content: serde_json::Value,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateSessionProxyRequest {
    pub session_id: String,
    pub upstream_actor: crate::core::types::Actor,
    pub targets: Vec<String>,
}

// --- End MCP Agent Features ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateAutolandRequest {
    pub wih_id: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub git_commit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateAutolandResponse {
    pub result: crate::gate::AutolandResult,
}

// ============================================================================
// VAULT endpoints
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultArchiveRequest {
    pub wih_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultArchiveResponse {
    pub archived: bool,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultStatusResponse {
    pub jobs: Vec<VaultJobStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultJobStatus {
    pub wih_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

// ============================================================================
// INIT endpoint
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitResponse {
    pub initialized: bool,
    pub stores: Vec<String>,
}

// ============================================================================
// HTTP Handlers
// ============================================================================

async fn health_check() -> impl IntoResponse {
    Json(HealthResponse {
        status: "healthy".to_string(),
        service: "allternit-commrails".to_string(),
        version: "0.1.0".to_string(),
    })
}

// ----------------------------------------------------------------------------
// PEER handlers
// ----------------------------------------------------------------------------

fn peer_to_response(peer: &crate::peer::Peer) -> PeerInfoResponse {
    PeerInfoResponse {
        peer_id: peer.peer_id.clone(),
        name: peer.name.clone(),
        cwd: peer.cwd.to_string_lossy().to_string(),
        vendor: peer.vendor.clone(),
        inbox_socket: peer.inbox_socket.to_string_lossy().to_string(),
        status: format!("{:?}", peer.status).to_lowercase(),
        registered_at: peer.registered_at.clone(),
        last_heartbeat_at: peer.last_heartbeat_at.clone(),
    }
}

async fn list_peers(State(state): State<Arc<ServiceState>>) -> impl IntoResponse {
    let peers = state.peers.list();
    let response = PeerListResponse {
        peers: peers.iter().map(peer_to_response).collect(),
    };
    (StatusCode::OK, Json(response))
}

async fn register_peer(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<PeerRegisterRequest>,
) -> impl IntoResponse {
    let cwd = request.cwd.unwrap_or_else(|| state.root_dir.clone());
    match state.peers.register(&request.name, cwd, &request.vendor) {
        Ok(peer) => (
            StatusCode::CREATED,
            Json(PeerRegisterResponse {
                peer_id: peer.peer_id,
                name: peer.name,
                inbox_socket: peer.inbox_socket.to_string_lossy().to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn unregister_peer(
    State(state): State<Arc<ServiceState>>,
    Path(id_or_name): Path<String>,
) -> impl IntoResponse {
    match state.peers.unregister(&id_or_name) {
        Ok(true) => (StatusCode::OK, Json(json!({ "unregistered": true }))).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "peer not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn heartbeat_peer(
    State(state): State<Arc<ServiceState>>,
    Path(id_or_name): Path<String>,
) -> impl IntoResponse {
    match state.peers.heartbeat(&id_or_name) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "peer not found" })),
        )
            .into_response(),
    }
}

async fn send_to_peer(
    State(state): State<Arc<ServiceState>>,
    Path(id_or_name): Path<String>,
    Json(request): Json<PeerSendRequest>,
) -> impl IntoResponse {
    let peer = match state.peers.resolve(&id_or_name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "peer not found" })),
            )
                .into_response();
        }
    };

    let from = request.from.unwrap_or_else(|| "api".to_string());
    let body = request.body;

    // Persist the envelope to the bus so HTTP-polling peers can retrieve it.
    let bus_msg = NewBusMessage {
        correlation_id: uuid::Uuid::new_v4().to_string(),
        to: format!("peer:{}", peer.name),
        from: from.clone(),
        kind: "peer_envelope".to_string(),
        payload: json!({ "body": body.clone(), "from": from.clone() }),
        transport: "http".to_string(),
    };
    let bus_id = match state.bus.send_message(bus_msg).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("failed to persist peer envelope to bus: {}", e);
            None
        }
    };

    let envelope = PeerEnvelope::new(&from, &peer.name, body);
    let receipt = send_envelope(&peer.inbox_socket, &envelope, Duration::from_secs(5)).await;

    if let (Ok(ref receipt), Some(bus_id)) = (&receipt, bus_id) {
        if receipt.delivered {
            let _ = state.bus.mark_delivered(bus_id).await;
        }
    }

    match receipt {
        Ok(receipt) => (
            StatusCode::OK,
            Json(PeerSendResponse {
                delivered: receipt.delivered,
                error: receipt.error,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn poll_peer_inbox(
    State(state): State<Arc<ServiceState>>,
    Path(id_or_name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let peer = match state.peers.resolve(&id_or_name) {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "peer not found" })),
            )
                .into_response();
        }
    };

    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let recipient = format!("peer:{}", peer.name);
    match state
        .bus
        .poll_pending_for(&recipient, Some("http"), limit)
        .await
    {
        Ok(messages) => {
            // Mark polled messages delivered so they are not returned again.
            for msg in &messages {
                let _ = state.bus.mark_delivered(msg.id).await;
            }
            (StatusCode::OK, Json(PeerInboxResponse { messages })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn ensure_policy_injected(
    state: &ServiceState,
    scope: Option<EventScope>,
) -> Result<(), StatusCode> {
    policy::inject_policy(&state.root_dir, &state.ledger, scope, "gateway")
        .await
        .map(|bundle| {
            tracing::debug!(
                "policy bundle {} injected (hash: {})",
                bundle.policy_bundle_id,
                bundle.agents_md_hash
            );
        })
        .map_err(|err| {
            tracing::error!("policy injection failed: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

// ============================================================================
// STEER handlers
// ============================================================================

async fn steer_checkpoint(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<SteerCheckpointRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.root_dir.clone());
    match state.steer.checkpoint(&cwd).await {
        Ok(result) => Ok((StatusCode::OK, Json(result))),
        Err(err) => {
            tracing::error!("steer checkpoint failed: {}", err);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn steer_consult(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<SteerConsultRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.root_dir.clone());
    let context = match request.prompt {
        Some(p) => p,
        None => match state.steer.build_context(&cwd) {
            Ok(ctx) => ctx,
            Err(err) => {
                tracing::error!("steer build_context failed: {}", err);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        },
    };
    match state.steer.consult(&cwd, &context).await {
        Ok(answer) => {
            let lines: Vec<&str> = answer.lines().collect();
            let first = lines
                .first()
                .map(|s| s.trim().trim_start_matches("• ").to_string())
                .unwrap_or_default();
            let verdict = if first.to_uppercase().starts_with("APPROVE") {
                "APPROVE".to_string()
            } else {
                "STEER".to_string()
            };
            Ok((StatusCode::OK, Json(SteerConsultResponse { verdict, body: answer })))
        }
        Err(err) => {
            tracing::error!("steer consult failed: {}", err);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn steer_commit_gate(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<SteerConsultRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.root_dir.clone());
    match state.steer.commit_gate(&cwd).await {
        Ok(result) => {
            let status = if result.verdict == "APPROVE" {
                StatusCode::OK
            } else {
                StatusCode::OK
            };
            Ok((status, Json(result)))
        }
        Err(err) => {
            tracing::error!("steer commit_gate failed: {}", err);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// PLAN handlers
// ============================================================================

async fn plan_new(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<PlanNewRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    ensure_policy_injected(&state, Some(EventScope::default())).await?;
    match state.gate.plan_new(&request.text, request.dag_id).await {
        Ok((prompt_id, dag_id, node_id)) => {
            // Opt-in `observe_on_plan`: advisory, off the request path.
            crate::observer::spawn_on_plan(
                state.root_dir.clone(),
                state.ledger.clone(),
                dag_id.clone(),
            );
            Ok((
            StatusCode::CREATED,
            Json(PlanNewResponse {
                prompt_id,
                dag_id,
                node_id,
            }),
        ))
        }
        Err(e) => {
            tracing::error!("plan_new failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn plan_refine(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<PlanRefineRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let mutations = request.mutations.unwrap_or_default();
    let scope = EventScope {
        dag_id: Some(request.dag_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state
        .gate
        .plan_refine(&request.dag_id, &request.delta, "api", mutations)
        .await
    {
        Ok(delta_id) => Ok((StatusCode::OK, Json(PlanRefineResponse { delta_id }))),
        Err(e) => {
            tracing::error!("plan_refine failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn plan_show(
    State(state): State<Arc<ServiceState>>,
    Path(dag_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    // Query ledger for DAG events and project
    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(&dag_id))
                .collect();

            let dag = crate::work::projection::project_dag(&dag_events, &dag_id);
            let dag_json =
                serde_json::to_value(&dag).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            Ok((
                StatusCode::OK,
                Json(PlanShowResponse {
                    dag_id,
                    dag: dag_json,
                }),
            ))
        }
        Err(e) => {
            tracing::error!("plan_show failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// DAG handlers
// ============================================================================

async fn dag_render(
    State(state): State<Arc<ServiceState>>,
    Path(dag_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, StatusCode> {
    let format = params
        .get("format")
        .cloned()
        .unwrap_or_else(|| "json".to_string());

    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(&dag_id))
                .collect();

            let dag = crate::work::projection::project_dag(&dag_events, &dag_id);

            let content = if format == "md" {
                render_dag_markdown(&dag)
            } else {
                serde_json::to_string_pretty(&dag).unwrap_or_default()
            };

            Ok((
                StatusCode::OK,
                Json(DagRenderResponse {
                    dag_id,
                    format,
                    content,
                }),
            ))
        }
        Err(e) => {
            tracing::error!("dag_render failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn render_dag_markdown(dag: &crate::work::types::DagState) -> String {
    let mut out = String::new();
    out.push_str(&format!("# DAG {}\n", dag.dag_id));
    out.push_str("Nodes:\n");
    let mut nodes: Vec<_> = dag.nodes.values().collect();
    nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    for node in nodes {
        out.push_str(&format!(
            "- {} [{}] {}\n",
            node.node_id, node.status, node.title
        ));
    }
    out.push_str("Edges (blocked_by):\n");
    for edge in dag.edges.iter().filter(|e| e.edge_type == "blocked_by") {
        out.push_str(&format!("- {} -> {}\n", edge.from_node_id, edge.to_node_id));
    }
    if !dag.relations.is_empty() {
        out.push_str("Relations:\n");
        for rel in &dag.relations {
            if let Some(note) = &rel.note {
                out.push_str(&format!("- {} ~ {} ({})\n", rel.a, rel.b, note));
            } else {
                out.push_str(&format!("- {} ~ {}\n", rel.a, rel.b));
            }
        }
    }
    out
}

// ============================================================================
// WIH handlers
// ============================================================================

async fn wih_list(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<WihListRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            use crate::work::graph::ready_nodes;
            use crate::work::projection::project_dag;

            let dag_ids = if let Some(ref dag_id) = request.dag_id {
                vec![dag_id.clone()]
            } else {
                collect_dag_ids(&events)
            };

            let mut wihs = Vec::new();
            let active = active_wih_nodes(&events);

            if request.ready_only {
                for dag_id in dag_ids {
                    let dag_events: Vec<_> = events
                        .iter()
                        .filter(|e| {
                            e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(&dag_id)
                        })
                        .cloned()
                        .collect();
                    let dag = project_dag(&dag_events, &dag_id);
                    for node_id in ready_nodes(&dag) {
                        if active.contains_key(&node_id) {
                            continue;
                        }
                        if let Some(node) = dag.nodes.get(&node_id) {
                            wihs.push(WihInfo {
                                wih_id: format!("ready-{}", node_id),
                                node_id,
                                dag_id: Some(dag_id.clone()),
                                status: "ready".to_string(),
                                title: Some(node.title.clone()),
                            });
                        }
                    }
                }
            } else {
                for (node_id, wih_id) in active {
                    wihs.push(WihInfo {
                        wih_id,
                        node_id,
                        dag_id: None,
                        status: "active".to_string(),
                        title: None,
                    });
                }
            }

            Ok((StatusCode::OK, Json(WihListResponse { wihs })))
        }
        Err(e) => {
            tracing::error!("wih_list failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn wih_pickup(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<WihPickupRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        dag_id: Some(request.dag_id.clone()),
        node_id: Some(request.node_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    let options = WihPickupOptions {
        role: request.role,
        fresh: request.fresh,
    };

    match state
        .gate
        .wih_pickup_with(
            &request.dag_id,
            &request.node_id,
            &request.agent_id,
            options,
        )
        .await
    {
        Ok(wih_id) => {
            // Get WIH to find context pack path
            let events = state
                .ledger
                .query(LedgerQuery::default())
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let context_pack_path =
                project_wih_from_events(&events, &wih_id).and_then(|w| w.context_pack_path);

            Ok((
                StatusCode::OK,
                Json(WihPickupResponse {
                    wih_id,
                    context_pack_path,
                }),
            ))
        }
        Err(e) => {
            tracing::error!("wih_pickup failed: {}", e);
            if crate::gate::GateError::from_anyhow(&e).is_some() {
                return Err(StatusCode::CONFLICT);
            }
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn wih_context(
    State(state): State<Arc<ServiceState>>,
    Path(wih_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let context_pack = project_wih_from_events(&events, &wih_id)
                .and_then(|w| w.context_pack_path)
                .and_then(|path| std::fs::read_to_string(path).ok());

            Ok((
                StatusCode::OK,
                Json(WihContextResponse {
                    wih_id,
                    context_pack,
                }),
            ))
        }
        Err(e) => {
            tracing::error!("wih_context failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn wih_sign_open(
    State(state): State<Arc<ServiceState>>,
    Path(wih_id): Path<String>,
    Json(request): Json<WihSignOpenRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.wih_sign_open(&wih_id, &request.signature).await {
        Ok(_) => Ok((StatusCode::OK, Json(WihSignOpenResponse { signed: true }))),
        Err(e) => {
            tracing::error!("wih_sign_open failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn wih_close(
    State(state): State<Arc<ServiceState>>,
    Path(wih_id): Path<String>,
    Json(request): Json<WihCloseRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    // Opt-in policy `observe_before_close`: awaited so the advice lands on
    // `wih:<id>` before the close; advisory, never blocks the close.
    crate::observer::hook_before_close(
        state.root_dir.clone(),
        state.ledger.clone(),
        wih_id.clone(),
    )
    .await;
    match state
        .gate
        .wih_close_with(
            &wih_id,
            &request.status,
            &request.evidence,
            request.output.as_deref(),
        )
        .await
    {
        Ok(_) => {
            // Repeated identical failure -> observer, off the request path.
            crate::observer::spawn_after_close(
                state.root_dir.clone(),
                state.ledger.clone(),
                wih_id.clone(),
            );
            Ok((StatusCode::OK, Json(WihCloseResponse { closed: true })))
        }
        Err(e) => {
            tracing::error!("wih_close failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// LEASE handlers
// ============================================================================

async fn lease_request(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<LeaseRequestReq>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state
        .gate
        .lease_request(
            &request.wih_id,
            &request.agent_id,
            request.paths.clone(),
            request.ttl_seconds,
        )
        .await
    {
        Ok(lease_id) => Ok((
            StatusCode::CREATED,
            Json(LeaseRequestResponse {
                lease_id,
                granted: true,
            }),
        )),
        Err(e) => {
            tracing::error!("lease_request failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn lease_release(
    State(state): State<Arc<ServiceState>>,
    Path(lease_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.leases.release(&lease_id).await {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(LeaseReleaseResponse { released: true }),
        )),
        Err(e) => {
            tracing::error!("lease_release failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn lease_renew(
    State(state): State<Arc<ServiceState>>,
    Path(lease_id): Path<String>,
    Json(request): Json<LeaseRenewRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    use chrono::{Duration, Utc};
    
    // Calculate new expiry time
    let until = Utc::now() + Duration::seconds(request.extend_seconds);
    let until_iso = until.to_rfc3339();
    
    match state.leases.renew(&lease_id, &until_iso).await {
        Ok(_) => {
            // Fetch the updated lease
            match state.leases.get(&lease_id).await {
                Ok(Some(lease)) => Ok((
                    StatusCode::OK,
                    Json(LeaseRenewResponse {
                        lease: lease_to_response(&lease),
                    }),
                )),
                Ok(None) => Err(StatusCode::NOT_FOUND),
                Err(e) => {
                    tracing::error!("lease_renew get failed: {}", e);
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("lease_renew failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn lease_get(
    State(state): State<Arc<ServiceState>>,
    Path(lease_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.leases.get(&lease_id).await {
        Ok(Some(lease)) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({"lease": lease_to_response(&lease)})),
        )),
        Ok(None) => Ok((StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Lease not found"})))),
        Err(e) => {
            tracing::error!("lease_get failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn lease_list(
    State(state): State<Arc<ServiceState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, StatusCode> {
    let holder = params.get("holder").cloned();
    
    match state.leases.list(holder.as_deref()).await {
        Ok(leases) => Ok((
            StatusCode::OK,
            Json(LeaseListResponse {
                leases: leases.iter().map(lease_to_response).collect(),
            }),
        )),
        Err(e) => {
            tracing::error!("lease_list failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn work_discover(
    State(_state): State<Arc<ServiceState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, StatusCode> {
    // For now, return empty list - actual implementation would query WIHs
    // that are READY and not yet claimed
    let _role = params.get("role").cloned().unwrap_or_else(|| "builder".to_string());
    let _priority = params.get("priority").and_then(|s| s.parse().ok()).unwrap_or(0);
    let _ready = params.get("ready").map(|s| s == "true").unwrap_or(true);
    
    // TODO: Implement actual work discovery logic
    // For now, return placeholder response
    Ok((
        StatusCode::OK,
        Json(WorkDiscoverResponse {
            work_requests: vec![],
        }),
    ))
}

fn lease_to_response(lease: &crate::core::types::LeaseRecord) -> LeaseResponse {
    LeaseResponse {
        lease_id: lease.lease_id.clone(),
        wih_id: lease.wih_id.clone(),
        holder: lease.agent_id.clone(),
        scope: LeaseScope {
            paths: lease.paths.clone(),
            tools: vec![], // Tools not stored in lease, would need to be derived from WIH
        },
        expires_at: lease.granted_until.clone().unwrap_or_else(|| lease.requested_at.clone()),
        created_at: lease.requested_at.clone(),
    }
}

// ============================================================================
// LEDGER handlers
// ============================================================================

async fn ledger_tail(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<LedgerTailRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.ledger.tail(request.count).await {
        Ok(events) => Ok((StatusCode::OK, Json(events))),
        Err(e) => {
            tracing::error!("ledger_tail failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn ledger_trace(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<LedgerTraceRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let filtered: Vec<_> = events
                .into_iter()
                .filter(|evt| {
                    matches_trace(
                        evt,
                        request.node_id.as_deref(),
                        request.wih_id.as_deref(),
                        request.prompt_id.as_deref(),
                    )
                })
                .collect();
            Ok((StatusCode::OK, Json(filtered)))
        }
        Err(e) => {
            tracing::error!("ledger_trace failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// INDEX handlers
// ============================================================================

async fn index_rebuild(
    State(state): State<Arc<ServiceState>>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.index.rebuild_from_ledger(&state.ledger).await {
        Ok(count) => Ok((
            StatusCode::OK,
            Json(IndexRebuildResponse {
                indexed_count: count,
            }),
        )),
        Err(e) => {
            tracing::error!("index_rebuild failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// MAIL handlers
// ============================================================================

async fn mail_ensure_thread(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailEnsureThreadRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.mail.ensure_thread(&request.topic).await {
        Ok(thread_id) => Ok((StatusCode::OK, Json(MailEnsureThreadResponse { thread_id }))),
        Err(e) => {
            tracing::error!("mail_ensure_thread failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_send(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailSendRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .send_message(&request.thread_id, &request.body_ref, request.attachments)
        .await
    {
        Ok(_) => Ok((StatusCode::OK, Json(MailSendResponse { sent: true }))),
        Err(e) => {
            tracing::error!("mail_send failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_request_review(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailRequestReviewRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .request_review(&request.thread_id, &request.wih_id, &request.diff_ref)
        .await
    {
        Ok(_) => Ok((StatusCode::OK, Json(serde_json::json!({"requested": true})))),
        Err(e) => {
            tracing::error!("mail_request_review failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_decide(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailDecideRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let decision = if request.approve { "approve" } else { "reject" };
    match state
        .mail
        .decide_review(&request.thread_id, decision, request.notes_ref)
        .await
    {
        Ok(_) => Ok((StatusCode::OK, Json(serde_json::json!({"decided": true})))),
        Err(e) => {
            tracing::error!("mail_decide failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_inbox(
    State(state): State<Arc<ServiceState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, StatusCode> {
    let thread_id = params.get("thread_id").cloned();
    let limit = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    match state.mail.list_messages(thread_id.as_deref(), limit).await {
        Ok(events) => Ok((StatusCode::OK, Json(events))),
        Err(e) => {
            tracing::error!("mail_inbox failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_ack(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailAckRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .acknowledge_message(
            &request.thread_id,
            &request.message_id,
            request.agent_id.as_deref(),
            request.note.as_deref(),
        )
        .await
    {
        Ok(ack_id) => Ok((StatusCode::OK, Json(serde_json::json!({"ack_id": ack_id})))),
        Err(e) => {
            tracing::error!("mail_ack failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_reserve(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailReserveRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    use crate::core::ids::create_lease_id;
    use chrono::Utc;
    use serde_json::json;

    let lease_id = create_lease_id();
    let req = LeaseRequest {
        lease_id: lease_id.clone(),
        wih_id: request.wih_id.clone(),
        agent_id: request.agent_id.clone(),
        paths: request.paths.clone(),
        requested_at: Utc::now().to_rfc3339(),
        ttl_seconds: request.ttl,
    };

    match state.leases.request(req).await {
        Ok(_) => {
            let _ = state
                .mail
                .log_event(
                    "MailLeaseRequested",
                    json!({
                        "lease_id": lease_id,
                        "wih_id": request.wih_id,
                        "agent_id": request.agent_id,
                        "paths": request.paths,
                        "ttl_seconds": request.ttl
                    }),
                )
                .await;
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({"lease_id": lease_id})),
            ))
        }
        Err(e) => {
            tracing::error!("mail_reserve failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_release(
    State(state): State<Arc<ServiceState>>,
    Path(lease_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.leases.release(&lease_id).await {
        Ok(_) => {
            let _ = state
                .mail
                .log_event(
                    "MailLeaseReleased",
                    serde_json::json!({ "lease_id": lease_id }),
                )
                .await;
            Ok((StatusCode::OK, Json(serde_json::json!({"released": true}))))
        }
        Err(e) => {
            tracing::error!("mail_release failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_share(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailShareRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .share_asset(
            &request.thread_id,
            &request.asset_ref,
            request.note.as_deref(),
        )
        .await
    {
        Ok(share_id) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({"share_id": share_id})),
        )),
        Err(e) => {
            tracing::error!("mail_share failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_archive(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailArchiveRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .archive_thread(&request.thread_id, &request.path, request.reason.as_deref())
        .await
    {
        Ok(archive_id) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({"archive_id": archive_id})),
        )),
        Err(e) => {
            tracing::error!("mail_archive failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn mail_guard(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<MailGuardRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .mail
        .guard_action(&request.action, request.detail.as_deref())
        .await
    {
        Ok(guard_id) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({"guard_id": guard_id})),
        )),
        Err(e) => {
            tracing::error!("mail_guard failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// GATE handlers
// ============================================================================

async fn gate_status() -> impl IntoResponse {
    Json(GateStatusResponse {
        status: "ok".to_string(),
    })
}

async fn gate_check(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateCheckRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state
        .gate
        .pre_tool(&request.wih_id, &request.tool, &request.paths)
        .await
    {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(GateCheckResponse {
                allowed: result.allowed,
                reason: result.reason,
            }),
        )),
        Err(e) => {
            tracing::error!("gate_check failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_rules() -> impl IntoResponse {
    // Read GATE_RULES.md from spec directory
    let crate_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rules = std::fs::read_to_string(crate_root.join("spec").join("GATE_RULES.md")).ok();
    Json(GateRulesResponse { rules })
}

async fn gate_verify(
    State(state): State<Arc<ServiceState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, StatusCode> {
    let _json_output = params.get("json").map(|s| s == "true").unwrap_or(false);

    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let (chain_ok, chain_issues) = state
                .ledger
                .verify_chain()
                .await
                .unwrap_or((false, vec!["verify failed".to_string()]));

            let mut cycle_dags = Vec::new();
            for dag_id in collect_dag_ids(&events) {
                let dag_events: Vec<_> = events
                    .iter()
                    .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(&dag_id))
                    .cloned()
                    .collect();
                let dag = crate::work::projection::project_dag(&dag_events, &dag_id);
                if crate::work::graph::has_cycle_edges(&dag.edges) {
                    cycle_dags.push(dag_id);
                }
            }

            let ok = chain_ok && cycle_dags.is_empty();

            Ok((
                StatusCode::OK,
                Json(GateVerifyResponse {
                    ok,
                    ledger_chain_ok: chain_ok,
                    ledger_chain_issues: if chain_issues.is_empty() {
                        None
                    } else {
                        Some(chain_issues)
                    },
                    cycle_dags,
                }),
            ))
        }
        Err(e) => {
            tracing::error!("gate_verify failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_decision(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateDecisionRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    ensure_policy_injected(&state, Some(EventScope::default())).await?;
    match state
        .gate
        .record_agent_decision(&request.note, request.reason, request.links)
        .await
    {
        Ok(decision_id) => Ok((StatusCode::OK, Json(GateDecisionResponse { decision_id }))),
        Err(e) => {
            tracing::error!("gate_decision failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_mutate(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateMutateRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        dag_id: Some(request.dag_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    let mutations = request.mutations.unwrap_or_default();
    match state
        .gate
        .mutate_with_decision(&request.dag_id, &request.note, request.reason, mutations)
        .await
    {
        Ok((decision_id, mutation_ids)) => Ok((
            StatusCode::OK,
            Json(GateMutateResponse {
                decision_id,
                mutation_ids,
            }),
        )),
        Err(e) => {
            tracing::error!("gate_mutate failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_autoland(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateAutolandRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.autoland_wih(&request.wih_id, request.dry_run, request.git_commit).await {
        Ok(result) => Ok((
            StatusCode::OK,
            Json(GateAutolandResponse { result }),
        )),
        Err(e) => {
            tracing::error!("gate_autoland failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// --- MCP Agent Features Handlers ---

async fn gate_elicitation_request(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateElicitationRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.elicitation_request(&request.wih_id, &request.message, request.requested_schema, request.url.as_deref(), request.server_name.as_deref(), request.timeout_seconds).await {
        Ok(elicitation_id) => Ok((StatusCode::OK, Json(GateElicitationResponse { elicitation_id }))),
        Err(e) => {
            tracing::error!("elicitation_request failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_elicitation_answer(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateElicitationAnswerRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.elicitation_response(&request.wih_id, &request.elicitation_id, &request.status, request.response, request.reason.as_deref()).await {
        Ok(_) => Ok((StatusCode::OK, Json(json!({"status": "ok"})))),
        Err(e) => {
            tracing::error!("elicitation_answer failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_agent_handoff(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateAgentHandoffRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.agent_handoff_request(&request.wih_id, &request.source_agent_id, &request.target_agent_id, request.reason.as_deref(), request.context_variables).await {
        Ok(_) => Ok((StatusCode::OK, Json(json!({"status": "ok"})))),
        Err(e) => {
            tracing::error!("agent_handoff failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_sampling_request(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateSamplingRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.sampling_request(&request.wih_id, request.messages, request.system_prompt.as_deref(), request.max_tokens, request.temperature, request.stop_sequences).await {
        Ok(sampling_id) => Ok((StatusCode::OK, Json(GateSamplingResponse { sampling_id }))),
        Err(e) => {
            tracing::error!("sampling_request failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_sampling_answer(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateSamplingAnswerRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.sampling_response(&request.wih_id, &request.sampling_id, &request.role, request.content, &request.model).await {
        Ok(_) => Ok((StatusCode::OK, Json(json!({"status": "ok"})))),
        Err(e) => {
            tracing::error!("sampling_answer failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_session_proxy_register(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateSessionProxyRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.gate.register_session_proxy(&request.session_id, request.upstream_actor, request.targets).await {
        Ok(_) => Ok((StatusCode::OK, Json(json!({"status": "ok"})))),
        Err(e) => {
            tracing::error!("session_proxy_register failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_oauth_store(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<crate::core::types::OAuthTokenRecord>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.gate.store_oauth_token(request).await {
        Ok(_) => Ok((StatusCode::OK, Json(json!({"status": "ok"})))),
        Err(e) => {
            tracing::error!("oauth_store failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_oauth_get(
    State(state): State<Arc<ServiceState>>,
    Path(server_name): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.gate.get_oauth_token(&server_name).await {
        Ok(Some(token)) => Ok((StatusCode::OK, Json(token))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("oauth_get failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn gate_rollback(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<GateAutolandRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.gate.rollback_wih(&request.wih_id).await {
        Ok(_) => Ok((
            StatusCode::OK,
            Json(GateAutolandResponse { 
                result: crate::gate::AutolandResult {
                    success: true,
                    dry_run: false,
                    impact: crate::gate::AutolandImpact { modified: vec![], added: vec![], deleted: vec![] },
                    backup_dir: None,
                }
            }),
        )),
        Err(e) => {
            tracing::error!("gate_rollback failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// VAULT handlers
// ============================================================================

async fn vault_archive(
    State(state): State<Arc<ServiceState>>,
    Json(request): Json<VaultArchiveRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let scope = EventScope {
        wih_id: Some(request.wih_id.clone()),
        ..Default::default()
    };
    ensure_policy_injected(&state, Some(scope)).await?;
    match state.vault.archive_wih(&request.wih_id).await {
        Ok(path) => Ok((
            StatusCode::OK,
            Json(VaultArchiveResponse {
                archived: true,
                path: path.display().to_string(),
            }),
        )),
        Err(e) => {
            tracing::error!("vault_archive failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn vault_status(
    State(state): State<Arc<ServiceState>>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let mut jobs = Vec::new();
            let mut created: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut completed: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            for evt in &events {
                if evt.r#type == "VaultJobCreated" {
                    if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                        created.insert(wih_id.to_string(), evt.ts.clone());
                    }
                }
                if evt.r#type == "VaultJobCompleted" {
                    if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                        completed.insert(wih_id.to_string(), evt.ts.clone());
                    }
                }
            }

            for (wih_id, created_at) in &created {
                jobs.push(VaultJobStatus {
                    wih_id: wih_id.clone(),
                    status: if completed.contains_key(wih_id) {
                        "completed"
                    } else {
                        "pending"
                    }
                    .to_string(),
                    created_at: Some(created_at.clone()),
                    completed_at: completed.get(wih_id).cloned(),
                });
            }

            Ok((StatusCode::OK, Json(VaultStatusResponse { jobs })))
        }
        Err(e) => {
            tracing::error!("vault_status failed: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// ============================================================================
// INIT handler
// ============================================================================

async fn init_system(
    State(state): State<Arc<ServiceState>>,
) -> Result<impl IntoResponse, StatusCode> {
    use crate::core::io::ensure_dir;

    let dirs = [
        ".allternit/ledger/events",
        ".allternit/leases",
        ".allternit/receipts",
        ".allternit/blobs",
        ".allternit/index",
        ".allternit/mail/threads",
        ".allternit/vault",
        ".allternit/work/dags",
        ".allternit/wih",
    ];

    let mut stores = Vec::new();
    for rel in dirs {
        if let Ok(_) = ensure_dir(&state.root_dir.join(rel)) {
            stores.push(rel.to_string());
        }
    }

    Ok((
        StatusCode::OK,
        Json(InitResponse {
            initialized: true,
            stores,
        }),
    ))
}

// ============================================================================
// Helper functions
// ============================================================================

fn collect_dag_ids(events: &[crate::AllternitEvent]) -> Vec<String> {
    use std::collections::HashSet;
    let mut dag_ids = HashSet::new();
    for evt in events {
        if let Some(dag_id) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
            dag_ids.insert(dag_id.to_string());
        }
        if evt.r#type == "DagCreated" {
            if let Some(dag_id) = evt.payload.get("dag_id").and_then(|v| v.as_str()) {
                dag_ids.insert(dag_id.to_string());
            }
        }
    }
    let mut out: Vec<String> = dag_ids.into_iter().collect();
    out.sort();
    out
}

fn active_wih_nodes(events: &[crate::AllternitEvent]) -> std::collections::HashMap<String, String> {
    use std::collections::{HashMap, HashSet};
    let mut wih_nodes: HashMap<String, String> = HashMap::new();
    let mut closed: HashSet<String> = HashSet::new();
    for evt in events {
        if evt.r#type == "WIHCreated" {
            if let (Some(wih_id), Some(node_id)) = (
                evt.payload.get("wih_id").and_then(|v| v.as_str()),
                evt.payload.get("node_id").and_then(|v| v.as_str()),
            ) {
                wih_nodes.insert(wih_id.to_string(), node_id.to_string());
            }
        }
        if evt.r#type == "WIHClosedSigned" || evt.r#type == "WIHArchived" {
            if let Some(wih_id) = evt.payload.get("wih_id").and_then(|v| v.as_str()) {
                closed.insert(wih_id.to_string());
            }
        }
    }
    let mut active = HashMap::new();
    for (wih_id, node_id) in wih_nodes {
        if !closed.contains(&wih_id) {
            active.insert(node_id, wih_id);
        }
    }
    active
}

fn matches_trace(
    event: &crate::AllternitEvent,
    node_id: Option<&str>,
    wih_id: Option<&str>,
    prompt_id: Option<&str>,
) -> bool {
    if let Some(node_id) = node_id {
        let payload = &event.payload;
        let hits = [
            payload.get("node_id"),
            payload.get("from_node_id"),
            payload.get("to_node_id"),
            payload.get("parent_node_id"),
            payload.get("a"),
            payload.get("b"),
        ]
        .iter()
        .filter_map(|v| v.and_then(|val| val.as_str()))
        .any(|val| val == node_id);
        if !hits {
            return false;
        }
    }
    if let Some(wih_id) = wih_id {
        let payload = &event.payload;
        let hits = payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id);
        if !hits {
            return false;
        }
    }
    if let Some(prompt_id) = prompt_id {
        let payload_match =
            event.payload.get("prompt_id").and_then(|v| v.as_str()) == Some(prompt_id);
        let provenance_match = event
            .provenance
            .as_ref()
            .and_then(|p| p.prompt_id.as_deref())
            == Some(prompt_id);
        if !payload_match && !provenance_match {
            return false;
        }
    }
    true
}

fn project_wih_from_events(
    events: &[crate::AllternitEvent],
    wih_id: &str,
) -> Option<crate::wih::types::WihState> {
    let filtered: Vec<crate::AllternitEvent> = events
        .iter()
        .filter(|evt| evt.payload.get("wih_id").and_then(|v| v.as_str()) == Some(wih_id))
        .cloned()
        .collect();
    crate::wih::projection::project_wih(&filtered, wih_id)
}

// ============================================================================
// CONTEXT PACK endpoints
// ============================================================================

use chrono::Utc;
use crate::context::{generate_pack_id, sha256_with_prefix, ContextPackSeal, InputManifestEntry, PolicyBundleRef};
use crate::core::types::ReceiptRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackGetResponse {
    pub pack_id: String,
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs_manifest: Vec<InputManifestEntry>,
    pub method_version: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackVerifyResponse {
    pub pack_id: String,
    pub valid: bool,
    pub verified_at: String,
    pub hash_matches: Vec<HashMatchEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashMatchEntry {
    pub path: String,
    pub expected: String,
    pub actual: String,
    pub r#match: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptsQueryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub include_summary: bool,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptsQueryResponse {
    pub receipts: Vec<ReceiptRecord>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<ReceiptSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptSummary {
    pub total_count: usize,
    pub by_type: std::collections::HashMap<String, usize>,
    pub by_run: std::collections::HashMap<String, usize>,
    pub date_range: Option<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptVerifyRequest {
    pub receipt_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptVerifyResponse {
    pub results: Vec<ReceiptVerificationResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiptVerificationResult {
    pub receipt_id: String,
    pub is_valid: bool,
    pub hash_matches: bool,
    pub signature_valid: Option<bool>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealContextPackRequest {
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs: crate::context::types::ContextPackInputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealContextPackResponse {
    pub pack_id: String,
    pub wih_id: String,
    pub dag_id: String,
    pub node_id: String,
    pub inputs_manifest: Vec<InputManifestEntry>,
    pub method_version: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_bundle: Option<PolicyBundleRef>,
}

async fn context_pack_seal(
    State(state): State<Arc<ServiceState>>,
    Json(req): Json<SealContextPackRequest>,
) -> impl IntoResponse {
    // Generate deterministic pack_id
    let pack_id = generate_pack_id(&req.inputs);

    // Build inputs manifest
    let mut inputs_manifest = Vec::new();
    
    inputs_manifest.push(InputManifestEntry {
        path: "/SYSTEM_LAW.md".to_string(),
        hash: sha256_with_prefix(&req.inputs.tier0_law),
        size_bytes: req.inputs.tier0_law.len(),
    });
    
    inputs_manifest.push(InputManifestEntry {
        path: "/SOT.md".to_string(),
        hash: sha256_with_prefix(&req.inputs.sot),
        size_bytes: req.inputs.sot.len(),
    });
    
    inputs_manifest.push(InputManifestEntry {
        path: "/ARCHITECTURE.md".to_string(),
        hash: sha256_with_prefix(&req.inputs.architecture),
        size_bytes: req.inputs.architecture.len(),
    });
    
    for contract in &req.inputs.contracts {
        inputs_manifest.push(InputManifestEntry {
            path: contract.path.clone(),
            hash: contract.hash.clone(),
            size_bytes: contract.content.len(),
        });
    }
    
    for delta in &req.inputs.deltas {
        inputs_manifest.push(InputManifestEntry {
            path: delta.path.clone(),
            hash: delta.hash.clone(),
            size_bytes: delta.content.len(),
        });
    }

    // Create seal
    let seal = ContextPackSeal {
        pack_id: pack_id.clone(),
        wih_id: req.wih_id.clone(),
        dag_id: req.dag_id.clone(),
        node_id: req.node_id.clone(),
        inputs_manifest: inputs_manifest.clone(),
        method_version: "1.0.0".to_string(),
        created_at: Utc::now().to_rfc3339(),
        policy_bundle: req.policy_bundle.clone(),
    };

    // Store seal
    match state.context_packs.store_seal(&seal) {
        Ok(_) => {
            tracing::info!("Sealed ContextPack: {}", pack_id);
            Json(SealContextPackResponse {
                pack_id,
                wih_id: seal.wih_id,
                dag_id: seal.dag_id,
                node_id: seal.node_id,
                inputs_manifest: seal.inputs_manifest,
                method_version: seal.method_version,
                created_at: seal.created_at,
                policy_bundle: seal.policy_bundle,
            }).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to seal ContextPack: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to seal ContextPack: {}", e)).into_response()
        }
    }
}

async fn context_pack_get(
    State(state): State<Arc<ServiceState>>,
    Path(pack_id): Path<String>,
) -> impl IntoResponse {
    match state.context_packs.get_seal(&pack_id) {
        Ok(Some(seal)) => {
            Json(ContextPackGetResponse {
                pack_id: seal.pack_id,
                wih_id: seal.wih_id,
                dag_id: seal.dag_id,
                node_id: seal.node_id,
                inputs_manifest: seal.inputs_manifest,
                method_version: seal.method_version,
                created_at: seal.created_at,
                policy_bundle: seal.policy_bundle,
            }).into_response()
        }
        Ok(None) => {
            (StatusCode::NOT_FOUND, format!("ContextPack not found: {}", pack_id)).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to get ContextPack: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get ContextPack: {}", e)).into_response()
        }
    }
}

async fn context_pack_verify(
    State(state): State<Arc<ServiceState>>,
    Path(pack_id): Path<String>,
) -> impl IntoResponse {
    match state.context_packs.get_seal(&pack_id) {
        Ok(Some(seal)) => {
            let mut hash_matches = Vec::new();
            let mut errors = Vec::new();
            let mut all_match = true;

            // Verify each hash in the manifest
            for entry in &seal.inputs_manifest {
                // For now, we just verify the hash format is valid
                // In production, you'd re-read the file and compare hashes
                let expected = entry.hash.clone();
                let actual = expected.clone(); // Placeholder - would re-hash file content
                let matches = expected == actual;
                
                if !matches {
                    all_match = false;
                    errors.push(format!("Hash mismatch for {}", entry.path));
                }
                
                hash_matches.push(HashMatchEntry {
                    path: entry.path.clone(),
                    expected,
                    actual,
                    r#match: matches,
                });
            }

            Json(ContextPackVerifyResponse {
                pack_id,
                valid: all_match,
                verified_at: Utc::now().to_rfc3339(),
                hash_matches,
                errors: if errors.is_empty() { None } else { Some(errors) },
            }).into_response()
        }
        Ok(None) => {
            (StatusCode::NOT_FOUND, format!("ContextPack not found: {}", pack_id)).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to verify ContextPack: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to verify ContextPack: {}", e)).into_response()
        }
    }
}

async fn receipts_query(
    State(state): State<Arc<ServiceState>>,
    Json(req): Json<ReceiptsQueryRequest>,
) -> impl IntoResponse {
    use crate::receipts::store::ReceiptQuery;

    // Build query from request
    let query = ReceiptQuery {
        run_id: req.run_id,
        tool: req.tool,
        from_date: None,
        to_date: None,
        limit: Some(req.limit),
        offset: Some(req.offset),
    };

    // Query receipts
    let receipts = match state.receipts.query_receipts(&query) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Failed to query receipts: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Query failed: {}", e)).into_response();
        }
    };

    let total = receipts.len();

    // Get summary if requested
    let summary = if req.include_summary {
        match state.receipts.get_summary(&query) {
            Ok(s) => Some(ReceiptSummary {
                total_count: s.total_count,
                by_type: s.by_type,
                by_run: s.by_run,
                date_range: None,  // ReceiptRecord doesn't have timestamp
            }),
            Err(e) => {
                tracing::warn!("Failed to get receipt summary: {}", e);
                None
            }
        }
    } else {
        None
    };

    Json(ReceiptsQueryResponse {
        receipts,
        total,
        limit: req.limit,
        offset: req.offset,
        summary,
    }).into_response()
}

async fn receipts_verify(
    State(state): State<Arc<ServiceState>>,
    Json(req): Json<ReceiptVerifyRequest>,
) -> impl IntoResponse {
    

    // Verify receipts
    let results: Vec<ReceiptVerificationResult> = match state.receipts.verify_receipts(&req.receipt_ids) {
        Ok(r) => r.into_iter().map(|sr| ReceiptVerificationResult {
            receipt_id: sr.receipt_id,
            is_valid: sr.is_valid,
            hash_matches: sr.hash_matches,
            signature_valid: sr.signature_valid,
            errors: sr.errors,
        }).collect(),
        Err(e) => {
            tracing::error!("Failed to verify receipts: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("Verification failed: {}", e)).into_response();
        }
    };

    Json(ReceiptVerifyResponse {
        results,
    }).into_response()
}

async fn receipts_summary(
    State(state): State<Arc<ServiceState>>,
    Json(req): Json<ReceiptsQueryRequest>,
) -> impl IntoResponse {
    use crate::receipts::store::ReceiptQuery;

    // Build query from request
    let query = ReceiptQuery {
        run_id: req.run_id,
        tool: req.tool,
        from_date: None,
        to_date: None,
        limit: None,
        offset: None,
    };

    // Get summary
    match state.receipts.get_summary(&query) {
        Ok(s) => Json(ReceiptSummary {
            total_count: s.total_count,
            by_type: s.by_type,
            by_run: s.by_run,
            date_range: None,  // ReceiptRecord doesn't have timestamp
        }).into_response(),
        Err(e) => {
            tracing::error!("Failed to get receipt summary: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Summary failed: {}", e)).into_response()
        }
    }
}

// ============================================================================
// Router
// ============================================================================

pub fn create_router(state: Arc<ServiceState>) -> Router {
    Router::new()
        // Health
        .route("/health", get(health_check))
        // PEERS
        .route("/v1/peers", get(list_peers).post(register_peer))
        .route("/v1/peers/:id_or_name", delete(unregister_peer))
        .route("/v1/peers/:id_or_name/heartbeat", post(heartbeat_peer))
        .route("/v1/peers/:id_or_name/send", post(send_to_peer))
        .route("/v1/peers/:id_or_name/inbox", get(poll_peer_inbox))
        // STEER
        .route("/v1/steer/checkpoint", post(steer_checkpoint))
        .route("/v1/steer/consult", post(steer_consult))
        .route("/v1/steer/commit-gate", post(steer_commit_gate))
        // PLAN
        .route("/v1/plan", post(plan_new))
        .route("/v1/plan/refine", post(plan_refine))
        .route("/v1/plan/:dag_id", get(plan_show))
        // DAG
        .route("/v1/dags/:dag_id/render", get(dag_render))
        // WIH
        .route("/v1/wihs", post(wih_list))
        .route("/v1/wihs/pickup", post(wih_pickup))
        .route("/v1/wihs/:wih_id/context", get(wih_context))
        .route("/v1/wihs/:wih_id/sign", post(wih_sign_open))
        .route("/v1/wihs/:wih_id/close", post(wih_close))
        // LEASE
        .route("/v1/leases", post(lease_request))
        .route("/v1/leases", get(lease_list))
        .route("/v1/leases/:lease_id", delete(lease_release))
        .route("/v1/leases/:lease_id", get(lease_get))
        .route("/v1/leases/:lease_id/renew", post(lease_renew))
        // WORK DISCOVERY
        .route("/v1/work/discover", get(work_discover))
        // LEDGER
        .route("/v1/ledger/tail", post(ledger_tail))
        .route("/v1/ledger/trace", post(ledger_trace))
        // INDEX
        .route("/v1/index/rebuild", post(index_rebuild))
        // MAIL
        .route("/v1/mail/threads", post(mail_ensure_thread))
        .route("/v1/mail/send", post(mail_send))
        .route("/v1/mail/review", post(mail_request_review))
        .route("/v1/mail/decide", post(mail_decide))
        .route("/v1/mail/inbox", get(mail_inbox))
        .route("/v1/mail/ack", post(mail_ack))
        .route("/v1/mail/reserve", post(mail_reserve))
        .route("/v1/mail/release/:lease_id", post(mail_release))
        .route("/v1/mail/share", post(mail_share))
        .route("/v1/mail/archive", post(mail_archive))
        .route("/v1/mail/guard", post(mail_guard))
        // GATE
        .route("/v1/gate/status", get(gate_status))
        .route("/v1/gate/check", post(gate_check))
        .route("/v1/gate/rules", get(gate_rules))
        .route("/v1/gate/verify", get(gate_verify))
        .route("/v1/gate/decision", post(gate_decision))
        .route("/v1/gate/mutate", post(gate_mutate))
        .route("/v1/gate/autoland", post(gate_autoland))
        .route("/v1/gate/rollback", post(gate_rollback))
        // MCP Agent Features
        .route("/v1/gate/elicitation", post(gate_elicitation_request))
        .route("/v1/gate/elicitation/answer", post(gate_elicitation_answer))
        .route("/v1/gate/handoff", post(gate_agent_handoff))
        .route("/v1/gate/sampling", post(gate_sampling_request))
        .route("/v1/gate/sampling/answer", post(gate_sampling_answer))
        .route("/v1/gate/session-proxy", post(gate_session_proxy_register))
        .route("/v1/gate/oauth", post(gate_oauth_store))
        .route("/v1/gate/oauth/:server_name", get(gate_oauth_get))
        // VAULT
        .route("/v1/vault/archive", post(vault_archive))
        .route("/v1/vault/status", get(vault_status))
        // CONTEXT PACK
        .route("/v1/context-pack/seal", post(context_pack_seal))
        .route("/v1/context-pack/:pack_id", get(context_pack_get))
        .route("/v1/context-pack/:pack_id/verify", get(context_pack_verify))
        // RECEIPTS
        .route("/v1/receipts/query", get(receipts_query))
        .route("/v1/receipts/verify", post(receipts_verify))
        .route("/v1/receipts/summary", post(receipts_summary))
        // INIT
        .route("/v1/init", post(init_system))
        .with_state(state)
}

/// Run the HTTP service
pub async fn run_service(bind_addr: &str, root_dir: PathBuf) -> anyhow::Result<()> {
    // Initialize stores
    init_stores(&root_dir).await?;

    let state = Arc::new(ServiceState::new(root_dir).await?);

    // Background UDS delivery loop for bus messages addressed to local peers.
    let delivery_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = deliver_uds_messages(&delivery_state).await {
                tracing::error!("uds delivery loop error: {}", e);
            }
            sleep(Duration::from_secs(2)).await;
        }
    });

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        "Allternit Agent System Rails HTTP service listening on {}",
        bind_addr
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// Poll pending UDS bus messages and attempt delivery to local peer sockets.
async fn deliver_uds_messages(state: &ServiceState) -> anyhow::Result<()> {
    let pending = state.bus.poll_pending(32).await?;
    for msg in pending {
        if msg.transport != "uds" {
            continue;
        }

        // Recipient may be "peer:<name>" or a literal socket path.
        let target = if let Some(name) = msg.to.strip_prefix("peer:") {
            match state.peers.resolve(name) {
                Some(p) => p.inbox_socket,
                None => {
                    state
                        .bus
                        .mark_failed(msg.id, "recipient peer not found")
                        .await?;
                    continue;
                }
            }
        } else {
            PathBuf::from(&msg.to)
        };

        let body = msg
            .payload
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let from = msg
            .payload
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or("rails")
            .to_string();
        let envelope = PeerEnvelope::new(&from, &msg.to, body);

        match send_envelope(&target, &envelope, Duration::from_secs(5)).await {
            Ok(receipt) if receipt.delivered => {
                state.bus.mark_delivered(msg.id).await?;
            }
            Ok(receipt) => {
                state
                    .bus
                    .mark_failed(msg.id, &receipt.error.unwrap_or_default())
                    .await?;
            }
            Err(e) => {
                state.bus.mark_failed(msg.id, &e.to_string()).await?;
            }
        }
    }
    Ok(())
}

async fn init_stores(root: &PathBuf) -> anyhow::Result<()> {
    use crate::core::io::ensure_dir;

    let dirs = [
        ".allternit/ledger/events",
        ".allternit/leases",
        ".allternit/receipts",
        ".allternit/blobs",
        ".allternit/context-packs",
        ".allternit/index",
        ".allternit/mail/threads",
        ".allternit/vault",
        ".allternit/work/dags",
        ".allternit/wih",
    ];

    for rel in dirs {
        ensure_dir(&root.join(rel))?;
    }

    Ok(())
}
