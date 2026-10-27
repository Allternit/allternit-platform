//! Seeding a new tree from the repo's warm base.
//!
//! The base is a clean detached worktree on origin/main at
//! `<repo parent>/.agent-trees-base/<repo>` that keeps its build output. A new
//! tree is made with `worktree add --no-checkout` at the base's commit, then
//! the base's files are cloned in with their mtimes (APFS `clonefile` through
//! `cp -c`, Linux `cp --reflink=always`), the index is filled from HEAD, and
//! the tree fast-forwards to origin/main, which rewrites only changed files.
//! Cargo then sees every unchanged crate as fresh (measured 2026-10-27: 16 s
//! with 0 crates rebuilt, against 101 s cold), and a clone costs only the
//! blocks that later change. `node_modules` is not cloned: pnpm already
//! hardlinks it from its store, and an offline install (≈43 s) is faster than
//! cloning its many small files (≈56 s) and has no stale absolute bin paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{bail, Context, Result};

use super::config::{Step, TreesConfig};
use super::git;
use super::{SeedInfo, SeedStrategy};

pub const BASE_DIR: &str = ".agent-trees-base";

/// Names (any depth) or relative paths never cloned from the base.
const ALWAYS_EXCLUDE: [&str; 2] = [".git", "node_modules"];

pub fn base_path(main: &Path) -> PathBuf {
    main.parent().unwrap_or(Path::new("/")).join(BASE_DIR).join(git::repo_name(main))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneKind {
    Apfs,
    Reflink,
}

/// Can this machine clone from `from` into `to`'s directory? Probed with a
/// real one-file clone so an ext4 host (no reflink) falls back to a plain
/// checkout instead of a full copy.
pub fn clone_kind(from: &Path, to_dir: &Path) -> Option<CloneKind> {
    let probe_src = from.join(".git");
    if !probe_src.exists() {
        return None;
    }
    let probe_dst = to_dir.join(format!(".agent-trees-probe-{}", std::process::id()));
    let kind = if cfg!(target_os = "macos") {
        Some(CloneKind::Apfs)
    } else if cfg!(target_os = "linux") {
        Some(CloneKind::Reflink)
    } else {
        None
    }?;
    let ok = clone_cmd(kind, &probe_src, &probe_dst).status().map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&probe_dst);
    ok.then_some(kind)
}

fn clone_cmd(kind: CloneKind, src: &Path, dst: &Path) -> Command {
    let mut c = Command::new("cp");
    match kind {
        CloneKind::Apfs => c.args(["-c", "-p", "-R"]),
        CloneKind::Reflink => c.args(["--reflink=always", "-a"]),
    };
    c.arg(src).arg(dst);
    c
}

/// Excluded names (match at any depth) and relative paths.
fn exclusions(cfg: &TreesConfig) -> (BTreeSet<String>, BTreeSet<PathBuf>) {
    let mut names: BTreeSet<String> = ALWAYS_EXCLUDE.iter().map(|s| s.to_string()).collect();
    let mut paths = BTreeSet::new();
    for e in &cfg.seed.exclude {
        if e.contains('/') {
            paths.insert(PathBuf::from(e.trim_end_matches('/')));
        } else {
            names.insert(e.clone());
        }
    }
    (names, paths)
}

/// Every excluded path under `base` (relative). Does not descend into
/// excluded dirs or build output, which can hold millions of files.
fn excluded_under(base: &Path, names: &BTreeSet<String>, paths: &BTreeSet<PathBuf>, build_dirs: &[String]) -> BTreeSet<PathBuf> {
    let mut found: BTreeSet<PathBuf> = paths.iter().filter(|p| base.join(p).exists()).cloned().collect();
    let walker = walkdir::WalkDir::new(base).min_depth(1).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && build_dirs.iter().any(|b| b == name.as_ref()) && !names.contains(name.as_ref()))
    });
    let mut it = walker;
    while let Some(Ok(e)) = it.next() {
        let name = e.file_name().to_string_lossy().into_owned();
        if names.contains(&name) {
            if let Ok(rel) = e.path().strip_prefix(base) {
                found.insert(rel.to_path_buf());
            }
            if e.file_type().is_dir() {
                it.skip_current_dir();
            }
        }
    }
    found
}

