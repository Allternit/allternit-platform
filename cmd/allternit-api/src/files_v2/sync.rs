//! Sync on the runtime (plan step 15): replication passes, restore with a hash
//! check, cross-computer copy and move as jobs, the upload throttle, and what
//! the heartbeat reports.
//!
//! - **Replication never wakes a computer** (plan D14). A pass runs on the source
//!   only because its runtime is already running (awake for another reason): the
//!   loop ([`spawn_replicator`]) and `POST /replicas/:id/sync` make outbound calls
//!   only. A host that is asleep or offline answers 503 or not at all; the pass
//!   records `behind` and waits for the next tick. It never sends a wake ticket,
//!   never calls cloud-api's wake or activity paths, and its traffic does not
//!   reset idle on either end.
//! - **Transport.** Direct to the host's loopback or mesh URL (`host_base_url`)
//!   with this runtime's device token (the host's auth middleware introspects it
//!   against cloud-api and sees the same owner). A host reachable only through
//!   the cloud relay needs a no-wake, no-touch runtime-to-runtime relay entry in
//!   cloud-api first; until then such a replica stays `behind` with a message.
//! - **Bandwidth.** `ALLTERNIT_FILES_UPLOAD_KBPS` (KiB/s, unset or 0 = no limit)
//!   paces every byte this runtime pushes to another computer: replica blobs and
//!   cross-computer copies share one budget.
//! - **Selective sync.** A `pinned_only` replica carries only files pinned on
//!   some device (folders always). Client-side selective sync and queued offline
//!   writes need nothing new here: pins are `POST /pin`, and a queued write is an
//!   ordinary write with `base_rev`, `on_conflict: keep_both` and the device's
//!   `utc_offset_minutes`, so a replayed edit never overwrites a newer one.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use allternit_files_core::{
    ChunkRef, CopyRequest, CopyResponse, ErrorBody, ErrorCode, HaveNeedRequest, HaveNeedResponse, ItemKind, JobRef,
    JobRefKind, JobRefState, MkdirRequest, MutationResponse, OnConflict, PrincipalKind, PrincipalRef, ReplicaMode,
    ReplicaRestoreReport, ReplicaRestoreRequest, ReplicaState, StatResponse, StorageDiscovery,
    UploadChunksAppend, UploadCommitResponse, UploadCreateRequest, UploadStatus,
};
use allternit_files_engine::crypto::{ChunkSealer, FilesRootKey};
use allternit_files_engine::replica::{self, Catalog, CatalogEntry, HostStore, RestoreSink};
use allternit_files_engine::{chunker, FilesError, ItemRecord, RootRecord};
use axum::extract::rejection::JsonRejection;
use axum::extract::Path as UrlPath;
use axum::{Extension, Json};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::runtime::Handle;

use super::replicas::{self, Role, Row};
use super::routes::run;
use super::{memory, meta, ApiError, Fail, FilesV2, Inner, Need, Req};

type Files = Extension<Arc<FilesV2>>;

/// KiB/s for everything this runtime pushes to another computer.
pub const UPLOAD_KBPS_ENV: &str = "ALLTERNIT_FILES_UPLOAD_KBPS";
/// Seconds between replication passes (default 300).
pub const REPLICA_EVERY_ENV: &str = "ALLTERNIT_FILES_REPLICA_EVERY_S";
/// `0` stops the replication loop on this computer.
pub const REPLICATION_ENV: &str = "ALLTERNIT_FILES_REPLICATION";
const PAGE: usize = 10_000;

// ---- throttle ----

/// Paces bytes sent to other computers. Called on blocking threads only.
pub(crate) struct Throttle {
    bytes_per_sec: u64,
    next: Mutex<Instant>,
}

impl Throttle {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { bytes_per_sec, next: Mutex::new(Instant::now()) }
    }

    /// The one process-wide budget from [`UPLOAD_KBPS_ENV`].
    pub fn shared() -> Arc<Throttle> {
        static T: OnceLock<Arc<Throttle>> = OnceLock::new();
        T.get_or_init(|| {
            let kbps = std::env::var(UPLOAD_KBPS_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
            Arc::new(Throttle::new(kbps.saturating_mul(1024)))
        })
        .clone()
    }

    /// How long to wait before sending `n` bytes (and books them).
    pub fn reserve(&self, n: u64) -> Duration {
        if self.bytes_per_sec == 0 {
            return Duration::ZERO;
        }
        let mut next = self.next.lock().expect("throttle lock");
        let now = Instant::now();
        let start = (*next).max(now);
        *next = start + Duration::from_secs_f64(n as f64 / self.bytes_per_sec as f64);
        start.saturating_duration_since(now)
    }

    pub fn wait(&self, n: u64) {
        let d = self.reserve(n);
        if !d.is_zero() {
            std::thread::sleep(d);
        }
    }
}

