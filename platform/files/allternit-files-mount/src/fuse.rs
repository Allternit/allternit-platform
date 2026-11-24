//! Linux FUSE frontend (D11): the computer's Files drive as a folder.
//!
//! - **Inodes.** Item IDs are stable across rename and move (SPEC section 4),
//!   so each entry key (`Entry::key`) gets one inode for the life of the mount.
//! - **Reads** go through the VFS block cache, 4 MiB per request.
//! - **Writes** are spooled to a local temp file per open handle and uploaded
//!   on `flush`/`release` (small write up to 4 MiB, else an upload session).
//!   The upload carries the rev the file was opened at, so an edit made
//!   elsewhere meanwhile lands as a conflict copy beside it, never lost.
//! - The session loop is single-threaded (`fuser` calls `&mut self`), which
//!   keeps the tables simple; a slow relay call holds other calls back.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, FUSE_ROOT_ID,
};

use crate::backend::MountError;
use crate::vfs::{Entry, Vfs};

const TTL: Duration = Duration::from_secs(1);

struct Node {
    entry: Entry,
    parent: u64,
}

/// An open file that may be written.
struct Handle {
    ino: u64,
    file: File,
    /// Kept so the temp file is deleted when the handle closes.
    _tmp: tempfile::TempPath,
    dirty: bool,
}

pub struct FilesFs {
    vfs: Arc<Vfs>,
    nodes: HashMap<u64, Node>,
    by_key: HashMap<String, u64>,
    next_ino: u64,
    handles: HashMap<u64, Handle>,
    next_fh: u64,
    uid: u32,
    gid: u32,
}

