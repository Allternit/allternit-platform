//! One test suite for both `SelfHostStore` implementations.
//!
//! Every test runs against SQLite (in memory). With `SELFHOST_TEST_PG_URL`
//! set (e.g. `postgres://postgres:postgres@127.0.0.1:5432/postgres` on a
//! `postgres:16` container), each test also runs against PostgreSQL in its
//! own fresh schema.

use std::collections::HashSet;
use std::sync::Arc;

use allternit_api::selfhost::store::pg::PgStore;
use allternit_api::selfhost::store::sqlite::SqliteStore;
use allternit_api::selfhost::store::*;
use chrono::{Duration, Utc};
use serde_json::json;

fn sqlite() -> DynStore {
    Arc::new(SqliteStore::open_memory().expect("open sqlite"))
}

async fn postgres() -> Option<DynStore> {
    let url = std::env::var("SELFHOST_TEST_PG_URL").ok().filter(|u| !u.is_empty())?;
    let schema = format!("selfhost_t_{}", uuid::Uuid::new_v4().simple());
    let store = PgStore::connect_in_schema(&url, 60, &schema)
        .await
        .expect("connect to SELFHOST_TEST_PG_URL");
    store.migrate().await.expect("migrate");
    // Migrations are idempotent.
    store.migrate().await.expect("migrate twice");
    Some(Arc::new(store))
}

macro_rules! store_suite {
    ($($name:ident),* $(,)?) => {
        mod on_sqlite {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    super::$name(super::sqlite()).await;
                }
            )*
        }
        mod on_postgres {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    let Some(store) = super::postgres().await else {
                        eprintln!("SELFHOST_TEST_PG_URL not set; Postgres run skipped");
                        return;
                    };
                    super::$name(store).await;
                }
            )*
        }
    };
}

store_suite!(
    agents_crud_pages_and_in_use,
    environments_keys_and_delete,
    session_status_cas_and_archive,
    event_seq_is_gapless_under_concurrency,
    claim_fencing_and_secrets,
    expired_leases_are_reclaimed,
    tool_calls_are_at_least_once_and_idempotent,
    api_keys_first_key_and_last_admin,
    workers_and_queue_depth,
    credentials_roundtrip,
    concurrent_claims_never_double_claim,
    ensure_open_work_makes_one_row_under_concurrency,
    affinity_prefers_the_last_worker_then_expires,
    credential_cas_loses_to_a_concurrent_write,
    idempotency_begin_race_has_one_winner_and_replays,
    orphaned_turns_fail_exactly_once,
    replicas_sweep_lock_and_wakes,
);

async fn agent(store: &DynStore, name: &str) -> AgentRecord {
    store
        .create_agent(NewAgent {
            name: name.into(),
            model: "claude-sonnet".into(),
            ..Default::default()
        })
        .await
        .unwrap()
}

async fn env(store: &DynStore, name: &str) -> EnvironmentRecord {
    store
        .create_environment(NewEnvironment {
            name: name.into(),
            network: "none".into(),
            ..Default::default()
        })
        .await
        .unwrap()
}

async fn session(store: &DynStore) -> (AgentRecord, EnvironmentRecord, SessionRecord) {
    let a = agent(store, "a").await;
    let e = env(store, "e").await;
    let s = store
        .create_session(NewSession {
            agent_id: a.id.clone(),
            environment_id: e.id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    (a, e, s)
}

/// `n` more sessions on the same agent and environment.
async fn more_sessions(
    store: &DynStore,
    a: &AgentRecord,
    e: &EnvironmentRecord,
    n: usize,
) -> Vec<SessionRecord> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(
            store
                .create_session(NewSession {
                    agent_id: a.id.clone(),
                    environment_id: e.id.clone(),
                    ..Default::default()
                })
                .await
                .unwrap(),
        );
    }
    out
}

fn claim_req(env_id: &str, worker: &str, now: chrono::DateTime<Utc>) -> ClaimRequest {
    ClaimRequest {
        environment_id: env_id.into(),
        worker_id: worker.into(),
        lease_cutoff_offset_ms: None,
        hostname: None,
        version: None,
        now,
    }
}

