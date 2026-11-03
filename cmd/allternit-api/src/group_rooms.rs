//! Server-side group-room persistence & sync (BOT_TEAMMATES_SPEC Phase 4, AD-2).
//!
//! Bounded room projections stored in SQLite (same pattern as the mail layer):
//! one row per room with a monotonic `revision`, a `tombstone` flag (disband
//! can't resurrect), and a bounded JSON `projection` (last 16 messages, 1200
//! chars/message, ≤48KB total — enforced at write, mirroring Hermes'
//! `group-chat.ts`). Writes are CAS-gated on `expected_revision` so two
//! surfaces can last-write-wins-by-merge without forking state.
//!
//! Per-member read positions live in `group_room_watermarks` (keyed
//! room+member, the `<thread>::<member>` analog) and `@user` escalations land
//! in `group_room_holds`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::AppState;

// Projection caps — Hermes group-chat.ts model, spec AD-2.
pub const MAX_PROJECTION_MESSAGES: usize = 16;
pub const MAX_MESSAGE_CHARS: usize = 1200;
pub const MAX_PROJECTION_BYTES: usize = 256 * 1024;

/// Keys recognized as the message array inside a projection document.
const MESSAGE_ARRAY_KEYS: [&str; 2] = ["messages", "log"];

#[derive(Debug)]
pub enum RoomError {
    NotFound,
    Tombstoned,
    Conflict { current_revision: Option<i64> },
    ProjectionTooLarge,
    Store(rusqlite::Error),
}

impl From<rusqlite::Error> for RoomError {
    fn from(e: rusqlite::Error) -> Self {
        RoomError::Store(e)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RoomRecord {
    pub room_id: String,
    pub name: String,
    pub revision: i64,
    pub tombstone: bool,
    pub projection: Value,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    pub revision: i64,
    pub tombstone: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HoldRecord {
    pub hold_id: String,
    pub room_id: String,
    pub member_id: String,
    pub kind: String,
    pub message_excerpt: Option<String>,
    pub created_at: String,
    pub resolved: bool,
    pub resolved_at: Option<String>,
}

/// SQLite-backed group-room store. Like `db::DbHandle`, this is a path holder
/// that opens a fresh connection per call — no shared-mutable connection state.
#[derive(Debug, Clone)]
pub struct GroupRoomsStore {
    db_path: PathBuf,
}

impl GroupRoomsStore {
    pub fn new(data_dir: &FsPath) -> Result<Self, rusqlite::Error> {
        std::fs::create_dir_all(data_dir).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("group-rooms data dir: {e}")),
            )
        })?;
        let store = Self {
            db_path: data_dir.join("group-rooms.sqlite3"),
        };
        let conn = store.connect()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS group_room_projections (
                room_id    TEXT PRIMARY KEY,
                name       TEXT NOT NULL DEFAULT '',
                revision   INTEGER NOT NULL DEFAULT 0,
                tombstone  INTEGER NOT NULL DEFAULT 0,
                projection TEXT NOT NULL DEFAULT '{}',
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS group_room_watermarks (
                room_id            TEXT NOT NULL,
                member_id          TEXT NOT NULL,
                last_seen_revision INTEGER NOT NULL DEFAULT 0,
                updated_at         TEXT NOT NULL,
                PRIMARY KEY (room_id, member_id)
            );
            CREATE TABLE IF NOT EXISTS group_room_holds (
                hold_id        TEXT PRIMARY KEY,
                room_id        TEXT NOT NULL,
                member_id      TEXT NOT NULL,
                kind           TEXT NOT NULL,
                message_excerpt TEXT,
                created_at     TEXT NOT NULL,
                resolved       INTEGER NOT NULL DEFAULT 0,
                resolved_at    TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_group_room_holds_room
                ON group_room_holds (room_id, resolved);",
        )?;
        Ok(store)
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        Connection::open(&self.db_path)
    }

