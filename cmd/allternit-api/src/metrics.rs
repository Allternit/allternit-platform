//! Prometheus metrics middleware and endpoint.
//!
//! Registers a histogram (`http_request_duration_seconds`) and a counter
//! (`http_requests_total`) labelled by method, status, and matched path,
//! plus the LLM gateway series (`llm_requests_total`, `llm_tokens_total`,
//! `llm_cost_microdollars_total`, `llm_request_duration_seconds`,
//! `llm_fallback_total`, `llm_failover_cooldown_total`,
//! `llm_failover_skipped_total`) emitted from the single
//! `llm_gateway::proxy::record_usage_event` choke point, plus the
//! `llm_response_cache_hits_total` / `llm_response_cache_misses_total`
//! counters for the P1.6 response-cache short-circuit.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use once_cell::sync::Lazy;
use prometheus::{CounterVec, Encoder, HistogramOpts, HistogramVec, Opts, Registry, TextEncoder};
use std::time::Instant;

struct Metrics {
    registry: Registry,
    http_request_duration_seconds: HistogramVec,
    http_requests_total: CounterVec,
    llm_requests_total: CounterVec,
    llm_tokens_total: CounterVec,
    llm_cost_microdollars_total: CounterVec,
    llm_request_duration_seconds: HistogramVec,
    llm_fallback_total: CounterVec,
    llm_failover_cooldown_total: CounterVec,
    llm_failover_skipped_total: CounterVec,
    llm_response_cache_hits_total: CounterVec,
    llm_response_cache_misses_total: CounterVec,
    completion_cache_events_total: CounterVec,
    memory_drive_events_total: CounterVec,
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new();

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ]),
            &["method", "status", "path"],
        )
        .expect("invalid http_request_duration_seconds metric");

        let http_requests_total = CounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["method", "status", "path"],
        )
        .expect("invalid http_requests_total metric");

        // ── LLM gateway series (B7) ─────────────────────────────────────
        let llm_requests_total = CounterVec::new(
            Opts::new("llm_requests_total", "Total LLM gateway requests"),
            &["model", "provider", "status"],
        )
        .expect("invalid llm_requests_total metric");

        let llm_tokens_total = CounterVec::new(
            Opts::new("llm_tokens_total", "Total LLM tokens by kind"),
            &["model", "kind"],
        )
        .expect("invalid llm_tokens_total metric");

        let llm_cost_microdollars_total = CounterVec::new(
            Opts::new(
                "llm_cost_microdollars_total",
                "Total LLM spend in microdollars",
            ),
            &["tenant", "model"],
        )
        .expect("invalid llm_cost_microdollars_total metric");

        let llm_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "llm_request_duration_seconds",
                "LLM gateway request duration in seconds",
            )
            .buckets(vec![
                0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0,
            ]),
            &["model"],
        )
        .expect("invalid llm_request_duration_seconds metric");

        let llm_fallback_total = CounterVec::new(
            Opts::new(
                "llm_fallback_total",
                "Total cross-provider failovers (from,to as provider/model)",
            ),
            &["from", "to"],
        )
        .expect("invalid llm_fallback_total metric");

        let llm_failover_cooldown_total = CounterVec::new(
            Opts::new(
                "llm_failover_cooldown_total",
                "Times a provider/model entered failover cooldown (incl. fail-open bypasses)",
            ),
            &["provider", "model"],
        )
        .expect("invalid llm_failover_cooldown_total metric");

        let llm_failover_skipped_total = CounterVec::new(
            Opts::new(
                "llm_failover_skipped_total",
                "Times a cooling-down provider/model was skipped during fallback selection",
            ),
            &["provider", "model"],
        )
        .expect("invalid llm_failover_skipped_total metric");

        // P1.6: response-cache short-circuit on the chat-completions hot path.
        let llm_response_cache_hits_total = CounterVec::new(
            Opts::new(
                "llm_response_cache_hits_total",
                "Non-streaming requests served from the response cache (no upstream call)",
            ),
            &["model"],
        )
        .expect("invalid llm_response_cache_hits_total metric");

        let llm_response_cache_misses_total = CounterVec::new(
            Opts::new(
                "llm_response_cache_misses_total",
                "Cacheable non-streaming requests not found in the response cache",
            ),
            &["model"],
        )
        .expect("invalid llm_response_cache_misses_total metric");

        // O7/O9 (WP-K1): shared exact cache + semantic shadow cache, per
        // surface (gateway | internal) and call type. Events: hit, miss,
        // store, sem_candidate, would_serve, would_not_serve, shadow_same,
        // shadow_different.
        // TODO(C1 #1126): also write these into the usage ledger once it
        // lands, so the Usage dashboard (WP-C2) can show cache hit rate and
        // S1 savings next to cost.
        let memory_drive_events_total = CounterVec::new(
            Opts::new(
                "memory_drive_events_total",
                "Memory Drive events (commit, commit_failed, pending_retry_failed, push_refused, push_accepted, dream_applied, dream_failed, purge, near_limit)",
            ),
            &["event"],
        )
        .expect("invalid memory_drive_events_total metric");
        registry
            .register(Box::new(memory_drive_events_total.clone()))
            .expect("failed to register memory_drive_events_total");

        let completion_cache_events_total = CounterVec::new(
            Opts::new(
                "completion_cache_events_total",
                "Exact and semantic (shadow) completion cache events",
            ),
            &["surface", "call_type", "event"],
        )
        .expect("invalid completion_cache_events_total metric");

        registry
            .register(Box::new(completion_cache_events_total.clone()))
            .expect("failed to register completion_cache_events_total");
        registry
            .register(Box::new(http_request_duration_seconds.clone()))
            .expect("failed to register http_request_duration_seconds");
        registry
            .register(Box::new(http_requests_total.clone()))
            .expect("failed to register http_requests_total");
        registry
            .register(Box::new(llm_requests_total.clone()))
            .expect("failed to register llm_requests_total");
        registry
            .register(Box::new(llm_tokens_total.clone()))
            .expect("failed to register llm_tokens_total");
        registry
            .register(Box::new(llm_cost_microdollars_total.clone()))
            .expect("failed to register llm_cost_microdollars_total");
        registry
            .register(Box::new(llm_request_duration_seconds.clone()))
            .expect("failed to register llm_request_duration_seconds");
        registry
            .register(Box::new(llm_fallback_total.clone()))
            .expect("failed to register llm_fallback_total");
        registry
            .register(Box::new(llm_failover_cooldown_total.clone()))
            .expect("failed to register llm_failover_cooldown_total");
        registry
            .register(Box::new(llm_failover_skipped_total.clone()))
            .expect("failed to register llm_failover_skipped_total");
        registry
            .register(Box::new(llm_response_cache_hits_total.clone()))
            .expect("failed to register llm_response_cache_hits_total");
        registry
            .register(Box::new(llm_response_cache_misses_total.clone()))
            .expect("failed to register llm_response_cache_misses_total");

        Self {
            registry,
            http_request_duration_seconds,
            http_requests_total,
            llm_requests_total,
            llm_tokens_total,
            llm_cost_microdollars_total,
            llm_request_duration_seconds,
            llm_fallback_total,
            llm_failover_cooldown_total,
            llm_failover_skipped_total,
            llm_response_cache_hits_total,
            llm_response_cache_misses_total,
            completion_cache_events_total,
            memory_drive_events_total,
        }
    }
}

