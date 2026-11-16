//! The running provider for one sync root: cfapi callbacks, the change feed and
//! local write-back.
//!
//! Callbacks only copy what they need into a job and return; a small worker pool
//! does the network work and answers with CfExecute. That keeps Explorer
//! responsive and lets a wake prompt wait without blocking the filter.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use allternit_files_core::*;
use serde_json::json;
use windows::core::{Result as WinResult, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::CloudFilters::*;

use crate::cf::{self, NewPlaceholder};
use crate::client::{ApiError, FilesClient};
use crate::conflict::{self, AfterWrite, Badge};
use crate::identity::Identity;
use crate::index::{join_rel, parent_rel, rev_version, Entry, Index, RootMeta};
use crate::paths::{relative, safe_component};
use crate::ranges;
use crate::upload::{self, Target};
use crate::wake::{is_background_reader, WakeContext, WakeGate};

const WORKERS: usize = 4;
/// Change-feed poll interval while the computer is awake.
pub const FEED_INTERVAL: Duration = Duration::from_secs(15);
/// Poll interval while it sleeps (answers come from the snapshot).
pub const FEED_ASLEEP_INTERVAL: Duration = Duration::from_secs(60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Default)]
pub struct Stats {
    pub fetch_data: AtomicU64,
    pub fetch_placeholders: AtomicU64,
    pub bytes_hydrated: AtomicU64,
    pub uploads: AtomicU64,
}

pub struct Options {
    pub root: PathBuf,
    pub state_file: PathBuf,
    pub icons_dir: PathBuf,
    /// Custom badges (StorageProviderSyncRootManager registration only).
    pub badges: bool,
}

#[derive(Clone, Copy)]
struct Keys {
    conn: CF_CONNECTION_KEY,
    transfer: i64,
    request: i64,
}

// CF_CONNECTION_KEY is a plain i64 wrapper.
unsafe impl Send for Keys {}

enum Job {
    FetchData { keys: Keys, identity: Vec<u8>, path: PathBuf, size: u64, off: u64, len: u64, background: bool },
    FetchPlaceholders { keys: Keys, identity: Vec<u8>, path: PathBuf },
    Delete { keys: Keys, identity: Vec<u8>, path: PathBuf },
    Rename { keys: Keys, identity: Vec<u8>, path: PathBuf, target: PathBuf, target_in_scope: bool },
}

pub struct Provider {
    pub root: PathBuf,
    opts: Options,
    pub client: Arc<FilesClient>,
    pub wake: Arc<WakeGate>,
    pub index: Mutex<Index>,
    pub stats: Stats,
    jobs: Mutex<Option<Sender<Job>>>,
    cancels: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    /// Lower-cased full paths the helper itself is renaming or deleting, so their
    /// NOTIFY callbacks are acknowledged without a server call.
    self_ops: Mutex<HashSet<String>>,
    stop: AtomicBool,
}

/// What [`Provider::sync_path`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    Skipped,
    InSync,
    Uploaded { item_id: String },
    Conflict { conflict_name: String },
    Created { item_id: String },
    Pinned,
    Freed,
}

fn key_of(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

fn status_for(e: &ApiError) -> NTSTATUS {
    match e.code() {
        Some(ErrorCode::WakeConsentRequired | ErrorCode::WakeDenied | ErrorCode::Forbidden | ErrorCode::ScopeViolation)
        | Some(ErrorCode::ProtectedRootLocked | ErrorCode::Unauthorized | ErrorCode::FeatureDisabled) => STATUS_CLOUD_FILE_ACCESS_DENIED,
        Some(ErrorCode::Asleep | ErrorCode::Offline | ErrorCode::Waking) => STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE,
        Some(ErrorCode::RevMismatch) => STATUS_CLOUD_FILE_NOT_IN_SYNC,
        None if matches!(e, ApiError::Transport(_)) => STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE,
        _ => STATUS_CLOUD_FILE_UNSUCCESSFUL,
    }
}

fn item_identity(item: &Item, root_link_item: Option<&str>) -> Vec<u8> {
    let dir = item.kind == ItemKind::Folder;
    match (&item.root_link, root_link_item) {
        // A folder that is another root (Inbox inside Allternit): list it as that root.
        (Some(link), Some(root_item)) => Identity::item(link, root_item, &item.rev, true).encode(),
        (Some(link), None) => Identity::item(link, &item.id, &item.rev, true).encode(),
        _ => Identity::item(&item.root_id, &item.id, &item.rev, dir).encode(),
    }
}

fn dir_metadata() -> CF_FS_METADATA {
    let now = cf::filetime(&chrono::Utc::now().to_rfc3339());
    CF_FS_METADATA {
        BasicInfo: windows::Win32::Storage::FileSystem::FILE_BASIC_INFO {
            CreationTime: now,
            LastAccessTime: now,
            LastWriteTime: now,
            ChangeTime: now,
            FileAttributes: windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0,
        },
        FileSize: 0,
    }
}

unsafe fn wide(p: PCWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        p.to_string().unwrap_or_default()
    }
}

unsafe fn full_path(info: &CF_CALLBACK_INFO, p: PCWSTR) -> PathBuf {
    // NormalizedPath / TargetPath start at the volume root ("\Users\...").
    PathBuf::from(format!("{}{}", wide(info.VolumeDosName), wide(p)))
}

unsafe fn identity_of(info: &CF_CALLBACK_INFO) -> Vec<u8> {
    if info.FileIdentity.is_null() || info.FileIdentityLength == 0 {
        return Vec::new();
    }
    std::slice::from_raw_parts(info.FileIdentity as *const u8, info.FileIdentityLength as usize).to_vec()
}

