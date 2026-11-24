//! Server-side tool registry and execution.
//!
//! Organization admins can register custom tools that run inside the platform
//! sandbox (WebVM / WASM / VM driver) rather than on the caller's machine.
//! Registered tools are invoked through the same `/tools/execute` surface as
//! native tools, and are also reachable from the MCP server tool list.
//!
//! Endpoints (all org-admin gated, merged at `/api/v1/admin/server-tools`):
//!   GET    /api/v1/admin/server-tools
//!   POST   /api/v1/admin/server-tools
//!   GET    /api/v1/admin/server-tools/:id
//!   PUT    /api/v1/admin/server-tools/:id
//!   DELETE /api/v1/admin/server-tools/:id

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{
    auth::AuthUser,
    sandbox_routes::{execute_sandbox_or_subprocess, SandboxExecuteRequest},
    AppState,
};

const ALLOWED_RUNTIMES: [&str; 4] = ["bash", "python", "node", "rust"];
const NAME_PATTERN: &str = "^[a-z0-9][a-z0-9_.-]{0,63}$";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/server-tools", get(list_server_tools))
        .route("/admin/server-tools", post(create_server_tool))
        .route("/admin/server-tools/:id", get(get_server_tool))
        .route("/admin/server-tools/:id", put(update_server_tool))
        .route("/admin/server-tools/:id", delete(delete_server_tool))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "server tool operation failed");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

fn admin_org(conn: &rusqlite::Connection, user: &AuthUser) -> Result<String, ApiError> {
    let org = user.organization_id.as_deref().ok_or_else(|| {
        error(
            StatusCode::FORBIDDEN,
            "organization_required",
            "An active organization is required.",
        )
    })?;
    if !crate::rbac::is_org_admin(conn, org, &user.user_id).map_err(internal)? {
        return Err(error(
            StatusCode::FORBIDDEN,
            "insufficient_role",
            "Only organization owners/admins can manage server tools.",
        ));
    }
    Ok(org.to_string())
}

fn validate_runtime(runtime: &str) -> Result<(), ApiError> {
    if ALLOWED_RUNTIMES.contains(&runtime.to_lowercase().as_str()) {
        Ok(())
    } else {
        Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_runtime",
            format!(
                "runtime must be one of: {}",
                ALLOWED_RUNTIMES.join(", ")
            ),
        ))
    }
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    let re = regex::Regex::new(NAME_PATTERN).unwrap();
    if re.is_match(name) {
        Ok(())
    } else {
        Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Tool name must be 1-64 lowercase alphanumeric characters, dots, dashes, or underscores.",
        ))
    }
}

fn tool_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "org_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "description": row.get::<_, Option<String>>(3)?,
        "runtime": row.get::<_, String>(4)?,
        "source": row.get::<_, String>(5)?,
        "entrypoint": row.get::<_, Option<String>>(6)?,
        "env": serde_json::from_str::<Value>(&row.get::<_, String>(7)?)
            .unwrap_or_else(|_| json!({})),
        "network_enabled": row.get::<_, i64>(8)? == 1,
        "timeout_secs": row.get::<_, i64>(9)?,
        "created_at": row.get::<_, String>(10)?,
        "updated_at": row.get::<_, String>(11)?,
    }))
}

async fn list_server_tools(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, org_id, name, description, runtime, source, entrypoint,
                        env, network_enabled, timeout_secs, created_at, updated_at
                 FROM server_tools WHERE org_id = ?1 ORDER BY name",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map([org], tool_json)
            .map_err(internal)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;
        Ok::<_, ApiError>(Json(json!({"items": rows})))
    })
    .await;
    match result {
        Ok(Ok(v)) => v.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateServerToolBody {
    name: String,
    description: Option<String>,
    runtime: String,
    source: String,
    entrypoint: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    network_enabled: Option<bool>,
    #[serde(default)]
    timeout_secs: Option<i64>,
}

fn validate_body(body: &CreateServerToolBody) -> Result<(), ApiError> {
    validate_name(&body.name)?;
    validate_runtime(&body.runtime)?;
    if body.source.trim().is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "missing_source",
            "Tool source must not be empty.",
        ));
    }
    if let Some(timeout) = body.timeout_secs {
        if timeout <= 0 || timeout > 3600 {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_timeout",
                "timeout_secs must be between 1 and 3600.",
            ));
        }
    }
    Ok(())
}

