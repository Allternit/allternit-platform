//! Guest drops kept while this computer slept (plan step 17; SPEC section 8,
//! `QueuedDrop`).
//!
//! A guest with an `upload_only` link who drops a file while this computer
//! sleeps gets a `202`: cloud-api keeps the bytes in the owner's R2 space
//! (`files_gateway::queue_drop`). Once this computer is awake it pulls them
//! itself, signed like its heartbeat (`cloud.rs`):
//!
//! 1. `GET /api/v1/computer-files/drops` lists this computer's queued drops.
//! 2. Each one's bytes are fetched and checked against its `size` and blake3
//!    `content_hash`. A mismatch is acked `rejected` and nothing is written.
//! 3. The file lands in the **Inbox root**, written by the guest
//!    (`PrincipalKind::Guest`), keep-both on a name clash. The Inbox change
//!    listener (`inbox::on_change`) then makes it a `needs_approval` task with
//!    `untrusted: true`, exactly like a drop on an awake computer: nothing
//!    reads it until the owner approves.
//! 4. `POST .../drops/:drop_id/ack` (`delivered` + item id); cloud-api deletes
//!    the R2 copy. Delivered drop ids are kept in `files_v2.sqlite`, so a lost
//!    ack is sent again without writing the file twice.
//!
//! When: shortly after the runtime starts (a cloud computer's runtime starts on
//! wake) and then every [`EVERY_ENV`] seconds (default 300). Never wakes
//! anything and is not activity.

use std::sync::Arc;
use std::time::Duration;

use allternit_files_core::{
    ErrorCode, OnConflict, PrincipalKind, PrincipalRef, QueuedDrop, QueuedDropAck, QueuedDropAckState, QueuedDropsResponse,
    RootKind,
};
use allternit_files_engine::{chunker, EditingDevice};
use rusqlite::{params, OptionalExtension};

use super::cloud::{self, CloudCalls, SignedCloud};
use super::inbox::SIDECAR_SUFFIX;
use super::routes::run;
use super::{ApiError, FilesV2};

pub const LIST_PATH: &str = "/api/v1/computer-files/drops";
/// Seconds between pulls (default 300, at least 60).
pub const EVERY_ENV: &str = "ALLTERNIT_FILES_DROPS_EVERY_S";
/// First pull after start: long enough for pairing and the relay to come up.
const FIRST_PULL: Duration = Duration::from_secs(20);

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS delivered_drops(
  drop_id TEXT PRIMARY KEY,
  item_id TEXT NOT NULL,
  at TEXT NOT NULL)";

/// A guest-chosen name made safe for Inbox: no leading dots (Inbox ignores
/// dot files, so a hidden drop would never become a task) and never a
/// provenance sidecar name (Inbox trusts those as its own).
pub(crate) fn safe_name(name: &str) -> String {
    let trimmed = name.trim().trim_start_matches('.').trim();
    let mut out = if trimmed.is_empty() { "Dropped file".to_string() } else { trimmed.to_string() };
    if out.ends_with(SIDECAR_SUFFIX) {
        out.push_str(".dropped");
    }
    out
}

fn offline(e: String) -> ApiError {
    ApiError::new(ErrorCode::Offline, format!("cloud-api is not reachable: {e}"))
}

enum Outcome {
    Delivered(String),
    Rejected(String),
    /// Not acked; tried again on the next pull.
    Later,
}

/// One pull: every queued drop for this computer into Inbox. Returns how many
/// were delivered.
pub(crate) async fn pull(files: &Arc<FilesV2>, cloud: &dyn CloudCalls) -> Result<usize, ApiError> {
    let (status, body) = cloud.send("GET", LIST_PATH, Vec::new()).await.map_err(offline)?;
    if status == 403 {
        // feature_disabled: the owner's flag is off or sharing isn't set up yet.
        return Ok(0);
    }
    if !(200..300).contains(&status) {
        return Err(ApiError::new(ErrorCode::Offline, format!("cloud-api answered {status} for the drop list")));
    }
    let list: QueuedDropsResponse =
        serde_json::from_slice(&body).map_err(|_| ApiError::new(ErrorCode::Internal, "cloud-api's drop list did not parse"))?;
    let mut delivered = 0;
    for drop in list.drops {
        let outcome = match already_delivered(files, &drop.drop_id).await? {
            Some(item_id) => Outcome::Delivered(item_id),
            None => deliver(files, cloud, &drop).await?,
        };
        let ack = match outcome {
            Outcome::Delivered(item_id) => {
                delivered += 1;
                QueuedDropAck { state: QueuedDropAckState::Delivered, item_id: Some(item_id), reason: None }
            }
            Outcome::Rejected(reason) => QueuedDropAck { state: QueuedDropAckState::Rejected, item_id: None, reason: Some(reason) },
            Outcome::Later => continue,
        };
        let path = format!("{LIST_PATH}/{}/ack", drop.drop_id);
        let body = serde_json::to_vec(&ack).map_err(|_| ApiError::internal())?;
        match cloud.send("POST", &path, body).await {
            Ok((s, _)) if (200..300).contains(&s) => {}
            // The file is safe in Inbox and its id is recorded: the next pull acks again.
            Ok((s, _)) => tracing::debug!(drop_id = %drop.drop_id, status = s, "files v2: drop ack not taken"),
            Err(e) => tracing::debug!(drop_id = %drop.drop_id, error = %e, "files v2: drop ack not sent"),
        }
    }
    Ok(delivered)
}

