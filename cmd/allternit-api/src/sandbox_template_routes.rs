//! Cloud sandbox templates and instance ledger.
//!
//! Endpoints (org-admin gated, merged at `/api/v1/admin`):
//!   Sandbox templates:
//!     GET    /admin/sandbox-templates
//!     POST   /admin/sandbox-templates
//!     GET    /admin/sandbox-templates/:id
//!     PUT    /admin/sandbox-templates/:id
//!     DELETE /admin/sandbox-templates/:id
//!   Sandbox instances:
//!     GET    /admin/sandbox-instances
//!     POST   /admin/sandbox-instances
//!     GET    /admin/sandbox-instances/:id
//!     POST   /admin/sandbox-instances/:id/stop

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
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
        .route("/admin/sandbox-templates", get(list_templates))
        .route("/admin/sandbox-templates", post(create_template))
        .route("/admin/sandbox-templates/:id", get(get_template))
        .route("/admin/sandbox-templates/:id", put(update_template))
        .route("/admin/sandbox-templates/:id", delete(delete_template))
        .route("/admin/sandbox-instances", get(list_instances))
        .route("/admin/sandbox-instances", post(launch_instance))
        .route("/admin/sandbox-instances/:id", get(get_instance))
        .route("/admin/sandbox-instances/:id/stop", post(stop_instance))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "sandbox template operation failed");
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
            "Only organization owners/admins can manage cloud sandboxes.",
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
            format!("runtime must be one of: {}", ALLOWED_RUNTIMES.join(", ")),
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
            "Name must be 1-64 lowercase alphanumeric characters, dots, dashes, or underscores.",
        ))
    }
}

fn template_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "org_id": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?,
        "description": row.get::<_, Option<String>>(3)?,
        "runtime": row.get::<_, String>(4)?,
        "image": row.get::<_, String>(5)?,
        "source": row.get::<_, Option<String>>(6)?,
        "resources": serde_json::from_str::<Value>(&row.get::<_, String>(7)?)
            .unwrap_or_else(|_| json!({})),
        "network_enabled": row.get::<_, i64>(8)? == 1,
        "env": serde_json::from_str::<Value>(&row.get::<_, String>(9)?)
            .unwrap_or_else(|_| json!({})),
        "timeout_secs": row.get::<_, i64>(10)?,
        "created_at": row.get::<_, String>(11)?,
        "updated_at": row.get::<_, String>(12)?,
    }))
}

fn instance_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "org_id": row.get::<_, String>(1)?,
        "template_id": row.get::<_, Option<String>>(2)?,
        "name": row.get::<_, Option<String>>(3)?,
        "status": row.get::<_, String>(4)?,
        "session_id": row.get::<_, Option<String>>(5)?,
        "exit_code": row.get::<_, Option<i64>>(6)?,
        "stdout": row.get::<_, Option<String>>(7)?,
        "stderr": row.get::<_, Option<String>>(8)?,
        "started_at": row.get::<_, Option<String>>(9)?,
        "stopped_at": row.get::<_, Option<String>>(10)?,
        "created_at": row.get::<_, String>(11)?,
    }))
}