static METRICS: Lazy<Metrics> = Lazy::new(Metrics::new);

/// Metrics endpoint handler — renders all registered metrics in Prometheus text format.
async fn metrics_handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = METRICS.registry.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to encode metrics: {e}"),
        )
            .into_response();
    }

    match String::from_utf8(buffer) {
        Ok(text) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, encoder.format_type())],
            text,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Metrics output is not valid UTF-8: {e}"),
        )
            .into_response(),
    }
}

/// Tower/Axum middleware that records request duration and count per method/status/path.
pub async fn metrics_middleware(request: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());

    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    METRICS
        .http_request_duration_seconds
        .with_label_values(&[&method, &status, &path])
        .observe(duration);
    METRICS
        .http_requests_total
        .with_label_values(&[&method, &status, &path])
        .inc();

    tracing::debug!(
        method = %method,
        path = %path,
        status = %status,
        duration_ms = %(duration * 1000.0),
        "request"
    );

    response
}

// ── LLM gateway emitters (called from llm_gateway::proxy::record_usage_event) ─

/// One completed gateway request, labelled by model/provider/status
/// (`ok`, `error`, `dlp_blocked`, `client_disconnected`, ...).
pub fn record_llm_request(model: &str, provider: &str, status: &str) {
    METRICS
        .llm_requests_total
        .with_label_values(&[model, provider, status])
        .inc();
}