    fn current_revision(&self, conn: &Connection, room_id: &str) -> Result<Option<i64>, RoomError> {
        let rev: Option<i64> = conn
            .query_row(
                "SELECT revision FROM group_room_projections WHERE room_id = ?1",
                params![room_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(rev)
    }

    /// CAS-gated projection write. `expected_revision` must equal the stored
    /// revision (None/0 when the room is absent) or the write is rejected
    /// with `RoomError::Conflict` so the client can pull, merge, and retry.
    pub fn put_projection(
        &self,
        room_id: &str,
        name: Option<&str>,
        projection: Value,
        revision: i64,
        expected_revision: Option<i64>,
    ) -> Result<RoomRecord, RoomError> {
        let projection = enforce_projection_caps(projection)?;
        let conn = self.connect()?;
        let current = self.current_revision(&conn, room_id)?;
        if current.is_some() {
            // Tombstones are final — a disbanded room can never resurrect.
            let tombstoned: bool = conn.query_row(
                "SELECT tombstone FROM group_room_projections WHERE room_id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )? != 0;
            if tombstoned {
                return Err(RoomError::Tombstoned);
            }
        }
        let expected_matches = match (current, expected_revision) {
            (Some(cur), Some(exp)) => cur == exp,
            (None, None) => true,
            (None, Some(exp)) => exp <= 0,
            (Some(_), None) => false,
        };
        if !expected_matches {
            return Err(RoomError::Conflict {
                current_revision: current,
            });
        }

        let new_revision = current.map(|c| c.max(revision)).unwrap_or(revision).max(1);
        let now = chrono::Utc::now().to_rfc3339();
        let projection_text = projection.to_string();
        let name = name.unwrap_or("");
        conn.execute(
            "INSERT INTO group_room_projections
                (room_id, name, revision, tombstone, projection, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)
             ON CONFLICT(room_id) DO UPDATE SET
                name = CASE WHEN ?2 != '' THEN ?2 ELSE name END,
                revision = ?3,
                projection = ?4,
                updated_at = ?5",
            params![room_id, name, new_revision, projection_text, now],
        )?;
        Ok(RoomRecord {
            room_id: room_id.to_string(),
            name: name.to_string(),
            revision: new_revision,
            tombstone: false,
            projection,
            updated_at: now,
        })
    }

    /// Fetch a room. Tombstoned rooms return `Ok(None)` — disband is final.
    pub fn get_room(&self, room_id: &str) -> Result<Option<RoomRecord>, RoomError> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT name, revision, tombstone, projection, updated_at
                 FROM group_room_projections WHERE room_id = ?1",
                params![room_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((name, revision, tombstone, projection, updated_at)) => {
                if tombstone != 0 {
                    return Ok(None);
                }
                Ok(Some(RoomRecord {
                    room_id: room_id.to_string(),
                    name,
                    revision,
                    tombstone: false,
                    projection: serde_json::from_str(&projection).unwrap_or(Value::Null),
                    updated_at,
                }))
            }
        }
    }

    /// List live (non-tombstoned) rooms with their revisions.
    pub fn list_rooms(&self) -> Result<Vec<RoomSummary>, RoomError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT room_id, name, revision, updated_at FROM group_room_projections
             WHERE tombstone = 0 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RoomSummary {
                room_id: row.get(0)?,
                name: row.get(1)?,
                revision: row.get(2)?,
                tombstone: false,
                updated_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Rooms changed since `since_revision` (inclusive of tombstoned rooms so
    /// pull clients learn about disbands). Used for re-seed / incremental pull.
    pub fn changes_since(&self, since_revision: i64) -> Result<Vec<RoomSummary>, RoomError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT room_id, name, revision, tombstone, updated_at
             FROM group_room_projections WHERE revision > ?1 ORDER BY revision ASC",
        )?;
        let rows = stmt.query_map(params![since_revision], |row| {
            Ok(RoomSummary {
                room_id: row.get(0)?,
                name: row.get(1)?,
                revision: row.get(2)?,
                tombstone: row.get::<_, i64>(3)? != 0,
                updated_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Mark a room disbanded. Revision bumps so `changes_since` surfaces it.
    /// Already-tombstoned or unknown rooms read as `NotFound` (GET semantics).
    pub fn tombstone(&self, room_id: &str) -> Result<(), RoomError> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                "SELECT tombstone FROM group_room_projections WHERE room_id = ?1",
                params![room_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        match row {
            None | Some(1..) => return Err(RoomError::NotFound),
            Some(_) => {}
        }
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE group_room_projections
             SET tombstone = 1, revision = revision + 1, updated_at = ?2
             WHERE room_id = ?1",
            params![room_id, now],
        )?;
        Ok(())
    }

    /// Bump a member's read position. Monotonic — never moves backwards.
    pub fn bump_watermark(
        &self,
        room_id: &str,
        member_id: &str,
        last_seen_revision: i64,
    ) -> Result<i64, RoomError> {
        let conn = self.connect()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO group_room_watermarks
                (room_id, member_id, last_seen_revision, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(room_id, member_id) DO UPDATE SET
                last_seen_revision = MAX(last_seen_revision, ?3),
                updated_at = ?4",
            params![room_id, member_id, last_seen_revision, now],
        )?;
        let rev: i64 = conn.query_row(
            "SELECT last_seen_revision FROM group_room_watermarks
             WHERE room_id = ?1 AND member_id = ?2",
            params![room_id, member_id],
            |row| row.get(0),
        )?;
        Ok(rev)
    }

    pub fn list_watermarks(
        &self,
        room_id: &str,
    ) -> Result<Vec<(String, i64)>, RoomError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            "SELECT member_id, last_seen_revision FROM group_room_watermarks
             WHERE room_id = ?1 ORDER BY member_id ASC",
        )?;
        let rows = stmt.query_map(params![room_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn create_hold(
        &self,
        room_id: &str,
        member_id: &str,
        kind: &str,
        message_excerpt: Option<&str>,
    ) -> Result<HoldRecord, RoomError> {
        let hold = HoldRecord {
            hold_id: uuid::Uuid::new_v4().to_string(),
            room_id: room_id.to_string(),
            member_id: member_id.to_string(),
            kind: kind.to_string(),
            message_excerpt: message_excerpt.map(|s| s.chars().take(280).collect()),
            created_at: chrono::Utc::now().to_rfc3339(),
            resolved: false,
            resolved_at: None,
        };
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO group_room_holds
                (hold_id, room_id, member_id, kind, message_excerpt, created_at, resolved)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![
                hold.hold_id,
                hold.room_id,
                hold.member_id,
                hold.kind,
                hold.message_excerpt,
                hold.created_at
            ],
        )?;
        Ok(hold)
    }

    pub fn resolve_hold(&self, room_id: &str, hold_id: &str) -> Result<bool, RoomError> {
        let conn = self.connect()?;
        let now = chrono::Utc::now().to_rfc3339();
        let n = conn.execute(
            "UPDATE group_room_holds SET resolved = 1, resolved_at = ?3
             WHERE room_id = ?1 AND hold_id = ?2 AND resolved = 0",
            params![room_id, hold_id, now],
        )?;
        Ok(n > 0)
    }

    pub fn list_holds(
        &self,
        room_id: &str,
        resolved: Option<bool>,
    ) -> Result<Vec<HoldRecord>, RoomError> {
        let conn = self.connect()?;
        let (sql, args): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match resolved {
            Some(r) => (
                "SELECT hold_id, member_id, kind, message_excerpt, created_at, resolved, resolved_at
                 FROM group_room_holds WHERE room_id = ?1 AND resolved = ?2 ORDER BY created_at ASC",
                vec![Box::new(room_id.to_string()), Box::new(r as i64)],
            ),
            None => (
                "SELECT hold_id, member_id, kind, message_excerpt, created_at, resolved, resolved_at
                 FROM group_room_holds WHERE room_id = ?1 ORDER BY created_at ASC",
                vec![Box::new(room_id.to_string())],
            ),
        };
        let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(arg_refs), |row| {
            Ok(HoldRecord {
                hold_id: row.get(0)?,
                room_id: room_id.to_string(),
                member_id: row.get(1)?,
                kind: row.get(2)?,
                message_excerpt: row.get(3)?,
                created_at: row.get(4)?,
                resolved: row.get::<_, i64>(5)? != 0,
                resolved_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

/// Enforce the bounded-projection caps at write time: last
/// `MAX_PROJECTION_MESSAGES` messages, `MAX_MESSAGE_CHARS` per message text,
/// total serialized size ≤ `MAX_PROJECTION_BYTES` (oldest messages dropped
/// until it fits). Returns `RoomError::ProjectionTooLarge` only when there is
/// no trimmable message array and the document still exceeds the cap.
fn complete_answer(text: &str) -> bool {
    let Some((opening, rest)) = text.trim().split_once('\n') else { return false };
    if !matches!(opening.trim_end(), "```openui" | "```openui-lang") { return false }
    let Some(body) = rest.strip_suffix("\n```") else { return false };
    crate::answer_contract::validate(body.trim_end_matches('\r')).is_ok()
}

pub fn enforce_projection_caps(mut value: Value) -> Result<Value, RoomError> {
    let mut array_key: Option<String> = None;
    if let Some(obj) = value.as_object_mut() {
        for key in MESSAGE_ARRAY_KEYS {
            if obj.get(key).map(|v| v.is_array()).unwrap_or(false) {
                array_key = Some(key.to_string());
                break;
            }
        }
    }

    let key = match array_key {
        Some(k) => k,
        None => {
            if value.to_string().len() > MAX_PROJECTION_BYTES {
                return Err(RoomError::ProjectionTooLarge);
            }
            return Ok(value);
        }
    };

    // Clip per-message text first so the size pass works on final content.
    if let Some(messages) = value.get_mut(&key).and_then(|v| v.as_array_mut()) {
        for message in messages.iter_mut() {
            if let Some(Value::String(text)) = message.get_mut("text") {
                if text.chars().count() > MAX_MESSAGE_CHARS && !complete_answer(text) {
                    let truncated: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
                    *text = truncated;
                }
            }
        }
    }

    loop {
        let count = value
            .get(&key)
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let over_size = value.to_string().len() > MAX_PROJECTION_BYTES;
        if (count <= MAX_PROJECTION_MESSAGES && !over_size) || count == 0 {
            break;
        }
        if let Some(messages) = value.get_mut(&key).and_then(|v| v.as_array_mut()) {
            messages.remove(0);
        }
    }

    if value.to_string().len() > MAX_PROJECTION_BYTES {
        return Err(RoomError::ProjectionTooLarge);
    }
    Ok(value)
}

// ============================================================================
// Store registry — the API may serve multiple data dirs (tests); main serves
// exactly one. Keyed by `AppState.data_dir` so no `AppState` field is needed.
// ============================================================================

static STORES: OnceLock<RwLock<HashMap<PathBuf, Arc<GroupRoomsStore>>>> = OnceLock::new();

pub fn store_for(data_dir: &FsPath) -> Result<Arc<GroupRoomsStore>, StatusCode> {
    let map = STORES.get_or_init(|| RwLock::new(HashMap::new()));
    if let Some(store) = map.read().ok().and_then(|m| m.get(data_dir).cloned()) {
        return Ok(store);
    }
    let store = Arc::new(GroupRoomsStore::new(data_dir).map_err(|e| {
        tracing::error!(error = %e, "group-rooms: failed to open store");
        StatusCode::INTERNAL_SERVER_ERROR
    })?);
    if let Ok(mut m) = map.write() {
        m.insert(data_dir.to_path_buf(), store.clone());
    }
    Ok(store)
}

fn error_response(err: &RoomError) -> (StatusCode, Json<Value>) {
    match err {
        RoomError::NotFound => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))),
        RoomError::Tombstoned => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "room_tombstoned" })),
        ),
        RoomError::Conflict { current_revision } => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "revision_conflict",
                "current_revision": current_revision,
            })),
        ),
        RoomError::ProjectionTooLarge => (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({ "error": "projection_too_large" })),
        ),
        RoomError::Store(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        ),
    }
}

