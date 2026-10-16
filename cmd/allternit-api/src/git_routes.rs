//! `POST /api/v1/git/diff` — the working-tree diff of a local folder.
//!
//! Backs Code mode's Changes pane (`CodeDiffPanel`), the console's Mission
//! Control line counts, the h5i diff panels and the `git_diff` agent tool in
//! allternit-ai. Body: `{ path, staged?, from?, to?, file? }`. Reply:
//! `{ files: [{ path, changeType, additions, deletions, diff }], totalAdditions, totalDeletions }`.
//!
//! Without `from`/`to` it shows uncommitted work: unstaged changes (or staged
//! with `staged: true`) plus untracked files as additions. git runs with
//! explicit args (no shell), external diff tools and textconv off.

use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

use crate::AppState;

/// Untracked files larger than this are listed without their contents.
const MAX_UNTRACKED_BYTES: u64 = 256 * 1024;
/// At most this many untracked files are read.
const MAX_UNTRACKED_FILES: usize = 200;

pub fn git_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/git/diff", post(git_diff))
        .route("/v1/git/discard", post(git_discard))
}

#[derive(Debug, Default, Deserialize)]
pub struct GitDiffRequest {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
}

fn err(status: StatusCode, message: impl Into<String>) -> axum::response::Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

/// A revision argument we pass to git: no leading `-` (would read as an option).
fn safe_rev(rev: &str) -> bool {
    !rev.is_empty() && !rev.starts_with('-') && !rev.contains(char::is_whitespace)
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off", "-c", "diff.external=", "--no-pager"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if detail.is_empty() { format!("git {} failed", args.join(" ")) } else { detail });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn count(diff: &str) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            add += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            del += 1;
        }
    }
    (add, del)
}

/// Split `git diff` output into one entry per file.
pub fn split_diff(raw: &str) -> Vec<serde_json::Value> {
    let mut files = Vec::new();
    let mut chunks: Vec<String> = Vec::new();
    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") || chunks.is_empty() {
            chunks.push(String::new());
        }
        if let Some(last) = chunks.last_mut() {
            last.push_str(line);
        }
    }
    for chunk in chunks.into_iter().filter(|c| c.starts_with("diff --git ")) {
        let header = chunk.lines().next().unwrap_or_default();
        // "diff --git a/<p> b/<p>": the b/ side is the current path.
        let path = header
            .rsplit_once(" b/")
            .map(|(_, p)| p.to_string())
            .unwrap_or_else(|| header.trim_start_matches("diff --git ").to_string());
        let change = if chunk.contains("\nnew file mode") {
            "added"
        } else if chunk.contains("\ndeleted file mode") {
            "deleted"
        } else if chunk.contains("\nrename from ") {
            "renamed"
        } else {
            "modified"
        };
        let (additions, deletions) = count(&chunk);
        files.push(json!({ "path": path, "changeType": change, "additions": additions, "deletions": deletions, "diff": chunk }));
    }
    files
}

/// An untracked file as an all-additions diff.
fn untracked_entry(root: &Path, rel: &str) -> Option<serde_json::Value> {
    let full = root.join(rel);
    let meta = std::fs::metadata(&full).ok()?;
    if !meta.is_file() {
        return None;
    }
    let body = if meta.len() > MAX_UNTRACKED_BYTES {
        None
    } else {
        std::fs::read(&full).ok().and_then(|b| String::from_utf8(b).ok())
    };
    let mut diff = format!("diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n");
    match &body {
        Some(text) if !text.is_empty() => {
            let n = text.lines().count();
            diff.push_str(&format!("@@ -0,0 +1,{n} @@\n"));
            for line in text.lines() {
                diff.push('+');
                diff.push_str(line);
                diff.push('\n');
            }
        }
        Some(_) => {}
        None => diff.push_str("Binary or large file not shown\n"),
    }
    let (additions, deletions) = count(&diff);
    Some(json!({ "path": rel, "changeType": "added", "additions": additions, "deletions": deletions, "diff": diff }))
}

