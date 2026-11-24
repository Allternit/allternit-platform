//! The drive: roots, the index DB, revisions, versions, trash and the change feed for
//! the files on one computer (SPEC sections 3, 4, 6).
//!
//! Disk is the source of truth for live content. The index gives every file and
//! folder a stable opaque ID, a revision and a manifest. Chunks are kept in the chunk
//! store only when something needs them later: an older version, a trashed file, or
//! an upload in progress (`retained` revisions hold refcounts).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use allternit_files_core::{
    ChangeOp, ChunkRef, HaveNeedResponse, ItemKind, Manifest, OnConflict, PrincipalRef, RootPolicy, Version,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::chunker::{self, Chunked};
use crate::db::{self, enum_str, from_json, to_json};
use crate::error::{FilesError, Result};
use crate::model::{
    local_principal, ChangeRecord, ChangesPage, ConflictRecord, ItemRecord, Mutation, RootRecord, RootSpec,
    TrashRecord,
};
use crate::naming::{conflict_copy_name, duplicate_name, validate_name, TEMP_PREFIX};
use crate::store::ChunkStore;

/// Largest small write (`PUT /roots/:root_id/content`).
pub const MAX_SMALL_WRITE: usize = chunker::MAX_BYTES as usize;
/// Most chunk refs per manifest page.
pub const MANIFEST_PAGE: usize = 10_000;
/// Relay request cap advertised in `UploadStatus.max_request_bytes`.
pub const RELAY_MAX_REQUEST_BYTES: u64 = 5_242_880;

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
pub type ChangeListener = Arc<dyn Fn(&ChangeRecord) + Send + Sync>;

#[derive(Clone)]
pub struct DriveOptions {
    /// Wall clock. Tests inject a fixed one.
    pub clock: Clock,
    /// Offset used for the time in conflict-copy names (the computer's local time).
    pub display_offset: FixedOffset,
    /// How long an upload session survives without a commit.
    pub upload_ttl: Duration,
    pub max_request_bytes: u64,
}

impl Default for DriveOptions {
    fn default() -> Self {
        Self {
            clock: Arc::new(Utc::now),
            display_offset: *chrono::Local::now().offset(),
            upload_ttl: Duration::hours(24),
            max_request_bytes: RELAY_MAX_REQUEST_BYTES,
        }
    }
}

/// The device making a write. Its name and UTC offset go into a keep-both conflict
/// copy's name, so the `HHmm` is the editing device's local time (SPEC section 4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EditingDevice {
    pub name: Option<String>,
    /// Minutes east of UTC on the editing device (e.g. -360 for US Central in winter).
    /// None falls back to the computer's `display_offset`.
    pub utc_offset_minutes: Option<i32>,
}

/// What `prune` removed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PruneReport {
    pub trash_purged: usize,
    pub versions_dropped: usize,
    pub uploads_expired: usize,
    pub chunks_deleted: usize,
}

pub struct Drive {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) store: ChunkStore,
    pub(crate) opts: DriveOptions,
    feed: (Mutex<u64>, Condvar),
    listener: RwLock<Option<ChangeListener>>,
}

pub(crate) fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>()))
}

fn mtime_ns(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

fn mtime_ts(md: &fs::Metadata) -> String {
    md.modified().map(|t| ts(DateTime::<Utc>::from(t))).unwrap_or_else(|_| ts(Utc::now()))
}

#[cfg(unix)]
fn inode(md: &fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    Some(md.ino() as i64)
}
#[cfg(not(unix))]
fn inode(_md: &fs::Metadata) -> Option<i64> {
    None
}

/// Rename, or copy then remove when source and destination are on different volumes.
fn rename_or_copy(src: &Path, dst: &Path) -> Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(18) => {
            copy_tree(src, dst)?;
            if src.is_dir() {
                fs::remove_dir_all(src)?;
            } else {
                fs::remove_file(src)?;
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
    } else {
        fs::copy(src, dst)?;
    }
    Ok(())
}

fn remove_path(p: &Path) -> Result<()> {
    let r = match fs::symlink_metadata(p) {
        Ok(md) if md.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    r.map_err(Into::into)
}

/// What the scanner found at one path.
#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    kind: ItemKind,
    size: u64,
    mtime_ns: i64,
    modified_at: String,
    inode: Option<i64>,
    root_link: Option<String>,
    link_escapes: bool,
}

fn probe(path: &Path, root_path: &Path, other_roots: &HashMap<PathBuf, String>) -> Result<Entry> {
    let md = fs::symlink_metadata(path)?;
    let mut e = Entry {
        path: path.to_path_buf(),
        kind: ItemKind::File,
        size: 0,
        mtime_ns: mtime_ns(&md),
        modified_at: mtime_ts(&md),
        inode: inode(&md),
        root_link: None,
        link_escapes: false,
    };
    if md.file_type().is_symlink() {
        // Followed only when it names a file inside this root (SPEC section 12).
        match fs::canonicalize(path) {
            Ok(t) if t.starts_with(root_path) && t.is_file() => {
                let tmd = fs::metadata(&t)?;
                e.size = tmd.len();
                e.mtime_ns = mtime_ns(&tmd);
                e.modified_at = mtime_ts(&tmd);
            }
            _ => e.link_escapes = true,
        }
    } else if md.is_dir() {
        e.kind = ItemKind::Folder;
        e.root_link = other_roots.get(path).cloned();
    } else {
        e.size = md.len();
    }
    Ok(e)
}

/// Where a new revision's chunks come from when it must be retained.
#[derive(Clone, Copy)]
enum Retain<'a> {
    No,
    /// Already in the chunk store (uploads, scans that stored while chunking).
    Store,
    /// Read the missing ranges from this file.
    File(&'a Path),
}

/// Everything one write does runs in one SQLite transaction through this context.
pub(crate) struct Ctx<'a> {
    pub(crate) c: &'a Connection,
    pub(crate) store: &'a ChunkStore,
    pub(crate) opts: &'a DriveOptions,
    pub(crate) now: DateTime<Utc>,
    pending: Vec<(String, u64)>,
}

impl Drive {
    /// Opens (or creates) the index DB and chunk store under `data_dir`.
    pub fn open(data_dir: impl AsRef<Path>, opts: DriveOptions) -> Result<Self> {
        let dir = data_dir.as_ref();
        fs::create_dir_all(dir)?;
        let conn = db::open(&dir.join("index.sqlite"))?;
        let store = ChunkStore::open(dir.join("chunks"))?;
        Ok(Self { conn: Mutex::new(conn), store, opts, feed: (Mutex::new(0), Condvar::new()), listener: RwLock::new(None) })
    }

