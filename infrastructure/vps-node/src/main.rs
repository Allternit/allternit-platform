//! Allternit Node Agent
//!
//! Runs on user's VPS or local machine.
//! Connects to Allternit Control Plane via WebSocket reverse tunnel.
//! Executes jobs, manages containers, provides terminal access.

use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn, error};

mod config;
mod container_pty;
mod docker;
mod executor;
mod pty;
mod run_manager;
mod websocket;

use config::NodeConfig;
use container_pty::ContainerPtyManager;
use docker::{DockerRuntime, detect_docker_capabilities};
use executor::JobExecutor;
use pty::PtyManager;
use run_manager::RunManager;
use websocket::{WebSocketClient, WebSocketConfig};

/// Allternit Node Agent
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to config file
    #[arg(short, long, default_value = "/etc/allternit/node.toml")]
    config: String,
    
    /// Node ID (overrides config)
    #[arg(long)]
    node_id: Option<String>,
    
    /// Auth token (overrides config)
    #[arg(long)]
    token: Option<String>,
    
    /// Control plane URL (overrides config)
    #[arg(long)]
    control_plane: Option<String>,
    
    /// Run once and exit (don't daemonize)
    #[arg(long)]
    once: bool,
}

/// Node state shared across tasks
pub struct NodeState {
    /// Node configuration
    pub config: RwLock<NodeConfig>,
    /// Current connection status
    pub connected: RwLock<bool>,
    /// Running jobs
    pub running_jobs: RwLock<Vec<String>>,
    /// Docker available
    pub docker_available: RwLock<bool>,
    /// PTY manager for native terminal sessions
    pub pty_manager: RwLock<Option<Arc<PtyManager>>>,
    /// Container PTY manager for sandboxed terminal sessions
    pub container_pty_manager: RwLock<Option<Arc<ContainerPtyManager>>>,
    /// Job executor for running containerized jobs
    pub executor: RwLock<Option<Arc<JobExecutor>>>,
    /// Run manager for run lifecycle
    pub run_manager: RwLock<Option<Arc<RunManager>>>,
}

impl NodeState {
    pub fn new(config: NodeConfig) -> Self {
        Self {
            config: RwLock::new(config),
            connected: RwLock::new(false),
            running_jobs: RwLock::new(Vec::new()),
            docker_available: RwLock::new(false),
            pty_manager: RwLock::new(Some(Arc::new(PtyManager::new()))),
            container_pty_manager: RwLock::new(Some(Arc::new(ContainerPtyManager::new()))),
            executor: RwLock::new(None),
            run_manager: RwLock::new(None),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info,allternit_node=debug")
        .with_target(false)
        .init();

    let args = Args::parse();

    info!("Allternit Node Agent v{}", env!("CARGO_PKG_VERSION"));
    
    // Load configuration
    let mut config = NodeConfig::load(&args.config).await?;
    
    // Allow CLI overrides
    if let Some(node_id) = args.node_id {
        config.node_id = node_id;
    }
    if let Some(token) = args.token {
        config.auth_token = token;
    }
    if let Some(cp) = args.control_plane {
        config.control_plane_url = cp;
    }

    // Validate configuration
    config.validate()?;

    info!("Node ID: {}", config.node_id);
    info!("Control Plane: {}", config.control_plane_url);

    // Create shared state
    let state = Arc::new(NodeState::new(config.clone()));

    // Initialize Docker runtime
    let docker = Arc::new(RwLock::new(DockerRuntime::new()));
    
    // Try to connect to Docker
    match docker.write().await.connect().await {
        Ok(()) => {
            *state.docker_available.write().await = true;
            let caps = detect_docker_capabilities().await;
            info!("Docker available: version={}, api={}", caps.version, caps.api_version);
        }
        Err(e) => {
            warn!("Docker not available: {}", e);
            *state.docker_available.write().await = false;
        }
    }

    // Initialize job executor
    let executor = Arc::new(JobExecutor::new(docker.clone(), config.max_concurrent_jobs));
    
    // Store executor in state
    *state.executor.write().await = Some(executor.clone());

    // Initialize PTY manager
    let pty_manager = Arc::new(PtyManager::new());

    // Start node
    let node = AllternitNode::new(config, state.clone(), docker, executor, pty_manager);
    
    if args.once {
        // Run once (for testing)
        node.run_once().await?;
    } else {
        // Run forever (normal operation)
        node.run().await?;
    }

    Ok(())
}

/// Main node struct
pub struct AllternitNode {
    config: NodeConfig,
    state: Arc<NodeState>,
    docker: Arc<RwLock<DockerRuntime>>,
    executor: Arc<JobExecutor>,
    pty_manager: Arc<PtyManager>,
}

impl AllternitNode {
    pub fn new(
        config: NodeConfig,
        state: Arc<NodeState>,
        docker: Arc<RwLock<DockerRuntime>>,
        executor: Arc<JobExecutor>,
        pty_manager: Arc<PtyManager>,
    ) -> Self {
        Self {
            config,
            state,
            docker,
            executor,
            pty_manager,
        }
    }

    /// Run the node forever (with reconnection)
    pub async fn run(self) -> Result<()> {
        info!("Starting Allternit Node...");

        // Channel for shutdown signals
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Handle Ctrl+C
        let shutdown_tx_clone = shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutdown signal received");
            let _ = shutdown_tx_clone.send(()).await;
        });

        // Main connection loop
        let mut reconnect_delay = std::time::Duration::from_secs(1);
        const MAX_RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Shutting down gracefully");
                    break;
                }
                
                result = self.connect_and_run() => {
                    match result {
                        Ok(()) => {
                            // Clean disconnect (shouldn't happen in normal operation)
                            warn!("Connection closed unexpectedly");
                        }
                        Err(e) => {
                            error!("Connection error: {}", e);
                        }
                    }
                    
                    // Wait before reconnecting
                    warn!("Reconnecting in {:?}...", reconnect_delay);
                    tokio::time::sleep(reconnect_delay).await;
                    
                    // Exponential backoff with jitter
                    reconnect_delay = std::cmp::min(
                        reconnect_delay * 2,
                        MAX_RECONNECT_DELAY
                    );
                    // Add jitter (+/-20%)
                    let jitter = rand::random::<f64>() * 0.4 + 0.8;
                    reconnect_delay = std::time::Duration::from_millis(
                        (reconnect_delay.as_millis() as f64 * jitter) as u64
                    );
                }
            }
        }

        info!("Allternit Node stopped");
        Ok(())
    }

    /// Single connection attempt
    async fn connect_and_run(&self) -> Result<()> {
        let ws_config = WebSocketConfig {
            url: self.config.control_plane_url.clone(),
            node_id: self.config.node_id.clone(),
            auth_token: self.config.auth_token.clone(),
            heartbeat_interval: std::time::Duration::from_secs(30),
        };

        let mut client = WebSocketClient::new(ws_config, self.state.clone());
        
        // Connect to control plane
        client.connect().await?;
        
        info!("Connected to control plane");
        *self.state.connected.write().await = true;
        
        // Run the connection (blocks until disconnect)
        client.run().await?;
        
        // Mark as disconnected
        *self.state.connected.write().await = false;
        
        Ok(())
    }

    /// Run once (for testing)
    pub async fn run_once(self) -> Result<()> {
        info!("Running once (test mode)");
        self.connect_and_run().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