async fn already_delivered(files: &Arc<FilesV2>, drop_id: &str) -> Result<Option<String>, ApiError> {
    let drop_id = drop_id.to_string();
    run(files, move |v| {
        let c = v.meta.lock().expect("meta lock");
        c.execute_batch(SCHEMA)?;
        Ok(c.query_row("SELECT item_id FROM delivered_drops WHERE drop_id = ?1", [&drop_id], |r| r.get(0)).optional()?)
    })
    .await
}

async fn deliver(files: &Arc<FilesV2>, cloud: &dyn CloudCalls, drop: &QueuedDrop) -> Result<Outcome, ApiError> {
    let path = format!("{LIST_PATH}/{}/content", drop.drop_id);
    let bytes = match cloud.send("GET", &path, Vec::new()).await {
        Ok((s, b)) if (200..300).contains(&s) => b,
        // Gone (acked by an earlier pull) or cloud-api unreachable: try again later.
        Ok(_) | Err(_) => return Ok(Outcome::Later),
    };
    if bytes.len() as u64 != drop.size || chunker::hash_bytes(&bytes) != drop.content_hash.to_ascii_lowercase() {
        return Ok(Outcome::Rejected("The bytes do not match the drop's size and content hash.".into()));
    }
    let name = safe_name(&drop.name);
    let guest = PrincipalRef { kind: PrincipalKind::Guest, id: drop.guest_id.clone(), display_name: None };
    let drop_id = drop.drop_id.clone();
    let written = run(files, move |v| {
        let inbox = v
            .drive
            .roots()?
            .into_iter()
            .find(|r| r.kind == RootKind::Inbox)
            .ok_or_else(|| ApiError::new(ErrorCode::RootNotFound, "This computer has no Inbox."))?;
        let m = v.drive.write_small(
            &inbox.root_id,
            &inbox.root_item_id,
            &name,
            &bytes,
            None,
            OnConflict::KeepBoth,
            &guest,
            &EditingDevice::default(),
        )?;
        let at = v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let c = v.meta.lock().expect("meta lock");
        c.execute_batch(SCHEMA)?;
        c.execute("INSERT OR REPLACE INTO delivered_drops(drop_id, item_id, at) VALUES(?1, ?2, ?3)", params![drop_id, m.item.id, at])?;
        Ok(m.item.id)
    })
    .await;
    Ok(match written {
        Ok(item_id) => Outcome::Delivered(item_id),
        Err(e) if matches!(e.body.code, ErrorCode::InvalidName | ErrorCode::PayloadTooLarge) => Outcome::Rejected(e.body.message),
        Err(e) => {
            tracing::debug!(drop_id = %drop.drop_id, error = ?e.body.message, "files v2: drop not written yet");
            Outcome::Later
        }
    })
}

/// Starts the pull loop (inert while the drive is off or the computer is not paired).
pub(crate) fn spawn(files: Arc<FilesV2>) {
    if !files.cfg.enabled || !files.cfg.watch {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let every = Duration::from_secs(std::env::var(EVERY_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(300).max(60));
    handle.spawn(async move {
        tokio::time::sleep(FIRST_PULL).await;
        loop {
            if cloud::paired() {
                match pull(&files, &SignedCloud).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(drops = n, "files v2: guest drops kept while asleep are in Inbox"),
                    Err(e) => tracing::debug!(error = ?e.body.message, "files v2: drop pull skipped"),
                }
            }
            tokio::time::sleep(every).await;
        }
    });
}