// ============================================================================
// Handlers
// ============================================================================

#[derive(Debug, Deserialize)]
struct PutProjectionBody {
    name: Option<String>,
    projection: Value,
    revision: i64,
    expected_revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PutWatermarkBody {
    last_seen_revision: i64,
}

#[derive(Debug, Deserialize)]
struct CreateHoldBody {
    member_id: String,
    #[serde(default = "default_hold_kind")]
    kind: String,
    message_excerpt: Option<String>,
}

fn default_hold_kind() -> String {
    "needs_you".to_string()
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    since_revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HoldsQuery {
    resolved: Option<bool>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/group-rooms", get(list_rooms_handler))
        .route("/group-rooms/changes", get(changes_handler))
        .route("/group-rooms/:room_id", get(get_room_handler))
        .route(
            "/group-rooms/:room_id/projection",
            put(put_projection_handler),
        )
        .route("/group-rooms/:room_id/tombstone", post(tombstone_handler))
        .route("/group-rooms/:room_id/watermarks", get(list_watermarks_handler))
        .route(
            "/group-rooms/:room_id/watermarks/:member_id",
            put(put_watermark_handler),
        )
        .route(
            "/group-rooms/:room_id/holds",
            get(list_holds_handler).post(create_hold_handler),
        )
        .route(
            "/group-rooms/:room_id/holds/:hold_id/resolve",
            post(resolve_hold_handler),
        )
}

async fn list_rooms_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.list_rooms() {
        Ok(rooms) => (StatusCode::OK, Json(json!({ "rooms": rooms }))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn changes_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ChangesQuery>,
) -> impl IntoResponse {
    let since = q.since_revision.unwrap_or(0);
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.changes_since(since) {
        Ok(rooms) => (StatusCode::OK, Json(json!({ "rooms": rooms }))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn get_room_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.get_room(&room_id) {
        Ok(Some(room)) => (StatusCode::OK, Json(json!(room))).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found" })),
        )
            .into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn put_projection_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Json(body): Json<PutProjectionBody>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.put_projection(
        &room_id,
        body.name.as_deref(),
        body.projection,
        body.revision,
        body.expected_revision,
    ) {
        Ok(room) => (StatusCode::OK, Json(json!(room))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn tombstone_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.tombstone(&room_id) {
        Ok(()) => (StatusCode::OK, Json(json!({ "tombstoned": true }))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn list_watermarks_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.list_watermarks(&room_id) {
        Ok(watermarks) => (
            StatusCode::OK,
            Json(json!({
                "watermarks": watermarks
                    .into_iter()
                    .map(|(member_id, rev)| json!({
                        "member_id": member_id,
                        "last_seen_revision": rev,
                    }))
                    .collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn put_watermark_handler(
    State(state): State<Arc<AppState>>,
    Path((room_id, member_id)): Path<(String, String)>,
    Json(body): Json<PutWatermarkBody>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.bump_watermark(&room_id, &member_id, body.last_seen_revision) {
        Ok(rev) => (
            StatusCode::OK,
            Json(json!({ "member_id": member_id, "last_seen_revision": rev })),
        )
            .into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn create_hold_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Json(body): Json<CreateHoldBody>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.create_hold(&room_id, &body.member_id, &body.kind, body.message_excerpt.as_deref()) {
        Ok(hold) => (StatusCode::OK, Json(json!(hold))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn resolve_hold_handler(
    State(state): State<Arc<AppState>>,
    Path((room_id, hold_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.resolve_hold(&room_id, &hold_id) {
        Ok(found) => (
            StatusCode::OK,
            Json(json!({ "resolved": true, "found": found })),
        )
            .into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

async fn list_holds_handler(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Query(q): Query<HoldsQuery>,
) -> impl IntoResponse {
    let store = match store_for(&state.data_dir) {
        Ok(s) => s,
        Err(resp) => return resp.into_response(),
    };
    match store.list_holds(&room_id, q.resolved) {
        Ok(holds) => (StatusCode::OK, Json(json!({ "holds": holds }))).into_response(),
        Err(e) => error_response(&e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, GroupRoomsStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = GroupRoomsStore::new(dir.path()).expect("store");
        (dir, store)
    }

    fn projection_with_messages(n: usize, text_len: usize) -> Value {
        let text = "x".repeat(text_len);
        json!({
            "messages": (0..n)
                .map(|i| json!({ "id": format!("m-{i}"), "text": text }))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn caps_trim_to_last_16_messages() {
        let capped = enforce_projection_caps(projection_with_messages(40, 10)).unwrap();
        let messages = capped["messages"].as_array().unwrap();
        assert_eq!(messages.len(), MAX_PROJECTION_MESSAGES);
        // Oldest surviving message is the 24th (40 - 16).
        assert_eq!(messages[0]["id"], "m-24");
    }

    #[test]
    fn caps_clip_message_text_to_1200_chars() {
        let capped = enforce_projection_caps(projection_with_messages(1, 5000)).unwrap();
        let text = capped["messages"][0]["text"].as_str().unwrap();
        assert_eq!(text.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[test]
    fn caps_enforce_48kb_total_by_dropping_oldest() {
        // 16 × 1200 chars ≈ 19KB fits; force the cap with oversized entries.
        let big = projection_with_messages(60, 1200);
        let capped = enforce_projection_caps(big).unwrap();
        assert!(capped.to_string().len() <= MAX_PROJECTION_BYTES);
        assert!(capped["messages"].as_array().unwrap().len() <= MAX_PROJECTION_MESSAGES);
    }

    #[test]
    fn caps_reject_oversized_document_without_message_array() {
        let blob = json!({ "blob": "y".repeat(MAX_PROJECTION_BYTES + 1) });
        assert!(matches!(
            enforce_projection_caps(blob),
            Err(RoomError::ProjectionTooLarge)
        ));
    }

    #[test]
    fn cas_accepts_matching_expected_revision() {
        let (_dir, store) = temp_store();
        let r1 = store
            .put_projection("room-a", Some("A"), json!({}), 1, Some(0))
            .unwrap();
        assert_eq!(r1.revision, 1);
        let r2 = store
            .put_projection("room-a", Some("A2"), json!({}), 2, Some(1))
            .unwrap();
        assert_eq!(r2.revision, 2);
        assert_eq!(r2.name, "A2");
    }

    #[test]
    fn cas_rejects_stale_expected_revision_with_conflict() {
        let (_dir, store) = temp_store();
        store
            .put_projection("room-a", None, json!({}), 1, Some(0))
            .unwrap();
        let err = store
            .put_projection("room-a", None, json!({}), 2, Some(0))
            .unwrap_err();
        match err {
            RoomError::Conflict { current_revision } => {
                assert_eq!(current_revision, Some(1));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn cas_absent_room_accepts_zero_expected() {
        let (_dir, store) = temp_store();
        // First touch creates the row.
        let r = store
            .put_projection("new-room", Some("New"), json!({}), 1, None)
            .unwrap();
        assert_eq!(r.revision, 1);
        // A second blind create with expected None must conflict now.
        assert!(matches!(
            store.put_projection("new-room", None, json!({}), 1, None),
            Err(RoomError::Conflict { .. })
        ));
    }

    #[test]
    fn tombstone_makes_room_absent_and_write_rejected() {
        let (_dir, store) = temp_store();
        store
            .put_projection("room-a", None, json!({}), 1, Some(0))
            .unwrap();
        store.tombstone("room-a").unwrap();
        // GET-equivalent: tombstoned room reads as absent.
        assert!(store.get_room("room-a").unwrap().is_none());
        // Writes to a tombstoned room are rejected — disband can't resurrect.
        assert!(matches!(
            store.put_projection("room-a", None, json!({}), 2, Some(1)),
            Err(RoomError::Tombstoned)
        ));
        // Tombstoning twice or an unknown room is a 404.
        assert!(matches!(
            store.tombstone("room-a"),
            Err(RoomError::NotFound)
        ));
        // changes_since still surfaces the tombstone for pull clients.
        let changes = store.changes_since(1).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].tombstone);
    }

    #[test]
    fn changes_since_filters_by_revision() {
        let (_dir, store) = temp_store();
        store
            .put_projection("r1", None, json!({}), 1, Some(0))
            .unwrap();
        store
            .put_projection("r2", None, json!({}), 2, Some(0))
            .unwrap();
        store
            .put_projection("r3", None, json!({}), 3, Some(0))
            .unwrap();
        // Re-putting r1 bumps it past the others.
        store
            .put_projection("r1", None, json!({}), 4, Some(1))
            .unwrap();
        // since=N is strictly-greater filtering; rev 2 drops out at since=2.
        let changes = store.changes_since(1).unwrap();
        let ids: Vec<&str> = changes.iter().map(|c| c.room_id.as_str()).collect();
        assert_eq!(ids, vec!["r2", "r3", "r1"]);
        assert_eq!(store.changes_since(2).unwrap().len(), 2);
    }

    #[test]
    fn watermark_bump_is_monotonic() {
        let (_dir, store) = temp_store();
        assert_eq!(store.bump_watermark("room-a", "bot-1", 5).unwrap(), 5);
        // Backwards moves are ignored.
        assert_eq!(store.bump_watermark("room-a", "bot-1", 2).unwrap(), 5);
        assert_eq!(store.bump_watermark("room-a", "bot-1", 9).unwrap(), 9);
        // Per-room+member isolation.
        assert_eq!(store.bump_watermark("room-a", "bot-2", 1).unwrap(), 1);
        assert_eq!(store.bump_watermark("room-b", "bot-1", 3).unwrap(), 3);
        let watermarks = store.list_watermarks("room-a").unwrap();
        assert_eq!(
            watermarks,
            vec![("bot-1".to_string(), 9), ("bot-2".to_string(), 1)]
        );
    }

    #[test]
    fn holds_create_list_resolve() {
        let (_dir, store) = temp_store();
        let hold = store
            .create_hold("room-a", "bot-1", "needs_you", Some("@user please decide"))
            .unwrap();
        assert!(!hold.resolved);

        let open = store.list_holds("room-a", Some(false)).unwrap();
        assert_eq!(open.len(), 1);
        assert!(store.list_holds("room-a", Some(true)).unwrap().is_empty());

        assert!(store.resolve_hold("room-a", &hold.hold_id).unwrap());
        assert!(!store.resolve_hold("room-a", &hold.hold_id).unwrap());
        assert_eq!(store.list_holds("room-a", Some(false)).unwrap().len(), 0);
        assert_eq!(store.list_holds("room-a", Some(true)).unwrap().len(), 1);
    }

    #[test]
    fn room_summary_lists_live_rooms_with_revisions() {
        let (_dir, store) = temp_store();
        store
            .put_projection("live", Some("Live"), json!({}), 3, Some(0))
            .unwrap();
        store
            .put_projection("dead", None, json!({}), 1, Some(0))
            .unwrap();
        store.tombstone("dead").unwrap();
        let rooms = store.list_rooms().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].room_id, "live");
        assert_eq!(rooms[0].revision, 3);
    }
}

#[cfg(test)]
mod route_tests {
    //! In-process HTTP round-trip over the mounted router (tower oneshot),
    //! following the idiom in admin_access_token_routes.rs.
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn group_room_routes_roundtrip() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = crate::test_helpers::app_state(&temp).await;
        let app = router().with_state(state);

        // Create via CAS PUT (expected_revision 0 on an absent room).
        let resp = app
            .clone()
            .oneshot(json_request(
                "PUT",
                "/group-rooms/room-a/projection",
                json!({
                    "name": "Room A",
                    "projection": { "messages": [
                        { "id": "m-1", "from": "user", "text": "hi", "timestamp": "2026-09-07T00:00:00Z" }
                    ]},
                    "revision": 1,
                    "expected_revision": 0,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["revision"], 1);

        // Stale CAS → 409 with current_revision.
        let resp = app
            .clone()
            .oneshot(json_request(
                "PUT",
                "/group-rooms/room-a/projection",
                json!({ "projection": {}, "revision": 2, "expected_revision": 0 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"], "revision_conflict");
        assert_eq!(body["current_revision"], 1);

        // GET /group-rooms/:id → 200; list includes it.
        let resp = app
            .clone()
            .oneshot(json_request("GET", "/group-rooms/room-a", json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Static segment wins over :room_id.
        let resp = app
            .clone()
            .oneshot(json_request(
                "GET",
                "/group-rooms/changes?since_revision=0",
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["rooms"].as_array().unwrap().len(), 1);

        // Watermark bump + read-back.
        let resp = app
            .clone()
            .oneshot(json_request(
                "PUT",
                "/group-rooms/room-a/watermarks/bot-1",
                json!({ "last_seen_revision": 3 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app
            .clone()
            .oneshot(json_request("GET", "/group-rooms/room-a/watermarks", json!({})))
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["watermarks"][0]["member_id"], "bot-1");

        // Hold create → list unresolved → resolve → list resolved.
        let resp = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/group-rooms/room-a/holds",
                json!({ "member_id": "bot-1", "kind": "needs_you", "message_excerpt": "@user please decide" }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        let hold_id = body["hold_id"].as_str().unwrap().to_string();

        let resp = app
            .clone()
            .oneshot(json_request("GET", "/group-rooms/room-a/holds?resolved=false", json!({})))
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["holds"].as_array().unwrap().len(), 1);

        let resp = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/group-rooms/room-a/holds/{hold_id}/resolve"),
                json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(json_request("GET", "/group-rooms/room-a/holds?resolved=false", json!({})))
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["holds"].as_array().unwrap().len(), 0);

        // Tombstone → GET 404, writes rejected, changes still surface it.
        let resp = app
            .clone()
            .oneshot(json_request("POST", "/group-rooms/room-a/tombstone", json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(json_request("GET", "/group-rooms/room-a", json!({})))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = app
            .clone()
            .oneshot(json_request(
                "PUT",
                "/group-rooms/room-a/projection",
                json!({ "projection": {}, "revision": 9, "expected_revision": 2 }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["error"], "room_tombstoned");

        let resp = app
            .clone()
            .oneshot(json_request(
                "GET",
                "/group-rooms/changes?since_revision=0",
                json!({}),
            ))
            .await
            .unwrap();
        let body = body_json(resp.into_body()).await;
        let room = &body["rooms"].as_array().unwrap()[0];
        assert_eq!(room["tombstone"], true);
    }
}
