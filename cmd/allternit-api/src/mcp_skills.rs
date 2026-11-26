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
//! * `source` is always `server` here; app/user/org skills come from the cloud.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mcp_client::SkillInfo;
use rusqlite::params;
use serde::Serialize;

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
    assemble(&snap.conns, &skills, &snap.prefs)
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
    let fetched = futures::future::join_all(stale.iter().map(|k| async move { (k.id.clone(), fetch_connector(k, ctx.allow_private).await) })).await;

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
    assemble(&conns, &skills, &prefs)
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
}
