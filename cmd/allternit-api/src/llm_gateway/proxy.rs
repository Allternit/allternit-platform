//! OpenAI-compatible chat completions backed by the Gizzi runtime.
//!
//! Flow: parse/validate → allowlist → idempotency → model resolution →
//! Gizzi session (fresh, or reused via `x-allternit-session-id`) → send
//! message → collect the response off the shared Gizzi event bus
//! (`gizzi_bus`) → OpenAI-shaped response (JSON or SSE stream).
//!
//! Every completed request — ok or error — writes one `llm_usage_events`
//! row with token/cost numbers taken verbatim from Gizzi events. The
//! collected [`RequestOutcome`] struct flows into the single
//! [`record_usage_event`] choke point, where B4 (pricing recompute,
//! Prometheus series) and B5 (routing-decision persistence) plug in.
//!
//! Notes / deliberate v1 limitations:
//! - `tools`/`tool_choice` are validated but not forwarded: Gizzi's agent
//!   loop owns tool execution and its `PromptInput` has no OpenAI-style tool
//!   field. Tool-use happens inside Gizzi; the final text is what returns.
//! - `Idempotency-Key` is honored for non-streaming requests only (a stream
//!   cannot be replayed from a stored body).
//! - P1.6: an opt-in in-memory response cache (`response_cache`) short-
//!   circuits identical non-streaming, tool-free requests before any Gizzi
//!   session/upstream call; hits return `x-allternit-cache: hit` and record
//!   zero-cost usage (same treatment as BYOK-served requests).
//! - B4 (pricing recompute + Prometheus counters) is wired into
//!   [`record_usage_event`]; B5 (routing policies) is wired into
//!   [`resolve_model`] — policy aliases resolve through `router::resolve`,
//!   and the derived fallback chain is sent to Gizzi as `fallbackModels`.

use axum::{
    extract::{Extension, Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::StreamExt;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tracing::{info, warn, Instrument};

use crate::db::DbHandle;
use crate::AppState;

use super::auth::LlmKeyContext;
use super::gizzi_bus::{self, GizziEvent};
use super::inference_hooks::{self, InferenceHooks, PreHookOutcome};
use super::llm_pricing::{self, TokenBreakdown};
use super::refusal;
use super::router::{self, RoutingDecision};
use super::safety;
use super::translate::{
    message_to_gizzi_parts, messages_to_gizzi_parts, new_completion_id, stream_error_data,
    validate_request, Annotation, ChatCompletionChunk, ChatCompletionRequest,
    ChatCompletionResponse, ChatMessage, FileCitation, ImageUrlPart, MessageContent,
    OpenAiErrorResponse, ToolCall, ToolCallDelta, ToolCallFunction, UrlCitation, Usage,
};
use super::context_cache;

/// Hard cap on a single completion, from send to `session.status` idle.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// Routing-policy aliases accepted in the `model` field. Aliases are scored
/// by `router::resolve` (B5); when routing cannot decide, the request
/// degrades to a Gizzi `auto/auto` passthrough.
pub const POLICY_ALIASES: [&str; 6] = [
    "auto",
    "allternit-balanced",
    "allternit-code",
    "allternit-reasoning",
    "allternit-knowledge",
    "allternit-instruct",
];

/// Whether the resolved provider is Anthropic, which supports native citations.
fn is_anthropic_provider(requested: &str, resolved: &ResolvedModel) -> bool {
    requested.starts_with("anthropic/")
        || resolved.provider_id == "anthropic"
        || requested.starts_with("claude-")
        || resolved.model_id.starts_with("claude-")
}

/// Resolve any `file_id` references in message content to inline base64 data
/// URLs. Session-scoped files are checked first, then the global files table.
async fn resolve_file_references(
    db: &DbHandle,
    messages: Vec<ChatMessage>,
) -> Result<Vec<ChatMessage>, OpenAiErrorResponse> {
    let mut out = messages;
    for message in &mut out {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else {
            continue;
        };
        for part in parts {
            let Some(file_id) = part.file_id.as_deref().filter(|s| !s.is_empty()) else {
                continue;
            };
            let (mime, bytes) = load_file_by_id(db, file_id).await?;
            let data_url = format!("data:{};base64,{}", mime, STANDARD.encode(&bytes));
            part.image_url = Some(ImageUrlPart {
                url: data_url,
                detail: None,
            });
            part.file_id = None;
        }
    }
    Ok(out)
}

async fn load_file_by_id(
    db: &DbHandle,
    file_id: &str,
) -> Result<(String, Vec<u8>), OpenAiErrorResponse> {
    let db = db.clone();
    let file_id = file_id.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| {
            OpenAiErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to open database: {e}"),
                "server_error",
                None,
                Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
            )
        })?;

        // Prefer session-scoped files.
        let session_row: Option<(String, String)> = conn
            .query_row(
                "SELECT storage_path, mime_type FROM session_files WHERE id = ?1",
                params![file_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| {
                OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("database error: {e}"),
                    "server_error",
                    None,
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
            })?;
        if let Some((path, mime)) = session_row {
            let bytes = std::fs::read(&path).map_err(|e| {
                OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to read session file: {e}"),
                    "server_error",
                    None,
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
            })?;
            return Ok((mime, bytes));
        }

        // Fall back to the global files table used by /v1/files.
        let global_row: Option<(Vec<u8>, String)> = conn
            .query_row(
                "SELECT bytes, filename FROM files WHERE id = ?1",
                params![file_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| {
                OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("database error: {e}"),
                    "server_error",
                    None,
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
            })?;
        if let Some((bytes, filename)) = global_row {
            let mime = if filename.ends_with(".pdf") {
                "application/pdf"
            } else {
                "application/octet-stream"
            };
            return Ok((mime.to_string(), bytes));
        }

        Err(OpenAiErrorResponse::invalid_request(
            format!("file_id `{file_id}` not found."),
            Some("file_id"),
        ))
    })
    .await
    .map_err(|e| {
        OpenAiErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load file: {e}"),
            "server_error",
            None,
            Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
        )
    })?
}

/// An `in_progress` idempotency row older than this is assumed abandoned
/// (worker died mid-request) and may be retried.
const IDEMPOTENCY_STALE_SECS: i64 = 600;

const SESSION_HEADER: &str = "x-allternit-session-id";
const FALLBACK_HEADER: &str = "x-allternit-fallback";
/// Set by the native batch executor on every sub-request so the usage row can
/// be attributed to its batch (task G10). Internal header — not part of the
/// public OpenAI surface.
const BATCH_HEADER: &str = "x-allternit-batch-id";

fn gizzi_base() -> String {
    crate::v1_routes::gizzi_base()
}

fn http_client() -> reqwest::Client {
    // Bounds the HTTP calls to Gizzi (session create, catalog); the
    // completion wait itself is bounded by COMPLETION_TIMEOUT on the event
    // collection loop, not by this client.
    reqwest::Client::builder()
        .timeout(COMPLETION_TIMEOUT + Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

/// Cost-attribution labels (task G8/G10): the explicit request `tags` win;
/// when absent, the virtual key's inherited defaults apply. Returned in the
/// serialized JSON-object form stored on `llm_usage_events.tags`.
fn resolve_outcome_tags(
    request_tags: &Option<std::collections::BTreeMap<String, String>>,
    key: &LlmKeyContext,
) -> Option<String> {
    let effective = request_tags.clone().or_else(|| key.tags.clone())?;
    serde_json::to_string(&effective).ok()
}

/// Batch attribution (task G10): the native batch executor stamps each
/// sub-request with `X-Allternit-Batch-Id` (the `llm_batches.id`).
fn batch_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(BATCH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_string)
}

/// Read a JSON number as i64 regardless of its wire representation.
fn json_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64)))
        .unwrap_or(0)
}

// ─── Model resolution ───────────────────────────────────────────────────────

/// Resolved target model for a request.
struct ResolvedModel {
    /// Policy alias when the request named one (auto, allternit-*).
    policy: Option<String>,
    provider_id: String,
    model_id: String,
    /// Cross-provider failover chain sent to Gizzi as `fallbackModels`.
    fallbacks: Vec<router::CandidateRef>,
    /// The routing decision, when a policy alias drove resolution. Persisted
    /// to `llm_routing_decisions` (linked to the usage row) at record time.
    routing_decision: Option<RoutingDecision>,
}

impl ResolvedModel {
    /// `provider/model`, or None for policy-driven resolution (the requested
    /// alias is what allowlists check).
    fn full_id(&self) -> Option<String> {
        if self.policy.is_some() {
            None
        } else {
            Some(format!("{}/{}", self.provider_id, self.model_id))
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderCatalog {
    #[serde(default)]
    pub all: Vec<ProviderEntry>,
    #[serde(default)]
    pub connected: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    pub id: String,
    #[serde(default)]
    pub models: HashMap<String, ModelEntry>,
}

/// Catalog model entry. `cost` feeds routing's cost-efficiency score and the
/// cost-quartile heuristic; `status` filters non-active models (Gizzi's
/// `status` field; absent means active).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelEntry {
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub cost: Option<ModelCost>,
    #[serde(default)]
    pub limit: Option<ModelLimit>,
    #[serde(default)]
    pub deprecated: Option<bool>,
    #[serde(default)]
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ModelLimit {
    #[serde(default)]
    pub context: u64,
    #[serde(default)]
    pub output: u64,
}

/// $ per 1M tokens as reported by the Gizzi catalog (extra fields ignored).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ModelCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
}

async fn fetch_catalog(
    client: &reqwest::Client,
    base: &str,
) -> Result<ProviderCatalog, OpenAiErrorResponse> {
    let resp = client
        .get(format!("{base}/v1/provider"))
        .send()
        .await
        .map_err(|err| {
            OpenAiErrorResponse::upstream(
                format!("Gizzi runtime unreachable: {err}"),
                "upstream_error",
            )
        })?;
    if !resp.status().is_success() {
        return Err(OpenAiErrorResponse::upstream(
            format!("Gizzi provider catalog returned {}", resp.status()),
            "upstream_error",
        ));
    }
    resp.json::<ProviderCatalog>().await.map_err(|err| {
        OpenAiErrorResponse::upstream(
            format!("Gizzi provider catalog was not decodable: {err}"),
            "upstream_error",
        )
    })
}

/// Resolve the requested `model` string to a concrete provider/model pair.
///
/// - policy alias → B5 routing: `router::resolve` scores the catalog against
///   benchmark weights; the winner becomes the primary model and the derived
///   chain (top 3 distinct providers) is sent as `fallbackModels`. When the
///   router cannot decide (no connected providers / no candidates) the
///   request degrades to the previous `auto/auto` passthrough so completions
///   keep working.
/// - `provider/model` → passed through unchanged as the primary; the
///   fallback chain is still derived from the balanced scorecard (routing is
///   bypassed for *selection*, borrowed for *failover*).
/// - bare model id → looked up in the Gizzi catalog, preferring connected
///   providers (the ones that can actually serve it); same fallback
///   derivation as explicit requests.
async fn resolve_model(
    db: &DbHandle,
    tenant_id: Option<&str>,
    client: &reqwest::Client,
    base: &str,
    requested: &str,
) -> Result<ResolvedModel, OpenAiErrorResponse> {
    if POLICY_ALIASES.contains(&requested) {
        let catalog = fetch_catalog(client, base).await?;
        let decision = {
            let db = db.clone();
            let tenant_id = tenant_id.map(str::to_string);
            let requested = requested.to_string();
            tokio::task::spawn_blocking(move || -> Result<RoutingDecision, router::RouterError> {
                let conn = db.connect().map_err(router::RouterError::from)?;
                router::resolve(&conn, tenant_id.as_deref(), &requested, &catalog)
            })
            .await
        };
        return match decision {
            Ok(Ok(decision)) => Ok(ResolvedModel {
                policy: Some(requested.to_string()),
                provider_id: decision.winner.provider_id.clone(),
                model_id: decision.winner.model_id.clone(),
                fallbacks: decision.fallbacks.clone(),
                routing_decision: Some(decision),
            }),
            Ok(Err(err)) => {
                // Routing could not decide — degrade to Gizzi auto so the
                // completion still works (previous behavior).
                warn!(policy = requested, error = ?err, "Routing failed; falling back to auto passthrough");
                Ok(ResolvedModel {
                    policy: Some(requested.to_string()),
                    provider_id: "auto".to_string(),
                    model_id: "auto".to_string(),
                    fallbacks: Vec::new(),
                    routing_decision: None,
                })
            }
            Err(err) => {
                warn!(error = %err, "Routing task failed; falling back to auto passthrough");
                Ok(ResolvedModel {
                    policy: Some(requested.to_string()),
                    provider_id: "auto".to_string(),
                    model_id: "auto".to_string(),
                    fallbacks: Vec::new(),
                    routing_decision: None,
                })
            }
        };
    }

    if let Some((provider, model)) = requested.split_once('/') {
        if provider.is_empty() || model.is_empty() {
            return Err(OpenAiErrorResponse::invalid_request(
                format!("Invalid model id `{requested}`; expected `provider/model`."),
                Some("model"),
            ));
        }
        let fallbacks = explicit_fallbacks(db, tenant_id, client, base, provider, model).await;
        return Ok(ResolvedModel {
            policy: None,
            provider_id: provider.to_string(),
            model_id: model.to_string(),
            fallbacks,
            routing_decision: None,
        });
    }

    let catalog = fetch_catalog(client, base).await?;
    let offers = |provider: &&ProviderEntry| provider.models.contains_key(requested);
    let found = catalog
        .all
        .iter()
        .filter(|p| catalog.connected.contains(&p.id))
        .find(offers)
        .or_else(|| catalog.all.iter().find(offers));

    match found {
        Some(provider) => {
            let fallbacks =
                derive_fallbacks(db, tenant_id, &catalog, &provider.id, requested).await;
            Ok(ResolvedModel {
                policy: None,
                provider_id: provider.id.clone(),
                model_id: requested.to_string(),
                fallbacks,
                routing_decision: None,
            })
        }
        None => Err(OpenAiErrorResponse::new(
            StatusCode::BAD_REQUEST,
            format!("The model `{requested}` does not exist or is not offered by any provider."),
            "invalid_request_error",
            Some("model"),
            Some(crate::llm_gateway::translate::error_code::MODEL_NOT_FOUND),
        )),
    }
}

/// Fallback-chain derivation for explicit `provider/model` requests: fetch
/// the catalog and score it with the balanced policy. Tolerates a missing
/// catalog (chain simply stays empty) — explicit requests must not start
/// failing because routing inputs are unavailable.
async fn explicit_fallbacks(
    db: &DbHandle,
    tenant_id: Option<&str>,
    client: &reqwest::Client,
    base: &str,
    provider_id: &str,
    model_id: &str,
) -> Vec<router::CandidateRef> {
    match fetch_catalog(client, base).await {
        Ok(catalog) => derive_fallbacks(db, tenant_id, &catalog, provider_id, model_id).await,
        Err(err) => {
            warn!(error = %err.error.message, "Catalog unavailable; no fallback chain for explicit model");
            Vec::new()
        }
    }
}

/// Run `router::fallback_chain_for_explicit` off the async executor.
async fn derive_fallbacks(
    db: &DbHandle,
    tenant_id: Option<&str>,
    catalog: &ProviderCatalog,
    provider_id: &str,
    model_id: &str,
) -> Vec<router::CandidateRef> {
    let db = db.clone();
    let tenant_id = tenant_id.map(str::to_string);
    let provider_id = provider_id.to_string();
    let model_id = model_id.to_string();
    let catalog = catalog.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().ok()?;
        Some(router::fallback_chain_for_explicit(
            &conn,
            tenant_id.as_deref(),
            &catalog,
            &provider_id,
            &model_id,
        ))
    })
    .await
    .unwrap_or_default()
    .unwrap_or_default()
}

