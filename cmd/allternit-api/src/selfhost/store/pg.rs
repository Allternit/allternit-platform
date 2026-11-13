//! PostgreSQL implementation of [`SelfHostStore`] (sqlx).
//!
//! Schema: `migrations_selfhost_pg/`, applied by [`PgStore::migrate`] with
//! `sqlx::migrate!`. Queries use the runtime API (no compile-time checking),
//! so building needs no live database.
//!
//! Work claims pick the oldest claimable row with `FOR UPDATE SKIP LOCKED`
//! and update it in the same statement, so concurrent claimers never wait on
//! each other and never get the same row. Fenced calls lock the row
//! (`FOR UPDATE`) before they compare the generation.

use std::str::FromStr;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};

use super::*;

const AGENT_COLS: &str =
    "id, name, instructions, model, tools, mcp_servers, skills, metadata, version, created_at, updated_at";
const ENV_COLS: &str = "id, name, network, allowed_hosts, git, mcp, image, packages, metadata, created_at, updated_at";
const SESSION_COLS: &str = "id, agent_id, environment_id, title, status, status_reason, last_seq, metadata, agent_snapshot, gizzi_session_id, created_at, updated_at, archived_at";
const EVENT_COLS: &str = "id, session_id, seq, type, data, created_at";
const WORK_COLS: &str = "id, environment_id, session_id, status, lease_generation, lease_expires_at, worker_id, error_message, created_at, updated_at, completed_at";
const WORK_COLS_W: &str = "w.id, w.environment_id, w.session_id, w.status, w.lease_generation, w.lease_expires_at, w.worker_id, w.error_message, w.created_at, w.updated_at, w.completed_at";
const TOOL_COLS: &str = "id, session_id, work_id, claimed_generation, name, input, status, output, is_error, created_at, updated_at, completed_at";
const WORKER_COLS: &str = "environment_id, worker_id, last_seen_at, current_work_id, hostname, version";
const API_KEY_COLS: &str =
    "id, name, role, prefix, key_hash, created_at, expires_at, last_used_at, revoked_at";
const ENV_KEY_COLS: &str =
    "id, environment_id, name, prefix, key_hash, created_at, expires_at, last_used_at, revoked_at";
const CRED_COLS: &str = "id, name, sealed, key_id, created_at, updated_at";

const UPSERT_WORKER: &str = "INSERT INTO workers (environment_id, worker_id, last_seen_at, current_work_id, hostname, version)
     VALUES ($1, $2, $3, $4, $5, $6)
     ON CONFLICT (environment_id, worker_id) DO UPDATE SET
       last_seen_at = EXCLUDED.last_seen_at,
       current_work_id = EXCLUDED.current_work_id,
       hostname = COALESCE(EXCLUDED.hostname, workers.hostname),
       version = COALESCE(EXCLUDED.version, workers.version)";

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => StoreError::NotFound,
            other => StoreError::Backend(other.to_string()),
        }
    }
}

impl From<sqlx::migrate::MigrateError> for StoreError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        StoreError::Backend(format!("migration failed: {e}"))
    }
}

