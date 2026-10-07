//! Allternit Scheduler Daemon
//!
//! Server-side scheduling service that:
//! - Polls schedules from the control plane database
//! - Triggers runs when schedules are due
//! - Handles misfires and retries

use anyhow::Result;
use clap::Parser;
use tracing::info;

mod daemon;
mod misfire;
mod scheduler;

use daemon::SchedulerDaemon;

/// How the scheduler executes jobs when they are due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    /// Call the control plane API to create a run (cloud mode).
    #[default]
    Api,
    /// Execute the configured command directly on this machine (local mode).
    Local,
}

impl std::str::FromStr for ExecutionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "api" => Ok(ExecutionMode::Api),
            "local" => Ok(ExecutionMode::Local),
            _ => Err(format!("Unknown execution mode: {}", s)),
        }
    }
}

/// Scheduler daemon configuration
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Database URL
    pub database_url: String,
    /// Control plane API URL (used in Api mode)
    pub api_url: String,
    /// API key for authentication
    pub api_key: Option<String>,
    /// Polling interval in seconds
    pub poll_interval_secs: u64,
    /// Max schedules to process per tick
    pub max_schedules_per_tick: usize,
    /// Misfire threshold in seconds
    pub misfire_threshold_secs: i64,
    /// Misfire policy: ignore, fire_once, fire_all
    pub misfire_policy: MisfirePolicy,
    /// Execution mode: api or local
    pub execution_mode: ExecutionMode,
    /// How long a claim on a due schedule is honoured before another daemon
    /// may take it over (covers a daemon that died mid-fire). Must exceed the
    /// longest job trigger (local jobs default to a 300s timeout).
    pub claim_ttl_secs: i64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            database_url: "postgres://localhost/allternit".to_string(),
            api_url: "http://localhost:3001".to_string(),
            api_key: None,
            poll_interval_secs: 60,
            max_schedules_per_tick: 100,
            misfire_threshold_secs: 300, // 5 minutes
            misfire_policy: MisfirePolicy::FireOnce,
            execution_mode: ExecutionMode::Api,
            claim_ttl_secs: 900,
        }
    }
}

/// Misfire policy for missed schedules
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MisfirePolicy {
    Ignore,
    FireOnce,
    FireAll,
}

impl std::str::FromStr for MisfirePolicy {
    type Err = String;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ignore" => Ok(MisfirePolicy::Ignore),
            "fire_once" => Ok(MisfirePolicy::FireOnce),
            "fire_all" => Ok(MisfirePolicy::FireAll),
            _ => Err(format!("Unknown misfire policy: {}", s)),
        }
    }
}

/// CLI arguments
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Postgres database URL (postgres://…) — the control-plane database that
    /// holds the `schedules` table
    #[arg(short, long, env = "ALLTERNIT_SCHEDULER_DATABASE_URL")]
    database_url: String,

    /// Control plane API URL
    #[arg(short, long, env = "ALLTERNIT_SCHEDULER_API_URL", default_value = "http://localhost:3001")]
    api_url: String,

    /// API key for authentication
    #[arg(long, env = "ALLTERNIT_SCHEDULER_API_KEY")]
    api_key: Option<String>,

    /// Polling interval in seconds
    #[arg(long, env = "ALLTERNIT_SCHEDULER_POLL_INTERVAL_SECS", default_value = "60")]
    poll_interval_secs: u64,

    /// Misfire policy
    #[arg(long, env = "ALLTERNIT_SCHEDULER_MISFIRE_POLICY", default_value = "fire_once")]
    misfire_policy: String,

    /// Execution mode: api (cloud) or local (run commands directly)
    #[arg(long, env = "ALLTERNIT_SCHEDULER_EXECUTION_MODE", default_value = "api")]
    execution_mode: String,

    /// Seconds a claim on a due schedule is honoured before it can be taken over
    #[arg(long, env = "ALLTERNIT_SCHEDULER_CLAIM_TTL_SECS", default_value = "900")]
    claim_ttl_secs: i64,

    /// Run once and exit (for testing)
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info,allternit_scheduler=debug")
        .with_target(false)
        .init();

    info!("Allternit Scheduler Daemon v{}", env!("CARGO_PKG_VERSION"));

    let args = Args::parse();

    // Build configuration
    let execution_mode = args.execution_mode.parse().unwrap_or(ExecutionMode::Api);
    let config = SchedulerConfig {
        database_url: args.database_url,
        api_url: args.api_url,
        api_key: args.api_key,
        poll_interval_secs: args.poll_interval_secs,
        max_schedules_per_tick: 100,
        misfire_threshold_secs: 300,
        misfire_policy: args.misfire_policy.parse().unwrap_or(MisfirePolicy::FireOnce),
        execution_mode,
        claim_ttl_secs: args.claim_ttl_secs.max(60),
    };

    info!("API URL: {}", config.api_url);
    info!("Poll interval: {}s", config.poll_interval_secs);
    info!("Misfire policy: {:?}", config.misfire_policy);
    info!("Execution mode: {:?}", config.execution_mode);

    // Create and run scheduler daemon
    let daemon = SchedulerDaemon::new(config).await?;

    if args.once {
        info!("Running once (test mode)");
        daemon.run_once().await?;
    } else {
        info!("Starting scheduler daemon...");
        daemon.run().await?;
    }

    Ok(())
}
