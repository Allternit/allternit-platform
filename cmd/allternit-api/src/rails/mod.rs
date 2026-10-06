//! Rails System Integration
//!
//! Provides HTTP API for the Allternit Agent System Rails:
//! - Ledger event querying
//! - DAG/Work management
//! - Lease operations
//! - Policy gate queries

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
pub mod routes_cowork;
pub mod fabric_transport_routes;
mod visibility;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

use crate::AppState;
use allternit_factory_engine::bus::{Bus, NewBusMessage};
use allternit_factory_engine::dependencies::{
    self, DependencyEdge, DependencyKind,
};
use allternit_factory_engine::graph::{views, GraphAnalytics, GraphView, InsightsConfig};
use allternit_factory_engine::rails_id::{HierarchicalId, TicketId};
use allternit_factory_engine::receipts::ReceiptQuery;
use allternit_factory_engine::tickets::{
    self, BlockedTicket, Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore,
    TicketUpdate,
};
use allternit_factory_engine::wait_gates::WaitGateStore;
use allternit_factory_engine::wih::{WihState, active_wihs, project_wih};
use allternit_factory_engine::work::{DagNode, ready_nodes};
use allternit_factory_engine::{
    Actor, ActorType, AllternitEvent, ContextPackSeal, ContextPackStore, ContextPackStoreOptions,
    DagMutation, EventScope, Gate, GateOptions, Index, IndexOptions, LeaseRecord, Leases,
    LeasesOptions, Ledger, LedgerOptions, LedgerQuery, Mail, MailImportance, MailIndex,
    MailIndexOptions, MailOptions, PeerEnvelope, PeerRegistry, ReceiptRecord, ReceiptStore,
    ReceiptStoreOptions, Steer, TypedMessage, Vault, VaultOptions, WihPickupOptions, WorkOps,
    project_dag, resolve_thread_id, send_envelope,
};

// ============================================================================
// State
// ============================================================================

/// Rails service state shared across handlers
#[derive(Clone)]
pub struct RailsState {
    pub root_dir: std::path::PathBuf,
    pub ledger: Arc<Ledger>,
    pub gate: Arc<Gate>,
    pub leases: Arc<Leases>,
    pub mail: Arc<Mail>,
    pub vault: Arc<Vault>,
    pub index: Arc<Index>,
    pub receipts: Arc<ReceiptStore>,
    pub work_ops: Arc<WorkOps>,
    pub context_packs: Arc<ContextPackStore>,
    pub peers: Arc<PeerRegistry>,
    pub bus: Arc<Bus>,
    pub steer: Arc<Steer>,
    /// Shared graph analytics engine (content-hash cache) for the B2 robot
    /// surface under `/graph/*`.
    pub graph_analytics: Arc<GraphAnalytics>,
}

impl RailsState {
    pub async fn new(root_dir: std::path::PathBuf) -> anyhow::Result<Self> {
        info!("Initializing Factory core state...");

        // Initialize Ledger
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root_dir.clone()),
            ledger_dir: Some(std::path::PathBuf::from(".allternit/ledger")),
        }));

        // Initialize Leases
        let leases = Arc::new(
            Leases::new(LeasesOptions {
                root_dir: Some(root_dir.clone()),
                leases_dir: Some(std::path::PathBuf::from(".allternit/leases")),
                event_sink: Some(ledger.clone()),
                actor_id: Some("api".to_string()),
                auto_renewal_enabled: true,
                auto_renewal_threshold_seconds: 300,
                auto_renewal_interval_seconds: 60,
                auto_renewal_extend_seconds: 600,
            })
            .await?,
        );

        // Initialize Receipts
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root_dir.clone()),
            receipts_dir: Some(std::path::PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(std::path::PathBuf::from(".allternit/blobs")),
        })?);

        // Initialize Context Packs
        let context_packs = Arc::new(ContextPackStore::new(ContextPackStoreOptions {
            root_dir: Some(root_dir.clone()),
            context_packs_dir: Some(std::path::PathBuf::from(".allternit/context-packs")),
        })?);

        // Initialize Index
        let index = Arc::new(
            Index::new(IndexOptions {
                root_dir: Some(root_dir.clone()),
                index_dir: Some(std::path::PathBuf::from(".allternit/index")),
            })
            .await?,
        );

        // Initialize Vault
        let vault = Arc::new(Vault::new(VaultOptions {
            root_dir: Some(root_dir.clone()),
            ledger: ledger.clone(),
            actor_id: Some("api".to_string()),
        }));

        // Initialize Gate
        let gate = Arc::new(Gate::new(GateOptions {
            ledger: ledger.clone(),
            leases: leases.clone(),
            receipts: receipts.clone(),
            index: Some(index.clone()),
            vault: Some(vault.clone()),
            oauth_vault: None,
            root_dir: Some(root_dir.clone()),
            actor_id: Some("api".to_string()),
            strict_provenance: Some(true),
            visual_provider: None,
            visual_config: None,
        }));

        // Initialize Mail (with the E2 FTS search index)
        let mail_index = Arc::new(
            MailIndex::new(MailIndexOptions {
                root_dir: Some(root_dir.clone()),
            })
            .await?,
        );
        let mail = Arc::new(Mail::new(MailOptions {
            root_dir: Some(root_dir.clone()),
            ledger: ledger.clone(),
            actor_id: Some("api".to_string()),
            actor_type: None,
            mail_index: Some(mail_index),
        }));

        // Initialize WorkOps
        let work_ops = Arc::new(WorkOps::new(ledger.clone(), Some("api".to_string()), None));

        // Initialize peer registry + bus for cross-session messaging.
        let peers = Arc::new(PeerRegistry::new(root_dir.clone())?);
        let bus = Arc::new(
            Bus::new(allternit_factory_engine::bus::BusOptions {
                root_dir: root_dir.clone(),
                ledger: ledger.clone(),
                actor_id: Some("api".to_string()),
                actor_type: Some(ActorType::Gate),
            })
            .await?,
        );

        // Initialize steering coordinator.
        let steer = Arc::new(Steer::new(ledger.clone()));

        info!("Factory core state initialized");

        Ok(Self {
            root_dir,
            ledger,
            gate,
            leases,
            mail,
            vault,
            index,
            receipts,
            work_ops,
            context_packs,
            peers,
            bus,
            steer,
            graph_analytics: Arc::new(GraphAnalytics::new()),
        })
    }
}

// ============================================================================
// Routes
// ============================================================================

/// The Factory routes allternit-api serves in-process (ledger, Gate, mail,
/// peers, steer, WIHs, tickets, plans …), mounted under `/api/factory` next to
/// the engine proxy (`factory_proxy`). Paths are static, so they win over the
/// proxy's `/api/factory/*rest`; none of them repeats an engine route
/// (`health`, `dags/:id`, `runs` belong to the engine), which is why health,
/// the dag views and run cancel live under `ledger/` and `plan/` here.
pub fn factory_core_router() -> Router<Arc<AppState>> {
    Router::new()
        // Health of the in-process ledger/Gate/leases (the engine's own is /api/factory/health)
        .route("/ledger/health", get(health_check))
        // Peers (cross-session messaging)
        .route("/peers", get(list_peers).post(register_peer))
        .route("/peers/:id_or_name", delete(unregister_peer))
        .route("/peers/:id_or_name/heartbeat", post(heartbeat_peer))
        .route("/peers/:id_or_name/send", post(send_to_peer))
        .route("/peers/:id_or_name/inbox", get(poll_peer_inbox))
        // Bot Agents BA-1: read-only pane/peer visibility DTO (empty panes if none)
        .route("/visibility", get(visibility))
        // Steer
        .route("/steer/checkpoint", post(steer_checkpoint))
        .route("/steer/consult", post(steer_consult))
        .route("/steer/commit-gate", post(steer_commit_gate))
        // Ledger
        .route("/ledger/events", get(query_ledger))
        .route("/ledger/tail", post(tail_ledger))
        // Receipts
        .route("/receipts", post(query_receipts))
        .route("/receipts/write", post(write_receipt))
        // Mail (orchestrator bridge / CLI agent access)
        .route("/mail/send", post(mail_send))
        .route("/mail/threads", get(list_mail_threads).post(ensure_mail_thread_handler))
        .route("/mail/thread/:thread_id", get(read_mail_thread))
        .route("/mail/ack", post(ack_mail_message))
        .route("/mail/share", post(mail_share))
        .route("/mail/decide", post(mail_decide))
        // Mail agent identities + per-agent boxes (E1)
        .route("/mail/agents", get(list_mail_agents))
        .route("/mail/agents/register", post(register_mail_agent))
        .route("/mail/inbox/:agent_id", get(read_mail_inbox))
        .route("/mail/outbox/:agent_id", get(read_mail_outbox))
        // Mail FTS search (E2)
        .route("/mail/search", get(search_mail_messages))
        // Mail ack overdue view (E3)
        .route("/mail/overdue", get(read_mail_overdue))
        // WIHs (compatibility surface used by platform cowork/chat views)
        .route("/wihs", get(list_wihs_get).post(list_wihs))
        .route("/wihs/pickup", post(pickup_wih))
        .route("/wihs/:wih_id/context", get(get_wih_context))
        .route("/wihs/:wih_id/sign", post(sign_wih))
        .route("/wihs/:wih_id/close", post(close_wih))
        // Tickets (event-sourced ticket subsystem)
        .route("/tickets", post(create_ticket).get(list_tickets))
        .route("/tickets/ready", get(list_ready_tickets))
        .route("/tickets/blocked", get(list_blocked_tickets))
        .route("/tickets/:id", get(get_ticket).patch(update_ticket))
        .route("/tickets/:id/close", post(close_ticket))
        .route("/tickets/:id/reopen", post(reopen_ticket))
        .route(
            "/tickets/:id/dependencies",
            post(add_ticket_dependency).delete(remove_ticket_dependency),
        )
        // Graph analytics (B2 robot surface over the ticket dependency graph)
        .route("/graph/insights", get(graph_insights))
        .route("/graph/triage", get(graph_triage))
        .route("/graph/impact/:ticket_id", get(graph_impact))
        // DAGs / Work
        .route("/workspace/:workspace_id/dags", get(list_dags))
        .route("/workspace/:workspace_id/dags", post(create_dag))
        .route("/workspace/:workspace_id/dags/:dag_id", get(get_dag))
        .route(
            "/workspace/:workspace_id/dags/:dag_id/start",
            post(start_dag),
        )
        // Plan (unscoped planning data plane; mirrors the standalone /v1/plan* surface)
        .route("/plans", get(list_plans))
        .route("/plans/dags", get(dags_view))
        .route("/plan", post(plan_new))
        .route("/plan/from-text", post(plan_from_text))
        .route("/plan/refine", post(plan_refine))
        .route("/plan/:dag_id", get(plan_show))
        .route("/plan/:dag_id/render", get(dag_render))
        .route("/plan/:dag_id/execute", post(dag_execute))
        .route("/plan/:dag_id/nodes", post(create_dag_node))
        .route("/plan/:dag_id/nodes/:node_id", patch(update_dag_node).delete(delete_dag_node))
        .route("/plan/runs/:run_id/cancel", post(run_cancel))
        // Leases
        .route("/leases", get(list_leases).post(request_lease))
        .route("/leases/:lease_id", get(get_lease).delete(release_lease))
        .route("/leases/:lease_id/renew", post(renew_lease))
        // Context Packs (Checkpoints)
        .route("/workspace/:workspace_id/packs", post(create_context_pack))
        .route(
            "/workspace/:workspace_id/packs/:pack_id",
            get(get_context_pack),
        )
        // Context Packs (unscoped data plane; mirrors the standalone /v1/context-pack* surface)
        .route("/context-packs", post(list_context_packs))
        .route("/context-packs/seal", post(seal_context_pack))
        // Mail review/archive data plane
        .route("/mail/review", post(mail_request_review))
        .route("/mail/archive", post(mail_archive_thread))
        // Index
        .route("/index/rebuild", post(index_rebuild))
        // Gate data plane (status/check/rules/verify/decision/mutate)
        .route("/gate/evaluate", post(evaluate_policy))
        .route("/gate/status", get(gate_status))
        .route("/gate/check", post(gate_check))
        .route("/gate/rules", get(gate_rules))
        .route("/gate/verify", post(gate_verify))
        .route("/gate/decision", post(gate_decision))
        .route("/gate/mutate", post(gate_mutate))
        // Vault
        .route("/vault/status", get(vault_status))
        .route("/vault/archive", post(vault_archive))
}

/// [`factory_core_router`] plus the subpaths it renamed (`/`, `/health`,
/// `/dags…`, `/runs/:id/cancel`) in their old form. Mounted only under the old
/// prefixes, for callers already deployed (see main.rs); nothing new calls it.
pub fn legacy_alias_router() -> Router<Arc<AppState>> {
    factory_core_router()
        .route("/", get(health_check))
        .route("/health", get(health_check))
        .route("/dags", get(dags_view))
        .route("/dags/:dag_id/render", get(dag_render))
        .route("/dags/:dag_id/execute", post(dag_execute))
        .route("/dags/:dag_id/nodes", post(create_dag_node))
        .route("/dags/:dag_id/nodes/:node_id", patch(update_dag_node).delete(delete_dag_node))
        .route("/runs/:run_id/cancel", post(run_cancel))
}

// ============================================================================
// Handlers
// ============================================================================

async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let _ = state;
    Json(serde_json::json!({
        "status": "healthy",
        "service": "allternit-factory-core",
        "core": {
            "ledger": true,
            "gate": true,
            "leases": true,
        }
    }))
}