async fn agents_crud_pages_and_in_use(store: DynStore) {
    let a1 = agent(&store, "one").await;
    let a2 = agent(&store, "two").await;
    let a3 = agent(&store, "three").await;
    assert_eq!(a1.version, 1);
    assert_eq!(a1.tools, json!([]));
    assert_eq!(a1.metadata, json!({}));

    // Oldest first, two pages.
    let p1 = store.list_agents(2, None).await.unwrap();
    assert_eq!(p1.iter().map(|a| &a.id).collect::<Vec<_>>(), vec![&a1.id, &a2.id]);
    let p2 = store.list_agents(2, Some(a2.id.as_str())).await.unwrap();
    assert_eq!(p2.iter().map(|a| &a.id).collect::<Vec<_>>(), vec![&a3.id]);
    assert!(matches!(
        store.list_agents(2, Some("agent_nope")).await,
        Err(StoreError::InvalidCursor)
    ));

    let up = store
        .update_agent(
            &a1.id,
            AgentPatch {
                name: Some("renamed".into()),
                tools: Some(json!([{"type": "builtin", "name": "bash"}])),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(up.name, "renamed");
    assert_eq!(up.model, "claude-sonnet", "absent fields stay");
    assert_eq!(up.version, 2);

    // In use while a session is not archived; the session keeps the old version.
    let e = env(&store, "e").await;
    let s = store
        .create_session(NewSession {
            agent_id: a1.id.clone(),
            environment_id: e.id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(s.agent_snapshot["version"], json!(2));
    store
        .update_agent(&a1.id, AgentPatch { name: Some("again".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(store.get_session(&s.id).await.unwrap().agent_snapshot["name"], json!("renamed"));
    assert!(matches!(
        store.delete_agent(&a1.id).await,
        Err(StoreError::Conflict("agent_in_use"))
    ));
    store.archive_session(&s.id).await.unwrap();
    store.delete_agent(&a1.id).await.unwrap();
    assert!(matches!(store.get_agent(&a1.id).await, Err(StoreError::NotFound)));
    assert!(matches!(store.delete_agent(&a1.id).await, Err(StoreError::NotFound)));
    assert_eq!(store.list_agents(10, None).await.unwrap().len(), 2);
    // Archived sessions stay readable after their agent is gone.
    assert_eq!(store.get_session(&s.id).await.unwrap().status, "archived");
}

async fn environments_keys_and_delete(store: DynStore) {
    let e = env(&store, "e").await;
    assert_eq!(e.network, "none");
    let up = store
        .update_environment(
            &e.id,
            EnvironmentPatch {
                network: Some("limited".into()),
                allowed_hosts: Some(vec!["pypi.org".into()]),
                image: Some(Some("python:3.12".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(up.allowed_hosts, vec!["pypi.org".to_string()]);
    assert_eq!(up.image.as_deref(), Some("python:3.12"));
    let cleared = store
        .update_environment(&e.id, EnvironmentPatch { image: Some(None), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(cleared.image, None);
    assert_eq!(cleared.network, "limited", "absent fields stay");
    assert!(matches!(
        store
            .update_environment(&e.id, EnvironmentPatch { network: Some("open".into()), ..Default::default() })
            .await,
        Err(StoreError::Invalid(_))
    ));

    let secret = new_secret("aek");
    let k = store
        .create_environment_key(NewEnvironmentKey {
            environment_id: e.id.clone(),
            name: Some("w".into()),
            prefix: secret[..12].into(),
            key_hash: hash_secret(&secret),
            expires_at: None,
        })
        .await
        .unwrap();
    let found = store.find_environment_key_by_hash(&hash_secret(&secret)).await.unwrap().unwrap();
    assert_eq!(found.id, k.id);
    assert!(found.is_active(Utc::now()));
    assert_eq!(store.list_environment_keys(&e.id, 10, None).await.unwrap().len(), 1);

    // Delete revokes keys.
    store.delete_environment(&e.id).await.unwrap();
    let after = store.find_environment_key_by_hash(&hash_secret(&secret)).await.unwrap().unwrap();
    assert!(after.revoked_at.is_some());
    assert!(matches!(store.get_environment(&e.id).await, Err(StoreError::NotFound)));

    // In use while a live session exists.
    let (_, e2, s) = session(&store).await;
    assert!(matches!(
        store.delete_environment(&e2.id).await,
        Err(StoreError::Conflict("environment_in_use"))
    ));
    store.archive_session(&s.id).await.unwrap();
    store.delete_environment(&e2.id).await.unwrap();
}

async fn session_status_cas_and_archive(store: DynStore) {
    let (_, _, s) = session(&store).await;
    assert_eq!(s.status, "idle");
    assert_eq!(s.last_seq, 0);
    let moved = store
        .transition_session_status(&s.id, &["idle"], "running", None)
        .await
        .unwrap();
    assert_eq!(moved.unwrap().status, "running");
    // A second start from idle loses.
    assert!(store
        .transition_session_status(&s.id, &["idle"], "running", None)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        store.transition_session_status("sess_missing", &["idle"], "running", None).await,
        Err(StoreError::NotFound)
    ));
    let w = store
        .set_session_status(&s.id, "waiting_for_worker", Some("no_worker"))
        .await
        .unwrap();
    assert_eq!(w.status_reason.as_deref(), Some("no_worker"));
    assert!(matches!(
        store.set_session_status(&s.id, "sleeping", None).await,
        Err(StoreError::Invalid(_))
    ));
    store.set_gizzi_session_id(&s.id, "gz_123").await.unwrap();
    let a = store.archive_session(&s.id).await.unwrap();
    assert_eq!(a.status, "archived");
    assert_eq!(a.gizzi_session_id.as_deref(), Some("gz_123"));
    let again = store.archive_session(&s.id).await.unwrap();
    assert_eq!(again.archived_at, a.archived_at, "archive is a one-time change");

    let hidden = store.list_sessions(&SessionFilter::default(), 10, None).await.unwrap();
    assert!(hidden.iter().all(|x| x.id != s.id));
    let shown = store
        .list_sessions(&SessionFilter { include_archived: true, ..Default::default() }, 10, None)
        .await
        .unwrap();
    assert!(shown.iter().any(|x| x.id == s.id));
}

async fn event_seq_is_gapless_under_concurrency(store: DynStore) {
    let (_, _, s) = session(&store).await;
    let mut tasks = Vec::new();
    for i in 0..40 {
        let store = store.clone();
        let sid = s.id.clone();
        let kind = if i % 2 == 0 { "message" } else { "status" };
        tasks.push(tokio::spawn(async move {
            store.append_event(&sid, kind, json!({ "i": i })).await.unwrap().seq
        }));
    }
    let mut seqs = Vec::new();
    for t in tasks {
        seqs.push(t.await.unwrap());
    }
    seqs.sort();
    assert_eq!(seqs, (1..=40).collect::<Vec<i64>>());
    assert_eq!(store.get_session(&s.id).await.unwrap().last_seq, 40);

    let after = store.list_events(&s.id, 35, 100, None).await.unwrap();
    assert_eq!(after.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![36, 37, 38, 39, 40]);
    let only_status = store
        .list_events(&s.id, 0, 100, Some(&["status".to_string()][..]))
        .await
        .unwrap();
    assert_eq!(only_status.len(), 20);
    assert!(only_status.iter().all(|e| e.event_type == "status"));
    assert!(matches!(
        store.append_event(&s.id, "bogus", json!({})).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.append_event("sess_missing", "message", json!({})).await,
        Err(StoreError::NotFound)
    ));
}

async fn claim_fencing_and_secrets(store: DynStore) {
    let (_, e, s) = session(&store).await;
    let item = store.enqueue_work(&e.id, &s.id).await.unwrap();
    assert_eq!(item.status, WorkStatus::Ready);
    assert_eq!(item.lease_generation, 0);
    let now = Utc::now();

    let c1 = store.claim_work(claim_req(&e.id, "w1", now)).await.unwrap().unwrap();
    assert_eq!(c1.item.id, item.id);
    assert_eq!(c1.item.lease_generation, 1);
    assert_eq!(c1.item.status, WorkStatus::Leased);
    assert_eq!(c1.item.worker_id.as_deref(), Some("w1"));
    assert!(store.claim_work(claim_req(&e.id, "w2", now)).await.unwrap().is_none());

    let found = store
        .find_work_by_secret_hash(&hash_secret(&c1.session_secret), now)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.session_id, s.id);

    assert!(matches!(store.heartbeat_work(&item.id, 0, now).await, Err(StoreError::LeaseStale)));
    assert!(matches!(store.renew_work(&item.id, 2, now).await, Err(StoreError::LeaseStale)));
    assert!(matches!(store.heartbeat_work("work_missing", 1, now).await, Err(StoreError::NotFound)));
    let later = now + Duration::seconds(20);
    let renewed = store.renew_work(&item.id, 1, later).await.unwrap();
    // Both stores keep microseconds.
    let exp = renewed.lease_expires_at.expect("leased item has an expiry");
    assert!((exp - lease_end(later)).num_milliseconds().abs() <= 1, "renew moves the lease to now + 60s");
    store.heartbeat_work(&item.id, 1, later).await.unwrap();

    // Release: back to ready now; the old secret stops working.
    let released = store.release_work(&item.id, 1, later).await.unwrap();
    assert_eq!(released.status, WorkStatus::Ready);
    assert_eq!(released.lease_expires_at, None);
    assert!(store
        .find_work_by_secret_hash(&hash_secret(&c1.session_secret), later)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(store.renew_work(&item.id, 1, later).await, Err(StoreError::LeaseStale)));

    // w1 just released it: for the affinity window another worker skips it.
    assert!(store.claim_work(claim_req(&e.id, "w2", later)).await.unwrap().is_none());
    let later2 = later + Duration::seconds(AFFINITY_SECS + 1);
    let c2 = store.claim_work(claim_req(&e.id, "w2", later2)).await.unwrap().unwrap();
    assert_eq!(c2.item.lease_generation, 2);
    assert_ne!(c2.session_secret, c1.session_secret);
    assert!(matches!(
        store.complete_work(&item.id, 1, WorkOutcome::Success, None, later2).await,
        Err(StoreError::LeaseStale)
    ));
    let done = store
        .complete_work(&item.id, 2, WorkOutcome::Success, None, later2)
        .await
        .unwrap();
    assert_eq!(done.status, WorkStatus::Done);
    assert_eq!(done.lease_expires_at, None);
    // Completing twice with the current generation is fine.
    let twice = store
        .complete_work(&item.id, 2, WorkOutcome::Success, None, later2)
        .await
        .unwrap();
    assert_eq!(twice.status, WorkStatus::Done);
    assert!(store
        .find_work_by_secret_hash(&hash_secret(&c2.session_secret), later2)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(store.renew_work(&item.id, 2, later2).await, Err(StoreError::LeaseStale)));

    // Failed outcome keeps the message. The session's next item inherits
    // w2's affinity (it just completed the session's item), so w2 claims it.
    let item2 = store.enqueue_work(&e.id, &s.id).await.unwrap();
    assert_eq!(item2.preferred_worker_id.as_deref(), Some("w2"));
    let c3 = store.claim_work(claim_req(&e.id, "w2", later2)).await.unwrap().unwrap();
    assert_eq!(c3.item.id, item2.id);
    assert_eq!(c3.item.preferred_worker_id, None, "a claim clears the preference");
    let failed = store
        .complete_work(&item2.id, 1, WorkOutcome::Failed, Some("sandbox died"), later2)
        .await
        .unwrap();
    assert_eq!(failed.status, WorkStatus::Failed);
    assert_eq!(failed.error_message.as_deref(), Some("sandbox died"));
}

async fn expired_leases_are_reclaimed(store: DynStore) {
    let (_, e, s) = session(&store).await;
    let item = store.enqueue_work(&e.id, &s.id).await.unwrap();
    let t0 = Utc::now();
    let c1 = store.claim_work(claim_req(&e.id, "dead", t0)).await.unwrap().unwrap();

    // 61s later the lease has run out. Without reclaim, nothing to claim.
    let t1 = t0 + Duration::seconds(LEASE_TTL_SECS + 1);
    assert!(store.claim_work(claim_req(&e.id, "w2", t1)).await.unwrap().is_none());
    // The secret is dead once the lease ran out.
    assert!(store
        .find_work_by_secret_hash(&hash_secret(&c1.session_secret), t1)
        .await
        .unwrap()
        .is_none());
    // Expired less than 5s ago: a 5s reclaim threshold still waits.
    let mut r = claim_req(&e.id, "w2", t1);
    r.lease_cutoff_offset_ms = Some(5_000);
    assert!(store.claim_work(r).await.unwrap().is_none());
    // 0ms threshold takes it.
    let mut r = claim_req(&e.id, "w2", t1);
    r.lease_cutoff_offset_ms = Some(0);
    let c2 = store.claim_work(r).await.unwrap().unwrap();
    assert_eq!(c2.item.id, item.id);
    assert_eq!(c2.item.lease_generation, 2);
    // The dead worker's late renew is fenced off.
    assert!(matches!(store.renew_work(&item.id, 1, t1).await, Err(StoreError::LeaseStale)));

    // The sweeper puts expired items back to ready.
    let t2 = t1 + Duration::seconds(LEASE_TTL_SECS + 10);
    assert_eq!(store.requeue_expired_work(t2, Duration::seconds(5)).await.unwrap(), 1);
    assert_eq!(store.get_work(&item.id).await.unwrap().status, WorkStatus::Ready);
    let c3 = store.claim_work(claim_req(&e.id, "w3", t2)).await.unwrap().unwrap();
    assert_eq!(c3.item.lease_generation, 3);

    // Cancel for the session.
    assert_eq!(store.cancel_open_work_for_session(&s.id).await.unwrap(), 1);
    assert_eq!(store.get_work(&item.id).await.unwrap().status, WorkStatus::Cancelled);
    assert!(store.find_open_work_for_session(&s.id).await.unwrap().is_none());
}

async fn tool_calls_are_at_least_once_and_idempotent(store: DynStore) {
    let (_, e, s) = session(&store).await;
    let new = NewToolCall {
        id: "toolu_1".into(),
        session_id: s.id.clone(),
        name: "bash".into(),
        input: json!({ "command": "echo ok" }),
    };
    let (tc, created) = store.insert_tool_call(new.clone()).await.unwrap();
    assert!(created);
    assert_eq!(tc.status, ToolCallStatus::Pending);
    let (_, again) = store.insert_tool_call(new.clone()).await.unwrap();
    assert!(!again, "a repeat insert is a no-op");

    let item = store.enqueue_work(&e.id, &s.id).await.unwrap();
    assert_eq!(store.find_open_work_for_session(&s.id).await.unwrap().unwrap().id, item.id);
    let now = Utc::now();
    let c1 = store.claim_work(claim_req(&e.id, "w1", now)).await.unwrap().unwrap();
    assert!(matches!(store.next_tool_call(&item.id, 0).await, Err(StoreError::LeaseStale)));
    let got = store.next_tool_call(&item.id, 1).await.unwrap().unwrap();
    assert_eq!(got.id, "toolu_1");
    assert_eq!(got.status, ToolCallStatus::Claimed);
    assert_eq!(got.claimed_generation, Some(1));
    assert!(store.next_tool_call(&item.id, 1).await.unwrap().is_none());

    // The worker dies; the next owner (after the affinity window) gets the
    // same call again.
    store.release_work(&item.id, c1.item.lease_generation, now).await.unwrap();
    let after_affinity = now + Duration::seconds(AFFINITY_SECS + 1);
    let c2 = store.claim_work(claim_req(&e.id, "w2", after_affinity)).await.unwrap().unwrap();
    let again = store.next_tool_call(&item.id, c2.item.lease_generation).await.unwrap().unwrap();
    assert_eq!(again.id, "toolu_1");
    assert_eq!(again.claimed_generation, Some(2));

    // One result per id.
    let big = "x".repeat(MAX_TOOL_RESULT_BYTES + 1);
    assert!(matches!(
        store.submit_tool_result("toolu_1", &big, false).await,
        Err(StoreError::ResultTooLarge { .. })
    ));
    let exact = "y".repeat(MAX_TOOL_RESULT_BYTES);
    let first = store.submit_tool_result("toolu_1", &exact, false).await.unwrap();
    assert!(first.stored);
    assert_eq!(first.call.status, ToolCallStatus::Done);
    let second = store.submit_tool_result("toolu_1", "other", true).await.unwrap();
    assert!(!second.stored);
    assert_eq!(second.call.output.as_deref(), Some(exact.as_str()));
    assert!(!second.call.is_error);
    assert!(matches!(
        store.submit_tool_result("toolu_missing", "x", false).await,
        Err(StoreError::NotFound)
    ));

    // The id belongs to one session.
    let (_, _, other) = session(&store).await;
    assert!(matches!(
        store
            .insert_tool_call(NewToolCall { session_id: other.id.clone(), ..new })
            .await,
        Err(StoreError::Conflict("tool_call_conflict"))
    ));

    // Interrupt finishes open calls as errors.
    store
        .insert_tool_call(NewToolCall {
            id: "toolu_2".into(),
            session_id: s.id.clone(),
            name: "read".into(),
            input: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(store.cancel_open_tool_calls(&s.id, "cancelled").await.unwrap(), 1);
    let t2 = store.get_tool_call("toolu_2").await.unwrap();
    assert_eq!(t2.status, ToolCallStatus::Done);
    assert!(t2.is_error);
}

async fn api_keys_first_key_and_last_admin(store: DynStore) {
    assert_eq!(store.count_api_keys().await.unwrap(), 0);
    let mk = |role: Role| {
        let secret = new_secret("ask");
        (
            secret.clone(),
            NewApiKey {
                name: format!("{} key", role.as_str()),
                role,
                prefix: secret[..12].to_string(),
                key_hash: hash_secret(&secret),
                expires_at: None,
            },
        )
    };
    let (s1, n1) = mk(Role::Admin);
    let first = store.create_first_api_key(n1).await.unwrap();
    let (_, n2) = mk(Role::Admin);
    assert!(matches!(
        store.create_first_api_key(n2).await,
        Err(StoreError::Conflict("setup_already_done"))
    ));
    assert_eq!(
        store.find_api_key_by_hash(&hash_secret(&s1)).await.unwrap().unwrap().id,
        first.id
    );
    assert!(store.find_api_key_by_hash(&hash_secret("ask_wrong")).await.unwrap().is_none());

    let (_, viewer) = mk(Role::Viewer);
    let viewer = store.create_api_key(viewer).await.unwrap();
    let now = Utc::now();
    // The only admin cannot be revoked; a viewer can.
    assert!(matches!(
        store.revoke_api_key(&first.id, now).await,
        Err(StoreError::Conflict("last_admin_key"))
    ));
    store.revoke_api_key(&viewer.id, now).await.unwrap();
    let (_, a2) = mk(Role::Admin);
    let a2 = store.create_api_key(a2).await.unwrap();
    let revoked = store.revoke_api_key(&first.id, now).await.unwrap();
    assert!(revoked.revoked_at.is_some());
    assert!(!revoked.is_active(now));
    assert!(matches!(
        store.revoke_api_key(&a2.id, now).await,
        Err(StoreError::Conflict("last_admin_key"))
    ));
    store.touch_api_key(&a2.id, now).await.unwrap();
    let keys = store.list_api_keys(10, None).await.unwrap();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().find(|k| k.id == a2.id).unwrap().last_used_at.is_some());
    assert_eq!(store.count_api_keys().await.unwrap(), 3);
}

async fn workers_and_queue_depth(store: DynStore) {
    let (a, e, s) = session(&store).await;
    store.enqueue_work(&e.id, &s.id).await.unwrap();
    for s in more_sessions(&store, &a, &e, 2).await {
        store.enqueue_work(&e.id, &s.id).await.unwrap();
    }
    assert_eq!(store.queue_depth(&e.id).await.unwrap(), 3);
    let now = Utc::now();
    let mut r = claim_req(&e.id, "w1", now);
    r.hostname = Some("box-1".into());
    let c = store.claim_work(r).await.unwrap().unwrap();
    assert_eq!(store.queue_depth(&e.id).await.unwrap(), 2);
    let old = now - Duration::minutes(30);
    store.touch_worker(&e.id, "w-old", None, None, None, old).await.unwrap();
    store
        .touch_worker(&e.id, "w2", None, Some("box-2"), Some("1.2.3"), now)
        .await
        .unwrap();
    let listed = store.list_workers(&e.id, now - Duration::minutes(10)).await.unwrap();
    let ids: HashSet<_> = listed.iter().map(|w| w.worker_id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["w1", "w2"]));
    let w1 = listed.iter().find(|w| w.worker_id == "w1").unwrap();
    assert_eq!(w1.current_work_id.as_deref(), Some(c.item.id.as_str()));
    assert_eq!(w1.hostname.as_deref(), Some("box-1"));
    // A touch without hostname keeps the known one.
    store.touch_worker(&e.id, "w1", None, None, None, now).await.unwrap();
    let w1 = store
        .list_workers(&e.id, now - Duration::minutes(10))
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.worker_id == "w1")
        .unwrap();
    assert_eq!(w1.hostname.as_deref(), Some("box-1"));
    assert_eq!(w1.current_work_id, None);
    assert_eq!(store.count_workers(&e.id, now - Duration::seconds(60)).await.unwrap(), 2);
}

async fn credentials_roundtrip(store: DynStore) {
    let c = store.put_credential("github", b"sealed-1", "k1").await.unwrap();
    assert_eq!(c.sealed, b"sealed-1".to_vec());
    let c2 = store.put_credential("github", b"sealed-2", "k2").await.unwrap();
    assert_eq!(c2.id, c.id, "replace by name keeps the row");
    assert_eq!(c2.key_id, "k2");
    assert_eq!(store.get_credential("github").await.unwrap().sealed, b"sealed-2".to_vec());
    store.put_credential("aws", b"s", "k2").await.unwrap();
    let names: Vec<_> = store.list_credentials().await.unwrap().into_iter().map(|c| c.name).collect();
    assert_eq!(names, vec!["aws".to_string(), "github".to_string()]);
    store.delete_credential("aws").await.unwrap();
    assert!(matches!(store.get_credential("aws").await, Err(StoreError::NotFound)));
    assert!(matches!(store.delete_credential("aws").await, Err(StoreError::NotFound)));
}

/// 50 claimers race for 200 items (one per session): every item is
/// claimed exactly once.
async fn concurrent_claims_never_double_claim(store: DynStore) {
    let (a, e, s) = session(&store).await;
    store.enqueue_work(&e.id, &s.id).await.unwrap();
    for s in more_sessions(&store, &a, &e, 199).await {
        store.enqueue_work(&e.id, &s.id).await.unwrap();
    }
    let now = Utc::now();
    let mut tasks = Vec::new();
    for w in 0..50 {
        let store = store.clone();
        let env_id = e.id.clone();
        tasks.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            let mut empty = 0;
            // A claim can come back empty while others hold row locks; stop
            // after a few empty answers in a row.
            while empty < 3 {
                match store
                    .claim_work(claim_req(&env_id, &format!("w{w}"), now))
                    .await
                    .unwrap()
                {
                    Some(c) => {
                        empty = 0;
                        mine.push((c.item.id, c.item.lease_generation));
                    }
                    None => {
                        empty += 1;
                        tokio::task::yield_now().await;
                    }
                }
            }
            mine
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    let unique: HashSet<_> = all.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(all.len(), unique.len(), "an item was claimed twice");
    assert_eq!(unique.len(), 200, "every item was claimed");
    assert!(all.iter().all(|(_, g)| *g == 1));
    assert_eq!(store.queue_depth(&e.id).await.unwrap(), 0);
}

// ─── Selfhost 1.1: several replicas on one database ────────────────────────

/// 20 racing callers (20 replicas queueing the same session's work) leave
/// exactly one open item, and exactly one of them inserted it.
async fn ensure_open_work_makes_one_row_under_concurrency(store: DynStore) {
    let (_, e, s) = session(&store).await;
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let store = store.clone();
        let (env, sid) = (e.id.clone(), s.id.clone());
        tasks.push(tokio::spawn(async move { store.ensure_open_work(&env, &sid).await.unwrap() }));
    }
    let mut results = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    let ids: HashSet<_> = results.iter().map(|(w, _)| w.id.clone()).collect();
    assert_eq!(ids.len(), 1, "every caller got the same item");
    assert_eq!(results.iter().filter(|(_, created)| *created).count(), 1, "one insert");
    assert_eq!(store.queue_depth(&e.id).await.unwrap(), 1, "one open row");
    let open = store.find_open_work_for_session(&s.id).await.unwrap().unwrap();
    assert!(ids.contains(&open.id));

    // A plain insert is refused while the item is open.
    assert!(matches!(
        store.enqueue_work(&e.id, &s.id).await,
        Err(StoreError::Conflict("work_already_open"))
    ));

    // Once it ends, the next call queues a new one.
    let now = Utc::now();
    let c = store.claim_work(claim_req(&e.id, "w1", now)).await.unwrap().unwrap();
    store
        .complete_work(&c.item.id, c.item.lease_generation, WorkOutcome::Success, None, now)
        .await
        .unwrap();
    let (next, created) = store.ensure_open_work(&e.id, &s.id).await.unwrap();
    assert!(created);
    assert_ne!(next.id, open.id);
    assert_eq!(next.preferred_worker_id.as_deref(), Some("w1"), "inherits the live preference");
}

/// The soft affinity rule: the claimer's preferred item first; another
/// worker skips it while the preferred worker is alive and the window is
/// open; the preference ends after the window or when the preferred worker
/// has been silent for more than AFFINITY_SEEN_SECS.
async fn affinity_prefers_the_last_worker_then_expires(store: DynStore) {
    let (a, e, s1) = session(&store).await;
    let s2 = more_sessions(&store, &a, &e, 1).await.remove(0);
    let t0 = Utc::now();
    let t1 = t0 + Duration::seconds(1);

    // w1 serves s1's item and completes it (idle release).
    store.enqueue_work(&e.id, &s1.id).await.unwrap();
    let c = store.claim_work(claim_req(&e.id, "w1", t0)).await.unwrap().unwrap();
    // Another session's item queues meanwhile: it is older than s1's next.
    let other = store.enqueue_work(&e.id, &s2.id).await.unwrap();
    store
        .complete_work(&c.item.id, c.item.lease_generation, WorkOutcome::Success, None, t1)
        .await
        .unwrap();
    store.touch_worker(&e.id, "w1", None, None, None, t1).await.unwrap();
    let (next, _) = store.ensure_open_work(&e.id, &s1.id).await.unwrap();
    assert_eq!(next.preferred_worker_id.as_deref(), Some("w1"));

    // Order: w1 gets its preferred item before the older one.
    let c = store.claim_work(claim_req(&e.id, "w1", t1)).await.unwrap().unwrap();
    assert_eq!(c.item.id, next.id);
    store.release_work(&next.id, c.item.lease_generation, t1).await.unwrap();
    let released = store.get_work(&next.id).await.unwrap();
    assert_eq!(released.preferred_worker_id.as_deref(), Some("w1"));
    assert_eq!(released.worker_id, None);

    // w2 skips w1's item and takes the other one; then there is nothing.
    let c2 = store.claim_work(claim_req(&e.id, "w2", t1)).await.unwrap().unwrap();
    assert_eq!(c2.item.id, other.id);
    assert!(store.claim_work(claim_req(&e.id, "w2", t1)).await.unwrap().is_none());

    // After the window, w2 may take it.
    let t2 = t1 + Duration::seconds(AFFINITY_SECS + 1);
    let c3 = store.claim_work(claim_req(&e.id, "w2", t2)).await.unwrap().unwrap();
    assert_eq!(c3.item.id, next.id);

    // A preferred worker silent for over 60 s does not hold the item. w2
    // was last seen at its claim (t2); it releases 70 s later.
    let t3 = t2 + Duration::seconds(70);
    store.release_work(&next.id, c3.item.lease_generation, t3).await.unwrap();
    let held = store.get_work(&next.id).await.unwrap();
    assert_eq!(held.preferred_worker_id.as_deref(), Some("w2"));
    assert!(held.prefer_until.unwrap() > t3);
    let c4 = store.claim_work(claim_req(&e.id, "w3", t3)).await.unwrap().unwrap();
    assert_eq!(c4.item.id, next.id, "a silent preferred worker is ignored");

    // A lease that ran out (dead worker) carries no preference.
    let t4 = t3 + Duration::seconds(LEASE_TTL_SECS + 1);
    assert_eq!(store.requeue_expired_work(t4, Duration::zero()).await.unwrap(), 2);
    let requeued = store.get_work(&next.id).await.unwrap();
    assert_eq!(requeued.preferred_worker_id, None);
    assert_eq!(requeued.prefer_until, None);
}

async fn credential_cas_loses_to_a_concurrent_write(store: DynStore) {
    let v1 = store.put_credential("gh", b"v1", "k1").await.unwrap();
    // Two quick writes still move updated_at (it is the version).
    let v1b = store.put_credential("gh", b"v1", "k1").await.unwrap();
    assert!(v1b.updated_at > v1.updated_at);
    let read = store.get_credential("gh").await.unwrap();

    // A writer slips in between the read and the rotation's write.
    store.put_credential("gh", b"concurrent", "k2").await.unwrap();
    let won = store
        .put_credential_if_unchanged("gh", read.updated_at, b"rotated", "k3")
        .await
        .unwrap();
    assert!(!won, "the stale write loses");
    assert_eq!(store.get_credential("gh").await.unwrap().sealed, b"concurrent".to_vec());

    // Re-read and retry wins.
    let fresh = store.get_credential("gh").await.unwrap();
    assert!(store
        .put_credential_if_unchanged("gh", fresh.updated_at, b"rotated", "k3")
        .await
        .unwrap());
    let now = store.get_credential("gh").await.unwrap();
    assert_eq!(now.sealed, b"rotated".to_vec());
    assert_eq!(now.key_id, "k3");
    assert!(now.updated_at > fresh.updated_at);

    // Ten racing CAS writes from the same read: one wins.
    let mut tasks = Vec::new();
    for i in 0..10 {
        let store = store.clone();
        let expected = now.updated_at;
        tasks.push(tokio::spawn(async move {
            store
                .put_credential_if_unchanged("gh", expected, format!("racer-{i}").as_bytes(), "k4")
                .await
                .unwrap()
        }));
    }
    let mut wins = 0;
    for t in tasks {
        wins += t.await.unwrap() as usize;
    }
    assert_eq!(wins, 1);

    store.delete_credential("gh").await.unwrap();
    assert!(matches!(
        store.put_credential_if_unchanged("gh", now.updated_at, b"x", "k").await,
        Err(StoreError::NotFound)
    ));
}

async fn idempotency_begin_race_has_one_winner_and_replays(store: DynStore) {
    let scope = IdempotencyScope {
        principal: "key_abc".into(),
        method: "POST".into(),
        path: "/v1/sessions/sess_1/messages".into(),
        key: "idem-1".into(),
    };
    let t0 = Utc::now();
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let (store, scope) = (store.clone(), scope.clone());
        tasks.push(tokio::spawn(async move { store.idempotency_begin(&scope, "h1", t0).await.unwrap() }));
    }
    let mut outcomes = Vec::new();
    for t in tasks {
        outcomes.push(t.await.unwrap());
    }
    assert_eq!(outcomes.iter().filter(|o| **o == IdempotencyBegin::New).count(), 1, "{outcomes:?}");
    assert!(outcomes
        .iter()
        .all(|o| matches!(o, IdempotencyBegin::New | IdempotencyBegin::InProgress)));

    let resp = StoredResponse {
        status: 201,
        headers: vec![("content-type".into(), "application/json".into())],
        body: br#"{"id":"evt_1"}"#.to_vec(),
    };
    store.idempotency_finish(&scope, "h1", &resp, t0).await.unwrap();
    assert_eq!(
        store.idempotency_begin(&scope, "h1", t0 + Duration::seconds(5)).await.unwrap(),
        IdempotencyBegin::Replay(resp.clone())
    );
    // Same key, other body.
    assert_eq!(
        store.idempotency_begin(&scope, "h2", t0 + Duration::seconds(5)).await.unwrap(),
        IdempotencyBegin::Mismatch
    );
    // The key is per principal.
    let other = IdempotencyScope { principal: "key_other".into(), ..scope.clone() };
    assert_eq!(store.idempotency_begin(&other, "h1", t0).await.unwrap(), IdempotencyBegin::New);
    // After 24 h the record counts as gone.
    assert_eq!(
        store
            .idempotency_begin(&scope, "h2", t0 + Duration::hours(IDEMPOTENCY_TTL_HOURS) + Duration::seconds(1))
            .await
            .unwrap(),
        IdempotencyBegin::New
    );

    // Abandon (non-2xx): the retry runs again.
    let s3 = IdempotencyScope { key: "idem-3".into(), ..scope.clone() };
    assert_eq!(store.idempotency_begin(&s3, "h", t0).await.unwrap(), IdempotencyBegin::New);
    store.idempotency_abandon(&s3, "h").await.unwrap();
    assert_eq!(store.idempotency_begin(&s3, "h", t0).await.unwrap(), IdempotencyBegin::New);

    // An in-progress record is taken over after 60 s, by one request.
    let t_mid = t0 + Duration::seconds(30);
    assert_eq!(store.idempotency_begin(&s3, "h", t_mid).await.unwrap(), IdempotencyBegin::InProgress);
    let t_late = t0 + Duration::seconds(IDEMPOTENCY_TAKEOVER_SECS + 1);
    assert_eq!(store.idempotency_begin(&s3, "h", t_late).await.unwrap(), IdempotencyBegin::New);
    assert_eq!(store.idempotency_begin(&s3, "h", t_late).await.unwrap(), IdempotencyBegin::InProgress);

    // Purge by creation time.
    let purged = store.idempotency_purge(t0 + Duration::seconds(1)).await.unwrap();
    assert!(purged >= 2, "purged {purged}");
    assert_eq!(store.idempotency_begin(&other, "h1", t0).await.unwrap(), IdempotencyBegin::New);
}

async fn event_codes(store: &DynStore, sid: &str) -> Vec<(String, serde_json::Value)> {
    store
        .list_events(sid, 0, 1000, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.event_type, e.data))
        .collect()
}

/// A turn whose replica died is failed once, even with several replicas'
/// keepers racing; live owners and idle sessions are left alone.
async fn orphaned_turns_fail_exactly_once(store: DynStore) {
    let (a, e, dead) = session(&store).await;
    let mut more = more_sessions(&store, &a, &e, 3).await;
    let (live, ownerless, idle) = (more.remove(0), more.remove(0), more.remove(0));
    for s in [&dead, &live, &ownerless] {
        store.transition_session_status(&s.id, &["idle"], "running", None).await.unwrap().unwrap();
    }
    assert!(store.claim_turn(&dead.id, "r-dead").await.unwrap());
    assert!(store.claim_turn(&dead.id, "r-dead").await.unwrap(), "re-claim by the owner");
    assert!(!store.claim_turn(&dead.id, "r-live").await.unwrap(), "another replica owns it");
    assert!(store.claim_turn(&live.id, "r-live").await.unwrap());
    assert!(store.claim_turn(&idle.id, "r-dead").await.unwrap());
    assert!(matches!(store.claim_turn("sess_missing", "r").await, Err(StoreError::NotFound)));
    store.set_gizzi_replica(&dead.id, Some("r-dead")).await.unwrap();
    assert_eq!(store.get_session(&dead.id).await.unwrap().gizzi_replica.as_deref(), Some("r-dead"));
    store
        .insert_tool_call(NewToolCall {
            id: "toolu_orphan".into(),
            session_id: dead.id.clone(),
            name: "bash".into(),
            input: json!({}),
        })
        .await
        .unwrap();

    let mut tasks = Vec::new();
    for _ in 0..5 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.fail_orphaned_turns(&["r-dead".to_string()], None).await.unwrap()
        }));
    }
    let mut failed = Vec::new();
    for t in tasks {
        failed.extend(t.await.unwrap().into_iter().map(|s| s.id));
    }
    assert_eq!(failed, vec![dead.id.clone()], "failed once, only the dead replica's running turn");

    let d = store.get_session(&dead.id).await.unwrap();
    assert_eq!(d.status, "error");
    assert_eq!(d.status_reason.as_deref(), Some(SERVER_RESTARTED));
    assert_eq!(d.turn_replica, None);
    let events = event_codes(&store, &dead.id).await;
    let errors: Vec<_> = events.iter().filter(|(t, _)| t == "error").collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(errors[0].1["code"], json!("server_restarted"));
    assert_eq!(events.iter().filter(|(t, _)| t == "status").count(), 1);
    let call = store.get_tool_call("toolu_orphan").await.unwrap();
    assert_eq!(call.status, ToolCallStatus::Done);
    assert!(call.is_error);
    assert!(!store.has_open_tool_calls(&dead.id).await.unwrap());

    assert_eq!(store.get_session(&live.id).await.unwrap().status, "running");
    assert_eq!(store.get_session(&idle.id).await.unwrap().status, "idle");
    assert_eq!(store.get_session(&ownerless.id).await.unwrap().status, "running");

    // Ownerless running rows (a 1.0 server's) older than the cutoff.
    let cutoff = Utc::now() + Duration::seconds(1);
    let swept = store.fail_orphaned_turns(&[], Some(cutoff)).await.unwrap();
    assert_eq!(swept.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), vec![ownerless.id.clone()]);
    assert!(store.fail_orphaned_turns(&["r-dead".to_string()], Some(cutoff)).await.unwrap().is_empty());
    assert_eq!(store.get_session(&live.id).await.unwrap().status, "running");

    // Turn release is owner-only.
    assert!(!store.release_turn(&live.id, "r-dead").await.unwrap());
    assert!(store.release_turn(&live.id, "r-live").await.unwrap());
    assert!(store.claim_turn(&live.id, "r-other").await.unwrap());
}

async fn replicas_sweep_lock_and_wakes(store: DynStore) {
    let now = Utc::now();
    store.replica_heartbeat("r1", now - Duration::minutes(5), Some("1.1.0"), now).await.unwrap();
    store
        .replica_heartbeat("r2", now - Duration::minutes(9), None, now - Duration::minutes(2))
        .await
        .unwrap();
    let live = store.list_live_replicas(now - Duration::seconds(45)).await.unwrap();
    assert_eq!(live.iter().map(|r| r.replica_id.as_str()).collect::<Vec<_>>(), vec!["r1"]);
    assert_eq!(live[0].version.as_deref(), Some("1.1.0"));
    assert_eq!(store.list_replicas().await.unwrap().len(), 2);
    // A restart with the same id resets started_at.
    store.replica_heartbeat("r1", now, Some("1.1.1"), now).await.unwrap();
    let r1 = store.list_live_replicas(now - Duration::seconds(45)).await.unwrap().remove(0);
    assert!((r1.started_at - now).num_milliseconds().abs() <= 1);
    store.remove_replica("r2").await.unwrap();
    store.remove_replica("r2").await.unwrap();
    assert_eq!(store.list_replicas().await.unwrap().len(), 1);

    // Sweep lock: one holder at a time on Postgres; always granted on SQLite.
    let g1 = store.sweep_lock().await.unwrap().expect("first lock");
    let second = store.sweep_lock().await.unwrap();
    if store.backend() == "postgres" {
        assert!(second.is_none(), "another replica holds the sweep lock");
    } else {
        assert!(second.is_some());
    }
    g1.release().await;
    if let Some(g) = second {
        g.release().await;
    }
    store.sweep_lock().await.unwrap().expect("free again").release().await;

    // Wakes: Postgres sends hints from inside the store's transactions.
    let Some(mut wakes) = store.subscribe_wakes().await.unwrap() else {
        assert_ne!(store.backend(), "postgres");
        return;
    };
    use futures::StreamExt;
    let (_, e, s) = session(&store).await;
    let item = store.enqueue_work(&e.id, &s.id).await.unwrap();
    let ev = store.append_event(&s.id, "message", json!({"text": "hi"})).await.unwrap();
    store.set_session_status(&s.id, "running", None).await.unwrap();
    let c = store.claim_work(claim_req(&e.id, "w1", Utc::now())).await.unwrap().unwrap();
    assert_eq!(c.item.id, item.id);
    store
        .insert_tool_call(NewToolCall {
            id: format!("toolu_wake_{}", s.id),
            session_id: s.id.clone(),
            name: "bash".into(),
            input: json!({}),
        })
        .await
        .unwrap();
    store.submit_tool_result(&format!("toolu_wake_{}", s.id), "ok", false).await.unwrap();

    let mut want: HashSet<Wake> = HashSet::from([
        Wake::Work { environment_id: e.id.clone() },
        Wake::Event { session_id: s.id.clone(), seq: ev.seq },
        Wake::Status { session_id: s.id.clone() },
        Wake::Session { session_id: s.id.clone() },
        Wake::Call { tool_call_id: format!("toolu_wake_{}", s.id) },
    ]);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !want.is_empty() {
        let next = tokio::time::timeout_at(deadline, wakes.next())
            .await
            .unwrap_or_else(|_| panic!("wake hints missing: {want:?}"))
            .expect("the wake stream never ends");
        // Other tests share the database; ignore their hints.
        want.remove(&next);
    }
}

// ─── SQLite only ────────────────────────────────────────────────────────────

/// The 1.0 shape of the tables 1.1 changes.
const SQLITE_V10_SCHEMA: &str = r#"
CREATE TABLE agents (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, instructions TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL, tools TEXT NOT NULL DEFAULT '[]', mcp_servers TEXT NOT NULL DEFAULT '[]',
    skills TEXT NOT NULL DEFAULT '[]', metadata TEXT NOT NULL DEFAULT '{}',
    version INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
);
CREATE TABLE environments (
    id TEXT PRIMARY KEY, name TEXT NOT NULL,
    network TEXT NOT NULL DEFAULT 'none' CHECK (network IN ('none','limited','full')),
    allowed_hosts TEXT NOT NULL DEFAULT '[]', git TEXT NOT NULL DEFAULT '{}', mcp TEXT NOT NULL DEFAULT '{}',
    image TEXT, packages TEXT NOT NULL DEFAULT '{}', metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT
);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY, agent_id TEXT NOT NULL REFERENCES agents(id),
    environment_id TEXT NOT NULL REFERENCES environments(id), title TEXT,
    status TEXT NOT NULL DEFAULT 'idle'
        CHECK (status IN ('idle','running','waiting_for_worker','interrupted','error','archived')),
    status_reason TEXT, last_seq INTEGER NOT NULL DEFAULT 0, metadata TEXT NOT NULL DEFAULT '{}',
    agent_snapshot TEXT NOT NULL DEFAULT '{}', gizzi_session_id TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, archived_at TEXT, traceparent TEXT
);
CREATE TABLE work_items (
    id TEXT PRIMARY KEY, environment_id TEXT NOT NULL REFERENCES environments(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    status TEXT NOT NULL DEFAULT 'ready' CHECK (status IN ('ready','leased','done','failed','cancelled')),
    lease_generation INTEGER NOT NULL DEFAULT 0, lease_expires_at TEXT, worker_id TEXT,
    session_secret_hash TEXT, error_message TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT, traceparent TEXT
);
CREATE INDEX work_items_session ON work_items (session_id, status);
CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), work_id TEXT,
    claimed_generation INTEGER, name TEXT NOT NULL, input TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','claimed','done')),
    output TEXT, is_error INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL, completed_at TEXT, traceparent TEXT
);
INSERT INTO agents (id, name, model, created_at, updated_at)
    VALUES ('agent_1', 'a', 'm', '2026-11-01T00:00:00.000000Z', '2026-11-01T00:00:00.000000Z');
INSERT INTO environments (id, name, created_at, updated_at)
    VALUES ('env_1', 'e', '2026-11-01T00:00:00.000000Z', '2026-11-01T00:00:00.000000Z');
INSERT INTO sessions (id, agent_id, environment_id, status, created_at, updated_at)
    VALUES ('sess_1', 'agent_1', 'env_1', 'running', '2026-11-01T00:00:00.000000Z', '2026-11-01T00:00:00.000000Z');
-- Two open items for one session (two 1.0 processes on one file): an older
-- leased one, and a newer ready one with a call claimed under it.
INSERT INTO work_items (id, environment_id, session_id, status, lease_generation, lease_expires_at, worker_id, session_secret_hash, created_at, updated_at)
    VALUES ('work_leased', 'env_1', 'sess_1', 'leased', 1, '2099-01-01T00:00:00.000000Z', 'w1', 'hash1', '2026-11-01T00:00:01.000000Z', '2026-11-01T00:00:01.000000Z');
INSERT INTO work_items (id, environment_id, session_id, status, lease_generation, created_at, updated_at)
    VALUES ('work_ready', 'env_1', 'sess_1', 'ready', 0, '2026-11-01T00:00:02.000000Z', '2026-11-01T00:00:02.000000Z');
INSERT INTO tool_calls (id, session_id, work_id, claimed_generation, name, input, status, created_at, updated_at)
    VALUES ('toolu_dup', 'sess_1', 'work_ready', 1, 'bash', '{}', 'claimed', '2026-11-01T00:00:03.000000Z', '2026-11-01T00:00:03.000000Z');
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_sqlite_file_upgrades_to_one_open_item() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selfhost.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(SQLITE_V10_SCHEMA).unwrap();
    }
    let store: DynStore = Arc::new(SqliteStore::open(&path).expect("a 1.0 file opens"));

    let open = store.find_open_work_for_session("sess_1").await.unwrap().unwrap();
    assert_eq!(open.id, "work_leased", "the leased item is kept");
    let dup = store.get_work("work_ready").await.unwrap();
    assert_eq!(dup.status, WorkStatus::Cancelled);
    let call = store.get_tool_call("toolu_dup").await.unwrap();
    assert_eq!(call.status, ToolCallStatus::Pending, "a call claimed under the dropped item is handed out again");
    assert!(matches!(
        store.enqueue_work("env_1", "sess_1").await,
        Err(StoreError::Conflict("work_already_open"))
    ));
    let s = store.get_session("sess_1").await.unwrap();
    assert_eq!(s.turn_replica, None);
    assert!(store.claim_turn("sess_1", "r1").await.unwrap());
    drop(store);

    // The index exists, and opening again changes nothing.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'work_items_one_open'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let open: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_items WHERE session_id = 'sess_1' AND status IN ('ready','leased')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(open, 1);
    }
    let again = SqliteStore::open(&path).expect("reopen");
    assert_eq!(again.get_session("sess_1").await.unwrap().turn_replica.as_deref(), Some("r1"));
    assert_eq!(again.get_work("work_leased").await.unwrap().status, WorkStatus::Leased);
}

#[test]
fn wake_payloads_round_trip() {
    let all = [
        Wake::Work { environment_id: "env_1".into() },
        Wake::Session { session_id: "sess_1".into() },
        Wake::Call { tool_call_id: "call:with:colons".into() },
        Wake::Event { session_id: "sess_1".into(), seq: 42 },
        Wake::Status { session_id: "sess_1".into() },
    ];
    for w in all {
        assert_eq!(Wake::parse(&w.payload()), Some(w.clone()), "{}", w.payload());
    }
    assert_eq!(Wake::parse("event:sess_1:x"), None);
    assert_eq!(Wake::parse("event::3"), None);
    assert_eq!(Wake::parse("nope:1"), None);
    assert_eq!(Wake::parse("work:"), None);
    assert_eq!(Wake::parse(""), None);
}
