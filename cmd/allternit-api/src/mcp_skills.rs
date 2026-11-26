//! Skills from connected MCP servers (SEP-2640), as one catalog per user with
//! stable ids and on/off switches (mcp-skills-app 2026-11-26, CONTRACT.md).
//!
//! ```text
//!  GET /api/v1/mcp/skills ─▶ catalog_refresh ─┬─ per enabled connector, in parallel, 8 s each:
//!                                             │    cache hit (< 10 min) or list_skills()
//!                                             ├─ ids  <connector tool prefix>:<skill>   (-2, -3 on collision)
//!                                             └─ enabled = mcp_skill_prefs row, else defaultEnabled
//!  PUT /api/v1/mcp/skills/prefs {id, enabled} ─▶ set_pref ─▶ updated item (404 skill_not_found)
//!  a turn (load_skill, run B) ─▶ catalog_cached   (memory only, never touches a connector)
//! ```
//!
//! * The prefix is the one the per-user proxy gives the connector's tools
//!   ([`crate::mcp_user_proxy::connector_prefixes`] over the same connector
//!   list), so `github:review-pr` and `github__create_issue` read as one server.
//! * The skill part follows Gizzi's `skillNameFromUri`
//!   (cmd/gizzi-code/src/runtime/skills/mcpSkills.ts): the path segment before
//!   `SKILL.md`, percent-decoded, `[^a-zA-Z0-9_-]` → `_`.
//! * A connector that is down, signed out or slow is left out of that refresh
//!   (and its cached entry dropped); nothing failed is cached.
//! * Cloud skills (`user | org | app`) come from cloud-api
//!   `GET /api/v1/skills/available`, path-signed as this computer's owner
//!   (account_sync.rs), cached per user beside the connectors with the same
//!   TTL. ids `cloud:<name>`, `org:<name>`, `app-<slug>:<name>`. Cloud down or
//!   unpaired → the server skills are still listed.
//! * `load_skill` (the per-user proxy, turn callers only) and the `/` pick on a
//!   chat turn resolve ids from [`catalog_cached`] only, never a URI.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mcp_client::SkillInfo;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mcp_apps::{self, Connector};
use crate::mcp_events_client::Ctx;
use crate::AppState;

/// How long one connector's skill list is reused before it is fetched again.
pub const CATALOG_TTL: Duration = Duration::from_secs(10 * 60);
/// Budget for one connector's `resources/list` (connect + pages + close).
pub const PER_CONNECTOR_TIMEOUT: Duration = Duration::from_secs(8);
/// Most numbered steps [`parse_steps`] returns.
pub const MAX_STEPS: usize = 20;

pub const SOURCE_SERVER: &str = "server";
pub const SOURCE_USER: &str = "user";
pub const SOURCE_ORG: &str = "org";
pub const SOURCE_APP: &str = "app";
/// The proxy tool name (no connector prefix: connector tools always carry `__`).
pub const LOAD_SKILL_TOOL: &str = "load_skill";
/// `_meta` key on the listed `load_skill` tool: `[{id, description, from, source}]`.
pub const SKILLS_META_KEY: &str = "allternit/skills";
/// Skills named in the `load_skill` description.
pub const MAX_LISTED: usize = 50;
const LISTED_DESCRIPTION_CHARS: usize = 160;
/// A skill read (connector or cloud) must finish inside this.
pub const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// Larger SKILL.md text is refused (`skill_too_large`).
pub const MAX_SKILL_BYTES: usize = 64 * 1024;
const CLOUD_AVAILABLE_PATH: &str = "/api/v1/skills/available";
const CLOUD_READ_PATH: &str = "/api/v1/skills/read";

/// One skill in a user's catalog (`GET /api/v1/mcp/skills` item).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillItem {
    pub id: String,
    /// `server | app | user | org`.
    pub source: String,
    /// Display name of where it comes from ("Nimbrook").
    pub from: String,
    /// Connector display name (kept for older clients).
    pub server: String,
    pub connector_id: String,
    pub uri: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub default_enabled: bool,
}

// ---------------------------------------------------------------- naming

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// Gizzi's `skillNameFromUri`: `skill://a/b/SKILL.md` → `b`, `skill://x.md` → `x`.
pub fn skill_name_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("skill://")?;
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let last = *parts.last()?;
    let raw = if parts.len() > 1 && last.eq_ignore_ascii_case("skill.md") {
        parts[parts.len() - 2].to_string()
    } else if last.len() >= 3 && last.get(last.len() - 3..).is_some_and(|ext| ext.eq_ignore_ascii_case(".md")) {
        last[..last.len() - 3].to_string()
    } else {
        last.to_string()
    };
    let decoded = urlencoding::decode(&raw).map(|c| c.into_owned()).unwrap_or(raw);
    (!decoded.is_empty()).then(|| sanitize(&decoded))
}

/// A connector as the catalog needs it: id, display name, tool prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnMeta {
    pub id: String,
    pub name: String,
    pub prefix: String,
}

/// [`ConnMeta`] for every connector, prefixes exactly as the user proxy names their tools.
pub fn conn_metas(connectors: &[Connector]) -> Vec<ConnMeta> {
    crate::mcp_user_proxy::connector_prefixes(connectors)
        .into_iter()
        .zip(connectors)
        .map(|(prefix, c)| ConnMeta { id: c.id.clone(), name: c.name.clone(), prefix })
        .collect()
}

