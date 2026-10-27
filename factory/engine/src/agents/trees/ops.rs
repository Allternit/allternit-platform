//! `new`, `list`, `sync`, `pause` / `resume`, `env`: the tree verbs other
//! than landing (`land`) and reaping (`reaper`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::config::TreesConfig;
use super::lease::{canonical, Lease, LeaseStore};
use super::overlap::{self, Overlap};
use super::procs::{self, TreeProcess};
use super::seed::{self, SeedRequest};
use super::{git, now, tree_id, Owner, SeedInfo, TreeError, TreeResult, TreeState};

/// A tree as the CLI, HTTP and UI see it (lease + live git and process facts).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeView {
    pub id: String,
    pub repo: String,
    pub repo_path: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub base: String,
    pub owner: Owner,
    pub state: TreeState,
    pub leased: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub expires_at: Option<String>,
    pub seed: Option<SeedInfo>,
    pub behind_main: Option<u32>,
    pub ahead_main: Option<u32>,
    pub dirty_files: Option<u32>,
    pub size_bytes: Option<u64>,
    pub size_at: Option<String>,
    pub processes: Vec<TreeProcess>,
    pub checkpoint_count: u32,
    pub last_checkpoint_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub free_bytes: Option<u64>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeList {
    pub trees: Vec<TreeView>,
    pub machine: Machine,
    pub overlaps: Vec<Overlap>,
}

const LEASE_HOURS: i64 = 24;
const ONCE_HOURS: i64 = 1;

fn expiry(hours: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::hours(hours)).to_rfc3339()
}

/// Free bytes on the volume holding `path`.
pub fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: valid C string and an out-pointer to a zeroed struct.
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return None;
        }
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// `du -sk` in bytes (clones count in full: du can't see shared blocks).
pub fn size_bytes(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("du").arg("-sk").arg(path).output().ok()?;
    String::from_utf8_lossy(&out.stdout).split_whitespace().next()?.parse::<u64>().ok().map(|k| k * 1024)
}

/// The default tree path: `<repo parent>/<repo>-<branch slug>`
/// (`session/abc` → `allternit-session-abc`, the session-worktree name).
pub fn default_path(main: &Path, branch: &str) -> PathBuf {
    let id = tree_id(&git::repo_name(main), branch);
    let slug = id.split_once('~').map(|(_, s)| s).unwrap_or(&id);
    main.parent().unwrap_or(Path::new("/")).join(format!("{}-{slug}", git::repo_name(main)))
}

pub struct NewRequest<'a> {
    pub cwd: &'a Path,
    pub branch: &'a str,
    pub owner: Owner,
    pub path: Option<PathBuf>,
    pub once: bool,
    pub source_only: bool,
    pub plain: bool,
    /// Skip the free-disk gate (a person decided).
    pub force_disk: bool,
    /// Use the local origin/main when the fetch fails.
    pub offline: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewOutcome {
    pub tree: TreeView,
    /// The tree already existed (same branch) and was reused.
    pub reused: bool,
    /// Lines to `eval` for the tree's env (data dir, target dir, owner).
    pub env: Vec<String>,
}

fn check_branch_name(branch: &str) -> TreeResult<()> {
    let bad = branch.is_empty()
        || branch == "main"
        || branch.starts_with('-')
        || branch.contains("..")
        || branch.contains(char::is_whitespace);
    if bad {
        return Err(TreeError::usage(
            format!("`{branch}` is not a branch a tree can use"),
            "Name a new work branch, e.g. `session/<id>` or `feat/<topic>` (never main).",
        ));
    }
    Ok(())
}

