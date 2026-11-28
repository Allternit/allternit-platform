//! Kernel errors (CONTRACT.md 2.0). The same struct is the wire `error` object.
use crate::types::{ErrorCode, Id};
use alloc::string::String;
use core::fmt;

/// A failed call. `receipt_id` is set for every code except `bad_request`, `frame_too_large`
/// and `unsupported_version` (a denial still writes a receipt).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct AuthorityError {
    pub code: ErrorCode,
    pub message: String,
    pub receipt_id: Option<Id>,
}

impl AuthorityError {
    pub fn new(code: ErrorCode, message: impl Into<String>, receipt_id: Option<Id>) -> Self {
        Self {
            code,
            message: message.into(),
            receipt_id,
        }
    }

    /// An error with no receipt (`bad_request` and the other two receipt-less codes).
    pub fn without_receipt(code: ErrorCode, message: impl Into<String>) -> Self {
        Self::new(code, message, None)
    }

    /// Fail closed: the caller treats this as DENY and no tool runs.
    pub fn kernel_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::KernelUnavailable, message, None)
    }

    /// True when the contract requires a receipt for this code but none is attached.
    pub fn missing_required_receipt(&self) -> bool {
        self.code.has_receipt() && self.receipt_id.is_none()
    }
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for AuthorityError {}
