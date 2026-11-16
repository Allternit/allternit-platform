//! Thin wrappers over the cfapi placeholder calls (raw `windows` 0.58, as in the spike).

use std::ffi::c_void;
use std::path::Path;

use allternit_files_core::{Item, ItemKind};
use windows::core::{Result, HSTRING};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::CloudFilters::*;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_BASIC_INFO,
};

/// `FILE_ATTRIBUTE_PINNED`: the user chose "Always keep on this device".
pub const ATTR_PINNED: u32 = 0x0008_0000;
/// `FILE_ATTRIBUTE_UNPINNED`: the user chose "Free up space".
pub const ATTR_UNPINNED: u32 = 0x0010_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: content is not on disk (dehydrated).
pub const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// RFC 3339 -> Windows FILETIME (100 ns ticks since 1601).
pub fn filetime(ts: &str) -> i64 {
    let unix = chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| (d.timestamp(), d.timestamp_subsec_nanos()))
        .unwrap_or((0, 0));
    (unix.0 + 11_644_473_600) * 10_000_000 + i64::from(unix.1 / 100)
}

pub fn fs_metadata(item: &Item) -> CF_FS_METADATA {
    let dir = item.kind == ItemKind::Folder;
    let mut attrs = if dir { FILE_ATTRIBUTE_DIRECTORY.0 } else { FILE_ATTRIBUTE_NORMAL.0 };
    if !dir && !item.permissions.write {
        attrs = FILE_ATTRIBUTE_READONLY.0;
    }
    let created = filetime(&item.created_at);
    let modified = filetime(&item.modified_at);
    CF_FS_METADATA {
        BasicInfo: FILE_BASIC_INFO {
            CreationTime: created,
            LastAccessTime: modified,
            LastWriteTime: modified,
            ChangeTime: modified,
            FileAttributes: attrs,
        },
        FileSize: if dir { 0 } else { item.size as i64 },
    }
}

/// One placeholder to create, owning the memory its create-info points at.
pub struct NewPlaceholder {
    pub name: HSTRING,
    pub identity: Vec<u8>,
    pub meta: CF_FS_METADATA,
    pub dir: bool,
}

impl NewPlaceholder {
    pub fn new(name: &str, identity: Vec<u8>, meta: CF_FS_METADATA, dir: bool) -> Self {
        NewPlaceholder { name: HSTRING::from(name), identity, meta, dir }
    }
}

/// Create-infos borrowing from `items` (keep `items` alive while they are used).
/// Folders keep on-demand population, so their children load when opened.
pub fn create_infos(items: &[NewPlaceholder]) -> Vec<CF_PLACEHOLDER_CREATE_INFO> {
    items
        .iter()
        .map(|p| {
            let mut info: CF_PLACEHOLDER_CREATE_INFO = unsafe { std::mem::zeroed() };
            info.RelativeFileName = windows::core::PCWSTR(p.name.as_ptr());
            info.FsMetadata = p.meta;
            info.FileIdentity = p.identity.as_ptr() as *const c_void;
            info.FileIdentityLength = p.identity.len() as u32;
            info.Flags = CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC;
            info
        })
        .collect()
}

/// Creates placeholders in `dir` (outside a callback: change feed, conflict copies).
pub fn create_placeholders(dir: &Path, items: &[NewPlaceholder]) -> Result<u32> {
    if items.is_empty() {
        return Ok(0);
    }
    let mut infos = create_infos(items);
    let mut done = 0u32;
    unsafe { CfCreatePlaceholders(&HSTRING::from(dir.as_os_str()), &mut infos, CF_CREATE_FLAG_NONE, Some(&mut done))? };
    Ok(done)
}

/// A protected handle from CfOpenFileWithOplock; cfapi calls take it as is.
pub struct CfHandle(HANDLE);

