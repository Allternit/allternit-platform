//! The Memory Drive root (SPEC section 3: "writes go through the Memory Drive
//! write path so its git history stays").
//!
//! The Memory Drive is a bare git repository per owner (`memory_drive.rs`), not
//! a folder. Files serves it from a read view: the drive's current snapshot
//! copied into `<files data>/views/memory-drive` and indexed by the engine.
//! A Files write or delete there is one commit through `memory_drive_service::apply`
//! (one git commit), and the view is refreshed from git afterwards. Nothing
//! writes the view folder directly, so git stays the source of truth.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use allternit_files_core::{ErrorCode, ItemKind, RootKind};
use allternit_files_engine::{FilesError, ItemRecord, RootRecord};

use super::{meta, roots, ApiError, Fail, Inner};
use crate::memory_drive::{DriveError, MemoryDrive, Operation};
use crate::memory_drive_service::{self as service, ServiceError};

pub fn is_memory(root: &RootRecord) -> bool {
    root.kind == RootKind::MemoryDrive
}

/// Mutations Files can't express on a git-backed store.
pub fn refuse(what: &str) -> Fail {
    ApiError::forbidden(format!("Memory Drive can't {what} through Files; edit its files instead.")).into()
}

fn map_service(e: ServiceError) -> Fail {
    match e {
        ServiceError::Drive(d) => map_drive(d),
        ServiceError::NotFound => ApiError::new(ErrorCode::NotFound, "This account has no Memory Drive yet.").into(),
        ServiceError::Forbidden => ApiError::forbidden("You don't have access to this Memory Drive.").into(),
        other => {
            tracing::warn!(error = %other, "files v2: memory drive");
            ApiError::internal().into()
        }
    }
}

fn map_drive(e: DriveError) -> Fail {
    match e {
        DriveError::Invalid { .. } | DriveError::Secret | DriveError::Limit(_) => ApiError::invalid(e.to_string()).into(),
        DriveError::Conflict { .. } => {
            ApiError::new(ErrorCode::RevMismatch, "Memory Drive changed; list it again and retry.").into()
        }
        DriveError::NotFound(_) => ApiError::new(ErrorCode::NotFound, "Not found in Memory Drive.").into(),
        other => {
            tracing::warn!(error = %other, "files v2: memory drive");
            ApiError::internal().into()
        }
    }
}

fn storage(inner: &Inner, owner: &str) -> Result<Option<MemoryDrive>, Fail> {
    let Some(db) = &inner.cfg.memory_db else { return Ok(None) };
    match service::resolve(db, owner) {
        Ok(record) => Ok(Some(record.storage().map_err(map_service)?)),
        Err(ServiceError::NotFound) => Ok(None),
        Err(e) => Err(map_service(e)),
    }
}

fn memory_root(inner: &Inner) -> Result<Option<RootRecord>, Fail> {
    let row = meta::provider(&inner.meta.lock().expect("meta lock"), "memory_drive")?;
    Ok(row.and_then(|(id, _, _)| inner.drive.root(&id).ok()))
}

/// Copies the owner's current Memory Drive into the view and reindexes it,
/// when git moved since the last copy.
pub fn sync_view(inner: &Inner, owner: &str) -> Result<(), Fail> {
    let Some(root) = memory_root(inner)? else { return Ok(()) };
    let Some(store) = storage(inner, owner)? else { return Ok(()) };
    let head = store.head().map_err(map_drive)?;
    if head.is_some() && *inner.memory_rev.lock().expect("memory rev") == head {
        return Ok(());
    }
    let snap = store.snapshot(None).map_err(map_drive)?;
    let view = roots::memory_view_dir(inner);
    let mut keep = BTreeSet::new();
    for (rel, content) in &snap.files {
        // Paths come from git, already validated on write; check again before
        // they become file names.
        if crate::memory_drive::validate_path(rel).is_err() && rel != "MEMORY.md" {
            continue;
        }
        let path = view.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Fail::Files(e.into()))?;
        }
        if std::fs::read(&path).ok().as_deref() != Some(content.as_bytes()) {
            std::fs::write(&path, content).map_err(|e| Fail::Files(e.into()))?;
        }
        keep.insert(path);
    }
    remove_others(&view, &keep).map_err(|e| Fail::Files(e.into()))?;
    inner.drive.scan_root(&root.root_id)?;
    *inner.memory_rev.lock().expect("memory rev") = head;
    Ok(())
}

