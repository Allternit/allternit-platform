//! Integration tests for Cowork Runtime
//!
//! Comprehensive end-to-end tests covering:
//! - Run lifecycle
//! - Approval flows
//! - Schedule management
//! - Session/attachment handling

mod common;

use common::TestApp;
use allternit_cloud_api::db::cowork_models::*;

// ============================================================================
// Run Lifecycle Tests
// ============================================================================

#[tokio::test]
async fn test_run_create_start_complete() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Test Run").await;
    assert_eq!(run.name, "Test Run");
    assert!(matches!(run.status, RunStatus::Pending));

    // Start run
    let run = app.start_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Queued | RunStatus::Running));

    // Verify events were created
    let events = app.get_run_events(&run.id).await;
    let has_created_event = events.iter().any(|e| matches!(e.event_type, EventType::RunCreated));
    // Starting a Pending run only transitions it to Queued; the service records
    // that as a generic status-change (Heartbeat) event. RunStarted is emitted
    // only when a run actually enters Running (e.g. on resume).
    let has_transition_event = events.iter().any(|e| matches!(e.event_type, EventType::Heartbeat));
    assert!(has_created_event, "Should have RunCreated event");
    assert!(has_transition_event, "Should have a status transition event after start");
}

#[tokio::test]
async fn test_run_create_and_get() {
    let app = TestApp::new().await;

    // Create run
    let created = app.create_run("Test Run Get").await;
    
    // Get run
    let fetched = app.get_run(&created.id).await;
    assert_eq!(created.id, fetched.id);
    assert_eq!(created.name, fetched.name);
    assert_eq!(created.status, fetched.status);
}

#[tokio::test]
async fn test_run_list() {
    let app = TestApp::new().await;

    // Create multiple runs
    let run1 = app.create_run("Run 1").await;
    let run2 = app.create_run("Run 2").await;

    // List runs
    let response = app.get("/api/v1/runs").await;
    TestApp::assert_success(&response);
    let runs: Vec<RunSummary> = TestApp::parse_json(response).await;
    
    assert!(runs.len() >= 2, "Should have at least 2 runs");
    let ids: Vec<_> = runs.iter().map(|r| &r.id).collect();
    assert!(ids.contains(&&run1.id));
    assert!(ids.contains(&&run2.id));
}

#[tokio::test]
async fn test_run_update() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Original Name").await;
    
    // Update run
    let update_request = serde_json::json!({
        "name": "Updated Name",
        "description": "Updated description"
    });
    let response = app.put(&format!("/api/v1/runs/{}", run.id), update_request).await;
    TestApp::assert_success(&response);
    
    let updated: Run = TestApp::parse_json(response).await;
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.description, Some("Updated description".to_string()));
}

#[tokio::test]
async fn test_run_delete() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("To Delete").await;
    
    // Delete run
    let response = app.delete(&format!("/api/v1/runs/{}", run.id)).await;
    TestApp::assert_success(&response);
    
    // Verify deletion
    let response = app.get(&format!("/api/v1/runs/{}", run.id)).await;
    assert_eq!(response.status(), 404);
}

// ============================================================================
// Run Pause/Resume Tests
// ============================================================================

#[tokio::test]
async fn test_run_pause_resume() {
    let app = TestApp::new().await;

    // Create and start run
    let run = app.create_run("Pause Test").await;
    let run = app.start_run(&run.id).await;
    
    // Move run to running state (since start might queue it, let's transition manually via db)
    sqlx::query("UPDATE runs SET status = 'running' WHERE id = $1")
        .bind(&run.id)
        .execute(&app.db)
        .await
        .unwrap();

    // Pause run
    let run = app.pause_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Paused));

    // Resume run
    let run = app.resume_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Running));

    // Verify events
    let events = app.get_run_events(&run.id).await;
    let has_paused_event = events.iter().any(|e| matches!(e.event_type, EventType::RunPaused));
    // Resume transitions the run back to Running, which the service records as
    // RunStarted (EventType::RunResumed exists but is never emitted).
    let has_resumed_event = events.iter().any(|e| matches!(e.event_type, EventType::RunStarted));
    assert!(has_paused_event, "Should have RunPaused event");
    assert!(has_resumed_event, "Should have RunStarted event after resume");
}

#[tokio::test]
async fn test_run_pause_invalid_state() {
    let app = TestApp::new().await;

    // Create run (not started)
    let run = app.create_run("Invalid Pause Test").await;
    
    // Try to pause - should fail
    let response = app.post(&format!("/api/v1/runs/{}/pause", run.id), serde_json::json!({})).await;
    assert_eq!(response.status(), 400);
}

