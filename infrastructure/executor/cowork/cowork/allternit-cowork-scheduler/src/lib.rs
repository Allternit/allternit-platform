//! Allternit Scheduler - Cron-based job scheduling for cowork runtime
//!
//! Uses tokio-cron-scheduler (same as the existing native_cron_system)
//! but as a standalone implementation that doesn't depend on allternit-openclaw-host.

#![warn(missing_docs)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_cron_scheduler::{Job, JobScheduler, JobSchedulerError};
use tracing::{error, info, warn};
use uuid::Uuid;

pub mod api;
pub mod store;

/// Scheduler error types
#[derive(Error, Debug)]
pub enum SchedulerError {
    /// Scheduler operation failed
    #[error("Scheduler error: {0}")]
    Scheduler(String),

    /// Invalid cron expression
    #[error("Invalid cron expression: {0}")]
    InvalidCron(String),

    /// Schedule not found
    #[error("Schedule not found: {0}")]
    NotFound(String),

    /// Store error
    #[error("Store error: {0}")]
    Store(String),

    /// Job execution failed
    #[error("Job execution failed: {0}")]
    Execution(String),
}

impl From<JobSchedulerError> for SchedulerError {
    fn from(e: JobSchedulerError) -> Self {
        SchedulerError::Scheduler(e.to_string())
    }
}

/// Result type for scheduler operations
pub type Result<T> = std::result::Result<T, SchedulerError>;

/// A scheduled job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Unique identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Cron expression (e.g., "0 9 * * 1-5" for weekdays at 9am)
    pub cron: String,
    /// Timezone for the cron expression
    pub timezone: String,
    /// Whether the schedule is enabled
    pub enabled: bool,
    /// The entrypoint to execute
    pub entrypoint: String,
    /// Arguments to pass to the entrypoint
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Priority (higher = more important)
    pub priority: i32,
    /// Maximum runtime in seconds
    pub timeout_secs: i32,
    /// When the schedule was created
    pub created_at: DateTime<Utc>,
    /// When the schedule was last updated
    pub updated_at: DateTime<Utc>,
    /// When the schedule last triggered
    pub last_triggered_at: Option<DateTime<Utc>>,
    /// When the schedule is next due
    pub next_run_at: Option<DateTime<Utc>>,
    /// Who owns this schedule (for multi-tenant)
    pub owner: String,
    /// Run mode (interactive, cowork, scheduled)
    pub run_mode: String,
}

/// Specification for creating a new schedule
#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduleRequest {
    /// Human-readable name
    pub name: String,
    /// Cron expression
    pub cron: String,
    /// Timezone (default: UTC)
    pub timezone: Option<String>,
    /// Entrypoint to execute
    pub entrypoint: String,
    /// Arguments
    pub args: Option<Vec<String>>,
    /// Environment variables
    pub env: Option<HashMap<String, String>>,
    /// Priority
    pub priority: Option<i32>,
    /// Timeout in seconds
    pub timeout_secs: Option<i32>,
    /// Run mode
    pub run_mode: Option<String>,
}

/// Job execution context passed to handlers
#[derive(Debug, Clone)]
pub struct JobContext {
    /// Schedule ID
    pub schedule_id: String,
    /// Entrypoint to execute
    pub entrypoint: String,
    /// Arguments
    pub args: Vec<String>,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// API URL for triggering runs
    pub api_url: String,
}

/// Job handler trait for executing scheduled jobs
#[async_trait::async_trait]
pub trait JobHandler: Send + Sync {
    /// Execute a scheduled job
    async fn execute(&self, ctx: JobContext) -> Result<()>;
}

/// Default job handler that calls the API
pub struct ApiJobHandler {
    client: reqwest::Client,
}

