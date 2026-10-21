//! A:// fabric-transport conformance test — the §8.24 adversarial two-worker proof.
//!
//! Scenario: worker A claims lease generation 1, checkpoints mid-job, and dies
//! (heartbeats stop). The sweeper expires the lease; the job requeues. Worker B
//! claims generation 2, replays from A's last committed checkpoint, and
//! completes. A's late completion under generation 1 is rejected as a stale
//! generation; the result commits exactly once; the event ledger preserves the
//! initiator/delegator/executor triple across both lease generations.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use allternit_cowork_runtime::{
    sqlite_store, CompleteOutcome, CoworkEvent, CreateJobSpec, CreateRunSpec, TransportErrorCode,
    JobState, LeaseGrant, RailsClient, RunManager, RunManagerConfig, RunState,
};

const WORKSPACE: &str = "ws-allternit";
const INITIATOR: &str = "a://workspace/allternit/user/joe";
const DELEGATOR: &str = "a://workspace/allternit/principal/al";
const WORKER_A: &str = "a://workspace/allternit/bot/worker-a";
const WORKER_B: &str = "a://workspace/allternit/bot/worker-b";
const SHELL_CAPS: &[&str] = &["shell.exec"];

/// Mock Rails client — the fabric-transport store does not need a Rails backend.
#[derive(Default, Clone)]
struct MockRailsClient {
    inner: Arc<Mutex<Vec<CoworkEvent>>>,
}

