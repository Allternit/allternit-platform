//! The asleep snapshot writer (SPEC section 11, plan step 13 runtime side).
//!
//! While this computer is awake it leaves file names, sizes and revisions
//! (never contents) in the owner's R2 space through cloud-api
//! (`PUT /api/v1/computer-files/snapshot/manifest`, then
//! `.../snapshot/roots/:root_id`), so the relay can answer `roots`, `list`,
//! `stat`, `changes` and name search while it sleeps.
//!
//! - **Plain names** (Eoj 2026-11-15): `encryption: none`, `names_encrypted: false`.
//! - **Never protected roots** (Memory Drive), never `path_hint` or local paths.
//!   cloud-api drops them again at ingest; this is the first of two checks.
//! - **When.** On the sleep notice ([`before_sleep`], called from the
//!   runtime's SIGTERM handler, bounded by [`SLEEP_NOTICE_BUDGET`]) and at
//!   most every [`EVERY_ENV`] seconds (default 300) while changes are
//!   pending. A pass that finds every root's cursor unchanged sends nothing.
//! - **Only what changed.** The manifest goes out when any root or cursor
//!   changed (cloud-api needs it first: it refuses a root file the stored
//!   manifest does not list); then only the roots whose cursor moved.
//! - **Never activity.** These are outbound calls from an awake runtime;
//!   cloud-api's ingest routes never touch the idle clock.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use allternit_files_core::{ItemKind, SnapshotEncryption, SnapshotItem, SnapshotManifest, SnapshotRootFile};

use super::cloud::{self, CloudCalls, SignedCloud};
use super::routes::run;
use super::{roots, ApiError, Fail, FilesV2, Inner};

pub const MANIFEST_PATH: &str = "/api/v1/computer-files/snapshot/manifest";
pub const ROOT_PATH_PREFIX: &str = "/api/v1/computer-files/snapshot/roots/";
pub const MANIFEST_SCHEMA: &str = "files-snapshot/1";
pub const ROOT_SCHEMA: &str = "files-snapshot-root/1";
/// Seconds between snapshot passes while changes are pending (default 300, at least 60).
pub const EVERY_ENV: &str = "ALLTERNIT_FILES_SNAPSHOT_EVERY_S";
/// How long the sleep notice may spend writing the snapshot.
pub const SLEEP_NOTICE_BUDGET: Duration = Duration::from_secs(5);
/// cloud-api's limits are 1 MiB (manifest) and 8 MiB (root file); stay under them.
const MANIFEST_MAX_BYTES: usize = 1024 * 1024 - 16 * 1024;
const ROOT_MAX_BYTES: usize = 8 * 1024 * 1024 - 64 * 1024;
const TICK: Duration = Duration::from_secs(60);

/// What one pass would upload.
pub(crate) struct Built {
    pub manifest: SnapshotManifest,
    /// Every non-protected root, with its cursor.
    pub roots: Vec<(String, SnapshotRootFile)>,
}

/// The cursors last stored in R2, per root. Empty = nothing stored by this process.
#[derive(Default)]
pub(crate) struct Written {
    cursors: BTreeMap<String, String>,
    at: Option<Instant>,
}

/// What changed since [`Written`]: `(manifest, root ids)` to upload.
pub(crate) fn plan(last: &BTreeMap<String, String>, now: &BTreeMap<String, String>) -> (bool, Vec<String>) {
    let changed: Vec<String> = now.iter().filter(|(id, cur)| last.get(*id) != Some(*cur)).map(|(id, _)| id.clone()).collect();
    let manifest = !changed.is_empty() || last.keys().any(|id| !now.contains_key(id));
    (manifest, changed)
}

