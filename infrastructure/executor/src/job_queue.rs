// OWNER: T1-A4

//! JobQueue Service
//!
//! Manages job scheduling and prioritization for the executor.

use crate::types::{ExecutorError, Job, JobId, Priority, Result};
use async_trait::async_trait;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use tokio::sync::RwLock;

/// Queued job wrapper with priority ordering
#[derive(Clone, Debug)]
pub struct QueuedJob {
    pub job: Job,
    pub priority: Priority,
    pub queued_at: chrono::DateTime<chrono::Utc>,
}

impl Eq for QueuedJob {}

impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.job.id == other.job.id
    }
}

impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first, then earlier queued_at
        self.priority
            .value()
            .cmp(&other.priority.value())
            .then_with(|| other.queued_at.cmp(&self.queued_at))
    }
}

/// Queue position information
pub struct QueuePosition {
    pub position: usize,
    pub estimated_wait: Option<std::time::Duration>,
}

/// Job queue service trait
#[async_trait]
pub trait JobQueueService {
    /// Enqueue a job
    async fn enqueue(&self, job: QueuedJob) -> Result<QueuePosition>;

    /// Dequeue next job for execution
    async fn dequeue(&self) -> Result<Option<QueuedJob>>;

    /// Peek at next job without removing
    async fn peek(&self) -> Result<Option<QueuedJob>>;

    /// Remove job from queue
    async fn remove(&self, job_id: JobId) -> Result<()>;

    /// Get queue length
    async fn len(&self) -> Result<usize>;

    /// Get job position in queue
    async fn position(&self, job_id: JobId) -> Result<Option<usize>>;
}

/// Job queue service implementation with priority queue
pub struct JobQueueServiceImpl {
    queue: RwLock<BinaryHeap<QueuedJob>>,
    job_index: RwLock<HashMap<JobId, Priority>>,
}

impl JobQueueServiceImpl {
    pub fn new() -> Self {
        Self {
            queue: RwLock::new(BinaryHeap::new()),
            job_index: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for JobQueueServiceImpl {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl JobQueueService for JobQueueServiceImpl {
    async fn enqueue(&self, job: QueuedJob) -> Result<QueuePosition> {
        let job_id = job.job.id;
        let priority = job.priority;

        // Add to index
        {
            let mut index = self.job_index.write().await;
            index.insert(job_id, priority);
        }

        // Add to priority queue
        let mut queue = self.queue.write().await;
        queue.push(job);

        // Calculate position (1-indexed, based on priority ordering)
        let position = queue.len();

        // Estimate wait time based on position and average job duration
        let estimated_wait = if position > 0 {
            Some(std::time::Duration::from_secs(position as u64 * 30))
        } else {
            None
        };

        Ok(QueuePosition {
            position,
            estimated_wait,
        })
    }

    async fn dequeue(&self) -> Result<Option<QueuedJob>> {
        let mut queue = self.queue.write().await;

        if let Some(job) = queue.pop() {
            // Remove from index
            let mut index = self.job_index.write().await;
            index.remove(&job.job.id);

            Ok(Some(job))
        } else {
            Ok(None)
        }
    }

    async fn peek(&self) -> Result<Option<QueuedJob>> {
        let queue = self.queue.read().await;
        Ok(queue.peek().cloned())
    }

    async fn remove(&self, job_id: JobId) -> Result<()> {
        // Remove from index
        {
            let mut index = self.job_index.write().await;
            if !index.contains_key(&job_id) {
                return Err(ExecutorError::ContainerError(
                    crate::types::ContainerError::NotFound(format!(
                        "Job {} not found in queue",
                        job_id
                    )),
                ));
            }
            index.remove(&job_id);
        }

        // Rebuild queue without the job
        let mut queue = self.queue.write().await;
        let jobs: Vec<QueuedJob> = queue.drain().filter(|j| j.job.id != job_id).collect();

        for job in jobs {
            queue.push(job);
        }

        Ok(())
    }

    async fn len(&self) -> Result<usize> {
        let queue = self.queue.read().await;
        Ok(queue.len())
    }

    async fn position(&self, job_id: JobId) -> Result<Option<usize>> {
        let queue = self.queue.read().await;
        let jobs: Vec<&QueuedJob> = queue.iter().collect();

        for (idx, job) in jobs.iter().enumerate() {
            if job.job.id == job_id {
                return Ok(Some(idx + 1)); // 1-indexed
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
