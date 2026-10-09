//! Errors for the artifacts routes: the standard `ApiError` JSON shape
//! (`{error, message, code}`), plus contract-specific codes such as
//! `stale_version` (409, with `current_version`), `link_not_allowed` (422)
//! and `body_too_large` (413).

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};

use crate::ApiError;

#[derive(Debug)]
pub enum ArtifactError {
    Api(ApiError),
    Coded {
        status: StatusCode,
        code: &'static str,
        message: String,
        extra: Map<String, Value>,
    },
}

impl ArtifactError {
    pub fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ArtifactError::Coded { status, code, message: message.into(), extra: Map::new() }
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        if let ArtifactError::Coded { extra, .. } = &mut self {
            extra.insert(key.to_string(), value);
        }
        self
    }

    /// 404 without revealing whether the artifact exists.
    pub fn not_found() -> Self {
        ArtifactError::Api(ApiError::NotFound("Artifact not found".to_string()))
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        ArtifactError::Api(ApiError::Forbidden(message.into()))
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        ArtifactError::Api(ApiError::BadRequest(message.into()))
    }

    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }
}

impl From<ApiError> for ArtifactError {
    fn from(error: ApiError) -> Self {
        ArtifactError::Api(error)
    }
}

impl From<sqlx::Error> for ArtifactError {
    fn from(error: sqlx::Error) -> Self {
        ArtifactError::Api(ApiError::DatabaseError(error))
    }
}

impl From<super::sharing::RuleError> for ArtifactError {
    fn from(error: super::sharing::RuleError) -> Self {
        Self::unprocessable(error.code, error.message)
    }
}

impl IntoResponse for ArtifactError {
    fn into_response(self) -> Response {
        match self {
            ArtifactError::Api(error) => error.into_response(),
            ArtifactError::Coded { status, code, message, extra } => {
                let mut body = json!({ "error": code, "message": message, "code": code });
                if let Value::Object(map) = &mut body {
                    map.extend(extra);
                }
                (status, Json(body)).into_response()
            }
        }
    }
}
