//! Anchored multi-file edits for the BUG_FIX patch step (memo C, upgrade 1).
//!
//! The model replies in plain text (never code inside JSON) with
//! SEARCH/REPLACE blocks anchored on unique context, no line numbers:
//!
//! ```text
//! path/to/file.js
//! <<<<<<< SEARCH
//! exact lines from the current file
//! =======
//! the replacement lines
//! >>>>>>> REPLACE
//! ```
//!
//! An empty SEARCH creates a new file, or rewrites a small (< 400 line) file
//! whole; `*** Delete File: path` deletes one. Every edit is validated and
//! applied in memory first ([`plan`]); nothing touches disk until the whole
//! patch applies. Matching is exact-and-unique first, then a
//! whitespace-tolerant line match; zero or multiple matches is an error the
//! repair round sees.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// Whole-file rewrites are only allowed for new files or files below this.
pub const WHOLE_FILE_MAX_LINES: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Replace the unique `search` anchor with `replace` (empty search = create / small whole-file).
    Replace { path: String, search: String, replace: String },
    Delete { path: String },
}

impl Edit {
    pub fn path(&self) -> &str {
        match self {
            Edit::Replace { path, .. } | Edit::Delete { path } => path,
        }
    }
}

/// Parse SEARCH/REPLACE blocks out of a reply. Text outside the blocks
/// (prose, code fences) is ignored. Errors on a malformed block.
pub fn parse_blocks(text: &str) -> Result<Vec<Edit>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let clean = |l: &str| l.trim().trim_matches('`').trim().to_string();
    while i < lines.len() {
        let l = lines[i].trim_end();
        if let Some(p) = l.trim().strip_prefix("*** Delete File:") {
            out.push(Edit::Delete { path: p.trim().to_string() });
            i += 1;
            continue;
        }
        if l.trim_start().starts_with("<<<<<<< SEARCH") {
            // The path is the nearest non-empty, non-fence line above.
            let path = lines[..i].iter().rev().map(|l| clean(l)).find(|l| !l.is_empty() && !l.starts_with("```"))
                .filter(|p| !p.contains(' ') || Path::new(p).extension().is_some())
                .ok_or_else(|| format!("SEARCH block at line {} has no file path above it", i + 1))?;
            let (mut search, mut replace) = (Vec::new(), Vec::new());
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim_end() != "=======" {
                search.push(lines[j]);
                j += 1;
            }
            if j >= lines.len() {
                return Err(format!("SEARCH block for {path} has no ======= divider"));
            }
            j += 1;
            while j < lines.len() && !lines[j].trim_start().starts_with(">>>>>>> REPLACE") {
                replace.push(lines[j]);
                j += 1;
            }
            if j >= lines.len() {
                return Err(format!("SEARCH block for {path} has no >>>>>>> REPLACE end"));
            }
            let join = |v: &[&str]| if v.is_empty() { String::new() } else { format!("{}\n", v.join("\n")) };
            out.push(Edit::Replace { path, search: join(&search), replace: join(&replace) });
            i = j + 1;
            continue;
        }
        i += 1;
    }
    if out.is_empty() {
        return Err("the reply has no SEARCH/REPLACE blocks".into());
    }
    Ok(out)
}

/// A proposal value from the scripted executor (or a structured reply):
/// legacy `{path, content}` (whole file), `{edits: [{path, search, replace} |
/// {path, delete: true}]}`, or `{text: "<SEARCH/REPLACE blocks>"}`.
pub fn from_value(v: &Value) -> Result<Vec<Edit>, String> {
    if let Some(t) = v["text"].as_str() {
        return parse_blocks(t);
    }
    if let (Some(p), Some(c)) = (v["path"].as_str(), v["content"].as_str()) {
        return Ok(vec![Edit::Replace { path: p.into(), search: String::new(), replace: c.into() }]);
    }
    let edits = v["edits"].as_array().ok_or("proposal has no edits")?;
    edits.iter().map(|e| {
        let path = e["path"].as_str().ok_or("edit has no path")?.to_string();
        if e["delete"] == true {
            return Ok(Edit::Delete { path });
        }
        Ok(Edit::Replace { path, search: e["search"].as_str().unwrap_or_default().into(), replace: e["replace"].as_str().unwrap_or_default().into() })
    }).collect()
}

/// The candidate set as journaled by the executor (WP-P1 replay): each entry
/// is `{edits: [...]}` (readable by [`from_value`]) or `{error}`.
pub fn candidates_to_journal(c: &[Result<Vec<Edit>, String>]) -> Value {
    let one = |r: &Result<Vec<Edit>, String>| match r {
        Ok(edits) => serde_json::json!({ "edits": edits.iter().map(|e| match e {
            Edit::Replace { path, search, replace } => serde_json::json!({ "path": path, "search": search, "replace": replace }),
            Edit::Delete { path } => serde_json::json!({ "path": path, "delete": true }),
        }).collect::<Vec<_>>() }),
        Err(e) => serde_json::json!({ "error": e }),
    };
    serde_json::json!({ "candidates": c.iter().map(one).collect::<Vec<_>>() })
}

/// Inverse of [`candidates_to_journal`]; a pre-WP-B1 journal entry (a single
/// `{path, content}`) reads as one whole-file candidate.
pub fn candidates_from_journal(v: &Value) -> Vec<Result<Vec<Edit>, String>> {
    match v["candidates"].as_array() {
        Some(c) => c.iter().map(|e| match e["error"].as_str() {
            Some(err) => Err(err.to_string()),
            None => from_value(e),
        }).collect(),
        None => vec![from_value(v)],
    }
}

