//! MCP edge: artifact apps exported as MCP Apps (plan v3 lane 46, D19).
//!
//! An artifact app whose owner turned export on opens inside outside AI
//! clients (Claude, ChatGPT) through `mcp.allternit.com`, per MCP Apps
//! (SEP-1865, ext-apps 2026-01-26):
//!
//! * `resources/list` adds one `ui://allternit/apps/<slug>` per exported app
//!   the caller can open; `resources/read` answers it here
//!   (mimeType `text/html;profile=mcp-app`).
//! * `tools/list` adds one `open_<slug>` tool per app (`_meta.ui.resourceUri`)
//!   and, when at least one app may use them, the mirrored bridge tools
//!   `app.storage.get|set|delete|list` and `app.ai.complete` (hidden from the
//!   model: `_meta.ui.visibility = ["app"]`). They let an exported app work in
//!   a host without the `x-allternit` extension. A mirrored tool returns the
//!   runtime result in `structuredContent`, or `isError` with `{code}`.
//!
//! **Export switch.** Migration 161 (#2921) has no `export_mcp` column, so the
//! switch is `app_capability_snapshot.export_mcp = true` (no migration here).
//! `capability_set` ignores the key, so it never counts as a capability.
//!
//! **Who sees what.** Only OAuth callers on the agents server (not project or
//! CLI keys). Listed: the caller's own apps, their org's apps (install scope
//! `org`), and link-shared `browser` apps they installed (D14).
//!
//! **Gate.** Same rule as `artifact_runtime::app_gate` (#2928): the app runs
//! at `min(app tier, cap)`, the method must be declared in the snapshot, and
//! anyone but the owner needs an `app_installs` grant that covers the app
//! (growth-only consent, D24). Stricter at the edge: only the owner and their
//! org get above `browser`; a link-installed app (the outsider) stays at
//! `browser`, so it is view-only here. Vendor apps are not exported (they live
//! on their own server). This base (#2919) does not have #2928, so the rule is
//! restated in [`mirror_gate`]; the train can point it at #2928's helpers.
//!
//! **What the host frame loads.** For an HTML app (`text/html`, the page
//! kind) the resource is the app's pinned HTML itself, with an empty CSP
//! (no connect, resource or frame domains). That is the safe option: the
//! host's sandbox and CSP govern every line of the app, it reaches no network,
//! and its only way to data is the gated mirror tools. A thin document that
//! frames `https://<slug>.<APP_DOMAIN>` would run the app under its own CSP,
//! outside the host's, so it is used only for non-HTML apps and only when
//! `APP_DOMAIN` is set; never the first-party `ai.allternit.com` (session
//! cookies). Without either, the app is not exported.
//!
//! Import (an outside MCP App opening here) is client-side (AppScreen), not
//! in this file.

use serde_json::{json, Value};

pub const APP_URI_PREFIX: &str = "ui://allternit/apps/";
pub const MCP_APP_MIME: &str = "text/html;profile=mcp-app";
pub const OPEN_TOOL_PREFIX: &str = "open_";
pub const STORAGE_GET: &str = "app.storage.get";
pub const STORAGE_SET: &str = "app.storage.set";
pub const STORAGE_DELETE: &str = "app.storage.delete";
pub const STORAGE_LIST: &str = "app.storage.list";
pub const AI_COMPLETE: &str = "app.ai.complete";
pub const MIRROR_TOOLS: [&str; 5] = [STORAGE_GET, STORAGE_SET, STORAGE_DELETE, STORAGE_LIST, AI_COMPLETE];
/// JSON-RPC internal error (not in `mcp_protocol::codes`).
const INTERNAL_ERROR: i64 = -32603;
/// At most this many apps are exported to one caller.
pub const MAX_EXPORTED_APPS: i64 = 50;

/// Permission tier (same order as `artifact_runtime::Tier`, #2928).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Browser,
    App,
    Full,
}

