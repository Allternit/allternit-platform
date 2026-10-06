//! `allternit-factory internal core bridge serve`: the scoped remote listener.
//!
//! Every request is authenticated (bearer token -> [`Identity`]), rate
//! limited per identity, classified against a fixed route table
//! ([`classify_route`]), and audited to the ledger with the caller as actor.
//! Only plan create/read, template instantiate, and mail send/read exist
//! here; execution (`wih pickup/close`), leases, wait-gate resolution and gate
//! approvals are answered 403 for any authenticated caller, whatever scopes
//! its file entry claims.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Extension, Json, Path, Query, Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use super::identity::{Identity, IdentityStore, Scope};
use crate::core::ids::create_event_id;
use crate::core::types::{Actor, ActorType, AllternitEvent, EventScope, LedgerQuery};
use crate::gate::gate::PromptOrigin;
use crate::gate::{Gate, GateError, GateOptions};
use crate::leases::leases::LeasesOptions;
use crate::ledger::ledger::LedgerOptions;
use crate::mail::{canonical_thread_id, MailImportance, TypedMessage};
use crate::templates::{plan_from_template_with_origin, TemplateStore};
use crate::work::projection::project_dag;
use crate::{Leases, Ledger, Mail, MailOptions, ReceiptStore, ReceiptStoreOptions};

/// Loopback default. The bridge never binds a network interface unless told.
pub const DEFAULT_BRIDGE_BIND: &str = "127.0.0.1:7433";
/// Requests per identity per minute.
pub const DEFAULT_RATE_LIMIT_PER_MIN: u32 = 60;

const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_PLAN_TEXT: usize = 16 * 1024;
const MAX_MAIL_BODY: usize = 64 * 1024;
const MAX_SUBJECT: usize = 256;
const MAX_INBOX_LIMIT: usize = 200;
const BRIDGE_ACTOR_ID: &str = "bridge";

/// Listener configuration.
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    pub bind: SocketAddr,
    pub root: PathBuf,
    pub identities_path: PathBuf,
    pub allow_remote: bool,
    pub rate_limit_per_min: u32,
}

/// Refuse unsafe binds before any socket is opened.
///
/// - loopback: always allowed (identity auth is still required per request);
/// - unspecified (`0.0.0.0`, `::`): always refused — bind the one mesh address;
/// - anything else: needs `--allow-remote` AND at least one active identity.
///
/// Also fails if the identities file is readable by group/other.
pub fn check_bind(cfg: &BridgeConfig, store: &IdentityStore) -> Result<()> {
    let active = store.active_count()?;
    let ip = cfg.bind.ip();
    if ip.is_loopback() {
        return Ok(());
    }
    if ip.is_unspecified() {
        bail!(
            "refusing to bind {}: the bridge never listens on every interface; \
             bind the single mesh address (e.g. 100.x.y.z:7433)",
            cfg.bind
        );
    }
    if !cfg.allow_remote {
        bail!(
            "refusing to bind non-loopback address {} without --allow-remote",
            cfg.bind
        );
    }
    if active == 0 {
        bail!(
            "refusing to bind non-loopback address {}: no active identities in {} \
             (run `allternit-factory internal core identity add` first)",
            cfg.bind,
            store.path().display()
        );
    }
    Ok(())
}

/// What a (method, path) pair is allowed to do on the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteClass {
    /// Authenticated, no scope needed (`GET /v1/whoami`).
    Open,
    /// Needs this scope.
    Scoped(Scope),
    /// Never served to a remote identity; the label names the withheld
    /// capability (`wih:pickup`, `wih:close`, `lease:*`, ...).
    Forbidden(&'static str),
    /// Not a bridge route.
    Unknown,
}

