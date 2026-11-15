//! Phase 5 batch metadata storage, provider execution, and polling.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::{db::DbHandle, AppState};

use super::{
    auth::LlmKeyContext,
    translate::{validate_request, ChatCompletionRequest, OpenAiErrorResponse},
};

/// Native batch creation body (`requests` array).
#[derive(Debug, Deserialize)]
pub struct CreateBatchRequest {
    pub requests: Vec<Value>,
}

/// OpenAI-compatible batch creation body.
#[derive(Debug, Deserialize)]
pub struct OpenAiCreateBatchRequest {
    pub input_file_id: String,
    pub endpoint: String,
    pub completion_window: String,
    #[serde(default)]
    pub metadata: Option<Value>,
}

impl OpenAiCreateBatchRequest {
    /// Convert the OpenAI request into the native `requests` array by reading
    /// and parsing the input file. Returns an OpenAI-shaped 400 on any
    /// validation failure.
    pub fn into_requests(self) -> Result<Vec<Value>, OpenAiErrorResponse> {
        if self.endpoint != "/v1/chat/completions" {
            return Err(OpenAiErrorResponse::invalid_request(
                "`endpoint` must be `/v1/chat/completions`.",
                Some("endpoint"),
            ));
        }
        if self.completion_window != "24h" {
            return Err(OpenAiErrorResponse::invalid_request(
                "`completion_window` must be `24h`.",
                Some("completion_window"),
            ));
        }
        load_batch_input_file(&self.input_file_id)
    }
}