/// Build the catalog: ids `<prefix>:<skill>` (collisions `-2`, `-3`, … in list
/// order), `enabled` = the pref row when there is one, else the default.
pub fn assemble(conns: &[ConnMeta], skills: &HashMap<String, Vec<SkillInfo>>, prefs: &HashMap<String, bool>) -> Vec<SkillItem> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for conn in conns {
        let Some(list) = skills.get(&conn.id) else { continue };
        for skill in list {
            let part = skill_name_from_uri(&skill.uri).unwrap_or_else(|| sanitize(&skill.name));
            let part = if part.is_empty() { "skill".to_string() } else { part };
            let base = format!("{}:{part}", conn.prefix);
            let mut id = base.clone();
            let mut n = 2;
            while !seen.insert(id.clone()) {
                id = format!("{base}-{n}");
                n += 1;
            }
            let default_enabled = true; // server skills default on (CONTRACT.md amendments)
            out.push(SkillItem {
                enabled: prefs.get(&id).copied().unwrap_or(default_enabled),
                default_enabled,
                id,
                source: SOURCE_SERVER.into(),
                from: conn.name.clone(),
                server: conn.name.clone(),
                connector_id: conn.id.clone(),
                uri: skill.uri.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------- cache

/// Process-wide; keys carry the database path so separate states (tests) never share entries.
type Key = (String, String);

struct UserSnapshot {
    conns: Vec<ConnMeta>,
    prefs: HashMap<String, bool>,
}

#[derive(Default)]
struct Cache {
    /// (db, user, connector) → skills listed at that instant.
    skills: HashMap<(String, String, String), (Instant, Vec<SkillInfo>)>,
    /// (db, user) → connector order + prefs from the last refresh (for [`catalog_cached`]).
    users: HashMap<Key, UserSnapshot>,
    /// (db, user) → cloud-api skills listed at that instant.
    cloud: HashMap<Key, (Instant, Vec<CloudSkill>)>,
    /// (db, user) → when a turn last started a background refresh ([`turn_tool`]).
    kicked: HashMap<Key, Instant>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

fn db_key(state: &AppState) -> String {
    state.db.path().to_string_lossy().into_owned()
}

/// Forget one connector's skills for `user` (connector added, removed, enabled
/// or signed in again). Every database's entry for that pair goes.
pub fn invalidate(user: &str, connector_id: &str) {
    if let Ok(mut c) = cache().lock() {
        c.skills.retain(|(_, u, cid), _| !(u == user && cid == connector_id));
        for ((_, u), snap) in c.users.iter_mut() {
            if u == user {
                snap.conns.retain(|m| m.id != connector_id);
            }
        }
    }
}

/// The user's catalog from memory only (never contacts a connector or the
/// database). Empty until a [`catalog_refresh`] ran in this process. Entries
/// older than [`CATALOG_TTL`] are still returned; the next refresh replaces them.
pub fn catalog_cached(state: &AppState, user: &str) -> Vec<SkillItem> {
    let db = db_key(state);
    let Ok(c) = cache().lock() else { return Vec::new() };
    let Some(snap) = c.users.get(&(db.clone(), user.to_string())) else { return Vec::new() };
    let skills: HashMap<String, Vec<SkillInfo>> = snap
        .conns
        .iter()
        .filter_map(|m| c.skills.get(&(db.clone(), user.to_string(), m.id.clone())).map(|(_, s)| (m.id.clone(), s.clone())))
        .collect();
    let mut items = assemble(&snap.conns, &skills, &snap.prefs);
    if let Some((_, cloud)) = c.cloud.get(&(db, user.to_string())) {
        merge_cloud(&mut items, cloud, &snap.prefs);
    }
    items
}

/// One catalog item by id, from memory ([`catalog_cached`]).
pub fn find_cached(state: &AppState, user: &str, id: &str) -> Option<SkillItem> {
    catalog_cached(state, user).into_iter().find(|s| s.id == id)
}

async fn fetch_connector(connector: &Connector, allow_private: bool) -> Option<Vec<SkillInfo>> {
    if connector.auth_failed() {
        return None;
    }
    tokio::time::timeout(PER_CONNECTOR_TIMEOUT, async {
        let client = mcp_apps::open_session(connector, allow_private).await.ok()?;
        let skills = client.list_skills().await.ok();
        mcp_apps::close_session(client).await;
        skills
    })
    .await
    .ok()
    .flatten()
}

/// The user's catalog, fetching (in parallel) every enabled connector whose
/// cached list is missing or older than [`CATALOG_TTL`]. Also refreshes the
/// memory [`catalog_cached`] reads.
pub async fn catalog_refresh(state: &Arc<AppState>, ctx: &Ctx, user: &str) -> Vec<SkillItem> {
    let db = db_key(state);
    let connectors = mcp_apps::load_user_connectors(state, user, ctx.allow_private).await;
    let conns = conn_metas(&connectors);
    let prefs = load_prefs(state, user).await;

    let fresh: HashMap<String, Vec<SkillInfo>> = {
        let c = cache().lock().map(|c| {
            connectors
                .iter()
                .filter_map(|k| {
                    c.skills
                        .get(&(db.clone(), user.to_string(), k.id.clone()))
                        .filter(|(at, _)| at.elapsed() < CATALOG_TTL)
                        .map(|(_, s)| (k.id.clone(), s.clone()))
                })
                .collect()
        });
        c.unwrap_or_default()
    };
    let stale: Vec<&Connector> = connectors.iter().filter(|k| !fresh.contains_key(&k.id)).collect();
    let (fetched, cloud) = tokio::join!(
        futures::future::join_all(stale.iter().map(|k| async move { (k.id.clone(), fetch_connector(k, ctx.allow_private).await) })),
        cloud_refresh(state, user),
    );

    let mut skills = fresh;
    if let Ok(mut c) = cache().lock() {
        for (id, listed) in fetched {
            let key = (db.clone(), user.to_string(), id.clone());
            match listed {
                Some(list) => {
                    c.skills.insert(key, (Instant::now(), list.clone()));
                    skills.insert(id, list);
                }
                None => {
                    c.skills.remove(&key);
                }
            }
        }
        c.users.insert((db.clone(), user.to_string()), UserSnapshot { conns: conns.clone(), prefs: prefs.clone() });
    }
    let mut items = assemble(&conns, &skills, &prefs);
    merge_cloud(&mut items, &cloud, &prefs);
    items
}

// ---------------------------------------------------------------- prefs (mcp_skill_prefs)

async fn load_prefs(state: &Arc<AppState>, user: &str) -> HashMap<String, bool> {
    let db = state.db.clone();
    let user = user.to_string();
    tokio::task::spawn_blocking(move || -> rusqlite::Result<HashMap<String, bool>> {
        let c = db.connect()?;
        let mut stmt = c.prepare("SELECT skill_id, enabled FROM mcp_skill_prefs WHERE user_id = ?1")?;
        let rows = stmt.query_map(params![user], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))?;
        let prefs = rows.collect::<rusqlite::Result<HashMap<String, bool>>>()?;
        Ok(prefs)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default()
}

#[derive(Debug, PartialEq, Eq)]
pub enum PrefError {
    NotFound,
    Db(String),
}

/// Turn one catalog skill on or off for `user`. `Err(NotFound)` when the id is
/// not in the user's catalog (memory first, then a refresh).
pub async fn set_pref(state: &Arc<AppState>, ctx: &Ctx, user: &str, id: &str, enabled: bool) -> Result<SkillItem, PrefError> {
    let known = match find_cached(state, user, id) {
        Some(item) => Some(item),
        None => catalog_refresh(state, ctx, user).await.into_iter().find(|s| s.id == id),
    };
    let Some(mut item) = known else { return Err(PrefError::NotFound) };
    let db = state.db.clone();
    let (u, sid) = (user.to_string(), id.to_string());
    tokio::task::spawn_blocking(move || -> rusqlite::Result<usize> {
        db.connect()?.execute(
            "INSERT INTO mcp_skill_prefs (user_id, skill_id, enabled, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id, skill_id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
            params![u, sid, enabled as i64, chrono::Utc::now().to_rfc3339()],
        )
    })
    .await
    .map_err(|e| PrefError::Db(e.to_string()))?
    .map_err(|e| PrefError::Db(e.to_string()))?;
    if let Ok(mut c) = cache().lock() {
        if let Some(snap) = c.users.get_mut(&(db_key(state), user.to_string())) {
            snap.prefs.insert(id.to_string(), enabled);
        }
    }
    item.enabled = enabled;
    Ok(item)
}

// ---------------------------------------------------------------- SKILL.md text

/// `text` without a leading YAML frontmatter block (`---` … `---`/`...`). Text
/// whose frontmatter never closes is returned unchanged.
pub fn strip_frontmatter(text: &str) -> &str {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = t.split_inclusive('\n');
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return text,
    }
    let mut offset = t.split_inclusive('\n').next().map(str::len).unwrap_or(0);
    for line in lines {
        offset += line.len();
        let l = line.trim_end();
        if l == "---" || l == "..." {
            return t[offset..].trim_start_matches(['\r', '\n']);
        }
    }
    text
}

/// The top-level numbered steps of a SKILL.md body (frontmatter stripped
/// first): lines matching `^\d+[.)]\s+(.+)` with no leading spaces, at most
/// [`MAX_STEPS`], markdown emphasis / code ticks / links reduced to plain text.
pub fn parse_steps(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in strip_frontmatter(body).lines() {
        if out.len() == MAX_STEPS {
            break;
        }
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let rest = &line[digits..];
        let Some(after) = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')')) else { continue };
        if !after.starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let text = plain_text(after.trim());
        if !text.is_empty() {
            out.push(text);
        }
    }
    out
}

/// Markdown inline → plain text: `[t](u)`/`![t](u)` → `t`, code ticks, `**`,
/// `__`, `~~` and `*` removed, `_` removed only at word edges (keeps `load_skill`).
fn plain_text(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut linked = String::new();
    let mut i = 0;
    while i < chars.len() {
        let bang = chars[i] == '!' && chars.get(i + 1) == Some(&'[');
        let start = if bang { i + 1 } else { i };
        if chars[start] == '[' {
            if let Some(close) = chars[start + 1..].iter().position(|&c| c == ']').map(|p| p + start + 1) {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = chars[close + 2..].iter().position(|&c| c == ')').map(|p| p + close + 2) {
                        linked.extend(&chars[start + 1..close]);
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        linked.push(chars[i]);
        i += 1;
    }
    let no_marks = linked.replace("**", "").replace("__", "").replace("~~", "").replace(['`', '*'], "");
    let c: Vec<char> = no_marks.chars().collect();
    let mut out = String::new();
    for (k, &ch) in c.iter().enumerate() {
        if ch == '_' {
            let before = k > 0 && c[k - 1].is_alphanumeric();
            let after = c.get(k + 1).is_some_and(|n| n.is_alphanumeric());
            if !(before && after) {
                continue;
            }
        }
        out.push(ch);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- cloud skills (cloud-api)

fn yes() -> bool {
    true
}

/// One `GET /api/v1/skills/available` item (cloud-api `cloud_skills.rs`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSkill {
    pub source: String,
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// "You", the org id, or the app name.
    #[serde(default)]
    pub from: String,
    #[serde(default = "yes")]
    pub default_enabled: bool,
}

/// cloud-api as this computer sees it: the account it acts as (the paired
/// owner) and path-signed GETs. Tests replace it (the network is the edge).
#[async_trait::async_trait]
pub trait SkillsCloud: Send + Sync {
    fn owner(&self) -> Option<String>;
    /// `(status, body)`; `Err` = not sent.
    async fn get(&self, path_and_query: &str) -> Result<(u16, Vec<u8>), String>;
}

/// Production: [`crate::account_sync::SignedAccountCloud`] (signature over
/// `GET` + the path without its query, empty body, `x-allternit-runtime-id`).
struct PairedCloud;

#[async_trait::async_trait]
impl SkillsCloud for PairedCloud {
    fn owner(&self) -> Option<String> {
        crate::relay_auth::process_secret().paired_owner()
    }
    async fn get(&self, path_and_query: &str) -> Result<(u16, Vec<u8>), String> {
        use crate::account_sync::AccountCloud as _;
        crate::account_sync::SignedAccountCloud.send("GET", path_and_query, Vec::new()).await
    }
}

fn test_clouds() -> &'static Mutex<HashMap<String, Arc<dyn SkillsCloud>>> {
    static CLOUDS: OnceLock<Mutex<HashMap<String, Arc<dyn SkillsCloud>>>> = OnceLock::new();
    CLOUDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cloud_for(state: &AppState) -> Arc<dyn SkillsCloud> {
    let found = test_clouds().lock().ok().and_then(|m| m.get(&db_key(state)).cloned());
    found.unwrap_or_else(|| Arc::new(PairedCloud))
}

/// Tests: the cloud this state's database talks to.
#[cfg(test)]
pub(crate) fn set_test_cloud(state: &AppState, cloud: Arc<dyn SkillsCloud>) {
    test_clouds().lock().unwrap().insert(db_key(state), cloud);
}

/// The cloud only acts as the computer's owner, so only the owner gets its skills.
fn cloud_for_user(state: &AppState, user: &str) -> Option<Arc<dyn SkillsCloud>> {
    let cloud = cloud_for(state);
    (cloud.owner().as_deref().map(str::trim).filter(|o| !o.is_empty()) == Some(user)).then_some(cloud)
}

/// The user's cloud skills: cached for [`CATALOG_TTL`]; unpaired, unreachable
/// or a bad answer → none (and nothing cached).
async fn cloud_refresh(state: &AppState, user: &str) -> Vec<CloudSkill> {
    let key = (db_key(state), user.to_string());
    let hit = cache().lock().ok().and_then(|c| c.cloud.get(&key).filter(|(at, _)| at.elapsed() < CATALOG_TTL).map(|(_, l)| l.clone()));
    if let Some(list) = hit {
        return list;
    }
    let listed = match cloud_for_user(state, user) {
        Some(cloud) => match tokio::time::timeout(PER_CONNECTOR_TIMEOUT, cloud.get(CLOUD_AVAILABLE_PATH)).await {
            Ok(Ok((200, body))) => serde_json::from_slice::<Vec<CloudSkill>>(&body).ok(),
            Ok(Ok((status, _))) => {
                tracing::warn!(status, "mcp skills: cloud skills unavailable");
                None
            }
            _ => None,
        },
        None => None,
    };
    if let Ok(mut c) = cache().lock() {
        match &listed {
            Some(list) => {
                c.cloud.insert(key, (Instant::now(), list.clone()));
            }
            None => {
                c.cloud.remove(&key);
            }
        }
    }
    listed.unwrap_or_default()
}

/// `cloud:<name>` (user), `org:<name>`, `app-<slug>:<name>`; other sources are skipped.
fn cloud_base_id(s: &CloudSkill) -> Option<String> {
    let part = skill_name_from_uri(&s.uri).unwrap_or_else(|| sanitize(&s.name));
    let part = if part.is_empty() { "skill".to_string() } else { part };
    match s.source.as_str() {
        SOURCE_USER => Some(format!("cloud:{part}")),
        SOURCE_ORG => Some(format!("org:{part}")),
        SOURCE_APP => {
            let slug = s.uri.strip_prefix("skill://app/")?.split('/').next().filter(|x| !x.is_empty())?;
            let slug = urlencoding::decode(slug).map(|d| d.into_owned()).unwrap_or_else(|_| slug.to_string());
            Some(format!("app-{}:{part}", sanitize(&slug)))
        }
        _ => None,
    }
}

/// Append the cloud skills after the server skills (same collision rule).
pub fn merge_cloud(items: &mut Vec<SkillItem>, cloud: &[CloudSkill], prefs: &HashMap<String, bool>) {
    let mut seen: HashSet<String> = items.iter().map(|s| s.id.clone()).collect();
    for s in cloud {
        let Some(base) = cloud_base_id(s) else { continue };
        let mut id = base.clone();
        let mut n = 2;
        while !seen.insert(id.clone()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let from = match s.source.as_str() {
            SOURCE_USER => "You".to_string(),
            // cloud-api returns an org id, not a name.
            SOURCE_ORG => "Your organization".to_string(),
            _ if !s.from.trim().is_empty() => s.from.trim().to_string(),
            _ => "an app".to_string(),
        };
        items.push(SkillItem {
            enabled: prefs.get(&id).copied().unwrap_or(s.default_enabled),
            default_enabled: s.default_enabled,
            id,
            source: s.source.clone(),
            server: from.clone(),
            from,
            connector_id: String::new(),
            uri: s.uri.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
        });
    }
}

// ---------------------------------------------------------------- load_skill

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    NotFound,
    Disabled,
    Unavailable,
    SignIn,
    TooLarge,
    Timeout,
}

impl LoadError {
    pub fn code(self) -> &'static str {
        match self {
            LoadError::NotFound => "skill_not_found",
            LoadError::Disabled => "skill_disabled",
            LoadError::Unavailable => "connector_unavailable",
            LoadError::SignIn => "connector_sign_in_required",
            LoadError::TooLarge => "skill_too_large",
            LoadError::Timeout => "skill_read_timeout",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            LoadError::NotFound => "There is no skill with that id in your list.",
            LoadError::Disabled => "That skill is turned off in Settings.",
            LoadError::Unavailable => "The server or app that has this skill could not be reached.",
            LoadError::SignIn => "Sign in to the server that has this skill again.",
            LoadError::TooLarge => "That skill is larger than 64 KB.",
            LoadError::Timeout => "Reading that skill took longer than 8 seconds.",
        }
    }
}

/// An id the caller picked, from their current catalog only (a URI is never an id).
pub fn pick(state: &AppState, user: &str, id: &str) -> Result<SkillItem, LoadError> {
    if id.is_empty() || id.contains("://") {
        return Err(LoadError::NotFound);
    }
    match find_cached(state, user, id) {
        None => Err(LoadError::NotFound),
        Some(s) if !s.enabled => Err(LoadError::Disabled),
        Some(s) => Ok(s),
    }
}

async fn read_unbounded(state: &Arc<AppState>, user: &str, item: &SkillItem, allow_private: bool) -> Result<String, LoadError> {
    if item.source == SOURCE_SERVER {
        let connectors = mcp_apps::load_user_connectors(state, user, allow_private).await;
        let Some(conn) = connectors.iter().find(|c| c.id == item.connector_id) else { return Err(LoadError::Unavailable) };
        if conn.auth_failed() {
            return Err(LoadError::SignIn);
        }
        let client = match mcp_apps::open_session(conn, allow_private).await {
            Ok(c) => c,
            Err(e) if e.code == "connector_unauthorized" => return Err(LoadError::SignIn),
            Err(_) => return Err(LoadError::Unavailable),
        };
        let text = client.read_skill(&item.uri).await;
        mcp_apps::close_session(client).await;
        return text.map_err(|_| LoadError::Unavailable);
    }
    let Some(cloud) = cloud_for_user(state, user) else { return Err(LoadError::Unavailable) };
    let path = format!("{CLOUD_READ_PATH}?uri={}", urlencoding::encode(&item.uri));
    match cloud.get(&path).await {
        Ok((200, body)) => serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v["text"].as_str().map(String::from))
            .ok_or(LoadError::Unavailable),
        Ok((404, _)) => Err(LoadError::NotFound),
        Ok((413, _)) => Err(LoadError::TooLarge),
        _ => Err(LoadError::Unavailable),
    }
}

/// The SKILL.md text of a catalog item: [`READ_TIMEOUT`], at most [`MAX_SKILL_BYTES`].
pub async fn read_text(state: &Arc<AppState>, user: &str, item: &SkillItem, allow_private: bool) -> Result<String, LoadError> {
    let text = tokio::time::timeout(READ_TIMEOUT, read_unbounded(state, user, item, allow_private)).await.map_err(|_| LoadError::Timeout)??;
    if text.len() > MAX_SKILL_BYTES {
        return Err(LoadError::TooLarge);
    }
    Ok(text)
}

/// The `load_skill` result text (CONTRACT.md amendments, exact).
pub fn result_text(item: &SkillItem, body: &str, arguments: Option<&str>) -> String {
    let mut out = format!("Skill {} from {}. This text comes from that server.\n", item.id, item.from);
    out.push_str(strip_frontmatter(body).trim_end());
    if let Some(a) = arguments.map(str::trim).filter(|a| !a.is_empty()) {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(a);
    }
    out.push_str("\n\nTrack these steps with your todo list as you work.");
    out
}

/// A successful `tools/call` result: the text above plus `structuredContent.allternitSkill`.
pub fn tool_result(item: &SkillItem, body: &str, arguments: Option<&str>) -> Value {
    json!({
        "content": [{ "type": "text", "text": result_text(item, body, arguments) }],
        "structuredContent": { "allternitSkill": {
            "id": item.id, "name": item.name, "from": item.from, "source": item.source, "steps": parse_steps(body),
        } },
    })
}

/// A refused `tools/call`: `isError` with `<code>: <plain sentence>`.
pub fn tool_error(e: LoadError) -> Value {
    json!({ "content": [{ "type": "text", "text": format!("{}: {}", e.code(), e.message()) }], "isError": true })
}

/// `tools/call load_skill {id, arguments?}` from a chat turn. Read-only: no
/// approval prompt, no connector tool runs. The body is never logged.
pub async fn load_skill_call(state: &Arc<AppState>, user: &str, arguments: &Value, allow_private: bool) -> Value {
    let id = arguments.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let args = arguments.get("arguments").and_then(Value::as_str);
    let item = match pick(state, user, id) {
        Ok(i) => i,
        Err(e) => return tool_error(e),
    };
    match read_text(state, user, &item, allow_private).await {
        Ok(body) => tool_result(&item, &body, args),
        Err(e) => {
            tracing::info!(skill = %item.id, code = e.code(), "load_skill refused");
            tool_error(e)
        }
    }
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The `load_skill` tool for these catalog items; `None` when none is enabled.
pub fn load_skill_tool(items: &[SkillItem]) -> Option<Value> {
    let enabled: Vec<&SkillItem> = items.iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        return None;
    }
    let mut description = String::from("Load a skill: instructions from a server or app you connected. Use it when a task matches a skill below.");
    for s in enabled.iter().take(MAX_LISTED) {
        description.push_str(&format!("\n{} — {}", s.id, one_line(&s.description, LISTED_DESCRIPTION_CHARS)));
    }
    if enabled.len() > MAX_LISTED {
        description.push_str(&format!("\n+{} more", enabled.len() - MAX_LISTED));
    }
    let meta: Vec<Value> = enabled.iter().map(|s| json!({ "id": s.id, "description": s.description, "from": s.from, "source": s.source })).collect();
    let mut tool_meta = serde_json::Map::new();
    tool_meta.insert(SKILLS_META_KEY.into(), Value::Array(meta));
    // Always written: loading is a read and never asks for approval.
    tool_meta.insert(crate::mcp_user_proxy::REQUIRES_CONFIRMATION_META_KEY.into(), Value::Bool(false));
    Some(json!({
        "name": LOAD_SKILL_TOOL,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "A skill id from the list." },
                "arguments": { "type": "string", "description": "Optional text for the skill to work on." },
            },
            "required": ["id"],
        },
        "annotations": { "readOnlyHint": true },
        "_meta": tool_meta,
    }))
}

/// The proxy's `load_skill` for a chat turn, from memory only. An empty catalog
/// starts a background refresh and lists nothing this turn (never blocks).
pub fn turn_tool(state: &Arc<AppState>, user: &str, allow_private: bool) -> Option<Value> {
    let items = catalog_cached(state, user);
    if items.is_empty() {
        // At most one background refresh per user per minute (a user with no skills
        // has an empty catalog every turn). Not in unit tests: a stray refresh would
        // reach the fake connectors other tests count calls on.
        let key = (db_key(state), user.to_string());
        let due = cache().lock().map(|mut c| {
            let recent = c.kicked.get(&key).is_some_and(|at| at.elapsed() < Duration::from_secs(60));
            if !recent {
                c.kicked.insert(key, Instant::now());
            }
            !recent
        });
        if cfg!(test) || !due.unwrap_or(false) {
            return None;
        }
        let (state, user) = (state.clone(), user.to_string());
        tokio::spawn(async move {
            let mut ctx = Ctx::production();
            ctx.allow_private = allow_private;
            catalog_refresh(&state, &ctx, &user).await;
        });
        return None;
    }
    load_skill_tool(&items)
}

// ---------------------------------------------------------------- `/` pick on a turn

/// The one-turn directive for a picked skill; only our words, the validated id
/// and the user's own arguments (quotes escaped, one line, clipped).
pub fn directive(id: &str, arguments: Option<&str>) -> String {
    let q = |s: &str| one_line(s, 2000).replace('\\', "\\\\").replace('"', "\\\"");
    match arguments.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => format!("The user picked the skill \"{}\". Call load_skill with id \"{}\", arguments \"{}\" before anything else.", q(id), q(id), q(a)),
        None => format!("The user picked the skill \"{}\". Call load_skill with id \"{}\" before anything else.", q(id), q(id)),
    }
}

/// `skill: {id, arguments?}` on a chat-turn POST body. `Ok(None)` = no pick;
/// `Err(code)` = 422 (`skill_not_found` | `skill_disabled`). A catalog not yet
/// built in this process is refreshed once (the user asked for this skill).
pub async fn turn_directive(state: &Arc<AppState>, user: &str, body: &Value) -> Result<Option<String>, &'static str> {
    let Some(picked) = body.get("skill").filter(|v| !v.is_null()) else { return Ok(None) };
    let id = picked.get("id").and_then(Value::as_str).unwrap_or("").trim();
    let arguments = picked.get("arguments").and_then(Value::as_str);
    if catalog_cached(state, user).is_empty() {
        catalog_refresh(state, &Ctx::production(), user).await;
    }
    let item = pick(state, user, id).map_err(LoadError::code)?;
    Ok(Some(directive(&item.id, arguments)))
}

