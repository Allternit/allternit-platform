//! `/api/factory`: the engine's half of API.md §3 (camelCase JSON).
//!
//! Mounted on the engine service (`allternit-factory serve`, 127.0.0.1:3011 and
//! `~/.allternit/factory/factory.sock`). allternit-api proxies `/api/factory/*`
//! here for Desktop, web and phone, except approvals, which it owns.
//!
//! Every read is a projection of the workspace ledger plus live pane facts.
//! Teams (team.yaml up/down), the board, node folders and proof upload are
//! stream F6E's modules (`agents::http`, `workspace::http`); this router calls
//! into them so every path has one route. Verbs not built yet answer
//! `404 not_found` "… is not built yet" rather than pretending (API.md §2).

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::backend;
use crate::agents::registry::{self, Registry};
use crate::agents::view::{self, Agent};
use crate::api::service::ServiceState;
use crate::core::types::{Actor, ActorType, AllternitEvent, EventScope, LedgerQuery};
use crate::judge::state::project_node_judge;
use crate::send::{self, ApiLink, DeliveryFilter, SendCtx, SendRequest};
use crate::templates::{plan_from_template, Template, TemplateStore, TEMPLATE_DIR};
use crate::work::graph::ready_nodes;
use crate::work::{project_dag, DagNode, DagState};

/// Ledger event recording which campaign a run's DAG belongs to.
pub const RUN_EVENT: &str = "factory.run.created";

type S = State<Arc<ServiceState>>;

pub fn router() -> Router<Arc<ServiceState>> {
    Router::new()
        .route("/api/factory/health", get(health))
        .route("/api/factory/agents", get(agents_list))
        .route("/api/factory/agents/:id", get(agent_get))
        .route("/api/factory/agents/:id/capture", get(agent_capture))
        .route("/api/factory/agents/:id/transcript", get(agent_transcript))
        .route("/api/factory/agents/:id/screen", get(agent_screen))
        .route("/api/factory/agents/:id/stream", get(agent_stream))
        .route("/api/factory/agents/:id/input", post(agent_input))
        .route("/api/factory/teams", get(teams_list))
        .route("/api/factory/teams/:name/up", post(teams_up))
        .route("/api/factory/teams/:name/down", post(teams_down))
        .route("/api/factory/send", post(send_h))
        .route("/api/factory/deliveries", get(deliveries_h))
        .route("/api/factory/events", get(events_h))
        .route("/api/factory/templates", get(templates_list))
        .route("/api/factory/templates/:id", get(template_get))
        .route("/api/factory/runs", post(runs_create))
        .route("/api/factory/dags/:dag_id", get(dag_get))
        .route("/api/factory/campaigns", get(campaigns_list))
        .route("/api/factory/campaigns/:id/board", get(campaign_board))
        .route("/api/factory/nodes", get(nodes_list))
        .route("/api/factory/nodes/:dag_id/:node_id", get(node_get))
        .route(
            "/api/factory/nodes/:dag_id/:node_id/proof",
            post(node_proof).layer(axum::extract::DefaultBodyLimit::max(
                crate::workspace::http::MAX_PROOF_BYTES + 64 * 1024,
            )),
        )
}

// ---------------------------------------------------------------- errors

/// API.md error body with the status that matches its code.
pub fn error(code: &str, fact: impl Into<String>, action: impl Into<String>) -> Response {
    let status = match code {
        "usage" => StatusCode::BAD_REQUEST,
        "refused" => StatusCode::FORBIDDEN,
        "not_found" => StatusCode::NOT_FOUND,
        "conflict" => StatusCode::CONFLICT,
        "transport" => StatusCode::BAD_GATEWAY,
        "timeout" => StatusCode::GATEWAY_TIMEOUT,
        "needs_person" => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "error": { "code": code, "fact": fact.into(), "action": action.into() } }))).into_response()
}

fn not_built(part: &str, verb: &str, action: &str) -> Response {
    error("not_found", format!("{part} {verb} is not built yet"), action)
}

fn internal(e: anyhow::Error) -> Response {
    if backend::is_transport(&e) {
        return error("transport", format!("{e:#}"), "Start the pane engine (allternit-factory pane) and retry.");
    }
    error("internal", format!("{e:#}"), "This is an engine failure; keep the engine log and report it.")
}

// ---------------------------------------------------------------- agents

async fn health() -> Response {
    Json(json!({ "ok": true, "service": "allternit-factory", "panes": backend::installed() })).into_response()
}

async fn full_snapshot(state: &ServiceState) -> Result<view::Snapshot, Response> {
    view::snapshot(&state.root_dir, &Registry::open_default(), None).await.map_err(internal)
}

