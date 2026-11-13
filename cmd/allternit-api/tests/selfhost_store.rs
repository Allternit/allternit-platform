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

fn claim_req(env_id: &str, worker: &str, now: chrono::DateTime<Utc>) -> ClaimRequest {
    ClaimRequest {
        environment_id: env_id.into(),
        worker_id: worker.into(),
        reclaim_older_than_ms: None,
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

    let c2 = store.claim_work(claim_req(&e.id, "w2", later)).await.unwrap().unwrap();
    assert_eq!(c2.item.lease_generation, 2);
    assert_ne!(c2.session_secret, c1.session_secret);
    assert!(matches!(
        store.complete_work(&item.id, 1, WorkOutcome::Success, None, later).await,
        Err(StoreError::LeaseStale)
    ));
    let done = store
        .complete_work(&item.id, 2, WorkOutcome::Success, None, later)
        .await
        .unwrap();
    assert_eq!(done.status, WorkStatus::Done);
    assert_eq!(done.lease_expires_at, None);
    // Completing twice with the current generation is fine.
    let twice = store
        .complete_work(&item.id, 2, WorkOutcome::Success, None, later)
        .await
        .unwrap();
    assert_eq!(twice.status, WorkStatus::Done);
    assert!(store
        .find_work_by_secret_hash(&hash_secret(&c2.session_secret), later)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(store.renew_work(&item.id, 2, later).await, Err(StoreError::LeaseStale)));

    // Failed outcome keeps the message.
    let item2 = store.enqueue_work(&e.id, &s.id).await.unwrap();
    let c3 = store.claim_work(claim_req(&e.id, "w1", later)).await.unwrap().unwrap();
    assert_eq!(c3.item.id, item2.id);
    let failed = store
        .complete_work(&item2.id, 1, WorkOutcome::Failed, Some("sandbox died"), later)
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
    r.reclaim_older_than_ms = Some(5_000);
    assert!(store.claim_work(r).await.unwrap().is_none());
    // 0ms threshold takes it.
    let mut r = claim_req(&e.id, "w2", t1);
    r.reclaim_older_than_ms = Some(0);
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

    // The worker dies; the next owner gets the same call again.
    store.release_work(&item.id, c1.item.lease_generation, now).await.unwrap();
    let c2 = store.claim_work(claim_req(&e.id, "w2", now)).await.unwrap().unwrap();
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
    let (_, e, s) = session(&store).await;
    for _ in 0..3 {
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

/// 50 claimers race for 200 items: every item is claimed exactly once.
async fn concurrent_claims_never_double_claim(store: DynStore) {
    let (_, e, s) = session(&store).await;
    for _ in 0..200 {
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
