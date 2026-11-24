//! Trace helpers for the agent loop and the work queue.
//!
//! One trace runs through a session:
//!
//! ```text
//! session                       (root, stored on sessions.traceparent)
//! └─ session.turn               (one per turn; start_turn / TurnSpan::finish)
//!    └─ tool_call               (stored on tool_calls.traceparent)
//!       └─ work.result          (the worker's result for that call)
//!    work.claim                 (child of the span that enqueued the work
//!                                item, stored on work_items.traceparent)
//! ```
//!
//! Spans are ordinary `tracing` spans, so with `OTEL_EXPORTER_OTLP_ENDPOINT`
//! set they go out through `crate::otel`'s OTLP/HTTP JSON exporter. The
//! parent of a span that starts in another request (a worker's claim or
//! result) comes from the `traceparent` stored on the row, set with
//! [`OpenTelemetrySpanExt::set_parent`]. With OTel off, trace ids still come
//! from `netops::hops` (W3C format), are stored on the rows, and appear in
//! the JSON logs as the `trace_id`/`span_id` span fields.
//!
//! Telemetry never fails a turn: store errors here are logged and dropped.
//! Each helper also feeds the Prometheus series in
//! [`super::metrics_listener`].

use std::time::Instant;

use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState};
use tracing::field::Empty;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use super::metrics_listener;
use super::store::{DynStore, StoreResult, ToolCallRecord, TraceTarget, WorkItem};
use crate::netops::hops::{self, TraceParent};

/// `service.name` of the Agents Server's traces when neither
/// `OTEL_SERVICE_NAME` nor `OTEL_RESOURCE_ATTRIBUTES` names one.
pub const DEFAULT_SERVICE_NAME: &str = "allternit-agents-server";

/// A span plus its W3C trace context.
#[derive(Debug, Clone)]
pub struct Traced {
    pub span: Span,
    pub traceparent: TraceParent,
}

impl Traced {
    /// The `traceparent` header value, e.g. to hand to the gizzi child.
    pub fn header(&self) -> String {
        self.traceparent.header()
    }
}

/// An OTel remote context for a stored traceparent.
pub fn remote_context(tp: &TraceParent) -> Option<opentelemetry::Context> {
    let trace_id = TraceId::from_hex(&tp.trace_id).ok()?;
    let span_id = SpanId::from_hex(&tp.span_id).ok()?;
    let flags = if tp.sampled {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::default()
    };
    let sc = SpanContext::new(trace_id, span_id, flags, true, TraceState::default());
    sc.is_valid()
        .then(|| opentelemetry::Context::new().with_remote_span_context(sc))
}

/// The span's OTel context as a traceparent. None when no OTel layer is
/// installed or the span is disabled.
pub fn otel_traceparent(span: &Span) -> Option<TraceParent> {
    let cx = span.context();
    let span_ref = cx.span();
    let sc = span_ref.span_context();
    if !sc.is_valid() {
        return None;
    }
    Some(TraceParent {
        trace_id: sc.trace_id().to_string(),
        span_id: sc.span_id().to_string(),
        sampled: sc.is_sampled(),
    })
}

/// Parent `span` under `parent` (when given) and work out its traceparent:
/// from OTel when it is on, else a hops child of the parent (or a new trace).
/// The span must declare `trace_id` and `span_id` fields (as `Empty`).
pub fn attach(span: Span, parent: Option<&TraceParent>) -> Traced {
    if let Some(cx) = parent.and_then(remote_context) {
        span.set_parent(cx);
    }
    let tp = otel_traceparent(&span)
        .unwrap_or_else(|| parent.map(hops::child).unwrap_or_else(hops::new_trace));
    span.record("trace_id", tp.trace_id.as_str());
    span.record("span_id", tp.span_id.as_str());
    Traced {
        span,
        traceparent: tp,
    }
}

async fn stored(store: &DynStore, target: TraceTarget, id: &str) -> Option<TraceParent> {
    match store.get_traceparent(target, id).await {
        Ok(v) => v.as_deref().and_then(hops::parse),
        Err(e) => {
            tracing::debug!(error = %e, id, table = target.table(), "no stored traceparent");
            None
        }
    }
}

async fn save(store: &DynStore, target: TraceTarget, id: &str, t: &Traced) {
    if let Err(e) = store.set_traceparent(target, id, &t.header()).await {
        tracing::warn!(error = %e, id, table = target.table(), "could not store traceparent");
    }
}

/// The session's root trace. Read from `sessions.traceparent`, or started
/// now as a short `session` span and stored.
pub async fn session_trace(store: &DynStore, session_id: &str) -> StoreResult<TraceParent> {
    if let Some(tp) = stored(store, TraceTarget::Session, session_id).await {
        return Ok(tp);
    }
    let span = tracing::info_span!(
        "session",
        otel.name = "session",
        session.id = %session_id,
        trace_id = Empty,
        span_id = Empty,
    );
    let t = attach(span, None);
    store
        .set_traceparent(TraceTarget::Session, session_id, &t.header())
        .await?;
    Ok(t.traceparent)
    // The span ends here; its id stays the parent of every turn.
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Error,
    Interrupted,
}

