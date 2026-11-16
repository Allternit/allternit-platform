//! Every v2 error is an `ErrorBody` with the HTTP status from SPEC section 16.
//! `FilesError` (the engine) is mapped here once.

use allternit_files_core::{ErrorBody, ErrorCode, Item};
use allternit_files_engine::FilesError;
use axum::extract::rejection::{BytesRejection, JsonRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// The status for a code, per SPEC section 16. The one table for every v2 route.
pub fn status_for(code: ErrorCode) -> StatusCode {
    use ErrorCode::*;
    let n = match code {
        InvalidRequest | InvalidName | ManifestInvalid | ChunkHashMismatch => 400,
        Unauthorized | PasscodeRequired | PasscodeInvalid => 401,
        Forbidden | FeatureDisabled | ScopeViolation | ProtectedRootLocked | AgentProcessDenied | ExecuteDenied
        | GuestWakeNotAllowed | WakeDenied => 403,
        NotFound | RootNotFound | UploadSessionNotFound => 404,
        AlreadyExists | RevMismatch | WakeConsentRequired | ChunkMissing => 409,
        ShareExpired | ShareRevoked | UploadSessionExpired | CursorExpired => 410,
        PayloadTooLarge => 413,
        IntegrityFailed => 422,
        RateLimited => 429,
        Asleep | Waking | Offline | ReplicaUnavailable | KeyUnavailable => 503,
        QuotaExceeded => 507,
        Unsupported => 501,
        Internal => 500,
    };
    StatusCode::from_u16(n).expect("valid status")
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: status_for(code.clone()),
            body: ErrorBody {
                error: code.clone(),
                code,
                message: message.into(),
                retry_after_s: None,
                computer_state: None,
                wake: None,
                current: None,
                missing_chunks: None,
            },
        }
    }

    /// The engine's error. `current` is the item for `rev_mismatch`, already
    /// turned into a wire item with the caller's permissions.
    pub fn from_files(e: FilesError, current: Option<Item>) -> Self {
        let code = e.code();
        let message = match &e {
            FilesError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => "Not found.".to_string(),
            // Never put local paths or SQL in a message sent to another device.
            FilesError::Io(_) | FilesError::Db(_) | FilesError::Internal(_) => {
                tracing::warn!(error = %e, "files v2: internal error");
                "The Files drive hit an internal error.".to_string()
            }
            other => other.to_string(),
        };
        let mut out = Self::new(code, message);
        if let FilesError::ChunkMissing { missing } = e {
            out.body.missing_chunks = Some(missing);
        }
        out.body.current = current;
        out
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden, msg)
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, msg)
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, msg)
    }
    pub fn internal() -> Self {
        Self::new(ErrorCode::Internal, "The Files drive hit an internal error.")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

fn too_large_or(status: StatusCode, text: String) -> ApiError {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::new(ErrorCode::PayloadTooLarge, "The request body is too large for this route.")
    } else {
        ApiError::invalid(text)
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        too_large_or(r.status(), r.body_text())
    }
}

impl From<BytesRejection> for ApiError {
    fn from(r: BytesRejection) -> Self {
        too_large_or(r.status(), r.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(r: QueryRejection) -> Self {
        ApiError::invalid(r.body_text())
    }
}