/// Make (or reuse) a tree: gate → fetch → lease → worktree → seed → env.
pub fn new_tree(store: &LeaseStore, req: NewRequest<'_>) -> TreeResult<NewOutcome> {
    check_branch_name(req.branch)?;
    let main = git::main_checkout(req.cwd).map_err(|e| {
        TreeError::usage(format!("{e:#}"), "Run it from inside the repo the tree is for (or pass --root).")
    })?;
    let repo = git::repo_name(&main);
    let cfg = TreesConfig::load(&main)?;
    let id = tree_id(&repo, req.branch);

    // Reuse before creating: a lease for this branch, or a worktree on it.
    if let Some(lease) = store.get(&id)?.filter(|l| l.path.exists()) {
        let tree = view(&lease, &cfg, false);
        return Ok(NewOutcome { env: env_lines(&lease), tree, reused: true });
    }
    if let Some(w) = git::worktrees(&main)?.into_iter().find(|w| w.branch.as_deref() == Some(req.branch)) {
        if canonical(&w.path) != canonical(&main) {
            let lease = adopt(store, &main, &w.path, req.owner.clone())?;
            let tree = view(&lease, &cfg, false);
            return Ok(NewOutcome { env: env_lines(&lease), tree, reused: true });
        }
        return Err(TreeError::refused(
            format!("`{}` is checked out in the shared checkout {}", req.branch, main.display()),
            "Use another branch name; the shared checkout stays on main.",
        ));
    }

    if !req.force_disk {
        if let Some(free) = free_bytes(&main) {
            let min = cfg.reaper.min_free_gb * 1024 * 1024 * 1024;
            if free < min {
                return Err(TreeError::refused(
                    format!("only {} GB free; a new tree needs at least {} GB", free / (1024 * 1024 * 1024), cfg.reaper.min_free_gb),
                    "Free space first: `gizzi agents tree reap` (dry run), then `--apply`. A person can pass --force-disk.",
                ));
            }
        }
    }
    if let Err(e) = git::fetch_main(&main) {
        if !req.offline {
            return Err(TreeError::new(
                "transport",
                format!("could not fetch origin/main: {e:#}"),
                "A tree starts from fresh origin/main. Fix the network, or pass --offline to use the last fetched main.",
            ));
        }
    }
    let origin_main = git::rev(&main, "origin/main").map_err(|e| {
        TreeError::not_found(format!("no origin/main in {}: {e:#}", main.display()), "Add the origin remote and fetch main.")
    })?;
    let local = git::rev(&main, &format!("refs/heads/{}", req.branch)).is_ok();
    let remote = git::rev(&main, &format!("refs/remotes/origin/{}", req.branch)).is_ok();
    let path = req.path.clone().unwrap_or_else(|| default_path(&main, req.branch));
    if path.exists() && std::fs::read_dir(&path).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(TreeError::refused(
            format!("{} already exists and is not a tree of `{}`", path.display(), req.branch),
            "Pass --path to put the tree somewhere else, or remove that folder if it is yours.",
        ));
    }
    let seed = seed::create(&SeedRequest {
        main: &main,
        path: &path,
        branch: req.branch,
        branch_exists: local || remote,
        origin_main: &origin_main,
        source_only: req.source_only || req.once,
        plain: req.plain || req.once,
        cfg: &cfg,
    })?;
    let t = now();
    let lease = Lease {
        id: id.clone(),
        repo,
        repo_path: main.clone(),
        path: path.clone(),
        branch: Some(req.branch.to_string()),
        base: origin_main,
        owner: req.owner,
        state: TreeState::Active,
        created_at: t.clone(),
        updated_at: t,
        expires_at: Some(expiry(if req.once { ONCE_HOURS } else { LEASE_HOURS })),
        seed: Some(seed),
        once: req.once,
        size_bytes: None,
        size_at: None,
        checkpoints: 0,
        last_checkpoint_at: None,
        last_checkpoint_tree: None,
    };
    write_env(&lease)?;
    store.put(lease.clone())?;
    Ok(NewOutcome { env: env_lines(&lease), tree: view(&lease, &cfg, false), reused: false })
}

/// Lease an existing worktree (made by hand or before trees existed).
pub fn adopt(store: &LeaseStore, main: &Path, path: &Path, owner: Owner) -> TreeResult<Lease> {
    let path = canonical(path);
    if let Some(l) = store.find_by_path(&path)?.filter(|l| canonical(&l.path) == path) {
        return Ok(l);
    }
    let branch = git::current_branch(&path);
    let dir_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let id = tree_id(&git::repo_name(main), branch.as_deref().unwrap_or(&dir_name));
    let t = now();
    let lease = Lease {
        id,
        repo: git::repo_name(main),
        repo_path: main.to_path_buf(),
        path: path.clone(),
        base: git::run(&path, &["merge-base", "origin/main", "HEAD"]).unwrap_or_default(),
        branch,
        owner,
        state: TreeState::Active,
        created_at: t.clone(),
        updated_at: t,
        expires_at: Some(expiry(LEASE_HOURS)),
        seed: None,
        once: false,
        size_bytes: None,
        size_at: None,
        checkpoints: 0,
        last_checkpoint_at: None,
        last_checkpoint_tree: None,
    };
    write_env(&lease)?;
    store.put(lease.clone())?;
    Ok(lease)
}