#[async_trait::async_trait]
impl RailsClient for MockRailsClient {
    async fn create_dag(
        &self,
        _run_id: allternit_cowork_runtime::RunId,
        _spec: &CreateRunSpec,
    ) -> anyhow::Result<String> {
        Ok("dag-transport-test".to_string())
    }
    async fn create_node(
        &self,
        _dag_id: &str,
        job_id: allternit_cowork_runtime::JobId,
        _spec: &CreateJobSpec,
    ) -> anyhow::Result<String> {
        Ok(format!("node-{job_id}"))
    }
    async fn update_run_state(&self, _d: &str, _s: RunState) -> anyhow::Result<()> {
        Ok(())
    }
    async fn update_job_state(&self, _n: &str, _s: JobState) -> anyhow::Result<()> {
        Ok(())
    }
    async fn request_lease(&self, _r: &str, _o: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    async fn release_lease(&self, _r: &str, _o: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn append_event(&self, event: &CoworkEvent) -> anyhow::Result<()> {
        self.inner.lock().unwrap().push(event.clone());
        Ok(())
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    db_path: std::path::PathBuf,
    manager: Arc<RunManager>,
    run_id: String,
    job_id: String,
}

fn open(db_path: &std::path::Path) -> rusqlite::Connection {
    sqlite_store::open_store(db_path).expect("open store")
}

fn event_rows(conn: &rusqlite::Connection, run_id: &str) -> Vec<(String, String, String, String)> {
    // (event_type, initiator, delegator, executor) in commit order.
    conn.prepare(
        "SELECT event_type, COALESCE(initiator,''), COALESCE(delegator,''), COALESCE(executor,'')
         FROM cowork_run_events WHERE run_id = ?1 ORDER BY rowid ASC",
    )
    .unwrap()
    .query_map(rusqlite::params![run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn job_state(conn: &rusqlite::Connection, job_id: &str) -> String {
    conn.query_row(
        "SELECT state FROM cowork_jobs WHERE id = ?1",
        rusqlite::params![job_id],
        |row| row.get(0),
    )
    .unwrap()
}

async fn setup() -> Fixture {
    setup_with_payload(serde_json::json!({
        "steps": ["echo step-0", "echo step-1", "echo step-2"],
        "checkpoint_every": 1,
    }))
    .await
}

async fn setup_with_payload(payload: serde_json::Value) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("transport.db");
    {
        let mut conn = open(&db_path);
        sqlite_store::apply_store_ddl(&mut conn).unwrap();
        sqlite_store::register_principal(&mut conn, WORKER_A, WORKSPACE, &caps(), &["worker".to_string()], "token-a")
            .unwrap();
        sqlite_store::register_principal(&mut conn, WORKER_B, WORKSPACE, &caps(), &["worker".to_string()], "token-b")
            .unwrap();
        // A principal without shell.exec — must be filtered by eligibility.
        sqlite_store::register_principal(&mut conn, "a://bot/no-shell", WORKSPACE, &[], &["worker".to_string()], "token-c")
            .unwrap();

        let run_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO cowork_runs
                (id, tenant_id, workspace_id, initiator, delegator, mode, state,
                 entrypoint, dag_id, policy_profile)
             VALUES (?1, 'tenant-1', ?2, ?3, ?4, 'cowork', 'running', 'proof-slice', 'dag-x', 'default')",
            rusqlite::params![run_id, WORKSPACE, INITIATOR, DELEGATOR],
        )
        .unwrap();
        let job_id = sqlite_store::enqueue_job(
            &mut conn,
            &run_id,
            "shell_steps",
            payload,
            &["shell.exec".to_string()],
            60,
            2,
            Some(INITIATOR),
            Some(DELEGATOR),
        )
        .unwrap();
        drop(conn);

        let config = RunManagerConfig {
            data_dir: tmp.path().join("runtime"),
            rails_base_url: "http://127.0.0.1:9".to_string(), // unroutable: local-only checkpoints
            attachment_timeout_secs: 300,
            lease_duration_secs: 2,
            max_checkpoint_age_hours: 24,
            store_path: Some(db_path.clone()),
            lease_sweep_interval_secs: 1,
        };
        let (manager, _events) =
            RunManager::new(config, Arc::new(MockRailsClient::default()))
                .await
                .unwrap();

        Fixture {
            _tmp: tmp,
            db_path,
            manager: Arc::new(manager),
            run_id,
            job_id,
        }
    }
}

fn caps() -> Vec<String> {
    SHELL_CAPS.iter().map(|s| s.to_string()).collect()
}

fn auth(conn: &rusqlite::Connection, token: &str) -> allternit_cowork_runtime::PrincipalRecord {
    sqlite_store::authenticate_principal(conn, token).expect("authenticate")
}

/// Adversarial §8.24 proof: A claims gen 1, dies, B claims gen 2, A's stale
/// completion is rejected, exactly-once result, full attribution triple.
#[tokio::test]
async fn test_adversarial_two_worker_failover() {
    let fx = setup().await;
    let lease_ttl = Duration::from_secs(2);
    let mut conn = open(&fx.db_path);

    // Eligibility: a principal without shell.exec can never claim this job
    // (checked while the job is still queued — a leased job is simply
    // unavailable to everyone else, whatever their capabilities).
    let no_shell = auth(&conn, "token-c");
    let ineligible = sqlite_store::claim_job(&mut conn, &no_shell, Some(&fx.job_id), lease_ttl)
        .expect_err("missing capability must be rejected");
    assert_eq!(ineligible.code, TransportErrorCode::CapabilityMissing);

    // ── 6–7. Worker A authenticates and claims lease generation 1 ──────────
    let a = auth(&conn, "token-a");
    let lease_a: LeaseGrant = sqlite_store::claim_job(&mut conn, &a, Some(&fx.job_id), lease_ttl)
        .expect("worker A claims");
    assert_eq!(lease_a.lease_generation, 1);
    assert!(lease_a.lease_id.starts_with("lease_"));
    assert_eq!(job_state(&conn, &fx.job_id), "leased");
    assert_eq!(lease_a.initiator.as_deref(), Some(INITIATOR));
    assert_eq!(lease_a.delegator.as_deref(), Some(DELEGATOR));

    // Concurrent conflict: B cannot take an already-leased job.
    let b = auth(&conn, "token-b");
    let conflict = sqlite_store::claim_job(&mut conn, &b, Some(&fx.job_id), lease_ttl)
        .expect_err("B must lose the claim race");
    assert_eq!(conflict.code, TransportErrorCode::JobAlreadyLeased);

    // ── 8. A begins execution: heartbeat, renew, then a committed checkpoint ──
    sqlite_store::record_heartbeat(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        lease_a.lease_generation,
        Some("2026-09-12T00:00:00Z".to_string()), // advisory only
    )
    .expect("heartbeat");
    let wrong_gen = sqlite_store::record_heartbeat(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        99,
        None,
    )
    .expect_err("wrong generation must be rejected");
    assert_eq!(wrong_gen.code, TransportErrorCode::StaleLeaseGeneration);

    let new_expiry = sqlite_store::renew_lease(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        lease_a.lease_generation,
        lease_ttl,
    )
    .expect("renew");
    assert!(new_expiry > lease_a.lease_expires_at);

    // A executes step 0 and commits a checkpoint (portable data, lock 3)
    // through the runtime's own checkpoint manager.
    let run_id = allternit_cowork_runtime::RunId(uuid::Uuid::parse_str(&fx.run_id).unwrap());
    let cp = fx
        .manager
        .checkpoint(
            run_id,
            None,
            0,
            serde_json::json!({ "completed_steps": 1, "outputs": ["step-0 done"] }),
        )
        .await
        .expect("checkpoint");
    conn.execute(
        "UPDATE cowork_runs SET current_checkpoint_id = ?1 WHERE id = ?2",
        rusqlite::params![cp.id, fx.run_id],
    )
    .unwrap();
    drop(conn);

    // ── 13–15. A is SIGKILLed: heartbeats stop, lease generation 1 expires ──
    // (No more heartbeats from A. The RunManager sweeper — a real background
    // task on the store — must expire the lease within a few ticks.)
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let conn = open(&fx.db_path);
        if job_state(&conn, &fx.job_id) == "queued" {
            let (retry, lease_id): (i64, Option<String>) = conn
                .query_row(
                    "SELECT retry_count, lease_id FROM cowork_jobs WHERE id = ?1",
                    rusqlite::params![fx.job_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(retry, 1, "requeue increments retry_count");
            assert!(lease_id.is_none(), "expired lease is cleared");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "sweeper never requeued the job"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Ghost heartbeats from A are dead on arrival.
    let mut conn = open(&fx.db_path);
    let ghost_hb = sqlite_store::record_heartbeat(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        lease_a.lease_generation,
        None,
    )
    .expect_err("ghost heartbeat must be rejected");
    assert!(matches!(
        ghost_hb.code,
        TransportErrorCode::InvalidLease | TransportErrorCode::LeaseExpired
    ));

    // ── 18–19. Worker B authenticates and claims lease generation 2 ─────────
    let lease_b = sqlite_store::claim_job(&mut conn, &b, Some(&fx.job_id), lease_ttl)
        .expect("worker B claims after requeue");
    assert_eq!(lease_b.lease_generation, 2, "ownership change bumps generation");
    assert_ne!(lease_b.lease_id, lease_a.lease_id);

    // ── Recovery (lock 3): B's grant carries A's last committed checkpoint ──
    assert_eq!(
        lease_b.current_checkpoint_id.as_deref(),
        Some(cp.id.as_str()),
        "B must replay from A's last committed checkpoint"
    );
    let latest_cps = fx.manager.list_checkpoints(run_id).await.unwrap();
    let latest = latest_cps.iter().max_by_key(|c| c.step_index).unwrap();
    let completed_steps = latest.cursor_state["completed_steps"].as_i64().unwrap();
    assert_eq!(completed_steps, 1, "checkpoint data is portable across generations");

    // ── The §8.24 assertion: A cannot complete under generation 1 ──────────
    let stale = sqlite_store::complete_job(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        lease_a.lease_generation, // generation 1 — stale
        true,
        Some("A finished late".to_string()),
        None,
    )
    .expect_err("killed worker's completion must be rejected");
    assert_eq!(
        stale.code,
        TransportErrorCode::StaleLeaseGeneration,
        "late completion under an old generation is A_STALE_LEASE_GENERATION"
    );
    assert_eq!(job_state(&conn, &fx.job_id), "leased", "rejection must not disturb B's lease");

    // A renewal under generation 1 is equally dead.
    let stale_renew = sqlite_store::renew_lease(
        &mut conn,
        &a,
        &fx.job_id,
        &lease_a.lease_id,
        1,
        lease_ttl,
    )
    .expect_err("stale renew must be rejected");
    assert_eq!(stale_renew.code, TransportErrorCode::StaleLeaseGeneration);

    // ── 21–22. B replays the remaining steps and completes — exactly once ──
    let outcome = sqlite_store::complete_job(
        &mut conn,
        &b,
        &fx.job_id,
        &lease_b.lease_id,
        lease_b.lease_generation,
        true,
        Some("replayed from checkpoint, steps 1-2 done".to_string()),
        Some(serde_json::json!({ "steps_replayed": 2 })),
    )
    .expect("B completes");
    let (committed_state, result_id): (String, String) = match outcome {
        CompleteOutcome::Committed { job_state, result } => {
            assert_eq!(job_state, "completed");
            (job_state, result["result_id"].as_str().unwrap().to_string())
        }
        CompleteOutcome::AlreadyCommitted { .. } => panic!("first completion must commit"),
    };

    // Repeat completion (B, correct generation) returns the canonical result.
    let dup = sqlite_store::complete_job(
        &mut conn,
        &b,
        &fx.job_id,
        &lease_b.lease_id,
        lease_b.lease_generation,
        true,
        Some("duplicate".to_string()),
        None,
    )
    .expect("duplicate completion is idempotent");
    match dup {
        CompleteOutcome::AlreadyCommitted { job_state, result } => {
            assert_eq!(job_state, committed_state);
            assert_eq!(
                result["result_id"].as_str().unwrap(),
                result_id,
                "exactly-once: canonical result is returned, no new side effects"
            );
        }
        CompleteOutcome::Committed { .. } => panic!("duplicate completion must not re-commit"),
    }

    // Run advanced to its terminal state; job has no active lease left.
    let run_state: String = conn
        .query_row(
            "SELECT state FROM cowork_runs WHERE id = ?1",
            rusqlite::params![fx.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_state, "completed");
    assert_eq!(job_state(&conn, &fx.job_id), "completed");

    // ── 25. Ledger attribution: initiator=user, delegator=Al, executor=A→B ──
    let rows = event_rows(&conn, &fx.run_id);
    let claimed_a = rows
        .iter()
        .filter(|r| r.0 == "job.claimed" && r.3 == WORKER_A)
        .count();
    let claimed_b = rows
        .iter()
        .filter(|r| r.0 == "job.claimed" && r.3 == WORKER_B)
        .count();
    assert_eq!(claimed_a, 1, "exactly one claim event for A");
    assert_eq!(claimed_b, 1, "exactly one claim event for B");

    for (etype, initiator, delegator, executor) in &rows {
        // Every transport event carries the full triple context.
        assert_eq!(initiator, INITIATOR, "{etype}: initiator attribution");
        assert_eq!(delegator, DELEGATOR, "{etype}: delegator attribution");
        match etype.as_str() {
            "job.claimed" | "job.heartbeat" | "job.lease_renewed" | "job.completed"
            | "result.created" | "run.completed" => {
                assert!(!executor.is_empty(), "{etype}: executor attribution");
            }
            "job.lease_expired" => assert_eq!(executor, WORKER_A),
            "job.requeued" => assert!(executor.is_empty(), "requeue is a fabric-transport action"),
            _ => {}
        }
    }

    assert_eq!(
        rows.iter().filter(|r| r.0 == "result.created").count(),
        1,
        "exactly-once result in the ledger"
    );
    assert_eq!(rows.iter().filter(|r| r.0 == "job.completed").count(), 1);
    assert!(rows.iter().any(|r| r.0 == "run.completed"));
}

/// Dead-letter: recovery policy exhausts after max_retries. Each expiry
/// requeues the job; a new lease must be claimed (and die) to expire again.
#[tokio::test]
async fn test_lease_exhaustion_dead_letters() {
    let fx = setup().await;

    // max_retries = 2: three claim-and-die cycles → dead_letter.
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        let state = {
            let mut conn = open(&fx.db_path);
            if job_state(&conn, &fx.job_id) == "queued" {
                let a = auth(&conn, "token-a");
                sqlite_store::claim_job(
                    &mut conn,
                    &a,
                    Some(&fx.job_id),
                    Duration::from_secs(1),
                )
                .expect("re-claim between deaths");
            }
            job_state(&conn, &fx.job_id)
        };
        if state == "dead_letter" {
            break;
        }
        assert_ne!(state, "completed");
        assert!(
            std::time::Instant::now() < deadline,
            "job never dead-lettered (state={state})"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let conn = open(&fx.db_path);
    let rows = event_rows(&conn, &fx.run_id);
    assert_eq!(rows.iter().filter(|r| r.0 == "job.lease_expired").count(), 3);
    assert_eq!(rows.iter().filter(|r| r.0 == "job.requeued").count(), 2);
    assert_eq!(rows.iter().filter(|r| r.0 == "job.dead_lettered").count(), 1);
}

/// Concurrent claims: persistence-level CAS gives exactly one winner.
#[test]
fn test_concurrent_claims_single_winner() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cc.db");
    {
        let mut conn = open(&db_path);
        sqlite_store::apply_store_ddl(&mut conn).unwrap();
        sqlite_store::register_principal(&mut conn, "w", WORKSPACE, &["shell.exec".to_string()], &["worker".to_string()], "t")
            .unwrap();
        conn.execute(
            "INSERT INTO cowork_runs
                (id, tenant_id, workspace_id, initiator, mode, state, entrypoint, dag_id, policy_profile)
             VALUES ('r1', 't', ?1, ?2, 'cowork', 'running', 'x', 'd', 'default')",
            rusqlite::params![WORKSPACE, INITIATOR],
        )
        .unwrap();
    }
    let job_id = {
        let mut conn = open(&db_path);
        sqlite_store::enqueue_job(
            &mut conn,
            "r1",
            "shell_steps",
            serde_json::json!({"steps": ["echo hi"]}),
            &["shell.exec".to_string()],
            60,
            0,
            Some(INITIATOR),
            Some(DELEGATOR),
        )
        .unwrap()
    };

    let winners: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let db_path = db_path.clone();
        let job_id = job_id.clone();
        let winners = winners.clone();
        handles.push(std::thread::spawn(move || {
            let mut conn = open(&db_path);
            let p = auth(&conn, "t");
            match sqlite_store::claim_job(&mut conn, &p, Some(&job_id), Duration::from_secs(60)) {
                Ok(_) => *winners.lock().unwrap() += 1,
                Err(e) => assert_eq!(e.code, TransportErrorCode::JobAlreadyLeased),
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(*winners.lock().unwrap(), 1, "exactly one concurrent winner");
}

const PROTECTED_CAPABILITY: &str = "connector.bank.payment.submit";
const PROTECTED_TARGET: &str = "payment/123";

async fn wait_until_queued(db_path: &std::path::Path, job_id: &str) {
    // tokio::time::sleep (not std::thread::sleep): the test runtime is
    // single-threaded and must yield for the sweeper task to run.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        {
            let conn = open(db_path);
            if job_state(&conn, job_id) == "queued" {
                return;
            }
        }
        assert!(std::time::Instant::now() < deadline, "sweeper never requeued");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The FULL §8.24 proof-of-protocol sequence, including the approval steps:
/// protected action → approval request → user grants → execution resumes →
/// worker killed → approval bound to generation 1 invalidated → the
/// generation-2 worker must re-obtain approval → completes exactly once with
/// correct attribution.
#[tokio::test]
async fn test_full_824_sequence_with_approval() {
    let fx = setup_with_payload(serde_json::json!({
        "steps": ["echo step-0", "echo pay", "echo step-2"],
        "checkpoint_every": 1,
        "protected": { "step": 1, "capability": PROTECTED_CAPABILITY, "target": PROTECTED_TARGET },
    }))
    .await;
    let lease_ttl = Duration::from_secs(2);
    let mut conn = open(&fx.db_path);
    let a = auth(&conn, "token-a");
    let b = auth(&conn, "token-b");
    let run_id_uuid =
        allternit_cowork_runtime::RunId(uuid::Uuid::parse_str(&fx.run_id).unwrap());

    // 5–7. A authenticates, claims lease generation 1, begins execution.
    let lease_a = sqlite_store::claim_job(&mut conn, &a, Some(&fx.job_id), lease_ttl)
        .expect("A claims gen 1");
    assert_eq!(lease_a.lease_generation, 1);

    // 8. A executes step 0 and checkpoints.
    let cp0 = fx
        .manager
        .checkpoint(run_id_uuid, None, 0, serde_json::json!({ "completed_steps": 1 }))
        .await
        .unwrap();
    conn.execute(
        "UPDATE cowork_runs SET current_checkpoint_id = ?1 WHERE id = ?2",
        rusqlite::params![cp0.id, fx.run_id],
    )
    .unwrap();

    // 9–10. Step 1 is a protected action: no approval on file → required.
    let required = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect_err("protected action without approval");
    assert_eq!(required.code, TransportErrorCode::ApprovalRequired);

    let binding1 = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        PROTECTED_CAPABILITY, PROTECTED_TARGET, None,
    )
    .expect("A requests approval");
    assert_eq!(binding1.status, "pending");
    assert_eq!(binding1.lease_generation, 1);
    assert_eq!(binding1.executor, WORKER_A);

    // Pending approval still blocks execution.
    let pending = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect_err("pending approval does not authorize");
    assert_eq!(pending.code, TransportErrorCode::ApprovalRequired);

    // 11. The user grants (approval bound to generation 1).
    let granted1 = sqlite_store::decide_approval(&mut conn, &binding1.id, true, INITIATOR)
        .expect("user grants");
    assert_eq!(granted1.status, "granted");
    sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect("granted approval authorizes");

    // 12. Execution resumes; A runs the protected step and checkpoints.
    let cp1 = fx
        .manager
        .checkpoint(run_id_uuid, None, 1, serde_json::json!({ "completed_steps": 2, "paid": true }))
        .await
        .unwrap();
    conn.execute(
        "UPDATE cowork_runs SET current_checkpoint_id = ?1 WHERE id = ?2",
        rusqlite::params![cp1.id, fx.run_id],
    )
    .unwrap();
    drop(conn);

    // 13–15. A is killed before completion. Heartbeats stop; gen 1 expires.
    wait_until_queued(&fx.db_path, &fx.job_id).await;
    let mut conn = open(&fx.db_path);

    // 16. The approval bound to generation 1 is invalidated by expiry.
    let b1 = sqlite_store::get_approval(&conn, &binding1.id).unwrap().unwrap();
    assert_eq!(b1.status, "invalidated", "lease expiry invalidates bound approvals");

    // The invalidated approval must not authorize anything, even from A.
    let stale_check = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect_err("invalidated approval cannot authorize");
    // The ghost's lease is dead (job requeued, lease cleared) — anything it
    // presents is invalid before approval state even matters.
    assert!(matches!(
        stale_check.code,
        TransportErrorCode::ApprovalInvalid
            | TransportErrorCode::LeaseExpired
            | TransportErrorCode::StaleLeaseGeneration
            | TransportErrorCode::InvalidLease
    ));

    // 18–19. B authenticates and claims lease generation 2.
    let lease_b = sqlite_store::claim_job(&mut conn, &b, Some(&fx.job_id), lease_ttl)
        .expect("B claims gen 2");
    assert_eq!(lease_b.lease_generation, 2);
    assert_eq!(
        lease_b.current_checkpoint_id.as_deref(),
        Some(cp1.id.as_str()),
        "B replays from A's last committed checkpoint"
    );

    // 20. The protected action is re-evaluated: the gen-1 approval is invalid.
    let invalid = sqlite_store::check_approval(
        &mut conn, &b, &fx.job_id, &lease_b.lease_id, 2,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect_err("stale-generation approval must be rejected");
    assert_eq!(invalid.code, TransportErrorCode::ApprovalInvalid);

    // B re-obtains approval under generation 2; the user grants again.
    let binding2 = sqlite_store::request_approval(
        &mut conn, &b, &fx.job_id, &lease_b.lease_id, 2,
        PROTECTED_CAPABILITY, PROTECTED_TARGET, None,
    )
    .expect("B re-requests approval");
    assert_eq!(binding2.lease_generation, 2);
    assert_ne!(binding2.id, binding1.id);
    sqlite_store::decide_approval(&mut conn, &binding2.id, true, INITIATOR).unwrap();
    sqlite_store::check_approval(
        &mut conn, &b, &fx.job_id, &lease_b.lease_id, 2,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect("gen-2 approval authorizes");

    // The killed worker still cannot complete under generation 1.
    let stale = sqlite_store::complete_job(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, 1,
        true, Some("A finished late".to_string()), None,
    )
    .expect_err("killed worker's completion rejected");
    assert_eq!(stale.code, TransportErrorCode::StaleLeaseGeneration);

    // 21–23. B completes; the result commits exactly once; the run completes.
    let outcome = sqlite_store::complete_job(
        &mut conn, &b, &fx.job_id, &lease_b.lease_id, 2,
        true, Some("re-approval under gen 2, steps replayed".to_string()),
        Some(serde_json::json!({ "paid": true, "reapproved": true })),
    )
    .expect("B completes");
    let result_id = match outcome {
        CompleteOutcome::Committed { result, .. } => {
            result["result_id"].as_str().unwrap().to_string()
        }
        CompleteOutcome::AlreadyCommitted { .. } => panic!("first completion must commit"),
    };
    let dup = sqlite_store::complete_job(
        &mut conn, &b, &fx.job_id, &lease_b.lease_id, 2,
        true, Some("duplicate".to_string()), None,
    )
    .unwrap();
    match dup {
        CompleteOutcome::AlreadyCommitted { result, .. } => {
            assert_eq!(result["result_id"].as_str().unwrap(), result_id);
        }
        _ => panic!("duplicate must return the canonical result"),
    }

    // 25. Ledger: attribution triple + the full approval event story.
    let rows = event_rows(&conn, &fx.run_id);
    let count = |t: &str| rows.iter().filter(|r| r.0 == t).count();
    assert_eq!(count("approval.requested"), 2);
    assert_eq!(count("approval.granted"), 2);
    assert_eq!(count("approval.invalidated"), 1);
    assert_eq!(count("result.created"), 1);
    assert_eq!(count("job.completed"), 1);
    assert!(rows.iter().any(|r| r.0 == "run.completed"));

    let invalidated_row = rows.iter().find(|r| r.0 == "approval.invalidated").unwrap();
    assert_eq!(invalidated_row.3, WORKER_A, "invalidated approval attributes its executor");

    let requested_a = rows.iter().filter(|r| r.0 == "approval.requested" && r.3 == WORKER_A).count();
    let requested_b = rows.iter().filter(|r| r.0 == "approval.requested" && r.3 == WORKER_B).count();
    assert_eq!(requested_a, 1);
    assert_eq!(requested_b, 1);

    for (etype, initiator, delegator, executor) in &rows {
        assert_eq!(initiator, INITIATOR, "{etype}: initiator");
        assert_eq!(delegator, DELEGATOR, "{etype}: delegator");
        assert!(
            !executor.is_empty() || etype == &"job.requeued",
            "{etype}: executor attribution"
        );
    }
}

/// §8.20/§8.21: leases that expire during downtime are recovered at boot by
/// the server clock — requeued per policy, never silently lost — and queued
/// and leased jobs rehydrate into the runtime mirror.
#[tokio::test]
async fn test_downtime_expiry_recovers_at_boot() {
    // No manager/sweeper: simulate a server that goes down while a lease is
    // held, and stays down past the lease expiry.
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("downtime.db");
    let (run_id, job_id) = {
        let mut conn = open(&db_path);
        sqlite_store::apply_store_ddl(&mut conn).unwrap();
        sqlite_store::register_principal(&mut conn, WORKER_A, WORKSPACE, &caps(), &["worker".to_string()], "token-a")
            .unwrap();
        let run_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO cowork_runs
                (id, tenant_id, workspace_id, initiator, delegator, mode, state,
                 entrypoint, dag_id, policy_profile)
             VALUES (?1, 'tenant-1', ?2, ?3, ?4, 'cowork', 'running', 'x', 'd', 'default')",
            rusqlite::params![run_id, WORKSPACE, INITIATOR, DELEGATOR],
        )
        .unwrap();
        let job_id = sqlite_store::enqueue_job(
            &mut conn, &run_id, "shell_steps",
            serde_json::json!({"steps": ["echo hi"]}),
            &["shell.exec".to_string()], 60, 2, Some(INITIATOR), Some(DELEGATOR),
        )
        .unwrap();
        // Worker claims with a 2s lease, then the server dies.
        let a = auth(&conn, "token-a");
        sqlite_store::claim_job(&mut conn, &a, Some(&job_id), Duration::from_secs(2)).unwrap();
        assert_eq!(job_state(&conn, &job_id), "leased");
        (run_id, job_id)
    };

    // "Downtime" passes (well past lease expiry). Boot: expire, then rehydrate.
    std::thread::sleep(Duration::from_secs(3));
    let actions = {
        let mut conn = open(&db_path);
        let actions = sqlite_store::expire_leases(&mut conn, chrono::Utc::now()).unwrap();
        assert_eq!(actions.len(), 1, "downtime-expired lease is recovered at boot");
        assert_eq!(actions[0].outcome, "queued");
        assert_eq!(job_state(&conn, &job_id), "queued");
        actions
    };

    // The rehydrated view a fresh manager would load: job is queued and
    // claimable by a replacement worker under a new generation.
    let mut conn = open(&db_path);
    let b = sqlite_store::authenticate_principal(&conn, "token-a").unwrap();
    let grant = sqlite_store::claim_job(&mut conn, &b, Some(&job_id), Duration::from_secs(60))
        .expect("requeued job claimable after boot recovery");
    assert!(grant.lease_generation > actions[0].lease_generation);
    let _ = run_id;
}

/// (a) Approval timeout: a request that expires un-acted is swept to
/// `expired` with an attributed event; a late grant is rejected; the worker
/// must re-request (recovery policy = re-request, deny-by-default).
#[tokio::test]
async fn test_approval_expiry_and_late_grant_rejected() {
    let fx = setup().await;
    let lease_ttl = Duration::from_secs(60);
    let mut conn = open(&fx.db_path);
    let a = auth(&conn, "token-a");
    let lease_a = sqlite_store::claim_job(&mut conn, &a, Some(&fx.job_id), lease_ttl).unwrap();

    let binding = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, lease_a.lease_generation,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
        Some(Duration::from_secs(1)),
    )
    .unwrap();
    assert!(binding.expires_at.is_some());

    // Not yet expired: the human decision works.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let late = sqlite_store::decide_approval(&mut conn, &binding.id, true, INITIATOR)
        .expect_err("late grant must be rejected (server clock)");
    assert_eq!(late.code, TransportErrorCode::ApprovalInvalid);

    // Sweeper (or this direct pass) marks it expired with an attributed
    // event. The fixture's background sweeper may have already done it.
    let _ = sqlite_store::expire_approvals(&mut conn, chrono::Utc::now()).unwrap();
    let view = sqlite_store::get_approval(&conn, &binding.id).unwrap().unwrap();
    assert_eq!(view.status, "expired");
    let rows = event_rows(&conn, &fx.run_id);
    let expired_row = rows.iter().find(|r| r.0 == "approval.expired").unwrap();
    assert_eq!(expired_row.3, WORKER_A, "approval.expired attributes its executor");

    // Execution awaiting the expired approval is told to re-request.
    let check = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, lease_a.lease_generation,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect_err("expired approval cannot authorize");
    assert_eq!(check.code, TransportErrorCode::ApprovalRequired);

    // Re-request under the same (still valid) lease succeeds.
    let rebinding = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, lease_a.lease_generation,
        PROTECTED_CAPABILITY, PROTECTED_TARGET, None,
    )
    .unwrap();
    assert_ne!(rebinding.id, binding.id);
    sqlite_store::decide_approval(&mut conn, &rebinding.id, true, INITIATOR).unwrap();
    sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lease_a.lease_id, lease_a.lease_generation,
        PROTECTED_CAPABILITY, PROTECTED_TARGET,
    )
    .expect("re-requested approval authorizes");
}

/// (b) Event idempotency: a client-supplied event_id dedupes retries — the
/// duplicate returns the canonical existing event instead of double-writing.
#[test]
fn test_event_post_idempotency() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("events.db");
    let run_id = {
        let mut conn = open(&db_path);
        sqlite_store::apply_store_ddl(&mut conn).unwrap();
        let run_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO cowork_runs
                (id, tenant_id, workspace_id, initiator, mode, state, entrypoint, dag_id, policy_profile)
             VALUES (?1, 't', ?2, ?3, 'cowork', 'running', 'x', 'd', 'default')",
            rusqlite::params![run_id, WORKSPACE, INITIATOR],
        )
        .unwrap();
        run_id
    };

    let mut conn = open(&db_path);
    let first = sqlite_store::insert_event_idempotent(
        &mut conn, &run_id, "custom.event", serde_json::json!({"n": 1}),
        Some(INITIATOR), Some(DELEGATOR), None, Some("evt-retry-key-1"),
    )
    .unwrap();
    let first_id = match first {
        sqlite_store::EventInsertOutcome::Inserted(id) => id,
        _ => panic!("first insert must commit"),
    };

    // Retry with the same key (at-least-once delivery) → canonical existing.
    let dup = sqlite_store::insert_event_idempotent(
        &mut conn, &run_id, "custom.event", serde_json::json!({"n": 1}),
        Some(INITIATOR), Some(DELEGATOR), None, Some("evt-retry-key-1"),
    )
    .unwrap();
    match dup {
        sqlite_store::EventInsertOutcome::Duplicate(id) => assert_eq!(id, first_id),
        _ => panic!("retry must dedupe"),
    }

    // A different key writes a second row; a None key always writes.
    sqlite_store::insert_event_idempotent(
        &mut conn, &run_id, "custom.event", serde_json::json!({"n": 2}),
        None, None, None, Some("evt-retry-key-2"),
    )
    .unwrap();
    sqlite_store::insert_event_idempotent(
        &mut conn, &run_id, "custom.other", serde_json::json!({}),
        None, None, None, None,
    )
    .unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cowork_run_events WHERE run_id = ?1",
            rusqlite::params![run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "exactly-once per idempotency key");
}

/// (c) Single policy path: risk rules decide whether a protected action needs
/// an approval at all (mirrors the cowork-engine ApprovalGate model);
/// bindings scope the approval when one is required.
#[tokio::test]
async fn test_risk_policy_single_evaluation_path() {
    let fx = setup().await;
    let lease_ttl = Duration::from_secs(60);
    let mut conn = open(&fx.db_path);
    let a = auth(&conn, "token-a");
    let lease_a = sqlite_store::claim_job(&mut conn, &a, Some(&fx.job_id), lease_ttl).unwrap();
    let gen = lease_a.lease_generation;
    let lid = lease_a.lease_id.clone();

    // Low-risk capability → auto-approved by the default rule, no binding row.
    let auto = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lid, gen, "web.read", "https://example.com",
    )
    .expect("low-risk auto-approve");
    assert_eq!(auto.status, "granted");
    assert!(auto.id.starts_with("auto:approve:"));
    assert!(auto.decided_by.as_deref().unwrap().contains("risk-rule"));

    // Medium/high/critical → binding required.
    let medium = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lid, gen, "shell.exec", "any",
    )
    .expect_err("medium-risk requires approval");
    assert_eq!(medium.code, TransportErrorCode::ApprovalRequired);
    let critical = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lid, gen,
        PROTECTED_CAPABILITY, PROTECTED_TARGET, None,
    )
    .expect("critical-risk creates a binding");
    assert!(critical.id.starts_with("appr_"));

    // Workspace policy override: auto-deny shell.exec, downgrade web.read risk.
    use allternit_cowork_runtime::{ApprovalPolicy, RiskRule};
    let policy = ApprovalPolicy {
        capability_risk: std::collections::HashMap::from([
            ("shell.exec".to_string(), "critical".to_string()),
        ]),
        rules: vec![
            RiskRule {
                action_type: Some("shell.exec.".to_string()),
                risk_level: vec![],
                decision: "reject".to_string(),
                reason: Some("workspace forbids shell".to_string()),
            },
            RiskRule {
                action_type: Some("web.read".to_string()),
                risk_level: vec!["low".to_string()],
                decision: "approve".to_string(),
                reason: Some("reading allowed".to_string()),
            },
        ],
    };
    sqlite_store::set_policy(&mut conn, WORKSPACE, &policy).unwrap();

    // Request path: auto-deny is rejected AND ledgered (check is read-only
    // and deliberately writes nothing, so polling cannot flood the ledger).
    let denied = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lid, gen, "shell.exec.cleanup", "any", None,
    )
    .expect_err("workspace policy auto-denies shell");
    assert_eq!(denied.code, TransportErrorCode::PermissionDenied);
    let rows = event_rows(&conn, &fx.run_id);
    let denied_row = rows.iter().find(|r| r.0 == "approval.denied").unwrap();
    assert_eq!(denied_row.3, WORKER_A, "auto-deny attributes its executor");
    // Auto-approve via the REQUEST path is granted and ledgered once.
    let auto_req = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lid, gen, "web.read", "https://example.com", None,
    )
    .expect("policy auto-approves via request");
    assert_eq!(auto_req.status, "granted");
    let rows = event_rows(&conn, &fx.run_id);
    assert_eq!(
        rows.iter().filter(|r| r.0 == "approval.granted").count(),
        1,
        "auto-approve via request is ledgered exactly once"
    );

    let still_auto = sqlite_store::check_approval(
        &mut conn, &a, &fx.job_id, &lid, gen, "web.read", "https://example.com",
    )
    .expect("policy still auto-approves web.read");
    assert_eq!(still_auto.status, "granted");
}

/// Gap 1 — default principals: Al and Gizzi are minted per workspace,
/// idempotently, without credentials; tokens are provisioned exactly once.
#[tokio::test]
async fn test_default_principal_seeding_and_token_provisioning() {
    let fx = setup().await; // fixture creates a ws-allternit run row
    let mut conn = open(&fx.db_path);

    let first = sqlite_store::seed_default_principals(&mut conn).unwrap();
    assert!(first.contains(&format!("a://workspace/{WORKSPACE}/principal/al")));
    assert!(first.contains(&format!("a://workspace/{WORKSPACE}/principal/gizzi")));

    // Idempotent: second pass touches the same rows without duplicating.
    let second = sqlite_store::seed_default_principals(&mut conn).unwrap();
    assert_eq!(second.len(), first.len());
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cowork_principals WHERE id LIKE '%principal/al'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "exactly one Al per workspace");

    // Seeded principals have roles and (for Gizzi) capabilities, no token.
    let al = conn
        .query_row(
            "SELECT roles, token_hash FROM cowork_principals WHERE id = ?1",
            rusqlite::params![format!("a://workspace/{WORKSPACE}/principal/al")],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .unwrap();
    assert!(al.0.contains("orchestrator"));
    assert!(al.1.is_none(), "minting never carries a credential");

    // Token provisioned exactly once; rotation kills the old token.
    let gizzi_id = format!("a://workspace/{WORKSPACE}/principal/gizzi");
    let token1 = sqlite_store::provision_principal_token(&mut conn, &gizzi_id).unwrap();
    let g = auth(&conn, &token1);
    assert_eq!(g.id, gizzi_id);
    assert!(g.roles.contains(&"worker".to_string()));
    assert!(!g.capabilities.is_empty(), "Gizzi carries its capability set");

    let token2 = sqlite_store::provision_principal_token(&mut conn, &gizzi_id).unwrap();
    assert_ne!(token1, token2);
    let old_dead = sqlite_store::authenticate_principal(&conn, &token1);
    assert!(old_dead.is_err(), "rotated token is dead");
    auth(&conn, &token2);
}

/// Gap 3 — delegation chains: cycles and depth are rejected (§8.15), depth
/// is workspace-configurable (default 4, reducible).
#[tokio::test]
async fn test_delegation_chain_cycle_and_depth() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);