/// The bridge route table. Single source of truth for both authorization and
/// the forbidden-capability 403s.
pub fn classify_route(method: &Method, path: &str) -> RouteClass {
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let m = method.clone();
    // Forbidden families first, for any method: a remote identity gets a 403
    // naming the withheld capability, never a 404 that invites probing.
    match segs.as_slice() {
        ["v1", "wihs", "pickup", ..] => return RouteClass::Forbidden("wih:pickup"),
        ["v1", "wihs", _, "close", ..] => return RouteClass::Forbidden("wih:close"),
        ["v1", "wihs", ..] | ["v1", "wih", ..] => return RouteClass::Forbidden("wih:pickup"),
        ["v1", "leases", ..] | ["v1", "lease", ..] => return RouteClass::Forbidden("lease:*"),
        ["v1", "mail", "reserve", ..] | ["v1", "mail", "release", ..] => {
            return RouteClass::Forbidden("lease:*")
        }
        ["v1", "wait-gates", ..] | ["v1", "wait-gate", ..] => {
            return RouteClass::Forbidden("wait-gate:resolve")
        }
        ["v1", "gate", ..] | ["v1", "mail", "decide", ..] | ["v1", "mail", "review", ..] => {
            return RouteClass::Forbidden("gate:*")
        }
        // Graph mutation on an existing plan (incl. ChangeStatus) could
        // emulate a close; it stays local.
        ["v1", "plan", "refine", ..] => return RouteClass::Forbidden("plan:refine"),
        _ => {}
    }
    match (m, segs.as_slice()) {
        (Method::GET, ["v1", "whoami"]) => RouteClass::Open,
        (Method::POST, ["v1", "plan"]) => RouteClass::Scoped(Scope::PlanCreate),
        (Method::GET, ["v1", "plan", _]) => RouteClass::Scoped(Scope::PlanRead),
        (Method::GET, ["v1", "plan", _, "nodes", _, "output"]) => {
            RouteClass::Scoped(Scope::PlanRead)
        }
        (Method::GET, ["v1", "templates"]) => RouteClass::Scoped(Scope::TemplateInstantiate),
        (Method::POST, ["v1", "templates", _, "instantiate"]) => {
            RouteClass::Scoped(Scope::TemplateInstantiate)
        }
        (Method::POST, ["v1", "mail", "send"]) => RouteClass::Scoped(Scope::MailSend),
        (Method::GET, ["v1", "mail", "inbox"]) => RouteClass::Scoped(Scope::MailRead),
        _ => RouteClass::Unknown,
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Window {
    start: Instant,
    count: u32,
    reported: bool,
}

enum RateDecision {
    Allowed,
    /// `first` = first refusal in this window (audit it once, not per request).
    Limited {
        first: bool,
        retry_after: u64,
    },
}

struct RateLimiter {
    per_min: u32,
    windows: Mutex<HashMap<String, Window>>,
}

impl RateLimiter {
    fn new(per_min: u32) -> Self {
        Self {
            per_min: per_min.max(1),
            windows: Mutex::new(HashMap::new()),
        }
    }

    fn check(&self, key: &str) -> RateDecision {
        let now = Instant::now();
        let period = Duration::from_secs(60);
        let mut windows = self.windows.lock().unwrap_or_else(|e| e.into_inner());
        if windows.len() > 10_000 {
            windows.retain(|_, w| now.duration_since(w.start) < period);
        }
        let w = windows.entry(key.to_string()).or_insert(Window {
            start: now,
            count: 0,
            reported: false,
        });
        if now.duration_since(w.start) >= period {
            *w = Window {
                start: now,
                count: 0,
                reported: false,
            };
        }
        if w.count < self.per_min {
            w.count += 1;
            return RateDecision::Allowed;
        }
        let first = !w.reported;
        w.reported = true;
        let retry_after = period
            .saturating_sub(now.duration_since(w.start))
            .as_secs()
            .max(1);
        RateDecision::Limited { first, retry_after }
    }
}

/// Shared listener state.
pub struct BridgeState {
    root: PathBuf,
    ledger: Arc<Ledger>,
    gate: Arc<Gate>,
    receipts: Arc<ReceiptStore>,
    identities: IdentityStore,
    limiter: RateLimiter,
}

impl BridgeState {
    pub async fn new(
        root: PathBuf,
        identities: IdentityStore,
        rate_limit_per_min: u32,
    ) -> Result<Arc<Self>> {
        for rel in [
            ".allternit/ledger/events",
            ".allternit/leases",
            ".allternit/receipts",
            ".allternit/blobs",
            ".allternit/mail/threads",
            ".allternit/work/dags",
        ] {
            std::fs::create_dir_all(root.join(rel))?;
        }
        let ledger = Arc::new(Ledger::new(LedgerOptions {
            root_dir: Some(root.clone()),
            ledger_dir: Some(PathBuf::from(".allternit/ledger")),
        }));
        // The gate needs a lease store handle; the bridge never touches leases.
        let leases = Arc::new(
            Leases::new(LeasesOptions {
                root_dir: Some(root.clone()),
                leases_dir: Some(PathBuf::from(".allternit/leases")),
                event_sink: Some(ledger.clone()),
                actor_id: Some("gate".to_string()),
                auto_renewal_enabled: false,
                auto_renewal_threshold_seconds: 300,
                auto_renewal_interval_seconds: 60,
                auto_renewal_extend_seconds: 600,
            })
            .await?,
        );
        let receipts = Arc::new(ReceiptStore::new(ReceiptStoreOptions {
            root_dir: Some(root.clone()),
            receipts_dir: Some(PathBuf::from(".allternit/receipts")),
            blobs_dir: Some(PathBuf::from(".allternit/blobs")),
        })?);
        let gate = Arc::new(Gate::new(GateOptions {
            ledger: ledger.clone(),
            leases,
            receipts: receipts.clone(),
            index: None,
            vault: None,
            oauth_vault: None,
            root_dir: Some(root.clone()),
            actor_id: Some("gate".to_string()),
            strict_provenance: None,
            visual_provider: None,
            visual_config: None,
        }));
        Ok(Arc::new(Self {
            root,
            ledger,
            gate,
            receipts,
            identities,
            limiter: RateLimiter::new(rate_limit_per_min),
        }))
    }

    async fn audit(&self, actor: Actor, ty: &str, dag_id: Option<String>, payload: Value) {
        let event = AllternitEvent {
            event_id: create_event_id(),
            ts: Utc::now().to_rfc3339(),
            actor,
            scope: dag_id.map(|d| EventScope {
                dag_id: Some(d),
                ..Default::default()
            }),
            r#type: ty.to_string(),
            payload,
            provenance: None,
        };
        if let Err(e) = self.ledger.append(event).await {
            tracing::error!(error = %e, "bridge: ledger audit append failed");
        }
    }
}

/// The authenticated caller, attached to the request by the guard.
#[derive(Clone)]
struct Caller {
    identity: Identity,
    request_id: String,
}

impl Caller {
    fn actor(&self) -> Actor {
        Actor {
            r#type: ActorType::Agent,
            id: self.identity.actor.clone(),
        }
    }
}

/// What a handler reports back to the audit event (response extension).
#[derive(Clone, Default)]
struct AuditInfo {
    dag_id: Option<String>,
    prompt_id: Option<String>,
    mail_thread: Option<String>,
    message_id: Option<String>,
    template_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    extra: Option<(&'static str, Value)>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    fn internal(err: anyhow::Error) -> Self {
        tracing::error!(error = %err, "bridge: internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error (see bridge log)",
        )
    }

    /// Gate 0 refusals are structured and safe to return; template/param
    /// validation errors are user-facing; everything else is internal.
    fn from_plan_error(err: anyhow::Error) -> Self {
        if let Some(gate_err) = GateError::from_anyhow(&err) {
            let mut e = Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "gate_refused",
                gate_err.to_string(),
            );
            e.extra = Some((
                "gate_error",
                serde_json::to_value(gate_err).unwrap_or(Value::Null),
            ));
            return e;
        }
        Self::internal(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.code, "message": self.message });
        if let Some((k, v)) = self.extra {
            body[k] = v;
        }
        (self.status, Json(body)).into_response()
    }
}