fn remove_others(dir: &Path, keep: &BTreeSet<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        let md = std::fs::symlink_metadata(&p)?;
        if md.is_dir() {
            remove_others(&p, keep)?;
            if std::fs::read_dir(&p)?.next().is_none() {
                std::fs::remove_dir(&p)?;
            }
        } else if !keep.contains(&p) {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// The git path of `name` inside `parent` (a folder in the view).
fn git_path(inner: &Inner, parent: &ItemRecord, name: &str) -> Result<String, Fail> {
    let view = std::fs::canonicalize(roots::memory_view_dir(inner)).map_err(|e| Fail::Files(e.into()))?;
    let dir = inner.drive.local_path(&parent.id)?;
    let rel = dir.strip_prefix(&view).map_err(|_| ApiError::forbidden("That folder is not in Memory Drive."))?;
    let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    parts.push(name.to_string());
    Ok(parts.join("/"))
}

/// One commit through the Memory Drive write path (`memory_drive_service::apply`:
/// managed-path guard, provenance checks, CAS on the head, reindex).
fn commit(inner: &Inner, owner: &str, store: &MemoryDrive, op: Operation, message: &str) -> Result<(), Fail> {
    let db = inner.cfg.memory_db.as_ref().ok_or_else(ApiError::internal)?;
    let head = store
        .head()
        .map_err(map_drive)?
        .ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This Memory Drive is not set up yet."))?;
    match service::apply(db, owner, &head, &[op], message) {
        // The commit is in git; only the row index lags, and the view reads git.
        Ok(_) | Err(ServiceError::IndexPending { .. }) => Ok(()),
        Err(e) => Err(map_service(e)),
    }
}

fn check_base(existing: Option<&ItemRecord>, base_rev: Option<&str>) -> Result<(), Fail> {
    match (existing, base_rev) {
        (Some(it), Some(br)) if it.rev() != br => Err(FilesError::RevMismatch { current: Box::new(it.clone()) }.into()),
        (Some(it), None) => Err(FilesError::AlreadyExists(it.name.clone()).into()),
        (None, Some(_)) => Err(FilesError::NotFound("the file this edit was based on is gone".into()).into()),
        _ => Ok(()),
    }
}

/// A small write into Memory Drive: one git commit, then the view.
/// Fails closed on any clash (no keep-both copies in Memory Drive).
pub fn write(
    inner: &Inner,
    owner: &str,
    parent_id: &str,
    name: &str,
    data: &[u8],
    base_rev: Option<&str>,
) -> Result<(ItemRecord, String), Fail> {
    let store = storage(inner, owner)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This account has no Memory Drive yet."))?;
    sync_view(inner, owner)?;
    let parent = inner.drive.stat(parent_id)?;
    if parent.kind != ItemKind::Folder {
        return Err(ApiError::invalid("parent_id is not a folder").into());
    }
    let content = String::from_utf8(data.to_vec()).map_err(|_| ApiError::invalid("Memory Drive files are UTF-8 Markdown."))?;
    let existing = inner.drive.list(&parent.id)?.into_iter().find(|i| i.name == name);
    check_base(existing.as_ref(), base_rev)?;
    let path = git_path(inner, &parent, name)?;
    commit(inner, owner, &store, Operation::SetFile { path, content }, "Files drive write")?;
    *inner.memory_rev.lock().expect("memory rev") = None;
    sync_view(inner, owner)?;
    let item = inner
        .drive
        .list(&parent.id)?
        .into_iter()
        .find(|i| i.name == name)
        .ok_or_else(|| ApiError::invalid("Memory Drive did not keep that file."))?;
    let cursor = inner.drive.current_cursor(&item.root_id)?;
    Ok((item, cursor))
}

/// A delete in Memory Drive: one git commit (git keeps the history; there is
/// no Files trash here).
pub fn delete(inner: &Inner, owner: &str, item: &ItemRecord, base_rev: Option<&str>) -> Result<String, Fail> {
    let store = storage(inner, owner)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "This account has no Memory Drive yet."))?;
    if item.kind != ItemKind::File {
        return Err(refuse("delete folders"));
    }
    if let Some(br) = base_rev {
        if item.rev() != br {
            return Err(FilesError::RevMismatch { current: Box::new(item.clone()) }.into());
        }
    }
    let parent = inner.drive.stat(item.parent_id.as_deref().unwrap_or_default())?;
    let path = git_path(inner, &parent, &item.name)?;
    commit(inner, owner, &store, Operation::DeleteFile { path }, "Files drive delete")?;
    *inner.memory_rev.lock().expect("memory rev") = None;
    sync_view(inner, owner)?;
    Ok(inner.drive.current_cursor(&item.root_id)?)
}