    // Valid chain of 4 (at the default limit) passes.
    let ok_chain: Vec<String> = ["u", "al", "research", "browser"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    sqlite_store::validate_delegation_chain(&conn, WORKSPACE, &ok_chain).unwrap();

    // Depth 5 exceeds the default of 4.
    let deep: Vec<String> = (0..5).map(|i| format!("p{i}")).collect();
    let depth_err = sqlite_store::validate_delegation_chain(&conn, WORKSPACE, &deep)
        .expect_err("depth over limit rejected");
    assert_eq!(depth_err.code, TransportErrorCode::DelegationDepthExceeded);

    // Cycle A→B→A rejected.
    let cyc = vec!["a".to_string(), "b".to_string(), "a".to_string()];
    let cycle_err = sqlite_store::validate_delegation_chain(&conn, WORKSPACE, &cyc)
        .expect_err("cycle rejected");
    assert_eq!(cycle_err.code, TransportErrorCode::DelegationCycle);

    // Workspace policy may reduce the limit.
    use allternit_cowork_runtime::{ApprovalPolicy, RiskRule};
    let mut policy = ApprovalPolicy::default();
    policy.rules.push(RiskRule {
        action_type: None,
        risk_level: vec![],
        decision: "require".to_string(),
        reason: None,
    });
    // set depth via a dedicated policy write (column rides along with the policy upsert)
    conn.execute(
        "INSERT INTO cowork_approval_policy (workspace, max_delegation_depth)
         VALUES (?1, 2) ON CONFLICT(workspace) DO UPDATE SET max_delegation_depth = 2",
        rusqlite::params![WORKSPACE],
    )
    .unwrap();
    let three: Vec<String> = (0..3).map(|i| format!("p{i}")).collect();
    let reduced = sqlite_store::validate_delegation_chain(&conn, WORKSPACE, &three)
        .expect_err("reduced workspace limit enforced");
    assert_eq!(reduced.code, TransportErrorCode::DelegationDepthExceeded);
    sqlite_store::validate_delegation_chain(&conn, WORKSPACE, &ok_chain[0..2]).unwrap();
}

/// Gap 4 — canonical intents: idempotent on intent_id (§5), validated
/// (version, chain), and resolve to a canonical queued run.
#[tokio::test]
async fn test_intent_submission_idempotent() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);

    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: "intent_demo_001".to_string(),
        workspace: format!("a://workspace/{WORKSPACE}"),
        initiator: INITIATOR.to_string(),
        delegator: Some(DELEGATOR.to_string()),
        target: Some(WORKER_A.to_string()),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "shell_steps".to_string(),
            description: "demo intent".to_string(),
            payload: Some(serde_json::json!({"steps": ["echo hi"]})),
        },
        permissions: vec!["shell.exec".to_string()],
        compute: None,
        model: None,
        approval: None,
        return_channel: Some(serde_json::json!({"channel": "cowork"})),
        causation_chain: vec![INITIATOR.to_string(), DELEGATOR.to_string(), WORKER_A.to_string()],
    };

    let first = sqlite_store::submit_intent(&mut conn, &envelope).unwrap();
    assert!(first.created);
    assert_eq!(job_state(&conn, &fx.job_id), "queued", "pre-existing job untouched");

    // Same intent_id → same canonical run, created=false (§5 idempotency).
    let replay = sqlite_store::submit_intent(&mut conn, &envelope).unwrap();
    assert!(!replay.created);
    assert_eq!(replay.run_id, first.run_id);

    // The run row carries the attribution triple and the chain.
    let run: (String, Option<String>, String) = conn
        .query_row(
            "SELECT state, delegator, causation_chain FROM cowork_runs WHERE id = ?1",
            rusqlite::params![first.run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(run.0, "queued", "§8.2 honesty: intents queue, leases make running");
    assert_eq!(run.1.as_deref(), Some(DELEGATOR));
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&run.2).unwrap(),
        vec![INITIATOR.to_string(), DELEGATOR.to_string(), WORKER_A.to_string()]
    );

    // Observe path round-trips the envelope.
    let view = sqlite_store::get_intent(&conn, "intent_demo_001").unwrap().unwrap();
    assert_eq!(view["run_id"], first.run_id);
    assert_eq!(view["envelope"]["action"]["action_type"], "shell_steps");

    // intent.accepted event is attributed.
    let rows = event_rows(&conn, &first.run_id);
    let accepted = rows.iter().find(|r| r.0 == "intent.accepted").unwrap();
    assert_eq!(accepted.1, INITIATOR);
    assert_eq!(accepted.2, DELEGATOR);
    assert_eq!(accepted.3, WORKER_A);

    // Bad version and cyclic chains are rejected.
    let mut bad_version = envelope.clone();
    bad_version.intent_id = "intent_demo_002".to_string();
    bad_version.version = "a/9.9".to_string();
    assert!(sqlite_store::submit_intent(&mut conn, &bad_version).is_err());

    let mut cyclic = envelope.clone();
    cyclic.intent_id = "intent_demo_003".to_string();
    cyclic.causation_chain = vec!["x".to_string(), "y".to_string(), "x".to_string()];
    let err = sqlite_store::submit_intent(&mut conn, &cyclic).expect_err("cycle rejected");
    assert_eq!(err.code, TransportErrorCode::DelegationCycle);
}