// ─── Gizzi session plumbing ─────────────────────────────────────────────────

async fn create_session(
    client: &reqwest::Client,
    base: &str,
    title: &str,
) -> Result<String, OpenAiErrorResponse> {
    let resp = client
        .post(format!("{base}/v1/session"))
        .json(&json!({ "title": title }))
        .send()
        .await
        .map_err(|err| {
            OpenAiErrorResponse::upstream(
                format!("Gizzi runtime unreachable: {err}"),
                "upstream_error",
            )
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(OpenAiErrorResponse::upstream(
            format!("Gizzi session creation failed ({status}): {body}"),
            "upstream_error",
        ));
    }
    let session: Value = resp.json().await.map_err(|err| {
        OpenAiErrorResponse::upstream(
            format!("Gizzi session creation was not decodable: {err}"),
            "upstream_error",
        )
    })?;
    session
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            OpenAiErrorResponse::upstream(
                "Gizzi session creation returned no id.",
                "upstream_error",
            )
        })
}

// ─── Event collection ───────────────────────────────────────────────────────

/// Usage numbers captured verbatim from Gizzi `message.updated` events.
#[derive(Debug, Clone, Default)]
pub struct GizziUsage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    /// Cache-write tokens, kept separately from `cached_tokens` (read) because
    /// the pricing recompute bills them at different rates.
    pub cache_write_tokens: i64,
    pub cost_microdollars: i64,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub finish: Option<String>,
}

/// What the collector learned from one bus event. `TextDelta` lets the
/// streaming path forward content as it arrives while the collector keeps
/// the aggregate.
enum EventEffect {
    None,
    TextDelta(String),
    ToolCallDelta(ToolCallDelta),
    Done,
    Error(String, String),
}

/// Accumulates incremental tool-call arguments for one tool call index.
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

/// Per-request aggregation of Gizzi bus events.
struct Collector {
    started: Instant,
    text: String,
    was_busy: bool,
    usage: GizziUsage,
    assistant_error: Option<(String, String)>,
    fallback_from: Option<String>,
    fallback_to: Option<String>,
    ttft: Option<Duration>,
    citations: Vec<Annotation>,
    tool_calls: BTreeMap<u32, ToolCallAccumulator>,
}

impl Collector {
    fn new(started: Instant) -> Self {
        Self {
            started,
            text: String::new(),
            was_busy: false,
            usage: GizziUsage::default(),
            assistant_error: None,
            fallback_from: None,
            fallback_to: None,
            ttft: None,
            citations: Vec::new(),
            tool_calls: BTreeMap::new(),
        }
    }

    /// Assemble the final tool_calls list from accumulated deltas, if any.
    fn final_tool_calls(&self) -> Option<Vec<ToolCall>> {
        if self.tool_calls.is_empty() {
            return None;
        }
        Some(
            self.tool_calls
                .values()
                .map(|acc| ToolCall {
                    id: acc.id.clone(),
                    call_type: "function".to_string(),
                    function: ToolCallFunction {
                        name: acc.name.clone(),
                        arguments: acc.arguments.clone(),
                    },
                })
                .collect(),
        )
    }

    /// Fold one bus event (already filtered to this session) into the
    /// collection state.
    fn handle_event(&mut self, event: &GizziEvent) -> EventEffect {
        let props = &event.properties;
        match event.event_type.as_str() {
            "message.part.delta" => {
                let field = props.get("field").and_then(Value::as_str).unwrap_or("text");
                match field {
                    "text" => {
                        if let Some(delta) = props.get("delta").and_then(Value::as_str) {
                            if self.ttft.is_none() {
                                self.ttft = Some(self.started.elapsed());
                            }
                            self.text.push_str(delta);
                            return EventEffect::TextDelta(delta.to_string());
                        }
                    }
                    "tool_calls" => {
                        if let Some(delta_json) = props.get("delta").and_then(Value::as_str) {
                            if let Ok(delta) = serde_json::from_str::<ToolCallDelta>(delta_json) {
                                if self.ttft.is_none() {
                                    self.ttft = Some(self.started.elapsed());
                                }
                                let acc = self.tool_calls.entry(delta.index).or_insert_with(|| {
                                    ToolCallAccumulator {
                                        id: delta.id.clone().unwrap_or_default(),
                                        name: delta
                                            .function
                                            .as_ref()
                                            .and_then(|f| f.name.clone())
                                            .unwrap_or_default(),
                                        arguments: String::new(),
                                    }
                                });
                                if let Some(func) = &delta.function {
                                    if let Some(name) = &func.name {
                                        acc.name.clone_from(name);
                                    }
                                    if let Some(args) = &func.arguments {
                                        acc.arguments.push_str(args);
                                    }
                                }
                                return EventEffect::ToolCallDelta(delta);
                            }
                        }
                    }
                    _ => {}
                }
                EventEffect::None
            }
            "message.updated" => {
                let Some(info) = props.get("info") else {
                    return EventEffect::None;
                };
                if info.get("role").and_then(Value::as_str) != Some("assistant") {
                    return EventEffect::None;
                }
                self.usage = parse_assistant_usage(info);
                self.citations = extract_citations(info);
                if let Some(error) = info.get("error") {
                    let name = error
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("upstream_error")
                        .to_string();
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Gizzi reported an assistant error")
                        .to_string();
                    self.assistant_error = Some((name, message));
                }
                EventEffect::None
            }
            "session.status" => {
                let status_type = props
                    .get("status")
                    .and_then(|s| s.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if status_type == "busy" {
                    self.was_busy = true;
                    EventEffect::None
                } else if status_type == "idle" && (self.was_busy || !self.text.is_empty()) {
                    EventEffect::Done
                } else {
                    EventEffect::None
                }
            }
            // Deprecated Gizzi event, still emitted alongside session.status.
            "session.idle" => {
                if self.was_busy || !self.text.is_empty() {
                    EventEffect::Done
                } else {
                    EventEffect::None
                }
            }
            "session.error" => {
                let error = props.get("error").cloned().unwrap_or(Value::Null);
                let name = error
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream_error")
                    .to_string();
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Gizzi session error")
                    .to_string();
                EventEffect::Error(name, message)
            }
            // Emitted by the Gizzi failover chain (Part A1); absent until it
            // lands, in which case fallback_* simply stay None.
            "session.model_fallback" => {
                let render = |key: &str| -> Option<String> {
                    let node = props.get(key)?;
                    let provider = node.get("providerID").and_then(Value::as_str)?;
                    let model = node.get("modelID").and_then(Value::as_str)?;
                    Some(format!("{provider}/{model}"))
                };
                if let Some(from) = render("from") {
                    self.fallback_from = Some(from);
                }
                if let Some(to) = render("to") {
                    self.fallback_to = Some(to);
                }
                EventEffect::None
            }
            _ => EventEffect::None,
        }
    }
}

/// Parse usage numbers off a Gizzi assistant message (`message.updated` info).
fn parse_assistant_usage(info: &Value) -> GizziUsage {
    let tokens = info.get("tokens");
    let cache = tokens.and_then(|t| t.get("cache"));
    let cost_dollars = info.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    GizziUsage {
        prompt_tokens: json_i64(tokens.and_then(|t| t.get("input"))),
        completion_tokens: json_i64(tokens.and_then(|t| t.get("output"))),
        reasoning_tokens: json_i64(tokens.and_then(|t| t.get("reasoning"))),
        cached_tokens: json_i64(cache.and_then(|c| c.get("read"))),
        cache_write_tokens: json_i64(cache.and_then(|c| c.get("write"))),
        cost_microdollars: (cost_dollars * 1_000_000.0).round() as i64,
        provider_id: info
            .get("providerID")
            .and_then(Value::as_str)
            .map(str::to_string),
        model_id: info
            .get("modelID")
            .and_then(Value::as_str)
            .map(str::to_string),
        finish: info
            .get("finish")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Extract citation annotations from a Gizzi assistant `message.updated` info
/// block. The runtime currently does not emit structured citations, so this
/// parses the documented shape ahead of the upstream landing and surfaces
/// OpenAI-style `message.annotations` when present.
fn extract_citations(info: &Value) -> Vec<Annotation> {
    let Some(citations) = info.get("citations").and_then(Value::as_array) else {
        return Vec::new();
    };
    citations
        .iter()
        .filter_map(|citation| {
            let cited_text = citation.get("cited_text").and_then(Value::as_str)?;
            if let Some(url) = citation.get("url").and_then(Value::as_str) {
                return Some(Annotation::UrlCitation {
                    url_citation: UrlCitation {
                        url: url.to_string(),
                        title: citation.get("title").and_then(Value::as_str).map(str::to_string),
                    },
                });
            }
            if let Some(file_id) = citation.get("file_id").and_then(Value::as_str) {
                return Some(Annotation::FileCitation {
                    file_citation: FileCitation {
                        file_id: file_id.to_string(),
                    },
                });
            }
            // No recognized source: emit a URL citation using the cited text as
            // the URL so the annotation is not silently dropped.
            Some(Annotation::UrlCitation {
                url_citation: UrlCitation {
                    url: cited_text.to_string(),
                    title: citation.get("title").and_then(Value::as_str).map(str::to_string),
                },
            })
        })
        .collect()
}

/// Everything learned about one completed request. This is the struct B4
/// (metering / pricing recompute) consumes — see the `record_usage_event`
/// call sites.
pub struct RequestOutcome {
    pub status: &'static str,
    pub error_type: Option<String>,
    pub usage: GizziUsage,
    pub policy: Option<String>,
    pub fallback_from: Option<String>,
    pub gizzi_session_id: Option<String>,
    pub latency_ms: i64,
    pub ttft_ms: Option<i64>,
    pub response_body: Option<Value>,
    /// B5 routing decision for policy-alias requests; persisted (linked to
    /// this row) by `record_usage_event`.
    pub routing_decision: Option<RoutingDecision>,
    /// Serialized `{key: value}` JSON for cost attribution (task G8): the
    /// request's `tags`, or the key's inherited default. `None` stores NULL.
    pub tags: Option<String>,
    /// `llm_batches.id` when this request ran as a native batch sub-request
    /// (task G10); `None` for interactive traffic.
    pub batch_id: Option<String>,
    /// `llm_context_caches.id` when the request referenced a reusable context
    /// cache (task G11); persisted on the usage row so cache-attributed spend
    /// is visible in /gateway/logs and /gateway/caching.
    pub context_cache_id: Option<String>,
}

/// Result of the collection loop: the aggregated state plus either success
/// or a terminal (error_type, message).
struct Collection {
    collector: Collector,
    failure: Option<(String, String)>,
}

/// Drive the send task and the event stream to completion. The send runs on
/// a task because Gizzi only answers `POST /session/:id/message` after its
/// agent loop settles; events must be collected concurrently.
async fn collect(
    mut send_task: JoinHandle<Result<reqwest::Response, reqwest::Error>>,
    events: impl futures::Stream<Item = GizziEvent> + Send + 'static,
    started: Instant,
) -> Collection {
    let mut collector = Collector::new(started);
    let mut send_done = false;
    let mut failure: Option<(String, String)> = None;
    tokio::pin!(events);
    let deadline = tokio::time::sleep(COMPLETION_TIMEOUT);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            send_result = &mut send_task, if !send_done => {
                send_done = true;
                match send_result {
                    Ok(Ok(resp)) if resp.status().is_success() => {}
                    Ok(Ok(resp)) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        // Preserve the 429 signal: failover::classify_outcome
                        // maps "rate_limit*" error types to the short cooldown.
                        let error_type = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                            "rate_limit_error"
                        } else {
                            "upstream_error"
                        };
                        failure = Some((
                            error_type.to_string(),
                            format!("Gizzi message endpoint returned {status}: {body}"),
                        ));
                    }
                    Ok(Err(err)) => {
                        failure = Some((
                            "upstream_error".to_string(),
                            format!("Failed to send message to Gizzi: {err}"),
                        ));
                    }
                    Err(err) => {
                        failure = Some((
                            "upstream_error".to_string(),
                            format!("Gizzi send task failed: {err}"),
                        ));
                    }
                }
                if failure.is_some() {
                    break;
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(event) => match collector.handle_event(&event) {
                        EventEffect::Done => break,
                        EventEffect::Error(name, message) => {
                            failure = Some((name, message));
                            break;
                        }
                        _ => {}
                    },
                    // The bus never closes in practice; treat as done.
                    None => break,
                }
            }
            _ = &mut deadline => {
                failure = Some((
                    "upstream_error".to_string(),
                    format!("Completion timed out after {}s.", COMPLETION_TIMEOUT.as_secs()),
                ));
                break;
            }
        }
    }

    Collection { collector, failure }
}