fn json_error(status: StatusCode, code: &str, message: &str, extra: Value) -> Response {
    let mut body = json!({ "error": code, "message": message });
    if let (Some(obj), Value::Object(more)) = (body.as_object_mut(), extra) {
        obj.extend(more);
    }
    (status, Json(body)).into_response()
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
        && !s.contains("..")
}

fn with_audit(mut resp: Response, info: AuditInfo) -> Response {
    resp.extensions_mut().insert(info);
    resp
}

// ---------------------------------------------------------------------------
// Guard middleware
// ---------------------------------------------------------------------------

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

async fn guard(State(st): State<Arc<BridgeState>>, mut req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let peer_ip = peer
        .rsplit_once(':')
        .map(|(ip, _)| ip.to_string())
        .unwrap_or(peer.clone());
    let request_id = format!("breq_{}", create_event_id().trim_start_matches("evt_"));

    // 1. Authenticate.
    let identity = match bearer(req.headers()) {
        None => Err("missing bearer token"),
        Some(token) => match st.identities.authenticate(token) {
            Ok(Some(identity)) => Ok(identity),
            Ok(None) => Err("unknown or revoked token"),
            Err(e) => {
                tracing::error!(error = %e, "bridge: identity store unusable");
                return json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "identity_store_unavailable",
                    "identity store unavailable",
                    json!({}),
                );
            }
        },
    };
    let identity = match identity {
        Ok(identity) => identity,
        Err(reason) => {
            // Unauthenticated callers share a per-IP budget so they cannot
            // flood the ledger with denial events.
            if let RateDecision::Allowed = st.limiter.check(&format!("anon:{peer_ip}")) {
                st.audit(
                    Actor {
                        r#type: ActorType::Gate,
                        id: BRIDGE_ACTOR_ID.to_string(),
                    },
                    "BridgeRequestDenied",
                    None,
                    json!({
                        "request_id": request_id,
                        "method": method.as_str(),
                        "path": path,
                        "status": 401,
                        "reason": reason,
                        "peer": peer,
                    }),
                )
                .await;
            }
            let mut resp = json_error(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                reason,
                json!({ "request_id": request_id }),
            );
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer realm=\"allternit-factory-bridge\""),
            );
            return resp;
        }
    };

    let caller = Caller {
        identity,
        request_id: request_id.clone(),
    };
    let base_payload = |status: u16, scope: Option<&str>| {
        json!({
            "request_id": request_id,
            "identity_id": caller.identity.id,
            "method": method.as_str(),
            "path": path,
            "status": status,
            "scope": scope,
            "peer": peer,
        })
    };

    // 2. Rate limit per identity.
    if let RateDecision::Limited { first, retry_after } = st.limiter.check(&caller.identity.id) {
        if first {
            st.audit(caller.actor(), "BridgeRequest", None, {
                let mut p = base_payload(429, None);
                p["reason"] = json!("rate limited (further refusals this window not audited)");
                p
            })
            .await;
        }
        let mut resp = json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "rate limit exceeded",
            json!({ "request_id": request_id, "retry_after_secs": retry_after }),
        );
        if let Ok(v) = header::HeaderValue::from_str(&retry_after.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }

    // 3. Authorize against the route table.
    let (status_scope, denial): (Option<&'static str>, Option<Response>) =
        match classify_route(&method, &path) {
            RouteClass::Open => (None, None),
            RouteClass::Scoped(scope) if caller.identity.has(scope) => (Some(scope.as_str()), None),
            RouteClass::Scoped(scope) => (
                Some(scope.as_str()),
                Some(json_error(
                    StatusCode::FORBIDDEN,
                    "missing_scope",
                    &format!("identity lacks scope {scope}"),
                    json!({ "request_id": request_id, "required_scope": scope.as_str() }),
                )),
            ),
            RouteClass::Forbidden(cap) => (
                Some(cap),
                Some(json_error(
                    StatusCode::FORBIDDEN,
                    "forbidden_capability",
                    &format!("{cap} is never available to remote identities"),
                    json!({ "request_id": request_id, "capability": cap }),
                )),
            ),
            RouteClass::Unknown => (
                None,
                Some(json_error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "not a bridge route",
                    json!({ "request_id": request_id }),
                )),
            ),
        };
    if let Some(resp) = denial {
        st.audit(
            caller.actor(),
            "BridgeRequest",
            None,
            base_payload(resp.status().as_u16(), status_scope),
        )
        .await;
        return resp;
    }

    // 4. Run the handler, then audit with whatever it reported.
    req.extensions_mut().insert(caller.clone());
    let resp = next.run(req).await;
    let info = resp
        .extensions()
        .get::<AuditInfo>()
        .cloned()
        .unwrap_or_default();
    let mut payload = base_payload(resp.status().as_u16(), status_scope);
    if let Some(v) = &info.dag_id {
        payload["target_dag"] = json!(v);
    }
    if let Some(v) = &info.prompt_id {
        payload["prompt_id"] = json!(v);
    }
    if let Some(v) = &info.mail_thread {
        payload["mail_thread"] = json!(v);
    }
    if let Some(v) = &info.message_id {
        payload["message_id"] = json!(v);
    }
    if let Some(v) = &info.template_id {
        payload["template_id"] = json!(v);
    }
    st.audit(
        caller.actor(),
        "BridgeRequest",
        info.dag_id.clone(),
        payload,
    )
    .await;
    resp
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn whoami(Extension(caller): Extension<Caller>) -> Response {
    Json(json!({
        "identity_id": caller.identity.id,
        "actor": caller.identity.actor,
        "scopes": caller.identity.scope_strings(),
        "request_id": caller.request_id,
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanCreateBody {
    text: String,
    #[serde(default)]
    decision_ref: Option<String>,
}

fn origin_for(caller: &Caller, decision_ref: Option<String>) -> Result<PromptOrigin, ApiError> {
    if let Some(d) = &decision_ref {
        if d.len() > 256 {
            return Err(ApiError::bad_request("decision_ref longer than 256 bytes"));
        }
    }
    Ok(PromptOrigin {
        actor: caller.actor(),
        source: "bridge".to_string(),
        decision_ref,
        request_id: Some(caller.request_id.clone()),
    })
}

async fn plan_create(
    State(st): State<Arc<BridgeState>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<PlanCreateBody>,
) -> Result<Response, ApiError> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(ApiError::bad_request("text is required"));
    }
    if text.len() > MAX_PLAN_TEXT {
        return Err(ApiError::bad_request(format!(
            "text longer than {MAX_PLAN_TEXT} bytes"
        )));
    }
    let origin = origin_for(&caller, body.decision_ref)?;
    let (prompt_id, dag_id, node_id) = st
        .gate
        .plan_new_with_origin(text, None, Some(&origin))
        .await
        .map_err(ApiError::from_plan_error)?;
    let resp = (
        StatusCode::CREATED,
        Json(json!({
            "prompt_id": prompt_id,
            "dag_id": dag_id,
            "node_id": node_id,
            "submitted_by": caller.identity.actor,
        })),
    )
        .into_response();
    Ok(with_audit(
        resp,
        AuditInfo {
            dag_id: Some(dag_id),
            prompt_id: Some(prompt_id),
            ..Default::default()
        },
    ))
}

async fn templates_list(State(st): State<Arc<BridgeState>>) -> Result<Response, ApiError> {
    let store = TemplateStore::new(&st.root).map_err(ApiError::internal)?;
    let templates = store.list().map_err(ApiError::internal)?;
    let out: Vec<Value> = templates
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "description": t.description,
                "params": t.params,
                "steps": t.steps.len(),
            })
        })
        .collect();
    Ok(Json(json!({ "templates": out })).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstantiateBody {
    #[serde(default)]
    params: HashMap<String, String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    decision_ref: Option<String>,
}

async fn template_instantiate(
    State(st): State<Arc<BridgeState>>,
    Extension(caller): Extension<Caller>,
    Path(template_id): Path<String>,
    Json(body): Json<InstantiateBody>,
) -> Result<Response, ApiError> {
    if !valid_id(&template_id) {
        return Err(ApiError::bad_request("invalid template id"));
    }
    if body.text.as_deref().map(str::len).unwrap_or(0) > MAX_PLAN_TEXT {
        return Err(ApiError::bad_request("text too long"));
    }
    let store = TemplateStore::new(&st.root).map_err(ApiError::internal)?;
    // `get` (store id only) — never `resolve`, which would accept a file path.
    let template = store
        .get(&template_id)
        .map_err(|e| ApiError::bad_request(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("template {template_id} not found")))?;
    // Validate params/steps up front so user errors are 422, not 500.
    template.expand_dag("__root__", &body.params).map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_template_params",
            e.to_string(),
        )
    })?;
    let origin = origin_for(&caller, body.decision_ref)?;
    let result = plan_from_template_with_origin(
        &st.gate,
        &template,
        &body.params,
        body.text.as_deref(),
        None,
        Some(&origin),
    )
    .await
    .map_err(ApiError::from_plan_error)?;
    let info = AuditInfo {
        dag_id: Some(result.dag_id.clone()),
        prompt_id: Some(result.prompt_id.clone()),
        template_id: Some(result.template_id.clone()),
        ..Default::default()
    };
    let resp = (
        StatusCode::CREATED,
        Json(json!({
            "template_id": result.template_id,
            "prompt_id": result.prompt_id,
            "dag_id": result.dag_id,
            "root_node_id": result.root_node_id,
            "delta_id": result.delta_id,
            "nodes": result.nodes,
            "params": result.params,
            "submitted_by": caller.identity.actor,
        })),
    )
        .into_response();
    Ok(with_audit(resp, info))
}