/// A-T1 — handoffs carry validated chains and can be acknowledged: the ack
/// completes the linked handoff job with a typed result and attribution.
#[tokio::test]
async fn test_handoff_ack_completes_linked_job() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);
    let job_id = sqlite_store::enqueue_job(
        &mut conn, &fx.run_id, "handoff",
        serde_json::json!({"to_agent_id": WORKER_B}),
        &[], 60, 0, Some(INITIATOR), Some(DELEGATOR),
    ).unwrap();
    let handoff_id = format!("ho_{}", uuid::Uuid::new_v4());
    let chain = vec![INITIATOR.to_string(), DELEGATOR.to_string(), WORKER_A.to_string()];
    conn.execute(
        "INSERT INTO cowork_handoffs (id, run_id, to_agent_id, status, job_id, causation_chain)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5)",
        rusqlite::params![handoff_id, fx.run_id, WORKER_B, job_id,
            serde_json::to_string(&chain).unwrap()],
    ).unwrap();

    let outcome = sqlite_store::ack_handoff(&mut conn, &handoff_id, WORKER_B, Some("took it".to_string()))
        .expect("ack succeeds");
    assert_eq!(outcome["status"], "completed");

    // Idempotent replay.
    let dup = sqlite_store::ack_handoff(&mut conn, &handoff_id, WORKER_B, None).unwrap();
    assert_eq!(dup["duplicated"], true);

    let job: (String, String) = conn
        .query_row(
            "SELECT state, result FROM cowork_jobs WHERE id = ?1",
            rusqlite::params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(job.0, "completed", "handoff job terminated by ack");
    let result: serde_json::Value = serde_json::from_str(&job.1).unwrap();
    assert_eq!(result["executor"], WORKER_B);

    let rows = event_rows(&conn, &fx.run_id);
    let completed = rows.iter().find(|r| r.0 == "handoff.completed").unwrap();
    assert_eq!(completed.3, WORKER_B, "ack executor attribution");
}