impl Tier {
    pub fn parse(raw: &str) -> Option<Tier> {
        match raw {
            "browser" => Some(Tier::Browser),
            "app" => Some(Tier::App),
            "full" => Some(Tier::Full),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Browser => "browser",
            Tier::App => "app",
            Tier::Full => "full",
        }
    }
}

/// How the caller reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Owner,
    /// Same org, install scope `org`.
    Org,
    /// Installed from someone else's link (browser tier only, D14).
    Installed,
}

/// One exported artifact app, as the caller sees it.
#[derive(Debug, Clone)]
pub struct ExportedApp {
    pub artifact_id: String,
    pub slug: String,
    pub name: String,
    pub tier: Tier,
    pub snapshot: Value,
    /// Body format of the pinned version (`text/html` for pages).
    pub body_format: String,
    pub relation: Relation,
    /// The caller's `app_installs` grant: `(granted_tier, granted_capabilities)`.
    pub grant: Option<(Tier, Value)>,
}

/// A refused or failed mirror call: `isError` with `{code}`.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorError {
    pub code: String,
    pub message: String,
    pub extra: Value,
}

impl MirrorError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into(), extra: json!({}) }
    }
    fn with(mut self, key: &str, value: Value) -> Self {
        self.extra[key] = value;
        self
    }
}

/// A storage call of a mirrored tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageOp {
    Get { scope: String, key: String },
    Set { scope: String, key: String, value: String },
    Delete { scope: String, key: String },
    List { scope: String, prefix: String },
}

/// Where the edge reads exported apps and runs mirrored calls.
#[async_trait::async_trait]
pub trait AppsStore: Send + Sync {
    /// Exported apps `user_id` can open (own, org, link-installed browser apps).
    async fn exported_apps(&self, user_id: &str) -> Result<Vec<ExportedApp>, String>;
    /// The pinned version's body (HTML for `text/html` apps).
    async fn app_body(&self, app: &ExportedApp) -> Result<Option<String>, String>;
    /// One storage call on the app, as `user_id` (after [`mirror_gate`]).
    async fn storage(&self, app: &ExportedApp, user_id: &str, op: StorageOp) -> Result<Value, MirrorError>;
    /// One AI completion billed to `user_id` (after [`mirror_gate`]).
    async fn ai_complete(&self, app: &ExportedApp, user_id: &str, args: &Value) -> Result<Value, MirrorError>;
    /// The hosted app domain (`APP_DOMAIN`), if set.
    fn app_domain(&self) -> Option<String> {
        std::env::var("APP_DOMAIN").ok().map(|d| d.trim().trim_matches('.').to_string()).filter(|d| !d.is_empty())
    }
}

/// Apps are exported only on the agents server, to OAuth callers (not
/// `platform-key:` / `cli-key:` callers, which are not a person in a client).
pub fn serves(agents_target: bool, client: &str) -> bool {
    agents_target && !client.starts_with("platform-key:") && !client.starts_with("cli-key:")
}

pub fn resource_uri(slug: &str) -> String {
    format!("{APP_URI_PREFIX}{slug}")
}

fn open_tool_name(slug: &str) -> String {
    format!("{OPEN_TOOL_PREFIX}{slug}")
}

fn is_html(format: &str) -> bool {
    format.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("text/html")
}

/// The app can be shown in a host frame (see the module doc).
fn exportable(app: &ExportedApp, domain: Option<&str>) -> bool {
    is_html(&app.body_format) || domain.is_some()
}

// ─── gate ─────────────────────────────────────────────────────────────────────

/// Capability names a snapshot or grant names (#2928 `capability_set`).
fn capability_set(snapshot: &Value) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for key in ["storage", "ai", "tasks", "events", "skill"] {
        if snapshot.get(key).and_then(Value::as_bool).unwrap_or(false) {
            out.insert(key.to_string());
        }
    }
    for entry in snapshot.get("connectors").and_then(Value::as_array).into_iter().flatten() {
        let Some(connector) = entry.get("connector").and_then(Value::as_str) else { continue };
        let tools: Vec<&str> = entry.get("tools").and_then(Value::as_array).map(|t| t.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        if tools.is_empty() {
            out.insert(format!("connector:{connector}/*"));
        }
        for tool in tools {
            out.insert(format!("connector:{connector}/{tool}"));
        }
    }
    out
}

