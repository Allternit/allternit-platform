//! The folder tree both frontends show, built from one computer's roots.
//!
//! - The top level lists the roots that have no parent root: Allternit
//!   (`~/Allternit`), Memory Drive, Code, Models and folders the user added.
//! - The special folders inside Allternit (Inbox, Shared, Agent Output, ...)
//!   are their own roots; their entry in the Allternit listing carries
//!   `root_link`, so opening it lists that root. Each folder appears once.
//! - Root entries (top level and `root_link` folders) can't be renamed or
//!   deleted from a mount.
//!
//! Listings are cached for `LIST_TTL` and dropped on every write in that
//! folder. File content is read in `BLOCK` pieces and the last few are kept,
//! because the kernel and WebDAV clients read in small pieces and every
//! request over the relay is a round trip.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use allternit_files_core::{Item, ItemKind, OnConflict, Root};

use crate::backend::{Backend, MountError, Result};

const LIST_TTL: Duration = Duration::from_secs(2);
const ROOTS_TTL: Duration = Duration::from_secs(30);
/// Read piece. Equal to the largest chunk, so one relay request each.
pub const BLOCK: u64 = 4 * 1024 * 1024;
const BLOCKS_KEPT: usize = 8;

/// Where a folder's children are listed: (root id, parent item id).
pub type ListAt = (String, String);

/// One entry in the tree.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: SystemTime,
    pub writable: bool,
    /// None for the top level and top-level roots (they are not items to the user).
    pub item: Option<Item>,
    /// Folders: where their children come from.
    pub list_at: Option<ListAt>,
    /// A top-level root or a `root_link` folder: no rename, no delete.
    pub fixed: bool,
    pub mime: Option<String>,
}

impl Entry {
    pub fn top() -> Self {
        Self {
            name: String::new(),
            is_dir: true,
            size: 0,
            modified: SystemTime::now(),
            writable: false,
            item: None,
            list_at: None,
            fixed: true,
            mime: None,
        }
    }

    /// A stable key for inode tables: item id, or the root for a top-level root.
    pub fn key(&self) -> String {
        match (&self.item, &self.list_at) {
            (Some(i), _) => format!("i:{}", i.id),
            (None, Some((root, _))) => format!("r:{root}"),
            (None, None) => "top".to_string(),
        }
    }

    pub fn etag(&self) -> Option<&str> {
        self.item.as_ref().map(|i| i.rev.as_str())
    }
}