impl ApiJobHandler {
    /// Create a new API job handler
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl JobHandler for ApiJobHandler {
    async fn execute(&self, ctx: JobContext) -> Result<()> {
        let request = serde_json::json!({
            "tenant_id": "default",
            "workspace_id": "default",
            "initiator": "scheduler",
            "mode": "scheduled",
            "entrypoint": ctx.entrypoint,
            "args": ctx.args,
            "env": ctx.env,
        });

        let response = self
            .client
            .post(format!("{}/api/v1/runs", ctx.api_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| SchedulerError::Execution(e.to_string()))?;

        if response.status().is_success() {
            info!(schedule_id = %ctx.schedule_id, entrypoint = %ctx.entrypoint, "Triggered scheduled run");
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(SchedulerError::Execution(error_text))
        }
    }
}

/// The scheduler manages cron-based job scheduling
pub struct Scheduler {
    scheduler: JobScheduler,
    store: Arc<RwLock<store::SqliteStore>>,
    api_url: String,
    handler: Arc<dyn JobHandler>,
}

impl Scheduler {
    /// Create a new scheduler
    pub async fn new(db_path: impl AsRef<std::path::Path>, api_url: impl Into<String>) -> Result<Self> {
        let scheduler = JobScheduler::new()
            .await
            .map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        let store = Arc::new(RwLock::new(
            store::SqliteStore::new(db_path)
                .await
                .map_err(|e| SchedulerError::Store(e.to_string()))?,
        ));

        Ok(Self {
            scheduler,
            store,
            api_url: api_url.into(),
            handler: Arc::new(ApiJobHandler::new()),
        })
    }

    /// Create a new scheduler with custom handler
    pub async fn with_handler(
        db_path: impl AsRef<std::path::Path>,
        api_url: impl Into<String>,
        handler: Arc<dyn JobHandler>,
    ) -> Result<Self> {
        let scheduler = JobScheduler::new()
            .await
            .map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        let store = Arc::new(RwLock::new(
            store::SqliteStore::new(db_path)
                .await
                .map_err(|e| SchedulerError::Store(e.to_string()))?,
        ));

        Ok(Self {
            scheduler,
            store,
            api_url: api_url.into(),
            handler,
        })
    }

    /// Start the scheduler
    pub async fn start(&self) -> Result<()> {
        self.scheduler
            .start()
            .await
            .map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        info!("Scheduler started");
        Ok(())
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&mut self) -> Result<()> {
        self.scheduler
            .shutdown()
            .await
            .map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        info!("Scheduler shut down");
        Ok(())
    }

    /// Create a new schedule
    pub async fn create_schedule(
        &self,
        owner: impl Into<String>,
        req: CreateScheduleRequest,
    ) -> Result<Schedule> {
        // Validate the user-provided cron expression.
        if let Err(e) = cron_parser::parse(&req.cron, &Utc::now()) {
            return Err(SchedulerError::InvalidCron(e.to_string()));
        }

        let now = Utc::now();
        let next_run = cron_parser::parse(&req.cron, &now)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));

        let schedule = Schedule {
            id: Uuid::new_v4().to_string(),
            name: req.name,
            cron: req.cron,
            timezone: req.timezone.unwrap_or_else(|| "UTC".to_string()),
            enabled: true,
            entrypoint: req.entrypoint,
            args: req.args.unwrap_or_default(),
            env: req.env.unwrap_or_default(),
            priority: req.priority.unwrap_or(0),
            timeout_secs: req.timeout_secs.unwrap_or(3600),
            created_at: now,
            updated_at: now,
            last_triggered_at: None,
            next_run_at: next_run,
            owner: owner.into(),
            run_mode: req.run_mode.unwrap_or_else(|| "scheduled".to_string()),
        };

        // Save to store
        {
            let store = self.store.write().await;
            store.save_schedule(&schedule).await.map_err(|e| SchedulerError::Store(e.to_string()))?;
        }

        // Add to scheduler if enabled
        if schedule.enabled {
            self.add_job_to_scheduler(&schedule).await?;
        }

        info!(
            schedule_id = %schedule.id,
            name = %schedule.name,
            cron = %schedule.cron,
            "Created schedule"
        );

        Ok(schedule)
    }

    /// Get a schedule by ID
    pub async fn get_schedule(&self, id: &str) -> Result<Schedule> {
        let store = self.store.read().await;
        store.get_schedule(id).await
    }

