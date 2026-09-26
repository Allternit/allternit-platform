//! Memory Reconstruction Jobs API (`/beta/memory-reconstruction`).
//!
//! Long-running (or synchronous) jobs that reconstruct or enrich an agent's
//! memory from session events, memory stores, or uploaded source material.
//! This Phase 1 implementation provides durable job CRUD and a synchronous
//! scaffold runner; async worker execution is Phase 2.

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn memory_reconstruction_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/beta/memory-reconstruction", post(create_job).get(list_jobs))
        .route(
            "/beta/memory-reconstruction/:id",
            get(get_job).delete(delete_job),
        )
        .route("/beta/memory-reconstruction/:id/run", post(run_job))
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "memory reconstruction operation failed");
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
            "Only organization owners/admins can manage memory reconstruction jobs.",
        ));
    }
    Ok(org.to_string())
}

fn validate_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "Name must be 1-128 characters.",
        ));
    }
    Ok(())
}

fn valid_source_type(source_type: &str) -> bool {
    matches!(source_type, "session" | "memory_store" | "file")
}

fn job_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let config: Value = serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or(json!({}));
    let result: Value = serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or(json!({}));
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "organization_id": row.get::<_, String>(1)?,
        "user_id": row.get::<_, String>(2)?,
        "name": row.get::<_, String>(3)?,
        "description": row.get::<_, Option<String>>(4)?,
        "source_type": row.get::<_, String>(5)?,
        "source_id": row.get::<_, String>(6)?,
        "config": config,
        "status": row.get::<_, String>(8)?,
        "result": result,
        "created_by": row.get::<_, String>(10)?,
        "created_at": row.get::<_, String>(11)?,
        "updated_at": row.get::<_, String>(12)?,
    }))
}

fn find_job(conn: &rusqlite::Connection, id: &str, org: &str) -> Result<Value, ApiError> {
    conn.query_row(
        "SELECT id, organization_id, user_id, name, description, source_type, source_id,
                config, status, result, created_by, created_at, updated_at
         FROM memory_reconstruction_jobs
         WHERE id = ?1 AND organization_id = ?2",
        params![id, org],
        job_json,
    )
    .optional()
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "job_not_found", "No such reconstruction job."))
}

#[derive(Deserialize)]
struct CreateJob {
    name: String,
    description: Option<String>,
    source_type: String,
    source_id: String,
    #[serde(default)]
    config: Value,
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateJob>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    validate_name(&body.name)?;
    if !valid_source_type(&body.source_type) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_source_type",
            "source_type must be one of: session, memory_store, file.",
        ));
    }
    if !body.config.is_object() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_config",
            "config must be an object.",
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let config_json = serde_json::to_string(&body.config).map_err(internal)?;
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO memory_reconstruction_jobs
         (id, organization_id, user_id, name, description, source_type, source_id, config,
          status, result, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', '{}', ?9, ?10, ?11)",
        params![
            id,
            org,
            user.user_id,
            body.name.trim(),
            body.description.as_deref().map(str::trim),
            body.source_type,
            body.source_id,
            config_json,
            user.user_id,
            now,
            now,
        ],
    )
    .map_err(internal)?;

    let job = find_job(&conn, &id, &org)?;
    Ok::<Response, ApiError>((StatusCode::CREATED, Json(job)).into_response())
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let mut stmt = conn
        .prepare(
            "SELECT id, organization_id, user_id, name, description, source_type, source_id,
                    config, status, result, created_by, created_at, updated_at
             FROM memory_reconstruction_jobs
             WHERE organization_id = ?1
             ORDER BY updated_at DESC",
        )
        .map_err(internal)?;
    let rows: Vec<Value> = stmt
        .query_map(params![org], job_json)
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": rows })))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let job = find_job(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(job))
}

async fn delete_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let rows = conn
        .execute(
            "DELETE FROM memory_reconstruction_jobs WHERE id = ?1 AND organization_id = ?2",
            params![id, org],
        )
        .map_err(internal)?;
    if rows == 0 {
        return Err(error(StatusCode::NOT_FOUND, "job_not_found", "No such reconstruction job."));
    }

    Ok::<Json<Value>, ApiError>(Json(json!({ "deleted": true })))
}

