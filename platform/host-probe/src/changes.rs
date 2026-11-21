//! Change records for `POST /v1/host/actions`: one JSON file per change in
//! the runtime home, written before the action runs.
//!
//! Layout under [`ChangeStore::root`] (default `~/.allternit/host-probe`,
//! override with `ALLTERNIT_HOST_PROBE_DIR`):
//!
//! | Path | Holds |
//! |---|---|
//! | `changes/<change_id>.json` | one [`ChangeRecord`] |
//! | `changes/approvals/<approval_id>` | marker: this approval id was used |
//! | `snippets/<snippet_id>.json` | saved snippets that `snippet.run` can run |
//!
//! No database and no migration: the files are the audit trail for the
//! Activity tab. A record left at `pending` means the runtime stopped while
//! the action ran.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::actions::{ActionType, UndoAction};
use crate::OsFamily;

/// Where a change stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeStatus {
    /// Written before the action ran. Still pending later = interrupted.
    Pending,
    Succeeded,
    Failed,
    /// The OS refused for lack of rights; nothing was changed and the
    /// approval id can be used again after the elevation setup.
    NeedsElevation,
}

/// One change, as stored and as listed in the Activity tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRecord {
    pub id: String,
    pub approval_id: String,
    #[serde(rename = "type")]
    pub action_type: ActionType,
    pub target: String,
    pub status: ChangeStatus,
    /// ISO 8601 UTC.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<OsFamily>,
    /// Restore point the cloud recorded for this change (D6), for example the
    /// id of the latest Orgo daily backup. `None` on paired computers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_point: Option<String>,
    /// What the target looked like before (service state, port owner,
    /// update versions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<serde_json::Value>,
    /// What it looked like after, read back once the commands finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<serde_json::Value>,
    /// Commands run, masked (program and arguments).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// Last part of the combined output, masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoAction>,
    /// Plain words for why there is no undo, or what the result means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// JSON file store for change records, approvals used, and snippets.
#[derive(Debug, Clone)]
pub struct ChangeStore {
    root: PathBuf,
}

/// Errors are plain strings: the caller turns them into
/// [`crate::actions::ActionError::Store`].
pub type StoreResult<T> = std::result::Result<T, String>;

impl ChangeStore {
    /// Store rooted at `root` (created on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ChangeStore { root: root.into() }
    }

    /// `ALLTERNIT_HOST_PROBE_DIR`, else `~/.allternit/host-probe`.
    pub fn default_root() -> PathBuf {
        if let Ok(dir) = std::env::var("ALLTERNIT_HOST_PROBE_DIR") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".allternit")
            .join("host-probe")
    }

    /// Store at [`ChangeStore::default_root`].
    pub fn open_default() -> Self {
        ChangeStore::new(ChangeStore::default_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn changes_dir(&self) -> PathBuf {
        self.root.join("changes")
    }

    fn approvals_dir(&self) -> PathBuf {
        self.changes_dir().join("approvals")
    }

    /// Directory holding saved snippets (`<id>.json`).
    pub fn snippets_dir(&self) -> PathBuf {
        self.root.join("snippets")
    }

    fn record_path(&self, id: &str) -> StoreResult<PathBuf> {
        if !safe_file_name(id) {
            return Err(format!("bad change id {id:?}"));
        }
        Ok(self.changes_dir().join(format!("{id}.json")))
    }

    /// Mark `approval_id` as used. Fails when it was used before, so one
    /// approval runs one action once. Atomic (`create_new`).
    pub fn claim_approval(&self, approval_id: &str, change_id: &str) -> StoreResult<bool> {
        if !safe_file_name(approval_id) {
            return Err(format!("bad approval id {approval_id:?}"));
        }
        let dir = self.approvals_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(approval_id))
        {
            Ok(mut f) => {
                f.write_all(change_id.as_bytes())
                    .map_err(|e| format!("write approval marker: {e}"))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(format!("approval marker: {e}")),
        }
    }

    /// Give an approval back (the action never changed anything).
    pub fn release_approval(&self, approval_id: &str) {
        if safe_file_name(approval_id) {
            let _ = fs::remove_file(self.approvals_dir().join(approval_id));
        }
    }

    /// Write a new record. Refuses to overwrite an existing id.
    pub fn create(&self, rec: &ChangeRecord) -> StoreResult<()> {
        let path = self.record_path(&rec.id)?;
        let dir = self.changes_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let body = serde_json::to_vec_pretty(rec).map_err(|e| e.to_string())?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        f.write_all(&body)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        f.sync_all()
            .map_err(|e| format!("sync {}: {e}", path.display()))
    }

    /// Replace a record (write to a temp file, then rename over it).
    pub fn update(&self, rec: &ChangeRecord) -> StoreResult<()> {
        let path = self.record_path(&rec.id)?;
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(rec).map_err(|e| e.to_string())?;
        fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))
    }

    pub fn get(&self, id: &str) -> StoreResult<Option<ChangeRecord>> {
        let path = self.record_path(id)?;
        match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {}: {e}", path.display())),
        }
    }

    /// Newest first (change ids start with their UTC time), at most `limit`.
    /// Unreadable files are skipped.
    pub fn list(&self, limit: usize) -> StoreResult<Vec<ChangeRecord>> {
        let dir = self.changes_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", dir.display())),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort_unstable_by(|a, b| b.cmp(a));
        Ok(names
            .iter()
            .filter_map(|n| fs::read(dir.join(n)).ok())
            .filter_map(|b| serde_json::from_slice::<ChangeRecord>(&b).ok())
            .take(limit)
            .collect())
    }
}