unsafe fn provider_of<'a>(info: &CF_CALLBACK_INFO) -> &'a Provider {
    &*(info.CallbackContext as *const Provider)
}

unsafe fn keys_of(info: &CF_CALLBACK_INFO) -> Keys {
    Keys { conn: info.ConnectionKey, transfer: info.TransferKey, request: info.RequestKey }
}

unsafe fn execute(keys: Keys, ty: CF_OPERATION_TYPE, op: &mut CF_OPERATION_PARAMETERS) -> WinResult<()> {
    let oi = CF_OPERATION_INFO {
        StructSize: std::mem::size_of::<CF_OPERATION_INFO>() as u32,
        Type: ty,
        ConnectionKey: keys.conn,
        TransferKey: keys.transfer,
        CorrelationVector: std::ptr::null(),
        SyncStatus: std::ptr::null(),
        RequestKey: keys.request,
    };
    op.ParamSize = std::mem::size_of::<CF_OPERATION_PARAMETERS>() as u32;
    CfExecute(&oi, op)
}

fn transfer_data(keys: Keys, status: NTSTATUS, data: Option<&[u8]>, off: u64, len: u64) -> WinResult<()> {
    unsafe {
        let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        op.Anonymous.TransferData.Flags = CF_OPERATION_TRANSFER_DATA_FLAG_NONE;
        op.Anonymous.TransferData.CompletionStatus = status;
        op.Anonymous.TransferData.Buffer = data.map(|d| d.as_ptr() as *const c_void).unwrap_or(std::ptr::null());
        op.Anonymous.TransferData.Offset = off as i64;
        op.Anonymous.TransferData.Length = len as i64;
        execute(keys, CF_OPERATION_TYPE_TRANSFER_DATA, &mut op)
    }
}

fn transfer_placeholders(keys: Keys, status: NTSTATUS, infos: &mut [CF_PLACEHOLDER_CREATE_INFO]) -> WinResult<()> {
    unsafe {
        let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        let t = &mut op.Anonymous.TransferPlaceholders;
        // The folder's whole listing is in this one call: mark it fully populated;
        // the change feed keeps it current from now on.
        t.Flags = CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION;
        t.CompletionStatus = status;
        t.PlaceholderTotalCount = infos.len() as i64;
        t.PlaceholderArray = if infos.is_empty() { std::ptr::null_mut() } else { infos.as_mut_ptr() };
        t.PlaceholderCount = infos.len() as u32;
        execute(keys, CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS, &mut op)
    }
}

fn ack_delete(keys: Keys, status: NTSTATUS) {
    unsafe {
        let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        op.Anonymous.AckDelete.Flags = CF_OPERATION_ACK_DELETE_FLAG_NONE;
        op.Anonymous.AckDelete.CompletionStatus = status;
        let _ = execute(keys, CF_OPERATION_TYPE_ACK_DELETE, &mut op);
    }
}

fn ack_rename(keys: Keys, status: NTSTATUS) {
    unsafe {
        let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        op.Anonymous.AckRename.Flags = CF_OPERATION_ACK_RENAME_FLAG_NONE;
        op.Anonymous.AckRename.CompletionStatus = status;
        let _ = execute(keys, CF_OPERATION_TYPE_ACK_RENAME, &mut op);
    }
}

// ---- callbacks (keep them short: copy, enqueue, return)

unsafe extern "system" fn cb_fetch_data(info: *const CF_CALLBACK_INFO, params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let p = provider_of(info);
    let fd = (*params).Anonymous.FetchData;
    let image = if info.ProcessInfo.is_null() { String::new() } else { wide((*info.ProcessInfo).ImagePath) };
    lock(&p.cancels).insert(info.TransferKey, Arc::new(AtomicBool::new(false)));
    p.submit(Job::FetchData {
        keys: keys_of(info),
        identity: identity_of(info),
        path: full_path(info, info.NormalizedPath),
        size: info.FileSize.max(0) as u64,
        off: fd.RequiredFileOffset.max(0) as u64,
        len: fd.RequiredLength.max(0) as u64,
        background: is_background_reader(&image),
    });
}

unsafe extern "system" fn cb_cancel_fetch_data(info: *const CF_CALLBACK_INFO, _params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let p = provider_of(info);
    if let Some(flag) = lock(&p.cancels).get(&info.TransferKey) {
        flag.store(true, Ordering::SeqCst);
    }
}

unsafe extern "system" fn cb_fetch_placeholders(info: *const CF_CALLBACK_INFO, _params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let p = provider_of(info);
    p.submit(Job::FetchPlaceholders { keys: keys_of(info), identity: identity_of(info), path: full_path(info, info.NormalizedPath) });
}

unsafe extern "system" fn cb_notify_delete(info: *const CF_CALLBACK_INFO, _params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let p = provider_of(info);
    let path = full_path(info, info.NormalizedPath);
    if lock(&p.self_ops).remove(&key_of(&path)) {
        ack_delete(keys_of(info), STATUS_SUCCESS);
        return;
    }
    p.submit(Job::Delete { keys: keys_of(info), identity: identity_of(info), path });
}

unsafe extern "system" fn cb_notify_rename(info: *const CF_CALLBACK_INFO, params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let p = provider_of(info);
    let r = (*params).Anonymous.Rename;
    let path = full_path(info, info.NormalizedPath);
    if lock(&p.self_ops).remove(&key_of(&path)) {
        ack_rename(keys_of(info), STATUS_SUCCESS);
        return;
    }
    let target_in_scope = r.Flags.0 & CF_CALLBACK_RENAME_FLAG_TARGET_IN_SCOPE.0 != 0;
    p.submit(Job::Rename { keys: keys_of(info), identity: identity_of(info), path, target: full_path(info, r.TargetPath), target_in_scope });
}