/// Phase 1 synchronous scaffold runner. In Phase 2 this will enqueue an async
/// worker that streams progress and writes reconstructed memory chunks.
async fn run_job(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;
    let job = find_job(&conn, &id, &org)?;

    if job["status"].as_str() == Some("running") {
        return Err(error(
            StatusCode::CONFLICT,
            "job_already_running",
            "This reconstruction job is already running.",
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE memory_reconstruction_jobs SET status = 'running', updated_at = ?1
         WHERE id = ?2 AND organization_id = ?3",
        params![now, id, org],
    )
    .map_err(internal)?;

    // Dreaming-style extraction: read session events and browser history, then
    // produce memory facts and procedural memory.
    let result = match run_dream_extraction(&state.db, &user.user_id, &job) {
        Ok(extraction) => {
            let result = json!({
                "memory_outline": extraction.outline,
                "facts_recorded": extraction.fact_count,
                "procedural_memory_id": extraction.procedural_memory_id,
                "chunks_generated": extraction.outline.len(),
                "note": "Synchronous dream extraction completed.",
            });
            let result_json = serde_json::to_string(&result).map_err(internal)?;
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE memory_reconstruction_jobs SET status = 'completed', result = ?1, updated_at = ?2
                 WHERE id = ?3 AND organization_id = ?4",
                params![result_json, now, id, org],
            )
            .map_err(internal)?;
            result
        }
        Err(err) => {
            let result = json!({ "error": err.to_string() });
            let result_json = serde_json::to_string(&result).map_err(internal)?;
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE memory_reconstruction_jobs SET status = 'failed', result = ?1, updated_at = ?2
                 WHERE id = ?3 AND organization_id = ?4",
                params![result_json, now, id, org],
            )
            .map_err(internal)?;
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "reconstruction_failed",
                err.to_string(),
            ));
        }
    };

    let job = find_job(&conn, &id, &org)?;
    Ok::<Json<Value>, ApiError>(Json(json!({ "job": job, "result": result })))
}

struct DreamExtractionResult {
    outline: Vec<Value>,
    fact_count: usize,
    procedural_memory_id: Option<String>,
}