async fn snapshot(state: &ServiceState) -> Result<Vec<Agent>, Response> {
    full_snapshot(state).await.map(|s| s.agents)
}

#[derive(Deserialize)]
struct TeamQ {
    team: Option<String>,
}

async fn agents_list(State(state): S, Query(q): Query<TeamQ>) -> Response {
    let snap = match full_snapshot(&state).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut agents = snap.agents;
    if let Some(team) = q.team {
        agents.retain(|a| a.team.as_deref() == Some(team.as_str()));
    }
    // `engine` (additive): whether the pane engine is up, so a screen can say
    // why every terminal bot reads offline.
    Json(json!({ "agents": agents, "engine": snap.engine })).into_response()
}

async fn find_agent(state: &ServiceState, id: &str) -> Result<Agent, Response> {
    let agents = snapshot(state).await?;
    view::find(&agents, id)
        .cloned()
        .ok_or_else(|| error("not_found", format!("no agent {id}"), "List agents with GET /api/factory/agents."))
}

async fn agent_get(State(state): S, Path(id): Path<String>) -> Response {
    match find_agent(&state, &id).await {
        Ok(a) => Json(a).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct LinesQ {
    lines: Option<u32>,
}

async fn agent_capture(State(state): S, Path(id): Path<String>, Query(q): Query<LinesQ>) -> Response {
    let agent = match find_agent(&state, &id).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if agent.pane.is_none() {
        return error("not_found", format!("{} has no live pane", agent.address), "Start it, or read its transcript.");
    }
    let pane = match backend::backend() {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    let session = registry::session_of(&agent.slug);
    let lines = q.lines.unwrap_or(80).clamp(1, 2000);
    match backend::blocking(move || pane.capture(&session, lines)).await {
        Ok(text) => Json(json!({ "text": text, "at": chrono::Utc::now().to_rfc3339() })).into_response(),
        Err(e) => internal(e),
    }
}

// ---------------------------------------------------------------- pane mirror

/// The live pane session of a terminal agent, or the error to answer with.
async fn pane_session(state: &ServiceState, id: &str) -> Result<(Agent, String), Response> {
    let agent = find_agent(state, id).await?;
    if agent.pane.is_none() {
        return Err(error("not_found", format!("{} has no live pane", agent.address), "Start it, or read its transcript."));
    }
    let session = registry::session_of(&agent.slug);
    Ok((agent, session))
}

async fn read_screen(session: String) -> anyhow::Result<backend::PaneScreen> {
    let pane = backend::backend()?;
    backend::blocking(move || pane.screen(&session)).await
}

/// `GET /agents/:id/screen` → `{ ansi, revision, at }`: the pane's visible
/// screen with its colors, the same screen the Rust pane wall draws.
async fn agent_screen(State(state): S, Path(id): Path<String>) -> Response {
    let (_, session) = match pane_session(&state, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match read_screen(session).await {
        Ok(s) => Json(json!({ "ansi": s.ansi, "revision": s.revision, "at": chrono::Utc::now().to_rfc3339() })).into_response(),
        Err(e) => internal(e),
    }
}

/// How often the mirror stream re-reads the screen. Only changed screens are
/// sent (the pane engine's `revision`).
const SCREEN_POLL: Duration = Duration::from_millis(200);

/// `GET /agents/:id/stream` (SSE): `screen` events `{ansi, revision, at}`
/// whenever the pane's screen changes (the first one at once), then one
/// `gone` event `{reason}` when the pane closes or can't be read, and the
/// stream ends. A mirror of the engine pane: the same pane the TUI wall
/// shows, so typing in either shows in both. Live only, no SSE ids.
async fn agent_stream(State(state): S, Path(id): Path<String>) -> Response {
    let session = match pane_session(&state, &id).await {
        Ok((_, s)) => s,
        Err(r) => return r,
    };
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        let mut last: Option<u64> = None;
        loop {
            match read_screen(session.clone()).await {
                Ok(s) => {
                    if last != Some(s.revision) {
                        last = Some(s.revision);
                        let body = json!({ "ansi": s.ansi, "revision": s.revision, "at": chrono::Utc::now().to_rfc3339() });
                        if tx.send(Ok(Event::default().event("screen").data(body.to_string()))).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let body = json!({ "reason": format!("{e:#}") });
                    let _ = tx.send(Ok(Event::default().event("gone").data(body.to_string()))).await;
                    return;
                }
            }
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(SCREEN_POLL).await;
        }
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

#[derive(Deserialize)]
struct InputBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    keys: Vec<String>,
}

/// `POST /agents/:id/input` `{ text?, keys? }` → `{ ok: true }`: keystrokes
/// into the pane, as a person at the wall would type them. Not a send: no
/// delivery, no ledger record (use `POST /send` for a message to the agent).
async fn agent_input(State(state): S, Path(id): Path<String>, body: Option<Json<InputBody>>) -> Response {
    let Some(Json(body)) = body else {
        return error("usage", "body must be JSON { text?, keys? }", "Send at least one of text or keys.");
    };
    if body.text.is_empty() && body.keys.is_empty() {
        return error("usage", "nothing to type: text and keys are both empty", "Send at least one of text or keys.");
    }
    let session = match pane_session(&state, &id).await {
        Ok((_, s)) => s,
        Err(r) => return r,
    };
    let pane = match backend::backend() {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    match backend::blocking(move || pane.input(&session, &body.text, &body.keys)).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct CursorQ {
    cursor: Option<String>,
}

/// Transcript chunks from the pane's tee'd log. `cursor` is a byte offset;
/// `next` is null once the session is gone and the log is fully read.
async fn agent_transcript(State(state): S, Path(id): Path<String>, Query(q): Query<CursorQ>) -> Response {
    const CHUNK: u64 = 64 * 1024;
    let agent = match find_agent(&state, &id).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let session = registry::session_of(&agent.slug);
    let entry = Registry::open_default().load().ok().and_then(|f| f.sessions.get(&session).cloned());
    let Some(log) = entry.and_then(|e| e.log) else {
        return error("not_found", format!("{} has no recorded transcript", agent.address), "Only engine-started panes keep a transcript.");
    };
    let from: u64 = match q.cursor.as_deref().map(str::parse) {
        None => 0,
        Some(Ok(n)) => n,
        Some(Err(_)) => return error("usage", "cursor must be the `next` value of a previous page", "Omit cursor to start at the beginning."),
    };
    let read = tokio::task::spawn_blocking(move || -> std::io::Result<(String, u64, u64, Option<String>)> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&log)?;
        let meta = f.metadata()?;
        let len = meta.len();
        let start = from.min(len);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.take(CHUNK).read_to_end(&mut buf)?;
        let at = meta.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
        Ok((String::from_utf8_lossy(&buf).into_owned(), start + buf.len() as u64, len, at))
    })
    .await;
    let (text, end, len, at) = match read {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return error("not_found", format!("transcript unreadable: {e}"), "The log may have been removed."),
        Err(e) => return internal(anyhow::anyhow!("{e}")),
    };
    let finished = agent.pane.is_none() && end >= len;
    let chunks = if text.is_empty() {
        Vec::new()
    } else {
        vec![json!({ "at": at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()), "text": text })]
    };
    Json(json!({ "chunks": chunks, "next": (!finished).then(|| end.to_string()) })).into_response()
}

/// Teams: every team.yaml (presets, edges, `invalid` for ones that don't
/// parse), plus teams only the registry knows (bots carrying a team with no
/// team.yaml), with no presets or edges.
async fn teams_list(State(state): S) -> Response {
    let mut listing = match crate::agents::http::teams_listing(state.root_dir.clone()).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let agents = match snapshot(&state).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let known: std::collections::BTreeSet<String> = listing["teams"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(listing["invalid"].as_array().into_iter().flatten())
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();
    let mut teams: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for a in &agents {
        if let Some(t) = a.team.as_ref().filter(|t| !known.contains(*t)) {
            teams.entry(t.clone()).or_default().push(a.id.clone());
        }
    }
    if let Some(list) = listing["teams"].as_array_mut() {
        list.extend(
            teams
                .into_iter()
                .map(|(name, agents)| json!({ "name": name, "preset": null, "presets": [], "agents": agents, "edges": [] })),
        );
    }
    Json(listing).into_response()
}

async fn teams_up(
    State(state): S,
    headers: HeaderMap,
    Path(name): Path<String>,
    body: Option<Json<crate::agents::http::UpBody>>,
) -> Response {
    // Terminal bots register with allternit-api as the caller (the proxy's
    // link); without one, the engine's own env decides.
    let api = crate::agents::team_apply::ApiClient::from_link(&api_link(&headers))
        .map(|c| Arc::new(c) as Arc<dyn crate::agents::team_apply::FactoryApi>);
    crate::agents::http::up(state.root_dir.clone(), api, name, body).await
}

async fn teams_down(State(state): S, Path(name): Path<String>, body: Option<Json<crate::agents::http::DownBody>>) -> Response {
    crate::agents::http::down(state.root_dir.clone(), name, body).await
}

// ---------------------------------------------------------------- send

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string).filter(|v| !v.is_empty())
}

/// Who is calling: allternit-api's proxy names the signed-in user.
fn caller(headers: &HeaderMap) -> String {
    header(headers, "x-allternit-user").map(|u| format!("user:{u}")).unwrap_or_else(|| "engine".to_string())
}

/// How to reach allternit-api for this request: the proxy says where it is
/// and passes the caller's own credentials through.
fn api_link(headers: &HeaderMap) -> ApiLink {
    let mut link = ApiLink::from_env();
    if let Some(base) = header(headers, "x-allternit-api-base") {
        link.base = Some(base);
    }
    if let Some(auth) = header(headers, "authorization") {
        link.authorization = Some(auth);
    }
    if let (Some(token), Some(user)) =
        (header(headers, "x-allternit-desktop-access-token"), header(headers, "x-allternit-user-id"))
    {
        link.desktop = Some((token, user));
    }
    link
}

async fn send_h(State(state): S, headers: HeaderMap, body: Option<Json<SendRequest>>) -> Response {
    let Some(Json(mut req)) = body else {
        return error("usage", "send needs a JSON body { to, text }", "See API.md: POST /api/factory/send.");
    };
    if req.idempotency_key.is_none() {
        req.idempotency_key = header(&headers, "idempotency-key");
    }
    let ctx = SendCtx {
        root: state.root_dir.clone(),
        registry: Registry::open_default(),
        api: api_link(&headers),
        sender: caller(&headers),
    };
    if req.dry_run {
        return match send::plan(&ctx, &req).await {
            Ok(plan) => Json(json!({ "dryRun": true, "plan": plan })).into_response(),
            Err(e) => error(e.code, e.fact, e.action),
        };
    }
    match send::send(&ctx, &req).await {
        Ok(d) => Json(d).into_response(),
        Err(e) => error(e.code, e.fact, e.action),
    }
}

async fn deliveries_h(State(state): S, Query(f): Query<DeliveryFilter>) -> Response {
    match send::deliveries(&state.root_dir, &f).await {
        Ok(d) => Json(json!({ "deliveries": d })).into_response(),
        Err(e) => internal(e),
    }
}

// ---------------------------------------------------------------- events

/// A ledger event as an API.md event, or `None` when it isn't one.
pub fn factory_event(evt: &AllternitEvent) -> Option<(String, Value)> {
    let p = &evt.payload;
    let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
    let (ty, data) = match evt.r#type.as_str() {
        send::DELIVERY_EVENT => {
            let mut d = p.clone();
            if let Some(o) = d.as_object_mut() {
                o.remove("idempotencyKey");
                o.remove("sender");
            }
            ("delivery", d)
        }
        "JudgeVerdictRecorded" | "JudgeHumanResolved" => (
            "proof.recorded",
            json!({ "dagId": s("dag_id"), "nodeId": s("node_id"), "outcome": s("outcome").or_else(|| s("verdict")) }),
        ),
        t if s("dag_id").is_some() && s("node_id").is_some() && is_node_status_event(t) => (
            "node.status",
            json!({ "dagId": s("dag_id"), "nodeId": s("node_id"), "cause": t, "status": s("status").or_else(|| s("to")) }),
        ),
        _ => return None,
    };
    Some((ty.to_string(), json!({ "type": ty, "at": evt.ts, "data": data })))
}

fn is_node_status_event(t: &str) -> bool {
    matches!(
        t,
        "DagNodeCreated"
            | "DagNodeUpdated"
            | "DagNodeStatusChanged"
            | "DagNodeRemoved"
            | "DagNodeOutputRecorded"
            | "DagNodeWaitGateAdded"
            | "DagNodeWaitGateResolved"
            | "WIHCreated"
            | "WIHPickedUp"
            | "WIHCloseDenied"
            | "WIHClosed"
            | "WIHClosedSigned"
            | "WIHReclaimed"
    )
}

#[derive(Deserialize)]
struct EventsQ {
    /// Same as the `Last-Event-ID` header (for clients that can't set it).
    after: Option<String>,
}

/// Server-Sent Events. Each ledger fact that is an API.md event is sent with
/// its ledger event id as the SSE id, so `Last-Event-ID` replays exactly what
/// was missed. `agent.state` events are live facts (pane state, not ledger
/// events) and carry no id. Polls the ledger and the panes once a second.
async fn events_h(State(state): S, headers: HeaderMap, Query(q): Query<EventsQ>) -> Response {
    let last = header(&headers, "last-event-id").or(q.after);
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(256);
    let root = state.root_dir.clone();
    let ledger = state.ledger.clone();
    tokio::spawn(async move {
        // Without a Last-Event-ID the stream starts now (no history dump).
        let mut cursor: Option<String> = match last {
            Some(id) => Some(id),
            None => ledger.tail(1).await.ok().and_then(|t| t.last().map(|e| e.event_id.clone())).or_else(|| Some(String::new())),
        };
        let mut states: HashMap<String, String> = HashMap::new();
        let mut first = true;
        loop {
            if let Ok(events) = ledger.query(LedgerQuery::default()).await {
                let after = cursor.clone().unwrap_or_default();
                for evt in events.iter().filter(|e| e.event_id.as_str() > after.as_str()) {
                    cursor = Some(evt.event_id.clone());
                    if let Some((ty, body)) = factory_event(evt) {
                        let event = Event::default().id(evt.event_id.clone()).event(ty).data(body.to_string());
                        if tx.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            if let Ok(snap) = view::snapshot(&root, &Registry::open_default(), None).await {
                for a in snap.agents {
                    let changed = states.get(&a.id) != Some(&a.state);
                    if changed && !first {
                        let body = json!({ "type": "agent.state", "at": chrono::Utc::now().to_rfc3339(),
                            "data": { "agentId": a.id, "state": a.state, "pane": a.pane } });
                        if tx.send(Ok(Event::default().event("agent.state").data(body.to_string()))).await.is_err() {
                            return;
                        }
                    }
                    states.insert(a.id, a.state);
                }
            }
            first = false;
            if tx.is_closed() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ---------------------------------------------------------------- templates / runs

fn template_json(t: &Template) -> Value {
    let steps: Vec<Value> = t
        .steps
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "title": s.title,
                "executor": s.executor,
                "blockedBy": s.blocked_by,
                "onFail": null,
                "waitGate": s.wait_gate.as_ref().map(|g| json!({
                    "kind": serde_json::to_value(&g.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                    "evidence": g.description,
                })),
                "retry": s.retry,
            })
        })
        .collect();
    json!({
        "id": t.id,
        "name": t.name,
        "description": t.description,
        "params": t.params.iter().map(|p| json!({ "name": p.name, "description": p.description, "default": p.default })).collect::<Vec<_>>(),
        "steps": steps,
        "maxRounds": null,
        "closure": null,
    })
}

fn template_store(root: &std::path::Path) -> Result<Option<TemplateStore>, Response> {
    if !root.join(TEMPLATE_DIR).is_dir() {
        return Ok(None);
    }
    TemplateStore::new(root).map(Some).map_err(internal)
}

async fn templates_list(State(state): S) -> Response {
    let list = match template_store(&state.root_dir) {
        Ok(Some(store)) => match store.list() {
            Ok(l) => l,
            Err(e) => return internal(e),
        },
        Ok(None) => Vec::new(),
        Err(r) => return r,
    };
    let summaries: Vec<Value> = list
        .iter()
        .map(|t| json!({ "id": t.id, "name": t.name, "description": t.description, "steps": t.steps.len(), "params": t.params.len() }))
        .collect();
    Json(json!({ "templates": summaries })).into_response()
}

fn resolve_template(root: &std::path::Path, id: &str) -> Result<Template, Response> {
    match template_store(root)? {
        Some(store) => store.resolve(id).map_err(|e| {
            let text = format!("{e:#}");
            if text.contains("not found") {
                error("not_found", format!("template {id} not found"), "List templates with GET /api/factory/templates.")
            } else {
                error("usage", text, "Fix the template file.")
            }
        }),
        None => Err(error("not_found", format!("template {id} not found (no {TEMPLATE_DIR})"), "Add templates under .allternit/rails/templates/.")),
    }
}

async fn template_get(State(state): S, Path(id): Path<String>) -> Response {
    match resolve_template(&state.root_dir, &id) {
        Ok(t) => Json(template_json(&t)).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunBody {
    template: String,
    intent: Option<String>,
    #[serde(default)]
    params: HashMap<String, String>,
    campaign_id: Option<String>,
    project_id: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

async fn runs_create(State(state): S, headers: HeaderMap, body: Option<Json<RunBody>>) -> Response {
    let Some(Json(b)) = body else {
        return error("usage", "runs needs a JSON body { template, campaignId|projectId }", "See API.md: POST /api/factory/runs.");
    };
    let template = match resolve_template(&state.root_dir, &b.template) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let campaigns = match campaign_ops(&state).all().await {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let campaign_id = match (&b.campaign_id, &b.project_id) {
        (Some(c), _) if campaigns.contains_key(c) => c.clone(),
        (Some(c), _) => return error("not_found", format!("campaign {c} not declared"), "Declare it with `gizzi workspace campaign new`."),
        (None, Some(p)) => match campaigns.keys().find(|id| *id == p || **id == format!("project-{p}")) {
            Some(id) => id.clone(),
            None => {
                return not_built(
                    "workflows",
                    "run with a new project",
                    "Creating a project's campaign from a run is not built yet; declare the campaign first and pass campaignId.",
                )
            }
        },
        (None, None) => return error("usage", "give campaignId or projectId", "Project = Campaign: name the one this run belongs to."),
    };
    let mut params = b.params.clone();
    if let Some(intent) = &b.intent {
        if template.params.iter().any(|p| p.name == "intent") {
            params.entry("intent".to_string()).or_insert_with(|| intent.clone());
        }
    }
    let expansion = match template.expand_dag("__root__", &params) {
        Ok(x) => x,
        Err(e) => return error("usage", format!("{e:#}"), "Check the template's params."),
    };
    if b.dry_run {
        let nodes: Vec<Value> = template
            .steps
            .iter()
            .map(|s| json!({ "nodeId": expansion.nodes.get(&s.id), "stepId": s.id, "title": s.title, "executor": s.executor }))
            .collect();
        return Json(json!({ "dryRun": true, "plan": { "template": template.id, "campaignId": campaign_id, "nodes": nodes,
            "records": ["PromptCreated", "DagCreated", "DagNodeCreated…", RUN_EVENT] } }))
        .into_response();
    }
    let result = match plan_from_template(&state.gate, &template, &params, b.intent.as_deref(), Some(campaign_id.clone())).await {
        Ok(r) => r,
        Err(e) => return error("refused", format!("{e:#}"), "Read the Gate's reason, then change the request."),
    };
    let actor = match header(&headers, "x-allternit-user") {
        Some(u) => Actor { r#type: ActorType::User, id: u },
        None => Actor { r#type: ActorType::Gate, id: "engine".into() },
    };
    let scope = EventScope { project_id: Some(campaign_id.clone()), dag_id: Some(result.dag_id.clone()), ..Default::default() };
    if let Err(e) = state
        .gate
        .record_factory_event(
            RUN_EVENT,
            actor,
            Some(scope),
            json!({ "dagId": result.dag_id, "campaignId": campaign_id, "template": template.id, "intent": b.intent }),
        )
        .await
    {
        return internal(e);
    }
    let events = match all_events(&state).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let dag = project_dag(&events, &result.dag_id);
    Json(json!({ "dagId": result.dag_id, "campaignId": campaign_id, "nodes": cards(&dag, &events) })).into_response()
}

// ---------------------------------------------------------------- DAGs / nodes / campaigns

async fn all_events(state: &ServiceState) -> Result<Vec<AllternitEvent>, Response> {
    state.ledger.query(LedgerQuery::default()).await.map_err(internal)
}

fn dag_ids(events: &[AllternitEvent]) -> Vec<String> {
    let mut ids = Vec::new();
    for e in events.iter().filter(|e| e.r#type == "DagCreated") {
        if let Some(id) = e.payload.get("dag_id").and_then(Value::as_str) {
            if !ids.iter().any(|x| x == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

/// API.md NodeCard status for an engine node status.
pub fn card_status(node: &DagNode, ready: bool, waiting_on_person: bool) -> &'static str {
    if waiting_on_person {
        return "needs_you";
    }
    match node.status.as_str() {
        "NEW" | "READY" if ready => "ready",
        "NEW" | "READY" => "new",
        "IN_PROGRESS" | "RUNNING" => "working",
        "VERIFYING" => "checking",
        "NEEDS_HUMAN" => "needs_you",
        "DONE" | "CLOSED" => "done",
        "FAILED" | "EXCEPTION" | "CANCELLED" => "failed",
        _ => "blocked",
    }
}

fn depth_of(dag: &DagState, node_id: &str, memo: &mut HashMap<String, usize>, seen: &mut Vec<String>) -> usize {
    if let Some(d) = memo.get(node_id) {
        return *d;
    }
    if seen.iter().any(|s| s == node_id) {
        return 0; // a cycle can't be in a valid DAG; don't loop on a bad one
    }
    seen.push(node_id.to_string());
    let d = dag
        .edges
        .iter()
        .filter(|e| e.to_node_id == node_id && e.edge_type == "blocked_by")
        .map(|e| depth_of(dag, &e.from_node_id, memo, seen) + 1)
        .max()
        .unwrap_or(0);
    seen.pop();
    memo.insert(node_id.to_string(), d);
    d
}

/// Every node of a DAG as API.md NodeCards (the root plan node excluded).
pub fn cards(dag: &DagState, events: &[AllternitEvent]) -> Vec<Value> {
    let ready: std::collections::HashSet<String> = ready_nodes(dag).into_iter().collect();
    let now = chrono::Utc::now();
    let mut memo = HashMap::new();
    let mut nodes: Vec<&DagNode> = dag.nodes.values().filter(|n| n.parent_node_id.is_some()).collect();
    nodes.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.node_id.cmp(&b.node_id)));
    nodes
        .into_iter()
        .map(|n| {
            let manual = n.blocking_wait_gates(now).into_iter().find(|g| {
                serde_json::to_value(&g.kind).ok().and_then(|v| v.as_str().map(str::to_string)).as_deref() != Some("timer")
            });
            let gate_kind = manual.map(|g| {
                serde_json::to_value(&g.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
            });
            let status = card_status(n, ready.contains(&n.node_id), gate_kind.is_some() && n.status != "DONE");
            let judged = project_node_judge(events, &dag.dag_id, &n.node_id);
            let proven = judged.last().is_some_and(|v| v.outcome.eq_ignore_ascii_case("accomplished"));
            let assignee = n
                .assignee
                .clone()
                .or_else(|| n.executor.as_deref().and_then(|e| e.strip_prefix("bot:")).map(str::to_string));
            let binding = match n.executor.as_deref() {
                Some(e) if e.starts_with("ao:") => Some("terminal"),
                _ => None,
            };
            let blocked_by: Vec<&str> = dag
                .edges
                .iter()
                .filter(|e| e.to_node_id == n.node_id && e.edge_type == "blocked_by")
                .map(|e| e.from_node_id.as_str())
                .collect();
            json!({
                "dagId": dag.dag_id,
                "nodeId": n.node_id,
                "title": n.title,
                "status": status,
                "assignee": assignee,
                "bindingType": binding,
                "proof": { "proven": if proven { 1 } else { 0 }, "total": if judged.verdicts.is_empty() { 0 } else { 1 } },
                "blockedBy": blocked_by,
                "needsYou": status == "needs_you",
                "depth": depth_of(dag, &n.node_id, &mut memo, &mut Vec::new()),
                "gate": gate_kind.map(|k| json!({ "kind": k })),
            })
        })
        .collect()
}

async fn dag_get(State(state): S, Path(dag_id): Path<String>) -> Response {
    let events = match all_events(&state).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    if !dag_ids(&events).contains(&dag_id) {
        return error("not_found", format!("no DAG {dag_id}"), "Check the id (runs return it as dagId).");
    }
    let dag = project_dag(&events, &dag_id);
    let title = dag
        .nodes
        .values()
        .find(|n| n.parent_node_id.is_none())
        .map(|n| n.title.clone())
        .unwrap_or_else(|| dag_id.clone());
    // Edges between the cards only: the root plan node (not a card) is
    // blocked by the last steps, and that edge is bookkeeping, not work.
    let is_card = |id: &str| dag.nodes.get(id).is_some_and(|n| n.parent_node_id.is_some());
    let edges: Vec<Value> = dag
        .edges
        .iter()
        .filter(|e| (e.edge_type == "blocked_by" || e.edge_type == "on_fail") && is_card(&e.from_node_id) && is_card(&e.to_node_id))
        .map(|e| json!({ "from": e.from_node_id, "to": e.to_node_id, "type": e.edge_type }))
        .collect();
    Json(json!({ "dagId": dag_id, "title": title, "nodes": cards(&dag, &events), "edges": edges })).into_response()
}

fn campaign_ops(state: &ServiceState) -> crate::campaign::CampaignOps {
    crate::campaign::CampaignOps::new(
        state.root_dir.clone(),
        state.ledger.clone(),
        crate::campaign::DEFAULT_CHECK_CEILING_SECS,
        Default::default(),
    )
}

/// DAGs belonging to each campaign: its declared DAG plus every run
/// recorded against it.
fn campaign_dags(events: &[AllternitEvent]) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for e in events.iter().filter(|e| e.r#type == RUN_EVENT) {
        if let (Some(c), Some(d)) = (e.payload["campaignId"].as_str(), e.payload["dagId"].as_str()) {
            out.entry(c.to_string()).or_default().push(d.to_string());
        }
    }
    out
}

async fn campaigns_list(State(state): S) -> Response {
    let campaigns = match campaign_ops(&state).all().await {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    let events = match all_events(&state).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let runs = campaign_dags(&events);
    let list: Vec<Value> = campaigns
        .values()
        .map(|c| {
            let mut dags: Vec<String> = c.dag_id.iter().cloned().collect();
            dags.extend(runs.get(&c.campaign_id).cloned().unwrap_or_default());
            let (mut proven, mut total, mut needs) = (0u64, 0u64, 0u64);
            for d in &dags {
                for card in cards(&project_dag(&events, d), &events) {
                    total += 1;
                    proven += card["proof"]["proven"].as_u64().unwrap_or(0);
                    needs += card["needsYou"].as_bool().unwrap_or(false) as u64;
                }
            }
            let status = serde_json::to_value(c.status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            json!({ "id": c.campaign_id, "projectId": null, "title": c.objective, "intent": c.objective,
                    "status": status, "proven": proven, "total": total, "needsYou": needs })
        })
        .collect();
    Json(json!({ "campaigns": list })).into_response()
}

async fn campaign_board(State(state): S, Path(id): Path<String>) -> Response {
    crate::workspace::http::board(state.root_dir.clone(), state.ledger.clone(), state.gate.clone(), id).await
}

#[derive(Deserialize)]
struct NodesQ {
    assignee: Option<String>,
    status: Option<String>,
}

async fn nodes_list(State(state): S, Query(q): Query<NodesQ>) -> Response {
    let open_only = match q.status.as_deref() {
        None | Some("open") => true,
        Some("all") => false,
        Some(other) => return error("usage", format!("status must be open or all, not {other}"), "Use ?status=open or ?status=all."),
    };
    let events = match all_events(&state).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let assignee = q.assignee.as_deref().map(|a| a.split('@').next().unwrap_or(a).to_string());
    let mut nodes = Vec::new();
    for d in dag_ids(&events) {
        for card in cards(&project_dag(&events, &d), &events) {
            let st = card["status"].as_str().unwrap_or_default();
            if open_only && (st == "done" || st == "failed") {
                continue;
            }
            if let Some(a) = &assignee {
                let card_a = card["assignee"].as_str().map(|x| x.split('@').next().unwrap_or(x));
                if card_a != Some(a.as_str()) {
                    continue;
                }
            }
            nodes.push(card);
        }
    }
    Json(json!({ "nodes": nodes })).into_response()
}

async fn node_get(State(state): S, Path((dag_id, node_id)): Path<(String, String)>) -> Response {
    let events = match all_events(&state).await {
        Ok(e) => e,
        Err(r) => return r,
    };
    let dag = project_dag(&events, &dag_id);
    let Some(node) = dag.nodes.get(&node_id) else {
        return error("not_found", format!("no node {dag_id}/{node_id}"), "Check the ids.");
    };
    let card = cards(&dag, &events).into_iter().find(|c| c["nodeId"] == json!(node_id));
    let deliveries = send::deliveries(&state.root_dir, &DeliveryFilter { node: Some(node_id.clone()), ..Default::default() })
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|d| d.dag_id.as_deref().map_or(true, |x| x == dag_id))
        .collect::<Vec<_>>();
    let wih = node.current_wih_id.as_ref().map(|id| json!({ "id": id, "status": node.status }));
    // spec / progress / proof / files come from the node folder; vendor
    // tickets (ledger) join the send deliveries. Approvals are served by
    // allternit-api.
    let folder = crate::workspace::http::node_page_value(state.root_dir.clone(), state.ledger.clone(), &dag_id, &node_id)
        .await
        .unwrap_or(Value::Null);
    let mut deliveries: Vec<Value> = deliveries.into_iter().map(|d| serde_json::to_value(d).unwrap_or_default()).collect();
    let seen: std::collections::HashSet<String> =
        deliveries.iter().filter_map(|d| d["id"].as_str().map(str::to_string)).collect();
    deliveries.extend(
        folder["deliveries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["id"].as_str().map_or(true, |id| !seen.contains(id)))
            .cloned(),
    );
    Json(json!({
        "card": card,
        "spec": folder["spec"],
        "progressMd": folder["progressMd"],
        "proofMd": folder["proofMd"],
        "files": folder.get("files").cloned().unwrap_or_else(|| json!([])),
        "deliveries": deliveries,
        "wih": wih,
        "approval": null,
    }))
    .into_response()
}

async fn node_proof(
    State(state): S,
    Path((dag_id, node_id)): Path<(String, String)>,
    form: axum::extract::Multipart,
) -> Response {
    crate::workspace::http::proof_upload(state.root_dir.clone(), state.ledger.clone(), state.gate.clone(), dag_id, node_id, form)
        .await
}

/// The workspace root this server reads (for tests and the proxy's health).
pub fn root_of(state: &ServiceState) -> PathBuf {
    state.root_dir.clone()
}