fn stamp(v: &Inner) -> String {
    v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The snapshot of every root this computer may show while asleep.
pub(crate) fn build(v: &Inner) -> Result<Built, Fail> {
    let at = stamp(v);
    let req = super::sync::system_req();
    let all = v.drive.roots()?;
    let mut wire_roots = Vec::new();
    let mut files = Vec::new();
    // Disk roots (whole-disk files, step 2.11) never leave the computer:
    // the cloud serves the snapshot without the whole-disk grant.
    for rec in all.iter().filter(|r| !r.policy.protected && !v.is_disk_root(r)) {
        let mut root = roots::to_wire(v, rec, &all, &req)?;
        root.path_hint = None;
        root.usage_bytes = None;
        let cursor = root.cursor.clone();
        files.push((
            rec.root_id.clone(),
            SnapshotRootFile {
                schema: ROOT_SCHEMA.into(),
                root_id: rec.root_id.clone(),
                cursor,
                written_at: at.clone(),
                encryption: SnapshotEncryption::None,
                names_encrypted: false,
                items: Some(items(v, &rec.root_item_id)?),
                ciphertext: None,
                nonce: None,
                key_id: None,
            },
        ));
        wire_roots.push(root);
    }
    let manifest = SnapshotManifest {
        schema: MANIFEST_SCHEMA.into(),
        computer_id: v.cfg.computer_id.clone(),
        written_at: at,
        encryption: SnapshotEncryption::None,
        names_encrypted: false,
        roots: Some(wire_roots),
        ciphertext: None,
        nonce: None,
        key_id: None,
    };
    Ok(Built { manifest, roots: files })
}

/// Every item under a root, breadth first (so a root too big for one file
/// keeps its top levels), up to [`ROOT_MAX_BYTES`] of JSON.
fn items(v: &Inner, root_item_id: &str) -> Result<Vec<SnapshotItem>, Fail> {
    let mut out = Vec::new();
    let mut bytes = 256usize;
    let mut queue = std::collections::VecDeque::from([root_item_id.to_string()]);
    while let Some(parent) = queue.pop_front() {
        for rec in v.drive.list(&parent)? {
            let item = SnapshotItem {
                id: rec.id.clone(),
                parent_id: rec.parent_id.clone(),
                name: rec.name.clone(),
                kind: rec.kind,
                size: rec.size,
                rev: rec.rev(),
                modified_at: rec.modified_at.clone(),
                root_link: rec.root_link.clone(),
            };
            bytes += serde_json::to_vec(&item).map(|b| b.len() + 1).unwrap_or(512);
            if bytes > ROOT_MAX_BYTES {
                tracing::info!(root_item = %root_item_id, items = out.len(), "files v2: snapshot root file is full; deeper items are left out");
                return Ok(out);
            }
            // A folder that opens another root is listed, not walked: that root has its own file.
            if rec.kind == ItemKind::Folder && rec.root_link.is_none() && !rec.link_escapes {
                queue.push_back(rec.id.clone());
            }
            out.push(item);
        }
    }
    Ok(out)
}

/// One pass: build, compare with what is stored, upload what changed.
/// Returns how many files went out.
pub(crate) async fn pass(files: &Arc<FilesV2>, cloud: &dyn CloudCalls, written: &Mutex<Written>) -> Result<usize, ApiError> {
    let built = run(files, |v| build(v)).await?;
    let now: BTreeMap<String, String> = built.roots.iter().map(|(id, f)| (id.clone(), f.cursor.clone())).collect();
    let (send_manifest, changed) = plan(&written.lock().expect("snapshot lock").cursors, &now);
    if !send_manifest && changed.is_empty() {
        return Ok(0);
    }
    let body = serde_json::to_vec(&built.manifest).map_err(|_| ApiError::internal())?;
    if body.len() > MANIFEST_MAX_BYTES {
        return Err(ApiError::new(allternit_files_core::ErrorCode::PayloadTooLarge, "The snapshot manifest is too big."));
    }
    let mut sent = 0;
    if send_manifest {
        put(cloud, MANIFEST_PATH, body).await?;
        sent += 1;
    }
    let mut stored = written.lock().expect("snapshot lock").cursors.clone();
    stored.retain(|id, _| now.contains_key(id));
    for (root_id, file) in built.roots.iter().filter(|(id, _)| changed.contains(id)) {
        let body = serde_json::to_vec(file).map_err(|_| ApiError::internal())?;
        match put(cloud, &format!("{ROOT_PATH_PREFIX}{root_id}"), body).await {
            Ok(()) => {
                stored.insert(root_id.clone(), file.cursor.clone());
                sent += 1;
            }
            // Keep the others going; this root is retried on the next pass.
            Err(e) => tracing::debug!(%root_id, error = ?e.body.message, "files v2: snapshot root not stored"),
        }
    }
    let mut w = written.lock().expect("snapshot lock");
    w.cursors = stored;
    w.at = Some(Instant::now());
    Ok(sent)
}

async fn put(cloud: &dyn CloudCalls, path: &str, body: Vec<u8>) -> Result<(), ApiError> {
    match cloud.send("PUT", path, body).await {
        Ok((s, _)) if (200..300).contains(&s) => Ok(()),
        Ok((s, b)) => Err(ApiError::new(
            allternit_files_core::ErrorCode::Offline,
            format!("cloud-api answered {s}: {}", String::from_utf8_lossy(&b).chars().take(200).collect::<String>()),
        )),
        Err(e) => Err(ApiError::new(allternit_files_core::ErrorCode::Offline, e)),
    }
}

fn written() -> &'static Mutex<Written> {
    static W: OnceLock<Mutex<Written>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(Written::default()))
}

static PROCESS: OnceLock<Arc<FilesV2>> = OnceLock::new();

/// Starts the snapshot loop for this process (inert while the drive is off or
/// the computer is not paired).
pub(crate) fn spawn(files: Arc<FilesV2>) {
    if !files.cfg.enabled || !files.cfg.watch {
        return;
    }
    let _ = PROCESS.set(files.clone());
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let every = Duration::from_secs(std::env::var(EVERY_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(300).max(60));
    handle.spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            if !cloud::paired() {
                continue;
            }
            let due = written().lock().expect("snapshot lock").at.is_none_or(|t| t.elapsed() >= every);
            if !due {
                continue;
            }
            if let Err(e) = pass(&files, &SignedCloud, written()).await {
                tracing::debug!(error = ?e.body.message, "files v2: snapshot pass skipped");
            }
        }
    });
}

/// The sleep notice: write what changed since the last pass, within
/// [`SLEEP_NOTICE_BUDGET`]. Called from the runtime's SIGTERM handler, which
/// is what a computer going to sleep (cloud stop, Desktop quit) sends.
pub async fn before_sleep() {
    let Some(files) = PROCESS.get().cloned() else { return };
    if !cloud::paired() {
        return;
    }
    match tokio::time::timeout(SLEEP_NOTICE_BUDGET, pass(&files, &SignedCloud, written())).await {
        Ok(Ok(n)) => tracing::info!(files = n, "files v2: snapshot written before sleep"),
        Ok(Err(e)) => tracing::warn!(error = ?e.body.message, "files v2: snapshot not written before sleep"),
        Err(_) => tracing::warn!("files v2: snapshot before sleep ran out of time"),
    }
}