/// A-T2 — per-principal memory grants: default-deny cross-principal unless
/// explicitly granted; owner and grantee see their entries.
#[tokio::test]
async fn test_memory_principal_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("mem.db");
    let mut conn = open(&db_path);
    sqlite_store::apply_store_ddl(&mut conn).unwrap();

    // Al writes an entry, grants Gizzi.
    let al_entry = sqlite_store::store_memory_entry(
        &mut conn, "user-1", None, None, "Al's private note", "fact",
        None, None, Some("a://ws/principal/al"), &["a://ws/principal/gizzi".to_string()],
    ).unwrap();
    // Gizzi writes its own.
    let gizzi_entry = sqlite_store::store_memory_entry(
        &mut conn, "user-1", None, None, "Gizzi's build note", "fact",
        None, None, Some("a://ws/principal/gizzi"), &[],
    ).unwrap();
    // Legacy unowned entry.
    sqlite_store::store_memory_entry(
        &mut conn, "user-1", None, None, "legacy user note", "fact",
        None, None, None, &[],
    ).unwrap();

    // Al sees: own + unowned (NOT gizzi's).
    let al_view = sqlite_store::search_memory_entries(
        &conn, "user-1", Some("a://ws/principal/al"), None, 50, 0).unwrap();
    let al_ids: Vec<String> = al_view.iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    assert!(al_ids.contains(&al_entry));
    assert!(!al_ids.contains(&gizzi_entry), "default-deny: Al cannot read Gizzi's entry");

    // Gizzi sees: own + granted + unowned.
    let g_view = sqlite_store::search_memory_entries(
        &conn, "user-1", Some("a://ws/principal/gizzi"), None, 50, 0).unwrap();
    let g_ids: Vec<String> = g_view.iter().map(|e| e["id"].as_str().unwrap().to_string()).collect();
    assert!(g_ids.contains(&gizzi_entry));
    assert!(g_ids.contains(&al_entry), "granted principal can read Al's entry");

    // Write check: a stranger has no grant on Al's entry.
    let denied = sqlite_store::check_memory_write(&conn, &al_entry, "a://ws/bot/stranger")
        .expect_err("default-deny cross-principal write");
    assert_eq!(denied.code, TransportErrorCode::PermissionDenied);
    sqlite_store::check_memory_write(&conn, &al_entry, "a://ws/principal/gizzi")
        .expect("granted principal may write");
}