/// Internal batch record. This is the native shape persisted to SQLite; the
/// OpenAI wire shape is built separately via [`OpenAiBatch::from`].
#[derive(Debug, Clone, Serialize)]
pub struct Batch {
    pub id: String,
    pub object: &'static str,
    pub status: String,
    pub request_count: usize,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

pub struct BatchesService {
    db: DbHandle,
}

impl BatchesService {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    pub fn create(&self, key: &LlmKeyContext, requests: &[Value]) -> rusqlite::Result<Batch> {
        let id = format!("batch_{}", uuid::Uuid::new_v4().simple());
        let requests_json = serde_json::to_string(requests)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        let conn = self.db.connect()?;
        conn.execute(
            "INSERT INTO llm_batches
             (id, virtual_key_id, user_id, tenant_id, requests_json, status, endpoint)
             VALUES (?1, ?2, ?3, ?4, ?5, 'validating', '/v1/chat/completions')",
            params![id, key.key_id, key.user_id, key.tenant_id, requests_json],
        )?;
        self.get(&key.key_id, &id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn create_openai(
        &self,
        key: &LlmKeyContext,
        requests: &[Value],
        input_file_id: &str,
        endpoint: &str,
        completion_window: &str,
        metadata: Option<Value>,
    ) -> rusqlite::Result<Batch> {
        let id = format!("batch_{}", uuid::Uuid::new_v4().simple());
        let requests_json = serde_json::to_string(requests)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        let metadata_json = metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        let conn = self.db.connect()?;
        conn.execute(
            "INSERT INTO llm_batches
             (id, virtual_key_id, user_id, tenant_id, requests_json, status,
              endpoint, input_file_id, completion_window, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, 'validating', ?6, ?7, ?8, ?9)",
            params![
                id,
                key.key_id,
                key.user_id,
                key.tenant_id,
                requests_json,
                endpoint,
                input_file_id,
                completion_window,
                metadata_json,
            ],
        )?;
        self.get(&key.key_id, &id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn list(&self, key_id: &str) -> rusqlite::Result<Vec<Batch>> {
        let conn = self.db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id, status, requests_json, created_at, updated_at, cancelled_at,
                    endpoint, input_file_id, completion_window, metadata
             FROM llm_batches WHERE virtual_key_id = ?1 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([key_id], batch_from_row)?;
        rows.collect()
    }

    pub fn get(&self, key_id: &str, id: &str) -> rusqlite::Result<Option<Batch>> {
        let conn = self.db.connect()?;
        conn.query_row(
            "SELECT id, status, requests_json, created_at, updated_at, cancelled_at,
                    endpoint, input_file_id, completion_window, metadata
             FROM llm_batches WHERE virtual_key_id = ?1 AND id = ?2",
            params![key_id, id],
            batch_from_row,
        )
        .optional()
    }

    pub fn cancel(&self, key_id: &str, id: &str) -> rusqlite::Result<Option<Batch>> {
        let conn = self.db.connect()?;
        conn.execute(
            "UPDATE llm_batches SET status = 'cancelled', cancelled_at = CURRENT_TIMESTAMP,
                    updated_at = CURRENT_TIMESTAMP
             WHERE virtual_key_id = ?1 AND id = ?2 AND status NOT IN ('cancelled', 'completed', 'failed')",
            params![key_id, id],
        )?;
        drop(conn);
        self.get(key_id, id)
    }

    pub fn results(&self, key_id: &str, id: &str) -> rusqlite::Result<Option<Value>> {
        let conn = self.db.connect()?;
        conn.query_row(
            "SELECT results_json FROM llm_batches WHERE virtual_key_id = ?1 AND id = ?2",
            params![key_id, id],
            |row| {
                let raw: String = row.get(0)?;
                serde_json::from_str(&raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            },
        )
        .optional()
    }
}

fn batch_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Batch> {
    let requests_json: String = row.get(2)?;
    let request_count = serde_json::from_str::<Vec<Value>>(&requests_json)
        .map(|requests| requests.len())
        .unwrap_or(0);
    Ok(Batch {
        id: row.get(0)?,
        object: "batch",
        status: row.get(1)?,
        request_count,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        cancelled_at: row.get(5)?,
        endpoint: row.get(6)?,
        input_file_id: row.get(7)?,
        completion_window: row.get(8)?,
        metadata: row
            .get::<_, Option<String>>(9)?
            .and_then(|raw| serde_json::from_str(&raw).ok()),
    })
}

/// Directory where batch input files are stored. Mirrors the uploads layout.
fn batch_inputs_dir() -> PathBuf {
    std::env::var("ALLTERNIT_DATA_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::data_dir().map(|d| d.join("allternit")))
        .unwrap_or_else(|| PathBuf::from("/var/lib/allternit"))
        .join("batch_inputs")
}

/// Read a batch input file (JSONL) and map the OpenAI `body` entries to the
/// native request array. Plain request objects are also accepted as a fallback.
fn load_batch_input_file(input_file_id: &str) -> Result<Vec<Value>, OpenAiErrorResponse> {
    load_batch_input_file_from(&batch_inputs_dir(), input_file_id)
}

fn load_batch_input_file_from(
    base: &std::path::Path,
    input_file_id: &str,
) -> Result<Vec<Value>, OpenAiErrorResponse> {
    if input_file_id.is_empty() || input_file_id.contains('/') || input_file_id.contains('\\') {
        return Err(OpenAiErrorResponse::invalid_request(
            "`input_file_id` is invalid.",
            Some("input_file_id"),
        ));
    }
    let path = base.join(input_file_id);
    let content = std::fs::read_to_string(&path).map_err(|err| {
        OpenAiErrorResponse::invalid_request(
            format!("Unable to read input file `{input_file_id}`: {err}"),
            Some("input_file_id"),
        )
    })?;

    let mut requests = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|err| {
            OpenAiErrorResponse::invalid_request(
                format!("Invalid JSON on line {}: {err}", index + 1),
                Some("input_file_id"),
            )
        })?;
        let body = value
            .get("body")
            .cloned()
            .unwrap_or_else(|| value.clone());
        requests.push(body);
    }

    if requests.is_empty() {
        return Err(OpenAiErrorResponse::invalid_request(
            "Input file contains no requests.",
            Some("input_file_id"),
        ));
    }
    Ok(requests)
}

/// OpenAI batch object returned by create/list/get/cancel.
#[derive(Debug, Clone, Serialize)]
pub struct OpenAiBatch {
    pub id: String,
    pub object: &'static str,
    pub status: String,
    pub endpoint: String,
    pub input_file_id: String,
    pub completion_window: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_file_id: Option<String>,
    pub request_counts: OpenAiBatchRequestCounts,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalizing_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelling_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiBatchRequestCounts {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
}

impl From<Batch> for OpenAiBatch {
    fn from(batch: Batch) -> Self {
        let total = batch.request_count;
        let completed = if batch.status == "completed" { total } else { 0 };
        let failed = if batch.status == "failed" { total } else { 0 };
        Self {
            id: batch.id.clone(),
            object: "batch",
            status: batch.status.clone(),
            endpoint: batch.endpoint.unwrap_or_else(|| "/v1/chat/completions".to_string()),
            input_file_id: batch
                .input_file_id
                .unwrap_or_else(|| format!("file_{}", batch.id)),
            completion_window: batch.completion_window.unwrap_or_else(|| "24h".to_string()),
            output_file_id: (batch.status == "completed").then_some(format!("{}_output", batch.id)),
            error_file_id: (batch.status == "failed").then_some(format!("{}_errors", batch.id)),
            request_counts: OpenAiBatchRequestCounts {
                total,
                completed,
                failed,
            },
            created_at: parse_datetime(&batch.created_at).unwrap_or(0),
            expires_at: None,
            finalizing_at: None,
            completed_at: (batch.status == "completed")
                .then(|| parse_datetime(batch.updated_at.as_deref().unwrap_or(&batch.created_at)))
                .flatten(),
            failed_at: (batch.status == "failed")
                .then(|| parse_datetime(batch.updated_at.as_deref().unwrap_or(&batch.created_at)))
                .flatten(),
            expired_at: None,
            cancelling_at: None,
            cancelled_at: batch.cancelled_at.as_deref().and_then(parse_datetime),
            metadata: batch.metadata.clone(),
        }
    }
}

fn parse_datetime(raw: &str) -> Option<i64> {
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

/// Creation body accepted by `POST /v1/batches`. Supports both the native
/// `requests` array and the OpenAI `input_file_id` form.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CreateBatchBody {
    Native(CreateBatchRequest),
    OpenAi(OpenAiCreateBatchRequest),
}

fn internal_error(err: impl std::fmt::Display) -> Response {
    OpenAiErrorResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Internal error: {err}"),
        "server_error",
        None,
        Some(super::translate::error_code::INTERNAL_ERROR),
    )
    .into_response()
}

fn not_found(id: &str) -> Response {
    OpenAiErrorResponse::new(
        StatusCode::NOT_FOUND,
        format!("Batch `{id}` was not found."),
        "invalid_request_error",
        Some("id"),
        Some(super::translate::error_code::INVALID_REQUEST),
    )
    .into_response()
}

pub async fn create_batch(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Json(body): Json<CreateBatchBody>,
) -> Response {
    let (requests, input_file_id, endpoint, completion_window, metadata) = match body {
        CreateBatchBody::Native(native) => {
            if native.requests.is_empty() {
                return OpenAiErrorResponse::invalid_request(
                    "`requests` must contain at least one request.",
                    Some("requests"),
                )
                .into_response();
            }
            (
                native.requests,
                None,
                "/v1/chat/completions".to_string(),
                "24h".to_string(),
                None,
            )
        }
        CreateBatchBody::OpenAi(openai) => {
            let input_file_id = openai.input_file_id.clone();
            let endpoint = openai.endpoint.clone();
            let completion_window = openai.completion_window.clone();
            let metadata = openai.metadata.clone();
            match openai.into_requests() {
                Ok(requests) => (requests, Some(input_file_id), endpoint, completion_window, metadata),
                Err(err) => return err.into_response(),
            }
        }
    };

    for (index, value) in requests.iter().enumerate() {
        let request: ChatCompletionRequest = match serde_json::from_value(value.clone()) {
            Ok(request) => request,
            Err(err) => {
                return OpenAiErrorResponse::invalid_request(
                    format!("Invalid request at index {index}: {err}"),
                    Some("requests"),
                )
                .into_response()
            }
        };
        if let Err(err) = validate_request(&request) {
            return err.into_response();
        }
        if !key.model_allowed(&request.model) {
            return OpenAiErrorResponse::new(
                StatusCode::FORBIDDEN,
                format!(
                    "This API key is not allowed to use model `{}`.",
                    request.model
                ),
                "permission_error",
                Some("model"),
                Some(super::translate::error_code::PERMISSION_DENIED),
            )
            .into_response();
        }
    }

    let result = if let Some(input_file_id) = &input_file_id {
        BatchesService::new(state.db.clone()).create_openai(
            &key,
            &requests,
            input_file_id,
            &endpoint,
            &completion_window,
            metadata,
        )
    } else {
        BatchesService::new(state.db.clone()).create(&key, &requests)
    };

    match result {
        Ok(batch) => {
            let openai_batch = OpenAiBatch::from(batch);
            (StatusCode::CREATED, Json(openai_batch)).into_response()
        }
        Err(err) => internal_error(err),
    }
}

pub async fn list_batches(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
) -> Response {
    match BatchesService::new(state.db.clone()).list(&key.key_id) {
        Ok(data) => {
            let openai_data: Vec<OpenAiBatch> = data.into_iter().map(OpenAiBatch::from).collect();
            Json(json!({ "object": "list", "data": openai_data })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

pub async fn get_batch(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Path(id): Path<String>,
) -> Response {
    match BatchesService::new(state.db.clone()).get(&key.key_id, &id) {
        Ok(Some(batch)) => Json(OpenAiBatch::from(batch)).into_response(),
        Ok(None) => not_found(&id),
        Err(err) => internal_error(err),
    }
}

pub async fn cancel_batch(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Path(id): Path<String>,
) -> Response {
    match BatchesService::new(state.db.clone()).cancel(&key.key_id, &id) {
        Ok(Some(batch)) => Json(OpenAiBatch::from(batch)).into_response(),
        Ok(None) => not_found(&id),
        Err(err) => internal_error(err),
    }
}

pub async fn batch_results(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Path(id): Path<String>,
) -> Response {
    match BatchesService::new(state.db.clone()).results(&key.key_id, &id) {
        Ok(Some(data)) => Json(json!({ "object": "list", "data": data })).into_response(),
        Ok(None) => not_found(&id),
        Err(err) => internal_error(err),
    }
}

// ─── Batch error schema ─────────────────────────────────────────────────────

/// Per-request error object returned in batch results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BatchRequestError {
    pub index: usize,
    pub code: String,
    pub message: String,
}

impl BatchRequestError {
    pub fn new(index: usize, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            index,
            code: code.into(),
            message: message.into(),
        }
    }
}

// ─── Provider abstraction ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum BatchJobStatus {
    Validating,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl BatchJobStatus {
    fn from_wire(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "validating" => BatchJobStatus::Validating,
            "in_progress" | "processing" | "queued" => BatchJobStatus::InProgress,
            "completed" | "done" => BatchJobStatus::Completed,
            "failed" | "error" => BatchJobStatus::Failed,
            "cancelled" | "canceled" => BatchJobStatus::Cancelled,
            _ => BatchJobStatus::InProgress,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderBatchJob {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ProviderBatchStatus {
    pub status: BatchJobStatus,
    pub results: Option<Vec<Value>>,
}

#[derive(Debug, Clone)]
pub struct BatchProviderError {
    pub is_transient: bool,
    pub code: String,
    pub message: String,
}

impl BatchProviderError {
    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            is_transient: true,
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            is_transient: false,
            code: code.into(),
            message: message.into(),
        }
    }
}

#[async_trait::async_trait]
pub trait BatchProvider: Send + Sync {
    /// `batch_id` is the local `llm_batches.id`; providers that execute
    /// through the local gateway (native) forward it so each sub-request's
    /// usage row is attributable to the batch (task G10).
    async fn submit(&self, batch_id: &str, requests: &[Value]) -> Result<ProviderBatchJob, BatchProviderError>;
    async fn poll(&self, provider_batch_id: &str) -> Result<ProviderBatchStatus, BatchProviderError>;
}

// ─── Generic HTTP batch provider ────────────────────────────────────────────

/// A generic HTTP provider that speaks an OpenAI-style `/v1/batches` surface.
pub struct HttpBatchProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl HttpBatchProvider {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            client: crate::net_client::client(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        let base_url = std::env::var("ALLTERNIT_BATCH_PROVIDER_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| config.terminal_server_url());
        let api_key = std::env::var("ALLTERNIT_BATCH_PROVIDER_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self::new(base_url, api_key)
    }
}

#[async_trait::async_trait]
impl BatchProvider for HttpBatchProvider {
    async fn submit(&self, batch_id: &str, requests: &[Value]) -> Result<ProviderBatchJob, BatchProviderError> {
        let url = format!("{}/v1/batches", self.base_url);
        let body = json!({
            "endpoint": "/v1/chat/completions",
            "requests": requests,
        });
        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        // Forward the local batch id so a chained Allternit gateway can meter
        // sub-requests against it (task G10).
        req = req.header("x-allternit-batch-id", batch_id);
        let resp = req.send().await.map_err(|err| {
            if err.is_timeout() || err.is_connect() {
                BatchProviderError::transient("provider_unavailable", err.to_string())
            } else {
                BatchProviderError::transient("provider_error", err.to_string())
            }
        })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_server_error() {
            return Err(BatchProviderError::transient(
                "provider_server_error",
                format!("{status}: {text}"),
            ));
        }
        if !status.is_success() {
            return Err(BatchProviderError::permanent(
                "provider_client_error",
                format!("{status}: {text}"),
            ));
        }

        let value: Value = serde_json::from_str(&text).map_err(|err| {
            BatchProviderError::permanent("provider_decode_error", err.to_string())
        })?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BatchProviderError::permanent("provider_missing_id", "batch id missing from response".to_string())
            })?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("in_progress")
            .to_string();
        Ok(ProviderBatchJob { id: id.to_string(), status })
    }

    async fn poll(&self, provider_batch_id: &str) -> Result<ProviderBatchStatus, BatchProviderError> {
        let url = format!("{}/v1/batches/{}", self.base_url, provider_batch_id);
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        let resp = req.send().await.map_err(|err| {
            if err.is_timeout() || err.is_connect() {
                BatchProviderError::transient("provider_unavailable", err.to_string())
            } else {
                BatchProviderError::transient("provider_error", err.to_string())
            }
        })?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_server_error() {
            return Err(BatchProviderError::transient(
                "provider_server_error",
                format!("{status}: {text}"),
            ));
        }
        if !status.is_success() {
            return Err(BatchProviderError::permanent(
                "provider_client_error",
                format!("{status}: {text}"),
            ));
        }

        let value: Value = serde_json::from_str(&text).map_err(|err| {
            BatchProviderError::permanent("provider_decode_error", err.to_string())
        })?;
        let job_status = value
            .get("status")
            .and_then(Value::as_str)
            .map(BatchJobStatus::from_wire)
            .unwrap_or(BatchJobStatus::InProgress);
        let results = value
            .get("results")
            .and_then(Value::as_array)
            .cloned();
        Ok(ProviderBatchStatus {
            status: job_status,
            results,
        })
    }
}

// ─── Background worker ──────────────────────────────────────────────────────

const WORKER_INTERVAL: Duration = Duration::from_secs(5);
const MAX_RETRIES: usize = 3;

struct PendingBatch {
    id: String,
    status: String,
    requests_json: String,
    provider_batch_id: Option<String>,
    retry_count: usize,
}

pub struct BatchWorker {
    db: DbHandle,
    provider: Arc<dyn BatchProvider>,
}

impl BatchWorker {
    pub fn new(db: DbHandle, provider: Arc<dyn BatchProvider>) -> Self {
        Self { db, provider }
    }

    pub async fn run(self, interval: Duration) {
        let mut interval = tokio::time::interval(interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(err) = self.process_once().await {
                warn!(error = %err, "Batch worker iteration failed");
            }
        }
    }

    async fn process_once(&self) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let batches = tokio::task::spawn_blocking(move || -> rusqlite::Result<Vec<PendingBatch>> {
            let conn = db.connect()?;
            let mut stmt = conn.prepare(
                "SELECT id, status, requests_json, provider_batch_id, retry_count
                 FROM llm_batches
                 WHERE status IN ('validating', 'in_progress')
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(PendingBatch {
                    id: row.get(0)?,
                    status: row.get(1)?,
                    requests_json: row.get(2)?,
                    provider_batch_id: row.get(3)?,
                    retry_count: row.get::<_, i64>(4)? as usize,
                })
            })?;
            rows.collect()
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))??;

        for batch in batches {
            if let Err(err) = self.process_batch(batch).await {
                warn!(error = %err, "Failed to process batch");
            }
        }
        Ok(())
    }

    async fn process_batch(&self, batch: PendingBatch) -> rusqlite::Result<()> {
        let requests: Vec<Value> =
            serde_json::from_str(&batch.requests_json).unwrap_or_default();
        let request_count = requests.len().max(1);

        match batch.status.as_str() {
            "validating" => match self.provider.submit(&batch.id, &requests).await {
                Ok(job) => {
                    if BatchJobStatus::from_wire(&job.status) == BatchJobStatus::Completed {
                        // Some providers complete synchronously; fetch results now.
                        match self.provider.poll(&job.id).await {
                            Ok(poll) if poll.status == BatchJobStatus::Completed => {
                                self.complete(&batch.id, &job.id, poll.results.unwrap_or_default())
                                    .await?;
                            }
                            _ => self.mark_in_progress(&batch.id, &job.id).await?,
                        }
                    } else {
                        self.mark_in_progress(&batch.id, &job.id).await?;
                    }
                }
                Err(err) => {
                    self.handle_provider_error(&batch, request_count, err).await?;
                }
            },
            "in_progress" => {
                if let Some(provider_id) = &batch.provider_batch_id {
                    match self.provider.poll(provider_id).await {
                        Ok(poll) => match poll.status {
                            BatchJobStatus::Completed => {
                                self.complete(
                                    &batch.id,
                                    provider_id,
                                    poll.results.unwrap_or_default(),
                                )
                                .await?;
                            }
                            BatchJobStatus::Failed => {
                                let errors = error_results(
                                    request_count,
                                    "provider_failed",
                                    "Provider reported the batch failed",
                                );
                                self.fail(&batch.id, errors).await?;
                            }
                            BatchJobStatus::Cancelled => self.cancel(&batch.id).await?,
                            _ => {}
                        },
                        Err(err) => {
                            self.handle_provider_error(&batch, request_count, err).await?;
                        }
                    }
                } else {
                    let errors = error_results(
                        request_count,
                        "missing_provider_batch_id",
                        "Batch has no provider batch id",
                    );
                    self.fail(&batch.id, errors).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_provider_error(
        &self,
        batch: &PendingBatch,
        request_count: usize,
        err: BatchProviderError,
    ) -> rusqlite::Result<()> {
        if err.is_transient && batch.retry_count < MAX_RETRIES {
            self.increment_retry(&batch.id).await?;
            info!(batch_id = %batch.id, retry = batch.retry_count + 1, "Transient batch provider error; will retry");
            Ok(())
        } else {
            let errors = error_results(request_count, err.code, err.message);
            self.fail(&batch.id, errors).await
        }
    }

    async fn mark_in_progress(&self, id: &str, provider_batch_id: &str) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let id = id.to_string();
        let provider_batch_id = provider_batch_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "UPDATE llm_batches
                 SET status = 'in_progress', provider_batch_id = ?2,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![id, provider_batch_id],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
    }

    async fn complete(
        &self,
        id: &str,
        provider_batch_id: &str,
        results: Vec<Value>,
    ) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let id = id.to_string();
        let provider_batch_id = provider_batch_id.to_string();
        let results_json = serde_json::to_string(&results)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "UPDATE llm_batches
                 SET status = 'completed', provider_batch_id = ?2,
                     results_json = ?3, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![id, provider_batch_id, results_json],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
    }

    async fn fail(&self, id: &str, errors: Vec<Value>) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let id = id.to_string();
        let errors_json = serde_json::to_string(&errors)
            .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "UPDATE llm_batches
                 SET status = 'failed', results_json = ?2,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![id, errors_json],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
    }

    async fn cancel(&self, id: &str) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "UPDATE llm_batches
                 SET status = 'cancelled', cancelled_at = CURRENT_TIMESTAMP,
                     updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
    }

    async fn increment_retry(&self, id: &str) -> rusqlite::Result<()> {
        let db = self.db.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect()?;
            conn.execute(
                "UPDATE llm_batches
                 SET retry_count = retry_count + 1, updated_at = CURRENT_TIMESTAMP
                 WHERE id = ?1",
                params![id],
            )?;
            Ok(())
        })
        .await
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?
    }
}

/// Native batch provider: executes every request through the local
/// `/v1/chat/completions` endpoint instead of delegating to an external
/// provider. This makes batch jobs first-class citizens of the Allternit
/// gateway and removes the external batch-API dependency.
pub struct NativeBatchProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl NativeBatchProvider {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            client: crate::net_client::client(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub fn from_config(config: &crate::config::AppConfig) -> Self {
        let base_url = format!("http://127.0.0.1:{}", config.api_port());
        let api_key = std::env::var("ALLTERNIT_BATCH_NATIVE_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self::new(base_url, api_key)
    }
}

#[async_trait::async_trait]
impl BatchProvider for NativeBatchProvider {
    async fn submit(&self, batch_id: &str, requests: &[Value]) -> Result<ProviderBatchJob, BatchProviderError> {
        // Native batches complete synchronously; we execute all requests now
        // and store the results for the first poll to return. Every
        // sub-request goes through the local /v1/chat/completions, so each
        // one is metered by the gateway's record_usage_event choke point with
        // batch attribution (task G10) — batch spend is no longer invisible.
        let results = self.run_requests(batch_id, requests).await;
        let id = format!("native_{}", uuid::Uuid::new_v4().simple());
        let results_json = serde_json::to_string(&results)
            .map_err(|err| BatchProviderError::permanent("serialize_error", err.to_string()))?;
        std::fs::write(native_results_path(&id), results_json)
            .map_err(|err| BatchProviderError::permanent("results_write_error", err.to_string()))?;
        Ok(ProviderBatchJob {
            id,
            status: "in_progress".to_string(),
        })
    }

    async fn poll(&self, provider_batch_id: &str) -> Result<ProviderBatchStatus, BatchProviderError> {
        let path = native_results_path(provider_batch_id);
        let text = std::fs::read_to_string(&path)
            .map_err(|err| BatchProviderError::permanent("results_read_error", err.to_string()))?;
        let results: Vec<Value> = serde_json::from_str(&text)
            .map_err(|err| BatchProviderError::permanent("results_decode_error", err.to_string()))?;
        let _ = std::fs::remove_file(&path);
        Ok(ProviderBatchStatus {
            status: BatchJobStatus::Completed,
            results: Some(results),
        })
    }
}

impl NativeBatchProvider {
    async fn run_requests(&self, batch_id: &str, requests: &[Value]) -> Vec<Value> {
        let mut results = Vec::with_capacity(requests.len());
        for (index, body) in requests.iter().enumerate() {
            let mut req = self
                .client
                .post(format!("{}/v1/chat/completions", self.base_url))
                .json(body)
                // Batch attribution: the gateway copies this onto the usage
                // row it records for the sub-request (task G10).
                .header("x-allternit-batch-id", batch_id);
            if let Some(key) = &self.api_key {
                req = req.header("Authorization", format!("Bearer {key}"));
            }
            let value = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    let mut value: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
                        json!({
                            "id": format!("chatcmpl-native-err-{index}"),
                            "object": "chat.completion",
                            "choices": [],
                            "error": { "message": text, "type": "upstream_error" }
                        })
                    });
                    if !status.is_success() {
                        value = json!({
                            "index": index,
                            "error": value,
                            "status": status.as_u16()
                        });
                    }
                    value
                }
                Err(err) => json!({
                    "index": index,
                    "error": { "message": err.to_string(), "type": "request_error" }
                }),
            };
            results.push(value);
        }
        results
    }
}

fn native_results_path(provider_batch_id: &str) -> PathBuf {
    batch_inputs_dir().join(format!("{provider_batch_id}_results.json"))
}

pub fn spawn_batch_worker(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    let provider: Arc<dyn BatchProvider> = if std::env::var("ALLTERNIT_BATCH_NATIVE")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        info!("Starting native batch execution worker");
        Arc::new(NativeBatchProvider::from_config(&state.config))
    } else {
        info!("Starting external batch execution worker");
        Arc::new(HttpBatchProvider::from_config(&state.config))
    };
    let worker = BatchWorker::new(state.db.clone(), provider);
    tokio::spawn(async move {
        worker.run(WORKER_INTERVAL).await;
    })
}

fn error_results(count: usize, code: impl Into<String>, message: impl Into<String>) -> Vec<Value> {
    let code = code.into();
    let message = message.into();
    (0..count)
        .map(|index| {
            json!({
                "index": index,
                "error": {
                    "code": code,
                    "message": message,
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct MockProvider {
        submit_results: Mutex<Vec<Result<ProviderBatchJob, BatchProviderError>>>,
        poll_results: Mutex<Vec<Result<ProviderBatchStatus, BatchProviderError>>>,
        /// Every (batch_id, request_count) pair handed to submit.
        submits: Arc<Mutex<Vec<(String, usize)>>>,
    }

    impl MockProvider {
        fn new(
            submit_results: Vec<Result<ProviderBatchJob, BatchProviderError>>,
            poll_results: Vec<Result<ProviderBatchStatus, BatchProviderError>>,
        ) -> Self {
            Self {
                submit_results: Mutex::new(submit_results),
                poll_results: Mutex::new(poll_results),
                submits: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl BatchProvider for MockProvider {
        async fn submit(&self, batch_id: &str, requests: &[Value]) -> Result<ProviderBatchJob, BatchProviderError> {
            self.submits
                .lock()
                .unwrap()
                .push((batch_id.to_string(), requests.len()));
            self.submit_results.lock().unwrap().remove(0)
        }

        async fn poll(&self, _provider_batch_id: &str) -> Result<ProviderBatchStatus, BatchProviderError> {
            self.poll_results.lock().unwrap().remove(0)
        }
    }

    fn test_db() -> (DbHandle, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::new(dir.path().join("test.db")).unwrap();
        (db, dir)
    }

    fn insert_test_key(db: &DbHandle) -> LlmKeyContext {
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO users (id, email) VALUES ('u1', 'test@example.com')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_virtual_keys
             (id, user_id, key_hash, key_prefix, allowed_models)
             VALUES ('vk1', 'u1', 'hash', 'ak-test', NULL)",
            [],
        )
        .unwrap();
        LlmKeyContext {
            key_id: "vk1".to_string(),
            user_id: "u1".to_string(),
            tenant_id: None,
            key_prefix: "ak-test".to_string(),
            monthly_budget_cents: None,
            rate_limit_rpm: None,
            allowed_models: None,
            tags: None,
        }
    }

    fn sample_requests() -> Vec<Value> {
        vec![
            json!({"model": "allternit-balanced", "messages": [{"role": "user", "content": "Hi"}]}),
            json!({"model": "allternit-balanced", "messages": [{"role": "user", "content": "Bye"}]}),
        ]
    }

    #[tokio::test]
    async fn create_stores_validating_status() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();
        assert_eq!(batch.status, "validating");
    }

    #[tokio::test]
    async fn worker_submits_validating_batch() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Ok(ProviderBatchJob {
                id: "prov_1".to_string(),
                status: "in_progress".to_string(),
            })],
            vec![],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();

        let updated = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(updated.status, "in_progress");
    }

    #[tokio::test]
    async fn worker_polls_to_completed() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Ok(ProviderBatchJob {
                id: "prov_1".to_string(),
                status: "in_progress".to_string(),
            })],
            vec![Ok(ProviderBatchStatus {
                status: BatchJobStatus::Completed,
                results: Some(vec![json!({"index": 0, "response": {"role": "assistant"}})]),
            })],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap(); // submit
        worker.process_once().await.unwrap(); // poll

        let updated = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(updated.status, "completed");
        let results = service.results(&key.key_id, &batch.id).unwrap().unwrap();
        let arr = results.as_array().unwrap();
        assert_eq!(arr[0]["index"], 0);
        assert!(arr[0]["response"].is_object());
    }

    #[tokio::test]
    async fn worker_returns_errors_alongside_outputs() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Ok(ProviderBatchJob {
                id: "prov_1".to_string(),
                status: "in_progress".to_string(),
            })],
            vec![Ok(ProviderBatchStatus {
                status: BatchJobStatus::Completed,
                results: Some(vec![
                    json!({"index": 0, "response": {"role": "assistant"}}),
                    json!({"index": 1, "error": {"code": "rate_limit", "message": "slow down"}}),
                ]),
            })],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();
        worker.process_once().await.unwrap();

        let results = service.results(&key.key_id, &batch.id).unwrap().unwrap();
        let arr = results.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr[0].get("response").is_some());
        assert_eq!(arr[1]["error"]["code"], "rate_limit");
    }

