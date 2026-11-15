//! OpenTelemetry wiring for allternit-api — additive and off by default.
//!
//! The workspace pins `opentelemetry` / `opentelemetry_sdk` /
//! `opentelemetry-http` / `tracing-opentelemetry` (pre-1.0 crates). This
//! module is the single place they are wired together:
//!
//! - Off by default. Nothing is exported unless `OTEL_EXPORTER_OTLP_ENDPOINT`
//!   is set; when unset, [`layer_from_env`] returns `None` and tracing stays
//!   exactly as it was (fmt only).
//! - When set, spans produced by `tracing` (including the GenAI spans in
//!   `llm_gateway::genai_spans`) are bridged through `tracing-opentelemetry`
//!   into an SDK tracer provider and batch-exported to
//!   `{OTEL_EXPORTER_OTLP_ENDPOINT}/v1/traces` as OTLP/HTTP JSON (the pinned
//!   dependency set does not include `opentelemetry-otlp`/protobuf, so the
//!   exporter serializes the OTLP JSON mapping directly over the
//!   `opentelemetry-http` reqwest client).
//! - `OTEL_SERVICE_NAME` overrides the resource `service.name`
//!   (default `allternit-api`; the selfhost binary passes its own default to
//!   [`layer_from_env_with_default`]).
//! - `OTEL_RESOURCE_ATTRIBUTES` (`key=value,key2=value2`, values
//!   percent-decoded) adds resource attributes. Its `service.name` is used
//!   only when `OTEL_SERVICE_NAME` is unset, per the OTel spec.
//!
//! [`enabled`] lets hot paths skip span-field recording entirely when no
//! exporter is configured, keeping text/JSON logs byte-identical in the
//! default configuration.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use futures::future::BoxFuture;
use opentelemetry::trace::{SpanKind, Status, TraceError, TracerProvider as _};
use opentelemetry::{Array, Key, Value};
use opentelemetry_http::HttpClient;
use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
use opentelemetry_sdk::trace::Tracer;
use opentelemetry_sdk::Resource;
use serde_json::{json, Value as Json};
use tracing::Subscriber;
use tracing_subscriber::registry::LookupSpan;

pub const ENV_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
pub const ENV_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
pub const ENV_RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";
pub const DEFAULT_SERVICE_NAME: &str = "allternit-api";

static OTEL_ENABLED: AtomicBool = AtomicBool::new(false);

/// True once an OTLP exporter has been configured via [`layer_from_env`].
/// Callers use this to gate GenAI span-field recording so the default
/// configuration produces exactly the same logs as before.
pub fn enabled() -> bool {
    OTEL_ENABLED.load(Ordering::Relaxed)
}

/// Build the `tracing-opentelemetry` layer when `OTEL_EXPORTER_OTLP_ENDPOINT`
/// is set; `None` (and zero overhead) otherwise.
pub fn layer_from_env<S>() -> Option<tracing_opentelemetry::OpenTelemetryLayer<S, Tracer>>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    layer_from_env_with_default(DEFAULT_SERVICE_NAME)
}

/// [`layer_from_env`] with a different default `service.name`.
pub fn layer_from_env_with_default<S>(
    default_service_name: &str,
) -> Option<tracing_opentelemetry::OpenTelemetryLayer<S, Tracer>>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    let endpoint = std::env::var(ENV_OTLP_ENDPOINT)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())?;
    let attrs = parse_resource_attributes(
        &std::env::var(ENV_RESOURCE_ATTRIBUTES).unwrap_or_default(),
    );
    let service_name = resolve_service_name(
        std::env::var(ENV_SERVICE_NAME).ok().as_deref(),
        &attrs,
        default_service_name,
    );
    let extra: Vec<(String, String)> =
        attrs.into_iter().filter(|(k, _)| k != "service.name").collect();

    let exporter = OtlpJsonExporter::new(format!("{endpoint}/v1/traces"), &service_name)
        .with_resource_attributes(extra.clone());
    let mut resource = vec![opentelemetry::KeyValue::new("service.name", service_name)];
    resource.extend(
        extra
            .into_iter()
            .map(|(k, v)| opentelemetry::KeyValue::new(k, v)),
    );
    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_config(
            opentelemetry_sdk::trace::Config::default().with_resource(Resource::new(resource)),
        )
        .build();
    let tracer = provider.tracer("allternit-api");
    opentelemetry::global::set_tracer_provider(provider);
    OTEL_ENABLED.store(true, Ordering::Relaxed);
    Some(tracing_opentelemetry::layer().with_tracer(tracer))
}