/// A-T2 pagination: offset skips N entries within the principal-filtered,
/// recency-ordered window, and limit + offset compose for page two.
#[tokio::test]
async fn test_memory_principal_search_offset_pagination() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("mem.db");
    let mut conn = open(&db_path);
    sqlite_store::apply_store_ddl(&mut conn).unwrap();

    // Five unowned entries; one cross-principal entry that must never leak
    // into the owner's pages.
    let mut expected = Vec::new();
    for i in 0..5 {
        let id = sqlite_store::store_memory_entry(
            &mut conn, "user-1", None, None,
            &format!("note {i}"), "fact", None, None, None, &[],
        ).unwrap();
        expected.push(id);
    }
    // created_at only has second resolution — backdate explicitly so the
    // recency ordering the pagination asserts against is deterministic.
    for (i, id) in expected.iter().enumerate() {
        conn.execute(
            "UPDATE cowork_memory_entries SET created_at = ?2 WHERE id = ?1",
            rusqlite::params![id, format!("2026-09-13 12:00:0{i}")],
        ).unwrap();
    }
    let private_id = sqlite_store::store_memory_entry(
        &mut conn, "user-1", None, None, "al private", "fact",
        None, None, Some("a://ws/principal/al"), &[],
    ).unwrap();

    let principal = Some("a://ws/principal/gizzi");
    let ids = |limit: i64, offset: i64| -> Vec<String> {
        sqlite_store::search_memory_entries(&conn, "user-1", principal, None, limit, offset)
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    };

    // Recency order is newest-first, so page 1 is the reverse of insertion.
    let mut newest_first = expected.clone();
    newest_first.reverse();
    assert_eq!(ids(50, 0), newest_first, "offset 0 returns the full window");
    assert_eq!(ids(2, 0), newest_first[0..2], "limit caps page 1");
    assert_eq!(ids(2, 2), newest_first[2..4], "limit + offset compose for page 2");
    assert_eq!(ids(2, 4), newest_first[4..5], "offset skips N entries");
    assert!(ids(2, 5).is_empty(), "offset at the window end is empty");
    // Cross-principal entry stays default-deny at every page.
    let all_pages: Vec<String> = (0..5).flat_map(|o| ids(1, o)).collect();
    assert!(!all_pages.contains(&private_id));
}