    #[tokio::test]
    async fn worker_retries_transient_errors_then_succeeds() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![
                Err(BatchProviderError::transient("timeout", "provider timeout")),
                Err(BatchProviderError::transient("timeout", "provider timeout")),
                Ok(ProviderBatchJob {
                    id: "prov_1".to_string(),
                    status: "in_progress".to_string(),
                }),
            ],
            vec![Ok(ProviderBatchStatus {
                status: BatchJobStatus::Completed,
                results: Some(vec![json!({"index": 0, "response": {}})]),
            })],
        ));
        let worker = BatchWorker::new(db.clone(), provider);

        worker.process_once().await.unwrap();
        worker.process_once().await.unwrap();
        let after_retries = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(after_retries.status, "validating");

        worker.process_once().await.unwrap();
        let in_progress = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(in_progress.status, "in_progress");

        worker.process_once().await.unwrap();
        let completed = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(completed.status, "completed");
    }

    #[tokio::test]
    async fn worker_fails_after_retry_exhausted() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![
                Err(BatchProviderError::transient("timeout", "provider timeout")),
                Err(BatchProviderError::transient("timeout", "provider timeout")),
                Err(BatchProviderError::transient("timeout", "provider timeout")),
                Err(BatchProviderError::transient("timeout", "provider timeout")),
            ],
            vec![],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();
        worker.process_once().await.unwrap();
        worker.process_once().await.unwrap();
        worker.process_once().await.unwrap();

        let failed = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        let results = service.results(&key.key_id, &batch.id).unwrap().unwrap();
        let arr = results.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["error"]["code"], "timeout");
    }

    #[tokio::test]
    async fn non_transient_error_marks_failed_immediately() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Err(BatchProviderError::permanent(
                "bad_request",
                "invalid batch body",
            ))],
            vec![],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();

        let failed = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(failed.status, "failed");
    }

    #[tokio::test]
    async fn cancel_batch_prevents_worker_processing() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();
        service.cancel(&key.key_id, &batch.id).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Ok(ProviderBatchJob {
                id: "prov_1".to_string(),
                status: "in_progress".to_string(),
            })],
            vec![],
        ));
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();

        let unchanged = service.get(&key.key_id, &batch.id).unwrap().unwrap();
        assert_eq!(unchanged.status, "cancelled");
    }

    #[test]
    fn openai_create_request_validates_endpoint_and_window() {
        let valid = OpenAiCreateBatchRequest {
            input_file_id: "file_1".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            completion_window: "24h".to_string(),
            metadata: None,
        };
        assert!(valid.into_requests().is_err()); // file does not exist

        let bad_endpoint = OpenAiCreateBatchRequest {
            input_file_id: "file_1".to_string(),
            endpoint: "/v1/embeddings".to_string(),
            completion_window: "24h".to_string(),
            metadata: None,
        };
        let err = bad_endpoint.into_requests().unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.error.message.contains("endpoint"));

        let bad_window = OpenAiCreateBatchRequest {
            input_file_id: "file_1".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            completion_window: "7d".to_string(),
            metadata: None,
        };
        let err = bad_window.into_requests().unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.error.message.contains("completion_window"));
    }

    #[test]
    fn openai_input_file_maps_jsonl_bodies_to_requests() {
        let dir = TempDir::new().unwrap();
        let file_id = "batch_input.jsonl";
        let jsonl = r#"{"custom_id":"req-1","method":"POST","url":"/v1/chat/completions","body":{"model":"allternit-balanced","messages":[{"role":"user","content":"Hi"}]}}
{"custom_id":"req-2","method":"POST","url":"/v1/chat/completions","body":{"model":"allternit-balanced","messages":[{"role":"user","content":"Bye"}]}}"#;
        std::fs::write(dir.path().join(file_id), jsonl).unwrap();

        let requests = load_batch_input_file_from(dir.path(), file_id).unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["model"], "allternit-balanced");
        assert_eq!(requests[1]["messages"][0]["content"], "Bye");
    }

    #[tokio::test]
    async fn openai_batch_response_matches_contract() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let requests = sample_requests();
        let batch = service
            .create_openai(
                &key,
                &requests,
                "file_abc123",
                "/v1/chat/completions",
                "24h",
                Some(json!({ "env": "test" })),
            )
            .unwrap();

        let openai = OpenAiBatch::from(batch);
        assert_eq!(openai.object, "batch");
        assert_eq!(openai.endpoint, "/v1/chat/completions");
        assert_eq!(openai.input_file_id, "file_abc123");
        assert_eq!(openai.completion_window, "24h");
        assert_eq!(openai.request_counts.total, 2);
        assert_eq!(openai.request_counts.completed, 0);
        assert_eq!(openai.request_counts.failed, 0);
        assert!(openai.metadata.is_some());

        let value = serde_json::to_value(&openai).unwrap();
        assert_eq!(value["object"], "batch");
        assert_eq!(value["status"], "validating");
        assert_eq!(value["request_counts"]["total"], 2);
        assert!(value.get("output_file_id").is_none());
        assert_eq!(value["metadata"]["env"], "test");
    }

    #[tokio::test]
    async fn worker_passes_local_batch_id_to_provider() {
        let (db, _dir) = test_db();
        let key = insert_test_key(&db);
        let service = BatchesService::new(db.clone());
        let batch = service.create(&key, &sample_requests()).unwrap();

        let provider = Arc::new(MockProvider::new(
            vec![Ok(ProviderBatchJob {
                id: "prov_1".to_string(),
                status: "in_progress".to_string(),
            })],
            vec![],
        ));
        let captured = provider.submits.clone();
        let worker = BatchWorker::new(db.clone(), provider);
        worker.process_once().await.unwrap();

        let submits = captured.lock().unwrap().clone();
        assert_eq!(submits, vec![(batch.id, 2)]);
    }

    /// Minimal TCP stub of the local gateway's /v1/chat/completions that
    /// records request headers. Proves the native provider stamps
    /// `x-allternit-batch-id` on every sub-request (task G10), which is what
    /// makes the gateway's usage rows attributable to the batch.
    #[tokio::test]
    async fn native_provider_stamps_batch_id_header() {
        use std::sync::Arc as StdArc;
        use std::sync::Mutex as StdMutex;

        let seen: StdArc<StdMutex<Vec<Option<String>>>> = StdArc::new(StdMutex::new(Vec::new()));
        let seen_state = seen.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(
                move |headers: axum::http::HeaderMap| {
                    let seen = seen_state.clone();
                    async move {
                        seen.lock().unwrap().push(
                            headers
                                .get("x-allternit-batch-id")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string),
                        );
                        axum::Json(json!({"id": "chatcmpl-stub", "choices": []}))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = NativeBatchProvider::new(format!("http://127.0.0.1:{port}"), None);
        let results = provider
            .run_requests("batch_42", &sample_requests())
            .await;
        assert_eq!(results.len(), 2);

        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![Some("batch_42".to_string()), Some("batch_42".to_string())]
        );
    }

    #[tokio::test]
    async fn native_provider_marks_failed_sub_requests() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(json!({"error": {"message": "boom"}})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = NativeBatchProvider::new(format!("http://127.0.0.1:{port}"), None);
        let results = provider
            .run_requests("batch_7", &sample_requests())
            .await;
        assert_eq!(results.len(), 2);
        // Error results are wrapped with their index so callers can tell
        // which sub-request failed.
        assert_eq!(results[0]["index"], 0);
        assert!(results[0].get("error").is_some());
        assert_eq!(results[1]["index"], 1);
    }
}