// ─── Usage recording (B4 choke point) ───────────────────────────────────────

/// Persist one `llm_usage_events` row for a completed request. When an
/// idempotency pre-insert exists (`status = 'in_progress'`) it is updated in
/// place, preserving the one-row-per-request invariant.
///
/// B4 hardening, all applied here at the single choke point:
/// - cost is recomputed server-side from the models.dev cache
///   ([`llm_pricing`]); both figures are stored and a |Δ| > 1% is flagged
///   (`cost_mismatch`) and logged. Billing reads
///   `COALESCE(recomputed_cost_microdollars, cost_microdollars)`.
/// - a B5 routing decision attached to the outcome is persisted to
///   `llm_routing_decisions`, linked by the usage row id, in the same
///   connection so both rows commit together.
/// - Prometheus counters/histograms (`crate::metrics`) are emitted.
pub(crate) fn record_usage_event(
    db: &DbHandle,
    key: &LlmKeyContext,
    outcome: &RequestOutcome,
    idempotency_key: Option<&str>,
) {
    // BYO-served requests (the caller supplied their own credential for the
    // provider that served the request) bill the customer's own cloud
    // console — meter the tokens but charge nothing.
    let byo = outcome
        .usage
        .provider_id
        .as_deref()
        .is_some_and(|provider_id| {
            super::route_credentials::has_any_credential(
                db,
                &key.user_id,
                key.tenant_id.as_deref(),
                provider_id,
            )
        });

    // B4: recompute cost from the models.dev cache (skipped → NULL when the
    // file or model is unavailable; the Gizzi-reported cost stands alone).
    let recomputed = if byo {
        Some(0)
    } else {
        match (&outcome.usage.provider_id, &outcome.usage.model_id) {
            (Some(provider_id), Some(model_id)) => llm_pricing::recompute_cost_microdollars(
                provider_id,
                model_id,
                &TokenBreakdown {
                    input: outcome.usage.prompt_tokens,
                    output: outcome.usage.completion_tokens,
                    reasoning: outcome.usage.reasoning_tokens,
                    cache_read: outcome.usage.cached_tokens,
                    cache_write: outcome.usage.cache_write_tokens,
                },
            ),
            _ => None,
        }
    };
    let gizzi_cost = if byo { 0 } else { outcome.usage.cost_microdollars };
    let cost_mismatch = recomputed.is_some_and(|re| llm_pricing::is_mismatch(re, gizzi_cost));
    if cost_mismatch {
        warn!(
            key_id = %key.key_id,
            provider = ?outcome.usage.provider_id,
            model = ?outcome.usage.model_id,
            gizzi_cost_microdollars = gizzi_cost,
            recomputed_cost_microdollars = ?recomputed,
            "LLM cost mismatch >1% between Gizzi-reported and gateway-recomputed cost"
        );
    }

    let result = (|| -> rusqlite::Result<String> {
        let conn = db.connect()?;
        let response_body = outcome.response_body.as_ref().map(Value::to_string);
        if let Some(idem) = idempotency_key {
            let updated = conn.execute(
                "UPDATE llm_usage_events SET
                    policy = ?2, provider_id = ?3, model_id = ?4, fallback_from = ?5,
                    prompt_tokens = ?6, completion_tokens = ?7, reasoning_tokens = ?8,
                    cached_tokens = ?9, cost_microdollars = ?10, latency_ms = ?11,
                    ttft_ms = ?12, status = ?13, error_type = ?14,
                    gizzi_session_id = ?15, response_body = ?16,
                    recomputed_cost_microdollars = ?17, cost_mismatch = ?18,
                    tags = ?19, batch_id = ?20, context_cache_id = ?21
                 WHERE idempotency_key = ?1 AND status = 'in_progress'",
                rusqlite::params![
                    idem,
                    outcome.policy,
                    outcome.usage.provider_id,
                    outcome.usage.model_id,
                    outcome.fallback_from,
                    outcome.usage.prompt_tokens,
                    outcome.usage.completion_tokens,
                    outcome.usage.reasoning_tokens,
                    outcome.usage.cached_tokens,
                    outcome.usage.cost_microdollars,
                    outcome.latency_ms,
                    outcome.ttft_ms,
                    outcome.status,
                    outcome.error_type,
                    outcome.gizzi_session_id,
                    response_body,
                    recomputed,
                    cost_mismatch as i64,
                    outcome.tags,
                    outcome.batch_id,
                    outcome.context_cache_id,
                ],
            )?;
            if updated > 0 {
                let row_id: String = conn.query_row(
                    "SELECT id FROM llm_usage_events WHERE idempotency_key = ?1",
                    rusqlite::params![idem],
                    |row| row.get(0),
                )?;
                if let Some(decision) = &outcome.routing_decision {
                    router::persist_decision(&conn, &row_id, decision)?;
                }
                return Ok(row_id);
            }
            warn!("idempotency pre-insert missing at record time; inserting fresh row");
        }
        let row_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO llm_usage_events
                (id, virtual_key_id, user_id, tenant_id, policy, provider_id, model_id,
                 fallback_from, prompt_tokens, completion_tokens, reasoning_tokens,
                 cached_tokens, cost_microdollars, latency_ms, ttft_ms, status,
                 error_type, gizzi_session_id, idempotency_key, response_body,
                 recomputed_cost_microdollars, cost_mismatch, tags, batch_id,
                 context_cache_id)
             VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                 ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
            rusqlite::params![
                row_id,
                key.key_id,
                key.user_id,
                key.tenant_id,
                outcome.policy,
                outcome.usage.provider_id,
                outcome.usage.model_id,
                outcome.fallback_from,
                outcome.usage.prompt_tokens,
                outcome.usage.completion_tokens,
                outcome.usage.reasoning_tokens,
                outcome.usage.cached_tokens,
                outcome.usage.cost_microdollars,
                outcome.latency_ms,
                outcome.ttft_ms,
                outcome.status,
                outcome.error_type,
                outcome.gizzi_session_id,
                idempotency_key,
                response_body,
                recomputed,
                cost_mismatch as i64,
                outcome.tags,
                outcome.batch_id,
                outcome.context_cache_id,
            ],
        )?;
        if let Some(decision) = &outcome.routing_decision {
            router::persist_decision(&conn, &row_id, decision)?;
        }
        Ok(row_id)
    })();

    match result {
        Ok(_row_id) => emit_llm_metrics(key, outcome, recomputed),
        Err(err) => {
            warn!(error = %err, key_id = %key.key_id, "Failed to record llm_usage_events row")
        }
    }
}

/// Emit the `llm_*` Prometheus series for one recorded request (B7). Cost
/// uses the recomputed figure when available (single source of truth).
fn emit_llm_metrics(key: &LlmKeyContext, outcome: &RequestOutcome, recomputed: Option<i64>) {
    let model = outcome.usage.model_id.as_deref().unwrap_or("unknown");
    let provider = outcome.usage.provider_id.as_deref().unwrap_or("unknown");
    let tenant = key.tenant_id.as_deref().unwrap_or("none");

    crate::metrics::record_llm_request(model, provider, outcome.status);
    crate::metrics::observe_llm_duration(model, outcome.latency_ms as f64 / 1000.0);
    if outcome.usage.prompt_tokens > 0 {
        crate::metrics::record_llm_tokens(model, "prompt", outcome.usage.prompt_tokens);
    }
    if outcome.usage.completion_tokens > 0 {
        crate::metrics::record_llm_tokens(model, "completion", outcome.usage.completion_tokens);
    }
    if outcome.usage.reasoning_tokens > 0 {
        crate::metrics::record_llm_tokens(model, "reasoning", outcome.usage.reasoning_tokens);
    }
    if outcome.usage.cached_tokens > 0 {
        crate::metrics::record_llm_tokens(model, "cached", outcome.usage.cached_tokens);
    }
    let effective_cost = recomputed.unwrap_or(outcome.usage.cost_microdollars);
    if effective_cost > 0 {
        crate::metrics::record_llm_cost(tenant, model, effective_cost);
    }
    if let Some(from) = &outcome.fallback_from {
        crate::metrics::inc_llm_fallback(from, &format!("{provider}/{model}"));
    }
}

// ─── Streaming client-disconnect guard ───────────────────────────────────────

/// Latest known state of an in-flight stream, shared with [`StreamUsageGuard`]
/// so a dropped stream can record its partial usage.
#[derive(Default)]
struct StreamProgress {
    /// Set when the stream reached its normal end (ok or handled error) —
    /// the guard then records nothing.
    disarmed: bool,
    usage: GizziUsage,
    fallback_from: Option<String>,
    ttft_ms: Option<i64>,
}

impl StreamProgress {
    fn sync_from(&mut self, collector: &Collector) {
        self.usage = collector.usage.clone();
        self.fallback_from = collector.fallback_from.clone();
        self.ttft_ms = collector.ttft.map(|d| d.as_millis() as i64);
    }
}

/// B4 gap fix: a streaming client that disconnects mid-stream used to leave
/// no usage row at all. This guard lives inside the SSE stream; when the
/// stream is dropped before its normal end (client disconnect, connection
/// reset), `Drop` records the partial usage seen so far with status
/// `client_disconnected`. The normal completion path disarms it before the
/// regular `record_usage_event` call, so exactly one row is written either
/// way.
struct StreamUsageGuard {
    db: DbHandle,
    key: LlmKeyContext,
    session_id: String,
    policy: Option<String>,
    routing_decision: Option<RoutingDecision>,
    started: Instant,
    progress: Arc<Mutex<StreamProgress>>,
    /// Cost-attribution labels applied to the disconnect-path usage row
    /// (same values the normal completion path records).
    tags: Option<String>,
    batch_id: Option<String>,
    /// Cache attribution (task G11), recorded on disconnect-path rows too.
    context_cache_id: Option<String>,
}

impl Drop for StreamUsageGuard {
    fn drop(&mut self) {
        let mut progress = self
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if progress.disarmed {
            return;
        }
        progress.disarmed = true;
        let outcome = RequestOutcome {
            status: "client_disconnected",
            error_type: None,
            usage: progress.usage.clone(),
            policy: self.policy.clone(),
            fallback_from: progress.fallback_from.clone(),
            gizzi_session_id: Some(self.session_id.clone()),
            latency_ms: self.started.elapsed().as_millis() as i64,
            ttft_ms: progress.ttft_ms,
            response_body: None,
            routing_decision: self.routing_decision.take(),
            tags: self.tags.clone(),
            batch_id: self.batch_id.clone(),
            context_cache_id: self.context_cache_id.clone(),
        };
        drop(progress);

        let db = self.db.clone();
        let key = self.key.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(move || record_usage_event(&db, &key, &outcome, None));
            }
            // No runtime context (should not happen under axum): record
            // synchronously rather than lose the row.
            Err(_) => record_usage_event(&db, &key, &outcome, None),
        }
    }
}

// ─── Idempotency ────────────────────────────────────────────────────────────

enum IdemBegin {
    /// No prior request with this key; an `in_progress` row was pre-inserted.
    Proceed,
    /// A completed request with this key exists — replay its stored body.
    Replay(String),
    /// Reject with this response (409 in-flight, or 500 on DB failure).
    Reject(Response),
}

fn idempotency_conflict(message: &str) -> Response {
    OpenAiErrorResponse::new(
        StatusCode::CONFLICT,
        message,
        "invalid_request_error",
        None,
        Some(crate::llm_gateway::translate::error_code::IDEMPOTENCY_CONFLICT),
    )
    .into_response()
}

async fn idempotency_begin(db: &DbHandle, key: &LlmKeyContext, idem_key: &str) -> IdemBegin {
    let db = db.clone();
    let key_id = key.key_id.clone();
    let user_id = key.user_id.clone();
    let tenant_id = key.tenant_id.clone();
    let idem = idem_key.to_string();

    let result = tokio::task::spawn_blocking(move || -> rusqlite::Result<IdemBegin> {
        let conn = db.connect()?;
        let existing = conn
            .query_row(
                "SELECT status, response_body,
                        CAST(strftime('%s', 'now') AS INTEGER)
                          - CAST(strftime('%s', created_at) AS INTEGER) AS age_secs
                 FROM llm_usage_events
                 WHERE idempotency_key = ?1 AND virtual_key_id = ?2",
                rusqlite::params![idem, key_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        if let Some((status, response_body, age_secs)) = existing {
            match (status.as_str(), response_body) {
                ("in_progress", _) if age_secs > IDEMPOTENCY_STALE_SECS => {
                    // Abandoned attempt (process died mid-request): allow retry.
                    conn.execute(
                        "DELETE FROM llm_usage_events WHERE idempotency_key = ?1 AND virtual_key_id = ?2",
                        rusqlite::params![idem, key_id],
                    )?;
                }
                ("in_progress", _) => {
                    return Ok(IdemBegin::Reject(idempotency_conflict(
                        "A request with this Idempotency-Key is already in progress.",
                    )));
                }
                (_, Some(body)) => return Ok(IdemBegin::Replay(body)),
                _ => {
                    return Ok(IdemBegin::Reject(idempotency_conflict(
                        "This Idempotency-Key was already used and cannot be replayed.",
                    )));
                }
            }
        }

        match conn.execute(
            "INSERT INTO llm_usage_events
                (id, virtual_key_id, user_id, tenant_id, status, idempotency_key)
             VALUES (?1, ?2, ?3, ?4, 'in_progress', ?5)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                key_id,
                user_id,
                tenant_id,
                idem
            ],
        ) {
            Ok(_) => Ok(IdemBegin::Proceed),
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // Lost a race against a concurrent request with the same key.
                Ok(IdemBegin::Reject(idempotency_conflict(
                    "A request with this Idempotency-Key is already in progress.",
                )))
            }
            Err(err) => Err(err),
        }
    })
    .await;

    match result {
        Ok(Ok(decision)) => decision,
        Ok(Err(err)) => {
            warn!(error = %err, "idempotency pre-check failed");
            IdemBegin::Reject(
                OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Internal error: {err}"),
                    "server_error",
                    None,
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
                .into_response(),
            )
        }
        Err(err) => {
            warn!(error = %err, "idempotency pre-check task failed");
            IdemBegin::Reject(
                OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Internal error: {err}"),
                    "server_error",
                    None,
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
                .into_response(),
            )
        }
    }
}