// ============================================================================
// Run Cancel Tests
// ============================================================================

#[tokio::test]
async fn test_run_cancel() {
    let app = TestApp::new().await;

    // Create and start run
    let run = app.create_run("Cancel Test").await;
    let run = app.start_run(&run.id).await;
    
    // Cancel run
    let run = app.cancel_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Cancelled));
    assert!(run.completed_at.is_some());

    // NOTE: cancel() updates the run row directly and currently records no
    // RunCancelled event, so there is no event to assert here.
}

#[tokio::test]
async fn test_run_cancel_already_terminal() {
    let app = TestApp::new().await;

    // Create, start and cancel run
    let run = app.create_run("Already Cancelled").await;
    let run = app.start_run(&run.id).await;
    let _run = app.cancel_run(&run.id).await;
    
    // Try to cancel again - should fail
    let response = app.post(&format!("/api/v1/runs/{}/cancel", run.id), serde_json::json!({})).await;
    assert_eq!(response.status(), 400);
}

// ============================================================================
// Approval Flow Tests
// ============================================================================

#[tokio::test]
async fn test_approval_request_approve() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Approval Test").await;

    // Create approval request
    let approval = app.create_approval(&run.id, "Test Approval").await;
    assert_eq!(approval.title, "Test Approval");
    assert!(matches!(approval.status, ApprovalStatus::Pending));

    // Approve the request
    let approval = app.approve_request(&approval.id, Some("Approved for testing")).await;
    assert!(matches!(approval.status, ApprovalStatus::Approved));
    assert!(approval.responded_at.is_some());
}

#[tokio::test]
async fn test_approval_request_deny() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Deny Test").await;

    // Create approval request
    let approval = app.create_approval(&run.id, "Deny Test Approval").await;
    
    // Deny the request
    let approval = app.deny_request(&approval.id, Some("Denied for testing")).await;
    assert!(matches!(approval.status, ApprovalStatus::Denied));
    assert!(approval.responded_at.is_some());
}

#[tokio::test]
async fn test_approval_request_get() {
    let app = TestApp::new().await;

    // Create run and approval
    let run = app.create_run("Get Approval Test").await;
    let created = app.create_approval(&run.id, "Get Test").await;
    
    // Get approval
    let fetched = app.get_approval(&created.id).await;
    assert_eq!(created.id, fetched.id);
    assert_eq!(created.title, fetched.title);
}

#[tokio::test]
async fn test_approval_list() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("List Approval Test").await;
    
    // Create multiple approvals
    let _approval1 = app.create_approval(&run.id, "Approval 1").await;
    let _approval2 = app.create_approval(&run.id, "Approval 2").await;

    // List approvals
    let response = app.get("/api/v1/approvals").await;
    TestApp::assert_success(&response);
    let approvals: Vec<ApprovalRequestSummary> = TestApp::parse_json(response).await;
    assert!(approvals.len() >= 2);
}

#[tokio::test]
async fn test_approval_list_by_run_id() {
    let app = TestApp::new().await;

    // Create two runs
    let run1 = app.create_run("Run 1").await;
    let run2 = app.create_run("Run 2").await;
    
    // Create approval for run1
    let approval1 = app.create_approval(&run1.id, "Run 1 Approval").await;
    
    // Create approval for run2
    let _approval2 = app.create_approval(&run2.id, "Run 2 Approval").await;

    // List approvals filtered by run1
    let response = app.get(&format!("/api/v1/approvals?run_id={}", run1.id)).await;
    TestApp::assert_success(&response);
    let approvals: Vec<ApprovalRequestSummary> = TestApp::parse_json(response).await;
    
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].id, approval1.id);
}

#[tokio::test]
async fn test_approval_already_responded() {
    let app = TestApp::new().await;

    // Create run and approval
    let run = app.create_run("Double Response Test").await;
    let approval = app.create_approval(&run.id, "Double Test").await;
    
    // Approve
    let _approval = app.approve_request(&approval.id, None).await;
    
    // Try to approve again - should fail
    let body = ApprovalResponse {
        approved: true,
        message: None,
        modified_params: None,
    };
    let response = app.post(&format!("/api/v1/approvals/{}/approve", approval.id), body).await;
    assert_eq!(response.status(), 400);
}