async fn load_dag(
    st: &BridgeState,
    dag_id: &str,
) -> Result<crate::work::types::DagState, ApiError> {
    if !valid_id(dag_id) {
        return Err(ApiError::bad_request("invalid dag id"));
    }
    let events = st
        .ledger
        .query(LedgerQuery::default())
        .await
        .map_err(ApiError::internal)?;
    let dag_events: Vec<_> = events
        .into_iter()
        .filter(|e| e.payload.get("dag_id").and_then(|v| v.as_str()) == Some(dag_id))
        .collect();
    let dag = project_dag(&dag_events, dag_id);
    if dag.nodes.is_empty() {
        return Err(ApiError::not_found(format!("dag {dag_id} not found")));
    }
    Ok(dag)
}

async fn plan_show(
    State(st): State<Arc<BridgeState>>,
    Path(dag_id): Path<String>,
) -> Result<Response, ApiError> {
    let dag = load_dag(&st, &dag_id).await?;
    let dag_json = serde_json::to_value(&dag).map_err(|e| ApiError::internal(e.into()))?;
    Ok(with_audit(
        Json(json!({ "dag_id": dag_id, "dag": dag_json })).into_response(),
        AuditInfo {
            dag_id: Some(dag_id),
            ..Default::default()
        },
    ))
}