// ─── Handlers ───────────────────────────────────────────────────────────────

/// Deterministic, provider-independent estimate used for preflight sizing.
/// Four UTF-8 characters per token is intentionally simple and stable.
pub fn estimate_tokens(request: &ChatCompletionRequest) -> u64 {
    let characters = request
        .messages
        .iter()
        .map(|message| message.role.chars().count() + message.content_text().chars().count())
        .sum::<usize>();
    ((characters + 3) / 4) as u64
}

/// Token count for a chat-completions-shaped body.
///
/// OpenAI models are counted with `tiktoken-rs` so the preflight estimate
/// matches the upstream tokenizer. Unsupported model names fall back to the
/// provider-independent heuristic.
pub fn count_tokens_request(request: &ChatCompletionRequest) -> u64 {
    let model = strip_openai_prefix(&request.model);
    let messages: Vec<tiktoken_rs::ChatCompletionRequestMessage> = request
        .messages
        .iter()
        .map(|message| tiktoken_rs::ChatCompletionRequestMessage {
            role: message.role.clone(),
            content: Some(message.content_text()),
            name: message.name.clone(),
            function_call: None,
            tool_calls: Vec::new(),
            refusal: None,
        })
        .collect();

    tiktoken_rs::num_tokens_from_messages(model, &messages)
        .map(|count| count as u64)
        .unwrap_or_else(|_| estimate_tokens(request))
}

fn strip_openai_prefix(model: &str) -> &str {
    model
        .strip_prefix("openai/")
        .or_else(|| model.strip_prefix("azure/"))
        .unwrap_or(model)
}

/// `POST /tokens` — estimate input tokens and cost for a chat-completions-shaped body.
pub async fn count_tokens(
    Extension(key): Extension<LlmKeyContext>,
    body: axum::body::Bytes,
) -> Response {
    let request: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(err) => {
            return OpenAiErrorResponse::invalid_request(
                format!("Invalid request body: {err}"),
                None,
            )
            .into_response()
        }
    };
    if let Err(response) = validate_request(&request) {
        return response.into_response();
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
            Some(crate::llm_gateway::translate::error_code::PERMISSION_DENIED),
        )
        .into_response();
    }
    let input_tokens = count_tokens_request(&request);
    let estimated_cost = estimate_request_cost(&request.model, input_tokens, request.max_tokens);
    Json(json!({
        "object": "token_count",
        "model": request.model,
        "input_tokens": input_tokens,
        "estimated_cost": estimated_cost,
    }))
    .into_response()
}

fn estimate_request_cost(model: &str, input_tokens: u64, max_tokens: Option<u32>) -> Value {
    let snapshot = llm_pricing::pricing_snapshot();
    // Try exact match, then bare-model suffix match.
    let pricing = snapshot.get(model).or_else(|| {
        let suffix = format!("/{model}");
        snapshot.iter().find(|(k, _)| k.ends_with(&suffix)).map(|(_, v)| v)
    });
    let output_tokens = max_tokens.unwrap_or(0) as u64;
    match pricing {
        Some(p) => {
            let input_cost = (input_tokens as f64 * p.input) / 1_000_000.0;
            let output_cost = (output_tokens as f64 * p.output) / 1_000_000.0;
            json!({
                "currency": "USD",
                "input_cost": input_cost,
                "output_cost": output_cost,
                "total_cost": input_cost + output_cost,
                "priced": true,
            })
        }
        None => json!({
            "currency": "USD",
            "input_cost": null,
            "output_cost": null,
            "total_cost": null,
            "priced": false,
        }),
    }
}

fn local_api_base(state: &AppState) -> String {
    format!("http://127.0.0.1:{}", state.config.api_port())
}

/// `POST /chat/completions:best-of` — generate N candidates and return the
/// full set (or the single best when `best_of` is true). This is the native
/// Allternit partial / best-of sampling surface.
pub async fn best_of_completions(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mut request: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(err) => {
            return OpenAiErrorResponse::invalid_request(
                format!("Invalid request body: {err}"),
                None,
            )
            .into_response();
        }
    };
    if let Err(response) = validate_request(&request) {
        return response.into_response();
    }
    if !key.model_allowed(&request.model) {
        return OpenAiErrorResponse::new(
            StatusCode::FORBIDDEN,
            format!("This API key is not allowed to use model `{}`.", request.model),
            "permission_error",
            Some("model"),
            Some(crate::llm_gateway::translate::error_code::PERMISSION_DENIED),
        )
        .into_response();
    }

    let n = request.n.unwrap_or(1).min(10).max(1);
    let best_only = request.best_of.unwrap_or(false);
    request.stream = Some(false);
    request.n = None;
    request.best_of = None;

    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let base = local_api_base(&state);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_default();

    let body_json = match serde_json::to_value(&request) {
        Ok(v) => v,
        Err(err) => {
            return OpenAiErrorResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize request: {err}"),
                "server_error",
                None,
                Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
            )
            .into_response()
        }
    };

    let mut candidates: Vec<Value> = Vec::with_capacity(n as usize);
    for index in 0..n {
        let mut req = client
            .post(format!("{base}/v1/chat/completions"))
            .json(&body_json);
        if let Some(auth) = &auth_header {
            req = req.header(header::AUTHORIZATION, auth.clone());
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                let mut value: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => {
                        candidates.push(json!({
                            "index": index,
                            "error": { "message": text, "type": "parse_error" }
                        }));
                        continue;
                    }
                };
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("index".to_string(), json!(index));
                }
                if !status.is_success() {
                    candidates.push(json!({
                        "index": index,
                        "error": value,
                        "status": status.as_u16()
                    }));
                    continue;
                }
                candidates.push(value);
            }
            Err(err) => {
                candidates.push(json!({
                    "index": index,
                    "error": { "message": err.to_string(), "type": "request_error" }
                }));
            }
        }
    }

    if best_only {
        let best = candidates
            .iter()
            .filter_map(|c| {
                let content = c
                    .get("choices")?
                    .get(0)?
                    .get("message")?
                    .get("content")?
                    .as_str()?;
                let len = content.len();
                Some((len, c))
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, c)| c.clone())
            .unwrap_or_else(|| candidates.into_iter().next().unwrap_or(json!({})));
        return Json(best).into_response();
    }

    Json(json!({
        "object": "chat.completion.best_of",
        "model": request.model,
        "n": n,
        "candidates": candidates,
    }))
    .into_response()
}

