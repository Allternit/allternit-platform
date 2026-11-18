//! Connector search (Primitives Wave D, D2): gizzi's `connector_search` tool.
//!
//! Cloud connectors and their OAuth tokens live here, in allternit-api; the
//! runtime only ever reaches a connected one as proxied MCP tools
//! (`mcp__<server>__<connector>__<tool>`, `mcp_user_proxy`). Whose connectors
//! they are comes from the session id ([`resolve_owner`], the same owner check
//! as the other bot-routing tools), never from the model. No token, link or
//! code is ever returned.
//!
//! * `POST /bot-routing/connector/search` `{sessionId, query, limit ≤10}` →
//!   `{hits: [{id, name, group, state, available, reason?, mcpConnectorId?, serverName?}]}`.
//!   - Candidates: the connector catalog (`assets/open-design/connectors.json`,
//!     the ids the Connect kit's `/connectors/<id>/connect` takes) with the
//!     owner's `connector_connections`, plus the owner's own MCP connectors
//!     (`mcp_connectors`) that no catalog connection already stands for.
//!   - `state`: `connected` (usable now), `available` (the user can connect it
//!     here; `reason` says when an old connection needs signing in again) or
//!     `unavailable` (can't be connected here; `reason` says why).
//!     `available` is `state != unavailable`.
//!   - `mcpConnectorId` + `serverName`: on a connected hit whose tools are
//!     proxied MCP tools (a custom MCP connector, or a catalog app connected
//!     through its official MCP server), the `mcp_connectors` id the proxy tags
//!     its tools with and the proxy server name, so gizzi can `load` them. A
//!     connected catalog app without them runs through the app connectors'
//!     `execute_action` tool.
//!   - Matching is by words over name, id, group, description and tool titles;
//!     connected hits win ties.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent_user_tools::{resolve_owner, Refusal};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::AppState;

/// Most hits one search returns.
pub const MAX_LIMIT: usize = 10;
pub const DEFAULT_LIMIT: usize = 5;
const MAX_QUERY_CHARS: usize = 200;
/// Group shown for the user's own MCP servers.
const CUSTOM_GROUP: &str = "Custom";

const CATALOG_JSON: &str = include_str!("../assets/open-design/connectors.json");

pub fn agent_connector_tools_router() -> Router<Arc<AppState>> {
    Router::new().route("/bot-routing/connector/search", post(search_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBody {
    pub session_id: String,
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitState {
    Connected,
    Available,
    Unavailable,
}

impl HitState {
    fn as_str(self) -> &'static str {
        match self {
            HitState::Connected => "connected",
            HitState::Available => "available",
            HitState::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: String,
    pub name: String,
    pub group: String,
    pub state: HitState,
    pub reason: Option<String>,
    pub mcp_connector_id: Option<String>,
}

impl Hit {
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "name": self.name,
            "group": self.group,
            "state": self.state.as_str(),
            "available": self.state != HitState::Unavailable,
        });
        if let Some(r) = &self.reason {
            v["reason"] = json!(r);
        }
        if let Some(m) = &self.mcp_connector_id {
            v["mcpConnectorId"] = json!(m);
            v["serverName"] = json!(crate::mcp_user_proxy::PROXY_SERVER_NAME);
        }
        v
    }
}

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into(), "code": code }))).into_response()
}

// ---- catalog ----

struct CatalogEntry {
    id: String,
    name: String,
    group: String,
    /// `status` in the catalog; anything but "available" can't be connected.
    listed: bool,
    /// Lowercased description and tool titles, for matching.
    text: String,
}

