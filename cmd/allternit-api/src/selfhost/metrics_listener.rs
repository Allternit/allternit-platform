//! Prometheus `/metrics` for the Agents Server, on its own listener.
//!
//! Served only when `observability.prometheus.enabled` is true, on
//! `observability.prometheus.listen` (default `127.0.0.1:9464`). The main API
//! router never mounts `/metrics` (`tests/selfhost_ops.rs` checks for 404).
//! This registry is separate from the hosted API's `crate::metrics`, so none
//! of the hosted series appear here.
//!
//! Series (all prefixed `allternit_selfhost_`):
//!
//! | Name | Type | Labels | Source |
//! |---|---|---|---|
//! | `sessions` | gauge | status | store, at scrape |
//! | `turns_total` | counter | outcome | `telemetry::TurnSpan::finish` |
//! | `turn_duration_seconds` | histogram | outcome | same |
//! | `tool_calls_total` | counter | name, outcome | `telemetry::work_result_span` |
//! | `queue_depth` | gauge | environment | store, at scrape |
//! | `workers_online` | gauge | environment | store, at scrape (seen in the last 60 s) |
//! | `work_reclaims_total` | counter | | `telemetry::work_claim_span` |
//! | `leases_expired_total` | counter | | `telemetry::record_expired_leases` |
//! | `egress_requests_total` | counter | host, result | egress guard counters |
//! | `egress_refusals_total` | counter | host | egress guard counters |
//! | `store_up` | gauge | | 1 when the scrape-time store reads worked |

use std::net::SocketAddr;

use axum::extract::State;
use axum::http::{header::CONTENT_TYPE, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::{Duration, Utc};
use once_cell::sync::Lazy;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry, TextEncoder,
};

use super::routes_environments::ONLINE_SECS;
use super::store::SESSION_STATUSES;
use super::SelfHostState;

const NS: &str = "allternit_selfhost";

struct LoopMetrics {
    registry: Registry,
    turns_total: IntCounterVec,
    turn_duration_seconds: HistogramVec,
    tool_calls_total: IntCounterVec,
    work_reclaims_total: IntCounter,
    leases_expired_total: IntCounter,
}

