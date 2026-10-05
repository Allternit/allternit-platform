//! Allternit Cloud API Server
//!
//! Main entry point for the cloud deployment API.

use allternit_cloud_api::{init_db, model_router, routes, runtime, services, ApiState};
use std::sync::Arc;
use tokio::sync::broadcast;

fn init_logging() {
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "pretty".to_string());
    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let env_filter = tracing_subscriber::EnvFilter::new(log_level);

    if log_format == "json" {
        // JSON format for production
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .with_target(true)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_line_number(true)
            .with_file(true)
            .init();
    } else {
        // Pretty format for development
        tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(env_filter)
            .init();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    init_logging();

    // Get configuration from environment
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://allternit-cloud.db".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3001".to_string());

    // Validate environment in production
    let is_production = !std::env::var("Allternit_API_DEVELOPMENT_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if is_production {
        // In production, fail fast if required env vars are not set
        if std::env::var("DATABASE_URL").is_err() {
            tracing::error!("DATABASE_URL environment variable must be set in production");
            std::process::exit(1);
        }
        if std::env::var("HETZNER_API_TOKEN").is_err() {
            tracing::warn!("HETZNER_API_TOKEN not set - cloud runs will not be available");
        }
    }

    tracing::info!("Starting Allternit Cloud API server");
    tracing::info!("Database: {}", database_url);
    tracing::info!("Bind address: {}", bind_addr);

    if allternit_cloud_api::auth::dev_token::dev_token_allowed() {
        tracing::warn!(
            "ALLTERNIT_ALLOW_DEV_TOKEN is enabled - the hardcoded 'dev-api-token' bearer \
             backdoor is ACTIVE (audit finding B1). Never enable this in production."
        );
    }

    // Initialize database
    tracing::info!("Initializing database...");
    let db = init_db(&database_url).await?;
    tracing::info!("Database initialized");

    // Create broadcast channel for deployment events
    let (event_tx, _event_rx) = broadcast::channel::<allternit_cloud_api::DeploymentEvent>(100);

    // Create shared services
    tracing::info!("Initializing shared services...");

    // Shared event store
    let event_store: Arc<dyn services::EventStore> =
        Arc::new(services::EventStoreImpl::new(db.clone()));

    // Shared session manager
    let session_manager = Arc::new(runtime::session_manager::SessionManager::new(db.clone()));

    // Shared run service
    let run_service: Arc<dyn services::RunService> =
        Arc::new(services::RunServiceImpl::new(db.clone()).with_event_store(event_store.clone()));

    // Shared cost service
    let cost_service: Arc<dyn services::CostService> =
        Arc::new(services::CostServiceImpl::new(db.clone()));

    tracing::info!("Shared services initialized");

    // Create rate limiter
    let rate_limit_config = allternit_cloud_api::RateLimitConfig {
        requests_per_minute: std::env::var("RATE_LIMIT_RPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60),
        window: std::time::Duration::from_secs(60),
    };
    tracing::info!(
        "Rate limiter initialized: {} req/min",
        rate_limit_config.requests_per_minute
    );
    let rate_limiter = allternit_cloud_api::create_rate_limiter(rate_limit_config);

    // Create stricter public-route rate limiter for pairing and relay endpoints.
    let public_rate_limit_config = allternit_cloud_api::RateLimitConfig {
        requests_per_minute: std::env::var("PUBLIC_RATE_LIMIT_RPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
        window: std::time::Duration::from_secs(60),
    };
    tracing::info!(
        "Public rate limiter initialized: {} req/min",
        public_rate_limit_config.requests_per_minute
    );
    let public_rate_limiter = allternit_cloud_api::create_rate_limiter(public_rate_limit_config);

    // Tight per-user limiter for the free inference path (users without a
    // credits row). Paying users never touch it.
    let free_inference_rate_limit_config = allternit_cloud_api::RateLimitConfig {
        requests_per_minute: std::env::var("FREE_INFERENCE_RATE_LIMIT_RPM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
        window: std::time::Duration::from_secs(60),
    };
    tracing::info!(
        "Free inference rate limiter initialized: {} req/min",
        free_inference_rate_limit_config.requests_per_minute
    );
    let free_inference_rate_limiter =
        allternit_cloud_api::create_rate_limiter(free_inference_rate_limit_config);

    // Create quota service for free-tier guardrails.
    let quota_service = Arc::new(services::QuotaService::new(db.clone()));
    tracing::info!("Quota service initialized");

    // Contabo runtime service for hosted runtimes: provisions and manages
    // workload containers on the Contabo VPS.
    let contabo_runtime_service = Arc::new(services::ContaboRuntimeService::new(
        db.clone(),
        std::env::var("HEADSCALE_API_KEY").ok(),
        std::env::var("ALLTERNIT_CLOUD_API_URL")
            .unwrap_or_else(|_| "https://api.allternit.com".to_string()),
    ));
    tracing::info!("Contabo runtime service initialized");

    // Create mesh enrollment service if a Headscale API key is available.
    let mesh_service = routes::mesh::MeshService::from_env();

    // Credential cipher for provider tokens and wizard checkpoints. In
    // production the key is required; in dev, secrets are stored plaintext
    // with a loud warning.
    let credential_cipher =
        allternit_cloud_core::CredentialCipher::from_env().map(std::sync::Arc::new);
    match (&credential_cipher, is_production) {
        (None, true) => {
            tracing::error!(
                "ALLTERNIT_CREDENTIALS_KEY must be set in production - provider tokens and wizard checkpoints require encryption at rest"
            );
            std::process::exit(1);
        }
        (None, false) => {
            tracing::warn!(
                "ALLTERNIT_CREDENTIALS_KEY unset - provider tokens and wizard checkpoints stored PLAINTEXT (dev only)"
            );
        }
        _ => tracing::info!("Credential cipher initialized"),
    }

    // Build the model router.
    let alias_map = model_router::catalog::starter_catalog();
    let mut providers: Vec<Arc<dyn model_router::UpstreamProvider>> = Vec::new();

    if let Some(config) = model_router::openrouter::OpenRouterConfig::from_env() {
        tracing::info!("OpenRouter model router enabled");
        providers.push(model_router::openrouter::OpenRouterProvider::new(config));
    }

    // Generic OpenAI-compatible providers. Each is enabled by setting
    // <PREFIX>_API_KEY. Base URLs default to the provider's known endpoint.
    let generic_providers: &[(&str, Option<&str>)] = &[
        ("TOGETHER", Some("https://api.together.xyz/v1")),
        ("FIREWORKS", Some("https://api.fireworks.ai/inference/v1")),
        ("DEEPINFRA", Some("https://api.deepinfra.com/v1/openai")),
        ("GROQ", Some("https://api.groq.com/openai/v1")),
    ];

    for (prefix, default_base) in generic_providers {
        if let Some(config) =
            model_router::generic_openai::GenericOpenAiConfig::from_env_with_default_base(
                prefix, *default_base,
            )
        {
            tracing::info!(provider = %config.provider_id, "OpenAI-compatible model provider enabled");
            providers.push(model_router::generic_openai::GenericOpenAiProvider::new(config));
        }
    }

    let model_router = if providers.is_empty() {
        if is_production {
            tracing::warn!(
                "No model provider API keys set - model router disabled in production"
            );
        } else {
            tracing::info!(
                "No model provider API keys set - model router disabled (set OPENROUTER_API_KEY, TOGETHER_API_KEY, etc. to enable /v1/chat/completions)"
            );
        }
        model_router::ModelRouter::disabled(alias_map)
    } else {
        model_router::ModelRouter::new(providers, alias_map)
    };

    // Create API state with shared services
    let inference_pool_service = Arc::new(services::InferencePoolService::new(db.clone()));
    // BYOK key store rides the same credential cipher as provider tokens.
    let inference_key_service = credential_cipher
        .clone()
        .map(|cipher| Arc::new(services::InferenceKeyService::new(db.clone(), cipher)));
    if inference_key_service.is_some() {
        tracing::info!("BYOK inference key store enabled");
    }
    // P1 control-plane namespaces (agent-sessions/office/beta): resolves the
    // caller's default data-plane node and relays through the runtime relay
    // machinery (routes::data_plane).
    // P2 per-subscription provisioning lane (Incus fleet): create/start/
    // stop/status/delete over provisioned_hosts + provisioned_instances.
    let provisioning_service = Arc::new(services::ProvisioningService::new(db.clone()));
    let data_plane_gateway = Arc::new(routes::data_plane::PgDataPlaneGateway::new(
        db.clone(),
        contabo_runtime_service.clone(),
        quota_service.clone(),
        provisioning_service.clone(),
    ));
    let state = Arc::new(ApiState {
        db,
        ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
        event_tx,
        event_store,
        run_service,
        session_manager,
        rate_limiter,
        public_rate_limiter,
        free_inference_rate_limiter,
        cost_service,
        quota_service,
        contabo_runtime_service,
        data_plane_gateway,
        provisioning_service,
        mesh_service,
        credential_cipher,
        inference_key_service,
        metrics_state: Arc::new(allternit_cloud_api::middleware::metrics::MetricsState::new()),
        model_router,
        inference_pool_service: inference_pool_service.clone(),
    });

    // Seed one inference pool per configured provider (idempotent; operator
    // budgets in the DB are never overwritten). Missing table (migrations not
    // applied yet) must not block startup — the breaker degrades to no-pool.
    match inference_pool_service.ensure_seeded().await {
        Ok(seeded) => tracing::info!("Inference pools seeded: {}", seeded.join(", ")),
        Err(error) => tracing::warn!("Inference pool seeding skipped: {}", error),
    }

    services::start_hosted_runtime_lifecycle_task(state.clone());
    // P2: keep provisioned instance statuses honest against the Incus hosts
    // and converge their metering sessions.
    services::start_provisioning_reconcile_task(state.clone());
    services::start_waiting_computer_task(state.clone());
    routes::admin_customers::start_daily_customer_summary_task(state.clone());
    // Cancel lifecycle: delete suspended computers 30 days after cancel,
    // expire their snapshot images after 6 months.
    services::start_provisioning_lifecycle_task(state.clone());
    // Free computers (decision 16): sleep when idle, wake for scheduled jobs,
    // delete when abandoned. Acts on free rows only.
    services::start_free_computer_task(state.clone());

    // Start scheduler service (background task)
    let scheduler_enabled = std::env::var("SCHEDULER_ENABLED")
        .map(|v| v.parse::<bool>().unwrap_or(true))
        .unwrap_or(true);

    if scheduler_enabled {
        let scheduler_config = services::SchedulerConfig::from_env();

        tracing::info!("Starting scheduler service...");
        services::start_scheduler_service(state.clone(), scheduler_config);
    } else {
        tracing::info!("Scheduler service disabled");
    }

    // Start cost tracking background task
    let cost_tracking_enabled = std::env::var("COST_TRACKING_ENABLED")
        .map(|v| v.parse::<bool>().unwrap_or(true))
        .unwrap_or(true);

    if cost_tracking_enabled {
        let db_clone = state.db.clone();
        tokio::spawn(async move {
            tracing::info!("Starting cost tracking background task");
            services::start_cost_tracking_task(db_clone).await;
        });
    } else {
        tracing::info!("Cost tracking disabled");
    }

    // Channels hybrid relay: deliver queued inbound channel requests to runtimes.
    allternit_cloud_api::routes::channel_inbound::start_channel_inbound_worker(state.clone());
    // Discord shared app gateway (a no-op until the ALLTERNIT_DISCORD_* env is set).
    allternit_cloud_api::routes::discord_app::start_discord_gateway(state.clone());

    // Telegram manager bot: point it at /channels/telegram-manager/<secret>
    // (no-op when ALLTERNIT_TELEGRAM_MANAGER_BOT_TOKEN is unset).
    allternit_cloud_api::routes::channel_onboarding::start_telegram_manager_onboarding();

    // Platform API: signed webhook delivery + monthly number usage (only when switched on).
    if allternit_cloud_api::routes::platform_v1::platform_api_enabled() {
        allternit_cloud_api::routes::platform_v1::events::spawn_worker(state.db.clone());
    }

    // 10DLC / toll-free registrations: retry pending ones (missed webhooks, campaigns waiting on our carrier account).
    allternit_cloud_api::routes::phone::start_registration_sweep(state.db.clone());

    // Voice calls: deliver queued call.* events to runtimes (wake on demand).
    allternit_cloud_api::routes::voice_calls_cloud::start_voice_calls_worker(state.clone());
    allternit_cloud_api::routes::inapp_calls::start_inapp_calls_worker(state.clone());
    // Teams shared app: deliver queued Bot Framework activities to runtimes.
    allternit_cloud_api::channels::teams_app::start_teams_app_worker(state.clone());
    // Slack shared app: deliver queued Slack events to the installing users' runtimes.
    allternit_cloud_api::routes::slack_app::start_slack_event_worker(state.clone());

    // Start stale gizzi-instance garbage collection (startup sweep + hourly)
    allternit_cloud_api::routes::gizzi_instances::start_gizzi_instance_gc_task(state.db.clone());

    // Start executor service (background task): claims queued runs and
    // dispatches them to online registered gizzi instances.
    let executor_enabled = std::env::var("EXECUTOR_ENABLED")
        .map(|v| v.parse::<bool>().unwrap_or(true))
        .unwrap_or(true);

    if executor_enabled {
        let executor_config = services::ExecutorConfig::from_env();
        tracing::info!("Starting executor service...");
        services::start_executor_service(
            executor_config,
            services::ExecutorDeps {
                db: state.db.clone(),
                event_store: state.event_store.clone(),
                run_service: state.run_service.clone(),
            },
        );
    } else {
        tracing::info!("Executor service disabled");
    }

    // Start server
    tracing::info!("Starting API server on {}", bind_addr);
    allternit_cloud_api::start_server(state, &bind_addr).await?;

    Ok(())
}