unsafe extern "system" fn cb_notify_dehydrate(info: *const CF_CALLBACK_INFO, _params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let id = identity_of(info);
    let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
    op.Anonymous.AckDehydrate.Flags = CF_OPERATION_ACK_DEHYDRATE_FLAG_NONE;
    op.Anonymous.AckDehydrate.CompletionStatus = STATUS_SUCCESS;
    op.Anonymous.AckDehydrate.FileIdentity = if id.is_empty() { std::ptr::null() } else { id.as_ptr() as *const c_void };
    op.Anonymous.AckDehydrate.FileIdentityLength = id.len() as u32;
    let _ = execute(keys_of(info), CF_OPERATION_TYPE_ACK_DEHYDRATE, &mut op);
}

static CALLBACKS: [CF_CALLBACK_REGISTRATION; 7] = [
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_DATA, Callback: Some(cb_fetch_data) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_CANCEL_FETCH_DATA, Callback: Some(cb_cancel_fetch_data) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, Callback: Some(cb_fetch_placeholders) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NOTIFY_DELETE, Callback: Some(cb_notify_delete) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NOTIFY_RENAME, Callback: Some(cb_notify_rename) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NOTIFY_DEHYDRATE, Callback: Some(cb_notify_dehydrate) },
    CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NONE, Callback: None },
];

/// A live connection; disconnect before unregistering.
pub struct Connection {
    key: CF_CONNECTION_KEY,
    ctx: *const Provider,
    provider: Arc<Provider>,
    workers: Vec<JoinHandle<()>>,
}

// The raw context pointer is an Arc<Provider> we own.
unsafe impl Send for Connection {}

impl Connection {
    pub fn provider(&self) -> &Arc<Provider> {
        &self.provider
    }

    pub fn disconnect(self) -> WinResult<()> {
        let r = unsafe { CfDisconnectSyncRoot(self.key) };
        self.provider.stop.store(true, Ordering::SeqCst);
        lock(&self.provider.jobs).take();
        for w in self.workers {
            let _ = w.join();
        }
        // Balance the Arc::into_raw in connect (callbacks are gone now).
        unsafe { drop(Arc::from_raw(self.ctx)) };
        let _ = self.provider.save_index();
        r
    }
}

impl Provider {
    pub fn new(mut opts: Options, client: Arc<FilesClient>, wake: Arc<WakeGate>) -> Arc<Self> {
        opts.root = cf::long_path(&opts.root);
        let index = Index::load(&opts.state_file);
        Arc::new(Provider {
            root: opts.root.clone(),
            opts,
            client,
            wake,
            index: Mutex::new(index),
            stats: Stats::default(),
            jobs: Mutex::new(None),
            cancels: Mutex::new(HashMap::new()),
            self_ops: Mutex::new(HashSet::new()),
            stop: AtomicBool::new(false),
        })
    }

    pub fn save_index(&self) -> std::io::Result<()> {
        lock(&self.index).save(&self.opts.state_file)
    }

    fn submit(&self, job: Job) {
        let sent = lock(&self.jobs).as_ref().map(|tx| tx.send(job).is_ok()).unwrap_or(false);
        if !sent {
            crate::emit(&json!({"type": "log", "level": "warn", "message": "job dropped: provider stopping"}));
        }
    }