async fn create_server_tool(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateServerToolBody>,
) -> Response {
    if let Err(e) = validate_body(&body) {
        return e.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let network = body.network_enabled.unwrap_or(false) as i64;
        let timeout = body.timeout_secs.unwrap_or(30);
        conn.execute(
            "INSERT INTO server_tools
                (id, org_id, name, description, runtime, source, entrypoint,
                 env, network_enabled, timeout_secs, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![
                &id,
                &org,
                &body.name,
                body.description.as_deref(),
                &body.runtime,
                &body.source,
                body.entrypoint.as_deref(),
                serde_json::to_string(&body.env).unwrap(),
                network,
                timeout,
                &now,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                error(
                    StatusCode::CONFLICT,
                    "duplicate_name",
                    "A server tool with this name already exists in the organization.",
                )
            } else {
                internal(e)
            }
        })?;
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, source, entrypoint,
                    env, network_enabled, timeout_secs, created_at, updated_at
             FROM server_tools WHERE id = ?1",
            [&id],
            tool_json,
        )
        .map_err(internal)
    })
    .await;
    match result {
        Ok(Ok(v)) => (StatusCode::CREATED, Json(v)).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn get_server_tool(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, source, entrypoint,
                    env, network_enabled, timeout_secs, created_at, updated_at
             FROM server_tools WHERE id = ?1 AND org_id = ?2",
            params![id, org],
            tool_json,
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Server tool not found."))
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn update_server_tool(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateServerToolBody>,
) -> Response {
    if let Err(e) = validate_body(&body) {
        return e.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let now = chrono::Utc::now().to_rfc3339();
        let network = body.network_enabled.unwrap_or(false) as i64;
        let timeout = body.timeout_secs.unwrap_or(30);
        let changed = conn.execute(
            "UPDATE server_tools SET
                name = ?1, description = ?2, runtime = ?3, source = ?4, entrypoint = ?5,
                env = ?6, network_enabled = ?7, timeout_secs = ?8, updated_at = ?9
             WHERE id = ?10 AND org_id = ?11",
            params![
                &body.name,
                body.description.as_deref(),
                &body.runtime,
                &body.source,
                body.entrypoint.as_deref(),
                serde_json::to_string(&body.env).unwrap(),
                network,
                timeout,
                &now,
                &id,
                &org,
            ],
        )
        .map_err(internal)?;
        if changed == 0 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Server tool not found.",
            ));
        }
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, source, entrypoint,
                    env, network_enabled, timeout_secs, created_at, updated_at
             FROM server_tools WHERE id = ?1",
            [&id],
            tool_json,
        )
        .map_err(internal)
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn delete_server_tool(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let changed = conn
            .execute(
                "DELETE FROM server_tools WHERE id = ?1 AND org_id = ?2",
                params![id, org],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Server tool not found.",
            ));
        }
        Ok::<_, ApiError>(Json(json!({"deleted": true})))
    })
    .await;
    match result {
        Ok(Ok(v)) => v.into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

/// Lookup and execute a server tool for the given organization. Returns
/// `Ok(None)` when no server tool with this name exists, so the caller can
/// fall through to native tool handling.
pub async fn execute_server_tool(
    state: &AppState,
    tool_name: &str,
    args: &Value,
    org_id: &str,
) -> Result<Option<Value>, String> {
    let tool = {
        let db = state.db.clone();
        let tool_name = tool_name.to_string();
        let org_id = org_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.connect().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT runtime, source, env, timeout_secs
                 FROM server_tools WHERE org_id = ?1 AND name = ?2",
                params![org_id, tool_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?
    };

    let Ok(Some((runtime, source, env_json, timeout_secs))) = tool else {
        return Ok(None);
    };
    let timeout_secs: i64 = timeout_secs;

    let mut env: HashMap<String, String> =
        serde_json::from_str(&env_json).map_err(|e| e.to_string())?;
    env.insert(
        "ALLTERNIT_TOOL_ARGS".to_string(),
        serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string()),
    );

    let request = SandboxExecuteRequest {
        code: source,
        language: runtime,
        workdir: None,
        env,
        timeout_secs: timeout_secs.max(1) as u64,
        resources: None,
        toolchains: vec![],
        network_enabled: false,
        files: Vec::new(),
    };

    let result = execute_sandbox_or_subprocess(state, &request).await?;

    // Best-effort JSON parse; otherwise return the raw stdout structure.
    let output = if let Ok(parsed) = serde_json::from_str::<Value>(&result.stdout) {
        parsed
    } else {
        json!({
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exit_code": result.exit_code,
        })
    };

    Ok(Some(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use crate::db::DbHandle;

    fn test_db() -> DbHandle {
        let id = uuid::Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(format!("allternit-server-tool-test-{}.db", id));
        DbHandle::new(path).unwrap()
    }

    fn seed_org(conn: &rusqlite::Connection, org_id: &str, user_id: &str) {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS organizations (id TEXT PRIMARY KEY, name TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE IF NOT EXISTS organization_members (id TEXT PRIMARY KEY, organization_id TEXT, user_id TEXT, role TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test')",
            params![org_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params![user_id, format!("{}@test.local", user_id)],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, 'owner')",
            params![format!("{}:{}", org_id, user_id), org_id, user_id],
        )
        .unwrap();
    }

    fn seed_server_tools_table(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS server_tools (
                id TEXT PRIMARY KEY,
                org_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                runtime TEXT NOT NULL,
                source TEXT NOT NULL,
                entrypoint TEXT,
                env TEXT NOT NULL DEFAULT '{}',
                network_enabled INTEGER NOT NULL DEFAULT 0,
                timeout_secs INTEGER NOT NULL DEFAULT 30,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(org_id, name)
            );",
        )
        .unwrap();
    }

    fn owner_user(org_id: &str, user_id: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: Some(format!("{}@test.local", user_id)),
            name: Some("Test Owner".to_string()),
            avatar_url: None,
            tenant_id: Some(org_id.to_string()),
            organization_id: Some(org_id.to_string()),
            organization_role: Some("owner".to_string()),
            organization_slug: None,
        }
    }

    #[test]
    fn crud_lifecycle_and_isolation() {
        let db = test_db();
        let conn = db.connect().unwrap();
        seed_org(&conn, "org-1", "user-1");
        seed_org(&conn, "org-2", "user-2");
        seed_server_tools_table(&conn);

        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO server_tools (id, org_id, name, runtime, source, timeout_secs)
             VALUES (?1, 'org-1', 'hello', 'bash', 'echo hi', 30)",
            [&id],
        )
        .unwrap();

        let names: Vec<String> = conn
            .prepare("SELECT name FROM server_tools WHERE org_id = 'org-1' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(names, vec!["hello".to_string()]);

        let other: Vec<String> = conn
            .prepare("SELECT name FROM server_tools WHERE org_id = 'org-2' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn validate_name_rejects_invalid() {
        assert!(validate_name("valid.tool_1").is_ok());
        assert!(validate_name("UPPER").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
    }
}
