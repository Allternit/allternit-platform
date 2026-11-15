//! Sealed Chrome profiles for the Meet bot Google accounts
//! (docs/meetings/BOT-ACCOUNTS.md).
//!
//! A person signs each account in by hand (scripts/meetings/bot-profile-signin.sh);
//! this store keeps the resulting profile archive (`tar.zst`) encrypted at
//! rest on the cloud-api host and hands a decrypted copy to each Meet pod.
//!
//! ```text
//! $MEETINGS_BOT_PROFILE_DIR (default /var/lib/allternit/bot-profiles)/<account>/
//!     gen-<n>.tar.zst.enc     current generation
//!     gen-<n-1>.tar.zst.enc   previous generation (rollback); older ones are deleted
//!     state.json              { status, gen, last_ok_at, last_signed_out_at, last_sealed_at }
//!     .lock                   per-account lock (std File::lock, so the ops
//!                             binary and cloud-api serialize on it)
//! ```
//!
//! Encryption: ChaCha20-Poly1305 (`ring`; no XChaCha crate is in the
//! workspace) under `MEETINGS_BOT_PROFILE_KEY` (base64 of 32 random bytes,
//! its own key, not `ALLTERNIT_CREDENTIALS_KEY`). Each seal draws a fresh
//! random 96-bit nonce; the account email is the AAD, so a file copied into
//! another account's directory does not open. File layout:
//! `MAGIC (8) || nonce (12) || ciphertext || tag (16)`.
//!
//! Every write is a temp file + fsync + rename, under the account lock.
//! Directories are 0700 and files 0600.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

pub const KEY_ENV: &str = "MEETINGS_BOT_PROFILE_KEY";
pub const DIR_ENV: &str = "MEETINGS_BOT_PROFILE_DIR";
pub const DEFAULT_DIR: &str = "/var/lib/allternit/bot-profiles";
/// A pod's profile is saved back at most this often per account.
pub const WRITEBACK_EVERY: chrono::Duration = chrono::Duration::hours(24);
const MAGIC: &[u8; 8] = b"ALTBPv1\n";
const STATE_FILE: &str = "state.json";
/// Generations kept on disk: the current one and the one before it.
const KEEP_GENERATIONS: u64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid bot account name {0:?}")]
    InvalidAccount(String),
    #[error("{KEY_ENV} is not set")]
    NoKey,
    #[error("{KEY_ENV} must be base64 of exactly 32 bytes")]
    BadKey,
    #[error("profile for {0} has no sealed generation")]
    NotSealed(String),
    #[error("sealed profile does not open (wrong key, wrong account or corrupted)")]
    OpenFailed,
    #[error("profile store io: {0}")]
    Io(#[from] std::io::Error),
    #[error("profile state.json is unreadable: {0}")]
    State(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Active,
    NeedsReauth,
    Suspended,
}

impl ProfileStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "needs_reauth" => Some(Self::NeedsReauth),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsReauth => "needs_reauth",
            Self::Suspended => "suspended",
        }
    }
}

/// `state.json`. `gen` 0 means nothing sealed yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileState {
    pub status: ProfileStatus,
    pub gen: u64,
    #[serde(default)]
    pub last_ok_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_signed_out_at: Option<DateTime<Utc>>,
    /// When the current generation was sealed (sign-in or write-back).
    #[serde(default)]
    pub last_sealed_at: Option<DateTime<Utc>>,
}

impl ProfileState {
    fn empty() -> Self {
        Self {
            status: ProfileStatus::NeedsReauth,
            gen: 0,
            last_ok_at: None,
            last_signed_out_at: None,
            last_sealed_at: None,
        }
    }

    /// A pod may save this account's profile back as a new generation:
    /// active, sealed, and the current generation is a day old or more.
    pub fn writeback_due(&self, now: DateTime<Utc>) -> bool {
        self.status == ProfileStatus::Active
            && self.gen > 0
            && self
                .last_sealed_at
                .is_none_or(|at| now - at >= WRITEBACK_EVERY)
    }
}

