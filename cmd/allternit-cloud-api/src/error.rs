//! API error types

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

/// API error response
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    pub code: String,
}

/// API error types
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Deployment not found: {0}")]
    DeploymentNotFound(String),

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Provider not found: {0}")]
    ProviderNotFound(String),

    #[error("Run not found: {0}")]
    NotFound(String),

    #[error("Invalid credentials: {0}")]
    InvalidCredentials(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Payment required — e.g. a credits transfer the wallet cannot cover.
    #[error("Payment required: {0}")]
    PaymentRequired(String),

    #[error("Token expired: {0}")]
    TokenExpired(String),

    #[error("Invalid token: {0}")]
    InvalidToken(String),

    #[error("Deployment failed: {0}")]
    DeploymentFailed(String),

    #[error("SSH error: {0}")]
    SshError(String),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Migration error: {0}")]
    MigrationError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Bad request: {0}")]
    BadRequest(String),

    /// Deliberate "the caller must do something first" signal — e.g. no
    /// data-plane node is registered for this user, so control-plane
    /// proxying cannot proceed until they pair a device. 428 (not 404):
    /// nothing is missing from the URL, the account is incomplete.
    #[error("Precondition required: {0}")]
    PreconditionRequired(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    /// A per-user limit was hit (e.g. free computer wakes per hour). 429.
    #[error("Too many requests: {0}")]
    TooManyRequests(String),

    /// The request conflicts with the account's state (e.g. a paid account
    /// asking for a free computer). 409.
    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

impl From<sqlx::migrate::MigrateError> for ApiError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        ApiError::MigrationError(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Check if we're in production mode (don't expose internal details)
        let is_production = std::env::var("Allternit_API_DEVELOPMENT_MODE")
            .map(|v| v != "true" && v != "1")
            .unwrap_or(true);

        // Log the real error detail server-side before any production sanitization
        // so server logs show the root cause instead of a generic message.
        // so fly.io logs show the root cause instead of a generic message.
        warn!(error = %self, "ApiError response");

        let (status, error_code, message) = match &self {
            ApiError::DeploymentNotFound(id) => {
                (StatusCode::NOT_FOUND, "DEPLOYMENT_NOT_FOUND", id.clone())
            }
            ApiError::InstanceNotFound(id) => {
                (StatusCode::NOT_FOUND, "INSTANCE_NOT_FOUND", id.clone())
            }
            ApiError::ProviderNotFound(id) => {
                (StatusCode::NOT_FOUND, "PROVIDER_NOT_FOUND", id.clone())
            }
            ApiError::NotFound(id) => (StatusCode::NOT_FOUND, "NOT_FOUND", id.clone()),
            ApiError::InvalidCredentials(_) => (
                StatusCode::UNAUTHORIZED,
                "INVALID_CREDENTIALS",
                "Invalid credentials".to_string(),
            ),
            ApiError::Unauthorized(_) => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                "Unauthorized".to_string(),
            ),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, "FORBIDDEN", msg.clone()),
            ApiError::PaymentRequired(msg) => (
                StatusCode::PAYMENT_REQUIRED,
                "PAYMENT_REQUIRED",
                msg.clone(),
            ),
            ApiError::TokenExpired(_) => (
                StatusCode::UNAUTHORIZED,
                "TOKEN_EXPIRED",
                "Token has expired".to_string(),
            ),
            ApiError::InvalidToken(_) => (
                StatusCode::UNAUTHORIZED,
                "INVALID_TOKEN",
                "Invalid token".to_string(),
            ),
            ApiError::DeploymentFailed(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DEPLOYMENT_FAILED",
                msg.clone(),
            ),
            ApiError::SshError(_) if is_production => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONNECTION_ERROR",
                "Connection failed".to_string(),
            ),
            ApiError::SshError(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "SSH_ERROR", msg.clone())
            }
            ApiError::DatabaseError(_) if is_production => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_ERROR",
                "Database error".to_string(),
            ),
            ApiError::DatabaseError(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_ERROR",
                e.to_string(),
            ),
            ApiError::MigrationError(_) if is_production => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "MIGRATION_ERROR",
                "Migration error".to_string(),
            ),
            ApiError::MigrationError(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "MIGRATION_ERROR",
                msg.clone(),
            ),
            ApiError::SerializationError(_) => (
                StatusCode::BAD_REQUEST,
                "SERIALIZATION_ERROR",
                "Invalid JSON".to_string(),
            ),
            ApiError::IoError(_) if is_production => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "IO_ERROR",
                "I/O error".to_string(),
            ),
            ApiError::IoError(e) => (StatusCode::INTERNAL_SERVER_ERROR, "IO_ERROR", e.to_string()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg.clone()),
            ApiError::TooManyRequests(msg) => (
                StatusCode::TOO_MANY_REQUESTS,
                "RATE_LIMITED",
                msg.clone(),
            ),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "CONFLICT", msg.clone()),
            ApiError::PreconditionRequired(msg) => (
                StatusCode::PRECONDITION_REQUIRED,
                "PRECONDITION_REQUIRED",
                msg.clone(),
            ),
            ApiError::ValidationError(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_ERROR",
                msg.clone(),
            ),
            ApiError::ServiceUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVICE_UNAVAILABLE",
                msg.clone(),
            ),
            ApiError::Internal(_) if is_production => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Internal server error".to_string(),
            ),
            ApiError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                msg.clone(),
            ),
            ApiError::NotImplemented(msg) => (
                StatusCode::NOT_IMPLEMENTED,
                "NOT_IMPLEMENTED",
                msg.clone(),
            ),
        };

        // Log the unsanitized error detail so production logs contain the real
        // failure reason even when the public response is generic.
        if status.is_server_error() {
            tracing::error!(
                error = %self,
                error_code,
                "API error response"
            );
        }

        let body = Json(ErrorResponse {
            error: error_code.to_string(),
            message,
            code: error_code.to_string(),
        });

        (status, body).into_response()
    }
}
