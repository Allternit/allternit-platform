//! `/v1/projects/:id/tracing`: send a project's traces to the developer's own
//! OpenTelemetry collector (Langfuse, Arize Phoenix, any OTLP/HTTP endpoint).
//!
//! The project stores an OTLP/HTTP traces URL and request headers (usually the
//! collector's secret). Headers are encrypted at rest with the credential
//! cipher, like model keys; only their names are ever returned. Once set,
//! every agent turn, phone run and call of the project becomes one span that
//! follows the OpenTelemetry GenAI conventions (`gen_ai.*`).
//!
//! Export never blocks or fails the request that produced it: [`emit`] spawns
//! a task, posts one OTLP protobuf request through the same SSRF-checked,
//! address-pinned client webhooks use, and records only `last_export_at` /
//! `last_error`. `POST …/tracing/test` sends one span and waits for the answer.
//! `:id` is the caller's project id or `current`. Scope `usage`.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use allternit_cloud_core::CredentialCipher;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json,
};
use chrono::{DateTime, Utc};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use super::{events, ApiJson, PlatformCaller, PlatformError, RouteTable};
use crate::ApiState;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/projects/:id/tracing", &["GET", "PUT", "DELETE"], get(get_tracing).put(put_tracing).delete(delete_tracing))
        .add("/v1/projects/:id/tracing/test", &["POST"], post(test_tracing))
}

const MAX_HEADERS: usize = 20;
const DEFAULT_SERVICE: &str = "allternit-platform";
/// Headers the exporter sets itself.
const RESERVED: [&str; 5] = ["content-type", "content-length", "host", "connection", "transfer-encoding"];

static CIPHER: OnceLock<Arc<CredentialCipher>> = OnceLock::new();

/// Called once when the router is built, so [`emit`] can read stored headers
/// from places that only hold the database pool.
pub fn set_cipher(cipher: Arc<CredentialCipher>) {
    let _ = CIPHER.set(cipher);
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Tracing {
    #[sqlx(skip)]
    pub object: &'static str,
    pub project_id: String,
    pub endpoint: String,
    pub header_names: Vec<String>,
    pub service_name: String,
    pub enabled: bool,
    pub last_export_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "project_id, endpoint, header_names, service_name, enabled, last_export_at, last_error, created_at, updated_at";

fn obj(mut t: Tracing) -> Tracing {
    t.object = "project.tracing";
    t
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PutBody {
    endpoint: String,
    /// Replaces the stored headers. Leave out to keep them.
    headers: Option<BTreeMap<String, String>>,
    service_name: Option<String>,
    enabled: Option<bool>,
}

fn project(caller: &PlatformCaller, id: &str) -> Result<(), PlatformError> {
    if id == "current" || id == caller.project_id {
        Ok(())
    } else {
        Err(PlatformError::not_found("project_not_found", "No project with that id for this API key."))
    }
}

fn cipher(state: &ApiState) -> Result<&CredentialCipher, PlatformError> {
    state.credential_cipher.as_deref().ok_or_else(|| PlatformError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        kind: "api_error",
        code: "tracing_unavailable".into(),
        message: "Trace export isn't available on this deployment.".into(),
        param: None,
        url: None,
    })
}

/// The traces URL: `endpoint` as given when it already ends in `/v1/traces`,
/// otherwise with `/v1/traces` appended (the OTLP base-URL convention).
pub fn traces_url(endpoint: &str) -> String {
    let e = endpoint.trim().trim_end_matches('/');
    if e.ends_with("/v1/traces") {
        e.to_string()
    } else {
        format!("{e}/v1/traces")
    }
}

fn check_headers(headers: &BTreeMap<String, String>) -> Result<(), PlatformError> {
    let bad = |m: String| PlatformError::invalid_request("invalid_headers", m).with_param("headers");
    if headers.len() > MAX_HEADERS {
        return Err(bad(format!("At most {MAX_HEADERS} headers.")));
    }
    for (k, v) in headers {
        let name_ok = !k.is_empty() && k.len() <= 128 && k.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b));
        if !name_ok {
            return Err(bad(format!("'{k}' isn't a valid header name.")));
        }
        if RESERVED.contains(&k.to_ascii_lowercase().as_str()) {
            return Err(bad(format!("'{k}' is set by Allternit and can't be overridden.")));
        }
        if v.len() > 4096 || v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(bad(format!("The value of '{k}' must be one line of at most 4096 bytes.")));
        }
    }
    Ok(())
}