/// `OTEL_RESOURCE_ATTRIBUTES`: `k=v` pairs split on `,`, keys and values
/// trimmed and percent-decoded. Malformed pairs are skipped.
pub fn parse_resource_attributes(raw: &str) -> Vec<(String, String)> {
    raw.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = urlencoding::decode(k.trim()).ok()?.into_owned();
            let v = urlencoding::decode(v.trim()).ok()?.into_owned();
            (!k.is_empty()).then_some((k, v))
        })
        .collect()
}

/// `OTEL_SERVICE_NAME`, else `service.name` from the resource attributes,
/// else the default.
pub fn resolve_service_name(
    env_service_name: Option<&str>,
    attrs: &[(String, String)],
    default: &str,
) -> String {
    env_service_name
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            attrs
                .iter()
                .find(|(k, v)| k == "service.name" && !v.is_empty())
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_else(|| default.to_string())
}

#[derive(Debug)]
struct OtlpError(String);

impl fmt::Display for OtlpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "otlp export failed: {}", self.0)
    }
}

impl std::error::Error for OtlpError {}

impl opentelemetry::ExportError for OtlpError {
    fn exporter_name(&self) -> &'static str {
        "otlp-http-json"
    }
}

/// `reqwest` 0.12 implementation of the pinned `opentelemetry-http` client
/// trait (the crate's own `reqwest` feature impls target reqwest 0.11 /
/// http 0.2 types, which predate this workspace's reqwest 0.12 / http 1.0).
#[derive(Debug, Default, Clone)]
struct ReqwestHttpClient(reqwest::Client);

#[async_trait::async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn send(
        &self,
        request: opentelemetry_http::Request<Vec<u8>>,
    ) -> Result<opentelemetry_http::Response<opentelemetry_http::Bytes>, opentelemetry_http::HttpError>
    {
        let url = request.uri().to_string();
        let method = reqwest::Method::from_bytes(request.method().as_str().as_bytes())
            .map_err(|e| Box::new(e) as opentelemetry_http::HttpError)?;
        let mut builder = self.0.request(method, url);
        for (name, value) in request.headers() {
            builder = builder.header(name.as_str(), value.as_bytes());
        }
        let response = builder
            .body(request.into_body())
            .send()
            .await
            .map_err(|e| Box::new(e) as opentelemetry_http::HttpError)?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| Box::new(e) as opentelemetry_http::HttpError)?;
        let out = opentelemetry_http::Response::builder()
            .status(status.as_u16())
            .body(opentelemetry_http::Bytes::from(body.to_vec()))
            .map_err(|e| Box::new(e) as opentelemetry_http::HttpError)?;
        Ok(out)
    }
}

/// Minimal OTLP/HTTP+JSON span exporter over the pinned `opentelemetry-http`
/// client trait. Batch export, no retry — the SDK's batch processor already
/// drops on error and logs it.
#[derive(Debug, Clone)]
pub struct OtlpJsonExporter {
    client: ReqwestHttpClient,
    traces_url: String,
    service_name: String,
    resource_attributes: Vec<(String, String)>,
}

impl OtlpJsonExporter {
    pub fn new(traces_url: String, service_name: &str) -> Self {
        Self {
            client: ReqwestHttpClient(crate::net_client::client()),
            traces_url,
            service_name: service_name.to_string(),
            resource_attributes: Vec::new(),
        }
    }

    /// Extra resource attributes sent with every batch (not `service.name`).
    pub fn with_resource_attributes(mut self, attrs: Vec<(String, String)>) -> Self {
        self.resource_attributes = attrs;
        self
    }
}