impl TurnOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnOutcome::Completed => "completed",
            TurnOutcome::Error => "error",
            TurnOutcome::Interrupted => "interrupted",
        }
    }
}

/// A running `session.turn` span. Call [`TurnSpan::finish`] when the turn
/// ends; that ends the span and records the turn metrics.
#[derive(Debug)]
pub struct TurnSpan {
    pub traced: Traced,
    started: Instant,
}

impl TurnSpan {
    pub fn span(&self) -> &Span {
        &self.traced.span
    }

    pub fn traceparent(&self) -> &TraceParent {
        &self.traced.traceparent
    }

    pub fn finish(self, outcome: TurnOutcome) {
        let span = &self.traced.span;
        span.record("turn.outcome", outcome.as_str());
        if outcome == TurnOutcome::Error {
            span.record("otel.status_code", "ERROR");
        }
        metrics_listener::record_turn(outcome.as_str(), self.started.elapsed().as_secs_f64());
    }
}

/// Start a `session.turn` span under the session's root trace.
pub async fn start_turn(store: &DynStore, session_id: &str, model: &str) -> TurnSpan {
    let root = match session_trace(store, session_id).await {
        Ok(tp) => Some(tp),
        Err(e) => {
            tracing::warn!(error = %e, session_id, "no session trace; the turn starts a new one");
            None
        }
    };
    let span = tracing::info_span!(
        "session.turn",
        otel.name = "session.turn",
        otel.status_code = Empty,
        session.id = %session_id,
        gen_ai.request.model = %model,
        turn.outcome = Empty,
        trace_id = Empty,
        span_id = Empty,
    );
    TurnSpan {
        traced: attach(span, root.as_ref()),
        started: Instant::now(),
    }
}

/// A `tool_call` span under the turn, stored on the tool call row (insert
/// the row first). The worker's `work.result` joins it later.
pub async fn start_tool_call(
    store: &DynStore,
    turn: &Traced,
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
) -> Traced {
    let span = tracing::info_span!(
        "tool_call",
        otel.name = "tool_call",
        session.id = %session_id,
        tool_call.id = %tool_call_id,
        gen_ai.tool.name = %tool_name,
        trace_id = Empty,
        span_id = Empty,
    );
    let t = attach(span, Some(&turn.traceparent));
    save(store, TraceTarget::ToolCall, tool_call_id, &t).await;
    t
}

/// Remember which span enqueued a work item, so its `work.claim` joins the
/// same trace. Call right after `enqueue_work`.
pub async fn record_work_enqueued(store: &DynStore, work_id: &str, parent: &Traced) {
    save(store, TraceTarget::Work, work_id, parent).await;
}

/// A `work.claim` span for a successful claim: child of the span that
/// enqueued the item, else of the session root. Counts a re-claim when the
/// item was leased before.
pub async fn work_claim_span(store: &DynStore, item: &WorkItem, worker_id: &str) -> Traced {
    let parent = match stored(store, TraceTarget::Work, &item.id).await {
        Some(tp) => Some(tp),
        None => stored(store, TraceTarget::Session, &item.session_id).await,
    };
    if item.lease_generation > 1 {
        metrics_listener::record_work_reclaim();
    }
    let span = tracing::info_span!(
        "work.claim",
        otel.name = "work.claim",
        otel.kind = "consumer",
        work.id = %item.id,
        session.id = %item.session_id,
        environment.id = %item.environment_id,
        worker.id = %worker_id,
        lease.generation = item.lease_generation,
        trace_id = Empty,
        span_id = Empty,
    );
    attach(span, parent.as_ref())
}

/// A `work.result` span for a stored tool result: child of the tool call
/// span. Records the tool call metric. Call once, when
/// `submit_tool_result` returned `stored: true`.
pub async fn work_result_span(store: &DynStore, call: &ToolCallRecord) -> Traced {
    let mut parent = stored(store, TraceTarget::ToolCall, &call.id).await;
    if parent.is_none() {
        if let Some(w) = &call.work_id {
            parent = stored(store, TraceTarget::Work, w).await;
        }
    }
    if parent.is_none() {
        parent = stored(store, TraceTarget::Session, &call.session_id).await;
    }
    let outcome = if call.is_error { "error" } else { "success" };
    metrics_listener::record_tool_call(&call.name, outcome);
    let span = tracing::info_span!(
        "work.result",
        otel.name = "work.result",
        otel.status_code = Empty,
        tool_call.id = %call.id,
        gen_ai.tool.name = %call.name,
        work.id = call.work_id.as_deref().unwrap_or(""),
        session.id = %call.session_id,
        tool.outcome = outcome,
        trace_id = Empty,
        span_id = Empty,
    );
    if call.is_error {
        span.record("otel.status_code", "ERROR");
    }
    attach(span, parent.as_ref())
}

/// Record leases the sweeper moved back to ready (`requeue_expired_work`).
pub fn record_expired_leases(n: u64) {
    metrics_listener::record_expired_leases(n);
}