// ---- talking to another computer ----

#[derive(Debug)]
pub(crate) enum TargetError {
    /// Asleep, offline, waking or not answering. Retried on a later pass; never woken.
    Unavailable(String),
    /// The other side refused (quota, bad request, wrong key...).
    Refused(ErrorCode, String),
}

impl From<TargetError> for Fail {
    fn from(e: TargetError) -> Self {
        match e {
            TargetError::Unavailable(m) => ApiError::new(ErrorCode::ReplicaUnavailable, format!("The other computer is not reachable now: {m}")).into(),
            TargetError::Refused(code, m) => ApiError::new(code, m).into(),
        }
    }
}

/// Another computer's `/api/v1/files/v2`, reached directly (loopback or mesh).
/// Sends only `authorization` and `content-type`: no wake ticket, ever.
pub(crate) struct HttpTarget {
    base: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl HttpTarget {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: crate::relay_auth::process_secret().device_token(),
            client: crate::net_client::client(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.client.request(method, format!("{}{path}", self.base)).timeout(Duration::from_secs(120));
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        r
    }

    async fn send(&self, r: reqwest::RequestBuilder) -> Result<reqwest::Response, TargetError> {
        let resp = r.send().await.map_err(|e| TargetError::Unavailable(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body: Option<ErrorBody> = resp.json().await.ok();
        let code = body.as_ref().map(|b| b.code).unwrap_or(ErrorCode::Internal);
        let msg = body.map(|b| b.message).unwrap_or_else(|| format!("HTTP {status}"));
        if status.as_u16() == 503 || matches!(code, ErrorCode::Asleep | ErrorCode::Offline | ErrorCode::Waking) {
            Err(TargetError::Unavailable(msg))
        } else {
            Err(TargetError::Refused(code, msg))
        }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, TargetError> {
        let resp = self.send(self.request(reqwest::Method::GET, path)).await?;
        resp.json().await.map_err(|e| TargetError::Refused(ErrorCode::Internal, e.to_string()))
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T, TargetError> {
        let resp = self.send(self.request(reqwest::Method::POST, path).json(body)).await?;
        resp.json().await.map_err(|e| TargetError::Refused(ErrorCode::Internal, e.to_string()))
    }

    pub async fn put_bytes(&self, path: &str, bytes: Vec<u8>) -> Result<(), TargetError> {
        let r = self.request(reqwest::Method::PUT, path).header("content-type", "application/octet-stream").body(bytes);
        self.send(r).await.map(|_| ())
    }

    /// `Ok(None)` on 404.
    pub async fn get_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, TargetError> {
        match self.send(self.request(reqwest::Method::GET, path)).await {
            Ok(resp) => resp.bytes().await.map(|b| Some(b.to_vec())).map_err(|e| TargetError::Unavailable(e.to_string())),
            Err(TargetError::Refused(ErrorCode::NotFound, _)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Where a replica's sealed copy lives, seen from this computer.
pub(crate) enum Dest<'a> {
    Http { target: HttpTarget, rt: &'a Handle, replica_id: String },
    /// This computer hosts it (a replica on another disk of the same machine).
    Local(HostStore),
}

fn local_err(e: FilesError) -> TargetError {
    TargetError::Refused(e.code(), e.to_string())
}

impl Dest<'_> {
    fn path(replica_id: &str, rest: &str) -> String {
        format!("/replicas/{replica_id}{rest}")
    }

    pub fn need(&self, locators: Vec<String>) -> Result<Vec<String>, TargetError> {
        let mut need = Vec::new();
        for page in locators.chunks(PAGE) {
            match self {
                Dest::Http { target, rt, replica_id } => {
                    let r: HaveNeedResponse =
                        rt.block_on(target.post_json(&Self::path(replica_id, "/have-need"), &HaveNeedRequest { hashes: page.to_vec() }))?;
                    need.extend(r.need);
                }
                Dest::Local(s) => need.extend(page.iter().filter(|l| !s.has_blob(l)).cloned()),
            }
        }
        Ok(need)
    }

    pub fn put_blob(&self, locator: &str, blob: Vec<u8>) -> Result<(), TargetError> {
        match self {
            Dest::Http { target, rt, replica_id } => rt.block_on(target.put_bytes(&Self::path(replica_id, &format!("/blobs/{locator}")), blob)),
            Dest::Local(s) => s.put_blob(locator, &blob, None).map(|_| ()).map_err(local_err),
        }
    }

    pub fn put_head(&self, head: Vec<u8>) -> Result<(), TargetError> {
        match self {
            Dest::Http { target, rt, replica_id } => rt.block_on(target.put_bytes(&Self::path(replica_id, "/head"), head)),
            Dest::Local(s) => s.put_head(&head, None).map(|_| ()).map_err(local_err),
        }
    }

    pub fn get_blob(&self, locator: &str) -> Result<Option<Vec<u8>>, TargetError> {
        match self {
            Dest::Http { target, rt, replica_id } => rt.block_on(target.get_bytes(&Self::path(replica_id, &format!("/blobs/{locator}")))),
            Dest::Local(s) => s.get_blob(locator).map_err(local_err),
        }
    }

    pub fn get_head(&self) -> Result<Option<Vec<u8>>, TargetError> {
        match self {
            Dest::Http { target, rt, replica_id } => match rt.block_on(target.get_bytes(&Self::path(replica_id, "/head"))) {
                Err(TargetError::Refused(ErrorCode::ReplicaUnavailable, _)) => Ok(None),
                other => other,
            },
            Dest::Local(s) => s.get_head().map_err(local_err),
        }
    }
}

/// The sealed copy of `replica_id` as this computer can reach it: hosted here,
/// or pushed to a `host_base_url` this computer recorded as the source.
fn locate<'a>(v: &Inner, rt: &'a Handle, replica_id: &str) -> Result<Dest<'a>, Fail> {
    let c = replicas::db(&v.cfg.data_dir)?;
    if let Some(row) = replicas::get(&c, replica_id, Role::Source)? {
        if let Some(base) = row.host_base_url {
            return Ok(Dest::Http { target: HttpTarget::new(&base), rt, replica_id: replica_id.to_string() });
        }
    }
    if replicas::get(&c, replica_id, Role::Host)?.is_some() {
        return Ok(Dest::Local(HostStore::open(v.cfg.data_dir.join("replica-host").join(replica_id))?));
    }
    Err(ApiError::new(ErrorCode::NotFound, "This computer does not know where that replica is.").into())
}

/// The principal replication, restore and the snapshot writer act as on this computer.
pub(crate) fn system_req() -> Req {
    Req {
        principal: PrincipalRef { kind: PrincipalKind::Owner, id: "files-replicator".into(), display_name: Some("Replica sync".into()) },
        user_id: String::new(),
        relayed: false,
        // The copy is sealed with the Files key, so protected roots replicate too.
        unlocked: true,
        device_name: Some("Replica sync".into()),
        guest: None,
    }
}

fn now(v: &Inner) -> String {
    v.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---- replication ----

fn all_chunks(v: &Inner, item_id: &str) -> Result<Vec<ChunkRef>, Fail> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let m = v.drive.manifest(item_id, token.as_deref())?;
        out.extend(m.chunks);
        match m.next_page_token {
            Some(t) => token = Some(t),
            None => return Ok(out),
        }
    }
}

fn entry(rec: &ItemRecord, chunks: Vec<ChunkRef>) -> CatalogEntry {
    CatalogEntry {
        item_id: rec.id.clone(),
        parent_id: rec.parent_id.clone(),
        name: rec.name.clone(),
        kind: rec.kind,
        size: rec.size,
        content_hash: rec.content_hash.clone(),
        rev: rec.rev(),
        modified_at: rec.modified_at.clone(),
        chunks,
    }
}

/// The root as it is now: folders before their children, files with their chunk
/// lists. Never follows a link into another root or outside this one.
fn build_catalog(v: &Inner, root: &RootRecord, row: &Row, at: &str) -> Result<Catalog, Fail> {
    let pinned_only = row.replica.mode == ReplicaMode::PinnedOnly;
    let root_item = v.drive.root_item(&root.root_id)?;
    let mut items = vec![entry(&root_item, Vec::new())];
    let mut stack = vec![root_item.id.clone()];
    while let Some(parent) = stack.pop() {
        for it in v.drive.list(&parent)? {
            if it.link_escapes || it.root_link.is_some() {
                continue;
            }
            match it.kind {
                ItemKind::Folder => {
                    items.push(entry(&it, Vec::new()));
                    stack.push(it.id.clone());
                }
                ItemKind::File => {
                    if pinned_only && meta::pins(&v.meta.lock().expect("meta lock"), &it.id)?.is_empty() {
                        continue;
                    }
                    let chunks = all_chunks(v, &it.id)?;
                    items.push(entry(&it, chunks));
                }
            }
        }
    }
    Ok(Catalog {
        replica_id: row.replica.replica_id.clone(),
        source_computer_id: row.replica.source_computer_id.clone(),
        root_id: root.root_id.clone(),
        root_item_id: root_item.id,
        cursor: Some(v.drive.current_cursor(&root.root_id)?),
        synced_at: at.to_string(),
        items,
    })
}

/// Pushes what the host lacks, then the sealed catalog and head.
fn push(v: &Inner, frk: &FilesRootKey, dest: &Dest<'_>, mut catalog: Catalog, throttle: &Throttle) -> Result<(Catalog, bool), Fail> {
    let sealer = ChunkSealer::for_replica(frk, &catalog.replica_id);
    let mut by_locator: HashMap<String, String> = HashMap::new();
    for e in &catalog.items {
        for c in &e.chunks {
            by_locator.entry(sealer.locator(&c.hash)?).or_insert_with(|| c.hash.clone());
        }
    }
    let need = dest.need(by_locator.keys().cloned().collect())?;
    let mut lost: HashSet<String> = HashSet::new();
    for locator in need {
        let Some(hash) = by_locator.get(&locator) else { continue };
        match v.drive.get_chunk(hash) {
            Ok(data) => {
                let (loc, blob) = replica::seal_blob(&sealer, &data)?;
                throttle.wait(blob.len() as u64);
                dest.put_blob(&loc, blob)?;
            }
            // The file changed under us: leave it out of this pass.
            Err(_) => {
                lost.insert(hash.clone());
            }
        }
    }
    let skipped = !lost.is_empty();
    if skipped {
        catalog.items.retain(|e| !e.chunks.iter().any(|c| lost.contains(&c.hash)));
    }
    let (head, parts) = replica::pack_catalog(&catalog)?;
    let sealed: Vec<(String, Vec<u8>)> = parts.iter().map(|(_, pt)| replica::seal_blob(&sealer, pt)).collect::<Result<_, _>>()?;
    let part_need: HashSet<String> = dest.need(sealed.iter().map(|(l, _)| l.clone()).collect())?.into_iter().collect();
    for (loc, blob) in sealed {
        if part_need.contains(&loc) {
            throttle.wait(blob.len() as u64);
            dest.put_blob(&loc, blob)?;
        }
    }
    dest.put_head(replica::seal_head(frk, &head)?)?;
    Ok((catalog, skipped))
}

/// One replication pass for a replica this computer is the source of. Records
/// the outcome on the row: `current`, `behind` (host unreachable, or a file
/// changed mid-pass), `paused` (policy) or `error`.
pub(crate) fn replicate(v: &Inner, rt: &Handle, replica_id: &str) -> Result<ReplicaState, Fail> {
    let c = replicas::db(&v.cfg.data_dir)?;
    let row = replicas::get(&c, replica_id, Role::Source)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This computer is not the source of that replica."))?;
    let record = |state: ReplicaState, msg: &str| replicas::set_source_state(&c, replica_id, state, None, Some(msg));
    let root = match replicas::check_source_root(v, &system_req(), &row.replica.root_id) {
        Ok(r) => r,
        Err(e) => {
            let e = v.fail(e);
            record(ReplicaState::Paused, &e.body.message)?;
            return Err(e.into());
        }
    };
    let outcome = (|| -> Result<(Catalog, bool, String), Fail> {
        let frk = replicas::files_key(v)?;
        let dest = locate(v, rt, replica_id)?;
        let at = now(v);
        let catalog = build_catalog(v, &root, &row, &at)?;
        let (catalog, skipped) = push(v, &frk, &dest, catalog, &Throttle::shared())?;
        Ok((catalog, skipped, frk.key_id().to_string()))
    })();
    match outcome {
        Ok((catalog, skipped, key_id)) => {
            let stored: u64 = catalog.items.iter().map(|e| e.size).sum();
            let state = if skipped { ReplicaState::Behind } else { ReplicaState::Current };
            let cursor = catalog.cursor.clone().unwrap_or_default();
            replicas::set_source_state(&c, replica_id, state, Some((&cursor, &catalog.synced_at, stored, &key_id)), None)?;
            Ok(state)
        }
        Err(e) => {
            let e = v.fail(e);
            let state = match e.body.code {
                ErrorCode::ReplicaUnavailable | ErrorCode::Asleep | ErrorCode::Offline | ErrorCode::Waking => ReplicaState::Behind,
                _ => ReplicaState::Error,
            };
            record(state, &e.body.message)?;
            Err(e.into())
        }
    }
}

/// `POST /replicas/:replica_id/sync`: a pass now, as a finished job.
pub(crate) async fn sync_now(Extension(files): Files, _req: Req, UrlPath(replica_id): UrlPath<String>) -> Result<Json<JobRef>, ApiError> {
    let rt = Handle::current();
    run(&files, move |v| {
        let c = replicas::db(&v.cfg.data_dir)?;
        let job = job_create(&c, JobRefKind::ReplicaSync, 0, &now(v))?;
        let (state, error) = match replicate(v, &rt, &replica_id) {
            Ok(_) => (JobRefState::Done, None),
            Err(e) => (JobRefState::Failed, Some(v.fail(e).body.code)),
        };
        job_update(&c, &job.job_id, state, 0, 0, error, &now(v))?;
        Ok(Json(job_get(&c, &job.job_id)?.expect("job just written")))
    })
    .await
}

/// The replication loop. Runs only inside an already-running runtime, so it can
/// never be the reason a computer is awake; it never wakes the host either.
pub(crate) fn spawn_replicator(files: Arc<FilesV2>) {
    if !files.cfg.enabled || !files.cfg.watch || std::env::var(REPLICATION_ENV).is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let Ok(handle) = Handle::try_current() else { return };
    let every = std::env::var(REPLICA_EVERY_ENV).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(300).max(30);
    handle.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(every)).await;
            let rt = Handle::current();
            let pass = run(&files, move |v| {
                for row in replicas::source_rows(v)? {
                    if let Err(e) = replicate(v, &rt, &row.replica.replica_id) {
                        tracing::debug!(replica = %row.replica.replica_id, error = ?v.fail(e).body.message, "files v2: replica pass did not finish");
                    }
                }
                Ok(())
            })
            .await;
            if let Err(e) = pass {
                tracing::debug!(error = ?e.body.message, "files v2: replication loop skipped a tick");
            }
        }
    });
}

// ---- restore ----

/// Writes restored files into a local root through the engine's upload path, so
/// every chunk and the whole file are checked again on commit.
struct DriveSink<'a> {
    v: &'a Inner,
    by: PrincipalRef,
    root_id: String,
    folders: HashMap<String, String>,
    sessions: HashMap<String, String>,
}