/// The tree's own env: its data dir and Rust target, never the installed
/// Desktop's sqlite or port 8013 (AGENTS.md "local data-dir ownership").
pub fn env_lines(lease: &Lease) -> Vec<String> {
    let data = lease.path.join(".allternit-tree/data");
    vec![
        format!("export ALLTERNIT_TREE={}", shell_quote(&lease.id)),
        format!("export ALLTERNIT_TREE_OWNER={}", shell_quote(&lease.owner.spec())),
        format!("export ALLTERNIT_DATA_DIR={}", shell_quote(&data.to_string_lossy())),
        format!("export ALLTERNIT_USER_DATA_DIR={}", shell_quote(&data.to_string_lossy())),
        format!("export CARGO_TARGET_DIR={}", shell_quote(&lease.path.join("target").to_string_lossy())),
        "unset ALLTERNIT_API_PORT".to_string(),
    ]
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `<tree>/.allternit-tree/env`, ignored through the repo's info/exclude.
fn write_env(lease: &Lease) -> anyhow::Result<()> {
    let dir = lease.path.join(".allternit-tree");
    std::fs::create_dir_all(dir.join("data"))?;
    std::fs::write(dir.join("env"), env_lines(lease).join("\n") + "\n")?;
    let common = git::run(&lease.path, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let exclude = PathBuf::from(common).join("info/exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == ".allternit-tree/") {
        std::fs::create_dir_all(exclude.parent().unwrap_or(Path::new(".")))?;
        let sep = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
        std::fs::write(&exclude, format!("{current}{sep}# Agent Trees env and scratch data\n.allternit-tree/\n"))?;
    }
    Ok(())
}

/// Live view of a lease.
pub fn view(lease: &Lease, cfg: &TreesConfig, with_size: bool) -> TreeView {
    let present = lease.path.exists();
    let (behind, ahead) = present.then(|| git::behind_ahead(&lease.path, "origin/main")).flatten().unzip();
    let size = if with_size && present { size_bytes(&lease.path) } else { lease.size_bytes };
    TreeView {
        id: lease.id.clone(),
        repo: lease.repo.clone(),
        repo_path: lease.repo_path.clone(),
        path: lease.path.clone(),
        branch: lease.branch.clone(),
        base: lease.base.clone(),
        owner: lease.owner.clone(),
        state: lease.state,
        leased: true,
        created_at: Some(lease.created_at.clone()),
        updated_at: Some(lease.updated_at.clone()),
        expires_at: lease.expires_at.clone(),
        seed: lease.seed.clone(),
        behind_main: behind,
        ahead_main: ahead,
        dirty_files: present.then(|| git::dirty_files(&lease.path).ok().map(|d| d.len() as u32)).flatten(),
        size_at: if with_size { Some(now()) } else { lease.size_at.clone() },
        size_bytes: size,
        processes: if present { procs::in_tree(&lease.path, |c| cfg.is_dev_server(c)) } else { Vec::new() },
        checkpoint_count: lease.checkpoints,
        last_checkpoint_at: lease.last_checkpoint_at.clone(),
    }
}

/// An un-leased worktree, shown so people see everything on disk.
fn unleased_view(main: &Path, w: &git::Worktree, cfg: &TreesConfig, with_size: bool) -> TreeView {
    let dir_name = w.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let present = w.path.exists();
    let (behind, ahead) = present.then(|| git::behind_ahead(&w.path, "origin/main")).flatten().unzip();
    TreeView {
        id: tree_id(&git::repo_name(main), w.branch.as_deref().unwrap_or(&dir_name)),
        repo: git::repo_name(main),
        repo_path: main.to_path_buf(),
        path: w.path.clone(),
        branch: w.branch.clone(),
        base: String::new(),
        owner: Owner { kind: super::OwnerKind::Human, id: "unknown".into(), label: "no lease".into() },
        state: TreeState::Active,
        leased: false,
        created_at: None,
        updated_at: None,
        expires_at: None,
        seed: None,
        behind_main: behind,
        ahead_main: ahead,
        dirty_files: present.then(|| git::dirty_files(&w.path).ok().map(|d| d.len() as u32)).flatten(),
        size_bytes: if with_size && present { size_bytes(&w.path) } else { None },
        size_at: with_size.then(now),
        processes: if present { procs::in_tree(&w.path, |c| cfg.is_dev_server(c)) } else { Vec::new() },
        checkpoint_count: 0,
        last_checkpoint_at: None,
    }
}

pub struct ListRequest<'a> {
    /// Repos to include besides every leased one (e.g. the caller's).
    pub repos: Vec<PathBuf>,
    /// Only the tree containing this path.
    pub path: Option<&'a Path>,
    /// Include worktrees that have no lease.
    pub all: bool,
    pub with_size: bool,
}

pub fn list(store: &LeaseStore, req: ListRequest<'_>) -> TreeResult<TreeList> {
    let leases = store.list()?;
    let mut repos: BTreeSet<PathBuf> = leases.iter().map(|l| canonical(&l.repo_path)).collect();
    repos.extend(req.repos.iter().filter_map(|r| git::main_checkout(r).ok()).map(|p| canonical(&p)));
    if let Some(p) = req.path {
        if let Ok(m) = git::main_checkout(p) {
            repos.insert(canonical(&m));
        }
    }
    let mut trees: Vec<TreeView> = Vec::new();
    let mut leased_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut free = None;
    let mut warnings = Vec::new();
    let mut dev_hours: std::collections::BTreeMap<PathBuf, u64> = Default::default();
    for main in &repos {
        let cfg = TreesConfig::load(main).unwrap_or_default();
        if free.is_none() {
            free = free_bytes(main);
            if let Some(f) = free {
                if f < cfg.reaper.min_free_gb * 1024 * 1024 * 1024 {
                    warnings.push(format!(
                        "{} GB free, under the {} GB floor: run `gizzi agents tree reap`",
                        f / (1024 * 1024 * 1024),
                        cfg.reaper.min_free_gb
                    ));
                }
            }
        }
        for l in leases.iter().filter(|l| canonical(&l.repo_path) == *main) {
            leased_paths.insert(canonical(&l.path));
            let v = view(l, &cfg, req.with_size);
            // A measured size is kept on the lease, so cheap lists (HTTP, the card) show it.
            if req.with_size {
                if let Some(bytes) = v.size_bytes {
                    store.modify(&l.id, |l| {
                        l.size_bytes = Some(bytes);
                        l.size_at = Some(now());
                    })?;
                }
            }
            trees.push(v);
        }
        if req.all || req.path.is_some() {
            let base = canonical(&seed::base_path(main));
            for w in git::worktrees(main).unwrap_or_default() {
                let p = canonical(&w.path);
                if p == *main || p == base || w.prunable || leased_paths.contains(&p) {
                    continue;
                }
                trees.push(unleased_view(main, &w, &cfg, req.with_size));
            }
        }
        dev_hours.insert(main.clone(), cfg.reaper.dev_server_hours);
    }
    for t in &trees {
        let hours = dev_hours.get(&canonical(&t.repo_path)).copied().unwrap_or(24);
        for p in t.processes.iter().filter(|p| p.dev_server && p.age_secs.unwrap_or(0) > hours * 3600) {
            warnings.push(format!(
                "dev server {} has run {} h in {}: stop it (`gizzi agents tree done`, or the reaper does it)",
                p.pid,
                p.age_secs.unwrap_or(0) / 3600,
                t.path.display()
            ));
        }
    }
    warnings.extend(fseventsd_warning());
    let changed: Vec<(String, String, BTreeSet<String>)> = trees
        .iter()
        .filter(|t| t.state != TreeState::Done && t.path.exists())
        .map(|t| (t.id.clone(), t.repo.clone(), overlap::changed_files(&t.path)))
        .collect();
    let overlaps = overlap::find(&changed);
    if let Some(p) = req.path {
        let p = canonical(p);
        let hit = trees
            .iter()
            .filter(|t| p.starts_with(canonical(&t.path)))
            .max_by_key(|t| t.path.as_os_str().len())
            .cloned();
        let ids: Vec<String> = hit.iter().map(|t| t.id.clone()).collect();
        let overlaps = overlaps.into_iter().filter(|o| ids.contains(&o.a) || ids.contains(&o.b)).collect();
        return Ok(TreeList { trees: hit.into_iter().collect(), machine: Machine { free_bytes: free, warnings }, overlaps });
    }
    trees.sort_by(|a, b| a.repo.cmp(&b.repo).then(a.path.cmp(&b.path)));
    Ok(TreeList { trees, machine: Machine { free_bytes: free, warnings }, overlaps })
}

/// fseventsd over 8 GB (the 2026-10-25 slowdown), macOS only.
fn fseventsd_warning() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("ps").args(["-axo", "rss=,comm="]).output().ok()?;
    let kb = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.trim_end().ends_with("fseventsd"))
        .filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok())
        .max()?;
    (kb > 8 * 1024 * 1024).then(|| {
        format!(
            "fseventsd holds {} GB: stop old dev servers; a person can run `sudo killall fseventsd`",
            kb / (1024 * 1024)
        )
    })
}