/// `POST /chat/completions` — OpenAI-compatible completion via Gizzi.
///
/// OTel GenAI semantic-convention fields are declared here and recorded by
/// [`super::genai_spans`] once routing resolves the provider/model and the
/// outcome settles (recorded only when OTel export is enabled).
#[tracing::instrument(
    skip_all,
    name = "llm_gateway.chat_completions",
    fields(
        gen_ai.operation.name = "chat",
        gen_ai.system = tracing::field::Empty,
        gen_ai.request.model = tracing::field::Empty,
        gen_ai.response.model = tracing::field::Empty,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        error.type = tracing::field::Empty,
    )
)]
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let started = Instant::now();

    let mut request: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(err) => {
            return OpenAiErrorResponse::invalid_request(
                format!("Invalid request body: {err}"),
                None,
            )
            .into_response();
        }
    };
    if let Err(response) = validate_request(&request) {
        return response.into_response();
    }

    // On-premise safety classifier: reject obvious jailbreak / prompt-injection
    // attempts before any routing or spend occurs.
    if let Some(result) = safety::classify_messages(&request.messages) {
        warn!(category = %result.category, "Safety classifier blocked request");
        return OpenAiErrorResponse::new(
            StatusCode::BAD_REQUEST,
            format!("{}: {}", result.reason, result.category),
            "content_filter",
            None,
            Some(crate::llm_gateway::translate::error_code::CONTENT_POLICY_VIOLATION),
        )
        .into_response();
    }

    // Load organization-level inference hooks. Pre-hooks run before routing so
    // mutations (e.g., model overrides) participate in allowlist checks.
    let hooks: Option<InferenceHooks> = key
        .tenant_id
        .as_deref()
        .and_then(|org| inference_hooks::load_hooks(&state.db, org).ok().flatten());

    let mut body = body;
    if let Some(hooks) = &hooks {
        let hook_client = inference_hooks::hook_client();
        match inference_hooks::run_pre_hook(&hook_client, hooks, body.clone()).await {
            PreHookOutcome::Proceed(new_body) => {
                if new_body != body {
                    body = new_body;
                    request = match serde_json::from_slice(&body) {
                        Ok(request) => request,
                        Err(err) => {
                            return OpenAiErrorResponse::invalid_request(
                                format!("Pre-hook returned invalid request body: {err}"),
                                None,
                            )
                            .into_response();
                        }
                    };
                    if let Err(response) = validate_request(&request) {
                        return response.into_response();
                    }
                }
            }
            PreHookOutcome::Abort(response) => return response,
        }
    }

    // Resolve session-scoped (or global) file_id references to inline data so
    // downstream Gizzi parts carry the actual file bytes.
    request.messages = match resolve_file_references(&state.db, request.messages).await {
        Ok(messages) => messages,
        Err(response) => return response.into_response(),
    };

    // Prepend cached context messages when a cache id is supplied.
    if let Some(cache_id) = &request.context_cache_id {
        match context_cache::load_cache_messages(&state.db, &key.key_id, cache_id) {
            Ok(Some(cached)) => {
                // Cache-hit accounting (task G11): bump hits + stamp
                // last_used_at at apply time. Best-effort — accounting must
                // never fail the request.
                if let Err(err) = context_cache::record_cache_hit(&state.db, cache_id) {
                    warn!(error = %err, cache_id = %cache_id, "Failed to record context cache hit");
                }
                let mut combined = cached;
                combined.extend(request.messages);
                request.messages = combined;
            }
            Ok(None) => {
                return OpenAiErrorResponse::invalid_request(
                    format!("context_cache_id `{cache_id}` not found or expired."),
                    Some("context_cache_id"),
                )
                .into_response();
            }
            Err(err) => {
                return OpenAiErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to load context cache: {err}"),
                    "server_error",
                    Some("context_cache_id"),
                    Some(crate::llm_gateway::translate::error_code::INTERNAL_ERROR),
                )
                .into_response();
            }
        }
    }

    // Cost-attribution labels (G8/G10): explicit request tags, else the key's
    // inherited defaults; batch sub-requests arrive with X-Allternit-Batch-Id.
    let outcome_tags = resolve_outcome_tags(&request.tags, &key);
    let batch_id = batch_id_from_headers(&headers);
    // Cache attribution (G11): carried onto the usage row when present.
    let context_cache_id = request.context_cache_id.clone();

    let stream = request.stream.unwrap_or(false);

    // Idempotency-Key is honored for non-streaming requests only (a stream
    // cannot be replayed from a stored body; the header is ignored there).
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if stream && idempotency_key.is_some() {
        warn!("Idempotency-Key header ignored for streaming request");
    }

    let base = gizzi_base();
    let client = http_client();

    // Resolve the requested model (policy alias, provider/model, or bare id).
    // Policy aliases go through B5 routing (winner + fallback chain).
    let resolved = match resolve_model(
        &state.db,
        key.tenant_id.as_deref(),
        &client,
        &base,
        &request.model,
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(response) => return response.into_response(),
    };

    // G13 data residency: drop candidates whose provider region violates the
    // org's pinned regions — the failover chain included. A compliant fallback
    // is promoted when the primary violates; when nothing is compliant the
    // request fails with data_residency_violation (never routed around).
    let mut resolved = resolved;
    {
        let primary = super::failover::ModelRef {
            provider_id: resolved.provider_id.clone(),
            model_id: resolved.model_id.clone(),
        };
        let fallback_refs: Vec<super::failover::ModelRef> = resolved
            .fallbacks
            .iter()
            .map(|c| super::failover::ModelRef {
                provider_id: c.provider_id.clone(),
                model_id: c.model_id.clone(),
            })
            .collect();
        match super::data_residency::enforce(
            &state.db,
            key.tenant_id.as_deref(),
            &primary,
            &fallback_refs,
        )
        .await
        {
            Ok(Some(outcome)) => {
                let filtered_primary = outcome.primary.full_id();
                resolved.provider_id = outcome.primary.provider_id;
                resolved.model_id = outcome.primary.model_id;
                resolved.fallbacks = outcome
                    .fallbacks
                    .iter()
                    .map(|m| router::CandidateRef {
                        provider_id: m.provider_id.clone(),
                        model_id: m.model_id.clone(),
                    })
                    .collect();
                if outcome.filtered {
                    info!(
                        org_id = %key.tenant_id.as_deref().unwrap_or(""),
                        primary = %filtered_primary,
                        "Data-residency policy filtered provider candidates"
                    );
                }
            }
            Ok(None) => {}
            Err(err_response) => return err_response.into_response(),
        }
    }

    // Provider routing (Products/ProviderRouting.md): the tenant's policy
    // pins which backend serves the active model. Resolved per model — the
    // primary below, and each failover attempt re-resolves its own pin in the
    // retry loop. Org-less keys inherit the platform-global (NULL-tenant) row.
    let provider_routing_policy = super::provider_routing::load_policy(
        &state.db,
        key.tenant_id.as_deref().unwrap_or(""),
    )
    .ok()
    .flatten();
    let provider_routing_primary = provider_routing_policy.as_ref().and_then(|policy| {
        super::provider_routing::resolve_for_model(policy, &resolved.provider_id, &resolved.model_id)
    });

    // BYO subscription keys (route_credentials): when the resolved provider
    // matches one of the caller's own credentials, attach it to the Gizzi
    // payload so the upstream call bills the customer's own cloud console
    // instead of Allternit credits. Resolution is user → org pool → platform
    // (P1.7): the org pool is consulted under the key's tenant/org scope and
    // rotates across healthy pool entries. Always a separate payload key —
    // never inside `provider`, which is forwarded to the aggregator.
    let byo_primary = super::route_credentials::resolve_credential(
        &state.db,
        &key.user_id,
        key.tenant_id.as_deref(),
        &resolved.provider_id,
    )
    .ok()
    .flatten();
    let attach_byo = |payload: &mut Value, credential: &super::route_credentials::RouteCredential| {
        let mut object = serde_json::Map::new();
        object.insert("apiKey".into(), json!(credential.api_key));
        if let Some(base_url) = &credential.base_url {
            object.insert("baseURL".into(), json!(base_url));
        }
        payload["provider_credentials"] = Value::Object(object);
    };
    let clear_byo = |payload: &mut Value| {
        if let Some(object) = payload.as_object_mut() {
            object.remove("provider_credentials");
        }
    };

    // Per-key model allowlist: the requested string and, for explicit ids,
    // the resolved provider/model form are both accepted.
    let allowed = key.model_allowed(&request.model)
        || resolved
            .full_id()
            .is_some_and(|full_id| key.model_allowed(&full_id));
    if !allowed {
        return OpenAiErrorResponse::new(
            StatusCode::FORBIDDEN,
            format!(
                "This API key is not allowed to use model `{}`.",
                request.model
            ),
            "permission_error",
            Some("model"),
            Some(crate::llm_gateway::translate::error_code::PERMISSION_DENIED),
        )
        .into_response();
    }

    // P1.6 response cache: short-circuit identical non-streaming, tool-free
    // requests before any Gizzi session/upstream work. The key is hashed here
    // — after file-reference resolution and context-cache prepends, so the
    // hash covers the final messages. The hit check runs after auth, safety,
    // DLP, resolution, and the allowlist, so a cached body is never served to
    // a caller who could not run the underlying request. Checked before the
    // idempotency gate: a cache hit never opens an in_progress row.
    let response_cache = super::response_cache::ResponseCache::global();
    let response_cache_key =
        if response_cache.enabled() && !stream && super::response_cache::is_cacheable(&request) {
            Some(super::response_cache::cache_key(&request))
        } else {
            None
        };
    if let Some(cache_key) = &response_cache_key {
        if let Some(cached_body) = response_cache.get(cache_key) {
            crate::metrics::inc_llm_response_cache_hit(&request.model);
            // Zero-cost usage, same treatment as BYOK: the cached body's
            // token counts are metered, but provider_id is None and cost is
            // 0, so record_usage_event stores/recomputes no spend.
            let cached_usage = cached_body.get("usage");
            let token = |name: &str| {
                cached_usage
                    .and_then(|u| u.get(name))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            };
            let detail = |group: &str, name: &str| {
                cached_usage
                    .and_then(|u| u.get(group))
                    .and_then(|g| g.get(name))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            };
            let outcome = RequestOutcome {
                status: "ok",
                error_type: None,
                usage: GizziUsage {
                    prompt_tokens: token("prompt_tokens"),
                    completion_tokens: token("completion_tokens"),
                    reasoning_tokens: detail("completion_tokens_details", "reasoning_tokens"),
                    cached_tokens: detail("prompt_tokens_details", "cached_tokens"),
                    cache_write_tokens: 0,
                    cost_microdollars: 0,
                    provider_id: None,
                    model_id: Some(request.model.clone()),
                    finish: None,
                },
                policy: resolved.policy.clone(),
                fallback_from: None,
                gizzi_session_id: None,
                latency_ms: started.elapsed().as_millis() as i64,
                ttft_ms: None,
                response_body: Some(cached_body.clone()),
                routing_decision: resolved.routing_decision.clone(),
                tags: outcome_tags.clone(),
                batch_id: batch_id.clone(),
                context_cache_id: context_cache_id.clone(),
            };
            let db = state.db.clone();
            let key_for_record = key.clone();
            tokio::task::spawn_blocking(move || {
                record_usage_event(&db, &key_for_record, &outcome, None)
            });
            return (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
                    (HeaderName::from_static("x-allternit-cache"), HeaderValue::from_static("hit")),
                ],
                Json(cached_body),
            )
                .into_response();
        }
        crate::metrics::inc_llm_response_cache_miss(&request.model);
    }

    // Prompt: system messages → Gizzi `system` field; history → Gizzi parts.
    // Image content is preserved as `file` parts for vision-capable models.
    let (system, all_parts) = messages_to_gizzi_parts(&request.messages);

    // RAG-attribution fallback: non-Anthropic providers get an explicit
    // citations context block prepended to the system prompt.
    let system = if request.citations == Some(true) && resolved.provider_id != "anthropic" {
        match super::citations::CitationsService::global().format_prompt_context() {
            Some(ctx) => Some(match system {
                Some(existing) => format!("{ctx}\n\n{existing}"),
                None => ctx,
            }),
            None => system,
        }
    } else {
        system
    };

    // Session strategy: fresh session titled `gateway:<key_prefix>`, or
    // reuse via header — then only the last user message is sent and Gizzi's
    // own session history provides the context.
    let reuse_session_id = headers
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Keep the full conversation so retries can create a fresh session with
    // complete context. Session reuse narrows `prompt_parts` to the last user
    // message only.
    let conversation_parts = all_parts.clone();

    let (mut session_id, prompt_parts) = match &reuse_session_id {
        Some(existing) => {
            let last_user = request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user");
            match last_user {
                Some(message) => {
                    let parts = message_to_gizzi_parts(message);
                    if parts.is_empty() {
                        return OpenAiErrorResponse::invalid_request(
                            "`messages` must contain a non-empty user message when reusing a session.",
                            Some("messages"),
                        )
                        .into_response();
                    }
                    (existing.clone(), parts)
                }
                None => {
                    return OpenAiErrorResponse::invalid_request(
                        "`messages` must contain a non-empty user message when reusing a session.",
                        Some("messages"),
                    )
                    .into_response();
                }
            }
        }
        None => {
            let title = format!("gateway:{}", key.key_prefix);
            match create_session(&client, &base, &title).await {
                Ok(id) => (id, all_parts),
                Err(response) => return response.into_response(),
            }
        }
    };

    if prompt_parts.is_empty() && system.is_none() {
        return OpenAiErrorResponse::invalid_request(
            "`messages` contain no usable content.",
            Some("messages"),
        )
        .into_response();
    }

    // Idempotency gate: everything fallible above (parse, validation,
    // allowlist, resolution, session create) runs first so rejected requests
    // never leave a stranded in_progress row. From here to the send there is
    // no fallible step, so the pre-inserted row always gets finalized by
    // record_usage_event.
    let mut idem_active = false;
    if let Some(idem) = &idempotency_key {
        if !stream {
            match idempotency_begin(&state.db, &key, idem).await {
                IdemBegin::Proceed => idem_active = true,
                IdemBegin::Replay(body) => {
                    return (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                        .into_response();
                }
                IdemBegin::Reject(response) => return response,
            }
        }
    }

    // Subscribe to the session's bus events BEFORE sending the message.
    let mut events = gizzi_bus::session_events(session_id.clone()).await;

    let mut payload = json!({
        "parts": prompt_parts,
        "model": { "providerID": resolved.provider_id, "modelID": resolved.model_id },
    });
    if let Some(system) = &system {
        payload["system"] = json!(system);
    }
    if let Some(format) = &request.response_format {
        if format.format_type == "json_schema" {
            let schema = format
                .json_schema
                .as_ref()
                .map(|value| value.schema.clone())
                .or_else(|| format.schema.clone())
                .unwrap_or_else(|| json!({}));
            payload["format"] = json!({ "type": "json_schema", "schema": schema });
        }
    }
    // Gizzi variants are the provider-neutral reasoning control and are
    // translated by its provider adapters to reasoning_effort/thinking.
    if let Some(effort) = &request.reasoning_effort {
        payload["variant"] = json!(effort);
    }
    // Service tier is forwarded as message metadata so Gizzi can set the
    // provider-specific option (e.g. OpenAI's `service_tier`).
    if let Some(tier) = &request.service_tier {
        payload["metadata"] = json!({ "service_tier": tier });
    }
    // Citations: Anthropic supports native citations; other providers get the
    // explicit RAG context block prepended above and the native flag disabled.
    if request.citations == Some(true) {
        if is_anthropic_provider(&request.model, &resolved) {
            payload["citations"] = json!({ "enabled": true });
        } else {
            payload["citations"] = json!({ "enabled": false });
        }
    }
    // B5: the router-derived failover chain (top 3 distinct providers).
    // Gizzi switches down this list on non-retryable provider errors and
    // reports each switch as `session.model_fallback`.
    if !resolved.fallbacks.is_empty() {
        payload["fallbackModels"] = json!(resolved
            .fallbacks
            .iter()
            .map(|fallback| json!({
                "providerID": fallback.provider_id,
                "modelID": fallback.model_id,
            }))
            .collect::<Vec<_>>());
    }
    // Provider routing pin for the primary model (empty policy → key omitted).
    if let Some(provider) = &provider_routing_primary {
        payload["provider"] = provider.clone();
    }
    // BYO credential for the primary provider, when the caller has one.
    match &byo_primary {
        Some(credential) => attach_byo(&mut payload, credential),
        None => clear_byo(&mut payload),
    }
    // The credential serving the in-flight attempt; the retry loop marks its
    // health (fail streak / reset) and re-resolves it per failover model.
    let mut current_byo = byo_primary;
    let message_url = format!(
        "{base}/v1/session/{}/message",
        urlencoding::encode(&session_id)
    );
    let mut send_task = tokio::spawn(client.post(message_url).json(&payload).send());
    let mut current_provider_id = resolved.provider_id.clone();
    let mut current_model_id = resolved.model_id.clone();

    info!(
        session_id = %session_id,
        model = %request.model,
        stream,
        key_prefix = %key.key_prefix,
        "LLM gateway request dispatched to Gizzi"
    );
    super::genai_spans::record_request(
        &tracing::Span::current(),
        &resolved.provider_id,
        &request.model,
    );

    // Retry policy + fallback chain, shared by both response paths: the
    // non-streaming loop retries across it (P9/P0.1); the streaming path uses
    // it to build the `allternit.retry_hint` event on failure (P0.2).
    let primary = super::failover::ModelRef {
        provider_id: resolved.provider_id.clone(),
        model_id: resolved.model_id.clone(),
    };
    let fallback_refs: Vec<super::failover::ModelRef> = resolved
        .fallbacks
        .iter()
        .map(|c| super::failover::ModelRef {
            provider_id: c.provider_id.clone(),
            model_id: c.model_id.clone(),
        })
        .collect();
    let policy = key
        .tenant_id
        .as_deref()
        .map(|org| {
            super::failover::load_policy(&state.db, org)
                .unwrap_or_else(|_| super::failover::RetryPolicy::default())
        })
        .unwrap_or_default();

    let response = if stream {
        stream_completion(
            state,
            key.clone(),
            send_task,
            events,
            started,
            session_id,
            request.model.clone(),
            resolved.policy,
            resolved.routing_decision,
            request
                .stream_options
                .and_then(|o| o.include_usage)
                .unwrap_or(false),
            if idem_active { idempotency_key.clone() } else { None },
            outcome_tags,
            batch_id,
            context_cache_id,
            policy,
            primary,
            fallback_refs,
        )
        .await
    } else {
        // P9: non-streaming requests retry across the fallback chain on
        // retryable errors or refusals. Only the final outcome is persisted.
        let mut attempt: u32 = 1;
        let (mut response, mut outcome): (Response, RequestOutcome);

        loop {
            // P1.5: one OTel span per retry/failover attempt (no-op when OTel
            // export is disabled — `genai_spans` returns Span::none()).
            let attempt_span = super::genai_spans::retry_attempt_span(
                attempt,
                &current_provider_id,
                &current_model_id,
            );
            (response, outcome) = nonstream_completion(
                key.clone(),
                send_task,
                events,
                started,
                session_id.clone(),
                request.model.clone(),
                current_provider_id.clone(),
                request.citations == Some(true),
                resolved.policy.clone(),
                resolved.routing_decision.clone(),
                outcome_tags.clone(),
                batch_id.clone(),
                context_cache_id.clone(),
            )
            .instrument(attempt_span.clone())
            .await;
            super::genai_spans::record_attempt_outcome(
                &attempt_span,
                outcome.status,
                outcome.error_type.as_deref(),
            );

            // Health-based failover (P0.1): feed every attempt outcome into
            // the cooldown tracker, keyed by the provider/model that actually
            // served the attempt. Success clears the record; 429s and
            // failure streaks put the candidate into cooldown.
            super::failover::record_attempt_outcome(
                &current_provider_id,
                &current_model_id,
                super::failover::classify_outcome(outcome.status, outcome.error_type.as_deref()),
                &policy,
            );

            // BYO key health (P1.7): the credential that served this attempt
            // gets its failure streak bumped (skipped after
            // MAX_CONSECUTIVE_FAILURES) or reset on success, so pool rotation
            // steers away from a sick key.
            if let Some(credential) = &current_byo {
                if let Some(credential_id) = credential.credential_id.clone() {
                    let db = state.db.clone();
                    let scope = credential.scope;
                    let ok = outcome.status == "ok";
                    tokio::task::spawn_blocking(move || {
                        let result = if ok {
                            super::route_credentials::mark_credential_ok(&db, scope, &credential_id)
                        } else {
                            super::route_credentials::mark_credential_failed(
                                &db,
                                scope,
                                &credential_id,
                            )
                        };
                        if let Err(err) = result {
                            warn!(error = %err, "BYO credential health marking failed");
                        }
                    });
                }
            }

            if outcome.status == "ok" {
                break;
            }

            attempt += 1;
            let error_type = outcome.error_type.as_deref();
            if !super::failover::should_retry(outcome.status, error_type, attempt, &policy) {
                break;
            }
            // Cooldown-aware selection: skips cooling-down candidates and
            // fail-opens to the soonest-expiring one rather than erroring.
            // The chain is pre-sorted by cost/health/kind signals (P2.11);
            // health remains a hard filter inside failover.
            let Some(next_model) = super::provider_routing::select_fallback_sorted(
                attempt,
                &primary,
                &fallback_refs,
                &policy,
                &super::llm_pricing::pricing_snapshot(),
                super::failover::cooldowns(),
            )
            else {
                break;
            };

            let backoff =
                super::failover::next_backoff_ms(attempt, policy.base_delay_ms, policy.max_delay_ms);
            tokio::time::sleep(Duration::from_millis(backoff)).await;

            // Create a fresh session for the retry; a reused session may be in a
            // failed state.
            let title = format!("gateway:{}", key.key_prefix);
            match create_session(&client, &base, &title).await {
                Ok(new_session_id) => session_id = new_session_id,
                Err(err_response) => {
                    response = err_response.into_response();
                    outcome = RequestOutcome {
                        status: "error",
                        error_type: Some("upstream_error".to_string()),
                        usage: GizziUsage::default(),
                        policy: resolved.policy.clone(),
                        fallback_from: None,
                        gizzi_session_id: None,
                        latency_ms: started.elapsed().as_millis() as i64,
                        ttft_ms: None,
                        response_body: None,
                        routing_decision: resolved.routing_decision.clone(),
                        tags: outcome_tags.clone(),
                        batch_id: batch_id.clone(),
                        context_cache_id: context_cache_id.clone(),
                    };
                    break;
                }
            }

            events = gizzi_bus::session_events(session_id.clone()).await;

            let mut payload = json!({
                "parts": conversation_parts.clone(),
                "model": { "providerID": next_model.provider_id, "modelID": next_model.model_id },
            });
            if let Some(system) = &system {
                payload["system"] = json!(system);
            }
            if let Some(format) = &request.response_format {
                if format.format_type == "json_schema" {
                    let schema = format
                        .json_schema
                        .as_ref()
                        .map(|value| value.schema.clone())
                        .or_else(|| format.schema.clone())
                        .unwrap_or_else(|| json!({}));
                    payload["format"] = json!({ "type": "json_schema", "schema": schema });
                }
            }
            if let Some(effort) = &request.reasoning_effort {
                payload["variant"] = json!(effort);
            }
            if request.citations == Some(true) {
                let is_anthropic = next_model.provider_id == "anthropic"
                    || next_model.model_id.starts_with("claude-")
                    || request.model.starts_with("anthropic/")
                    || request.model.starts_with("claude-");
                payload["citations"] = json!({ "enabled": is_anthropic });
            }
            let remaining_fallbacks: Vec<_> = fallback_refs
                .iter()
                .skip(attempt.saturating_sub(1) as usize)
                .map(|f| json!({ "providerID": f.provider_id, "modelID": f.model_id }))
                .collect();
            if !remaining_fallbacks.is_empty() {
                payload["fallbackModels"] = json!(remaining_fallbacks);
            }
            // Provider routing follows the currently-active model: the
            // failover attempt re-resolves the pin for `next_model`.
            if let Some(provider) = provider_routing_policy.as_ref().and_then(|policy| {
                super::provider_routing::resolve_for_model(
                    policy,
                    &next_model.provider_id,
                    &next_model.model_id,
                )
            }) {
                payload["provider"] = provider;
            }
            // The BYO credential must follow the failover model too — and be
            // removed when the next provider is not the caller's own, so a
            // user/org key never rides along to a provider they did not
            // supply credentials for. Re-resolved user → org pool → platform.
            current_byo = super::route_credentials::resolve_credential(
                &state.db,
                &key.user_id,
                key.tenant_id.as_deref(),
                &next_model.provider_id,
            )
            .ok()
            .flatten();
            match &current_byo {
                Some(credential) => attach_byo(&mut payload, credential),
                None => clear_byo(&mut payload),
            }

            let message_url = format!(
                "{base}/v1/session/{}/message",
                urlencoding::encode(&session_id)
            );
            send_task = tokio::spawn(client.post(message_url).json(&payload).send());
            current_provider_id = next_model.provider_id.clone();
            current_model_id = next_model.model_id.clone();

            info!(
                session_id = %session_id,
                model = %request.model,
                attempt,
                fallback_provider = %current_provider_id,
                "LLM gateway retry dispatched to Gizzi"
            );
        }

        // GenAI response fields: final model that served the request plus
        // token usage, recorded once on the handler span after the loop.
        super::genai_spans::record_response(
            &tracing::Span::current(),
            outcome.usage.model_id.as_deref(),
            outcome.usage.prompt_tokens,
            outcome.usage.completion_tokens,
            outcome.error_type.as_deref(),
        );

        // P1.6: store successful responses for future short-circuits. Only
        // `ok` outcomes with a 2xx body are cached; refusals and errors are
        // not. Done before the (spawned) usage persist so the borrow is clean.
        if let (Some(cache_key), "ok") = (&response_cache_key, outcome.status) {
            if response.status() == StatusCode::OK {
                if let Some(body) = outcome.response_body.clone() {
                    response_cache.put(cache_key.clone(), body);
                }
            }
        }

        // Persist the final outcome once. For idempotent requests this
        // finalizes the pre-inserted in_progress row; for non-idempotent
        // requests it inserts the usage row.
        let idem_for_record = if idem_active { idempotency_key.clone() } else { None };
        let db = state.db.clone();
        let key_for_record = key.clone();
        tokio::task::spawn_blocking(move || {
            record_usage_event(
                &db,
                &key_for_record,
                &outcome,
                idem_for_record.as_deref(),
            )
        });

        response
    };

    // Inference hooks: post-hook runs after provider inference.
    if let Some(hooks) = hooks {
        let hook_client = inference_hooks::hook_client();
        let latency_ms = started.elapsed().as_millis() as u64;
        if stream {
            inference_hooks::fire_post_hook(
                &hook_client,
                &hooks,
                body,
                response.status().as_u16(),
                latency_ms,
            )
            .await;
            response
        } else {
            inference_hooks::run_post_hook(&hook_client, &hooks, body, response, latency_ms).await
        }
    } else {
        response
    }
}