pub async fn diff_for(req: GitDiffRequest) -> Result<serde_json::Value, (StatusCode, String)> {
    let dir = PathBuf::from(req.path.as_deref().filter(|p| !p.trim().is_empty()).unwrap_or("."));
    if !dir.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("not a folder: {}", dir.display())));
    }
    let root = git(&dir, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, "not a git repository".to_string()))?;
    let root = PathBuf::from(root.trim());

    let mut args: Vec<String> = vec!["diff".into(), "--no-color".into(), "--no-ext-diff".into(), "--no-textconv".into(), "-M".into()];
    for rev in [&req.from, &req.to].into_iter().flatten() {
        if !safe_rev(rev) {
            return Err((StatusCode::BAD_REQUEST, format!("invalid revision: {rev}")));
        }
    }
    let comparing = req.from.is_some() || req.to.is_some();
    if req.staged && !comparing {
        args.push("--cached".into());
    }
    if let Some(from) = &req.from {
        args.push(from.clone());
    }
    if let Some(to) = &req.to {
        args.push(to.clone());
    }
    args.push("--".into());
    if let Some(file) = req.file.as_deref().filter(|f| !f.is_empty()) {
        args.push(file.to_string());
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let raw = git(&root, &argv).await.map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let mut files = split_diff(&raw);

    if !req.staged && !comparing {
        let mut ls: Vec<&str> = vec!["ls-files", "--others", "--exclude-standard", "--"];
        if let Some(file) = req.file.as_deref().filter(|f| !f.is_empty()) {
            ls.push(file);
        }
        if let Ok(list) = git(&root, &ls).await {
            for rel in list.lines().filter(|l| !l.is_empty()).take(MAX_UNTRACKED_FILES) {
                if let Some(entry) = untracked_entry(&root, rel) {
                    files.push(entry);
                }
            }
        }
    }

    let total_add: u64 = files.iter().filter_map(|f| f["additions"].as_u64()).sum();
    let total_del: u64 = files.iter().filter_map(|f| f["deletions"].as_u64()).sum();
    Ok(json!({ "files": files, "totalAdditions": total_add, "totalDeletions": total_del, "root": root }))
}

/// `POST /api/v1/git/discard` — undo uncommitted work in one file.
/// Body: `{ path, file, patch?, untracked? }`. `file` is relative to the
/// repository root (as `/git/diff` reports it).
/// - with `patch` (that file's diff header plus one hunk): only that hunk is
///   reversed in the working tree (`git apply -R`), after a `--check`;
/// - without: a tracked file goes back to its staged/committed content
///   (`git checkout -- file`); an untracked file is deleted, but only when the
///   caller says `untracked: true` and git confirms it is untracked.
#[derive(Debug, Default, Deserialize)]
pub struct GitDiscardRequest {
    #[serde(default)]
    pub path: Option<String>,
    pub file: String,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(default)]
    pub untracked: bool,
}

/// A repo-relative path we may hand to git: no absolute paths, no `..`.
fn safe_rel(file: &str) -> bool {
    !file.is_empty()
        && !file.starts_with('/')
        && !file.starts_with('-')
        && !file.contains('\0')
        && !Path::new(file).components().any(|c| matches!(c, std::path::Component::ParentDir))
}

