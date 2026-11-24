//! What the mount frontends need from a computer's Files drive. `HttpBackend`
//! (`http.rs`) implements it over `/api/v1/files/v2`; tests can use their own.

use std::fmt;
use std::path::Path;

use allternit_files_core::{Item, OnConflict, Root};

/// A failed Files call, kept close to the contract's `ErrorBody`.
#[derive(Debug, Clone)]
pub struct MountError {
    /// The contract `ErrorCode` (`not_found`, `wake_consent_required`, ...), or
    /// `offline` when the computer could not be reached.
    pub code: String,
    /// HTTP status (0 when nothing answered).
    pub status: u16,
    pub message: String,
    pub retry_after_s: Option<u64>,
    /// `chunk_missing`: hashes still needed.
    pub missing_chunks: Vec<String>,
}

impl MountError {
    pub fn new(code: &str, status: u16, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), status, message: message.into(), retry_after_s: None, missing_chunks: Vec::new() }
    }

    pub fn not_found() -> Self {
        Self::new("not_found", 404, "That file or folder is gone.")
    }

    pub fn read_only() -> Self {
        Self::new("forbidden", 403, "This mount is read-only.")
    }

    pub fn io(e: std::io::Error) -> Self {
        Self::new("internal", 500, format!("local file: {e}"))
    }

    /// The HTTP status a WebDAV client should see.
    pub fn http_status(&self) -> u16 {
        match self.code.as_str() {
            "not_found" | "root_not_found" | "upload_session_not_found" => 404,
            "already_exists" => 405,
            "rev_mismatch" | "chunk_missing" => 409,
            "unauthorized" | "forbidden" | "feature_disabled" | "scope_violation" | "protected_root_locked"
            | "agent_process_denied" | "execute_denied" | "wake_denied" => 403,
            "invalid_name" | "invalid_request" => 400,
            "payload_too_large" => 413,
            "quota_exceeded" => 507,
            "asleep" | "waking" | "offline" | "wake_consent_required" | "replica_unavailable" => 503,
            "unsupported" => 501,
            _ => 502,
        }
    }

    /// The errno a FUSE caller should see.
    #[cfg(target_os = "linux")]
    pub fn errno(&self) -> i32 {
        match self.code.as_str() {
            "not_found" | "root_not_found" => libc::ENOENT,
            "already_exists" => libc::EEXIST,
            "unauthorized" | "forbidden" | "feature_disabled" | "scope_violation" | "protected_root_locked"
            | "agent_process_denied" | "execute_denied" | "wake_denied" => libc::EACCES,
            "rev_mismatch" => libc::ESTALE,
            "invalid_name" => libc::EINVAL,
            "payload_too_large" => libc::EFBIG,
            "quota_exceeded" => libc::ENOSPC,
            // Asleep and no consent to wake: the host is down, not the file.
            "asleep" | "waking" | "wake_consent_required" => libc::EHOSTDOWN,
            "offline" | "replica_unavailable" => libc::ENETUNREACH,
            "unsupported" => libc::ENOTSUP,
            _ => libc::EIO,
        }
    }
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}): {}", self.code, self.status, self.message)
    }
}

impl std::error::Error for MountError {}

pub type Result<T> = std::result::Result<T, MountError>;

/// One computer's Files drive, blocking calls. Every write carries the rev the
/// caller last saw, so a change made elsewhere lands as a conflict copy (or
/// `rev_mismatch`), never as a silent overwrite.
pub trait Backend: Send + Sync + 'static {
    fn roots(&self) -> Result<Vec<Root>>;
    /// Every item in a folder (all pages).
    fn list(&self, root_id: &str, parent_id: &str) -> Result<Vec<Item>>;
    /// `len` bytes from `offset` (fewer at the end of the file).
    fn read(&self, item: &Item, offset: u64, len: u64) -> Result<Vec<u8>>;
    /// Write the local file `src` as `name` in `parent_id`, replacing `replace`
    /// (at its rev) when given.
    fn write(&self, root_id: &str, parent_id: &str, name: &str, replace: Option<&Item>, on_conflict: OnConflict, src: &Path) -> Result<Item>;
    fn mkdir(&self, parent_id: &str, name: &str) -> Result<Item>;
    /// Rename and/or move. `on_conflict: replace` gives POSIX rename semantics.
    fn rename(&self, item: &Item, new_parent_id: Option<&str>, new_name: Option<&str>, on_conflict: OnConflict) -> Result<Item>;
    fn copy(&self, item: &Item, dest_parent_id: &str, new_name: Option<&str>, on_conflict: OnConflict) -> Result<Item>;
    /// To trash (the root's policy decides how long it stays).
    fn delete(&self, item: &Item) -> Result<()>;
}
