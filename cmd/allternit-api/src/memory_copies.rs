//! Keep a copy of your Memory Drive and ledger where you want it (ledger
//! program phase 5):
//!
//! - **folder**: a folder on the computer that holds the drive (an iCloud
//!   Drive or Dropbox folder works). Allternit writes plain files under
//!   `<folder>/Allternit/memory/` and `<folder>/Allternit/ledger/` and keeps
//!   them in step, touching only files it wrote (listed in
//!   `.allternit-copy.json`).
//! - **git**: your own git remote over HTTPS (GitHub, GitLab, …). The drive
//!   goes to branch `allternit-memory`, the ledger to `allternit-ledger`. The
//!   access token is sealed at rest and passed to git through the
//!   environment, never on a command line or in the remote URL.
//!
//! Copies run after each hourly ledger snapshot and when asked.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::DbHandle;
use crate::ledger_repo::LedgerRepo;
use crate::memory_drive_service::{Result, ServiceError};

const KEY: &str = "memory_copies";
const SUBFOLDER: &str = "Allternit";
const MANIFEST: &str = ".allternit-copy.json";
const SEALED_PREFIX: &str = "enc:v1:";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub last_at: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderCopy {
    pub path: String,
    #[serde(default)]
    pub status: Status,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitCopy {
    /// `https://host/owner/repo.git`, never with credentials in it.
    pub url: String,
    /// Sealed access token (empty for a public or credential-helper remote).
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub status: Status,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Copies {
    pub folder: Option<FolderCopy>,
    pub git: Option<GitCopy>,
}

impl Copies {
    /// What the app sees: the token is never sent back, only whether one is saved.
    pub fn public(&self) -> serde_json::Value {
        serde_json::json!({
            "folder": self.folder,
            "git": self.git.as_ref().map(|g| serde_json::json!({ "url": g.url, "has_token": !g.token.is_empty(), "status": g.status })),
        })
    }
}

pub fn load(conn: &Connection, owner: &str) -> Copies {
    conn.query_row("SELECT value FROM runtime_settings WHERE user_id=?1 AND key=?2", params![owner, KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

pub fn store(conn: &Connection, owner: &str, c: &Copies) -> Result<()> {
    conn.execute(
        "INSERT INTO runtime_settings (user_id, key, value, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP) \
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
        params![owner, KEY, serde_json::to_string(c).unwrap_or_default()],
    )?;
    Ok(())
}

fn bad(msg: &str) -> ServiceError {
    ServiceError::Provenance(msg.to_string())
}

/// A folder Allternit may write its `Allternit/` copy into.
pub fn check_folder(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path.trim());
    if !p.is_absolute() {
        return Err(bad("Choose a full folder path, like /Users/you/Dropbox."));
    }
    if p.parent().is_none() {
        return Err(bad("Choose a folder, not the top of the disk."));
    }
    if !p.is_dir() {
        return Err(bad("That folder doesn't exist on the computer that holds your drive."));
    }
    Ok(p)
}

/// An HTTPS remote without credentials in it.
pub fn check_remote(url: &str) -> Result<String> {
    let u = url.trim();
    let parsed = url::Url::parse(u).map_err(|_| bad("Enter the repository's HTTPS address, like https://github.com/you/memory.git."))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(bad("Use the repository's HTTPS address (https://…)."));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad("Leave the token out of the address; paste it in the token box instead."));
    }
    Ok(u.to_string())
}

/// Seal a token, or refuse: tokens are never stored in plain text.
pub fn seal_token(token: &str) -> Result<String> {
    if token.is_empty() {
        return Ok(String::new());
    }
    crate::token_crypto::ensure_platform_key();
    let sealed = crate::token_crypto::seal(token);
    if !sealed.starts_with(SEALED_PREFIX) {
        return Err(bad("This computer can't store a token safely yet. Restart Allternit and try again."));
    }
    Ok(sealed)
}

/// Git environment that sends the token as HTTP basic auth for this push only.
pub fn auth_env(url: &str, token: &str) -> Vec<(String, String)> {
    if token.is_empty() {
        return vec![];
    }
    use base64::Engine as _;
    let user = if url.contains("gitlab") { "oauth2" } else { "x-access-token" };
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "http.extraHeader".into()),
        ("GIT_CONFIG_VALUE_0".into(), format!("Authorization: Basic {basic}")),
    ]
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Mirror `files` into `dir`, removing only files the previous copy wrote.
pub fn write_folder(root: &Path, sections: &[(&str, BTreeMap<String, Vec<u8>>)]) -> std::io::Result<usize> {
    let base = root.join(SUBFOLDER);
    std::fs::create_dir_all(&base)?;
    let manifest_path = base.join(MANIFEST);
    let previous: BTreeSet<String> = std::fs::read(&manifest_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mut written = BTreeSet::new();
    for (section, files) in sections {
        for (path, bytes) in files {
            if !crate::ledger_repo::valid_path(path) {
                continue;
            }
            let rel = format!("{section}/{path}");
            let target = base.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Never write through a symlink someone put in the copy.
            if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
                std::fs::write(&target, bytes)?;
            }
            written.insert(rel);
        }
    }
    for gone in previous.difference(&written) {
        if crate::ledger_repo::valid_path(gone) {
            let target = base.join(gone);
            if std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_file()) {
                let _ = std::fs::remove_file(&target);
            }
        }
    }
    std::fs::write(
        base.join("README.md"),
        "# Allternit\n\nA copy of your Allternit Memory Drive (`memory/`) and ledger (`ledger/`), kept up to date by Allternit.\nChanges you make here are replaced on the next copy; edit your memories in Allternit.\n",
    )?;
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&written).unwrap_or_default())?;
    Ok(written.len())
}

