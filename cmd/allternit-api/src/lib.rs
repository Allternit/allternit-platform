//! Allternit API Library
//!
//! Shared state and route handlers for the Allternit API.

pub mod aci_approvals;
pub mod aci_batch;
#[cfg(test)]
mod aci_batch_adversarial;
pub mod aci_code;
pub mod aci_credentials;
pub mod aci_routes;
pub mod aci_safety;
pub mod admin_audit_routes;
pub mod admin_mcp_tunnel_routes;
pub mod admin_access_token_routes;
pub mod admin_service_account_routes;
pub mod fabric;
pub mod federation_routes;
pub mod outcome_rubric_routes;
pub mod page_agent_routes;
pub mod quickstart_routes;
pub mod admin_rate_limit_routes;
pub mod admin_spend_limit_routes;
pub mod admin_workspace_routes;
pub mod agent_execution;
pub mod agent_operations_routes;
pub mod agent_email_routes;
pub mod agent_preferences_routes;
pub mod agent_cloud_routes;
pub mod agent_routes;
pub mod agent_runtime_routes;
pub mod agent_session_routes;
pub mod agent_workspace_paths;
pub mod agent_workspace_routes;
pub mod agents_v1_routes;
pub mod alabs_routes;
pub mod allternit_vault;
pub mod analytics_routes;
pub mod artifact_routes;
pub mod audit_log_routes;
pub mod auth;
pub mod auth_dp_jwt;
pub mod benchmark_routes;
pub mod automation_routes;
pub mod backend_install_routes;
pub mod bb;
pub mod beta_deployment_routes;
pub mod beta_memory_store_routes;
pub mod beta_session_routes;
pub mod beta_work_routes;
pub mod bot_assets;
pub mod bot_desktop_audit;
pub mod bot_desktop_billing;
pub mod bot_desktop_capacity;
pub mod bot_desktop_input;
pub mod bot_desktop_mesh;
pub mod bot_desktop_mux;
pub mod bot_desktop_quotas;
pub mod bot_desktop_queue;
pub mod bot_desktop_routes;
pub mod bot_desktop_snapshots;
pub mod bot_desktop_admin;
pub mod bot_desktop_stream;
pub mod bot_event_routes;
pub mod browser_history_service;
pub mod procedural_memory_service;
pub mod bot_desktop_templates;
pub mod bot_desktop_windows;
pub mod user_profile_routes;
pub mod billing;
pub mod board_routes;
pub mod board_stream_routes;
pub mod brain_routes;
pub mod canvas_routes;
pub mod chat_routes;
pub mod checkpoints_routes;
pub mod cli_provider_detector;
pub mod cloud_credentials_routes;
pub mod cloud_agents_routes;
pub mod compliance_routes;
pub mod computer_control;
pub mod computer_routes;
pub mod computer_groups;
pub mod computer_idle;
pub mod computer_screens;
pub mod computer_audit;
pub mod computer_ws;
pub mod vnc_auth;
pub mod vnc_readonly;
pub mod wallet;
pub mod computer_embed;
pub mod desktop_template_build;
pub mod template_catalog;
pub mod bot_group_routes;
pub mod data_residency_routes;
pub mod device_attestation_routes;
pub mod config;
pub mod connector_routes;
pub mod control_plane;
pub mod content_artifact_file_routes;
pub mod content_artifact_publish;
pub mod content_artifact_relay;
pub mod content_artifact_routes;
pub mod console_announcement_routes;
pub mod conversation_routes;
pub mod al_persona_routes;
pub mod deliverable_routes;
pub mod routine_routes;
pub mod continuation;
pub mod cloud_worker;
pub mod cors;
pub mod credits;
pub mod cowork;
pub mod cowork_preferences_routes;
pub mod cowork_routes;
pub mod cowork_team_routes;
pub mod cron_lite;
pub mod db;
pub mod deployment_scheduler;
pub mod desktop_host_registry;
pub mod desktop_host_provisioner;
pub mod desktop_host_admin;
pub mod fabric_admin_routes;
pub mod fabric_credits_routes;
pub mod fabric_model_routes;
pub mod fabric_node_routes;
pub mod fabric_resources_routes;
pub mod fabric_usage_routes;
pub mod design_connector_routes;
pub mod env_allowlist;
pub mod error;
pub mod enterprise_auth;
pub mod eval_metric_routes;
pub mod eval_metrics;
pub mod eval_routes;
pub mod external_keys_routes;
pub mod fabric_routes;
pub mod fallback_credit_routes;
pub mod fallback_retry_policy_routes;
pub mod fallback_routes;
pub mod groundedness_check_routes;
pub mod latency_budget_routes;
pub mod prompt_leak_routes;
pub mod file_routes;
pub mod gizzi_chat_stream;
pub mod gizzi_completion;
pub mod gizzi_provider_auth;
pub mod h5i_routes;
pub mod har_api_routes;
pub mod har_api_service;
pub mod inference_router_routes;
pub mod inference_router_executor;
pub mod health;
pub mod hud_routes;
pub mod idempotency;
pub mod inbox_routes;
pub mod internal_auth;
pub mod internal_routes;
pub mod library_routes;
pub mod llm_gateway;
pub mod local_brain_routes;
pub mod local_engine_routes;
pub mod long_running_task_routes;
pub mod local_studio_routes;
pub mod mcp_dispatcher;
pub mod mcp_routes;
pub mod mcp_server_routes;
pub mod mcp_tunnel_auth;
pub mod marketplace_routes;
pub mod me_routes;
pub mod mailflare_client;
pub mod media;
pub mod model_training_routes;
pub mod photon_routes;
pub mod memory_notes_routes;
pub mod memory_reconstruction_routes;
pub mod memory_routes;
pub mod memory_kernel_service;
pub mod metrics;
pub mod monitor_routes;
pub mod oauth_routes;
pub mod passkey_routes;
pub mod office_cli_mcp;
pub mod office_cli_routes;
pub mod office_engine_routes;
pub mod office_routes;
pub mod onboarding_routes;
pub mod open_connector_proxy;
pub mod otel;
pub mod orchestrator_routes;
pub mod permission_policy;
pub mod policy_audit;
pub mod policy_config;
pub mod allternit_bus_routes;
pub mod platform_static;
pub mod playground_routes;
pub mod pricing;
pub mod provider_routes;
pub mod queue_routes;
pub mod rails;
pub mod remote_control_routes;
pub mod rate_limit;
pub mod rails_client_impl;
pub mod research_task_service;
pub mod research_task_routes;
pub mod rbac;
pub mod rbac_routes;
pub mod runtime_backend_routes;
pub mod runtime_discover_routes;
pub mod runtime_settings_routes;
pub mod sandbox_routes;
pub mod sandbox_template_routes;
pub mod scim_routes;
pub mod skills_routes;
pub mod server_tool_routes;
pub mod session_memory_service;
pub mod slack_webhook_routes;
pub mod ssh_key_routes;
pub mod ssh_routes;
pub mod status_routes;
pub mod stream;
pub mod swarm_routes;
pub mod tag_routes;
pub mod task_routes;
pub mod team_skill_routes;
// Unix-only: talks to allternit-mux over a UDS (tokio::net::UnixStream).
#[cfg(unix)]
pub mod terminal_routes;
pub mod token_crypto;
pub mod tool_routes;
pub mod udemy_routes;
pub mod upload_routes;
pub mod usage_routes;
pub mod v1_routes;
pub mod viz_routes;
pub mod vm_pool;
pub mod vm_session_routes;
pub mod web_proxy_routes;
pub mod webhook_routes;
pub mod webhook_subscription_routes;
pub mod webhook_trigger_routes;
pub mod workflow_routes;
pub mod workspace_routes;
pub mod remote_peers;
pub mod group_rooms;

