//! The helper's local map: item ID -> relative path, root cursors, and which
//! folders are populated. Saved as JSON in the state dir so a restart resumes the
//! change feed instead of re-listing everything.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Path under the sync root, `\` separated.
    pub rel: String,
    pub root_id: String,
    pub dir: bool,
    pub rev: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootMeta {
    pub name: String,
    pub kind: String,
    pub root_item_id: String,
    pub parent_root_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Index {
    pub items: BTreeMap<String, Entry>,
    pub cursors: BTreeMap<String, String>,
    pub roots: BTreeMap<String, RootMeta>,
    /// Folders (relative paths) whose children have been placed.
    pub populated: BTreeSet<String>,
}

/// The `meta_version` part of a rev (`<meta_version>-<hash>`); it rises on every change.
pub fn rev_version(rev: &str) -> Option<u64> {
    rev.split('-').next()?.parse().ok()
}

pub fn parent_rel(rel: &str) -> &str {
    match rel.rfind('\\') {
        Some(i) => &rel[..i],
        None => "",
    }
}

pub fn join_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}\\{name}")
    }
}

fn under(rel: &str, prefix: &str) -> bool {
    rel == prefix || (rel.len() > prefix.len() && rel.starts_with(prefix) && rel.as_bytes()[prefix.len()] == b'\\')
}

impl Index {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("index serializes"))?;
        std::fs::rename(tmp, path)
    }

    pub fn put(&mut self, item_id: &str, entry: Entry) {
        self.items.insert(item_id.to_string(), entry);
    }

    pub fn by_rel(&self, rel: &str) -> Option<(&String, &Entry)> {
        // Windows paths compare case-insensitively.
        self.items.iter().find(|(_, e)| e.rel.eq_ignore_ascii_case(rel))
    }

    /// Removes an item and everything under it (a deleted folder).
    pub fn remove_tree(&mut self, item_id: &str) -> Option<Entry> {
        let e = self.items.remove(item_id)?;
        if e.dir {
            let prefix = e.rel.clone();
            self.items.retain(|_, x| !under(&x.rel, &prefix));
            self.populated.retain(|p| !under(p, &prefix));
        }
        Some(e)
    }

    /// Moves an item (and its subtree) to `new_rel`.
    pub fn move_tree(&mut self, item_id: &str, new_rel: &str) {
        let Some(old) = self.items.get(item_id).map(|e| e.rel.clone()) else { return };
        let fix = |rel: &str| format!("{new_rel}{}", &rel[old.len()..]);
        for e in self.items.values_mut() {
            if under(&e.rel, &old) {
                e.rel = fix(&e.rel);
            }
        }
        self.populated = self.populated.iter().map(|p| if under(p, &old) { fix(p) } else { p.clone() }).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rel: &str, dir: bool) -> Entry {
        Entry { rel: rel.into(), root_id: "rt".into(), dir, rev: "1-dir".into() }
    }

    // A server-side folder rename must move every child path and populated mark,
    // but not a sibling that only shares the prefix ("docs" vs "docs2").
    #[test]
    fn moving_and_removing_a_folder_touches_only_its_subtree() {
        let mut ix = Index::default();
        ix.put("a", e("Allternit\\docs", true));
        ix.put("b", e("Allternit\\docs\\note.md", false));
        ix.put("c", e("Allternit\\docs2", true));
        ix.populated.insert("Allternit\\docs".into());
        ix.move_tree("a", "Allternit\\papers");
        assert_eq!(ix.items["b"].rel, "Allternit\\papers\\note.md");
        assert_eq!(ix.items["c"].rel, "Allternit\\docs2");
        assert!(ix.populated.contains("Allternit\\papers"));
        assert!(ix.by_rel("allternit\\PAPERS\\note.md").is_some());
        ix.remove_tree("a");
        assert!(!ix.items.contains_key("b"));
        assert!(ix.items.contains_key("c"));
        assert_eq!(parent_rel("Allternit\\docs2"), "Allternit");
        assert_eq!(join_rel("", "Code"), "Code");
        assert_eq!(rev_version("12-ab12"), Some(12));
        assert_eq!(rev_version("3-dir"), Some(3));
    }
}