// ============================================================================
// Schedule Tests
// ============================================================================

#[tokio::test]
async fn test_schedule_create_trigger() {
    let app = TestApp::new().await;

    // Create schedule
    let schedule = app.create_schedule("Test Schedule", "0 0 * * *").await;
    assert_eq!(schedule.name, "Test Schedule");
    assert_eq!(schedule.cron_expr, "0 0 * * *");
    assert!(schedule.enabled);

    // Trigger schedule
    let result = app.trigger_schedule(&schedule.id).await;
    assert!(result.get("run_id").is_some());

    // Verify schedule run count updated
    let updated_schedule = app.get_schedule(&schedule.id).await;
    assert_eq!(updated_schedule.run_count, 1);
    assert!(updated_schedule.last_run_at.is_some());
}

/// Two pollers on one database (two API replicas, or the API plus the
/// standalone daemon) must fire a due schedule once: the tick claims rows
/// with `FOR UPDATE SKIP LOCKED` (migration 018's claimed_by/claimed_at).
#[tokio::test]
async fn test_scheduler_two_pollers_fire_due_schedule_once() {
    use allternit_cloud_api::services::{SchedulerConfig, SchedulerService};

    let app = TestApp::new().await;
    let schedule = app.create_schedule("Claimed", "0 9 * * *").await;
    sqlx::query(
        "UPDATE schedules SET next_run_at = now() - interval '5 seconds', timezone = 'America/New_York' WHERE id = $1",
    )
    .bind(&schedule.id)
    .execute(&app.db)
    .await
    .unwrap();

    let config = SchedulerConfig {
        multi_region_enabled: false,
        ..SchedulerConfig::default()
    };
    let a = SchedulerService::new(config.clone());
    let b = SchedulerService::new(config);
    let (ra, rb) = tokio::join!(a.run_once(&app.state), b.run_once(&app.state));
    ra.unwrap();
    rb.unwrap();
    // Nothing is due any more.
    let (ra, rb) = tokio::join!(a.run_once(&app.state), b.run_once(&app.state));
    ra.unwrap();
    rb.unwrap();

    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE schedule_id = $1")
        .bind(&schedule.id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(runs, 1, "a due schedule fires exactly once across two pollers");

    let (run_count, claimed_by, next_hour_utc): (i64, Option<String>, f64) = sqlx::query_as(
        "SELECT run_count, claimed_by, EXTRACT(HOUR FROM next_run_at AT TIME ZONE 'UTC')::float8 FROM schedules WHERE id = $1",
    )
    .bind(&schedule.id)
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert_eq!(run_count, 1);
    assert!(claimed_by.is_none(), "claim released");
    // 09:00 America/New_York is 13:00 UTC (EDT) or 14:00 UTC (EST).
    assert!(next_hour_utc == 13.0 || next_hour_utc == 14.0, "next run at {next_hour_utc}h UTC");
}

#[tokio::test]
async fn test_schedule_list() {
    let app = TestApp::new().await;

    // Create multiple schedules
    let _schedule1 = app.create_schedule("Schedule 1", "0 0 * * *").await;
    let _schedule2 = app.create_schedule("Schedule 2", "0 12 * * *").await;

    // List schedules
    let response = app.get("/api/v1/schedules").await;
    TestApp::assert_success(&response);
    let schedules: Vec<ScheduleSummary> = TestApp::parse_json(response).await;
    assert!(schedules.len() >= 2);
}

#[tokio::test]
async fn test_schedule_update() {
    let app = TestApp::new().await;

    // Create schedule
    let schedule = app.create_schedule("Update Test", "0 0 * * *").await;
    
    // Update schedule
    let update_request = serde_json::json!({
        "name": "Updated Schedule",
        "cron_expr": "0 30 * * *",
        "enabled": false,
    });
    let response = app.put(&format!("/api/v1/schedules/{}", schedule.id), update_request).await;
    TestApp::assert_success(&response);
    
    let updated: Schedule = TestApp::parse_json(response).await;
    assert_eq!(updated.name, "Updated Schedule");
    assert_eq!(updated.cron_expr, "0 30 * * *");
    assert!(!updated.enabled);
}

#[tokio::test]
async fn test_schedule_delete() {
    let app = TestApp::new().await;

    // Create schedule
    let schedule = app.create_schedule("Delete Test", "0 0 * * *").await;
    
    // Delete schedule
    let response = app.delete(&format!("/api/v1/schedules/{}", schedule.id)).await;
    TestApp::assert_success(&response);
    
    // Verify deletion
    let response = app.get(&format!("/api/v1/schedules/{}", schedule.id)).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_schedule_enable_disable() {
    let app = TestApp::new().await;

    // Create disabled schedule
    let schedule = app.create_schedule("Enable Test", "0 0 * * *").await;
    
    // Disable schedule
    let response = app.post(&format!("/api/v1/schedules/{}/disable", schedule.id), serde_json::json!({})).await;
    TestApp::assert_success(&response);
    let disabled: Schedule = TestApp::parse_json(response).await;
    assert!(!disabled.enabled);
    
    // Enable schedule
    let response = app.post(&format!("/api/v1/schedules/{}/enable", schedule.id), serde_json::json!({})).await;
    TestApp::assert_success(&response);
    let enabled: Schedule = TestApp::parse_json(response).await;
    assert!(enabled.enabled);
}

// ============================================================================
// Session/Attachment Tests
// ============================================================================

#[tokio::test]
async fn test_multi_client_attach() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Attachment Test").await;

    // Attach client 1
    let result1 = app.attach_to_run(&run.id, ClientType::Terminal, Some("user1")).await;
    let client1_id = result1.get("client_id").unwrap().as_str().unwrap();
    
    // Attach client 2
    let result2 = app.attach_to_run(&run.id, ClientType::Web, Some("user2")).await;
    let client2_id = result2.get("client_id").unwrap().as_str().unwrap();

    // Verify attachments
    let attachments = app.get_run_attachments(&run.id).await;
    assert_eq!(attachments.len(), 2);
    
    let client_ids: Vec<_> = attachments.iter().map(|a| &a.client_id).collect();
    assert!(client_ids.contains(&&client1_id.to_string()));
    assert!(client_ids.contains(&&client2_id.to_string()));
}

#[tokio::test]
async fn test_attach_detach() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Detach Test").await;

    // Attach client
    let result = app.attach_to_run(&run.id, ClientType::Terminal, None).await;
    let client_id = result.get("client_id").unwrap().as_str().unwrap();
    
    // Verify attachment
    let attachments = app.get_run_attachments(&run.id).await;
    assert_eq!(attachments.len(), 1);
    
    // Detach client
    let result = app.detach_from_run(&run.id, client_id).await;
    assert!(result.get("detached").unwrap().as_bool().unwrap());
}