fn capability_tier(name: &str) -> Tier {
    match name {
        "storage" | "ai" => Tier::App,
        _ => Tier::Full,
    }
}

/// The tier the app runs at for this caller at the edge.
pub fn effective_tier(app: &ExportedApp) -> Tier {
    let cap = match app.relation {
        Relation::Owner | Relation::Org => Tier::Full,
        Relation::Installed => Tier::Browser,
    };
    app.tier.min(cap)
}

/// `app_gate` (#2928) for a mirrored method (`storage` or `ai`, both `app` tier).
pub fn mirror_gate(app: &ExportedApp, capability: &str) -> Result<(), MirrorError> {
    let effective = effective_tier(app);
    let required = capability_tier(capability);
    if effective < required {
        return Err(MirrorError::new("tier_too_low", format!("This app runs at {} level here; that needs {} level", effective.as_str(), required.as_str()))
            .with("tier", json!(effective.as_str()))
            .with("required_tier", json!(required.as_str())));
    }
    let declared = capability_set(&app.snapshot);
    if !declared.contains(capability) {
        return Err(MirrorError::new("capability_not_declared", "The app's published version doesn't use this"));
    }
    if app.relation == Relation::Owner {
        return Ok(());
    }
    let wanted: Vec<String> = declared.into_iter().filter(|c| capability_tier(c) <= effective).collect();
    let (granted_tier, granted) = match &app.grant {
        Some((tier, caps)) => (Some(*tier), capability_set(caps)),
        None => (None, Default::default()),
    };
    let added: Vec<String> = wanted.into_iter().filter(|c| !granted.contains(c)).collect();
    if granted_tier.map_or(true, |g| g < effective) || !added.is_empty() {
        return Err(MirrorError::new("consent_required", "Open the app in Allternit once to approve this version")
            .with("requested_tier", json!(effective.as_str()))
            .with("added_capabilities", json!(added)));
    }
    Ok(())
}

/// The app may use at least one mirrored tool.
fn uses_mirror(app: &ExportedApp) -> bool {
    mirror_gate(app, "storage").is_ok() || mirror_gate(app, "ai").is_ok()
}

// ─── listings ─────────────────────────────────────────────────────────────────

fn resource_entry(app: &ExportedApp) -> Value {
    json!({ "uri": resource_uri(&app.slug), "name": app.name, "mimeType": MCP_APP_MIME })
}

fn open_tool(app: &ExportedApp) -> Value {
    json!({
        "name": open_tool_name(&app.slug),
        "title": app.name,
        "description": format!("Open the Allternit app \"{}\".", app.name),
        "inputSchema": { "type": "object", "properties": {} },
        "annotations": { "readOnlyHint": true },
        "_meta": { "ui": { "resourceUri": resource_uri(&app.slug) } },
    })
}

fn mirror_tools() -> Vec<Value> {
    let app = json!({ "type": "string", "description": "The app's slug" });
    let scope = json!({ "type": "string", "enum": ["user", "shared"], "default": "user" });
    let key = json!({ "type": "string" });
    let tool = |name: &str, description: &str, properties: Value, required: Vec<&str>| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": properties, "required": required },
            "_meta": { "ui": { "visibility": ["app"] } },
        })
    };
    vec![
        tool(STORAGE_GET, "Read one stored value of an Allternit app.", json!({ "app": app, "scope": scope, "key": key }), vec!["app", "key"]),
        tool(STORAGE_SET, "Store one value for an Allternit app.", json!({ "app": app, "scope": scope, "key": key, "value": { "type": "string" } }), vec!["app", "key", "value"]),
        tool(STORAGE_DELETE, "Delete one stored value of an Allternit app.", json!({ "app": app, "scope": scope, "key": key }), vec!["app", "key"]),
        tool(STORAGE_LIST, "List stored keys of an Allternit app.", json!({ "app": app, "scope": scope, "prefix": { "type": "string" } }), vec!["app"]),
        tool(AI_COMPLETE, "One AI completion for an Allternit app, billed to you.", json!({ "app": app, "prompt": { "type": "string" }, "system": { "type": "string" }, "messages": { "type": "array" }, "max_tokens": { "type": "integer" }, "temperature": { "type": "number" } }), vec!["app"]),
    ]
}