/// Account names are directory names: an email-like token, no path parts.
pub fn validate_account(account: &str) -> Result<(), ProfileError> {
    let bad = || ProfileError::InvalidAccount(account.to_string());
    if account.is_empty()
        || account.len() > 254
        || account.starts_with('.')
        || account.contains("..")
    {
        return Err(bad());
    }
    if !account
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '@'))
    {
        return Err(bad());
    }
    if account.matches('@').count() != 1 || account.starts_with('@') || account.ends_with('@') {
        return Err(bad());
    }
    Ok(())
}

/// The profile key. Only ever built from `MEETINGS_BOT_PROFILE_KEY` (or a test value).
pub struct ProfileKey {
    key: LessSafeKey,
}

impl std::fmt::Debug for ProfileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfileKey(..)")
    }
}

impl ProfileKey {
    pub fn from_base64(encoded: &str) -> Result<Self, ProfileError> {
        let raw = STANDARD
            .decode(encoded.trim())
            .map_err(|_| ProfileError::BadKey)?;
        if raw.len() != 32 {
            return Err(ProfileError::BadKey);
        }
        let unbound =
            UnboundKey::new(&CHACHA20_POLY1305, &raw).map_err(|_| ProfileError::BadKey)?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
        })
    }

    /// `Ok(None)` when the variable is unset or empty.
    pub fn from_env() -> Result<Option<Self>, ProfileError> {
        match std::env::var(KEY_ENV).ok().filter(|v| !v.trim().is_empty()) {
            Some(value) => Self::from_base64(&value).map(Some),
            None => Ok(None),
        }
    }

    pub fn seal(&self, account: &str, plaintext: &[u8]) -> Result<Vec<u8>, ProfileError> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| std::io::Error::other("system RNG failed"))?;
        let mut out = Vec::with_capacity(
            MAGIC.len() + NONCE_LEN + plaintext.len() + CHACHA20_POLY1305.tag_len(),
        );
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        let body_at = out.len();
        out.extend_from_slice(plaintext);
        let tag = self
            .key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(account.as_bytes()),
                &mut out[body_at..],
            )
            .map_err(|_| std::io::Error::other("seal failed"))?;
        out.extend_from_slice(tag.as_ref());
        Ok(out)
    }

    pub fn open(&self, account: &str, mut sealed: Vec<u8>) -> Result<Vec<u8>, ProfileError> {
        let head = MAGIC.len() + NONCE_LEN;
        if sealed.len() < head + CHACHA20_POLY1305.tag_len() || &sealed[..MAGIC.len()] != MAGIC {
            return Err(ProfileError::OpenFailed);
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&sealed[MAGIC.len()..head]);
        let len = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(account.as_bytes()),
                &mut sealed[head..],
            )
            .map_err(|_| ProfileError::OpenFailed)?
            .len();
        sealed.drain(..head);
        sealed.truncate(len);
        Ok(sealed)
    }
}

/// Held for the life of one store operation on one account.
struct AccountLock(File);

impl Drop for AccountLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The store. Cheap to clone. All methods are blocking file IO; async
/// callers go through `tokio::task::spawn_blocking`.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    dir: PathBuf,
    key: Option<Arc<ProfileKey>>,
}

impl ProfileStore {
    pub fn new(dir: impl Into<PathBuf>, key: Option<ProfileKey>) -> Self {
        Self {
            dir: dir.into(),
            key: key.map(Arc::new),
        }
    }

    /// From env. A malformed key is logged and treated as unset, so Meet
    /// pods fall back to joining signed out instead of cloud-api failing.
    pub fn from_env() -> Self {
        let dir = std::env::var(DIR_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_DIR.to_string());
        let key = match ProfileKey::from_env() {
            Ok(key) => key,
            Err(error) => {
                tracing::error!(%error, "meetings: {KEY_ENV} is malformed; bot profiles are not delivered");
                None
            }
        };
        Self::new(dir, key)
    }

