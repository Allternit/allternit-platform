//! Roots registry and policy (SPEC section 3).
//!
//! On first use the drive creates `~/Allternit` and its six folders, and the
//! three provider roots: Memory Drive (a read view of the git-backed store;
//! writes go through `memory.rs`), Code and Models. A provider whose source
//! is missing is registered on an empty placeholder folder with
//! `provider_ready: false`, and moved onto its real source when that appears.

use std::path::{Path, PathBuf};

use allternit_files_core::{ErrorCode, PrincipalKind, Root, RootKind, RootPolicy, VersionPolicy};
use allternit_files_engine::{RootRecord, RootSpec};

use super::{meta, ApiError, Fail, Inner, Req};

pub fn policy(kind: RootKind) -> RootPolicy {
    let base = RootPolicy {
        read: true,
        write: true,
        shareable: false,
        agent_process: false,
        protected: false,
        offline: true,
        replica: true,
        versions: VersionPolicy { keep_count: 10, keep_days: 30 },
        trash_days: 30,
    };
    match kind {
        RootKind::Inbox => RootPolicy { shareable: true, agent_process: true, ..base },
        RootKind::Shared => RootPolicy { shareable: true, ..base },
        RootKind::AgentOutput => RootPolicy { agent_process: true, ..base },
        // Git keeps the history; Files keeps no versions or trash of its own.
        RootKind::MemoryDrive => RootPolicy {
            protected: true,
            offline: false,
            replica: false,
            versions: VersionPolicy { keep_count: 0, keep_days: 0 },
            trash_days: 0,
            ..base
        },
        // local-engine owns the cache; Files reads and transfers it.
        RootKind::Models => RootPolicy {
            write: false,
            offline: false,
            versions: VersionPolicy { keep_count: 0, keep_days: 0 },
            trash_days: 0,
            ..base
        },
        _ => base,
    }
}

fn kind_str(kind: RootKind) -> &'static str {
    match kind {
        RootKind::MemoryDrive => "memory_drive",
        RootKind::Code => "code",
        RootKind::Models => "models",
        _ => "other",
    }
}

const HOME_FOLDERS: &[(RootKind, &str)] = &[
    (RootKind::Inbox, "Inbox"),
    (RootKind::Shared, "Shared"),
    (RootKind::AgentOutput, "Agent Output"),
    (RootKind::Downloads, "Downloads"),
    (RootKind::Meetings, "Meetings"),
    (RootKind::Scans, "Scans"),
];

/// Folders inside ~/Allternit that no provider fills yet. Downloads (the
/// agent browser) and Meetings (meeting notes) are filled by `routing.rs`.
fn waiting_for_provider(kind: RootKind) -> bool {
    matches!(kind, RootKind::Scans)
}

pub fn memory_view_dir(inner: &Inner) -> PathBuf {
    inner.cfg.data_dir.join("views").join("memory-drive")
}

/// Creates whatever default roots are missing. Safe to run on every start.
pub fn ensure_defaults(inner: &Inner) -> Result<(), Fail> {
    let base = inner.home.join("Allternit");
    let mut specs = vec![RootSpec {
        kind: RootKind::AllternitHome,
        name: "Allternit".into(),
        local_path: base.clone(),
        policy: policy(RootKind::AllternitHome),
    }];
    for (kind, name) in HOME_FOLDERS {
        specs.push(RootSpec { kind: *kind, name: (*name).into(), local_path: base.join(name), policy: policy(*kind) });
    }
    inner.drive.ensure_roots(specs)?;
    refresh_providers(inner)?;
    Ok(())
}

/// Puts each provider root on its source, or on a placeholder while it has none.
pub fn refresh_providers(inner: &Inner) -> Result<(), Fail> {
    let memory_ready = inner
        .cfg
        .memory_db
        .as_ref()
        .and_then(|db| crate::memory_drive_writer::configured_root(&db.connect().ok()?).ok().flatten())
        .is_some();
    let view = memory_view_dir(inner);
    std::fs::create_dir_all(&view).map_err(|e| Fail::Files(e.into()))?;
    let existing = |p: &Option<PathBuf>| p.clone().filter(|p| p.is_dir());
    let providers: [(RootKind, &str, Option<PathBuf>, bool); 3] = [
        (RootKind::MemoryDrive, "Memory Drive", Some(view), memory_ready),
        (RootKind::Code, "Code", existing(&inner.cfg.code_dir), true),
        (RootKind::Models, "Models", existing(&inner.cfg.models_dir), true),
    ];
    for (kind, name, source, ready_if_source) in providers {
        let k = kind_str(kind);
        let (path, ready) = match source {
            Some(p) => (std::fs::canonicalize(&p).map_err(|e| Fail::Files(e.into()))?, ready_if_source),
            None => (inner.cfg.data_dir.join("placeholders").join(k), false),
        };
        let row = meta::provider(&inner.meta.lock().expect("meta lock"), k)?;
        let source_str = path.to_string_lossy().to_string();
        match row {
            Some((root_id, src, was_ready)) if src == source_str && inner.drive.root(&root_id).is_ok() => {
                if was_ready != ready {
                    meta::set_provider(&inner.meta.lock().expect("meta lock"), k, &root_id, &src, ready)?;
                }
            }
            other => {
                if let Some((old, _, _)) = other {
                    // Placeholder replaced by the real source (or the other way round).
                    let _ = inner.drive.remove_root(&old);
                }
                let rec = inner.drive.add_root(RootSpec { kind, name: name.into(), local_path: path, policy: policy(kind) })?;
                if let Some(w) = &inner.watcher {
                    let _ = w.watch_root(&rec.local_path);
                }
                meta::set_provider(&inner.meta.lock().expect("meta lock"), k, &rec.root_id, &source_str, ready)?;
            }
        }
    }
    Ok(())
}