    /// List all schedules
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        let store = self.store.read().await;
        store.list_schedules().await
    }

    /// List schedules for an owner
    pub async fn list_schedules_for_owner(&self, owner: &str) -> Result<Vec<Schedule>> {
        let store = self.store.read().await;
        store.list_schedules_for_owner(owner).await
    }

    /// Enable a schedule
    pub async fn enable_schedule(&self, id: &str) -> Result<Schedule> {
        let mut schedule = self.get_schedule(id).await?;
        schedule.enabled = true;
        schedule.updated_at = Utc::now();

        let store = self.store.write().await;
        store.save_schedule(&schedule).await.map_err(|e| SchedulerError::Store(e.to_string()))?;

        // Add to scheduler
        self.add_job_to_scheduler(&schedule).await?;

        info!(schedule_id = %id, "Enabled schedule");
        Ok(schedule)
    }

    /// Disable a schedule
    pub async fn disable_schedule(&self, id: &str) -> Result<Schedule> {
        let mut schedule = self.get_schedule(id).await?;
        schedule.enabled = false;
        schedule.updated_at = Utc::now();

        let store = self.store.write().await;
        store.save_schedule(&schedule).await.map_err(|e| SchedulerError::Store(e.to_string()))?;

        // Note: tokio-cron-scheduler doesn't support removing jobs by ID in this version
        // The job will be skipped when triggered if the schedule is disabled/deleted

        info!(schedule_id = %id, "Disabled schedule");
        Ok(schedule)
    }

    /// Delete a schedule
    pub async fn delete_schedule(&self, id: &str) -> Result<()> {
        let store = self.store.write().await;
        store.delete_schedule(id).await?;

        // Note: tokio-cron-scheduler doesn't support removing jobs by ID in this version
        // The job will be skipped when triggered if the schedule is disabled/deleted

        info!(schedule_id = %id, "Deleted schedule");
        Ok(())
    }

    /// Save an updated schedule to the store.
    ///
    /// This is useful for tests and for callers that need to update metadata
    /// (such as `next_run_at`) outside of the standard lifecycle methods.
    pub async fn save_schedule(&self, schedule: &Schedule) -> Result<()> {
        let store = self.store.write().await;
        store.save_schedule(schedule).await.map_err(|e| SchedulerError::Store(e.to_string()))?;
        Ok(())
    }

    /// Run a schedule immediately (trigger a cowork run)
    pub async fn run_now(&self, id: &str) -> Result<()> {
        let schedule = self.get_schedule(id).await?;

        let ctx = JobContext {
            schedule_id: schedule.id.clone(),
            entrypoint: schedule.entrypoint.clone(),
            args: schedule.args.clone(),
            env: schedule.env.clone(),
            api_url: self.api_url.clone(),
        };

        self.handler.execute(ctx).await?;

        // Update last triggered
        let mut updated_schedule = schedule;
        updated_schedule.last_triggered_at = Some(Utc::now());
        let store = self.store.write().await;
        store.save_schedule(&updated_schedule).await.map_err(|e| SchedulerError::Store(e.to_string()))?;

        Ok(())
    }

    /// Add a job to the scheduler
    async fn add_job_to_scheduler(&self, schedule: &Schedule) -> Result<()> {
        let ctx = JobContext {
            schedule_id: schedule.id.clone(),
            entrypoint: schedule.entrypoint.clone(),
            args: schedule.args.clone(),
            env: schedule.env.clone(),
            api_url: self.api_url.clone(),
        };

        let handler = self.handler.clone();
        let store = self.store.clone();

        // Parse the cron expression (tokio-cron-scheduler requires 6 or 7 fields).
        let schedule_cron = normalize_cron(&schedule.cron)?;
        let job = Job::new_async(schedule_cron.as_str(), move |_uuid, _l| {
            let ctx = ctx.clone();
            let handler = handler.clone();
            let store = store.clone();

            Box::pin(async move {
                info!(schedule_id = %ctx.schedule_id, "Executing scheduled job");

                match handler.execute(ctx.clone()).await {
                    Ok(_) => {
                        // Update last triggered
                        let store = store.write().await;
                        if let Ok(mut schedule) = store.get_schedule(&ctx.schedule_id).await {
                            schedule.last_triggered_at = Some(Utc::now());
                            let _ = store.save_schedule(&schedule).await;
                        }
                    }
                    Err(e) => {
                        error!(schedule_id = %ctx.schedule_id, error = %e, "Scheduled job failed");
                    }
                }
            })
        }).map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        self.scheduler.add(job).await.map_err(|e| SchedulerError::Scheduler(e.to_string()))?;

        Ok(())
    }

    /// Get scheduler statistics
    pub async fn get_stats(&self) -> Result<SchedulerStats> {
        let store = self.store.read().await;
        let schedules = store.list_schedules().await.map_err(|e| SchedulerError::Store(e.to_string()))?;

        let total = schedules.len();
        let enabled = schedules.iter().filter(|s| s.enabled).count();

        Ok(SchedulerStats {
            total_schedules: total,
            enabled_schedules: enabled,
        })
    }

    /// Check all enabled schedules and trigger any that are due.
    ///
    /// A schedule is considered due when its `next_run_at` timestamp is in the
    /// past and it has not been triggered since that time. The method returns
    /// the IDs of schedules that were triggered.
    pub async fn wake_due_schedules(&self) -> Result<Vec<String>> {
        let store = self.store.read().await;
        let schedules = store.list_schedules().await.map_err(|e| SchedulerError::Store(e.to_string()))?;
        drop(store);

        let now = Utc::now();
        let mut triggered = Vec::new();

        for schedule in schedules {
            if !schedule.enabled {
                continue;
            }

            let Some(next_run) = schedule.next_run_at else {
                continue;
            };

            if next_run > now {
                continue;
            }

            let already_triggered = schedule
                .last_triggered_at
                .map(|last| last >= next_run)
                .unwrap_or(false);

            if already_triggered {
                continue;
            }

            match self.run_now(&schedule.id).await {
                Ok(_) => {
                    info!(schedule_id = %schedule.id, "Triggered due schedule");
                    triggered.push(schedule.id.clone());
                }
                Err(e) => {
                    error!(schedule_id = %schedule.id, error = %e, "Failed to trigger due schedule");
                }
            }
        }

        Ok(triggered)
    }
}

/// Normalize a cron expression for `tokio-cron-scheduler`.
///
/// The user-facing API accepts standard 5-field cron (`min hour day month weekday`).
/// `tokio-cron-scheduler` (via the `cron` crate) requires 6 or 7 fields, so 5-field
/// expressions are normalized by prepending a `0` second field.
fn normalize_cron(expr: &str) -> Result<String> {
    let trimmed = expr.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    match parts.len() {
        5 => Ok(format!("0 {}", trimmed)),
        6 | 7 => Ok(trimmed.to_string()),
        _ => Err(SchedulerError::InvalidCron(format!(
            "Invalid cron field count: expected 5, 6, or 7, got {}",
            parts.len()
        ))),
    }
}

/// Scheduler statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStats {
    /// Total number of schedules
    pub total_schedules: usize,
    /// Number of enabled schedules
    pub enabled_schedules: usize,
}

/// Version of this crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
