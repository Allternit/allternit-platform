//! `PlatformError`: the `/v1` error envelope.
//!
//! Every non-2xx response from the Platform API is
//! `{"error":{"type","code","message","param"}}`. `type` is one of
//! `invalid_request_error`, `authentication_error`, `permission_error`,
//! `not_found_error`, `rate_limit_error`, `conflict_error`, `billing_error`,
//! `api_error`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct PlatformError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub code: String,
    pub message: String,
    pub param: Option<String>,
    /// Where to fix it (`payment_method_required`: the console billing page).
    /// Sent as `error.url` only when set.
    pub url: Option<String>,
}

impl PlatformError {
    fn new(status: StatusCode, kind: &'static str, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            code: code.to_string(),
            message: message.into(),
            param: None,
            url: None,
        }
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn with_param(mut self, param: &str) -> Self {
        self.param = Some(param.to_string());
        self
    }

    pub fn invalid_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", code, message)
    }

    pub fn authentication(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "authentication_error", code, message)
    }

    pub fn permission(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "permission_error", code, message)
    }

    pub fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found_error", code, message)
    }

    pub fn rate_limit(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limit_error", code, message)
    }

    pub fn conflict(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict_error", code, message)
    }

    /// 402 `billing_error`: the project can't take on more billable work (spend cap).
    pub fn payment_required(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYMENT_REQUIRED, "billing_error", code, message)
    }

    /// 503 `api_error`: try again shortly.
    pub fn service_unavailable(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "api_error", code, message)
    }

    pub fn api_error(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "api_error", code, message)
    }

    /// 404 `platform_api_disabled`: the whole `/v1` namespace is inert until
    /// `ALLTERNIT_PLATFORM_API=1`.
    pub fn disabled() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found_error",
            "platform_api_disabled",
            "The Allternit Platform API is not enabled on this deployment.",
        )
    }
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl IntoResponse for PlatformError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "error": {
                "type": self.kind,
                "code": self.code,
                "message": self.message,
                "param": self.param,
            }
        });
        if let Some(url) = self.url {
            body["error"]["url"] = json!(url);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for PlatformError {
    fn from(error: sqlx::Error) -> Self {
        // Never leak SQL text to a developer; log it for us.
        tracing::error!(%error, "platform api database error");
        Self::api_error("internal_error", "An internal error occurred. Please retry.")
    }
}

impl From<ApiError> for PlatformError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::BadRequest(m) | ApiError::ValidationError(m) => {
                Self::invalid_request("invalid_request", m)
            }
            ApiError::NotFound(m) => Self::not_found("not_found", m),
            ApiError::Forbidden(m) => Self::permission("forbidden", m),
            ApiError::Conflict(m) => Self::conflict("conflict", m),
            ApiError::Unauthorized(m)
            | ApiError::InvalidCredentials(m)
            | ApiError::InvalidToken(m)
            | ApiError::TokenExpired(m) => Self::authentication("unauthorized", m),
            ApiError::TooManyRequests(m) => Self::rate_limit("rate_limited", m),
            other => {
                tracing::error!(error = %other, "platform api internal error");
                Self::api_error("internal_error", "An internal error occurred. Please retry.")
            }
        }
    }
}