use allternit_cowork_runtime::RunManager;
use allternit_cowork_scheduler::Scheduler;
use auth::{AuthConfig, JwksManager};
use config::AppConfig;
use cowork::background_service::BackgroundServiceHandle;
use db::DbHandle;
use design_connector_routes::DesignSkillCache;
use rails::RailsState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(unix)]
use terminal_routes::TerminalSessionStore;
use tokio::sync::RwLock;
use vm_session_routes::VmSessionStore;

// Unit tests (`cfg(test)`) and integration tests in `tests/` (which build the
// crate without `cfg(test)` but with debug assertions) both use this factory.
#[cfg(any(test, debug_assertions))]
pub mod test_helpers {
    //! Minimal `AppState` factory for unit tests that need the full struct.
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    pub async fn app_state(temp: &Path) -> Arc<AppState> {
        app_state_with_driver(temp, None).await
    }

    pub async fn app_state_with_driver(
        temp: &Path,
        vm_driver: Option<Arc<dyn allternit_driver_interface::ExecutionDriver>>,
    ) -> Arc<AppState> {
        app_state_with_driver_and_os(temp, vm_driver, None).await
    }

    pub async fn app_state_with_driver_and_os(
        temp: &Path,
        vm_driver: Option<Arc<dyn allternit_driver_interface::ExecutionDriver>>,
        os_control_plane: Option<crate::fabric::os_client::OsControlPlaneClient>,
    ) -> Arc<AppState> {
        let config = AppConfig {
            company: config::CompanyConfig::default(),
            user: config::UserConfig::default(),
        };
        app_state_with_config_and_os(temp, config, vm_driver, os_control_plane).await
    }

