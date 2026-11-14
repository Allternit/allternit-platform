//! This process as one of several server replicas on one database.
//!
//! - **Id.** `<data_dir>/replica-id`, made once and reused across restarts
//!   ([`load_or_create_id`]). Turns this replica runs carry it in
//!   `sessions.turn_replica`.
//! - **Heartbeat.** Every [`HEARTBEAT_EVERY`] the replica refreshes its
//!   `server_replicas` row ([`heartbeat_once`]). The first one is written
//!   before the server takes requests.
//! - **Keeper.** Every [`KEEPER_EVERY`] each replica looks for replicas
//!   silent for [`DEAD_AFTER`]: their running turns end with the session
//!   error `server_restarted`, then their rows are removed. Turns with no
//!   owner older than [`OWNERLESS_AFTER`] (left by a 1.0 server) end the same
//!   way ([`keeper_once`]). The store does the check and the change in one
//!   step, so several keepers fail a session exactly once.
//! - **Boot.** A replica that starts again under its id fails its own
//!   leftover turns first ([`recover_at_boot`]). On SQLite (one node) every
//!   turn left running is a leftover.
//! - **Drain.** On SIGTERM the server stops taking turns, waits up to
//!   `server.shutdown_grace` for its running turns, then fails what is left
//!   and removes its row ([`drain`]).
//!
//! A turn is never moved to another replica: its context is rebuilt from the
//! stored events when the user sends the message again.

use std::path::Path;
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};

use super::store::{SessionRecord, StoreResult};
use super::SelfHostState;

/// The file under the data dir that holds this replica's id.
pub const ID_FILE: &str = "replica-id";
pub const HEARTBEAT_EVERY: StdDuration = StdDuration::from_secs(10);
pub const KEEPER_EVERY: StdDuration = StdDuration::from_secs(5);
/// A replica with no heartbeat for this long is dead.
pub const DEAD_AFTER: Duration = Duration::seconds(45);
/// A running turn with no owner, unchanged for this long, is a leftover.
pub const OWNERLESS_AFTER: Duration = Duration::minutes(15);
/// How often the drain checks whether the running turns are done.
const DRAIN_POLL: StdDuration = StdDuration::from_millis(200);

/// Read `<data_dir>/replica-id`, or make it (`r-<32 hex>`) the first time.
pub fn load_or_create_id(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join(ID_FILE);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        let id = raw.trim();
        if !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_graphic()) {
            return Ok(id.to_string());
        }
        tracing::warn!(path = %path.display(), "replica-id file is not a usable id; making a new one");
    }
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let id = format!("r-{}", uuid::Uuid::new_v4().simple());
    std::fs::write(&path, format!("{id}\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(id)
}

/// Refresh this replica's `server_replicas` row.
pub async fn heartbeat_once(st: &SelfHostState) -> StoreResult<()> {
    st.store
        .replica_heartbeat(
            &st.replica.id,
            st.replica.started_at,
            Some(env!("CARGO_PKG_VERSION")),
            Utc::now(),
        )
        .await
}

/// Live replicas, this one included (at least 1). What `GET /v1/server/info`
/// reports as `replicas`.
pub async fn live_count(st: &SelfHostState) -> usize {
    match st.store.list_live_replicas(Utc::now() - DEAD_AFTER).await {
        Ok(rows) => {
            let me = rows.iter().any(|r| r.replica_id == st.replica.id);
            (rows.len() + usize::from(!me)).max(1)
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot count replicas");
            1
        }
    }
}

/// Tell this process's waiters about sessions the store just failed (their
/// new events were written by the store, not through `append_event`).
fn announce(st: &SelfHostState, failed: &[SessionRecord]) {
    for s in failed {
        tracing::warn!(
            session_id = %s.id,
            replica = ?s.turn_replica,
            "turn lost with its replica: session error server_restarted"
        );
        st.hint_session(&s.id, s.last_seq);
        st.work.notify_session(&s.id);
    }
}

/// One keeper pass: fail the turns of replicas silent for [`DEAD_AFTER`]
/// and remove their rows; fail ownerless turns older than
/// [`OWNERLESS_AFTER`]. Returns the sessions failed by this call.
pub async fn keeper_once(st: &SelfHostState) -> StoreResult<Vec<SessionRecord>> {
    let now = Utc::now();
    let dead: Vec<String> = st
        .store
        .list_replicas()
        .await?
        .into_iter()
        .filter(|r| r.replica_id != st.replica.id && r.heartbeat_at < now - DEAD_AFTER)
        .map(|r| r.replica_id)
        .collect();
    let failed = st
        .store
        .fail_orphaned_turns(&dead, Some(now - OWNERLESS_AFTER))
        .await?;
    announce(st, &failed);
    // Only after its turns are failed, so a turn is never left with an
    // owner nobody checks.
    for id in &dead {
        tracing::warn!(replica = %id, "replica stopped sending heartbeats; removed");
        st.store.remove_replica(id).await?;
    }
    Ok(failed)
}

/// At start: fail the turns a previous run of this replica id left. On a
/// single-node store every turn still running is such a leftover.
pub async fn recover_at_boot(st: &SelfHostState) -> StoreResult<Vec<SessionRecord>> {
    let ownerless = (st.store.backend() != "postgres").then(Utc::now);
    let failed = st
        .store
        .fail_orphaned_turns(std::slice::from_ref(&st.replica.id), ownerless)
        .await?;
    announce(st, &failed);
    Ok(failed)
}

/// Heartbeat and keeper loops. Abort the handle to stop them.
pub fn spawn(st: SelfHostState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut beat = tokio::time::interval(HEARTBEAT_EVERY);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut keep = tokio::time::interval(KEEPER_EVERY);
        keep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = beat.tick() => {
                    if st.replica.draining() {
                        // The drain removes the row; don't put it back.
                        continue;
                    }
                    if let Err(e) = heartbeat_once(&st).await {
                        tracing::warn!(error = %e, "replica heartbeat failed");
                    }
                }
                _ = keep.tick() => {
                    if let Err(e) = keeper_once(&st).await {
                        tracing::warn!(error = %e, "replica keeper pass failed");
                    }
                }
            }
        }
    })
}

/// Graceful stop. Stops new turns at once, waits up to `grace` for the
/// turns this replica runs, then fails the rest (`server_restarted`) and
/// removes this replica's row. Returns the sessions it had to fail.
pub async fn drain(st: &SelfHostState, grace: StdDuration) -> Vec<SessionRecord> {
    st.replica.begin_drain();
    let running = st.turn_runner.running_turns();
    tracing::info!(turns = running, grace_ms = grace.as_millis() as u64, "draining: no new turns");
    let deadline = tokio::time::Instant::now() + grace;
    while st.turn_runner.running_turns() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(DRAIN_POLL).await;
    }
    let failed = match st
        .store
        .fail_orphaned_turns(std::slice::from_ref(&st.replica.id), None)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(error = %e, "could not fail this replica's remaining turns");
            Vec::new()
        }
    };
    announce(st, &failed);
    st.turn_runner.abort_all();
    if let Err(e) = st.store.remove_replica(&st.replica.id).await {
        tracing::warn!(error = %e, "could not remove this replica's row");
    }
    st.replica.mark_stopped();
    failed
}
