//! Twin memory in the Memory Drive (Phase 3). `twin_memory` stays the source
//! of truth for activation: only the owner's accept makes a fact active.
//! The drive gets a read-only projection under `twin/` (active and proposed
//! files, with status, visibility and provenance on every line) so the owner
//! sees and clones it with the rest of their memory. Users and pushes can't
//! write `twin/` (see `guard_managed_paths` and the pre-receive hook), and
//! twin lines are never indexed as active personal facts.
use std::collections::BTreeMap;

use serde_json::Value;

use crate::db::DbHandle;
use crate::memory_drive::{Entry, Operation};
use crate::memory_drive_service::{self as service, Result};

fn clean(v: &str) -> String {
    v.split_whitespace().collect::<Vec<_>>().join(" ").replace([';', '[', ']', '\\'], "")
}

fn entry(m: &Value) -> Option<Entry> {
    let id = m["id"].as_str()?;
    let content = clean(m["content"].as_str()?);
    let subject = clean(m["subject"].as_str().unwrap_or_default());
    let text = if subject.is_empty() { content } else { format!("{subject}: {content}") };
    let added = m["provenance"]["learnedAt"].as_str().unwrap_or_default().get(..10)?.to_string();
    let mut e = Entry { id: id.to_string(), text, source: format!("allternit:twin/{id}"), added, metadata: Default::default() };
    e.metadata.insert("scope".into(), "twin".into());
    e.metadata.insert("status".into(), m["status"].as_str()?.to_string());
    e.metadata.insert("visibility".into(), m["visibility"].as_str()?.to_string());
    e.metadata.insert("twin_kind".into(), m["kind"].as_str()?.to_string());
    if let Some(bot) = m["botId"].as_str() {
        e.metadata.insert("bot".into(), clean(bot));
    }
    e.metadata.insert("learned_from".into(), clean(m["provenance"]["source"].as_str().unwrap_or("owner")));
    e.render().ok()?;
    Some(e)
}

fn file(title: &str, entries: &[Entry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut s = format!("# {title}\n\n## Managed by Allternit from Settings → Your twin\n\n");
    for e in entries {
        s.push_str(&e.render().ok()?);
        s.push('\n');
    }
    Some(s)
}

/// Rewrite `twin/` from the twin store when it differs. Best effort for
/// callers: the twin store is canonical, so a failure only delays the mirror.
pub fn sync(db: &DbHandle, owner: &str) -> Result<()> {
    let Some(drive) = crate::memory_drive_writer::ensure(db, owner)? else { return Ok(()) };
    let rows = crate::twin_persona::list_memory(&db.connect()?, owner, None).map_err(service::ServiceError::Provenance)?;
    let mut by_status: BTreeMap<&str, Vec<Entry>> = BTreeMap::new();
    for m in &rows {
        if let Some(e) = entry(m) {
            let key = if m["status"].as_str() == Some("active") { "active" } else { "proposed" };
            by_status.entry(key).or_default().push(e);
        }
    }
    for list in by_status.values_mut() {
        list.sort_by(|a, b| a.id.cmp(&b.id));
    }
    for _ in 0..4 {
        let storage = drive.storage()?;
        let Some(head) = storage.head()? else { return Ok(()) };
        let snapshot = storage.snapshot(Some(&head))?;
        let mut ops = Vec::new();
        for (key, title) in [("active", "Twin: active"), ("proposed", "Twin: waiting for review")] {
            let path = format!("twin/{key}.md");
            match file(title, by_status.get(key).map(Vec::as_slice).unwrap_or(&[])) {
                Some(content) if snapshot.files.get(&path) != Some(&content) => ops.push(Operation::SetFile { path, content }),
                None if snapshot.files.contains_key(&path) => ops.push(Operation::DeleteFile { path }),
                _ => {}
            }
        }
        if ops.is_empty() {
            return Ok(());
        }
        match service::apply_as(db, owner, &head, &ops, "Sync twin memory", "Allternit") {
            Ok(_) => return Ok(()),
            Err(service::ServiceError::Drive(crate::memory_drive::DriveError::Conflict { .. })) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn sync_logged(db: &DbHandle, owner: &str) {
    if let Err(e) = sync(db, owner) {
        tracing::warn!("twin memory mirror to drive failed (twin store unchanged): {e}");
    }
}