    /// Process-wide store from env (cloud-api routes and the launcher).
    pub fn shared() -> Self {
        static STORE: OnceLock<ProfileStore> = OnceLock::new();
        STORE.get_or_init(Self::from_env).clone()
    }

    pub fn has_key(&self) -> bool {
        self.key.is_some()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn key(&self) -> Result<&ProfileKey, ProfileError> {
        self.key.as_deref().ok_or(ProfileError::NoKey)
    }

    fn account_dir(&self, account: &str) -> Result<PathBuf, ProfileError> {
        validate_account(account)?;
        Ok(self.dir.join(account))
    }

    fn gen_path(dir: &Path, gen: u64) -> PathBuf {
        dir.join(format!("gen-{gen}.tar.zst.enc"))
    }

    fn lock(&self, account: &str) -> Result<(PathBuf, AccountLock), ProfileError> {
        let dir = self.account_dir(account)?;
        create_private_dir(&self.dir)?;
        create_private_dir(&dir)?;
        let file = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(".lock"))?;
        file.lock()?;
        Ok((dir, AccountLock(file)))
    }

    fn read_state_at(dir: &Path) -> Result<Option<ProfileState>, ProfileError> {
        match fs::read(dir.join(STATE_FILE)) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write_state_at(dir: &Path, state: &ProfileState) -> Result<(), ProfileError> {
        write_atomic(&dir.join(STATE_FILE), &serde_json::to_vec_pretty(state)?)
    }

    /// `state.json`, or `None` for an account never sealed or marked.
    pub fn state(&self, account: &str) -> Result<Option<ProfileState>, ProfileError> {
        Self::read_state_at(&self.account_dir(account)?)
    }

    /// Accounts with a directory in the store, sorted.
    pub fn accounts(&self) -> Result<Vec<String>, ProfileError> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if entry.file_type()?.is_dir() && validate_account(name).is_ok() {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// A pod can get this account's profile: the key is set, the account is
    /// `active`, and its current generation is on disk.
    pub fn deliverable(&self, account: &str) -> bool {
        if self.key.is_none() {
            return false;
        }
        let Ok(dir) = self.account_dir(account) else {
            return false;
        };
        match Self::read_state_at(&dir) {
            Ok(Some(state)) => {
                state.status == ProfileStatus::Active
                    && state.gen > 0
                    && Self::gen_path(&dir, state.gen).is_file()
            }
            _ => false,
        }
    }

    /// Decrypt the current generation (the `tar.zst` archive).
    pub fn open_current(&self, account: &str) -> Result<Vec<u8>, ProfileError> {
        let key = self.key()?;
        let (dir, _lock) = self.lock(account)?;
        let state = Self::read_state_at(&dir)?
            .filter(|s| s.gen > 0)
            .ok_or_else(|| ProfileError::NotSealed(account.to_string()))?;
        let sealed = fs::read(Self::gen_path(&dir, state.gen))?;
        key.open(account, sealed)
    }

    /// Seal `archive` as the next generation. `mark_active` (the operator's
    /// sign-in) also sets the account `active`; otherwise the status stays.
    /// Returns the new generation.
    pub fn seal(
        &self,
        account: &str,
        archive: &[u8],
        now: DateTime<Utc>,
        mark_active: bool,
    ) -> Result<u64, ProfileError> {
        let key = self.key()?;
        let (dir, _lock) = self.lock(account)?;
        let mut state = Self::read_state_at(&dir)?.unwrap_or_else(ProfileState::empty);
        Self::seal_locked(key, account, &dir, &mut state, archive, now)?;
        if mark_active {
            state.status = ProfileStatus::Active;
        }
        Self::write_state_at(&dir, &state)?;
        Ok(state.gen)
    }

    /// Save a pod's profile back after a clean session, at most once per
    /// [`WRITEBACK_EVERY`] per account (re-checked under the lock).
    /// `Ok(None)` = not due (or no longer active), nothing written.
    pub fn seal_writeback(
        &self,
        account: &str,
        archive: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Option<u64>, ProfileError> {
        let key = self.key()?;
        let (dir, _lock) = self.lock(account)?;
        let Some(mut state) = Self::read_state_at(&dir)? else {
            return Ok(None);
        };
        if !state.writeback_due(now) {
            return Ok(None);
        }
        Self::seal_locked(key, account, &dir, &mut state, archive, now)?;
        state.last_ok_at = Some(now);
        Self::write_state_at(&dir, &state)?;
        Ok(Some(state.gen))
    }

    fn seal_locked(
        key: &ProfileKey,
        account: &str,
        dir: &Path,
        state: &mut ProfileState,
        archive: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), ProfileError> {
        let next = state.gen + 1;
        write_atomic(&Self::gen_path(dir, next), &key.seal(account, archive)?)?;
        state.gen = next;
        state.last_sealed_at = Some(now);
        // Keep exactly the current and previous generations.
        for entry in fs::read_dir(dir)? {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(gen) = name
                .strip_prefix("gen-")
                .and_then(|r| r.strip_suffix(".tar.zst.enc"))
                .and_then(|g| g.parse::<u64>().ok())
            else {
                continue;
            };
            if gen + KEEP_GENERATIONS <= next {
                let _ = fs::remove_file(dir.join(name));
            }
        }
        Ok(())
    }

    /// Set the status (ops `mark`, or the launcher on `bot_profile_signed_out`).
    pub fn set_status(
        &self,
        account: &str,
        status: ProfileStatus,
        now: DateTime<Utc>,
    ) -> Result<ProfileState, ProfileError> {
        let (dir, _lock) = self.lock(account)?;
        let mut state = Self::read_state_at(&dir)?.unwrap_or_else(ProfileState::empty);
        state.status = status;
        if status == ProfileStatus::NeedsReauth {
            state.last_signed_out_at = Some(now);
        }
        Self::write_state_at(&dir, &state)?;
        Ok(state)
    }

    /// A session ran live on this account's profile.
    pub fn record_ok(&self, account: &str, now: DateTime<Utc>) -> Result<(), ProfileError> {
        let (dir, _lock) = self.lock(account)?;
        if let Some(mut state) = Self::read_state_at(&dir)? {
            state.last_ok_at = Some(now);
            Self::write_state_at(&dir, &state)?;
        }
        Ok(())
    }

    /// Generations on disk for an account, ascending (ops `status`).
    pub fn generations(&self, account: &str) -> Result<Vec<u64>, ProfileError> {
        let dir = self.account_dir(account)?;
        let mut gens = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(gens),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            if let Some(gen) = entry?.file_name().to_str().and_then(|n| {
                n.strip_prefix("gen-")?
                    .strip_suffix(".tar.zst.enc")?
                    .parse::<u64>()
                    .ok()
            }) {
                gens.push(gen);
            }
        }
        gens.sort_unstable();
        Ok(gens)
    }
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn create_private_dir(dir: &Path) -> Result<(), ProfileError> {
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(dir)?,
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Temp file in the same directory, fsync, rename over `path`, fsync the dir.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ProfileError> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = private_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        // Best effort: some platforms refuse fsync on a directory handle.
        let _ = File::open(dir).and_then(|d| d.sync_all());
        Ok::<(), std::io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> ProfileKey {
        ProfileKey::from_base64(&STANDARD.encode([byte; 32])).unwrap()
    }

    #[test]
    fn bot_profiles_seal_open_roundtrip_and_wrong_key_or_account_fails() {
        let k = key(7);
        let sealed = k
            .seal("bot1@example.com", b"profile archive bytes")
            .unwrap();
        assert!(
            !sealed.windows(7).any(|w| w == b"archive"),
            "plaintext leaked into the sealed file"
        );
        assert_eq!(
            k.open("bot1@example.com", sealed.clone()).unwrap(),
            b"profile archive bytes"
        );
        assert!(matches!(
            key(8).open("bot1@example.com", sealed.clone()),
            Err(ProfileError::OpenFailed)
        ));
        assert!(
            matches!(
                k.open("bot2@example.com", sealed.clone()),
                Err(ProfileError::OpenFailed)
            ),
            "account is the AAD"
        );
        let mut tampered = sealed;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(matches!(
            k.open("bot1@example.com", tampered),
            Err(ProfileError::OpenFailed)
        ));
        assert!(matches!(
            ProfileKey::from_base64(&STANDARD.encode([1u8; 16])),
            Err(ProfileError::BadKey)
        ));
    }

    #[test]
    fn bot_profiles_account_names_cannot_escape_the_store() {
        for ok in ["gizzi-meet-01@example.com", "first.last+meet@gmail.com"] {
            assert!(validate_account(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../etc@x.com",
            "a/b@x.com",
            "bot@x.com/..",
            ".hidden@x.com",
            "no-at-sign",
            "a@b@c",
            "@x.com",
            "x@",
            "a b@x.com",
            "a\0@x.com",
        ] {
            assert!(validate_account(bad).is_err(), "{bad:?} accepted");
        }
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path(), Some(key(1)));
        assert!(matches!(
            store.seal("../x@y.com", b"a", Utc::now(), true),
            Err(ProfileError::InvalidAccount(_))
        ));
        assert!(
            fs::read_dir(tmp.path()).unwrap().next().is_none(),
            "nothing written for a bad name"
        );
    }

    #[test]
    fn bot_profiles_rotation_keeps_exactly_two_generations() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path(), Some(key(3)));
        let acct = "bot@example.com";
        assert!(!store.deliverable(acct));
        let now = Utc::now();
        for n in 1..=4u8 {
            assert_eq!(store.seal(acct, &[n; 5], now, true).unwrap(), u64::from(n));
        }
        assert_eq!(store.generations(acct).unwrap(), vec![3, 4]);
        assert_eq!(store.open_current(acct).unwrap(), vec![4u8; 5]);
        assert!(store.deliverable(acct));
        // The sealed files never hold the plaintext and are private.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(tmp.path().join(acct).join("gen-4.tar.zst.enc"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Without the key nothing is deliverable.
        assert!(!ProfileStore::new(tmp.path(), None).deliverable(acct));
        store
            .set_status(acct, ProfileStatus::NeedsReauth, now)
            .unwrap();
        assert!(!store.deliverable(acct));
    }

    #[test]
    fn bot_profiles_writeback_at_most_once_per_day() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(tmp.path(), Some(key(4)));
        let acct = "bot@example.com";
        let t0 = Utc::now();
        // Nothing sealed yet: no write-back creates an account.
        assert_eq!(store.seal_writeback(acct, b"x", t0).unwrap(), None);
        store.seal(acct, b"signed-in", t0, true).unwrap();
        assert_eq!(
            store
                .seal_writeback(acct, b"a", t0 + chrono::Duration::hours(23))
                .unwrap(),
            None,
            "sealed under a day ago"
        );
        assert_eq!(
            store
                .seal_writeback(acct, b"b", t0 + chrono::Duration::hours(25))
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            store
                .seal_writeback(acct, b"c", t0 + chrono::Duration::hours(26))
                .unwrap(),
            None,
            "once per day"
        );
        assert_eq!(store.open_current(acct).unwrap(), b"b");
        // A signed-out account is never written back over.
        store
            .set_status(acct, ProfileStatus::NeedsReauth, t0)
            .unwrap();
        assert_eq!(
            store
                .seal_writeback(acct, b"d", t0 + chrono::Duration::hours(80))
                .unwrap(),
            None
        );
        assert_eq!(store.state(acct).unwrap().unwrap().gen, 2);
    }
}