/// The apps `user_id` sees, filtered to the ones a host frame can show.
/// A lookup failure lists nothing (logged).
async fn visible_apps(store: &dyn AppsStore, user_id: &str) -> Vec<ExportedApp> {
    let domain = store.app_domain();
    match store.exported_apps(user_id).await {
        Ok(apps) => apps.into_iter().filter(|a| exportable(a, domain.as_deref())).collect(),
        Err(error) => {
            tracing::warn!("mcp edge apps: lookup failed: {error}");
            Vec::new()
        }
    }
}

/// Adds the exported apps to a relayed `tools/list` or `resources/list` reply
/// (status 200). A `resources/list` the runtime doesn't implement (JSON-RPC
/// error) becomes a list of just the apps. Anything else passes unchanged.
pub async fn with_app_entries(store: &dyn AppsStore, user_id: &str, rpc_method: &str, reply: Vec<u8>) -> Vec<u8> {
    let (key, is_tools) = match rpc_method {
        "tools/list" => ("tools", true),
        "resources/list" => ("resources", false),
        _ => return reply,
    };
    let Ok(mut doc) = serde_json::from_slice::<Value>(&reply) else { return reply };
    let apps = visible_apps(store, user_id).await;
    if apps.is_empty() {
        return reply;
    }
    let mut extra: Vec<Value> = if is_tools { apps.iter().map(open_tool).collect() } else { apps.iter().map(resource_entry).collect() };
    if is_tools && apps.iter().any(uses_mirror) {
        extra.extend(mirror_tools());
    }
    if doc.get("error").is_some() && !is_tools {
        doc = json!({ "jsonrpc": "2.0", "id": doc["id"].clone(), "result": { "resources": [] } });
    }
    let Some(list) = doc.get_mut("result").and_then(|r| r.get_mut(key)).and_then(Value::as_array_mut) else { return reply };
    // The runtime can't shadow the edge's own entries.
    list.retain(|t| {
        if is_tools {
            !t["name"].as_str().is_some_and(|n| MIRROR_TOOLS.contains(&n) || n.starts_with(OPEN_TOOL_PREFIX))
        } else {
            !t["uri"].as_str().is_some_and(|u| u.starts_with(APP_URI_PREFIX))
        }
    });
    list.extend(extra);
    serde_json::to_vec(&doc).unwrap_or(reply)
}

/// An `initialize` reply that also advertises `resources` (so hosts read the
/// `ui://` apps even when the runtime has none of its own).
pub fn advertise_resources(reply: Vec<u8>) -> Vec<u8> {
    let Ok(mut doc) = serde_json::from_slice::<Value>(&reply) else { return reply };
    let Some(caps) = doc.get_mut("result").and_then(|r| r.get_mut("capabilities")).and_then(Value::as_object_mut) else { return reply };
    if caps.contains_key("resources") {
        return reply;
    }
    caps.insert("resources".into(), json!({}));
    serde_json::to_vec(&doc).unwrap_or(reply)
}

// ─── resources/read ──────────────────────────────────────────────────────────

fn esc_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The thin document for a non-HTML app: the hosted app in a sandboxed frame
/// (no `allow-same-origin`, D12).
fn hosted_shell(app: &ExportedApp, domain: &str) -> String {
    let src = format!("https://{}.{}/", app.slug, domain);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{name}</title>\
<style>html,body{{margin:0;height:100%}}iframe{{border:0;width:100%;height:100%;min-height:480px}}</style></head>\
<body><iframe src=\"{src}\" title=\"{name}\" sandbox=\"allow-scripts allow-forms allow-popups\" referrerpolicy=\"no-referrer\"></iframe></body></html>",
        name = esc_attr(&app.name),
        src = esc_attr(&src),
    )
}