impl DriveSink<'_> {
    fn parent(&self, e: &CatalogEntry) -> allternit_files_engine::Result<String> {
        e.parent_id
            .as_ref()
            .and_then(|p| self.folders.get(p))
            .cloned()
            .ok_or_else(|| FilesError::NotFound(format!("the folder for {} was not restored", e.name)))
    }
}

impl RestoreSink for DriveSink<'_> {
    fn folder(&mut self, e: &CatalogEntry) -> allternit_files_engine::Result<()> {
        let parent = self.parent(e)?;
        let existing = self.v.drive.list(&parent)?.into_iter().find(|i| i.kind == ItemKind::Folder && i.name == e.name);
        let id = match existing {
            Some(i) => i.id,
            None => self.v.drive.mkdir(&parent, &e.name, OnConflict::KeepBoth, &self.by)?.item.id,
        };
        self.folders.insert(e.item_id.clone(), id);
        Ok(())
    }

    fn begin(&mut self, e: &CatalogEntry) -> allternit_files_engine::Result<()> {
        let parent = self.parent(e)?;
        let first = e.chunks.iter().take(PAGE).cloned().collect();
        let st = self.v.drive.create_upload(
            UploadCreateRequest {
                root_id: self.root_id.clone(),
                parent_id: parent,
                name: e.name.clone(),
                item_id: None,
                base_rev: None,
                size: e.size,
                content_hash: e.content_hash.clone().unwrap_or_else(|| allternit_files_engine::model::EMPTY_HASH.to_string()),
                chunker: chunker::params(),
                chunks: first,
                total_chunks: e.chunks.len() as u64,
                on_conflict: OnConflict::KeepBoth,
                modified_at: Some(e.modified_at.clone()),
                device_name: Some("Replica restore".into()),
                utc_offset_minutes: None,
            },
            &self.by,
        )?;
        for page in e.chunks.chunks(PAGE).skip(1) {
            self.v.drive.append_upload_chunks(&st.session_id, page)?;
        }
        self.sessions.insert(e.item_id.clone(), st.session_id);
        Ok(())
    }

    fn chunk(&mut self, e: &CatalogEntry, c: &ChunkRef, data: &[u8]) -> allternit_files_engine::Result<()> {
        let sid = self.sessions.get(&e.item_id).ok_or(FilesError::UploadSessionNotFound)?;
        self.v.drive.put_upload_chunk(sid, &c.hash, data)?;
        Ok(())
    }

    fn finish(&mut self, e: &CatalogEntry) -> allternit_files_engine::Result<()> {
        let sid = self.sessions.remove(&e.item_id).ok_or(FilesError::UploadSessionNotFound)?;
        self.v.drive.commit_upload(&sid)?;
        Ok(())
    }

    fn abort(&mut self, e: &CatalogEntry) {
        if let Some(sid) = self.sessions.remove(&e.item_id) {
            let _ = self.v.drive.abort_upload(&sid);
        }
    }
}

