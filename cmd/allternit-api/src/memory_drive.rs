//! Canonical Memory Drive storage. Callers authenticate the owner and supply
//! its absolute bare-repository path; this module never chooses a data root.
//! SQLite, transport, and UI are deliberately outside this storage boundary.
//! Every mutation builds a private index and publishes with update-ref CAS.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_DRIVE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FILES: usize = 256;
pub const MAX_INDEX_LINES: usize = 200;
const ZERO_OID: &str = "0000000000000000000000000000000000000000";
/// git's empty tree object, the diff base for a first push.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const OWNER_FILE: &str = "allternit-memory-owner";
const REF_PREFIX: &str = "refs/heads/";

pub type Result<T> = std::result::Result<T, DriveError>;

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("memory contains a possible credential; remove it before saving")]
    Secret,
    #[error("memory drive changed; read the latest snapshot and retry (expected {expected:?}, actual {actual:?})")]
    Conflict {
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error("memory drive belongs to a different owner")]
    OwnerMismatch,
    #[error("memory drive is not initialized")]
    Uninitialized,
    #[error("memory file or entry not found: {0}")]
    NotFound(String),
    #[error("memory drive exceeds {0}")]
    Limit(&'static str),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
    // No content, stdin, credential, or git stderr is included in an error.
    #[error("git {operation} failed (exit {code:?})")]
    Git {
        operation: String,
        code: Option<i32>,
    },
}

