//! SQLite implementation of [`SelfHostStore`] (rusqlite).
//!
//! One connection behind a mutex; every call runs on the blocking pool. That
//! is plenty for dev and single-box mode, and it is the only way an
//! in-memory (`:memory:`) database can be shared. Claims still use a
//! conditional UPDATE inside an IMMEDIATE transaction, so the logic does not
//! depend on the mutex for correctness.
//!
//! Timestamps are TEXT in one fixed-width UTC format
//! (`2026-11-12T10:00:00.123456Z`) so string order is time order. JSON
//! columns are TEXT.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, TransactionBehavior};
use serde_json::Value;

use super::*;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    instructions TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL,
    tools TEXT NOT NULL DEFAULT '[]',
    mcp_servers TEXT NOT NULL DEFAULT '[]',
    skills TEXT NOT NULL DEFAULT '[]',
    metadata TEXT NOT NULL DEFAULT '{}',
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS agents_created ON agents (created_at, id);

CREATE TABLE IF NOT EXISTS environments (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    network TEXT NOT NULL DEFAULT 'none' CHECK (network IN ('none','limited','full')),
    allowed_hosts TEXT NOT NULL DEFAULT '[]',
    git TEXT NOT NULL DEFAULT '{}',
    mcp TEXT NOT NULL DEFAULT '{}',
    image TEXT,
    packages TEXT NOT NULL DEFAULT '{}',
    metadata TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS environments_created ON environments (created_at, id);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    environment_id TEXT NOT NULL REFERENCES environments(id),
    title TEXT,
    status TEXT NOT NULL DEFAULT 'idle'
        CHECK (status IN ('idle','running','waiting_for_worker','interrupted','error','archived')),
    status_reason TEXT,
    last_seq INTEGER NOT NULL DEFAULT 0,
    metadata TEXT NOT NULL DEFAULT '{}',
    agent_snapshot TEXT NOT NULL DEFAULT '{}',
    gizzi_session_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    archived_at TEXT
);
CREATE INDEX IF NOT EXISTS sessions_created ON sessions (created_at, id);
CREATE INDEX IF NOT EXISTS sessions_agent ON sessions (agent_id);
CREATE INDEX IF NOT EXISTS sessions_environment ON sessions (environment_id);

CREATE TABLE IF NOT EXISTS session_events (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    seq INTEGER NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('message','tool_call','tool_result','status','error')),
    data TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (session_id, seq)
);

