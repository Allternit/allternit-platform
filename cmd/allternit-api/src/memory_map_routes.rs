//! `GET /memory-map?bot=<id>` - one graph of what a bot (or the whole
//! account) knows and uses: skills, memories, threads, routines, souls and
//! entities, with only the links the data really contains. Same data plane as
//! `/cowork/memory` (allternit-api SQLite).
//!
//! Link direction: `from` depends on `to`. skill_use rows give
//! thread -> skill and routine -> skill ("uses"); a memory whose `source`
//! names a thread gives memory -> thread ("learned_from"); `memory_edges`
//! rows keep their stored source -> target direction.

use axum::{
    extract::{Extension, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::agent_workspace_paths::workspace_dir_for;
use crate::agent_workspace_routes::authorize_agent;
use crate::auth::AuthUser;
use crate::AppState;

const CAP: usize = 500;

pub fn memory_map_router() -> Router<Arc<AppState>> {
    Router::new().route("/memory-map", get(memory_map))
}

#[derive(Deserialize)]
struct MapQuery {
    bot: Option<String>,
}

async fn memory_map(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<MapQuery>,
) -> impl IntoResponse {
    let bot = q.bot.filter(|b| !b.trim().is_empty());
    if let Some(b) = &bot {
        if let Some(resp) = authorize_agent(&state.db, user.user_id.clone(), b.clone()).await {
            return resp;
        }
    }
    let db = state.db.clone();
    let user_id = user.user_id;
    let built = tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| e.to_string())?;
        Ok::<_, String>(build(&conn, &user_id, bot.as_deref()))
    })
    .await;
    match built {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal error"})),
        )
            .into_response(),
    }
}

fn rows<T>(conn: &Connection, sql: &str, p: &[&dyn rusqlite::ToSql], f: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>) -> Vec<T> {
    // A missing table (feature never used on this install) is an empty source.
    conn.prepare(sql)
        .and_then(|mut s| s.query_map(p, |r| f(r))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default()
}

struct Item {
    id: String,
    kind: &'static str,
    label: String,
    uses: Option<i64>,
    last_at: Option<String>,
    r#ref: String,
}

fn clip(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}...", one.chars().take(n).collect::<String>())
    }
}