fn catalog() -> &'static [CatalogEntry] {
    static C: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    C.get_or_init(|| {
        let v: Value = serde_json::from_str(CATALOG_JSON).unwrap_or_else(|_| json!({ "connectors": [] }));
        let s = |c: &Value, k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        v["connectors"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|c| {
                        let id = s(c, "id");
                        if id.is_empty() {
                            return None;
                        }
                        let mut text = s(c, "description");
                        for t in c["tools"].as_array().into_iter().flatten() {
                            text.push(' ');
                            text.push_str(t["title"].as_str().unwrap_or(""));
                            text.push(' ');
                            text.push_str(t["name"].as_str().unwrap_or(""));
                        }
                        let name = Some(s(c, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                        Some(CatalogEntry {
                            listed: c.get("status").and_then(|x| x.as_str()).unwrap_or("available") == "available",
                            group: s(c, "category"),
                            text: text.to_lowercase(),
                            name,
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

// ---- matching ----

/// Words that match nearly every description, so they never count.
const STOPWORDS: &[&str] = &["an", "and", "the", "to", "for", "my", "our", "in", "on", "of", "with", "from", "app", "apps"];

fn words(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2 && !STOPWORDS.contains(w))
        .map(str::to_string)
        .collect()
}

/// 0 = no match.
fn score(query: &str, terms: &[String], id: &str, name: &str, group: &str, text: &str) -> u32 {
    let (id, name, group) = (id.to_lowercase(), name.to_lowercase(), group.to_lowercase());
    let q = query.trim().to_lowercase();
    let mut s = 0;
    if !q.is_empty() && (name == q || id == q) {
        s += 100;
    } else if !q.is_empty() && (name.contains(&q) || id.contains(&q)) {
        s += 50;
    }
    for t in terms {
        if name.contains(t.as_str()) || id.contains(t.as_str()) {
            s += 20;
        }
        if group.contains(t.as_str()) {
            s += 10;
        }
        if text.contains(t.as_str()) {
            s += 4;
        }
    }
    s
}

// ---- the owner's connections ----

/// The owner's catalog connection for one connector id, folded over its accounts.
#[derive(Default)]
struct Conn {
    connected: bool,
    pending: bool,
    broken: bool,
    /// `mcp_connectors` id of a connection made through the vendor's official MCP server.
    mcp_connector_id: Option<String>,
}

fn connections(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<HashMap<String, Conn>> {
    let mut stmt = conn.prepare(
        "SELECT connector_id, status,
                CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.official_mcp.mcp_connector_id') END
         FROM connector_connections WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map(params![user_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))?;
    let mut out: HashMap<String, Conn> = HashMap::new();
    for (connector_id, status, mcp_id) in rows.filter_map(Result::ok) {
        let c = out.entry(connector_id).or_default();
        match status.as_str() {
            "connected" | "active" => {
                c.connected = true;
                if c.mcp_connector_id.is_none() {
                    c.mcp_connector_id = mcp_id.filter(|m| !m.is_empty());
                }
            }
            "pending" => c.pending = true,
            "disconnected" | "" => {}
            _ => c.broken = true,
        }
    }
    Ok(out)
}

struct McpRow {
    id: String,
    name: String,
    enabled: bool,
}

fn mcp_connectors(conn: &rusqlite::Connection, user_id: &str) -> rusqlite::Result<Vec<McpRow>> {
    let mut stmt = conn.prepare("SELECT id, name, enabled FROM mcp_connectors WHERE user_id = ?1 ORDER BY name, id")?;
    let rows = stmt.query_map(params![user_id], |r| Ok(McpRow { id: r.get(0)?, name: r.get(1)?, enabled: r.get::<_, i64>(2)? != 0 }))?;
    let out: Vec<McpRow> = rows.filter_map(Result::ok).collect();
    Ok(out)
}

/// Search `user_id`'s connectors. The caller has already resolved the owner.
pub fn search(conn: &rusqlite::Connection, user_id: &str, query: &str, limit: usize) -> rusqlite::Result<Vec<Hit>> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let terms = words(query);
    let conns = connections(conn, user_id)?;
    let mut scored: Vec<(u32, Hit)> = Vec::new();

    for c in catalog() {
        let s = score(query, &terms, &c.id, &c.name, &c.group, &c.text);
        if s == 0 {
            continue;
        }
        let mine = conns.get(&c.id);
        let (state, reason, mcp) = match mine {
            Some(m) if m.connected => (HitState::Connected, None, m.mcp_connector_id.clone()),
            _ if !c.listed => (HitState::Unavailable, Some("Not offered on this computer.".to_string()), None),
            Some(m) if m.broken => (HitState::Available, Some("The connection stopped working; the user needs to sign in again.".to_string()), None),
            Some(m) if m.pending => (HitState::Available, Some("A sign-in was started but not finished.".to_string()), None),
            _ => (HitState::Available, None, None),
        };
        scored.push((s, Hit { id: c.id.clone(), name: c.name.clone(), group: c.group.clone(), state, reason, mcp_connector_id: mcp }));
    }

    // The user's own MCP servers, unless a catalog connection already stands for one.
    let linked: HashSet<&str> = conns.values().filter_map(|c| c.mcp_connector_id.as_deref()).collect();
    for m in mcp_connectors(conn, user_id)? {
        if linked.contains(m.id.as_str()) {
            continue;
        }
        let s = score(query, &terms, &m.id, &m.name, CUSTOM_GROUP, "");
        if s == 0 {
            continue;
        }
        let hit = if m.enabled {
            Hit { id: m.id.clone(), name: m.name, group: CUSTOM_GROUP.into(), state: HitState::Connected, reason: None, mcp_connector_id: Some(m.id) }
        } else {
            Hit {
                id: m.id,
                name: m.name,
                group: CUSTOM_GROUP.into(),
                state: HitState::Unavailable,
                reason: Some("Turned off. The user can turn it on in Settings → Connectors.".into()),
                mcp_connector_id: None,
            }
        };
        scored.push((s, hit));
    }

    scored.sort_by(|(sa, a), (sb, b)| {
        sb.cmp(sa)
            .then_with(|| (b.state == HitState::Connected).cmp(&(a.state == HitState::Connected)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(scored.into_iter().take(limit).map(|(_, h)| h).collect())
}

/// The whole route behind the HTTP edge: owner check, input checks, search.
pub fn search_for(db: &DbHandle, caller_id: &str, body: &SearchBody, paired_owner: Option<&str>) -> Result<Vec<Hit>, Refusal> {
    let owner = resolve_owner(db, caller_id, &body.session_id, paired_owner)?;
    let query: String = body.query.trim().chars().take(MAX_QUERY_CHARS).collect();
    if query.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "bad_query", "query is empty".into()));
    }
    let limit = body.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err((StatusCode::BAD_REQUEST, "bad_limit", format!("limit must be 1 to {MAX_LIMIT}")));
    }
    let conn = db.connect().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string()))?;
    search(&conn, &owner.user_id, &query, limit).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, "failed", e.to_string()))
}

async fn search_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<SearchBody>) -> Response {
    let db = state.db.clone();
    let uid = user.user_id.clone();
    let paired = crate::relay_auth::process_secret().paired_owner();
    match tokio::task::spawn_blocking(move || search_for(&db, &uid, &body, paired.as_deref())).await {
        Ok(Ok(hits)) => Json(json!({ "hits": hits.iter().map(Hit::to_json).collect::<Vec<_>>() })).into_response(),
        Ok(Err((status, code, msg))) => err(status, code, msg),
        Err(e) => err(StatusCode::BAD_GATEWAY, "failed", e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::INTERNAL_SERVICE_USER_ID;

    async fn setup() -> Arc<AppState> {
        let dir = std::env::temp_dir().join(format!("allternit-connector-tools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::test_helpers::app_state(&dir).await
    }

    fn body(session: &str, query: &str) -> SearchBody {
        SearchBody { session_id: session.into(), query: query.into(), limit: Some(10) }
    }

    fn seed(st: &AppState) {
        let c = st.db.connect().unwrap();
        c.execute_batch(
            "INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account_key, is_default)
               VALUES ('a1', 'slack', 'user-a', 'oauth2', 'connected', 'a@x', 1);
             INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account_key, is_default, metadata)
               VALUES ('a2', 'notion', 'user-a', 'official_mcp', 'connected', 'a@x', 1, '{\"official_mcp\":{\"mcp_connector_id\":\"mc-notion-a\"}}');
             INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account_key, is_default)
               VALUES ('a3', 'linear', 'user-a', 'oauth2', 'expired', 'a@x', 1);
             INSERT INTO mcp_connectors (id, user_id, name, name_id, url, enabled) VALUES ('mc-notion-a', 'user-a', 'Notion', 'notion', 'https://mcp.notion.com/mcp', 1);
             INSERT INTO mcp_connectors (id, user_id, name, name_id, url, enabled) VALUES ('mc-wiki', 'user-a', 'Team Wiki', 'team-wiki', 'https://wiki.test/mcp', 1);
             INSERT INTO mcp_connectors (id, user_id, name, name_id, url, enabled) VALUES ('mc-old', 'user-a', 'Old Wiki', 'old-wiki', 'https://old.test/mcp', 0);
             INSERT INTO connector_connections (id, connector_id, user_id, auth_type, status, account_key, is_default)
               VALUES ('b1', 'github', 'user-b', 'oauth2', 'connected', 'b@x', 1);
             INSERT INTO mcp_connectors (id, user_id, name, name_id, url, enabled) VALUES ('mc-b', 'user-b', 'Bobs Wiki', 'bobs-wiki', 'https://b.test/mcp', 1);",
        )
        .unwrap();
    }

    fn find<'a>(hits: &'a [Hit], id: &str) -> Option<&'a Hit> {
        hits.iter().find(|h| h.id == id)
    }

    #[tokio::test]
    async fn only_the_session_owner_sees_their_connections() {
        let st = setup().await;
        seed(&st);
        // A Gizzi Code session on a runtime paired as user-a.
        let mine = search_for(&st.db, "user-a", &body("s-code", "github"), Some("user-a")).unwrap();
        assert_eq!(find(&mine, "github").unwrap().state, HitState::Available, "user-b's GitHub is not user-a's");
        // Someone else can't search through user-a's session.
        let refused = search_for(&st.db, "user-b", &body("s-code", "github"), Some("user-a")).unwrap_err();
        assert_eq!(refused.1, "not_your_session");
        // The internal service acts for the paired owner.
        let svc = search_for(&st.db, INTERNAL_SERVICE_USER_ID, &body("s-code", "slack"), Some("user-a")).unwrap();
        assert_eq!(find(&svc, "slack").unwrap().state, HitState::Connected);
        // Another user's own MCP servers never show.
        let wiki = search_for(&st.db, "user-a", &body("s-code", "wiki"), Some("user-a")).unwrap();
        assert!(find(&wiki, "mc-b").is_none());
        assert!(search_for(&st.db, "user-a", &body("../x", "wiki"), None).unwrap_err().1 == "bad_session");
    }

    #[tokio::test]
    async fn states_and_loadable_tools() {
        let st = setup().await;
        seed(&st);
        let conn = st.db.connect().unwrap();

        let notion = search(&conn, "user-a", "notion", 10).unwrap();
        let n = find(&notion, "notion").unwrap();
        assert_eq!((n.state, n.mcp_connector_id.as_deref()), (HitState::Connected, Some("mc-notion-a")));
        assert_eq!(n.to_json()["serverName"], crate::mcp_user_proxy::PROXY_SERVER_NAME);
        // The linked MCP row is the catalog hit, not a second one.
        assert!(find(&notion, "mc-notion-a").is_none());

        let slack = search(&conn, "user-a", "slack", 10).unwrap();
        let s = find(&slack, "slack").unwrap();
        assert_eq!((s.state, s.mcp_connector_id.as_deref()), (HitState::Connected, None), "sidecar apps run through execute_action");
        assert!(s.to_json().get("serverName").is_none());

        let linear = search(&conn, "user-a", "linear", 10).unwrap();
        let l = find(&linear, "linear").unwrap();
        assert_eq!(l.state, HitState::Available);
        assert!(l.reason.as_deref().unwrap().contains("sign in again"));

        let wiki = search(&conn, "user-a", "wiki", 10).unwrap();
        let w = find(&wiki, "mc-wiki").unwrap();
        assert_eq!((w.state, w.group.as_str(), w.mcp_connector_id.as_deref()), (HitState::Connected, CUSTOM_GROUP, Some("mc-wiki")));
        let old = find(&wiki, "mc-old").unwrap();
        assert_eq!((old.state, old.to_json()["available"].as_bool()), (HitState::Unavailable, Some(false)));
    }

    #[tokio::test]
    async fn limits_and_bad_input() {
        let st = setup().await;
        seed(&st);
        let conn = st.db.connect().unwrap();
        assert!(search(&conn, "user-a", "a", 10).unwrap().len() <= MAX_LIMIT);
        let mut b = body("s-code", "mail");
        b.limit = Some(11);
        assert_eq!(search_for(&st.db, "user-a", &b, Some("user-a")).unwrap_err().1, "bad_limit");
        b.limit = Some(2);
        assert!(search_for(&st.db, "user-a", &b, Some("user-a")).unwrap().len() <= 2);
        assert_eq!(search_for(&st.db, "user-a", &body("s-code", "   "), Some("user-a")).unwrap_err().1, "bad_query");
        // Nothing matches: an empty list, not an error.
        assert!(search(&conn, "user-a", "zzqqxx", 10).unwrap().is_empty());
    }
}