CREATE TABLE IF NOT EXISTS work_items (
    id TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES environments(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    status TEXT NOT NULL DEFAULT 'ready'
        CHECK (status IN ('ready','leased','done','failed','cancelled')),
    lease_generation INTEGER NOT NULL DEFAULT 0,
    lease_expires_at TEXT,
    worker_id TEXT,
    session_secret_hash TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS work_items_claim ON work_items (environment_id, status, created_at, id);
CREATE INDEX IF NOT EXISTS work_items_session ON work_items (session_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS work_items_secret ON work_items (session_secret_hash)
    WHERE session_secret_hash IS NOT NULL;

CREATE TABLE IF NOT EXISTS tool_calls (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    work_id TEXT,
    claimed_generation INTEGER,
    name TEXT NOT NULL,
    input TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','claimed','done')),
    output TEXT CHECK (output IS NULL OR length(CAST(output AS BLOB)) <= 102400),
    is_error INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT
);
CREATE INDEX IF NOT EXISTS tool_calls_session ON tool_calls (session_id, status, created_at);

CREATE TABLE IF NOT EXISTS workers (
    environment_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    current_work_id TEXT,
    hostname TEXT,
    version TEXT,
    PRIMARY KEY (environment_id, worker_id)
);
CREATE INDEX IF NOT EXISTS workers_seen ON workers (environment_id, last_seen_at);

CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin','developer','viewer')),
    prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    last_used_at TEXT,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS api_keys_created ON api_keys (created_at, id);

CREATE TABLE IF NOT EXISTS environment_keys (
    id TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES environments(id),
    name TEXT,
    prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT,
    last_used_at TEXT,
    revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS environment_keys_env ON environment_keys (environment_id, created_at, id);

CREATE TABLE IF NOT EXISTS credentials (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    sealed BLOB NOT NULL,
    key_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
"#;

const AGENT_COLS: &str =
    "id, name, instructions, model, tools, mcp_servers, skills, metadata, version, created_at, updated_at";
const ENV_COLS: &str = "id, name, network, allowed_hosts, git, mcp, image, packages, metadata, created_at, updated_at";
const SESSION_COLS: &str = "id, agent_id, environment_id, title, status, status_reason, last_seq, metadata, agent_snapshot, gizzi_session_id, created_at, updated_at, archived_at";
const EVENT_COLS: &str = "id, session_id, seq, type, data, created_at";
const WORK_COLS: &str = "id, environment_id, session_id, status, lease_generation, lease_expires_at, worker_id, error_message, created_at, updated_at, completed_at";
const TOOL_COLS: &str = "id, session_id, work_id, claimed_generation, name, input, status, output, is_error, created_at, updated_at, completed_at";
const WORKER_COLS: &str = "environment_id, worker_id, last_seen_at, current_work_id, hostname, version";
const API_KEY_COLS: &str =
    "id, name, role, prefix, key_hash, created_at, expires_at, last_used_at, revoked_at";
const ENV_KEY_COLS: &str =
    "id, environment_id, name, prefix, key_hash, created_at, expires_at, last_used_at, revoked_at";
const CRED_COLS: &str = "id, name, sealed, key_id, created_at, updated_at";

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound,
            other => StoreError::Backend(other.to_string()),
        }
    }
}

/// Fixed-width UTC text, so string comparison is time comparison.
fn ts(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

fn conv_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn parse_ts(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(conv_err)
}

fn ts_col(row: &Row, name: &str) -> rusqlite::Result<DateTime<Utc>> {
    let s: String = row.get(name)?;
    parse_ts(&s)
}

fn ts_opt(row: &Row, name: &str) -> rusqlite::Result<Option<DateTime<Utc>>> {
    let s: Option<String> = row.get(name)?;
    s.map(|s| parse_ts(&s)).transpose()
}

fn json_col(row: &Row, name: &str) -> rusqlite::Result<Value> {
    let s: String = row.get(name)?;
    serde_json::from_str(&s).map_err(conv_err)
}

fn js(v: &Value) -> String {
    v.to_string()
}

fn agent_row(row: &Row) -> rusqlite::Result<AgentRecord> {
    Ok(AgentRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        instructions: row.get("instructions")?,
        model: row.get("model")?,
        tools: json_col(row, "tools")?,
        mcp_servers: json_col(row, "mcp_servers")?,
        skills: json_col(row, "skills")?,
        metadata: json_col(row, "metadata")?,
        version: row.get("version")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

fn env_row(row: &Row) -> rusqlite::Result<EnvironmentRecord> {
    let hosts = json_col(row, "allowed_hosts")?;
    Ok(EnvironmentRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        network: row.get("network")?,
        allowed_hosts: serde_json::from_value(hosts).map_err(conv_err)?,
        git: json_col(row, "git")?,
        mcp: json_col(row, "mcp")?,
        image: row.get("image")?,
        packages: json_col(row, "packages")?,
        metadata: json_col(row, "metadata")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

fn session_row(row: &Row) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get("id")?,
        agent_id: row.get("agent_id")?,
        environment_id: row.get("environment_id")?,
        title: row.get("title")?,
        status: row.get("status")?,
        status_reason: row.get("status_reason")?,
        last_seq: row.get("last_seq")?,
        metadata: json_col(row, "metadata")?,
        agent_snapshot: json_col(row, "agent_snapshot")?,
        gizzi_session_id: row.get("gizzi_session_id")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
        archived_at: ts_opt(row, "archived_at")?,
    })
}

fn event_row(row: &Row) -> rusqlite::Result<EventRecord> {
    Ok(EventRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        seq: row.get("seq")?,
        event_type: row.get("type")?,
        data: json_col(row, "data")?,
        created_at: ts_col(row, "created_at")?,
    })
}

fn work_row(row: &Row) -> rusqlite::Result<WorkItem> {
    let status: String = row.get("status")?;
    Ok(WorkItem {
        id: row.get("id")?,
        environment_id: row.get("environment_id")?,
        session_id: row.get("session_id")?,
        status: WorkStatus::parse(&status).map_err(conv_err)?,
        lease_generation: row.get("lease_generation")?,
        lease_expires_at: ts_opt(row, "lease_expires_at")?,
        worker_id: row.get("worker_id")?,
        error_message: row.get("error_message")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
        completed_at: ts_opt(row, "completed_at")?,
    })
}

fn tool_row(row: &Row) -> rusqlite::Result<ToolCallRecord> {
    let status: String = row.get("status")?;
    Ok(ToolCallRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        work_id: row.get("work_id")?,
        claimed_generation: row.get("claimed_generation")?,
        name: row.get("name")?,
        input: json_col(row, "input")?,
        status: ToolCallStatus::parse(&status).map_err(conv_err)?,
        output: row.get("output")?,
        is_error: row.get("is_error")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
        completed_at: ts_opt(row, "completed_at")?,
    })
}

fn worker_row(row: &Row) -> rusqlite::Result<WorkerRecord> {
    Ok(WorkerRecord {
        environment_id: row.get("environment_id")?,
        worker_id: row.get("worker_id")?,
        last_seen_at: ts_col(row, "last_seen_at")?,
        current_work_id: row.get("current_work_id")?,
        hostname: row.get("hostname")?,
        version: row.get("version")?,
    })
}

fn api_key_row(row: &Row) -> rusqlite::Result<ApiKeyRecord> {
    let role: String = row.get("role")?;
    Ok(ApiKeyRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        role: Role::parse(&role)
            .ok_or_else(|| conv_err(StoreError::Backend(format!("unknown role {role}"))))?,
        prefix: row.get("prefix")?,
        key_hash: row.get("key_hash")?,
        created_at: ts_col(row, "created_at")?,
        expires_at: ts_opt(row, "expires_at")?,
        last_used_at: ts_opt(row, "last_used_at")?,
        revoked_at: ts_opt(row, "revoked_at")?,
    })
}

fn env_key_row(row: &Row) -> rusqlite::Result<EnvironmentKeyRecord> {
    Ok(EnvironmentKeyRecord {
        id: row.get("id")?,
        environment_id: row.get("environment_id")?,
        name: row.get("name")?,
        prefix: row.get("prefix")?,
        key_hash: row.get("key_hash")?,
        created_at: ts_col(row, "created_at")?,
        expires_at: ts_opt(row, "expires_at")?,
        last_used_at: ts_opt(row, "last_used_at")?,
        revoked_at: ts_opt(row, "revoked_at")?,
    })
}

fn cred_row(row: &Row) -> rusqlite::Result<CredentialRecord> {
    Ok(CredentialRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        sealed: row.get("sealed")?,
        key_id: row.get("key_id")?,
        created_at: ts_col(row, "created_at")?,
        updated_at: ts_col(row, "updated_at")?,
    })
}

fn get_agent_live(conn: &Connection, id: &str) -> StoreResult<AgentRecord> {
    Ok(conn.query_row(
        &format!("SELECT {AGENT_COLS} FROM agents WHERE id = ?1 AND deleted_at IS NULL"),
        params![id],
        agent_row,
    )?)
}

fn get_env_live(conn: &Connection, id: &str) -> StoreResult<EnvironmentRecord> {
    Ok(conn.query_row(
        &format!("SELECT {ENV_COLS} FROM environments WHERE id = ?1 AND deleted_at IS NULL"),
        params![id],
        env_row,
    )?)
}

fn get_session_c(conn: &Connection, id: &str) -> StoreResult<SessionRecord> {
    Ok(conn.query_row(
        &format!("SELECT {SESSION_COLS} FROM sessions WHERE id = ?1"),
        params![id],
        session_row,
    )?)
}