impl LoopMetrics {
    fn new() -> Self {
        let registry = Registry::new();
        let turns_total = IntCounterVec::new(
            Opts::new("turns_total", "Agent turns that ended, by outcome.").namespace(NS),
            &["outcome"],
        )
        .expect("turns_total");
        let turn_duration_seconds = HistogramVec::new(
            HistogramOpts::new("turn_duration_seconds", "Agent turn duration in seconds.")
                .namespace(NS)
                .buckets(vec![0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0]),
            &["outcome"],
        )
        .expect("turn_duration_seconds");
        let tool_calls_total = IntCounterVec::new(
            Opts::new("tool_calls_total", "Tool calls a worker finished, by tool and outcome.")
                .namespace(NS),
            &["name", "outcome"],
        )
        .expect("tool_calls_total");
        let work_reclaims_total = IntCounter::with_opts(
            Opts::new(
                "work_reclaims_total",
                "Claims of a work item an earlier lease had held (expired, swept or released).",
            )
            .namespace(NS),
        )
        .expect("work_reclaims_total");
        let leases_expired_total = IntCounter::with_opts(
            Opts::new(
                "leases_expired_total",
                "Leases the sweeper moved back to ready after they ran out.",
            )
            .namespace(NS),
        )
        .expect("leases_expired_total");
        for c in [
            Box::new(turns_total.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(turn_duration_seconds.clone()),
            Box::new(tool_calls_total.clone()),
            Box::new(work_reclaims_total.clone()),
            Box::new(leases_expired_total.clone()),
        ] {
            registry.register(c).expect("register selfhost metric");
        }
        Self {
            registry,
            turns_total,
            turn_duration_seconds,
            tool_calls_total,
            work_reclaims_total,
            leases_expired_total,
        }
    }
}

static METRICS: Lazy<LoopMetrics> = Lazy::new(LoopMetrics::new);

pub fn record_turn(outcome: &str, seconds: f64) {
    METRICS.turns_total.with_label_values(&[outcome]).inc();
    METRICS
        .turn_duration_seconds
        .with_label_values(&[outcome])
        .observe(seconds);
}

pub fn record_tool_call(name: &str, outcome: &str) {
    METRICS.tool_calls_total.with_label_values(&[name, outcome]).inc();
}

pub fn record_work_reclaim() {
    METRICS.work_reclaims_total.inc();
}

pub fn record_expired_leases(n: u64) {
    METRICS.leases_expired_total.inc_by(n);
}

/// Values read at scrape time, in a fresh registry each scrape.
async fn scrape_registry(st: &SelfHostState) -> prometheus::Result<Registry> {
    let reg = Registry::new();
    let sessions = IntGaugeVec::new(
        Opts::new("sessions", "Sessions by status.").namespace(NS),
        &["status"],
    )?;
    let queue_depth = IntGaugeVec::new(
        Opts::new("queue_depth", "Ready work items per environment.").namespace(NS),
        &["environment"],
    )?;
    let workers_online = IntGaugeVec::new(
        Opts::new(
            "workers_online",
            "Workers seen in the last 60 seconds, per environment.",
        )
        .namespace(NS),
        &["environment"],
    )?;
    let egress_requests = IntCounterVec::new(
        Opts::new(
            "egress_requests_total",
            "Outbound requests checked by the egress guard, by host and result.",
        )
        .namespace(NS),
        &["host", "result"],
    )?;
    let egress_refusals = IntCounterVec::new(
        Opts::new(
            "egress_refusals_total",
            "Outbound requests the egress guard refused, by host.",
        )
        .namespace(NS),
        &["host"],
    )?;
    let store_up = IntGauge::with_opts(
        Opts::new("store_up", "1 when the scrape-time store reads worked.").namespace(NS),
    )?;
    reg.register(Box::new(sessions.clone()))?;
    reg.register(Box::new(queue_depth.clone()))?;
    reg.register(Box::new(workers_online.clone()))?;
    reg.register(Box::new(egress_requests.clone()))?;
    reg.register(Box::new(egress_refusals.clone()))?;
    reg.register(Box::new(store_up.clone()))?;

    for (host, s) in st.egress.guard().stats() {
        egress_requests
            .with_label_values(&[host.as_str(), "allowed"])
            .inc_by(s.allowed);
        egress_requests
            .with_label_values(&[host.as_str(), "refused"])
            .inc_by(s.refused);
        if s.refused > 0 {
            egress_refusals.with_label_values(&[host.as_str()]).inc_by(s.refused);
        }
    }

    let mut ok = true;
    for status in SESSION_STATUSES {
        sessions.with_label_values(&[*status]).set(0);
    }
    match st.store.count_sessions_by_status().await {
        Ok(rows) => {
            for (status, n) in rows {
                sessions.with_label_values(&[status.as_str()]).set(n);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "metrics: cannot count sessions");
            ok = false;
        }
    }

    let since = Utc::now() - Duration::seconds(ONLINE_SECS);
    let mut after: Option<String> = None;
    'pages: loop {
        let page = match st.store.list_environments(100, after.as_deref()).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "metrics: cannot list environments");
                ok = false;
                break;
            }
        };
        for env in &page {
            match (
                st.store.queue_depth(&env.id).await,
                st.store.count_workers(&env.id, since).await,
            ) {
                (Ok(q), Ok(w)) => {
                    queue_depth.with_label_values(&[env.id.as_str()]).set(q);
                    workers_online.with_label_values(&[env.id.as_str()]).set(w);
                }
                (Err(e), _) | (_, Err(e)) => {
                    tracing::warn!(error = %e, environment = %env.id, "metrics: store read failed");
                    ok = false;
                    break 'pages;
                }
            }
        }
        match page.last() {
            Some(last) if page.len() == 100 => after = Some(last.id.clone()),
            _ => break,
        }
    }
    store_up.set(i64::from(ok));
    Ok(reg)
}

/// The Prometheus text exposition for this server.
pub async fn render(st: &SelfHostState) -> Result<String, String> {
    let mut families = METRICS.registry.gather();
    let scrape = scrape_registry(st).await.map_err(|e| e.to_string())?;
    families.extend(scrape.gather());
    let mut buf = Vec::new();
    TextEncoder::new()
        .encode(&families, &mut buf)
        .map_err(|e| e.to_string())?;
    String::from_utf8(buf).map_err(|e| e.to_string())
}

async fn metrics_handler(State(st): State<SelfHostState>) -> Response {
    match render(&st).await {
        Ok(text) => (
            StatusCode::OK,
            [(CONTENT_TYPE, TextEncoder::new().format_type().to_string())],
            text,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "metrics: render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "metrics render failed").into_response()
        }
    }
}

/// `GET /metrics` and nothing else. Never merge this into the main router.
pub fn metrics_router(state: SelfHostState) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Start the metrics listener when it is enabled. Returns the bound address.
pub async fn spawn(state: SelfHostState) -> Result<Option<SocketAddr>, String> {
    let prom = state.settings.observability.prometheus.clone();
    if !prom.enabled {
        return Ok(None);
    }
    let listener = tokio::net::TcpListener::bind(&prom.listen)
        .await
        .map_err(|e| format!("cannot listen for metrics on {}: {e}", prom.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if !addr.ip().is_loopback() {
        tracing::warn!(
            addr = %addr,
            "metrics listener is not on loopback; keep it off public networks (it has no auth)"
        );
    }
    let app = metrics_router(state);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "metrics listener stopped");
        }
    });
    tracing::info!(addr = %addr, "metrics listening (GET /metrics)");
    Ok(Some(addr))
}