impl CfHandle {
    pub fn open(path: &Path, write: bool) -> Result<Self> {
        // Updates and dehydration want an exclusive handle (Cloud Mirror sample);
        // it fails while another app has the file open, and the caller retries.
        let flags = if write {
            CF_OPEN_FILE_FLAGS(CF_OPEN_FILE_FLAG_WRITE_ACCESS.0 | CF_OPEN_FILE_FLAG_EXCLUSIVE.0)
        } else {
            CF_OPEN_FILE_FLAG_NONE
        };
        let h = unsafe { CfOpenFileWithOplock(&HSTRING::from(path.as_os_str()), flags)? };
        Ok(CfHandle(h))
    }
    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for CfHandle {
    fn drop(&mut self) {
        unsafe { CfCloseHandle(self.0) };
    }
}

pub struct PlaceholderState {
    pub in_sync: bool,
    pub pin: CF_PIN_STATE,
    pub identity: Vec<u8>,
}

/// Placeholder state, or None when `path` is a plain file (not yet uploaded).
pub fn placeholder_state(path: &Path) -> Option<PlaceholderState> {
    let h = CfHandle::open(path, false).ok()?;
    // u64 buffer: CF_PLACEHOLDER_BASIC_INFO needs 8-byte alignment.
    let mut buf = vec![0u64; (std::mem::size_of::<CF_PLACEHOLDER_BASIC_INFO>() + 4096) / 8 + 1];
    let len = (buf.len() * 8) as u32;
    unsafe {
        CfGetPlaceholderInfo(h.raw(), CF_PLACEHOLDER_INFO_BASIC, buf.as_mut_ptr() as *mut c_void, len, None).ok()?;
        let info = &*(buf.as_ptr() as *const CF_PLACEHOLDER_BASIC_INFO);
        let id = std::slice::from_raw_parts(info.FileIdentity.as_ptr(), info.FileIdentityLength as usize).to_vec();
        Some(PlaceholderState { in_sync: info.InSyncState == CF_IN_SYNC_STATE_IN_SYNC, pin: info.PinState, identity: id })
    }
}

/// New identity (and metadata when given), marked in sync; `dehydrate` drops the
/// local bytes so the next open reads the new revision.
pub fn update(path: &Path, meta: Option<&CF_FS_METADATA>, identity: &[u8], dehydrate: bool) -> Result<()> {
    let h = CfHandle::open(path, true)?;
    let mut flags = CF_UPDATE_FLAG_MARK_IN_SYNC.0;
    if dehydrate {
        flags |= CF_UPDATE_FLAG_DEHYDRATE.0;
    }
    unsafe {
        CfUpdatePlaceholder(
            h.raw(),
            meta.map(|m| m as *const CF_FS_METADATA),
            Some(identity.as_ptr() as *const c_void),
            identity.len() as u32,
            None,
            CF_UPDATE_FLAGS(flags),
            None,
            None,
        )
    }
}

/// Turns a plain local file or folder (just uploaded) into an in-sync placeholder.
pub fn convert(path: &Path, identity: &[u8]) -> Result<()> {
    let h = CfHandle::open(path, true)?;
    unsafe {
        CfConvertToPlaceholder(
            h.raw(),
            Some(identity.as_ptr() as *const c_void),
            identity.len() as u32,
            CF_CONVERT_FLAG_MARK_IN_SYNC,
            None,
            None,
        )
    }
}

pub fn set_in_sync(path: &Path) -> Result<()> {
    let h = CfHandle::open(path, true)?;
    unsafe { CfSetInSyncState(h.raw(), CF_IN_SYNC_STATE_IN_SYNC, CF_SET_IN_SYNC_FLAG_NONE, None) }
}

/// "Free up space": drop the local bytes, keep the placeholder.
pub fn dehydrate(path: &Path) -> Result<()> {
    let h = CfHandle::open(path, true)?;
    unsafe { CfDehydratePlaceholder(h.raw(), 0, -1, CF_DEHYDRATE_FLAG_NONE, None) }
}

/// "Always keep on this device": fetch the whole file now.
pub fn hydrate(path: &Path) -> Result<()> {
    let h = CfHandle::open(path, false)?;
    unsafe { CfHydratePlaceholder(h.raw(), 0, -1, CF_HYDRATE_FLAG_NONE, None) }
}

pub fn attributes(path: &Path) -> u32 {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path).map(|m| m.file_attributes()).unwrap_or(0)
}

/// The long, absolute form of an existing path (`C:\Users\runneradmin\...`, not
/// the 8.3 `RUNNER~1` form `%TEMP%` can give). cfapi callbacks report long paths,
/// so the helper compares everything in this form.
pub fn long_path(p: &Path) -> std::path::PathBuf {
    match std::fs::canonicalize(p) {
        Ok(c) => {
            let s = c.to_string_lossy();
            match s.strip_prefix(r"\\?\UNC\") {
                Some(rest) => std::path::PathBuf::from(format!(r"\\{rest}")),
                None => std::path::PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()),
            }
        }
        Err(_) => p.to_path_buf(),
    }
}