fn fetch_err(e: TargetError) -> FilesError {
    match e {
        TargetError::Unavailable(m) | TargetError::Refused(_, m) => FilesError::NotFound(m),
    }
}

/// `POST /replicas/restore`, on the computer the files go back to. It needs the
/// Files key; the host never decrypts. The report says `verified: true` only
/// when every chunk hash and every whole-file hash matched.
pub(crate) async fn restore_route(
    Extension(files): Files,
    req: Req,
    body: Result<Json<ReplicaRestoreRequest>, JsonRejection>,
) -> Result<Json<ReplicaRestoreReport>, ApiError> {
    let Json(body) = body?;
    let rt = Handle::current();
    run(&files, move |v| {
        if body.target_computer_id != v.cfg.computer_id {
            return Err(ApiError::invalid("Send a restore to the computer the files go back to.").into());
        }
        let frk = replicas::files_key(v)?;
        let dest = locate(v, &rt, &body.replica_id)?;
        let unavailable = |m: String| -> Fail { ApiError::new(ErrorCode::ReplicaUnavailable, m).into() };
        let head_bytes = dest.get_head()?.ok_or_else(|| unavailable("The replica has no copy yet.".into()))?;
        let head = replica::open_head(&frk, &body.replica_id, &head_bytes)?;
        let catalog = replica::read_catalog(&frk, &head, &mut |l: &str| dest.get_blob(l).map_err(fetch_err)).map_err(|e| match e {
            FilesError::NotFound(m) => unavailable(m),
            other => other.into(),
        })?;
        let root_id = body.target_root_id.clone().unwrap_or_else(|| catalog.root_id.clone());
        let root = v.guard(&req, &root_id, Need::Write)?;
        if memory::is_memory(&root) {
            return Err(memory::refuse("take a replica restore"));
        }
        let mut sink = DriveSink {
            v,
            by: req.principal.clone(),
            root_id: root.root_id.clone(),
            folders: HashMap::from([(catalog.root_item_id.clone(), root.root_item_id.clone())]),
            sessions: HashMap::new(),
        };
        let report = replica::restore(&frk, &catalog, body.item_ids.as_deref(), &mut |l: &str| dest.get_blob(l).map_err(fetch_err), &mut sink);
        Ok(Json(report))
    })
    .await
}