fn run_dream_extraction(
    db: &crate::db::DbHandle,
    user_id: &str,
    job: &Value,
) -> Result<DreamExtractionResult, Box<dyn std::error::Error>> {
    let source_type = job["source_type"].as_str().unwrap_or("unknown");
    let source_id = job["source_id"].as_str().unwrap_or("unknown");
    let config = &job["config"];

    let mut outline = Vec::new();
    let mut fact_count = 0;
    let mut procedural_memory_id = None;

    outline.push(json!({
        "type": "summary",
        "content": format!("Dream extraction from {source_type} {source_id}"),
    }));

    if source_type == "session" {
        // Extract facts from session events.
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT event_type, data FROM beta_session_events WHERE session_id = ?1 ORDER BY sequence"
        )?;
        let rows = stmt.query_map(params![source_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut event_texts = Vec::new();
        for row in rows {
            let (event_type, data) = row?;
            event_texts.push(format!("{event_type}: {data}"));
        }

        let combined = event_texts.join("\n");
        let facts = crate::memory_kernel_service::extract_facts_heuristic(&combined);
        if !facts.is_empty() {
            let observation_id = crate::memory_kernel_service::record_observation(
                db,
                user_id,
                None,
                Some(source_id),
                "dream_extraction",
                &format!("Dream extraction from session {source_id}"),
                Some("memory_reconstruction"),
            )?;
            let persisted = crate::memory_kernel_service::persist_facts(
                db,
                user_id,
                None,
                &observation_id,
                &facts,
            )?;
            fact_count = persisted.len();
            for fact in &facts {
                outline.push(json!({"type": "fact", "content": fact}));
            }
        }

        // Extract procedural memory from session browser history.
        if let Ok(Some(memory)) = crate::procedural_memory_service::extract_from_session_history(
            db,
            user_id,
            None,
            source_id,
        ) {
            procedural_memory_id = Some(memory.id.clone());
            outline.push(json!({
                "type": "procedural_memory",
                "content": json!({
                    "id": memory.id,
                    "name": memory.name,
                    "patterns": memory.trigger_patterns,
                }),
            }));
        }
    }

    if let Some(topics) = config["topics"].as_array() {
        for topic in topics {
            outline.push(json!({
                "type": "topic",
                "content": topic.as_str().unwrap_or(""),
            }));
        }
    }

    outline.push(json!({
        "type": "metadata",
        "content": json!({
            "reconstructed_at": chrono::Utc::now().to_rfc3339(),
            "source_type": source_type,
            "source_id": source_id,
        }),
    }));

    Ok(DreamExtractionResult {
        outline,
        fact_count,
        procedural_memory_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::Path;
    use tower::ServiceExt;

    fn test_user(user_id: &str, org_id: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: org_id.map(|s| s.to_string()),
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: Some("org:admin".to_string()),
            organization_slug: None,
        }
    }

    async fn test_app_state(temp: &Path) -> Arc<AppState> {
        let config = crate::AppConfig {
            company: Default::default(),
            user: Default::default(),
        };
        let db = crate::db::DbHandle::new(temp.join("test.db")).expect("test db");
        let conn = db.connect().expect("test db conn");
        conn.execute(
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Test Org')",
            params!["org-1"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
            params!["admin-1", "admin-1@test.local"],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
            params!["org-1:admin-1", "org-1", "admin-1", "owner"],
        )
        .unwrap();
        drop(conn);
        let auth_config = crate::auth::AuthConfig::from_app_config(&config);
        let jwks = crate::auth::JwksManager::new(&auth_config);
        let rails = crate::rails::RailsState::new(temp.join("rails"))
            .await
            .expect("test rails");
        let desktop_host_registry = crate::desktop_host_registry::DesktopHostRegistry::new(db.clone());
        Arc::new(AppState {
            config,
            db: db.clone(),
            data_dir: temp.to_path_buf(),
            jwks,
            auth_config,
            vm_driver: None,
            incus_driver: None,
            desktop_host_registry,
            desktop_host_provisioner: None,
            bot_desktop_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            computer_guest_tokens: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            rails,
            vm_sessions: crate::vm_session_routes::new_vm_session_store(),
            cowork_scheduler: None,
            cowork_background: None,
            cowork_run_manager: None,
            webhook_secret: None,
            office_runtime: Arc::new(tokio::sync::RwLock::new(
                crate::office_routes::OfficeRuntimeFile::default(),
            )),
            office_cli_docs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_watches: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            office_cli_mcp_sessions: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            design_skill_cache: crate::design_connector_routes::DesignSkillCache::new(),
            #[cfg(unix)]
            terminal_sessions: crate::terminal_routes::TerminalSessionStore::new(),
            mcp_dispatcher: crate::mcp_dispatcher::McpDispatcher::new(),
            approval_store: Arc::new(crate::permission_policy::ApprovalStore::new()),
            passkey_state: None,
            resource_class_catalog: crate::fabric::sku::ResourceClassCatalog::builtin(),
            fabric_node_provider: allternit_computer_cloud::providers::fabric_node::FabricNodeProvider::new(
                std::sync::Arc::new(allternit_computer_cloud::providers::fabric_node::FabricNodePool::new()),
                "__test__".to_string(),
            ),
            fabric_provider_registry: allternit_computer_cloud::fabric::FabricProviderRegistry::empty(),
            fabric_scheduler: crate::fabric::Scheduler::new(crate::fabric::CostEngine::default_engine()),
            fabric_price_cache: crate::fabric::PriceCache::new(db.clone()),
            os_control_plane: None,
            dp_jwks: crate::auth_dp_jwt::DataPlaneJwks::disabled(),
            deployment_scheduler: Arc::new(
                crate::deployment_scheduler::DeploymentSchedulerState::new(),
            ),
        })
    }

    async fn body_json(body: Body) -> Value {
        let bytes = body.collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_body(value: &Value) -> Body {
        Body::from(value.to_string())
    }

    #[tokio::test]
    async fn memory_reconstruction_job_lifecycle() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = memory_reconstruction_router().with_state(state);

        let job = json!({
            "name": "Reconstruct session memory",
            "description": "Summarize last session",
            "source_type": "session",
            "source_id": "session-1",
            "config": { "topics": ["goals", "decisions"] }
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/beta/memory-reconstruction")
                    .header("content-type", "application/json")
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(json_body(&job))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = body_json(resp.into_body()).await;
        let job_id = body["id"].as_str().unwrap().to_string();
        assert_eq!(body["status"], "pending");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&format!("/beta/memory-reconstruction/{}/run", job_id))
                    .extension(test_user("admin-1", Some("org-1")))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["job"]["status"], "completed");
        assert!(body["result"]["memory_outline"].as_array().unwrap().len() > 0);
    }
}