pub(crate) fn build(conn: &Connection, user_id: &str, bot: Option<&str>) -> Value {
    // Bots in scope (always the caller's own).
    let bots: Vec<(String, String)> = match bot {
        Some(b) => rows(conn, "SELECT id, name FROM agents WHERE id = ?1 AND user_id = ?2", &[&b, &user_id], |r| Ok((r.get(0)?, r.get(1)?))),
        None => rows(conn, "SELECT id, name FROM agents WHERE user_id = ?1", &[&user_id], |r| Ok((r.get(0)?, r.get(1)?))),
    };
    let bot_ids: HashSet<String> = bots.iter().map(|b| b.0.clone()).collect();
    let mut items: Vec<Item> = Vec::new();
    let mut links: Vec<Value> = Vec::new();
    let mut truncated = false;

    // Skills: every skill a bot in scope ran, with counts.
    let use_rows: Vec<(String, String, String, String)> = rows(
        conn,
        "SELECT agent_id, skill, session_id, used_at FROM skill_use WHERE user_id = ?1 ORDER BY used_at DESC LIMIT 20000",
        &[&user_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .into_iter()
    .filter(|u| bot_ids.contains(&u.0))
    .collect();
    let mut skill_stats: BTreeMap<String, (i64, String)> = BTreeMap::new();
    for (_, skill, _, at) in &use_rows {
        let e = skill_stats.entry(skill.clone()).or_insert((0, at.clone()));
        e.0 += 1;
        if *at > e.1 {
            e.1 = at.clone();
        }
    }
    for (skill, (n, at)) in &skill_stats {
        items.push(Item { id: format!("skill:{skill}"), kind: "skill", label: skill.clone(), uses: Some(*n), last_at: Some(at.clone()), r#ref: skill.clone() });
    }

    // Threads (latest 60) and the sessions they run in.
    let thread_rows: Vec<(String, String, String, Option<String>)> = rows(
        conn,
        "SELECT id, title, last_activity_at, current_session_id FROM bot_threads WHERE user_id = ?1 AND (?2 IS NULL OR bot_id = ?2) ORDER BY last_activity_at DESC LIMIT 61",
        &[&user_id, &bot],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    );
    if thread_rows.len() > 60 {
        truncated = true;
    }
    let mut session_thread: HashMap<String, String> = HashMap::new();
    let mut thread_ids: HashSet<String> = HashSet::new();
    for (id, title, at, session) in thread_rows.into_iter().take(60) {
        if let Some(s) = session {
            session_thread.insert(s, id.clone());
        }
        thread_ids.insert(id.clone());
        items.push(Item { id: format!("thread:{id}"), kind: "thread", label: clip(&title, 60), uses: None, last_at: Some(at), r#ref: id });
    }
    let mut seen_links: HashSet<(String, String)> = HashSet::new();
    let mut thread_skill_uses: HashMap<String, i64> = HashMap::new();
    for (_, skill, session, _) in &use_rows {
        if let Some(t) = session_thread.get(session) {
            *thread_skill_uses.entry(t.clone()).or_default() += 1;
            if seen_links.insert((t.clone(), skill.clone())) {
                links.push(json!({"from": format!("thread:{t}"), "to": format!("skill:{skill}"), "kind": "uses"}));
            }
        }
    }
    for it in items.iter_mut().filter(|i| i.kind == "thread") {
        it.uses = thread_skill_uses.get(&it.r#ref).copied();
    }

    // Routines; a routine's thread (metadata.threadId) carries its skill use.
    let routine_rows: Vec<(String, String, Option<String>, Option<String>)> = rows(
        conn,
        "SELECT id, name, json_extract(metadata, '$.threadId'), updated_at FROM routines WHERE user_id = ?1 AND (?2 IS NULL OR agent_id = ?2) ORDER BY updated_at DESC LIMIT 61",
        &[&user_id, &bot],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    );
    if routine_rows.len() > 60 {
        truncated = true;
    }
    for (id, name, thread, at) in routine_rows.into_iter().take(60) {
        if let Some(t) = thread {
            let skills_of_thread: HashSet<&String> = use_rows
                .iter()
                .filter(|u| session_thread.get(&u.2) == Some(&t))
                .map(|u| &u.1)
                .collect();
            for s in skills_of_thread {
                links.push(json!({"from": format!("routine:{id}"), "to": format!("skill:{s}"), "kind": "uses"}));
            }
        }
        items.push(Item { id: format!("routine:{id}"), kind: "routine", label: clip(&name, 60), uses: None, last_at: at, r#ref: id });
    }

    // Memories: the entries each bot owns (account: all of the user's).
    let mut mem_source: HashMap<String, String> = HashMap::new();
    let principals: Vec<Option<String>> = match bot {
        Some(b) => vec![crate::cowork_routes::bot_memory_principal(conn, user_id, b).ok().flatten()],
        None => vec![None],
    };
    for p in principals {
        let got = allternit_cowork_runtime::sqlite_store::search_memory_entries(conn, user_id, p.as_deref(), None, 201, 0).unwrap_or_default();
        if got.len() > 200 {
            truncated = true;
        }
        for m in got.into_iter().take(200) {
            if bot.is_some() && m["owner_principal"].as_str() != p.as_deref() {
                continue;
            }
            let id = m["id"].as_str().unwrap_or_default().to_string();
            if let Some(src) = m["source"].as_str() {
                mem_source.insert(id.clone(), src.to_string());
            }
            items.push(Item {
                id: format!("memory:{id}"),
                kind: "memory",
                label: clip(m["content"].as_str().unwrap_or(""), 80),
                uses: None,
                last_at: m["created_at"].as_str().map(str::to_string),
                r#ref: id,
            });
        }
    }
    // Facts (v2 memory kernel), current ones only.
    let facts: Vec<(String, String, Option<String>)> = rows(
        conn,
        "SELECT id, fact, valid_from FROM memory_facts WHERE user_id = ?1 AND valid_until IS NULL AND (?2 IS NULL OR agent_id = ?2) ORDER BY valid_from DESC LIMIT 151",
        &[&user_id, &bot],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    );
    if facts.len() > 150 {
        truncated = true;
    }
    for (id, fact, at) in facts.into_iter().take(150) {
        items.push(Item { id: format!("memory:fact:{id}"), kind: "memory", label: clip(&fact, 80), uses: None, last_at: at, r#ref: format!("fact:{id}") });
    }
    // learned_from: a memory whose source is a thread id or that thread's session.
    for (mid, src) in &mem_source {
        let thread = if thread_ids.contains(src) { Some(src.clone()) } else { session_thread.get(src).cloned() };
        if let Some(t) = thread {
            links.push(json!({"from": format!("memory:{mid}"), "to": format!("thread:{t}"), "kind": "learned_from"}));
        }
    }

    // Entities, then the stored edges between known ids.
    let entities: Vec<(String, String, Option<String>)> = rows(
        conn,
        "SELECT entity_id, name, last_updated FROM memory_entities WHERE user_id = ?1 AND (?2 IS NULL OR agent_id = ?2) ORDER BY last_updated DESC LIMIT 101",
        &[&user_id, &bot],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    );
    if entities.len() > 100 {
        truncated = true;
    }
    let mut entity_ids: HashSet<String> = HashSet::new();
    for (eid, name, at) in entities.into_iter().take(100) {
        entity_ids.insert(eid.clone());
        items.push(Item { id: format!("entity:{eid}"), kind: "entity", label: clip(&name, 60), uses: None, last_at: at, r#ref: eid });
    }
    let edge_rows: Vec<(String, String, String)> = rows(
        conn,
        "SELECT source, relationship, target FROM memory_edges WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 500",
        &[&user_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    );
    let by_raw: HashMap<String, String> = items
        .iter()
        .flat_map(|i| [(i.r#ref.clone(), i.id.clone()), (i.id.clone(), i.id.clone())])
        .collect();
    for (src, rel, dst) in edge_rows {
        let (Some(from), Some(to)) = (by_raw.get(&src), by_raw.get(&dst)) else { continue };
        if from == to {
            continue;
        }
        let rel = rel.to_lowercase();
        let kind = if rel.contains("contradict") {
            "contradicts"
        } else if rel.contains("updat") || rel.contains("supersed") {
            "updates"
        } else {
            "mentions"
        };
        links.push(json!({"from": from, "to": to, "kind": kind}));
    }

    // Souls: one per bot in scope that has a SOUL.md.
    for (id, name) in &bots {
        let path = workspace_dir_for(id).join("SOUL.md");
        if let Ok(meta) = std::fs::metadata(&path) {
            let at = meta.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339());
            items.push(Item { id: format!("soul:{id}"), kind: "soul", label: format!("{name} Soul"), uses: None, last_at: at, r#ref: id.clone() });
        }
    }

    if items.len() > CAP {
        truncated = true;
        items.truncate(CAP);
    }
    let kept: HashSet<&String> = items.iter().map(|i| &i.id).collect();
    let mut dedup: HashSet<String> = HashSet::new();
    let links: Vec<Value> = links
        .into_iter()
        .filter(|l| {
            kept.contains(&l["from"].as_str().unwrap_or_default().to_string())
                && kept.contains(&l["to"].as_str().unwrap_or_default().to_string())
                && dedup.insert(l.to_string())
        })
        .collect();
    json!({
        "items": items.iter().map(|i| json!({"id": i.id, "kind": i.kind, "label": i.label, "uses": i.uses, "last_at": i.last_at, "ref": i.r#ref})).collect::<Vec<_>>(),
        "links": links,
        "truncated": truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE agents (id TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL);
             CREATE TABLE skill_use (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT, agent_id TEXT, skill TEXT, session_id TEXT, call_id TEXT, used_at TEXT);
             CREATE TABLE bot_threads (id TEXT PRIMARY KEY, user_id TEXT, bot_id TEXT, title TEXT, last_activity_at TEXT, current_session_id TEXT);
             INSERT INTO agents VALUES ('b1','u1','One'), ('b2','u2','Two');
             INSERT INTO bot_threads VALUES ('t1','u1','b1','Research','2026-10-30T10:00:00Z','s1'), ('t2','u2','b2','Other','2026-10-30T10:00:00Z','s2');
             INSERT INTO skill_use (user_id,agent_id,skill,session_id,call_id,used_at) VALUES
               ('u1','b1','web-research','s1','c1','2026-10-30T10:01:00Z'),
               ('u1','b1','web-research','s1','c2','2026-10-30T10:02:00Z'),
               ('u2','b2','secret-skill','s2','c3','2026-10-30T10:03:00Z');",
        )
        .unwrap();
        c
    }

    #[test]
    fn thread_depends_on_the_skill_it_used_with_real_counts() {
        let out = build(&conn(), "u1", Some("b1"));
        let items = out["items"].as_array().unwrap();
        let skill = items.iter().find(|i| i["id"] == "skill:web-research").unwrap();
        assert_eq!(skill["uses"], 2);
        assert_eq!(skill["last_at"], "2026-10-30T10:02:00Z");
        let links = out["links"].as_array().unwrap();
        assert_eq!(links.len(), 1, "one deduped link: {links:?}");
        assert_eq!(links[0], json!({"from": "thread:t1", "to": "skill:web-research", "kind": "uses"}));
    }

    #[test]
    fn another_accounts_skills_threads_and_bots_never_appear() {
        let out = build(&conn(), "u1", None);
        let text = out.to_string();
        assert!(!text.contains("secret-skill") && !text.contains("thread:t2"), "{text}");
        // asking for someone else's bot yields nothing from it either
        let other = build(&conn(), "u1", Some("b2"));
        assert!(other["items"].as_array().unwrap().is_empty());
    }
}
