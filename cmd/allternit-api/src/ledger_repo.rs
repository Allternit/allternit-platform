//! The git repository behind a person's ledger: a bare repo beside their
//! Memory Drive (`<drive dir>/ledger.git`). Each snapshot replaces the whole
//! tree in one commit, and only when something changed, so history shows
//! exactly what changed and when.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

const BRANCH: &str = "refs/heads/main";
/// Largest file or tree listing read back through git.
const MAX_OUTPUT: usize = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LedgerRepoError {
    #[error("git {0} failed: {1}")]
    Git(String, String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid path {0}")]
    Path(String),
}

pub type Result<T> = std::result::Result<T, LedgerRepoError>;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FileInfo {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Commit {
    pub revision: String,
    pub timestamp: String,
    pub message: String,
}

pub struct LedgerRepo {
    repo: PathBuf,
    branch: String,
}

/// Paths are relative, `/`-separated, without `..`, empty or hidden-git parts.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 400
        && !path.starts_with('/')
        && path.split('/').all(|p| !p.is_empty() && p != "." && p != ".." && p != ".git")
        && !path.chars().any(|c| c.is_control() || c == '\\')
}

impl LedgerRepo {
    /// Open (creating if needed) the ledger repo at `repo`.
    pub fn open(repo: PathBuf) -> Result<Self> {
        let r = LedgerRepo { repo, branch: BRANCH.to_string() };
        if !r.repo.join("HEAD").exists() {
            std::fs::create_dir_all(&r.repo)?;
            r.git(&["init", "--bare", "--quiet"], None, None)?;
            r.git(&["symbolic-ref", "HEAD", BRANCH], None, None)?;
        }
        Ok(r)
    }

    /// An existing bare repo (e.g. a Memory Drive) read on `branch`.
    pub fn existing(repo: PathBuf, branch: &str) -> Self {
        LedgerRepo { repo, branch: format!("refs/heads/{branch}") }
    }

    pub fn path(&self) -> &Path {
        &self.repo
    }

    fn git(&self, args: &[&str], input: Option<&[u8]>, index: Option<&Path>) -> Result<Vec<u8>> {
        self.git_env(args, input, index, &[])
    }

    fn git_env(&self, args: &[&str], input: Option<&[u8]>, index: Option<&Path>, env: &[(String, String)]) -> Result<Vec<u8>> {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let mut cmd = Command::new("git");
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_AUTHOR_NAME", "Allternit")
            .env("GIT_COMMITTER_NAME", "Allternit")
            .env("GIT_AUTHOR_EMAIL", "ledger@allternit.invalid")
            .env("GIT_COMMITTER_EMAIL", "ledger@allternit.invalid")
            .arg("-c")
            .arg(format!("core.hooksPath={null}"))
            .arg("--git-dir")
            .arg(&self.repo)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        for name in ["SystemRoot", "TEMP", "TMP"] {
            if let Some(v) = std::env::var_os(name) {
                cmd.env(name, v);
            }
        }
        if let Some(index) = index {
            cmd.env("GIT_INDEX_FILE", index);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let out_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            (&mut stdout).take(MAX_OUTPUT as u64).read_to_end(&mut buf).map(|_| buf)
        });
        let err_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            (&mut stderr).take(64 * 1024).read_to_end(&mut buf).map(|_| buf)
        });
        if let (Some(mut stdin), Some(bytes)) = (child.stdin.take(), input) {
            stdin.write_all(bytes)?;
        }
        let status = child.wait()?;
        let out = out_reader.join().map_err(|_| LedgerRepoError::Git(args[0].into(), "reader".into()))??;
        let err = err_reader.join().map_err(|_| LedgerRepoError::Git(args[0].into(), "reader".into()))??;
        if !status.success() {
            let op = args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).unwrap_or(&args[0]);
            return Err(LedgerRepoError::Git((*op).into(), String::from_utf8_lossy(&err).trim().chars().take(300).collect()));
        }
        Ok(out)
    }

    fn text(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git(args, None, None)?).trim().to_string())
    }

    pub fn head(&self) -> Option<String> {
        self.text(&["rev-parse", "--verify", "--quiet", &self.branch]).ok().filter(|s| !s.is_empty())
    }

    /// Replace the whole tree with `files` in one commit. Returns the new
    /// revision, or `None` when nothing changed.
    pub fn write_snapshot(&self, files: &BTreeMap<String, Vec<u8>>, message: &str) -> Result<Option<String>> {
        if let Some(bad) = files.keys().find(|p| !valid_path(p)) {
            return Err(LedgerRepoError::Path(bad.clone()));
        }
        // A throwaway index inside the repo; git creates it on first use.
        let index_path = self.repo.join(format!("ledger-index-{}", uuid::Uuid::new_v4().simple()));
        let mut entries = String::new();
        for (path, bytes) in files {
            let oid = self.text_input(&["hash-object", "-w", "--stdin"], bytes)?;
            entries.push_str(&format!("100644 {oid}\t{path}\0"));
        }
        let tree = self
            .git(&["update-index", "--add", "-z", "--index-info"], Some(entries.as_bytes()), Some(&index_path))
            .and_then(|_| self.git(&["write-tree"], None, Some(&index_path)));
        let _ = std::fs::remove_file(&index_path);
        let tree = String::from_utf8_lossy(&tree?).trim().to_string();
        let parent = self.head();
        if let Some(p) = &parent {
            if self.text(&["rev-parse", &format!("{p}^{{tree}}")])? == tree {
                return Ok(None);
            }
        }
        let mut args = vec!["commit-tree", tree.as_str(), "-m", message];
        if let Some(p) = &parent {
            args.push("-p");
            args.push(p);
        }
        let commit = self.text(&args)?;
        let mut update = vec!["update-ref", self.branch.as_str(), commit.as_str()];
        if let Some(p) = &parent {
            update.push(p);
        }
        self.git(&update, None, None)?;
        Ok(Some(commit))
    }

    fn text_input(&self, args: &[&str], input: &[u8]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git(args, Some(input), None)?).trim().to_string())
    }

    pub fn files(&self, revision: Option<&str>) -> Result<Vec<FileInfo>> {
        let Some(rev) = revision.map(str::to_string).or_else(|| self.head()) else { return Ok(vec![]) };
        let out = self.git(&["ls-tree", "-r", "-l", "-z", &rev], None, None)?;
        Ok(out
            .split(|b| *b == 0)
            .filter_map(|line| {
                let line = String::from_utf8_lossy(line);
                let (meta, path) = line.split_once('\t')?;
                let bytes = meta.split_whitespace().nth(3)?.parse().ok()?;
                Some(FileInfo { path: path.to_string(), bytes })
            })
            .collect())
    }

    pub fn read(&self, path: &str, revision: Option<&str>) -> Result<Option<Vec<u8>>> {
        if !valid_path(path) {
            return Err(LedgerRepoError::Path(path.into()));
        }
        let Some(rev) = revision.map(str::to_string).or_else(|| self.head()) else { return Ok(None) };
        match self.git(&["cat-file", "blob", &format!("{rev}:{path}")], None, None) {
            Ok(b) => Ok(Some(b)),
            Err(LedgerRepoError::Git(..)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn history(&self, limit: usize) -> Result<Vec<Commit>> {
        if self.head().is_none() {
            return Ok(vec![]);
        }
        let out = self.text(&["log", &format!("-n{}", limit.clamp(1, 200)), "--format=%H%x1f%cI%x1f%s", &self.branch])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut p = l.split('\u{1f}');
                Some(Commit { revision: p.next()?.into(), timestamp: p.next()?.into(), message: p.next().unwrap_or("").into() })
            })
            .collect())
    }

    /// The paths that changed in one commit (against its parent).
    pub fn changed(&self, revision: &str) -> Result<Vec<String>> {
        let out = self.text(&["diff-tree", "--root", "--no-commit-id", "-r", "--name-only", revision])?;
        Ok(out.lines().map(str::to_string).collect())
    }

    /// Every file at the latest revision.
    pub fn all_files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut out = BTreeMap::new();
        for f in self.files(None)? {
            if let Some(bytes) = self.read(&f.path, None)? {
                out.insert(f.path, bytes);
            }
        }
        Ok(out)
    }

    /// Push the branch to `url` as `remote_branch`, replacing what's there
    /// (history can be rewritten by "delete forever"). `env` carries auth.
    pub fn push(&self, url: &str, remote_branch: &str, env: &[(String, String)]) -> Result<()> {
        if self.head().is_none() {
            return Ok(());
        }
        let refspec = format!("+{}:refs/heads/{remote_branch}", self.branch);
        self.git_env(
            &["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60", "push", "--quiet", "--no-verify", url, &refspec],
            None,
            None,
            env,
        )
        .map(|_| ())
    }

    /// The tree at `revision` (or the latest) as a zip, under a `ledger/` folder.
    pub fn zip(&self, revision: Option<&str>) -> Result<Option<Vec<u8>>> {
        let Some(rev) = revision.map(str::to_string).or_else(|| self.head()) else { return Ok(None) };
        self.git(&["archive", "--format=zip", "--prefix=ledger/", &rev], None, None).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        pairs.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect()
    }

    #[test]
    fn snapshots_commit_only_changes_and_keep_history() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = LedgerRepo::open(tmp.path().join("ledger.git")).unwrap();
        assert!(repo.head().is_none());
        let a = repo.write_snapshot(&files(&[("README.md", "hi"), ("bots/sam/bot.json", "{}")]), "First").unwrap().unwrap();
        assert_eq!(repo.write_snapshot(&files(&[("README.md", "hi"), ("bots/sam/bot.json", "{}")]), "Same").unwrap(), None);
        let b = repo.write_snapshot(&files(&[("README.md", "hi"), ("routines/daily.json", "{}")]), "Second").unwrap().unwrap();
        assert_ne!(a, b);
        assert_eq!(repo.files(None).unwrap().iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["README.md", "routines/daily.json"]);
        assert_eq!(repo.read("bots/sam/bot.json", Some(&a)).unwrap().as_deref(), Some(&b"{}"[..]));
        assert_eq!(repo.read("bots/sam/bot.json", None).unwrap(), None);
        let h = repo.history(10).unwrap();
        assert_eq!(h.iter().map(|c| c.message.as_str()).collect::<Vec<_>>(), vec!["Second", "First"]);
        let mut changed = repo.changed(&b).unwrap();
        changed.sort();
        assert_eq!(changed, vec!["bots/sam/bot.json", "routines/daily.json"]);
        assert!(repo.zip(None).unwrap().unwrap().starts_with(b"PK"));
    }

    #[test]
    fn refuses_unsafe_paths() {
        for bad in ["", "/etc/passwd", "a/../b", ".git/config", "a//b", "a\\b"] {
            assert!(!valid_path(bad), "{bad}");
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = LedgerRepo::open(tmp.path().join("l.git")).unwrap();
        assert!(repo.write_snapshot(&files(&[("../x", "y")]), "m").is_err());
    }
}