// ---- jobs ----

fn job_create(c: &Connection, kind: JobRefKind, total: u64, at: &str) -> rusqlite::Result<JobRef> {
    let id = format!("job_{}", hex::encode(rand::random::<[u8; 10]>()));
    c.execute(
        "INSERT INTO jobs(job_id, kind, state, done_bytes, total_bytes, created_at, updated_at) VALUES(?1,?2,'queued',0,?3,?4,?4)",
        params![id, kind.as_str(), total as i64, at],
    )?;
    Ok(JobRef { job_id: id, kind, state: JobRefState::Queued, done_bytes: 0, total_bytes: total, error: None })
}

fn job_update(c: &Connection, id: &str, state: JobRefState, done: u64, total: u64, error: Option<ErrorCode>, at: &str) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE jobs SET state = ?2, done_bytes = MAX(done_bytes, ?3), total_bytes = MAX(total_bytes, ?4), error = ?5, updated_at = ?6 WHERE job_id = ?1",
        params![id, state.as_str(), done as i64, total as i64, error.map(|e| e.as_str()), at],
    )?;
    Ok(())
}

fn parse<T: Copy>(all: &[T], name: fn(&T) -> &'static str, s: &str) -> Option<T> {
    all.iter().copied().find(|v| name(v) == s)
}