pub fn parse_time(s: &str) -> SystemTime {
    chrono::DateTime::parse_from_rfc3339(s).map(SystemTime::from).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Top-level names must be unique; a clash gets " (2)", " (3)".
fn unique(name: &str, taken: &mut Vec<String>) -> String {
    let mut n = name.to_string();
    let mut i = 2;
    while taken.iter().any(|t| t == &n) {
        n = format!("{name} ({i})");
        i += 1;
    }
    taken.push(n.clone());
    n
}

pub struct Vfs {
    backend: Arc<dyn Backend>,
    read_only: bool,
    roots: Mutex<Option<(Instant, Vec<Root>)>>,
    lists: Mutex<HashMap<ListAt, (Instant, Vec<Entry>)>>,
    blocks: Mutex<VecDeque<((String, String, u64), Arc<Vec<u8>>)>>,
}

impl Vfs {
    pub fn new(backend: Arc<dyn Backend>, read_only: bool) -> Self {
        Self { backend, read_only, roots: Mutex::new(None), lists: Mutex::new(HashMap::new()), blocks: Mutex::new(VecDeque::new()) }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    fn roots(&self) -> Result<Vec<Root>> {
        let mut cached = self.roots.lock().expect("roots lock");
        if let Some((at, roots)) = cached.as_ref() {
            if at.elapsed() < ROOTS_TTL {
                return Ok(roots.clone());
            }
        }
        let roots = self.backend.roots()?;
        *cached = Some((Instant::now(), roots.clone()));
        Ok(roots)
    }

    fn entry_for_item(&self, item: Item, roots: &[Root]) -> Entry {
        let is_dir = item.kind == ItemKind::Folder;
        let link = item.root_link.as_ref().and_then(|id| roots.iter().find(|r| &r.id == id));
        let list_at = match (is_dir, link) {
            (true, Some(r)) => Some((r.id.clone(), r.root_item_id.clone())),
            (true, None) => Some((item.root_id.clone(), item.id.clone())),
            _ => None,
        };
        Entry {
            name: item.name.clone(),
            is_dir,
            size: if is_dir { 0 } else { item.size },
            modified: parse_time(&item.modified_at),
            writable: !self.read_only && item.permissions.write,
            fixed: link.is_some(),
            mime: item.mime.clone(),
            list_at,
            item: Some(item),
        }
    }

    /// Children of a folder entry.
    pub fn children(&self, dir: &Entry) -> Result<Vec<Entry>> {
        let Some(at) = dir.list_at.clone() else {
            return self.top_children();
        };
        if let Some((when, list)) = self.lists.lock().expect("lists lock").get(&at) {
            if when.elapsed() < LIST_TTL {
                return Ok(list.clone());
            }
        }
        let roots = self.roots()?;
        let items = self.backend.list(&at.0, &at.1)?;
        let list: Vec<Entry> = items.into_iter().map(|i| self.entry_for_item(i, &roots)).collect();
        self.lists.lock().expect("lists lock").insert(at, (Instant::now(), list.clone()));
        Ok(list)
    }

    fn top_children(&self) -> Result<Vec<Entry>> {
        let mut taken = Vec::new();
        Ok(self
            .roots()?
            .into_iter()
            .filter(|r| r.parent_root_id.is_none())
            .map(|r| Entry {
                name: unique(&r.name, &mut taken),
                is_dir: true,
                size: 0,
                modified: SystemTime::now(),
                writable: !self.read_only && r.policy.write,
                item: None,
                list_at: Some((r.id.clone(), r.root_item_id.clone())),
                fixed: true,
                mime: None,
            })
            .collect())
    }

    pub fn child(&self, dir: &Entry, name: &str) -> Result<Option<Entry>> {
        Ok(self.children(dir)?.into_iter().find(|e| e.name == name))
    }

    /// Walk path segments from the top. `Ok(None)` when a segment is missing.
    pub fn resolve(&self, segments: &[String]) -> Result<Option<Entry>> {
        let mut cur = Entry::top();
        for s in segments {
            if !cur.is_dir {
                return Ok(None);
            }
            match self.child(&cur, s)? {
                Some(e) => cur = e,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    fn forget(&self, dir: &Entry) {
        if let Some(at) = &dir.list_at {
            self.lists.lock().expect("lists lock").remove(at);
        }
    }

    fn writable_dir<'a>(&self, dir: &'a Entry) -> Result<&'a ListAt> {
        if self.read_only {
            return Err(MountError::read_only());
        }
        match &dir.list_at {
            Some(at) if dir.is_dir => Ok(at),
            _ => Err(MountError::new("forbidden", 403, "Files can't be created here. Open one of the folders first.")),
        }
    }

    fn entry_item<'a>(&self, e: &'a Entry) -> Result<&'a Item> {
        if self.read_only {
            return Err(MountError::read_only());
        }
        if e.fixed {
            return Err(MountError::new("forbidden", 403, "This is one of the computer's main folders; it can't be renamed or removed here."));
        }
        e.item.as_ref().ok_or_else(MountError::not_found)
    }

    /// `len` bytes at `offset`, through the block cache.
    pub fn read(&self, e: &Entry, offset: u64, len: u64) -> Result<Vec<u8>> {
        let item = e.item.as_ref().ok_or_else(MountError::not_found)?;
        if offset >= item.size || len == 0 {
            return Ok(Vec::new());
        }
        let end = (offset + len).min(item.size);
        let mut out = Vec::with_capacity((end - offset) as usize);
        let mut block = offset / BLOCK;
        while block * BLOCK < end {
            let data = self.block(item, block)?;
            let b_start = block * BLOCK;
            let from = offset.max(b_start) - b_start;
            let to = (end.min(b_start + BLOCK) - b_start).min(data.len() as u64);
            if from < to {
                out.extend_from_slice(&data[from as usize..to as usize]);
            }
            if (data.len() as u64) < BLOCK {
                break;
            }
            block += 1;
        }
        Ok(out)
    }

    fn block(&self, item: &Item, index: u64) -> Result<Arc<Vec<u8>>> {
        let key = (item.id.clone(), item.rev.clone(), index);
        if let Some((_, data)) = self.blocks.lock().expect("blocks lock").iter().find(|(k, _)| k == &key) {
            return Ok(data.clone());
        }
        let data = Arc::new(self.backend.read(item, index * BLOCK, BLOCK)?);
        let mut blocks = self.blocks.lock().expect("blocks lock");
        blocks.push_back((key, data.clone()));
        while blocks.len() > BLOCKS_KEPT {
            blocks.pop_front();
        }
        Ok(data)
    }

    /// Save the local file `src` as `name` in `dir`. Replaces `existing` at the
    /// rev it was read at; a change made elsewhere since lands as a conflict copy.
    pub fn write(&self, dir: &Entry, name: &str, existing: Option<&Entry>, src: &Path) -> Result<Entry> {
        let at = self.writable_dir(dir)?.clone();
        let replace = existing.and_then(|e| e.item.as_ref());
        let item = self.backend.write(&at.0, &at.1, name, replace, OnConflict::KeepBoth, src)?;
        self.forget(dir);
        let roots = self.roots()?;
        Ok(self.entry_for_item(item, &roots))
    }

    pub fn mkdir(&self, dir: &Entry, name: &str) -> Result<Entry> {
        let at = self.writable_dir(dir)?.clone();
        let item = self.backend.mkdir(&at.1, name)?;
        self.forget(dir);
        let roots = self.roots()?;
        Ok(self.entry_for_item(item, &roots))
    }

    /// Rename and/or move; replaces a file already at the destination (POSIX rename).
    pub fn rename(&self, e: &Entry, from: &Entry, to: &Entry, new_name: &str, replace: bool) -> Result<Entry> {
        let item = self.entry_item(e)?.clone();
        let to_at = self.writable_dir(to)?.clone();
        let new_parent = (from.list_at.as_ref().map(|a| &a.1) != Some(&to_at.1)).then_some(to_at.1.as_str());
        let new_name = (new_name != item.name).then_some(new_name);
        if new_parent.is_none() && new_name.is_none() {
            return Ok(e.clone());
        }
        let conflict = if replace { OnConflict::Replace } else { OnConflict::Fail };
        let moved = self.backend.rename(&item, new_parent, new_name, conflict)?;
        self.forget(from);
        self.forget(to);
        let roots = self.roots()?;
        Ok(self.entry_for_item(moved, &roots))
    }

    pub fn copy(&self, e: &Entry, to: &Entry, new_name: &str, replace: bool) -> Result<Entry> {
        let item = e.item.as_ref().ok_or_else(MountError::not_found)?.clone();
        let to_at = self.writable_dir(to)?.clone();
        let conflict = if replace { OnConflict::Replace } else { OnConflict::Fail };
        let copied = self.backend.copy(&item, &to_at.1, (new_name != item.name).then_some(new_name), conflict)?;
        self.forget(to);
        let roots = self.roots()?;
        Ok(self.entry_for_item(copied, &roots))
    }

    /// To trash.
    pub fn delete(&self, e: &Entry, dir: &Entry) -> Result<()> {
        let item = self.entry_item(e)?.clone();
        self.backend.delete(&item)?;
        self.forget(dir);
        Ok(())
    }
}
