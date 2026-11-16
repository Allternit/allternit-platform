//! Engine errors, mapped 1:1 onto the contract's `ErrorCode` (SPEC section 16).

use allternit_files_core::ErrorCode;

use crate::model::ItemRecord;

pub type Result<T, E = FilesError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("root not found: {0}")]
    RootNotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Stale `base_rev`. Carries the item as it is now (`ErrorBody.current`).
    #[error("base_rev is stale (current rev {})", .current.rev())]
    RevMismatch { current: Box<ItemRecord> },
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("scope violation: {0}")]
    ScopeViolation(String),
    #[error("upload session not found")]
    UploadSessionNotFound,
    #[error("upload session expired")]
    UploadSessionExpired,
    /// Commit with chunks never sent (`ErrorBody.missing_chunks`).
    #[error("{} chunk(s) missing", .missing.len())]
    ChunkMissing { missing: Vec<String> },
    #[error("chunk hash mismatch: {0}")]
    ChunkHashMismatch(String),
    #[error("manifest invalid: {0}")]
    ManifestInvalid(String),
    #[error("cursor expired")]
    CursorExpired,
    #[error("integrity failed: {0}")]
    IntegrityFailed(String),
    #[error("key unavailable: {0}")]
    KeyUnavailable(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("internal: {0}")]
    Internal(String),
}

impl FilesError {
    pub fn code(&self) -> ErrorCode {
        use FilesError::*;
        match self {
            NotFound(_) => ErrorCode::NotFound,
            RootNotFound(_) => ErrorCode::RootNotFound,
            AlreadyExists(_) => ErrorCode::AlreadyExists,
            InvalidName(_) => ErrorCode::InvalidName,
            InvalidRequest(_) => ErrorCode::InvalidRequest,
            RevMismatch { .. } => ErrorCode::RevMismatch,
            PayloadTooLarge(_) => ErrorCode::PayloadTooLarge,
            Forbidden(_) => ErrorCode::Forbidden,
            ScopeViolation(_) => ErrorCode::ScopeViolation,
            UploadSessionNotFound => ErrorCode::UploadSessionNotFound,
            UploadSessionExpired => ErrorCode::UploadSessionExpired,
            ChunkMissing { .. } => ErrorCode::ChunkMissing,
            ChunkHashMismatch(_) => ErrorCode::ChunkHashMismatch,
            ManifestInvalid(_) => ErrorCode::ManifestInvalid,
            CursorExpired => ErrorCode::CursorExpired,
            IntegrityFailed(_) => ErrorCode::IntegrityFailed,
            KeyUnavailable(_) => ErrorCode::KeyUnavailable,
            Io(e) if e.kind() == std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            Io(_) | Db(_) | Internal(_) => ErrorCode::Internal,
        }
    }

    /// HTTP status per SPEC section 16.
    pub fn http_status(&self) -> u16 {
        match self.code() {
            ErrorCode::InvalidRequest
            | ErrorCode::InvalidName
            | ErrorCode::ManifestInvalid
            | ErrorCode::ChunkHashMismatch => 400,
            ErrorCode::Forbidden | ErrorCode::ScopeViolation => 403,
            ErrorCode::NotFound | ErrorCode::RootNotFound | ErrorCode::UploadSessionNotFound => 404,
            ErrorCode::AlreadyExists | ErrorCode::RevMismatch | ErrorCode::ChunkMissing => 409,
            ErrorCode::UploadSessionExpired | ErrorCode::CursorExpired => 410,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::IntegrityFailed => 422,
            ErrorCode::KeyUnavailable => 503,
            _ => 500,
        }
    }
}