async fn git_stdin(dir: &Path, args: &[&str], input: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub async fn discard(req: GitDiscardRequest) -> Result<serde_json::Value, (StatusCode, String)> {
    let dir = PathBuf::from(req.path.as_deref().filter(|p| !p.trim().is_empty()).unwrap_or("."));
    if !dir.is_dir() {
        return Err((StatusCode::BAD_REQUEST, format!("not a folder: {}", dir.display())));
    }
    if !safe_rel(&req.file) {
        return Err((StatusCode::BAD_REQUEST, format!("invalid file: {}", req.file)));
    }
    let root = git(&dir, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| (StatusCode::UNPROCESSABLE_ENTITY, "not a git repository".to_string()))?;
    let root = PathBuf::from(root.trim());

    if let Some(patch) = req.patch.as_deref().filter(|p| !p.trim().is_empty()) {
        // The patch must touch exactly this file.
        let header = patch.lines().next().unwrap_or_default();
        if !header.starts_with("diff --git ") || !header.ends_with(&format!(" b/{}", req.file)) {
            return Err((StatusCode::BAD_REQUEST, "patch does not match the file".to_string()));
        }
        let mut body = patch.to_string();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        git_stdin(&root, &["apply", "-R", "--check", "--recount", "-"], &body)
            .await
            .map_err(|e| (StatusCode::CONFLICT, format!("that change no longer matches the file: {e}")))?;
        git_stdin(&root, &["apply", "-R", "--recount", "-"], &body)
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        return Ok(json!({ "ok": true, "file": req.file, "undone": "hunk" }));
    }

    let tracked = git(&root, &["ls-files", "--error-unmatch", "--", &req.file]).await.is_ok();
    if tracked {
        git(&root, &["checkout", "--", &req.file]).await.map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        return Ok(json!({ "ok": true, "file": req.file, "undone": "file" }));
    }
    if !req.untracked {
        return Err((StatusCode::BAD_REQUEST, "file is not tracked; pass untracked: true to delete it".to_string()));
    }
    let listed = git(&root, &["ls-files", "--others", "--exclude-standard", "--", &req.file])
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if listed.lines().next() != Some(req.file.as_str()) {
        return Err((StatusCode::BAD_REQUEST, "not an untracked file".to_string()));
    }
    let full = root.join(&req.file);
    if !full.is_file() {
        return Err((StatusCode::BAD_REQUEST, "only files can be deleted".to_string()));
    }
    std::fs::remove_file(&full).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(json!({ "ok": true, "file": req.file, "undone": "deleted" }))
}

async fn git_discard(Json(req): Json<GitDiscardRequest>) -> axum::response::Response {
    match discard(req).await {
        Ok(body) => Json(body).into_response(),
        Err((status, message)) => err(status, message),
    }
}

async fn git_diff(Json(req): Json<GitDiffRequest>) -> axum::response::Response {
    match diff_for(req).await {
        Ok(body) => Json(body).into_response(),
        Err((status, message)) => err(status, message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discards_one_hunk_a_whole_file_and_untracked_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        sh(d, &["init", "-q"]);
        let original: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(d.join("a.txt"), &original).unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-qm", "init"]);
        let edited = original.replace("line 2\n", "LINE 2\n").replace("line 28\n", "LINE 28\n");
        std::fs::write(d.join("a.txt"), &edited).unwrap();
        let path = Some(d.display().to_string());

        // Undo only the second hunk.
        let out = diff_for(GitDiffRequest { path: path.clone(), ..Default::default() }).await.unwrap();
        let diff = out["files"][0]["diff"].as_str().unwrap().to_string();
        let first_hunk = diff.find("\n@@").unwrap() + 1;
        let second_hunk = diff[first_hunk + 2..].find("\n@@").unwrap() + first_hunk + 3;
        let patch = format!("{}{}", &diff[..first_hunk], &diff[second_hunk..]);
        discard(GitDiscardRequest { path: path.clone(), file: "a.txt".into(), patch: Some(patch), untracked: false }).await.unwrap();
        let now = std::fs::read_to_string(d.join("a.txt")).unwrap();
        assert!(now.contains("LINE 2\n") && now.contains("line 28\n"));

        // A stale patch is refused.
        let stale = format!("{}{}", &diff[..first_hunk], &diff[second_hunk..]);
        let e = discard(GitDiscardRequest { path: path.clone(), file: "a.txt".into(), patch: Some(stale), untracked: false }).await.unwrap_err();
        assert_eq!(e.0, StatusCode::CONFLICT);

        // Whole file back to committed.
        discard(GitDiscardRequest { path: path.clone(), file: "a.txt".into(), ..Default::default() }).await.unwrap();
        assert_eq!(std::fs::read_to_string(d.join("a.txt")).unwrap(), original);

        // Untracked: needs the explicit flag; paths can't escape the repo.
        std::fs::write(d.join("new.md"), "hi\n").unwrap();
        assert!(discard(GitDiscardRequest { path: path.clone(), file: "new.md".into(), ..Default::default() }).await.is_err());
        assert!(discard(GitDiscardRequest { path: path.clone(), file: "../x".into(), untracked: true, ..Default::default() }).await.is_err());
        discard(GitDiscardRequest { path: path.clone(), file: "new.md".into(), untracked: true, ..Default::default() }).await.unwrap();
        assert!(!d.join("new.md").exists());
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn reports_modified_and_untracked_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        sh(d, &["init", "-q"]);
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        sh(d, &["add", "."]);
        sh(d, &["commit", "-qm", "init"]);
        std::fs::write(d.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(d.join("new.md"), "hello\n").unwrap();

        let out = diff_for(GitDiffRequest { path: Some(d.display().to_string()), ..Default::default() }).await.unwrap();
        let files = out["files"].as_array().unwrap();
        let a = files.iter().find(|f| f["path"] == "a.txt").unwrap();
        assert_eq!(a["changeType"], "modified");
        assert_eq!((a["additions"].as_u64(), a["deletions"].as_u64()), (Some(1), Some(1)));
        let n = files.iter().find(|f| f["path"] == "new.md").unwrap();
        assert_eq!(n["changeType"], "added");
        assert!(n["diff"].as_str().unwrap().contains("+hello"));
        assert_eq!(out["totalAdditions"], 2);
    }

    #[tokio::test]
    async fn rejects_non_repos_and_option_like_revisions() {
        let tmp = tempfile::tempdir().unwrap();
        let e = diff_for(GitDiffRequest { path: Some(tmp.path().display().to_string()), ..Default::default() }).await.unwrap_err();
        assert_eq!(e.0, StatusCode::UNPROCESSABLE_ENTITY);
        sh(tmp.path(), &["init", "-q"]);
        let e = diff_for(GitDiffRequest {
            path: Some(tmp.path().display().to_string()),
            from: Some("--output=/tmp/x".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
    }
}
