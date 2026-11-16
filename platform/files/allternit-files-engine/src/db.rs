//! Index DB schema and row helpers (SQLite via rusqlite, as allternit-api).

use std::path::PathBuf;

use allternit_files_core::{ItemKind, PrincipalRef, RootPolicy};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::{FilesError, Result};
use crate::model::{ItemRecord, RootRecord};

pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS roots(
  root_id TEXT PRIMARY KEY, kind TEXT NOT NULL, name TEXT NOT NULL, local_path TEXT NOT NULL UNIQUE,
  policy TEXT NOT NULL, root_item_id TEXT NOT NULL, epoch TEXT NOT NULL,
  next_seq INTEGER NOT NULL DEFAULT 1, min_seq INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items(
  item_id TEXT PRIMARY KEY, root_id TEXT NOT NULL, parent_id TEXT, name TEXT NOT NULL, kind TEXT NOT NULL,
  size INTEGER NOT NULL, meta_version INTEGER NOT NULL, content_hash TEXT,
  created_at TEXT NOT NULL, modified_at TEXT NOT NULL, modified_by TEXT,
  root_link TEXT, link_escapes INTEGER NOT NULL DEFAULT 0,
  mtime_ns INTEGER, inode INTEGER, current_rev_id INTEGER, trash_id TEXT);
CREATE UNIQUE INDEX IF NOT EXISTS items_live_name ON items(parent_id, name) WHERE trash_id IS NULL;
CREATE INDEX IF NOT EXISTS items_root ON items(root_id);
CREATE INDEX IF NOT EXISTS items_trash ON items(trash_id);
CREATE TABLE IF NOT EXISTS revisions(
  rev_id INTEGER PRIMARY KEY AUTOINCREMENT, item_id TEXT NOT NULL, meta_version INTEGER NOT NULL,
  content_hash TEXT NOT NULL, size INTEGER NOT NULL, created_at TEXT NOT NULL, created_by TEXT,
  label TEXT, retained INTEGER NOT NULL DEFAULT 0, superseded_at TEXT);
CREATE INDEX IF NOT EXISTS revisions_item ON revisions(item_id);
CREATE TABLE IF NOT EXISTS revision_chunks(
  rev_id INTEGER NOT NULL, idx INTEGER NOT NULL, hash TEXT NOT NULL, offset INTEGER NOT NULL,
  size INTEGER NOT NULL, PRIMARY KEY(rev_id, idx));
CREATE INDEX IF NOT EXISTS revision_chunks_hash ON revision_chunks(hash);
CREATE TABLE IF NOT EXISTS chunks(hash TEXT PRIMARY KEY, size INTEGER NOT NULL, refcount INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS changes(
  root_id TEXT NOT NULL, seq INTEGER NOT NULL, op TEXT NOT NULL, item_id TEXT NOT NULL,
  old_parent_id TEXT, at TEXT NOT NULL, by TEXT NOT NULL, PRIMARY KEY(root_id, seq));
CREATE TABLE IF NOT EXISTS trash(
  trash_id TEXT PRIMARY KEY, root_id TEXT NOT NULL, item_id TEXT NOT NULL, original_parent_id TEXT NOT NULL,
  trashed_at TEXT NOT NULL, trashed_by TEXT NOT NULL, purge_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS uploads(
  session_id TEXT PRIMARY KEY, request TEXT NOT NULL, principal TEXT NOT NULL, state TEXT NOT NULL,
  created_at TEXT NOT NULL, expires_at TEXT NOT NULL, device_utc_offset INTEGER);
CREATE TABLE IF NOT EXISTS upload_chunks(
  session_id TEXT NOT NULL, idx INTEGER NOT NULL, hash TEXT NOT NULL, offset INTEGER NOT NULL,
  size INTEGER NOT NULL, PRIMARY KEY(session_id, idx));
CREATE INDEX IF NOT EXISTS upload_chunks_hash ON upload_chunks(hash);
"#;

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.execute_batch(SCHEMA)?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .optional()?;
    match v {
        None => {
            conn.execute("INSERT INTO meta(key, value) VALUES('schema_version', ?1)", params![SCHEMA_VERSION.to_string()])?;
        }
        Some(v) if v == SCHEMA_VERSION.to_string() => {}
        Some(v) => return Err(FilesError::Internal(format!("index schema {v} is newer than this engine ({SCHEMA_VERSION})"))),
    }
    Ok(conn)
}

pub fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("contract types serialize")
}

pub fn from_json<T: DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(s).map_err(|e| FilesError::Internal(format!("index row: {e}")))
}