fn invalid(field: &'static str, reason: impl Into<String>) -> DriveError {
    DriveError::Invalid {
        field,
        reason: reason.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub text: String,
    /// A session URL, or exactly `imported:unknown` for missing provenance.
    pub source: String,
    pub added: String,
    /// Open standard metadata (e.g. agent, observation, memory_type).
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl Entry {
    pub fn render(&self) -> Result<String> {
        validate_identifier(&self.id, "entry id")?;
        validate_text(&self.text)?;
        validate_source(&self.source)?;
        if chrono::NaiveDate::parse_from_str(&self.added, "%Y-%m-%d").is_err()
            || self.added.len() != 10
        {
            return Err(invalid("added", "use YYYY-MM-DD"));
        }
        let mut parts = vec![
            format!("source: {}", self.source),
            format!("added: {}", self.added),
            format!("id: {}", self.id),
        ];
        for (key, value) in &self.metadata {
            validate_identifier(key, "metadata key")?;
            if ["id", "source", "added"].contains(&key.as_str()) {
                return Err(invalid("metadata key", "reserved key"));
            }
            validate_metadata_value(value)?;
            parts.push(format!("{key}: {value}"));
        }
        let line = format!("- {} [{}]", self.text, parts.join("; "));
        scan_secrets(&line)?;
        Ok(line)
    }

    pub fn parse(line: &str) -> Result<Self> {
        let line = line
            .strip_prefix("- ")
            .ok_or_else(|| invalid("entry", "use a one-line bullet"))?;
        let (text, metadata) = line
            .rsplit_once(" [")
            .ok_or_else(|| invalid("entry", "missing provenance metadata"))?;
        let metadata = metadata
            .strip_suffix(']')
            .ok_or_else(|| invalid("entry", "unterminated metadata"))?;
        let mut fields = BTreeMap::new();
        for part in metadata.split("; ") {
            let (key, value) = part
                .split_once(": ")
                .ok_or_else(|| invalid("entry", "invalid metadata pair"))?;
            if fields.insert(key.to_string(), value.to_string()).is_some() {
                return Err(invalid("entry", "duplicate metadata key"));
            }
        }
        let explicit_id = fields.remove("id");
        let mut required = |key: &str| {
            fields
                .remove(key)
                .ok_or_else(|| invalid("entry", format!("missing {key}")))
        };
        let source = required("source")?;
        let added = required("added")?;
        // `id` is an Allternit extension, not a requirement of the open
        // standard. External agents can write the published source/date
        // format; deterministic identity lets the rebuilt index track it.
        let id = explicit_id.unwrap_or_else(|| {
            format!(
                "entry-{}",
                hex::encode(Sha256::digest(
                    format!("{text}\0{source}\0{added}").as_bytes()
                ))
            )
        });
        let entry = Self {
            id,
            text: text.to_string(),
            source,
            added,
            metadata: fields,
        };
        entry.render()?;
        Ok(entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub revision: Option<String>,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operation {
    /// Complete markdown; entries need source/date, with optional stable ids.
    SetFile {
        path: String,
        content: String,
    },
    DeleteFile {
        path: String,
    },
    /// Replaces by stable id across the drive; re-applying is a no-op.
    UpsertEntry {
        path: String,
        entry: Entry,
    },
    DeleteEntry {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub revision: String,
    pub changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub revision: String,
    pub parents: Vec<String>,
    pub author: String,
    pub timestamp: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct MemoryDrive {
    repo: PathBuf,
    owner: String,
    branch: String,
    /// Git object-quarantine variables, set only inside a pre-receive hook so
    /// validation can read objects that are not yet published.
    quarantine: Vec<(String, std::ffi::OsString)>,
}

/// Commits one push may add; larger histories are refused.
pub const MAX_PUSH_COMMITS: usize = 200;
const QUARANTINE_VARS: [&str; 3] = ["GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_QUARANTINE_PATH"];

impl MemoryDrive {
    pub fn new(repo: PathBuf, authenticated_owner: &str, branch: &str) -> Result<Self> {
        validate_repo_path(&repo)?;
        if authenticated_owner.is_empty()
            || authenticated_owner.len() > 256
            || authenticated_owner.chars().any(char::is_control)
        {
            return Err(invalid("owner", "missing or invalid authenticated owner"));
        }
        validate_branch(branch)?;
        Ok(Self {
            repo,
            owner: authenticated_owner.to_string(),
            branch: branch.to_string(),
            quarantine: Vec::new(),
        })
    }

    /// For the pre-receive hook only: read the quarantined objects of the
    /// push being validated.
    pub fn with_quarantine_from_env(mut self) -> Self {
        self.quarantine = QUARANTINE_VARS
            .iter()
            .filter_map(|k| std::env::var_os(k).map(|v| (k.to_string(), v)))
            .collect();
        self
    }

    /// Validate one pushed ref update before git publishes it: only the
    /// drive branch, no deletion, fast-forward only, bounded commit count,
    /// and every new commit's whole tree passes the same format, path, mode,
    /// size and secret checks as a server write.
    pub fn validate_push(&self, old: &str, new: &str, refname: &str) -> Result<()> {
        self.check_repo()?;
        validate_oid(old)?;
        validate_oid(new)?;
        if refname != self.reference() {
            return Err(invalid("ref", format!("only {} can be pushed", self.reference())));
        }
        if new == ZERO_OID {
            return Err(invalid("ref", "deleting the memory branch is not allowed"));
        }
        if old != ZERO_OID {
            let ancestor = self.git_raw(&["merge-base", "--is-ancestor", old, new], None, None, None)?;
            match ancestor.status.code() {
                Some(0) => {}
                Some(1) => return Err(invalid("push", "history rewrite refused; pull, merge, then push")),
                _ => return Err(git_error("merge-base", &ancestor)),
            }
        }
        // twin/ mirrors owner-approved twin memory; only Allternit writes it.
        let base = if old == ZERO_OID { EMPTY_TREE } else { old };
        let managed = self.git_text(&["diff", "--no-ext-diff", "--no-textconv", "--name-only", base, new, "--", "twin"])?;
        if !managed.trim().is_empty() {
            return Err(invalid("push", "twin/ is managed by Allternit; change twin memories in Settings"));
        }
        let range = if old == ZERO_OID { new.to_string() } else { format!("{old}..{new}") };
        let listed = self.git_text(&["rev-list", "--max-count=201", &range])?;
        let commits: Vec<&str> = listed.lines().filter(|l| !l.is_empty()).collect();
        if commits.len() > MAX_PUSH_COMMITS {
            return Err(DriveError::Limit("commits per push"));
        }
        for commit in commits {
            validate_oid(commit)?;
            let snapshot = self.snapshot(Some(commit))?;
            for content in snapshot.files.values() {
                scan_secrets(content)?;
            }
        }
        Ok(())
    }

    pub fn repo_path(&self) -> &Path {
        &self.repo
    }
    fn reference(&self) -> String {
        format!("{REF_PREFIX}{}", self.branch)
    }
    fn owner_digest(&self) -> String {
        format!("{}\n", hex::encode(Sha256::digest(self.owner.as_bytes())))
    }

    /// Construct the whole bare repo privately, then atomically publish its
    /// directory. Concurrent initialization losers inspect the winner; they
    /// never initialize over it or share a checkout/index. Only this module's
    /// owner-marked bare repos may be opened (no arbitrary repo adoption).
    pub fn initialize(&self) -> Result<Snapshot> {
        validate_repo_path(&self.repo)?;
        if !self.repo.exists() {
            let parent = self
                .repo
                .parent()
                .ok_or_else(|| invalid("repo", "missing parent"))?;
            fs::create_dir_all(parent)?;
            validate_repo_path(&self.repo)?;
            let staged = Scratch::directory(parent, "init")?;
            let stage_drive = Self {
                repo: staged.path.clone(),
                owner: self.owner.clone(),
                branch: self.branch.clone(),
                quarantine: Vec::new(),
            };
            stage_drive.git(
                &[
                    "init",
                    "--bare",
                    "--object-format=sha1",
                    &format!("--initial-branch={}", self.branch),
                ],
                None,
                None,
                None,
            )?;
            fs::write(staged.path.join(OWNER_FILE), self.owner_digest())?;
            stage_drive.git(
                &["config", "receive.denyNonFastForwards", "true"],
                None,
                None,
                None,
            )?;
            stage_drive.git(&["config", "receive.denyDeletes", "true"], None, None, None)?;
            match fs::rename(&staged.path, &self.repo) {
                Ok(()) => {}
                Err(e) if self.repo.exists() => {
                    // Target has the winner's complete repo. No replacement
                    // of an existing nonempty directory is possible.
                    let _ = e;
                }
                Err(e) => return Err(e.into()),
            }
        }
        self.check_repo()?;
        if self.head()?.is_none() {
            match self.apply_batch(None, &[], "Initialize Memory Drive", "Allternit") {
                Ok(_) | Err(DriveError::Conflict { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        self.snapshot(None)
    }

    fn check_repo(&self) -> Result<()> {
        validate_repo_path(&self.repo)?;
        let marker = self.repo.join(OWNER_FILE);
        if fs::symlink_metadata(&marker)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(invalid("repo", "owner marker is a symlink"));
        }
        match fs::read_to_string(marker) {
            Ok(owner) if owner == self.owner_digest() => {}
            Ok(_) => return Err(DriveError::OwnerMismatch),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(DriveError::Uninitialized)
            }
            Err(e) => return Err(e.into()),
        }
        if self
            .git_text(&["rev-parse", "--is-bare-repository"])?
            .trim()
            != "true"
        {
            return Err(invalid("repo", "expected a bare repository"));
        }
        if self.git_text(&["symbolic-ref", "HEAD"])?.trim() != self.reference() {
            return Err(invalid(
                "branch",
                "repository HEAD differs from configured branch",
            ));
        }
        Ok(())
    }

    pub fn head(&self) -> Result<Option<String>> {
        self.check_repo()?;
        let output = self.git_raw(
            &["rev-parse", "--verify", "--quiet", &self.reference()],
            None,
            None,
            None,
        )?;
        if output.status.success() {
            let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
            validate_oid(&oid)?;
            Ok(Some(oid))
        } else if output.status.code() == Some(1) {
            Ok(None)
        } else {
            Err(git_error("rev-parse", &output))
        }
    }

    pub fn snapshot(&self, revision: Option<&str>) -> Result<Snapshot> {
        self.check_repo()?;
        let revision = match revision {
            Some(oid) => {
                validate_oid(oid)?;
                Some(oid.to_string())
            }
            None => self.head()?,
        };
        let Some(oid) = revision.as_deref() else {
            return Ok(Snapshot {
                revision,
                files: BTreeMap::new(),
            });
        };
        if self.git_text(&["cat-file", "-t", oid])?.trim() != "commit" {
            return Err(invalid("revision", "expected a commit"));
        }
        let tree = self.git(
            &["ls-tree", "-r", "-z", "--full-tree", oid],
            None,
            None,
            None,
        )?;
        let mut files = BTreeMap::new();
        let mut bytes = 0;
        for record in tree.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            if files.len() >= MAX_FILES {
                return Err(DriveError::Limit("file count"));
            }
            let record =
                std::str::from_utf8(record).map_err(|_| invalid("tree", "non-UTF8 path"))?;
            let (header, path) = record
                .split_once('\t')
                .ok_or_else(|| invalid("tree", "invalid tree record"))?;
            validate_path(path)?;
            let fields: Vec<_> = header.split_whitespace().collect();
            if fields.len() != 3 || fields[0] != "100644" || fields[1] != "blob" {
                return Err(invalid(
                    "tree",
                    "only non-executable regular markdown files are allowed",
                ));
            }
            validate_oid(fields[2])?;
            let size: usize = self
                .git_text(&["cat-file", "-s", fields[2]])?
                .trim()
                .parse()
                .map_err(|_| invalid("blob", "invalid size"))?;
            if size > MAX_FILE_BYTES {
                return Err(DriveError::Limit("file size"));
            }
            bytes += size;
            if bytes > MAX_DRIVE_BYTES {
                return Err(DriveError::Limit("drive size"));
            }
            let blob = self.git(&["cat-file", "blob", fields[2]], None, None, None)?;
            let text =
                String::from_utf8(blob).map_err(|_| invalid("file", "markdown must be UTF8"))?;
            files.insert(path.to_string(), text);
        }
        validate_files(&files)?;
        Ok(Snapshot { revision, files })
    }

    pub fn read_tree(&self, revision: Option<&str>) -> Result<Vec<String>> {
        Ok(self.snapshot(revision)?.files.into_keys().collect())
    }

    pub fn read_file(&self, path: &str, revision: Option<&str>) -> Result<String> {
        validate_path(path)?;
        self.snapshot(revision)?
            .files
            .remove(path)
            .ok_or_else(|| DriveError::NotFound(path.to_string()))
    }

    pub fn history(&self, path: Option<&str>, limit: usize) -> Result<Vec<Commit>> {
        self.check_repo()?;
        if let Some(path) = path {
            validate_path(path)?;
        }
        let Some(head) = self.head()? else {
            return Ok(vec![]);
        };
        let limit = limit.clamp(1, 100).to_string();
        let mut args = vec![
            "log",
            "-z",
            "--no-show-signature",
            "--max-count",
            &limit,
            "--format=%H%x00%P%x00%an%x00%aI%x00%s",
            &head,
            "--",
        ];
        if let Some(path) = path {
            args.push(path);
        }
        let output = self.git(&args, None, None, None)?;
        let parts: Vec<_> = output.split(|b| *b == 0).collect();
        let mut commits = vec![];
        for fields in parts.chunks(5).filter(|f| f.len() == 5) {
            let field = |i: usize| String::from_utf8_lossy(fields[i]).to_string();
            commits.push(Commit {
                revision: field(0),
                parents: field(1).split_whitespace().map(str::to_string).collect(),
                author: field(2),
                timestamp: field(3),
                message: field(4),
            });
        }
        Ok(commits)
    }

    pub fn diff(&self, from: &str, to: &str, path: Option<&str>) -> Result<String> {
        // Validate both complete trees, including historical content. The
        // empty tree is the base for a drive's first commit.
        if from != EMPTY_TREE {
            self.snapshot(Some(from))?;
        }
        self.snapshot(Some(to))?;
        if let Some(path) = path {
            validate_path(path)?;
        }
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            from,
            to,
            "--",
        ];
        if let Some(path) = path {
            args.push(path);
        }
        self.git_text(&args)
    }

    /// No automatic blind retry. A conflict includes the new head so callers
    /// can re-read, reconcile their intent by stable id, and retry the batch.
    /// Even a no-op checks revision again before acknowledging success.
    pub fn apply_batch(
        &self,
        expected: Option<&str>,
        operations: &[Operation],
        message: &str,
        author: &str,
    ) -> Result<ApplyResult> {
        self.check_repo()?;
        if let Some(oid) = expected {
            validate_oid(oid)?;
        }
        validate_label(message, "commit message")?;
        validate_label(author, "author")?;
        if operations.len() > 1000 {
            return Err(DriveError::Limit("batch operation count"));
        }
        self.check_expected(expected)?;
        let original = match expected {
            Some(oid) => self.snapshot(Some(oid))?.files,
            None => BTreeMap::new(),
        };
        let mut files = original.clone();
        files
            .entry("MEMORY.md".to_string())
            .or_insert_with(|| "# Memory\n\n## Index\n".to_string());
        for operation in operations {
            match operation {
                Operation::SetFile { path, content } => {
                    validate_path(path)?;
                    validate_file(path, content)?;
                    files.insert(path.clone(), content.clone());
                }
                Operation::DeleteFile { path } => {
                    validate_path(path)?;
                    if path == "MEMORY.md" {
                        return Err(invalid("path", "MEMORY.md is required"));
                    }
                    files.remove(path);
                }
                Operation::UpsertEntry { path, entry } => {
                    validate_path(path)?;
                    let line = entry.render()?;
                    // Find the stable id throughout the tree, removing its
                    // previous location if the entry is moved to another file.
                    remove_entry(&mut files, &entry.id, Some((path, &line)))?;
                    let content = files
                        .entry(path.clone())
                        .or_insert_with(|| format!("# {}\n", path.trim_end_matches(".md")));
                    if !content.lines().any(|existing| existing == line) {
                        if path == "MEMORY.md" {
                            let index = content
                                .find("\n## Index\n")
                                .map(|offset| offset + 1)
                                .ok_or_else(|| invalid("MEMORY.md", "missing Index heading"))?;
                            content.insert_str(index, &format!("{line}\n\n"));
                        } else {
                            if !content.ends_with('\n') {
                                content.push('\n');
                            }
                            content.push_str(&format!("{line}\n"));
                        }
                    }
                }
                Operation::DeleteEntry { id } => {
                    validate_identifier(id, "entry id")?;
                    remove_entry(&mut files, id, None)?;
                }
            }
        }
        rebuild_index(&mut files)?;
        // Whole candidate scan: unchanged files, imports and snapshot edits
        // are checked too, before any blobs/commit are written.
        validate_files(&files)?;
        if files == original {
            self.check_expected(expected)?;
            return Ok(ApplyResult {
                revision: expected.ok_or(DriveError::Uninitialized)?.to_string(),
                changed: false,
            });
        }
        let index = Scratch::absent_file(&self.repo, "index")?;
        self.git(&["read-tree", "--empty"], None, Some(&index.path), None)?;
        // One index update for the whole tree (not one git process per file).
        let mut records = String::new();
        for (path, content) in &files {
            let blob = String::from_utf8_lossy(&self.git(
                &["hash-object", "-w", "--stdin"],
                Some(content.as_bytes()),
                None,
                None,
            )?)
            .trim()
            .to_string();
            validate_oid(&blob)?;
            records.push_str(&format!("100644 {blob}\t{path}\0"));
        }
        self.git(
            &["update-index", "-z", "--index-info"],
            Some(records.as_bytes()),
            Some(&index.path),
            None,
        )?;
        let tree =
            String::from_utf8_lossy(&self.git(&["write-tree"], None, Some(&index.path), None)?)
                .trim()
                .to_string();
        validate_oid(&tree)?;
        let mut args = vec!["commit-tree", &tree];
        if let Some(parent) = expected {
            args.extend(["-p", parent]);
        }
        let commit = String::from_utf8_lossy(&self.git(
            &args,
            Some(format!("{message}\n").as_bytes()),
            None,
            Some(author),
        )?)
        .trim()
        .to_string();
        validate_oid(&commit)?;
        let output = self.git_raw(
            &[
                "update-ref",
                &self.reference(),
                &commit,
                expected.unwrap_or(ZERO_OID),
            ],
            None,
            None,
            None,
        )?;
        if !output.status.success() {
            let actual = self.head()?;
            if actual.as_deref() != expected {
                return Err(DriveError::Conflict {
                    expected: expected.map(str::to_string),
                    actual,
                });
            }
            return Err(git_error("update-ref", &output));
        }
        Ok(ApplyResult {
            revision: commit,
            changed: true,
        })
    }

    /// Publish a complete, already reconciled snapshot against the current
    /// expected revision. This is a CAS primitive, not a blind history reset:
    /// Dream/import/undo callers must preserve intervening edits themselves.
    pub fn replace_snapshot(
        &self,
        expected: Option<&str>,
        snapshot: &Snapshot,
        message: &str,
        author: &str,
    ) -> Result<ApplyResult> {
        validate_files(&snapshot.files)?;
        self.check_expected(expected)?;
        let current = match expected {
            Some(oid) => self.snapshot(Some(oid))?.files,
            None => BTreeMap::new(),
        };
        let mut operations: Vec<Operation> = current
            .keys()
            .filter(|path| !snapshot.files.contains_key(*path))
            .map(|path| Operation::DeleteFile { path: path.clone() })
            .collect();
        operations.extend(
            snapshot
                .files
                .iter()
                .map(|(path, content)| Operation::SetFile {
                    path: path.clone(),
                    content: content.clone(),
                }),
        );
        self.apply_batch(expected, &operations, message, author)
    }

    fn check_expected(&self, expected: Option<&str>) -> Result<()> {
        let actual = self.head()?;
        if actual.as_deref() != expected {
            return Err(DriveError::Conflict {
                expected: expected.map(str::to_string),
                actual,
            });
        }
        Ok(())
    }

    fn git_text(&self, args: &[&str]) -> Result<String> {
        String::from_utf8(self.git(args, None, None, None)?)
            .map_err(|_| invalid("git output", "non-UTF8 output"))
    }

    fn git(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: Option<&Path>,
        author: Option<&str>,
    ) -> Result<Vec<u8>> {
        let output = self.git_raw(args, input, index, author)?;
        if !output.status.success() {
            return Err(git_error(args[0], &output));
        }
        Ok(output.stdout)
    }

    fn git_raw(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: Option<&Path>,
        author: Option<&str>,
    ) -> Result<Output> {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .env("GIT_TERMINAL_PROMPT", "0")
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
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (key, value) in &self.quarantine {
            command.env(key, value);
        }
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        if let Some(author) = author {
            command
                .env("GIT_AUTHOR_NAME", author)
                .env("GIT_COMMITTER_NAME", author)
                .env("GIT_AUTHOR_EMAIL", "memory@allternit.invalid")
                .env("GIT_COMMITTER_EMAIL", "memory@allternit.invalid");
        }
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().expect("piped git stdout");
        let stderr = child.stderr.take().expect("piped git stderr");
        // Drain both pipes concurrently and bound retained output. This also
        // prevents a broken git process blocking a large stdin write while it
        // fills stderr. Never allocate an unbounded external tree/log output.
        let stdout_reader = std::thread::spawn(move || read_bounded(stdout, MAX_DRIVE_BYTES * 4));
        let stderr_reader = std::thread::spawn(move || read_bounded(stderr, MAX_FILE_BYTES));
        let mut input_error = None;
        if let Some(mut stdin) = child.stdin.take() {
            if let Some(bytes) = input {
                if let Err(error) = stdin.write_all(bytes) {
                    input_error = Some(error);
                }
            }
        }
        let status = child.wait();
        let stdout = stdout_reader.join();
        let stderr = stderr_reader.join();
        let (stdout, stdout_overflow) =
            stdout.map_err(|_| invalid("git output", "stdout reader failed"))??;
        let (stderr, stderr_overflow) =
            stderr.map_err(|_| invalid("git output", "stderr reader failed"))??;
        if let Some(error) = input_error {
            return Err(error.into());
        }
        if stdout_overflow || stderr_overflow {
            return Err(DriveError::Limit("git output size"));
        }
        Ok(Output {
            status: status?,
            stdout,
            stderr,
        })
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut overflow = false;
    let mut buffer = [0u8; 8192];
    loop {
        let size = reader.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        let keep = size.min(limit.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        overflow |= keep != size;
    }
    Ok((output, overflow))
}

fn git_error(operation: &str, output: &Output) -> DriveError {
    DriveError::Git {
        operation: operation.to_string(),
        code: output.status.code(),
    }
}

/// Temporary index/init artifacts are unique per invocation, removed on all
/// normal/error paths. No working checkout or cross-session lock is shared.
struct Scratch {
    path: PathBuf,
    directory: bool,
}
impl Scratch {
    fn directory(parent: &Path, label: &str) -> Result<Self> {
        let path = parent.join(format!("memory-{label}-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self {
            path,
            directory: true,
        })
    }
    fn absent_file(parent: &Path, label: &str) -> Result<Self> {
        // Reserve a private directory; read-tree expects the index not to
        // exist, so put that path inside the exclusively-created directory.
        let directory = Self::directory(parent, label)?;
        let path = directory.path.join("index");
        std::mem::forget(directory);
        Ok(Self {
            path,
            directory: false,
        })
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let root = if self.directory {
            self.path.as_path()
        } else {
            self.path.parent().unwrap_or(&self.path)
        };
        let _ = fs::remove_dir_all(root);
    }
}

fn validate_identifier(value: &str, field: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid(
            field,
            "use 1–128 letters, digits, underscores or hyphens",
        ));
    }
    Ok(())
}

pub fn validate_branch(branch: &str) -> Result<()> {
    if branch.is_empty() || branch.len() > 128 || branch == "HEAD" {
        return Err(invalid("branch", "invalid branch name"));
    }
    for segment in branch.split('/') {
        validate_identifier(segment, "branch")?;
        if segment.starts_with('-') {
            return Err(invalid("branch", "segment starts with '-'"));
        }
    }
    Ok(())
}

pub fn validate_oid(oid: &str) -> Result<()> {
    if oid.len() != 40
        || !oid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || oid == ZERO_OID
    {
        return Err(invalid(
            "revision",
            "expected a nonzero lowercase SHA-1 object id",
        ));
    }
    Ok(())
}

pub fn validate_path(path: &str) -> Result<()> {
    if path.len() > 240 || path.is_empty() || !path.ends_with(".md") {
        return Err(invalid(
            "path",
            "use a relative markdown path of at most 240 bytes",
        ));
    }
    for segment in path.split('/') {
        if segment.is_empty()
            || segment.starts_with('.')
            || segment.starts_with('-')
            || segment.contains("..")
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        {
            return Err(invalid(
                "path",
                "absolute, hidden, traversal and special paths are forbidden",
            ));
        }
        let stem = segment.trim_end_matches(".md").to_ascii_lowercase();
        if stem.contains("transcript")
            || [
                "log",
                "logs",
                "chat",
                "chats",
                "session",
                "sessions",
                "conversation",
                "conversations",
            ]
            .contains(&stem.as_str())
            || stem.ends_with("-log")
            || stem.ends_with("_log")
            || stem.starts_with("session-")
            || stem.starts_with("turn-")
        {
            return Err(invalid(
                "path",
                "store durable notes, not transcripts or logs",
            ));
        }
    }
    Ok(())
}

fn validate_repo_path(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.to_string_lossy().chars().any(char::is_control)
        || path
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid(
            "repo",
            "caller must supply a safe absolute repository path",
        ));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(invalid("repo", "symlink ancestor is forbidden"))
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(invalid("repo", "repository ancestors must be directories"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn validate_metadata_value(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 2048
        || value != value.trim()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, ';' | '[' | ']' | '\\'))
    {
        return Err(invalid("metadata", "empty, multiline or injected metadata"));
    }
    Ok(())
}

pub fn validate_source(source: &str) -> Result<()> {
    validate_metadata_value(source)?;
    if source == "imported:unknown" {
        return Ok(());
    }
    let lower = source.to_ascii_lowercase();
    // Opaque provenance labels from other agents and local sessions, e.g.
    // `gizzi:session/abc` or `claude-code:session/xyz`. Never rendered as a
    // link; dangerous schemes are refused.
    static LABEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let label = LABEL.get_or_init(|| {
        regex::Regex::new(r"^([a-z][a-z0-9+.-]{0,31}):([A-Za-z0-9._~/:@=&?+-]{1,512})$").expect("constant regex")
    });
    if let Some(caps) = label.captures(source) {
        let scheme = &caps[1];
        if !matches!(scheme, "http" | "https") {
            if matches!(scheme, "javascript" | "data" | "file" | "vbscript" | "blob" | "about" | "ftp" | "ws" | "wss") {
                return Err(invalid("source", "unsafe URL scheme"));
            }
            if caps[2].starts_with("//") {
                return Err(invalid("source", "use a session URL or a provenance label"));
            }
            return Ok(());
        }
    }
    if source.chars().any(char::is_whitespace)
        || ["%00", "%0a", "%0d", "%5c"]
            .iter()
            .any(|s| lower.contains(s))
    {
        return Err(invalid("source", "unsafe URL"));
    }
    let relative = source.starts_with('/') && !source.starts_with("//");
    let base = url::Url::parse("https://ai.allternit.com/").expect("constant URL");
    let parsed = if relative {
        base.join(source)
    } else {
        url::Url::parse(source)
    }
    .map_err(|_| invalid("source", "use a session URL or imported:unknown"))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
        || !(parsed.scheme() == "https"
            || (parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        || (relative && parsed.host_str() != Some("ai.allternit.com"))
    {
        return Err(invalid(
            "source",
            "only HTTPS, loopback HTTP or product-relative session links are accepted",
        ));
    }
    Ok(())
}

fn validate_text(text: &str) -> Result<()> {
    if text.is_empty()
        || text.len() > 4096
        || text != text.trim()
        || text.chars().any(char::is_control)
    {
        return Err(invalid(
            "entry text",
            "use a nonempty single line of at most 4096 bytes",
        ));
    }
    // Metadata belongs to the renderer, never embedded in the fact text.
    if text.contains(" [")
        && regex::Regex::new(r" \[[A-Za-z_][A-Za-z0-9_-]*:")
            .expect("constant regex")
            .is_match(text)
    {
        return Err(invalid("entry text", "metadata injection"));
    }
    validate_markdown_links(text)?;
    scan_secrets(text)
}

fn validate_label(label: &str, field: &'static str) -> Result<()> {
    if label.is_empty()
        || label.len() > 240
        || label != label.trim()
        || label
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>'))
    {
        return Err(invalid(field, "use a short single-line label"));
    }
    scan_secrets(label)
}

/// Whole-file scanner, including punctuation-adjacent keys and private key
/// blocks. Reuses the kernel baseline and adds credential token patterns.
/// Errors never include the matched value.
pub fn scan_secrets(content: &str) -> Result<()> {
    static TOKENS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let tokens = TOKENS.get_or_init(|| regex::Regex::new(
        r"(?i)(sk-[a-z0-9_-]{8,}|gh[pousr]_[a-z0-9_]{8,}|github_pat_[a-z0-9_]+|glpat-[a-z0-9_-]+|xox[baprs]-[a-z0-9-]+|AKIA[A-Z0-9]{16}|ASIA[A-Z0-9]{16}|allternit_git_[a-z0-9]+|eyJ[a-z0-9_-]+\.[a-z0-9_-]+\.[a-z0-9_-]+|-----BEGIN [A-Z ]*PRIVATE KEY-----|(?:token|secret|password)\s*[:=]\s*[^\s]+)"
    ).expect("constant secret regex"));
    if crate::memory_kernel_service::mentions_secret(content) || tokens.is_match(content) {
        return Err(DriveError::Secret);
    }
    Ok(())
}

fn validate_markdown_links(content: &str) -> Result<()> {
    // Raw HTML/images/autolinks are unnecessary in a durable memory note.
    if content.contains('<') || content.contains('>') || content.contains("![") {
        return Err(invalid(
            "markdown",
            "HTML, images and autolinks are forbidden",
        ));
    }
    static LINKS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let links = LINKS.get_or_init(|| regex::Regex::new(r"\]\(([^)]*)\)").expect("constant regex"));
    for captures in links.captures_iter(content) {
        validate_source(&captures[1])?;
    }
    Ok(())
}

fn validate_file(path: &str, content: &str) -> Result<()> {
    validate_path(path)?;
    if content.len() > MAX_FILE_BYTES {
        return Err(DriveError::Limit("file size"));
    }
    scan_secrets(content)?;
    validate_markdown_links(content)?;
    if content.chars().any(|c| c.is_control() && c != '\n') {
        return Err(invalid("file", "control characters are forbidden"));
    }
    if path == "MEMORY.md" {
        let title = content.lines().next().unwrap_or_default();
        if !(title == "# Memory" || title.starts_with("# Memory: "))
            || content.lines().filter(|l| *l == "## Index").count() != 1
        {
            return Err(invalid(
                "MEMORY.md",
                "must start with # Memory and contain one ## Index",
            ));
        }
        if content.lines().count() > MAX_INDEX_LINES || content.len() > 16 * 1024 {
            return Err(DriveError::Limit("short MEMORY.md index"));
        }
    }
    let mut index = false;
    for line in content.lines() {
        if line == "## Index" && path == "MEMORY.md" {
            index = true;
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') && !index {
            continue;
        }
        if line.starts_with("- [[") && line.ends_with("]]") {
            let target = &line[4..line.len() - 2];
            if target.ends_with(".md") {
                return Err(invalid("cross-link", "omit .md"));
            }
            validate_path(&format!("{target}.md"))?;
        } else if index {
            return Err(invalid("Index", "only [[topic]] links go under ## Index"));
        } else {
            Entry::parse(line)?;
        }
    }
    Ok(())
}

/// Validate a full proposed tree before transport/index callers accept it.
pub fn validate_files(files: &BTreeMap<String, String>) -> Result<()> {
    if !files.contains_key("MEMORY.md") {
        return Err(invalid("tree", "MEMORY.md is required"));
    }
    if files.len() > MAX_FILES {
        return Err(DriveError::Limit("file count"));
    }
    if files.values().map(String::len).sum::<usize>() > MAX_DRIVE_BYTES {
        return Err(DriveError::Limit("drive size"));
    }
    let mut ids = BTreeSet::new();
    static CROSS_LINKS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let links = CROSS_LINKS
        .get_or_init(|| regex::Regex::new(r"\[\[([^\[\]]+)\]\]").expect("constant regex"));
    for (path, content) in files {
        validate_file(path, content)?;
        for line in content
            .lines()
            .filter(|l| l.starts_with("- ") && !l.starts_with("- [["))
        {
            let entry = Entry::parse(line)?;
            if !ids.insert(entry.id) {
                return Err(invalid("entry id", "duplicate id across files"));
            }
        }
        for captures in links.captures_iter(content) {
            let target = &captures[1];
            if target.ends_with(".md") {
                return Err(invalid("cross-link", "omit .md"));
            }
            let filename = format!("{target}.md");
            validate_path(&filename)?;
            if !files.contains_key(&filename) {
                return Err(invalid("cross-link", format!("missing topic {filename}")));
            }
        }
    }
    Ok(())
}

fn rebuild_index(files: &mut BTreeMap<String, String>) -> Result<()> {
    let memory = files
        .get("MEMORY.md")
        .ok_or_else(|| invalid("tree", "MEMORY.md is required"))?;
    let index = memory
        .lines()
        .position(|l| l == "## Index")
        .ok_or_else(|| invalid("MEMORY.md", "missing Index heading"))?;
    let mut content = memory
        .lines()
        .take(index)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();
    content.push_str("\n\n## Index\n");
    for filename in files.keys().filter(|p| p.as_str() != "MEMORY.md") {
        content.push_str(&format!("- [[{}]]\n", filename.trim_end_matches(".md")));
    }
    files.insert("MEMORY.md".to_string(), content);
    Ok(())
}

fn remove_entry(
    files: &mut BTreeMap<String, String>,
    id: &str,
    replacement: Option<(&str, &str)>,
) -> Result<()> {
    for (path, content) in files.iter_mut() {
        let mut updated = Vec::new();
        for line in content.lines() {
            if line.starts_with("- ") && !line.starts_with("- [[") && Entry::parse(line)?.id == id {
                if let Some((target, new_line)) = replacement {
                    if target == path {
                        updated.push(new_line);
                    }
                }
            } else {
                updated.push(line);
            }
        }
        *content = format!("{}\n", updated.join("\n"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn subprocess_capture_is_bounded_and_drains_the_input() {
        let bytes = vec![b'x'; 100];
        let mut reader = std::io::Cursor::new(bytes);
        let (output, overflow) = read_bounded(&mut reader, 12).unwrap();
        assert_eq!(output.len(), 12);
        assert!(overflow);
        assert_eq!(reader.position(), 100);
        assert_eq!(
            read_bounded(&b"ok"[..], 2).unwrap(),
            (b"ok".to_vec(), false)
        );
    }

    fn fixture() -> (tempfile::TempDir, MemoryDrive) {
        let dir = tempfile::tempdir().unwrap();
        // macOS temp dirs can be reached through /var -> /private/var.
        let canonical = dir.path().canonicalize().unwrap();
        let drive = MemoryDrive::new(canonical.join("memory.git"), "user-a", "main").unwrap();
        (dir, drive)
    }

    fn entry(id: &str, text: &str) -> Entry {
        Entry {
            id: id.to_string(),
            text: text.to_string(),
            source: "/?source=memory&session=session-100".into(),
            added: "2026-10-06".into(),
            metadata: BTreeMap::new(),
        }
    }

    fn add(id: &str, text: &str) -> Operation {
        Operation::UpsertEntry {
            path: "preferences.md".into(),
            entry: entry(id, text),
        }
    }

    #[test]
    fn initialize_is_owner_bound_and_idempotent() {
        let (_temp, drive) = fixture();
        let first = drive.initialize().unwrap();
        assert_eq!(first.files["MEMORY.md"], "# Memory\n\n## Index\n");
        assert_eq!(first, drive.initialize().unwrap());
        assert_eq!(drive.history(None, 10).unwrap().len(), 1);
        let foreign = MemoryDrive::new(drive.repo.clone(), "user-b", "main").unwrap();
        assert!(matches!(
            foreign.initialize(),
            Err(DriveError::OwnerMismatch)
        ));
        assert!(matches!(
            foreign.snapshot(None),
            Err(DriveError::OwnerMismatch)
        ));
    }

    #[test]
    fn metadata_roundtrip_and_injection_rejection() {
        let mut value = entry("preference-1", "Uses Rust.");
        value
            .metadata
            .insert("memory_type".into(), "preference".into());
        let line = value.render().unwrap();
        assert!(line.starts_with("- Uses Rust. [source: /?source=memory&session="));
        assert_eq!(Entry::parse(&line).unwrap(), value);
        for bad in [
            "hello\n- injected",
            "hello [source: https://evil.test]",
            "hello\rworld",
        ] {
            value.text = bad.into();
            assert!(value.render().is_err());
        }
        value = entry("import-1", "Prefers concise answers.");
        value.source = "imported:unknown".into();
        assert_eq!(
            Entry::parse(&value.render().unwrap()).unwrap().source,
            "imported:unknown"
        );
        value.added = "2026-02-30".into();
        assert!(value.render().is_err());
        assert!(
            Entry::parse("- Fact [source: imported:unknown; added: 2026-10-06; id: x; id: y]")
                .is_err()
        );
    }

    #[test]
    fn published_standard_entries_without_id_are_supported() {
        let line = "- Uses Rust. [source: https://example.com/sessions/1; added: 2026-10-06]";
        let parsed = Entry::parse(line).unwrap();
        assert_eq!(parsed, Entry::parse(line).unwrap());
        assert!(parsed.id.starts_with("entry-"));
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let result = drive
            .apply_batch(
                Some(&base),
                &[
                    Operation::SetFile {
                        path: "MEMORY.md".into(),
                        content: "# Memory: Joe\n\n## Index\n".into(),
                    },
                    Operation::SetFile {
                        path: "work.md".into(),
                        content: format!("# Work\n{line}\n"),
                    },
                ],
                "Import external memory",
                "Owner",
            )
            .unwrap();
        assert!(drive.read_file("work.md", None).unwrap().contains(line));
        drive
            .apply_batch(
                Some(&result.revision),
                &[Operation::DeleteEntry { id: parsed.id }],
                "Forget external entry",
                "Owner",
            )
            .unwrap();
        assert!(!drive
            .read_file("work.md", None)
            .unwrap()
            .contains("Uses Rust."));
    }

    #[cfg(unix)]
    #[test]
    fn repository_is_private_to_its_local_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, drive) = fixture();
        drive.initialize().unwrap();
        assert_eq!(
            fs::metadata(drive.repo_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn stale_revision_never_overwrites_and_retry_keeps_both() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let a = drive
            .apply_batch(
                Some(&base),
                &[add("a", "Prefers Rust.")],
                "Remember Rust",
                "Agent A",
            )
            .unwrap();
        let error = drive
            .apply_batch(
                Some(&base),
                &[add("b", "Uses Linear.")],
                "Remember Linear",
                "Agent B",
            )
            .unwrap_err();
        assert!(
            matches!(error, DriveError::Conflict { actual: Some(ref h), .. } if h == &a.revision)
        );
        let latest = drive.snapshot(None).unwrap();
        drive
            .apply_batch(
                latest.revision.as_deref(),
                &[add("b", "Uses Linear.")],
                "Remember Linear",
                "Agent B",
            )
            .unwrap();
        let topic = drive.read_file("preferences.md", None).unwrap();
        assert!(topic.contains("id: a]") && topic.contains("id: b]"));
    }

    #[test]
    fn concurrent_initialization_and_first_adds_are_safe_with_retry() {
        let (_temp, drive) = fixture();
        let barrier = Arc::new(Barrier::new(2));
        let mut threads = vec![];
        for id in ["a", "b"] {
            let drive = drive.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let snapshot = drive.initialize().unwrap();
                barrier.wait();
                let operation = add(id, &format!("Independent memory {id}."));
                let result = drive.apply_batch(
                    snapshot.revision.as_deref(),
                    std::slice::from_ref(&operation),
                    "Remember independently",
                    "Agent",
                );
                match result {
                    Ok(_) => {}
                    Err(DriveError::Conflict { .. }) => {
                        let latest = drive.snapshot(None).unwrap();
                        drive
                            .apply_batch(
                                latest.revision.as_deref(),
                                &[operation],
                                "Retry memory",
                                "Agent",
                            )
                            .unwrap();
                    }
                    Err(error) => panic!("{error}"),
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let files = drive.snapshot(None).unwrap().files;
        assert!(files["preferences.md"].contains("id: a]"));
        assert!(files["preferences.md"].contains("id: b]"));
        assert_eq!(drive.history(None, 10).unwrap().len(), 3);
    }

    #[test]
    fn update_delete_diff_and_history_are_real_git_operations() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let first = drive
            .apply_batch(
                Some(&base),
                &[add("a", "Uses Rust.")],
                "Remember Rust",
                "Agent",
            )
            .unwrap();
        let noop = drive
            .apply_batch(
                Some(&first.revision),
                &[add("a", "Uses Rust.")],
                "Remember Rust again",
                "Agent",
            )
            .unwrap();
        assert!(!noop.changed);
        assert_eq!(noop.revision, first.revision);
        let updated = drive
            .apply_batch(
                Some(&first.revision),
                &[add("a", "Uses Go.")],
                "Correct language",
                "Agent",
            )
            .unwrap();
        let patch = drive
            .diff(&first.revision, &updated.revision, Some("preferences.md"))
            .unwrap();
        assert!(patch.contains("-- Uses Rust.") && patch.contains("+- Uses Go."));
        assert_eq!(drive.history(Some("preferences.md"), 10).unwrap().len(), 2);
        let deleted = drive
            .apply_batch(
                Some(&updated.revision),
                &[Operation::DeleteEntry { id: "a".into() }],
                "Forget language",
                "Owner",
            )
            .unwrap();
        assert!(!drive
            .read_file("preferences.md", None)
            .unwrap()
            .contains("Uses Go."));
        assert!(
            !drive
                .apply_batch(
                    Some(&deleted.revision),
                    &[Operation::DeleteEntry { id: "a".into() }],
                    "Forget again",
                    "Owner"
                )
                .unwrap()
                .changed
        );
        let removed = drive
            .apply_batch(
                Some(&deleted.revision),
                &[Operation::DeleteFile {
                    path: "preferences.md".into(),
                }],
                "Remove topic",
                "Owner",
            )
            .unwrap();
        assert!(!drive
            .read_file("MEMORY.md", None)
            .unwrap()
            .contains("[[preferences]]"));
        assert!(
            !drive
                .apply_batch(
                    Some(&removed.revision),
                    &[Operation::DeleteFile {
                        path: "preferences.md".into()
                    }],
                    "Remove again",
                    "Owner"
                )
                .unwrap()
                .changed
        );
        assert!(drive
            .read_file("preferences.md", Some(&first.revision))
            .unwrap()
            .contains("Uses Rust."));
        assert_eq!(drive.read_tree(None).unwrap(), vec!["MEMORY.md"]);
    }

    #[test]
    fn bad_content_is_rejected_before_commit() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        for secret in [
            "Key (sk-abcdefgh123456).",
            "github_pat_abcdef123456",
            "-----BEGIN PRIVATE KEY-----",
            "allternit_git_1234567890",
            "My password is hunter2.",
        ] {
            assert!(matches!(
                drive.apply_batch(Some(&base), &[add("secret", secret)], "Remember", "Agent"),
                Err(DriveError::Secret)
            ));
        }
        for path in [
            "../outside.md",
            "/absolute.md",
            ".git/config.md",
            "notes/.hidden.md",
            "a\\b.md",
            "a\n.md",
            "notes.sh",
            "transcripts.md",
            "sessions/1.md",
        ] {
            assert!(
                drive
                    .apply_batch(
                        Some(&base),
                        &[Operation::SetFile {
                            path: path.into(),
                            content: "# Note\n".into()
                        }],
                        "Remember",
                        "Agent"
                    )
                    .is_err(),
                "accepted {path}"
            );
        }
        for source in [
            "javascript:alert(1)",
            "data:text/plain,hi",
            "https://user:pw@example.test/x",
            "//evil.test/x",
            "http://evil.test/x",
            "/%0aheader",
            "/?a=x; added: 2020-01-01",
            "/?a=x\n",
        ] {
            assert!(validate_source(source).is_err(), "accepted {source}");
        }
        assert_eq!(drive.head().unwrap().as_deref(), Some(base.as_str()));
        assert_eq!(drive.history(None, 10).unwrap().len(), 1);
        assert!(drive
            .apply_batch(
                Some(&base),
                &[Operation::SetFile {
                    path: "large.md".into(),
                    content: "a".repeat(MAX_FILE_BYTES + 1)
                }],
                "Remember",
                "Agent"
            )
            .is_err());
    }

    #[test]
    fn batch_import_is_one_commit_and_whole_candidate_is_validated() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let mut a = entry("legacy-a", "Uses Rust.");
        a.source = "imported:unknown".into();
        let ops = vec![
            Operation::UpsertEntry {
                path: "topics/work.md".into(),
                entry: a,
            },
            add("b", "Uses Linear."),
        ];
        let imported = drive
            .apply_batch(Some(&base), &ops, "Import existing memory", "Allternit")
            .unwrap();
        assert_eq!(drive.history(None, 10).unwrap().len(), 2);
        assert!(
            !drive
                .apply_batch(
                    Some(&imported.revision),
                    &ops,
                    "Import existing memory",
                    "Allternit"
                )
                .unwrap()
                .changed
        );
        assert!(drive
            .read_file("MEMORY.md", None)
            .unwrap()
            .contains("[[topics/work]]"));
        let missing = Operation::SetFile {
            path: "broken.md".into(),
            content: format!(
                "# Broken\n{}\n",
                entry("broken", "See [[missing]].").render().unwrap()
            ),
        };
        assert!(drive
            .apply_batch(Some(&imported.revision), &[missing], "Remember", "Agent")
            .is_err());
        let duplicate = Operation::SetFile {
            path: "duplicate.md".into(),
            content: format!(
                "# Duplicate\n{}\n",
                entry("b", "Duplicate id.").render().unwrap()
            ),
        };
        assert!(drive
            .apply_batch(Some(&imported.revision), &[duplicate], "Remember", "Agent")
            .is_err());
    }

    #[test]
    fn invalid_refs_and_snapshot_ids_cannot_be_git_options() {
        let (_temp, drive) = fixture();
        drive.initialize().unwrap();
        for branch in [
            "",
            "HEAD",
            "../main",
            "--all",
            "main.lock",
            "main@{1}",
            "a//b",
            "a\nmain",
        ] {
            assert!(validate_branch(branch).is_err());
        }
        for revision in ["HEAD", "--all", ZERO_OID, "HEAD~1", "abc"] {
            assert!(drive.snapshot(Some(revision)).is_err());
        }
    }

    #[test]
    fn external_standard_bullet_gets_identity_and_can_be_updated() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let bullet = "- Uses Rust. [source: https://ai.allternit.com/?source=memory&session=s1; added: 2026-10-06]";
        let mut parsed = Entry::parse(bullet).unwrap();
        assert_eq!(parsed.id, Entry::parse(bullet).unwrap().id);
        assert!(parsed.id.starts_with("entry-"));
        let first = drive
            .apply_batch(
                Some(&base),
                &[Operation::SetFile {
                    path: "work.md".into(),
                    content: format!("# Work\n{bullet}\n"),
                }],
                "Import external note",
                "Agent",
            )
            .unwrap();
        parsed.text = "Uses Go.".into();
        let corrected = drive
            .apply_batch(
                Some(&first.revision),
                &[Operation::UpsertEntry {
                    path: "work.md".into(),
                    entry: parsed.clone(),
                }],
                "Correct note",
                "Agent",
            )
            .unwrap();
        let file = drive.read_file("work.md", None).unwrap();
        assert!(!file.contains("Uses Rust."));
        assert_eq!(file.lines().filter(|l| l.starts_with("- ")).count(), 1);
        assert_eq!(
            Entry::parse(file.lines().find(|l| l.starts_with("- ")).unwrap())
                .unwrap()
                .id,
            parsed.id
        );
        drive
            .apply_batch(
                Some(&corrected.revision),
                &[Operation::DeleteEntry { id: parsed.id }],
                "Forget note",
                "Owner",
            )
            .unwrap();
        assert!(!drive
            .read_file("work.md", None)
            .unwrap()
            .contains("Uses Go."));
    }

    #[test]
    fn complete_snapshot_cas_creates_a_new_commit_without_resetting_history() {
        let (_temp, drive) = fixture();
        let initial = drive.initialize().unwrap();
        let first = drive
            .apply_batch(
                initial.revision.as_deref(),
                &[add("a", "Uses Rust.")],
                "Remember",
                "Agent",
            )
            .unwrap();
        let before_correction = drive.snapshot(None).unwrap();
        let second = drive
            .apply_batch(
                Some(&first.revision),
                &[add("a", "Uses Go.")],
                "Correct",
                "Agent",
            )
            .unwrap();
        assert!(matches!(
            drive.replace_snapshot(Some(&first.revision), &initial, "Undo", "Owner"),
            Err(DriveError::Conflict { .. })
        ));
        let reverted = drive
            .replace_snapshot(
                Some(&second.revision),
                &before_correction,
                "Revert correction",
                "Owner",
            )
            .unwrap();
        let history = drive.history(None, 10).unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].revision, reverted.revision);
        assert_eq!(history[0].parents, vec![second.revision]);
        assert!(drive
            .read_file("preferences.md", None)
            .unwrap()
            .contains("Uses Rust."));
    }

    #[test]
    fn snapshots_reject_unchecked_symlinks_executables_and_secrets() {
        let (_temp, drive) = fixture();
        let initial = drive.initialize().unwrap().revision.unwrap();
        // Simulate future transport candidates WITHOUT publishing their refs.
        // The snapshot validator must inspect every blob, not trust file names.
        let memory = drive
            .git(
                &["hash-object", "-w", "--stdin"],
                Some(b"# Memory\n\n## Index\n"),
                None,
                None,
            )
            .unwrap();
        let memory = String::from_utf8(memory).unwrap().trim().to_string();
        for (mode, content) in [
            ("120000", "../../outside"),
            ("100755", "# Executable\n"),
            ("100644", "# Note\n- Uses (sk-abcdefgh123456). [source: imported:unknown; added: 2026-10-06; id: a]\n"),
        ] {
            let blob = drive.git(&["hash-object", "-w", "--stdin"], Some(content.as_bytes()), None, None).unwrap();
            let blob = String::from_utf8(blob).unwrap().trim().to_string();
            let tree_input = format!("100644 blob {memory}\tMEMORY.md\n{mode} blob {blob}\tnote.md\n");
            let tree = drive.git(&["mktree"], Some(tree_input.as_bytes()), None, None).unwrap();
            let tree = String::from_utf8(tree).unwrap().trim().to_string();
            let oid = drive.git(&["commit-tree", &tree, "-p", &initial], Some(b"Unchecked candidate\n"), None, Some("Test")).unwrap();
            let oid = String::from_utf8(oid).unwrap().trim().to_string();
            assert!(drive.snapshot(Some(&oid)).is_err());
            assert_eq!(drive.head().unwrap().as_deref(), Some(initial.as_str()));
        }
    }

    #[test]
    fn same_id_retry_does_not_duplicate_and_artifacts_are_cleaned() {
        let (_temp, drive) = fixture();
        let base = drive.initialize().unwrap().revision.unwrap();
        let ops = [add("a", "Uses Rust.")];
        drive
            .apply_batch(Some(&base), &ops, "Remember", "Agent A")
            .unwrap();
        assert!(matches!(
            drive.apply_batch(Some(&base), &ops, "Remember", "Agent B"),
            Err(DriveError::Conflict { .. })
        ));
        let latest = drive.head().unwrap();
        assert!(
            !drive
                .apply_batch(latest.as_deref(), &ops, "Retry", "Agent B")
                .unwrap()
                .changed
        );
        assert_eq!(
            drive
                .read_file("preferences.md", None)
                .unwrap()
                .lines()
                .filter(|l| l.starts_with("- "))
                .count(),
            1
        );
        assert!(!fs::read_dir(&drive.repo).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("memory-index-")));
        assert!(!fs::read_dir(drive.repo.parent().unwrap())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("memory-init-")));
    }
    #[cfg(unix)]
    #[test]
    fn symlink_repository_paths_are_rejected() {
        use std::os::unix::fs::symlink;
        let (temp, _drive) = fixture();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir(root.join("real")).unwrap();
        symlink(root.join("real"), root.join("alias")).unwrap();
        assert!(MemoryDrive::new(root.join("alias/memory.git"), "user-a", "main").is_err());
    }
}