/// A-T3 — Al orchestration loop: an intent targeted at Al is delegated per
/// the workspace rules, monitored, and the result recorded on the parent.
#[tokio::test]
async fn test_al_orchestration_loop() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);
    // V166 default rule shape: shell* → gizzi.
    conn.execute(
        "INSERT OR IGNORE INTO cowork_delegation_rules (workspace, action_type, target_principal, priority)
         VALUES (?1, 'shell', 'gizzi', 100)",
        rusqlite::params![WORKSPACE],
    ).unwrap();

    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: "intent_orch_001".to_string(),
        workspace: format!("a://workspace/{WORKSPACE}"),
        initiator: INITIATOR.to_string(),
        delegator: None,
        target: Some(format!("a://workspace/{WORKSPACE}/principal/al")),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "shell_steps".to_string(),
            description: "orchestrate this".to_string(),
            payload: None,
        },
        permissions: vec![],
        compute: None, model: None, approval: None, return_channel: None,
        causation_chain: vec![INITIATOR.to_string()],
    };
    let parent = sqlite_store::submit_intent(&mut conn, &envelope).unwrap();

    // Orchestrator tick: delegates to Gizzi per the rule.
    let actions = sqlite_store::orchestrate_pending_intents(&mut conn).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].outcome, "delegated");
    assert!(actions[0].detail.ends_with("/principal/gizzi"));

    // Second tick: nothing new (already delegated).
    let again = sqlite_store::orchestrate_pending_intents(&mut conn).unwrap();
    assert!(again.is_empty(), "no duplicate delegation");

    let parent_row: (Option<String>, String) = conn
        .query_row(
            "SELECT child_run_id, orchestration_status FROM cowork_intents WHERE intent_id = 'intent_orch_001'",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
    assert_eq!(parent_row.1, "delegated");
    let child_run = parent_row.0.unwrap();

    // delegation.created is attributed: delegator Al, executor Gizzi.
    // (Orchestration events live on the intent's run, not the fixture's.)
    let rows = event_rows(&conn, &parent.run_id);
    let created = rows.iter().find(|r| r.0 == "delegation.created").unwrap();
    assert_eq!(created.2, format!("a://workspace/{WORKSPACE}/principal/al"));
    assert!(created.3.ends_with("/principal/gizzi"));

    // Child run completes → orchestrator records the result on the parent.
    conn.execute(
        "UPDATE cowork_runs SET state = 'completed', completed_at = ?1 WHERE id = ?2",
        rusqlite::params![chrono::Utc::now().to_rfc3339(), child_run],
    ).unwrap();
    let recorded = sqlite_store::record_orchestration_results(&mut conn).unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].outcome, "completed");

    let parent_state: String = conn
        .query_row("SELECT state FROM cowork_runs WHERE id = ?1", rusqlite::params![parent.run_id], |row| row.get(0))
        .unwrap();
    assert_eq!(parent_state, "completed", "parent run mirrors the child terminal state");
    let orch: String = conn
        .query_row("SELECT orchestration_status FROM cowork_intents WHERE intent_id = 'intent_orch_001'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(orch, "completed");

    // Rejection path: no matching rule.
    let mut no_rule = envelope.clone();
    no_rule.intent_id = "intent_orch_002".to_string();
    no_rule.action.action_type = "quantum_fold".to_string();
    let rejected_sub = sqlite_store::submit_intent(&mut conn, &no_rule).unwrap();
    let rejected = sqlite_store::orchestrate_pending_intents(&mut conn).unwrap();
    assert_eq!(rejected[0].outcome, "rejected");
    let rows = event_rows(&conn, &rejected_sub.run_id);
    assert!(rows.iter().any(|r| r.0 == "delegation.rejected"));
}