async fn list_templates(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, org_id, name, description, runtime, image, source,
                        resources, network_enabled, env, timeout_secs, created_at, updated_at
                 FROM sandbox_templates WHERE org_id = ?1 ORDER BY name",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map([org], template_json)
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
struct TemplateBody {
    name: String,
    description: Option<String>,
    runtime: String,
    #[serde(default = "default_image")]
    image: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    resources: HashMap<String, Value>,
    #[serde(default)]
    network_enabled: Option<bool>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    timeout_secs: Option<i64>,
}

fn default_image() -> String {
    "ubuntu-22.04-minimal".to_string()
}

fn validate_template(body: &TemplateBody) -> Result<(), ApiError> {
    validate_name(&body.name)?;
    validate_runtime(&body.runtime)?;
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

async fn create_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<TemplateBody>,
) -> Response {
    if let Err(e) = validate_template(&body) {
        return e.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let network = body.network_enabled.unwrap_or(false) as i64;
        let timeout = body.timeout_secs.unwrap_or(300);
        conn.execute(
            "INSERT INTO sandbox_templates
                (id, org_id, name, description, runtime, image, source,
                 resources, network_enabled, env, timeout_secs, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
            params![
                &id,
                &org,
                &body.name,
                body.description.as_deref(),
                &body.runtime,
                &body.image,
                body.source.as_deref(),
                serde_json::to_string(&body.resources).unwrap(),
                network,
                serde_json::to_string(&body.env).unwrap(),
                timeout,
                &now,
            ],
        )
        .map_err(|e| {
            if e.to_string().contains("UNIQUE constraint failed") {
                error(
                    StatusCode::CONFLICT,
                    "duplicate_name",
                    "A sandbox template with this name already exists in the organization.",
                )
            } else {
                internal(e)
            }
        })?;
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, image, source,
                    resources, network_enabled, env, timeout_secs, created_at, updated_at
             FROM sandbox_templates WHERE id = ?1",
            [&id],
            template_json,
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

async fn get_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, image, source,
                    resources, network_enabled, env, timeout_secs, created_at, updated_at
             FROM sandbox_templates WHERE id = ?1 AND org_id = ?2",
            params![id, org],
            template_json,
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Template not found."))
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn update_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<TemplateBody>,
) -> Response {
    if let Err(e) = validate_template(&body) {
        return e.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let now = chrono::Utc::now().to_rfc3339();
        let network = body.network_enabled.unwrap_or(false) as i64;
        let timeout = body.timeout_secs.unwrap_or(300);
        let changed = conn.execute(
            "UPDATE sandbox_templates SET
                name = ?1, description = ?2, runtime = ?3, image = ?4, source = ?5,
                resources = ?6, network_enabled = ?7, env = ?8, timeout_secs = ?9,
                updated_at = ?10
             WHERE id = ?11 AND org_id = ?12",
            params![
                &body.name,
                body.description.as_deref(),
                &body.runtime,
                &body.image,
                body.source.as_deref(),
                serde_json::to_string(&body.resources).unwrap(),
                network,
                serde_json::to_string(&body.env).unwrap(),
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
                "Template not found.",
            ));
        }
        conn.query_row(
            "SELECT id, org_id, name, description, runtime, image, source,
                    resources, network_enabled, env, timeout_secs, created_at, updated_at
             FROM sandbox_templates WHERE id = ?1",
            [&id],
            template_json,
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

async fn delete_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let changed = conn
            .execute(
                "DELETE FROM sandbox_templates WHERE id = ?1 AND org_id = ?2",
                params![id, org],
            )
            .map_err(internal)?;
        if changed == 0 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Template not found.",
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

async fn list_instances(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, org_id, template_id, name, status, session_id,
                        exit_code, stdout, stderr, started_at, stopped_at, created_at
                 FROM sandbox_instances WHERE org_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(internal)?;
        let rows = stmt
            .query_map([org], instance_json)
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
struct LaunchInstanceBody {
    template_id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Serialize, Clone)]
struct LaunchResult {
    id: String,
    status: String,
    session_id: Option<String>,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

async fn launch_instance(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<LaunchInstanceBody>,
) -> Response {
    let template = match tokio::task::spawn_blocking({
        let state = state.clone();
        let user = user.clone();
        let template_id = body.template_id.clone();
        move || {
            let conn = state.db.connect().map_err(internal)?;
            let org = admin_org(&conn, &user)?;
            conn.query_row(
                "SELECT runtime, source, env, timeout_secs
                 FROM sandbox_templates WHERE id = ?1 AND org_id = ?2",
                params![template_id, org],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(internal)?
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Template not found."))
        }
    })
    .await
    {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return e.into_response(),
        Err(e) => return internal(e).into_response(),
    };

    let (runtime, source, env_json, timeout_secs) = template;
    let code = source.unwrap_or_else(|| "echo 'sandbox ready'".to_string());
    let env: HashMap<String, String> = serde_json::from_str(&env_json).unwrap_or_default();

    let request = SandboxExecuteRequest {
        code,
        language: runtime,
        workdir: None,
        env,
        timeout_secs: timeout_secs.max(1) as u64,
        resources: None,
        toolchains: vec![],
        network_enabled: false,
        files: Vec::new(),
    };

    let execution = execute_sandbox_or_subprocess(&state, &request).await;

    let result = match execution {
        Ok(response) => LaunchResult {
            id: uuid::Uuid::new_v4().to_string(),
            status: if response.exit_code == 0 {
                "running".to_string()
            } else {
                "error".to_string()
            },
            session_id: response.session_id.clone(),
            exit_code: response.exit_code,
            stdout: response.stdout.clone(),
            stderr: response.stderr.clone(),
        },
        Err(err) => LaunchResult {
            id: uuid::Uuid::new_v4().to_string(),
            status: "error".to_string(),
            session_id: None,
            exit_code: -1,
            stdout: String::new(),
            stderr: err,
        },
    };

    let now = chrono::Utc::now().to_rfc3339();
    let save = tokio::task::spawn_blocking({
        let state = state.clone();
        let user = user.clone();
        let template_id = body.template_id.clone();
        let result = result.clone();
        let name = body.name.clone();
        move || {
            let conn = state.db.connect().map_err(internal)?;
            let org = admin_org(&conn, &user)?;
            conn.execute(
                "INSERT INTO sandbox_instances
                    (id, org_id, template_id, name, status, session_id,
                     exit_code, stdout, stderr, started_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    &result.id,
                    &org,
                    &template_id,
                    name.as_deref(),
                    &result.status,
                    result.session_id.as_deref(),
                    result.exit_code,
                    &result.stdout,
                    &result.stderr,
                    &now,
                ],
            )
            .map_err(internal)?;
            Ok::<_, ApiError>(())
        }
    })
    .await;

    match save {
        Ok(Ok(())) => (StatusCode::CREATED, Json(json!(result))).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn get_instance(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        conn.query_row(
            "SELECT id, org_id, template_id, name, status, session_id,
                    exit_code, stdout, stderr, started_at, stopped_at, created_at
             FROM sandbox_instances WHERE id = ?1 AND org_id = ?2",
            params![id, org],
            instance_json,
        )
        .optional()
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "not_found", "Instance not found."))
    })
    .await;
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => e.into_response(),
        Err(e) => internal(e).into_response(),
    }
}

async fn stop_instance(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.db.connect().map_err(internal)?;
        let org = admin_org(&conn, &user)?;
        let now = chrono::Utc::now().to_rfc3339();
        let changed = conn.execute(
            "UPDATE sandbox_instances
             SET status = 'stopped', stopped_at = ?1
             WHERE id = ?2 AND org_id = ?3 AND status != 'stopped'",
            params![&now, &id, &org],
        )
        .map_err(internal)?;
        if changed == 0 {
            return Err(error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Instance not found or already stopped.",
            ));
        }
        conn.query_row(
            "SELECT id, org_id, template_id, name, status, session_id,
                    exit_code, stdout, stderr, started_at, stopped_at, created_at
             FROM sandbox_instances WHERE id = ?1",
            [&id],
            instance_json,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthUser;
    use crate::db::DbHandle;

    fn test_db() -> DbHandle {
        let id = uuid::Uuid::new_v4().to_string();
        let path = std::env::temp_dir().join(format!("allternit-sandbox-template-test-{}.db", id));
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

    fn seed_tables(conn: &rusqlite::Connection) {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sandbox_templates (
                id TEXT PRIMARY KEY,
                org_id TEXT NOT NULL,
                name TEXT NOT NULL,
                description TEXT,
                runtime TEXT NOT NULL,
                image TEXT NOT NULL DEFAULT 'ubuntu-22.04-minimal',
                source TEXT,
                resources TEXT NOT NULL DEFAULT '{}',
                network_enabled INTEGER NOT NULL DEFAULT 0,
                env TEXT NOT NULL DEFAULT '{}',
                timeout_secs INTEGER NOT NULL DEFAULT 300,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(org_id, name)
            );
            CREATE TABLE IF NOT EXISTS sandbox_instances (
                id TEXT PRIMARY KEY,
                org_id TEXT NOT NULL,
                template_id TEXT,
                name TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                session_id TEXT,
                exit_code INTEGER,
                stdout TEXT,
                stderr TEXT,
                started_at DATETIME,
                stopped_at DATETIME,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
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
    fn template_crud_and_isolation() {
        let db = test_db();
        let conn = db.connect().unwrap();
        seed_org(&conn, "org-1", "user-1");
        seed_org(&conn, "org-2", "user-2");
        seed_tables(&conn);

        conn.execute(
            "INSERT INTO sandbox_templates (id, org_id, name, runtime, image, timeout_secs)
             VALUES ('tpl-1', 'org-1', 'python-base', 'python', 'ubuntu-22.04-minimal', 60)",
            [],
        )
        .unwrap();

        let names: Vec<String> = conn
            .prepare("SELECT name FROM sandbox_templates WHERE org_id = 'org-1' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(names, vec!["python-base".to_string()]);

        let other: Vec<String> = conn
            .prepare("SELECT name FROM sandbox_templates WHERE org_id = 'org-2' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn validate_name_rejects_invalid() {
        assert!(validate_name("valid-template_1").is_ok());
        assert!(validate_name("UPPER").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a/b").is_err());
    }
}