fn job_get(c: &Connection, id: &str) -> rusqlite::Result<Option<JobRef>> {
    c.query_row("SELECT kind, state, done_bytes, total_bytes, error FROM jobs WHERE job_id = ?1", params![id], |r| {
        let kind: String = r.get(0)?;
        let state: String = r.get(1)?;
        let error: Option<String> = r.get(4)?;
        Ok(JobRef {
            job_id: id.to_string(),
            kind: parse(JobRefKind::ALL, JobRefKind::as_str, &kind).unwrap_or(JobRefKind::Copy),
            state: parse(JobRefState::ALL, JobRefState::as_str, &state).unwrap_or(JobRefState::Failed),
            done_bytes: r.get::<_, i64>(2)?.max(0) as u64,
            total_bytes: r.get::<_, i64>(3)?.max(0) as u64,
            error: error.and_then(|e| parse(ErrorCode::ALL, ErrorCode::as_str, &e)),
        })
    })
    .optional()
}

/// `GET /jobs/:job_id`.
pub(crate) async fn job(files: &FilesV2, job_id: String) -> Result<Json<JobRef>, ApiError> {
    run(files, move |v| {
        let c = replicas::db(&v.cfg.data_dir)?;
        job_get(&c, &job_id)?.map(Json).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such job on this computer.").into())
    })
    .await
}

