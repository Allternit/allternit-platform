//! Allternit Cloud API
//!
//! Backend API for cloud deployment management.
//! Provides REST endpoints and WebSocket event streaming.

pub mod auth;
pub mod db;
pub mod error;
pub mod middleware;
pub mod model_router;
pub mod routes;
pub mod runtime;
pub mod services;
pub mod validation;
pub mod websocket;

pub use middleware::rate_limit::{create_rate_limiter, RateLimitConfig, RateLimiter};
pub use validation::{validate_email, validate_run_name, validate_uuid, Validatable};

use axum::{
    extract::DefaultBodyLimit,
    http::{header, Method},
};
use axum::{
    middleware as axum_middleware,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;

pub use db::models::*;
pub use error::ApiError;
pub use websocket::DeploymentEvent;

/// Origins allowed to call production cloud-api from a browser.
/// Includes the packaged Desktop UI (`:8013`) — without these, Chromium
/// reports CORS on `GET /v1/models` from `http://127.0.0.1:8013`.
pub const DEFAULT_CORS_ORIGINS: &[&str] = &[
    "http://localhost:3013",
    "http://127.0.0.1:3013",
    "http://localhost:8013",
    "http://127.0.0.1:8013",
    "https://platform.allternit.com",
    "https://ai.allternit.com",
    // Allternit Mobile (phone PWA), formerly fabrictransport.allternit.com.
    "https://m.allternit.com",
];

/// API application state
pub struct ApiState {
    pub db: sqlx::PgPool,
    pub ssh_executor: allternit_cloud_ssh::SshExecutor,
    pub event_tx: broadcast::Sender<DeploymentEvent>,
    /// Shared event store for all operations
    pub event_store: Arc<dyn services::EventStore>,
    /// Shared run service for run lifecycle management
    pub run_service: Arc<dyn services::RunService>,
    /// Shared session manager for multi-client attachments
    pub session_manager: Arc<runtime::session_manager::SessionManager>,
    /// Rate limiter for API protection
    pub rate_limiter: Arc<RateLimiter>,
    /// Public-route rate limiter for pairing and relay endpoints
    pub public_rate_limiter: Arc<RateLimiter>,
    /// Tight per-user limiter for the free inference path (no credits row)
    pub free_inference_rate_limiter: Arc<RateLimiter>,
    /// Cost service for tracking run costs
    pub cost_service: Arc<dyn services::CostService>,
    /// Quota service for free-tier guardrails
    pub quota_service: services::SharedQuotaService,
    /// Contabo runtime service for hosted runtimes (provisions and manages
    /// workload containers on the Contabo VPS)
    pub contabo_runtime_service: Arc<services::ContaboRuntimeService>,
    /// Gateway used by the P1 control-plane namespaces (agent-sessions,
    /// office, beta; routes::data_plane): resolves the caller's default
    /// data-plane node and relays through the runtime relay machinery.
    /// Behind a trait so handler tests can substitute a mock at the service
    /// boundary.
    pub data_plane_gateway: Arc<dyn routes::data_plane::DataPlaneGateway>,
    /// P2 per-subscription provisioning lane (Incus fleet): fleet
    /// scheduling, per-instance lifecycle (create/start/stop/status/delete),
    /// pairing bind, and run-interval metering.
    pub provisioning_service: Arc<services::ProvisioningService>,
    /// Mesh enrollment service (Headscale), absent when HEADSCALE_API_KEY is unset
    pub mesh_service: Option<Arc<routes::mesh::MeshService>>,
    /// AES-256-GCM cipher for provider tokens and wizard checkpoints,
    /// absent (plaintext-at-rest, dev only) when ALLTERNIT_CREDENTIALS_KEY is unset
    pub credential_cipher: Option<Arc<allternit_cloud_core::CredentialCipher>>,
    /// BYOK inference key store, absent when the credential cipher is absent
    pub inference_key_service: Option<Arc<services::InferenceKeyService>>,
    /// Shared metrics state for request tracking
    pub metrics_state: Arc<crate::middleware::metrics::MetricsState>,
    /// Model router for dispatching inference requests to upstream providers
    pub model_router: crate::model_router::ModelRouter,
    /// Inference pool budgets/circuit breaker for upstream provider spend
    pub inference_pool_service: Arc<services::InferencePoolService>,
}

/// Create the API router
pub fn create_router(state: Arc<ApiState>) -> Router {
    // Create protected routes with auth middleware
    let protected_routes = Router::new()
        // Run endpoints (Cowork Runtime)
        .route("/api/v1/runs", post(routes::runs::create_run))
        .route("/api/v1/runs", get(routes::runs::list_runs))
        .route("/api/v1/runs/:id", get(routes::runs::get_run))
        .route(
            "/api/v1/runs/:id",
            axum::routing::put(routes::runs::update_run),
        )
        .route(
            "/api/v1/runs/:id",
            axum::routing::delete(routes::runs::delete_run),
        )
        .route("/api/v1/runs/:id/start", post(routes::runs::start_run))
        .route("/api/v1/runs/:id/pause", post(routes::runs::pause_run))
        .route("/api/v1/runs/:id/resume", post(routes::runs::resume_run))
        .route("/api/v1/runs/:id/cancel", post(routes::runs::cancel_run))
        .route("/api/v1/runs/:id/attach", post(routes::runs::attach_to_run))
        .route(
            "/api/v1/runs/:id/detach",
            post(routes::runs::detach_from_run),
        )
        .route(
            "/api/v1/runs/:id/attachments",
            get(routes::runs::get_run_attachments),
        )
        .route("/api/v1/runs/:id/events", get(routes::runs::get_run_events))
        .route(
            "/api/v1/runs/:id/events/stream",
            get(routes::runs::run_events_sse),
        )
        // Checkpoint endpoints
        .route(
            "/api/v1/runs/:id/checkpoints",
            post(routes::runs::create_checkpoint),
        )
        .route(
            "/api/v1/runs/:id/checkpoints",
            get(routes::runs::list_checkpoints),
        )
        .route("/api/v1/checkpoints/:id", get(routes::runs::get_checkpoint))
        .route(
            "/api/v1/checkpoints/:id",
            axum::routing::delete(routes::runs::delete_checkpoint),
        )
        .route(
            "/api/v1/runs/:id/restore",
            post(routes::runs::restore_checkpoint),
        )
        .route("/api/v1/runs/:id/recover", post(routes::runs::recover_run))
        .route(
            "/api/v1/runs/:run_id/handoffs",
            get(routes::runs::list_handoffs),
        )
        .route(
            "/api/v1/runs/:run_id/handoffs",
            post(routes::runs::create_handoff),
        )
        // Job endpoints
        .route("/api/v1/runs/:run_id/jobs", post(routes::jobs::create_job))
        .route("/api/v1/runs/:run_id/jobs", get(routes::jobs::list_jobs))
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id",
            get(routes::jobs::get_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id",
            axum::routing::put(routes::jobs::update_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id",
            axum::routing::delete(routes::jobs::delete_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id/start",
            post(routes::jobs::start_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id/complete",
            post(routes::jobs::complete_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id/fail",
            post(routes::jobs::fail_job),
        )
        .route(
            "/api/v1/runs/:run_id/jobs/:job_id/cancel",
            post(routes::jobs::cancel_job),
        )
        // Schedule endpoints
        .route(
            "/api/v1/schedules",
            post(routes::schedules::create_schedule),
        )
        .route("/api/v1/schedules", get(routes::schedules::list_schedules))
        .route(
            "/api/v1/schedules/:id",
            get(routes::schedules::get_schedule),
        )
        .route(
            "/api/v1/schedules/:id",
            axum::routing::put(routes::schedules::update_schedule),
        )
        .route(
            "/api/v1/schedules/:id",
            axum::routing::delete(routes::schedules::delete_schedule),
        )
        .route(
            "/api/v1/schedules/:id/enable",
            post(routes::schedules::enable_schedule),
        )
        .route(
            "/api/v1/schedules/:id/disable",
            post(routes::schedules::disable_schedule),
        )
        .route(
            "/api/v1/schedules/:id/trigger",
            post(routes::schedules::trigger_schedule),
        )
        // Approval endpoints
        .route(
            "/api/v1/approvals",
            post(routes::approvals::create_approval),
        )
        .route("/api/v1/approvals", get(routes::approvals::list_approvals))
        .route(
            "/api/v1/approvals/:id",
            get(routes::approvals::get_approval),
        )
        .route(
            "/api/v1/approvals/:id/approve",
            post(routes::approvals::approve_request),
        )
        .route(
            "/api/v1/approvals/:id/deny",
            post(routes::approvals::deny_request),
        )
        .route(
            "/api/v1/approvals/:id/cancel",
            post(routes::approvals::cancel_request),
        )
        // Task endpoints
        .merge(routes::tasks::task_routes())
        // Mirror session endpoints (merged from api/cloud/allternit-cloud-api)
        .merge(routes::mirror::create_mirror_routes())
        // Deployment endpoints (existing)
        .route(
            "/api/v1/deployments",
            post(routes::deployments::create_deployment),
        )
        .route(
            "/api/v1/deployments",
            get(routes::deployments::list_deployments),
        )
        .route(
            "/api/v1/deployments/:id",
            get(routes::deployments::get_deployment),
        )
        .route(
            "/api/v1/deployments/:id/cancel",
            delete(routes::deployments::cancel_deployment),
        )
        // WebSocket endpoint for live events
        .route("/api/v1/deployments/:id/events", get(websocket::ws_handler))
        // Provider endpoints
        .route("/api/v1/providers", get(routes::providers::list_providers))
        .route(
            "/api/v1/providers/:id/validate",
            post(routes::providers::validate_credentials),
        )
        // Model router chat completions (auth-protected)
        .route(
            "/v1/chat/completions",
            post(routes::model_router::chat_completions),
        )
        // Region endpoints
        .route("/api/v1/regions", get(routes::regions::list_regions))
        .route("/api/v1/regions/:id", get(routes::regions::get_region))
        .route(
            "/api/v1/regions/:id/capacity",
            get(routes::regions::get_region_capacity),
        )
        // Instance endpoints
        .route("/api/v1/instances", get(routes::instances::list_instances))
        .route(
            "/api/v1/instances/:id",
            get(routes::instances::get_instance),
        )
        .route(
            "/api/v1/instances/:id/restart",
            post(routes::instances::restart_instance),
        )
        .route(
            "/api/v1/instances/:id/destroy",
            delete(routes::instances::destroy_instance),
        )
        // Cost tracking endpoints
        .route("/api/v1/runs/:id/cost", get(routes::costs::get_run_cost))
        .route(
            "/api/v1/costs/summary",
            get(routes::costs::get_cost_summary),
        )
        .route(
            "/api/v1/costs/breakdown",
            get(routes::costs::get_cost_breakdown),
        )
        .route("/api/v1/costs/budget", get(routes::costs::get_budget))
        .route(
            "/api/v1/costs/budget",
            axum::routing::put(routes::costs::update_budget),
        )
        .route("/api/v1/costs/rates", get(routes::costs::list_cost_rates))
        .route("/api/v1/costs/rates", post(routes::costs::set_cost_rate))
        .route(
            "/api/v1/costs/recalculate",
            post(routes::costs::recalculate_costs),
        )
        .route(
            "/api/v1/costs/reset",
            post(routes::costs::reset_monthly_costs),
        )
        .layer(axum_middleware::from_fn_with_state(
            state.rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            auth::middleware::auth_middleware,
        ))
        .with_state(state.clone());

    // Create public routes (no auth required)
    let public_runtime_routes = Router::new()
        // Runtime pairing performs its own Clerk or device-credential checks on
        // each endpoint. Pairing creation/exchange intentionally remain public
        // like an OAuth device authorization endpoint.
        .merge(routes::runtime_pairing::routes())
        .merge(routes::runtime_relay::routes())
        // These handlers verify Clerk or billing credentials themselves. They
        // must not pass through the legacy allternit_* API-token middleware.
        .merge(routes::hosted_runtimes::routes())
        .merge(routes::contabo_hosted_runtimes::routes())
        .merge(routes::hosted_entitlements::routes())
        // Server-to-server wallet transfers (org fabric ledger bridge) are
        // gated by the billing sync secret, like the entitlement sync routes.
        .merge(routes::billing_transfer::routes())
        // Credit balance verifies the Clerk session per-request, like the
        // hosted runtime routes.
        .merge(routes::billing_credits::routes())
        // Desktop device tokens and Clerk sessions both read Allternit
        // subscription remaining compute here (plan + credits).
        .merge(routes::me_usage::routes())
        // The pack catalog is public; checkout creation verifies the Clerk
        // session per-request and answers 503 billing_not_configured when
        // STRIPE_SECRET_KEY is unset.
        .merge(routes::billing_checkout::routes())
        // The plan catalog is public; subscribe/portal verify the Clerk session
        // per-request and answer 503 billing_not_configured when STRIPE_SECRET_KEY
        // or the plan's STRIPE_PRICE_* id is unset.
        .merge(routes::billing_subscriptions::routes())
        // BYOK inference keys verify the Clerk session per-request and answer
        // 503 inference_keys_not_configured when ALLTERNIT_CREDENTIALS_KEY is unset.
        .merge(routes::inference_keys::routes())
        // API keys verify the Clerk session per-request and store only hashes.
        .merge(routes::api_keys::routes())
        // The Stripe webhook verifies the Stripe-Signature HMAC itself and
        // answers 503 webhook_not_configured when STRIPE_WEBHOOK_SECRET is unset.
        .merge(routes::billing_webhooks::routes())
        .merge(routes::clerk_webhooks::routes())
        // Dispatch handoff verifies the Clerk session per-request, like the
        // pairing routes (the token only ever means "this user, that
        // runtime" — it is not itself a credential).
        .merge(routes::dispatch_handoff::routes())
        // Gizzi instances are self-registered over a per-request Clerk
        // session, like the pairing routes — no allternit_* API token.
        .merge(routes::gizzi_instances::routes())
        // Agent-sessions control-plane namespace (P1): Clerk session verified
        // per-request inside each handler (resolve_user_scoped), then proxied
        // to the caller's default data-plane node via the runtime relay.
        .merge(routes::agent_sessions::routes())
        .merge(routes::continuation::routes())
        // Office namespace (P1): bindings/bootstrap/runtime-state, relayed to
        // the caller's default node. Office state is in-memory per :8013
        // process, so these handlers are node-affine by design (§3.3).
        .merge(routes::office::routes())
        // Beta namespace (P1): research tasks + playground sessions, relayed
        // to the caller's default node. The WS event stream
        // (/beta/sessions/:id/events/ws) is intentionally not exposed — it
        // needs the socket-ticket WS relay, not the request relay.
        .merge(routes::beta::routes())
        // Canvas namespace (P1, tranche 2): user-wide list plus per-canvas
        // get/patch/delete, relayed to the caller's default node. The data
        // plane owns no streaming canvas variant — plain JSON CRUD only.
        .merge(routes::canvases::routes())
        // P2 per-subscription provisioning lane: owner-scoped instance
        // lifecycle (subscription-gated create, start/stop/status/delete,
        // usage) plus admin-only fleet host management. Control plane only —
        // the provisioned container is reached via the runtime relay like
        // any registered node.
        .merge(routes::provisioned_instances::routes())
        // Data-plane JWT public key (decision A1): nodes fetch cloud-api's
        // Ed25519 verifying key at startup. Public, fail-closed 503 when
        // ALLTERNIT_DP_JWT_SEED is unset.
        .route(
            "/api/v1/auth/dp-jwks",
            get(auth::dataplane_jwt::dp_jwks),
        )
        // Mesh enrollment verifies the Clerk session per-request and answers
        // 503 mesh_not_configured when HEADSCALE_API_KEY is unset.
        .merge(routes::mesh::routes())
        .merge(routes::paired_computers::routes())
        // BYO-VPS deploy wizard: Clerk session verified per-request (injected
        // as the wizard's AuthenticatedUser), checkpoints in wizard_sessions.
        .merge(routes::wizard::routes(&state))
        // Provider token management: Clerk session per-request, tokens
        // encrypted at rest, never echoed back.
        .merge(routes::providers::clerk_routes())
        // Agency API (WP11) + public receipt JWKS, forwarded to the node on
        // this host. Clerk session or allternit_* token verified per-request
        // before anything reaches the node (see routes::agency_forward).
        .merge(routes::agency_forward::routes(Arc::new(
            routes::agency_forward::AgencyForward::from_env(),
        )))
        .layer(axum_middleware::from_fn_with_state(
            state.public_rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        .with_state(state.clone());

    let public_health_routes = Router::new()
        // Health endpoints (public). `/health` (no /api/v1 prefix) is the
        // conventional load-balancer/UptimeRobot probe path — serve the same
        // public health handler there so monitors don't get 401.
        .route("/health", get(routes::health::health_check))
        .route("/api/v1/health", get(routes::health::health_check))
        .route("/api/v1/health/ready", get(routes::health::readiness_check))
        .route("/api/v1/health/live", get(routes::health::liveness_check))
        .route("/api/v1/metrics", get(routes::health::metrics))
        .with_state(state.clone());

    let public_model_routes = Router::new()
        // Public model catalog
        .route("/v1/models", get(routes::model_router::list_models))
        .with_state(state.clone());

    // Run-event WebSocket: authenticates inside the handler — the legacy
    // auth_middleware only understands API tokens and would reject browser
    // clients holding a Clerk session JWT before the upgrade is decided.
    let run_ws_routes = Router::new()
        .route("/ws/runs/:id", get(websocket::run_ws_handler))
        .layer(axum_middleware::from_fn_with_state(
            state.public_rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        .with_state(state.clone());

    // Create auth-protected routes (require auth but listed separately for clarity)
    let auth_routes = Router::new()
        .route("/api/v1/auth/me", get(routes::auth::get_current_user))
        .route("/api/v1/auth/tokens", get(routes::auth::list_tokens))
        .route("/api/v1/auth/tokens", post(routes::auth::create_token))
        .route(
            "/api/v1/auth/tokens/:id",
            delete(routes::auth::revoke_token),
        )
        .layer(axum_middleware::from_fn_with_state(
            state.rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            auth::middleware::auth_middleware,
        ))
        .with_state(state.clone());

    // Configure CORS
    let cors = if std::env::var("Allternit_API_DEVELOPMENT_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
    {
        // Development: Allow all origins
        CorsLayer::permissive()
    } else {
        // Production: Restrictive CORS
        let mut origin_list = DEFAULT_CORS_ORIGINS
            .iter()
            .map(|origin| origin.to_string())
            .collect::<Vec<_>>();
        if let Ok(extra) = std::env::var("CORS_ALLOWED_ORIGINS") {
            for origin in extra.split(',') {
                let origin = origin.trim();
                if !origin.is_empty() && !origin_list.iter().any(|existing| existing == origin) {
                    origin_list.push(origin.to_string());
                }
            }
        }
        let allowed_origins: Vec<_> = origin_list
            .iter()
            .filter_map(|s| s.parse::<axum::http::HeaderValue>().ok())
            .collect();

        CorsLayer::new()
            .allow_origin(allowed_origins)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::PATCH,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::CONTENT_TYPE,
                header::AUTHORIZATION,
                header::ACCEPT,
                header::ORIGIN,
                header::HeaderName::from_static("x-requested-with"),
                header::HeaderName::from_static("x-client-version"),
                header::HeaderName::from_static("x-allternit-tenant-id"),
                // D16: the person's Clerk session on human-act calls, relayed
                // to their runtime (see runtime_relay::filtered_headers).
                header::HeaderName::from_static("x-allternit-human-proof"),
                // Subscriptions: the one-use human action a task send carries.
                header::HeaderName::from_static("x-allternit-human-action"),
                // Agency API: idempotent creates and SSE resume.
                header::HeaderName::from_static("idempotency-key"),
                header::HeaderName::from_static("last-event-id"),
            ])
            .allow_credentials(true)
            .max_age(std::time::Duration::from_secs(3600))
    };

    // Get max body size from env (default 10MB)
    let max_body_size = std::env::var("MAX_BODY_SIZE_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(10 * 1024 * 1024); // 10MB default

    // Combine all routes
    Router::new()
        .merge(public_health_routes)
        .merge(run_ws_routes)
        .merge(public_model_routes)
        .merge(public_runtime_routes)
        .merge(auth_routes)
        .merge(protected_routes)
        // Everything else under /api/v1 that lives on the user's runtime
        // (subscriptions, agent gateway, computers, …) is relayed to their
        // default data-plane node, so the web works like the Desktop app.
        .fallback(routes::data_plane_fallback::fallback)
        .layer(DefaultBodyLimit::max(max_body_size))
        .layer(cors)
        .layer(axum::middleware::from_fn_with_state(
            state.metrics_state.clone(),
            crate::middleware::metrics::metrics_middleware,
        ))
        .with_state(state)
}

/// Initialize the database with configured connection pooling.
///
/// The API targets PostgreSQL in production. After connecting, the embedded
/// `migrations_pg` directory is applied with `sqlx::migrate!` — every file is
/// written to be idempotent (IF NOT EXISTS / guarded DO blocks) so the first
/// run against the already-migrated production database converges instead of
/// failing. Set `ALLTERNIT_SKIP_MIGRATIONS=1` to opt out (escape hatch for
/// deploys where migrations are applied manually).
///
/// Note: `db::migrations` (the earlier opt-in ordered runner) is superseded
/// by this path — its `MIGRATIONS` list excludes 001 and predates the
/// idempotent rewrite of the whole directory.
pub async fn init_db(database_url: &str) -> Result<sqlx::PgPool, ApiError> {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;

    // Get pool configuration from environment
    let max_connections = std::env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let min_connections = std::env::var("DB_MIN_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let acquire_timeout_secs = std::env::var("DB_ACQUIRE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let max_lifetime_mins = std::env::var("DB_MAX_LIFETIME_MINS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let idle_timeout_mins = std::env::var("DB_IDLE_TIMEOUT_MINS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    tracing::info!(
        "Database pool config: max={}, min={}, acquire_timeout={}s, max_lifetime={}m, idle_timeout={}m",
        max_connections, min_connections, acquire_timeout_secs, max_lifetime_mins, idle_timeout_mins
    );

    let connect_options = PgConnectOptions::from_str(database_url)?;

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .acquire_timeout(std::time::Duration::from_secs(acquire_timeout_secs))
        .max_lifetime(std::time::Duration::from_secs(max_lifetime_mins * 60))
        .idle_timeout(std::time::Duration::from_secs(idle_timeout_mins * 60))
        .connect_with(connect_options)
        .await?;

    if std::env::var("ALLTERNIT_SKIP_MIGRATIONS")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        tracing::warn!("ALLTERNIT_SKIP_MIGRATIONS set — skipping embedded Postgres migrations");
    } else {
        // Path is relative to the crate root (CARGO_MANIFEST_DIR).
        sqlx::migrate!("./migrations_pg")
            .run(&pool)
            .await
            .map_err(|e| ApiError::Internal(format!("Postgres migration failed: {e}")))?;
        tracing::info!("Postgres migrations applied (migrations_pg)");
    }

    tracing::info!(
        "Database pool initialized with {} max connections",
        max_connections
    );
    Ok(pool)
}

/// Start the API server with graceful shutdown
pub async fn start_server(state: Arc<ApiState>, addr: &str) -> Result<(), ApiError> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("API server listening on {}", addr);

    let router = create_router(state);

    // Setup graceful shutdown
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        // Wait for shutdown signal
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to create SIGTERM handler");
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to create SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => tracing::info!("Received SIGTERM, shutting down gracefully..."),
            _ = sigint.recv() => tracing::info!("Received SIGINT, shutting down gracefully..."),
        }

        let _ = shutdown_tx.send(());
    });

    // Start server with graceful shutdown
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await?;

    tracing::info!("Server shutdown complete");
    Ok(())
}

#[cfg(test)]
mod cors_origin_tests {
    use super::DEFAULT_CORS_ORIGINS;

    #[test]
    fn desktop_loopback_ui_is_an_allowed_origin() {
        assert!(DEFAULT_CORS_ORIGINS.contains(&"http://127.0.0.1:8013"));
        assert!(DEFAULT_CORS_ORIGINS.contains(&"http://localhost:8013"));
    }
}

#[cfg(test)]
mod migration_tests {
    /// Applies the embedded `migrations_pg` directory (the same
    /// `sqlx::migrate!("./migrations_pg")` call `init_db` makes) against the
    /// local PG test database. The migrations are idempotent, so this passes
    /// on an empty database, on a fully-migrated one, and on re-runs — and it
    /// proves the macro resolves the directory relative to the crate root.
    #[tokio::test]
    async fn migrations_pg_applies_cleanly() {
        let pool = sqlx::PgPool::connect(
            "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test",
        )
        .await
        .expect("connect to the local PG test database (allternit_test)");
        sqlx::migrate!("./migrations_pg")
            .run(&pool)
            .await
            .expect("embedded migrations_pg must apply cleanly and idempotently");
    }
}