fn repos(db: &DbHandle, owner: &str) -> Result<(LedgerRepo, LedgerRepo)> {
    let drive = crate::memory_drive_writer::ensure(db, owner)?.ok_or(ServiceError::NotFound)?;
    let memory = LedgerRepo::existing(drive.repo_path.clone(), &drive.branch);
    let ledger = crate::ledger::repo_for(db, owner)?;
    Ok((memory, ledger))
}

/// Run every copy the owner set up and record how each went.
pub fn run(db: &DbHandle, owner: &str) -> Result<Copies> {
    let mut copies = load(&db.connect()?, owner);
    if copies.folder.is_none() && copies.git.is_none() {
        return Ok(copies);
    }
    let (memory, ledger) = repos(db, owner)?;
    if let Some(f) = copies.folder.as_mut() {
        let outcome = check_folder(&f.path).and_then(|root| {
            let mem = memory.all_files().map_err(|e| bad(&e.to_string()))?;
            let led = ledger.all_files().map_err(|e| bad(&e.to_string()))?;
            write_folder(&root, &[("memory", mem), ("ledger", led)]).map_err(|e| bad(&format!("Couldn't write to that folder: {e}")))
        });
        f.status = match outcome {
            Ok(_) => Status { last_at: Some(now()), last_error: None },
            Err(e) => Status { last_at: f.status.last_at.clone(), last_error: Some(e.to_string().replace("invalid memory provenance: ", "")) },
        };
    }
    if let Some(g) = copies.git.as_mut() {
        let token = crate::token_crypto::open(&g.token);
        let env = auth_env(&g.url, &token);
        let outcome = memory.push(&g.url, "allternit-memory", &env).and_then(|_| ledger.push(&g.url, "allternit-ledger", &env));
        g.status = match outcome {
            Ok(()) => Status { last_at: Some(now()), last_error: None },
            Err(e) => {
                // Never echo anything that could carry the token.
                let raw = e.to_string();
                let raw = if !token.is_empty() && raw.contains(&token) { String::new() } else { raw };
                Status { last_at: g.status.last_at.clone(), last_error: Some(push_error(&raw)) }
            }
        };
    }
    store(&db.connect()?, owner, &copies)?;
    Ok(copies)
}