/// Serde name of a unit enum variant (`RootKind::Inbox` -> `inbox`).
pub fn enum_str<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v).expect("enum serializes") {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn enum_parse<T: DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| FilesError::Internal(format!("index enum {s:?}: {e}")))
}

pub const ITEM_COLS: &str = "item_id, root_id, parent_id, name, kind, size, meta_version, content_hash, created_at, modified_at, modified_by, root_link, link_escapes";

pub fn item_from_row(r: &Row<'_>) -> rusqlite::Result<(ItemRecord, String, Option<String>)> {
    let kind: String = r.get(4)?;
    let by: Option<String> = r.get(10)?;
    Ok((
        ItemRecord {
            id: r.get(0)?,
            root_id: r.get(1)?,
            parent_id: r.get(2)?,
            name: r.get(3)?,
            kind: if kind == "folder" { ItemKind::Folder } else { ItemKind::File },
            size: r.get::<_, i64>(5)? as u64,
            meta_version: r.get::<_, i64>(6)? as u64,
            content_hash: r.get(7)?,
            created_at: r.get(8)?,
            modified_at: r.get(9)?,
            modified_by: None,
            root_link: r.get(11)?,
            link_escapes: r.get::<_, i64>(12)? != 0,
        },
        kind,
        by,
    ))
}

fn finish_item(row: (ItemRecord, String, Option<String>)) -> Result<ItemRecord> {
    let (mut item, _, by) = row;
    item.modified_by = by.as_deref().map(from_json::<PrincipalRef>).transpose()?;
    Ok(item)
}

pub fn item_opt(c: &Connection, id: &str) -> Result<Option<ItemRecord>> {
    let sql = format!("SELECT {ITEM_COLS} FROM items WHERE item_id = ?1");
    c.query_row(&sql, params![id], item_from_row).optional()?.map(finish_item).transpose()
}

/// A live (not trashed) item.
pub fn live_item(c: &Connection, id: &str) -> Result<ItemRecord> {
    let sql = format!("SELECT {ITEM_COLS} FROM items WHERE item_id = ?1 AND trash_id IS NULL");
    c.query_row(&sql, params![id], item_from_row)
        .optional()?
        .map(finish_item)
        .transpose()?
        .ok_or_else(|| FilesError::NotFound(format!("item {id}")))
}

pub fn items_where(c: &Connection, cond: &str, p: impl rusqlite::Params) -> Result<Vec<ItemRecord>> {
    let sql = format!("SELECT {ITEM_COLS} FROM items WHERE {cond}");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map(p, item_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(finish_item).collect()
}

pub fn live_children(c: &Connection, parent_id: &str) -> Result<Vec<ItemRecord>> {
    items_where(c, "parent_id = ?1 AND trash_id IS NULL ORDER BY name", params![parent_id])
}

pub fn child_by_name(c: &Connection, parent_id: &str, name: &str) -> Result<Option<ItemRecord>> {
    Ok(items_where(c, "parent_id = ?1 AND name = ?2 AND trash_id IS NULL", params![parent_id, name])?.pop())
}

const ROOT_COLS: &str = "root_id, kind, name, local_path, policy, root_item_id, created_at";

fn root_from_row(r: &Row<'_>) -> rusqlite::Result<(String, String, String, String, String, String, String)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
}

fn finish_root(t: (String, String, String, String, String, String, String)) -> Result<RootRecord> {
    Ok(RootRecord {
        root_id: t.0,
        kind: enum_parse(&t.1)?,
        name: t.2,
        local_path: PathBuf::from(t.3),
        policy: from_json::<RootPolicy>(&t.4)?,
        root_item_id: t.5,
        created_at: t.6,
    })
}

pub fn root(c: &Connection, root_id: &str) -> Result<RootRecord> {
    let sql = format!("SELECT {ROOT_COLS} FROM roots WHERE root_id = ?1");
    c.query_row(&sql, params![root_id], root_from_row)
        .optional()?
        .map(finish_root)
        .transpose()?
        .ok_or_else(|| FilesError::RootNotFound(root_id.to_string()))
}

pub fn roots(c: &Connection) -> Result<Vec<RootRecord>> {
    let sql = format!("SELECT {ROOT_COLS} FROM roots ORDER BY created_at, root_id");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map([], root_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(finish_root).collect()
}