/// Non-streaming path: collect to completion, return one chat.completion.
///
/// The [`RequestOutcome`] is returned alongside the HTTP response so callers
/// can implement retry/failover loops without re-parsing the body. The caller
/// is responsible for persisting the final outcome with [`record_usage_event`].
async fn nonstream_completion(
    _key: LlmKeyContext,
    send_task: JoinHandle<Result<reqwest::Response, reqwest::Error>>,
    events: impl futures::Stream<Item = GizziEvent> + Send + 'static,
    started: Instant,
    session_id: String,
    requested_model: String,
    provider_id: String,
    citations_enabled: bool,
    policy: Option<String>,
    routing_decision: Option<RoutingDecision>,
    tags: Option<String>,
    batch_id: Option<String>,
    context_cache_id: Option<String>,
) -> (Response, RequestOutcome) {
    let Collection { collector, failure } = collect(send_task, events, started).await;
    let latency_ms = started.elapsed().as_millis() as i64;

    match failure.or(collector.assistant_error.clone()) {
        Some((error_type, message)) => {
            let outcome = RequestOutcome {
                status: "error",
                error_type: Some(error_type.clone()),
                usage: collector.usage,
                policy,
                fallback_from: collector.fallback_from,
                gizzi_session_id: Some(session_id),
                latency_ms,
                ttft_ms: collector.ttft.map(|d| d.as_millis() as i64),
                response_body: None,
                routing_decision,
                tags,
                batch_id,
                context_cache_id,
            };
            (
                OpenAiErrorResponse::upstream(message, &error_type).into_response(),
                outcome,
            )
        }
        None => {
            let completion_id = new_completion_id();
            let created = chrono::Utc::now().timestamp();
            let refusal = refusal::detect_refusal(&collector.text);
            let upstream_finish = collector.usage.finish.as_deref();
            let finish = refusal::normalized_finish_reason(upstream_finish, &collector.text);
            let is_refusal = finish == "refusal";
            let usage = Usage::new(
                collector.usage.prompt_tokens,
                collector.usage.completion_tokens,
                collector.usage.reasoning_tokens,
                collector.usage.cached_tokens,
            );
            let rag_citations = if citations_enabled && provider_id != "anthropic" && !is_refusal {
                super::citations::CitationsService::global().parse_citations(&collector.text)
            } else {
                Vec::new()
            };
            let assistant_message = if is_refusal {
                super::translate::AssistantMessage::with_refusal(
                    collector.text.clone(),
                    refusal::refusal_message().to_string(),
                )
            } else {
                collector
                    .final_tool_calls()
                    .map(|tc| {
                        super::translate::AssistantMessage::with_tool_calls(
                            collector.text.clone(),
                            tc,
                        )
                    })
                    .unwrap_or_else(|| super::translate::AssistantMessage::new(collector.text.clone()))
            };
            let mut response = ChatCompletionResponse {
                id: completion_id,
                object: "chat.completion".to_string(),
                created,
                model: requested_model,
                choices: vec![super::translate::Choice {
                    index: 0,
                    message: assistant_message,
                    finish_reason: finish.clone(),
                    refusal: refusal.map(|_| refusal::refusal_message().to_string()),
                }],
                usage: Some(usage),
                citations: None,
            }
            .with_citations(rag_citations);
            if !collector.citations.is_empty() && !is_refusal {
                response = response.with_annotations(collector.citations.clone());
            }
            let body = serde_json::to_value(&response)
                .unwrap_or_else(|_| json!({"error": "serialization failure"}));

            let outcome = RequestOutcome {
                status: if is_refusal { "refused" } else { "ok" },
                error_type: if is_refusal { Some("refusal".to_string()) } else { None },
                usage: collector.usage,
                policy,
                fallback_from: collector.fallback_from.clone(),
                gizzi_session_id: Some(session_id.clone()),
                latency_ms,
                ttft_ms: collector.ttft.map(|d| d.as_millis() as i64),
                response_body: Some(body.clone()),
                routing_decision,
                tags,
                batch_id,
                context_cache_id,
            };
            let mut resp = (StatusCode::OK, Json(body)).into_response();
            if let Ok(value) = HeaderValue::from_str(&session_id) {
                resp.headers_mut()
                    .insert(HeaderName::from_static(SESSION_HEADER), value);
            }
            if let (Some(from), Some(to)) = (collector.fallback_from, collector.fallback_to) {
                if let Ok(value) = HeaderValue::from_str(&format!("{from}->{to}")) {
                    resp.headers_mut()
                        .insert(HeaderName::from_static(FALLBACK_HEADER), value);
                }
            }
            (resp, outcome)
        }
    }
}

/// One iteration of the streaming collection loop. Kept separate from the
/// `tokio::select!` so SSE yields never happen inside a select arm.
enum Progress {
    Continue,
    Delta(String),
    ToolCallDelta(ToolCallDelta),
    Done,
    Failed(String, String),
}