/// Find a lease by id, or the one containing `path`.
pub fn resolve(store: &LeaseStore, id_or_path: Option<&str>, cwd: &Path) -> TreeResult<Lease> {
    let found = match id_or_path {
        Some(s) if s.contains('~') => store.get(s)?,
        Some(s) => store.find_by_path(&cwd.join(s))?,
        None => store.find_by_path(cwd)?,
    };
    found.ok_or_else(|| {
        TreeError::not_found(
            format!("no tree for {}", id_or_path.unwrap_or(&cwd.to_string_lossy())),
            "List trees with `gizzi agents tree list --all`; a worktree made by hand is leased by `gizzi agents tree new <its branch>`.",
        )
    })
}

pub fn ensure_owner(lease: &Lease, caller: &Owner, as_owner: bool) -> TreeResult<()> {
    if as_owner || lease.owner.id == caller.id {
        return Ok(());
    }
    Err(TreeError::refused(
        format!("{} belongs to {} ({}); you are {}", lease.id, lease.owner.label, lease.owner.spec(), caller.spec()),
        "Worktree ownership is absolute: message its owner. A person can pass --as-owner.",
    ))
}

/// Forward-only update to origin/main: a fast-forward of a clean tree whose
/// HEAD is already in main's past. Anything else is refused, never forced.
pub fn sync(store: &LeaseStore, lease: &Lease) -> TreeResult<TreeView> {
    git::fetch_main(&lease.path).ok();
    let dirty = git::dirty_files(&lease.path)?;
    if !dirty.is_empty() {
        return Err(TreeError::refused(
            format!("{} has {} uncommitted file(s)", lease.id, dirty.len()),
            "Commit (or let its owner commit) first; sync never moves uncommitted work.",
        ));
    }
    if !git::is_ancestor(&lease.path, "HEAD", "origin/main") {
        return Err(TreeError::refused(
            format!("{} has commits origin/main doesn't, so it can't fast-forward", lease.id),
            "Its owner merges or rebases on origin/main; sync only moves forward.",
        ));
    }
    git::run(&lease.path, &["merge", "--quiet", "--ff-only", "origin/main"])?;
    store.modify(&lease.id, |l| l.expires_at = Some(expiry(LEASE_HOURS)))?;
    let lease = store.get(&lease.id)?.unwrap_or_else(|| lease.clone());
    Ok(view(&lease, &TreesConfig::load(&lease.repo_path).unwrap_or_default(), false))
}

