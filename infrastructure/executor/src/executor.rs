// OWNER: T1-A4

//! Executor Service
//!
//! Primary job execution engine that coordinates job lifecycle and manages execution state.

use crate::types::{
    ContainerError, ExecutorError, Job, JobFilter, JobId, JobStatus, JobSummary, Result,
};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Executor service trait for job execution
#[async_trait]
pub trait ExecutorService {
    /// Submit a new job for execution
    async fn submit(&self, job: Job) -> Result<JobId>;

    /// Cancel a running or pending job
    async fn cancel(&self, job_id: JobId) -> Result<()>;

    /// Get job status
    async fn status(&self, job_id: JobId) -> Result<JobStatus>;

    /// List all jobs with optional filter
    async fn list(&self, filter: Option<JobFilter>) -> Result<Vec<JobSummary>>;
}

/// Executor service implementation with in-memory job storage
pub struct ExecutorServiceImpl {
    jobs: RwLock<HashMap<JobId, Job>>,
    job_statuses: RwLock<HashMap<JobId, JobStatus>>,
}

impl ExecutorServiceImpl {
    pub fn new() -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
            job_statuses: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for ExecutorServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutorService for ExecutorServiceImpl {
    async fn submit(&self, job: Job) -> Result<JobId> {
        let job_id = job.id;

        // Store the job
        let mut jobs = self.jobs.write().await;
        jobs.insert(job_id, job);

        // Set initial status to Pending
        let mut statuses = self.job_statuses.write().await;
        statuses.insert(job_id, JobStatus::Pending);

        Ok(job_id)
    }

    async fn cancel(&self, job_id: JobId) -> Result<()> {
        let mut statuses = self.job_statuses.write().await;

        let status = statuses.get(&job_id).ok_or_else(|| {
            ExecutorError::ContainerError(ContainerError::NotFound(format!("Job {}", job_id)))
        })?;

        // Can only cancel Pending, Queued, or Starting jobs
        match status {
            JobStatus::Pending | JobStatus::Queued { .. } | JobStatus::Starting => {
                statuses.insert(job_id, JobStatus::Cancelled);
                Ok(())
            }
            JobStatus::Running { .. } => {
                // Mark for cancellation - actual stop happens in runtime
                statuses.insert(job_id, JobStatus::Cancelled);
                Ok(())
            }
            _ => Err(ExecutorError::InvalidStateTransition {
                from: status.clone(),
                to: JobStatus::Cancelled,
            }),
        }
    }

    async fn status(&self, job_id: JobId) -> Result<JobStatus> {
        let statuses = self.job_statuses.read().await;

        statuses.get(&job_id).cloned().ok_or_else(|| {
            ExecutorError::ContainerError(ContainerError::NotFound(format!("Job {}", job_id)))
        })
    }

    async fn list(&self, filter: Option<JobFilter>) -> Result<Vec<JobSummary>> {
        let jobs = self.jobs.read().await;
        let statuses = self.job_statuses.read().await;

        let mut summaries = Vec::new();

        for (job_id, job) in jobs.iter() {
            // Apply filter if provided
            if let Some(ref f) = filter {
                // Filter by status
                if let Some(ref status_filter) = f.status {
                    if let Some(current_status) = statuses.get(job_id) {
                        if !status_matches_filter(current_status, status_filter) {
                            continue;
                        }
                    }
                }

                // Filter by priority
                if let Some(ref priority_filter) = f.priority {
                    if job.priority != *priority_filter {
                        continue;
                    }
                }
            }

            let status = statuses.get(job_id).cloned().unwrap_or(JobStatus::Pending);

            summaries.push(JobSummary {
                id: *job_id,
                name: job.name.clone(),
                status,
                image: job.image.clone(),
                priority: job.priority,
                created_at: job.created_at,
                updated_at: None,
            });
        }

        // Sort by created_at
        summaries.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        Ok(summaries)
    }
}

/// Helper function to check if a status matches a filter
fn status_matches_filter(status: &JobStatus, filter: &JobStatus) -> bool {
    match (status, filter) {
        (JobStatus::Pending, JobStatus::Pending) => true,
        (JobStatus::Queued { .. }, JobStatus::Queued { .. }) => true,
        (JobStatus::Starting, JobStatus::Starting) => true,
        (JobStatus::Running { .. }, JobStatus::Running { .. }) => true,
        (JobStatus::Completed { .. }, JobStatus::Completed { .. }) => true,
        (JobStatus::Failed { .. }, JobStatus::Failed { .. }) => true,
        (JobStatus::Cancelled, JobStatus::Cancelled) => true,
        (JobStatus::Terminated, JobStatus::Terminated) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
