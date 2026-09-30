//! Scoped bearer identities for the CommRails bridge.
//!
//! A token file (JSON, mode 0600) maps the SHA-256 of each bearer token to an
//! actor id (`bot:<slug>` / `agent:<slug>`) and a scope set. Tokens are
//! printed once at creation and never stored in clear. The file is re-read on
//! every authentication, so `identity revoke` takes effect on the next request
//! without restarting the listener.
//!
//! Only [`GRANTABLE_SCOPES`] can ever be held by a remote identity. Execution
//! and approval scopes (`wih:pickup`, `wih:close`, `lease:*`,
//! `wait-gate:resolve`, `gate:*`) are refused at `identity add` and stripped
//! at load time if someone hand-edits the file to include them.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Env override for the identities file location.
pub const IDENTITIES_ENV: &str = "ALLTERNIT_COMMRAILS_BRIDGE_IDENTITIES";

/// Prefix of every bridge bearer token (lets secret scanners spot leaks).
pub const TOKEN_PREFIX: &str = "crb_";

/// A scope a remote bridge identity may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    PlanCreate,
    PlanRead,
    MailSend,
    MailRead,
    TemplateInstantiate,
}

/// Every scope a remote identity can be granted. Nothing else, ever.
pub const GRANTABLE_SCOPES: [Scope; 5] = [
    Scope::PlanCreate,
    Scope::PlanRead,
    Scope::MailSend,
    Scope::MailRead,
    Scope::TemplateInstantiate,
];

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::PlanCreate => "plan:create",
            Scope::PlanRead => "plan:read",
            Scope::MailSend => "mail:send",
            Scope::MailRead => "mail:read",
            Scope::TemplateInstantiate => "template:instantiate",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        GRANTABLE_SCOPES.iter().copied().find(|g| g.as_str() == s)
    }
}