impl FilesFs {
    pub fn new(vfs: Arc<Vfs>) -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(FUSE_ROOT_ID, Node { entry: Entry::top(), parent: FUSE_ROOT_ID });
        // SAFETY: getuid/getgid have no preconditions and cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Self { vfs, nodes, by_key: HashMap::new(), next_ino: FUSE_ROOT_ID + 1, handles: HashMap::new(), next_fh: 1, uid, gid }
    }

    /// The inode for an entry under `parent`, refreshing what we know of it.
    fn intern(&mut self, entry: Entry, parent: u64) -> u64 {
        let key = entry.key();
        if let Some(&ino) = self.by_key.get(&key) {
            if let Some(n) = self.nodes.get_mut(&ino) {
                n.entry = entry;
                n.parent = parent;
            }
            return ino;
        }
        let ino = self.next_ino;
        self.next_ino += 1;
        self.by_key.insert(key, ino);
        self.nodes.insert(ino, Node { entry, parent });
        ino
    }

    fn attr(&self, ino: u64, e: &Entry) -> FileAttr {
        let perm = match (e.is_dir, e.writable) {
            (true, true) => 0o755,
            (true, false) => 0o555,
            (false, true) => 0o644,
            (false, false) => 0o444,
        };
        FileAttr {
            ino,
            size: e.size,
            blocks: e.size.div_ceil(512),
            atime: e.modified,
            mtime: e.modified,
            ctime: e.modified,
            crtime: e.modified,
            kind: if e.is_dir { FileType::Directory } else { FileType::RegularFile },
            perm,
            nlink: if e.is_dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn entry(&self, ino: u64) -> Option<Entry> {
        self.nodes.get(&ino).map(|n| n.entry.clone())
    }

    fn parent_entry(&self, ino: u64) -> Option<Entry> {
        let p = self.nodes.get(&ino)?.parent;
        self.entry(p)
    }

    fn new_handle(&mut self, ino: u64, prefill: Option<&Entry>, dirty: bool) -> Result<u64, MountError> {
        let tmp = tempfile::NamedTempFile::new().map_err(MountError::io)?;
        let (mut file, path) = tmp.into_parts();
        if let Some(e) = prefill {
            // Opening an existing file for writing without O_TRUNC: start from its content.
            let mut off = 0;
            while off < e.size {
                let piece = self.vfs.read(e, off, crate::vfs::BLOCK)?;
                if piece.is_empty() {
                    break;
                }
                file.write_all(&piece).map_err(MountError::io)?;
                off += piece.len() as u64;
            }
        }
        let fh = self.next_fh;
        self.next_fh += 1;
        self.handles.insert(fh, Handle { ino, file, _tmp: path, dirty });
        Ok(fh)
    }

    /// Upload a dirty handle and refresh its node.
    fn save(&mut self, fh: u64) -> Result<(), MountError> {
        let Some(h) = self.handles.get_mut(&fh) else { return Ok(()) };
        if !h.dirty {
            return Ok(());
        }
        h.file.flush().map_err(MountError::io)?;
        let ino = h.ino;
        let path = h._tmp.to_path_buf();
        let node = self.nodes.get(&ino).ok_or_else(MountError::not_found)?;
        let parent = node.parent;
        let current = node.entry.clone();
        let dir = self.entry(parent).ok_or_else(MountError::not_found)?;
        let saved = self.vfs.write(&dir, &current.name, Some(&current), Path::new(&path))?;
        if saved.name != current.name {
            tracing::warn!(file = %current.name, copy = %saved.name, "changed elsewhere since it was opened: saved as a conflict copy");
            self.intern(saved, parent);
        } else if let Some(n) = self.nodes.get_mut(&ino) {
            n.entry = saved;
        }
        if let Some(h) = self.handles.get_mut(&fh) {
            h.dirty = false;
        }
        Ok(())
    }

    fn lookup_child(&mut self, parent: u64, name: &OsStr) -> Result<(u64, Entry), MountError> {
        let dir = self.entry(parent).ok_or_else(MountError::not_found)?;
        let name = name.to_str().ok_or_else(|| MountError::new("invalid_name", 400, "names must be UTF-8"))?;
        let e = self.vfs.child(&dir, name)?.ok_or_else(MountError::not_found)?;
        let ino = self.intern(e.clone(), parent);
        Ok((ino, e))
    }
}

impl Filesystem for FilesFs {
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        match self.lookup_child(parent, name) {
            Ok((ino, e)) => reply.entry(&TTL, &self.attr(ino, &e), 0),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, fh: Option<u64>, reply: ReplyAttr) {
        let Some(mut e) = self.entry(ino) else { return reply.error(libc::ENOENT) };
        // An open, written handle knows the size before the upload.
        if let Some(h) = fh.and_then(|fh| self.handles.get(&fh)) {
            if let Ok(meta) = h.file.metadata() {
                e.size = meta.len();
            }
        }
        reply.attr(&TTL, &self.attr(ino, &e));
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        let Some(mut e) = self.entry(ino) else { return reply.error(libc::ENOENT) };
        if let Some(size) = size {
            if e.is_dir {
                return reply.error(libc::EISDIR);
            }
            if self.vfs.read_only() || !e.writable {
                return reply.error(libc::EACCES);
            }
            // Truncate: through the open handle, or open one just for this.
            let (fh, temporary) = match fh.filter(|f| self.handles.contains_key(f)) {
                Some(f) => (f, false),
                None => match self.new_handle(ino, (size > 0).then_some(&e), true) {
                    Ok(f) => (f, true),
                    Err(err) => return reply.error(err.errno()),
                },
            };
            if let Some(h) = self.handles.get_mut(&fh) {
                if h.file.set_len(size).is_err() {
                    return reply.error(libc::EIO);
                }
                h.dirty = true;
            }
            if temporary {
                let saved = self.save(fh);
                self.handles.remove(&fh);
                if let Err(err) = saved {
                    return reply.error(err.errno());
                }
                e = self.entry(ino).unwrap_or(e);
            }
            e.size = size;
        }
        // Mode, owner and times are the drive's own; accept and report.
        reply.attr(&TTL, &self.attr(ino, &e));
    }

    fn readdir(&mut self, _req: &Request<'_>, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        let Some(dir) = self.entry(ino) else { return reply.error(libc::ENOENT) };
        if !dir.is_dir {
            return reply.error(libc::ENOTDIR);
        }
        let kids = match self.vfs.children(&dir) {
            Ok(k) => k,
            Err(err) => return reply.error(err.errno()),
        };
        let parent = self.nodes.get(&ino).map(|n| n.parent).unwrap_or(FUSE_ROOT_ID);
        let mut rows: Vec<(u64, FileType, String)> = vec![(ino, FileType::Directory, ".".into()), (parent, FileType::Directory, "..".into())];
        for k in kids {
            let kind = if k.is_dir { FileType::Directory } else { FileType::RegularFile };
            let name = k.name.clone();
            let child = self.intern(k, ino);
            rows.push((child, kind, name));
        }
        for (i, (child, kind, name)) in rows.into_iter().enumerate().skip(offset as usize) {
            if reply.add(child, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        let Some(e) = self.entry(ino) else { return reply.error(libc::ENOENT) };
        if e.is_dir {
            return reply.error(libc::EISDIR);
        }
        let access = flags & libc::O_ACCMODE;
        if access == libc::O_RDONLY {
            // Reads go straight to the cache; no handle state needed.
            return reply.opened(0, 0);
        }
        if self.vfs.read_only() || !e.writable {
            return reply.error(libc::EACCES);
        }
        let trunc = flags & libc::O_TRUNC != 0;
        match self.new_handle(ino, (!trunc).then_some(&e), trunc) {
            Ok(fh) => reply.opened(fh, 0),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn read(&mut self, _req: &Request<'_>, ino: u64, fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        if let Some(h) = self.handles.get_mut(&fh) {
            let mut buf = vec![0u8; size as usize];
            let n = h.file.seek(SeekFrom::Start(offset as u64)).and_then(|_| h.file.read(&mut buf));
            return match n {
                Ok(n) => reply.data(&buf[..n]),
                Err(_) => reply.error(libc::EIO),
            };
        }
        let Some(e) = self.entry(ino) else { return reply.error(libc::ENOENT) };
        match self.vfs.read(&e, offset.max(0) as u64, size as u64) {
            Ok(data) => reply.data(&data),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn write(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, offset: i64, data: &[u8], _wf: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        let Some(h) = self.handles.get_mut(&fh) else { return reply.error(libc::EBADF) };
        let ok = h.file.seek(SeekFrom::Start(offset.max(0) as u64)).and_then(|_| h.file.write_all(data));
        match ok {
            Ok(()) => {
                h.dirty = true;
                reply.written(data.len() as u32)
            }
            Err(_) => reply.error(libc::EIO),
        }
    }

    fn flush(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        // Upload on close(2), so the writer sees a failure.
        match self.save(fh) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn fsync(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _datasync: bool, reply: ReplyEmpty) {
        match self.save(fh) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn release(&mut self, _req: &Request<'_>, _ino: u64, fh: u64, _flags: i32, _lock: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let saved = self.save(fh);
        self.handles.remove(&fh);
        match saved {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn create(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        let Some(dir) = self.entry(parent) else { return reply.error(libc::ENOENT) };
        let Some(name) = name.to_str() else { return reply.error(libc::EINVAL) };
        // Create the empty file now so it has an ID; content follows on close.
        let empty = match tempfile::NamedTempFile::new() {
            Ok(t) => t,
            Err(_) => return reply.error(libc::EIO),
        };
        let e = match self.vfs.write(&dir, name, None, empty.path()) {
            Ok(e) => e,
            Err(err) => return reply.error(err.errno()),
        };
        let ino = self.intern(e.clone(), parent);
        match self.new_handle(ino, None, false) {
            Ok(fh) => reply.created(&TTL, &self.attr(ino, &e), 0, fh, 0),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn mkdir(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        let Some(dir) = self.entry(parent) else { return reply.error(libc::ENOENT) };
        let Some(name) = name.to_str() else { return reply.error(libc::EINVAL) };
        match self.vfs.mkdir(&dir, name) {
            Ok(e) => {
                let ino = self.intern(e.clone(), parent);
                reply.entry(&TTL, &self.attr(ino, &e), 0)
            }
            Err(err) => reply.error(err.errno()),
        }
    }

    fn unlink(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let res = self.lookup_child(parent, name).and_then(|(_, e)| {
            let dir = self.entry(parent).ok_or_else(MountError::not_found)?;
            self.vfs.delete(&e, &dir)
        });
        match res {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn rmdir(&mut self, req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        // A folder goes to trash with its contents, like the file managers do.
        self.unlink(req, parent, name, reply)
    }

    fn rename(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, newparent: u64, newname: &OsStr, flags: u32, reply: ReplyEmpty) {
        let Some(new_name) = newname.to_str() else { return reply.error(libc::EINVAL) };
        let no_replace = flags & libc::RENAME_NOREPLACE != 0;
        let res = self.lookup_child(parent, name).and_then(|(ino, e)| {
            let from = self.entry(parent).ok_or_else(MountError::not_found)?;
            let to = self.entry(newparent).ok_or_else(MountError::not_found)?;
            let moved = self.vfs.rename(&e, &from, &to, new_name, !no_replace)?;
            if let Some(n) = self.nodes.get_mut(&ino) {
                n.entry = moved;
                n.parent = newparent;
            }
            Ok(())
        });
        match res {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(err.errno()),
        }
    }

    fn statfs(&mut self, _req: &Request<'_>, _ino: u64, reply: ReplyStatfs) {
        // The computer's real free space comes from /storage; a large constant
        // keeps file managers from refusing copies before the server decides.
        reply.statfs(1 << 32, 1 << 31, 1 << 31, 1 << 24, 1 << 23, 4096, 255, 4096);
    }
}

/// Mount and serve until unmounted (`fusermount3 -u <dir>` / `gizzi files unmount`).
pub fn mount(vfs: Arc<Vfs>, mountpoint: &Path, name: &str) -> std::io::Result<()> {
    let mut opts = vec![MountOption::FSName(format!("allternit:{name}")), MountOption::Subtype("allternit".into()), MountOption::NoDev, MountOption::NoSuid];
    opts.push(if vfs.read_only() { MountOption::RO } else { MountOption::RW });
    fuser::mount2(FilesFs::new(vfs), mountpoint, &opts)
}