    pub fn store(&self) -> &ChunkStore {
        &self.store
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.opts.clock)()
    }

    /// Called after commit for every change (publish `files.changed`, feed sockets).
    pub fn set_change_listener(&self, listener: Option<ChangeListener>) {
        *self.listener.write().expect("listener lock") = listener;
    }

    pub(crate) fn with_tx<T>(&self, f: impl FnOnce(&mut Ctx<'_>) -> Result<T>) -> Result<T> {
        let mut guard = self.conn.lock().expect("index lock");
        let tx = guard.transaction()?;
        let mut ctx = Ctx { c: &tx, store: &self.store, opts: &self.opts, now: (self.opts.clock)(), pending: Vec::new() };
        let out = f(&mut ctx)?;
        let pending = std::mem::take(&mut ctx.pending);
        drop(ctx);
        tx.commit()?;
        if !pending.is_empty() {
            let records: Vec<ChangeRecord> =
                pending.iter().filter_map(|(r, s)| load_change(&guard, r, *s).ok().flatten()).collect();
            drop(guard);
            {
                let mut gen = self.feed.0.lock().expect("feed lock");
                *gen += 1;
                self.feed.1.notify_all();
            }
            if let Some(l) = self.listener.read().expect("listener lock").clone() {
                for r in &records {
                    l(r);
                }
            }
        }
        Ok(out)
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock().expect("index lock");
        f(&guard)
    }

    // ---- roots ----

    /// Adds a root and indexes it. The folder is created if missing. Scoping of
    /// user-picked paths (home only, secrets deny-list) is the runtime's job before this.
    pub fn add_root(&self, spec: RootSpec) -> Result<RootRecord> {
        let name = validate_name(&spec.name)?;
        fs::create_dir_all(&spec.local_path)?;
        let path = fs::canonicalize(&spec.local_path)?;
        let root_id = new_id("rt_");
        self.with_tx(|ctx| {
            let exists: Option<String> = ctx
                .c
                .query_row("SELECT root_id FROM roots WHERE local_path = ?1", params![path.to_string_lossy()], |r| r.get(0))
                .optional()?;
            if let Some(id) = exists {
                return Err(FilesError::AlreadyExists(format!("{} is already root {id}", path.display())));
            }
            let now = ts(ctx.now);
            let root_item = new_id("it_");
            let epoch = format!("{:08x}", rand::random::<u32>());
            ctx.c.execute(
                "INSERT INTO roots(root_id, kind, name, local_path, policy, root_item_id, epoch, created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![root_id, enum_str(&spec.kind), name, path.to_string_lossy(), to_json(&spec.policy), root_item, epoch, now],
            )?;
            ctx.c.execute(
                "INSERT INTO items(item_id, root_id, parent_id, name, kind, size, meta_version, created_at, modified_at) VALUES(?1,?2,NULL,?3,'folder',0,1,?4,?4)",
                params![root_item, root_id, name, now],
            )?;
            Ok(())
        })?;
        self.scan_root(&root_id)?;
        // A root inside another root shows there as a link: rescan the parent folder.
        if let Some(parent) = path.parent() {
            self.reconcile_paths(&[parent.to_path_buf()])?;
        }
        self.root(&root_id)
    }

    /// Adds the roots in `specs` that are not there yet, deepest paths first, so the
    /// Allternit home root sees Inbox, Shared… as links from its first scan.
    pub fn ensure_roots(&self, mut specs: Vec<RootSpec>) -> Result<Vec<RootRecord>> {
        specs.sort_by_key(|s| std::cmp::Reverse(s.local_path.components().count()));
        for spec in specs {
            fs::create_dir_all(&spec.local_path)?;
            let path = fs::canonicalize(&spec.local_path)?;
            if !self.roots()?.iter().any(|r| r.local_path == path) {
                self.add_root(spec)?;
            }
        }
        self.roots()
    }

    /// Forgets a root. Files stay on disk.
    pub fn remove_root(&self, root_id: &str) -> Result<()> {
        let root = self.root(root_id)?;
        self.with_tx(|ctx| {
            let revs: Vec<i64> = {
                let mut s = ctx.c.prepare("SELECT r.rev_id FROM revisions r JOIN items i ON i.item_id = r.item_id WHERE i.root_id = ?1")?;
                let v = s.query_map(params![root_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
                v
            };
            for rev in revs {
                ctx.drop_revision(rev)?;
            }
            for t in ["items", "changes", "trash", "roots"] {
                ctx.c.execute(&format!("DELETE FROM {t} WHERE root_id = ?1"), params![root_id])?;
            }
            Ok(())
        })?;
        if let Some(parent) = root.local_path.parent() {
            self.reconcile_paths(&[parent.to_path_buf()])?;
        }
        Ok(())
    }

    pub fn update_root_policy(&self, root_id: &str, policy: RootPolicy) -> Result<RootRecord> {
        self.with_tx(|ctx| {
            db::root(ctx.c, root_id)?;
            ctx.c.execute("UPDATE roots SET policy = ?1 WHERE root_id = ?2", params![to_json(&policy), root_id])?;
            Ok(())
        })?;
        self.root(root_id)
    }

    pub fn roots(&self) -> Result<Vec<RootRecord>> {
        self.read(db::roots)
    }

    pub fn root(&self, root_id: &str) -> Result<RootRecord> {
        self.read(|c| db::root(c, root_id))
    }

    // ---- scanning ----

    /// Full reconcile of a root with the disk.
    pub fn scan_root(&self, root_id: &str) -> Result<usize> {
        self.with_tx(|ctx| {
            let root = db::root(ctx.c, root_id)?;
            let item = db::live_item(ctx.c, &root.root_item_id)?;
            let path = root.local_path.clone();
            ctx.scan(&root, vec![(item, path, true)])
        })
    }

    /// Reconciles the index around changed paths (from the watcher). Each path's parent
    /// folder is rescanned; new folders are scanned in full; renames and moves keep
    /// their item ID (matched by inode, then by content hash).
    pub fn reconcile_paths(&self, paths: &[PathBuf]) -> Result<usize> {
        let roots = self.roots()?;
        let mut by_root: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for p in paths {
            let p = canonical_lenient(p);
            let best = roots
                .iter()
                .filter(|r| p.starts_with(&r.local_path))
                .max_by_key(|r| r.local_path.components().count());
            if let Some(r) = best {
                by_root.entry(r.root_id.clone()).or_default().push(p);
            }
        }
        let mut total = 0;
        for (root_id, paths) in by_root {
            total += self.with_tx(|ctx| {
                let root = db::root(ctx.c, &root_id)?;
                let mut starts = Vec::new();
                let mut seen = HashSet::new();
                for p in &paths {
                    let mut targets = vec![p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone())];
                    if p.is_dir() {
                        targets.push(p.clone());
                    }
                    for t in targets {
                        let t = if t.starts_with(&root.local_path) { t } else { root.local_path.clone() };
                        let (item, path) = ctx.deepest_folder(&root, &t)?;
                        if seen.insert(item.id.clone()) {
                            starts.push((item, path, false));
                        }
                    }
                }
                ctx.scan(&root, starts)
            })?;
        }
        Ok(total)
    }

    // ---- reads ----

    pub fn stat(&self, item_id: &str) -> Result<ItemRecord> {
        self.read(|c| db::live_item(c, item_id))
    }

    pub fn list(&self, parent_id: &str) -> Result<Vec<ItemRecord>> {
        self.read(|c| {
            let parent = db::live_item(c, parent_id)?;
            if parent.kind != ItemKind::Folder {
                return Err(FilesError::InvalidRequest(format!("{parent_id} is not a folder")));
            }
            db::live_children(c, parent_id)
        })
    }

    pub fn root_item(&self, root_id: &str) -> Result<ItemRecord> {
        self.read(|c| {
            let root = db::root(c, root_id)?;
            db::live_item(c, &root.root_item_id)
        })
    }

    /// Absolute path of a live item. Owner-only; refuses symlinks that leave the root.
    pub fn local_path(&self, item_id: &str) -> Result<PathBuf> {
        self.read(|c| {
            let item = db::live_item(c, item_id)?;
            if item.link_escapes {
                return Err(FilesError::ScopeViolation(format!("{} links outside its root", item.name)));
            }
            item_path(c, &item)
        })
    }

    /// Opens a file's live content for reading (the runtime handles Range and ETag).
    pub fn open_content(&self, item_id: &str) -> Result<(ItemRecord, fs::File)> {
        let item = self.stat(item_id)?;
        if item.kind != ItemKind::File {
            return Err(FilesError::InvalidRequest(format!("{item_id} is a folder")));
        }
        let path = self.local_path(item_id)?;
        Ok((item, fs::File::open(path)?))
    }

    /// The current revision's manifest, 10 000 chunk refs per page (`p1.<index>`).
    pub fn manifest(&self, item_id: &str, page_token: Option<&str>) -> Result<Manifest> {
        self.read(|c| {
            let item = db::live_item(c, item_id)?;
            if item.kind != ItemKind::File || item.link_escapes {
                return Err(FilesError::InvalidRequest(format!("{item_id} has no content manifest")));
            }
            let start: usize = match page_token {
                None => 0,
                Some(t) => t
                    .strip_prefix("p1.")
                    .and_then(|n| n.parse().ok())
                    .ok_or_else(|| FilesError::InvalidRequest(format!("bad page_token {t:?}")))?,
            };
            let rev_id = current_rev_id(c, item_id)?;
            let refs = refs_of(c, rev_id)?;
            let end = (start + MANIFEST_PAGE).min(refs.len());
            Ok(Manifest {
                item_id: item.id.clone(),
                rev: item.rev(),
                size: item.size,
                content_hash: item.content_hash.clone().unwrap_or_else(|| crate::model::EMPTY_HASH.into()),
                chunker: chunker::params(),
                chunks: refs.get(start..end).unwrap_or_default().to_vec(),
                next_page_token: (end < refs.len()).then(|| format!("p1.{end}")),
            })
        })
    }

    /// One chunk's bytes: from the chunk store, else read from the live file that holds
    /// it (hash re-checked).
    pub fn get_chunk(&self, hash: &str) -> Result<Vec<u8>> {
        if self.store.has(hash) {
            match self.store.get(hash) {
                Ok(d) => return Ok(d),
                Err(FilesError::ChunkHashMismatch(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let sources: Vec<(String, u64, u64)> = self.read(|c| {
            let mut s = c.prepare(
                "SELECT i.item_id, rc.offset, rc.size FROM revision_chunks rc JOIN items i ON i.current_rev_id = rc.rev_id \
                 WHERE rc.hash = ?1 AND i.trash_id IS NULL AND i.link_escapes = 0 LIMIT 8",
            )?;
            let v = s
                .query_map(params![hash], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })?;
        for (item_id, offset, size) in sources {
            let Ok(path) = self.local_path(&item_id) else { continue };
            if let Ok(data) = read_range(&path, offset, size) {
                if chunker::hash_bytes(&data) == hash {
                    return Ok(data);
                }
            }
        }
        Err(FilesError::NotFound(format!("chunk {hash}")))
    }

    /// Which of `hashes` this computer already stores (dedup before an upload).
    pub fn have_need(&self, hashes: &[String]) -> Result<HaveNeedResponse> {
        let mut have = Vec::new();
        let mut need = Vec::new();
        let mut seen = HashSet::new();
        for h in hashes {
            if !chunker::is_hash(h) {
                return Err(FilesError::InvalidRequest(format!("{h:?} is not a blake3 hex hash")));
            }
            if seen.insert(h.clone()) {
                if self.store.has(h) {
                    have.push(h.clone());
                } else {
                    need.push(h.clone());
                }
            }
        }
        Ok(HaveNeedResponse { have, need })
    }

    // ---- writes ----

    /// Small write (at most 4 MiB): create, or replace at `base_rev` (SPEC section 4).
    #[allow(clippy::too_many_arguments)]
    pub fn write_small(
        &self,
        root_id: &str,
        parent_id: &str,
        name: &str,
        data: &[u8],
        base_rev: Option<&str>,
        on_conflict: OnConflict,
        by: &PrincipalRef,
        device: &EditingDevice,
    ) -> Result<Mutation> {
        if data.len() > MAX_SMALL_WRITE {
            return Err(FilesError::PayloadTooLarge(format!("small writes take at most {MAX_SMALL_WRITE} bytes; use an upload session")));
        }
        let chunked = chunker::chunk_bytes(data);
        self.with_tx(|ctx| {
            let parent = ctx.writable_folder(root_id, parent_id)?;
            let temp = item_path(ctx.c, &parent)?.join(format!("{TEMP_PREFIX}write-{}.part", rand::random::<u64>()));
            fs::write(&temp, data)?;
            let out = ctx.place_content(
                &parent,
                name,
                None,
                base_rev,
                on_conflict,
                by,
                device,
                Incoming { temp: &temp, chunked: &chunked, from_store: false, modified_at: None },
            );
            if out.is_err() {
                let _ = fs::remove_file(&temp);
            }
            out
        })
    }

    pub fn mkdir(&self, parent_id: &str, name: &str, on_conflict: OnConflict, by: &PrincipalRef) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let parent = db::live_item(ctx.c, parent_id)?;
            let parent = ctx.writable_folder(&parent.root_id, parent_id)?;
            let name = validate_name(name)?;
            let parent_path = item_path(ctx.c, &parent)?;
            let name = match db::child_by_name(ctx.c, &parent.id, &name)? {
                Some(existing) => match on_conflict {
                    OnConflict::KeepBoth => duplicate_name(&name, |n| ctx.name_taken(&parent, &parent_path, n)),
                    _ if existing.kind == ItemKind::Folder && on_conflict == OnConflict::Replace => {
                        let cursor = ctx.cursor(&parent.root_id)?;
                        return Ok(Mutation { item: existing, conflict: None, cursor });
                    }
                    _ => return Err(FilesError::AlreadyExists(name)),
                },
                None if parent_path.join(&name).exists() => match on_conflict {
                    OnConflict::KeepBoth => duplicate_name(&name, |n| ctx.name_taken(&parent, &parent_path, n)),
                    _ => return Err(FilesError::AlreadyExists(name)),
                },
                None => name,
            };
            let path = parent_path.join(&name);
            fs::create_dir(&path)?;
            let md = fs::symlink_metadata(&path)?;
            let item = ctx.insert_item(&parent, &name, ItemKind::Folder, 0, None, Some(by), &md, None, false)?;
            ctx.push_change(&parent.root_id, ChangeOp::Created, &item.id, None, by)?;
            let cursor = ctx.cursor(&parent.root_id)?;
            Ok(Mutation { item, conflict: None, cursor })
        })
    }

    /// Rename and/or move, also between roots on this computer. The item ID stays.
    pub fn move_item(
        &self,
        item_id: &str,
        base_rev: &str,
        new_parent_id: Option<&str>,
        new_name: Option<&str>,
        on_conflict: OnConflict,
        by: &PrincipalRef,
    ) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let item = db::live_item(ctx.c, item_id)?;
            let old_parent_id = item.parent_id.clone().ok_or_else(|| FilesError::Forbidden("a root folder can't be moved".into()))?;
            if item.rev() != base_rev {
                return Err(FilesError::RevMismatch { current: Box::new(item) });
            }
            ctx.require_write(&item.root_id)?;
            let dest_id = new_parent_id.unwrap_or(&old_parent_id);
            let dest = db::live_item(ctx.c, dest_id)?;
            let dest = ctx.writable_folder(&dest.root_id, dest_id)?;
            if item.kind == ItemKind::Folder && ctx.is_self_or_descendant(&dest.id, &item.id)? {
                return Err(FilesError::InvalidRequest("a folder can't move into itself".into()));
            }
            let name = validate_name(new_name.unwrap_or(&item.name))?;
            if dest.id == old_parent_id && name == item.name {
                let cursor = ctx.cursor(&item.root_id)?;
                return Ok(Mutation { item, conflict: None, cursor });
            }
            let src = item_path(ctx.c, &item)?;
            let dest_path = item_path(ctx.c, &dest)?;
            let same_file = |n: &str| -> bool {
                // A case-only rename on a case-insensitive disk is not a clash with itself.
                match (fs::symlink_metadata(dest_path.join(n)), fs::symlink_metadata(&src)) {
                    (Ok(a), Ok(b)) => inode(&a).is_some() && inode(&a) == inode(&b),
                    _ => false,
                }
            };
            let clash = db::child_by_name(ctx.c, &dest.id, &name)?.filter(|e| e.id != item.id);
            let name = if clash.is_some() || (dest_path.join(&name).exists() && !same_file(&name)) {
                match on_conflict {
                    OnConflict::KeepBoth => duplicate_name(&name, |n| ctx.name_taken(&dest, &dest_path, n) && !same_file(n)),
                    OnConflict::Replace => match clash {
                        Some(c) if c.kind == ItemKind::File && item.kind == ItemKind::File => {
                            ctx.trash_item(&c, by)?;
                            name
                        }
                        _ => return Err(FilesError::AlreadyExists(name)),
                    },
                    OnConflict::Fail => return Err(FilesError::AlreadyExists(name)),
                }
            } else {
                name
            };
            rename_or_copy(&src, &dest_path.join(&name))?;
            let now = ts(ctx.now);
            ctx.c.execute(
                "UPDATE items SET parent_id = ?1, name = ?2, meta_version = meta_version + 1, modified_by = ?3, modified_at = ?4 WHERE item_id = ?5",
                params![dest.id, name, to_json(by), now, item.id],
            )?;
            if dest.root_id != item.root_id {
                for id in ctx.subtree(&item.id, None)? {
                    ctx.c.execute("UPDATE items SET root_id = ?1 WHERE item_id = ?2", params![dest.root_id, id])?;
                }
                ctx.push_change(&item.root_id, ChangeOp::Deleted, &item.id, Some(&old_parent_id), by)?;
                ctx.push_change(&dest.root_id, ChangeOp::Moved, &item.id, Some(&old_parent_id), by)?;
            } else if dest.id == old_parent_id {
                ctx.push_change(&item.root_id, ChangeOp::Renamed, &item.id, None, by)?;
            } else {
                ctx.push_change(&item.root_id, ChangeOp::Moved, &item.id, Some(&old_parent_id), by)?;
            }
            let item = db::live_item(ctx.c, &item.id)?;
            let cursor = ctx.cursor(&item.root_id)?;
            Ok(Mutation { item, conflict: None, cursor })
        })
    }

    /// Copy on this computer (files keep their chunk list; folders copy recursively).
    pub fn copy_item(
        &self,
        item_id: &str,
        dest_parent_id: &str,
        new_name: Option<&str>,
        on_conflict: OnConflict,
        by: &PrincipalRef,
    ) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let item = db::live_item(ctx.c, item_id)?;
            if item.link_escapes {
                return Err(FilesError::ScopeViolation(format!("{} links outside its root", item.name)));
            }
            let dest = db::live_item(ctx.c, dest_parent_id)?;
            let dest = ctx.writable_folder(&dest.root_id, dest_parent_id)?;
            if item.kind == ItemKind::Folder && ctx.is_self_or_descendant(&dest.id, &item.id)? {
                return Err(FilesError::InvalidRequest("a folder can't be copied into itself".into()));
            }
            let dest_path = item_path(ctx.c, &dest)?;
            let name = validate_name(new_name.unwrap_or(&item.name))?;
            let name = if ctx.name_taken(&dest, &dest_path, &name) {
                match (on_conflict, db::child_by_name(ctx.c, &dest.id, &name)?) {
                    (OnConflict::KeepBoth, _) => duplicate_name(&name, |n| ctx.name_taken(&dest, &dest_path, n)),
                    (OnConflict::Replace, Some(c)) if c.kind == ItemKind::File && item.kind == ItemKind::File => {
                        ctx.trash_item(&c, by)?;
                        name
                    }
                    _ => return Err(FilesError::AlreadyExists(name)),
                }
            } else {
                name
            };
            let src = item_path(ctx.c, &item)?;
            let new = ctx.copy_rec(&item, &src, &dest, &dest_path, &name, by)?;
            let cursor = ctx.cursor(&dest.root_id)?;
            Ok(Mutation { item: new, conflict: None, cursor })
        })
    }

    /// Delete: to trash (restorable for `trash_days`), or permanent. Stale `base_rev`
    /// is always `rev_mismatch`. Returns the trash entry (if trashed) and the cursor.
    pub fn delete(
        &self,
        item_id: &str,
        base_rev: Option<&str>,
        permanent: bool,
        by: &PrincipalRef,
    ) -> Result<(Option<TrashRecord>, String)> {
        self.with_tx(|ctx| {
            let item = db::live_item(ctx.c, item_id)?;
            if item.parent_id.is_none() {
                return Err(FilesError::Forbidden("a root folder can't be deleted; remove the root".into()));
            }
            if let Some(br) = base_rev {
                if item.rev() != br {
                    return Err(FilesError::RevMismatch { current: Box::new(item) });
                }
            }
            let root = ctx.require_write(&item.root_id)?;
            let trashed = if permanent || root.policy.trash_days == 0 {
                let path = item_path(ctx.c, &item)?;
                ctx.delete_rows(&item.id, None)?;
                remove_path(&path)?;
                ctx.push_change(&item.root_id, ChangeOp::Deleted, &item.id, item.parent_id.as_deref(), by)?;
                None
            } else {
                Some(ctx.trash_item(&item, by)?)
            };
            Ok((trashed, ctx.cursor(&item.root_id)?))
        })
    }

    pub fn trash_list(&self, root_id: &str) -> Result<Vec<TrashRecord>> {
        self.read(|c| {
            db::root(c, root_id)?;
            let ids: Vec<String> = {
                let mut s = c.prepare("SELECT trash_id FROM trash WHERE root_id = ?1 ORDER BY trashed_at DESC")?;
                let v = s.query_map(params![root_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
                v
            };
            ids.iter().map(|t| trash_record(c, t)).collect()
        })
    }

    /// Restores a trash entry to its old folder (or the root folder if that is gone).
    pub fn restore(&self, trash_id: &str, on_conflict: OnConflict, by: &PrincipalRef) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let rec = trash_record(ctx.c, trash_id)?;
            let root = ctx.require_write(&rec.item.root_id)?;
            let parent = match db::live_item(ctx.c, &rec.original_parent_id) {
                Ok(p) if p.kind == ItemKind::Folder => p,
                _ => db::live_item(ctx.c, &root.root_item_id)?,
            };
            let parent_path = item_path(ctx.c, &parent)?;
            let mut name = rec.item.name.clone();
            if ctx.name_taken(&parent, &parent_path, &name) {
                name = match (on_conflict, db::child_by_name(ctx.c, &parent.id, &name)?) {
                    (OnConflict::KeepBoth, _) => duplicate_name(&name, |n| ctx.name_taken(&parent, &parent_path, n)),
                    (OnConflict::Replace, Some(c)) if c.kind == ItemKind::File && rec.item.kind == ItemKind::File => {
                        ctx.trash_item(&c, by)?;
                        name
                    }
                    _ => return Err(FilesError::AlreadyExists(name)),
                };
            }
            ctx.materialize(&rec.item, &parent_path.join(&name), trash_id)?;
            let now = ts(ctx.now);
            ctx.c.execute(
                "UPDATE items SET parent_id = ?1, name = ?2, meta_version = meta_version + 1, modified_at = ?3, modified_by = ?4 WHERE item_id = ?5",
                params![parent.id, name, now, to_json(by), rec.item.id],
            )?;
            for id in ctx.subtree(&rec.item.id, Some(trash_id))? {
                ctx.c.execute("UPDATE items SET trash_id = NULL WHERE item_id = ?1", params![id])?;
                if let Some(item) = db::item_opt(ctx.c, &id)? {
                    if item.kind == ItemKind::File {
                        let path = item_path(ctx.c, &item)?;
                        ctx.record_disk(&id, &path)?;
                        if root.policy.versions.keep_count == 0 {
                            if let Some(rev) = current_rev_id_opt(ctx.c, &id)? {
                                ctx.unretain(rev)?;
                            }
                        }
                    }
                }
            }
            ctx.c.execute("DELETE FROM trash WHERE trash_id = ?1", params![trash_id])?;
            ctx.push_change(&root.root_id, ChangeOp::Restored, &rec.item.id, None, by)?;
            let item = db::live_item(ctx.c, &rec.item.id)?;
            Ok(Mutation { item, conflict: None, cursor: ctx.cursor(&root.root_id)? })
        })
    }

    /// Older versions of a file, newest first (the current revision is not listed).
    pub fn versions(&self, item_id: &str) -> Result<Vec<Version>> {
        self.read(|c| {
            let item = db::live_item(c, item_id)?;
            let cur = current_rev_id_opt(c, item_id)?.unwrap_or(-1);
            let mut s = c.prepare(
                "SELECT rev_id, meta_version, content_hash, size, created_at, created_by, label FROM revisions \
                 WHERE item_id = ?1 AND retained = 1 AND rev_id != ?2 ORDER BY rev_id DESC",
            )?;
            let rows = s
                .query_map(params![item.id, cur], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter()
                .map(|(id, mv, hash, size, at, by, label)| {
                    Ok(Version {
                        version_id: format!("v_{id}"),
                        rev: format!("{mv}-{hash}"),
                        size: size as u64,
                        content_hash: hash,
                        created_at: at,
                        created_by: by.as_deref().map(from_json).transpose()?.unwrap_or_else(local_principal),
                        label,
                    })
                })
                .collect()
        })
    }

    /// Makes an older version current again (the replaced content becomes a version).
    pub fn restore_version(&self, item_id: &str, version_id: &str, base_rev: &str, by: &PrincipalRef) -> Result<Mutation> {
        self.with_tx(|ctx| {
            let item = db::live_item(ctx.c, item_id)?;
            if item.rev() != base_rev {
                return Err(FilesError::RevMismatch { current: Box::new(item) });
            }
            let root = ctx.require_write(&item.root_id)?;
            let rev_id: i64 = version_id
                .strip_prefix("v_")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| FilesError::NotFound(format!("version {version_id}")))?;
            let (owner, retained, hash, size): (String, i64, String, i64) = ctx
                .c
                .query_row("SELECT item_id, retained, content_hash, size FROM revisions WHERE rev_id = ?1", params![rev_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()?
                .ok_or_else(|| FilesError::NotFound(format!("version {version_id}")))?;
            if owner != item.id || retained == 0 {
                return Err(FilesError::NotFound(format!("version {version_id}")));
            }
            let refs = refs_of(ctx.c, rev_id)?;
            let path = item_path(ctx.c, &item)?;
            let temp = path.with_file_name(format!("{TEMP_PREFIX}restore-{}.part", rand::random::<u64>()));
            ctx.assemble(&refs, size as u64, &hash, &temp)?;
            fs::rename(&temp, &path)?;
            let chunked = Chunked { chunks: refs, size: size as u64, content_hash: hash };
            ctx.replace_content(&root, &item, &chunked, Retain::Store, by, None)?;
            ctx.record_disk(&item.id, &path)?;
            ctx.push_change(&item.root_id, ChangeOp::Modified, &item.id, None, by)?;
            let item = db::live_item(ctx.c, &item.id)?;
            Ok(Mutation { item, conflict: None, cursor: ctx.cursor(&root.root_id)? })
        })
    }

    // ---- housekeeping ----

    /// Purges expired trash, applies version limits, expires stale upload sessions and
    /// deletes chunks nothing holds. Safe to run any time; the runtime runs it while awake.
    pub fn prune(&self) -> Result<PruneReport> {
        let mut report = self.with_tx(|ctx| {
            let mut report = PruneReport::default();
            let now = ts(ctx.now);
            let due: Vec<(String, String)> = {
                let mut s = ctx.c.prepare("SELECT trash_id, item_id FROM trash WHERE purge_at <= ?1")?;
                let v = s.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                v
            };
            for (trash_id, item_id) in due {
                ctx.delete_rows(&item_id, Some(&trash_id))?;
                ctx.c.execute("DELETE FROM trash WHERE trash_id = ?1", params![trash_id])?;
                report.trash_purged += 1;
            }
            let items: Vec<(String, String)> = {
                let mut s = ctx.c.prepare(
                    "SELECT DISTINCT i.item_id, i.root_id FROM revisions r JOIN items i ON i.item_id = r.item_id \
                     WHERE r.rev_id != IFNULL(i.current_rev_id, -1) AND i.trash_id IS NULL",
                )?;
                let v = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                v
            };
            for (item_id, root_id) in items {
                let policy = db::root(ctx.c, &root_id)?.policy;
                report.versions_dropped += ctx.prune_versions(&item_id, &policy)?;
            }
            report.uploads_expired = ctx.c.execute(
                "UPDATE uploads SET state = 'expired' WHERE state = 'open' AND expires_at <= ?1",
                params![now],
            )?;
            ctx.c.execute(
                "DELETE FROM upload_chunks WHERE session_id IN (SELECT session_id FROM uploads WHERE state != 'open')",
                [],
            )?;
            Ok(report)
        })?;
        report.chunks_deleted = self.gc()?;
        Ok(report)
    }

    /// Deletes stored chunks with no refcount that no open upload session lists.
    pub fn gc(&self) -> Result<usize> {
        let dead: Vec<String> = self.with_tx(|ctx| {
            let v: Vec<String> = {
                let mut s = ctx.c.prepare(
                    "SELECT hash FROM chunks WHERE refcount <= 0 AND hash NOT IN (SELECT hash FROM upload_chunks)",
                )?;
                let v = s.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
                v
            };
            for h in &v {
                ctx.c.execute("DELETE FROM chunks WHERE hash = ?1", params![h])?;
            }
            Ok(v)
        })?;
        for h in &dead {
            self.store.remove(h)?;
        }
        Ok(dead.len())
    }

    /// Stored refcount for a chunk (0 when unknown).
    pub fn chunk_refcount(&self, hash: &str) -> Result<u64> {
        self.read(|c| {
            Ok(c.query_row("SELECT refcount FROM chunks WHERE hash = ?1", params![hash], |r| r.get::<_, i64>(0))
                .optional()?
                .unwrap_or(0)
                .max(0) as u64)
        })
    }

    // ---- change feed ----

    pub fn current_cursor(&self, root_id: &str) -> Result<String> {
        self.read(|c| cursor_for(c, root_id))
    }

    /// Changes after `cursor` (SPEC section 6). `None` starts at the current position.
    pub fn changes(&self, root_id: &str, cursor: Option<&str>, limit: usize) -> Result<ChangesPage> {
        self.read(|c| {
            let root = db::root(c, root_id)?;
            let (epoch, next_seq, min_seq): (String, i64, i64) = c.query_row(
                "SELECT epoch, next_seq, min_seq FROM roots WHERE root_id = ?1",
                params![root_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let last = (next_seq - 1).max(0) as u64;
            let current = format_cursor(&root.root_id, &epoch, last);
            let after = match cursor {
                None => return Ok(ChangesPage { root_id: root.root_id, changes: vec![], cursor: current, has_more: false, reset_required: false }),
                Some(cur) => parse_cursor(cur, &root.root_id)?,
            };
            if after.0 != epoch || (after.1 as i64) < min_seq || after.1 > last {
                return Ok(ChangesPage { root_id: root.root_id, changes: vec![], cursor: current, has_more: false, reset_required: true });
            }
            let limit = limit.clamp(1, 1000);
            let seqs: Vec<i64> = {
                let mut s = c.prepare("SELECT seq FROM changes WHERE root_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3")?;
                let v = s
                    .query_map(params![root_id, after.1 as i64, (limit + 1) as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()?;
                v
            };
            let has_more = seqs.len() > limit;
            let mut changes = Vec::new();
            for s in seqs.into_iter().take(limit) {
                if let Some(rec) = load_change(c, root_id, s as u64)? {
                    changes.push(rec);
                }
            }
            let end = changes.last().map(|c| c.seq).unwrap_or(after.1);
            Ok(ChangesPage { root_id: root.root_id.clone(), changes, cursor: format_cursor(&root.root_id, &epoch, end), has_more, reset_required: false })
        })
    }

    /// Blocks until the root has changes after `cursor` or `timeout` passes (long poll).
    pub fn wait_for_change(&self, root_id: &str, cursor: &str, timeout: std::time::Duration) -> Result<bool> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let page = self.changes(root_id, Some(cursor), 1)?;
            if page.reset_required || !page.changes.is_empty() {
                return Ok(true);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let gen = self.feed.0.lock().expect("feed lock");
            let _ = self.feed.1.wait_timeout(gen, deadline - now).expect("feed lock");
        }
    }

    /// Drops change rows at or below `seq`; older cursors then get `reset_required`.
    pub fn compact_changes(&self, root_id: &str, keep_last: u64) -> Result<()> {
        self.with_tx(|ctx| {
            let next: i64 = ctx.c.query_row("SELECT next_seq FROM roots WHERE root_id = ?1", params![root_id], |r| r.get(0))?;
            let floor = (next - 1 - keep_last as i64).max(0);
            ctx.c.execute("DELETE FROM changes WHERE root_id = ?1 AND seq <= ?2", params![root_id, floor])?;
            ctx.c.execute("UPDATE roots SET min_seq = MAX(min_seq, ?1) WHERE root_id = ?2", params![floor, root_id])?;
            Ok(())
        })
    }

    /// Starts a new index generation for a root: every cursor issued before gets
    /// `reset_required`. Item IDs are kept.
    pub fn new_epoch(&self, root_id: &str) -> Result<()> {
        self.with_tx(|ctx| {
            let epoch = format!("{:08x}", rand::random::<u32>());
            let n = ctx.c.execute("UPDATE roots SET epoch = ?1 WHERE root_id = ?2", params![epoch, root_id])?;
            if n == 0 {
                return Err(FilesError::RootNotFound(root_id.into()));
            }
            Ok(())
        })
    }
}

/// Cursor `c1.<root tag>.<epoch>.<seq>`: the root tag lets a cursor from another root
/// be refused (410), the epoch marks an index generation (reset), seq is monotonic.
fn format_cursor(root_id: &str, epoch: &str, seq: u64) -> String {
    format!("c1.{}.{epoch}.{seq}", root_id.trim_start_matches("rt_"))
}

fn parse_cursor(cursor: &str, root_id: &str) -> Result<(String, u64)> {
    let mut parts = cursor.split('.');
    let (Some("c1"), Some(tag), Some(epoch), Some(seq), None) = (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(FilesError::CursorExpired);
    };
    if tag != root_id.trim_start_matches("rt_") {
        return Err(FilesError::CursorExpired);
    }
    let seq = seq.parse().map_err(|_| FilesError::CursorExpired)?;
    Ok((epoch.to_string(), seq))
}

fn cursor_for(c: &Connection, root_id: &str) -> Result<String> {
    let (epoch, next): (String, i64) = c
        .query_row("SELECT epoch, next_seq FROM roots WHERE root_id = ?1", params![root_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| FilesError::RootNotFound(root_id.into()))?;
    Ok(format_cursor(root_id, &epoch, (next - 1).max(0) as u64))
}

fn load_change(c: &Connection, root_id: &str, seq: u64) -> Result<Option<ChangeRecord>> {
    let row: Option<(String, String, Option<String>, String, String)> = c
        .query_row(
            "SELECT op, item_id, old_parent_id, at, by FROM changes WHERE root_id = ?1 AND seq = ?2",
            params![root_id, seq as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((op, item_id, old_parent_id, at, by)) = row else { return Ok(None) };
    let op: ChangeOp = db::enum_parse(&op)?;
    let item = match op {
        ChangeOp::Deleted | ChangeOp::Trashed => None,
        _ => db::item_opt(c, &item_id)?.filter(|i| i.root_id == root_id),
    };
    let item = match item {
        Some(i) if is_live(c, &i.id)? => Some(i),
        _ => None,
    };
    Ok(Some(ChangeRecord { root_id: root_id.into(), seq, op, item_id, item, old_parent_id, at, by: from_json(&by)? }))
}

fn is_live(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row("SELECT trash_id IS NULL FROM items WHERE item_id = ?1", params![id], |r| r.get::<_, bool>(0))
        .optional()?
        .unwrap_or(false))
}

fn canonical_lenient(p: &Path) -> PathBuf {
    if let Ok(c) = fs::canonicalize(p) {
        return c;
    }
    match (p.parent(), p.file_name()) {
        (Some(parent), Some(name)) => canonical_lenient(parent).join(name),
        _ => p.to_path_buf(),
    }
}

pub(crate) fn item_path(c: &Connection, item: &ItemRecord) -> Result<PathBuf> {
    let mut names = Vec::new();
    let mut cur = item.clone();
    let mut guard = 0;
    while let Some(parent) = cur.parent_id.clone() {
        names.push(cur.name.clone());
        cur = db::item_opt(c, &parent)?.ok_or_else(|| FilesError::Internal(format!("orphan item {}", item.id)))?;
        guard += 1;
        if guard > 4096 {
            return Err(FilesError::Internal("folder loop in index".into()));
        }
    }
    let root = db::root(c, &cur.root_id)?;
    let mut p = root.local_path;
    for n in names.iter().rev() {
        p.push(n);
    }
    Ok(p)
}

fn current_rev_id_opt(c: &Connection, item_id: &str) -> Result<Option<i64>> {
    Ok(c.query_row("SELECT current_rev_id FROM items WHERE item_id = ?1", params![item_id], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

fn current_rev_id(c: &Connection, item_id: &str) -> Result<i64> {
    current_rev_id_opt(c, item_id)?.ok_or_else(|| FilesError::NotFound(format!("content of {item_id}")))
}

pub(crate) fn refs_of(c: &Connection, rev_id: i64) -> Result<Vec<ChunkRef>> {
    let mut s = c.prepare("SELECT hash, offset, size FROM revision_chunks WHERE rev_id = ?1 ORDER BY idx")?;
    let v = s
        .query_map(params![rev_id], |r| {
            Ok(ChunkRef { hash: r.get(0)?, offset: r.get::<_, i64>(1)? as u64, size: r.get::<_, i64>(2)? as u64 })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

fn read_range(path: &Path, offset: u64, size: u64) -> Result<Vec<u8>> {
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn trash_record(c: &Connection, trash_id: &str) -> Result<TrashRecord> {
    let (item_id, parent, at, by, purge): (String, String, String, String, String) = c
        .query_row(
            "SELECT item_id, original_parent_id, trashed_at, trashed_by, purge_at FROM trash WHERE trash_id = ?1",
            params![trash_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or_else(|| FilesError::NotFound(format!("trash entry {trash_id}")))?;
    let item = db::item_opt(c, &item_id)?.ok_or_else(|| FilesError::NotFound(format!("item {item_id}")))?;
    Ok(TrashRecord { trash_id: trash_id.into(), item, original_parent_id: parent, trashed_at: at, trashed_by: from_json(&by)?, purge_at: purge })
}

/// New content arriving at a write.
pub(crate) struct Incoming<'a> {
    pub temp: &'a Path,
    pub chunked: &'a Chunked,
    /// Its chunks are already in the chunk store (upload sessions).
    pub from_store: bool,
    pub modified_at: Option<String>,
}

impl Ctx<'_> {
    /// Now, on the editing device's wall clock.
    fn device_time(&self, device: &EditingDevice) -> NaiveDateTime {
        let offset = device
            .utc_offset_minutes
            .and_then(|m| FixedOffset::east_opt(m.checked_mul(60)?))
            .unwrap_or(self.opts.display_offset);
        self.now.with_timezone(&offset).naive_local()
    }

    pub(crate) fn cursor(&self, root_id: &str) -> Result<String> {
        cursor_for(self.c, root_id)
    }

    fn push_change(&mut self, root_id: &str, op: ChangeOp, item_id: &str, old_parent: Option<&str>, by: &PrincipalRef) -> Result<u64> {
        let seq: i64 = self.c.query_row("SELECT next_seq FROM roots WHERE root_id = ?1", params![root_id], |r| r.get(0))?;
        self.c.execute("UPDATE roots SET next_seq = next_seq + 1 WHERE root_id = ?1", params![root_id])?;
        self.c.execute(
            "INSERT INTO changes(root_id, seq, op, item_id, old_parent_id, at, by) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![root_id, seq, enum_str(&op), item_id, old_parent, ts(self.now), to_json(by)],
        )?;
        self.pending.push((root_id.to_string(), seq as u64));
        Ok(seq as u64)
    }

    fn require_write(&self, root_id: &str) -> Result<RootRecord> {
        let root = db::root(self.c, root_id)?;
        if !root.policy.write {
            return Err(FilesError::Forbidden(format!("root {} is read-only", root.name)));
        }
        Ok(root)
    }

    pub(crate) fn writable_folder(&self, root_id: &str, parent_id: &str) -> Result<ItemRecord> {
        self.require_write(root_id)?;
        let parent = db::live_item(self.c, parent_id)?;
        if parent.root_id != root_id {
            return Err(FilesError::InvalidRequest(format!("{parent_id} is not in root {root_id}")));
        }
        if parent.kind != ItemKind::Folder {
            return Err(FilesError::InvalidRequest(format!("{parent_id} is not a folder")));
        }
        if let Some(link) = &parent.root_link {
            return Err(FilesError::InvalidRequest(format!("{} opens root {link}; write there", parent.name)));
        }
        Ok(parent)
    }

    pub(crate) fn name_taken(&self, parent: &ItemRecord, parent_path: &Path, name: &str) -> bool {
        matches!(db::child_by_name(self.c, &parent.id, name), Ok(Some(_))) || fs::symlink_metadata(parent_path.join(name)).is_ok()
    }

    fn is_self_or_descendant(&self, candidate: &str, ancestor: &str) -> Result<bool> {
        let mut cur = Some(candidate.to_string());
        while let Some(id) = cur {
            if id == ancestor {
                return Ok(true);
            }
            cur = db::item_opt(self.c, &id)?.and_then(|i| i.parent_id);
        }
        Ok(false)
    }

    /// IDs of `top` and everything under it (live rows, or rows in one trash entry).
    fn subtree(&self, top: &str, trash_id: Option<&str>) -> Result<Vec<String>> {
        let sql = "WITH RECURSIVE sub(id) AS (SELECT ?1 UNION ALL SELECT i.item_id FROM items i JOIN sub ON i.parent_id = sub.id \
                   WHERE (?2 IS NULL AND i.trash_id IS NULL) OR i.trash_id = ?2) SELECT id FROM sub";
        let mut s = self.c.prepare(sql)?;
        let v = s.query_map(params![top, trash_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(v)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_item(
        &mut self,
        parent: &ItemRecord,
        name: &str,
        kind: ItemKind,
        size: u64,
        content_hash: Option<&str>,
        by: Option<&PrincipalRef>,
        md: &fs::Metadata,
        root_link: Option<&str>,
        link_escapes: bool,
    ) -> Result<ItemRecord> {
        let id = new_id("it_");
        let now = ts(self.now);
        self.c.execute(
            "INSERT INTO items(item_id, root_id, parent_id, name, kind, size, meta_version, content_hash, created_at, modified_at, modified_by, root_link, link_escapes, mtime_ns, inode) \
             VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                id,
                parent.root_id,
                parent.id,
                name,
                enum_str(&kind),
                size as i64,
                content_hash,
                now,
                if by.is_some() { now.clone() } else { mtime_ts(md) },
                by.map(to_json),
                root_link,
                link_escapes as i64,
                mtime_ns(md),
                inode(md)
            ],
        )?;
        db::live_item(self.c, &id)
    }

    fn record_disk(&self, item_id: &str, path: &Path) -> Result<()> {
        let md = fs::metadata(path)?;
        self.c.execute(
            "UPDATE items SET mtime_ns = ?1, inode = ?2 WHERE item_id = ?3",
            params![mtime_ns(&md), inode(&md), item_id],
        )?;
        Ok(())
    }

    fn incref(&self, refs: &[ChunkRef]) -> Result<()> {
        for r in refs {
            self.c.execute("INSERT OR IGNORE INTO chunks(hash, size, refcount) VALUES(?1, ?2, 0)", params![r.hash, r.size as i64])?;
            self.c.execute("UPDATE chunks SET refcount = refcount + 1 WHERE hash = ?1", params![r.hash])?;
        }
        Ok(())
    }

    fn ensure_from_file(&self, refs: &[ChunkRef], path: &Path) -> Result<()> {
        for r in refs {
            if !self.store.has(&r.hash) {
                let data = read_range(path, r.offset, r.size)?;
                self.store.put(&r.hash, &data).map_err(|e| match e {
                    FilesError::ChunkHashMismatch(_) => FilesError::IntegrityFailed(format!("{} changed while being saved", path.display())),
                    other => other,
                })?;
            }
        }
        Ok(())
    }

    fn add_revision(&mut self, item_id: &str, meta_version: u64, chunked: &Chunked, by: Option<&PrincipalRef>, retain: Retain<'_>) -> Result<i64> {
        let retained = !matches!(retain, Retain::No);
        if let Retain::File(p) = retain {
            self.ensure_from_file(&chunked.chunks, p)?;
        }
        if retained {
            for r in &chunked.chunks {
                if !self.store.has(&r.hash) {
                    return Err(FilesError::ChunkMissing { missing: vec![r.hash.clone()] });
                }
            }
            self.incref(&chunked.chunks)?;
        }
        self.c.execute(
            "INSERT INTO revisions(item_id, meta_version, content_hash, size, created_at, created_by, retained) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![item_id, meta_version as i64, chunked.content_hash, chunked.size as i64, ts(self.now), by.map(to_json), retained as i64],
        )?;
        let rev_id = self.c.last_insert_rowid();
        {
            let mut s = self.c.prepare("INSERT INTO revision_chunks(rev_id, idx, hash, offset, size) VALUES(?1,?2,?3,?4,?5)")?;
            for (i, r) in chunked.chunks.iter().enumerate() {
                s.execute(params![rev_id, i as i64, r.hash, r.offset as i64, r.size as i64])?;
            }
        }
        self.c.execute("UPDATE items SET current_rev_id = ?1 WHERE item_id = ?2", params![rev_id, item_id])?;
        Ok(rev_id)
    }

    fn retain_current(&self, item: &ItemRecord, path: &Path) -> Result<()> {
        let Some(rev) = current_rev_id_opt(self.c, &item.id)? else { return Ok(()) };
        let retained: i64 = self.c.query_row("SELECT retained FROM revisions WHERE rev_id = ?1", params![rev], |r| r.get(0))?;
        if retained == 0 {
            let refs = refs_of(self.c, rev)?;
            self.ensure_from_file(&refs, path)?;
            self.incref(&refs)?;
            self.c.execute("UPDATE revisions SET retained = 1 WHERE rev_id = ?1", params![rev])?;
        }
        Ok(())
    }

    fn unretain(&self, rev_id: i64) -> Result<()> {
        let retained: Option<i64> =
            self.c.query_row("SELECT retained FROM revisions WHERE rev_id = ?1", params![rev_id], |r| r.get(0)).optional()?;
        if retained == Some(1) {
            self.c.execute(
                "UPDATE chunks SET refcount = refcount - 1 WHERE hash IN (SELECT hash FROM revision_chunks WHERE rev_id = ?1)",
                params![rev_id],
            )?;
            // A chunk listed twice in one revision was counted twice.
            let dups: Vec<(String, i64)> = {
                let mut s = self.c.prepare("SELECT hash, COUNT(*) - 1 FROM revision_chunks WHERE rev_id = ?1 GROUP BY hash HAVING COUNT(*) > 1")?;
                let v = s.query_map(params![rev_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                v
            };
            for (h, extra) in dups {
                self.c.execute("UPDATE chunks SET refcount = refcount - ?1 WHERE hash = ?2", params![extra, h])?;
            }
            self.c.execute("UPDATE revisions SET retained = 0 WHERE rev_id = ?1", params![rev_id])?;
        }
        Ok(())
    }

    fn drop_revision(&self, rev_id: i64) -> Result<()> {
        self.unretain(rev_id)?;
        self.c.execute("DELETE FROM revision_chunks WHERE rev_id = ?1", params![rev_id])?;
        self.c.execute("DELETE FROM revisions WHERE rev_id = ?1", params![rev_id])?;
        Ok(())
    }

    /// Keeps at most `keep_count` older versions, none replaced more than `keep_days`
    /// ago (0 days = no age limit). Returns how many were dropped.
    fn prune_versions(&self, item_id: &str, policy: &RootPolicy) -> Result<usize> {
        let cur = current_rev_id_opt(self.c, item_id)?.unwrap_or(-1);
        let old: Vec<(i64, Option<String>)> = {
            let mut s = self.c.prepare("SELECT rev_id, superseded_at FROM revisions WHERE item_id = ?1 AND rev_id != ?2 ORDER BY rev_id DESC")?;
            let v = s.query_map(params![item_id, cur], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        let cutoff = (policy.versions.keep_days > 0).then(|| ts(self.now - Duration::days(policy.versions.keep_days as i64)));
        let mut dropped = 0;
        for (i, (rev, superseded)) in old.into_iter().enumerate() {
            let too_many = i as u64 >= policy.versions.keep_count;
            let too_old = matches!((&cutoff, &superseded), (Some(c), Some(s)) if s < c);
            if too_many || too_old {
                self.drop_revision(rev)?;
                dropped += 1;
            }
        }
        Ok(dropped)
    }

    /// New content for an existing file: the old revision becomes a version (or is
    /// dropped when the root keeps none), meta_version rises.
    fn replace_content(
        &mut self,
        root: &RootRecord,
        item: &ItemRecord,
        chunked: &Chunked,
        retain: Retain<'_>,
        by: &PrincipalRef,
        modified_at: Option<&str>,
    ) -> Result<()> {
        let keep = root.policy.versions.keep_count > 0;
        if let Some(old) = current_rev_id_opt(self.c, &item.id)? {
            let retained: i64 = self.c.query_row("SELECT retained FROM revisions WHERE rev_id = ?1", params![old], |r| r.get(0))?;
            if keep && retained == 1 {
                self.c.execute("UPDATE revisions SET superseded_at = ?1 WHERE rev_id = ?2", params![ts(self.now), old])?;
            } else {
                self.drop_revision(old)?;
            }
        }
        let mv = item.meta_version + 1;
        self.c.execute(
            "UPDATE items SET size = ?1, content_hash = ?2, meta_version = ?3, modified_at = ?4, modified_by = ?5 WHERE item_id = ?6",
            params![chunked.size as i64, chunked.content_hash, mv as i64, modified_at.map(str::to_string).unwrap_or_else(|| ts(self.now)), to_json(by), item.id],
        )?;
        self.add_revision(&item.id, mv, chunked, Some(by), if keep { retain } else { Retain::No })?;
        self.prune_versions(&item.id, &root.policy)?;
        Ok(())
    }

    /// The write core shared by small writes and upload commits (SPEC section 4).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn place_content(
        &mut self,
        parent: &ItemRecord,
        name: &str,
        target_item: Option<&str>,
        base_rev: Option<&str>,
        on_conflict: OnConflict,
        by: &PrincipalRef,
        device: &EditingDevice,
        inc: Incoming<'_>,
    ) -> Result<Mutation> {
        let root = db::root(self.c, &parent.root_id)?;
        let name = validate_name(name)?;
        let parent_path = item_path(self.c, parent)?;
        let existing = match target_item {
            Some(id) => db::item_opt(self.c, id)?.filter(|i| i.root_id == parent.root_id && is_live(self.c, &i.id).unwrap_or(false)),
            None => db::child_by_name(self.c, &parent.id, &name)?,
        };
        enum Plan {
            Replace(ItemRecord),
            Create(PathBuf, ItemRecord, String),
        }
        let mut conflict_of: Option<ItemRecord> = None;
        let plan = match (base_rev, existing) {
            (Some(_), Some(cur)) if cur.kind == ItemKind::Folder => return Err(FilesError::AlreadyExists(cur.name)),
            (Some(br), Some(cur)) if cur.rev() == br => Plan::Replace(cur),
            (Some(_), Some(cur)) => match on_conflict {
                OnConflict::KeepBoth => {
                    let cur_parent = db::live_item(self.c, cur.parent_id.as_deref().unwrap_or(&parent.id))?;
                    let cur_parent_path = item_path(self.c, &cur_parent)?;
                    let n = conflict_copy_name(&cur.name, device.name.as_deref().unwrap_or(""), self.device_time(device), |n| {
                        self.name_taken(&cur_parent, &cur_parent_path, n)
                    });
                    conflict_of = Some(cur);
                    Plan::Create(cur_parent_path, cur_parent, n)
                }
                _ => return Err(FilesError::RevMismatch { current: Box::new(cur) }),
            },
            (Some(_), None) => match on_conflict {
                OnConflict::KeepBoth => {
                    let n = if self.name_taken(parent, &parent_path, &name) {
                        duplicate_name(&name, |n| self.name_taken(parent, &parent_path, n))
                    } else {
                        name.clone()
                    };
                    Plan::Create(parent_path.clone(), parent.clone(), n)
                }
                _ => return Err(FilesError::NotFound(format!("{name} no longer exists"))),
            },
            (None, Some(cur)) => match on_conflict {
                OnConflict::KeepBoth => {
                    let n = duplicate_name(&name, |n| self.name_taken(parent, &parent_path, n));
                    Plan::Create(parent_path.clone(), parent.clone(), n)
                }
                OnConflict::Replace if cur.kind == ItemKind::File => Plan::Replace(cur),
                _ => return Err(FilesError::AlreadyExists(name)),
            },
            (None, None) if fs::symlink_metadata(parent_path.join(&name)).is_ok() => match on_conflict {
                // On disk but not indexed yet (the watcher hasn't run): never overwrite blind.
                OnConflict::KeepBoth => {
                    let n = duplicate_name(&name, |n| self.name_taken(parent, &parent_path, n));
                    Plan::Create(parent_path.clone(), parent.clone(), n)
                }
                _ => return Err(FilesError::AlreadyExists(name)),
            },
            (None, None) => Plan::Create(parent_path.clone(), parent.clone(), name.clone()),
        };
        match plan {
            Plan::Replace(cur) => {
                let path = item_path(self.c, &cur)?;
                fs::rename(inc.temp, &path)?;
                let retain = if inc.from_store { Retain::Store } else { Retain::File(&path) };
                self.replace_content(&root, &cur, inc.chunked, retain, by, inc.modified_at.as_deref())?;
                self.record_disk(&cur.id, &path)?;
                self.push_change(&root.root_id, ChangeOp::Modified, &cur.id, None, by)?;
                let item = db::live_item(self.c, &cur.id)?;
                Ok(Mutation { item, conflict: None, cursor: self.cursor(&root.root_id)? })
            }
            Plan::Create(dir_path, dir, n) => {
                let path = dir_path.join(&n);
                fs::rename(inc.temp, &path)?;
                let md = fs::metadata(&path)?;
                let item = self.insert_item(&dir, &n, ItemKind::File, inc.chunked.size, Some(&inc.chunked.content_hash), Some(by), &md, None, false)?;
                if let Some(m) = &inc.modified_at {
                    self.c.execute("UPDATE items SET modified_at = ?1 WHERE item_id = ?2", params![m, item.id])?;
                }
                let retain = if root.policy.versions.keep_count == 0 {
                    Retain::No
                } else if inc.from_store {
                    Retain::Store
                } else {
                    Retain::File(&path)
                };
                self.add_revision(&item.id, 1, inc.chunked, Some(by), retain)?;
                self.push_change(&root.root_id, ChangeOp::Created, &item.id, None, by)?;
                let item = db::live_item(self.c, &item.id)?;
                let conflict = conflict_of.map(|cur| ConflictRecord {
                    original_item_id: cur.id.clone(),
                    base_rev: base_rev.unwrap_or_default().to_string(),
                    current_rev: cur.rev(),
                    conflict_item: item.clone(),
                    conflict_name: n.clone(),
                });
                Ok(Mutation { item, conflict, cursor: self.cursor(&root.root_id)? })
            }
        }
    }

    /// Writes a file from its stored chunks into `temp`, checking every chunk and the
    /// whole-file hash.
    pub(crate) fn assemble(&self, refs: &[ChunkRef], size: u64, content_hash: &str, temp: &Path) -> Result<()> {
        let res = (|| {
            let mut out = std::io::BufWriter::new(fs::File::create(temp)?);
            let mut whole = blake3::Hasher::new();
            let mut written = 0u64;
            for r in refs {
                let data = self.store.get(&r.hash)?;
                if data.len() as u64 != r.size {
                    return Err(FilesError::ManifestInvalid(format!("chunk {} is {} bytes, manifest says {}", r.hash, data.len(), r.size)));
                }
                whole.update(&data);
                out.write_all(&data)?;
                written += r.size;
            }
            out.flush()?;
            out.get_ref().sync_all()?;
            if written != size {
                return Err(FilesError::ManifestInvalid(format!("chunks add up to {written} bytes, not {size}")));
            }
            let got = whole.finalize().to_hex().to_string();
            if got != content_hash {
                return Err(FilesError::ManifestInvalid(format!("assembled content hashes to {got}, not {content_hash}")));
            }
            Ok(())
        })();
        if res.is_err() {
            let _ = fs::remove_file(temp);
        }
        res
    }

    /// Moves an item (and its subtree) to the trash: content kept in the chunk store,
    /// removed from disk.
    fn trash_item(&mut self, item: &ItemRecord, by: &PrincipalRef) -> Result<TrashRecord> {
        let root = db::root(self.c, &item.root_id)?;
        let path = item_path(self.c, item)?;
        let trash_id = new_id("tr_");
        for id in self.subtree(&item.id, None)? {
            let it = db::live_item(self.c, &id)?;
            if it.kind == ItemKind::File && !it.link_escapes {
                let p = item_path(self.c, &it)?;
                self.retain_current(&it, &p)?;
            }
        }
        self.mark_trashed(&root, item, &trash_id, by)?;
        remove_path(&path)?;
        self.push_change(&root.root_id, ChangeOp::Trashed, &item.id, item.parent_id.as_deref(), by)?;
        trash_record(self.c, &trash_id)
    }

    fn mark_trashed(&self, root: &RootRecord, item: &ItemRecord, trash_id: &str, by: &PrincipalRef) -> Result<()> {
        for id in self.subtree(&item.id, None)? {
            self.c.execute("UPDATE items SET trash_id = ?1 WHERE item_id = ?2", params![trash_id, id])?;
        }
        let purge = ts(self.now + Duration::days(root.policy.trash_days as i64));
        self.c.execute(
            "INSERT INTO trash(trash_id, root_id, item_id, original_parent_id, trashed_at, trashed_by, purge_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![trash_id, root.root_id, item.id, item.parent_id.clone().unwrap_or_default(), ts(self.now), to_json(by), purge],
        )?;
        Ok(())
    }

    /// Deletes the rows of `top` and its subtree and drops their revisions.
    fn delete_rows(&self, top: &str, trash_id: Option<&str>) -> Result<()> {
        for id in self.subtree(top, trash_id)? {
            let revs: Vec<i64> = {
                let mut s = self.c.prepare("SELECT rev_id FROM revisions WHERE item_id = ?1")?;
                let v = s.query_map(params![id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
                v
            };
            for r in revs {
                self.drop_revision(r)?;
            }
            self.c.execute("DELETE FROM items WHERE item_id = ?1", params![id])?;
        }
        Ok(())
    }

    /// Re-creates a trashed subtree on disk at `dest` from retained chunks.
    fn materialize(&self, top: &ItemRecord, dest: &Path, trash_id: &str) -> Result<()> {
        let mut queue = VecDeque::from([(top.clone(), dest.to_path_buf())]);
        while let Some((it, path)) = queue.pop_front() {
            match it.kind {
                ItemKind::Folder => {
                    fs::create_dir_all(&path)?;
                    for child in db::items_where(self.c, "parent_id = ?1 AND trash_id = ?2", params![it.id, trash_id])? {
                        let p = path.join(&child.name);
                        queue.push_back((child, p));
                    }
                }
                ItemKind::File if it.link_escapes => {
                    self.delete_rows(&it.id, Some(trash_id))?;
                }
                ItemKind::File => {
                    let rev = current_rev_id(self.c, &it.id)?;
                    let refs = refs_of(self.c, rev)?;
                    let temp = path.with_file_name(format!("{TEMP_PREFIX}restore-{}.part", rand::random::<u64>()));
                    self.assemble(&refs, it.size, it.content_hash.as_deref().unwrap_or(crate::model::EMPTY_HASH), &temp)?;
                    fs::rename(&temp, &path)?;
                }
            }
        }
        Ok(())
    }

    fn copy_rec(&mut self, item: &ItemRecord, src: &Path, dest: &ItemRecord, dest_path: &Path, name: &str, by: &PrincipalRef) -> Result<ItemRecord> {
        let path = dest_path.join(name);
        let root = db::root(self.c, &dest.root_id)?;
        match item.kind {
            ItemKind::Folder => {
                fs::create_dir(&path)?;
                let md = fs::symlink_metadata(&path)?;
                let new = self.insert_item(dest, name, ItemKind::Folder, 0, None, Some(by), &md, None, false)?;
                self.push_change(&dest.root_id, ChangeOp::Created, &new.id, None, by)?;
                if item.root_link.is_none() {
                    for child in db::live_children(self.c, &item.id)? {
                        if child.link_escapes {
                            continue;
                        }
                        let csrc = src.join(&child.name);
                        self.copy_rec(&child, &csrc, &new, &path, &child.name.clone(), by)?;
                    }
                }
                Ok(new)
            }
            ItemKind::File => {
                fs::copy(src, &path)?;
                let md = fs::metadata(&path)?;
                let rev = current_rev_id(self.c, &item.id)?;
                let chunked = Chunked {
                    chunks: refs_of(self.c, rev)?,
                    size: item.size,
                    content_hash: item.content_hash.clone().unwrap_or_else(|| crate::model::EMPTY_HASH.into()),
                };
                let new = self.insert_item(dest, name, ItemKind::File, item.size, Some(&chunked.content_hash), Some(by), &md, None, false)?;
                let retain = if root.policy.versions.keep_count == 0 { Retain::No } else { Retain::File(&path) };
                self.add_revision(&new.id, 1, &chunked, Some(by), retain)?;
                self.push_change(&dest.root_id, ChangeOp::Created, &new.id, None, by)?;
                Ok(new)
            }
        }
    }

    /// Finds the deepest indexed folder on the way to `target` (inside `root`).
    fn deepest_folder(&self, root: &RootRecord, target: &Path) -> Result<(ItemRecord, PathBuf)> {
        let mut item = db::live_item(self.c, &root.root_item_id)?;
        let mut path = root.local_path.clone();
        if let Ok(rel) = target.strip_prefix(&root.local_path) {
            for comp in rel.components() {
                let name = comp.as_os_str().to_string_lossy();
                match db::child_by_name(self.c, &item.id, &name)? {
                    Some(child) if child.kind == ItemKind::Folder && child.root_link.is_none() => {
                        path.push(&*name);
                        item = child;
                    }
                    _ => break,
                }
            }
        }
        Ok((item, path))
    }

    fn other_roots(&self, root: &RootRecord) -> Result<HashMap<PathBuf, String>> {
        Ok(db::roots(self.c)?
            .into_iter()
            .filter(|r| r.root_id != root.root_id)
            .map(|r| (r.local_path, r.root_id))
            .collect())
    }

    fn disk_state(&self, item_id: &str) -> Result<(Option<i64>, Option<i64>)> {
        Ok(self.c.query_row("SELECT inode, mtime_ns FROM items WHERE item_id = ?1", params![item_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
    }

    /// Chunks a file on disk; stores its chunks when the root keeps versions.
    fn chunk_file(&self, root: &RootRecord, path: &Path) -> Result<Chunked> {
        let keep = root.policy.versions.keep_count > 0;
        let f = fs::File::open(path)?;
        chunker::chunk_reader(std::io::BufReader::with_capacity(1 << 20, f), |c| {
            if keep {
                self.store.put(&c.hash, &c.data)?;
            }
            Ok(())
        })
    }

    fn scan_update_file(&mut self, root: &RootRecord, item: &ItemRecord, e: &Entry, chunked: Option<Chunked>) -> Result<usize> {
        let chunked = match chunked {
            Some(c) => c,
            None => self.chunk_file(root, &e.path)?,
        };
        let by = local_principal();
        let changed = item.content_hash.as_deref() != Some(chunked.content_hash.as_str());
        if changed {
            let retain = if root.policy.versions.keep_count > 0 { Retain::Store } else { Retain::No };
            self.replace_content(root, item, &chunked, retain, &by, Some(&e.modified_at))?;
        }
        self.c.execute("UPDATE items SET mtime_ns = ?1, inode = ?2 WHERE item_id = ?3", params![e.mtime_ns, e.inode, item.id])?;
        if changed {
            self.push_change(&root.root_id, ChangeOp::Modified, &item.id, None, &by)?;
            return Ok(1);
        }
        Ok(0)
    }

    /// The reconcile loop. See [`Drive::reconcile_paths`].
    fn scan(&mut self, root: &RootRecord, starts: Vec<(ItemRecord, PathBuf, bool)>) -> Result<usize> {
        let others = self.other_roots(root)?;
        let by = local_principal();
        let mut queue: VecDeque<(ItemRecord, PathBuf, bool)> = starts.into();
        let mut vanished: Vec<ItemRecord> = Vec::new();
        let mut count = 0usize;
        loop {
            let mut appeared: Vec<(ItemRecord, String, Entry, bool)> = Vec::new();
            while let Some((dir, dir_path, recursive)) = queue.pop_front() {
                let rd = match fs::read_dir(&dir_path) {
                    Ok(rd) => rd,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                let mut on_disk: HashMap<String, Entry> = HashMap::new();
                for ent in rd {
                    let ent = ent?;
                    let Ok(name) = ent.file_name().into_string() else { continue };
                    if name.starts_with(TEMP_PREFIX) {
                        continue;
                    }
                    match probe(&ent.path(), &root.local_path, &others) {
                        Ok(e) => {
                            on_disk.insert(name, e);
                        }
                        Err(FilesError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
                let mut seen = HashSet::new();
                for child in db::live_children(self.c, &dir.id)? {
                    match on_disk.get(&child.name) {
                        Some(e) if e.kind == child.kind && e.root_link == child.root_link && e.link_escapes == child.link_escapes => {
                            seen.insert(child.name.clone());
                            if child.kind == ItemKind::File {
                                if !child.link_escapes {
                                    let (_, mt) = self.disk_state(&child.id)?;
                                    if mt != Some(e.mtime_ns) || child.size != e.size {
                                        count += self.scan_update_file(root, &child, e, None)?;
                                    }
                                }
                            } else if recursive && child.root_link.is_none() {
                                let p = e.path.clone();
                                queue.push_back((child, p, true));
                            }
                        }
                        _ => vanished.push(child),
                    }
                }
                for (name, e) in on_disk {
                    if !seen.contains(&name) {
                        appeared.push((dir.clone(), name, e, recursive));
                    }
                }
            }
            if appeared.is_empty() {
                break;
            }
            for (dir, name, e, recursive) in appeared {
                let mut matched = None;
                if e.inode.is_some() {
                    for (i, v) in vanished.iter().enumerate() {
                        if v.kind == e.kind && v.root_link == e.root_link && self.disk_state(&v.id)?.0 == e.inode {
                            matched = Some(i);
                            break;
                        }
                    }
                }
                let chunked = if e.kind == ItemKind::File && !e.link_escapes { Some(self.chunk_file(root, &e.path)?) } else { None };
                if matched.is_none() {
                    if let Some(c) = &chunked {
                        matched = vanished
                            .iter()
                            .position(|v| v.kind == ItemKind::File && v.size == c.size && v.content_hash.as_deref() == Some(c.content_hash.as_str()));
                    }
                }
                if let Some(i) = matched {
                    let v = vanished.remove(i);
                    let old_parent = v.parent_id.clone();
                    self.c.execute(
                        "UPDATE items SET parent_id = ?1, name = ?2, meta_version = meta_version + 1, inode = ?3 WHERE item_id = ?4",
                        params![dir.id, name, e.inode, v.id],
                    )?;
                    let op = if old_parent.as_deref() == Some(dir.id.as_str()) { ChangeOp::Renamed } else { ChangeOp::Moved };
                    let old = if op == ChangeOp::Moved { old_parent.as_deref() } else { None };
                    self.push_change(&root.root_id, op, &v.id, old, &by)?;
                    count += 1;
                    let v = db::live_item(self.c, &v.id)?;
                    match e.kind {
                        ItemKind::File => {
                            if v.content_hash.as_deref() != chunked.as_ref().map(|c| c.content_hash.as_str()) || v.size != e.size {
                                count += self.scan_update_file(root, &v, &e, chunked)?;
                            } else {
                                self.c.execute("UPDATE items SET mtime_ns = ?1 WHERE item_id = ?2", params![e.mtime_ns, v.id])?;
                            }
                        }
                        ItemKind::Folder if recursive => queue.push_back((v, e.path.clone(), true)),
                        ItemKind::Folder => {}
                    }
                    continue;
                }
                let md = fs::symlink_metadata(&e.path)?;
                let size = chunked.as_ref().map(|c| c.size).unwrap_or(0);
                let hash = chunked.as_ref().map(|c| c.content_hash.clone());
                let item = self.insert_item(&dir, &name, e.kind, size, hash.as_deref(), None, &md, e.root_link.as_deref(), e.link_escapes)?;
                self.c.execute("UPDATE items SET mtime_ns = ?1, inode = ?2 WHERE item_id = ?3", params![e.mtime_ns, e.inode, item.id])?;
                if let Some(c) = &chunked {
                    let retain = if root.policy.versions.keep_count > 0 { Retain::Store } else { Retain::No };
                    self.add_revision(&item.id, 1, c, None, retain)?;
                }
                self.push_change(&root.root_id, ChangeOp::Created, &item.id, None, &by)?;
                count += 1;
                if e.kind == ItemKind::Folder && e.root_link.is_none() {
                    queue.push_back((item, e.path.clone(), true));
                }
            }
        }
        for v in vanished {
            // Gone from disk. Restorable from trash when its content was kept. A name that
            // is still on disk changed identity (file became folder, folder became a root
            // link): forget the old entry instead of trashing it.
            let still_there = fs::symlink_metadata(item_path(self.c, &v)?).is_ok();
            let restorable = !still_there
                && root.policy.trash_days > 0
                && match v.kind {
                    ItemKind::File => current_rev_id_opt(self.c, &v.id)?
                        .map(|r| self.c.query_row("SELECT retained FROM revisions WHERE rev_id = ?1", params![r], |x| x.get::<_, i64>(0)))
                        .transpose()?
                        == Some(1),
                    ItemKind::Folder => true,
                };
            if restorable {
                if v.kind == ItemKind::Folder {
                    // Files inside whose content was not kept can't come back: forget them.
                    for id in self.subtree(&v.id, None)? {
                        let it = db::live_item(self.c, &id)?;
                        if it.kind == ItemKind::File {
                            let kept = current_rev_id_opt(self.c, &id)?
                                .map(|r| self.c.query_row("SELECT retained FROM revisions WHERE rev_id = ?1", params![r], |x| x.get::<_, i64>(0)))
                                .transpose()?
                                == Some(1);
                            if !kept || it.link_escapes {
                                self.delete_rows(&id, None)?;
                            }
                        }
                    }
                }
                let trash_id = new_id("tr_");
                self.mark_trashed(root, &v, &trash_id, &by)?;
                self.push_change(&root.root_id, ChangeOp::Trashed, &v.id, v.parent_id.as_deref(), &by)?;
            } else {
                self.delete_rows(&v.id, None)?;
                self.push_change(&root.root_id, ChangeOp::Deleted, &v.id, v.parent_id.as_deref(), &by)?;
            }
            count += 1;
        }
        Ok(count)
    }
}