/// Streaming path: SSE frames — role chunk, content deltas, finish chunk,
/// optional usage chunk, then `[DONE]`. Mid-stream failures emit an
/// `allternit.retry_hint` event (P0.2 — built by
/// [`super::failover::stream_retry_hint`], which owns the retry policy and
/// cooldown-aware fallback pick) followed by an OpenAI-shaped error frame
/// before closing; Gizzi re-drives retryable failures with full session
/// context. A [`StreamUsageGuard`] inside the stream records partial usage if
/// the client disconnects mid-stream.
async fn stream_completion(
    state: Arc<AppState>,
    key: LlmKeyContext,
    send_task: JoinHandle<Result<reqwest::Response, reqwest::Error>>,
    events: impl futures::Stream<Item = GizziEvent> + Send + 'static,
    started: Instant,
    session_id: String,
    requested_model: String,
    policy: Option<String>,
    routing_decision: Option<RoutingDecision>,
    include_usage: bool,
    idempotency_key: Option<String>,
    tags: Option<String>,
    batch_id: Option<String>,
    context_cache_id: Option<String>,
    retry_policy: super::failover::RetryPolicy,
    primary: super::failover::ModelRef,
    fallbacks: Vec<super::failover::ModelRef>,
) -> Response {
    let completion_id = new_completion_id();
    let created = chrono::Utc::now().timestamp();
    let wire_model = requested_model;
    let session_id_for_header = session_id.clone();

    // Client-disconnect metering: the guard lives in the stream and records
    // partial usage on drop unless the normal end path disarms it first.
    let shared_progress = Arc::new(Mutex::new(StreamProgress::default()));
    let mut guard = StreamUsageGuard {
        db: state.db.clone(),
        key: key.clone(),
        session_id: session_id.clone(),
        policy: policy.clone(),
        routing_decision,
        started,
        progress: shared_progress.clone(),
        tags: tags.clone(),
        batch_id: batch_id.clone(),
        context_cache_id: context_cache_id.clone(),
    };

    let stream = async_stream::stream! {
        let mut collector = Collector::new(started);
        let mut send_task = send_task;
        let mut send_done = false;
        let mut failure: Option<(String, String)> = None;
        tokio::pin!(events);
        let deadline = tokio::time::sleep(COMPLETION_TIMEOUT);
        tokio::pin!(deadline);

        yield Ok::<_, Infallible>(Event::default().data(
            ChatCompletionChunk::role_chunk(&completion_id, created, &wire_model).to_sse_data(),
        ));

        'collect: loop {
            let progress = tokio::select! {
                send_result = &mut send_task, if !send_done => {
                    send_done = true;
                    match send_result {
                        Ok(Ok(resp)) if resp.status().is_success() => Progress::Continue,
                        Ok(Ok(resp)) => {
                            let status = resp.status();
                            let body = resp.text().await.unwrap_or_default();
                            // Preserve the 429 signal (see collect()).
                            let error_type = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                                "rate_limit_error"
                            } else {
                                "upstream_error"
                            };
                            Progress::Failed(
                                error_type.to_string(),
                                format!("Gizzi message endpoint returned {status}: {body}"),
                            )
                        }
                        Ok(Err(err)) => Progress::Failed(
                            "upstream_error".to_string(),
                            format!("Failed to send message to Gizzi: {err}"),
                        ),
                        Err(err) => Progress::Failed(
                            "upstream_error".to_string(),
                            format!("Gizzi send task failed: {err}"),
                        ),
                    }
                }
                maybe_event = events.next() => {
                    match maybe_event {
                        Some(event) => {
                            let effect = collector.handle_event(&event);
                            // Keep the disconnect guard's snapshot current.
                            if let Ok(mut snapshot) = shared_progress.lock() {
                                snapshot.sync_from(&collector);
                            }
                            match effect {
                                EventEffect::TextDelta(delta) => Progress::Delta(delta),
                                EventEffect::ToolCallDelta(delta) => Progress::ToolCallDelta(delta),
                                EventEffect::Done => Progress::Done,
                                EventEffect::Error(name, message) => Progress::Failed(name, message),
                                EventEffect::None => Progress::Continue,
                            }
                        },
                        // The bus never closes in practice; treat as done.
                        None => Progress::Done,
                    }
                }
                _ = &mut deadline => {
                    Progress::Failed(
                        "upstream_error".to_string(),
                        format!("Completion timed out after {}s.", COMPLETION_TIMEOUT.as_secs()),
                    )
                }
            };

            match progress {
                Progress::Continue => {}
                Progress::Delta(delta) => {
                    yield Ok(Event::default().data(
                        ChatCompletionChunk::content_chunk(
                            &completion_id, created, &wire_model, &delta,
                        )
                        .to_sse_data(),
                    ));
                }
                Progress::ToolCallDelta(delta) => {
                    yield Ok(Event::default().data(
                        ChatCompletionChunk::tool_calls_chunk(
                            &completion_id, created, &wire_model, vec![delta],
                        )
                        .to_sse_data(),
                    ));
                }
                Progress::Done => break 'collect,
                Progress::Failed(name, message) => {
                    failure = Some((name, message));
                    break 'collect;
                }
            }
        }

        let latency_ms = started.elapsed().as_millis() as i64;
        // The stream reached its normal end — the guard must not record.
        if let Ok(mut snapshot) = shared_progress.lock() {
            snapshot.disarmed = true;
        }
        let routing_decision = guard.routing_decision.take();
        let outcome: RequestOutcome;

        match failure.or_else(|| collector.assistant_error.clone()) {
            Some((error_type, message)) => {
                // P0.2: feed the failed attempt into the cooldown tracker,
                // then emit the retry hint so Gizzi can re-drive. The hint is
                // health-aware: next_fallback skips cooling-down candidates.
                super::failover::record_attempt_outcome(
                    &primary.provider_id,
                    &primary.model_id,
                    super::failover::classify_outcome("error", Some(&error_type)),
                    &retry_policy,
                );
                let hint = super::failover::stream_retry_hint(
                    "error",
                    Some(&error_type),
                    &primary,
                    &fallbacks,
                    &retry_policy,
                );
                yield Ok(Event::default()
                    .event("allternit.retry_hint")
                    .data(serde_json::to_string(&hint).unwrap_or_else(|_| {
                        "{\"retryable\":false}".to_string()
                    })));
                yield Ok(Event::default().data(stream_error_data(
                    &message,
                    &error_type,
                    None,
                )));
                yield Ok(Event::default().data("[DONE]"));
                outcome = RequestOutcome {
                    status: "error",
                    error_type: Some(error_type),
                    usage: collector.usage,
                    policy,
                    fallback_from: collector.fallback_from,
                    gizzi_session_id: Some(session_id),
                    latency_ms,
                    ttft_ms: collector.ttft.map(|d| d.as_millis() as i64),
                    response_body: None,
                    routing_decision,
                    tags,
                    batch_id,
                    context_cache_id,
                };
            }
            None => {
                let refusal = refusal::detect_refusal(&collector.text);
                let finish = refusal::normalized_finish_reason(collector.usage.finish.as_deref(), &collector.text);
                let is_refusal = finish == "refusal";
                yield Ok(Event::default().data(
                    ChatCompletionChunk::finish_chunk_with_refusal(
                        &completion_id, created, &wire_model, &finish,
                        refusal.map(|_| refusal::refusal_message().to_string()),
                    )
                    .to_sse_data(),
                ));
                if include_usage {
                    let usage = Usage::new(
                        collector.usage.prompt_tokens,
                        collector.usage.completion_tokens,
                        collector.usage.reasoning_tokens,
                        collector.usage.cached_tokens,
                    );
                    yield Ok(Event::default().data(
                        ChatCompletionChunk::usage_chunk(
                            &completion_id, created, &wire_model, usage,
                        )
                        .to_sse_data(),
                    ));
                }
                yield Ok(Event::default().data("[DONE]"));
                outcome = RequestOutcome {
                    status: if is_refusal { "refused" } else { "ok" },
                    error_type: if is_refusal { Some("refusal".to_string()) } else { None },
                    usage: collector.usage,
                    policy,
                    fallback_from: collector.fallback_from,
                    gizzi_session_id: Some(session_id),
                    latency_ms,
                    ttft_ms: collector.ttft.map(|d| d.as_millis() as i64),
                    response_body: None,
                    routing_decision,
                    tags,
                    batch_id,
                    context_cache_id,
                };
            }
        }

        // B4: single metering choke point — persists the usage row (with
        // pricing recompute + routing decision) and emits Prometheus series.
        let db = state.db.clone();
        tokio::task::spawn_blocking(move || {
            record_usage_event(&db, &key, &outcome, idempotency_key.as_deref())
        });
    };

    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&session_id_for_header) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(SESSION_HEADER), value);
    }
    response
}

/// `GET /models` — OpenAI model list built from the Gizzi catalog plus the
/// policy aliases, filtered by the key's allowlist.
pub async fn list_models(
    State(_state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
) -> Response {
    let base = gizzi_base();
    let client = http_client();
    let catalog = match fetch_catalog(&client, &base).await {
        Ok(catalog) => catalog,
        Err(response) => return response.into_response(),
    };

    let mut data: Vec<Value> = Vec::new();
    for alias in POLICY_ALIASES {
        if key.model_allowed(alias) {
            data.push(json!({
                "id": alias,
                "object": "model",
                "created": 0,
                "owned_by": "allternit",
            }));
        }
    }

    let mut entries: Vec<(String, String, i64, ModelEntry)> = Vec::new();
    for provider in &catalog.all {
        for model_id in provider.models.keys() {
            let full_id = format!("{}/{}", provider.id, model_id);
            if !(key.model_allowed(&full_id) || key.model_allowed(model_id)) {
                continue;
            }
            let model = provider.models.get(model_id).cloned().unwrap_or_default();
            let created = model
                .release_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc().timestamp())
                .unwrap_or(0);
            entries.push((full_id, provider.id.clone(), created, model));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (full_id, provider_id, created, model) in entries {
        data.push(json!({
            "id": full_id,
            "object": "model",
            "created": created,
            "owned_by": provider_id,
            "context_window": model.limit.map(|value| value.context),
            "max_output_tokens": model.limit.map(|value| value.output),
            "deprecated": model.deprecated.unwrap_or(false),
            "replacement": model.replacement,
        }));
    }

    Json(json!({ "object": "list", "data": data })).into_response()
}

/// `GET /models/:id` — OpenAI-compatible model retrieval.
pub async fn get_model(
    State(_state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
    Path(model_id): Path<String>,
) -> Response {
    let base = gizzi_base();
    let client = http_client();

    // Policy aliases are allowed synthetic models.
    if POLICY_ALIASES.contains(&model_id.as_str()) && key.model_allowed(&model_id) {
        return Json(json!({
            "id": model_id,
            "object": "model",
            "created": 0,
            "owned_by": "allternit",
        }))
        .into_response();
    }

    let catalog = match fetch_catalog(&client, &base).await {
        Ok(catalog) => catalog,
        Err(response) => return response.into_response(),
    };

    for provider in &catalog.all {
        for (id, model) in &provider.models {
            let full_id = format!("{}/{}", provider.id, id);
            if full_id != model_id && id != &model_id {
                continue;
            }
            if !(key.model_allowed(&full_id) || key.model_allowed(id)) {
                break;
            }
            let created = model
                .release_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc().timestamp())
                .unwrap_or(0);
            return Json(json!({
                "id": full_id,
                "object": "model",
                "created": created,
                "owned_by": provider.id,
                "context_window": model.limit.map(|value| value.context),
                "max_output_tokens": model.limit.map(|value| value.output),
                "deprecated": model.deprecated.unwrap_or(false),
                "replacement": model.replacement,
            }))
            .into_response();
        }
    }

    OpenAiErrorResponse::new(
        StatusCode::NOT_FOUND,
        format!("The model `{model_id}` does not exist or you do not have access to it."),
        "invalid_request_error",
        Some("model"),
        Some(crate::llm_gateway::translate::error_code::MODEL_NOT_FOUND),
    )
    .into_response()
}

/// `GET /pricing` — cross-provider per-model pricing snapshot.
///
/// Returns the current models.dev cache rates as an OpenAI-style list of
/// `model_pricing` objects. Rates are dollars per 1M tokens.
pub async fn list_pricing() -> Response {
    Json(build_pricing_response(&llm_pricing::pricing_snapshot())).into_response()
}

fn build_pricing_response(map: &llm_pricing::PricingMap) -> Value {
    let mut data: Vec<Value> = map
        .iter()
        .map(|(id, pricing)| {
            let mut value = serde_json::to_value(pricing).unwrap_or_else(|_| json!({}));
            if let Some(obj) = value.as_object_mut() {
                obj.insert("id".to_string(), json!(id));
                obj.insert("object".to_string(), json!("model_pricing"));
            }
            value
        })
        .collect();
    data.sort_by(|a, b| {
        a.get("id")
            .and_then(|v| v.as_str())
            .cmp(&b.get("id").and_then(|v| v.as_str()))
    });
    json!({ "object": "list", "data": data })
}

/// `GET /rate-limits` — caller quota snapshot derived from the key's rate
/// limit, key monthly budget, and tenant hard cap (when present).
pub async fn rate_limits(
    State(state): State<Arc<AppState>>,
    Extension(key): Extension<LlmKeyContext>,
) -> Response {
    let limit = key
        .rate_limit_rpm
        .unwrap_or(super::auth::DEFAULT_RATE_LIMIT_RPM)
        .max(1) as usize;
    let rate = super::auth::rate_limit_status(&key.key_id, limit);

    let budget = tokio::task::spawn_blocking({
        let db = state.db.clone();
        let key = key.clone();
        move || -> Result<super::auth::TokenBudgetStatus, rusqlite::Error> {
            let conn = db.connect()?;
            super::auth::token_budget_status(&conn, &key)
        }
    })
    .await;

    match budget {
        Ok(Ok(budget)) => {
            let reset_at = chrono::Utc::now()
                + chrono::Duration::seconds(rate.reset_in_secs as i64);
            Json(json!({
                "object": "rate_limits",
                "requests_remaining": rate.remaining,
                "requests_limit": rate.limit,
                "tokens_remaining": budget.remaining_cents,
                "tokens_limit": budget.limit_cents,
                "reset_at": reset_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            }))
            .into_response()
        }
        Ok(Err(err)) => {
            warn!(error = %err, "rate_limits DB query failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": {
                        "message": format!("Internal error: {err}"),
                        "type": "server_error",
                    }
                })),
            )
                .into_response()
        }
        Err(err) => {
            warn!(error = %err, "rate_limits task failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": {
                        "message": format!("Internal error: {err}"),
                        "type": "server_error",
                    }
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn token_estimate_is_deterministic_and_rounds_up() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "test/model",
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .unwrap();
        // "user" + "hello" = 9 characters, rounded up at four chars/token.
        assert_eq!(estimate_tokens(&request), 3);
    }

    #[test]
    fn count_tokens_uses_tiktoken_for_openai_models() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user", "content": "Hello" }
            ]
        }))
        .unwrap();

        let count = count_tokens_request(&request);
        let expected = tiktoken_rs::num_tokens_from_messages("gpt-4o", &[
            tiktoken_rs::ChatCompletionRequestMessage {
                role: "system".to_string(),
                content: Some("You are a helpful assistant.".to_string()),
                name: None,
                function_call: None,
                tool_calls: Vec::new(),
                refusal: None,
            },
            tiktoken_rs::ChatCompletionRequestMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                name: None,
                function_call: None,
                tool_calls: Vec::new(),
                refusal: None,
            },
        ])
        .unwrap() as u64;

        assert!(count > 0);
        assert_eq!(count, expected);
    }

    #[test]
    fn count_tokens_strips_openai_provider_prefix() {
        let prefixed: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "openai/gpt-4o",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user", "content": "Hello" }
            ]
        }))
        .unwrap();
        let bare: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user", "content": "Hello" }
            ]
        }))
        .unwrap();

        assert_eq!(count_tokens_request(&prefixed), count_tokens_request(&bare));
    }

    #[test]
    fn count_tokens_falls_back_to_heuristic_for_unknown_models() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "unknown-provider/unknown-model",
            "messages": [{ "role": "user", "content": "hello" }]
        }))
        .unwrap();

        assert_eq!(count_tokens_request(&request), estimate_tokens(&request));
    }
}

#[cfg(test)]
mod citation_tests {
    use super::*;
    use serde_json::json;

    fn resolved(provider_id: &str, model_id: &str) -> ResolvedModel {
        ResolvedModel {
            policy: None,
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            fallbacks: Vec::new(),
            routing_decision: None,
        }
    }

    #[test]
    fn detects_anthropic_provider() {
        assert!(is_anthropic_provider(
            "anthropic/claude-sonnet-4",
            &resolved("anthropic", "claude-sonnet-4")
        ));
        assert!(is_anthropic_provider(
            "allternit-balanced",
            &resolved("anthropic", "claude-sonnet-4")
        ));
        assert!(is_anthropic_provider(
            "claude-sonnet-4",
            &resolved("openai", "gpt-4o")
        ));
        assert!(!is_anthropic_provider(
            "openai/gpt-4o",
            &resolved("openai", "gpt-4o")
        ));
    }

