//! Side tables beside the engine DB (`files_v2.sqlite`): pins, comments, the
//! provider registry and root display names. Plus read-only queries on the
//! engine's index for name search and per-root usage.

use std::collections::BTreeMap;
use std::path::Path;

use allternit_files_core::{Comment, ItemKind, PrincipalRef};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pins(
  item_id TEXT NOT NULL, device_id TEXT NOT NULL, pinned_at TEXT NOT NULL, PRIMARY KEY(item_id, device_id));
CREATE TABLE IF NOT EXISTS comments(
  comment_id TEXT PRIMARY KEY, item_id TEXT NOT NULL, author TEXT NOT NULL, body TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS comments_item ON comments(item_id, created_at);
CREATE TABLE IF NOT EXISTS providers(
  kind TEXT PRIMARY KEY, root_id TEXT NOT NULL, source TEXT NOT NULL, ready INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS root_names(root_id TEXT PRIMARY KEY, name TEXT NOT NULL);
"#;

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

pub fn pins(c: &Connection, item_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut s = c.prepare("SELECT device_id FROM pins WHERE item_id = ?1 ORDER BY pinned_at, device_id")?;
    let v = s.query_map(params![item_id], |r| r.get(0))?.collect();
    v
}

pub fn set_pin(c: &Connection, item_id: &str, device_id: &str, pinned: bool, now: &str) -> rusqlite::Result<()> {
    if pinned {
        c.execute("INSERT OR IGNORE INTO pins(item_id, device_id, pinned_at) VALUES(?1,?2,?3)", params![item_id, device_id, now])?;
    } else {
        c.execute("DELETE FROM pins WHERE item_id = ?1 AND device_id = ?2", params![item_id, device_id])?;
    }
    Ok(())
}

pub fn comments(c: &Connection, item_id: &str) -> rusqlite::Result<Vec<Comment>> {
    let mut s = c.prepare("SELECT comment_id, author, body, created_at FROM comments WHERE item_id = ?1 ORDER BY created_at, comment_id")?;
    let rows = s.query_map(params![item_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (comment_id, author, body, created_at) = row?;
        let author: PrincipalRef = serde_json::from_str(&author)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e)))?;
        out.push(Comment { comment_id, item_id: item_id.to_string(), author, body, created_at });
    }
    Ok(out)
}

pub fn add_comment(c: &Connection, comment: &Comment) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO comments(comment_id, item_id, author, body, created_at) VALUES(?1,?2,?3,?4,?5)",
        params![
            comment.comment_id,
            comment.item_id,
            serde_json::to_string(&comment.author).expect("principal serializes"),
            comment.body,
            comment.created_at
        ],
    )?;
    Ok(())
}

/// `(root_id, source, ready)` of a provider root.
pub fn provider(c: &Connection, kind: &str) -> rusqlite::Result<Option<(String, String, bool)>> {
    c.query_row("SELECT root_id, source, ready FROM providers WHERE kind = ?1", params![kind], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0))
    })
    .optional()
}

pub fn set_provider(c: &Connection, kind: &str, root_id: &str, source: &str, ready: bool) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO providers(kind, root_id, source, ready) VALUES(?1,?2,?3,?4) \
         ON CONFLICT(kind) DO UPDATE SET root_id = excluded.root_id, source = excluded.source, ready = excluded.ready",
        params![kind, root_id, source, ready as i64],
    )?;
    Ok(())
}

/// Provider kind and readiness by root id.
pub fn provider_by_root(c: &Connection, root_id: &str) -> rusqlite::Result<Option<(String, bool)>> {
    c.query_row("SELECT kind, ready FROM providers WHERE root_id = ?1", params![root_id], |r| {
        Ok((r.get(0)?, r.get::<_, i64>(1)? != 0))
    })
    .optional()
}

pub fn root_name(c: &Connection, root_id: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT name FROM root_names WHERE root_id = ?1", params![root_id], |r| r.get(0)).optional()
}

pub fn set_root_name(c: &Connection, root_id: &str, name: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO root_names(root_id, name) VALUES(?1,?2) ON CONFLICT(root_id) DO UPDATE SET name = excluded.name",
        params![root_id, name],
    )?;
    Ok(())
}

/// Forget everything kept for a removed root's items.
pub fn forget_root(c: &Connection, root_id: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM root_names WHERE root_id = ?1", params![root_id])?;
    c.execute("DELETE FROM providers WHERE root_id = ?1", params![root_id])?;
    Ok(())
}

fn index(engine_dir: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open_with_flags(engine_dir.join("index.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(c)
}

/// Name search on the engine index (`SearchRequest` mode `name`): live,
/// non-root items in `root_ids` whose name contains `query` (ASCII
/// case-insensitive). Returns `(item_id, score)`, best first.
pub fn search_names(
    engine_dir: &Path,
    query: &str,
    root_ids: &[String],
    kind: Option<ItemKind>,
    modified_after: Option<&str>,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<Vec<(String, f64)>> {
    if root_ids.is_empty() {
        return Ok(Vec::new());
    }
    let c = index(engine_dir)?;
    let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let marks = (0..root_ids.len()).map(|i| format!("?{}", i + 6)).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT item_id, name FROM items WHERE trash_id IS NULL AND parent_id IS NOT NULL \
         AND name LIKE ?1 ESCAPE '\\' AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR modified_at > ?3) \
         AND root_id IN ({marks}) \
         ORDER BY (lower(name) = lower(?4)) DESC, (lower(name) LIKE lower(?5) ESCAPE '\\') DESC, length(name), name \
         LIMIT {limit} OFFSET {offset}"
    );
    let kind = kind.map(|k| if k == ItemKind::Folder { "folder" } else { "file" });
    let mut values: Vec<rusqlite::types::Value> = vec![
        format!("%{escaped}%").into(),
        kind.map(|k| k.to_string()).into(),
        modified_after.map(|m| m.to_string()).into(),
        query.to_string().into(),
        format!("{escaped}%").into(),
    ];
    values.extend(root_ids.iter().map(|r| rusqlite::types::Value::from(r.clone())));
    let mut s = c.prepare(&sql)?;
    let q = query.to_lowercase();
    let rows = s.query_map(rusqlite::params_from_iter(values), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name) = row?;
        let n = name.to_lowercase();
        let score = if n == q { 1.0 } else if n.starts_with(&q) { 0.8 } else { 0.5 };
        out.push((id, score));
    }
    Ok(out)
}

/// Bytes of live files per root, from the index.
pub fn root_usage(engine_dir: &Path) -> rusqlite::Result<BTreeMap<String, u64>> {
    let c = index(engine_dir)?;
    let mut s = c.prepare("SELECT root_id, COALESCE(SUM(size), 0) FROM items WHERE trash_id IS NULL AND kind = 'file' GROUP BY root_id")?;
    let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.max(0) as u64)))?;
    rows.collect()
}
