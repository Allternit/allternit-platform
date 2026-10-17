//! Memory Drive activity: what touched a person's memory, and when.
//!
//! One row per save, recall, forget, import, Dream step or vendor read, in
//! `memory_activity` (V266). Chat reads the rows for its session to show
//! "Saved to Memory Drive" under a turn; Memory Drive → Activity lists them
//! all; the Dreams list reads a run's rows as live steps.
//!
//! - **Best effort.** Recording never fails the thing being recorded.
//! - **Previews only.** At most [`MAX_ITEMS`] lines of [`MAX_TEXT`] chars per
//!   row; the drive's git history stays the full record.
//! - **Incognito.** Rows for an incognito session are never written.
//! - **Bounded.** Rows older than [`KEEP_DAYS`] are dropped as new ones come in.
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

/// Kinds the API writes. Agents may report only [`AGENT_KINDS`].
pub const KINDS: &[&str] = &["saved", "recalled", "forgot", "imported", "dream", "vendor"];
pub const AGENT_KINDS: &[&str] = &["saved", "recalled", "forgot"];
const MAX_ITEMS: usize = 20;
const MAX_TEXT: usize = 240;
const MAX_TITLE: usize = 160;
const KEEP_DAYS: i64 = 90;
pub const MAX_PAGE: i64 = 200;

#[derive(Debug, Clone, Default)]
pub struct Activity<'a> {
    pub kind: &'a str,
    pub title: String,
    /// Remembered lines with their source: `[{text, source}]`.
    pub items: Vec<(String, String)>,
    /// Anything else worth showing (counts, a status, an error).
    pub extra: Option<Value>,
    pub source: Option<String>,
    pub session: Option<String>,
    pub run: Option<String>,
}

fn clip(s: &str, n: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= n {
        t
    } else {
        format!("{}…", t.chars().take(n - 1).collect::<String>())
    }
}

/// The session a drive source points at (`/?session=<id>`), if any.
pub fn session_of(source: &str) -> Option<String> {
    source.strip_prefix("/?session=").map(|s| s.split('&').next().unwrap_or(s).to_string()).filter(|s| !s.is_empty())
}

fn incognito(conn: &Connection, session: &str) -> bool {
    conn.prepare("SELECT 1 FROM ephemeral_sessions WHERE session_id = ?1").and_then(|mut s| s.exists(params![session])).unwrap_or(false)
}

/// Write one row. Returns its id, or `None` when it was skipped or failed.
pub fn record(conn: &Connection, owner: &str, a: Activity<'_>) -> Option<i64> {
    if !KINDS.contains(&a.kind) || owner.is_empty() {
        return None;
    }
    if a.session.as_deref().is_some_and(|s| incognito(conn, s)) {
        return None;
    }
    let items: Vec<Value> = a
        .items
        .iter()
        .take(MAX_ITEMS)
        .map(|(t, s)| json!({ "text": clip(t, MAX_TEXT), "source": clip(s, MAX_TEXT) }))
        .collect();
    let mut detail = json!({ "items": items, "total": a.items.len() });
    if let Some(Value::Object(extra)) = a.extra {
        for (k, v) in extra {
            detail[k] = v;
        }
    }
    let res = conn.execute(
        "INSERT INTO memory_activity (user_id, at, kind, title, detail, source, session_id, run_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            owner,
            chrono::Utc::now().to_rfc3339(),
            a.kind,
            clip(&a.title, MAX_TITLE),
            detail.to_string(),
            a.source.as_deref().map(|s| clip(s, MAX_TEXT)),
            a.session,
            a.run
        ],
    );
    match res {
        Ok(_) => {
            let id = conn.last_insert_rowid();
            if id % 200 == 0 {
                let _ = conn.execute(
                    &format!("DELETE FROM memory_activity WHERE user_id = ?1 AND at < datetime('now', '-{KEEP_DAYS} days')"),
                    params![owner],
                );
            }
            Some(id)
        }
        Err(e) => {
            tracing::debug!("memory activity not recorded: {e}");
            None
        }
    }
}

/// [`record`] with a fresh connection; for callers that hold none.
pub fn record_db(db: &crate::db::DbHandle, owner: &str, a: Activity<'_>) -> Option<i64> {
    db.connect().ok().and_then(|c| record(&c, owner, a))
}

/// The activity a drive commit stands for, from its message and changes.
/// `None` for commits that aren't worth a row (no adds, no removes).
pub fn for_commit(message: &str, added: &[(String, String)], removed: usize) -> Option<(&'static str, String)> {
    let n = added.len();
    let plural = |n: usize| if n == 1 { "memory" } else { "memories" };
    if n == 0 && removed == 0 {
        return None;
    }
    if let Some(from) = message.strip_prefix("Forget memories from ") {
        return Some(("forgot", format!("Forgot {removed} {} from {from}", plural(removed))));
    }
    if let Some(from) = message.strip_prefix("Import memory from ").or_else(|| message.strip_prefix("Sync memory from ")) {
        return Some(("imported", format!("Imported {n} {} from {from}", plural(n))));
    }
    if n > 0 {
        let what = if n == 1 { format!("Saved: {}", added[0].0) } else { format!("Saved {n} memories") };
        return Some(("saved", what));
    }
    Some(("forgot", format!("Removed {removed} {}", plural(removed))))
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub id: i64,
    pub at: String,
    pub kind: String,
    pub title: String,
    pub detail: Value,
    pub source: Option<String>,
    pub session: Option<String>,
    pub run: Option<String>,
}