    /// Like `app_state`, with an explicit config — for tests that exercise
    /// config-gated behavior (e.g. the credits-purchase honesty gate).
    pub async fn app_state_with_config(
        temp: &Path,
        config: AppConfig,
    ) -> Arc<AppState> {
        app_state_with_config_and_os(temp, config, None, None).await
    }

    /// Serialize tests that mutate the process-wide
    /// `ALLTERNIT_COMPUTER_USE_DIR` env var. The audit/grant/run-buffer paths
    /// read that var at call time, so a concurrent `set_var` from another
    /// test redirects rows/files between temp dirs — the root cause of the
    /// `audit_api_returns_rows_with_bot_filter` flake (1-in-N under default
    /// test threading). Same mutex as `policy_config::POLICY_TEST_LOCK`
    /// (policy-seat and policy-audit tests already serialize on it); tests
    /// holding that lock must NOT take this guard again — std `Mutex` is not
    /// reentrant and the same-thread second lock deadlocks.
    #[cfg(test)]
    pub fn computer_use_dir_test_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::policy_config::POLICY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    async fn app_state_with_config_and_os(
        temp: &Path,
        config: AppConfig,
        vm_driver: Option<Arc<dyn allternit_driver_interface::ExecutionDriver>>,
        os_control_plane: Option<crate::fabric::os_client::OsControlPlaneClient>,
    ) -> Arc<AppState> {
        let db = db::DbHandle::new(temp.join("test.db")).expect("test db");
        let auth_config = auth::AuthConfig::from_app_config(&config);
        let jwks = auth::JwksManager::new(&auth_config);
        let rails = RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        let resource_class_catalog = crate::fabric::sku::ResourceClassCatalog::from_db(&db)
            .expect("initialize resource class catalog");
        let fabric_node_pool = std::sync::Arc::new(
            allternit_computer_cloud::providers::fabric_node::FabricNodePool::new(),
        );
        let fabric_node_provider =
            allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                fabric_node_pool,
                "__system".to_string(),
            );
        let fabric_provider_registry = crate::fabric::build_provider_registry(fabric_node_provider.clone());
        let fabric_price_cache = crate::fabric::PriceCache::new(db.clone());
        let fabric_scheduler = crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine())
            .with_price_cache(fabric_price_cache.clone());
        Arc::new(AppState {
            config,
            db,
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(RwLock::new(HashMap::new())),
            computer_guest_tokens: Arc::new(RwLock::new(HashMap::new())),
            rails,
            vm_sessions: vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(RwLock::new(office_routes::OfficeRuntimeFile::default())),
            office_cli_docs: Arc::new(RwLock::new(HashMap::new())),
            office_cli_watches: Arc::new(RwLock::new(HashMap::new())),
            office_cli_mcp_sessions: Arc::new(RwLock::new(HashMap::new())),
            design_skill_cache: DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(permission_policy::ApprovalStore::new()),
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
}

/// Globally accessible application configuration, initialized once at startup.
/// Routes and helpers that do not receive `AppState` can read from here so the
/// entire crate uses the same layered config.
pub static APP_CONFIG: once_cell::sync::OnceCell<AppConfig> = once_cell::sync::OnceCell::new();

/// Initialize the global configuration. Must be called exactly once, from
/// `main`, before any route handler runs.
pub fn init_app_config() -> &'static AppConfig {
    APP_CONFIG.get_or_init(AppConfig::load)
}