async fn node_output(
    State(st): State<Arc<BridgeState>>,
    Path((dag_id, node_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !valid_id(&node_id) {
        return Err(ApiError::bad_request("invalid node id"));
    }
    let dag = load_dag(&st, &dag_id).await?;
    let node = dag
        .nodes
        .get(&node_id)
        .ok_or_else(|| ApiError::not_found(format!("node {node_id} not in {dag_id}")))?;
    let output = node
        .output
        .as_ref()
        .ok_or_else(|| ApiError::not_found(format!("node {node_id} has no recorded output")))?;
    if !valid_id(&output.blob_id) {
        return Err(ApiError::internal(anyhow::anyhow!(
            "ledger blob id {:?} is malformed",
            output.blob_id
        )));
    }
    let bytes = std::fs::read(st.receipts.blob_path(&output.blob_id))
        .with_context(|| format!("read blob {}", output.blob_id))
        .map_err(ApiError::internal)?;
    let mut resp = (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        bytes,
    )
        .into_response();
    if let Ok(v) = header::HeaderValue::from_str(&output.sha256) {
        resp.headers_mut().insert("x-allternit-factory-sha256", v);
    }
    Ok(with_audit(
        resp,
        AuditInfo {
            dag_id: Some(dag_id),
            ..Default::default()
        },
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailSendBody {
    thread_id: String,
    body: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    importance: Option<MailImportance>,
}

async fn mail_send(
    State(st): State<Arc<BridgeState>>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<MailSendBody>,
) -> Result<Response, ApiError> {
    let thread_id =
        canonical_thread_id(&body.thread_id).map_err(|e| ApiError::bad_request(e.to_string()))?;
    if body.body.trim().is_empty() {
        return Err(ApiError::bad_request("body is required"));
    }
    if body.body.len() > MAX_MAIL_BODY {
        return Err(ApiError::bad_request(format!(
            "body longer than {MAX_MAIL_BODY} bytes"
        )));
    }
    if body.subject.as_deref().map(str::len).unwrap_or(0) > MAX_SUBJECT {
        return Err(ApiError::bad_request("subject too long"));
    }
    if body.to.len() > 32 || body.to.iter().any(|t| t.len() > 128) {
        return Err(ApiError::bad_request("too many or too long recipients"));
    }
    // A per-request Mail whose actor IS the caller: MessageSent is attributed
    // to bot:<slug>, and from_agent cannot be spoofed by the body.
    let mail = Mail::new(MailOptions {
        root_dir: Some(st.root.clone()),
        ledger: st.ledger.clone(),
        actor_id: Some(caller.identity.actor.clone()),
        actor_type: Some(ActorType::Agent),
        mail_index: None,
    });
    let message_id = mail
        .send_typed_message(
            &thread_id,
            TypedMessage {
                from_agent: caller.identity.actor.clone(),
                to_agents: body.to,
                subject: body.subject,
                importance: body.importance.unwrap_or_default(),
                ack_required: false,
                body: body.body,
            },
        )
        .await
        .map_err(ApiError::internal)?;
    let dag_id = thread_id.strip_prefix("dag:").map(str::to_string);
    Ok(with_audit(
        Json(json!({ "sent": true, "message_id": message_id, "thread_id": thread_id }))
            .into_response(),
        AuditInfo {
            dag_id,
            mail_thread: Some(thread_id),
            message_id: Some(message_id),
            ..Default::default()
        },
    ))
}

#[derive(Deserialize)]
struct InboxQuery {
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn mail_inbox(
    State(st): State<Arc<BridgeState>>,
    Query(q): Query<InboxQuery>,
) -> Result<Response, ApiError> {
    let thread = match q.thread_id.as_deref() {
        Some(t) => Some(canonical_thread_id(t).map_err(|e| ApiError::bad_request(e.to_string()))?),
        None => None,
    };
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_INBOX_LIMIT);
    let mail = Mail::new(MailOptions {
        root_dir: Some(st.root.clone()),
        ledger: st.ledger.clone(),
        actor_id: Some(BRIDGE_ACTOR_ID.to_string()),
        actor_type: Some(ActorType::Gate),
        mail_index: None,
    });
    let events = mail
        .list_messages(thread.as_deref(), usize::MAX)
        .await
        .map_err(ApiError::internal)?;
    let start = events.len().saturating_sub(limit);
    let messages: Vec<Value> = events[start..]
        .iter()
        .filter(|e| e.r#type == "MessageSent")
        .map(|e| {
            let p = &e.payload;
            let body = p
                .get("body_path")
                .and_then(|v| v.as_str())
                .filter(|bp| bp.starts_with(".allternit/mail/messages/") && !bp.contains(".."))
                .and_then(|bp| std::fs::read(st.root.join(bp)).ok())
                .map(|b| {
                    let cut = b.len().min(MAX_MAIL_BODY);
                    String::from_utf8_lossy(&b[..cut]).into_owned()
                });
            json!({
                "message_id": e.event_id,
                "ts": e.ts,
                "actor": e.actor.id,
                "thread_id": p.get("thread_id"),
                "from_agent": p.get("from_agent"),
                "to_agents": p.get("to_agents"),
                "subject": p.get("subject"),
                "importance": p.get("importance"),
                "body": body,
                "body_ref": p.get("body_ref"),
            })
        })
        .collect();
    Ok(with_audit(
        Json(json!({ "messages": messages })).into_response(),
        AuditInfo {
            mail_thread: thread,
            ..Default::default()
        },
    ))
}

async fn fallback() -> Response {
    json_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "not a bridge route",
        json!({}),
    )
}

/// Build the bridge router (guard + scoped routes only).
pub fn router(state: Arc<BridgeState>) -> Router {
    Router::new()
        .route("/v1/whoami", get(whoami))
        .route("/v1/plan", post(plan_create))
        .route("/v1/plan/:dag_id", get(plan_show))
        .route("/v1/plan/:dag_id/nodes/:node_id/output", get(node_output))
        .route("/v1/templates", get(templates_list))
        .route(
            "/v1/templates/:template_id/instantiate",
            post(template_instantiate),
        )
        .route("/v1/mail/send", post(mail_send))
        .route("/v1/mail/inbox", get(mail_inbox))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Serve on an already-bound listener (tests bind 127.0.0.1:0).
pub async fn serve_listener(
    listener: tokio::net::TcpListener,
    state: Arc<BridgeState>,
) -> Result<()> {
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// `bridge serve`: check the bind, build state, listen.
pub async fn serve(cfg: BridgeConfig) -> Result<()> {
    let store = IdentityStore::new(cfg.identities_path.clone());
    check_bind(&cfg, &store)?;
    let root = std::fs::canonicalize(&cfg.root)
        .with_context(|| format!("workspace root {} does not exist", cfg.root.display()))?;
    let state = BridgeState::new(root.clone(), store, cfg.rate_limit_per_min).await?;
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;
    eprintln!(
        "factory bridge listening on {} (root {}, identities {}, {} req/min per identity)",
        cfg.bind,
        root.display(),
        cfg.identities_path.display(),
        cfg.rate_limit_per_min
    );
    serve_listener(listener, state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_table() {
        use RouteClass::*;
        assert_eq!(
            classify_route(&Method::POST, "/v1/plan"),
            Scoped(Scope::PlanCreate)
        );
        assert_eq!(
            classify_route(&Method::GET, "/v1/plan/dag_1"),
            Scoped(Scope::PlanRead)
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/wihs/pickup"),
            Forbidden("wih:pickup")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/wihs/w1/close"),
            Forbidden("wih:close")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/leases"),
            Forbidden("lease:*")
        );
        assert_eq!(
            classify_route(&Method::DELETE, "/v1/leases/l1"),
            Forbidden("lease:*")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/wait-gates/resolve"),
            Forbidden("wait-gate:resolve")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/gate/decision"),
            Forbidden("gate:*")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/mail/decide"),
            Forbidden("gate:*")
        );
        assert_eq!(
            classify_route(&Method::POST, "/v1/plan/refine"),
            Forbidden("plan:refine")
        );
        assert_eq!(classify_route(&Method::GET, "/v1/ledger/tail"), Unknown);
        assert_eq!(classify_route(&Method::DELETE, "/v1/plan/dag_1"), Unknown);
    }

    #[test]
    fn bind_policy() {
        let dir = tempfile::tempdir().unwrap();
        let store = IdentityStore::new(dir.path().join("ids.json"));
        let mut cfg = BridgeConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            root: dir.path().to_path_buf(),
            identities_path: store.path().to_path_buf(),
            allow_remote: false,
            rate_limit_per_min: 60,
        };
        assert!(check_bind(&cfg, &store).is_ok());
        cfg.bind = "100.64.0.9:7433".parse().unwrap();
        assert!(check_bind(&cfg, &store).is_err(), "remote without flag");
        cfg.allow_remote = true;
        assert!(
            check_bind(&cfg, &store).is_err(),
            "remote with flag but no identity"
        );
        store.add("bot:chief", &[Scope::PlanRead], None).unwrap();
        assert!(check_bind(&cfg, &store).is_ok());
        cfg.bind = "0.0.0.0:7433".parse().unwrap();
        assert!(
            check_bind(&cfg, &store).is_err(),
            "unspecified is always refused"
        );
    }
}