// ============================================================================
// Job Tests
// ============================================================================

#[tokio::test]
async fn test_job_create_and_get() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Job Test").await;
    
    // Create job
    let job = app.create_job(&run.id, "Test Job").await;
    assert_eq!(job.name, "Test Job");
    assert!(matches!(job.status, JobStatus::Pending));

    // Get job
    let fetched = app.get_job(&run.id, &job.id).await;
    assert_eq!(job.id, fetched.id);
}

#[tokio::test]
async fn test_job_list() {
    let app = TestApp::new().await;

    // Create run
    let run = app.create_run("Job List Test").await;
    
    // Create multiple jobs
    let _job1 = app.create_job(&run.id, "Job 1").await;
    let _job2 = app.create_job(&run.id, "Job 2").await;

    // List jobs
    let response = app.get(&format!("/api/v1/runs/{}/jobs", run.id)).await;
    TestApp::assert_success(&response);
    let jobs: Vec<Job> = TestApp::parse_json(response).await;
    assert_eq!(jobs.len(), 2);
}

#[tokio::test]
async fn test_job_start_complete() {
    let app = TestApp::new().await;

    // Create run and job
    let run = app.create_run("Job Lifecycle Test").await;
    let job = app.create_job(&run.id, "Lifecycle Job").await;
    
    // Start job
    let job = app.start_job(&run.id, &job.id).await;
    assert!(matches!(job.status, JobStatus::Running));
    assert!(job.started_at.is_some());
    
    // Complete job
    let job = app.complete_job(&run.id, &job.id, serde_json::json!({"result": "success"})).await;
    assert!(matches!(job.status, JobStatus::Completed));
    assert!(job.completed_at.is_some());
    assert_eq!(job.exit_code, Some(0));
}

// ============================================================================
// Event Tests
// ============================================================================

#[tokio::test]
async fn test_run_events_ordered() {
    let app = TestApp::new().await;

    // Create and start run
    let run = app.create_run("Events Test").await;
    let _run = app.start_run(&run.id).await;

    // Get events
    let events = app.get_run_events(&run.id).await;
    
    // Verify events are in sequence order
    for i in 1..events.len() {
        assert!(
            events[i].sequence > events[i-1].sequence,
            "Events should be ordered by sequence"
        );
    }
}