fn get_work_c(conn: &Connection, id: &str) -> StoreResult<WorkItem> {
    Ok(conn.query_row(
        &format!("SELECT {WORK_COLS} FROM work_items WHERE id = ?1"),
        params![id],
        work_row,
    )?)
}

fn get_tool_c(conn: &Connection, id: &str) -> StoreResult<ToolCallRecord> {
    Ok(conn.query_row(
        &format!("SELECT {TOOL_COLS} FROM tool_calls WHERE id = ?1"),
        params![id],
        tool_row,
    )?)
}

fn get_api_key_c(conn: &Connection, id: &str) -> StoreResult<ApiKeyRecord> {
    Ok(conn.query_row(
        &format!("SELECT {API_KEY_COLS} FROM api_keys WHERE id = ?1"),
        params![id],
        api_key_row,
    )?)
}

/// `created_at` of the cursor row. `table` is always a constant.
fn cursor_ts(conn: &Connection, table: &str, id: &str) -> StoreResult<String> {
    conn.query_row(
        &format!("SELECT created_at FROM {table} WHERE id = ?1"),
        params![id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .ok_or(StoreError::InvalidCursor)
}

/// Oldest-first page of a table with a `created_at` column.
fn page_oldest<T>(
    conn: &Connection,
    table: &str,
    cols: &str,
    extra_where: &str,
    extra_params: Vec<SqlValue>,
    limit: i64,
    after: Option<&str>,
    map: fn(&Row) -> rusqlite::Result<T>,
) -> StoreResult<Vec<T>> {
    let mut sql = format!("SELECT {cols} FROM {table} WHERE 1=1 {extra_where}");
    let mut p = extra_params;
    if let Some(after) = after {
        let c = cursor_ts(conn, table, after)?;
        sql.push_str(" AND (created_at > ? OR (created_at = ? AND id > ?))");
        p.push(SqlValue::Text(c.clone()));
        p.push(SqlValue::Text(c));
        p.push(SqlValue::Text(after.to_string()));
    }
    sql.push_str(" ORDER BY created_at, id LIMIT ?");
    p.push(SqlValue::Integer(limit.max(0)));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(p), map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<T>>>()?)
}

pub struct SqliteStore {
    conn: Arc<Mutex<Connection>>,
    kind: &'static str,
}

impl SqliteStore {
    /// Open (or create) a database file and apply the schema.
    pub fn open(path: &Path) -> StoreResult<Self> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(|e| StoreError::Backend(e.to_string()))?;
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn, "sqlite")
    }

    /// A private in-memory database. Gone when the process exits.
    pub fn open_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn, "memory")
    }

    fn init(conn: Connection, kind: &'static str) -> StoreResult<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        add_trace_columns(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            kind,
        })
    }

    async fn call<T, F>(&self, f: F) -> StoreResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> StoreResult<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| StoreError::Backend("sqlite connection mutex poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| StoreError::Backend(format!("blocking task failed: {e}")))?
    }
}