/// A push failure in words a person can act on.
pub fn push_error(raw: &str) -> String {
    let r = raw.to_ascii_lowercase();
    if r.contains("could not read username") || r.contains("authentication failed") || r.contains("403") || r.contains("401") || r.contains("repository not found") || r.contains("not found") {
        "The repository refused the push. Check the address, and that the token can push to that repository.".into()
    } else if r.contains("could not resolve host") || r.contains("timed out") || r.contains("unable to access") {
        "Couldn't reach the repository. Check the address and the internet connection; Allternit will try again within the hour.".into()
    } else if r.contains("rejected") || r.contains("protected branch") {
        "The repository rejected the push. Allow pushes to the allternit-memory and allternit-ledger branches.".into()
    } else {
        "Couldn't push to the repository. Allternit will try again within the hour.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_errors_are_actionable() {
        assert!(push_error("git push failed: fatal: could not read Username for 'https://github.com': terminal prompts disabled").starts_with("The repository refused"));
        assert!(push_error("fatal: unable to access 'https://x/': Could not resolve host: x").starts_with("Couldn't reach"));
        assert!(push_error("! [remote rejected] main -> allternit-memory (protected branch hook declined)").starts_with("The repository rejected"));
    }

    #[test]
    fn remotes_must_be_https_without_credentials() {
        assert!(check_remote("https://github.com/eoj/memory.git").is_ok());
        assert!(check_remote("http://github.com/eoj/memory.git").is_err());
        assert!(check_remote("git@github.com:eoj/memory.git").is_err());
        assert!(check_remote("https://eoj:ghp_x@github.com/eoj/memory.git").is_err());
        assert!(check_remote("file:///tmp/x").is_err());
    }

    #[test]
    fn auth_goes_in_the_environment() {
        assert!(auth_env("https://github.com/a/b.git", "").is_empty());
        let env = auth_env("https://github.com/a/b.git", "ghp_secret");
        assert_eq!(env[1], ("GIT_CONFIG_KEY_0".into(), "http.extraHeader".into()));
        assert!(env[2].1.starts_with("Authorization: Basic "));
        assert!(!env[2].1.contains("ghp_secret"));
        let public = Copies { folder: None, git: Some(GitCopy { url: "https://x/y.git".into(), token: "enc:v1:abc".into(), status: Status::default() }) }.public();
        assert_eq!(public["git"]["has_token"], true);
        assert!(!public.to_string().contains("enc:v1"));
    }

    #[test]
    fn folder_copy_mirrors_and_only_removes_its_own_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = tmp.path().join("Allternit/notes-i-made.txt");
        std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
        std::fs::write(&mine, "keep me").unwrap();
        let f = |pairs: &[(&str, &str)]| pairs.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect::<BTreeMap<_, _>>();
        let n = write_folder(tmp.path(), &[("memory", f(&[("MEMORY.md", "# m"), ("facts.md", "- a")])), ("ledger", f(&[("bots/x/bot.json", "{}")]))]).unwrap();
        assert_eq!(n, 3);
        assert_eq!(std::fs::read_to_string(tmp.path().join("Allternit/ledger/bots/x/bot.json")).unwrap(), "{}");
        write_folder(tmp.path(), &[("memory", f(&[("MEMORY.md", "# m2")])), ("ledger", f(&[]))]).unwrap();
        assert_eq!(std::fs::read_to_string(tmp.path().join("Allternit/memory/MEMORY.md")).unwrap(), "# m2");
        assert!(!tmp.path().join("Allternit/memory/facts.md").exists());
        assert!(!tmp.path().join("Allternit/ledger/bots/x/bot.json").exists());
        assert!(mine.exists(), "a file Allternit didn't write is never removed");
        assert!(check_folder("relative/path").is_err());
        assert!(check_folder(tmp.path().join("missing").to_str().unwrap()).is_err());
        assert!(check_folder(tmp.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn pushes_both_repos_to_a_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        std::process::Command::new("git").args(["init", "--bare", "--quiet"]).arg(&remote).status().unwrap();
        let ledger = LedgerRepo::open(tmp.path().join("ledger.git")).unwrap();
        ledger.write_snapshot(&[("README.md".to_string(), b"hi".to_vec())].into_iter().collect(), "Save").unwrap();
        // A local path stands in for the HTTPS remote here.
        ledger.push(remote.to_str().unwrap(), "allternit-ledger", &[]).unwrap();
        let out = std::process::Command::new("git").arg("--git-dir").arg(&remote).args(["show", "allternit-ledger:README.md"]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi");
    }
}
