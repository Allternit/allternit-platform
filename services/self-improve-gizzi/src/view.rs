//! The harness view: the small set of Gizzi files a candidate may edit.
//!
//! `WorkingCopy::create` copies a whole tree and only drops protected paths,
//! so it must never run on the repo root (the suite, graders and tests would
//! be inside it). Instead [`export_view`] copies just the editable files into
//! a temp dir, keeping their repo-relative paths, and the working copy is
//! made from that view. Nothing else from the repo is ever in it.

use allternit_self_improve::working_copy::WorkingCopy;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Gizzi package, relative to the platform repo root.
pub const GIZZI_DIR: &str = "cmd/gizzi-code";
/// `*.txt` directly in here may be edited (system prompt parts).
pub const PROMPT_DIR: &str = "cmd/gizzi-code/src/runtime/session/prompt";
/// `*.txt` directly in here may be edited (tool descriptions).
pub const TOOL_TEXT_DIR: &str = "cmd/gizzi-code/src/runtime/tools/builtins";
/// Usage-limit thresholds. Numbers only.
pub const LIMIT_TS: &str = "cmd/gizzi-code/src/runtime/session/limit.ts";
/// Turn guardrail defaults. Numbers only.
pub const GUARDRAIL_TS: &str = "cmd/gizzi-code/src/runtime/session/guardrail-defaults.ts";

/// Paths that score or test a candidate. They are never in the view, and a
/// patch that names any of them is rejected (defense in depth on top of the
/// allowlist).
pub const PROTECTED: &[&str] = &[
    "cmd/gizzi-code/test",
    "cmd/gizzi-code/src/runtime/eval",
    "domains/agent-evals",
    "contracts/eval",
    "cmd/allternit-api/src/agency_api/evals",
];

pub fn protected_paths() -> Vec<PathBuf> {
    PROTECTED.iter().map(PathBuf::from).collect()
}

/// True when `rel` (a repo-relative path with `/` separators) is a file a
/// candidate may edit.
pub fn is_allowed(rel: &str) -> bool {
    rel == LIMIT_TS || rel == GUARDRAIL_TS || is_txt_in(rel, PROMPT_DIR) || is_txt_in(rel, TOOL_TEXT_DIR)
}

/// `rel` is a `.txt` file directly inside `dir` (no subfolders).
fn is_txt_in(rel: &str, dir: &str) -> bool {
    match rel.strip_prefix(dir).and_then(|rest| rest.strip_prefix('/')) {
        Some(name) => !name.contains('/') && name.len() > ".txt".len() && name.ends_with(".txt") && !name.starts_with('.'),
        None => false,
    }
}

/// An exported view. The temp dir is removed when this is dropped.
#[derive(Debug)]
pub struct ViewDir {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    files: Vec<String>,
}

impl ViewDir {
    /// The view root. Files sit under it at their repo-relative paths.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Repo-relative paths of every exported file, sorted.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// A working copy of the view (never of the repo), with the protected
    /// list passed through so `finalize_diff` keeps its own guard too.
    #[allow(dead_code)] // used by the proposer loop (step 3.4)
    pub fn working_copy(&self, dest: &Path) -> Result<WorkingCopy> {
        WorkingCopy::create(&self.root, dest, &protected_paths()).context("could not make a working copy of the harness view")
    }
}

/// Copy only the editable files of `repo` into a new temp dir.
pub fn export_view(repo: &Path) -> Result<ViewDir> {
    let tmp = tempfile::Builder::new().prefix("harness-view-").tempdir().context("could not make a temp dir for the view")?;
    let root = tmp.path().to_path_buf();
    let files = export_view_into(repo, &root)?;
    Ok(ViewDir { _tmp: tmp, root, files })
}

/// Copy only the editable files of `repo` into `dest`. Returns their
/// repo-relative paths, sorted. Symlinks are skipped.
pub fn export_view_into(repo: &Path, dest: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for dir in [PROMPT_DIR, TOOL_TEXT_DIR] {
        let abs = repo.join(dir);
        if !abs.is_dir() {
            bail!("{} is missing; is {} the platform repo root?", dir, repo.display());
        }
        for entry in std::fs::read_dir(&abs).with_context(|| format!("could not read {dir}"))? {
            let entry = entry?;
            // file_type() does not follow symlinks, so a link is never copied.
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let rel = format!("{dir}/{name}");
            if is_allowed(&rel) {
                files.push(rel);
            }
        }
    }
    for rel in [LIMIT_TS, GUARDRAIL_TS] {
        let meta = std::fs::symlink_metadata(repo.join(rel))
            .with_context(|| format!("{rel} is missing; is {} the platform repo root?", repo.display()))?;
        if !meta.is_file() {
            bail!("{rel} is not a regular file");
        }
        files.push(rel.to_string());
    }
    files.sort();
    for rel in &files {
        let target = dest.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(repo.join(rel), &target).with_context(|| format!("could not copy {rel}"))?;
    }
    Ok(files)
}