async fn fetch(db: &PgPool, project_id: &str) -> Result<Option<Tracing>, PlatformError> {
    Ok(sqlx::query_as::<_, Tracing>(&format!("SELECT {COLUMNS} FROM platform_project_tracing WHERE project_id = $1"))
        .bind(project_id)
        .fetch_optional(db)
        .await?
        .map(obj))
}

fn not_configured() -> PlatformError {
    PlatformError::not_found("tracing_not_configured", "This project has no trace export set up.")
}

async fn get_tracing(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Tracing>, PlatformError> {
    caller.require("usage")?;
    project(&caller, &id)?;
    fetch(&state.db, &caller.project_id).await?.map(Json).ok_or_else(not_configured)
}

async fn put_tracing(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<PutBody>,
) -> Result<Json<Tracing>, PlatformError> {
    caller.require("usage")?;
    caller.require_unbound()?;
    project(&caller, &id)?;
    let c = cipher(&state)?;
    let endpoint = body.endpoint.trim().to_string();
    if endpoint.len() > 2048 {
        return Err(PlatformError::invalid_request("invalid_url", "endpoint must be at most 2048 characters.").with_param("endpoint"));
    }
    // Same rules as webhook URLs: https, no credentials, a public address.
    events::check_url(&traces_url(&endpoint)).await.map_err(|e| e.with_param("endpoint"))?;
    let service = body.service_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SERVICE).to_string();
    if service.chars().count() > 120 {
        return Err(PlatformError::invalid_request("invalid_service_name", "service_name must be at most 120 characters.").with_param("service_name"));
    }
    let existing = fetch(&state.db, &caller.project_id).await?;
    let headers = match &body.headers {
        Some(h) => {
            check_headers(h)?;
            let plain = serde_json::to_string(h).unwrap_or_else(|_| "{}".into());
            Some((c.encrypt(&plain).map_err(|_| PlatformError::api_error("internal_error", "The headers couldn't be stored."))?, h.keys().cloned().collect::<Vec<_>>()))
        }
        // A first setup with no headers (an open collector) stores an empty set.
        None if existing.is_none() => Some((c.encrypt("{}").map_err(|_| PlatformError::api_error("internal_error", "The headers couldn't be stored."))?, Vec::new())),
        None => None,
    };
    let enabled = body.enabled.unwrap_or(true);
    let row = match headers {
        Some((enc, names)) => sqlx::query_as::<_, Tracing>(&format!(
            "INSERT INTO platform_project_tracing (project_id, endpoint, headers_encrypted, header_names, service_name, enabled) VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (project_id) DO UPDATE SET endpoint = excluded.endpoint, headers_encrypted = excluded.headers_encrypted, header_names = excluded.header_names, \
               service_name = excluded.service_name, enabled = excluded.enabled, last_error = NULL, updated_at = NOW() \
             RETURNING {COLUMNS}"
        ))
        .bind(&caller.project_id)
        .bind(&endpoint)
        .bind(&enc)
        .bind(&names)
        .bind(&service)
        .bind(enabled)
        .fetch_one(&state.db)
        .await?,
        None => sqlx::query_as::<_, Tracing>(&format!(
            "UPDATE platform_project_tracing SET endpoint = $2, service_name = $3, enabled = $4, last_error = NULL, updated_at = NOW() WHERE project_id = $1 RETURNING {COLUMNS}"
        ))
        .bind(&caller.project_id)
        .bind(&endpoint)
        .bind(&service)
        .bind(enabled)
        .fetch_one(&state.db)
        .await?,
    };
    Ok(Json(obj(row)))
}

async fn delete_tracing(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("usage")?;
    caller.require_unbound()?;
    project(&caller, &id)?;
    let gone = sqlx::query("DELETE FROM platform_project_tracing WHERE project_id = $1").bind(&caller.project_id).execute(&state.db).await?;
    if gone.rows_affected() == 0 {
        return Err(not_configured());
    }
    Ok(Json(json!({ "object": "project.tracing", "project_id": caller.project_id, "deleted": true })))
}