/// Repo-relative, no traversal, not .git / .allternit, non-empty.
pub fn path_is_safe(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains('\\')
        && !Path::new(path).components().any(|c| matches!(c, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
        && !path.starts_with(".git") && !path.starts_with(".allternit")
}

/// Gating tests are never edited by the patch step (memo C pitfall:
/// overfitting to the reproduction test). Enforced by path.
pub fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    let stem = name.split('.').next().unwrap_or(name);
    p.split('/').rev().skip(1).any(|d| matches!(d, "test" | "tests" | "__tests__" | "spec" | "specs"))
        || matches!(stem, "test" | "tests" | "spec" | "conftest")
        || stem.starts_with("test_") || stem.ends_with("_test") || stem.ends_with("_spec")
        || name.contains(".test.") || name.contains(".spec.")
}

/// The result of applying a patch in memory: new content per path (`None` =
/// deleted), plus whether the file existed before.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Planned {
    pub files: BTreeMap<String, Option<String>>,
    pub created: Vec<String>,
    /// Changed-line count (removed + added), the "smallest diff" tie-breaker.
    pub diff_lines: usize,
}

impl Planned {
    pub fn paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    /// Whitespace-normalized digest of the outcome: candidates that make the
    /// same change in different whitespace vote together (never vote on raw diffs).
    pub fn normalized_key(&self) -> String {
        let mut s = String::new();
        for (p, c) in &self.files {
            s.push_str(p);
            s.push('\0');
            match c {
                Some(c) => s.extend(c.split_whitespace().flat_map(|w| [w, " "])),
                None => s.push_str("<deleted>"),
            }
            s.push('\0');
        }
        allternit_commrails::receipts::jcs::sha256_tagged(s.as_bytes())
    }
}

/// Validate and apply `edits` in memory against `read` (current content of a
/// repo-relative path, `None` when absent). Any error rejects the whole patch.
pub fn plan(edits: &[Edit], read: impl Fn(&str) -> Option<String>) -> Result<Planned, String> {
    let mut out = Planned::default();
    for (n, e) in edits.iter().enumerate() {
        let path = e.path();
        let at = |m: String| format!("edit {} ({path}): {m}", n + 1);
        if !path_is_safe(path) {
            return Err(at("path is not a safe repo-relative path".into()));
        }
        if is_test_path(path) {
            return Err(at("test files are read-only for the patch step".into()));
        }
        let current = match out.files.get(path) {
            Some(c) => c.clone(),
            None => read(path),
        };
        match e {
            Edit::Delete { .. } => {
                let old = current.ok_or_else(|| at("cannot delete a file that does not exist".into()))?;
                out.diff_lines += old.lines().count();
                out.files.insert(path.into(), None);
            }
            Edit::Replace { search, replace, .. } => {
                let new = match current {
                    None if search.trim().is_empty() => {
                        out.created.push(path.into());
                        out.diff_lines += replace.lines().count();
                        replace.clone()
                    }
                    None => return Err(at("file does not exist (use an empty SEARCH to create it)".into())),
                    Some(old) if search.trim().is_empty() => {
                        if old.lines().count() >= WHOLE_FILE_MAX_LINES {
                            return Err(at(format!("whole-file rewrite refused for a file of {}+ lines; anchor the edit", WHOLE_FILE_MAX_LINES)));
                        }
                        out.diff_lines += old.lines().count() + replace.lines().count();
                        replace.clone()
                    }
                    Some(old) => {
                        let new = apply_anchor(&old, search, replace).map_err(at)?;
                        out.diff_lines += search.lines().count() + replace.lines().count();
                        new
                    }
                };
                if new.is_empty() {
                    return Err(at("the edit leaves the file empty (use *** Delete File)".into()));
                }
                out.files.insert(path.into(), Some(new));
            }
        }
    }
    if out.files.is_empty() {
        return Err("the patch changes nothing".into());
    }
    Ok(out)
}

/// Replace the unique occurrence of `search` in `old`: exact match first,
/// then a whitespace-tolerant line match (indentation / trailing spaces).
pub fn apply_anchor(old: &str, search: &str, replace: &str) -> Result<String, String> {
    let exact: Vec<usize> = old.match_indices(search).map(|(i, _)| i).collect();
    match exact.len() {
        1 => return Ok(format!("{}{}{}", &old[..exact[0]], replace, &old[exact[0] + search.len()..])),
        n if n > 1 => return Err(format!("SEARCH anchor matches {n} places; include more surrounding lines so it is unique")),
        _ => {}
    }
    // Whitespace-tolerant: compare trimmed lines, ignoring blank-line edges of the anchor.
    let norm = |l: &str| l.split_whitespace().collect::<Vec<_>>().join(" ");
    let want: Vec<String> = search.lines().map(norm).collect();
    let (s, e) = (want.iter().position(|l| !l.is_empty()), want.iter().rposition(|l| !l.is_empty()));
    let Some((s, e)) = s.zip(e) else { return Err("SEARCH anchor is blank".into()) };
    let want = &want[s..=e];
    let have: Vec<&str> = old.split_inclusive('\n').collect();
    let hn: Vec<String> = have.iter().map(|l| norm(l)).collect();
    let hits: Vec<usize> = (0..hn.len().saturating_sub(want.len() - 1)).filter(|&i| hn[i..i + want.len()] == *want).collect();
    match hits.len() {
        0 => Err("SEARCH anchor not found in the file (copy the lines exactly as they are now)".into()),
        1 => {
            let i = hits[0];
            let mut out: String = have[..i].concat();
            out.push_str(replace);
            if !replace.is_empty() && !replace.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&have[i + want.len()..].concat());
            Ok(out)
        }
        n => Err(format!("SEARCH anchor matches {n} places (ignoring whitespace); include more surrounding lines so it is unique")),
    }
}