/// `WorkingCopy::finalize_diff` runs `git diff --no-index <view> <copy>`, so
/// its headers carry both absolute roots. This rewrites them to plain
/// repo-relative `a/<rel>` / `b/<rel>` so the patch applies to a checkout.
#[allow(dead_code)] // used by the proposer loop (step 3.4)
pub fn normalize_view_diff(raw: &str, view_root: &Path, copy_root: &Path) -> String {
    let strip = |p: &Path| p.to_string_lossy().trim_start_matches('/').trim_end_matches('/').to_string();
    let a_prefix = format!("a/{}/", strip(view_root));
    let b_prefix = format!("b/{}/", strip(copy_root));
    let mut out = String::with_capacity(raw.len());
    let mut in_header = false;
    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            in_header = true;
        } else if line.starts_with("@@") {
            in_header = false;
        }
        if in_header && (line.starts_with("diff --git ") || line.starts_with("--- ") || line.starts_with("+++ ")) {
            out.push_str(&line.replace(&a_prefix, "a/").replace(&b_prefix, "b/"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Apply a repo-relative patch to a candidate checkout with `git apply`.
/// Validity is checked separately; this only applies.
pub fn apply_to_checkout(patch: &str, candidate_checkout: &Path) -> Result<()> {
    let mut file = tempfile::Builder::new().prefix("harness-").suffix(".patch").tempfile()?;
    std::io::Write::write_all(&mut file, patch.as_bytes())?;
    let output = Command::new("git")
        .arg("-C")
        .arg(candidate_checkout)
        .args(["apply", "--whitespace=nowarn"])
        .arg(file.path())
        .output()
        .context("could not run git apply")?;
    if !output.status.success() {
        bail!("git apply failed in {}: {}", candidate_checkout.display(), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

/// A throwaway git worktree of the repo for running the guard tests and the
/// eval against one candidate. The editable files are copied over from the
/// repo's working tree so they match the view the patch was made from, and
/// `node_modules` folders are linked (never installed). Removed on drop.
#[derive(Debug)]
pub struct CandidateCheckout {
    repo: PathBuf,
    path: PathBuf,
    _tmp: tempfile::TempDir,
}

impl CandidateCheckout {
    pub fn create(repo: &Path) -> Result<Self> {
        let tmp = tempfile::Builder::new().prefix("harness-candidate-").tempdir()?;
        let path = tmp.path().join("checkout");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg("HEAD")
            .output()
            .context("could not run git worktree add")?;
        if !output.status.success() {
            bail!("git worktree add failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        let checkout = Self { repo: repo.to_path_buf(), path, _tmp: tmp };
        export_view_into(repo, &checkout.path)?;
        for rel in ["node_modules", "cmd/gizzi-code/node_modules"] {
            let source = repo.join(rel);
            let link = checkout.path.join(rel);
            if source.is_dir() && !link.exists() {
                link_dir(&source, &link)?;
            }
        }
        Ok(checkout)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CandidateCheckout {
    fn drop(&mut self) {
        let _ = Command::new("git").arg("-C").arg(&self.repo).args(["worktree", "remove", "--force"]).arg(&self.path).output();
    }
}

#[cfg(unix)]
fn link_dir(source: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, link).with_context(|| format!("could not link {}", link.display()))
}

#[cfg(not(unix))]
fn link_dir(source: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(source, link).with_context(|| format!("could not link {}", link.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A tiny fake repo with editable files and things that must stay out.
    fn fake_repo() -> tempfile::TempDir {
        let repo = tempfile::tempdir().unwrap();
        let r = repo.path();
        write(r, &format!("{PROMPT_DIR}/base-code.txt"), "Be brief.\n");
        write(r, &format!("{PROMPT_DIR}/safety.txt"), "Stay safe.\n");
        write(r, &format!("{PROMPT_DIR}/notes.md"), "not a prompt\n");
        write(r, &format!("{TOOL_TEXT_DIR}/read.txt"), "Reads a file.\n");
        write(r, &format!("{TOOL_TEXT_DIR}/read.ts"), "export {}\n");
        write(r, &format!("{TOOL_TEXT_DIR}/AgentTool/nested.txt"), "nested, not editable\n");
        write(r, LIMIT_TS, "export const DEFAULT_WARN_AT = 0.8\n");
        write(r, GUARDRAIL_TS, "export const X = 1\n");
        write(r, "cmd/gizzi-code/test/session/guardrails.test.ts", "test\n");
        write(r, "domains/agent-evals/gizzi/suites/shared.v1/suite.json", "{}\n");
        write(r, "cmd/gizzi-code/src/runtime/session/prompt.ts", "code\n");
        repo
    }

    #[test]
    fn allowlist_matches_only_editable_files() {
        assert!(is_allowed(&format!("{PROMPT_DIR}/base-code.txt")));
        assert!(is_allowed(&format!("{TOOL_TEXT_DIR}/bash.txt")));
        assert!(is_allowed(LIMIT_TS));
        assert!(is_allowed(GUARDRAIL_TS));
        assert!(!is_allowed(&format!("{TOOL_TEXT_DIR}/AgentTool/x.txt")), "subfolders are not in the glob");
        assert!(!is_allowed(&format!("{PROMPT_DIR}/base-code.ts")));
        assert!(!is_allowed(&format!("{PROMPT_DIR}/.txt")));
        assert!(!is_allowed(&format!("{PROMPT_DIR}/../prompt.ts")));
        assert!(!is_allowed("cmd/gizzi-code/test/session/guardrails.test.ts"));
        assert!(!is_allowed("cmd/gizzi-code/src/runtime/session/prompt.ts"));
    }

    #[test]
    fn export_copies_only_the_editable_files() {
        let repo = fake_repo();
        let view = export_view(repo.path()).unwrap();
        assert_eq!(
            view.files().to_vec(),
            vec![
                GUARDRAIL_TS.to_string(),
                LIMIT_TS.to_string(),
                format!("{PROMPT_DIR}/base-code.txt"),
                format!("{PROMPT_DIR}/safety.txt"),
                format!("{TOOL_TEXT_DIR}/read.txt"),
            ]
        );
        let root = view.root();
        assert_eq!(std::fs::read_to_string(root.join(LIMIT_TS)).unwrap(), "export const DEFAULT_WARN_AT = 0.8\n");
        assert!(!root.join("cmd/gizzi-code/test").exists(), "tests must never be in the view");
        assert!(!root.join("domains").exists(), "the suite must never be in the view");
        assert!(!root.join(format!("{PROMPT_DIR}/notes.md")).exists());
        assert!(!root.join(format!("{TOOL_TEXT_DIR}/AgentTool")).exists());
        assert!(!root.join("cmd/gizzi-code/src/runtime/session/prompt.ts").exists());
    }

    #[test]
    fn working_copy_is_made_from_the_view_not_the_repo() {
        let repo = fake_repo();
        let view = export_view(repo.path()).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let copy = view.working_copy(dest.path()).unwrap();
        assert!(copy.path().join(LIMIT_TS).exists());
        assert!(!copy.path().join("cmd/gizzi-code/test").exists());
        assert!(!copy.path().join("domains").exists());
    }

    #[test]
    fn export_fails_outside_the_platform_repo() {
        let empty = tempfile::tempdir().unwrap();
        let err = export_view(empty.path()).unwrap_err().to_string();
        assert!(err.contains("platform repo root"), "{err}");
    }

    #[test]
    fn normalize_rewrites_both_roots_in_headers_only() {
        let raw = concat!(
            "diff --git a/tmp/v/cmd/x/f.txt b/tmp/w/cmd/x/f.txt\n",
            "index 1..2 100644\n",
            "--- a/tmp/v/cmd/x/f.txt\n",
            "+++ b/tmp/w/cmd/x/f.txt\n",
            "@@ -1 +1 @@\n",
            "-see a/tmp/v/ here\n",
            "+changed\n",
        );
        let out = normalize_view_diff(raw, Path::new("/tmp/v"), Path::new("/tmp/w"));
        assert!(out.starts_with("diff --git a/cmd/x/f.txt b/cmd/x/f.txt\n"));
        assert!(out.contains("--- a/cmd/x/f.txt\n+++ b/cmd/x/f.txt\n"));
        assert!(out.contains("-see a/tmp/v/ here\n"), "hunk bodies are left alone");
    }

    #[test]
    fn apply_to_checkout_applies_a_repo_relative_patch() {
        let checkout = tempfile::tempdir().unwrap();
        write(checkout.path(), LIMIT_TS, "export const DEFAULT_WARN_AT = 0.8\n");
        let patch = format!(
            "diff --git a/{LIMIT_TS} b/{LIMIT_TS}\n--- a/{LIMIT_TS}\n+++ b/{LIMIT_TS}\n@@ -1 +1 @@\n-export const DEFAULT_WARN_AT = 0.8\n+export const DEFAULT_WARN_AT = 0.85\n"
        );
        apply_to_checkout(&patch, checkout.path()).unwrap();
        assert_eq!(std::fs::read_to_string(checkout.path().join(LIMIT_TS)).unwrap(), "export const DEFAULT_WARN_AT = 0.85\n");
    }

    #[test]
    fn apply_to_checkout_surfaces_a_patch_that_does_not_fit() {
        let checkout = tempfile::tempdir().unwrap();
        write(checkout.path(), LIMIT_TS, "something else\n");
        let patch = format!(
            "diff --git a/{LIMIT_TS} b/{LIMIT_TS}\n--- a/{LIMIT_TS}\n+++ b/{LIMIT_TS}\n@@ -1 +1 @@\n-export const DEFAULT_WARN_AT = 0.8\n+export const DEFAULT_WARN_AT = 0.85\n"
        );
        assert!(apply_to_checkout(&patch, checkout.path()).is_err());
        assert_eq!(std::fs::read_to_string(checkout.path().join(LIMIT_TS)).unwrap(), "something else\n");
    }
}