async fn read_app(store: &dyn AppsStore, user_id: &str, id: &Value, uri: &str) -> Value {
    let slug = &uri[APP_URI_PREFIX.len()..];
    let not_found = || mcp_protocol::rpc_err(id, mcp_protocol::codes::INVALID_PARAMS, format!("Resource not found: {uri}"));
    let domain = store.app_domain();
    let Some(app) = visible_apps(store, user_id).await.into_iter().find(|a| a.slug == slug) else { return not_found() };
    let (text, frame_domains) = if is_html(&app.body_format) {
        match store.app_body(&app).await {
            Ok(Some(html)) => (html, Vec::<String>::new()),
            Ok(None) => return not_found(),
            Err(error) => {
                tracing::warn!("mcp edge apps: body read failed: {error}");
                return mcp_protocol::rpc_err(id, INTERNAL_ERROR, "The app couldn't be loaded");
            }
        }
    } else {
        let Some(domain) = domain.as_deref() else { return not_found() };
        (hosted_shell(&app, domain), vec![format!("https://{}.{}", app.slug, domain)])
    };
    mcp_protocol::rpc_ok(
        id,
        json!({ "contents": [{
            "uri": uri,
            "mimeType": MCP_APP_MIME,
            "text": text,
            "_meta": { "ui": {
                "csp": { "connectDomains": [], "resourceDomains": [], "frameDomains": frame_domains },
                "prefersBorder": true,
            } },
        }] }),
    )
}

/// Called by `mcp_edge::serve` for `resources/list` and `resources/read`.
/// `Some(reply)` is answered here (a read of an app's `ui://` URI); `None`
/// relays (`resources/list` is relayed, then [`with_app_entries`] adds apps).
pub async fn intercept_resources(store: &dyn AppsStore, user_id: &str, rpc_method: &str, id: &Value, params: &Value) -> Option<Value> {
    if rpc_method != "resources/read" {
        return None;
    }
    let uri = params["uri"].as_str().filter(|u| u.starts_with(APP_URI_PREFIX))?;
    Some(read_app(store, user_id, id, uri).await)
}

// ─── tools/call ──────────────────────────────────────────────────────────────

fn tool_error(e: MirrorError) -> Value {
    let mut data = e.extra.clone();
    data["code"] = json!(e.code);
    json!({ "isError": true, "content": [{ "type": "text", "text": e.message }], "structuredContent": data })
}

fn tool_ok(result: Value, text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": result })
}

fn storage_op(tool: &str, args: &Value) -> Result<StorageOp, MirrorError> {
    let scope = args["scope"].as_str().unwrap_or("user").to_string();
    if scope != "user" && scope != "shared" {
        return Err(MirrorError::new("invalid_scope", "scope is user or shared"));
    }
    let key = || args["key"].as_str().map(str::to_string).ok_or_else(|| MirrorError::new("invalid_key", "key is required"));
    Ok(match tool {
        STORAGE_GET => StorageOp::Get { scope, key: key()? },
        STORAGE_DELETE => StorageOp::Delete { scope, key: key()? },
        STORAGE_LIST => StorageOp::List { scope, prefix: args["prefix"].as_str().unwrap_or_default().to_string() },
        _ => {
            let value = args["value"].as_str().map(str::to_string).ok_or_else(|| MirrorError::new("invalid_value", "value must be a string"))?;
            StorageOp::Set { scope, key: key()?, value }
        }
    })
}

async fn call_mirror(store: &dyn AppsStore, user_id: &str, tool: &str, args: &Value) -> Result<Value, MirrorError> {
    let slug = args["app"].as_str().ok_or_else(|| MirrorError::new("app_required", "app (the app's slug) is required"))?;
    let app = visible_apps(store, user_id).await.into_iter().find(|a| a.slug == slug).ok_or_else(|| MirrorError::new("not_found", "No such app"))?;
    if tool == AI_COMPLETE {
        mirror_gate(&app, "ai")?;
        return store.ai_complete(&app, user_id, args).await;
    }
    // Shared writes need no separate consent: the install grant is the
    // consent (#2928), and `mirror_gate` checked it.
    mirror_gate(&app, "storage")?;
    let op = storage_op(tool, args)?;
    store.storage(&app, user_id, op).await
}