/// Clone `src` into `dst` (which must not exist), skipping `excluded`
/// (relative to `root`). Subtrees with nothing excluded go in one `cp`.
fn clone_tree(kind: CloneKind, root: &Path, src: &Path, dst: &Path, excluded: &BTreeSet<PathBuf>) -> Result<()> {
    let rel = src.strip_prefix(root).unwrap_or(Path::new(""));
    // Paths sort by component, so anything under `rel` follows it directly.
    let has_excluded_inside =
        excluded.range(rel.to_path_buf()..).find(|p| p.as_path() != rel).is_some_and(|p| p.starts_with(rel));
    // A symlink (even to a dir) is cloned as the link itself, never followed.
    let is_dir = std::fs::symlink_metadata(src).map(|m| m.is_dir()).unwrap_or(false);
    if !is_dir || !has_excluded_inside {
        let status = clone_cmd(kind, src, dst).status().with_context(|| format!("cloning {}", src.display()))?;
        if !status.success() {
            bail!("cp could not clone {}", src.display());
        }
        return Ok(());
    }
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let child = entry.path();
        let child_rel = child.strip_prefix(root).unwrap_or(&child).to_path_buf();
        if excluded.contains(&child_rel) {
            continue;
        }
        clone_tree(kind, root, &child, &dst.join(entry.file_name()), excluded)?;
    }
    Ok(())
}

/// Run the repo's dependency steps in `dir` (`when` = a file that must exist).
/// Child output goes to stderr: stdout carries the engine's `--json` document.
pub fn run_steps(dir: &Path, steps: &[Step]) -> Result<Vec<String>> {
    let mut ran = Vec::new();
    for step in steps {
        if let Some(when) = &step.when {
            if !dir.join(when).exists() {
                continue;
            }
        }
        let status = Command::new("sh")
            .arg("-c")
            .arg(&step.run)
            .current_dir(dir)
            .stdout(std::io::stderr())
            .status()
            .with_context(|| format!("running `{}`", step.run))?;
        if !status.success() {
            bail!("`{}` failed in {}", step.run, dir.display());
        }
        ran.push(step.run.clone());
    }
    Ok(ran)
}

/// The base, if it exists and is a clean checkout we can clone from.
pub fn usable_base(main: &Path) -> Option<(PathBuf, String)> {
    let base = base_path(main);
    if !base.join(".git").exists() {
        return None;
    }
    let head = git::rev(&base, "HEAD").ok()?;
    git::dirty_files(&base).ok().filter(|d| d.is_empty())?;
    Some((base, head))
}

pub struct SeedRequest<'a> {
    pub main: &'a Path,
    pub path: &'a Path,
    pub branch: &'a str,
    /// Existing branch: plain checkout of it. New: from origin/main.
    pub branch_exists: bool,
    pub origin_main: &'a str,
    /// Skip dependency steps (workers that only write code).
    pub source_only: bool,
    /// Never clone, even with a base.
    pub plain: bool,
    pub cfg: &'a TreesConfig,
}

/// Make the worktree and fill it. Returns how it was seeded.
pub fn create(req: &SeedRequest<'_>) -> Result<SeedInfo> {
    let start = Instant::now();
    let parent = req.path.parent().unwrap_or(Path::new("/"));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let base = (!req.plain && !req.branch_exists).then(|| usable_base(req.main)).flatten();
    let kind = base.as_ref().and_then(|(b, _)| clone_kind(b, parent));
    let strategy = match (base, kind) {
        (Some((base, head)), Some(kind)) => {
            let path = req.path.to_string_lossy().into_owned();
            git::run(req.main, &["worktree", "add", "--no-checkout", "-b", req.branch, &path, &head])?;
            let (names, paths) = exclusions(req.cfg);
            let excluded = excluded_under(&base, &names, &paths, &req.cfg.clean.build_dirs);
            for entry in std::fs::read_dir(&base)? {
                let entry = entry?;
                let rel = PathBuf::from(entry.file_name());
                if excluded.contains(&rel) {
                    continue;
                }
                clone_tree(kind, &base, &entry.path(), &req.path.join(entry.file_name()), &excluded)?;
            }
            git::run(req.path, &["reset", "--quiet"])?;
            if head != req.origin_main {
                git::run(req.path, &["merge", "--quiet", "--ff-only", req.origin_main])?;
            }
            SeedStrategy::Clone
        }
        _ => {
            let path = req.path.to_string_lossy().into_owned();
            if req.branch_exists {
                git::run(req.main, &["worktree", "add", &path, req.branch])?;
            } else {
                git::run(req.main, &["worktree", "add", "-b", req.branch, &path, req.origin_main])?;
            }
            SeedStrategy::Plain
        }
    };
    if !req.source_only {
        run_steps(req.path, &req.cfg.seed.deps)?;
    }
    Ok(SeedInfo { strategy, seconds: (start.elapsed().as_secs_f64() * 10.0).round() / 10.0 })
}