/// Tests: put a user's server catalog in memory as if a refresh had just run.
#[cfg(test)]
pub(crate) fn seed_for_test(state: &AppState, user: &str, conns: Vec<ConnMeta>, skills: Vec<(&str, Vec<SkillInfo>)>, prefs: HashMap<String, bool>) {
    let db = db_key(state);
    let mut c = cache().lock().unwrap();
    for (cid, list) in skills {
        c.skills.insert((db.clone(), user.to_string(), cid.to_string()), (Instant::now(), list));
    }
    c.users.insert((db, user.to_string()), UserSnapshot { conns, prefs });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(uri: &str, name: &str) -> SkillInfo {
        SkillInfo { uri: uri.into(), name: name.into(), description: format!("{name} desc") }
    }

    #[test]
    fn skill_part_follows_gizzi_rule() {
        assert_eq!(skill_name_from_uri("skill://review-pr/SKILL.md").as_deref(), Some("review-pr"));
        assert_eq!(skill_name_from_uri("skill://team/Weekly%20Report/skill.md").as_deref(), Some("Weekly_Report"));
        assert_eq!(skill_name_from_uri("skill://notes.md").as_deref(), Some("notes"));
        assert_eq!(skill_name_from_uri("skill://a/b.c/SKILL.md").as_deref(), Some("b_c"));
        assert_eq!(skill_name_from_uri("https://x/SKILL.md"), None);
        assert_eq!(skill_name_from_uri("skill://"), None);
    }

    #[test]
    fn ids_use_the_proxy_tool_prefix_and_number_collisions() {
        let connectors = vec![
            Connector::for_test("c-gh", "GitHub", "https://a.test/mcp", None),
            Connector::for_test("c-gh2", "github", "https://b.test/mcp", None),
        ];
        let conns = conn_metas(&connectors);
        // Same prefixes the proxy gives these connectors' tools.
        assert_eq!(conns.iter().map(|m| m.prefix.as_str()).collect::<Vec<_>>(), crate::mcp_user_proxy::connector_prefixes(&connectors));
        let mut skills = HashMap::new();
        skills.insert(
            "c-gh".to_string(),
            vec![skill("skill://review-pr/SKILL.md", "Review PR"), skill("skill://other/review-pr/SKILL.md", "Review PR v2"), skill("skill://x/review-pr/SKILL.md", "third")],
        );
        skills.insert("c-gh2".to_string(), vec![skill("skill://review-pr/SKILL.md", "Review PR")]);
        let items = assemble(&conns, &skills, &HashMap::new());
        let ids: Vec<&str> = items.iter().map(|s| s.id.as_str()).collect();
        let p0 = &conns[0].prefix;
        let p1 = &conns[1].prefix;
        assert_ne!(p0, p1);
        assert_eq!(ids, vec![format!("{p0}:review-pr"), format!("{p0}:review-pr-2"), format!("{p0}:review-pr-3"), format!("{p1}:review-pr")]);
        assert!(items.iter().all(|s| s.source == "server" && s.default_enabled && s.enabled));
        assert_eq!(items[3].connector_id, "c-gh2");
        assert_eq!(items[0].from, "GitHub");
    }

    #[test]
    fn a_pref_row_overrides_the_default() {
        let conns = vec![ConnMeta { id: "c1".into(), name: "Nimbrook".into(), prefix: "nimbrook".into() }];
        let mut skills = HashMap::new();
        skills.insert("c1".to_string(), vec![skill("skill://a/SKILL.md", "a"), skill("skill://b/SKILL.md", "b"), skill("skill://c/SKILL.md", "c")]);
        let mut prefs = HashMap::new();
        prefs.insert("nimbrook:a".to_string(), false);
        prefs.insert("nimbrook:c".to_string(), true);
        prefs.insert("nimbrook:gone".to_string(), false);
        let items = assemble(&conns, &skills, &prefs);
        let enabled: Vec<(&str, bool, bool)> = items.iter().map(|s| (s.id.as_str(), s.enabled, s.default_enabled)).collect();
        assert_eq!(enabled, vec![("nimbrook:a", false, true), ("nimbrook:b", true, true), ("nimbrook:c", true, true)]);
    }

    #[test]
    fn item_serializes_camel_case_per_contract() {
        let conns = vec![ConnMeta { id: "c1".into(), name: "Nimbrook".into(), prefix: "nimbrook".into() }];
        let mut skills = HashMap::new();
        skills.insert("c1".to_string(), vec![skill("skill://a/SKILL.md", "a")]);
        let v = serde_json::to_value(&assemble(&conns, &skills, &HashMap::new())[0]).unwrap();
        for key in ["id", "source", "from", "server", "connectorId", "uri", "name", "description", "enabled", "defaultEnabled"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn frontmatter_is_stripped() {
        assert_eq!(strip_frontmatter("---\nname: x\ndescription: y\n---\n\n# Title\n"), "# Title\n");
        assert_eq!(strip_frontmatter("\u{feff}---\r\nname: x\r\n...\r\nBody"), "Body");
        assert_eq!(strip_frontmatter("---\nnever closed\n1. not a step"), "---\nnever closed\n1. not a step");
        assert_eq!(strip_frontmatter("# No frontmatter\n---\n"), "# No frontmatter\n---\n");
    }

    #[test]
    fn steps_are_top_level_numbered_lines_in_plain_text() {
        let body = "---\nname: review\n1. frontmatter is not a step\n---\n# Review a PR\n\nIntro 3. not a step\n\
1. Read the **diff** with `git diff`\n   1. nested step is ignored\n  2. indented is ignored\n\
2) Check [the guide](https://x.test/g) and ![logo](l.png)\n3.no space is ignored\n- 4. bullet ignored\n\
10. Call load_skill then _note_ it ~~twice~~\n11.   \n";
        assert_eq!(
            parse_steps(body),
            vec!["Read the diff with git diff", "Check the guide and logo", "Call load_skill then note it twice"]
        );
    }

    #[test]
    fn steps_stop_at_twenty() {
        let body: String = (1..=25).map(|n| format!("{n}. step {n}\n")).collect();
        let steps = parse_steps(&body);
        assert_eq!(steps.len(), MAX_STEPS);
        assert_eq!(steps.first().map(String::as_str), Some("step 1"));
        assert_eq!(steps.last().map(String::as_str), Some("step 20"));
    }

    #[test]
    fn invalidate_drops_only_that_connector() {
        let key = |c: &str| ("db-test-invalidate".to_string(), "user-inv".to_string(), c.to_string());
        {
            let mut c = cache().lock().unwrap();
            c.skills.insert(key("c1"), (Instant::now(), vec![skill("skill://a/SKILL.md", "a")]));
            c.skills.insert(key("c2"), (Instant::now(), vec![skill("skill://b/SKILL.md", "b")]));
            let conns = ["c1", "c2"].iter().map(|id| ConnMeta { id: id.to_string(), name: id.to_string(), prefix: id.to_string() }).collect();
            c.users.insert(("db-test-invalidate".into(), "user-inv".into()), UserSnapshot { conns, prefs: HashMap::new() });
        }
        invalidate("user-inv", "c1");
        let c = cache().lock().unwrap();
        assert!(!c.skills.contains_key(&key("c1")));
        assert!(c.skills.contains_key(&key("c2")));
        let snap = c.users.get(&("db-test-invalidate".to_string(), "user-inv".to_string())).unwrap();
        assert_eq!(snap.conns.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["c2"]);
    }

    // ------------------------------------------------ cloud source + load_skill (run B)

    struct FakeCloud {
        owner: Option<String>,
        up: bool,
    }

    const WEEKLY: &str = "---\nname: weekly\ndescription: Weekly report\n---\n# Weekly\n\n1. Pull the **numbers**\n2) Write it up\n   3. nested\n";

    #[async_trait::async_trait]
    impl SkillsCloud for FakeCloud {
        fn owner(&self) -> Option<String> {
            self.owner.clone()
        }
        async fn get(&self, path: &str) -> Result<(u16, Vec<u8>), String> {
            if !self.up {
                return Err("connection refused".into());
            }
            if path == CLOUD_AVAILABLE_PATH {
                let list = json!([
                    { "source": "user", "uri": "skill://user/weekly/SKILL.md", "name": "weekly", "description": "Weekly report", "from": "You", "defaultEnabled": true },
                    { "source": "org", "uri": "skill://org/org_9/handbook/SKILL.md", "name": "handbook", "description": "Team handbook", "from": "org_9", "defaultEnabled": true },
                    { "source": "app", "uri": "skill://app/tips/tip-of-day/SKILL.md", "name": "tip-of-day", "description": "A tip", "from": "Tips", "defaultEnabled": false },
                ]);
                return Ok((200, list.to_string().into_bytes()));
            }
            let uri = path.strip_prefix("/api/v1/skills/read?uri=").map(|q| urlencoding::decode(q).unwrap().into_owned());
            match uri.as_deref() {
                Some("skill://user/weekly/SKILL.md") => Ok((200, json!({ "text": WEEKLY }).to_string().into_bytes())),
                Some("skill://org/org_9/handbook/SKILL.md") => Ok((413, b"{}".to_vec())),
                _ => Ok((404, b"{}".to_vec())),
            }
        }
    }

    fn seed_server(state: &AppState, user: &str) {
        let conns = vec![ConnMeta { id: "conn-1".into(), name: "Nimbrook".into(), prefix: "dash-server".into() }];
        seed_for_test(state, user, conns, vec![("conn-1", vec![skill("skill://review/SKILL.md", "review")])], HashMap::new());
    }

    #[test]
    fn cloud_ids_from_and_defaults() {
        let cloud: Vec<CloudSkill> = serde_json::from_value(json!([
            { "source": "user", "uri": "skill://user/weekly/SKILL.md", "name": "weekly" },
            { "source": "org", "uri": "skill://org/o1/weekly/SKILL.md", "name": "weekly", "from": "o1" },
            { "source": "app", "uri": "skill://app/tips/weekly/SKILL.md", "name": "weekly", "from": "Tips", "defaultEnabled": false },
            { "source": "app", "uri": "skill://app/tips/weekly/SKILL.md", "name": "weekly", "from": "Tips", "defaultEnabled": false },
            { "source": "mystery", "uri": "skill://x/SKILL.md", "name": "x" },
        ]))
        .unwrap();
        let mut items = Vec::new();
        let mut prefs = HashMap::new();
        prefs.insert("app-tips:weekly".to_string(), true);
        merge_cloud(&mut items, &cloud, &prefs);
        let got: Vec<(&str, &str, &str, bool)> = items.iter().map(|s| (s.id.as_str(), s.source.as_str(), s.from.as_str(), s.enabled)).collect();
        assert_eq!(
            got,
            vec![
                ("cloud:weekly", "user", "You", true),
                ("org:weekly", "org", "Your organization", true),
                ("app-tips:weekly", "app", "Tips", true),
                ("app-tips:weekly-2", "app", "Tips", false),
            ]
        );
        assert!(items.iter().all(|s| s.connector_id.is_empty()));
    }

    #[test]
    fn result_text_is_exact_and_steps_parse() {
        let item = SkillItem {
            id: "cloud:weekly".into(),
            source: "user".into(),
            from: "You".into(),
            server: "You".into(),
            connector_id: String::new(),
            uri: "skill://user/weekly/SKILL.md".into(),
            name: "weekly".into(),
            description: "Weekly report".into(),
            enabled: true,
            default_enabled: true,
        };
        let text = result_text(&item, WEEKLY, Some("  last week  "));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Skill cloud:weekly from You. This text comes from that server.");
        assert_eq!(lines[1], "# Weekly");
        assert!(!text.contains("description: Weekly report"), "frontmatter is stripped");
        assert!(text.contains("\n\nARGUMENTS: last week\n"));
        assert_eq!(*lines.last().unwrap(), "Track these steps with your todo list as you work.");
        let without = result_text(&item, WEEKLY, None);
        assert!(!without.contains("ARGUMENTS"));
        assert_eq!(without.lines().last(), Some("Track these steps with your todo list as you work."));
        let v = tool_result(&item, WEEKLY, None);
        assert_eq!(v["structuredContent"]["allternitSkill"]["steps"], json!(["Pull the numbers", "Write it up"]));
        assert_eq!(v["structuredContent"]["allternitSkill"]["from"], "You");
        let e = tool_error(LoadError::Disabled);
        assert_eq!(e["isError"], true);
        assert!(e["content"][0]["text"].as_str().unwrap().starts_with("skill_disabled: "));
    }

    #[test]
    fn tool_lists_enabled_skills_only_and_caps_at_fifty() {
        let conns = vec![ConnMeta { id: "c1".into(), name: "Nimbrook".into(), prefix: "nimbrook".into() }];
        let mut skills = HashMap::new();
        skills.insert("c1".to_string(), (0..55).map(|n| skill(&format!("skill://s{n}/SKILL.md"), &format!("s{n}"))).collect::<Vec<_>>());
        let mut prefs = HashMap::new();
        prefs.insert("nimbrook:s0".to_string(), false);
        let items = assemble(&conns, &skills, &prefs);
        let tool = load_skill_tool(&items).unwrap();
        let d = tool["description"].as_str().unwrap();
        assert!(d.starts_with("Load a skill: instructions from a server or app you connected. Use it when a task matches a skill below.\n"));
        assert!(!d.contains("nimbrook:s0 "));
        assert!(d.contains("\nnimbrook:s1 — s1 desc"));
        assert!(d.ends_with("\n+4 more"));
        assert_eq!(tool["_meta"][SKILLS_META_KEY].as_array().unwrap().len(), 54);
        assert_eq!(tool["_meta"][SKILLS_META_KEY][0], json!({ "id": "nimbrook:s1", "description": "s1 desc", "from": "Nimbrook", "source": "server" }));
        let all_off: HashMap<String, bool> = items.iter().map(|s| (s.id.clone(), false)).collect();
        assert!(load_skill_tool(&assemble(&conns, &skills, &all_off)).is_none());
    }

    #[test]
    fn directive_escapes_and_names_the_id() {
        assert_eq!(directive("cloud:weekly", None), "The user picked the skill \"cloud:weekly\". Call load_skill with id \"cloud:weekly\" before anything else.");
        assert_eq!(
            directive("a:b", Some("say \"hi\"\nnow")),
            "The user picked the skill \"a:b\". Call load_skill with id \"a:b\", arguments \"say \\\"hi\\\" now\" before anything else."
        );
    }

    #[tokio::test]
    async fn cloud_down_still_lists_server_skills() {
        let (state, _server, _) = crate::mcp_apps::tests::fixture().await;
        set_test_cloud(&state, Arc::new(FakeCloud { owner: Some("user-1".into()), up: false }));
        seed_server(&state, "user-1");
        let mut ctx = Ctx::production();
        ctx.allow_private = true;
        let items = catalog_refresh(&state, &ctx, "user-1").await;
        assert!(items.iter().any(|s| s.source == "server"), "{items:?}");
        assert!(items.iter().all(|s| s.source == "server"));
    }

    #[tokio::test]
    async fn cloud_skills_are_listed_loaded_and_refused_in_plain_codes() {
        let (state, _server, _) = crate::mcp_apps::tests::fixture().await;
        set_test_cloud(&state, Arc::new(FakeCloud { owner: Some("user-1".into()), up: true }));
        let mut ctx = Ctx::production();
        ctx.allow_private = true;
        let ids: Vec<String> = catalog_refresh(&state, &ctx, "user-1").await.into_iter().map(|s| s.id).collect();
        for id in ["cloud:weekly", "org:handbook", "app-tips:tip-of-day"] {
            assert!(ids.iter().any(|i| i == id), "{id} missing from {ids:?}");
        }
        // Only the paired owner gets the cloud's skills.
        assert!(catalog_refresh(&state, &ctx, "user-2").await.iter().all(|s| s.source == "server"));

        let ok = load_skill_call(&state, "user-1", &json!({ "id": "cloud:weekly", "arguments": "Q3" }), true).await;
        let text = ok["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Skill cloud:weekly from You. This text comes from that server.\n# Weekly"), "{text}");
        assert!(text.ends_with("ARGUMENTS: Q3\n\nTrack these steps with your todo list as you work."));
        assert_eq!(ok["structuredContent"]["allternitSkill"]["steps"], json!(["Pull the numbers", "Write it up"]));

        let code = |v: Value| v["content"][0]["text"].as_str().unwrap().split(':').next().unwrap().to_string();
        assert_eq!(code(load_skill_call(&state, "user-1", &json!({ "id": "org:handbook" }), true).await), "skill_too_large");
        // App installed by share link: off by default.
        assert_eq!(code(load_skill_call(&state, "user-1", &json!({ "id": "app-tips:tip-of-day" }), true).await), "skill_disabled");
        assert_eq!(code(load_skill_call(&state, "user-1", &json!({ "id": "cloud:nope" }), true).await), "skill_not_found");
        assert_eq!(code(load_skill_call(&state, "user-1", &json!({ "id": "skill://user/weekly/SKILL.md" }), true).await), "skill_not_found");

        // `/` pick on a turn body.
        assert_eq!(turn_directive(&state, "user-1", &json!({})).await, Ok(None));
        assert_eq!(turn_directive(&state, "user-1", &json!({ "skill": { "id": "cloud:nope" } })).await, Err("skill_not_found"));
        assert_eq!(turn_directive(&state, "user-1", &json!({ "skill": { "id": "app-tips:tip-of-day" } })).await, Err("skill_disabled"));
        assert!(turn_directive(&state, "user-1", &json!({ "skill": { "id": "cloud:weekly" } })).await.unwrap().unwrap().contains("id \"cloud:weekly\""));
    }
}