fn agent_from(r: &PgRow) -> StoreResult<AgentRecord> {
    Ok(AgentRecord {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        instructions: r.try_get("instructions")?,
        model: r.try_get("model")?,
        tools: r.try_get("tools")?,
        mcp_servers: r.try_get("mcp_servers")?,
        skills: r.try_get("skills")?,
        metadata: r.try_get("metadata")?,
        version: r.try_get("version")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn env_from(r: &PgRow) -> StoreResult<EnvironmentRecord> {
    let hosts: Value = r.try_get("allowed_hosts")?;
    Ok(EnvironmentRecord {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        network: r.try_get("network")?,
        allowed_hosts: serde_json::from_value(hosts)
            .map_err(|e| StoreError::Backend(format!("allowed_hosts: {e}")))?,
        git: r.try_get("git")?,
        mcp: r.try_get("mcp")?,
        image: r.try_get("image")?,
        packages: r.try_get("packages")?,
        metadata: r.try_get("metadata")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn session_from(r: &PgRow) -> StoreResult<SessionRecord> {
    Ok(SessionRecord {
        id: r.try_get("id")?,
        agent_id: r.try_get("agent_id")?,
        environment_id: r.try_get("environment_id")?,
        title: r.try_get("title")?,
        status: r.try_get("status")?,
        status_reason: r.try_get("status_reason")?,
        last_seq: r.try_get("last_seq")?,
        metadata: r.try_get("metadata")?,
        agent_snapshot: r.try_get("agent_snapshot")?,
        gizzi_session_id: r.try_get("gizzi_session_id")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        archived_at: r.try_get("archived_at")?,
    })
}

fn event_from(r: &PgRow) -> StoreResult<EventRecord> {
    Ok(EventRecord {
        id: r.try_get("id")?,
        session_id: r.try_get("session_id")?,
        seq: r.try_get("seq")?,
        event_type: r.try_get("type")?,
        data: r.try_get("data")?,
        created_at: r.try_get("created_at")?,
    })
}

fn work_from(r: &PgRow) -> StoreResult<WorkItem> {
    let status: String = r.try_get("status")?;
    Ok(WorkItem {
        id: r.try_get("id")?,
        environment_id: r.try_get("environment_id")?,
        session_id: r.try_get("session_id")?,
        status: WorkStatus::parse(&status)?,
        lease_generation: r.try_get("lease_generation")?,
        lease_expires_at: r.try_get("lease_expires_at")?,
        worker_id: r.try_get("worker_id")?,
        error_message: r.try_get("error_message")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        completed_at: r.try_get("completed_at")?,
    })
}

fn tool_from(r: &PgRow) -> StoreResult<ToolCallRecord> {
    let status: String = r.try_get("status")?;
    Ok(ToolCallRecord {
        id: r.try_get("id")?,
        session_id: r.try_get("session_id")?,
        work_id: r.try_get("work_id")?,
        claimed_generation: r.try_get("claimed_generation")?,
        name: r.try_get("name")?,
        input: r.try_get("input")?,
        status: ToolCallStatus::parse(&status)?,
        output: r.try_get("output")?,
        is_error: r.try_get("is_error")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        completed_at: r.try_get("completed_at")?,
    })
}

fn worker_from(r: &PgRow) -> StoreResult<WorkerRecord> {
    Ok(WorkerRecord {
        environment_id: r.try_get("environment_id")?,
        worker_id: r.try_get("worker_id")?,
        last_seen_at: r.try_get("last_seen_at")?,
        current_work_id: r.try_get("current_work_id")?,
        hostname: r.try_get("hostname")?,
        version: r.try_get("version")?,
    })
}

fn api_key_from(r: &PgRow) -> StoreResult<ApiKeyRecord> {
    let role: String = r.try_get("role")?;
    Ok(ApiKeyRecord {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        role: Role::parse(&role).ok_or_else(|| StoreError::Backend(format!("unknown role {role}")))?,
        prefix: r.try_get("prefix")?,
        key_hash: r.try_get("key_hash")?,
        created_at: r.try_get("created_at")?,
        expires_at: r.try_get("expires_at")?,
        last_used_at: r.try_get("last_used_at")?,
        revoked_at: r.try_get("revoked_at")?,
    })
}

fn env_key_from(r: &PgRow) -> StoreResult<EnvironmentKeyRecord> {
    Ok(EnvironmentKeyRecord {
        id: r.try_get("id")?,
        environment_id: r.try_get("environment_id")?,
        name: r.try_get("name")?,
        prefix: r.try_get("prefix")?,
        key_hash: r.try_get("key_hash")?,
        created_at: r.try_get("created_at")?,
        expires_at: r.try_get("expires_at")?,
        last_used_at: r.try_get("last_used_at")?,
        revoked_at: r.try_get("revoked_at")?,
    })
}

fn cred_from(r: &PgRow) -> StoreResult<CredentialRecord> {
    Ok(CredentialRecord {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        sealed: r.try_get("sealed")?,
        key_id: r.try_get("key_id")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn map_rows<T>(rows: Vec<PgRow>, f: fn(&PgRow) -> StoreResult<T>) -> StoreResult<Vec<T>> {
    rows.iter().map(f).collect()
}

type Tx = Transaction<'static, Postgres>;

pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connect. Does not migrate; call [`PgStore::migrate`].
    pub async fn connect(url: &str, max_connections: u32) -> StoreResult<Self> {
        let opts = PgConnectOptions::from_str(url)?;
        Self::connect_with(opts, max_connections).await
    }

    /// Connect with every connection's `search_path` set to `schema`, creating
    /// the schema first. Lets the test suite run each test in its own schema
    /// of one shared database.
    pub async fn connect_in_schema(
        url: &str,
        max_connections: u32,
        schema: &str,
    ) -> StoreResult<Self> {
        if schema.is_empty() || !schema.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(StoreError::Invalid(format!("bad schema name {schema}")));
        }
        let base = PgConnectOptions::from_str(url)?;
        let boot = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone())
            .await?;
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
            .execute(&boot)
            .await?;
        boot.close().await;
        Self::connect_with(base.options([("search_path", schema)]), max_connections).await
    }

    async fn connect_with(opts: PgConnectOptions, max_connections: u32) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }

    /// Apply `migrations_selfhost_pg/`. Safe to run again.
    pub async fn migrate(&self) -> StoreResult<()> {
        sqlx::migrate!("./migrations_selfhost_pg")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn begin(&self) -> StoreResult<Tx> {
        Ok(self.pool.begin().await?)
    }

    /// `created_at` of a cursor row. `table` is always a constant.
    async fn cursor_ts(&self, table: &str, id: &str) -> StoreResult<DateTime<Utc>> {
        let row = sqlx::query(&format!("SELECT created_at FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            Some(r) => Ok(r.try_get("created_at")?),
            None => Err(StoreError::InvalidCursor),
        }
    }

    /// Oldest-first page. `filter` is an optional `column = value` pair.
    async fn page_oldest(
        &self,
        table: &str,
        cols: &str,
        live_only: bool,
        filter: Option<(&str, String)>,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<PgRow>> {
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new(format!("SELECT {cols} FROM {table} WHERE TRUE"));
        if live_only {
            qb.push(" AND deleted_at IS NULL");
        }
        if let Some((col, val)) = filter {
            qb.push(format!(" AND {col} = ")).push_bind(val);
        }
        if let Some(after) = after {
            let c = self.cursor_ts(table, after).await?;
            qb.push(" AND (created_at, id) > (")
                .push_bind(c)
                .push(", ")
                .push_bind(after.to_string())
                .push(")");
        }
        qb.push(" ORDER BY created_at, id LIMIT ").push_bind(limit.max(0));
        Ok(qb.build().fetch_all(&self.pool).await?)
    }
}

/// Row-lock suffixes for the parent lookups below.
const FOR_UPDATE: &str = " FOR UPDATE";
const FOR_SHARE: &str = " FOR SHARE";

async fn agent_live(tx: &mut Tx, id: &str, lock: &str) -> StoreResult<AgentRecord> {
    let sql = format!("SELECT {AGENT_COLS} FROM agents WHERE id = $1 AND deleted_at IS NULL{lock}");
    let row = sqlx::query(&sql).bind(id).fetch_one(&mut **tx).await?;
    agent_from(&row)
}

async fn env_live(tx: &mut Tx, id: &str, lock: &str) -> StoreResult<EnvironmentRecord> {
    let sql = format!("SELECT {ENV_COLS} FROM environments WHERE id = $1 AND deleted_at IS NULL{lock}");
    let row = sqlx::query(&sql).bind(id).fetch_one(&mut **tx).await?;
    env_from(&row)
}

async fn session_locked(tx: &mut Tx, id: &str) -> StoreResult<SessionRecord> {
    let row = sqlx::query(&format!("SELECT {SESSION_COLS} FROM sessions WHERE id = $1 FOR UPDATE"))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    session_from(&row)
}

async fn work_locked(tx: &mut Tx, id: &str) -> StoreResult<WorkItem> {
    let row = sqlx::query(&format!("SELECT {WORK_COLS} FROM work_items WHERE id = $1 FOR UPDATE"))
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    work_from(&row)
}

/// Locked, leased and on this generation, or LeaseStale.
async fn fenced(tx: &mut Tx, id: &str, generation: i64) -> StoreResult<WorkItem> {
    let item = work_locked(tx, id).await?;
    if item.status != WorkStatus::Leased || item.lease_generation != generation {
        return Err(StoreError::LeaseStale);
    }
    Ok(item)
}

async fn requeue_claimed_calls(tx: &mut Tx, session_id: &str, now: DateTime<Utc>) -> StoreResult<()> {
    sqlx::query(
        "UPDATE tool_calls SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = $2
         WHERE session_id = $1 AND status = 'claimed'",
    )
    .bind(session_id)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[async_trait]
impl SelfHostStore for PgStore {
    fn backend(&self) -> &'static str {
        "postgres"
    }

    async fn health(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    // ── Agents ──

    async fn create_agent(&self, new: NewAgent) -> StoreResult<AgentRecord> {
        let now = Utc::now();
        let row = sqlx::query(&format!(
            "INSERT INTO agents (id, name, instructions, model, tools, mcp_servers, skills, metadata, version, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, $9, $9) RETURNING {AGENT_COLS}"
        ))
        .bind(new_id("agent"))
        .bind(new.name)
        .bind(new.instructions)
        .bind(new.model)
        .bind(or_array(new.tools))
        .bind(or_array(new.mcp_servers))
        .bind(or_array(new.skills))
        .bind(or_object(new.metadata))
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        agent_from(&row)
    }

    async fn get_agent(&self, id: &str) -> StoreResult<AgentRecord> {
        let row = sqlx::query(&format!(
            "SELECT {AGENT_COLS} FROM agents WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        agent_from(&row)
    }

    async fn list_agents(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<AgentRecord>> {
        let rows = self.page_oldest("agents", AGENT_COLS, true, None, limit, after).await?;
        map_rows(rows, agent_from)
    }

    async fn update_agent(&self, id: &str, patch: AgentPatch) -> StoreResult<AgentRecord> {
        let mut tx = self.begin().await?;
        let cur = agent_live(&mut tx, id, FOR_UPDATE).await?;
        let row = sqlx::query(&format!(
            "UPDATE agents SET name = $2, instructions = $3, model = $4, tools = $5, mcp_servers = $6,
                 skills = $7, metadata = $8, version = version + 1, updated_at = $9
             WHERE id = $1 RETURNING {AGENT_COLS}"
        ))
        .bind(id)
        .bind(patch.name.unwrap_or(cur.name))
        .bind(patch.instructions.unwrap_or(cur.instructions))
        .bind(patch.model.unwrap_or(cur.model))
        .bind(patch.tools.map(or_array).unwrap_or(cur.tools))
        .bind(patch.mcp_servers.map(or_array).unwrap_or(cur.mcp_servers))
        .bind(patch.skills.map(or_array).unwrap_or(cur.skills))
        .bind(patch.metadata.map(or_object).unwrap_or(cur.metadata))
        .bind(Utc::now())
        .fetch_one(&mut *tx)
        .await?;
        let out = agent_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn delete_agent(&self, id: &str) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        agent_live(&mut tx, id, FOR_UPDATE).await?;
        let in_use: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE agent_id = $1 AND archived_at IS NULL",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if in_use > 0 {
            return Err(StoreError::Conflict("agent_in_use"));
        }
        sqlx::query("UPDATE agents SET deleted_at = $2 WHERE id = $1")
            .bind(id)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ── Environments ──

    async fn create_environment(&self, new: NewEnvironment) -> StoreResult<EnvironmentRecord> {
        check_network(&new.network)?;
        let now = Utc::now();
        let row = sqlx::query(&format!(
            "INSERT INTO environments (id, name, network, allowed_hosts, git, mcp, image, packages, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10) RETURNING {ENV_COLS}"
        ))
        .bind(new_id("env"))
        .bind(new.name)
        .bind(new.network)
        .bind(Value::from(new.allowed_hosts))
        .bind(or_object(new.git))
        .bind(or_object(new.mcp))
        .bind(new.image)
        .bind(or_object(new.packages))
        .bind(or_object(new.metadata))
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        env_from(&row)
    }

    async fn get_environment(&self, id: &str) -> StoreResult<EnvironmentRecord> {
        let row = sqlx::query(&format!(
            "SELECT {ENV_COLS} FROM environments WHERE id = $1 AND deleted_at IS NULL"
        ))
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        env_from(&row)
    }

    async fn list_environments(
        &self,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentRecord>> {
        let rows = self.page_oldest("environments", ENV_COLS, true, None, limit, after).await?;
        map_rows(rows, env_from)
    }

    async fn update_environment(
        &self,
        id: &str,
        patch: EnvironmentPatch,
    ) -> StoreResult<EnvironmentRecord> {
        if let Some(n) = &patch.network {
            check_network(n)?;
        }
        let mut tx = self.begin().await?;
        let cur = env_live(&mut tx, id, FOR_UPDATE).await?;
        let row = sqlx::query(&format!(
            "UPDATE environments SET name = $2, network = $3, allowed_hosts = $4, git = $5, mcp = $6,
                 image = $7, packages = $8, metadata = $9, updated_at = $10
             WHERE id = $1 RETURNING {ENV_COLS}"
        ))
        .bind(id)
        .bind(patch.name.unwrap_or(cur.name))
        .bind(patch.network.unwrap_or(cur.network))
        .bind(Value::from(patch.allowed_hosts.unwrap_or(cur.allowed_hosts)))
        .bind(patch.git.map(or_object).unwrap_or(cur.git))
        .bind(patch.mcp.map(or_object).unwrap_or(cur.mcp))
        .bind(patch.image.unwrap_or(cur.image))
        .bind(patch.packages.map(or_object).unwrap_or(cur.packages))
        .bind(patch.metadata.map(or_object).unwrap_or(cur.metadata))
        .bind(Utc::now())
        .fetch_one(&mut *tx)
        .await?;
        let out = env_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn delete_environment(&self, id: &str) -> StoreResult<()> {
        let mut tx = self.begin().await?;
        env_live(&mut tx, id, FOR_UPDATE).await?;
        let in_use: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE environment_id = $1 AND archived_at IS NULL",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if in_use > 0 {
            return Err(StoreError::Conflict("environment_in_use"));
        }
        let now = Utc::now();
        sqlx::query(
            "UPDATE environment_keys SET revoked_at = $2 WHERE environment_id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE environments SET deleted_at = $2 WHERE id = $1")
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    // ── Sessions ──

    async fn create_session(&self, new: NewSession) -> StoreResult<SessionRecord> {
        let mut tx = self.begin().await?;
        // Share-lock both parents so a concurrent delete cannot slip between
        // the check and the insert.
        let agent = agent_live(&mut tx, &new.agent_id, FOR_SHARE).await?;
        env_live(&mut tx, &new.environment_id, FOR_SHARE).await?;
        let now = Utc::now();
        let row = sqlx::query(&format!(
            "INSERT INTO sessions (id, agent_id, environment_id, title, status, last_seq, metadata, agent_snapshot, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'idle', 0, $5, $6, $7, $7) RETURNING {SESSION_COLS}"
        ))
        .bind(new_id("sess"))
        .bind(&new.agent_id)
        .bind(&new.environment_id)
        .bind(new.title)
        .bind(or_object(new.metadata))
        .bind(agent_snapshot(&agent))
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = session_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn get_session(&self, id: &str) -> StoreResult<SessionRecord> {
        let row = sqlx::query(&format!("SELECT {SESSION_COLS} FROM sessions WHERE id = $1"))
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        session_from(&row)
    }

    async fn list_sessions(
        &self,
        filter: &SessionFilter,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<SessionRecord>> {
        let mut qb: QueryBuilder<Postgres> =
            QueryBuilder::new(format!("SELECT {SESSION_COLS} FROM sessions WHERE TRUE"));
        if let Some(a) = &filter.agent_id {
            qb.push(" AND agent_id = ").push_bind(a.clone());
        }
        if let Some(e) = &filter.environment_id {
            qb.push(" AND environment_id = ").push_bind(e.clone());
        }
        if let Some(s) = &filter.status {
            qb.push(" AND status = ").push_bind(s.clone());
        }
        if !filter.include_archived {
            qb.push(" AND archived_at IS NULL");
        }
        if let Some(after) = after {
            let c = self.cursor_ts("sessions", after).await?;
            qb.push(" AND (created_at, id) < (")
                .push_bind(c)
                .push(", ")
                .push_bind(after.to_string())
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ").push_bind(limit.max(0));
        let rows = qb.build().fetch_all(&self.pool).await?;
        map_rows(rows, session_from)
    }

    async fn set_session_status(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> StoreResult<SessionRecord> {
        check_session_status(status)?;
        let row = sqlx::query(&format!(
            "UPDATE sessions SET status = $2, status_reason = $3, updated_at = $4 WHERE id = $1 RETURNING {SESSION_COLS}"
        ))
        .bind(id)
        .bind(status)
        .bind(reason)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        session_from(&row)
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
        let row = sqlx::query(&format!(
            "UPDATE sessions SET status = $3, status_reason = $4, updated_at = $5
             WHERE id = $1 AND status = ANY($2) RETURNING {SESSION_COLS}"
        ))
        .bind(id)
        .bind(from)
        .bind(to)
        .bind(reason)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(Some(session_from(&r)?)),
            None => {
                // Tell a missing session apart from a status mismatch.
                self.get_session(id).await?;
                Ok(None)
            }
        }
    }

    async fn set_gizzi_session_id(&self, id: &str, gizzi_session_id: &str) -> StoreResult<()> {
        let res = sqlx::query("UPDATE sessions SET gizzi_session_id = $2, updated_at = $3 WHERE id = $1")
            .bind(id)
            .bind(gizzi_session_id)
            .bind(Utc::now())
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn archive_session(&self, id: &str) -> StoreResult<SessionRecord> {
        let mut tx = self.begin().await?;
        let cur = session_locked(&mut tx, id).await?;
        if cur.archived_at.is_some() {
            return Ok(cur);
        }
        let row = sqlx::query(&format!(
            "UPDATE sessions SET status = 'archived', status_reason = NULL, archived_at = $2, updated_at = $2
             WHERE id = $1 RETURNING {SESSION_COLS}"
        ))
        .bind(id)
        .bind(Utc::now())
        .fetch_one(&mut *tx)
        .await?;
        let out = session_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    // ── Events ──

    async fn append_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: Value,
    ) -> StoreResult<EventRecord> {
        check_event_type(event_type)?;
        let now = Utc::now();
        let mut tx = self.begin().await?;
        // The row lock on the session serializes appends, so seq has no gaps
        // or repeats.
        let seq: Option<i64> = sqlx::query_scalar(
            "UPDATE sessions SET last_seq = last_seq + 1, updated_at = $2 WHERE id = $1 RETURNING last_seq",
        )
        .bind(session_id)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let seq = seq.ok_or(StoreError::NotFound)?;
        let row = sqlx::query(&format!(
            "INSERT INTO session_events (id, session_id, seq, type, data, created_at)
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING {EVENT_COLS}"
        ))
        .bind(new_id("evt"))
        .bind(session_id)
        .bind(seq)
        .bind(event_type)
        .bind(data)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = event_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn list_events(
        &self,
        session_id: &str,
        after_seq: i64,
        limit: i64,
        types: Option<&[String]>,
    ) -> StoreResult<Vec<EventRecord>> {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(format!(
            "SELECT {EVENT_COLS} FROM session_events WHERE session_id = "
        ));
        qb.push_bind(session_id.to_string())
            .push(" AND seq > ")
            .push_bind(after_seq);
        if let Some(types) = types.filter(|t| !t.is_empty()) {
            qb.push(" AND type = ANY(").push_bind(types.to_vec()).push(")");
        }
        qb.push(" ORDER BY seq LIMIT ").push_bind(limit.max(0));
        let rows = qb.build().fetch_all(&self.pool).await?;
        map_rows(rows, event_from)
    }

    // ── Work queue ──

    async fn enqueue_work(&self, environment_id: &str, session_id: &str) -> StoreResult<WorkItem> {
        let row = sqlx::query(&format!(
            "INSERT INTO work_items (id, environment_id, session_id, status, lease_generation, created_at, updated_at)
             VALUES ($1, $2, $3, 'ready', 0, $4, $4) RETURNING {WORK_COLS}"
        ))
        .bind(new_id("work"))
        .bind(environment_id)
        .bind(session_id)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        work_from(&row)
    }

    async fn get_work(&self, id: &str) -> StoreResult<WorkItem> {
        let row = sqlx::query(&format!("SELECT {WORK_COLS} FROM work_items WHERE id = $1"))
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        work_from(&row)
    }

    async fn find_open_work_for_session(&self, session_id: &str) -> StoreResult<Option<WorkItem>> {
        let row = sqlx::query(&format!(
            "SELECT {WORK_COLS} FROM work_items WHERE session_id = $1 AND status IN ('ready', 'leased')
             ORDER BY created_at DESC, id DESC LIMIT 1"
        ))
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(work_from).transpose()
    }

    async fn claim_work(&self, req: ClaimRequest) -> StoreResult<Option<WorkClaim>> {
        let cutoff: Option<DateTime<Utc>> = req
            .reclaim_older_than_ms
            .map(|ms| req.now - Duration::milliseconds(ms));
        let secret = new_secret("ss");
        let mut tx = self.begin().await?;
        let row = sqlx::query(&format!(
            "WITH cand AS (
                 SELECT id FROM work_items
                 WHERE environment_id = $1
                   AND (status = 'ready'
                        OR ($2::timestamptz IS NOT NULL AND status = 'leased' AND lease_expires_at <= $2))
                 ORDER BY created_at, id
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             UPDATE work_items w
             SET status = 'leased', lease_generation = w.lease_generation + 1, lease_expires_at = $3,
                 worker_id = $4, session_secret_hash = $5, updated_at = $6
             FROM cand WHERE w.id = cand.id
             RETURNING {WORK_COLS_W}"
        ))
        .bind(&req.environment_id)
        .bind(cutoff)
        .bind(lease_end(req.now))
        .bind(&req.worker_id)
        .bind(hash_secret(&secret))
        .bind(req.now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let item = work_from(&row)?;
        requeue_claimed_calls(&mut tx, &item.session_id, req.now).await?;
        sqlx::query(UPSERT_WORKER)
            .bind(&req.environment_id)
            .bind(&req.worker_id)
            .bind(req.now)
            .bind(&item.id)
            .bind(&req.hostname)
            .bind(&req.version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(WorkClaim {
            item,
            session_secret: secret,
        }))
    }

    async fn heartbeat_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let mut tx = self.begin().await?;
        fenced(&mut tx, id, generation).await?;
        let row = sqlx::query(&format!(
            "UPDATE work_items SET updated_at = $2 WHERE id = $1 RETURNING {WORK_COLS}"
        ))
        .bind(id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = work_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn renew_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let mut tx = self.begin().await?;
        fenced(&mut tx, id, generation).await?;
        let row = sqlx::query(&format!(
            "UPDATE work_items SET lease_expires_at = $2, updated_at = $3 WHERE id = $1 RETURNING {WORK_COLS}"
        ))
        .bind(id)
        .bind(lease_end(now))
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = work_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn complete_work(
        &self,
        id: &str,
        generation: i64,
        outcome: WorkOutcome,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let mut tx = self.begin().await?;
        let item = work_locked(&mut tx, id).await?;
        if item.lease_generation != generation {
            return Err(StoreError::LeaseStale);
        }
        if item.status.is_terminal() {
            return Ok(item);
        }
        if item.status != WorkStatus::Leased {
            return Err(StoreError::LeaseStale);
        }
        let row = sqlx::query(&format!(
            "UPDATE work_items SET status = $2, error_message = $3, lease_expires_at = NULL,
                 session_secret_hash = NULL, completed_at = $4, updated_at = $4
             WHERE id = $1 RETURNING {WORK_COLS}"
        ))
        .bind(id)
        .bind(outcome.status().as_str())
        .bind(error_message)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = work_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn release_work(
        &self,
        id: &str,
        generation: i64,
        now: DateTime<Utc>,
    ) -> StoreResult<WorkItem> {
        let mut tx = self.begin().await?;
        let item = fenced(&mut tx, id, generation).await?;
        let row = sqlx::query(&format!(
            "UPDATE work_items SET status = 'ready', lease_expires_at = NULL, worker_id = NULL,
                 session_secret_hash = NULL, updated_at = $2
             WHERE id = $1 RETURNING {WORK_COLS}"
        ))
        .bind(id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        requeue_claimed_calls(&mut tx, &item.session_id, now).await?;
        let out = work_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn requeue_expired_work(&self, now: DateTime<Utc>, grace: Duration) -> StoreResult<u64> {
        let cutoff = now - grace;
        let mut tx = self.begin().await?;
        sqlx::query(
            "UPDATE tool_calls SET status = 'pending', work_id = NULL, claimed_generation = NULL, updated_at = $2
             WHERE status = 'claimed' AND session_id IN (
                 SELECT session_id FROM work_items WHERE status = 'leased' AND lease_expires_at <= $1)",
        )
        .bind(cutoff)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let res = sqlx::query(
            "UPDATE work_items SET status = 'ready', lease_expires_at = NULL, worker_id = NULL,
                 session_secret_hash = NULL, updated_at = $2
             WHERE status = 'leased' AND lease_expires_at <= $1",
        )
        .bind(cutoff)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(res.rows_affected())
    }

    async fn cancel_open_work_for_session(&self, session_id: &str) -> StoreResult<u64> {
        let res = sqlx::query(
            "UPDATE work_items SET status = 'cancelled', lease_expires_at = NULL, session_secret_hash = NULL,
                 completed_at = $2, updated_at = $2
             WHERE session_id = $1 AND status IN ('ready', 'leased')",
        )
        .bind(session_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    async fn find_work_by_secret_hash(
        &self,
        secret_hash: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<WorkItem>> {
        let row = sqlx::query(&format!(
            "SELECT {WORK_COLS} FROM work_items
             WHERE session_secret_hash = $1 AND status = 'leased' AND lease_expires_at > $2"
        ))
        .bind(secret_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(work_from).transpose()
    }

    async fn queue_depth(&self, environment_id: &str) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM work_items WHERE environment_id = $1 AND status = 'ready'",
        )
        .bind(environment_id)
        .fetch_one(&self.pool)
        .await?)
    }

    // ── Tool calls ──

    async fn insert_tool_call(&self, new: NewToolCall) -> StoreResult<(ToolCallRecord, bool)> {
        let res = sqlx::query(
            "INSERT INTO tool_calls (id, session_id, name, input, status, is_error, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'pending', FALSE, $5, $5)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(&new.id)
        .bind(&new.session_id)
        .bind(&new.name)
        .bind(&new.input)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        let call = self.get_tool_call(&new.id).await?;
        if call.session_id != new.session_id {
            return Err(StoreError::Conflict("tool_call_conflict"));
        }
        Ok((call, res.rows_affected() == 1))
    }

    async fn get_tool_call(&self, id: &str) -> StoreResult<ToolCallRecord> {
        let row = sqlx::query(&format!("SELECT {TOOL_COLS} FROM tool_calls WHERE id = $1"))
            .bind(id)
            .fetch_one(&self.pool)
            .await?;
        tool_from(&row)
    }

    async fn next_tool_call(
        &self,
        work_id: &str,
        generation: i64,
    ) -> StoreResult<Option<ToolCallRecord>> {
        let mut tx = self.begin().await?;
        let item = fenced(&mut tx, work_id, generation).await?;
        let row = sqlx::query(&format!(
            "WITH cand AS (
                 SELECT id FROM tool_calls WHERE session_id = $1 AND status = 'pending'
                 ORDER BY created_at, id LIMIT 1
                 FOR UPDATE SKIP LOCKED
             )
             UPDATE tool_calls t SET status = 'claimed', work_id = $2, claimed_generation = $3, updated_at = $4
             FROM cand WHERE t.id = cand.id
             RETURNING t.id"
        ))
        .bind(&item.session_id)
        .bind(work_id)
        .bind(generation)
        .bind(Utc::now())
        .fetch_optional(&mut *tx)
        .await?;
        let out = match row {
            Some(r) => {
                let id: String = r.try_get("id")?;
                let full = sqlx::query(&format!("SELECT {TOOL_COLS} FROM tool_calls WHERE id = $1"))
                    .bind(&id)
                    .fetch_one(&mut *tx)
                    .await?;
                Some(tool_from(&full)?)
            }
            None => None,
        };
        tx.commit().await?;
        Ok(out)
    }

    async fn submit_tool_result(
        &self,
        id: &str,
        output: &str,
        is_error: bool,
    ) -> StoreResult<ToolResultOutcome> {
        check_result_size(output)?;
        let row = sqlx::query(&format!(
            "UPDATE tool_calls SET status = 'done', output = $2, is_error = $3, completed_at = $4, updated_at = $4
             WHERE id = $1 AND status <> 'done' RETURNING {TOOL_COLS}"
        ))
        .bind(id)
        .bind(output)
        .bind(is_error)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(r) => Ok(ToolResultOutcome {
                call: tool_from(&r)?,
                stored: true,
            }),
            None => Ok(ToolResultOutcome {
                call: self.get_tool_call(id).await?,
                stored: false,
            }),
        }
    }

    async fn cancel_open_tool_calls(&self, session_id: &str, output: &str) -> StoreResult<u64> {
        check_result_size(output)?;
        let res = sqlx::query(
            "UPDATE tool_calls SET status = 'done', output = $2, is_error = TRUE, completed_at = $3, updated_at = $3
             WHERE session_id = $1 AND status <> 'done'",
        )
        .bind(session_id)
        .bind(output)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
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
        sqlx::query(UPSERT_WORKER)
            .bind(environment_id)
            .bind(worker_id)
            .bind(now)
            .bind(current_work_id)
            .bind(hostname)
            .bind(version)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_workers(
        &self,
        environment_id: &str,
        since: DateTime<Utc>,
    ) -> StoreResult<Vec<WorkerRecord>> {
        let rows = sqlx::query(&format!(
            "SELECT {WORKER_COLS} FROM workers WHERE environment_id = $1 AND last_seen_at >= $2
             ORDER BY last_seen_at DESC, worker_id"
        ))
        .bind(environment_id)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        map_rows(rows, worker_from)
    }

    async fn count_workers(&self, environment_id: &str, since: DateTime<Utc>) -> StoreResult<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM workers WHERE environment_id = $1 AND last_seen_at >= $2",
        )
        .bind(environment_id)
        .bind(since)
        .fetch_one(&self.pool)
        .await?)
    }

    // ── Server API keys ──

    async fn count_api_keys(&self) -> StoreResult<i64> {
        Ok(sqlx::query_scalar("SELECT COUNT(*) FROM api_keys")
            .fetch_one(&self.pool)
            .await?)
    }

    async fn create_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
        let mut tx = self.begin().await?;
        let out = insert_api_key(&mut tx, new).await?;
        tx.commit().await?;
        Ok(out)
    }

    async fn create_first_api_key(&self, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
        let mut tx = self.begin().await?;
        // Serialize first-key attempts so two setup calls cannot both see an
        // empty table.
        sqlx::query("LOCK TABLE api_keys IN EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys")
            .fetch_one(&mut *tx)
            .await?;
        if n > 0 {
            return Err(StoreError::Conflict("setup_already_done"));
        }
        let out = insert_api_key(&mut tx, new).await?;
        tx.commit().await?;
        Ok(out)
    }

    async fn find_api_key_by_hash(&self, key_hash: &str) -> StoreResult<Option<ApiKeyRecord>> {
        let row = sqlx::query(&format!("SELECT {API_KEY_COLS} FROM api_keys WHERE key_hash = $1"))
            .bind(key_hash)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(api_key_from).transpose()
    }

    async fn list_api_keys(&self, limit: i64, after: Option<&str>) -> StoreResult<Vec<ApiKeyRecord>> {
        let rows = self.page_oldest("api_keys", API_KEY_COLS, false, None, limit, after).await?;
        map_rows(rows, api_key_from)
    }

    async fn revoke_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<ApiKeyRecord> {
        let mut tx = self.begin().await?;
        // Lock every active admin key, so two revokes cannot each leave the
        // other as "the last one" and both succeed.
        sqlx::query("SELECT id FROM api_keys WHERE role = 'admin' AND revoked_at IS NULL FOR UPDATE")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(&format!("SELECT {API_KEY_COLS} FROM api_keys WHERE id = $1 FOR UPDATE"))
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
        let key = api_key_from(&row)?;
        if key.revoked_at.is_some() {
            return Ok(key);
        }
        if key.role == Role::Admin && key.is_active(now) {
            let others: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM api_keys WHERE role = 'admin' AND id <> $1 AND revoked_at IS NULL
                   AND (expires_at IS NULL OR expires_at > $2)",
            )
            .bind(id)
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
            if others == 0 {
                return Err(StoreError::Conflict("last_admin_key"));
            }
        }
        let row = sqlx::query(&format!(
            "UPDATE api_keys SET revoked_at = $2 WHERE id = $1 RETURNING {API_KEY_COLS}"
        ))
        .bind(id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        let out = api_key_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn touch_api_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = $2 WHERE id = $1")
            .bind(id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Environment keys ──

    async fn create_environment_key(
        &self,
        new: NewEnvironmentKey,
    ) -> StoreResult<EnvironmentKeyRecord> {
        let mut tx = self.begin().await?;
        env_live(&mut tx, &new.environment_id, FOR_SHARE).await?;
        let row = sqlx::query(&format!(
            "INSERT INTO environment_keys (id, environment_id, name, prefix, key_hash, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {ENV_KEY_COLS}"
        ))
        .bind(new_id("ekey"))
        .bind(&new.environment_id)
        .bind(new.name)
        .bind(new.prefix)
        .bind(new.key_hash)
        .bind(Utc::now())
        .bind(new.expires_at)
        .fetch_one(&mut *tx)
        .await?;
        let out = env_key_from(&row)?;
        tx.commit().await?;
        Ok(out)
    }

    async fn find_environment_key_by_hash(
        &self,
        key_hash: &str,
    ) -> StoreResult<Option<EnvironmentKeyRecord>> {
        let row = sqlx::query(&format!(
            "SELECT {ENV_KEY_COLS} FROM environment_keys WHERE key_hash = $1"
        ))
        .bind(key_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(env_key_from).transpose()
    }

    async fn list_environment_keys(
        &self,
        environment_id: &str,
        limit: i64,
        after: Option<&str>,
    ) -> StoreResult<Vec<EnvironmentKeyRecord>> {
        let rows = self
            .page_oldest(
                "environment_keys",
                ENV_KEY_COLS,
                false,
                Some(("environment_id", environment_id.to_string())),
                limit,
                after,
            )
            .await?;
        map_rows(rows, env_key_from)
    }

    async fn revoke_environment_key(
        &self,
        environment_id: &str,
        id: &str,
        now: DateTime<Utc>,
    ) -> StoreResult<EnvironmentKeyRecord> {
        sqlx::query(
            "UPDATE environment_keys SET revoked_at = $3
             WHERE id = $1 AND environment_id = $2 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(environment_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        let row = sqlx::query(&format!(
            "SELECT {ENV_KEY_COLS} FROM environment_keys WHERE id = $1 AND environment_id = $2"
        ))
        .bind(id)
        .bind(environment_id)
        .fetch_one(&self.pool)
        .await?;
        env_key_from(&row)
    }

    async fn touch_environment_key(&self, id: &str, now: DateTime<Utc>) -> StoreResult<()> {
        sqlx::query("UPDATE environment_keys SET last_used_at = $2 WHERE id = $1")
            .bind(id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ── Credentials ──

    async fn put_credential(
        &self,
        name: &str,
        sealed: &[u8],
        key_id: &str,
    ) -> StoreResult<CredentialRecord> {
        let row = sqlx::query(&format!(
            "INSERT INTO credentials (id, name, sealed, key_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $5)
             ON CONFLICT (name) DO UPDATE SET sealed = EXCLUDED.sealed, key_id = EXCLUDED.key_id,
                 updated_at = EXCLUDED.updated_at
             RETURNING {CRED_COLS}"
        ))
        .bind(new_id("cred"))
        .bind(name)
        .bind(sealed)
        .bind(key_id)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await?;
        cred_from(&row)
    }

    async fn get_credential(&self, name: &str) -> StoreResult<CredentialRecord> {
        let row = sqlx::query(&format!("SELECT {CRED_COLS} FROM credentials WHERE name = $1"))
            .bind(name)
            .fetch_one(&self.pool)
            .await?;
        cred_from(&row)
    }

    async fn list_credentials(&self) -> StoreResult<Vec<CredentialRecord>> {
        let rows = sqlx::query(&format!("SELECT {CRED_COLS} FROM credentials ORDER BY name"))
            .fetch_all(&self.pool)
            .await?;
        map_rows(rows, cred_from)
    }

    async fn delete_credential(&self, name: &str) -> StoreResult<()> {
        let res = sqlx::query("DELETE FROM credentials WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
}

async fn insert_api_key(tx: &mut Tx, new: NewApiKey) -> StoreResult<ApiKeyRecord> {
    let row = sqlx::query(&format!(
        "INSERT INTO api_keys (id, name, role, prefix, key_hash, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {API_KEY_COLS}"
    ))
    .bind(new_id("key"))
    .bind(new.name)
    .bind(new.role.as_str())
    .bind(new.prefix)
    .bind(new.key_hash)
    .bind(Utc::now())
    .bind(new.expires_at)
    .fetch_one(&mut **tx)
    .await?;
    api_key_from(&row)
}
