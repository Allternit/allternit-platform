//! Store-level fabric-transport operations over the canonical SQLite store.
//!
//! Every claim, renew, complete, and expiry is a transactional
//! compare-and-swap against the persisted `cowork_jobs` row (A:// lock 2):
//! exclusivity of ownership is decided by SQLite, never by an in-process lock.
//! Eligibility is computed here in fabric transport; the CAS makes the winner
//! single. Lease times are server-authoritative RFC3339 UTC (lock 3).
//!
//! Consolidation boundary (P-T1): within a deployment, this module is the
//! only writer of `cowork_runs`, `cowork_jobs`, and `cowork_run_events`.
//! Product surfaces (e.g. the Rails cowork REST API in `cmd/allternit-api`)
//! persist their RunManager state through the projection helpers in the
//! "Consolidation boundary projections" section below — never through raw
//! SQL — so a product projection can never clobber fabric-transport lease
//! ownership. Only `enqueue_job` / `claim_job` / `record_heartbeat` /
//! `renew_lease` / `complete_job` / `expire_leases` / `continue_job_in_cloud`
//! may write lease columns.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::transport::{
    ApprovalBinding, CompleteOutcome, ExpiryAction, LeaseGrant, PrincipalRecord,
    TransportError, TransportErrorCode as Code,
};

fn store_err(e: rusqlite::Error) -> TransportError {
    TransportError::new(Code::Store, format!("sqlite: {e}"))
}

/// Open a store connection with a busy timeout so the sweeper, API handlers,
/// and tests can write concurrently against the same DB file.
pub fn open_store(path: &Path) -> Result<Connection, TransportError> {
    let conn = Connection::open(path).map_err(store_err)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(store_err)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;").map_err(store_err)?;
    Ok(conn)
}