/// `[A-Za-z0-9@._:-]`, at most 200 chars, not starting with `.` or `-`, so
/// an id can never leave its directory or read as a flag.
pub fn safe_file_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('.')
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | ':' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "host-probe-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn rec(id: &str, approval: &str) -> ChangeRecord {
        ChangeRecord {
            id: id.into(),
            approval_id: approval.into(),
            action_type: ActionType::ServiceStop,
            target: "nginx.service".into(),
            status: ChangeStatus::Pending,
            created_at: "2026-11-20T12:00:00Z".into(),
            finished_at: None,
            os: Some(OsFamily::Linux),
            restore_point: None,
            before: None,
            after: None,
            commands: Vec::new(),
            output: None,
            error: None,
            undo: None,
            note: None,
        }
    }

    #[test]
    fn create_get_update_list() {
        let root = temp_root("store");
        let s = ChangeStore::new(&root);
        assert!(s.list(10).unwrap().is_empty(), "missing dir lists empty");
        s.create(&rec("chg_20261120T120000Z_aaaa", "ap1")).unwrap();
        s.create(&rec("chg_20261120T130000Z_bbbb", "ap2")).unwrap();
        assert!(
            s.create(&rec("chg_20261120T120000Z_aaaa", "ap1")).is_err(),
            "no overwrite on create"
        );

        let mut r = s.get("chg_20261120T120000Z_aaaa").unwrap().unwrap();
        r.status = ChangeStatus::Succeeded;
        s.update(&r).unwrap();
        assert_eq!(
            s.get(&r.id).unwrap().unwrap().status,
            ChangeStatus::Succeeded
        );
        assert!(s.get("chg_missing").unwrap().is_none());

        let all = s.list(10).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "chg_20261120T130000Z_bbbb", "newest first");
        assert_eq!(s.list(1).unwrap().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn approval_is_single_use_until_released() {
        let root = temp_root("approval");
        let s = ChangeStore::new(&root);
        assert!(s.claim_approval("ap-1", "chg_1").unwrap());
        assert!(!s.claim_approval("ap-1", "chg_2").unwrap());
        s.release_approval("ap-1");
        assert!(s.claim_approval("ap-1", "chg_3").unwrap());
        assert!(s.claim_approval("../etc/passwd", "chg_4").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_names_cannot_escape() {
        assert!(safe_file_name("chg_20261120T120000Z_ab12"));
        assert!(safe_file_name("apr:01HX@cloud"));
        for bad in ["", "..", ".hidden", "-rf", "a/b", "a\\b", "a b", "a\0b"] {
            assert!(!safe_file_name(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn record_json_is_camel_case() {
        let mut r = rec("chg_x", "ap");
        r.status = ChangeStatus::NeedsElevation;
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["type"], "service.stop");
        assert_eq!(v["approvalId"], "ap");
        assert_eq!(v["createdAt"], "2026-11-20T12:00:00Z");
        assert_eq!(v["status"], "needsElevation");
        assert!(v.get("finishedAt").is_none());
    }
}
