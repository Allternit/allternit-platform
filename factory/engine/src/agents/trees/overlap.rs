//! Early conflict warning: files changed in more than one live tree of the
//! same repo (committed since the fork from origin/main, plus uncommitted).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use super::git;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Overlap {
    pub a: String,
    pub b: String,
    pub files: Vec<String>,
}

/// Files a tree changed against origin/main.
pub fn changed_files(dir: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Ok(base) = git::run(dir, &["merge-base", "origin/main", "HEAD"]) {
        if let Ok(out) = git::run(dir, &["diff", "--name-only", &base, "HEAD"]) {
            set.extend(out.lines().map(String::from));
        }
    }
    if let Ok(dirty) = git::dirty_files(dir) {
        set.extend(dirty);
    }
    set
}

/// `(tree id, repo, changed files)`.
pub type Changed = (String, String, BTreeSet<String>);

/// Pairs of trees (same repo) that touch the same files.
pub fn find(trees: &[Changed]) -> Vec<Overlap> {
    let mut by_repo: BTreeMap<&str, Vec<&Changed>> = BTreeMap::new();
    for t in trees {
        by_repo.entry(t.1.as_str()).or_default().push(t);
    }
    let mut out = Vec::new();
    for list in by_repo.values() {
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                let files: Vec<String> = a.2.intersection(&b.2).cloned().collect();
                if !files.is_empty() {
                    out.push(Overlap { a: a.0.clone(), b: b.0.clone(), files });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_only_within_a_repo() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<BTreeSet<_>>();
        let trees = vec![
            ("r~a".to_string(), "r".to_string(), s(&["x.rs", "y.rs"])),
            ("r~b".to_string(), "r".to_string(), s(&["y.rs"])),
            ("q~c".to_string(), "q".to_string(), s(&["y.rs"])),
        ];
        let o = find(&trees);
        assert_eq!(o, vec![Overlap { a: "r~a".into(), b: "r~b".into(), files: vec!["y.rs".into()] }]);
    }
}