/// Office runtime state (bindings + sessions) — kept in memory for concurrency safety
pub type OfficeRuntimeState = Arc<RwLock<crate::office_routes::OfficeRuntimeFile>>;

/// OfficeCLI document registry — in-memory mirror of `<office_cli_dir>/docs.json`.
pub type OfficeCliDocsState =
    Arc<RwLock<std::collections::HashMap<uuid::Uuid, crate::office_cli_routes::OfficeCliDoc>>>;

/// Live `officecli watch` child processes keyed by doc_id (not serialized).
pub type OfficeCliWatchState =
    Arc<RwLock<std::collections::HashMap<uuid::Uuid, tokio::process::Child>>>;

/// OfficeCLI MCP stdio sessions, one per user_id.
pub type OfficeCliMcpState =
    Arc<RwLock<std::collections::HashMap<String, crate::office_cli_mcp::McpSession>>>;

/// Runtime state for a bot's virtual-computer desktop session.
#[derive(Debug, Clone)]
pub struct BotDesktopSession {
    pub bot_id: String,
    pub sandbox_id: String,
    pub control_state: BotDesktopControlState,
    pub taken_over_by_user_id: Option<String>,
    pub taken_over_at: Option<chrono::DateTime<chrono::Utc>>,
    pub handed_back_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotDesktopControlState {
    BotControls,
    HumanControls,
    HumanObserving,
}

/// Application state shared across all route handlers
pub struct AppState {
    /// Unified app configuration (company + user + env overrides)
    pub config: AppConfig,
    /// SQLite database handle
    pub db: DbHandle,
    /// Local data directory for on-disk file storage.
    pub data_dir: PathBuf,
    /// Clerk JWKS manager for JWT verification
    pub jwks: JwksManager,
    /// Unified auth configuration
    pub auth_config: AuthConfig,
    /// Data-plane (cloud-api → node) JWT verifier: JWKS cache + EdDSA
    /// verification of tokens minted by allternit-cloud-api.
    pub dp_jwks: crate::auth_dp_jwt::DataPlaneJwks,
    /// VM execution driver (Incus/Tart Computer Cloud; Firecracker on Linux hosts)
    pub vm_driver: Option<Arc<dyn allternit_driver_interface::ExecutionDriver>>,
    /// Concrete Incus driver when one is configured; used to add/remove cloud
    /// provisioned hosts at runtime.
    pub incus_driver: Option<Arc<allternit_computer_cloud::IncusDriver>>,
    /// Registry for cloud-provisioned Desktop Cloud Incus hosts.
    pub desktop_host_registry: crate::desktop_host_registry::DesktopHostRegistry,
    /// Provisioner / autoscaler for desktop hosts.
    pub desktop_host_provisioner: Option<crate::desktop_host_provisioner::DesktopHostProvisioner>,
    /// Bot desktop take-over state: bot_id -> session metadata + control state.
    pub bot_desktop_sessions: Arc<RwLock<HashMap<String, BotDesktopSession>>>,
    /// Phase 3 guest bridge tokens: "{computer_id}:{pty|events}" -> token.
    /// In-memory only; a stale bridge is re-bootstrapped on connection
    /// refusal.
    pub computer_guest_tokens: Arc<RwLock<HashMap<String, String>>>,