    /// Connects the callbacks and starts the workers.
    pub fn connect(self: &Arc<Self>) -> WinResult<Connection> {
        let (tx, rx) = channel::<Job>();
        *lock(&self.jobs) = Some(tx);
        let rx = Arc::new(Mutex::new(rx));
        let workers = (0..WORKERS)
            .map(|_| {
                let p = Arc::clone(self);
                let rx: Arc<Mutex<Receiver<Job>>> = Arc::clone(&rx);
                std::thread::spawn(move || loop {
                    let job = { lock(&rx).recv() };
                    match job {
                        Ok(job) => {
                            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.run_job(job)));
                            if r.is_err() {
                                crate::emit(&json!({"type": "log", "level": "error", "message": "worker job panicked"}));
                            }
                        }
                        Err(_) => break,
                    }
                })
            })
            .collect();
        let ctx = Arc::into_raw(Arc::clone(self));
        let flags = CF_CONNECT_FLAGS(CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO.0 | CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH.0);
        let key = unsafe {
            CfConnectSyncRoot(&HSTRING::from(self.root.as_os_str()), CALLBACKS.as_ptr(), Some(ctx as *const c_void), flags)
        };
        match key {
            Ok(key) => Ok(Connection { key, ctx, provider: Arc::clone(self), workers }),
            Err(e) => {
                unsafe { drop(Arc::from_raw(ctx)) };
                lock(&self.jobs).take();
                Err(e)
            }
        }
    }

    fn run_job(&self, job: Job) {
        match job {
            Job::FetchData { keys, identity, path, size, off, len, background } => {
                self.fetch_data(keys, &identity, &path, size, off, len, background);
                lock(&self.cancels).remove(&keys.transfer);
            }
            Job::FetchPlaceholders { keys, identity, path } => self.fetch_placeholders(keys, &identity, &path),
            Job::Delete { keys, identity, path } => self.notify_delete(keys, &identity, &path),
            Job::Rename { keys, identity, path, target, target_in_scope } => {
                self.notify_rename(keys, &identity, &path, &target, target_in_scope)
            }
        }
    }

    // ---- hydration

    #[allow(clippy::too_many_arguments)]
    fn fetch_data(&self, keys: Keys, identity: &[u8], path: &Path, size: u64, off: u64, len: u64, background: bool) {
        self.stats.fetch_data.fetch_add(1, Ordering::SeqCst);
        let (span_start, span_end) = ranges::covering(off, len, size);
        let fail = |from: u64, status: NTSTATUS| {
            if span_end > from {
                let _ = transfer_data(keys, status, None, from, span_end - from);
            }
        };
        let Some(Identity::Item { i: item_id, .. }) = Identity::decode(identity) else {
            return fail(span_start, STATUS_CLOUD_FILE_UNSUCCESSFUL);
        };
        let cancel = lock(&self.cancels).get(&keys.transfer).cloned().unwrap_or_default();
        // Keep the request alive while a wake prompt is up or a computer wakes.
        let done = Arc::new(AtomicU64::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let ticker = {
            let (done, alive) = (Arc::clone(&done), Arc::clone(&alive));
            std::thread::spawn(move || {
                let mut last = Instant::now();
                while alive.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    if last.elapsed() >= Duration::from_secs(10) {
                        last = Instant::now();
                        unsafe {
                            let _ = CfReportProviderProgress(keys.conn, keys.transfer, size as i64, done.load(Ordering::SeqCst) as i64);
                        }
                    }
                }
            })
        };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ctx = WakeContext { reason: WakeReason::OpenFile, item_id: Some(&item_id), what: format!("Opening \"{name}\""), background };
        for (o, l) in ranges::plan(off, len, size) {
            if cancel.load(Ordering::SeqCst) {
                break; // Windows already failed the request; nothing to answer.
            }
            match self.wake.run(&self.client, &ctx, |t| self.client.read_range(&item_id, o, l, t)) {
                Ok((bytes, _)) if bytes.len() as u64 == l => {
                    if transfer_data(keys, STATUS_SUCCESS, Some(&bytes), o, l).is_err() {
                        break;
                    }
                    done.fetch_add(l, Ordering::SeqCst);
                    self.stats.bytes_hydrated.fetch_add(l, Ordering::SeqCst);
                }
                Ok(_) => {
                    // Size on the server no longer matches the placeholder.
                    fail(o, STATUS_CLOUD_FILE_NOT_IN_SYNC);
                    break;
                }
                Err(e) => {
                    crate::emit(&json!({"type": "fetch_failed", "item_id": item_id, "error": e.to_string()}));
                    if e.is(ErrorCode::WakeConsentRequired) {
                        crate::emit(&json!({"type": "state", "computer_state": "asleep"}));
                    }
                    fail(o, status_for(&e));
                    break;
                }
            }
        }
        alive.store(false, Ordering::SeqCst);
        let _ = ticker.join();
    }

    // ---- population (never wakes: list is answered from the snapshot when asleep)

    fn root_item_of(&self, root_id: &str) -> Option<String> {
        lock(&self.index).roots.get(root_id).map(|r| r.root_item_id.clone())
    }

    fn root_kind(&self, root_id: &str) -> Option<RootKind> {
        let kind = lock(&self.index).roots.get(root_id).map(|r| r.kind.clone())?;
        serde_json::from_value(json!(kind)).ok()
    }

    /// The listing of one folder as placeholders (and index entries).
    fn listing(&self, identity: &Identity, rel: &str) -> Result<Vec<(NewPlaceholder, Option<Item>)>, ApiError> {
        let mut out = Vec::new();
        match identity {
            Identity::Top { .. } => {
                let roots = self.client.roots()?;
                let mut ix = lock(&self.index);
                for r in &roots.roots {
                    ix.roots.insert(
                        r.id.clone(),
                        RootMeta {
                            name: r.name.clone(),
                            kind: r.kind.as_str().to_string(),
                            root_item_id: r.root_item_id.clone(),
                            parent_root_id: r.parent_root_id.clone(),
                        },
                    );
                    ix.cursors.entry(r.id.clone()).or_insert_with(|| r.cursor.clone());
                }
                // Top level: roots that are not inside another root. Protected roots
                // (Memory Drive) need a device unlock, which Explorer cannot give.
                for r in roots.roots.iter().filter(|r| r.parent_root_id.is_none() && !r.policy.protected) {
                    let name = safe_component(&r.name);
                    let rev = "0-dir".to_string();
                    ix.put(&r.root_item_id, Entry { rel: join_rel(rel, &name), root_id: r.id.clone(), dir: true, rev: rev.clone() });
                    let id = Identity::item(&r.id, &r.root_item_id, &rev, true).encode();
                    out.push((NewPlaceholder::new(&name, id, dir_metadata(), true), None));
                }
            }
            Identity::Item { r, i, d: true, .. } => {
                let mut page: Option<String> = None;
                loop {
                    let resp = self.client.list(r, i, page.as_deref())?;
                    if resp.freshness.served_by == ServedBy::Snapshot {
                        crate::emit(&json!({"type": "state", "computer_state": resp.freshness.computer_state.as_str()}));
                    }
                    for it in resp.items {
                        let name = safe_component(&it.name);
                        let link_item = it.root_link.as_ref().and_then(|l| self.root_item_of(l));
                        let id = item_identity(&it, link_item.as_deref());
                        let dir = it.kind == ItemKind::Folder;
                        let (entry_root, entry_id) = match (&it.root_link, &link_item) {
                            (Some(l), Some(ri)) => (l.clone(), ri.clone()),
                            _ => (it.root_id.clone(), it.id.clone()),
                        };
                        lock(&self.index).put(&entry_id, Entry { rel: join_rel(rel, &name), root_id: entry_root, dir, rev: it.rev.clone() });
                        out.push((NewPlaceholder::new(&name, id, cf::fs_metadata(&it), dir), Some(it)));
                    }
                    match resp.next_page_token {
                        Some(t) => page = Some(t),
                        None => break,
                    }
                }
            }
            Identity::Item { .. } => {}
        }
        Ok(out)
    }

    fn fetch_placeholders(&self, keys: Keys, identity: &[u8], path: &Path) {
        self.stats.fetch_placeholders.fetch_add(1, Ordering::SeqCst);
        let rel = relative(&self.root, path).unwrap_or_default();
        let ident = Identity::decode(identity).unwrap_or_else(Identity::top);
        match self.listing(&ident, &rel) {
            Ok(list) => {
                let (holders, items): (Vec<NewPlaceholder>, Vec<Option<Item>>) = list.into_iter().unzip();
                let mut infos = cf::create_infos(&holders);
                let r = transfer_placeholders(keys, STATUS_SUCCESS, &mut infos);
                drop(infos);
                if r.is_ok() {
                    lock(&self.index).populated.insert(rel.clone());
                    let _ = self.save_index();
                    let items: Vec<Item> = items.into_iter().flatten().collect();
                    self.refresh_badges(path, &items);
                }
            }
            Err(e) => {
                crate::emit(&json!({"type": "list_failed", "path": rel, "error": e.to_string()}));
                let _ = transfer_placeholders(keys, status_for(&e), &mut []);
            }
        }
    }

    fn refresh_badges(&self, dir: &Path, items: &[Item]) {
        if !self.opts.badges {
            return;
        }
        for it in items {
            let b = conflict::badges_for(it, self.root_kind(&it.root_id).as_ref());
            if !b.is_empty() {
                let _ = crate::badges::set(&dir.join(safe_component(&it.name)), it.kind == ItemKind::Folder, &b, &self.opts.icons_dir);
            }
        }
    }

    // ---- local delete / rename seen by Windows

    fn is_root_folder(&self, item_id: &str) -> bool {
        lock(&self.index).roots.values().any(|r| r.root_item_id == item_id)
    }

    /// Sends a local delete to the server's trash. Err is the status to veto with.
    fn trash_on_server(&self, identity: &[u8], path: &Path) -> Result<(), NTSTATUS> {
        let Some(Identity::Item { i, rev, .. }) = Identity::decode(identity) else {
            return Err(STATUS_CLOUD_FILE_ACCESS_DENIED);
        };
        if self.is_root_folder(&i) {
            // A whole root is removed in the app, not by deleting its folder.
            return Err(STATUS_CLOUD_FILE_ACCESS_DENIED);
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ctx = WakeContext { reason: WakeReason::Sync, item_id: Some(&i), what: format!("Deleting \"{name}\""), background: false };
        let req = DeleteRequest { item_id: i.clone(), base_rev: Some(rev), permanent: false };
        match self.wake.run(&self.client, &ctx, |t| self.client.delete(&req, t)) {
            Ok(_) => {}
            Err(e) if e.is(ErrorCode::NotFound) => {}
            Err(e) => {
                crate::emit(&json!({"type": "delete_refused", "item_id": i, "error": e.to_string()}));
                return Err(status_for(&e));
            }
        }
        lock(&self.index).remove_tree(&i);
        let _ = self.save_index();
        Ok(())
    }

    fn notify_delete(&self, keys: Keys, identity: &[u8], path: &Path) {
        match self.trash_on_server(identity, path) {
            Ok(()) => ack_delete(keys, STATUS_SUCCESS),
            Err(status) => ack_delete(keys, status),
        }
    }

    fn notify_rename(&self, keys: Keys, identity: &[u8], path: &Path, target: &Path, target_in_scope: bool) {
        let Some(Identity::Item { r, i, rev, d, .. }) = Identity::decode(identity) else {
            return ack_rename(keys, STATUS_CLOUD_FILE_ACCESS_DENIED);
        };
        if self.is_root_folder(&i) {
            return ack_rename(keys, STATUS_CLOUD_FILE_ACCESS_DENIED);
        }
        if !target_in_scope {
            // Moved out of the sync root: the cloud copy goes to trash, like a delete.
            return match self.trash_on_server(identity, path) {
                Ok(()) => ack_rename(keys, STATUS_SUCCESS),
                Err(status) => ack_rename(keys, status),
            };
        }
        let Some(target_rel) = relative(&self.root, target) else { return ack_rename(keys, STATUS_CLOUD_FILE_ACCESS_DENIED) };
        let old_rel = relative(&self.root, path).unwrap_or_default();
        let new_name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let new_parent = {
            let ix = lock(&self.index);
            ix.by_rel(parent_rel(&target_rel)).map(|(id, _)| id.clone())
        };
        let Some(new_parent) = new_parent else {
            // The top level holds roots only.
            return ack_rename(keys, STATUS_CLOUD_FILE_ACCESS_DENIED);
        };
        let moved = !parent_rel(&target_rel).eq_ignore_ascii_case(parent_rel(&old_rel));
        let req = MoveRequest {
            item_id: i.clone(),
            base_rev: rev,
            new_parent_id: moved.then_some(new_parent),
            new_name: Some(new_name.clone()),
            on_conflict: OnConflict::Fail,
        };
        let ctx = WakeContext { reason: WakeReason::Sync, item_id: Some(&i), what: format!("Renaming \"{new_name}\""), background: false };
        match self.wake.run(&self.client, &ctx, |t| self.client.move_item(&req, t)) {
            Ok(resp) => {
                ack_rename(keys, STATUS_SUCCESS);
                {
                    let mut ix = lock(&self.index);
                    ix.move_tree(&i, &target_rel);
                    if let Some(e) = ix.items.get_mut(&i) {
                        e.rev = resp.item.rev.clone();
                    }
                }
                let _ = self.save_index();
                // The rename happens after the ack; then store the new rev.
                let id = Identity::item(&r, &i, &resp.item.rev, d).encode();
                for _ in 0..50 {
                    if target.exists() && cf::update(target, None, &id, false).is_ok() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            Err(e) => {
                crate::emit(&json!({"type": "rename_refused", "item_id": i, "error": e.to_string()}));
                ack_rename(keys, status_for(&e));
            }
        }
    }

    // ---- change feed (never wakes)

    fn full(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    fn remove_local(&self, rel: &str, dir: bool) {
        let p = self.full(rel);
        lock(&self.self_ops).insert(key_of(&p));
        let _ = if dir { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }

    fn rename_local(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        lock(&self.self_ops).insert(key_of(from));
        let r = std::fs::rename(from, to);
        if r.is_err() {
            lock(&self.self_ops).remove(&key_of(from));
        }
        r
    }

    /// Places or updates one server item locally (feed and conflict handling).
    fn place_item(&self, item: &Item) {
        let dir = item.kind == ItemKind::Folder;
        let known = lock(&self.index).items.get(&item.id).cloned();
        let parent = item.parent_id.as_ref().and_then(|pid| lock(&self.index).items.get(pid).cloned());
        let name = safe_component(&item.name);
        match (known, parent) {
            (Some(e), parent) => {
                // Our own write coming back, or an older change replayed after a newer one.
                let newer = match (rev_version(&e.rev), rev_version(&item.rev)) {
                    (Some(have), Some(got)) => got > have,
                    _ => e.rev != item.rev,
                };
                if !newer {
                    return;
                }
                // Moved or renamed?
                let want_rel = parent.map(|p| join_rel(&p.rel, &name)).unwrap_or_else(|| e.rel.clone());
                if !want_rel.eq_ignore_ascii_case(&e.rel) && self.rename_local(&self.full(&e.rel), &self.full(&want_rel)).is_ok() {
                    lock(&self.index).move_tree(&item.id, &want_rel);
                }
                let path = self.full(&want_rel);
                // A rename bumps meta_version but keeps the hash: keep the local bytes then.
                let content_changed = e.rev.split_once('-').map(|x| x.1) != item.rev.split_once('-').map(|x| x.1);
                match cf::placeholder_state(&path) {
                    // Local edits pending: leave them; their upload becomes a conflict copy.
                    Some(st) if !st.in_sync => {}
                    Some(_) => {
                        let id = item_identity(item, None);
                        if cf::update(&path, Some(&cf::fs_metadata(item)), &id, !dir && content_changed).is_ok() {
                            if let Some(x) = lock(&self.index).items.get_mut(&item.id) {
                                x.rev = item.rev.clone();
                            }
                        }
                    }
                    None => {}
                }
            }
            (None, Some(p)) => {
                if !lock(&self.index).populated.contains(&p.rel) {
                    return; // the folder lists it when it is opened
                }
                let link_item = item.root_link.as_ref().and_then(|l| self.root_item_of(l));
                let holder = NewPlaceholder::new(&name, item_identity(item, link_item.as_deref()), cf::fs_metadata(item), dir);
                if cf::create_placeholders(&self.full(&p.rel), std::slice::from_ref(&holder)).is_ok() {
                    lock(&self.index).put(&item.id, Entry { rel: join_rel(&p.rel, &name), root_id: item.root_id.clone(), dir, rev: item.rev.clone() });
                    self.refresh_badges(&self.full(&p.rel), std::slice::from_ref(item));
                }
            }
            (None, None) => {}
        }
    }

    fn apply_change(&self, change: &ChangeEntry) {
        match change.op {
            ChangeOp::Deleted | ChangeOp::Trashed => {
                let e = lock(&self.index).items.get(&change.item_id).cloned();
                if let Some(e) = e {
                    let p = self.full(&e.rel);
                    // Only drop what is in sync; local unsaved edits stay as a local file.
                    if cf::placeholder_state(&p).map(|s| s.in_sync).unwrap_or(false) {
                        self.remove_local(&e.rel, e.dir);
                    }
                    lock(&self.index).remove_tree(&change.item_id);
                }
            }
            _ => match &change.item {
                Some(item) => self.place_item(item),
                None => {
                    if let Ok(s) = self.client.stat(&change.item_id) {
                        self.place_item(&s.item);
                    }
                }
            },
        }
    }

    /// Re-lists every populated folder of a root after `reset_required`.
    fn resync_root(&self, root_id: &str) {
        let dirs: Vec<(String, String)> = {
            let ix = lock(&self.index);
            ix.items
                .iter()
                .filter(|(_, e)| e.dir && e.root_id == root_id && ix.populated.contains(&e.rel))
                .map(|(id, e)| (id.clone(), e.rel.clone()))
                .collect()
        };
        for (dir_id, dir_rel) in dirs {
            let Ok(resp) = self.client.list(root_id, &dir_id, None) else { continue };
            let server: HashSet<String> = resp.items.iter().map(|i| i.id.clone()).collect();
            let stale: Vec<(String, Entry)> = {
                let ix = lock(&self.index);
                ix.items
                    .iter()
                    .filter(|(id, e)| parent_rel(&e.rel).eq_ignore_ascii_case(&dir_rel) && !e.rel.is_empty() && !server.contains(*id) && e.root_id == root_id)
                    .map(|(id, e)| (id.clone(), e.clone()))
                    .collect()
            };
            for (id, e) in stale {
                if cf::placeholder_state(&self.full(&e.rel)).map(|s| s.in_sync).unwrap_or(false) {
                    self.remove_local(&e.rel, e.dir);
                }
                lock(&self.index).remove_tree(&id);
            }
            for it in &resp.items {
                self.place_item(it);
            }
        }
    }

    /// Reads one page set of changes for a root and applies them. Returns the
    /// number applied and the computer state the server reported.
    pub fn poll_once(&self, root_id: &str) -> Result<(usize, ComputerState), ApiError> {
        let mut applied = 0;
        loop {
            let Some(cursor) = lock(&self.index).cursors.get(root_id).cloned() else { return Ok((0, ComputerState::Awake)) };
            let resp = match self.client.changes(root_id, &cursor, 0) {
                Err(e) if e.is(ErrorCode::CursorExpired) => {
                    self.resync_root(root_id);
                    return Ok((applied, ComputerState::Awake));
                }
                other => other?,
            };
            if resp.reset_required {
                self.resync_root(root_id);
            } else {
                for c in &resp.changes {
                    self.apply_change(c);
                    applied += 1;
                }
            }
            lock(&self.index).cursors.insert(root_id.to_string(), resp.cursor.clone());
            let _ = self.save_index();
            if !resp.has_more || resp.reset_required {
                return Ok((applied, resp.freshness.computer_state));
            }
        }
    }

    /// The feed loop: polls every known root until `stop`.
    pub fn run_feed(self: &Arc<Self>) -> JoinHandle<()> {
        let p = Arc::clone(self);
        std::thread::spawn(move || {
            while !p.stop.load(Ordering::SeqCst) {
                let roots: Vec<String> = lock(&p.index).cursors.keys().cloned().collect();
                let mut asleep = false;
                for r in roots {
                    match p.poll_once(&r) {
                        Ok((_, ComputerState::Awake)) => {}
                        Ok((_, _)) => asleep = true,
                        Err(e) => {
                            asleep = asleep || e.is(ErrorCode::Asleep) || e.is(ErrorCode::Offline);
                            crate::emit(&json!({"type": "feed_error", "root_id": r, "error": e.to_string()}));
                        }
                    }
                }
                let wait = if asleep { FEED_ASLEEP_INTERVAL } else { FEED_INTERVAL };
                let until = Instant::now() + wait;
                while Instant::now() < until && !p.stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    // ---- local changes (watcher and tests)

    fn is_temp_name(name: &str) -> bool {
        name.starts_with("~$") || name.starts_with(".~") || name.starts_with(".allternit-") || name.ends_with(".tmp") || name.eq_ignore_ascii_case("desktop.ini") || name.eq_ignore_ascii_case("thumbs.db")
    }

    /// Looks at one local path and sends what changed: an edited file (upload with
    /// its base rev), a new file or folder, or a pin / free-up-space choice.
    pub fn sync_path(&self, path: &Path) -> Result<SyncOutcome, String> {
        let long = cf::long_path(path);
        let path = long.as_path();
        let Some(rel) = relative(&self.root, path) else { return Ok(SyncOutcome::Skipped) };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if rel.is_empty() || Self::is_temp_name(&name) || !path.exists() {
            return Ok(SyncOutcome::Skipped);
        }
        let is_dir = path.is_dir();
        let attrs = cf::attributes(path);
        match cf::placeholder_state(path) {
            Some(st) => {
                let Some(Identity::Item { r, i, rev, .. }) = Identity::decode(&st.identity) else { return Ok(SyncOutcome::Skipped) };
                if !is_dir && st.in_sync {
                    let dehydrated = attrs & cf::ATTR_RECALL_ON_DATA_ACCESS != 0;
                    if attrs & cf::ATTR_PINNED != 0 && dehydrated {
                        cf::hydrate(path).map_err(|e| e.to_string())?;
                        let _ = self.client.pin(&i, true);
                        return Ok(SyncOutcome::Pinned);
                    }
                    if attrs & cf::ATTR_UNPINNED != 0 && !dehydrated {
                        cf::dehydrate(path).map_err(|e| e.to_string())?;
                        let _ = self.client.pin(&i, false);
                        return Ok(SyncOutcome::Freed);
                    }
                }
                if st.in_sync {
                    return Ok(SyncOutcome::InSync);
                }
                if is_dir {
                    cf::set_in_sync(path).map_err(|e| e.to_string())?;
                    return Ok(SyncOutcome::InSync);
                }
                let parent_id = {
                    let ix = lock(&self.index);
                    ix.by_rel(parent_rel(&rel)).map(|(id, _)| id.clone())
                };
                let parent_id = parent_id.ok_or("parent folder unknown")?;
                let target = Target { root_id: r.clone(), parent_id, name: name.clone(), item_id: Some(i.clone()), base_rev: Some(rev) };
                self.write_back(path, &rel, &target, &r)
            }
            None => {
                let parent = {
                    let ix = lock(&self.index);
                    ix.by_rel(parent_rel(&rel)).map(|(id, e)| (id.clone(), e.clone()))
                };
                let Some((parent_id, parent_entry)) = parent else {
                    crate::emit(&json!({"type": "log", "level": "warn", "message": format!("{rel}: the top level holds roots only")}));
                    return Ok(SyncOutcome::Skipped);
                };
                let root_id = parent_entry.root_id.clone();
                if is_dir {
                    let ctx = WakeContext { reason: WakeReason::Upload, item_id: None, what: format!("Adding folder \"{name}\""), background: false };
                    let req = MkdirRequest { parent_id, name: name.clone(), on_conflict: OnConflict::KeepBoth };
                    let resp = self.wake.run(&self.client, &ctx, |t| self.client.mkdir(&req, t)).map_err(|e| e.to_string())?;
                    let final_path = self.adopt_name(path, &resp.item.name);
                    let final_rel = relative(&self.root, &final_path).unwrap_or(rel);
                    cf::convert(&final_path, &item_identity(&resp.item, None)).map_err(|e| e.to_string())?;
                    let mut ix = lock(&self.index);
                    ix.put(&resp.item.id, Entry { rel: final_rel.clone(), root_id, dir: true, rev: resp.item.rev.clone() });
                    ix.populated.insert(final_rel);
                    drop(ix);
                    let _ = self.save_index();
                    return Ok(SyncOutcome::Created { item_id: resp.item.id });
                }
                let target = Target { root_id: root_id.clone(), parent_id, name: name.clone(), item_id: None, base_rev: None };
                self.write_back(path, &rel, &target, &root_id)
            }
        }
    }

    /// The server may give a new item another name (`x (2).txt`); follow it locally.
    fn adopt_name(&self, path: &Path, server_name: &str) -> PathBuf {
        let local = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let want = safe_component(server_name);
        if want == local {
            return path.to_path_buf();
        }
        let to = path.with_file_name(&want);
        match std::fs::rename(path, &to) {
            Ok(()) => to,
            Err(_) => path.to_path_buf(),
        }
    }

    fn write_back(&self, path: &Path, rel: &str, target: &Target, root_id: &str) -> Result<SyncOutcome, String> {
        self.stats.uploads.fetch_add(1, Ordering::SeqCst);
        let resp = upload::upload_file(&self.client, &self.wake, path, target).map_err(|e| e.to_string())?;
        match conflict::after_write(&resp) {
            AfterWrite::Landed { item } => {
                let replacing = target.item_id.is_some();
                let final_path = if replacing { path.to_path_buf() } else { self.adopt_name(path, &item.name) };
                let final_rel = relative(&self.root, &final_path).unwrap_or_else(|| rel.to_string());
                let id = item_identity(&item, None);
                if replacing {
                    cf::update(&final_path, None, &id, false).map_err(|e| e.to_string())?;
                } else {
                    cf::convert(&final_path, &id).map_err(|e| e.to_string())?;
                }
                lock(&self.index).put(&item.id, Entry { rel: final_rel, root_id: root_id.to_string(), dir: false, rev: item.rev.clone() });
                let _ = self.save_index();
                Ok(if replacing { SyncOutcome::Uploaded { item_id: item.id } } else { SyncOutcome::Created { item_id: item.id } })
            }
            AfterWrite::Conflict { conflict_name, conflict_item, original_item_id } => {
                self.mirror_conflict(path, rel, &conflict_name, &conflict_item, &original_item_id)?;
                Ok(SyncOutcome::Conflict { conflict_name })
            }
        }
    }

    /// The server kept both: our bytes are now the conflict copy. Rename the local
    /// file to the server's conflict name and put the server's file back under the
    /// original name (SPEC section 4; no silent data loss on either side).
    fn mirror_conflict(&self, path: &Path, rel: &str, conflict_name: &str, conflict_item: &Item, original_item_id: &str) -> Result<(), String> {
        let copy_path = path.with_file_name(safe_component(conflict_name));
        self.rename_local(path, &copy_path).map_err(|e| e.to_string())?;
        cf::update(&copy_path, None, &item_identity(conflict_item, None), false).map_err(|e| e.to_string())?;
        let copy_rel = relative(&self.root, &copy_path).unwrap_or_default();
        lock(&self.index).put(
            &conflict_item.id,
            Entry { rel: copy_rel, root_id: conflict_item.root_id.clone(), dir: false, rev: conflict_item.rev.clone() },
        );
        if let Ok(orig) = self.client.stat(original_item_id) {
            let dir = path.parent().unwrap_or(self.root.as_path()).to_path_buf();
            let name = safe_component(&orig.item.name);
            let holder = NewPlaceholder::new(&name, item_identity(&orig.item, None), cf::fs_metadata(&orig.item), false);
            cf::create_placeholders(&dir, std::slice::from_ref(&holder)).map_err(|e| e.to_string())?;
            lock(&self.index).put(
                &orig.item.id,
                Entry { rel: join_rel(parent_rel(rel), &name), root_id: orig.item.root_id.clone(), dir: false, rev: orig.item.rev.clone() },
            );
        }
        let _ = self.save_index();
        if self.opts.badges {
            let _ = crate::badges::set(&copy_path, false, &[Badge::Conflict], &self.opts.icons_dir);
        }
        crate::emit(&json!({"type": "conflict", "name": conflict_name, "item_id": conflict_item.id, "original_item_id": original_item_id}));
        Ok(())
    }

    /// "Free up space" from the command line or Desktop.
    pub fn free_up(&self, path: &Path) -> Result<(), String> {
        let long = cf::long_path(path);
        let path = long.as_path();
        cf::dehydrate(path).map_err(|e| e.to_string())?;
        if let Some(Identity::Item { i, .. }) = cf::placeholder_state(path).and_then(|s| Identity::decode(&s.identity)) {
            let _ = self.client.pin(&i, false);
        }
        Ok(())
    }
}