/// Pause: keep the tree for a later resume (the reaper never removes a paused
/// tree); push its branch and checkpoints so another machine can resume.
pub fn set_state(store: &LeaseStore, lease: &Lease, state: TreeState) -> TreeResult<TreeView> {
    if state == TreeState::Paused {
        if let Some(b) = &lease.branch {
            git::run(&lease.path, &["push", "--quiet", "-u", "origin", b]).ok();
        }
        super::checkpoint::push(lease).ok();
    }
    store.modify(&lease.id, |l| {
        l.state = state;
        l.expires_at = (state == TreeState::Active).then(|| expiry(LEASE_HOURS));
    })?;
    let lease = store.get(&lease.id)?.unwrap_or_else(|| lease.clone());
    Ok(view(&lease, &TreesConfig::load(&lease.repo_path).unwrap_or_default(), false))
}

/// Push the lease's expiry out (any checkpoint or command in the tree).
pub fn touch(store: &LeaseStore, id: &str) -> TreeResult<()> {
    store.modify(id, |l| {
        if l.state == TreeState::Active {
            l.expires_at = Some(expiry(if l.once { ONCE_HOURS } else { LEASE_HOURS }));
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_matches_session_worktree_names() {
        let p = default_path(Path::new("/w/allternit"), "session/abc123");
        assert_eq!(p, PathBuf::from("/w/allternit-session-abc123"));
    }

    #[test]
    fn refuses_main_and_odd_branch_names() {
        assert!(check_branch_name("main").is_err());
        assert!(check_branch_name("a b").is_err());
        assert!(check_branch_name("feat/ok").is_ok());
    }
}