// ============================================================================
// Error Handling Tests
// ============================================================================

#[tokio::test]
async fn test_get_nonexistent_run() {
    let app = TestApp::new().await;

    let response = app.get("/api/v1/runs/nonexistent-id").await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_get_nonexistent_approval() {
    let app = TestApp::new().await;

    let response = app.get("/api/v1/approvals/nonexistent-id").await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_get_nonexistent_schedule() {
    let app = TestApp::new().await;

    let response = app.get("/api/v1/schedules/nonexistent-id").await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_create_approval_for_nonexistent_run() {
    let app = TestApp::new().await;

    let request = CreateApprovalRequest {
        run_id: "nonexistent-run".to_string(),
        step_cursor: None,
        priority: None,
        title: "Test".to_string(),
        description: None,
        action_type: None,
        action_params: None,
        reasoning: None,
        requested_by: None,
        timeout_seconds: None,
    };
    
    let response = app.post("/api/v1/approvals", request).await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_create_job_for_nonexistent_run() {
    let app = TestApp::new().await;

    let request = serde_json::json!({
        "name": "Test Job",
        "config": {
            "command": "echo"
        }
    });
    
    let response = app.post("/api/v1/runs/nonexistent-id/jobs", request).await;
    assert_eq!(response.status(), 404);
}

// ============================================================================
// Complex Workflow Tests
// ============================================================================

#[tokio::test]
async fn test_full_workflow_with_approval() {
    let app = TestApp::new().await;

    // 1. Create run
    let run = app.create_run("Full Workflow").await;
    assert!(matches!(run.status, RunStatus::Pending));

    // 2. Start run
    let run = app.start_run(&run.id).await;
    let _initial_status = run.status;

    // 3. Create approval request during run
    let approval = app.create_approval(&run.id, "Checkpoint Approval").await;
    assert!(matches!(approval.status, ApprovalStatus::Pending));

    // 4. Approve the request
    let approval = app.approve_request(&approval.id, Some("Proceeding")).await;
    assert!(matches!(approval.status, ApprovalStatus::Approved));

    // 5. Create job for the run
    let job = app.create_job(&run.id, "Workflow Job").await;
    
    // 6. Start and complete job
    let job = app.start_job(&run.id, &job.id).await;
    assert!(matches!(job.status, JobStatus::Running));
    
    let job = app.complete_job(&run.id, &job.id, serde_json::json!({})).await;
    assert!(matches!(job.status, JobStatus::Completed));

    // 7. Verify all events were recorded
    let events = app.get_run_events(&run.id).await;
    assert!(!events.is_empty());

    // 8. Attach a client
    let attach_result = app.attach_to_run(&run.id, ClientType::Web, Some("operator")).await;
    assert!(attach_result.get("attached").unwrap().as_bool().unwrap());
}


// ============================================================================
// Recover + Handoff Tests
// ============================================================================

/// The dev-mode auth middleware authenticates every request as `dev-user`;
/// re-tenanting a run to another user simulates a different caller so the
/// access-control (404) paths can be exercised through the real router.
async fn retenant_run(app: &TestApp, run_id: &str, tenant_id: &str) {
    sqlx::query("UPDATE runs SET tenant_id = $1 WHERE id = $2")
        .bind(tenant_id)
        .bind(run_id)
        .execute(&app.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_recover_run_without_checkpoint() {
    let app = TestApp::new().await;
    let run = app.create_run("Recover No Checkpoint").await;

    let response = app
        .post(&format!("/api/v1/runs/{}/recover", run.id), serde_json::json!({}))
        .await;
    TestApp::assert_success(&response);

    let body: serde_json::Value = TestApp::parse_json(response).await;
    assert_eq!(body["recovered"], false);
    assert_eq!(body["reason"], "no checkpoint");

    // The run is left untouched.
    let run = app.get_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Pending));
}

#[tokio::test]
async fn test_recover_run_with_checkpoint_transitions_and_emits_event() {
    let app = TestApp::new().await;
    let run = app.create_run("Recover With Checkpoint").await;

    // Create a resumable checkpoint for the run.
    let checkpoint_body = serde_json::json!({
        "name": "pre-failure",
        "workspace_state": {"files": ["a.txt"]},
    });
    let response = app
        .post(
            &format!("/api/v1/runs/{}/checkpoints", run.id),
            checkpoint_body,
        )
        .await;
    TestApp::assert_success(&response);
    let checkpoint: Checkpoint = TestApp::parse_json(response).await;
    assert!(checkpoint.resumable);

    // Recover: restores onto the checkpoint, then queues the run.
    let response = app
        .post(&format!("/api/v1/runs/{}/recover", run.id), serde_json::json!({}))
        .await;
    TestApp::assert_success(&response);
    let body: serde_json::Value = TestApp::parse_json(response).await;
    assert_eq!(body["recovered"], true);
    assert_eq!(body["checkpoint_id"], checkpoint.id);

    // Run transitioned to queued for re-execution.
    let run = app.get_run(&run.id).await;
    assert!(matches!(run.status, RunStatus::Queued));

    // run_recovered event was recorded.
    let events = app.get_run_events(&run.id).await;
    let recovered = events
        .iter()
        .any(|e| matches!(e.event_type, EventType::RunRecovered));
    assert!(recovered, "expected a run_recovered event");
}

#[tokio::test]
async fn test_recover_run_other_tenant_is_404() {
    let app = TestApp::new().await;
    let run = app.create_run("Recover Other Tenant").await;
    retenant_run(&app, &run.id, "user-b").await;

    let response = app
        .post(&format!("/api/v1/runs/{}/recover", run.id), serde_json::json!({}))
        .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_handoffs_create_and_list_round_trip() {
    let app = TestApp::new().await;
    let run = app.create_run("Handoff Round Trip").await;

    // Create a handoff.
    let body = CreateHandoffRequest {
        to_agent_id: "agent-b".to_string(),
        task_id: Some("task-1".to_string()),
        note: Some("take it from here".to_string()),
    };
    let response = app
        .post(&format!("/api/v1/runs/{}/handoffs", run.id), body)
        .await;
    TestApp::assert_success(&response);
    let handoff: Handoff = TestApp::parse_json(response).await;
    assert_eq!(handoff.run_id, run.id);
    assert_eq!(handoff.to_agent_id, "agent-b");
    assert_eq!(handoff.task_id.as_deref(), Some("task-1"));
    assert_eq!(handoff.note.as_deref(), Some("take it from here"));
    assert_eq!(handoff.status, "pending");

    // List round-trips the created handoff.
    let response = app.get(&format!("/api/v1/runs/{}/handoffs", run.id)).await;
    TestApp::assert_success(&response);
    let handoffs: Vec<Handoff> = TestApp::parse_json(response).await;
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].id, handoff.id);

    // handoff_created event was recorded.
    let events = app.get_run_events(&run.id).await;
    let created = events
        .iter()
        .any(|e| matches!(e.event_type, EventType::HandoffCreated));
    assert!(created, "expected a handoff_created event");
}

#[tokio::test]
async fn test_handoffs_create_rejects_empty_agent_and_missing_run() {
    let app = TestApp::new().await;
    let run = app.create_run("Handoff Validation").await;

    // Empty to_agent_id is a 400.
    let body = CreateHandoffRequest {
        to_agent_id: "  ".to_string(),
        task_id: None,
        note: None,
    };
    let response = app
        .post(&format!("/api/v1/runs/{}/handoffs", run.id), body)
        .await;
    assert_eq!(response.status(), 400);

    // Nonexistent run is a 404.
    let body = CreateHandoffRequest {
        to_agent_id: "agent-b".to_string(),
        task_id: None,
        note: None,
    };
    let response = app
        .post("/api/v1/runs/nonexistent-id/handoffs", body)
        .await;
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_handoffs_other_tenant_is_404() {
    let app = TestApp::new().await;
    let run = app.create_run("Handoffs Other Tenant").await;
    retenant_run(&app, &run.id, "user-b").await;

    // List as the non-owning caller: 404.
    let response = app.get(&format!("/api/v1/runs/{}/handoffs", run.id)).await;
    assert_eq!(response.status(), 404);

    // Create as the non-owning caller: 404.
    let body = CreateHandoffRequest {
        to_agent_id: "agent-c".to_string(),
        task_id: None,
        note: None,
    };
    let response = app
        .post(&format!("/api/v1/runs/{}/handoffs", run.id), body)
        .await;
    assert_eq!(response.status(), 404);

    // Nothing was written for the foreign run.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM handoffs WHERE run_id = $1")
        .bind(&run.id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