/// Step 13: a nullable `traceparent` on sessions, work items and tool calls.
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, so look first. Safe to run on
/// every open.
fn add_trace_columns(conn: &Connection) -> StoreResult<()> {
    for target in [TraceTarget::Session, TraceTarget::Work, TraceTarget::ToolCall] {
        let table = target.table();
        let cols: Vec<String> = {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let has = cols.iter().any(|c| c == "traceparent");
        if !has {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN traceparent TEXT"))?;
        }
    }
    Ok(())
}

/// Leased, matching generation, or LeaseStale (NotFound when missing).
fn fenced(conn: &Connection, id: &str, generation: i64) -> StoreResult<WorkItem> {
    let item = get_work_c(conn, id)?;
    if item.status != WorkStatus::Leased || item.lease_generation != generation {
        return Err(StoreError::LeaseStale);
    }
    Ok(item)
}

#[async_trait]
impl SelfHostStore for SqliteStore {
    fn backend(&self) -> &'static str {
        self.kind
    }

    async fn health(&self) -> StoreResult<()> {
        self.call(|c| {
            c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?;
            Ok(())
        })
        .await
    }

    // ── Agents ──

    async fn create_agent(&self, new: NewAgent) -> StoreResult<AgentRecord> {
        self.call(move |c| {
            let id = new_id("agent");
            let now = ts(Utc::now());
            c.execute(
                "INSERT INTO agents (id, name, instructions, model, tools, mcp_servers, skills, metadata, version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
                params![
                    id,
                    new.name,
                    new.instructions,
                    new.model,
                    js(&or_array(new.tools)),
                    js(&or_array(new.mcp_servers)),
                    js(&or_array(new.skills)),
                    js(&or_object(new.metadata)),
                    now
                ],
            )?;
            get_agent_live(c, &id)
        })
        .await
    }

    async fn get_agent(&self, id: &str) -> StoreResult<AgentRecord> {
        let id = id.to_string();
        self.call(move |c| get_agent_live(c, &id)).await
    }

    async fn list_agents(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<AgentRecord>> {
        let after = after.map(str::to_string);
        self.call(move |c| {
            page_oldest(
                c,
                "agents",
                AGENT_COLS,
                "AND deleted_at IS NULL",
                vec![],
                limit,
                after.as_deref(),
                agent_row,
            )
        })
        .await
    }

    async fn update_agent(&self, id: &str, patch: AgentPatch) -> StoreResult<AgentRecord> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cur = get_agent_live(&tx, &id)?;
            let name = patch.name.unwrap_or(cur.name);
            let instructions = patch.instructions.unwrap_or(cur.instructions);
            let model = patch.model.unwrap_or(cur.model);
            let tools = patch.tools.map(or_array).unwrap_or(cur.tools);
            let mcp = patch.mcp_servers.map(or_array).unwrap_or(cur.mcp_servers);
            let skills = patch.skills.map(or_array).unwrap_or(cur.skills);
            let metadata = patch.metadata.map(or_object).unwrap_or(cur.metadata);
            tx.execute(
                "UPDATE agents SET name = ?2, instructions = ?3, model = ?4, tools = ?5, mcp_servers = ?6,
                     skills = ?7, metadata = ?8, version = version + 1, updated_at = ?9
                 WHERE id = ?1",
                params![
                    id,
                    name,
                    instructions,
                    model,
                    js(&tools),
                    js(&mcp),
                    js(&skills),
                    js(&metadata),
                    ts(Utc::now())
                ],
            )?;
            let out = get_agent_live(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn delete_agent(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            get_agent_live(&tx, &id)?;
            let in_use: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE agent_id = ?1 AND archived_at IS NULL",
                params![id],
                |r| r.get(0),
            )?;
            if in_use > 0 {
                return Err(StoreError::Conflict("agent_in_use"));
            }
            tx.execute(
                "UPDATE agents SET deleted_at = ?2 WHERE id = ?1",
                params![id, ts(Utc::now())],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    // ── Environments ──

    async fn create_environment(&self, new: NewEnvironment) -> StoreResult<EnvironmentRecord> {
        check_network(&new.network)?;
        self.call(move |c| {
            let id = new_id("env");
            let now = ts(Utc::now());
            c.execute(
                "INSERT INTO environments (id, name, network, allowed_hosts, git, mcp, image, packages, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    new.name,
                    new.network,
                    js(&Value::from(new.allowed_hosts)),
                    js(&or_object(new.git)),
                    js(&or_object(new.mcp)),
                    new.image,
                    js(&or_object(new.packages)),
                    js(&or_object(new.metadata)),
                    now
                ],
            )?;
            get_env_live(c, &id)
        })
        .await
    }

    async fn get_environment(&self, id: &str) -> StoreResult<EnvironmentRecord> {
        let id = id.to_string();
        self.call(move |c| get_env_live(c, &id)).await
    }

    async fn list_environments(
        &self,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentRecord>> {
        let after = after.map(str::to_string);
        self.call(move |c| {
            page_oldest(
                c,
                "environments",
                ENV_COLS,
                "AND deleted_at IS NULL",
                vec![],
                limit,
                after.as_deref(),
                env_row,
            )
        })
        .await
    }

    async fn update_environment(
        &self,
        id: &str,
        patch: EnvironmentPatch,
    ) -> StoreResult<EnvironmentRecord> {
        if let Some(n) = &patch.network {
            check_network(n)?;
        }
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cur = get_env_live(&tx, &id)?;
            let name = patch.name.unwrap_or(cur.name);
            let network = patch.network.unwrap_or(cur.network);
            let hosts = patch.allowed_hosts.unwrap_or(cur.allowed_hosts);
            let git = patch.git.map(or_object).unwrap_or(cur.git);
            let mcp = patch.mcp.map(or_object).unwrap_or(cur.mcp);
            let image = patch.image.unwrap_or(cur.image);
            let packages = patch.packages.map(or_object).unwrap_or(cur.packages);
            let metadata = patch.metadata.map(or_object).unwrap_or(cur.metadata);
            tx.execute(
                "UPDATE environments SET name = ?2, network = ?3, allowed_hosts = ?4, git = ?5, mcp = ?6,
                     image = ?7, packages = ?8, metadata = ?9, updated_at = ?10
                 WHERE id = ?1",
                params![
                    id,
                    name,
                    network,
                    js(&Value::from(hosts)),
                    js(&git),
                    js(&mcp),
                    image,
                    js(&packages),
                    js(&metadata),
                    ts(Utc::now())
                ],
            )?;
            let out = get_env_live(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn delete_environment(&self, id: &str) -> StoreResult<()> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            get_env_live(&tx, &id)?;
            let in_use: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sessions WHERE environment_id = ?1 AND archived_at IS NULL",
                params![id],
                |r| r.get(0),
            )?;
            if in_use > 0 {
                return Err(StoreError::Conflict("environment_in_use"));
            }
            let now = ts(Utc::now());
            tx.execute(
                "UPDATE environment_keys SET revoked_at = ?2 WHERE environment_id = ?1 AND revoked_at IS NULL",
                params![id, now],
            )?;
            tx.execute(
                "UPDATE environments SET deleted_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    // ── Sessions ──

    async fn create_session(&self, new: NewSession) -> StoreResult<SessionRecord> {
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let agent = get_agent_live(&tx, &new.agent_id)?;
            get_env_live(&tx, &new.environment_id)?;
            let id = new_id("sess");
            let now = ts(Utc::now());
            tx.execute(
                "INSERT INTO sessions (id, agent_id, environment_id, title, status, last_seq, metadata, agent_snapshot, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'idle', 0, ?5, ?6, ?7, ?7)",
                params![
                    id,
                    new.agent_id,
                    new.environment_id,
                    new.title,
                    js(&or_object(new.metadata)),
                    js(&agent_snapshot(&agent)),
                    now
                ],
            )?;
            let out = get_session_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn get_session(&self, id: &str) -> StoreResult<SessionRecord> {
        let id = id.to_string();
        self.call(move |c| get_session_c(c, &id)).await
    }

    async fn list_sessions(
        &self,
        filter: &SessionFilter,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<SessionRecord>> {
        let filter = filter.clone();
        let after = after.map(str::to_string);
        self.call(move |c| {
            let mut sql = format!("SELECT {SESSION_COLS} FROM sessions WHERE 1=1");
            let mut p: Vec<SqlValue> = vec![];
            if let Some(a) = filter.agent_id {
                sql.push_str(" AND agent_id = ?");
                p.push(SqlValue::Text(a));
            }
            if let Some(e) = filter.environment_id {
                sql.push_str(" AND environment_id = ?");
                p.push(SqlValue::Text(e));
            }
            if let Some(s) = filter.status {
                sql.push_str(" AND status = ?");
                p.push(SqlValue::Text(s));
            }
            if !filter.include_archived {
                sql.push_str(" AND archived_at IS NULL");
            }
            if let Some(after) = after {
                let cur = cursor_ts(c, "sessions", &after)?;
                sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
                p.push(SqlValue::Text(cur.clone()));
                p.push(SqlValue::Text(cur));
                p.push(SqlValue::Text(after));
            }
            sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
            p.push(SqlValue::Integer(limit.max(0)));
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(p), session_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    async fn set_session_status(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> StoreResult<SessionRecord> {
        check_session_status(status)?;
        let (id, status, reason) = (id.to_string(), status.to_string(), reason.map(str::to_string));
        self.call(move |c| {
            let n = c.execute(
                "UPDATE sessions SET status = ?2, status_reason = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, status, reason, ts(Utc::now())],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            get_session_c(c, &id)
        })
        .await
    }

    async fn transition_session_status(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
        reason: Option<&str>,
    ) -> StoreResult<Option<SessionRecord>> {
        check_session_status(to)?;
        let from: Vec<String> = from.iter().map(|s| s.to_string()).collect();
        let (id, to, reason) = (id.to_string(), to.to_string(), reason.map(str::to_string));
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cur = get_session_c(&tx, &id)?;
            if !from.iter().any(|f| *f == cur.status) {
                return Ok(None);
            }
            tx.execute(
                "UPDATE sessions SET status = ?2, status_reason = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, to, reason, ts(Utc::now())],
            )?;
            let out = get_session_c(&tx, &id)?;
            tx.commit()?;
            Ok(Some(out))
        })
        .await
    }

    async fn set_gizzi_session_id(&self, id: &str, gizzi_session_id: &str) -> StoreResult<()> {
        let (id, g) = (id.to_string(), gizzi_session_id.to_string());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE sessions SET gizzi_session_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, g, ts(Utc::now())],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn archive_session(&self, id: &str) -> StoreResult<SessionRecord> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cur = get_session_c(&tx, &id)?;
            if cur.archived_at.is_some() {
                return Ok(cur);
            }
            let now = ts(Utc::now());
            tx.execute(
                "UPDATE sessions SET status = 'archived', status_reason = NULL, archived_at = ?2, updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            let out = get_session_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    // ── Events ──

    async fn append_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: Value,
    ) -> StoreResult<EventRecord> {
        check_event_type(event_type)?;
        let (sid, et) = (session_id.to_string(), event_type.to_string());
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = ts(Utc::now());
            let n = tx.execute(
                "UPDATE sessions SET last_seq = last_seq + 1, updated_at = ?2 WHERE id = ?1",
                params![sid, now],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            let seq: i64 =
                tx.query_row("SELECT last_seq FROM sessions WHERE id = ?1", params![sid], |r| r.get(0))?;
            let id = new_id("evt");
            tx.execute(
                "INSERT INTO session_events (id, session_id, seq, type, data, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, sid, seq, et, js(&data), now],
            )?;
            let out = tx.query_row(
                &format!("SELECT {EVENT_COLS} FROM session_events WHERE id = ?1"),
                params![id],
                event_row,
            )?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn list_events(
        &self,
        session_id: &str,
        after_seq: i64,
        limit: i64,
        types: Option<&[String]>,
    ) -> StoreResult<Vec<EventRecord>> {
        let sid = session_id.to_string();
        let types: Option<Vec<String>> = types.map(|t| t.to_vec());
        self.call(move |c| {
            let mut sql =
                format!("SELECT {EVENT_COLS} FROM session_events WHERE session_id = ? AND seq > ?");
            let mut p: Vec<SqlValue> = vec![SqlValue::Text(sid), SqlValue::Integer(after_seq)];
            if let Some(types) = types.filter(|t| !t.is_empty()) {
                let marks = vec!["?"; types.len()].join(", ");
                sql.push_str(&format!(" AND type IN ({marks})"));
                p.extend(types.into_iter().map(SqlValue::Text));
            }
            sql.push_str(" ORDER BY seq LIMIT ?");
            p.push(SqlValue::Integer(limit.max(0)));
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(p), event_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    // ── Work queue ──

    async fn enqueue_work(&self, environment_id: &str, session_id: &str) -> StoreResult<WorkItem> {
        let (env, sid) = (environment_id.to_string(), session_id.to_string());
        self.call(move |c| {
            let id = new_id("work");
            let now = ts(Utc::now());
            c.execute(
                "INSERT INTO work_items (id, environment_id, session_id, status, lease_generation, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'ready', 0, ?4, ?4)",
                params![id, env, sid, now],
            )?;
            get_work_c(c, &id)
        })
        .await
    }

    async fn get_work(&self, id: &str) -> StoreResult<WorkItem> {
        let id = id.to_string();
        self.call(move |c| get_work_c(c, &id)).await
    }

    async fn find_open_work_for_session(&self, session_id: &str) -> StoreResult<Option<WorkItem>> {
        let sid = session_id.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {WORK_COLS} FROM work_items WHERE session_id = ?1 AND status IN ('ready','leased')
                     ORDER BY created_at DESC, id DESC LIMIT 1"
                ),
                params![sid],
                work_row,
            )
            .optional()?)
        })
        .await
    }

    async fn claim_work(&self, req: ClaimRequest) -> StoreResult<Option<WorkClaim>> {
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cutoff: Option<String> = req
                .reclaim_older_than_ms
                .map(|ms| ts(req.now - Duration::milliseconds(ms)));
            let candidate: Option<String> = tx
                .query_row(
                    "SELECT id FROM work_items
                     WHERE environment_id = ?1
                       AND (status = 'ready' OR (?2 IS NOT NULL AND status = 'leased' AND lease_expires_at <= ?2))
                     ORDER BY created_at, id LIMIT 1",
                    params![req.environment_id, cutoff],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(work_id) = candidate else {
                return Ok(None);
            };
            let secret = new_secret("ss");
            let now = ts(req.now);
            // Conditional on the same predicate: if the row changed under
            // us, nothing is updated and the claim reports no work.
            let changed = tx.execute(
                "UPDATE work_items
                 SET status = 'leased', lease_generation = lease_generation + 1, lease_expires_at = ?3,
                     worker_id = ?4, session_secret_hash = ?5, updated_at = ?6
                 WHERE id = ?1
                   AND (status = 'ready' OR (?2 IS NOT NULL AND status = 'leased' AND lease_expires_at <= ?2))",
                params![
                    work_id,
                    cutoff,
                    ts(lease_end(req.now)),
                    req.worker_id,
                    hash_secret(&secret),
                    now
                ],
            )?;
            if changed != 1 {
                return Ok(None);
            }
            let item = get_work_c(&tx, &work_id)?;
            tx.execute(
                "UPDATE tool_calls SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = ?2
                 WHERE session_id = ?1 AND status = 'claimed'",
                params![item.session_id, now],
            )?;
            tx.execute(
                &format!(
                    "INSERT INTO workers ({WORKER_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (environment_id, worker_id) DO UPDATE SET
                       last_seen_at = excluded.last_seen_at,
                       current_work_id = excluded.current_work_id,
                       hostname = COALESCE(excluded.hostname, workers.hostname),
                       version = COALESCE(excluded.version, workers.version)"
                ),
                params![
                    req.environment_id,
                    req.worker_id,
                    now,
                    work_id,
                    req.hostname,
                    req.version
                ],
            )?;
            tx.commit()?;
            Ok(Some(WorkClaim {
                item,
                session_secret: secret,
            }))
        })
        .await
    }

    async fn heartbeat_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            fenced(&tx, &id, generation)?;
            tx.execute(
                "UPDATE work_items SET updated_at = ?2 WHERE id = ?1",
                params![id, ts(now)],
            )?;
            let out = get_work_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn renew_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            fenced(&tx, &id, generation)?;
            tx.execute(
                "UPDATE work_items SET lease_expires_at = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, ts(lease_end(now)), ts(now)],
            )?;
            let out = get_work_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn complete_work(
        &self,
        id: &str,
        generation: i64,
        outcome: WorkOutcome,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let id = id.to_string();
        let msg = error_message.map(str::to_string);
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let item = get_work_c(&tx, &id)?;
            if item.lease_generation != generation {
                return Err(StoreError::LeaseStale);
            }
            if item.status.is_terminal() {
                return Ok(item);
            }
            if item.status != WorkStatus::Leased {
                return Err(StoreError::LeaseStale);
            }
            tx.execute(
                "UPDATE work_items SET status = ?2, error_message = ?3, lease_expires_at = NULL,
                     session_secret_hash = NULL, completed_at = ?4, updated_at = ?4
                 WHERE id = ?1",
                params![id, outcome.status().as_str(), msg, ts(now)],
            )?;
            let out = get_work_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn release_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let item = fenced(&tx, &id, generation)?;
            let now = ts(now);
            tx.execute(
                "UPDATE work_items SET status = 'ready', lease_expires_at = NULL, worker_id = NULL,
                     session_secret_hash = NULL, updated_at = ?2
                 WHERE id = ?1",
                params![id, now],
            )?;
            tx.execute(
                "UPDATE tool_calls SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = ?2
                 WHERE session_id = ?1 AND status = 'claimed'",
                params![item.session_id, now],
            )?;
            let out = get_work_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn requeue_expired_work(&self, now: DateTime<Utc>, grace: Duration) -> StoreResult<u64> {
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cutoff = ts(now - grace);
            let stamp = ts(now);
            tx.execute(
                "UPDATE tool_calls SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = ?2
                 WHERE status = 'claimed' AND session_id IN (
                     SELECT session_id FROM work_items WHERE status = 'leased' AND lease_expires_at <= ?1)",
                params![cutoff, stamp],
            )?;
            let n = tx.execute(
                "UPDATE work_items SET status = 'ready', lease_expires_at = NULL, worker_id = NULL,
                     session_secret_hash = NULL, updated_at = ?2
                 WHERE status = 'leased' AND lease_expires_at <= ?1",
                params![cutoff, stamp],
            )?;
            tx.commit()?;
            Ok(n as u64)
        })
        .await
    }

    async fn cancel_open_work_for_session(&self, session_id: &str) -> StoreResult<u64> {
        let sid = session_id.to_string();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE work_items SET status = 'cancelled', lease_expires_at = NULL, session_secret_hash = NULL,
                     completed_at = ?2, updated_at = ?2
                 WHERE session_id = ?1 AND status IN ('ready','leased')",
                params![sid, ts(Utc::now())],
            )?;
            Ok(n as u64)
        })
        .await
    }

    async fn find_work_by_secret_hash(
        &self,
        secret_hash: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<WorkItem>> {
        let h = secret_hash.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {WORK_COLS} FROM work_items
                     WHERE session_secret_hash = ?1 AND status = 'leased' AND lease_expires_at > ?2"
                ),
                params![h, ts(now)],
                work_row,
            )
            .optional()?)
        })
        .await
    }

    async fn queue_depth(&self, environment_id: &str) -> StoreResult<i64> {
        let env = environment_id.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM work_items WHERE environment_id = ?1 AND status = 'ready'",
                params![env],
                |r| r.get(0),
            )?)
        })
        .await
    }

    // ── Tool calls ──

    async fn insert_tool_call(&self, new: NewToolCall) -> StoreResult<(ToolCallRecord, bool)> {
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let now = ts(Utc::now());
            let n = tx.execute(
                "INSERT INTO tool_calls (id, session_id, name, input, status, is_error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?5)
                 ON CONFLICT (id) DO NOTHING",
                params![new.id, new.session_id, new.name, js(&new.input), now],
            )?;
            let call = get_tool_c(&tx, &new.id)?;
            if call.session_id != new.session_id {
                return Err(StoreError::Conflict("tool_call_conflict"));
            }
            tx.commit()?;
            Ok((call, n == 1))
        })
        .await
    }

    async fn get_tool_call(&self, id: &str) -> StoreResult<ToolCallRecord> {
        let id = id.to_string();
        self.call(move |c| get_tool_c(c, &id)).await
    }

    async fn next_tool_call(
        &self,
        work_id: &str,
        generation: i64,
    ) -> StoreResult<Option<ToolCallRecord>> {
        let wid = work_id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let item = fenced(&tx, &wid, generation)?;
            let next: Option<String> = tx
                .query_row(
                    "SELECT id FROM tool_calls WHERE session_id = ?1 AND status = 'pending'
                     ORDER BY created_at, id LIMIT 1",
                    params![item.session_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(tc) = next else {
                return Ok(None);
            };
            let changed = tx.execute(
                "UPDATE tool_calls SET status = 'claimed', work_id = ?2, claimed_generation = ?3, updated_at = ?4
                 WHERE id = ?1 AND status = 'pending'",
                params![tc, wid, generation, ts(Utc::now())],
            )?;
            if changed != 1 {
                return Ok(None);
            }
            let out = get_tool_c(&tx, &tc)?;
            tx.commit()?;
            Ok(Some(out))
        })
        .await
    }

    async fn submit_tool_result(
        &self,
        id: &str,
        output: &str,
        is_error: bool,
    ) -> StoreResult<ToolResultOutcome> {
        check_result_size(output)?;
        let (id, output) = (id.to_string(), output.to_string());
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cur = get_tool_c(&tx, &id)?;
            if cur.status == ToolCallStatus::Done {
                return Ok(ToolResultOutcome {
                    call: cur,
                    stored: false,
                });
            }
            let now = ts(Utc::now());
            tx.execute(
                "UPDATE tool_calls SET status = 'done', output = ?2, is_error = ?3, completed_at = ?4, updated_at = ?4
                 WHERE id = ?1",
                params![id, output, is_error, now],
            )?;
            let call = get_tool_c(&tx, &id)?;
            tx.commit()?;
            Ok(ToolResultOutcome { call, stored: true })
        })
        .await
    }

    async fn cancel_open_tool_calls(&self, session_id: &str, output: &str) -> StoreResult<u64> {
        check_result_size(output)?;
        let (sid, output) = (session_id.to_string(), output.to_string());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE tool_calls SET status = 'done', output = ?2, is_error = 1, completed_at = ?3, updated_at = ?3
                 WHERE session_id = ?1 AND status <> 'done'",
                params![sid, output, ts(Utc::now())],
            )?;
            Ok(n as u64)
        })
        .await
    }

    // ── Workers ──

    async fn touch_worker(
        &self,
        environment_id: &str,
        worker_id: &str,
        current_work_id: Option<&str>,
        hostname: Option<&str>,
        version: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<()> {
        let args = (
            environment_id.to_string(),
            worker_id.to_string(),
            current_work_id.map(str::to_string),
            hostname.map(str::to_string),
            version.map(str::to_string),
        );
        self.call(move |c| {
            let (env, wid, cur, host, ver) = args;
            c.execute(
                &format!(
                    "INSERT INTO workers ({WORKER_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (environment_id, worker_id) DO UPDATE SET
                       last_seen_at = excluded.last_seen_at,
                       current_work_id = excluded.current_work_id,
                       hostname = COALESCE(excluded.hostname, workers.hostname),
                       version = COALESCE(excluded.version, workers.version)"
                ),
                params![env, wid, ts(now), cur, host, ver],
            )?;
            Ok(())
        })
        .await
    }

    async fn list_workers(
        &self,
        environment_id: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<Vec<WorkerRecord>> {
        let env = environment_id.to_string();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {WORKER_COLS} FROM workers WHERE environment_id = ?1 AND last_seen_at >= ?2
                 ORDER BY last_seen_at DESC, worker_id"
            ))?;
            let rows = stmt.query_map(params![env, ts(since)], worker_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    async fn count_workers(&self, environment_id: &str, since: DateTime<Utc>) -> StoreResult<i64> {
        let env = environment_id.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM workers WHERE environment_id = ?1 AND last_seen_at >= ?2",
                params![env, ts(since)],
                |r| r.get(0),
            )?)
        })
        .await
    }

    // ── Server API keys ──

    async fn count_api_keys(&self) -> StoreResult<i64> {
        self.call(|c| Ok(c.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))?))
            .await
    }

    async fn create_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
        self.call(move |c| insert_api_key(c, new)).await
    }

    async fn create_first_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let n: i64 = tx.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))?;
            if n > 0 {
                return Err(StoreError::Conflict("setup_already_done"));
            }
            let out = insert_api_key(&tx, new)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn find_api_key_by_hash(&self, key_hash: &str) -> StoreResult<Option<ApiKeyRecord>> {
        let h = key_hash.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {API_KEY_COLS} FROM api_keys WHERE key_hash = ?1"),
                params![h],
                api_key_row,
            )
            .optional()?)
        })
        .await
    }

    async fn list_api_keys(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<ApiKeyRecord>> {
        let after = after.map(str::to_string);
        self.call(move |c| {
            page_oldest(c, "api_keys", API_KEY_COLS, "", vec![], limit, after.as_deref(), api_key_row)
        })
        .await
    }

    async fn revoke_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<ApiKeyRecord> {
        let id = id.to_string();
        self.call(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let key = get_api_key_c(&tx, &id)?;
            if key.revoked_at.is_some() {
                return Ok(key);
            }
            let stamp = ts(now);
            if key.role == Role::Admin && key.is_active(now) {
                let others: i64 = tx.query_row(
                    "SELECT COUNT(*) FROM api_keys WHERE role = 'admin' AND id <> ?1 AND revoked_at IS NULL
                       AND (expires_at IS NULL OR expires_at > ?2)",
                    params![id, stamp],
                    |r| r.get(0),
                )?;
                if others == 0 {
                    return Err(StoreError::Conflict("last_admin_key"));
                }
            }
            tx.execute(
                "UPDATE api_keys SET revoked_at = ?2 WHERE id = ?1",
                params![id, stamp],
            )?;
            let out = get_api_key_c(&tx, &id)?;
            tx.commit()?;
            Ok(out)
        })
        .await
    }

    async fn touch_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()> {
        let id = id.to_string();
        self.call(move |c| {
            c.execute(
                "UPDATE api_keys SET last_used_at = ?2 WHERE id = ?1",
                params![id, ts(now)],
            )?;
            Ok(())
        })
        .await
    }

    // ── Environment keys ──

    async fn create_environment_key(
        &self,
        new: NewEnvironmentKey,
    ) -> StoreResult<EnvironmentKeyRecord> {
        self.call(move |c| {
            get_env_live(c, &new.environment_id)?;
            let id = new_id("ekey");
            c.execute(
                "INSERT INTO environment_keys (id, environment_id, name, prefix, key_hash, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    id,
                    new.environment_id,
                    new.name,
                    new.prefix,
                    new.key_hash,
                    ts(Utc::now()),
                    new.expires_at.map(ts)
                ],
            )?;
            Ok(c.query_row(
                &format!("SELECT {ENV_KEY_COLS} FROM environment_keys WHERE id = ?1"),
                params![id],
                env_key_row,
            )?)
        })
        .await
    }

    async fn find_environment_key_by_hash(
        &self,
        key_hash: &str,
    ) -> StoreResult<Option<EnvironmentKeyRecord>> {
        let h = key_hash.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {ENV_KEY_COLS} FROM environment_keys WHERE key_hash = ?1"),
                params![h],
                env_key_row,
            )
            .optional()?)
        })
        .await
    }

    async fn list_environment_keys(
        &self,
        environment_id: &str,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentKeyRecord>> {
        let env = environment_id.to_string();
        let after = after.map(str::to_string);
        self.call(move |c| {
            page_oldest(
                c,
                "environment_keys",
                ENV_KEY_COLS,
                "AND environment_id = ?",
                vec![SqlValue::Text(env)],
                limit,
                after.as_deref(),
                env_key_row,
            )
        })
        .await
    }

    async fn revoke_environment_key(
        &self,
        environment_id: &str,
        id: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<EnvironmentKeyRecord> {
        let (env, id) = (environment_id.to_string(), id.to_string());
        self.call(move |c| {
            c.execute(
                "UPDATE environment_keys SET revoked_at = ?3
                 WHERE id = ?1 AND environment_id = ?2 AND revoked_at IS NULL",
                params![id, env, ts(now)],
            )?;
            Ok(c.query_row(
                &format!(
                    "SELECT {ENV_KEY_COLS} FROM environment_keys WHERE id = ?1 AND environment_id = ?2"
                ),
                params![id, env],
                env_key_row,
            )?)
        })
        .await
    }

    async fn touch_environment_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()> {
        let id = id.to_string();
        self.call(move |c| {
            c.execute(
                "UPDATE environment_keys SET last_used_at = ?2 WHERE id = ?1",
                params![id, ts(now)],
            )?;
            Ok(())
        })
        .await
    }

    // ── Credentials ──

    async fn put_credential(
        &self,
        name: &str,
        sealed: &[u8],
        key_id: &str,
    ) -> StoreResult<CredentialRecord> {
        let (name, sealed, key_id) = (name.to_string(), sealed.to_vec(), key_id.to_string());
        self.call(move |c| {
            let now = ts(Utc::now());
            c.execute(
                "INSERT INTO credentials (id, name, sealed, key_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, key_id = excluded.key_id,
                     updated_at = excluded.updated_at",
                params![new_id("cred"), name, sealed, key_id, now],
            )?;
            Ok(c.query_row(
                &format!("SELECT {CRED_COLS} FROM credentials WHERE name = ?1"),
                params![name],
                cred_row,
            )?)
        })
        .await
    }

    async fn get_credential(&self, name: &str) -> StoreResult<CredentialRecord> {
        let name = name.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {CRED_COLS} FROM credentials WHERE name = ?1"),
                params![name],
                cred_row,
            )?)
        })
        .await
    }

    async fn list_credentials(&self) -> StoreResult<Vec<CredentialRecord>> {
        self.call(|c| {
            let mut stmt =
                c.prepare(&format!("SELECT {CRED_COLS} FROM credentials ORDER BY name"))?;
            let rows = stmt.query_map([], cred_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }

    async fn delete_credential(&self, name: &str) -> StoreResult<()> {
        let name = name.to_string();
        self.call(move |c| {
            let n = c.execute("DELETE FROM credentials WHERE name = ?1", params![name])?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }

    // ── Observability ──

    async fn set_traceparent(
        &self,
        target: TraceTarget,
        id: &str,
        traceparent: &str,
    ) -> StoreResult<()> {
        check_traceparent(traceparent)?;
        let (id, tp) = (id.to_string(), traceparent.to_string());
        self.call(move |c| {
            let n = c.execute(
                &format!("UPDATE {} SET traceparent = ?2 WHERE id = ?1", target.table()),
                params![id, tp],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound);
            }
            Ok(())
        })
        .await
    }

    async fn get_traceparent(&self, target: TraceTarget, id: &str) -> StoreResult<Option<String>> {
        let id = id.to_string();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT traceparent FROM {} WHERE id = ?1", target.table()),
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )?)
        })
        .await
    }

    async fn count_sessions_by_status(&self) -> StoreResult<Vec<(String, i64)>> {
        self.call(|c| {
            let mut stmt =
                c.prepare("SELECT status, COUNT(*) FROM sessions GROUP BY status ORDER BY status")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
        .await
    }
}

fn insert_api_key(c: &Connection, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
    let id = new_id("key");
    c.execute(
        "INSERT INTO api_keys (id, name, role, prefix, key_hash, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            new.name,
            new.role.as_str(),
            new.prefix,
            new.key_hash,
            ts(Utc::now()),
            new.expires_at.map(ts)
        ],
    )?;
    get_api_key_c(c, &id)
}