/// `POST /roots` (section 12): check the path like the legacy routes, add the
/// folder as a `user_added` root. Over the relay it starts unshared and with
/// agents off.
pub fn add_user_root(inner: &Inner, req: &Req, name: &str, local_path: &str, mut pol: RootPolicy) -> Result<RootRecord, Fail> {
    let path = inner
        .scope
        .authorize_root(local_path, &[inner.cfg.data_dir.clone()])
        .map_err(|d| ApiError::new(ErrorCode::ScopeViolation, d.message()))?;
    if !path.is_dir() {
        return Err(ApiError::new(ErrorCode::NotFound, "That folder doesn't exist on this computer.").into());
    }
    if req.relayed {
        pol.shareable = false;
        pol.agent_process = false;
    }
    let rec = inner.drive.add_root(RootSpec { kind: RootKind::UserAdded, name: name.into(), local_path: path, policy: pol })?;
    if let Some(w) = &inner.watcher {
        let _ = w.watch_root(&rec.local_path);
    }
    Ok(rec)
}

/// `PATCH /roots/:id`. Fails closed on changes that would widen access from
/// another device, or unprotect Memory Drive.
pub fn check_policy_change(root: &RootRecord, req: &Req, new: &RootPolicy) -> Result<(), ApiError> {
    let old = &root.policy;
    if root.kind == RootKind::MemoryDrive && !new.protected {
        return Err(ApiError::forbidden("Memory Drive is always protected."));
    }
    if root.kind == RootKind::Models && new.write {
        return Err(ApiError::forbidden("The Models folder is managed by the local engine and stays read-only here."));
    }
    if req.relayed && ((new.shareable && !old.shareable) || (new.agent_process && !old.agent_process) || (old.protected && !new.protected)) {
        return Err(ApiError::forbidden(
            "Sharing, agent access and protection can only be loosened from this computer.",
        ));
    }
    Ok(())
}

/// The wire root. `path_hint` only for the owner and own devices, and never
/// for a provider's internal view or placeholder.
pub fn to_wire(inner: &Inner, rec: &RootRecord, all: &[RootRecord], req: &Req) -> Result<Root, Fail> {
    let (provider, display_name) = {
        let m = inner.meta.lock().expect("meta lock");
        (meta::provider_by_root(&m, &rec.root_id)?, meta::root_name(&m, &rec.root_id)?)
    };
    // `internal`: the folder is the drive's own view or placeholder, not a user path.
    let (provider_ready, internal) = match provider {
        Some((_, ready)) => (ready, rec.kind == RootKind::MemoryDrive || !ready),
        None => (!waiting_for_provider(rec.kind), false),
    };
    let owner = matches!(req.principal.kind, PrincipalKind::Owner | PrincipalKind::OwnDevice);
    let parent_root_id = all
        .iter()
        .filter(|o| o.root_id != rec.root_id && rec.local_path.starts_with(&o.local_path))
        .max_by_key(|o| o.local_path.components().count())
        .map(|o| o.root_id.clone());
    Ok(Root {
        id: rec.root_id.clone(),
        kind: rec.kind,
        name: display_name.unwrap_or_else(|| rec.name.clone()),
        path_hint: (owner && !internal).then(|| display_path(&inner.home, &rec.local_path)),
        policy: rec.policy.clone(),
        root_item_id: rec.root_item_id.clone(),
        parent_root_id,
        cursor: inner.drive.current_cursor(&rec.root_id)?,
        usage_bytes: None,
        provider_ready,
    })
}

/// `~/…` for paths in the home folder.
fn display_path(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => p.display().to_string(),
    }
}
