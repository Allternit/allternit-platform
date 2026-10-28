// OWNER: T1-A5

//! JobQueue Service Unit Tests
//!
//! Unit tests for the JobQueueService implementation.

#[cfg(test)]
mod tests {
    use super::super::job_queue::{JobQueueService, JobQueueServiceImpl, QueuePosition, QueuedJob};
    use super::super::types::{Job, Priority};

    // ========================================================================
    // JobQueueService Creation Tests
    // ========================================================================

    // ========================================================================
    // QueuedJob Tests
    // ========================================================================

    #[test]
    fn test_queued_job_creation() {
        let job = Job::new("test-job", "alpine:latest");
        let queued_job = QueuedJob {
            job: job.clone(),
            priority: Priority::High,
            queued_at: chrono::Utc::now(),
        };

        assert_eq!(queued_job.job.name, "test-job");
        assert_eq!(queued_job.priority, Priority::High);
    }

    #[test]
    fn test_queued_job_with_different_priorities() {
        let priorities = vec![
            Priority::Critical,
            Priority::High,
            Priority::Normal,
            Priority::Low,
            Priority::Batch,
        ];

        for priority in priorities {
            let job = Job::new("test-job", "alpine:latest");
            let queued_job = QueuedJob {
                job,
                priority,
                queued_at: chrono::Utc::now(),
            };

            assert_eq!(queued_job.priority, priority);
        }
    }

    // ========================================================================
    // QueuePosition Tests
    // ========================================================================

    #[test]
    fn test_queue_position_creation() {
        let position = QueuePosition {
            position: 1,
            estimated_wait: Some(std::time::Duration::from_secs(60)),
        };

        assert_eq!(position.position, 1);
        assert_eq!(
            position.estimated_wait,
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn test_queue_position_without_estimate() {
        let position = QueuePosition {
            position: 0,
            estimated_wait: None,
        };

        assert_eq!(position.position, 0);
        assert!(position.estimated_wait.is_none());
    }

    // ========================================================================
    // JobQueueService Enqueue Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_enqueue_returns_position() {
        let queue = JobQueueServiceImpl::new();
        let job = Job::new("test-job", "alpine:latest");
        let queued_job = QueuedJob {
            job,
            priority: Priority::Normal,
            queued_at: chrono::Utc::now(),
        };

        let result = queue.enqueue(queued_job).await;

        assert!(result.is_ok());
        let position = result.unwrap();
        assert_eq!(position.position, 1); // First item is at position 1
    }

    #[tokio::test]
    async fn test_queue_enqueue_multiple_jobs() {
        let queue = JobQueueServiceImpl::new();

        for i in 0..5 {
            let job = Job::new(&format!("job-{}", i), "alpine:latest");
            let queued_job = QueuedJob {
                job,
                priority: Priority::Normal,
                queued_at: chrono::Utc::now(),
            };

            let result = queue.enqueue(queued_job).await;
            assert!(result.is_ok());
        }

        // Verify queue length
        assert_eq!(queue.len().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_queue_enqueue_with_different_priorities() {
        let queue = JobQueueServiceImpl::new();

        let priorities = vec![
            Priority::Critical,
            Priority::High,
            Priority::Normal,
            Priority::Low,
            Priority::Batch,
        ];

        for (i, priority) in priorities.iter().enumerate() {
            let job = Job::new(&format!("job-{}", i), "alpine:latest");
            let queued_job = QueuedJob {
                job,
                priority: *priority,
                queued_at: chrono::Utc::now(),
            };

            let result = queue.enqueue(queued_job).await;
            assert!(result.is_ok());
        }
    }

    // ========================================================================
    // JobQueueService Dequeue Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_dequeue_empty() {
        let queue = JobQueueServiceImpl::new();

        let result = queue.dequeue().await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    // ========================================================================
    // JobQueueService Peek Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_peek_empty() {
        let queue = JobQueueServiceImpl::new();

        let result = queue.peek().await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    // ========================================================================
    // JobQueueService Remove Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_remove_returns_ok() {
        let queue = JobQueueServiceImpl::new();

        // First enqueue a job
        let job = Job::new("test-job", "alpine:latest");
        let job_id = job.id;
        let queued_job = QueuedJob {
            job,
            priority: Priority::Normal,
            queued_at: chrono::Utc::now(),
        };
        queue.enqueue(queued_job).await.unwrap();

        // Now remove should work
        let result = queue.remove(job_id).await;
        assert!(result.is_ok());

        // Verify queue is empty
        assert_eq!(queue.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_queue_remove_not_found() {
        let queue = JobQueueServiceImpl::new();
        let job_id = uuid::Uuid::new_v4();

        // Remove non-existent job should fail
        let result = queue.remove(job_id).await;
        assert!(result.is_err());
    }

    // ========================================================================
    // JobQueueService Length Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_len_empty() {
        let queue = JobQueueServiceImpl::new();

        let len = queue.len().await;

        // Stub implementation returns 0
        assert!(len.is_ok());
        assert_eq!(len.unwrap(), 0);
    }

    // ========================================================================
    // JobQueueService Position Tests
    // ========================================================================

    #[tokio::test]
    async fn test_queue_position_not_found() {
        let queue = JobQueueServiceImpl::new();
        let job_id = uuid::Uuid::new_v4();

        let result = queue.position(job_id).await;

        // Stub implementation returns None
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    // ========================================================================
    // Priority Ordering Tests (Future Implementation)
    // ========================================================================

    // TODO: When implementation is complete, add tests for:
    // - Higher priority jobs are dequeued first
    // - Same priority jobs are dequeued in FIFO order
    // - Critical priority jumps to front of queue
    // - Batch priority is always last

    // ========================================================================
    // Queue Capacity Tests (Future Implementation)
    // ========================================================================

    // TODO: When implementation is complete, add tests for:
    // - Queue has maximum capacity
    // - Enqueue returns error when queue is full
    // - Queue length respects capacity limits

    // ========================================================================
    // Queue Error Handling Tests (Future Implementation)
    // ========================================================================

    // TODO: When implementation is complete, add tests for:
    // - Remove non-existent job returns error
    // - Position lookup for non-existent job returns error
}