#[derive(Debug, Default)]
pub struct Filter {
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub session: Option<String>,
    pub run: Option<String>,
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

/// Newest first when paging back (`before`); oldest first when following
/// (`after`), so a poller appends in order.
pub fn list(conn: &Connection, owner: &str, f: &Filter) -> rusqlite::Result<Vec<Row>> {
    let limit = f.limit.unwrap_or(50).clamp(1, MAX_PAGE);
    let order = if f.after.is_some() { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT id, at, kind, title, detail, source, session_id, run_id FROM memory_activity
         WHERE user_id = ?1 AND (?2 IS NULL OR id > ?2) AND (?3 IS NULL OR id < ?3)
           AND (?4 IS NULL OR session_id = ?4) AND (?5 IS NULL OR run_id = ?5) AND (?6 IS NULL OR kind = ?6)
         ORDER BY id {order} LIMIT ?7"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![owner, f.after, f.before, f.session, f.run, f.kind, limit], |r| {
        Ok(Row {
            id: r.get(0)?,
            at: r.get(1)?,
            kind: r.get(2)?,
            title: r.get(3)?,
            detail: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or(Value::Null),
            source: r.get(5)?,
            session: r.get(6)?,
            run: r.get(7)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../migrations/V266__memory_activity.sql")).unwrap();
        c.execute_batch("CREATE TABLE ephemeral_sessions (session_id TEXT PRIMARY KEY)").unwrap();
        c
    }

    #[test]
    fn records_lists_and_filters() {
        let c = db();
        let a = record(&c, "u", Activity { kind: "saved", title: "Saved: likes tea".into(), items: vec![("Likes tea.".into(), "/?session=s1".into())], session: Some("s1".into()), ..Default::default() }).unwrap();
        record(&c, "u", Activity { kind: "dream", title: "Reading yesterday's chats".into(), run: Some("d1".into()), ..Default::default() }).unwrap();
        record(&c, "other", Activity { kind: "saved", title: "x".into(), session: Some("s1".into()), ..Default::default() }).unwrap();
        let s = list(&c, "u", &Filter { session: Some("s1".into()), ..Default::default() }).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].detail["items"][0]["text"], "Likes tea.");
        assert_eq!(list(&c, "u", &Filter { run: Some("d1".into()), ..Default::default() }).unwrap().len(), 1);
        assert_eq!(list(&c, "u", &Filter { after: Some(a), ..Default::default() }).unwrap().len(), 1);
        assert_eq!(list(&c, "u", &Filter::default()).unwrap().len(), 2);
    }

    #[test]
    fn incognito_sessions_and_unknown_kinds_are_never_written() {
        let c = db();
        c.execute("INSERT INTO ephemeral_sessions VALUES ('inc')", []).unwrap();
        assert!(record(&c, "u", Activity { kind: "saved", title: "x".into(), session: Some("inc".into()), ..Default::default() }).is_none());
        assert!(record(&c, "u", Activity { kind: "shell", title: "x".into(), ..Default::default() }).is_none());
    }

    #[test]
    fn previews_are_clipped() {
        let c = db();
        let long = "word ".repeat(200);
        let items = (0..30).map(|i| (format!("{i} {long}"), "chatgpt:memory".to_string())).collect();
        record(&c, "u", Activity { kind: "imported", title: long.clone(), items, ..Default::default() }).unwrap();
        let r = &list(&c, "u", &Filter::default()).unwrap()[0];
        assert!(r.title.chars().count() <= MAX_TITLE);
        assert_eq!(r.detail["items"].as_array().unwrap().len(), MAX_ITEMS);
        assert_eq!(r.detail["total"], 30);
    }

    #[test]
    fn commit_messages_map_to_kinds() {
        let one = vec![("Likes tea.".to_string(), "x:y".to_string())];
        assert_eq!(for_commit("Remember from conversation", &one, 0).unwrap().0, "saved");
        assert_eq!(for_commit("Import memory from Codex", &one, 0).unwrap().1, "Imported 1 memory from Codex");
        assert_eq!(for_commit("Sync memory from Codex", &one, 0).unwrap().0, "imported");
        assert_eq!(for_commit("Forget memories from telegram", &[], 3).unwrap().1, "Forgot 3 memories from telegram");
        assert!(for_commit("Edit", &[], 0).is_none());
        assert_eq!(session_of("/?session=abc"), Some("abc".into()));
        assert_eq!(session_of("telegram:chat/1"), None);
    }
}