/// Seam defects (Part 1) — intent-created runs must be owner-stamped,
/// short-alias Al targets must orchestrate identically to canonical ones,
/// and store-direct runs must load back through the manager-mirror shape.
#[tokio::test]
async fn test_intent_created_run_is_owner_stamped() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);
    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: "intent_owner_001".to_string(),
        workspace: format!("a://workspace/{WORKSPACE}"),
        initiator: INITIATOR.to_string(),
        delegator: None,
        target: Some(format!("a://workspace/{WORKSPACE}/principal/gizzi")),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "shell_steps".to_string(),
            description: "owner stamping".to_string(),
            payload: None,
        },
        permissions: vec![],
        compute: None, model: None, approval: None, return_channel: None,
        causation_chain: vec![INITIATOR.to_string()],
    };
    let submission = sqlite_store::submit_intent_for_user(&mut conn, &envelope, Some("user-joe"))
        .unwrap();
    assert!(submission.created);

    // The run carries the owner: V142/V169 scoping (list_runs,
    // ensure_run_owner) must see intent-created runs like any other.
    let (run_user, ): (Option<String>,) = conn
        .query_row(
            "SELECT user_id FROM cowork_runs WHERE id = ?1",
            rusqlite::params![submission.run_id],
            |row| Ok((row.get(0)?,)),
        )
        .unwrap();
    assert_eq!(run_user.as_deref(), Some("user-joe"));

    // The auto-created canonical job is owner-stamped too.
    let job_user: Option<String> = conn
        .query_row(
            "SELECT user_id FROM cowork_jobs WHERE run_id = ?1",
            rusqlite::params![submission.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(job_user.as_deref(), Some("user-joe"));

    // The bare submit_intent entry point keeps working (no owner).
    let bare = sqlite_store::submit_intent(&mut conn, &envelope).unwrap();
    assert!(!bare.created, "idempotent on intent_id");
}

#[tokio::test]
async fn test_orchestrator_accepts_short_alias_target() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);
    conn.execute(
        "INSERT OR IGNORE INTO cowork_delegation_rules (workspace, action_type, target_principal, priority)
         VALUES (?1, 'shell', 'gizzi', 100)",
        rusqlite::params![WORKSPACE],
    ).unwrap();

    // A_PROTOCOL §5 short alias for the Al principal.
    let mut alias = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: "intent_orch_alias".to_string(),
        workspace: format!("a://workspace/{WORKSPACE}"),
        initiator: INITIATOR.to_string(),
        delegator: None,
        target: Some("principal/al".to_string()),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "shell_alias".to_string(),
            description: "short alias probe".to_string(),
            payload: None,
        },
        permissions: vec![],
        compute: None, model: None, approval: None, return_channel: None,
        causation_chain: vec![INITIATOR.to_string()],
    };
    let submission = sqlite_store::submit_intent(&mut conn, &alias).unwrap();

    // The alias names Al, so the parent gets NO claimable job (the
    // orchestrator's child carries the executable work).
    let parent_jobs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cowork_jobs WHERE run_id = ?1",
            rusqlite::params![submission.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_jobs, 0, "Al-targeted parent must not enqueue a job");

    // One tick delegates it exactly like the canonical long form.
    let actions = sqlite_store::orchestrate_pending_intents(&mut conn).unwrap();
    let delegated = actions.iter().find(|a| a.intent_id == "intent_orch_alias");
    let delegated = delegated.expect("short-alias intent must be orchestrated");
    assert_eq!(delegated.outcome, "delegated");
    assert!(delegated.child_run_id.is_some());

    // targets_al covers both forms.
    assert!(sqlite_store::targets_al(Some("principal/al")));
    assert!(sqlite_store::targets_al(Some(&format!("a://workspace/{WORKSPACE}/principal/al"))));
    assert!(!sqlite_store::targets_al(Some(&format!("a://workspace/{WORKSPACE}/principal/gizzi"))));
    assert!(!sqlite_store::targets_al(None));

    // Idempotency guard: the alias intent must not double-delegate.
    alias.intent_id = "intent_orch_alias_2".to_string();
    sqlite_store::submit_intent(&mut conn, &alias).unwrap();
    let _ = delegated;
    let second = sqlite_store::orchestrate_pending_intents(&mut conn).unwrap();
    assert_eq!(
        second.iter().filter(|a| a.outcome == "delegated").count(),
        1,
        "exactly the new intent delegates"
    );
}

#[tokio::test]
async fn test_load_run_record_roundtrip() {
    let fx = setup().await;
    let mut conn = open(&fx.db_path);
    let envelope = allternit_cowork_runtime::IntentEnvelope {
        version: "a/0.1".to_string(),
        intent_id: "intent_mirror_001".to_string(),
        workspace: format!("a://workspace/{WORKSPACE}"),
        initiator: INITIATOR.to_string(),
        delegator: None,
        target: Some(format!("a://workspace/{WORKSPACE}/principal/gizzi")),
        action: allternit_cowork_runtime::IntentAction {
            action_type: "shell_steps".to_string(),
            description: "mirror probe".to_string(),
            payload: None,
        },
        permissions: vec![],
        compute: None, model: None, approval: None, return_channel: None,
        causation_chain: vec![INITIATOR.to_string()],
    };
    let submission = sqlite_store::submit_intent(&mut conn, &envelope).unwrap();

    let run = sqlite_store::load_run_record(&conn, &submission.run_id)
        .unwrap()
        .expect("run loads back for the manager mirror");
    assert_eq!(run.id.to_string(), submission.run_id);
    assert_eq!(run.workspace_id, WORKSPACE);
    assert_eq!(run.initiator, INITIATOR);
    assert_eq!(run.state, RunState::Queued);
    assert!(sqlite_store::load_run_record(&conn, "not-a-uuid").unwrap().is_none());
    assert!(sqlite_store::load_run_record(&conn, &uuid::Uuid::new_v4().to_string())
        .unwrap()
        .is_none());
}

/// A-T5 — connector broker: sessions are lease+policy validated, secrets
/// never leave the server, and the system-side invoke is attributed (and
/// honest when the env target is unset: simulated, not silent).
#[tokio::test]
async fn test_connector_broker_sessions() {
    let fx = setup().await;
    let lease_ttl = Duration::from_secs(60);
    let mut conn = open(&fx.db_path);
    let a = auth(&conn, "token-a");
    let lease = sqlite_store::claim_job(&mut conn, &a, Some(&fx.job_id), lease_ttl).unwrap();

    // The reference capability is critical-risk (.send) — grant approval for
    // the current generation first (the realistic broker flow).
    let binding = sqlite_store::request_approval(
        &mut conn, &a, &fx.job_id, &lease.lease_id, lease.lease_generation,
        "connector.webhook.send", "*", None,
    ).unwrap();
    sqlite_store::decide_approval(&mut conn, &binding.id, true, INITIATOR).unwrap();

    // Reference capability (seeded): session issued, no secret in the response.
    let session = sqlite_store::request_connector_session(
        &mut conn, &a, &fx.job_id, &lease.lease_id, lease.lease_generation,
        "connector.webhook.send", Some(Duration::from_secs(60)),
    ).unwrap();
    assert!(session["session_id"].as_str().unwrap().starts_with("cs_"));
    assert!(session.get("secret").is_none() && session.get("secret_env").is_none(),
        "raw secrets never reach the worker");

    // Invoke: env unset in tests → simulated, delivered:false, attributed.
    let outcome = sqlite_store::invoke_connector_session(
        &mut conn, &a, &fx.job_id, &lease.lease_id, lease.lease_generation,
        session["session_id"].as_str().unwrap(),
        serde_json::json!({"event": "demo"}),
    ).await.unwrap();
    assert_eq!(outcome["simulated"], true);
    assert_eq!(outcome["delivered"], false);

    let rows = event_rows(&conn, &fx.run_id);
    let invoked = rows.iter().find(|r| r.0 == "connector.invoked").unwrap();
    assert_eq!(invoked.1, INITIATOR);
    assert_eq!(invoked.3, WORKER_A, "broker invocation attributes the executor");

    // Another principal cannot invoke the session.
    let b = auth(&conn, "token-b");
    let stolen = sqlite_store::invoke_connector_session(
        &mut conn, &b, &fx.job_id, &lease.lease_id, lease.lease_generation,
        session["session_id"].as_str().unwrap(), serde_json::json!({}),
    ).await;
    assert!(stolen.is_err(), "session is principal-bound");

    // Protected capability requires an approval binding first.
    conn.execute(
        "INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.bank.payment.submit', 'ALLTERNIT_BROKER_BANK_URL')",
        [],
    ).unwrap();
    let blocked = sqlite_store::request_connector_session(
        &mut conn, &a, &fx.job_id, &lease.lease_id, lease.lease_generation,
        "connector.bank.payment.submit", None,
    ).expect_err("critical capability needs approval");
    assert_eq!(blocked.code, TransportErrorCode::ApprovalRequired);
}

/// A bot's Memory tab can forget an entry; only the owning user's rows go.
#[tokio::test]
async fn test_memory_delete_is_user_scoped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut conn = open(&tmp.path().join("mem-del.db"));
    sqlite_store::apply_store_ddl(&mut conn).unwrap();
    let mine = sqlite_store::store_memory_entry(
        &mut conn, "user-1", None, None, "Ledger never moves money", "fact",
        None, None, Some("a://local/bot/ledger"), &[],
    ).unwrap();
    assert!(!sqlite_store::delete_memory_entry(&conn, "user-2", &mine).unwrap(), "another user can't forget it");
    assert!(sqlite_store::delete_memory_entry(&conn, "user-1", &mine).unwrap());
    assert!(!sqlite_store::delete_memory_entry(&conn, "user-1", &mine).unwrap(), "already gone");
    let left = sqlite_store::search_memory_entries(&conn, "user-1", Some("a://local/bot/ledger"), None, 50, 0).unwrap();
    assert!(left.is_empty());
}