/// Token usage by kind (`prompt`, `completion`, `reasoning`, `cached`).
pub fn record_llm_tokens(model: &str, kind: &str, count: i64) {
    if count <= 0 {
        return;
    }
    METRICS
        .llm_tokens_total
        .with_label_values(&[model, kind])
        .inc_by(count as f64);
}

/// Spend in microdollars (the recomputed cost when available).
pub fn record_llm_cost(tenant: &str, model: &str, microdollars: i64) {
    if microdollars <= 0 {
        return;
    }
    METRICS
        .llm_cost_microdollars_total
        .with_label_values(&[tenant, model])
        .inc_by(microdollars as f64);
}

/// End-to-end gateway latency for one request.
pub fn observe_llm_duration(model: &str, seconds: f64) {
    METRICS
        .llm_request_duration_seconds
        .with_label_values(&[model])
        .observe(seconds);
}

/// One cross-provider failover (`from`/`to` as `provider/model`).
pub fn inc_llm_fallback(from: &str, to: &str) {
    METRICS
        .llm_fallback_total
        .with_label_values(&[from, to])
        .inc();
}

/// One provider/model entering failover cooldown (429 or failure-rate trip;
/// also incremented on fail-open bypasses, where the cooldown was overridden).
pub fn record_llm_failover_cooldown(provider: &str, model: &str) {
    METRICS
        .llm_failover_cooldown_total
        .with_label_values(&[provider, model])
        .inc();
}

/// One cooling-down provider/model skipped during fallback selection.
pub fn record_llm_failover_skipped(provider: &str, model: &str) {
    METRICS
        .llm_failover_skipped_total
        .with_label_values(&[provider, model])
        .inc();
}

/// One chat-completions request served from the response cache (P1.6).
pub fn inc_llm_response_cache_hit(model: &str) {
    METRICS
        .llm_response_cache_hits_total
        .with_label_values(&[model])
        .inc();
}

/// One cacheable chat-completions request that missed the response cache (P1.6).
pub fn inc_llm_response_cache_miss(model: &str) {
    METRICS
        .llm_response_cache_misses_total
        .with_label_values(&[model])
        .inc();
}

/// One Memory Drive event, for alerting on failed writes, refused pushes and
/// failed Dreams.
pub fn inc_memory_drive_event(event: &str) {
    METRICS.memory_drive_events_total.with_label_values(&[event]).inc();
}

/// One completion-cache event (O7 exact / O9 semantic shadow).
pub fn inc_completion_cache_event(surface: &str, call_type: &str, event: &str) {
    METRICS
        .completion_cache_events_total
        .with_label_values(&[surface, call_type, event])
        .inc();
}

/// Current value of one completion-cache counter (tests, admin readouts).
pub fn completion_cache_event_count(surface: &str, call_type: &str, event: &str) -> u64 {
    METRICS
        .completion_cache_events_total
        .with_label_values(&[surface, call_type, event])
        .get() as u64
}

/// Router for the metrics endpoint (no auth required).
pub fn metrics_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Register metrics — a no-op because metrics are initialized lazily on first use.
pub fn register_metrics() {}