impl Serialize for Scope {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// True for scopes that are never grantable to a remote identity: execution
/// (`wih:pickup`, `wih:close`), locks (`lease:*`), and approvals
/// (`wait-gate:resolve`, `gate:*`).
pub fn is_forbidden_scope(s: &str) -> bool {
    matches!(
        s,
        "wih:pickup" | "wih:close" | "wait-gate:resolve" | "plan:refine"
    ) || s == "lease"
        || s.starts_with("lease:")
        || s == "gate"
        || s.starts_with("gate:")
}

/// Parse a user-supplied scope list for `identity add`. Forbidden and unknown
/// scopes are hard errors (never silently dropped at grant time).
pub fn parse_grant_scopes(raw: &[String]) -> Result<Vec<Scope>> {
    let mut out = Vec::new();
    for item in raw.iter().flat_map(|s| s.split(',')) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if is_forbidden_scope(item) {
            bail!(
                "scope {item:?} is never grantable to a remote identity \
                 (execution, lease, and approval scopes stay local)"
            );
        }
        match Scope::parse(item) {
            Some(scope) => {
                if !out.contains(&scope) {
                    out.push(scope);
                }
            }
            None => bail!(
                "unknown scope {item:?}; grantable scopes: {}",
                GRANTABLE_SCOPES
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    if out.is_empty() {
        bail!("at least one scope is required");
    }
    out.sort();
    Ok(out)
}

/// Validate a remote actor id: `bot:<slug>` or `agent:<slug>`, slug
/// `[a-z0-9][a-z0-9._-]{0,63}`. `user:` actors are refused — a remote
/// identity never speaks as a human approver.
pub fn validate_actor(actor: &str) -> Result<()> {
    let slug = actor
        .strip_prefix("bot:")
        .or_else(|| actor.strip_prefix("agent:"))
        .with_context(|| format!("actor {actor:?} must be bot:<slug> or agent:<slug>"))?;
    let mut chars = slug.chars();
    let first_ok = chars
        .next()
        .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .unwrap_or(false);
    let rest_ok = chars
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-');
    if !first_ok || !rest_ok || slug.len() > 64 {
        bail!("actor slug {slug:?} must match [a-z0-9][a-z0-9._-]{{0,63}}");
    }
    Ok(())
}

/// One identity record as stored on disk (hash only, never the token).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRecord {
    pub id: String,
    pub actor: String,
    /// Raw scope strings as stored. Only grantable ones are effective.
    pub scopes: Vec<String>,
    /// Hex SHA-256 of the bearer token.
    pub token_sha256: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl IdentityRecord {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }

    /// Effective scopes: grantable scopes only. A hand-edited forbidden scope
    /// (`wih:pickup`, `lease:*`, ...) is ignored here, so it can never
    /// authorize anything.
    pub fn effective_scopes(&self) -> Vec<Scope> {
        let mut out: Vec<Scope> = self
            .scopes
            .iter()
            .filter(|s| !is_forbidden_scope(s))
            .filter_map(|s| Scope::parse(s))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IdentityFile {
    #[serde(default = "file_version")]
    version: u32,
    #[serde(default)]
    identities: Vec<IdentityRecord>,
}

fn file_version() -> u32 {
    1
}

/// An authenticated caller.
#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub id: String,
    pub actor: String,
    pub scopes: Vec<Scope>,
}

impl Identity {
    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn scope_strings(&self) -> Vec<&'static str> {
        self.scopes.iter().map(|s| s.as_str()).collect()
    }
}

/// Result of `identity add`: the only time the clear token exists.
#[derive(Debug, Clone)]
pub struct IssuedIdentity {
    pub record: IdentityRecord,
    pub token: String,
}

/// File-backed identity store.
#[derive(Debug, Clone)]
pub struct IdentityStore {
    path: PathBuf,
}

/// Default identities file: `$ALLTERNIT_COMMRAILS_BRIDGE_IDENTITIES`, else
/// `~/.allternit/commrails-bridge/identities.json`. Kept outside any
/// workspace root so it is never swept into a repo.
pub fn default_identities_path() -> PathBuf {
    if let Some(p) = std::env::var_os(IDENTITIES_ENV) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".allternit/commrails-bridge/identities.json")
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Constant-time equality over two equal-length hex digests.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

impl IdentityStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Refuse to use an identities file other users can read or write.
    #[cfg(unix)]
    fn check_permissions(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&self.path)
            .with_context(|| format!("stat {}", self.path.display()))?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            bail!(
                "identities file {} has mode {:o}; it must be 0600 (chmod 600 it)",
                self.path.display(),
                mode
            );
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn check_permissions(&self) -> Result<()> {
        Ok(())
    }

    fn load(&self) -> Result<IdentityFile> {
        if !self.path.exists() {
            return Ok(IdentityFile {
                version: 1,
                identities: Vec::new(),
            });
        }
        self.check_permissions()?;
        let raw = std::fs::read_to_string(&self.path)
            .with_context(|| format!("read {}", self.path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parse {}", self.path.display()))
    }

    fn save(&self, file: &IdentityFile) -> Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Tighten only a dir we own and that is not a shared parent like
            // $HOME: the dedicated default dir, or any dir named for us.
            if parent.ends_with("commrails-bridge") {
                let _ = std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let tmp = parent.join(format!(
            ".{}.tmp-{}",
            self.path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("identities.json"),
            random_hex(4)
        ));
        let body = serde_json::to_string_pretty(file)?;
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts
                .open(&tmp)
                .with_context(|| format!("create {}", tmp.display()))?;
            f.write_all(body.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("replace {}", self.path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Mint a new identity. Returns the clear token exactly once.
    pub fn add(
        &self,
        actor: &str,
        scopes: &[Scope],
        note: Option<String>,
    ) -> Result<IssuedIdentity> {
        validate_actor(actor)?;
        if scopes.is_empty() {
            bail!("at least one scope is required");
        }
        let mut file = self.load()?;
        let token = format!("{TOKEN_PREFIX}{}", random_hex(32));
        let record = IdentityRecord {
            id: format!("bid_{}", random_hex(6)),
            actor: actor.to_string(),
            scopes: scopes.iter().map(|s| s.as_str().to_string()).collect(),
            token_sha256: hash_token(&token),
            created_at: Utc::now().to_rfc3339(),
            revoked_at: None,
            note,
        };
        file.identities.push(record.clone());
        self.save(&file)?;
        Ok(IssuedIdentity { record, token })
    }

    pub fn list(&self) -> Result<Vec<IdentityRecord>> {
        Ok(self.load()?.identities)
    }

    /// Number of active (non-revoked) identities with at least one effective
    /// scope.
    pub fn active_count(&self) -> Result<usize> {
        Ok(self
            .list()?
            .iter()
            .filter(|r| r.is_active() && !r.effective_scopes().is_empty())
            .count())
    }

    /// Revoke by identity id and/or every active identity of `actor`.
    /// Returns the revoked records.
    pub fn revoke(&self, id: Option<&str>, actor: Option<&str>) -> Result<Vec<IdentityRecord>> {
        if id.is_none() && actor.is_none() {
            bail!("revoke needs --id or --actor");
        }
        let mut file = self.load()?;
        let now = Utc::now().to_rfc3339();
        let mut revoked = Vec::new();
        for rec in file.identities.iter_mut() {
            if !rec.is_active() {
                continue;
            }
            let hit = id.map(|i| rec.id == i).unwrap_or(false)
                || actor.map(|a| rec.actor == a).unwrap_or(false);
            if hit {
                rec.revoked_at = Some(now.clone());
                revoked.push(rec.clone());
            }
        }
        if !revoked.is_empty() {
            self.save(&file)?;
        }
        Ok(revoked)
    }

    /// Authenticate a presented bearer token. Reads the file each call so a
    /// revoke is effective immediately. `Ok(None)` = unknown or revoked.
    pub fn authenticate(&self, token: &str) -> Result<Option<Identity>> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 256 {
            return Ok(None);
        }
        let presented = hash_token(token);
        let file = self.load()?;
        let mut found = None;
        // Compare against every record (no early exit) to keep timing flat.
        for rec in &file.identities {
            if ct_eq(&rec.token_sha256, &presented) && rec.is_active() && found.is_none() {
                found = Some(Identity {
                    id: rec.id.clone(),
                    actor: rec.actor.clone(),
                    scopes: rec.effective_scopes(),
                });
            }
        }
        // An identity whose only scopes were forbidden/unknown authenticates
        // but can do nothing; keep it so the 403 names the real reason.
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_scopes_are_refused_at_grant() {
        for s in [
            "wih:pickup",
            "wih:close",
            "lease:request",
            "lease:*",
            "wait-gate:resolve",
            "gate:check",
            "gate:*",
        ] {
            assert!(
                parse_grant_scopes(&[s.to_string()]).is_err(),
                "{s} must be refused"
            );
        }
        assert!(parse_grant_scopes(&["plan:bogus".to_string()]).is_err());
        let ok =
            parse_grant_scopes(&["plan:create,plan:read".to_string(), "mail:send".to_string()])
                .unwrap();
        assert_eq!(
            ok,
            vec![Scope::PlanCreate, Scope::PlanRead, Scope::MailSend]
        );
    }

    #[test]
    fn actor_validation() {
        assert!(validate_actor("bot:chief").is_ok());
        assert!(validate_actor("agent:raven-1").is_ok());
        assert!(validate_actor("user:joe").is_err());
        assert!(validate_actor("bot:").is_err());
        assert!(validate_actor("bot:Chief").is_err());
        assert!(validate_actor("chief").is_err());
    }

    #[test]
    fn add_authenticate_revoke_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStore::new(dir.path().join("ids.json"));
        let issued = store
            .add("bot:chief", &[Scope::PlanCreate, Scope::PlanRead], None)
            .unwrap();
        assert!(issued.token.starts_with(TOKEN_PREFIX));
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert!(
            !raw.contains(&issued.token),
            "clear token must not be stored"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let who = store.authenticate(&issued.token).unwrap().unwrap();
        assert_eq!(who.actor, "bot:chief");
        assert!(store.authenticate("crb_nope").unwrap().is_none());
        assert_eq!(store.revoke(None, Some("bot:chief")).unwrap().len(), 1);
        assert!(store.authenticate(&issued.token).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn loose_permissions_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStore::new(dir.path().join("ids.json"));
        let issued = store.add("bot:chief", &[Scope::PlanRead], None).unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.authenticate(&issued.token).is_err());
    }
}