impl SpanExporter for OtlpJsonExporter {
    fn export(&mut self, batch: Vec<SpanData>) -> BoxFuture<'static, ExportResult> {
        let client = self.client.clone();
        let url = self.traces_url.clone();
        let payload =
            export_payload_with_resource(&batch, &self.service_name, &self.resource_attributes);
        Box::pin(async move {
            let body = serde_json::to_vec(&payload)
                .map_err(|e| TraceError::from(OtlpError(e.to_string())))?;
            let request = opentelemetry_http::Request::builder()
                .method("POST")
                .uri(&url)
                .header("content-type", "application/json")
                .body(body)
                .map_err(|e| TraceError::from(OtlpError(e.to_string())))?;
            let response = client
                .send(request)
                .await
                .map_err(|e| TraceError::from(OtlpError(e.to_string())))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(TraceError::from(OtlpError(format!(
                    "collector returned HTTP {}",
                    response.status()
                ))))
            }
        })
    }
}

fn unix_nanos(t: SystemTime) -> String {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn span_kind_code(kind: &SpanKind) -> i64 {
    // OTLP SpanKind enum values.
    match kind {
        SpanKind::Internal => 1,
        SpanKind::Server => 2,
        SpanKind::Client => 3,
        SpanKind::Producer => 4,
        SpanKind::Consumer => 5,
    }
}

fn attribute_json(key: &Key, value: &Value) -> Json {
    let value = match value {
        Value::Bool(v) => json!({ "boolValue": v }),
        Value::I64(v) => json!({ "intValue": v.to_string() }),
        Value::F64(v) => json!({ "doubleValue": v }),
        Value::String(v) => json!({ "stringValue": v.as_str() }),
        Value::Array(Array::Bool(v)) => json!({ "arrayValue": { "values": v } }),
        Value::Array(Array::I64(v)) => {
            json!({ "arrayValue": { "values": v.iter().map(|n| json!({ "intValue": n.to_string() })).collect::<Vec<_>>() } })
        }
        Value::Array(Array::F64(v)) => {
            json!({ "arrayValue": { "values": v.iter().map(|n| json!({ "doubleValue": n })).collect::<Vec<_>>() } })
        }
        Value::Array(Array::String(v)) => {
            json!({ "arrayValue": { "values": v.iter().map(|s| json!({ "stringValue": s.as_str() })).collect::<Vec<_>>() } })
        }
        _ => json!({ "stringValue": format!("{value:?}") }),
    };
    json!({ "key": key.as_str(), "value": value })
}

/// Serialize a span batch into the OTLP/JSON `ExportTraceServiceRequest`
/// body. Kept separate from I/O so it is unit-testable without a collector.
pub fn export_payload(batch: &[SpanData], service_name: &str) -> Json {
    export_payload_with_resource(batch, service_name, &[])
}

/// [`export_payload`] plus extra resource attributes.
pub fn export_payload_with_resource(
    batch: &[SpanData],
    service_name: &str,
    resource_attributes: &[(String, String)],
) -> Json {
    let mut resource_attrs =
        vec![json!({ "key": "service.name", "value": { "stringValue": service_name } })];
    resource_attrs.extend(
        resource_attributes
            .iter()
            .map(|(k, v)| json!({ "key": k, "value": { "stringValue": v } })),
    );
    let spans: Vec<Json> = batch
        .iter()
        .map(|span| {
            let parent = span.parent_span_id;
            let status = match &span.status {
                Status::Unset => json!({}),
                Status::Ok => json!({ "code": 1 }),
                Status::Error { description } => {
                    json!({ "code": 2, "message": description })
                }
            };
            json!({
                "traceId": span.span_context.trace_id().to_string(),
                "spanId": span.span_context.span_id().to_string(),
                "parentSpanId": if parent == opentelemetry::trace::SpanId::INVALID {
                    Json::Null
                } else {
                    Json::String(parent.to_string())
                },
                "name": span.name,
                "kind": span_kind_code(&span.span_kind),
                "startTimeUnixNano": unix_nanos(span.start_time),
                "endTimeUnixNano": unix_nanos(span.end_time),
                "attributes": span
                    .attributes
                    .iter()
                    .map(|kv| attribute_json(&kv.key, &kv.value))
                    .collect::<Vec<_>>(),
                "droppedAttributesCount": span.dropped_attributes_count,
                "status": status,
            })
        })
        .collect();

    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": resource_attrs,
            },
            "scopeSpans": [{
                "scope": { "name": "allternit-api" },
                "spans": spans,
            }],
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use std::borrow::Cow;

    fn sample_span() -> SpanData {
        SpanData {
            span_context: SpanContext::new(
                TraceId::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
                SpanId::from_hex("0123456789abcdef").unwrap(),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ),
            parent_span_id: SpanId::INVALID,
            span_kind: SpanKind::Client,
            name: Cow::Borrowed("chat gpt-4o"),
            start_time: SystemTime::UNIX_EPOCH,
            end_time: SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(250),
            attributes: vec![
                opentelemetry::KeyValue::new("gen_ai.request.model", "gpt-4o"),
                opentelemetry::KeyValue::new("gen_ai.usage.input_tokens", 42_i64),
            ],
            dropped_attributes_count: 0,
            events: opentelemetry_sdk::trace::SpanEvents::default(),
            links: opentelemetry_sdk::trace::SpanLinks::default(),
            status: Status::Ok,
            resource: Cow::Owned(Resource::empty()),
            instrumentation_lib: opentelemetry::InstrumentationLibrary::new(
                "test",
                None::<&str>,
                None::<&str>,
                None,
            ),
        }
    }

    #[test]
    fn payload_is_valid_otlp_json() {
        let payload = export_payload(&[sample_span()], "allternit-api");
        let spans = &payload["resourceSpans"][0]["scopeSpans"][0]["spans"];
        let span = &spans[0];
        assert_eq!(span["name"], "chat gpt-4o");
        assert_eq!(span["kind"], 3);
        assert_eq!(span["traceId"], "0123456789abcdef0123456789abcdef");
        assert_eq!(span["spanId"], "0123456789abcdef");
        assert_eq!(span["endTimeUnixNano"], "250000000");
        let attrs = span["attributes"].as_array().unwrap();
        let model = attrs
            .iter()
            .find(|a| a["key"] == "gen_ai.request.model")
            .unwrap();
        assert_eq!(model["value"]["stringValue"], "gpt-4o");
        let tokens = attrs
            .iter()
            .find(|a| a["key"] == "gen_ai.usage.input_tokens")
            .unwrap();
        assert_eq!(tokens["value"]["intValue"], "42");
        assert_eq!(
            payload["resourceSpans"][0]["resource"]["attributes"][0]["value"]["stringValue"],
            "allternit-api"
        );
    }

    #[test]
    fn resource_attributes_and_service_name() {
        let attrs = parse_resource_attributes(
            "deployment.environment=prod, service.name=from-attrs ,team=a%2Cb,broken",
        );
        assert_eq!(
            attrs,
            vec![
                ("deployment.environment".to_string(), "prod".to_string()),
                ("service.name".to_string(), "from-attrs".to_string()),
                ("team".to_string(), "a,b".to_string()),
            ]
        );
        assert_eq!(resolve_service_name(Some("env-name"), &attrs, "dflt"), "env-name");
        assert_eq!(resolve_service_name(None, &attrs, "dflt"), "from-attrs");
        assert_eq!(resolve_service_name(Some("  "), &[], "dflt"), "dflt");

        let payload = export_payload_with_resource(
            &[sample_span()],
            "allternit-agents-server",
            &[("deployment.environment".into(), "prod".into())],
        );
        let res = &payload["resourceSpans"][0]["resource"]["attributes"];
        assert_eq!(res[0]["value"]["stringValue"], "allternit-agents-server");
        assert_eq!(res[1]["key"], "deployment.environment");
        assert_eq!(res[1]["value"]["stringValue"], "prod");
    }

    #[test]
    fn layer_absent_without_endpoint_env() {
        // No OTEL_EXPORTER_OTLP_ENDPOINT in the test environment → no layer.
        // (Guards the off-by-default contract.)
        if std::env::var(ENV_OTLP_ENDPOINT).is_err() {
            assert!(!enabled());
        }
    }
}