/// Apply the cowork migration DDL needed by the store (V5, V8, V152–V154).
/// Used by tests and one-off tooling; the API applies the full refinery set.
pub fn apply_store_ddl(conn: &mut Connection) -> Result<(), TransportError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cowork_runs (
            id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, workspace_id TEXT NOT NULL,
            initiator TEXT NOT NULL, mode TEXT NOT NULL, state TEXT NOT NULL,
            entrypoint TEXT NOT NULL, dag_id TEXT NOT NULL,
            current_job_id TEXT, current_checkpoint_id TEXT, policy_profile TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP, updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            completed_at DATETIME, delegator TEXT, user_id TEXT);
         CREATE TABLE IF NOT EXISTS cowork_jobs (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, dag_node_id TEXT NOT NULL DEFAULT '',
            job_type TEXT NOT NULL DEFAULT 'task', priority INTEGER NOT NULL DEFAULT 0,
            state TEXT NOT NULL, lease_owner TEXT, retry_count INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 0, timeout_sec INTEGER NOT NULL DEFAULT 0,
            payload TEXT NOT NULL DEFAULT '{}', created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP, started_at DATETIME, completed_at DATETIME,
            lease_id TEXT, lease_generation INTEGER NOT NULL DEFAULT 0,
            lease_expires_at DATETIME, claimed_at DATETIME,
            required_capabilities TEXT NOT NULL DEFAULT '[]', result TEXT,
            initiator TEXT, delegator TEXT);
         CREATE TABLE IF NOT EXISTS cowork_run_events (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, event_type TEXT NOT NULL,
            payload TEXT NOT NULL, created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            initiator TEXT, delegator TEXT, executor TEXT);
         CREATE TABLE IF NOT EXISTS cowork_principals (
            id TEXT PRIMARY KEY, workspace TEXT NOT NULL,
            capabilities TEXT NOT NULL DEFAULT '[]', token_hash TEXT,
            status TEXT NOT NULL DEFAULT 'active',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         CREATE TABLE IF NOT EXISTS cowork_approval_bindings (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, job_id TEXT NOT NULL,
            lease_id TEXT NOT NULL, lease_generation INTEGER NOT NULL,
            executor TEXT NOT NULL, capability TEXT NOT NULL, target TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            decided_at DATETIME, decided_by TEXT, expires_at DATETIME);
         ALTER TABLE cowork_run_events ADD COLUMN client_event_id TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_cowork_run_events_client_id
             ON cowork_run_events(run_id, client_event_id);
         CREATE TABLE IF NOT EXISTS cowork_approval_policy (
            workspace TEXT PRIMARY KEY,
            capability_risk TEXT NOT NULL DEFAULT '{}',
            rules TEXT NOT NULL DEFAULT '[]',
            max_delegation_depth INTEGER NOT NULL DEFAULT 4,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         ALTER TABLE cowork_principals ADD COLUMN roles TEXT NOT NULL DEFAULT '[]';
         CREATE TABLE IF NOT EXISTS cowork_intents (
            intent_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, envelope TEXT NOT NULL,
            initiator TEXT, delegator TEXT,
            causation_chain TEXT NOT NULL DEFAULT '[]',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         ALTER TABLE cowork_runs ADD COLUMN causation_chain TEXT NOT NULL DEFAULT '[]';
         ALTER TABLE cowork_jobs ADD COLUMN causation_chain TEXT NOT NULL DEFAULT '[]';
         CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, workspace_id TEXT);
         CREATE TABLE IF NOT EXISTS cowork_handoffs (
            id TEXT PRIMARY KEY, run_id TEXT NOT NULL, to_agent_id TEXT NOT NULL,
            task_id TEXT, note TEXT, status TEXT NOT NULL DEFAULT 'pending',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP, completed_at DATETIME);
         ALTER TABLE cowork_handoffs ADD COLUMN job_id TEXT;
         ALTER TABLE cowork_handoffs ADD COLUMN causation_chain TEXT NOT NULL DEFAULT '[]';
         CREATE TABLE IF NOT EXISTS cowork_memory_entries (
            id TEXT PRIMARY KEY, user_id TEXT NOT NULL, project_id TEXT,
            session_id TEXT, content TEXT NOT NULL, type TEXT NOT NULL DEFAULT 'fact',
            tags TEXT, source TEXT, created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         ALTER TABLE cowork_memory_entries ADD COLUMN owner_principal TEXT;
         ALTER TABLE cowork_memory_entries ADD COLUMN grants TEXT NOT NULL DEFAULT '[]';
         CREATE TABLE IF NOT EXISTS cowork_delegation_rules (
            workspace TEXT NOT NULL, action_type TEXT NOT NULL,
            target_principal TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 100,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (workspace, action_type));
         ALTER TABLE cowork_intents ADD COLUMN child_run_id TEXT;
         ALTER TABLE cowork_intents ADD COLUMN orchestration_status TEXT NOT NULL DEFAULT 'pending';
         CREATE TABLE IF NOT EXISTS cowork_connector_sessions (
            id TEXT PRIMARY KEY, principal TEXT NOT NULL, run_id TEXT NOT NULL,
            job_id TEXT NOT NULL, capability TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'active', expires_at DATETIME NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         CREATE TABLE IF NOT EXISTS cowork_connector_secrets (
            capability TEXT PRIMARY KEY, secret_env TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP);
         INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.webhook.send', 'ALLTERNIT_BROKER_WEBHOOK_URL');
        INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.github.read', 'ALLTERNIT_BROKER_GITHUB_TOKEN');
        INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.github.write', 'ALLTERNIT_BROKER_GITHUB_TOKEN');
        INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.files.read', 'ALLTERNIT_BROKER_FILES_ROOT');
        INSERT OR IGNORE INTO cowork_connector_secrets (capability, secret_env)
         VALUES ('connector.files.write', 'ALLTERNIT_BROKER_FILES_ROOT');",
    )
    .map_err(store_err)?;
    // V142 ownership columns — present on API-managed stores via refinery
    // migrations; ensured here so test/tooling stores match the API schema.
    // Guarded because the batch ALTERs above are not re-entrant.
    ensure_column(conn, "cowork_runs", "user_id", "ALTER TABLE cowork_runs ADD COLUMN user_id TEXT")?;
    ensure_column(conn, "cowork_jobs", "user_id", "ALTER TABLE cowork_jobs ADD COLUMN user_id TEXT")?;
    ensure_column(conn, "cowork_run_events", "user_id", "ALTER TABLE cowork_run_events ADD COLUMN user_id TEXT")?;
    Ok(())
}

/// Add a column to a table when missing (idempotent ALTER).
fn ensure_column(
    conn: &mut Connection,
    table: &str,
    column: &str,
    ddl: &str,
) -> Result<(), TransportError> {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut stmt| {
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<String>, _>>()?;
            Ok(rows.iter().any(|c| c == column))
        })
        .map_err(store_err)?;
    if !exists {
        conn.execute_batch(ddl).map_err(store_err)?;
    }
    Ok(())
}

// ─── Principals (§8.3–8.4) ───────────────────────────────────────────────────

/// Register a principal; the bearer token is stored only as a SHA-256 hash.
#[allow(clippy::too_many_arguments)]
pub fn register_principal(
    conn: &mut Connection,
    id: &str,
    workspace: &str,
    capabilities: &[String],
    roles: &[String],
    token: &str,
) -> Result<(), TransportError> {
    // Canonical workspace ids are stripped (`evidence`), matching the
    // built-in seeds and `cowork_runs.workspace_id`; callers may pass the
    // full A:// URI (`a://workspace/evidence`).
    let workspace = workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(workspace);
    conn.execute(
        "INSERT INTO cowork_principals
            (id, workspace, capabilities, roles, token_hash, status, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET
             workspace = excluded.workspace,
             capabilities = excluded.capabilities,
             roles = excluded.roles,
             token_hash = excluded.token_hash,
             status = 'active',
             updated_at = CURRENT_TIMESTAMP",
        params![
            id,
            workspace,
            serde_json::to_string(capabilities).unwrap(),
            serde_json::to_string(roles).unwrap(),
            crate::transport::hash_token(token),
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Mint a principal with no credential (token_hash NULL). Used by linkage
/// and default-principal seeding; the credential is provisioned separately
/// exactly once via `provision_principal_token`.
pub fn mint_principal(
    conn: &mut Connection,
    id: &str,
    workspace: &str,
    capabilities: &[String],
    roles: &[String],
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_principals
            (id, workspace, capabilities, roles, token_hash, status, updated_at)
         VALUES (?1, ?2, ?3, ?4, NULL, 'active', CURRENT_TIMESTAMP)
         ON CONFLICT(id) DO UPDATE SET
             capabilities = excluded.capabilities,
             roles = excluded.roles,
             status = 'active',
             updated_at = CURRENT_TIMESTAMP",
        params![
            id,
            workspace,
            serde_json::to_string(capabilities).unwrap(),
            serde_json::to_string(roles).unwrap(),
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Provision (or rotate) a principal's bearer token. Returns the raw token
/// exactly once; only its SHA-256 hash is persisted. User-auth endpoint.
pub fn provision_principal_token(
    conn: &mut Connection,
    principal_id: &str,
) -> Result<String, TransportError> {
    let exists = conn
        .query_row(
            "SELECT COUNT(*) FROM cowork_principals WHERE id = ?1",
            params![principal_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(store_err)?;
    if exists == 0 {
        return Err(TransportError::new(
            Code::PrincipalNotFound,
            format!("principal {principal_id} not found"),
        ));
    }
    let token = format!("atok_{}", Uuid::new_v4());
    conn.execute(
        "UPDATE cowork_principals SET token_hash = ?1, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?2",
        params![crate::transport::hash_token(&token), principal_id],
    )
    .map_err(store_err)?;
    Ok(token)
}

/// Canonical default capabilities for the Gizzi worker principal
/// (GIZZI_WORKER_SPEC.md §3).
pub const GIZZI_CAPABILITIES: &[&str] = &[
    "shell.exec",
    "git.read",
    "git.write",
    "files.project.read",
    "files.project.write",
    "artifact.create",
    "artifact.modify",
    "memory.read",
    "memory.write",
    "compute.local",
];

/// Seed the default Al and Gizzi principals for every workspace that has
/// runs, agents, or intents (idempotent upsert; never touches credentials —
/// provision those via `provision_principal_token`). Returns the ids minted
/// or updated this pass.
pub fn seed_default_principals(conn: &mut Connection) -> Result<Vec<String>, TransportError> {
    let mut workspaces: Vec<String> = Vec::new();
    for sql in [
        "SELECT DISTINCT workspace_id FROM cowork_runs",
        "SELECT DISTINCT workspace_id FROM agents WHERE workspace_id IS NOT NULL",
        "SELECT DISTINCT workspace FROM cowork_principals",
    ] {
        let mut stmt = conn.prepare(sql).map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        for w in rows {
            if !w.is_empty() && !workspaces.contains(&w) {
                workspaces.push(w);
            }
        }
    }

    let mut touched = Vec::new();
    for ws in workspaces {
        let al = format!("a://workspace/{ws}/principal/al");
        mint_principal(conn, &al, &ws, &[], &["orchestrator".into(), "user-interface".into()])?;
        touched.push(al);

        let gizzi = format!("a://workspace/{ws}/principal/gizzi");
        mint_principal(
            conn,
            &gizzi,
            &ws,
            &GIZZI_CAPABILITIES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &["worker".into(), "code".into(), "terminal".into()],
        )?;
        touched.push(gizzi);
    }
    Ok(touched)
}

/// Managed-runtime ensure (consumer desktop, P1): the `gizzi` worker
/// principal for a workspace exists with its canonical capability set and
/// roles. Idempotent upsert — never touches credentials (the desktop
/// provisions those separately via the local ensure route, which returns
/// the fresh token once). Returns the principal id.
pub fn ensure_gizzi_principal(
    conn: &mut Connection,
    workspace: &str,
) -> Result<String, TransportError> {
    let ws = workspace.strip_prefix("a://workspace/").unwrap_or(workspace);
    let id = format!("a://workspace/{ws}/principal/gizzi");
    mint_principal(
        conn,
        &id,
        ws,
        &GIZZI_CAPABILITIES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        &["worker".into(), "code".into(), "terminal".into()],
    )?;
    Ok(id)
}

/// Always-on cloud worker principal (consumer cloud continuation).
/// Distinct from the laptop `gizzi` principal: it declares `compute.cloud`
/// and never `compute.local`, so laptop-closed jobs cannot be reclaimed by
/// the desktop worker. Idempotent; never touches credentials.
pub fn ensure_gizzi_cloud_principal(
    conn: &mut Connection,
    workspace: &str,
) -> Result<String, TransportError> {
    let ws = workspace.strip_prefix("a://workspace/").unwrap_or(workspace);
    let id = format!("a://workspace/{ws}/principal/gizzi-cloud");
    let caps: Vec<String> = GIZZI_CAPABILITIES
        .iter()
        .filter(|c| **c != "compute.local")
        .map(|s| s.to_string())
        .chain(std::iter::once("compute.cloud".to_string()))
        .collect();
    mint_principal(
        conn,
        &id,
        ws,
        &caps,
        &["worker".into(), "code".into(), "terminal".into()],
    )?;
    Ok(id)
}

/// Outcome of promoting a job to cloud continuation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CloudContinuation {
    /// Job that was handed off.
    pub job_id: String,
    /// Parent run.
    pub run_id: String,
    /// Capabilities after the handoff (`compute.cloud`, no `compute.local`).
    pub required_capabilities: Vec<String>,
    /// Job state after the handoff (`queued`).
    pub state: String,
    /// Intent envelope to replay onto an always-on API. None when the job
    /// was not created through submit_intent (synthetic envelope is built
    /// by the API layer).
    pub envelope: Option<serde_json::Value>,
}

/// Hand a job to the cloud worker: drop any local lease, require
/// `compute.cloud`, strip `compute.local` so the laptop worker cannot
/// reclaim it. Terminal jobs are refused. Identity/attribution unchanged.
pub fn continue_job_in_cloud(
    conn: &mut Connection,
    job_id: &str,
) -> Result<CloudContinuation, TransportError> {
    let now = Utc::now().to_rfc3339();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    let row: Option<(String, String, String)> = tx
        .query_row(
            "SELECT run_id, state, required_capabilities FROM cowork_jobs WHERE id = ?1",
            params![job_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(store_err)?;
    let Some((run_id, state, caps_json)) = row else {
        return Err(TransportError::new(
            Code::JobNotFound,
            format!("job {job_id} not found"),
        ));
    };
    if matches!(state.as_str(), "completed" | "failed" | "dead_letter") {
        return Err(TransportError::new(
            Code::ResultAlreadyCommitted,
            format!("job {job_id} is terminal ({state}); cannot continue in cloud"),
        ));
    }
    let mut caps: Vec<String> = serde_json::from_str(&caps_json).unwrap_or_default();
    caps.retain(|c| c != "compute.local");
    if !caps.iter().any(|c| c == "compute.cloud") {
        caps.push("compute.cloud".to_string());
    }
    let caps_out = serde_json::to_string(&caps).unwrap();
    tx.execute(
        "UPDATE cowork_jobs SET
            required_capabilities = ?1,
            state = 'queued',
            lease_id = NULL,
            lease_owner = NULL,
            lease_expires_at = NULL,
            updated_at = ?2
         WHERE id = ?3 AND state NOT IN ('completed','failed','dead_letter')",
        params![caps_out, now, job_id],
    )
    .map_err(store_err)?;
    let attr = load_run_attribution(&tx, &run_id)?;
    insert_event(
        &tx,
        &run_id,
        "continuation.handed_off",
        serde_json::json!({
            "job_id": job_id,
            "from_state": state,
            "required_capabilities": caps,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        None,
    )?;
    let envelope: Option<serde_json::Value> = tx
        .query_row(
            "SELECT envelope FROM cowork_intents WHERE run_id = ?1 LIMIT 1",
            params![run_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(store_err)?
        .and_then(|raw| serde_json::from_str(&raw).ok());
    tx.commit().map_err(store_err)?;
    Ok(CloudContinuation {
        job_id: job_id.to_string(),
        run_id,
        required_capabilities: caps,
        state: "queued".to_string(),
        envelope,
    })
}

/// Promote every non-terminal job on a run.
pub fn continue_run_in_cloud(
    conn: &mut Connection,
    run_id: &str,
) -> Result<Vec<CloudContinuation>, TransportError> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM cowork_jobs WHERE run_id = ?1
                 AND state NOT IN ('completed','failed','dead_letter')",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map(params![run_id], |r| r.get(0))
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        rows
    };
    let mut out = Vec::new();
    for id in ids {
        out.push(continue_job_in_cloud(conn, &id)?);
    }
    Ok(out)
}

/// Promote every in-flight job (queued / leased / running) — desktop quit.
pub fn handoff_all_in_flight(
    conn: &mut Connection,
) -> Result<Vec<CloudContinuation>, TransportError> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM cowork_jobs
                 WHERE state IN ('queued','leased','running')",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |r| r.get(0))
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        rows
    };
    let mut out = Vec::new();
    for id in ids {
        out.push(continue_job_in_cloud(conn, &id)?);
    }
    Ok(out)
}

/// Compute placement resolution (§8.8, P-T2): an explicit compute policy on
/// the intent becomes a mandatory job capability; claim eligibility then
/// restricts the job to workers that declared that capability.
/// `auto`/absent stays capability-neutral — placement resolves by
/// capability intersection at claim time. Placement never rewrites
/// identity or attribution.
pub fn compute_requirements(compute: &Option<serde_json::Value>) -> Vec<String> {
    let Some(c) = compute else { return Vec::new() };
    let policy = c
        .as_str()
        .or_else(|| c.get("policy").and_then(|p| p.as_str()))
        .unwrap_or("auto");
    match policy {
        "vm" => vec!["compute.vm".to_string()],
        "local" => vec!["compute.local".to_string()],
        "byo" => vec!["compute.byo".to_string()],
        "cloud" => vec!["compute.cloud".to_string()],
        _ => Vec::new(),
    }
}

/// Replace a principal's declared capability set (operator action, P-T2):
/// a worker running in VM mode declares `compute.vm` here and placement
/// routes vm-required jobs to it. Roles and status are untouched.
pub fn set_principal_capabilities(
    conn: &Connection,
    principal_id: &str,
    capabilities: &[String],
) -> Result<(), TransportError> {
    let n = conn
        .execute(
            "UPDATE cowork_principals SET capabilities = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2",
            params![serde_json::to_string(capabilities).unwrap(), principal_id],
        )
        .map_err(store_err)?;
    if n == 0 {
        return Err(TransportError::new(
            Code::PrincipalNotFound,
            format!("principal {principal_id} not found"),
        ));
    }
    Ok(())
}

/// Authenticate a bearer token to a principal identity.
pub fn authenticate_principal(
    conn: &Connection,
    token: &str,
) -> Result<PrincipalRecord, TransportError> {
    let hash = crate::transport::hash_token(token);
    let row = conn
        .query_row(
            "SELECT id, workspace, capabilities, status, roles FROM cowork_principals
             WHERE token_hash = ?1",
            params![hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(store_err)?;

    match row {
        None => Err(TransportError::new(
            Code::AuthenticationFailed,
            "bearer token does not map to any principal",
        )),
        Some((id, workspace, caps, status, roles)) if status == "active" => Ok(PrincipalRecord {
            id,
            workspace,
            capabilities: serde_json::from_str(&caps).unwrap_or_default(),
            roles: serde_json::from_str(&roles).unwrap_or_default(),
            status,
        }),
        Some((id, _, _, status, _)) => Err(TransportError::new(
            Code::PermissionDenied,
            format!("principal {id} is not active (status={status})"),
        )),
    }
}

// ─── Job enqueue helper ─────────────────────────────────────────────────────

/// Insert a queued job row directly (fabric-transport path; skips the Rails DAG
/// mirror, which the API layer maintains separately).
#[allow(clippy::too_many_arguments)]
pub fn enqueue_job(
    conn: &mut Connection,
    run_id: &str,
    job_type: &str,
    payload: serde_json::Value,
    required_capabilities: &[String],
    timeout_sec: i64,
    max_retries: i64,
    initiator: Option<&str>,
    delegator: Option<&str>,
) -> Result<String, TransportError> {
    let job_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO cowork_jobs
            (id, run_id, dag_node_id, job_type, state, payload, required_capabilities, timeout_sec,
             max_retries, initiator, delegator)
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            job_id,
            run_id,
            format!("dag-{job_id}"),
            job_type,
            payload.to_string(),
            serde_json::to_string(required_capabilities).unwrap(),
            timeout_sec,
            max_retries,
            initiator,
            delegator,
        ],
    )
    .map_err(store_err)?;
    Ok(job_id)
}

// ─── Events (§8.18 attribution triple) ───────────────────────────────────────

fn insert_event(
    conn: &Connection,
    run_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    initiator: Option<&str>,
    delegator: Option<&str>,
    executor: Option<&str>,
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_run_events (id, run_id, event_type, payload, initiator, delegator, executor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            Uuid::new_v4().to_string(),
            run_id,
            event_type,
            payload.to_string(),
            initiator,
            delegator,
            executor,
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

struct JobRow {
    id: String,
    run_id: String,
    state: String,
    payload: String,
    _required_capabilities: String,
    lease_id: Option<String>,
    lease_generation: i64,
    lease_expires_at: Option<String>,
    lease_owner: Option<String>,
    retry_count: i64,
    max_retries: i64,
    result: Option<String>,
    initiator: Option<String>,
    delegator: Option<String>,
}

fn load_job(conn: &Connection, job_id: &str) -> Result<Option<JobRow>, TransportError> {
    conn.query_row(
        "SELECT id, run_id, state, payload, required_capabilities, lease_id,
                lease_generation, lease_expires_at, lease_owner, retry_count,
                max_retries, result, initiator, delegator
         FROM cowork_jobs WHERE id = ?1",
        params![job_id],
        |row| {
            Ok(JobRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                state: row.get(2)?,
                payload: row.get(3)?,
                _required_capabilities: row.get(4)?,
                lease_id: row.get(5)?,
                lease_generation: row.get(6)?,
                lease_expires_at: row.get(7)?,
                lease_owner: row.get(8)?,
                retry_count: row.get(9)?,
                max_retries: row.get(10)?,
                result: row.get(11)?,
                initiator: row.get(12)?,
                delegator: row.get(13)?,
            })
        },
    )
    .optional()
    .map_err(store_err)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Run attribution: job-level triple falls back to the run row.
struct RunAttribution {
    initiator: Option<String>,
    delegator: Option<String>,
    current_checkpoint_id: Option<String>,
}

fn load_run_attribution(
    conn: &Connection,
    run_id: &str,
) -> Result<RunAttribution, TransportError> {
    conn.query_row(
        "SELECT initiator, delegator, current_checkpoint_id FROM cowork_runs WHERE id = ?1",
        params![run_id],
        |row| {
            Ok(RunAttribution {
                initiator: row.get(0)?,
                delegator: row.get(1)?,
                current_checkpoint_id: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(store_err)
    .map(|o| {
        o.unwrap_or(RunAttribution {
            initiator: None,
            delegator: None,
            current_checkpoint_id: None,
        })
    })
}

// ─── Claim (§8.10) ───────────────────────────────────────────────────────────

/// Atomically claim a job for an authenticated principal.
///
/// Eligibility (workspace, capabilities, run state) is evaluated here; the
/// actual claim is `UPDATE cowork_jobs ... WHERE id=? AND state='queued'
/// RETURNING ...` inside an immediate transaction, so exactly one concurrent
/// caller wins the row.
pub fn claim_job(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: Option<&str>,
    lease_ttl: Duration,
) -> Result<LeaseGrant, TransportError> {
    let now = Utc::now();
    let expires = now + lease_ttl;
    let lease_id = format!("lease_{}", Uuid::new_v4());

    // Candidate selection (fabric-transport eligibility).
    let mut candidates: Vec<(String, String, String, String, String)> = Vec::new(); // id, run_id, payload, req_caps, workspace
    {
        let sql = "SELECT j.id, j.run_id, j.payload, j.required_capabilities, r.workspace_id
                   FROM cowork_jobs j JOIN cowork_runs r ON r.id = j.run_id
                   WHERE j.state = 'queued'
                     AND r.state NOT IN ('completed', 'failed', 'cancelled')
                   ORDER BY j.priority DESC, j.created_at ASC";
        let mut stmt = conn.prepare(sql).map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;

        for (id, run_id, payload, req_caps, workspace) in rows {
            if let Some(want) = job_id {
                if id != want {
                    continue;
                }
            }
            if workspace != principal.workspace {
                if job_id == Some(id.as_str()) {
                    return Err(TransportError::new(
                        Code::WorkspaceMismatch,
                        format!(
                            "job {id} is in workspace {workspace}, principal {} is in {}",
                            principal.id, principal.workspace
                        ),
                    ));
                }
                continue;
            }
            let required: Vec<String> =
                serde_json::from_str(&req_caps).unwrap_or_default();
            let missing: Vec<&String> = required
                .iter()
                .filter(|c| !principal.capabilities.contains(c))
                .collect();
            if !missing.is_empty() {
                if job_id == Some(id.as_str()) {
                    return Err(TransportError::new(
                        Code::CapabilityMissing,
                        format!(
                            "principal {} lacks required capabilities: {}",
                            principal.id,
                            missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
                continue;
            }
            candidates.push((id, run_id, payload, req_caps, workspace));
        }
    }

    if candidates.is_empty() {
        return Err(TransportError::new(
            if job_id.is_some() {
                Code::JobAlreadyLeased
            } else {
                Code::NoEligibleWorker
            },
            if job_id.is_some() {
                "job is not claimable (already leased, terminal, or missing)"
            } else {
                "no queued job matches this principal's workspace and capabilities"
            },
        ));
    }

    let (chosen, run_id, payload, req_caps, _workspace) = candidates.remove(0);
    let caps: Vec<String> = serde_json::from_str(&req_caps).unwrap_or_default();

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;

    // Re-check run state inside the transaction.
    let run_state: Option<String> = tx
        .query_row(
            "SELECT state FROM cowork_runs WHERE id = ?1",
            params![run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(store_err)?;
    match run_state.as_deref() {
        Some("cancelled") => {
            tx.rollback().map_err(store_err)?;
            return Err(TransportError::new(
                Code::RunCancelled,
                format!("run {run_id} is cancelled"),
            ));
        }
        Some("completed") | Some("failed") | None => {
            tx.rollback().map_err(store_err)?;
            return Err(TransportError::new(
                Code::NoEligibleWorker,
                format!("run {run_id} is not transportable (state={run_state:?})"),
            ));
        }
        _ => {}
    }

    // The persistence-level CAS: exactly one winner.
    let granted: Option<(i64,)> = tx
        .query_row(
            "UPDATE cowork_jobs
             SET state = 'leased',
                 lease_owner = ?1,
                 lease_id = ?2,
                 lease_generation = lease_generation + 1,
                 lease_expires_at = ?3,
                 claimed_at = ?4,
                 started_at = COALESCE(started_at, ?4),
                 updated_at = ?4
             WHERE id = ?5 AND state = 'queued'
             RETURNING lease_generation",
            params![
                principal.id,
                lease_id,
                expires.to_rfc3339(),
                now.to_rfc3339(),
                chosen,
            ],
            |row| Ok((row.get(0)?,)),
        )
        .optional()
        .map_err(store_err)?;

    let generation = match granted {
        Some((g,)) => g,
        None => {
            tx.rollback().map_err(store_err)?;
            return Err(TransportError::new(
                Code::JobAlreadyLeased,
                format!("job {chosen} was claimed by another worker first"),
            ));
        }
    };

    let attr = load_run_attribution(&tx, &run_id)?;
    let initiator = attr.initiator.as_deref();
    let delegator = attr.delegator.as_deref();

    // RUNNING means a worker holds a valid lease (§8.2): the first claim moves
    // the run from queued to running atomically with the lease grant.
    tx.execute(
        "UPDATE cowork_runs SET state = 'running', updated_at = ?1
         WHERE id = ?2 AND state = 'queued'",
        params![now.to_rfc3339(), run_id],
    )
    .map_err(store_err)?;

    tx.execute(
        "UPDATE cowork_runs SET current_job_id = ?1, updated_at = ?2 WHERE id = ?3",
        params![chosen, now.to_rfc3339(), run_id],
    )
    .map_err(store_err)?;

    insert_event(
        &tx,
        &run_id,
        "job.claimed",
        serde_json::json!({
            "job_id": chosen,
            "lease_id": lease_id,
            "lease_generation": generation,
            "lease_expires_at": expires.to_rfc3339(),
            "capabilities": caps,
        }),
        initiator,
        delegator,
        Some(principal.id.as_str()),
    )?;

    tx.commit().map_err(store_err)?;

    Ok(LeaseGrant {
        job_id: chosen.clone(),
        run_id,
        lease_id,
        lease_generation: generation,
        lease_expires_at: expires.to_rfc3339(),
        payload: serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null),
        required_capabilities: caps,
        current_checkpoint_id: attr.current_checkpoint_id,
        initiator: initiator.map(|s| s.to_string()),
        delegator: delegator.map(|s| s.to_string()),
    })
}

// ─── Lease validation shared by heartbeat/renew/complete (§8.11–8.12, 8.16) ──

fn validate_lease(
    conn: &Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
) -> Result<JobRow, TransportError> {
    let job = load_job(conn, job_id)?
        .ok_or_else(|| TransportError::new(Code::JobNotFound, format!("job {job_id} not found")))?;

    // Stale generations are rejected before anything else: a worker never
    // decides whether its own lease remains valid (§8.26).
    if job.lease_generation != generation {
        return Err(TransportError::new(
            Code::StaleLeaseGeneration,
            format!(
                "job {job_id} is at lease generation {}, caller presented {generation}",
                job.lease_generation
            ),
        ));
    }
    if job.lease_id.as_deref() != Some(lease_id) {
        return Err(TransportError::new(
            Code::InvalidLease,
            format!("lease_id {lease_id} does not own job {job_id}"),
        ));
    }
    if job.lease_owner.as_deref() != Some(principal.id.as_str()) {
        return Err(TransportError::new(
            Code::PermissionDenied,
            format!(
                "job {job_id} is leased to {:?}, not {}",
                job.lease_owner, principal.id
            ),
        ));
    }
    match job.state.as_str() {
        "leased" | "running" => {}
        "cancelled" => {
            return Err(TransportError::new(
                Code::RunCancelled,
                format!("job {job_id} is cancelled"),
            ))
        }
        _ => {
            return Err(TransportError::new(
                Code::LeaseExpired,
                format!("job {job_id} lease is no longer active (state={})", job.state),
            ))
        }
    }
    if let Some(exp) = job.lease_expires_at.as_deref() {
        if parse_time(exp).map(|e| e < Utc::now()).unwrap_or(true) {
            return Err(TransportError::new(
                Code::LeaseExpired,
                format!("lease on job {job_id} expired at {exp} (server clock)"),
            ));
        }
    }
    Ok(job)
}

/// Heartbeat: proves liveness, records an attributed event; `worker_time` is
/// advisory only — the server clock decides expiry (lock 3, §8.11).
pub fn record_heartbeat(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    worker_time: Option<String>,
) -> Result<(), TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;
    let attr = load_run_attribution(conn, &job.run_id)?;
    insert_event(
        conn,
        &job.run_id,
        "job.heartbeat",
        serde_json::json!({
            "job_id": job_id,
            "lease_id": lease_id,
            "lease_generation": generation,
            "worker_time": worker_time,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    Ok(())
}

/// Renew a lease before expiration (§8.12). Renewal never resurrects an
/// expired generation — expiry is judged by the server clock at renew time.
pub fn renew_lease(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    lease_ttl: Duration,
) -> Result<String, TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;
    let expires = (Utc::now() + lease_ttl).to_rfc3339();

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    let updated = tx
        .execute(
            "UPDATE cowork_jobs SET lease_expires_at = ?1, updated_at = ?2
             WHERE id = ?3 AND state IN ('leased','running')
               AND lease_id = ?4 AND lease_generation = ?5 AND lease_owner = ?6",
            params![
                expires,
                Utc::now().to_rfc3339(),
                job_id,
                lease_id,
                generation,
                principal.id,
            ],
        )
        .map_err(store_err)?;
    if updated == 0 {
        tx.rollback().map_err(store_err)?;
        return Err(TransportError::new(
            Code::InvalidLease,
            format!("renewal lost the race on job {job_id}"),
        ));
    }
    let attr = load_run_attribution(&tx, &job.run_id)?;
    insert_event(
        &tx,
        &job.run_id,
        "job.lease_renewed",
        serde_json::json!({
            "job_id": job_id,
            "lease_id": lease_id,
            "lease_generation": generation,
            "lease_expires_at": expires,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    tx.commit().map_err(store_err)?;
    Ok(expires)
}

/// Complete (or fail) a job under the active lease generation (§8.16).
///
/// Validation order: stale generation is rejected even for terminal jobs (the
/// killed worker can never submit a valid completion under an old generation),
/// while the worker that holds the terminal generation gets the canonical
/// existing result back on repeat — exactly-once side effects either way.
#[allow(clippy::too_many_arguments)]
pub fn complete_job(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    success: bool,
    summary: Option<String>,
    outputs: Option<serde_json::Value>,
) -> Result<CompleteOutcome, TransportError> {
    let job = load_job(conn, job_id)?
        .ok_or_else(|| TransportError::new(Code::JobNotFound, format!("job {job_id} not found")))?;

    if job.lease_generation != generation {
        return Err(TransportError::new(
            Code::StaleLeaseGeneration,
            format!(
                "job {job_id} is at lease generation {}, caller presented {generation}",
                job.lease_generation
            ),
        ));
    }

    let terminal = matches!(
        job.state.as_str(),
        "completed" | "failed" | "dead_letter" | "cancelled"
    );
    if terminal {
        let result: serde_json::Value = job
            .result
            .as_deref()
            .and_then(|r| serde_json::from_str(r).ok())
            .unwrap_or(serde_json::Value::Null);
        return Ok(CompleteOutcome::AlreadyCommitted {
            job_state: job.state.clone(),
            result,
        });
    }

    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;
    let now = Utc::now();
    let new_state = if success { "completed" } else { "failed" };
    let result = serde_json::json!({
        "result_id": format!("result_{}", Uuid::new_v4()),
        "run_id": job.run_id,
        "job_id": job_id,
        "status": new_state,
        "summary": summary.unwrap_or_default(),
        "outputs": outputs.unwrap_or(serde_json::json!({})),
        "executor": principal.id,
        "completed_at": now.to_rfc3339(),
    });

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;

    // Terminal CAS: state, lease identity, generation, and owner must all still
    // match; the lease is released in the same write as the result commit.
    let updated = tx
        .execute(
            "UPDATE cowork_jobs
             SET state = ?1, result = ?2, completed_at = ?3, updated_at = ?3,
                 lease_id = NULL, lease_expires_at = NULL
             WHERE id = ?4 AND state IN ('leased','running')
               AND lease_id = ?5 AND lease_generation = ?6 AND lease_owner = ?7",
            params![
                new_state,
                result.to_string(),
                now.to_rfc3339(),
                job_id,
                lease_id,
                generation,
                principal.id,
            ],
        )
        .map_err(store_err)?;
    if updated == 0 {
        tx.rollback().map_err(store_err)?;
        // Lost a race with the sweeper or another completion: report the
        // canonical outcome of whoever won.
        if let Some(current) = load_job(conn, job_id)? {
            if matches!(
                current.state.as_str(),
                "completed" | "failed" | "dead_letter" | "cancelled"
            ) {
                let result: serde_json::Value = current
                    .result
                    .as_deref()
                    .and_then(|r| serde_json::from_str(r).ok())
                    .unwrap_or(serde_json::Value::Null);
                return Ok(CompleteOutcome::AlreadyCommitted {
                    job_state: current.state.clone(),
                    result,
                });
            }
        }
        return Err(TransportError::new(
            Code::LeaseExpired,
            format!("completion lost the race on job {job_id}"),
        ));
    }

    let attr = load_run_attribution(&tx, &job.run_id)?;
    insert_event(
        &tx,
        &job.run_id,
        if success { "job.completed" } else { "job.failed" },
        serde_json::json!({
            "job_id": job_id,
            "lease_id": lease_id,
            "lease_generation": generation,
            "result_id": result["result_id"],
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    insert_event(
        &tx,
        &job.run_id,
        "result.created",
        result.clone(),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;

    // Advance the run when no non-terminal jobs remain (§8.16).
    let remaining: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM cowork_jobs
             WHERE run_id = ?1 AND state NOT IN ('completed','failed','dead_letter','cancelled')",
            params![job.run_id],
            |row| row.get(0),
        )
        .map_err(store_err)?;
    if remaining == 0 {
        let failures: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM cowork_jobs
                 WHERE run_id = ?1 AND state IN ('failed','dead_letter')",
                params![job.run_id],
                |row| row.get(0),
            )
            .map_err(store_err)?;
        let run_state = if failures > 0 { "failed" } else { "completed" };
        tx.execute(
            "UPDATE cowork_runs SET state = ?1, completed_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![run_state, now.to_rfc3339(), job.run_id],
        )
        .map_err(store_err)?;
        insert_event(
            &tx,
            &job.run_id,
            "run.completed",
            serde_json::json!({ "state": run_state }),
            attr.initiator.as_deref(),
            attr.delegator.as_deref(),
            Some(principal.id.as_str()),
        )?;
    }

    tx.commit().map_err(store_err)?;

    Ok(CompleteOutcome::Committed {
        job_state: new_state.to_string(),
        result,
    })
}

// ─── Lease expiry sweeper (§8.13) ────────────────────────────────────────────

/// Expire every lease whose server-authoritative `lease_expires_at` has
/// passed. Recovery policy: requeue (`retry_count+1 <= max_retries`) or
/// dead-letter. Each expiry is itself a CAS so a worker renewing mid-sweep
/// keeps its lease. Returns the actions taken.
pub fn expire_leases(conn: &mut Connection, now: DateTime<Utc>) -> Result<Vec<ExpiryAction>, TransportError> {
    let candidates: Vec<JobRow> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, run_id, state, payload, required_capabilities, lease_id,
                        lease_generation, lease_expires_at, lease_owner, retry_count,
                        max_retries, result, initiator, delegator
                 FROM cowork_jobs
                 WHERE state IN ('leased','running') AND lease_expires_at IS NOT NULL",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| {
            Ok(JobRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                state: row.get(2)?,
                payload: row.get(3)?,
                _required_capabilities: row.get(4)?,
                lease_id: row.get(5)?,
                lease_generation: row.get(6)?,
                lease_expires_at: row.get(7)?,
                lease_owner: row.get(8)?,
                retry_count: row.get(9)?,
                max_retries: row.get(10)?,
                result: row.get(11)?,
                initiator: row.get(12)?,
                delegator: row.get(13)?,
            })
        })
        .map_err(store_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store_err)?;
        rows
    };

    let mut actions = Vec::new();
    for job in candidates {
        let Some(exp) = job.lease_expires_at.as_deref() else {
            continue;
        };
        if !parse_time(exp).map(|e| e < now).unwrap_or(false) {
            continue;
        }

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_err)?;
        // CAS: only expire if the same lease is still un-renewed.
        let updated = tx
            .execute(
                "UPDATE cowork_jobs
                 SET retry_count = retry_count + 1,
                     state = CASE WHEN retry_count + 1 > max_retries
                                  THEN 'dead_letter' ELSE 'queued' END,
                     lease_owner = CASE WHEN retry_count + 1 > max_retries
                                        THEN lease_owner ELSE NULL END,
                     lease_id = NULL,
                     lease_expires_at = NULL,
                     updated_at = ?1
                 WHERE id = ?2 AND state IN ('leased','running')
                   AND lease_id IS ?3 AND lease_expires_at = ?4",
                params![
                    now.to_rfc3339(),
                    job.id,
                    job.lease_id,
                    job.lease_expires_at,
                ],
            )
            .map_err(store_err)?;
        if updated == 0 {
            tx.rollback().map_err(store_err)?;
            continue; // renewed or completed mid-sweep
        }

        let new_retry = job.retry_count + 1;
        let outcome = if new_retry > job.max_retries {
            "dead_letter"
        } else {
            "queued"
        };

        let attr = load_run_attribution(&tx, &job.run_id)?;
        insert_event(
            &tx,
            &job.run_id,
            "job.lease_expired",
            serde_json::json!({
                "job_id": job.id,
                "lease_id": job.lease_id,
                "lease_generation": job.lease_generation,
                "lease_expires_at": job.lease_expires_at,
                "expired_at": now.to_rfc3339(),
            }),
            attr.initiator.as_deref(),
            attr.delegator.as_deref(),
            job.lease_owner.as_deref(),
        )?;

        // §8.13 / §8.14: a lease-bound approval dies with its generation. A
        // replacement worker under a new generation must re-obtain approval.
        let invalidated: Vec<(String, String, String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, executor, capability, target FROM cowork_approval_bindings
                     WHERE job_id = ?1 AND lease_generation = ?2
                       AND status IN ('pending','granted')",
                )
                .map_err(store_err)?;
            let rows = stmt
                .query_map(
                    rusqlite::params![job.id, job.lease_generation],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .map_err(store_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_err)?;
            rows
        };
        tx.execute(
            "UPDATE cowork_approval_bindings SET status = 'invalidated'
             WHERE job_id = ?1 AND lease_generation = ?2 AND status IN ('pending','granted')",
            params![job.id, job.lease_generation],
        )
        .map_err(store_err)?;
        for (binding_id, executor, capability, target) in invalidated {
            insert_event(
                &tx,
                &job.run_id,
                "approval.invalidated",
                serde_json::json!({
                    "approval_id": binding_id,
                    "capability": capability,
                    "target": target,
                    "lease_generation": job.lease_generation,
                }),
                attr.initiator.as_deref(),
                attr.delegator.as_deref(),
                Some(executor.as_str()),
            )?;
        }
        insert_event(
            &tx,
            &job.run_id,
            if outcome == "dead_letter" {
                "job.dead_lettered"
            } else {
                "job.requeued"
            },
            serde_json::json!({
                "job_id": job.id,
                "retry_count": new_retry,
                "max_retries": job.max_retries,
            }),
            attr.initiator.as_deref(),
            attr.delegator.as_deref(),
            None, // fabric-transport action, not an executor
        )?;
        tx.commit().map_err(store_err)?;

        actions.push(ExpiryAction {
            job_id: job.id,
            run_id: job.run_id,
            lease_id: job.lease_id,
            lease_generation: job.lease_generation,
            executor: job.lease_owner,
            outcome: outcome.to_string(),
            retry_count: new_retry,
        });
    }
    Ok(actions)
}

/// Read back a job's canonical state + result (idempotent result read).
pub fn get_job_view(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<serde_json::Value>, TransportError> {
    let Some(job) = load_job(conn, job_id)? else {
        return Ok(None);
    };
    let result: serde_json::Value = job
        .result
        .as_deref()
        .and_then(|r| serde_json::from_str(r).ok())
        .unwrap_or(serde_json::Value::Null);
    Ok(Some(serde_json::json!({
        "job_id": job.id,
        "run_id": job.run_id,
        "state": job.state,
        "lease_owner": job.lease_owner,
        "lease_id": job.lease_id,
        "lease_generation": job.lease_generation,
        "lease_expires_at": job.lease_expires_at,
        "retry_count": job.retry_count,
        "max_retries": job.max_retries,
        "payload": serde_json::from_str::<serde_json::Value>(&job.payload).unwrap_or(serde_json::Value::Null),
        "result": result,
        "initiator": job.initiator,
        "delegator": job.delegator,
    })))
}

// ─── Approval bindings (§8.14) ───────────────────────────────────────────────

fn load_binding(conn: &Connection, approval_id: &str) -> Result<Option<ApprovalBinding>, TransportError> {
    conn.query_row(
        "SELECT id, run_id, job_id, lease_id, lease_generation, executor,
                capability, target, status, decided_by, expires_at
         FROM cowork_approval_bindings WHERE id = ?1",
        params![approval_id],
        |row| {
            Ok(ApprovalBinding {
                id: row.get(0)?,
                run_id: row.get(1)?,
                job_id: row.get(2)?,
                lease_id: row.get(3)?,
                lease_generation: row.get(4)?,
                executor: row.get(5)?,
                capability: row.get(6)?,
                target: row.get(7)?,
                status: row.get(8)?,
                decided_by: row.get(9)?,
                expires_at: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(store_err)
}

/// Load the approval policy for a workspace (global default when unset).
pub fn load_policy(conn: &Connection, workspace: &str) -> Result<crate::risk_policy::ApprovalPolicy, TransportError> {
    let row = conn
        .query_row(
            "SELECT capability_risk, rules FROM cowork_approval_policy WHERE workspace = ?1",
            params![workspace],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(store_err)?;
    match row {
        Some((risk, rules)) => Ok(crate::risk_policy::ApprovalPolicy {
            capability_risk: serde_json::from_str(&risk).unwrap_or_default(),
            rules: serde_json::from_str(&rules).unwrap_or_default(),
        }),
        None => Ok(crate::risk_policy::ApprovalPolicy::default()),
    }
}

/// Store (or replace) a workspace approval policy.
pub fn set_policy(
    conn: &mut Connection,
    workspace: &str,
    policy: &crate::risk_policy::ApprovalPolicy,
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_approval_policy (workspace, capability_risk, rules, updated_at)
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(workspace) DO UPDATE SET
             capability_risk = excluded.capability_risk,
             rules = excluded.rules,
             updated_at = CURRENT_TIMESTAMP",
        params![
            workspace,
            serde_json::to_string(&policy.capability_risk).unwrap(),
            serde_json::to_string(&policy.rules).unwrap(),
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

fn run_workspace(conn: &Connection, run_id: &str) -> Result<String, TransportError> {
    conn.query_row(
        "SELECT workspace_id FROM cowork_runs WHERE id = ?1",
        params![run_id],
        |row| row.get(0),
    )
    .map_err(store_err)
}

fn synthetic_auto_binding(
    conn: &Connection,
    job: &JobRow,
    principal: &PrincipalRecord,
    capability: &str,
    target: &str,
    granted: bool,
    reason: &str,
) -> Result<(), TransportError> {
    let attr = load_run_attribution(conn, &job.run_id)?;
    insert_event(
        conn,
        &job.run_id,
        if granted { "approval.granted" } else { "approval.denied" },
        serde_json::json!({
            "approval_id": format!("risk-rule:{}", if granted { "approve" } else { "reject" }),
            "job_id": job.id,
            "capability": capability,
            "target": target,
            "decided_by": format!("risk-rule ({reason})"),
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )
}

/// Request an approval for a protected action under the caller's current
/// lease. Single policy path: risk rules first decide whether an approval is
/// needed at all — auto-approve returns a synthetic granted binding (no row,
/// attributed `approval.granted` ledger event), auto-deny is rejected with an
/// attributed `approval.denied` event, and only genuinely protected actions
/// create a binding scoped to (job, capability, target, generation) with a
/// server-clock `expires_at`. Idempotent per scope+generation.
#[allow(clippy::too_many_arguments)]
pub fn request_approval(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    capability: &str,
    target: &str,
    ttl: Option<Duration>,
) -> Result<ApprovalBinding, TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;
    let workspace = run_workspace(conn, &job.run_id)?;
    let policy = load_policy(conn, &workspace)?;

    match crate::risk_policy::evaluate_protection(&policy, capability) {
        crate::risk_policy::ProtectionDecision::AutoApprove(reason) => {
            synthetic_auto_binding(conn, &job, principal, capability, target, true, &reason)?;
            return Ok(ApprovalBinding {
                id: format!("auto:approve:{capability}"),
                run_id: job.run_id.clone(),
                job_id: job_id.to_string(),
                lease_id: lease_id.to_string(),
                lease_generation: generation,
                executor: principal.id.clone(),
                capability: capability.to_string(),
                target: target.to_string(),
                status: "granted".to_string(),
                decided_by: Some(format!("risk-rule ({reason})")),
                expires_at: None,
            });
        }
        crate::risk_policy::ProtectionDecision::AutoDeny(reason) => {
            synthetic_auto_binding(conn, &job, principal, capability, target, false, &reason)?;
            return Err(TransportError::new(
                Code::PermissionDenied,
                format!("risk policy auto-denies {capability}: {reason}"),
            ));
        }
        crate::risk_policy::ProtectionDecision::RequiresApproval => {}
    }

    if let Some(existing) = conn
        .query_row(
            "SELECT id FROM cowork_approval_bindings
             WHERE job_id = ?1 AND capability = ?2 AND target = ?3
               AND lease_generation = ?4 AND status IN ('pending','granted')
             ORDER BY created_at DESC LIMIT 1",
            params![job_id, capability, target, generation],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(store_err)?
    {
        return load_binding(conn, &existing)?
            .ok_or_else(|| TransportError::new(Code::Store, "binding vanished"));
    }

    let id = format!("appr_{}", Uuid::new_v4());
    let expires_at = (Utc::now() + ttl.unwrap_or(Duration::from_secs(300))).to_rfc3339();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute(
        "INSERT INTO cowork_approval_bindings
            (id, run_id, job_id, lease_id, lease_generation, executor, capability, target, status, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9)",
        params![
            id,
            job.run_id,
            job_id,
            lease_id,
            generation,
            principal.id,
            capability,
            target,
            expires_at,
        ],
    )
    .map_err(store_err)?;
    let attr = load_run_attribution(&tx, &job.run_id)?;
    insert_event(
        &tx,
        &job.run_id,
        "approval.requested",
        serde_json::json!({
            "approval_id": id,
            "job_id": job_id,
            "capability": capability,
            "target": target,
            "lease_generation": generation,
            "expires_at": expires_at,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    tx.commit().map_err(store_err)?;

    load_binding(conn, &id)?
        .ok_or_else(|| TransportError::new(Code::Store, "binding vanished after insert"))
}

/// Human decision on a pending approval (§8.22 approval.granted/denied).
pub fn decide_approval(
    conn: &mut Connection,
    approval_id: &str,
    grant: bool,
    decided_by: &str,
) -> Result<ApprovalBinding, TransportError> {
    let binding = load_binding(conn, approval_id)?
        .ok_or_else(|| TransportError::new(Code::JobNotFound, format!("approval {approval_id} not found")))?;
    // Server clock: a decision arriving after expires_at is dead on arrival —
    // the requester has already been told to re-request (A_APPROVAL_REQUIRED).
    if let Some(exp) = binding.expires_at.as_deref() {
        if parse_time(exp).map(|e| e < Utc::now()).unwrap_or(false) {
            return Err(TransportError::new(
                Code::ApprovalInvalid,
                format!("approval {approval_id} expired at {exp}; a new request is required"),
            ));
        }
    }
    match binding.status.as_str() {
        "granted" if grant => return Ok(binding),
        "denied" if !grant => return Ok(binding),
        "pending" => {}
        other => {
            return Err(TransportError::new(
                Code::ApprovalInvalid,
                format!("approval {approval_id} is {other}, not pending"),
            ))
        }
    }

    let new_status = if grant { "granted" } else { "denied" };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute(
        "UPDATE cowork_approval_bindings SET status = ?1, decided_at = ?2, decided_by = ?3
         WHERE id = ?4 AND status = 'pending'",
        params![new_status, Utc::now().to_rfc3339(), decided_by, approval_id],
    )
    .map_err(store_err)?;
    let attr = load_run_attribution(&tx, &binding.run_id)?;
    insert_event(
        &tx,
        &binding.run_id,
        if grant { "approval.granted" } else { "approval.denied" },
        serde_json::json!({
            "approval_id": approval_id,
            "capability": binding.capability,
            "target": binding.target,
            "lease_generation": binding.lease_generation,
            "decided_by": decided_by,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(binding.executor.as_str()),
    )?;
    tx.commit().map_err(store_err)?;

    load_binding(conn, approval_id)?
        .ok_or_else(|| TransportError::new(Code::Store, "binding vanished after decision"))
}

/// Check whether a protected action may proceed under the caller's current
/// lease (§8.14). Stale-generation or invalidated approvals are rejected so
/// approvals can never be replayed across reassignment.
pub fn check_approval(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    capability: &str,
    target: &str,
) -> Result<ApprovalBinding, TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;

    // Single policy path: risk rules decide whether an approval is needed.
    let workspace = run_workspace(conn, &job.run_id)?;
    let policy = load_policy(conn, &workspace)?;
    match crate::risk_policy::evaluate_protection(&policy, capability) {
        crate::risk_policy::ProtectionDecision::AutoApprove(reason) => {
            return Ok(ApprovalBinding {
                id: format!("auto:approve:{capability}"),
                run_id: job.run_id.clone(),
                job_id: job_id.to_string(),
                lease_id: lease_id.to_string(),
                lease_generation: generation,
                executor: principal.id.clone(),
                capability: capability.to_string(),
                target: target.to_string(),
                status: "granted".to_string(),
                decided_by: Some(format!("risk-rule ({reason})")),
                expires_at: None,
            });
        }
        crate::risk_policy::ProtectionDecision::AutoDeny(reason) => {
            return Err(TransportError::new(
                Code::PermissionDenied,
                format!("risk policy auto-denies {capability}: {reason}"),
            ));
        }
        crate::risk_policy::ProtectionDecision::RequiresApproval => {}
    }

    let latest = conn
        .query_row(
            "SELECT id FROM cowork_approval_bindings
             WHERE job_id = ?1 AND capability = ?2 AND target = ?3
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![job_id, capability, target],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(store_err)?;

    let Some(binding_id) = latest else {
        return Err(TransportError::new(
            Code::ApprovalRequired,
            format!("no approval on file for {capability}/{target} on job {job_id}"),
        ));
    };
    let binding = load_binding(conn, &binding_id)?
        .ok_or_else(|| TransportError::new(Code::Store, "binding vanished"))?;

    if binding.lease_generation != job.lease_generation {
        return Err(TransportError::new(
            Code::ApprovalInvalid,
            format!(
                "approval {} is bound to lease generation {}, current is {}",
                binding.id, binding.lease_generation, job.lease_generation
            ),
        ));
    }
    // Server clock: a pending binding past expires_at is treated as expired
    // here even before the sweeper marks it — the worker must re-request.
    let expired = binding.status == "pending"
        && binding
            .expires_at
            .as_deref()
            .and_then(parse_time)
            .map(|e| e < Utc::now())
            .unwrap_or(false);

    match (binding.status.as_str(), expired) {
        ("granted", _) => Ok(binding),
        ("pending", false) => Err(TransportError::new(
            Code::ApprovalRequired,
            format!("approval {} is still pending for {capability}/{target}", binding.id),
        )),
        ("pending", true) => Err(TransportError::new(
            Code::ApprovalRequired,
            format!(
                "approval {} expired un-acted; re-request per recovery policy",
                binding.id
            ),
        )),
        ("denied", _) => Err(TransportError::new(
            Code::ApprovalRequired,
            format!("approval {} was denied; a new approval must be obtained", binding.id),
        )),
        // Expired (sweeper or server-clock): the worker must re-request —
        // recovery policy is re-request, deny-by-default.
        ("expired", _) => Err(TransportError::new(
            Code::ApprovalRequired,
            format!("approval {} expired un-acted; re-request per recovery policy", binding.id),
        )),
        (other, _) => Err(TransportError::new(
            Code::ApprovalInvalid,
            format!("approval {} is {other} and cannot be reused", binding.id),
        )),
    }
}

/// Read an approval binding (worker polling for the human decision).
pub fn get_approval(
    conn: &Connection,
    approval_id: &str,
) -> Result<Option<ApprovalBinding>, TransportError> {
    load_binding(conn, approval_id)
}

/// Expire stale approval requests (server clock): pending bindings past
/// `expires_at` become `expired` with an attributed `approval.expired` event.
/// The sweeper and the boot pass both run this so an approval can never be
/// granted late after a worker has already been told to re-request.
pub fn expire_approvals(
    conn: &mut Connection,
    now: DateTime<Utc>,
) -> Result<Vec<String>, TransportError> {
    let stale: Vec<(String, String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT b.id, b.run_id, b.executor, b.capability
                 FROM cowork_approval_bindings b
                 WHERE b.status = 'pending' AND b.expires_at IS NOT NULL",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        rows
    };

    let mut expired = Vec::new();
    for (id, run_id, executor, capability) in stale {
        let is_stale = conn
            .query_row(
                "SELECT expires_at FROM cowork_approval_bindings WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(store_err)?
            .flatten()
            .and_then(|e| parse_time(&e))
            .map(|e| e < now)
            .unwrap_or(false);
        if !is_stale {
            continue;
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_err)?;
        let updated = tx
            .execute(
                "UPDATE cowork_approval_bindings SET status = 'expired'
                 WHERE id = ?1 AND status = 'pending'",
                params![id],
            )
            .map_err(store_err)?;
        if updated == 0 {
            tx.rollback().map_err(store_err)?;
            continue;
        }
        let attr = load_run_attribution(&tx, &run_id)?;
        insert_event(
            &tx,
            &run_id,
            "approval.expired",
            serde_json::json!({
                "approval_id": id,
                "capability": capability,
                "expired_at": now.to_rfc3339(),
            }),
            attr.initiator.as_deref(),
            attr.delegator.as_deref(),
            Some(executor.as_str()),
        )?;
        tx.commit().map_err(store_err)?;
        expired.push(id);
    }
    Ok(expired)
}

/// Idempotent event insert (A:// §5): a client-supplied `client_event_id`
/// dedupes retries — duplicate delivery returns the canonical existing event
/// id instead of double-writing. `None` keeps the legacy always-insert path.
pub enum EventInsertOutcome {
    /// A new event row was written.
    Inserted(String),
    /// The idempotency key already exists; canonical existing event id.
    Duplicate(String),
}

/// Insert a run event with a client-supplied idempotency key (§5): retries
/// with the same `event_id` return the canonical existing event instead of
/// double-writing. Returns `InsertEventOutcome::Inserted` or `Duplicate`.
#[allow(clippy::too_many_arguments)]
pub fn insert_event_idempotent(
    conn: &mut Connection,
    run_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    initiator: Option<&str>,
    delegator: Option<&str>,
    executor: Option<&str>,
    client_event_id: Option<&str>,
) -> Result<EventInsertOutcome, TransportError> {
    if let Some(key) = client_event_id {
        if let Some(existing) = conn
            .query_row(
                "SELECT id FROM cowork_run_events
                 WHERE run_id = ?1 AND client_event_id = ?2",
                params![run_id, key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(store_err)?
        {
            return Ok(EventInsertOutcome::Duplicate(existing));
        }
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO cowork_run_events
                (id, run_id, event_type, payload, initiator, delegator, executor, client_event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                run_id,
                event_type,
                payload.to_string(),
                initiator,
                delegator,
                executor,
                key,
            ],
        )
        .map_err(store_err)?;
        return Ok(EventInsertOutcome::Inserted(id));
    }

    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO cowork_run_events (id, run_id, event_type, payload, initiator, delegator, executor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            run_id,
            event_type,
            payload.to_string(),
            initiator,
            delegator,
            executor,
        ],
    )
    .map_err(store_err)?;
    Ok(EventInsertOutcome::Inserted(id))
}

// ─── Consolidation boundary projections (P-T1) ───────────────────────────────
//
// The Rails cowork REST surface (`cmd/allternit-api/src/rails/routes_cowork.rs`)
// mirrors RunManager state into the canonical tables through these helpers.
// Rules enforced here:
//   * lease columns (`lease_owner`, `lease_id`, `lease_generation`,
//     `lease_expires_at`, `claimed_at`) are never touched by a projection;
//   * a projection state write is refused (returns `Ok(false)`) when the job
//     is currently leased — the transport (complete/expire) owns that state;
//   * run events go through the idempotent insert (A:// §5).

use crate::types::{Job, Run};

/// Persist a RunManager run as a canonical `cowork_runs` projection row.
/// Upserts run state columns only; never touches attribution written by the
/// transport (`delegator` is set once via [`set_run_delegator`]).
pub fn persist_run_record(
    conn: &Connection,
    run: &Run,
    user_id: &str,
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_runs (id, tenant_id, workspace_id, initiator, mode, state, entrypoint, dag_id, current_job_id, current_checkpoint_id, policy_profile, created_at, updated_at, completed_at, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET
            state=excluded.state,
            current_job_id=excluded.current_job_id,
            current_checkpoint_id=excluded.current_checkpoint_id,
            updated_at=excluded.updated_at,
            completed_at=excluded.completed_at",
        params![
            run.id.to_string(),
            run.tenant_id,
            run.workspace_id,
            run.initiator,
            run.mode.to_string(),
            run.state.to_string(),
            run.entrypoint,
            run.dag_id,
            run.current_job_id.map(|j| j.to_string()),
            run.current_checkpoint_id,
            run.policy_profile,
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
            run.completed_at.map(|dt| dt.to_rfc3339()),
            user_id,
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Record the A:// delegator attribution (§8.18) on a run, once.
pub fn set_run_delegator(
    conn: &Connection,
    run_id: &str,
    delegator: &str,
) -> Result<(), TransportError> {
    conn.execute(
        "UPDATE cowork_runs SET delegator = ?1 WHERE id = ?2",
        params![delegator, run_id],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Persist a RunManager job as a canonical `cowork_jobs` projection row.
/// Lease-safe: the upsert never overwrites lease columns, and a stored row
/// that currently holds a fabric-transport lease keeps its `state` — only
/// the transport (`complete_job` / `expire_leases`) may move a leased job.
pub fn persist_job_record(
    conn: &Connection,
    job: &Job,
    user_id: &str,
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_jobs (id, run_id, dag_node_id, job_type, priority, state, lease_owner, retry_count, max_retries, timeout_sec, payload, created_at, updated_at, started_at, completed_at, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(id) DO UPDATE SET
            state = CASE WHEN cowork_jobs.lease_id IS NOT NULL
                         THEN cowork_jobs.state ELSE excluded.state END,
            retry_count=excluded.retry_count,
            updated_at=excluded.updated_at,
            started_at=excluded.started_at,
            completed_at=excluded.completed_at",
        params![
            job.id.to_string(),
            job.run_id.to_string(),
            job.dag_node_id,
            job.job_type,
            job.priority,
            job.state.to_string(),
            job.lease_owner,
            job.retry_count,
            job.max_retries,
            job.timeout_sec,
            job.payload.to_string(),
            job.created_at.to_rfc3339(),
            job.updated_at.to_rfc3339(),
            job.started_at.map(|dt| dt.to_rfc3339()),
            job.completed_at.map(|dt| dt.to_rfc3339()),
            user_id,
        ],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Move a freshly-created job into the fabric-transport queue: state
/// `queued`, mandatory capabilities (§8.6–8.7), attribution inherited from
/// the parent run, and the validated causation chain (§8.15). Refused
/// (`Ok(false)`) if the job already holds a lease — the transport owns it.
pub fn mark_job_queued_for_transport(
    conn: &Connection,
    job_id: &str,
    run_id: &str,
    required_capabilities_json: &str,
    causation_chain_json: &str,
) -> Result<bool, TransportError> {
    let leased: bool = conn
        .query_row(
            "SELECT lease_id IS NOT NULL FROM cowork_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(store_err)?;
    if leased {
        return Ok(false);
    }
    conn.execute(
        "UPDATE cowork_jobs SET state = 'queued', required_capabilities = ?1,
            initiator = (SELECT initiator FROM cowork_runs WHERE id = ?2),
            delegator = (SELECT delegator FROM cowork_runs WHERE id = ?2),
            causation_chain = ?4
         WHERE id = ?3",
        params![required_capabilities_json, run_id, job_id, causation_chain_json],
    )
    .map_err(store_err)?;
    Ok(true)
}

/// Projection state transition for a RunManager job. Refused (`Ok(false)`)
/// when the job currently holds a fabric-transport lease: a product surface
/// must not move leased work — only `complete_job` / `expire_leases` may.
pub fn transition_job_record(
    conn: &Connection,
    job_id: &str,
    state: &str,
    started_at: Option<String>,
    completed_at: Option<String>,
) -> Result<bool, TransportError> {
    let leased: bool = conn
        .query_row(
            "SELECT lease_id IS NOT NULL FROM cowork_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(store_err)?;
    if leased {
        return Ok(false);
    }
    conn.execute(
        "UPDATE cowork_jobs SET state = ?1, updated_at = CURRENT_TIMESTAMP, started_at = COALESCE(?2, started_at), completed_at = COALESCE(?3, completed_at) WHERE id = ?4",
        params![state, started_at, completed_at, job_id],
    )
    .map_err(store_err)?;
    Ok(true)
}

/// Point a run's `current_job_id` at a (newly created) job.
pub fn set_current_run_job(conn: &Connection, run_id: &str, job_id: &str) -> Result<(), TransportError> {
    conn.execute(
        "UPDATE cowork_runs SET current_job_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![job_id, run_id],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Record a run's latest checkpoint pointer.
pub fn set_run_checkpoint(
    conn: &Connection,
    run_id: &str,
    checkpoint_id: &str,
) -> Result<(), TransportError> {
    conn.execute(
        "UPDATE cowork_runs SET current_checkpoint_id = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
        params![checkpoint_id, run_id],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Projection state transition for a run (runs carry no lease; the transport
/// mirrors terminal run state itself via `submit_intent` bookkeeping).
/// `completed_at` is caller-computed for terminal states.
pub fn update_run_state_record(
    conn: &Connection,
    run_id: &str,
    state: &str,
    completed_at: Option<String>,
) -> Result<(), TransportError> {
    conn.execute(
        "UPDATE cowork_runs SET state = ?1, updated_at = CURRENT_TIMESTAMP, completed_at = COALESCE(?2, completed_at) WHERE id = ?3",
        params![state, completed_at, run_id],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Load a run row from the store as a runtime `Run` (manager-mirror shape).
/// This is the store-side half of the route-level mirror in
/// `fabric_transport_routes::submit_intent`; anything that creates runs
/// directly in the store (the Al orchestrator's child intents) uses this so
/// the in-memory RunManager and the REST run surface stay consistent.
pub fn load_run_record(
    conn: &Connection,
    run_id: &str,
) -> Result<Option<crate::Run>, TransportError> {
    let row = conn
        .query_row(
            "SELECT tenant_id, workspace_id, initiator, mode, state, entrypoint, dag_id,
                    policy_profile, created_at
             FROM cowork_runs WHERE id = ?1",
            params![run_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(store_err)?;
    let Some((tenant_id, workspace_id, initiator, mode, state, entrypoint, dag_id, policy_profile, created_at)) = row
    else {
        return Ok(None);
    };
    let parse = |v: &str| {
        chrono::DateTime::parse_from_rfc3339(v)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    };
    let run_uuid = uuid::Uuid::parse_str(run_id).map_err(|e| {
        TransportError::new(Code::Store, format!("run id {run_id} is not a uuid: {e}"))
    })?;
    Ok(Some(crate::Run {
        id: crate::RunId(run_uuid),
        tenant_id,
        workspace_id,
        initiator,
        mode: mode.parse().unwrap_or(crate::RunMode::Cowork),
        state: state.parse().unwrap_or(crate::RunState::Queued),
        entrypoint,
        dag_id,
        current_job_id: None,
        current_checkpoint_id: None,
        policy_profile,
        created_at: parse(&created_at),
        updated_at: parse(&created_at),
        completed_at: None,
    }))
}

/// Append a product-surface run event through the canonical idempotent
/// insert, preserving V142 ownership attribution. Returns the event id.
pub fn record_run_event_projection(
    conn: &mut Connection,
    run_id: &str,
    event_type: &str,
    payload: serde_json::Value,
    user_id: &str,
) -> Result<String, TransportError> {
    let outcome = insert_event_idempotent(conn, run_id, event_type, payload, None, None, None, None)?;
    let id = match outcome {
        EventInsertOutcome::Inserted(id) => {
            conn.execute(
                "UPDATE cowork_run_events SET user_id = ?1 WHERE id = ?2",
                params![user_id, id],
            )
            .map_err(store_err)?;
            id
        }
        EventInsertOutcome::Duplicate(id) => id,
    };
    Ok(id)
}

// ─── Delegation chains (§8.15) and canonical intents (§5) ────────────────────

/// Validate a causation chain (§8.15): reject cycles and depth over the
/// workspace limit (default 4; workspace policy may reduce it).
pub fn validate_delegation_chain(
    conn: &Connection,
    workspace: &str,
    chain: &[String],
) -> Result<(), TransportError> {
    let max_depth: i64 = conn
        .query_row(
            "SELECT max_delegation_depth FROM cowork_approval_policy WHERE workspace = ?1",
            params![workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(store_err)?
        .unwrap_or(4);
    if chain.len() as i64 > max_depth {
        return Err(TransportError::new(
            Code::DelegationDepthExceeded,
            format!(
                "delegation chain length {} exceeds workspace limit {max_depth} (§8.15)",
                chain.len()
            ),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for principal in chain {
        if !seen.insert(principal) {
            return Err(TransportError::new(
                Code::DelegationCycle,
                format!("delegation cycle: {principal} appears twice in the chain"),
            ));
        }
    }
    Ok(())
}

/// True when an intent target addresses Al, in either the canonical long
/// form (`a://workspace/{ws}/principal/al`) or the documented short alias
/// (`principal/al`, A_PROTOCOL §5). Both forms name the same orchestrator;
/// the alias must not change execution semantics.
pub fn targets_al(target: Option<&str>) -> bool {
    target
        .map(|t| t == "principal/al" || t.ends_with("/principal/al"))
        .unwrap_or(false)
}

/// Submit an IntentEnvelope (§5). Idempotent on `intent_id`: resubmission
/// resolves to the canonical existing run. Creates the run row directly in
/// the store (state `queued`, §8.2 honesty) with the attribution triple and
/// causation chain; callers mirror it into the in-memory manager.
///
/// The run is stamped with `owner` (the authenticated product user) so the
/// V142 ownership scoping on the run/job/event routes sees it: intent-created
/// runs must be listable and job-postable like any other run.
pub fn submit_intent(
    conn: &mut Connection,
    envelope: &crate::transport::IntentEnvelope,
) -> Result<crate::transport::IntentSubmission, TransportError> {
    submit_intent_for_user(conn, envelope, None)
}

/// [`submit_intent`] with an owning product user stamped on the created run.
pub fn submit_intent_for_user(
    conn: &mut Connection,
    envelope: &crate::transport::IntentEnvelope,
    owner: Option<&str>,
) -> Result<crate::transport::IntentSubmission, TransportError> {
    if envelope.version != "a/0.1" {
        return Err(TransportError::new(
            Code::PermissionDenied,
            format!("unsupported intent version {} (A_UNSUPPORTED_VERSION)", envelope.version),
        ));
    }
    if envelope.intent_id.is_empty() || envelope.initiator.is_empty() {
        return Err(TransportError::new(
            Code::PermissionDenied,
            "intent_id and initiator are required",
        ));
    }
    let workspace = envelope
        .workspace
        .strip_prefix("a://workspace/")
        .unwrap_or(envelope.workspace.as_str())
        .to_string();
    validate_delegation_chain(conn, &workspace, &envelope.causation_chain)?;

    // Idempotency: same intent_id → same canonical run (§5).
    if let Some(existing_run) = conn
        .query_row(
            "SELECT run_id FROM cowork_intents WHERE intent_id = ?1",
            params![envelope.intent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(store_err)?
    {
        return Ok(crate::transport::IntentSubmission {
            intent_id: envelope.intent_id.clone(),
            run_id: existing_run,
            created: false,
        });
    }

    let run_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute(
        "INSERT INTO cowork_runs
            (id, tenant_id, workspace_id, initiator, delegator, mode, state,
             entrypoint, dag_id, policy_profile, created_at, updated_at, causation_chain, user_id)
         VALUES (?1, 'local', ?2, ?3, ?4, 'cowork', 'queued', ?5, ?6, 'default', ?7, ?7, ?8, ?9)",
        params![
            run_id,
            workspace,
            envelope.initiator,
            envelope.delegator,
            envelope.action.action_type,
            format!("dag-{run_id}"),
            now,
            serde_json::to_string(&envelope.causation_chain).unwrap(),
            owner,
        ],
    )
    .map_err(store_err)?;
    tx.execute(
        "INSERT INTO cowork_intents (intent_id, run_id, envelope, initiator, delegator, causation_chain)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            envelope.intent_id,
            run_id,
            serde_json::to_string(envelope).unwrap(),
            envelope.initiator,
            envelope.delegator,
            serde_json::to_string(&envelope.causation_chain).unwrap(),
        ],
    )
    .map_err(store_err)?;
    // P-T2: enqueue the run's canonical job with the compute policy resolved
    // to mandatory capabilities (§8.8), completing Intent → Run → Job →
    // queue. Intents targeted at Al are the exception: the parent is planned,
    // not executed, by Al — the orchestrator's child intent carries the
    // claimable job, so a parent job here would let a worker bypass
    // delegation.
    let targeted_at_al = targets_al(envelope.target.as_deref());
    if !targeted_at_al {
        let req = compute_requirements(&envelope.compute);
        let job_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO cowork_jobs
                (id, run_id, dag_node_id, job_type, state, payload, required_capabilities, timeout_sec,
                 max_retries, initiator, delegator, user_id)
             VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                job_id,
                run_id,
                format!("dag-{job_id}"),
                envelope.action.action_type,
                envelope
                    .action
                    .payload
                    .clone()
                    .unwrap_or(serde_json::json!({}))
                    .to_string(),
                serde_json::to_string(&req).unwrap(),
                3600,
                3,
                envelope.initiator,
                envelope.delegator,
                owner,
            ],
        )
        .map_err(store_err)?;
    }
    insert_event(
        &tx,
        &run_id,
        "intent.accepted",
        serde_json::json!({
            "intent_id": envelope.intent_id,
            "action_type": envelope.action.action_type,
            "target": envelope.target,
        }),
        Some(envelope.initiator.as_str()),
        envelope.delegator.as_deref(),
        envelope.target.as_deref(),
    )?;
    tx.commit().map_err(store_err)?;

    Ok(crate::transport::IntentSubmission {
        intent_id: envelope.intent_id.clone(),
        run_id,
        created: true,
    })
}

/// Stamp the authenticated owner on a canonical run (P-T1 boundary). The
/// Rails cowork REST surface scopes every read/mutation to `user_id`
/// (V169); fabric-transport run creation goes through this helper so
/// intent-created runs are visible to their owner there. Idempotent.
pub fn set_run_owner(
    conn: &mut Connection,
    run_id: &str,
    user_id: &str,
) -> Result<(), TransportError> {
    conn.execute(
        "UPDATE cowork_runs SET user_id = ?2 WHERE id = ?1",
        params![run_id, user_id],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Acknowledge a handoff (A-T1): marks the handoff completed and terminates
/// its linked job with a typed result. The handoff job was never leased, so
/// this is the contract-conformant completion path for handoffs.
pub fn ack_handoff(
    conn: &mut Connection,
    handoff_id: &str,
    responder: &str,
    note: Option<String>,
) -> Result<serde_json::Value, TransportError> {
    let handoff: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT run_id, job_id, status FROM cowork_handoffs WHERE id = ?1",
            params![handoff_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(store_err)?;
    let Some((run_id, job_id, status)) = handoff else {
        return Err(TransportError::new(
            Code::JobNotFound,
            format!("handoff {handoff_id} not found"),
        ));
    };
    if status == "completed" {
        return Ok(serde_json::json!({ "handoff_id": handoff_id, "status": "completed", "duplicated": true }));
    }

    let now = Utc::now().to_rfc3339();
    let result = serde_json::json!({
        "result_id": format!("result_handoff_{}", Uuid::new_v4()),
        "status": "completed",
        "summary": note.unwrap_or_else(|| "handoff acknowledged".to_string()),
        "executor": responder,
        "completed_at": now,
    });

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute(
        "UPDATE cowork_handoffs SET status = 'completed', completed_at = ?1 WHERE id = ?2 AND status != 'completed'",
        params![now, handoff_id],
    )
    .map_err(store_err)?;
    if let Some(jid) = &job_id {
        tx.execute(
            "UPDATE cowork_jobs SET state = 'completed', result = ?1, completed_at = ?2, updated_at = ?2
             WHERE id = ?3 AND state NOT IN ('completed','failed','dead_letter','cancelled')",
            params![result.to_string(), now, jid],
        )
        .map_err(store_err)?;
    }
    let attr = load_run_attribution(&tx, &run_id)?;
    insert_event(
        &tx,
        &run_id,
        "handoff.completed",
        serde_json::json!({
            "handoff_id": handoff_id,
            "job_id": job_id,
            "result_id": result["result_id"],
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(responder),
    )?;
    tx.commit().map_err(store_err)?;

    Ok(serde_json::json!({ "handoff_id": handoff_id, "status": "completed", "duplicated": false }))
}

/// Store a memory entry with principal ownership + grant list (A-T2).
#[allow(clippy::too_many_arguments)]
pub fn store_memory_entry(
    conn: &mut Connection,
    user_id: &str,
    project_id: Option<&str>,
    session_id: Option<&str>,
    content: &str,
    type_: &str,
    tags: Option<&str>,
    source: Option<&str>,
    owner_principal: Option<&str>,
    grants: &[String],
) -> Result<String, TransportError> {
    let id = format!("mem_{}", Uuid::new_v4());
    conn.execute(
        "INSERT INTO cowork_memory_entries
            (id, user_id, project_id, session_id, content, type, tags, source, owner_principal, grants)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            user_id,
            project_id,
            session_id,
            content,
            type_,
            tags,
            source,
            owner_principal,
            serde_json::to_string(grants).unwrap(),
        ],
    )
    .map_err(store_err)?;
    Ok(id)
}

/// Memory entries visible to a principal: unowned (legacy/user-scoped),
/// owned by the principal, or explicitly granted. Default-deny otherwise.
/// `idx` is the positional placeholder bound to the principal id.
fn memory_principal_filter(idx: usize) -> String {
    format!(
        "(owner_principal IS NULL OR owner_principal = ?{idx}
          OR EXISTS (SELECT 1 FROM json_each(cowork_memory_entries.grants) WHERE value = ?{idx}))"
    )
}

/// Read/search memory for a user + optional principal scope (A-T2).
///
/// `offset` skips the first N rows of the principal-filtered, recency-ordered
/// result set, so `limit` + `offset` page within the same visibility window.
pub fn search_memory_entries(
    conn: &Connection,
    user_id: &str,
    principal: Option<&str>,
    query: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<serde_json::Value>, TransportError> {
    let like = query.map(|q| format!("%{q}%"));
    let out = match (principal, like) {
        (Some(p), Some(q)) => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT id, content, type, tags, source, owner_principal, grants, created_at
                     FROM cowork_memory_entries
                     WHERE user_id = ?1 AND content LIKE ?2 AND {}
                     ORDER BY created_at DESC LIMIT ?3 OFFSET ?5",
                    memory_principal_filter(4),
                ))
                .map_err(store_err)?;
            let rows = stmt
                .query_map(params![user_id, q, limit, p.to_string(), offset], memory_row)
                .map_err(store_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_err)?;
            rows
        }
        (Some(p), None) => {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT id, content, type, tags, source, owner_principal, grants, created_at
                     FROM cowork_memory_entries
                     WHERE user_id = ?1 AND {}
                     ORDER BY created_at DESC LIMIT ?2 OFFSET ?4",
                    memory_principal_filter(3),
                ))
                .map_err(store_err)?;
            let rows = stmt
                .query_map(params![user_id, limit, p.to_string(), offset], memory_row)
                .map_err(store_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_err)?;
            rows
        }
        (None, Some(q)) => {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, type, tags, source, owner_principal, grants, created_at
                     FROM cowork_memory_entries
                     WHERE user_id = ?1 AND content LIKE ?2
                     ORDER BY created_at DESC LIMIT ?3 OFFSET ?4",
                )
                .map_err(store_err)?;
            let rows = stmt
                .query_map(params![user_id, q, limit, offset], memory_row)
                .map_err(store_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_err)?;
            rows
        }
        (None, None) => {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, type, tags, source, owner_principal, grants, created_at
                     FROM cowork_memory_entries
                     WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
                )
                .map_err(store_err)?;
            let rows = stmt
                .query_map(params![user_id, limit, offset], memory_row)
                .map_err(store_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(store_err)?;
            rows
        }
    };
    Ok(out)
}

/// Forget one memory entry the user owns. Returns whether a row was removed.
pub fn delete_memory_entry(conn: &Connection, user_id: &str, id: &str) -> Result<bool, TransportError> {
    let n = conn
        .execute("DELETE FROM cowork_memory_entries WHERE id = ?1 AND user_id = ?2", params![id, user_id])
        .map_err(store_err)?;
    Ok(n > 0)
}

fn memory_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "id": row.get::<_, String>(0)?,
        "content": row.get::<_, String>(1)?,
        "type": row.get::<_, String>(2)?,
        "tags": row.get::<_, Option<String>>(3)?,
        "source": row.get::<_, Option<String>>(4)?,
        "owner_principal": row.get::<_, Option<String>>(5)?,
        "grants": row.get::<_, String>(6)?,
        "created_at": row.get::<_, String>(7)?,
    }))
}

/// Write-time access check (A-T2): a principal may write only entries it
/// owns, unowned entries, or entries explicitly granted to it. Default-deny.
pub fn check_memory_write(
    conn: &Connection,
    entry_id: &str,
    principal: &str,
) -> Result<(), TransportError> {
    let allowed: Option<bool> = conn
        .query_row(
            &format!(
                "SELECT 1 FROM cowork_memory_entries
                 WHERE id = ?1 AND {}",
                memory_principal_filter(2),
            ),
            params![entry_id, principal.to_string()],
            |_| Ok(true),
        )
        .optional()
        .map_err(store_err)?;
    if allowed.is_none() {
        return Err(TransportError::new(
            Code::PermissionDenied,
            format!("principal {principal} has no grant on memory entry {entry_id}"),
        ));
    }
    Ok(())
}

// ─── Al orchestration loop v0.1 (A-T3; deterministic, AL_IMPLEMENTATION_SPEC §7) ──

/// One orchestration action, reported for logging/mirror sync.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrchestrationAction {
    /// Canonical intent the action advanced.
    pub intent_id: String,
    /// Parent (Al-targeted) run being orchestrated.
    pub parent_run_id: String,
    /// Action outcome, e.g. `delegated`, `rejected`.
    pub outcome: String,
    /// Human-readable detail for logs/mirror sync.
    pub detail: String,
    /// Run created by a `delegated` action (the child run). `None` for
    /// rejections and result-recording actions. The API mirrors this run
    /// into the in-memory RunManager so store-direct children are as
    /// listable/job-postable as route-created runs.
    pub child_run_id: Option<String>,
}

fn al_principal_id(workspace: &str) -> String {
    format!("a://workspace/{workspace}/principal/al")
}

/// Process intents targeted at Al: resolve the delegation target from
/// `cowork_delegation_rules` (first match by priority, action-type prefix),
/// submit the child intent with the extended chain, and record attributed
/// `delegation.created` / `delegation.rejected` events. Deterministic — no
/// model involvement (§8.25 keeps model reasoning out of eligibility).
/// Resolve the delegation target for an action type (first matching rule by
/// priority). Shared by the Al orchestration loop and the P-T5 persona
/// runtime so both resolve targets identically.
pub fn resolve_delegation_rule(
    conn: &Connection,
    workspace: &str,
    action_type: &str,
) -> Result<Option<String>, TransportError> {
    let rule: Option<(String,)> = conn
        .query_row(
            "SELECT target_principal FROM cowork_delegation_rules
             WHERE workspace = ?1 AND ?2 LIKE action_type || '%'
             ORDER BY priority ASC, created_at ASC LIMIT 1",
            params![workspace, action_type],
            |row| Ok((row.get(0)?,)),
        )
        .optional()
        .map_err(store_err)?;
    Ok(rule.map(|(t,)| t))
}

/// Deterministic Al orchestration tick: for each pending Al-targeted intent,
/// resolve the delegation rule, submit the child intent with the extended
/// causation chain, and record attributed events. Returns the actions taken.
pub fn orchestrate_pending_intents(
    conn: &mut Connection,
) -> Result<Vec<OrchestrationAction>, TransportError> {
    let pending: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT intent_id, run_id FROM cowork_intents
                 WHERE orchestration_status = 'pending'
                   AND json_extract(envelope, '$.target') LIKE '%principal/al'",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        rows
    };

    let mut actions = Vec::new();
    for (intent_id, parent_run_id) in pending {
        let envelope_json: Option<String> = conn
            .query_row(
                "SELECT envelope FROM cowork_intents WHERE intent_id = ?1",
                params![intent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_err)?;
        let Some(env_raw) = envelope_json else { continue };
        let Ok(envelope) =
            serde_json::from_str::<crate::transport::IntentEnvelope>(&env_raw)
        else {
            continue;
        };
        let workspace = envelope
            .workspace
            .strip_prefix("a://workspace/")
            .unwrap_or(envelope.workspace.as_str())
            .to_string();
        let al = al_principal_id(&workspace);
        let initiator = Some(envelope.initiator.as_str());

        // Resolve the delegation target (first matching rule by priority).
        let target: Option<String> =
            resolve_delegation_rule(conn, &workspace, &envelope.action.action_type)?;
        let Some(target) = target else {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(store_err)?;
            tx.execute(
                "UPDATE cowork_intents SET orchestration_status = 'rejected' WHERE intent_id = ?1",
                params![intent_id],
            )
            .map_err(store_err)?;
            insert_event(
                &tx,
                &parent_run_id,
                "delegation.rejected",
                serde_json::json!({
                    "intent_id": intent_id,
                    "action_type": envelope.action.action_type,
                    "reason": "no delegation rule matched",
                }),
                initiator,
                Some(al.as_str()),
                None,
            )?;
            tx.commit().map_err(store_err)?;
            actions.push(OrchestrationAction {
                intent_id,
                parent_run_id,
                outcome: "rejected".to_string(),
                detail: "no delegation rule matched".to_string(),
                child_run_id: None,
            });
            continue;
        };

        // Canonicalize the target principal id.
        let target_principal = if target.contains("://") {
            target
        } else if target.starts_with("bot/") || target.contains('/') {
            format!("a://workspace/{workspace}/{target}")
        } else {
            format!("a://workspace/{workspace}/principal/{target}")
        };

        // Build the child intent: same action, chain extended with Al.
        let mut child_chain = envelope.causation_chain.clone();
        child_chain.push(al.clone());
        let child = crate::transport::IntentEnvelope {
            version: "a/0.1".to_string(),
            intent_id: format!("{intent_id}:child:{}", Uuid::new_v4()),
            workspace: envelope.workspace.clone(),
            initiator: envelope.initiator.clone(),
            delegator: Some(al.clone()),
            target: Some(target_principal.clone()),
            action: envelope.action.clone(),
            permissions: envelope.permissions.clone(),
            compute: envelope.compute.clone(),
            model: envelope.model.clone(),
            approval: envelope.approval.clone(),
            return_channel: envelope.return_channel.clone(),
            causation_chain: child_chain,
        };
        let submission = submit_intent(conn, &child)?;

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_err)?;
        tx.execute(
            "UPDATE cowork_intents SET child_run_id = ?1, orchestration_status = 'delegated'
             WHERE intent_id = ?2",
            params![submission.run_id, intent_id],
        )
        .map_err(store_err)?;
        insert_event(
            &tx,
            &parent_run_id,
            "delegation.created",
            serde_json::json!({
                "intent_id": intent_id,
                "child_intent_id": submission.intent_id,
                "child_run_id": submission.run_id,
                "action_type": envelope.action.action_type,
            }),
            initiator,
            Some(al.as_str()),
            Some(target_principal.as_str()),
        )?;
        tx.commit().map_err(store_err)?;
        actions.push(OrchestrationAction {
            intent_id,
            parent_run_id,
            outcome: "delegated".to_string(),
            detail: target_principal,
            child_run_id: Some(submission.run_id.clone()),
        });
    }
    Ok(actions)
}

/// Record orchestration results: when a delegated child run reaches a
/// terminal state, mirror it onto the parent run and write an attributed
/// `delegation.completed` / `delegation.failed` event.
pub fn record_orchestration_results(
    conn: &mut Connection,
) -> Result<Vec<OrchestrationAction>, TransportError> {
    let delegated: Vec<(String, String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT intent_id, run_id, child_run_id FROM cowork_intents
                 WHERE orchestration_status = 'delegated' AND child_run_id IS NOT NULL",
            )
            .map_err(store_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(store_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(store_err)?;
        rows
    };

    let mut actions = Vec::new();
    for (intent_id, parent_run_id, child_run_id) in delegated {
        let child: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT state, completed_at FROM cowork_runs WHERE id = ?1",
                params![child_run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(store_err)?;
        let Some((child_state, child_completed)) = child else { continue };
        if !matches!(child_state.as_str(), "completed" | "failed" | "cancelled") {
            continue;
        }

        let envelope_json: Option<String> = conn
            .query_row(
                "SELECT envelope FROM cowork_intents WHERE intent_id = ?1",
                params![intent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_err)?;
        let envelope: Option<crate::transport::IntentEnvelope> = envelope_json
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let al = envelope
            .as_ref()
            .map(|e| {
                let ws = e
                    .workspace
                    .strip_prefix("a://workspace/")
                    .unwrap_or(e.workspace.as_str());
                al_principal_id(ws)
            })
            .unwrap_or_default();
        let executor = envelope.as_ref().and_then(|e| e.target.clone());

        let now = Utc::now().to_rfc3339();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(store_err)?;
        tx.execute(
            "UPDATE cowork_intents SET orchestration_status = ?1 WHERE intent_id = ?2",
            params![if child_state == "completed" { "completed" } else { "failed" }, intent_id],
        )
        .map_err(store_err)?;
        // Mirror the terminal state onto the parent run (§6 lifecycle honesty).
        tx.execute(
            "UPDATE cowork_runs SET state = ?1, completed_at = COALESCE(?2, ?3), updated_at = ?3
             WHERE id = ?4 AND state NOT IN ('completed','failed','cancelled')",
            params![child_state, child_completed, now, parent_run_id],
        )
        .map_err(store_err)?;
        let attr = load_run_attribution(&tx, &parent_run_id)?;
        insert_event(
            &tx,
            &parent_run_id,
            if child_state == "completed" { "delegation.completed" } else { "delegation.failed" },
            serde_json::json!({
                "intent_id": intent_id,
                "child_run_id": child_run_id,
                "child_state": child_state,
            }),
            attr.initiator.as_deref().or(envelope.as_ref().map(|e| e.initiator.as_str())),
            Some(al.as_str()).filter(|s| !s.is_empty()).or(attr.delegator.as_deref()),
            executor.as_deref(),
        )?;
        tx.commit().map_err(store_err)?;
        actions.push(OrchestrationAction {
            intent_id,
            parent_run_id,
            outcome: child_state,
            detail: child_run_id,
            child_run_id: None,
        });
    }
    Ok(actions)
}

// ─── Connector broker v0.1 (A-T5; A:// §8.5) ────────────────────────────────

/// Request a brokered connector session (A:// §8.5). Validates the caller's
/// lease, the risk policy (protected capabilities require a granted approval
/// for the current generation), and that the capability is registered. Issues
/// a short-lived session id — raw secrets are NEVER returned or embedded in
/// job payloads; the SYSTEM performs the external call at invoke time.
pub fn request_connector_session(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    capability: &str,
    ttl: Option<Duration>,
) -> Result<serde_json::Value, TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;

    let secret_env: Option<String> = conn
        .query_row(
            "SELECT secret_env FROM cowork_connector_secrets WHERE capability = ?1",
            params![capability],
            |row| row.get(0),
        )
        .optional()
        .map_err(store_err)?;
    if secret_env.is_none() {
        return Err(TransportError::new(
            Code::PermissionDenied,
            format!("no connector registered for capability {capability}"),
        ));
    }

    // Risk policy: protected capabilities require a granted approval binding.
    let workspace = run_workspace(conn, &job.run_id)?;
    let policy = load_policy(conn, &workspace)?;
    if matches!(
        crate::risk_policy::evaluate_protection(&policy, capability),
        crate::risk_policy::ProtectionDecision::RequiresApproval
    ) {
        check_approval(conn, principal, job_id, lease_id, generation, capability, "*")?;
    }

    let id = format!("cs_{}", Uuid::new_v4());
    let expires_at = (Utc::now() + ttl.unwrap_or(Duration::from_secs(300))).to_rfc3339();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute(
        "INSERT INTO cowork_connector_sessions (id, principal, run_id, job_id, capability, status, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6)",
        params![id, principal.id, job.run_id, job_id, capability, expires_at],
    )
    .map_err(store_err)?;
    let attr = load_run_attribution(&tx, &job.run_id)?;
    insert_event(
        &tx,
        &job.run_id,
        "connector.session_created",
        serde_json::json!({
            "session_id": id,
            "capability": capability,
            "expires_at": expires_at,
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    tx.commit().map_err(store_err)?;

    Ok(serde_json::json!({
        "session_id": id,
        "capability": capability,
        "expires_at": expires_at,
    }))
}

/// Outcome of a brokered connector invocation (the external call the SYSTEM
/// performs with the registered secret — never the worker).
struct ConnectorInvokeOutcome {
    delivered: bool,
    detail: String,
}

/// Confine a connector-supplied relative path under the configured root
/// (P-T4 files connector). Rejects absolute paths, NULs, and `..` escapes.
fn confine_under_root(root: &std::path::Path, rel: &str) -> Result<std::path::PathBuf, String> {
    if rel.contains('\0') {
        return Err("path contains NUL".to_string());
    }
    let rel_path = std::path::Path::new(rel);
    if rel_path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("root not usable: {e}"))?;
    // Lexical normalization: resolve `.`/`..` components without requiring
    // the target to exist (writes create new files).
    let mut out = root_canon.clone();
    for comp in rel_path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err("path escapes connector root".to_string());
                }
            }
            std::path::Component::Normal(c) => out.push(c),
            _ => return Err("unsupported path component".to_string()),
        }
    }
    if !out.starts_with(&root_canon) {
        return Err("path escapes connector root".to_string());
    }
    Ok(out)
}

/// Dispatch a brokered invocation by capability family (P-T4). The `secret`
/// is the env-registered value: a webhook URL, a GitHub token, or the files
/// root directory. The worker never sees any of these.
async fn dispatch_connector_invoke(
    capability: &str,
    secret: &str,
    payload: &serde_json::Value,
) -> ConnectorInvokeOutcome {
    match capability {
        "connector.github.read" | "connector.github.write" => {
            github_connector_invoke(capability, secret, payload).await
        }
        "connector.files.read" | "connector.files.write" => {
            files_connector_invoke(capability, secret, payload)
        }
        _ => {
            // Reference webhook connector: POST the payload to the secret URL.
            let client = reqwest::Client::new();
            match client
                .post(secret)
                .header("X-Allternit-Connector", capability)
                .json(payload)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => ConnectorInvokeOutcome {
                    delivered: true,
                    detail: format!("HTTP {}", resp.status()),
                },
                Ok(resp) => ConnectorInvokeOutcome {
                    delivered: false,
                    detail: format!("HTTP {}", resp.status()),
                },
                Err(e) => ConnectorInvokeOutcome {
                    delivered: false,
                    detail: format!("transport error: {e}"),
                },
            }
        }
    }
}

/// GitHub connector (P-T4): repo read/write via a token from the operator's
/// env (`ALLTERNIT_BROKER_GITHUB_TOKEN`). Payloads:
///   read:  `{repo: "owner/name", path?: "dir/file", ref?: "main"}`
///   write: `{repo, path, content, message}` (write is approval-gated)
async fn github_connector_invoke(
    capability: &str,
    token: &str,
    payload: &serde_json::Value,
) -> ConnectorInvokeOutcome {
    let repo = payload
        .get("repo")
        .and_then(|r| r.as_str())
        .unwrap_or_default();
    if repo.is_empty() || !repo.contains('/') {
        return ConnectorInvokeOutcome {
            delivered: false,
            detail: "payload.repo must be 'owner/name'".to_string(),
        };
    }
    let path = payload.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.github.com/repos/{repo}/contents/{}",
        path.trim_start_matches('/')
    );
    let mut req = client
        .request(
            if capability == "connector.github.write" {
                reqwest::Method::PUT
            } else {
                reqwest::Method::GET
            },
            &url,
        )
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "allternit-fabric-transport");
    if capability == "connector.github.write" {
        let content = payload.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let message = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("allternit connector write");
        req = req.json(&serde_json::json!({
            "message": message,
            "content": base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                content.as_bytes(),
            ),
        }));
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => ConnectorInvokeOutcome {
            delivered: true,
            detail: format!("HTTP {}", resp.status()),
        },
        Ok(resp) => ConnectorInvokeOutcome {
            delivered: false,
            detail: format!("HTTP {}", resp.status()),
        },
        Err(e) => ConnectorInvokeOutcome {
            delivered: false,
            detail: format!("transport error: {e}"),
        },
    }
}

/// Files/local connector (P-T4): scoped folder read/write. The secret is the
/// operator-configured root directory (`ALLTERNIT_BROKER_FILES_ROOT`); every
/// payload path is confined under it (`..`/absolute escapes refused).
fn files_connector_invoke(
    capability: &str,
    root: &str,
    payload: &serde_json::Value,
) -> ConnectorInvokeOutcome {
    let rel = match payload.get("path").and_then(|p| p.as_str()) {
        Some(p) if !p.is_empty() => p,
        _ => {
            return ConnectorInvokeOutcome {
                delivered: false,
                detail: "payload.path is required".to_string(),
            }
        }
    };
    let target = match confine_under_root(std::path::Path::new(root), rel) {
        Ok(t) => t,
        Err(e) => return ConnectorInvokeOutcome { delivered: false, detail: e },
    };
    match capability {
        "connector.files.read" => match std::fs::read(&target) {
            Ok(bytes) => ConnectorInvokeOutcome {
                delivered: true,
                detail: format!("read {} bytes", bytes.len()),
            },
            Err(e) => ConnectorInvokeOutcome {
                delivered: false,
                detail: format!("read error: {e}"),
            },
        },
        _ => {
            let content = payload.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if let Some(parent) = target.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ConnectorInvokeOutcome {
                        delivered: false,
                        detail: format!("mkdir error: {e}"),
                    };
                }
            }
            match std::fs::write(&target, content) {
                Ok(()) => ConnectorInvokeOutcome {
                    delivered: true,
                    detail: format!("wrote {} bytes", content.len()),
                },
                Err(e) => ConnectorInvokeOutcome {
                    delivered: false,
                    detail: format!("write error: {e}"),
                },
            }
        }
    }
}

/// Invoke a brokered connector session: the SYSTEM performs the external call
/// with the registered secret (read from the env var at invoke time, server
/// side). The worker never sees the secret. When the env var is unset the
/// invocation is recorded as simulated (delivered: false) — honest, never
/// silent.
pub async fn invoke_connector_session(
    conn: &mut Connection,
    principal: &PrincipalRecord,
    job_id: &str,
    lease_id: &str,
    generation: i64,
    session_id: &str,
    payload: serde_json::Value,
) -> Result<serde_json::Value, TransportError> {
    let job = validate_lease(conn, principal, job_id, lease_id, generation)?;
    let session: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT capability, status, expires_at, principal FROM cowork_connector_sessions
             WHERE id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(store_err)?;
    let Some((capability, status, expires_at, session_principal)) = session else {
        return Err(TransportError::new(
            Code::JobNotFound,
            format!("connector session {session_id} not found"),
        ));
    };
    if session_principal != principal.id {
        return Err(TransportError::new(
            Code::PermissionDenied,
            "connector session belongs to another principal",
        ));
    }
    if status != "active" {
        return Err(TransportError::new(
            Code::InvalidLease,
            format!("connector session is {status}"),
        ));
    }
    if parse_time(&expires_at).map(|e| e < Utc::now()).unwrap_or(true) {
        return Err(TransportError::new(
            Code::LeaseExpired,
            format!("connector session expired at {expires_at}"),
        ));
    }

    let secret_env: String = conn
        .query_row(
            "SELECT secret_env FROM cowork_connector_secrets WHERE capability = ?1",
            params![capability],
            |row| row.get(0),
        )
        .map_err(store_err)?;

    let mut delivered = false;
    let mut simulated = true;
    let mut detail = "env unset".to_string();
    if let Ok(secret) = std::env::var(&secret_env) {
        if !secret.is_empty() {
            simulated = false;
            let outcome = dispatch_connector_invoke(&capability, &secret, &payload).await;
            delivered = outcome.delivered;
            detail = outcome.detail;
        }
    }

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    let attr = load_run_attribution(&tx, &job.run_id)?;
    insert_event(
        &tx,
        &job.run_id,
        "connector.invoked",
        serde_json::json!({
            "session_id": session_id,
            "capability": capability,
            "invoked_by": "broker",
            "delivered": delivered,
            "simulated": simulated,
            "detail": detail,
            "payload_keys": payload.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),
        }),
        attr.initiator.as_deref(),
        attr.delegator.as_deref(),
        Some(principal.id.as_str()),
    )?;
    tx.commit().map_err(store_err)?;

    Ok(serde_json::json!({
        "session_id": session_id,
        "capability": capability,
        "delivered": delivered,
        "simulated": simulated,
        "detail": detail,
    }))
}

/// List principals (P-T6 control surface). Optional workspace filter; token
/// hashes are never returned.
pub fn list_principals(
    conn: &Connection,
    workspace: Option<&str>,
) -> Result<Vec<serde_json::Value>, TransportError> {
    let (sql, ws) = match workspace {
        Some(w) => (
            "SELECT id, workspace, capabilities, roles, status, created_at
             FROM cowork_principals WHERE workspace = ?1 ORDER BY id",
            Some(w.to_string()),
        ),
        None => (
            "SELECT id, workspace, capabilities, roles, status, created_at
             FROM cowork_principals ORDER BY workspace, id",
            None,
        ),
    };
    let mut stmt = conn.prepare(sql).map_err(store_err)?;
    let rows = match &ws {
        Some(w) => stmt
            .query_map(params![w], principal_row)
            .map_err(store_err)?,
        None => stmt.query_map([], principal_row).map_err(store_err)?,
    };
    rows.collect::<Result<Vec<_>, _>>().map_err(store_err)
}

fn principal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Value> {
    let caps: String = row.get(2)?;
    let roles: String = row.get(3)?;
    Ok(serde_json::json!({
        "id": row.get::<_, String>(0)?,
        "workspace": row.get::<_, String>(1)?,
        "capabilities": serde_json::from_str::<serde_json::Value>(&caps).unwrap_or(serde_json::json!([])),
        "roles": serde_json::from_str::<serde_json::Value>(&roles).unwrap_or(serde_json::json!([])),
        "status": row.get::<_, String>(4)?,
        "created_at": row.get::<_, String>(5)?,
    }))
}

/// List delegation rules for a workspace (P-T6 control surface).
pub fn list_delegation_rules(
    conn: &Connection,
    workspace: &str,
) -> Result<Vec<serde_json::Value>, TransportError> {
    let mut stmt = conn
        .prepare(
            "SELECT workspace, action_type, target_principal, priority, created_at
             FROM cowork_delegation_rules WHERE workspace = ?1
             ORDER BY priority ASC, action_type ASC",
        )
        .map_err(store_err)?;
    let rows = stmt
        .query_map(params![workspace], |row| {
            Ok(serde_json::json!({
                "workspace": row.get::<_, String>(0)?,
                "action_type": row.get::<_, String>(1)?,
                "target_principal": row.get::<_, String>(2)?,
                "priority": row.get::<_, i64>(3)?,
                "created_at": row.get::<_, String>(4)?,
            }))
        })
        .map_err(store_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(store_err)
}

/// Upsert a delegation rule (P-T6). The rule takes effect on the next
/// orchestrator tick / persona resolution.
pub fn upsert_delegation_rule(
    conn: &Connection,
    workspace: &str,
    action_type: &str,
    target_principal: &str,
    priority: i64,
) -> Result<(), TransportError> {
    conn.execute(
        "INSERT INTO cowork_delegation_rules (workspace, action_type, target_principal, priority, created_at)
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)
         ON CONFLICT(workspace, action_type) DO UPDATE SET
            target_principal = excluded.target_principal,
            priority = excluded.priority",
        params![workspace, action_type, target_principal, priority],
    )
    .map_err(store_err)?;
    Ok(())
}

/// Delete a delegation rule; returns false when no row matched.
pub fn delete_delegation_rule(
    conn: &Connection,
    workspace: &str,
    action_type: &str,
) -> Result<bool, TransportError> {
    let n = conn
        .execute(
            "DELETE FROM cowork_delegation_rules WHERE workspace = ?1 AND action_type = ?2",
            params![workspace, action_type],
        )
        .map_err(store_err)?;
    Ok(n > 0)
}

/// List connector sessions (P-T6 control surface): short-lived brokered
/// sessions, optionally filtered by run.
pub fn list_connector_sessions(
    conn: &Connection,
    run_id: Option<&str>,
    limit: i64,
) -> Result<Vec<serde_json::Value>, TransportError> {
    let (sql, rid) = match run_id {
        Some(r) => (
            "SELECT id, principal, run_id, job_id, capability, status, expires_at, created_at
             FROM cowork_connector_sessions WHERE run_id = ?1
             ORDER BY created_at DESC LIMIT ?2",
            Some(r.to_string()),
        ),
        None => (
            "SELECT id, principal, run_id, job_id, capability, status, expires_at, created_at
             FROM cowork_connector_sessions ORDER BY created_at DESC LIMIT ?1",
            None,
        ),
    };
    let mut stmt = conn.prepare(sql).map_err(store_err)?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "principal": row.get::<_, String>(1)?,
            "run_id": row.get::<_, String>(2)?,
            "job_id": row.get::<_, String>(3)?,
            "capability": row.get::<_, String>(4)?,
            "status": row.get::<_, String>(5)?,
            "expires_at": row.get::<_, String>(6)?,
            "created_at": row.get::<_, String>(7)?,
        }))
    };
    let rows = match &rid {
        Some(r) => stmt.query_map(params![r, limit], map).map_err(store_err)?,
        None => stmt.query_map(params![limit], map).map_err(store_err)?,
    };
    rows.collect::<Result<Vec<_>, _>>().map_err(store_err)
}

/// Read back a submitted intent (idempotent observe).
pub fn get_intent(
    conn: &Connection,
    intent_id: &str,
) -> Result<Option<serde_json::Value>, TransportError> {
    let row = conn
        .query_row(
            "SELECT run_id, envelope, created_at FROM cowork_intents WHERE intent_id = ?1",
            params![intent_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(store_err)?;
    Ok(row.map(|(run_id, envelope, created_at)| {
        serde_json::json!({
            "intent_id": intent_id,
            "run_id": run_id,
            "envelope": serde_json::from_str::<serde_json::Value>(&envelope).unwrap_or(serde_json::Value::Null),
            "created_at": created_at,
        })
    }))
}
