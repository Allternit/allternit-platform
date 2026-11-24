//! Allternit Node Daemon — always-on `node.core` presence without the
//! desktop app. The daemon adopts the paired runtime's identity and holds a
//! relay connection with a reduced, capability-gated surface so Fabric
//! Transport can use a paired machine (terminal, files, exec, metrics, and
//! launching the desktop app) regardless of whether Allternit Desktop runs.

use std::path::PathBuf;
use std::sync::Arc;

use allternit_node_daemon::handlers::DaemonState;
use allternit_node_daemon::{config, identity, relay, service};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "allternit-node", about = "Allternit Node Daemon — always-on node.core presence")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon in the foreground (the service entry point).
    Run,
    /// Install the LaunchDaemon (macOS) / systemd unit (Linux).
    Install,
    /// Remove the service. The desktop app is never touched; the shared
    /// pairing identity is kept unless --purge is passed.
    Uninstall {
        /// Also remove the shared pairing identity and node config. This
        /// unpairs Allternit Desktop too.
        #[arg(long)]
        purge: bool,
    },
    /// Print service status.
    Status,
    /// Show recent daemon logs.
    Logs,
}

fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home)
        .join(".config")
        .join("allternit")
        .join("node-config.json")
}

/// SIGTERM (launchd shutdown) + SIGINT (Ctrl-C): kill and reap every tracked
/// PTY terminal child, then exit 0. The signal task owns the whole sequence, so a
/// dying daemon never leaves PTY children behind as zombies. Pane engine
/// terminals are the engine's: they keep running, and the next daemon lists
/// and re-attaches them.
#[cfg(unix)]
fn install_shutdown_handler(state: Arc<DaemonState>) {
    use tokio::signal::unix::{signal, SignalKind};

    tokio::spawn(async move {
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "SIGTERM handler unavailable; shutdown cleanup disabled");
                return;
            }
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!(%error, "SIGINT handler unavailable; shutdown cleanup disabled");
                return;
            }
        };
        tokio::select! {
            _ = terminate.recv() => {},
            _ = interrupt.recv() => {},
        }
        tracing::info!("shutdown signal received; killing PTY terminal sessions");
        state.terminals.shutdown_all();
        std::process::exit(0);
    });
}

#[cfg(not(unix))]
fn install_shutdown_handler(_state: Arc<DaemonState>) {
    // Non-unix platforms: no signal-based shutdown path yet.
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Run => {
            let config = config::NodeConfig::load()?;
            let identity = identity::RuntimeIdentity::load(&config.identity_path)?;
            let state = DaemonState::new(config.clone());
            tracing::info!(
                runtime_id = %identity.runtime_id,
                cloud = %config.cloud_api_url,
                "allternit-node {} starting (node.core: health, exec, fs, processes, metrics, launch, terminal)",
                env!("CARGO_PKG_VERSION"),
            );
            install_shutdown_handler(state.clone());
            // Which backend serves terminals (logged); requests probe again
            // while no pane engine has answered.
            state.engine.get().await;
            relay::run(config, identity, state).await;
            Ok(())
        }
        Command::Install => {
            let config = config::NodeConfig::load()?;
            let binary = std::env::current_exe()?;
            match service::install(&binary, &config.identity_path) {
                Ok(message) => {
                    println!("{message}");
                    Ok(())
                }
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
            }
        }
        Command::Uninstall { purge } => {
            let config = config::NodeConfig::load().unwrap_or_default();
            match service::uninstall(&config.identity_path, &default_config_path(), purge) {
                Ok(message) => {
                    println!("{message}");
                    Ok(())
                }
                Err(message) => {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
            }
        }
        Command::Status => {
            let config = config::NodeConfig::load().unwrap_or_default();
            println!("{}", service::status(&config.identity_path));
            Ok(())
        }
        Command::Logs => {
            let config = config::NodeConfig::load().unwrap_or_default();
            println!("{}", service::logs(&config.identity_path));
            Ok(())
        }
    }
}