/// Make the base if it is missing, else fast-forward it to origin/main (git
/// rewrites only changed files, so build output stays fresh), then run the
/// dependency steps and, with `build`, the repo's base build commands
/// (through `scripts/build-queue` when the repo has one).
pub fn refresh_base(main: &Path, cfg: &TreesConfig, build: bool) -> Result<String> {
    let base = base_path(main);
    git::fetch_main(main).ok();
    let target = git::rev(main, "origin/main")?;
    if !base.join(".git").exists() {
        std::fs::create_dir_all(base.parent().unwrap_or(Path::new("/")))?;
        git::run(main, &["worktree", "add", "--detach", &base.to_string_lossy(), &target])?;
    } else {
        let dirty = git::dirty_files(&base)?;
        if !dirty.is_empty() {
            bail!(
                "the base {} has {} changed file(s) ({}); trees seed from it only when it is clean",
                base.display(),
                dirty.len(),
                dirty.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            );
        }
        git::run(&base, &["checkout", "--quiet", "--detach", &target])?;
    }
    run_steps(&base, &cfg.seed.deps)?;
    if build {
        let queue = base.join("scripts/build-queue");
        for cmd in &cfg.base.build {
            let status = if queue.exists() {
                Command::new(&queue).args(["run", "trees-base", "--", "sh", "-c", cmd]).current_dir(&base).stdout(std::io::stderr()).status()
            } else {
                Command::new("sh").arg("-c").arg(cmd).current_dir(&base).stdout(std::io::stderr()).status()
            }
            .with_context(|| format!("running `{cmd}` in the base"))?;
            if !status.success() {
                bail!("base build `{cmd}` failed in {}", base.display());
            }
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions_skip_nested_node_modules_but_not_build_output() {
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path();
        for p in ["src", "pkg/a/node_modules/x", "node_modules/y", "target/debug", ".steering/state", "pkg/a/src"] {
            std::fs::create_dir_all(b.join(p)).unwrap();
        }
        let mut cfg = TreesConfig::default();
        cfg.seed.exclude = vec![".steering/state".into()];
        let (names, paths) = exclusions(&cfg);
        let ex = excluded_under(b, &names, &paths, &cfg.clean.build_dirs);
        assert!(ex.contains(Path::new("node_modules")));
        assert!(ex.contains(Path::new("pkg/a/node_modules")));
        assert!(ex.contains(Path::new(".steering/state")));
        assert!(!ex.iter().any(|p| p.starts_with("target")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clone_tree_keeps_mtimes_and_skips_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("a/node_modules")).unwrap();
        std::fs::write(src.join("a/file.rs"), "x").unwrap();
        std::fs::write(src.join("a/node_modules/m.js"), "y").unwrap();
        let old = std::fs::metadata(src.join("a/file.rs")).unwrap().modified().unwrap();
        let excluded: BTreeSet<PathBuf> = [PathBuf::from("a/node_modules")].into();
        let dst = dir.path().join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        clone_tree(CloneKind::Apfs, &src, &src.join("a"), &dst.join("a"), &excluded).unwrap();
        assert!(dst.join("a/file.rs").exists());
        assert!(!dst.join("a/node_modules").exists());
        assert_eq!(std::fs::metadata(dst.join("a/file.rs")).unwrap().modified().unwrap(), old);
    }
}
