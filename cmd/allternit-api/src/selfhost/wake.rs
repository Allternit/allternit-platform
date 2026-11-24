//! Wake hints from every replica, pumped into this process's waiters.
//!
//! The Postgres store sends a NOTIFY for each change that someone may be
//! waiting on ([`Wake`]). [`spawn`] opens one listener per replica
//! ([`super::store::SelfHostStore::subscribe_wakes`]) and forwards each hint:
//!
//! - `work:` → the claim long-polls ([`super::routes_work::WorkHub::notify_work`]),
//! - `session:` → that session's `tool_calls/next` waiters,
//! - `call:` → the internal route waiting on that tool call,
//! - `event:` / `status:` → the SSE streams ([`SelfHostState::wakes`]),
//! - `Resync` (the listener reconnected; hints may be lost) → claims and SSE
//!   (session and call waiters re-read the store every second anyway).
//!
//! Hints only make waits faster. Every waiter still re-reads the store on its
//! own timer (queue waits about once a second, SSE every
//! [`super::routes_sessions::SSE_SAFETY_READ`]), so a lost hint, or no
//! listener at all (SQLite, one process: wake-ups stay local), costs latency,
//! never correctness.

use futures::StreamExt;

use super::store::Wake;
use super::SelfHostState;

/// Forward one hint to this process's waiters.
pub fn dispatch(st: &SelfHostState, wake: Wake) {
    match &wake {
        Wake::Work { .. } => st.work.notify_work(),
        Wake::Session { session_id } => st.work.notify_session(session_id),
        Wake::Call { tool_call_id } => st.work.notify_call(tool_call_id),
        Wake::Event { .. } | Wake::Status { .. } => {
            let _ = st.wakes.send(wake.clone());
        }
        Wake::Resync => {
            // Session and call waiters re-read the store every second
            // anyway; claims and SSE are woken now.
            st.work.notify_work();
            let _ = st.wakes.send(Wake::Resync);
        }
    }
}

/// Start the pump. None when the store has no cross-process hints (SQLite).
/// The store's stream reconnects by itself (yielding `Resync`); if it ends
/// anyway, the pump subscribes again after a pause.
pub async fn spawn(st: SelfHostState) -> Option<tokio::task::JoinHandle<()>> {
    let first = match st.store.subscribe_wakes().await {
        Ok(Some(s)) => s,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "cannot listen for wake hints yet; waits fall back to polling");
            futures::stream::empty().boxed()
        }
    };
    Some(tokio::spawn(async move {
        let mut stream = first;
        loop {
            while let Some(w) = stream.next().await {
                dispatch(&st, w);
            }
            // Ended: anything may have been missed.
            dispatch(&st, Wake::Resync);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            stream = match st.store.subscribe_wakes().await {
                Ok(Some(s)) => s,
                Ok(None) => return,
                Err(e) => {
                    tracing::warn!(error = %e, "wake listener down; waits fall back to polling");
                    futures::stream::empty().boxed()
                }
            };
        }
    }))
}