    #[test]
    fn extracts_citations_from_message_info() {
        let info = json!({
            "role": "assistant",
            "citations": [
                { "cited_text": "source one", "url": "https://example.com/1", "title": "One" },
                { "cited_text": "source two", "file_id": "file_abc" }
            ]
        });
        let annotations = extract_citations(&info);
        assert_eq!(annotations.len(), 2);
        match &annotations[0] {
            Annotation::UrlCitation { url_citation } => {
                assert_eq!(url_citation.url, "https://example.com/1");
                assert_eq!(url_citation.title.as_deref(), Some("One"));
            }
            _ => panic!("expected URL citation"),
        }
        match &annotations[1] {
            Annotation::FileCitation { file_citation } => {
                assert_eq!(file_citation.file_id, "file_abc");
            }
            _ => panic!("expected file citation"),
        }
    }
}

#[cfg(test)]
mod model_deprecation_tests {
    use super::ModelEntry;
    use serde_json::json;

    #[test]
    fn model_entry_parses_deprecation_fields() {
        let entry: ModelEntry = serde_json::from_value(json!({
            "release_date": "2024-01-01",
            "status": "active",
            "deprecated": true,
            "replacement": "openai/gpt-5"
        }))
        .unwrap();
        assert_eq!(entry.deprecated, Some(true));
        assert_eq!(entry.replacement.as_deref(), Some("openai/gpt-5"));
    }

    #[test]
    fn model_entry_defaults_deprecation_to_none() {
        let entry: ModelEntry = serde_json::from_value(json!({
            "release_date": "2024-01-01"
        }))
        .unwrap();
        assert_eq!(entry.deprecated, None);
        assert_eq!(entry.replacement, None);
    }
}

#[cfg(test)]
mod pricing_tests {
    use super::{build_pricing_response, llm_pricing::ModelPricing};
    use std::collections::HashMap;

    #[test]
    fn pricing_response_lists_models_sorted() {
        let mut map = HashMap::new();
        map.insert(
            "openai/gpt-4o".to_string(),
            ModelPricing {
                input: 2.5,
                output: 10.0,
                cache_read: 1.25,
                cache_write: 0.0,
                context_over_200k: None,
            },
        );
        map.insert(
            "anthropic/claude-sonnet-4-5".to_string(),
            ModelPricing {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                context_over_200k: None,
            },
        );

        let resp = build_pricing_response(&map);
        let data = resp["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["id"], "anthropic/claude-sonnet-4-5");
        assert_eq!(data[0]["object"], "model_pricing");
        assert_eq!(data[0]["input"], 3.0);
        assert_eq!(data[1]["id"], "openai/gpt-4o");
        assert_eq!(data[1]["output"], 10.0);
    }

    #[test]
    fn pricing_response_empty_when_no_cache() {
        let resp = build_pricing_response(&HashMap::new());
        assert_eq!(resp["data"].as_array().unwrap().len(), 0);
        assert_eq!(resp["object"], "list");
    }
}

#[cfg(test)]
mod metering_tests {
    use super::*;
    use super::super::auth::LlmKeyContext;

    fn test_db() -> (DbHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbHandle::new(dir.path().join("test.db")).unwrap();
        (db, dir)
    }

    fn insert_key(db: &DbHandle, key_tags: Option<&str>) -> LlmKeyContext {
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO users (id, email) VALUES ('u1', 'test@example.com')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO llm_virtual_keys
             (id, user_id, key_hash, key_prefix, allowed_models, tags)
             VALUES ('vk1', 'u1', 'hash', 'ak-test', NULL, ?1)",
            params![key_tags],
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
            tags: key_tags.and_then(|raw| serde_json::from_str(raw).ok()),
        }
    }

    fn outcome(status: &'static str) -> RequestOutcome {
        RequestOutcome {
            status,
            error_type: None,
            usage: GizziUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                cost_microdollars: 123_000,
                ..GizziUsage::default()
            },
            policy: None,
            fallback_from: None,
            gizzi_session_id: None,
            latency_ms: 42,
            ttft_ms: None,
            response_body: None,
            routing_decision: None,
            tags: None,
            batch_id: None,
            context_cache_id: None,
        }
    }

    #[test]
    fn record_usage_event_persists_tags_and_batch_id() {
        let (db, _dir) = test_db();
        let key = insert_key(&db, None);

        let mut tagged = outcome("ok");
        tagged.tags = Some(r#"{"team":"alpha"}"#.to_string());
        tagged.batch_id = Some("batch_123".to_string());
        record_usage_event(&db, &key, &tagged, None);

        // Requests without tags store NULL.
        record_usage_event(&db, &key, &outcome("ok"), None);

        let conn = db.connect().unwrap();
        let tagged_row: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT tags, batch_id FROM llm_usage_events WHERE batch_id IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            tagged_row,
            (
                Some(r#"{"team":"alpha"}"#.to_string()),
                Some("batch_123".to_string())
            )
        );

        // Requests without tags store NULL.
        let untagged: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT tags, batch_id FROM llm_usage_events WHERE batch_id IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(untagged, (None, None));
    }

    #[test]
    fn record_usage_event_persists_context_cache_id() {
        let (db, _dir) = test_db();
        let key = insert_key(&db, None);

        let mut cached = outcome("ok");
        cached.context_cache_id = Some("ctxcache_abc".to_string());
        record_usage_event(&db, &key, &cached, None);
        record_usage_event(&db, &key, &outcome("ok"), None);

        let conn = db.connect().unwrap();
        let cached_row: Option<String> = conn
            .query_row(
                "SELECT context_cache_id FROM llm_usage_events
                 WHERE context_cache_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cached_row.as_deref(), Some("ctxcache_abc"));

        let uncached: Option<String> = conn
            .query_row(
                "SELECT context_cache_id FROM llm_usage_events
                 WHERE context_cache_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(uncached, None);
    }

    #[test]
    fn record_usage_event_updates_context_cache_id_on_idempotent_finalize() {
        let (db, _dir) = test_db();
        let key = insert_key(&db, None);
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO llm_usage_events
             (id, virtual_key_id, user_id, status, idempotency_key)
             VALUES ('pre', 'vk1', 'u1', 'in_progress', 'idem-cache')",
            [],
        )
        .unwrap();
        drop(conn);

        let mut cached = outcome("ok");
        cached.context_cache_id = Some("ctxcache_idem".to_string());
        record_usage_event(&db, &key, &cached, Some("idem-cache"));

        let conn = db.connect().unwrap();
        let context_cache_id: Option<String> = conn
            .query_row(
                "SELECT context_cache_id FROM llm_usage_events
                 WHERE idempotency_key = 'idem-cache'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(context_cache_id.as_deref(), Some("ctxcache_idem"));
    }

    #[test]
    fn record_cache_hit_increments_and_stamps() {
        let (db, _dir) = test_db();
        let key = insert_key(&db, None);
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO llm_context_caches
             (id, virtual_key_id, tenant_id, name, messages_json, ttl_seconds, expires_at)
             VALUES ('ctxcache_hit1', 'vk1', NULL, 'docs',
                     '[{\"role\":\"system\",\"content\":\"ctx\"}]', 3600, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        super::super::context_cache::record_cache_hit(&db, "ctxcache_hit1").unwrap();
        super::super::context_cache::record_cache_hit(&db, "ctxcache_hit1").unwrap();

        let conn = db.connect().unwrap();
        let (hits, last_used_at): (i64, Option<String>) = conn
            .query_row(
                "SELECT hits, last_used_at FROM llm_context_caches WHERE id = 'ctxcache_hit1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(hits, 2);
        assert!(
            last_used_at.is_some_and(|ts| !ts.is_empty()),
            "last_used_at stamped on hit"
        );
    }

    #[test]
    fn record_usage_event_updates_tags_on_idempotent_finalize() {
        let (db, _dir) = test_db();
        let key = insert_key(&db, None);
        let conn = db.connect().unwrap();
        conn.execute(
            "INSERT INTO llm_usage_events
             (id, virtual_key_id, user_id, status, idempotency_key)
             VALUES ('pre', 'vk1', 'u1', 'in_progress', 'idem-1')",
            [],
        )
        .unwrap();
        drop(conn);

        let mut tagged = outcome("ok");
        tagged.tags = Some(r#"{"project":"p1"}"#.to_string());
        record_usage_event(&db, &key, &tagged, Some("idem-1"));

        let conn = db.connect().unwrap();
        let (tags, count): (Option<String>, i64) = conn
            .query_row(
                "SELECT tags, COUNT(*) FROM llm_usage_events
                 WHERE idempotency_key = 'idem-1' GROUP BY id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tags.as_deref(), Some(r#"{"project":"p1"}"#));
        let _ = count;
    }

    #[test]
    fn resolve_outcome_tags_prefers_request_over_key_inheritance() {
        let mut key = insert_key(&DbHandle::new_memory().unwrap(), None);
        key.tags = Some(BTreeMap::from([("org".to_string(), "acme".to_string())]));

        // Request tags win.
        let request_tags = Some(BTreeMap::from([("team".to_string(), "blue".to_string())]));
        assert_eq!(
            resolve_outcome_tags(&request_tags, &key).as_deref(),
            Some(r#"{"team":"blue"}"#)
        );

        // Key defaults apply when the request has none.
        assert_eq!(
            resolve_outcome_tags(&None, &key).as_deref(),
            Some(r#"{"org":"acme"}"#)
        );

        // Neither → NULL.
        key.tags = None;
        assert_eq!(resolve_outcome_tags(&None, &key), None);
    }

    #[test]
    fn batch_id_header_parsing() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(batch_id_from_headers(&headers), None);
        headers.insert(
            "x-allternit-batch-id",
            axum::http::HeaderValue::from_static("batch_abc"),
        );
        assert_eq!(batch_id_from_headers(&headers).as_deref(), Some("batch_abc"));
        headers.insert("x-allternit-batch-id", axum::http::HeaderValue::from_static(""));
        assert_eq!(batch_id_from_headers(&headers), None);
    }
}

#[cfg(test)]
mod stream_retry_hint_tests {
    //! P0.2: streaming failover — a failed streaming attempt must emit an
    //! `allternit.retry_hint` SSE event on the wire before the error frame.
    use super::*;
    use super::super::auth::LlmKeyContext;
    use super::super::failover::{ModelRef, RetryPolicy};

    fn hint_key() -> LlmKeyContext {
        LlmKeyContext {
            key_id: "vk-hint".to_string(),
            user_id: "u-hint".to_string(),
            tenant_id: None,
            key_prefix: "ak-hint".to_string(),
            monthly_budget_cents: None,
            rate_limit_rpm: None,
            allowed_models: None,
            tags: None,
        }
    }

    /// Drive `stream_completion` with a send task that never resolves and an
    /// event stream that fails mid-stream via `session.error`; return the full
    /// SSE body text.
    async fn run_failed_stream(policy: RetryPolicy, fallbacks: Vec<ModelRef>) -> String {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(dir.path()).await;
        {
            let conn = state.db.connect().unwrap();
            conn.execute(
                "INSERT INTO users (id, email) VALUES ('u-hint', 'hint@example.com')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO llm_virtual_keys (id, user_id, key_hash, key_prefix)
                 VALUES ('vk-hint', 'u-hint', 'hash', 'ak-hint')",
                [],
            )
            .unwrap();
        }

        let send_task = tokio::spawn(
            futures::future::pending::<Result<reqwest::Response, reqwest::Error>>(),
        );
        let events = futures::stream::iter(vec![
            GizziEvent {
                event_type: "session.status".to_string(),
                properties: json!({"sessionID": "sess-hint", "status": {"type": "busy"}}),
            },
            GizziEvent {
                event_type: "session.error".to_string(),
                properties: json!({
                    "sessionID": "sess-hint",
                    "error": {"name": "rate_limit_error", "message": "upstream 429"}
                }),
            },
        ]);

        let response = stream_completion(
            state,
            hint_key(),
            send_task,
            events,
            Instant::now(),
            "sess-hint".to_string(),
            "mock-a/model-a".to_string(),
            None,
            None,
            false,
            None,
            None,
            None,
            None,
            policy,
            ModelRef {
                provider_id: "mock-a".to_string(),
                model_id: "model-a".to_string(),
            },
            fallbacks,
        )
        .await;

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read SSE body");
        String::from_utf8(bytes.to_vec()).expect("SSE body is UTF-8")
    }

    /// Extract the JSON payload of the `allternit.retry_hint` SSE event.
    fn hint_payload(body: &str) -> Value {
        let mut event_lines = body.split("\n\n");
        for frame in &mut event_lines {
            if frame.starts_with("event: allternit.retry_hint") {
                let data = frame
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .expect("hint frame carries data");
                return serde_json::from_str(data).expect("hint data is JSON");
            }
        }
        panic!("no allternit.retry_hint event in stream body:\n{body}");
    }

    #[tokio::test]
    async fn stream_failure_emits_retry_hint_on_the_wire() {
        let body = run_failed_stream(
            RetryPolicy::default(),
            vec![ModelRef {
                provider_id: "mock-b".to_string(),
                model_id: "model-b".to_string(),
            }],
        )
        .await;

        let hint = hint_payload(&body);
        assert_eq!(hint["retryable"], true);
        assert_eq!(hint["reason"], "rate_limit_error");
        assert_eq!(
            hint["next_fallback"],
            json!({"provider_id": "mock-b", "model_id": "model-b"})
        );

        // The hint precedes the terminal OpenAI-shaped error frame and [DONE].
        let hint_pos = body.find("event: allternit.retry_hint").unwrap();
        let err_pos = body.find("\"upstream_error\"").unwrap_or(usize::MAX);
        let done_pos = body.find("[DONE]").unwrap();
        assert!(hint_pos < done_pos, "hint must precede [DONE]:\n{body}");
        let _ = err_pos;
    }

    #[tokio::test]
    async fn terminal_stream_failure_says_not_retryable() {
        let body = run_failed_stream(
            RetryPolicy {
                enabled: false,
                ..RetryPolicy::default()
            },
            vec![ModelRef {
                provider_id: "mock-b".to_string(),
                model_id: "model-b".to_string(),
            }],
        )
        .await;

        let hint = hint_payload(&body);
        assert_eq!(hint["retryable"], false);
        assert_eq!(hint["next_fallback"], Value::Null);
    }
}