/// Called by `mcp_edge::serve` for `tools/call` before the Tasks hook.
/// `Some(reply)` for `open_<slug>` and the mirrored `app.*` tools; `None` relays.
pub async fn intercept_tool_call(store: &dyn AppsStore, user_id: &str, rpc_method: &str, id: &Value, params: &Value) -> Option<Value> {
    if rpc_method != "tools/call" {
        return None;
    }
    let tool = params["name"].as_str()?;
    let args = &params["arguments"];
    if MIRROR_TOOLS.contains(&tool) {
        let result = match call_mirror(store, user_id, tool, args).await {
            Ok(v) => tool_ok(v.clone(), v.to_string()),
            Err(e) => tool_error(e),
        };
        return Some(mcp_protocol::rpc_ok(id, result));
    }
    let slug = tool.strip_prefix(OPEN_TOOL_PREFIX)?;
    let app = visible_apps(store, user_id).await.into_iter().find(|a| a.slug == slug)?;
    let mut result = tool_ok(json!({ "app": { "slug": app.slug, "name": app.name, "tier": effective_tier(&app).as_str() } }), format!("Opened {}.", app.name));
    result["_meta"] = json!({ "ui": { "resourceUri": resource_uri(&app.slug) } });
    Some(mcp_protocol::rpc_ok(id, result))
}

// ─── Postgres store ──────────────────────────────────────────────────────────

/// Reads migration 161 columns (#2921); storage and AI use the page runtime's
/// tables and billing (`artifact_runtime`).
pub struct PgAppsStore<'a> {
    pub state: &'a std::sync::Arc<crate::ApiState>,
}

fn ai_calls() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

fn ai_rate_ok(user_id: &str, artifact_id: &str) -> bool {
    let mut map = ai_calls().lock().unwrap_or_else(|e| e.into_inner());
    let q = map.entry(format!("{user_id}:{artifact_id}")).or_default();
    let now = std::time::Instant::now();
    while q.front().is_some_and(|t| now.duration_since(*t) > std::time::Duration::from_secs(60)) {
        q.pop_front();
    }
    if q.len() >= crate::artifacts::runtime::AI_CALLS_PER_MINUTE {
        return false;
    }
    q.push_back(now);
    true
}

fn db_error(e: sqlx::Error) -> MirrorError {
    tracing::warn!("mcp edge apps: storage failed: {e}");
    MirrorError::new("unavailable", "Storage isn't available right now")
}

type AppRow = (String, String, String, String, String, String, Option<Value>, Option<String>, Option<String>, Option<Value>);