    /// Rails service state (Ledger, Gate, Leases, etc.)
    pub rails: RailsState,
    /// Persistent VM sessions — each gizzi-code session gets one VM that stays
    /// alive for the entire session lifetime (not torn down between exec calls).
    pub vm_sessions: VmSessionStore,
    /// Cowork cron scheduler — runs enabled tasks on their configured intervals.
    pub cowork_scheduler: Option<Arc<RwLock<Scheduler>>>,
    /// Cowork background service — periodic autonomous loop for proactive suggestions.
    pub cowork_background: Option<BackgroundServiceHandle>,
    /// Cowork runtime run manager — persistent, detachable run lifecycle.
    pub cowork_run_manager: Option<Arc<RunManager>>,
    /// Webhook secret for verifying incoming webhooks
    pub webhook_secret: Option<String>,
    /// Office add-in runtime bindings and sessions
    pub office_runtime: OfficeRuntimeState,
    /// OfficeCLI document registry (snapshot docs uploaded by the add-in)
    pub office_cli_docs: OfficeCliDocsState,
    /// Live `officecli watch` preview processes keyed by doc_id
    pub office_cli_watches: OfficeCliWatchState,
    /// OfficeCLI MCP stdio sessions, one per user
    pub office_cli_mcp_sessions: OfficeCliMcpState,
    /// Daemon-side Open Design skill cache with hot-reload semantics.
    pub design_skill_cache: DesignSkillCache,
    /// Local tmux-backed terminal sessions for Code Mode. Unix-only (mux UDS).
    #[cfg(unix)]
    pub terminal_sessions: TerminalSessionStore,
    /// Attached MCP servers reachable through the server-side MCP dispatcher.
    pub mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher,
    /// Pending/resolved tool-execution approval requests from `ask` policy decisions.
    pub approval_store: Arc<crate::permission_policy::ApprovalStore>,
    /// Passkey / WebAuthn state for the vault.
    pub passkey_state: Option<crate::passkey_routes::PasskeyState>,
    /// Fabric SKU / capability-class catalog, loaded from DB at startup.
    pub resource_class_catalog: crate::fabric::sku::ResourceClassCatalog,
    /// Private Fabric node provider pool; refreshed from `FabricNodeRegistry`.
    pub fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider,
    /// Fabric provider registry exposed to the scheduler. Includes live
    /// providers (Runpod, Vast.ai) when configured and the Private Fabric node
    /// provider.
    pub fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry,
    /// Fabric scheduler with cache-first offer selection and credit holds.
    pub fabric_scheduler: crate::fabric::Scheduler,
    /// Fabric provider price cache; refreshed by a background worker.
    pub fabric_price_cache: crate::fabric::PriceCache,
    /// Optional canonical AllternitOS control-plane client. When set, Fabric
    /// resource creation is routed through the OS `POST /v1/leases/issue`
    /// endpoint instead of the internal Cloud scheduler.
    pub os_control_plane: Option<crate::fabric::os_client::OsControlPlaneClient>,
    /// Deployment scheduler counters (last tick, total runs fired) surfaced
    /// in `GET /monitor/system`.
    pub deployment_scheduler: Arc<crate::deployment_scheduler::DeploymentSchedulerState>,
}

/// Return the default LLM provider/model pair used when a request does not
/// specify one. Reads from the unified app config (file + env overrides).
/// Returns empty strings when nothing is configured; callers fall back to the
/// local Ollama brain rather than a hardcoded provider.
pub fn default_model() -> (String, String) {
    APP_CONFIG
        .get()
        .map(|c| c.default_model())
        .unwrap_or_else(|| (String::new(), String::new()))
}