// ---- cross-computer copy and move ----

struct Planned {
    /// Names from the copied item down to this one, inclusive.
    path: Vec<String>,
    rec: ItemRecord,
    chunks: Vec<ChunkRef>,
}

fn plan(v: &Inner, top: &ItemRecord, top_name: &str) -> Result<Vec<Planned>, Fail> {
    let mut out = Vec::new();
    let chunks = if top.kind == ItemKind::File { all_chunks(v, &top.id)? } else { Vec::new() };
    out.push(Planned { path: vec![top_name.to_string()], rec: top.clone(), chunks });
    let mut i = 0;
    while i < out.len() {
        if out[i].rec.kind == ItemKind::Folder {
            let base = out[i].path.clone();
            for child in v.drive.list(&out[i].rec.id.clone())? {
                if child.link_escapes || child.root_link.is_some() {
                    continue;
                }
                let chunks = if child.kind == ItemKind::File { all_chunks(v, &child.id)? } else { Vec::new() };
                let mut path = base.clone();
                path.push(child.name.clone());
                out.push(Planned { path, rec: child, chunks });
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Pushes the plan to another computer's v2 routes (mkdir, upload session with
/// have/need, commit) and checks each committed content hash.
fn copy_worker(v: &Inner, rt: &Handle, c: &Connection, job_id: &str, body: &CopyRequest, base: &str, plan: &[Planned], total: u64) -> Result<(), Fail> {
    let t = HttpTarget::new(base);
    let throttle = Throttle::shared();
    let dest: StatResponse = rt.block_on(t.get_json(&format!("/items/{}", body.dest_parent_id)))?;
    let dest_root = dest.item.root_id;
    let mut folders: HashMap<Vec<String>, String> = HashMap::new();
    let mut done = 0u64;
    job_update(c, job_id, JobRefState::Running, 0, total, None, &now(v))?;
    for p in plan {
        let parent = if p.path.len() == 1 {
            body.dest_parent_id.clone()
        } else {
            folders.get(&p.path[..p.path.len() - 1]).cloned().ok_or_else(|| ApiError::internal())?
        };
        let name = p.path.last().cloned().unwrap_or_default();
        match p.rec.kind {
            ItemKind::Folder => {
                let m: MutationResponse =
                    rt.block_on(t.post_json("/mkdir", &MkdirRequest { parent_id: parent, name, on_conflict: body.on_conflict }))?;
                folders.insert(p.path.clone(), m.item.id);
            }
            ItemKind::File => {
                let content_hash = p.rec.content_hash.clone().unwrap_or_else(|| allternit_files_engine::model::EMPTY_HASH.to_string());
                let mut st: UploadStatus = rt.block_on(t.post_json(
                    "/uploads",
                    &UploadCreateRequest {
                        root_id: dest_root.clone(),
                        parent_id: parent,
                        name,
                        item_id: None,
                        base_rev: None,
                        size: p.rec.size,
                        content_hash: content_hash.clone(),
                        chunker: chunker::params(),
                        chunks: p.chunks.iter().take(PAGE).cloned().collect(),
                        total_chunks: p.chunks.len() as u64,
                        on_conflict: body.on_conflict,
                        modified_at: Some(p.rec.modified_at.clone()),
                        device_name: None,
                        utc_offset_minutes: None,
                    },
                ))?;
                for page in p.chunks.chunks(PAGE).skip(1) {
                    st = rt.block_on(t.post_json(&format!("/uploads/{}/chunks", st.session_id), &UploadChunksAppend { chunks: page.to_vec() }))?;
                }
                let need: HashSet<String> = st.need.iter().cloned().collect();
                let mut sent: HashSet<&str> = HashSet::new();
                for ch in &p.chunks {
                    if need.contains(&ch.hash) && sent.insert(ch.hash.as_str()) {
                        let data = v.drive.get_chunk(&ch.hash)?;
                        throttle.wait(data.len() as u64);
                        rt.block_on(t.put_bytes(&format!("/uploads/{}/chunks/{}", st.session_id, ch.hash), data))?;
                    }
                    done += ch.size;
                    job_update(c, job_id, JobRefState::Running, done, total, None, &now(v))?;
                }
                let r: UploadCommitResponse =
                    rt.block_on(t.post_json(&format!("/uploads/{}/commit", st.session_id), &serde_json::json!({})))?;
                if r.item.content_hash.as_deref().unwrap_or(allternit_files_engine::model::EMPTY_HASH) != content_hash {
                    return Err(ApiError::new(ErrorCode::IntegrityFailed, format!("{} arrived with different content.", p.rec.name)).into());
                }
            }
        }
    }
    Ok(())
}

/// `POST /copy` with another `dest_computer_id`: answers a `JobRef` at once and
/// copies in the background. With `delete_source` it is a move: the source goes
/// to the trash only after every file committed on the destination with the
/// same content hash.
pub(crate) async fn copy_to_computer(files: Arc<FilesV2>, req: Req, body: CopyRequest) -> Result<CopyResponse, ApiError> {
    let base = body
        .dest_base_url
        .as_deref()
        .ok_or_else(|| ApiError::invalid("dest_base_url is required for a copy to another computer."))
        .and_then(replicas::check_base_url)?;
    let inner = files.inner().await?;
    let rt = Handle::current();
    let job = {
        let inner = inner.clone();
        let req = req.clone();
        let body = body.clone();
        tokio::task::spawn_blocking(move || -> Result<(JobRef, Vec<Planned>, u64), ApiError> {
            let v = &*inner;
            let prep = (|| -> Result<_, Fail> {
                let (_, item) = v.guard_item(&req, &body.item_id, Need::Read)?;
                if body.delete_source == Some(true) {
                    v.guard_item(&req, &body.item_id, Need::Write)?;
                    if item.parent_id.is_none() {
                        return Err(ApiError::forbidden("A root folder can't be moved to another computer.").into());
                    }
                }
                let name = body.new_name.clone().unwrap_or_else(|| item.name.clone());
                let planned = plan(v, &item, &name)?;
                let total: u64 = planned.iter().map(|p| if p.rec.kind == ItemKind::File { p.rec.size } else { 0 }).sum();
                let kind = if body.delete_source == Some(true) { JobRefKind::Move } else { JobRefKind::Copy };
                let c = replicas::db(&v.cfg.data_dir)?;
                Ok((job_create(&c, kind, total, &now(v))?, planned, total))
            })();
            prep.map_err(|e| v.fail(e))
        })
        .await
        .map_err(|_| ApiError::internal())??
    };
    let (job_ref, planned, total) = job;
    let job_id = job_ref.job_id.clone();
    tokio::task::spawn_blocking(move || {
        let v = &*inner;
        let Ok(c) = replicas::db(&v.cfg.data_dir) else { return };
        let result = copy_worker(v, &rt, &c, &job_id, &body, &base, &planned, total).and_then(|()| {
            if body.delete_source == Some(true) {
                let rec = v.drive.stat(&body.item_id)?;
                v.drive.delete(&body.item_id, Some(rec.rev().as_str()), false, &req.principal)?;
            }
            Ok(())
        });
        let (state, error) = match result {
            Ok(()) => (JobRefState::Done, None),
            Err(e) => (JobRefState::Failed, Some(v.fail(e).body.code)),
        };
        let done = if state == JobRefState::Done { total } else { 0 };
        let _ = job_update(&c, &job_id, state, done, total, error, &now(v));
    });
    Ok(CopyResponse { item: None, job: Some(job_ref), cursor: None })
}

// ---- heartbeat (SPEC section 15) ----

static PROCESS: OnceLock<Arc<FilesV2>> = OnceLock::new();

pub(crate) fn register_process(files: &Arc<FilesV2>) {
    let _ = PROCESS.set(files.clone());
}

/// What the presence heartbeat carries for the Files drive under
/// `filesStorage` (SPEC section 15): this computer's `StorageDiscovery`
/// (volumes, bytes per root, replica-host capacity), or `None` when the drive
/// is off. cloud-api keeps the last one for the replica picker and placement.
/// Heartbeats never count as activity.
pub async fn heartbeat_report() -> Option<serde_json::Value> {
    let files = PROCESS.get()?.clone();
    if !files.cfg.enabled {
        return None;
    }
    let storage: StorageDiscovery = run(&files, |v| super::storage::discovery(v)).await.ok()?;
    serde_json::to_value(storage).ok()
}
