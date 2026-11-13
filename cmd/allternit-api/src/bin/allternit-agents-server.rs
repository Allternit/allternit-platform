//! `allternit-agents-server`: the customer-run Agents Server.
//!
//! Mounts only the selfhost router (`allternit_api::selfhost`). It never
//! builds the hosted `AppState`, never warms Clerk JWKS, and never inits
//! Stripe or the cloud API client. Logs go to stderr as JSON (or text, per
//! `observability.log_format`).
//!
//! ```text
//! allternit-agents-server [serve] [--config <path>]
//! allternit-agents-server admin <command>     (see `admin help`)
//! allternit-agents-server --version | --help
//! ```

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use allternit_api::selfhost::egress_guard::EgressGuard;
use allternit_api::selfhost::settings::{config_path, LogFormat, Settings};
use allternit_api::selfhost::store::pg::PgStore;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::DynStore;
use allternit_api::selfhost::{router, SelfHostState};
use tracing::{error, info, warn};

const USAGE: &str = "\
allternit-agents-server: the self-hosted Allternit Agents Server

USAGE:
    allternit-agents-server [serve] [--config <path>]
    allternit-agents-server admin <setup|migrate|keys|env-keys|rotate-key|doctor> ...
    allternit-agents-server --version
    allternit-agents-server --help

The settings file (YAML or JSON) comes from --config or ALLTERNIT_SELFHOST_CONFIG.
Schema: openapi/selfhost-settings.schema.json.
";

enum Command {
    Serve(Vec<String>),
    Admin(Vec<String>),
    Help,
    Version,
}

fn parse(args: Vec<String>) -> Result<Command, String> {
    match args.first().map(String::as_str) {
        None => Ok(Command::Serve(args)),
        Some("-h" | "--help" | "help") => Ok(Command::Help),
        Some("-V" | "--version" | "version") => Ok(Command::Version),
        Some("serve") => Ok(Command::Serve(args[1..].to_vec())),
        Some("admin") => Ok(Command::Admin(args[1..].to_vec())),
        Some(a) if a.starts_with('-') => Ok(Command::Serve(args)),
        Some(other) => Err(format!("unknown command {other:?}")),
    }
}

fn init_logging(format: LogFormat) {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let otel = allternit_api::otel::layer_from_env_with_default(
        allternit_api::selfhost::telemetry::DEFAULT_SERVICE_NAME,
    );
    let registry = tracing_subscriber::registry().with(filter).with(otel);
    match format {
        LogFormat::Json => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(std::io::stderr),
            )
            .init(),
        LogFormat::Text => registry
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init(),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(args) {
        Ok(Command::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("allternit-agents-server {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Admin(rest)) => {
            ExitCode::from(allternit_api::selfhost::admin::run(rest).await)
        }
        Ok(Command::Serve(rest)) => match serve(rest).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(msg) => {
                // Logging may not be up yet; stderr either way.
                eprintln!("allternit-agents-server: {msg}");
                ExitCode::from(1)
            }
        },
        Err(msg) => {
            eprintln!("allternit-agents-server: {msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn open_store(settings: &Settings) -> Result<DynStore, String> {
    if let Some(url) = settings.database_url_resolved().await.map_err(|e| e.to_string())? {
        let pg = PgStore::connect(&url, settings.database.max_connections)
            .await
            .map_err(|e| format!("cannot connect to PostgreSQL: {e}"))?;
        pg.migrate()
            .await
            .map_err(|e| format!("cannot migrate PostgreSQL: {e}"))?;
        info!(store = "postgres", "store ready");
        return Ok(Arc::new(pg));
    }
    if settings.ephemeral {
        warn!(store = "memory", "ephemeral mode: all state is lost when the server stops");
        return Ok(Arc::new(SqliteStore::open_memory().map_err(|e| e.to_string())?));
    }
    let path = settings.data_dir.join("agents-server.db");
    let store = SqliteStore::open(&path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    info!(store = "sqlite", path = %path.display(), "store ready");
    Ok(Arc::new(store))
}

async fn serve(args: Vec<String>) -> Result<(), String> {
    let path = config_path(&args).ok_or_else(|| {
        allternit_api::selfhost::settings::SettingsError::NoConfig.to_string()
    })?;
    let settings = Settings::load(&path).map_err(|e| e.to_string())?;
    init_logging(settings.observability.log_format);
    info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %path.display(),
        provider = settings.inference.provider.as_str(),
        "starting allternit-agents-server"
    );
    info!(
        hosts = ?EgressGuard::hosts_for(&settings),
        "egress allowlist (every other outbound host is refused)"
    );

    let store = open_store(&settings).await?;
    let listen = settings.server.listen.clone();
    let state = SelfHostState::new(store, settings).map_err(|e| e.to_string())?;
    if state.setup_token.is_some() {
        info!("setup token set: POST /v1/api_keys can mint the first admin key while none exists");
    }
    allternit_api::selfhost::metrics_listener::spawn(state.clone()).await?;
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    info!(addr = %local, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| {
        error!(error = %e, "server stopped with an error");
        e.to_string()
    })?;
    info!("stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    info!("shutdown signal received");
}