// ----------------------------------------------------------------------------
// Peer handlers (cross-session messaging)
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerInfoResponse {
    peer_id: String,
    name: String,
    cwd: String,
    vendor: String,
    inbox_socket: String,
    status: String,
    registered_at: String,
    last_heartbeat_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerListResponse {
    peers: Vec<PeerInfoResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerRegisterRequest {
    name: String,
    #[serde(default)]
    vendor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerRegisterResponse {
    peer_id: String,
    name: String,
    inbox_socket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerSendRequest {
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerSendResponse {
    delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerInboxResponse {
    messages: Vec<allternit_factory_engine::bus::BusMessage>,
}

fn peer_to_response(peer: &allternit_factory_engine::peer::Peer) -> PeerInfoResponse {
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

async fn list_peers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let peers = state.rails.peers.list();
    (
        StatusCode::OK,
        Json(PeerListResponse {
            peers: peers.iter().map(peer_to_response).collect(),
        }),
    )
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VisibilityPane {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) state: String,
    #[serde(rename = "lastMessage", skip_serializing_if = "Option::is_none")]
    pub(crate) last_message: Option<String>,
    #[serde(rename = "lastMessageAt", skip_serializing_if = "Option::is_none")]
    pub(crate) last_message_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VisibilityNeedNode {
    pub(crate) dag_id: String,
    pub(crate) node_id: String,
    pub(crate) title: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VisibilityNeed {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) node: Option<VisibilityNeedNode>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VisibilityDto {
    pub(crate) panes: Vec<VisibilityPane>,
    pub(crate) machines: Vec<serde_json::Value>,
    #[serde(rename = "fabricDevices")]
    pub(crate) fabric_devices: Vec<serde_json::Value>,
    #[serde(rename = "needsYou")]
    pub(crate) needs_you: Vec<VisibilityNeed>,
}

/// Read-only visibility DTO for the Factory rail. Prefers the pane engine's
/// `allternit-factory pane visibility` (blocked/idle panes + waiting-on-you);
/// falls back to the local peer registry when the engine is down.
async fn visibility(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dto = visibility::load_visibility(&state.rails.root_dir, &state.rails.peers, &state.rails.ledger).await;
    (StatusCode::OK, Json(dto))
}

async fn register_peer(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PeerRegisterRequest>,
) -> impl IntoResponse {
    let cwd = request.cwd.unwrap_or_else(|| state.rails.root_dir.clone());
    match state.rails.peers.register(&request.name, cwd, &request.vendor) {
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
    State(state): State<Arc<AppState>>,
    Path(id_or_name): Path<String>,
) -> impl IntoResponse {
    match state.rails.peers.unregister(&id_or_name) {
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
    State(state): State<Arc<AppState>>,
    Path(id_or_name): Path<String>,
) -> impl IntoResponse {
    match state.rails.peers.heartbeat(&id_or_name) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "peer not found" })),
        )
            .into_response(),
    }
}

async fn send_to_peer(
    State(state): State<Arc<AppState>>,
    Path(id_or_name): Path<String>,
    Json(request): Json<PeerSendRequest>,
) -> impl IntoResponse {
    let peer = match state.rails.peers.resolve(&id_or_name) {
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

    // Persist the envelope to the bus so peers can retrieve it via HTTP polling
    // even when the UDS push path is unavailable.
    let bus_msg = NewBusMessage {
        correlation_id: uuid::Uuid::new_v4().to_string(),
        to: format!("peer:{}", peer.name),
        from: from.clone(),
        kind: "peer_envelope".to_string(),
        payload: json!({ "body": body.clone(), "from": from.clone() }),
        transport: "http".to_string(),
    };
    let bus_id = match state.rails.bus.send_message(bus_msg).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!("failed to persist peer envelope to bus: {}", e);
            None
        }
    };

    let envelope = PeerEnvelope::new(&from, &peer.name, body);
    let receipt = send_envelope(&peer.inbox_socket, &envelope, Duration::from_secs(5)).await;

    // If UDS delivery succeeded, mark the bus message delivered so pollers do not
    // receive it again.
    if let (Ok(ref receipt), Some(bus_id)) = (&receipt, bus_id) {
        if receipt.delivered {
            let _ = state.rails.bus.mark_delivered(bus_id).await;
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
    State(state): State<Arc<AppState>>,
    Path(id_or_name): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let peer = match state.rails.peers.resolve(&id_or_name) {
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
        .rails
        .bus
        .poll_pending_for(&recipient, Some("http"), limit)
        .await
    {
        Ok(messages) => {
            for msg in &messages {
                let _ = state.rails.bus.mark_delivered(msg.id).await;
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

// ---------------------------------------------------------------------------
// Steer handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SteerCheckpointRequest {
    cwd: Option<std::path::PathBuf>,
}

#[derive(Debug, Serialize)]
struct SteerCheckpointResponse {
    changed: bool,
    hash: String,
    event_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SteerConsultRequest {
    cwd: Option<std::path::PathBuf>,
    prompt: Option<String>,
}

#[derive(Debug, Serialize)]
struct SteerConsultResponse {
    verdict: String,
    body: String,
}

async fn steer_checkpoint(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SteerCheckpointRequest>,
) -> impl IntoResponse {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.rails.root_dir.clone());
    match state.rails.steer.checkpoint(&cwd).await {
        Ok(result) => (
            StatusCode::OK,
            Json(SteerCheckpointResponse {
                changed: result.changed,
                hash: result.hash,
                event_id: result.event_id,
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

async fn steer_consult(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SteerConsultRequest>,
) -> impl IntoResponse {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.rails.root_dir.clone());
    let context = match request.prompt {
        Some(p) => p,
        None => match state.rails.steer.build_context(&cwd) {
            Ok(ctx) => ctx,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        },
    };
    match state.rails.steer.consult(&cwd, &context).await {
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
            (
                StatusCode::OK,
                Json(SteerConsultResponse { verdict, body: answer }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn steer_commit_gate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SteerConsultRequest>,
) -> impl IntoResponse {
    let cwd = request
        .cwd
        .unwrap_or_else(|| state.rails.root_dir.clone());
    match state.rails.steer.commit_gate(&cwd).await {
        Ok(result) => (
            StatusCode::OK,
            Json(SteerConsultResponse {
                verdict: result.verdict,
                body: result.body,
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

#[derive(Debug, Deserialize)]
struct LedgerQueryParams {
    since: Option<String>,
    dag_id: Option<String>,
    wih_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WihListRequest {
    dag_id: Option<String>,
    ready_only: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WihInfoResponse {
    wih_id: String,
    node_id: String,
    dag_id: Option<String>,
    status: String,
    title: Option<String>,
    description: Option<String>,
    assignee: Option<String>,
    blocked_by: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    execution_mode: Option<String>,
    picked_up_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct WihListResponse {
    wihs: Vec<WihInfoResponse>,
}

#[derive(Debug, Deserialize)]
struct WihPickupRequest {
    dag_id: String,
    node_id: String,
    agent_id: String,
    role: Option<String>,
    fresh: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WihPickupResponse {
    wih_id: String,
    context_pack_path: Option<String>,
    /// Node description with `{{ <node>.output }}` placeholders resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_prompt_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_description: Option<String>,
    /// Nonce of the untrusted-content fence around inlined outputs (S7).
    fence_nonce: String,
}

#[derive(Debug, Serialize)]
struct WihContextResponse {
    wih_id: String,
    context_pack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WihSignRequest {
    signature: String,
}

#[derive(Debug, Serialize)]
struct WihSignResponse {
    signed: bool,
}

#[derive(Debug, Deserialize)]
struct WihCloseRequest {
    status: String,
    evidence: Option<Vec<String>>,
    #[serde(default)]
    agent_id: String,
    /// Node output text: stored as an immutable blob behind a `node.output`
    /// receipt and exposed to dependents (ContextPack + placeholders).
    #[serde(default)]
    output: Option<String>,
}

#[derive(Debug, Serialize)]
struct WihCloseResponse {
    closed: bool,
    /// Node status after the close (`DONE`, `EXCEPTION`, `NEEDS_HUMAN`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    node_status: Option<String>,
    /// Judge verdict when the node policy has `verify: judge`.
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<serde_json::Value>,
}

async fn query_ledger(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LedgerQueryParams>,
) -> impl IntoResponse {
    debug!(?params, "Querying ledger");

    let mut scope = allternit_factory_engine::EventScope::default();
    if let Some(dag_id) = params.dag_id {
        scope.dag_id = Some(dag_id);
    }
    if let Some(wih_id) = params.wih_id {
        scope.wih_id = Some(wih_id);
    }

    let query = LedgerQuery {
        r#type: None,
        types: None,
        scope: if scope.dag_id.is_some() || scope.wih_id.is_some() {
            Some(scope)
        } else {
            None
        },
        since: params.since,
        until: None,
        limit: params.limit.or(Some(100)),
    };

    match state.rails.ledger.query(query).await {
        Ok(events) => {
            let response: Vec<LedgerEventResponse> = events
                .into_iter()
                .map(|e| LedgerEventResponse {
                    event_id: e.event_id,
                    ts: e.ts,
                    actor_type: format!("{:?}", e.actor.r#type),
                    actor_id: e.actor.id,
                    event_type: e.r#type,
                    payload: e.payload,
                    scope: e.scope.map(|s| EventScopeResponse {
                        dag_id: s.dag_id,
                        wih_id: s.wih_id,
                        run_id: s.run_id,
                    }),
                })
                .collect();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to query ledger");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// Request body for the UI's ledger tail surface (`rails.service.ts` ledger.tail).
#[derive(Debug, Deserialize)]
struct LedgerTailRequest {
    count: Option<usize>,
}

/// Ledger event shape the web surfaces render (`rails.service.ts` LedgerEvent).
#[derive(Debug, Serialize)]
struct UiLedgerEvent {
    event_id: String,
    event_type: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<UiLedgerEventScope>,
    payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct UiLedgerEventScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    dag_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wih_id: Option<String>,
}

async fn tail_ledger(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LedgerTailRequest>,
) -> impl IntoResponse {
    let count = req.count.unwrap_or(50);
    debug!(count, "Tailing ledger");

    match state.rails.ledger.tail(count).await {
        Ok(events) => {
            let response: Vec<UiLedgerEvent> = events
                .into_iter()
                .map(|e| UiLedgerEvent {
                    event_id: e.event_id,
                    event_type: e.r#type,
                    timestamp: e.ts,
                    scope: e.scope.map(|s| UiLedgerEventScope {
                        dag_id: s.dag_id,
                        node_id: s.node_id,
                        wih_id: s.wih_id,
                    }),
                    payload: e.payload,
                })
                .collect();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to tail ledger");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// Request body for the UI's receipts surface (`rails.service.ts` ReceiptQueryRequest).
#[derive(Debug, Deserialize)]
struct ReceiptsUiQueryRequest {
    dag_id: Option<String>,
    node_id: Option<String>,
    wih_id: Option<String>,
    kinds: Option<Vec<String>>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<usize>,
}

async fn query_receipts(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReceiptsUiQueryRequest>,
) -> impl IntoResponse {
    debug!(limit = ?req.limit, "Querying receipts");

    // Records carry only run_id/tool; the orchestrator bridge tags executor
    // receipts with run_id = executor slug, so dag/node filters map to run_id.
    const TOOL_CALL_KIND: &str = "tool_call_post";
    if let Some(kinds) = &req.kinds {
        if !kinds.iter().any(|k| k == TOOL_CALL_KIND) {
            return (StatusCode::OK, Json(json!({ "receipts": [] }))).into_response();
        }
    }

    let since_ts = req.since.as_deref().and_then(parse_rfc3339);
    let until_ts = req.until.as_deref().and_then(parse_rfc3339);

    let store_query = ReceiptQuery {
        run_id: req
            .node_id
            .clone()
            .or_else(|| req.dag_id.clone())
            .or_else(|| req.wih_id.clone()),
        limit: req.limit,
        ..Default::default()
    };

    let records = match state.rails.receipts.query_receipts(&store_query) {
        Ok(records) => records,
        Err(e) => {
            error!(error = %e, "Failed to query receipts");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let receipts: Vec<serde_json::Value> = records
        .into_iter()
        .filter_map(|record| {
            let written_at = receipt_written_at(&state.rails, &record.receipt_id);
            if let Some(since) = since_ts {
                if written_at < since {
                    return None;
                }
            }
            if let Some(until) = until_ts {
                if written_at > until {
                    return None;
                }
            }
            let outputs_ref = record.outputs_ref.clone();
            let run_id = record.run_id.clone();
            let mut payload = serde_json::to_value(&record).unwrap_or_else(|_| json!({}));
            // Artifact scanners (ArtifactCenter, executor tiles) look for a
            // `path` key in the payload — alias outputs_ref to it.
            if let (Some(obj), Some(reference)) = (payload.as_object_mut(), outputs_ref) {
                obj.insert("path".to_string(), json!(reference));
            }
            Some(json!({
                "receipt_id": record.receipt_id,
                "kind": TOOL_CALL_KIND,
                "run_id": run_id.clone(),
                "dag_id": "",
                "node_id": run_id,
                "wih_id": "",
                "timestamp": written_at.to_rfc3339(),
                "payload": payload,
            }))
        })
        .collect();

    (StatusCode::OK, Json(json!({ "receipts": receipts }))).into_response()
}

fn parse_rfc3339(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Receipts have no timestamp field; the receipt file's mtime is when it was
/// written. Falls back to the epoch if the file cannot be stat'ed.
fn receipt_written_at(rails: &RailsState, receipt_id: &str) -> chrono::DateTime<chrono::Utc> {
    let path = rails.receipts.receipt_path(receipt_id);
    std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or_else(|_| chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::UNIX_EPOCH))
}

// ---------------------------------------------------------------------------
// Mail + receipt write routes (orchestrator bridge / CLI agent access)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MailWriteRequest {
    /// UI convention is `thread_id`; `thread` is the legacy backend name.
    #[serde(rename = "thread_id", alias = "thread")]
    thread: Option<String>,
    body_ref: Option<String>,
    body: Option<String>,
    attachments: Option<Vec<String>>,
    // Typed envelope fields (E1-R2). `from_agent` present => typed send.
    from_agent: Option<String>,
    to_agents: Option<Vec<String>>,
    /// Convenience single-recipient alias for `to_agents`.
    to_agent_id: Option<String>,
    subject: Option<String>,
    importance: Option<MailImportance>,
    /// UI convention is `priority`; maps to `importance`.
    priority: Option<String>,
    ack_required: Option<bool>,
    /// UI convention is `requires_ack`; maps to `ack_required`.
    requires_ack: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct MailEnsureThreadRequest {
    topic: String,
    #[serde(default)]
    participants: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MailAckRequest {
    thread_id: String,
    message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

async fn ensure_mail_thread(state: &AppState, topic: &str) -> Result<String, axum::response::Response> {
    state.rails.mail.ensure_thread(topic).await.map_err(|e| {
        error!(error = %e, "mail: ensure_thread failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response()
    })
}

/// THE shared thread-resolution helper for every mail endpoint (E1-R4):
/// an omitted thread defaults to `mail:general`, and thread validation
/// accepts `dag:`/`wih:`/`mail:` prefixes (enforced inside `ensure_thread`
/// via `resolve_thread_id`/`canonical_thread_id` in the rails crate).
/// mail_send / mail_share / mail_decide all funnel through here — do not
/// re-inline the pattern.
async fn resolve_mail_thread(
    state: &AppState,
    thread: Option<String>,
) -> Result<String, axum::response::Response> {
    let topic = match resolve_thread_id(thread.as_deref()) {
        Ok(topic) => topic,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response());
        }
    };
    ensure_mail_thread(state, &topic).await
}

fn parse_importance(priority: Option<&str>, importance: Option<MailImportance>) -> MailImportance {
    if let Some(imp) = importance {
        return imp;
    }
    match priority.unwrap_or("normal").to_lowercase().as_str() {
        "low" => MailImportance::Low,
        "high" | "urgent" => MailImportance::High,
        _ => MailImportance::Normal,
    }
}

fn message_to_json(msg: &allternit_factory_engine::mail::MailMessage, root_dir: &std::path::Path) -> serde_json::Value {
    let body = if let Some(path) = &msg.body_path {
        let abs = root_dir.join(path);
        std::fs::read_to_string(&abs).unwrap_or_default()
    } else {
        msg.body_ref.clone().unwrap_or_default()
    };
    json!({
        "message_id": msg.message_id,
        "thread_id": msg.thread_id,
        "from_agent": msg.from_agent,
        "to_agents": msg.to_agents,
        "to_agent": msg.to_agents.first(),
        "subject": msg.subject,
        "importance": msg.importance,
        "priority": msg.importance,
        "ack_required": msg.ack_required,
        "acknowledged": !msg.ack_required,
        "body_path": msg.body_path,
        "body_ref": msg.body_ref,
        "body": body,
        "timestamp": msg.timestamp,
    })
}

async fn mail_send(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailWriteRequest>,
) -> impl IntoResponse {
    let thread_id = match resolve_mail_thread(&state, req.thread).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    let body = req.body.or(req.body_ref).unwrap_or_default();
    if let Some(from_agent) = req.from_agent {
        // Typed envelope path (E1-R2).
        let mut to_agents = req.to_agents.unwrap_or_default();
        if let Some(to) = req.to_agent_id {
            if !to_agents.contains(&to) {
                to_agents.push(to);
            }
        }
        let message = TypedMessage {
            from_agent,
            to_agents,
            subject: req.subject,
            importance: parse_importance(req.priority.as_deref(), req.importance),
            ack_required: req.ack_required.or(req.requires_ack).unwrap_or(false),
            body,
        };
        return match state.rails.mail.send_typed_message(&thread_id, message).await {
            Ok(message_id) => (
                StatusCode::OK,
                Json(json!({ "sent": true, "thread_id": thread_id, "message_id": message_id })),
            )
                .into_response(),
            Err(e) => {
                error!(error = %e, "mail: send_typed_message failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        };
    }
    // Legacy body/body_ref path — unchanged behavior.
    match state
        .rails
        .mail
        .send_message(&thread_id, &body, req.attachments.unwrap_or_default())
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "sent": true, "thread_id": thread_id }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: send_message failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn ensure_mail_thread_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailEnsureThreadRequest>,
) -> impl IntoResponse {
    let topic = if req.topic.starts_with("dag:") || req.topic.starts_with("wih:") || req.topic.starts_with("mail:") {
        req.topic
    } else {
        format!("mail:{}", req.topic)
    };
    match state.rails.mail.ensure_thread(&topic).await {
        Ok(thread_id) => (
            StatusCode::OK,
            Json(json!({ "thread_id": thread_id, "topic": topic })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "mail: ensure_thread handler failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn ack_mail_message(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailAckRequest>,
) -> impl IntoResponse {
    match state
        .rails
        .mail
        .acknowledge_message(&req.thread_id, &req.message_id, req.agent_id.as_deref(), req.note.as_deref())
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "acknowledged": true }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: ack failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Mail agent identities + per-agent inbox/outbox (E1)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MailAgentRegisterRequest {
    agent_id: String,
    display_name: Option<String>,
    metadata: Option<serde_json::Value>,
}

async fn register_mail_agent(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailAgentRegisterRequest>,
) -> impl IntoResponse {
    match state
        .rails
        .mail
        .agents()
        .register_agent(&req.agent_id, req.display_name.as_deref(), req.metadata)
        .await
    {
        Ok((agent, created)) => (
            StatusCode::OK,
            Json(json!({ "agent": agent, "created": created })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "mail: register agent failed");
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn list_mail_agents(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.mail.agents().list_agents().await {
        Ok(agents) => (StatusCode::OK, Json(json!({ "agents": agents }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: list agents failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailSearchQuery {
    q: Option<String>,
    limit: Option<i64>,
}

async fn search_mail_messages(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MailSearchQuery>,
) -> impl IntoResponse {
    let q = query.q.unwrap_or_default();
    let q = q.trim();
    if q.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "q required" })),
        )
            .into_response();
    }
    let limit = query.limit.unwrap_or(20);
    match state.rails.mail.search_messages(q, limit).await {
        Ok(hits) => (
            StatusCode::OK,
            Json(json!({ "q": q, "results": hits })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "mail: search failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailOverdueQuery {
    agent: Option<String>,
    older_than: Option<i64>,
}

async fn read_mail_overdue(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MailOverdueQuery>,
) -> impl IntoResponse {
    let older_than = query.older_than.unwrap_or(0);
    match state
        .rails
        .mail
        .overdue(query.agent.as_deref(), older_than)
        .await
    {
        Ok(rows) => (StatusCode::OK, Json(json!({ "overdue": rows }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: overdue failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailBoxQuery {
    limit: Option<usize>,
}

async fn read_mail_inbox(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<String>,
    Query(query): Query<MailBoxQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100);
    match state.rails.mail.inbox(&agent_id, limit).await {
        Ok(messages) => {
            let messages: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| message_to_json(m, &state.rails.root_dir))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "agent_id": agent_id, "messages": messages })),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: inbox failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn read_mail_outbox(
    State(state): State<Arc<AppState>>,
    Path(agent_id): Path<String>,
    Query(query): Query<MailBoxQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100);
    match state.rails.mail.outbox(&agent_id, limit).await {
        Ok(messages) => {
            let messages: Vec<serde_json::Value> = messages
                .iter()
                .map(|m| message_to_json(m, &state.rails.root_dir))
                .collect();
            (
                StatusCode::OK,
                Json(json!({ "agent_id": agent_id, "messages": messages })),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: outbox failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn list_mail_threads(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.mail.list_messages(None, 5000).await {
        Ok(events) => {
            // Events arrive sorted by ts ascending; overwriting keeps the latest.
            let mut threads: std::collections::HashMap<String, (usize, String)> =
                std::collections::HashMap::new();
            for evt in &events {
                if let Some(thread_id) = evt.payload.get("thread_id").and_then(|v| v.as_str()) {
                    let entry = threads
                        .entry(thread_id.to_string())
                        .or_insert((0, evt.ts.clone()));
                    entry.0 += 1;
                    entry.1 = evt.ts.clone();
                }
            }
            let rows: Vec<serde_json::Value> = threads
                .into_iter()
                .map(|(thread_id, (messages, last_ts))| {
                    json!({ "thread_id": thread_id, "messages": messages, "last_ts": last_ts })
                })
                .collect();
            (StatusCode::OK, Json(json!({ "threads": rows }))).into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: list threads failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn read_mail_thread(
    State(state): State<Arc<AppState>>,
    Path(thread_id): Path<String>,
) -> impl IntoResponse {
    match state.rails.mail.list_messages(Some(&thread_id), 500).await {
        Ok(events) => {
            let messages: Vec<serde_json::Value> = events
                .into_iter()
                .filter_map(|evt| {
                    if evt.r#type != "MessageSent" {
                        return None;
                    }
                    let msg = allternit_factory_engine::mail::MailMessage::from_event(&evt)?;
                    Some(message_to_json(&msg, &state.rails.root_dir))
                })
                .collect();
            (StatusCode::OK, Json(json!({ "messages": messages }))).into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: read thread failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailShareRequest {
    thread: Option<String>,
    asset_ref: Option<String>,
    path: Option<String>,
    note: Option<String>,
}

async fn mail_share(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailShareRequest>,
) -> impl IntoResponse {
    let asset = match req.asset_ref.or(req.path) {
        Some(asset) => asset,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "asset_ref or path required" })),
            )
                .into_response();
        }
    };
    let thread_id = match resolve_mail_thread(&state, req.thread).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state
        .rails
        .mail
        .share_asset(&thread_id, &asset, req.note.as_deref())
        .await
    {
        Ok(share_id) => {
            // Self-announced assets become receipts so artifact surfaces
            // (ArtifactCenter, executor tiles) pick them up via one channel.
            let run_id = thread_id
                .strip_prefix("wih:executor-")
                .or_else(|| thread_id.strip_prefix("executor:"))
                .unwrap_or(&thread_id)
                .to_string();
            let receipt = ReceiptRecord {
                receipt_id: allternit_factory_engine::core::ids::create_receipt_id(),
                run_id,
                step: None,
                tool: "executor".to_string(),
                tool_version: None,
                inputs_ref: None,
                outputs_ref: Some(asset.clone()),
                exit: None,
                input_tokens: None,
                output_tokens: None,
                total_tokens: None,
            };
            if let Err(e) = state.rails.receipts.write_receipt(&receipt) {
                error!(error = %e, "receipt write for shared asset failed");
            }
            (
                StatusCode::OK,
                Json(json!({ "shared": true, "share_id": share_id, "thread_id": thread_id })),
            )
                .into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: share_asset failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailDecideRequest {
    thread: Option<String>,
    decision: Option<String>,
    approve: Option<bool>,
    notes_ref: Option<String>,
}

async fn mail_decide(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MailDecideRequest>,
) -> impl IntoResponse {
    let decision = match req.decision.or_else(|| {
        req.approve
            .map(|approve| if approve { "accepted".to_string() } else { "rejected".to_string() })
    }) {
        Some(decision) => decision,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "decision or approve required" })),
            )
                .into_response();
        }
    };
    let thread_id = match resolve_mail_thread(&state, req.thread).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state
        .rails
        .mail
        .decide_review(&thread_id, &decision, req.notes_ref)
        .await
    {
        Ok(_) => {
            // When the thread belongs to a pending outbound agent email, action
            // the mailflare side (approve/reject). No-op for ordinary threads.
            let approved = matches!(decision.as_str(), "accepted" | "approved" | "approve" | "accept");
            let email_outcome = crate::agent_email_routes::decide_outbound_for_thread(
                &state, &thread_id, approved,
            )
            .await;
            let mut body = json!({ "decided": true, "thread_id": thread_id });
            match email_outcome {
                crate::agent_email_routes::EmailDecisionOutcome::NotEmailThread => {}
                crate::agent_email_routes::EmailDecisionOutcome::Applied => {
                    body["email"] = json!({
                        "actioned": true,
                        "status": if approved { "sent" } else { "rejected" },
                    });
                }
                crate::agent_email_routes::EmailDecisionOutcome::Failed(message) => {
                    // The review decision stands (already in the ledger); the
                    // provider-side action failed and is recorded on the
                    // outbound row's error column.
                    body["email"] = json!({ "actioned": false, "error": message });
                }
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            error!(error = %e, "mail: decide_review failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReceiptWriteRequest {
    tool: Option<String>,
    run_id: Option<String>,
    inputs_ref: Option<String>,
    outputs_ref: Option<String>,
    exit_code: Option<i32>,
    summary: Option<String>,
}

async fn write_receipt(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReceiptWriteRequest>,
) -> impl IntoResponse {
    let exit = if req.exit_code.is_some() || req.summary.is_some() {
        Some(allternit_factory_engine::core::types::ReceiptExit {
            code: req.exit_code,
            summary: req.summary,
        })
    } else {
        None
    };
    let receipt = ReceiptRecord {
        receipt_id: allternit_factory_engine::core::ids::create_receipt_id(),
        run_id: req.run_id.unwrap_or_else(|| "run_orchestrator".to_string()),
        step: None,
        tool: req.tool.unwrap_or_else(|| "executor".to_string()),
        tool_version: None,
        inputs_ref: req.inputs_ref,
        outputs_ref: req.outputs_ref,
        exit,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
    };
    let receipt_id = receipt.receipt_id.clone();
    match state.rails.receipts.write_receipt(&receipt) {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "receipt_id": receipt_id }))).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to write receipt");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn list_wihs(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<WihListRequest>,
) -> impl IntoResponse {
    info!(dag_id = ?req.dag_id, ready_only = ?req.ready_only, "Listing WIHs");

    (StatusCode::OK, Json(WihListResponse { wihs: Vec::new() }))
}

async fn list_wihs_get(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let wihs = active_wihs(&events)
                .into_iter()
                .map(|wih| WihInfoResponse {
                    wih_id: wih.wih_id,
                    node_id: wih.node_id,
                    dag_id: Some(wih.dag_id),
                    status: wih.status,
                    title: None,
                    description: None,
                    assignee: wih.agent_id,
                    blocked_by: Vec::new(),
                    created_at: wih.picked_up_at.clone(),
                    updated_at: wih.last_heartbeat,
                    execution_mode: wih.execution_mode,
                    picked_up_at: wih.picked_up_at,
                })
                .collect();
            (StatusCode::OK, Json(WihListResponse { wihs })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn pickup_wih(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WihPickupRequest>,
) -> impl IntoResponse {
    info!(
        dag_id = %req.dag_id,
        node_id = %req.node_id,
        agent_id = %req.agent_id,
        role = ?req.role,
        fresh = ?req.fresh,
        "Picking up WIH"
    );

    if req.dag_id.trim().is_empty() || req.node_id.trim().is_empty() || req.agent_id.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "dag_id, node_id, and agent_id are required" })),
        )
            .into_response();
    }
    let scope = EventScope {
        dag_id: Some(req.dag_id.clone()),
        node_id: Some(req.node_id.clone()),
        ..Default::default()
    };
    if let Err(resp) = ensure_policy_injected(&state, Some(scope)).await {
        return resp.into_response();
    }
    let options = WihPickupOptions {
        role: req.role,
        fresh: req.fresh.unwrap_or(false),
    };
    match state
        .rails
        .gate
        .wih_pickup_detailed(&req.dag_id, &req.node_id, &req.agent_id, options)
        .await
    {
        Ok(pickup) => (
            StatusCode::OK,
            Json(WihPickupResponse {
                wih_id: pickup.wih_id,
                context_pack_path: pickup.context_pack_path,
                resolved_prompt_path: pickup.resolved_prompt_path,
                resolved_description: pickup.resolved_description,
                fence_nonce: pickup.fence_nonce,
            }),
        )
            .into_response(),
        Err(e) => {
            // Structured Gate 1 denial (wait-gate, blockers, placeholder refs).
            let gate_error = allternit_factory_engine::GateError::from_anyhow(&e)
                .and_then(|g| serde_json::to_value(g).ok());
            (
                StatusCode::CONFLICT,
                Json(json!({ "error": e.to_string(), "gate_error": gate_error })),
            )
                .into_response()
        }
    }
}

async fn get_wih_context(
    State(state): State<Arc<AppState>>,
    Path(wih_id): Path<String>,
) -> impl IntoResponse {
    info!(wih_id = %wih_id, "Fetching WIH context");

    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let wih = project_wih(&events, &wih_id);
            let context_pack = wih
                .as_ref()
                .and_then(|w| w.context_pack_path.clone())
                .and_then(|path| std::fs::read_to_string(path).ok());
            let resolved_prompt = wih
                .as_ref()
                .and_then(|w| w.resolved_prompt_path.clone())
                .and_then(|path| std::fs::read_to_string(path).ok());
            (
                StatusCode::OK,
                Json(WihContextResponse {
                    wih_id,
                    context_pack,
                    resolved_prompt,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn sign_wih(
    State(state): State<Arc<AppState>>,
    Path(wih_id): Path<String>,
    Json(req): Json<WihSignRequest>,
) -> impl IntoResponse {
    info!(wih_id = %wih_id, signature = %req.signature, "Signing WIH");

    let scope = EventScope {
        wih_id: Some(wih_id.clone()),
        ..Default::default()
    };
    if let Err(resp) = ensure_policy_injected(&state, Some(scope)).await {
        return resp.into_response();
    }
    match state.rails.gate.wih_sign_open(&wih_id, &req.signature).await {
        Ok(_) => (StatusCode::OK, Json(WihSignResponse { signed: true })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn close_wih(
    State(state): State<Arc<AppState>>,
    Path(wih_id): Path<String>,
    Json(req): Json<WihCloseRequest>,
) -> impl IntoResponse {
    info!(
        wih_id = %wih_id,
        status = %req.status,
        evidence_count = req.evidence.as_ref().map(|items| items.len()).unwrap_or(0),
        "Closing WIH"
    );

    if req.agent_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "agent_id is required" })),
        )
            .into_response();
    }

    let status = req.status.trim().to_uppercase();
    if status != "DONE" && status != "FAILED" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "status must be DONE or FAILED" })),
        )
            .into_response();
    }

    // Agent ownership check happens before any gate mutation.
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => match project_wih(&events, &wih_id) {
            Some(wih) => {
                if let Some(owner) = &wih.agent_id {
                    if owner != &req.agent_id {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "error": "wih owned by another agent" })),
                        )
                            .into_response();
                    }
                }
            }
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "wih not found" })),
                )
                    .into_response();
            }
        },
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    let scope = EventScope {
        wih_id: Some(wih_id.clone()),
        ..Default::default()
    };
    if let Err(resp) = ensure_policy_injected(&state, Some(scope)).await {
        return resp.into_response();
    }
    let evidence = req.evidence.clone().unwrap_or_default();
    // Read-only observer hooks (factory/engine/spec/OBSERVER.md): opt-in pre-close
    // advice, repeat-failure advice after the close. Advisory only.
    allternit_factory_engine::observer::hook_before_close(
        state.rails.root_dir.clone(),
        state.rails.ledger.clone(),
        wih_id.clone(),
    )
    .await;
    match state
        .rails
        .gate
        // The route's closer is always the owning agent (checked above), so
        // under `close_by: verifier` a DONE close here is refused (409).
        .wih_close_as(
            &wih_id,
            &status,
            &evidence,
            req.output.as_deref(),
            Some(&allternit_factory_engine::Actor {
                r#type: allternit_factory_engine::ActorType::Agent,
                id: req.agent_id.clone(),
            }),
        )
        .await
    {
        Ok(outcome) => {
            allternit_factory_engine::observer::spawn_after_close(
                state.rails.root_dir.clone(),
                state.rails.ledger.clone(),
                wih_id.clone(),
            );
            (
                StatusCode::OK,
                Json(WihCloseResponse {
                    closed: true,
                    node_status: Some(outcome.node_status),
                    verdict: outcome.verdict.and_then(|v| serde_json::to_value(v).ok()),
                }),
            )
                .into_response()
        }
        Err(e) => {
            if let Some(gate_error) = allternit_factory_engine::GateError::from_anyhow(&e) {
                // Structured Gate 4 refusal (close_by_verifier, wih_already_closed).
                return (
                    StatusCode::CONFLICT,
                    Json(json!({ "error": e.to_string(), "gate_error": gate_error })),
                )
                    .into_response();
            }
            let status = if e.to_string().contains("evidence") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

async fn ensure_policy_injected(
    state: &AppState,
    scope: Option<EventScope>,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    allternit_factory_engine::policy::inject_policy(
        &state.rails.root_dir,
        &state.rails.ledger,
        scope,
        "gateway",
    )
    .await
    .map(|_| ())
    .map_err(|e| {
        error!(error = %e, "policy injection failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })
}

async fn list_dags(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
) -> impl IntoResponse {
    debug!(workspace_id, "Listing DAGs");

    // Query ledger for all events to project DAGs
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            // Group events by dag_id and project each
            let mut dags = std::collections::HashMap::new();
            for event in &events {
                if let Some(dag_id) = event.payload.get("dag_id").and_then(|v| v.as_str()) {
                    if !dags.contains_key(dag_id) {
                        dags.insert(dag_id.to_string(), project_dag(&events, dag_id));
                    }
                }
            }

            let response: Vec<DagSummaryResponse> = dags
                .into_iter()
                .map(|(dag_id, dag)| {
                    let status = dag
                        .nodes
                        .values()
                        .find(|n| n.parent_node_id.is_none())
                        .map(|n| n.status.clone())
                        .unwrap_or_else(|| "UNKNOWN".to_string());

                    let created_at = dag
                        .nodes
                        .values()
                        .filter_map(|n| n.created_at.as_ref())
                        .min()
                        .cloned()
                        .unwrap_or_default();

                    let updated_at = dag
                        .nodes
                        .values()
                        .filter_map(|n| n.updated_at.as_ref())
                        .max()
                        .cloned()
                        .unwrap_or_default();

                    DagSummaryResponse {
                        dag_id,
                        status,
                        node_count: dag.nodes.len(),
                        created_at,
                        updated_at,
                    }
                })
                .collect();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to query ledger for DAGs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn create_dag(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateDagRequest>,
) -> impl IntoResponse {
    info!(workspace_id, name = req.name, "Creating DAG");

    let dag_id = req
        .dag_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let node_id = format!("{}-root", dag_id);

    // In the new system, creating a DAG often means creating the first node
    let mutation = DagMutation::CreateNode {
        node_id: node_id.clone(),
        node_kind: "task".to_string(),
        title: req.name,
        parent_node_id: None,
        execution_mode: "shared".to_string(),
        description: None,
        executor: None,
    };

    match state
        .rails
        .gate
        .mutate_with_decision(
            &dag_id,
            "Creating DAG from API",
            Some(req.description.unwrap_or_default()),
            vec![mutation],
        )
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "dag_id": dag_id,
                "status": "created",
            })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "Failed to create DAG");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn get_dag(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, dag_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!(workspace_id, dag_id, "Getting DAG");

    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag = project_dag(&events, &dag_id);
            let status = dag
                .nodes
                .values()
                .find(|n| n.parent_node_id.is_none())
                .map(|n| n.status.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());

            let created_at = dag
                .nodes
                .values()
                .filter_map(|n| n.created_at.as_ref())
                .min()
                .cloned()
                .unwrap_or_default();

            let updated_at = dag
                .nodes
                .values()
                .filter_map(|n| n.updated_at.as_ref())
                .max()
                .cloned()
                .unwrap_or_default();

            let response = DagDetailResponse {
                dag_id: dag.dag_id,
                status,
                nodes: dag
                    .nodes
                    .into_iter()
                    .map(|(id, n)| DagNodeResponse {
                        id,
                        name: n.title,
                        description: n.description.unwrap_or_default(),
                        status: n.status,
                        execution_mode: n.execution_mode,
                        blocked_by: vec![], // Edges handled separately
                        related_to: vec![], // Relations handled separately
                    })
                    .collect(),
                edges: dag
                    .edges
                    .into_iter()
                    .map(|e| DagEdgeResponse {
                        from: e.from_node_id,
                        to: e.to_node_id,
                        edge_type: e.edge_type,
                    })
                    .collect(),
                created_at,
                updated_at,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to get DAG");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn start_dag(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, dag_id)): Path<(String, String)>,
) -> impl IntoResponse {
    info!(workspace_id, dag_id, "Starting DAG execution");

    // Starting a DAG usually means setting the state of its root node(s)
    let mutation = DagMutation::SetState {
        node_id: format!("{}-root", dag_id),
        dimension: "status".to_string(),
        value: "RUNNING".to_string(),
        reason: Some("Starting via API".to_string()),
    };

    match state
        .rails
        .gate
        .mutate_with_decision(&dag_id, "Starting DAG", None, vec![mutation])
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "dag_id": dag_id,
                "status": "started",
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn request_lease(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RequestLeaseRequest>,
) -> impl IntoResponse {
    info!(wih_id = req.wih_id, "Requesting lease");

    let lease_req = allternit_factory_engine::LeaseRequest {
        lease_id: uuid::Uuid::new_v4().to_string(),
        wih_id: req.wih_id,
        agent_id: req.agent_id,
        paths: req.paths,
        requested_at: chrono::Utc::now().to_rfc3339(),
        ttl_seconds: req.ttl_seconds,
    };

    match state.rails.leases.request(lease_req).await {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "lease_id": uuid::Uuid::new_v4().to_string(), // Would return actual lease_id
                "status": "requested",
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_lease(
    State(state): State<Arc<AppState>>,
    Path(lease_id): Path<String>,
) -> impl IntoResponse {
    debug!(lease_id, "Getting lease");
    let _ = state;

    // Placeholder - would query leases
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "lease_id": lease_id,
            "status": "granted",
        })),
    )
        .into_response()
}

async fn create_context_pack(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreatePackRequest>,
) -> impl IntoResponse {
    info!(workspace_id, "Creating context pack (checkpoint)");

    let pack_id = format!("cp_{}", uuid::Uuid::new_v4());
    let seal = ContextPackSeal {
        pack_id: pack_id.clone(),
        wih_id: req.wih_id.unwrap_or_else(|| "default".to_string()),
        dag_id: req.dag_id.unwrap_or_else(|| "default".to_string()),
        node_id: req.run_id.unwrap_or_else(|| "default".to_string()),
        inputs_manifest: vec![],
        method_version: "1.0.0".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        policy_bundle: None,
    };

    match state.rails.context_packs.store_seal(&seal) {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "pack_id": pack_id,
                "status": "created",
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn get_context_pack(
    State(state): State<Arc<AppState>>,
    Path((workspace_id, pack_id)): Path<(String, String)>,
) -> impl IntoResponse {
    debug!(workspace_id, pack_id, "Getting context pack");
    let _ = state;

    // Placeholder
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "pack_id": pack_id,
            "status": "available",
        })),
    )
        .into_response()
}

async fn evaluate_policy(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EvaluatePolicyRequest>,
) -> impl IntoResponse {
    debug!(
        action = req.action,
        resource = req.resource,
        tenant_id = req.tenant_id,
        context = ?req.context,
        "Evaluating policy"
    );
    let _ = state;

    // Placeholder - actual implementation would use Gate.evaluate()
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "action": req.action,
            "decision": "allow",
            "requires_approval": false,
        })),
    )
        .into_response()
}

/// Vault jobs are recorded in the ledger as `VaultJobCompleted` events (see
/// `Vault::archive_wih`). Status maps them to the shape the UI polls.
async fn vault_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let jobs: Vec<serde_json::Value> = events
                .into_iter()
                .filter(|e| e.r#type == "VaultJobCompleted")
                .map(|e| {
                    let wih_id = e
                        .payload
                        .get("wih_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    json!({
                        "wih_id": wih_id,
                        "status": "completed",
                        "created_at": e.ts,
                        "completed_at": e.ts,
                    })
                })
                .collect();
            (StatusCode::OK, Json(json!({ "jobs": jobs }))).into_response()
        }
        Err(e) => {
            error!(error = %e, "Failed to query vault jobs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct VaultArchiveRequest {
    wih_id: String,
}

async fn vault_archive(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VaultArchiveRequest>,
) -> impl IntoResponse {
    info!(wih_id = %req.wih_id, "Archiving WIH to vault");

    match state.rails.vault.archive_wih(&req.wih_id).await {
        Ok(path) => (
            StatusCode::OK,
            Json(json!({
                "archived": true,
                "path": path.to_string_lossy().to_string(),
            })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, wih_id = %req.wih_id, "Failed to archive WIH");
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

// ============================================================================
// Plan data plane
//
// Unscoped planning/execution routes mirroring the standalone rails service's
// /v1/plan* and /v1/dags* surface. The web rails client (surfaces/
// ai.allternit.com/src/lib/agents/rails.service.ts) calls these; DAG state is
// projected from the ledger exactly like the workspace-scoped routes above.
// ============================================================================

#[derive(Debug, Deserialize)]
struct PlanNewRequest {
    text: String,
    /// Accepted for standalone-dialect compatibility. The gateway has no
    /// project scope, so the gate mints a fresh dag_id and it is returned in
    /// the response (same behavior as the standalone service).
    #[allow(dead_code)]
    dag_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlanRefineRequest {
    dag_id: String,
    delta: String,
    /// Accepted for contract compatibility; the gate records author + delta
    /// text, so this is informational only.
    #[allow(dead_code)]
    reason: Option<String>,
    mutations: Option<Vec<UiDagMutation>>,
}

#[derive(Debug, Deserialize)]
struct PlanFromTextRequest {
    title: String,
    todos: Vec<PlanFromTextTodo>,
}

#[derive(Debug, Deserialize)]
struct PlanFromTextTodo {
    title: String,
    #[serde(default)]
    depth: u32,
    #[serde(default)]
    done: bool,
}

#[derive(Debug, Deserialize)]
struct UiDagMutation {
    action: String,
    node_id: Option<String>,
    parent_id: Option<String>,
    /// Accepted for contract compatibility. Ordering hints have no dedicated
    /// ledger event in the gate projection, so this is not applied.
    #[allow(dead_code)]
    after_node_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
}

/// Map the client's mutation dialect (`rails.service.ts` DagMutation) onto
/// the crate's `DagMutation` events. `remove` has no ledger representation in
/// the gate projection and is rejected rather than silently dropped. A
/// `set_status` without an explicit `node_id` falls back to `default_node_id`
/// (the DAG's root node) — the pause/resume call sites transition the run
/// root and do not know its node id.
fn ui_dag_mutation_to_core(
    m: UiDagMutation,
    default_node_id: Option<&str>,
) -> std::result::Result<DagMutation, String> {
    match m.action.as_str() {
        "add" => {
            let node_id = m
                .node_id
                .ok_or_else(|| "add mutation requires node_id".to_string())?;
            Ok(DagMutation::CreateNode {
                node_id: node_id.clone(),
                node_kind: "task".to_string(),
                title: m.title.unwrap_or(node_id),
                parent_node_id: m.parent_id,
                execution_mode: "shared".to_string(),
                description: None,
                executor: None,
            })
        }
        "modify" => {
            let node_id = m
                .node_id
                .ok_or_else(|| "modify mutation requires node_id".to_string())?;
            let mut patch = serde_json::json!({});
            if let Some(title) = m.title {
                patch["title"] = serde_json::json!(title);
            }
            if let Some(description) = m.description {
                patch["description"] = serde_json::json!(description);
            }
            Ok(DagMutation::UpdateNode { node_id, patch })
        }
        "set_status" => {
            let node_id = match m.node_id.or_else(|| default_node_id.map(|s| s.to_string())) {
                Some(node_id) => node_id,
                None => {
                    return Err(
                        "set_status mutation requires node_id (no root node to default to)"
                            .to_string(),
                    );
                }
            };
            let value = m
                .status
                .ok_or_else(|| "set_status mutation requires status".to_string())?;
            Ok(DagMutation::SetState {
                node_id,
                dimension: "status".to_string(),
                value,
                reason: None,
            })
        }
        "remove" => Err(
            "remove mutations have no ledger representation in the gate projection"
                .to_string(),
        ),
        other => Err(format!("unsupported mutation action: {other}")),
    }
}

/// Project a DAG from the ledger and return its root node's id, if any.
async fn project_root_node_id(ledger: &Ledger, dag_id: &str) -> Option<String> {
    let events = ledger.query(LedgerQuery::default()).await.ok()?;
    let dag_events: Vec<_> = events
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    let dag = project_dag(&dag_events, dag_id);
    dag.nodes
        .values()
        .find(|n| n.parent_node_id.is_none())
        .map(|n| n.node_id.clone())
}

fn collect_dag_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut dag_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
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
    dag_ids.into_iter().collect()
}

async fn list_plans(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let mut dags = Vec::new();
            for dag_id in collect_dag_ids(&events) {
                let dag_events: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str())
                    })
                    .cloned()
                    .collect();
                let dag = project_dag(&dag_events, &dag_id);
                // Version = number of dag-scoped events; every mutation bumps it.
                let version = dag_events.len().to_string();
                let created_at = dag
                    .nodes
                    .values()
                    .filter_map(|n| n.created_at.clone())
                    .min()
                    .unwrap_or_default();
                let root = dag
                    .nodes
                    .values()
                    .find(|n| n.parent_node_id.is_none())
                    .or_else(|| dag.nodes.values().next());
                let mut entry = json!({
                    "dag_id": dag_id,
                    "version": version,
                    "created_at": created_at,
                });
                if let Some(root) = root {
                    let mut metadata = json!({ "title": root.title });
                    if let Some(description) = &root.description {
                        metadata["description"] = json!(description);
                    }
                    entry["metadata"] = metadata;
                }
                dags.push(entry);
            }
            (StatusCode::OK, Json(json!({ "dags": dags }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct DagsViewQuery {
    view: Option<String>,
    dag_id: Option<String>,
    agent: Option<String>,
}

fn wih_json(wih: &WihState) -> serde_json::Value {
    json!({
        "wih_id": wih.wih_id,
        "dag_id": wih.dag_id,
        "node_id": wih.node_id,
        "agent_id": wih.agent_id,
        "status": wih.status,
        "execution_mode": wih.execution_mode,
        "picked_up_at": wih.picked_up_at,
    })
}

fn dag_node_json(node: &DagNode, ready: bool, wih: Option<&WihState>) -> serde_json::Value {
    json!({
        "node_id": node.node_id,
        "parent_node_id": node.parent_node_id,
        "title": node.title,
        "description": node.description,
        "labels": node.labels,
        "priority": node.priority,
        "status": node.status,
        "ready": ready,
        "assignee": wih.and_then(|w| w.agent_id.clone()),
        "current_wih_id": wih.map(|w| w.wih_id.clone()),
    })
}

async fn dags_view(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DagsViewQuery>,
) -> impl IntoResponse {
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let view = params.view.clone().unwrap_or_else(|| "all".to_string());
            let agent = params.agent.clone();
            let matches_agent = |wih: &&WihState| {
                agent.is_none() || wih.agent_id.as_deref() == agent.as_deref()
            };
            let active = active_wihs(&events);
            let active_wihs_json: Vec<serde_json::Value> = active
                .iter()
                .filter(matches_agent)
                .map(wih_json)
                .collect();
            let wih_by_node: std::collections::HashMap<&str, &WihState> = active
                .iter()
                .filter(matches_agent)
                .map(|w| (w.node_id.as_str(), w))
                .collect();

            let dag_ids: Vec<String> = match &params.dag_id {
                Some(id) => vec![id.clone()],
                None => collect_dag_ids(&events),
            };
            let mut dags = Vec::new();
            for dag_id in dag_ids {
                // Project from the full event log: DagNodeUpdated /
                // DagNodeStatusChanged-style events carry no dag_id in their
                // payload, so the dag-scoped filter drops renames and status
                // transitions from the projection.
                let dag = project_dag(&events, &dag_id);
                let ready_set: std::collections::HashSet<String> =
                    ready_nodes(&dag).into_iter().collect();
                let ready_count = ready_set.len();
                let done_count = dag.nodes.values().filter(|n| n.status == "DONE").count();
                let include_dag = match view.as_str() {
                    "ready" => ready_count > 0,
                    "mine" => dag
                        .nodes
                        .values()
                        .any(|n| wih_by_node.contains_key(n.node_id.as_str())),
                    _ => true,
                };
                if !include_dag {
                    continue;
                }
                let mut nodes: Vec<&DagNode> = dag.nodes.values().collect();
                nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
                let node_jsons: Vec<serde_json::Value> = nodes
                    .into_iter()
                    .filter(|n| match view.as_str() {
                        "ready" => n.status == "READY" || n.status == "RUNNING",
                        _ => true,
                    })
                    .map(|n| {
                        let wih = wih_by_node.get(n.node_id.as_str()).copied();
                        dag_node_json(n, ready_set.contains(&n.node_id), wih)
                    })
                    .collect();
                let root_title = dag
                    .nodes
                    .values()
                    .find(|n| n.parent_node_id.is_none())
                    .map(|n| n.title.clone());
                dags.push(json!({
                    "dag_id": dag_id,
                    "root_title": root_title,
                    "nodes": node_jsons,
                    "ready_count": ready_count,
                    "done_count": done_count,
                }));
            }
            (
                StatusCode::OK,
                Json(json!({ "dags": dags, "active_wihs": active_wihs_json })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn plan_new(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PlanNewRequest>,
) -> impl IntoResponse {
    match state.rails.gate.plan_new(&request.text, None).await {
        Ok((prompt_id, dag_id, node_id)) => {
            allternit_factory_engine::observer::spawn_on_plan(
                state.rails.root_dir.clone(),
                state.rails.ledger.clone(),
                dag_id.clone(),
            );
            (
                StatusCode::CREATED,
                Json(json!({
                    "prompt_id": prompt_id,
                    "dag_id": dag_id,
                    "node_id": node_id,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn plan_refine(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PlanRefineRequest>,
) -> impl IntoResponse {
    let ui_mutations = request.mutations.unwrap_or_default();
    let default_node_id = if ui_mutations
        .iter()
        .any(|m| m.action == "set_status" && m.node_id.is_none())
    {
        project_root_node_id(&state.rails.ledger, &request.dag_id).await
    } else {
        None
    };
    let mut mutations = Vec::new();
    for mutation in ui_mutations {
        match ui_dag_mutation_to_core(mutation, default_node_id.as_deref()) {
            Ok(mutation) => mutations.push(mutation),
            Err(message) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
                    .into_response();
            }
        }
    }
    match state
        .rails
        .gate
        .plan_refine(&request.dag_id, &request.delta, "api", mutations)
        .await
    {
        Ok(delta_id) => (StatusCode::OK, Json(json!({ "delta_id": delta_id }))).into_response(),
        Err(e) => {
            // A refine against a DAG with no linked prompt is a client error;
            // anything else is a storage/backend failure.
            let status = if e.to_string().contains("prompt") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

async fn plan_from_text(
    State(state): State<Arc<AppState>>,
    Json(request): Json<PlanFromTextRequest>,
) -> impl IntoResponse {
    let title = request.title.trim();
    if title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "title must not be empty" })),
        )
            .into_response();
    }
    if request.todos.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "todos must not be empty" })),
        )
            .into_response();
    }
    if request.todos.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "todos capped at 200" })),
        )
            .into_response();
    }
    if request.todos.iter().any(|t| t.title.trim().is_empty()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "todo titles must not be empty" })),
        )
            .into_response();
    }

    let (_, dag_id, root_node_id) = match state.rails.gate.plan_new(title, None).await {
        Ok(ids) => ids,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let mut mutations = Vec::new();
    let mut stack: Vec<(u32, String)> = Vec::new();
    for todo in &request.todos {
        let node_id = format!("n_{}", rand::random::<u32>() % 10_000);
        while stack.last().map(|(d, _)| *d) >= Some(todo.depth) {
            stack.pop();
        }
        let parent = if todo.depth == 0 {
            root_node_id.clone()
        } else {
            stack
                .last()
                .map(|(_, id)| id.clone())
                .unwrap_or_else(|| root_node_id.clone())
        };
        mutations.push(DagMutation::CreateNode {
            node_id: node_id.clone(),
            node_kind: "task".to_string(),
            title: todo.title.trim().to_string(),
            parent_node_id: Some(parent),
            execution_mode: "shared".to_string(),
            description: None,
            executor: None,
        });
        if todo.done {
            mutations.push(DagMutation::ChangeStatus {
                node_id: node_id.clone(),
                from: "NEW".to_string(),
                to: "DONE".to_string(),
                reason: Some("marked done at import".to_string()),
            });
        }
        stack.push((todo.depth, node_id));
    }
    let node_count = request.todos.len();

    match state
        .rails
        .gate
        .plan_refine(
            &dag_id,
            "plan from gizzi-code ExitPlanMode",
            "gizzi",
            mutations,
        )
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "dag_id": dag_id,
                "root_node_id": root_node_id,
                "node_count": node_count,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn plan_show(
    State(state): State<Arc<AppState>>,
    Path(dag_id): Path<String>,
) -> impl IntoResponse {
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
                .collect();
            let dag = project_dag(&dag_events, &dag_id);
            let dag_json = match serde_json::to_value(&dag) {
                Ok(value) => value,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": e.to_string() })),
                    )
                        .into_response();
                }
            };
            (StatusCode::OK, Json(json!({ "dag_id": dag_id, "dag": dag_json }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CreateDagNodeRequest {
    title: String,
    parent_node_id: String,
}

async fn create_dag_node(
    State(state): State<Arc<AppState>>,
    Path(dag_id): Path<String>,
    Json(req): Json<CreateDagNodeRequest>,
) -> impl IntoResponse {
    info!(dag_id = %dag_id, title = %req.title, "Creating DAG node");

    let title = req.title.trim();
    let parent = req.parent_node_id.trim();
    if title.is_empty() || parent.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "title and parent_node_id are required" })),
        )
            .into_response();
    }

    let events = match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let dag = project_dag(&events, &dag_id);
    if !dag.nodes.contains_key(parent) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "parent node not found" })),
        )
            .into_response();
    }

    let node_id = format!("n_{}", rand::random::<u32>() % 10_000);
    match state
        .rails
        .gate
        .mutate_with_decision(
            &dag_id,
            "api node add",
            None,
            vec![DagMutation::CreateNode {
                node_id: node_id.clone(),
                node_kind: "task".to_string(),
                title: title.to_string(),
                parent_node_id: Some(parent.to_string()),
                execution_mode: "shared".to_string(),
                description: None,
                executor: None,
            }],
        )
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({ "node_id": node_id })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateDagNodeRequest {
    title: Option<String>,
    // Double Option: absent field = no reparent, null = reparent to root,
    // string = reparent under that node. serde_json collapses a nested
    // Option<Option<String>> (null deserializes to the outer None, identical
    // to an absent field), so the distinction needs an explicit visitor.
    #[serde(default, deserialize_with = "deserialize_reparent")]
    parent_node_id: Option<Option<String>>,
    // Present (even as []) replaces the node's label set; absent = unchanged.
    labels: Option<Vec<String>>,
    // Absent = unchanged; empty string passes through and clears the text.
    description: Option<String>,
    // Three-state, same serde collapse as parent_node_id: absent = unchanged,
    // null = explicit clear attempt (NOT supported — apply_node_patch ignores
    // a null priority, so it is a no-op), integer = set.
    #[serde(default, deserialize_with = "deserialize_priority_patch")]
    priority: Option<Option<i64>>,
}

fn deserialize_priority_patch<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct PriorityVisitor;

    impl<'de> serde::de::Visitor<'de> for PriorityVisitor {
        type Value = Option<Option<i64>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null or an integer priority")
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(None))
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(None))
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            i64::deserialize(deserializer).map(Some).map(Some)
        }
    }

    deserializer.deserialize_option(PriorityVisitor)
}

fn deserialize_reparent<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ReparentVisitor;

    impl<'de> serde::de::Visitor<'de> for ReparentVisitor {
        type Value = Option<Option<String>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null or a parent node id string")
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(None))
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(None))
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            String::deserialize(deserializer).map(Some).map(Some)
        }
    }

    deserializer.deserialize_option(ReparentVisitor)
}

async fn update_dag_node(
    State(state): State<Arc<AppState>>,
    Path((dag_id, node_id)): Path<(String, String)>,
    Json(req): Json<UpdateDagNodeRequest>,
) -> impl IntoResponse {
    info!(dag_id = %dag_id, node_id = %node_id, "Updating DAG node");

    let title = req.title.as_deref().map(str::trim);
    if matches!(title, Some("")) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "title is required" })),
        )
            .into_response();
    }
    if req.title.is_none()
        && req.parent_node_id.is_none()
        && req.labels.is_none()
        && req.description.is_none()
        && req.priority.is_none()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "no changes: one of title, parent_node_id, labels, description, priority is required" })),
        )
            .into_response();
    }

    let events = match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let dag = project_dag(&events, &dag_id);
    let node = match dag.nodes.get(&node_id) {
        Some(node) => node,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "node not found" })),
            )
                .into_response();
        }
    };

    let mut mutations = Vec::new();
    if let Some(title) = title {
        mutations.push(DagMutation::UpdateNode {
            node_id: node_id.clone(),
            patch: serde_json::json!({ "title": title }),
        });
    }
    if let Some(new_parent) = &req.parent_node_id {
        if new_parent.as_deref() != node.parent_node_id.as_deref() {
            mutations.push(DagMutation::ReparentNode {
                node_id: node_id.clone(),
                new_parent_id: new_parent.clone(),
            });
        }
    }
    // labels/description/priority ride a single patch mutation carrying
    // exactly the provided keys.
    let mut patch_map = serde_json::Map::new();
    if let Some(labels) = &req.labels {
        patch_map.insert("labels".to_string(), serde_json::Value::from(labels.clone()));
    }
    if let Some(description) = &req.description {
        patch_map.insert(
            "description".to_string(),
            serde_json::Value::from(description.clone()),
        );
    }
    if let Some(Some(priority)) = req.priority {
        patch_map.insert("priority".to_string(), serde_json::Value::from(priority));
    }
    if !patch_map.is_empty() {
        mutations.push(DagMutation::UpdateNode {
            node_id: node_id.clone(),
            patch: serde_json::Value::Object(patch_map),
        });
    }
    if mutations.is_empty() {
        // Nothing to record: reparent identical to the current parent, or a
        // priority:null clear attempt (unsupported — apply_node_patch ignores
        // null priorities, so clearing is a documented v6 deferral).
        return (StatusCode::OK, Json(json!({ "node_id": node_id }))).into_response();
    }

    match state
        .rails
        .gate
        .mutate_with_decision(&dag_id, "api node update", None, mutations)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "node_id": node_id }))).into_response(),
        Err(e) => {
            let message = e.to_string();
            let status = if message.contains("cycle") {
                StatusCode::CONFLICT
            } else if message.contains("not found") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": message }))).into_response()
        }
    }
}

async fn delete_dag_node(
    State(state): State<Arc<AppState>>,
    Path((dag_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    info!(dag_id = %dag_id, node_id = %node_id, "Deleting DAG node");

    let events = match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => events,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let dag = project_dag(&events, &dag_id);
    let node = match dag.nodes.get(&node_id) {
        Some(node) => node,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "node not found" })),
            )
                .into_response();
        }
    };

    // Guard against deleting a node that has an ACTIVE WIH. The node's own
    // `current_wih_id` field is never populated by the projection (it is always
    // None), so scan active WIHs by node id instead — smoke-tested 2026-09-13:
    // relying on current_wih_id let a delete through and orphaned the WIH.
    let blocking_wihs: Vec<String> = active_wihs(&events)
        .into_iter()
        .filter(|w| w.dag_id == dag_id && w.node_id == node_id)
        .map(|w| w.wih_id)
        .collect();
    if !blocking_wihs.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!(
                    "node has an active WIH ({}); close it before deleting the node",
                    blocking_wihs.join(", ")
                )
            })),
        )
            .into_response();
    }

    let open_children: Vec<String> = dag
        .nodes
        .values()
        .filter(|n| n.parent_node_id.as_deref() == Some(node_id.as_str()))
        .filter(|n| n.status != "DONE")
        .map(|n| n.node_id.clone())
        .collect();
    if !open_children.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "node has children that are not DONE",
                "children": open_children
            })),
        )
            .into_response();
    }

    match state
        .rails
        .gate
        .mutate_with_decision(
            &dag_id,
            "api node delete",
            None,
            vec![DagMutation::DeleteNode { node_id }],
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn dag_render(
    State(state): State<Arc<AppState>>,
    Path(dag_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let format = params
        .get("format")
        .cloned()
        .unwrap_or_else(|| "json".to_string());

    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
                .collect();
            let dag = project_dag(&dag_events, &dag_id);
            let content = if format == "markdown" || format == "md" {
                render_dag_markdown(&dag)
            } else {
                serde_json::to_string_pretty(&dag).unwrap_or_default()
            };
            (
                StatusCode::OK,
                Json(json!({
                    "dag_id": dag_id,
                    "format": format,
                    "content": content,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

fn render_dag_markdown(dag: &allternit_factory_engine::work::types::DagState) -> String {
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

#[derive(Debug, Deserialize)]
struct DagExecuteRequest {
    run_id: Option<String>,
}

async fn dag_execute(
    State(state): State<Arc<AppState>>,
    Path(dag_id): Path<String>,
    Json(request): Json<DagExecuteRequest>,
) -> impl IntoResponse {
    let run_id = request
        .run_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // Project the DAG so the RUNNING transition lands on the real root node;
    // refuse to "start" a DAG the ledger has never seen.
    let root_node_id = match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag_events: Vec<_> = events
                .into_iter()
                .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str()))
                .collect();
            let dag = project_dag(&dag_events, &dag_id);
            match dag.nodes.values().find(|n| n.parent_node_id.is_none()) {
                Some(root) => root.node_id.clone(),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(json!({ "error": format!("dag {dag_id} not found") })),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let mutation = DagMutation::SetState {
        node_id: root_node_id,
        dimension: "status".to_string(),
        value: "RUNNING".to_string(),
        reason: Some(format!("execute run {run_id}")),
    };
    match state
        .rails
        .gate
        .mutate_with_decision(&dag_id, "Execute DAG", None, vec![mutation])
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "run_id": run_id, "status": "started" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn run_cancel(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let event = AllternitEvent {
        event_id: allternit_factory_engine::core::ids::create_event_id(),
        ts: chrono::Utc::now().to_rfc3339(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "api".to_string(),
        },
        scope: None,
        r#type: "RunCancelled".to_string(),
        payload: json!({ "run_id": run_id }),
        provenance: None,
    };
    match state.rails.ledger.append(event).await {
        Ok(_) => (StatusCode::OK, Json(json!({ "cancelled": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ============================================================================
// Leases data plane (list/renew/release; mirrors the standalone /v1/leases surface)
// ============================================================================

/// Gateway WIH ids follow the `"{dag_id}:{node_id}"` convention (see
/// `pickup_wih`); split them back apart for the UI lease view.
fn split_wih_id(wih_id: &str) -> (String, String) {
    match wih_id.split_once(':') {
        Some((dag_id, node_id)) => (dag_id.to_string(), node_id.to_string()),
        None => (String::new(), wih_id.to_string()),
    }
}

fn parse_epoch_millis(iso: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn lease_record_to_ui(lease: &LeaseRecord) -> serde_json::Value {
    let acquired_at = parse_epoch_millis(&lease.requested_at);
    let expires_at = lease
        .granted_until
        .as_deref()
        .map(parse_epoch_millis)
        .unwrap_or(0);
    let now = chrono::Utc::now().timestamp_millis();
    // The lease store does not track renewal counts; a granted lease that has
    // been renewed still reports 0.
    let status = match lease.status.as_str() {
        "released" => "released",
        "granted" | "requested" if expires_at > 0 && expires_at <= now => "expired",
        "granted" | "requested" if expires_at > 0 && expires_at - now <= 300_000 => "expiring",
        "granted" | "requested" => "active",
        // Denied/superseded leases are no longer live; the UI lease enum has
        // no "denied" state, so they fold into "released".
        _ => "released",
    };
    let (dag_id, node_id) = split_wih_id(&lease.wih_id);
    json!({
        "lease_id": lease.lease_id,
        "wih_id": lease.wih_id,
        "dag_id": dag_id,
        "node_id": node_id,
        "agent_id": lease.agent_id,
        "acquired_at": acquired_at,
        "expires_at": expires_at,
        "keys": lease.paths,
        "tools": [],
        "renewal_count": 0,
        "status": status,
    })
}

async fn list_leases(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let holder = params.get("holder").cloned();
    match state.rails.leases.list(holder.as_deref()).await {
        Ok(leases) => {
            let leases: Vec<serde_json::Value> =
                leases.iter().map(lease_record_to_ui).collect();
            (StatusCode::OK, Json(json!({ "leases": leases }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct LeaseRenewRequest {
    ttl_seconds: Option<i64>,
}

async fn renew_lease(
    State(state): State<Arc<AppState>>,
    Path(lease_id): Path<String>,
    Json(request): Json<LeaseRenewRequest>,
) -> impl IntoResponse {
    match state.rails.leases.get(&lease_id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("lease {lease_id} not found") })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }
    let ttl = request.ttl_seconds.unwrap_or(300).max(1);
    let until = chrono::Utc::now() + chrono::Duration::seconds(ttl);
    match state.rails.leases.renew(&lease_id, &until.to_rfc3339()).await {
        Ok(_) => match state.rails.leases.get(&lease_id).await {
            Ok(Some(lease)) => {
                let expires_at = lease
                    .granted_until
                    .as_deref()
                    .map(parse_epoch_millis)
                    .unwrap_or(0);
                (
                    StatusCode::OK,
                    Json(json!({ "renewed": true, "expires_at": expires_at })),
                )
                    .into_response()
            }
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "lease disappeared after renew" })),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn release_lease(
    State(state): State<Arc<AppState>>,
    Path(lease_id): Path<String>,
) -> impl IntoResponse {
    match state.rails.leases.get(&lease_id).await {
        Ok(Some(_)) => match state.rails.leases.release(&lease_id).await {
            Ok(_) => (StatusCode::OK, Json(json!({ "released": true }))).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("lease {lease_id} not found") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ============================================================================
// Context packs data plane (unscoped; mirrors the standalone /v1/context-pack* surface)
//
// Gateway-sealed packs persist the client-supplied inputs verbatim as a
// `pack.json` doc under `.allternit/context-packs/<pack_id>/` (the crate seal
// layout uses the same per-pack directories with a `seal.json`, so the two
// never collide). A `ContextPackSealed` ledger event is emitted for each seal.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayContextPack {
    context_pack_id: String,
    version: String,
    created_at: String,
    inputs: serde_json::Value,
    correlation_id: String,
}

#[derive(Debug, Deserialize)]
struct SealContextPackGatewayRequest {
    dag_id: String,
    node_id: String,
    wih_id: String,
    inputs: serde_json::Value,
}

fn gateway_packs_base(state: &AppState) -> PathBuf {
    state.rails.root_dir.join(".allternit").join("context-packs")
}

async fn seal_context_pack(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SealContextPackGatewayRequest>,
) -> impl IntoResponse {
    let stored_at = chrono::Utc::now().to_rfc3339();
    let pack = GatewayContextPack {
        context_pack_id: format!("cp_{}", uuid::Uuid::new_v4()),
        version: "1.0.0".to_string(),
        created_at: stored_at.clone(),
        inputs: request.inputs,
        correlation_id: uuid::Uuid::new_v4().to_string(),
    };

    let pack_dir = gateway_packs_base(&state).join(&pack.context_pack_id);
    if let Err(e) = std::fs::create_dir_all(&pack_dir) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let body = match serde_json::to_vec_pretty(&pack) {
        Ok(body) => body,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if let Err(e) = std::fs::write(pack_dir.join("pack.json"), body) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let event = AllternitEvent {
        event_id: allternit_factory_engine::core::ids::create_event_id(),
        ts: stored_at.clone(),
        actor: Actor {
            r#type: ActorType::Gate,
            id: "api".to_string(),
        },
        scope: None,
        r#type: "ContextPackSealed".to_string(),
        payload: json!({
            "context_pack_id": pack.context_pack_id,
            "dag_id": request.dag_id,
            "node_id": request.node_id,
            "wih_id": request.wih_id,
        }),
        provenance: None,
    };
    if let Err(e) = state.rails.ledger.append(event).await {
        error!(error = %e, "context pack seal: ledger append failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({
            "sealed": true,
            "context_pack_id": pack.context_pack_id,
            "stored_at": stored_at,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ContextPackListRequest {
    dag_id: Option<String>,
    node_id: Option<String>,
    wih_id: Option<String>,
    limit: Option<usize>,
}

async fn list_context_packs(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ContextPackListRequest>,
) -> impl IntoResponse {
    let base = gateway_packs_base(&state);
    let mut packs: Vec<GatewayContextPack> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let pack_dir = entry.path();
            if !pack_dir.is_dir() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(pack_dir.join("pack.json")) else {
                continue;
            };
            let Ok(pack) = serde_json::from_str::<GatewayContextPack>(&content) else {
                continue;
            };
            let matches = request
                .dag_id
                .as_deref()
                .map(|d| pack.inputs.get("dag_id").and_then(|v| v.as_str()) == Some(d))
                .unwrap_or(true)
                && request
                    .node_id
                    .as_deref()
                    .map(|d| pack.inputs.get("node_id").and_then(|v| v.as_str()) == Some(d))
                    .unwrap_or(true)
                && request
                    .wih_id
                    .as_deref()
                    .map(|d| pack.inputs.get("wih_id").and_then(|v| v.as_str()) == Some(d))
                    .unwrap_or(true);
            if matches {
                packs.push(pack);
            }
        }
    }
    packs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    packs.truncate(request.limit.unwrap_or(50));
    (StatusCode::OK, Json(json!({ "packs": packs }))).into_response()
}

// ============================================================================
// Mail review/archive data plane
// ============================================================================

#[derive(Debug, Deserialize)]
struct MailReviewRequest {
    /// UI convention is `thread_id`; `thread` is the legacy backend name.
    #[serde(rename = "thread_id", alias = "thread")]
    thread: Option<String>,
    wih_id: String,
    diff_ref: String,
}

async fn mail_request_review(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MailReviewRequest>,
) -> impl IntoResponse {
    let thread_id = match resolve_mail_thread(&state, request.thread).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state
        .rails
        .mail
        .request_review(&thread_id, &request.wih_id, &request.diff_ref)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "requested": true }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: request_review failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct MailArchiveRequest {
    /// UI convention is `thread_id`; `thread` is the legacy backend name.
    #[serde(rename = "thread_id", alias = "thread")]
    thread: Option<String>,
    path: String,
    reason: Option<String>,
}

async fn mail_archive_thread(
    State(state): State<Arc<AppState>>,
    Json(request): Json<MailArchiveRequest>,
) -> impl IntoResponse {
    let thread_id = match resolve_mail_thread(&state, request.thread).await {
        Ok(id) => id,
        Err(response) => return response,
    };
    match state
        .rails
        .mail
        .archive_thread(&thread_id, &request.path, request.reason.as_deref())
        .await
    {
        Ok(archive_id) => (StatusCode::OK, Json(json!({ "archive_id": archive_id }))).into_response(),
        Err(e) => {
            error!(error = %e, "mail: archive_thread failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// Index data plane
// ============================================================================

async fn index_rebuild(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.rails.index.rebuild_from_ledger(&state.rails.ledger).await {
        Ok(count) => (
            StatusCode::OK,
            Json(json!({ "indexed_count": count })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "index: rebuild failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

// ============================================================================
// Gate data plane (status/check/rules/verify/decision/mutate)
// ============================================================================

#[derive(Debug, Serialize)]
struct GateRulesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    rules: Option<String>,
}

async fn gate_status() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

#[derive(Debug, Deserialize)]
struct GateCheckRequest {
    wih_id: String,
    tool: String,
    paths: Option<Vec<String>>,
}

async fn gate_check(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GateCheckRequest>,
) -> impl IntoResponse {
    let paths = request.paths.unwrap_or_default();
    match state
        .rails
        .gate
        .pre_tool(&request.wih_id, &request.tool, &paths)
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(json!({
                "allowed": result.allowed,
                "reason": result.reason,
            })),
        )
            .into_response(),
        Err(e) => {
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

async fn gate_rules() -> impl IntoResponse {
    // The rules file ships with the factory engine's spec directory; in a
    // dev checkout that is two levels up from this crate's manifest dir.
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let candidates = [repo_root
        .join("factory")
        .join("engine")
        .join("spec")
        .join("GATE_RULES.md")];
    let rules = candidates
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok());
    (StatusCode::OK, Json(GateRulesResponse { rules })).into_response()
}

#[derive(Debug, Deserialize)]
struct GateVerifyRequest {
    json: Option<bool>,
}

async fn gate_verify(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GateVerifyRequest>,
) -> impl IntoResponse {
    let _json_output = request.json.unwrap_or(true);

    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let (chain_ok, chain_issues) = state
                .rails
                .ledger
                .verify_chain()
                .await
                .unwrap_or((false, vec!["verify failed".to_string()]));

            let mut cycle_dags = Vec::new();
            for dag_id in collect_dag_ids(&events) {
                let dag_events: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id.as_str())
                    })
                    .cloned()
                    .collect();
                let dag = project_dag(&dag_events, &dag_id);
                if allternit_factory_engine::work::graph::has_cycle_edges(&dag.edges) {
                    cycle_dags.push(dag_id);
                }
            }

            let ok = chain_ok && cycle_dags.is_empty();
            let mut body = json!({
                "ok": ok,
                "ledger_chain_ok": chain_ok,
                "cycle_dags": cycle_dags,
            });
            if !chain_issues.is_empty() {
                body["ledger_chain_issues"] = json!(chain_issues);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct GateDecisionRequest {
    note: String,
    reason: Option<String>,
    links: Option<Vec<String>>,
}

async fn gate_decision(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GateDecisionRequest>,
) -> impl IntoResponse {
    match state
        .rails
        .gate
        .record_agent_decision(&request.note, request.reason, request.links.unwrap_or_default())
        .await
    {
        Ok(decision_id) => (
            StatusCode::OK,
            Json(json!({ "decision_id": decision_id })),
        )
            .into_response(),
        Err(e) => {
            // Strict-provenance rejections (e.g. a decision with no linked
            // events) are client errors, not backend failures.
            (StatusCode::BAD_REQUEST, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct GateMutateRequest {
    dag_id: String,
    note: String,
    reason: Option<String>,
    mutations: Option<Vec<UiDagMutation>>,
}

async fn gate_mutate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GateMutateRequest>,
) -> impl IntoResponse {
    let ui_mutations = request.mutations.unwrap_or_default();
    let default_node_id = if ui_mutations
        .iter()
        .any(|m| m.action == "set_status" && m.node_id.is_none())
    {
        project_root_node_id(&state.rails.ledger, &request.dag_id).await
    } else {
        None
    };
    let mut mutations = Vec::new();
    for mutation in ui_mutations {
        match ui_dag_mutation_to_core(mutation, default_node_id.as_deref()) {
            Ok(mutation) => mutations.push(mutation),
            Err(message) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
                    .into_response();
            }
        }
    }
    match state
        .rails
        .gate
        .mutate_with_decision(&request.dag_id, &request.note, request.reason, mutations)
        .await
    {
        Ok((decision_id, mutation_ids)) => (
            StatusCode::OK,
            Json(json!({
                "decision_id": decision_id,
                "mutation_ids": mutation_ids,
            })),
        )
            .into_response(),
        Err(e) => {
            let status = if e.to_string().contains("required") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

// ============================================================================
// Response Structs
// ============================================================================

#[derive(Debug, Serialize)]
struct LedgerEventResponse {
    event_id: String,
    ts: String,
    actor_type: String,
    actor_id: String,
    event_type: String,
    payload: serde_json::Value,
    scope: Option<EventScopeResponse>,
}

#[derive(Debug, Serialize)]
struct EventScopeResponse {
    dag_id: Option<String>,
    wih_id: Option<String>,
    run_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct DagSummaryResponse {
    dag_id: String,
    status: String,
    node_count: usize,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct DagDetailResponse {
    dag_id: String,
    status: String,
    nodes: Vec<DagNodeResponse>,
    edges: Vec<DagEdgeResponse>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct DagNodeResponse {
    id: String,
    name: String,
    description: String,
    status: String,
    execution_mode: String,
    blocked_by: Vec<String>,
    related_to: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DagEdgeResponse {
    from: String,
    to: String,
    edge_type: String,
}

// ============================================================================
// Request Structs
// ============================================================================

#[derive(Debug, Deserialize)]
struct CreateDagRequest {
    dag_id: Option<String>,
    name: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RequestLeaseRequest {
    wih_id: String,
    agent_id: String,
    paths: Vec<String>,
    ttl_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct CreatePackRequest {
    dag_id: Option<String>,
    wih_id: Option<String>,
    run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EvaluatePolicyRequest {
    action: String,
    resource: String,
    tenant_id: String,
    context: Option<serde_json::Value>,
}

// ============================================================================
// Tickets (event-sourced ticket subsystem)
// ============================================================================

#[derive(Debug, Deserialize)]
struct TicketCreateRequest {
    title: String,
    description: Option<String>,
    kind: Option<String>,
    priority: Option<String>,
    labels: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct TicketResponse {
    ticket: Ticket,
}

#[derive(Debug, Deserialize)]
struct TicketListParams {
    status: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Serialize)]
struct TicketListResponse {
    tickets: Vec<Ticket>,
}

#[derive(Debug, Deserialize)]
struct TicketUpdateRequest {
    title: Option<String>,
    description: Option<String>,
    priority: Option<String>,
    labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TicketCloseRequest {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TicketDependencyRequest {
    to: String,
    kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct TicketReadyListResponse {
    ready: Vec<Ticket>,
}

#[derive(Debug, Serialize)]
struct TicketBlockedListResponse {
    blocked: Vec<BlockedTicket>,
}

fn ticket_error(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

fn ticket_store(state: &AppState) -> Result<TicketStore, axum::response::Response> {
    TicketStore::new(&state.rails.root_dir).map_err(|e| {
        error!(error = %e, "tickets: failed to open store");
        ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })
}

fn parse_ticket_id(raw: &str) -> Result<TicketId, axum::response::Response> {
    raw.parse::<TicketId>()
        .map_err(|e| ticket_error(StatusCode::BAD_REQUEST, e.to_string()))
}

async fn create_ticket(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TicketCreateRequest>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let kind = match req
        .kind
        .as_deref()
        .unwrap_or("task")
        .parse::<TicketKind>()
    {
        Ok(kind) => kind,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };
    let priority = match req
        .priority
        .as_deref()
        .unwrap_or("P2")
        .parse::<TicketPriority>()
    {
        Ok(priority) => priority,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let now = chrono::Utc::now();
    let id = TicketId::mint(format!("{}:{}", req.title, now).as_bytes());
    let ticket = Ticket {
        id: id.clone(),
        hierarchical_id: HierarchicalId::root(id),
        title: req.title,
        description: req.description.unwrap_or_default(),
        design: None,
        acceptance: None,
        notes: Vec::new(),
        status: TicketStatus::Open,
        kind,
        priority,
        assignee: None,
        estimate_minutes: None,
        due_at: None,
        defer_until: None,
        labels: req.labels.unwrap_or_default(),
        external_ref: None,
        metadata: std::collections::HashMap::new(),
        created_at: now,
        updated_at: now,
        closed_at: None,
        close_reason: None,
    };

    match store.create(ticket) {
        Ok(ticket) => (StatusCode::CREATED, Json(TicketResponse { ticket })).into_response(),
        Err(e) => {
            error!(error = %e, "tickets: create failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

async fn list_tickets(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TicketListParams>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let status_filter = match params
        .status
        .as_deref()
        .map(|s| s.parse::<TicketStatus>())
        .transpose()
    {
        Ok(status) => status,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    match store.list() {
        Ok(tickets) => {
            let tickets: Vec<Ticket> = tickets
                .into_iter()
                .filter(|t| status_filter.map(|s| t.status == s).unwrap_or(true))
                .filter(|t| {
                    params
                        .label
                        .as_ref()
                        .map(|l| t.labels.contains(l))
                        .unwrap_or(true)
                })
                .collect();
            (StatusCode::OK, Json(TicketListResponse { tickets })).into_response()
        }
        Err(e) => {
            error!(error = %e, "tickets: list failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

async fn get_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let id = match parse_ticket_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    match store.get(&id) {
        Ok(Some(ticket)) => (StatusCode::OK, Json(TicketResponse { ticket })).into_response(),
        Ok(None) => ticket_error(StatusCode::NOT_FOUND, format!("ticket {id} not found")),
        Err(e) => {
            error!(error = %e, "tickets: get failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

async fn update_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TicketUpdateRequest>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let id = match parse_ticket_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let priority = match req
        .priority
        .as_deref()
        .map(|p| p.parse::<TicketPriority>())
        .transpose()
    {
        Ok(priority) => priority,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    let patch = TicketUpdate {
        title: req.title,
        description: req.description,
        priority,
        labels: req.labels,
        ..Default::default()
    };
    match store.update(&id, patch) {
        Ok(ticket) => (StatusCode::OK, Json(TicketResponse { ticket })).into_response(),
        Err(e) => {
            error!(error = %e, "tickets: update failed");
            ticket_error(StatusCode::NOT_FOUND, e.to_string())
        }
    }
}

async fn close_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TicketCloseRequest>,
) -> impl IntoResponse {
    set_ticket_status(state, id, TicketStatus::Closed, req.reason).await
}

async fn reopen_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    set_ticket_status(state, id, TicketStatus::Open, None).await
}

async fn set_ticket_status(
    state: Arc<AppState>,
    raw_id: String,
    status: TicketStatus,
    reason: Option<String>,
) -> axum::response::Response {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let id = match parse_ticket_id(&raw_id) {
        Ok(id) => id,
        Err(response) => return response,
    };

    match store.set_status(&id, status, "api", reason) {
        Ok(ticket) => (StatusCode::OK, Json(TicketResponse { ticket })).into_response(),
        Err(e) => {
            error!(error = %e, "tickets: status change failed");
            ticket_error(StatusCode::NOT_FOUND, e.to_string())
        }
    }
}

async fn list_ready_tickets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match ready_inputs(&state) {
        Ok((all, graph, gates)) => match tickets::ready(&all, &graph, &gates, chrono::Utc::now()) {
            Ok(ready) => {
                (StatusCode::OK, Json(TicketReadyListResponse { ready })).into_response()
            }
            Err(e) => {
                error!(error = %e, "tickets: ready computation failed");
                ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        },
        Err(response) => response,
    }
}

async fn list_blocked_tickets(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match ready_inputs(&state) {
        Ok((all, graph, gates)) => {
            match tickets::blocked(&all, &graph, &gates, chrono::Utc::now()) {
                Ok(blocked) => {
                    (StatusCode::OK, Json(TicketBlockedListResponse { blocked })).into_response()
                }
                Err(e) => {
                    error!(error = %e, "tickets: blocked computation failed");
                    ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
                }
            }
        }
        Err(response) => response,
    }
}

fn ready_inputs(
    state: &AppState,
) -> Result<
    (
        Vec<Ticket>,
        dependencies::DependencyGraph,
        WaitGateStore,
    ),
    axum::response::Response,
> {
    let store = ticket_store(state)?;
    let all = store.list().map_err(|e| {
        error!(error = %e, "tickets: list failed");
        ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let graph = dependencies::load_graph(&state.rails.root_dir).map_err(|e| {
        error!(error = %e, "tickets: failed to load dependency graph");
        ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    let gates = WaitGateStore::new(&state.rails.root_dir).map_err(|e| {
        error!(error = %e, "tickets: failed to open wait-gate store");
        ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;
    Ok((all, graph, gates))
}

// ============================================================================
// Graph analytics (B2 robot surface)
//
// View-model builders live in the rails crate (`graph::views`) and are shared
// with the `rails graph` CLI, so both surfaces emit identical JSON. Per the
// B1 NOTES, `compute_insights` can block its caller for up to 5s on large
// graphs, so the whole load → ready → compute → build pipeline runs inside
// `tokio::task::spawn_blocking`.
// ============================================================================

/// Load tickets + graph + gates, compute the ready list, then run `build`.
/// Everything happens on a blocking thread; `build` receives the shared
/// state, all tickets, the dependency graph, and the ready list.
async fn run_graph_view<T, F>(state: Arc<AppState>, build: F) -> axum::response::Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&AppState, &[Ticket], &dependencies::DependencyGraph, Vec<Ticket>) -> Result<T, axum::response::Response>
        + Send
        + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let (all, graph, gates) = ready_inputs(&state)?;
        let ready = tickets::ready(&all, &graph, &gates, chrono::Utc::now()).map_err(|e| {
            error!(error = %e, "graph: ready computation failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;
        build(&state, &all, &graph, ready)
    })
    .await
    {
        Ok(Ok(view)) => (StatusCode::OK, Json(view)).into_response(),
        Ok(Err(response)) => response,
        Err(e) => {
            error!(error = %e, "graph: blocking task panicked or was cancelled");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

async fn graph_insights(State(state): State<Arc<AppState>>) -> axum::response::Response {
    run_graph_view(state, |state, _all, graph, ready| {
        let insights = state
            .rails
            .graph_analytics
            .compute_insights(graph, &InsightsConfig::default());
        let view = GraphView::from_graph(graph);
        Ok(views::build_insights_view(&insights, &ready, &view))
    })
    .await
}

async fn graph_triage(State(state): State<Arc<AppState>>) -> axum::response::Response {
    run_graph_view(state, |state, _all, graph, ready| {
        let insights = state
            .rails
            .graph_analytics
            .compute_insights(graph, &InsightsConfig::default());
        let view = GraphView::from_graph(graph);
        Ok(views::build_triage_view(&insights, &ready, &view))
    })
    .await
}

async fn graph_impact(
    State(state): State<Arc<AppState>>,
    Path(ticket_id): Path<String>,
) -> axum::response::Response {
    let id = match parse_ticket_id(&ticket_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    run_graph_view(state, move |state, all, graph, _ready| {
        let insights = state
            .rails
            .graph_analytics
            .compute_insights(graph, &InsightsConfig::default());
        let view = GraphView::from_graph(graph);
        views::build_impact_view(&insights, &view, all, &id).map_err(|e| match e {
            views::ViewError::UnknownTicket(id) => {
                ticket_error(StatusCode::NOT_FOUND, format!("ticket {id} not found"))
            }
        })
    })
    .await
}

async fn add_ticket_dependency(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TicketDependencyRequest>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let from = match parse_ticket_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let to = match parse_ticket_id(&req.to) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let kind = match req
        .kind
        .as_deref()
        .unwrap_or("blocks")
        .parse::<DependencyKind>()
    {
        Ok(kind) => kind,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    // Validate before applying (BatchExecutor precedent): no partial mutation.
    for endpoint in [&from, &to] {
        match store.get(endpoint) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return ticket_error(
                    StatusCode::NOT_FOUND,
                    format!("ticket {endpoint} not found"),
                )
            }
            Err(e) => {
                error!(error = %e, "tickets: get failed");
                return ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
        }
    }

    let graph = match dependencies::load_graph(&state.rails.root_dir) {
        Ok(graph) => graph,
        Err(e) => {
            error!(error = %e, "tickets: failed to load dependency graph");
            return ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    };
    let edge = DependencyEdge::new(from.clone(), to.clone(), kind);
    if edge.kind.is_blocking() && graph.would_cycle(&edge) {
        let mut g = graph.clone();
        g.add(edge.clone());
        let cycle: Vec<String> = g
            .find_cycle()
            .map(|ids| ids.iter().map(|id| id.to_string()).collect())
            .unwrap_or_else(|| vec![from.to_string(), to.to_string()]);
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "dependency would create a blocking cycle",
                "cycle": cycle,
            })),
        )
            .into_response();
    }

    match dependencies::add_edge(&state.rails.root_dir, &store, edge) {
        Ok(()) => (
            StatusCode::CREATED,
            Json(json!({ "added": true, "from": from, "to": to, "kind": kind.to_string() })),
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "tickets: add dependency failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
    }
}

async fn remove_ticket_dependency(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TicketDependencyRequest>,
) -> impl IntoResponse {
    let store = match ticket_store(&state) {
        Ok(store) => store,
        Err(response) => return response,
    };
    let from = match parse_ticket_id(&id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let to = match parse_ticket_id(&req.to) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let kind = match req
        .kind
        .as_deref()
        .unwrap_or("blocks")
        .parse::<DependencyKind>()
    {
        Ok(kind) => kind,
        Err(e) => return ticket_error(StatusCode::BAD_REQUEST, e.to_string()),
    };

    match dependencies::remove_edge(&state.rails.root_dir, &store, &from, &to, kind) {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "removed": true, "from": from, "to": to, "kind": kind.to_string() })),
        )
            .into_response(),
        Ok(false) => ticket_error(
            StatusCode::NOT_FOUND,
            format!("no {kind} dependency from {from} to {to}"),
        ),
        Err(e) => {
            error!(error = %e, "tickets: remove dependency failed");
            ticket_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        }
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
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    async fn body_json(body: axum::body::Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn test_app_state(temp: &std::path::Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    fn new_ticket(title: &str) -> Ticket {
        let now = chrono::Utc::now();
        let id = TicketId::mint(title.as_bytes());
        Ticket {
            hierarchical_id: HierarchicalId::root(id.clone()),
            id,
            title: title.to_string(),
            description: String::new(),
            design: None,
            acceptance: None,
            notes: Vec::new(),
            status: TicketStatus::Open,
            kind: TicketKind::Task,
            priority: TicketPriority::P2,
            assignee: None,
            estimate_minutes: None,
            due_at: None,
            defer_until: None,
            labels: Vec::new(),
            external_ref: None,
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
            closed_at: None,
            close_reason: None,
        }
    }

    async fn get(app: &Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn post_json(app: &Router, uri: &str, body: Value) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn delete_req(app: &Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn patch_json(app: &Router, uri: &str, body: Value) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// E1 mail round-trip: thread default fix (R4), agent registry (R1),
    /// typed envelope send (R2), per-agent inbox/outbox (R3), and the
    /// untouched share path whose events the inbox must skip.
    #[tokio::test]
    async fn mail_e1_endpoints_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-mail-e1-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // R4: legacy send with only {"body": ...} and no thread lands in
        // mail:general (previously a 500 on the "default" topic).
        let resp = post_json(&app, "/mail/send", json!({ "body": "hello" })).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["sent"], json!(true));
        assert_eq!(body["thread_id"], json!("mail:general"));

        // Invalid explicit thread is rejected.
        let resp = post_json(
            &app,
            "/mail/send",
            json!({ "thread": "bogus", "body": "x" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // R1: register alpha and beta; re-registering alpha is idempotent.
        let resp = post_json(
            &app,
            "/mail/agents/register",
            json!({ "agent_id": "alpha", "display_name": "Alpha" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["created"], json!(true));
        assert_eq!(body["agent"]["agent_id"], json!("alpha"));

        let resp = post_json(
            &app,
            "/mail/agents/register",
            json!({ "agent_id": "alpha", "display_name": "Renamed" }),
        )
        .await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["created"], json!(false));
        assert_eq!(body["agent"]["display_name"], json!("Alpha"));

        let resp = post_json(&app, "/mail/agents/register", json!({ "agent_id": "beta" })).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = get(&app, "/mail/agents").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["agents"].as_array().unwrap().len(), 2);

        // R2: typed send alpha -> beta, high importance, ack required.
        let resp = post_json(
            &app,
            "/mail/send",
            json!({
                "thread": "mail:general",
                "from_agent": "alpha",
                "to_agents": ["beta"],
                "subject": "S",
                "importance": "high",
                "ack_required": true,
                "body": "# Hi beta"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["sent"], json!(true));
        let message_id = body["message_id"].as_str().unwrap().to_string();
        // Body persisted as a file under .allternit/mail/messages/.
        let body_file = state
            .rails
            .root_dir
            .join(format!(".allternit/mail/messages/{}.md", message_id));
        assert_eq!(std::fs::read_to_string(body_file).unwrap(), "# Hi beta");

        // R3: beta's inbox carries the full typed envelope + the legacy
        // broadcast; alpha's outbox has the typed message.
        let resp = get(&app, "/mail/inbox/beta").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        let typed = messages
            .iter()
            .find(|m| m["message_id"] == json!(message_id))
            .expect("typed message in beta inbox");
        assert_eq!(typed["from_agent"], json!("alpha"));
        assert_eq!(typed["to_agents"], json!(["beta"]));
        assert_eq!(typed["subject"], json!("S"));
        assert_eq!(typed["importance"], json!("high"));
        assert_eq!(typed["ack_required"], json!(true));
        assert!(typed["body_path"]
            .as_str()
            .unwrap()
            .ends_with(&format!("{}.md", message_id)));

        let resp = get(&app, "/mail/outbox/alpha?limit=10").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);

        // NON-GOAL check: the share path still works (wih:pipeline-* shape)
        // and its MailAssetShared events do not leak into inboxes.
        let resp = post_json(
            &app,
            "/mail/share",
            json!({ "thread": "wih:pipeline-probe", "asset_ref": "outputs/probe.txt" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["shared"], json!(true));
        assert_eq!(body["thread_id"], json!("wih:pipeline-probe"));

        let resp = get(&app, "/mail/inbox/beta").await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// E2 mail search round-trip: typed sends are indexed on emit and found
    /// by subject/body word; legacy untyped sends stay out of the index;
    /// empty q is a 400.
    #[tokio::test]
    async fn mail_e2_search_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-mail-e2-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Typed send with a searchable subject.
        let resp = post_json(
            &app,
            "/mail/send",
            json!({
                "from_agent": "alpha",
                "to_agents": ["beta"],
                "subject": "deploy window Friday",
                "body": "Please review the rollout plan."
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let message_id = body["message_id"].as_str().unwrap().to_string();

        // Legacy untyped send — must not break or enter the index.
        let resp = post_json(&app, "/mail/send", json!({ "body": "legacy deploy chatter" })).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Search by subject word (Gherkin: q=deploy finds the message with
        // an excerpt) — and the legacy send stays out.
        let resp = get(&app, "/mail/search?q=deploy").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["message_id"], json!(message_id));
        assert_eq!(results[0]["thread_id"], json!("mail:general"));
        assert_eq!(results[0]["from_agent"], json!("alpha"));
        assert_eq!(results[0]["subject"], json!("deploy window Friday"));
        assert!(results[0]["ts"].as_str().unwrap().len() > 0);
        assert!(results[0]["excerpt"].as_str().unwrap().len() > 0);

        // Search by body-only word.
        let resp = get(&app, "/mail/search?q=rollout&limit=5").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["results"].as_array().unwrap().len(), 1);

        // Empty / missing q -> 400.
        let resp = get(&app, "/mail/search").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let resp = get(&app, "/mail/search?q=%20").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// E3 overdue round-trip: ack-required send appears in the recipient's
    /// overdue list until that recipient acks; ack_required=false never
    /// appears; older_than filters by age.
    #[tokio::test]
    async fn mail_e3_overdue_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-mail-e3-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // ack_required message alpha -> beta.
        let resp = post_json(
            &app,
            "/mail/send",
            json!({
                "from_agent": "alpha",
                "to_agents": ["beta"],
                "subject": "please ack",
                "ack_required": true,
                "body": "needs ack"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let message_id = body["message_id"].as_str().unwrap().to_string();

        // ack_required = false — never overdue.
        let resp = post_json(
            &app,
            "/mail/send",
            json!({
                "from_agent": "alpha",
                "to_agents": ["beta"],
                "subject": "fyi",
                "ack_required": false,
                "body": "no ack needed"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // beta's overdue list contains exactly the ack-required message.
        let resp = get(&app, "/mail/overdue?agent=beta").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let rows = body["overdue"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["message_id"], json!(message_id));
        assert_eq!(rows[0]["from_agent"], json!("alpha"));
        assert_eq!(rows[0]["subject"], json!("please ack"));
        assert_eq!(rows[0]["pending"], json!(["beta"]));
        assert!(rows[0]["age_seconds"].as_i64().unwrap() >= 0);

        // Unfiltered view shows the pending recipient list too.
        let resp = get(&app, "/mail/overdue").await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["overdue"].as_array().unwrap().len(), 1);

        // older_than beyond the message age filters it out.
        let resp = get(&app, "/mail/overdue?agent=beta&older_than=999999").await;
        let body = body_json(resp.into_body()).await;
        assert!(body["overdue"].as_array().unwrap().is_empty());

        // beta acks — the message leaves overdue.
        state
            .rails
            .mail
            .acknowledge_message("mail:general", &message_id, Some("beta"), None)
            .await
            .unwrap();
        let resp = get(&app, "/mail/overdue?agent=beta").await;
        let body = body_json(resp.into_body()).await;
        assert!(body["overdue"].as_array().unwrap().is_empty());
        let resp = get(&app, "/mail/overdue").await;
        let body = body_json(resp.into_body()).await;
        assert!(body["overdue"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Gherkin scenario "keystones over HTTP": diamond graph (A blocks B and
    /// C; B and C block D) → A top keystone, D most blocked, all metrics
    /// computed. Plus triage ranking and impact 404/400 handling.
    #[tokio::test]
    async fn graph_endpoints_over_diamond_fixture() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-graph-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let root = state.rails.root_dir.clone();

        // Seed the diamond via the ticket store + dependency events.
        let store = TicketStore::new(&root).unwrap();
        let a = store.create(new_ticket("A")).unwrap();
        let b = store.create(new_ticket("B")).unwrap();
        let c = store.create(new_ticket("C")).unwrap();
        let d = store.create(new_ticket("D")).unwrap();
        for (from, to) in [(&a, &b), (&a, &c), (&b, &d), (&c, &d)] {
            dependencies::add_edge(
                &root,
                &store,
                DependencyEdge::new(from.id.clone(), to.id.clone(), DependencyKind::Blocks),
            )
            .unwrap();
        }

        let app = factory_core_router().with_state(state.clone());

        // ── Insights ─────────────────────────────────────────────────────
        let resp = get(&app, "/graph/insights").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["keystones"][0]["ticket"], json!(a.id.to_string()));
        assert_eq!(body["keystones"][0]["impact"], json!(3));
        assert_eq!(body["health"]["most_blocked"], json!(d.id.to_string()));
        assert_eq!(body["health"]["ready_count"], json!(1));
        assert_eq!(body["metric_statuses"]["pagerank"], json!("computed"));
        assert_eq!(body["metric_statuses"]["betweenness"], json!("computed"));
        assert_eq!(body["metric_statuses"]["critical_path"], json!("computed"));
        assert!(body["content_hash"].as_str().unwrap().len() == 64);

        // ── Triage ───────────────────────────────────────────────────────
        let resp = get(&app, "/graph/triage").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["ready_count"], json!(1));
        assert_eq!(body["items"][0]["ticket"], json!(a.id.to_string()));
        assert_eq!(body["items"][0]["unblocks"], json!(3));
        assert!(body["items"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("unblocks 3 tickets"));

        // ── Impact ───────────────────────────────────────────────────────
        let resp = get(&app, &format!("/graph/impact/{}", a.id)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["critical_path_impact"], json!(3));
        assert_eq!(body["transitive_dependents"].as_array().unwrap().len(), 3);
        assert_eq!(body["direct_dependents"].as_array().unwrap().len(), 2);

        // Unknown (but well-formed) ticket id → 404.
        let unknown = format!("T-{}", "0".repeat(32));
        let resp = get(&app, &format!("/graph/impact/{unknown}")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Malformed ticket id → 400.
        let resp = get(&app, "/graph/impact/bogus").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Plan data plane round-trip: plan.new → list/show/render → refine →
    /// execute → cancel, plus the not-found/bad-mutation error paths.
    #[tokio::test]
    async fn plan_data_plane_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-plan-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Create a plan.
        let resp = post_json(
            &app,
            "/plan",
            json!({ "text": "Build the provisioning surface" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let dag_id = body["dag_id"].as_str().unwrap().to_string();
        assert!(body["prompt_id"].as_str().unwrap().starts_with("p_"));
        assert!(body["node_id"].as_str().unwrap().starts_with("n_"));

        // List plans: the new DAG shows up with metadata + version.
        let resp = get(&app, "/plans").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let dags = body["dags"].as_array().unwrap();
        let plan = dags
            .iter()
            .find(|d| d["dag_id"] == json!(dag_id))
            .expect("plan listed");
        assert!(plan["version"].as_str().unwrap().parse::<usize>().unwrap() > 0);
        assert!(plan["created_at"].as_str().unwrap().len() > 0);
        assert_eq!(
            plan["metadata"]["title"],
            json!("Build the provisioning surface")
        );

        // Show projects the DAG with its root node.
        let resp = get(&app, &format!("/plan/{dag_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["dag_id"], json!(dag_id));
        assert_eq!(body["dag"]["nodes"].as_object().unwrap().len(), 1);

        // Render: JSON and markdown formats.
        let resp = get(&app, &format!("/plan/{dag_id}/render?format=json")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["format"], json!("json"));
        assert!(body["content"].as_str().unwrap().contains(&dag_id));

        let resp = get(&app, &format!("/plan/{dag_id}/render?format=markdown")).await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["format"], json!("markdown"));
        assert!(body["content"].as_str().unwrap().starts_with("# DAG "));

        // Refine with an add mutation appends a second node.
        let resp = post_json(
            &app,
            "/plan/refine",
            json!({
                "dag_id": dag_id,
                "delta": "add a review step",
                "mutations": [
                    { "action": "add", "node_id": "n_review", "title": "Review" }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["delta_id"].as_str().unwrap().starts_with("d_"));

        // Refining an unknown DAG is a client error (no linked prompt).
        let resp = post_json(
            &app,
            "/plan/refine",
            json!({ "dag_id": "dag_missing", "delta": "x" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Unsupported mutation actions are rejected, not dropped.
        let resp = post_json(
            &app,
            "/plan/refine",
            json!({
                "dag_id": dag_id,
                "delta": "drop it",
                "mutations": [{ "action": "remove", "node_id": "n_review" }]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Execute flips the root node to RUNNING and returns a run id.
        let resp = post_json(&app, &format!("/plan/{dag_id}/execute"), json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], json!("started"));
        let run_id = body["run_id"].as_str().unwrap().to_string();

        // Executing an unknown DAG 404s.
        let resp = post_json(&app, "/plan/dag_missing/execute", json!({})).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Cancel records a RunCancelled ledger event.
        let resp = post_json(&app, &format!("/plan/runs/{run_id}/cancel"), json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["cancelled"], json!(true));
        let events = state.rails.ledger.tail(10).await.unwrap();
        assert!(events.iter().any(|e| e.r#type == "RunCancelled"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Leases data plane round-trip: request → list → renew → release, with
    /// 404s for unknown lease ids.
    #[tokio::test]
    async fn leases_data_plane_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-leases-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Request a lease (wih_id follows the dag:node convention).
        let resp = post_json(
            &app,
            "/leases",
            json!({
                "wih_id": "dag_p2:n_root",
                "agent_id": "agent-a",
                "paths": ["src/main.rs"],
                "ttl_seconds": 300
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        // List surfaces it as an active lease with derived dag/node ids.
        let resp = get(&app, "/leases").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let leases = body["leases"].as_array().unwrap();
        assert_eq!(leases.len(), 1);
        let lease = &leases[0];
        let lease_id = lease["lease_id"].as_str().unwrap().to_string();
        assert_eq!(lease["dag_id"], json!("dag_p2"));
        assert_eq!(lease["node_id"], json!("n_root"));
        assert_eq!(lease["agent_id"], json!("agent-a"));
        assert_eq!(lease["keys"], json!(["src/main.rs"]));
        assert_eq!(lease["status"], json!("active"));
        assert!(lease["acquired_at"].as_i64().unwrap() > 0);

        // Renew extends the expiry.
        let resp = post_json(
            &app,
            &format!("/leases/{lease_id}/renew"),
            json!({ "ttl_seconds": 600 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["renewed"], json!(true));
        let expires_at = body["expires_at"].as_i64().unwrap();
        assert!(expires_at > chrono::Utc::now().timestamp_millis());

        // Renew/release of an unknown lease 404s.
        let resp = post_json(
            &app,
            "/leases/lease_missing/renew",
            json!({ "ttl_seconds": 60 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let resp = delete_req(&app, "/leases/lease_missing").await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Release transitions the lease out of the active list.
        let resp = delete_req(&app, &format!("/leases/{lease_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["released"], json!(true));
        let resp = get(&app, "/leases").await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["leases"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Context packs data plane: seal persists the client inputs verbatim and
    /// list filters by dag/node/wih.
    #[tokio::test]
    async fn context_packs_data_plane_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-packs-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        let inputs = json!({
            "wih_id": "wih_1",
            "dag_id": "dag_p2",
            "node_id": "n_root",
            "receipt_refs": ["receipts/a.json"]
        });
        let resp = post_json(
            &app,
            "/context-packs/seal",
            json!({
                "dag_id": "dag_p2",
                "node_id": "n_root",
                "wih_id": "wih_1",
                "inputs": inputs
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["sealed"], json!(true));
        let pack_id = body["context_pack_id"].as_str().unwrap().to_string();
        assert!(body["stored_at"].as_str().unwrap().len() > 0);

        // The seal is durably on disk and announced in the ledger.
        let pack_file = state
            .rails
            .root_dir
            .join(".allternit/context-packs")
            .join(&pack_id)
            .join("pack.json");
        let on_disk: Value = serde_json::from_slice(&std::fs::read(&pack_file).unwrap()).unwrap();
        assert_eq!(on_disk["inputs"], inputs);
        let events = state.rails.ledger.tail(10).await.unwrap();
        assert!(events.iter().any(|e| e.r#type == "ContextPackSealed"));

        // List without filters returns the pack; filters narrow correctly.
        let resp = post_json(&app, "/context-packs", json!({})).await;
        let body = body_json(resp.into_body()).await;
        let packs = body["packs"].as_array().unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0]["inputs"], inputs);
        assert_eq!(packs[0]["version"], json!("1.0.0"));

        let resp = post_json(&app, "/context-packs", json!({ "dag_id": "dag_p2" })).await;
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["packs"].as_array().unwrap().len(), 1);

        let resp = post_json(&app, "/context-packs", json!({ "wih_id": "wih_other" })).await;
        let body = body_json(resp.into_body()).await;
        assert!(body["packs"].as_array().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Mail review/archive data plane: both routes log their event types to
    /// the ledger on the resolved thread.
    #[tokio::test]
    async fn mail_review_archive_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-mail-review-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        let resp = post_json(
            &app,
            "/mail/review",
            json!({
                "thread_id": "wih:pipeline-x",
                "wih_id": "wih_1",
                "diff_ref": "diffs/1.patch"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["requested"], json!(true));

        let resp = post_json(
            &app,
            "/mail/archive",
            json!({
                "thread_id": "wih:pipeline-x",
                "path": "archives/pipeline-x.json",
                "reason": "done"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["archive_id"].as_str().unwrap().len() > 0);

        let events = state.rails.ledger.tail(20).await.unwrap();
        assert!(events.iter().any(|e| e.r#type == "ReviewRequested"));
        assert!(events.iter().any(|e| e.r#type == "MailThreadArchived"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Gate data plane round-trip: status/rules/verify, check against a real
    /// signed WIH, decision strict-provenance enforcement, and mutate with
    /// the client's mutation dialect.
    #[tokio::test]
    async fn gate_data_plane_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-gate-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Status + rules (dev checkout ships GATE_RULES.md next to the crate).
        let resp = get(&app, "/gate/status").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["status"], json!("ok"));

        let resp = get(&app, "/gate/rules").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["rules"].as_str().unwrap().contains("Gate Rules"));

        // Create + pick up + sign a WIH so gate.check has real evidence.
        let resp = post_json(&app, "/plan", json!({ "text": "Gate me" })).await;
        let body = body_json(resp.into_body()).await;
        let dag_id = body["dag_id"].as_str().unwrap().to_string();
        let node_id = body["node_id"].as_str().unwrap().to_string();
        let wih_id = state
            .rails
            .gate
            .wih_pickup(&dag_id, &node_id, "agent-a")
            .await
            .unwrap();
        state.rails.gate.wih_sign_open(&wih_id, "sig").await.unwrap();

        let resp = post_json(
            &app,
            "/gate/check",
            json!({ "wih_id": wih_id, "tool": "bash", "paths": [] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["allowed"], json!(true));

        // Unknown WIH -> 404.
        let resp = post_json(
            &app,
            "/gate/check",
            json!({ "wih_id": "wih_missing", "tool": "bash", "paths": [] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // Verify: chain intact, no cycle DAGs.
        let resp = post_json(&app, "/gate/verify", json!({ "json": true })).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["ledger_chain_ok"], json!(true));
        assert!(body["cycle_dags"].as_array().unwrap().is_empty());

        // Decision: strict provenance rejects an unlinked decision, accepts a
        // linked one.
        let resp = post_json(
            &app,
            "/gate/decision",
            json!({ "note": "unlinked" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = post_json(
            &app,
            "/gate/decision",
            json!({ "note": "linked", "links": ["evt_1"] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["decision_id"].as_str().unwrap().starts_with("dec_"));

        // Mutate with the client dialect: add + set_status mutations apply.
        let resp = post_json(
            &app,
            "/gate/mutate",
            json!({
                "dag_id": dag_id,
                "note": "add reviewer node",
                "mutations": [
                    { "action": "add", "node_id": "n_review", "parent_id": node_id },
                    { "action": "set_status", "node_id": "n_review", "status": "READY" }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["decision_id"].as_str().unwrap().starts_with("dec_"));
        assert_eq!(body["mutation_ids"].as_array().unwrap().len(), 2);

        // Unsupported action rejected.
        let resp = post_json(
            &app,
            "/gate/mutate",
            json!({
                "dag_id": dag_id,
                "note": "drop node",
                "mutations": [{ "action": "remove", "node_id": "n_review" }]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // set_status without a node_id defaults to the DAG root (pause/resume
        // call-site shape).
        let resp = post_json(
            &app,
            "/gate/mutate",
            json!({
                "dag_id": dag_id,
                "note": "Pause execution",
                "mutations": [{ "action": "set_status", "status": "paused" }]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Index rebuild re-indexes the ledger and reports the count.
    #[tokio::test]
    async fn index_rebuild_round_trip() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-index-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Seed a few ledger events.
        post_json(&app, "/plan", json!({ "text": "Index me" })).await;
        post_json(&app, "/mail/send", json!({ "body": "hello" })).await;

        let resp = post_json(&app, "/index/rebuild", json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert!(body["indexed_count"].as_u64().unwrap() >= 3);

        // Rebuild is idempotent-count stable (clear + re-index).
        let resp = post_json(&app, "/index/rebuild", json!({})).await;
        let body = body_json(resp.into_body()).await;
        assert!(body["indexed_count"].as_u64().unwrap() >= 3);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Delete guard: a node with an ACTIVE WIH must 409 even though the node's
    /// own `current_wih_id` projection field is never populated (always None).
    /// Live-smoke regression 2026-09-13: the field-based guard let the delete
    /// through and orphaned the WIH.
    #[tokio::test]
    async fn delete_node_with_active_wih_conflicts() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-delwih-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        // Plan with a parent + child so we can also exercise the children guard.
        let resp = post_json(
            &app,
            "/plan/from-text",
            json!({
                "title": "delete guard",
                "todos": [
                    { "title": "parent", "depth": 0 },
                    { "title": "leaf", "depth": 1 }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let dag_id = body["dag_id"].as_str().unwrap().to_string();

        let resp = get(&app, "/plans/dags?view=all").await;
        let body = body_json(resp.into_body()).await;
        let dag = body["dags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["dag_id"] == json!(dag_id))
            .unwrap()
            .clone();
        let nodes: Vec<(String, String)> = dag["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                (
                    n["node_id"].as_str().unwrap().to_string(),
                    n["title"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let (parent_id, _) = nodes.iter().find(|(_, t)| t == "parent").unwrap().clone();
        let (leaf_id, _) = nodes.iter().find(|(_, t)| t == "leaf").unwrap().clone();

        // Children guard: parent has a non-DONE child.
        let resp = delete_req(&app, &format!("/plan/{dag_id}/nodes/{parent_id}")).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // Pick up the leaf; active WIH must block its deletion.
        let wih_id = state
            .rails
            .gate
            .wih_pickup(&dag_id, &leaf_id, "agent-a")
            .await
            .unwrap();
        let resp = delete_req(&app, &format!("/plan/{dag_id}/nodes/{leaf_id}")).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("active WIH"),
            "expected active-WIH conflict, got: {body}"
        );

        // Close the WIH; deletion now succeeds.
        let evidence = vec!["smoke evidence".to_string()];
        state
            .rails
            .gate
            .wih_close(&wih_id, "DONE", &evidence)
            .await
            .unwrap();
        let resp = delete_req(&app, &format!("/plan/{dag_id}/nodes/{leaf_id}")).await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// PATCH parent_node_id: reparent under another node, reparent to root
    /// (null), reparent onto an own descendant → 409 cycle, and title-only
    /// PATCH keeps the v4 rename contract.
    #[tokio::test]
    async fn patch_reparent_nodes_with_cycle_409() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-reparent-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        let resp = post_json(
            &app,
            "/plan/from-text",
            json!({
                "title": "reparent plan",
                "todos": [
                    { "title": "task A", "depth": 0 },
                    { "title": "task B", "depth": 1 },
                    { "title": "task C", "depth": 1 }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let dag_id = body["dag_id"].as_str().unwrap().to_string();

        // (node_id, parent_node_id) for a node title, via the dags view.
        async fn node_info(app: &Router, dag_id: &str, title: &str) -> (String, Value) {
            let resp = get(app, "/plans/dags?view=all").await;
            let body = body_json(resp.into_body()).await;
            let dag = body["dags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["dag_id"] == json!(dag_id))
                .unwrap()
                .clone();
            let node = dag["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["title"] == json!(title))
                .unwrap_or_else(|| panic!("node {title:?} missing from dags view"))
                .clone();
            (
                node["node_id"].as_str().unwrap().to_string(),
                node["parent_node_id"].clone(),
            )
        }

        // plan/from-text wraps todos under its own plan-root node and nests
        // them per depth; the exact initial shape is not load-bearing here.
        let (a_id, _) = node_info(&app, &dag_id, "task A").await;
        let (b_id, _) = node_info(&app, &dag_id, "task B").await;
        let (c_id, _) = node_info(&app, &dag_id, "task C").await;
        assert!(a_id != b_id && b_id != c_id && a_id != c_id);

        // Reparent C to the root (explicit null — serde double-Option).
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "parent_node_id": null }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (_, c_parent) = node_info(&app, &dag_id, "task C").await;
        assert!(c_parent.is_null());

        // Reparent C under B, then move it back to the root: assert the
        // parent actually changed via the dags view each time.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "parent_node_id": b_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (_, c_parent) = node_info(&app, &dag_id, "task C").await;
        assert_eq!(c_parent, json!(b_id));
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "parent_node_id": null }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (_, c_parent) = node_info(&app, &dag_id, "task C").await;
        assert!(c_parent.is_null());

        // Set up the cycle chain: B under C ...
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{b_id}"),
            json!({ "parent_node_id": c_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let (_, b_parent) = node_info(&app, &dag_id, "task B").await;
        assert_eq!(b_parent, json!(c_id));

        // ... then reparenting C under its own descendant must 409.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "parent_node_id": b_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert!(
            body["error"].as_str().unwrap().contains("cycle"),
            "expected cycle message, got: {body}"
        );

        // Unknown parent maps the library error to 400.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "parent_node_id": "n_bogus" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Empty PATCH is rejected.
        let resp = patch_json(&app, &format!("/plan/{dag_id}/nodes/{c_id}"), json!({})).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Title-only PATCH keeps the v4 rename contract.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{c_id}"),
            json!({ "title": "task C renamed" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["node_id"], json!(c_id));
        let (c_id2, _) = node_info(&app, &dag_id, "task C renamed").await;
        assert_eq!(c_id2, c_id);

        let _ = std::fs::remove_dir_all(&temp);
    }

    /// PATCH labels/description/priority: one call sets all three, labels
    /// replace (not merge), an empty array clears, and priority null is a
    /// no-op (clearing priority is not supported — apply_node_patch ignores
    /// null).
    #[tokio::test]
    async fn patch_node_labels_description_priority() {
        let temp = std::env::temp_dir().join(format!(
            "allternit-factory-core-node-meta-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp).unwrap();
        let state = test_app_state(&temp).await;
        let app = factory_core_router().with_state(state.clone());

        let resp = post_json(
            &app,
            "/plan/from-text",
            json!({
                "title": "meta plan",
                "todos": [
                    { "title": "task X", "depth": 0 }
                ]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let dag_id = body["dag_id"].as_str().unwrap().to_string();

        async fn node_meta(app: &Router, dag_id: &str, title: &str) -> Value {
            let resp = get(app, "/plans/dags?view=all").await;
            let body = body_json(resp.into_body()).await;
            let dag = body["dags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["dag_id"] == json!(dag_id))
                .unwrap()
                .clone();
            dag["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["title"] == json!(title))
                .unwrap_or_else(|| panic!("node {title:?} missing from dags view"))
                .clone()
        }

        let x = node_meta(&app, &dag_id, "task X").await;
        let x_id = x["node_id"].as_str().unwrap().to_string();
        assert_eq!(x["labels"], json!([]));
        assert_eq!(x["description"], Value::Null);
        assert_eq!(x["priority"], Value::Null);

        // One PATCH sets labels + description + priority together.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{x_id}"),
            json!({
                "labels": ["backend", "urgent"],
                "description": "wire the meta endpoint",
                "priority": 3
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let x = node_meta(&app, &dag_id, "task X").await;
        assert_eq!(x["labels"], json!(["backend", "urgent"]));
        assert_eq!(x["description"], json!("wire the meta endpoint"));
        assert_eq!(x["priority"], json!(3));

        // Labels replace: the old set is gone.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{x_id}"),
            json!({ "labels": ["frontend"] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let x = node_meta(&app, &dag_id, "task X").await;
        assert_eq!(x["labels"], json!(["frontend"]));
        // Untouched fields keep their values.
        assert_eq!(x["description"], json!("wire the meta endpoint"));
        assert_eq!(x["priority"], json!(3));

        // Empty array clears the label set.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{x_id}"),
            json!({ "labels": [] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let x = node_meta(&app, &dag_id, "task X").await;
        assert_eq!(x["labels"], json!([]));

        // Priority clear is not supported: null is a no-op, not a clear.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{x_id}"),
            json!({ "priority": null }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let x = node_meta(&app, &dag_id, "task X").await;
        assert_eq!(
            x["priority"],
            json!(3),
            "priority null must leave the value unchanged (no clear in v6)"
        );

        // Empty description string passes through and clears the text.
        let resp = patch_json(
            &app,
            &format!("/plan/{dag_id}/nodes/{x_id}"),
            json!({ "description": "" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let x = node_meta(&app, &dag_id, "task X").await;
        assert_eq!(x["description"], json!(""));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn old_prefix_aliases_serve_the_old_paths_beside_the_factory_routes() {
        // The same three mounts as main.rs: a duplicate or conflicting route
        // panics here instead of crash-looping the server at boot.
        let temp = std::env::temp_dir().join(format!("old-prefix-aliases-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let app = Router::new()
            .nest("/api/factory", factory_core_router())
            .nest("/api/rails", legacy_alias_router())
            .nest("/api/commrails", legacy_alias_router())
            .with_state(test_app_state(&temp).await);
        // What deployed callers send (allternit-ai use-rails-dags, gizzi <= 2.1.9).
        for (path, want) in [
            ("/api/commrails/dags?view=mine", StatusCode::OK),
            ("/api/rails/health", StatusCode::OK),
            ("/api/rails/mail/threads", StatusCode::OK),
            ("/api/factory/plans/dags?view=mine", StatusCode::OK),
            // The old subpaths exist only under the old prefixes.
            ("/api/factory/dags", StatusCode::NOT_FOUND),
        ] {
            let res = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(res.status(), want, "{path}");
        }
        let _ = std::fs::remove_dir_all(&temp);
    }
}