/// Send one test span now and report what the collector answered.
async fn test_tracing(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    caller.require("usage")?;
    project(&caller, &id)?;
    let c = cipher(&state)?;
    let target = load(&state.db, c, &caller.project_id).await?.ok_or_else(not_configured)?;
    let now = Utc::now();
    let span = SpanOut::new("allternit.tracing.test", now - chrono::Duration::milliseconds(5), now)
        .attr("allternit.project.id", caller.project_id.as_str())
        .attr("allternit.test", true);
    let result = send(&state.db, &caller.project_id, &target, vec![span]).await;
    let (ok, status, message) = match result {
        Ok(code @ 200..=299) => (true, Some(code), "The collector accepted the test span.".to_string()),
        Ok(code) => (false, Some(code), format!("The collector answered HTTP {code}.")),
        Err(m) => (false, None, m),
    };
    Ok(Json(json!({ "object": "project.tracing.test", "ok": ok, "status": status, "message": message })))
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// A span attribute value.
#[derive(Debug, Clone)]
pub enum Attr {
    S(String),
    I(i64),
    B(bool),
}

impl From<&str> for Attr {
    fn from(v: &str) -> Self {
        Attr::S(v.to_string())
    }
}
impl From<String> for Attr {
    fn from(v: String) -> Self {
        Attr::S(v)
    }
}
impl From<i64> for Attr {
    fn from(v: i64) -> Self {
        Attr::I(v)
    }
}
impl From<bool> for Attr {
    fn from(v: bool) -> Self {
        Attr::B(v)
    }
}

/// One finished span to export.
#[derive(Debug, Clone)]
pub struct SpanOut {
    pub name: String,
    /// OTLP SpanKind: 1 internal, 2 server, 3 client.
    pub kind: i32,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub attrs: Vec<(String, Attr)>,
    pub error: Option<String>,
    /// Spans with the same seed (a conversation or call id) share a trace id.
    pub trace_seed: Option<String>,
}

impl SpanOut {
    pub fn new(name: impl Into<String>, start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { name: name.into(), kind: 1, start, end, attrs: Vec::new(), error: None, trace_seed: None }
    }
    pub fn kind(mut self, kind: i32) -> Self {
        self.kind = kind;
        self
    }
    pub fn attr(mut self, key: &str, value: impl Into<Attr>) -> Self {
        self.attrs.push((key.to_string(), value.into()));
        self
    }
    pub fn attr_opt(self, key: &str, value: Option<impl Into<Attr>>) -> Self {
        match value {
            Some(v) => self.attr(key, v),
            None => self,
        }
    }
    pub fn error(mut self, error: Option<&str>) -> Self {
        self.error = error.map(str::to_string);
        self
    }
    pub fn trace(mut self, seed: &str) -> Self {
        self.trace_seed = Some(seed.to_string());
        self
    }
}

/// The span for a phone event from the owner's Desktop: `phone.run.completed`
/// (a task or skill run) or `phone.step` (one phone tool call). Spans of one run
/// share a trace. Only the allowlisted fields of the event are used.
pub fn phone_span(event: &str, data: &Value) -> SpanOut {
    let end = Utc::now();
    let start = end - chrono::Duration::milliseconds(data["duration_ms"].as_i64().unwrap_or(0).clamp(0, 86_400_000));
    let s = |k: &str| data[k].as_str().map(str::to_string);
    let failed = data["ok"].as_bool() == Some(false);
    let span = if event == "phone.step" {
        let tool = s("tool").unwrap_or_else(|| "phone.tool".into());
        SpanOut::new(format!("execute_tool {tool}"), start, end)
            .attr("gen_ai.operation.name", "execute_tool")
            .attr("gen_ai.tool.name", tool)
            .attr_opt("allternit.phone.step.id", s("step_id"))
            .error(failed.then(|| data["error"].as_str().unwrap_or("phone step failed")))
    } else {
        SpanOut::new(format!("invoke_agent {}", s("skill").unwrap_or_else(|| "phone.task".into())), start, end)
            .attr("gen_ai.operation.name", "invoke_agent")
            .attr_opt("allternit.phone.skill", s("skill"))
            .attr_opt("allternit.phone.status", s("status"))
            .attr_opt("allternit.phone.steps", data["steps"].as_i64())
            .attr_opt("allternit.phone.model_calls", data["model_calls"].as_i64())
            .error(failed.then(|| data["status"].as_str().unwrap_or("phone run failed")))
    };
    let run = s("run_id");
    let span = span.attr_opt("allternit.phone.run.id", run.clone()).attr_opt("allternit.phone.serial", s("serial")).attr_opt("allternit.phone.source", s("source"));
    match run {
        Some(r) => span.trace(&r),
        None => span,
    }
}

struct Target {
    url: String,
    headers: Vec<(String, String)>,
    service: String,
}

async fn load(db: &PgPool, cipher: &CredentialCipher, project_id: &str) -> Result<Option<Target>, PlatformError> {
    let row: Option<(String, String, String, bool)> =
        sqlx::query_as("SELECT endpoint, headers_encrypted, service_name, enabled FROM platform_project_tracing WHERE project_id = $1")
            .bind(project_id)
            .fetch_optional(db)
            .await?;
    let Some((endpoint, enc, service, enabled)) = row else { return Ok(None) };
    if !enabled {
        return Ok(None);
    }
    let plain = cipher.decrypt(&enc).map_err(|_| PlatformError::api_error("internal_error", "The stored headers couldn't be read."))?;
    let map: BTreeMap<String, String> = serde_json::from_str(&plain).unwrap_or_default();
    Ok(Some(Target { url: traces_url(&endpoint), headers: map.into_iter().collect(), service }))
}

/// Export `span` for `project_id` in the background. Does nothing when the
/// project has no (enabled) trace export; never fails the caller.
pub fn emit(db: &PgPool, project_id: &str, span: SpanOut) {
    let Some(cipher) = CIPHER.get().cloned() else { return };
    let (db, project_id) = (db.clone(), project_id.to_string());
    tokio::spawn(async move {
        match load(&db, &cipher, &project_id).await {
            Ok(Some(target)) => {
                let _ = send(&db, &project_id, &target, vec![span]).await;
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(%project_id, code = %e.code, "platform: trace export config unreadable"),
        }
    });
}

/// POST one OTLP/HTTP protobuf request; records the outcome on the project.
async fn send(db: &PgPool, project_id: &str, target: &Target, spans: Vec<SpanOut>) -> Result<u16, String> {
    let body = encode(&target.service, project_id, spans).encode_to_vec();
    let mut headers = vec![("content-type".to_string(), "application/x-protobuf".to_string())];
    headers.extend(target.headers.iter().cloned());
    let result = match events::post_vetted(&target.url, &headers, &body, 0).await {
        Ok((status, _)) => Ok(status),
        Err(e) => Err(format!("The collector couldn't be reached: {}.", e.message())),
    };
    let error = match &result {
        Ok(s) if (200..300).contains(s) => None,
        Ok(s) => Some(format!("collector answered HTTP {s}")),
        Err(m) => Some(m.clone()),
    };
    if let Some(e) = &error {
        tracing::debug!(%project_id, error = %e, "platform: trace export failed");
    }
    let _ = sqlx::query("UPDATE platform_project_tracing SET last_export_at = CASE WHEN $2::text IS NULL THEN NOW() ELSE last_export_at END, last_error = $2 WHERE project_id = $1")
        .bind(project_id)
        .bind(&error)
        .execute(db)
        .await;
    result
}

fn unix_nanos(t: DateTime<Utc>) -> u64 {
    t.timestamp_nanos_opt().unwrap_or(0).max(0) as u64
}

fn trace_id(seed: Option<&str>) -> Vec<u8> {
    match seed {
        Some(s) => Sha256::digest(s.as_bytes())[..16].to_vec(),
        None => rand::random::<[u8; 16]>().to_vec(),
    }
}

fn kv(key: &str, value: Attr) -> pb::KeyValue {
    let value = match value {
        Attr::S(s) => pb::any_value::Value::StringValue(s),
        Attr::I(i) => pb::any_value::Value::IntValue(i),
        Attr::B(b) => pb::any_value::Value::BoolValue(b),
    };
    pb::KeyValue { key: key.to_string(), value: Some(pb::AnyValue { value: Some(value) }) }
}

/// The OTLP `ExportTraceServiceRequest` for `spans`.
fn encode(service: &str, project_id: &str, spans: Vec<SpanOut>) -> pb::ExportTraceServiceRequest {
    let spans = spans
        .into_iter()
        .map(|s| {
            let status = match &s.error {
                Some(m) => pb::Status { message: m.clone(), code: 2 },
                None => pb::Status { message: String::new(), code: 1 },
            };
            let mut attrs: Vec<pb::KeyValue> = s.attrs.into_iter().map(|(k, v)| kv(&k, v)).collect();
            if let Some(e) = &s.error {
                attrs.push(kv("error.type", Attr::S(if e.len() > 200 { "error".into() } else { e.clone() })));
            }
            pb::Span {
                trace_id: trace_id(s.trace_seed.as_deref()),
                span_id: rand::random::<[u8; 8]>().to_vec(),
                parent_span_id: Vec::new(),
                name: s.name,
                kind: s.kind,
                start_time_unix_nano: unix_nanos(s.start),
                end_time_unix_nano: unix_nanos(s.end.max(s.start)),
                attributes: attrs,
                status: Some(status),
            }
        })
        .collect();
    pb::ExportTraceServiceRequest {
        resource_spans: vec![pb::ResourceSpans {
            resource: Some(pb::Resource {
                attributes: vec![kv("service.name", Attr::S(service.to_string())), kv("allternit.project.id", Attr::S(project_id.to_string()))],
            }),
            scope_spans: vec![pb::ScopeSpans { scope: Some(pb::InstrumentationScope { name: "allternit-platform".into(), version: env!("CARGO_PKG_VERSION").into() }), spans }],
        }],
    }
}

/// The subset of the OTLP trace protobuf (opentelemetry-proto
/// `collector/trace/v1`, `trace/v1`, `common/v1`, `resource/v1`) this
/// exporter writes. Field tags match the upstream `.proto` files.
mod pb {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ExportTraceServiceRequest {
        #[prost(message, repeated, tag = "1")]
        pub resource_spans: Vec<ResourceSpans>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ResourceSpans {
        #[prost(message, optional, tag = "1")]
        pub resource: Option<Resource>,
        #[prost(message, repeated, tag = "2")]
        pub scope_spans: Vec<ScopeSpans>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Resource {
        #[prost(message, repeated, tag = "1")]
        pub attributes: Vec<KeyValue>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ScopeSpans {
        #[prost(message, optional, tag = "1")]
        pub scope: Option<InstrumentationScope>,
        #[prost(message, repeated, tag = "2")]
        pub spans: Vec<Span>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct InstrumentationScope {
        #[prost(string, tag = "1")]
        pub name: String,
        #[prost(string, tag = "2")]
        pub version: String,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Span {
        #[prost(bytes = "vec", tag = "1")]
        pub trace_id: Vec<u8>,
        #[prost(bytes = "vec", tag = "2")]
        pub span_id: Vec<u8>,
        #[prost(bytes = "vec", tag = "4")]
        pub parent_span_id: Vec<u8>,
        #[prost(string, tag = "5")]
        pub name: String,
        #[prost(int32, tag = "6")]
        pub kind: i32,
        #[prost(fixed64, tag = "7")]
        pub start_time_unix_nano: u64,
        #[prost(fixed64, tag = "8")]
        pub end_time_unix_nano: u64,
        #[prost(message, repeated, tag = "9")]
        pub attributes: Vec<KeyValue>,
        #[prost(message, optional, tag = "15")]
        pub status: Option<Status>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Status {
        #[prost(string, tag = "2")]
        pub message: String,
        #[prost(int32, tag = "3")]
        pub code: i32,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct KeyValue {
        #[prost(string, tag = "1")]
        pub key: String,
        #[prost(message, optional, tag = "2")]
        pub value: Option<AnyValue>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct AnyValue {
        #[prost(oneof = "any_value::Value", tags = "1, 2, 3")]
        pub value: Option<any_value::Value>,
    }
    pub mod any_value {
        #[derive(Clone, PartialEq, prost::Oneof)]
        pub enum Value {
            #[prost(string, tag = "1")]
            StringValue(String),
            #[prost(bool, tag = "2")]
            BoolValue(bool),
            #[prost(int64, tag = "3")]
            IntValue(i64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traces_url_appends_once() {
        assert_eq!(traces_url("https://cloud.langfuse.com/api/public/otel"), "https://cloud.langfuse.com/api/public/otel/v1/traces");
        assert_eq!(traces_url("https://app.phoenix.arize.com/s/me/v1/traces/"), "https://app.phoenix.arize.com/s/me/v1/traces");
    }

    #[test]
    fn encodes_a_span() {
        let now = Utc::now();
        let span = SpanOut::new("invoke_agent support", now, now).attr("gen_ai.usage.input_tokens", 12i64).trace("conv_1");
        let req = encode("svc", "proj_1", vec![span]);
        let bytes = req.encode_to_vec();
        let back = pb::ExportTraceServiceRequest::decode(bytes.as_slice()).unwrap();
        let s = &back.resource_spans[0].scope_spans[0].spans[0];
        assert_eq!(s.name, "invoke_agent support");
        assert_eq!(s.trace_id.len(), 16);
        assert_eq!(s.trace_id, trace_id(Some("conv_1")));
    }

    #[test]
    fn rejects_reserved_and_multiline_headers() {
        let mut h = BTreeMap::new();
        h.insert("Content-Type".to_string(), "x".to_string());
        assert!(check_headers(&h).is_err());
        let mut h = BTreeMap::new();
        h.insert("Authorization".to_string(), "Basic a\nb".to_string());
        assert!(check_headers(&h).is_err());
        let mut h = BTreeMap::new();
        h.insert("Authorization".to_string(), "Basic abc".to_string());
        assert!(check_headers(&h).is_ok());
    }
}
