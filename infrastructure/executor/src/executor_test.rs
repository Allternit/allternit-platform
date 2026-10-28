// OWNER: T1-A5

//! Executor Service Unit Tests
//!
//! Unit tests for the ExecutorService implementation.

#[cfg(test)]
mod tests {
    use super::super::executor::{ExecutorService, ExecutorServiceImpl};
    use super::super::types::{Job, JobFilter, JobStatus, Priority};
    use std::collections::HashMap;

    // ========================================================================
    // ExecutorService Creation Tests
    // ========================================================================

    #[tokio::test]
    async fn test_executor_submit_returns_job_id() {
        let executor = ExecutorServiceImpl::new();
        let job = Job::new("test-job", "alpine:latest");
        let original_id = job.id;

        let result = executor.submit(job).await;

        // Currently returns the original job ID (stub implementation)
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), original_id);
    }

    #[tokio::test]
    async fn test_executor_submit_multiple_jobs() {
        let executor = ExecutorServiceImpl::new();

        let job1 = Job::new("job-1", "alpine:latest");
        let job2 = Job::new("job-2", "alpine:latest");
        let job3 = Job::new("job-3", "alpine:latest");

        let id1 = executor.submit(job1).await.unwrap();
        let id2 = executor.submit(job2).await.unwrap();
        let id3 = executor.submit(job3).await.unwrap();

        // All jobs should have unique IDs
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[tokio::test]
    async fn test_executor_status_returns_pending() {
        let executor = ExecutorServiceImpl::new();

        // First submit a job
        let job = Job::new("test-job", "alpine:latest");
        let job_id = executor.submit(job).await.unwrap();

        // Now check status
        let status = executor.status(job_id).await;
        assert!(status.is_ok());
        assert_eq!(status.unwrap(), JobStatus::Pending);
    }

    #[tokio::test]
    async fn test_executor_status_different_jobs() {
        let executor = ExecutorServiceImpl::new();

        // Submit two jobs
        let job1 = Job::new("job-1", "alpine:latest");
        let job2 = Job::new("job-2", "alpine:latest");

        let id1 = executor.submit(job1).await.unwrap();
        let id2 = executor.submit(job2).await.unwrap();

        // Status should work for both
        let status1 = executor.status(id1).await.unwrap();
        let status2 = executor.status(id2).await.unwrap();

        assert_eq!(status1, JobStatus::Pending);
        assert_eq!(status2, JobStatus::Pending);
    }

    #[tokio::test]
    async fn test_executor_list_returns_empty() {
        let executor = ExecutorServiceImpl::new();

        let jobs = executor.list(None).await;

        assert!(jobs.is_ok());
        assert!(jobs.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_executor_list_with_filter() {
        let executor = ExecutorServiceImpl::new();

        // Submit some jobs first
        let job1 = Job::new("job-1", "alpine:latest").with_priority(Priority::High);
        let job2 = Job::new("job-2", "alpine:latest").with_priority(Priority::Low);
        executor.submit(job1).await.unwrap();
        executor.submit(job2).await.unwrap();

        // Filter by High priority
        let filter = JobFilter {
            priority: Some(Priority::High),
            ..Default::default()
        };

        let jobs = executor.list(Some(filter)).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].priority, Priority::High);
    }

    #[tokio::test]
    async fn test_executor_cancel_returns_ok() {
        let executor = ExecutorServiceImpl::new();

        // Submit a job first
        let job = Job::new("test-job", "alpine:latest");
        let job_id = executor.submit(job).await.unwrap();

        // Now cancel should work
        let result = executor.cancel(job_id).await;
        assert!(result.is_ok());

        // Verify status changed to Cancelled
        let status = executor.status(job_id).await.unwrap();
        assert_eq!(status, JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_executor_cancel_not_found() {
        let executor = ExecutorServiceImpl::new();
        let job_id = uuid::Uuid::new_v4();

        // Cancel non-existent job should fail
        let result = executor.cancel(job_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_executor_status_not_found() {
        let executor = ExecutorServiceImpl::new();
        let job_id = uuid::Uuid::new_v4();

        // Status for non-existent job should fail
        let result = executor.status(job_id).await;
        assert!(result.is_err());
    }

    // ========================================================================
    // Job Builder Integration Tests
    // ========================================================================

    #[tokio::test]
    async fn test_executor_submit_with_command() {
        let executor = ExecutorServiceImpl::new();
        let job = Job::new("test-job", "alpine:latest")
            .with_command(vec!["echo".to_string(), "hello".to_string()]);

        let result = executor.submit(job).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_submit_with_env() {
        let executor = ExecutorServiceImpl::new();
        let mut env = HashMap::new();
        env.insert("TEST".to_string(), "VALUE".to_string());

        let job = Job::new("test-job", "alpine:latest").with_env(env);

        let result = executor.submit(job).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_submit_with_priority() {
        let executor = ExecutorServiceImpl::new();

        let priorities = vec![
            Priority::Critical,
            Priority::High,
            Priority::Normal,
            Priority::Low,
            Priority::Batch,
        ];

        for priority in priorities {
            let job = Job::new("test-job", "alpine:latest").with_priority(priority);

            let result = executor.submit(job).await;
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_executor_submit_with_resources() {
        let executor = ExecutorServiceImpl::new();
        let job = Job::new("test-job", "alpine:latest").with_resources(
            super::super::types::ResourceRequest::default()
                .with_cpu(2.0)
                .with_memory(1024 * 1024 * 1024),
        );

        let result = executor.submit(job).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_executor_submit_with_timeout() {
        let executor = ExecutorServiceImpl::new();
        let job =
            Job::new("test-job", "alpine:latest").with_timeout(std::time::Duration::from_secs(300));

        let result = executor.submit(job).await;
        assert!(result.is_ok());
    }

    // ========================================================================
    // Error Handling Tests (Future Implementation)
    // ========================================================================

    // TODO: Add tests for error cases when implementation is complete:
    // - Submit job with invalid image
    // - Submit job with insufficient resources
    // - Cancel non-existent job (already tested above)
    // - Status for non-existent job (already tested above)
}