#[async_trait::async_trait]
impl AppsStore for PgAppsStore<'_> {
    async fn exported_apps(&self, user_id: &str) -> Result<Vec<ExportedApp>, String> {
        // Org lookup failing lists only own + installed apps (fail closed).
        let orgs: Vec<String> = crate::services::user_trust::clerk_org_memberships_checked(user_id).await.map(|m| m.into_iter().map(|o| o.id).collect()).unwrap_or_default();
        let rows = sqlx::query_as::<_, AppRow>(
            "SELECT a.id, a.app_slug, a.app_name, a.owner_id, COALESCE(a.org_id, ''), a.app_permission_tier, \
                    a.app_capability_snapshot, v.body_format, i.granted_tier, i.granted_capabilities \
             FROM artifacts a \
             JOIN artifact_versions v ON v.artifact_id = a.id AND v.version = a.app_pinned_version \
             LEFT JOIN app_installs i ON i.user_id = $1 AND i.app_kind = 'artifact' AND i.app_ref = a.id \
             WHERE a.app_slug IS NOT NULL AND a.app_name IS NOT NULL AND a.app_permission_tier IS NOT NULL \
               AND a.app_capability_snapshot @> '{\"export_mcp\": true}'::jsonb \
               AND (a.owner_id = $1 \
                    OR (a.org_id = ANY($2) AND a.app_install_scope = 'org') \
                    OR (i.id IS NOT NULL AND a.app_install_scope = 'link' AND a.app_permission_tier = 'browser')) \
             ORDER BY a.updated_at DESC LIMIT $3",
        )
        .bind(user_id)
        .bind(&orgs)
        .bind(MAX_EXPORTED_APPS)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, slug, name, owner, org, tier, snapshot, format, granted_tier, granted_caps)| {
                let relation = if owner == user_id {
                    Relation::Owner
                } else if !org.is_empty() && orgs.contains(&org) {
                    Relation::Org
                } else {
                    Relation::Installed
                };
                let grant = granted_tier.as_deref().and_then(Tier::parse).map(|t| (t, granted_caps.unwrap_or_else(|| json!({}))));
                Some(ExportedApp { artifact_id: id, slug, name, tier: Tier::parse(&tier)?, snapshot: snapshot.unwrap_or_else(|| json!({})), body_format: format?, relation, grant })
            })
            .collect())
    }

    async fn app_body(&self, app: &ExportedApp) -> Result<Option<String>, String> {
        sqlx::query_scalar::<_, String>(
            "SELECT v.body FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id AND v.version = a.app_pinned_version WHERE a.id = $1",
        )
        .bind(&app.artifact_id)
        .fetch_optional(&self.state.db)
        .await
        .map_err(|e| e.to_string())
    }

    async fn storage(&self, app: &ExportedApp, user_id: &str, op: StorageOp) -> Result<Value, MirrorError> {
        use crate::artifacts::runtime as rules;
        let db = &self.state.db;
        let scope_of = |raw: &str| rules::Scope::parse(raw).ok_or_else(|| MirrorError::new("invalid_scope", "scope is user or shared"));
        let bad_key = || MirrorError::new("invalid_key", "key must be 1-200 printable characters");
        match op {
            StorageOp::Get { scope, key } => {
                let scope = scope_of(&scope)?;
                if !rules::valid_key(&key) {
                    return Err(bad_key());
                }
                let value = sqlx::query_scalar::<_, String>("SELECT value FROM artifact_storage WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key = $4")
                    .bind(&app.artifact_id)
                    .bind(scope.as_str())
                    .bind(scope.owner_column(user_id))
                    .bind(&key)
                    .fetch_optional(db)
                    .await
                    .map_err(db_error)?;
                Ok(json!({ "key": key, "scope": scope.as_str(), "value": value }))
            }
            StorageOp::List { scope, prefix } => {
                let scope = scope_of(&scope)?;
                if !rules::valid_prefix(&prefix) {
                    return Err(MirrorError::new("invalid_prefix", "invalid prefix"));
                }
                let keys = sqlx::query_scalar::<_, String>(
                    "SELECT key FROM artifact_storage WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key LIKE $4 ESCAPE '\\' ORDER BY key LIMIT $5",
                )
                .bind(&app.artifact_id)
                .bind(scope.as_str())
                .bind(scope.owner_column(user_id))
                .bind(rules::prefix_pattern(&prefix))
                .bind(rules::MAX_LIST_KEYS)
                .fetch_all(db)
                .await
                .map_err(db_error)?;
                let truncated = keys.len() as i64 >= rules::MAX_LIST_KEYS;
                Ok(json!({ "scope": scope.as_str(), "keys": keys, "truncated": truncated }))
            }
            StorageOp::Delete { scope, key } => {
                let scope = scope_of(&scope)?;
                if !rules::valid_key(&key) {
                    return Err(bad_key());
                }
                sqlx::query("DELETE FROM artifact_storage WHERE artifact_id = $1 AND scope = $2 AND user_id = $3 AND key = $4")
                    .bind(&app.artifact_id)
                    .bind(scope.as_str())
                    .bind(scope.owner_column(user_id))
                    .bind(&key)
                    .execute(db)
                    .await
                    .map_err(db_error)?;
                Ok(json!({ "key": key, "scope": scope.as_str(), "deleted": true }))
            }
            StorageOp::Set { scope, key, value } => {
                let scope = scope_of(&scope)?;
                if !rules::valid_key(&key) {
                    return Err(bad_key());
                }
                if value.len() > rules::MAX_VALUE_BYTES {
                    return Err(MirrorError::new("value_too_large", "A stored value can be at most 5 MB"));
                }
                let owner = scope.owner_column(user_id);
                let new_bytes = rules::entry_bytes(&key, &value);
                // One writer per artifact at a time (same 20 MB cap as the page runtime).
                let mut tx = db.begin().await.map_err(db_error)?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))").bind(&app.artifact_id).execute(&mut *tx).await.map_err(db_error)?;
                let others: i64 = sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT SUM(bytes)::BIGINT FROM artifact_storage WHERE artifact_id = $1 AND NOT (scope = $2 AND user_id = $3 AND key = $4)",
                )
                .bind(&app.artifact_id)
                .bind(scope.as_str())
                .bind(&owner)
                .bind(&key)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_error)?
                .unwrap_or(0);
                if !rules::fits_quota(others, new_bytes) {
                    return Err(MirrorError::new("storage_full", "This app's storage is full (20 MB)").with("used_bytes", json!(others)).with("limit_bytes", json!(rules::MAX_STORAGE_BYTES)));
                }
                sqlx::query(
                    "INSERT INTO artifact_storage (artifact_id, scope, user_id, key, value, bytes, updated_at) VALUES ($1, $2, $3, $4, $5, $6, now()) \
                     ON CONFLICT (artifact_id, scope, user_id, key) DO UPDATE SET value = EXCLUDED.value, bytes = EXCLUDED.bytes, updated_at = now()",
                )
                .bind(&app.artifact_id)
                .bind(scope.as_str())
                .bind(&owner)
                .bind(&key)
                .bind(&value)
                .bind(new_bytes as i32)
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                Ok(json!({ "key": key, "scope": scope.as_str(), "bytes": new_bytes }))
            }
        }
    }

    async fn ai_complete(&self, app: &ExportedApp, user_id: &str, args: &Value) -> Result<Value, MirrorError> {
        use crate::artifacts::runtime as rules;
        let mut messages: Vec<(String, String)> = Vec::new();
        if let Some(prompt) = args["prompt"].as_str() {
            if let Some(system) = args["system"].as_str() {
                messages.push(("system".into(), system.to_string()));
            }
            messages.push(("user".into(), prompt.to_string()));
        } else if let Some(list) = args["messages"].as_array() {
            for m in list {
                let (Some(role), Some(content)) = (m["role"].as_str(), m["content"].as_str()) else {
                    return Err(MirrorError::new("invalid_ai_request", "each message needs a text role and content"));
                };
                messages.push((role.to_string(), content.to_string()));
            }
        }
        let max_tokens = args["max_tokens"].as_u64().map(|n| n.min(u32::MAX as u64) as u32);
        let shape: Vec<(String, usize)> = messages.iter().map(|(r, t)| (r.clone(), t.chars().count())).collect();
        let max_tokens = rules::check_ai_request(&shape, max_tokens).map_err(|why| MirrorError::new("invalid_ai_request", why))?;
        let temperature = args["temperature"].as_f64().map(|t| t.clamp(0.0, 1.5) as f32);
        if !ai_rate_ok(user_id, &app.artifact_id) {
            return Err(MirrorError::new("rate_limited", "This app is making too many AI calls. Wait a minute."));
        }
        crate::routes::artifacts_v2::artifact_runtime::billed_completion(self.state, user_id, messages, max_tokens, temperature).await.map_err(|e| match e {
            crate::artifacts::error::ArtifactError::Coded { code, message, .. } => MirrorError::new(code, message),
            crate::artifacts::error::ArtifactError::Api(_) => MirrorError::new("ai_failed", "The AI call failed"),
        })
    }
}

#[cfg(test)]
#[path = "mcp_edge_apps_tests.rs"]
mod tests;
