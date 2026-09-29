//! Artifact API routes — local SQLite persistence.
//!
//! Mirrors the Next.js `/api/v1/artifacts` layer.

use axum::extract::Extension;
use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, patch},
    Router,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::auth::AuthUser;
use crate::AppState;

pub fn artifact_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/artifacts", get(list_artifacts).post(create_artifact))
        .route("/artifacts/search", get(search_artifacts))
        .route("/artifacts/stats", get(get_artifact_stats))
        .route(
            "/artifacts/:id",
            get(get_artifact)
                .patch(update_artifact)
                .delete(delete_artifact),
        )
        .route("/artifacts/:id/revisions", get(list_revisions))
        .route("/artifacts/:id/sharing", patch(update_sharing))
        .route(
            "/artifacts/:id/sections",
            get(list_sections).post(add_section),
        )
        .route(
            "/artifacts/:id/sections/:section_id",
            patch(update_section).delete(delete_section),
        )
}

// ═══════════════════════════════════════════════════════════════════════════════
// Data models
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ArtifactRow {
    id: String,
    user_id: String,
    workspace_id: String,
    title: String,
    #[serde(rename = "type")]
    artifact_type: String,
    status: String,
    summary: Option<String>,
    tags: Vec<String>,
    created_at: String,
    updated_at: String,
    /// `private` (owner only) or `org` (the owner's organization).
    visibility: String,
    /// What org members may do when shared: `view` or `edit`.
    org_access: String,
    /// Whether the caller owns it (only the owner deletes or changes sharing).
    is_owner: bool,
    /// Whether the caller may change it.
    can_edit: bool,
    /// The owner's display name, for "Shared with you".
    owner_name: Option<String>,
    sections: Vec<SectionRow>,
    revisions: Vec<RevisionRow>,
}

#[derive(Serialize, Clone)]
struct SectionRow {
    id: String,
    artifact_id: String,
    heading: String,
    kind: String,
    body: String,
    position: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct RevisionRow {
    id: String,
    artifact_id: String,
    reason: String,
    snapshot: serde_json::Value,
    created_at: String,
}

#[derive(Deserialize)]
struct ListQuery {
    // The web surface's artifacts client sends camelCase query params.
    #[serde(alias = "workspaceId")]
    workspace_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    _q: Option<String>,
    /// `mine` (default): your own. `shared`: shared with you by others in your org.
    scope: Option<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    #[serde(alias = "workspaceId")]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct CreateBody {
    workspace_id: String,
    title: String,
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    status: Option<String>,
    summary: Option<String>,
    tags: Option<serde_json::Value>,
    sections: Option<Vec<CreateSectionBody>>,
}

#[derive(Deserialize)]
struct CreateSectionBody {
    heading: Option<String>,
    kind: Option<String>,
    body: Option<String>,
    position: Option<i64>,
}

#[derive(Deserialize)]
struct UpdateBody {
    title: Option<String>,
    #[serde(rename = "type")]
    artifact_type: Option<String>,
    status: Option<String>,
    summary: Option<String>,
    tags: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct SharingBody {
    visibility: String,
    access: Option<String>,
}

#[derive(Deserialize)]
struct SectionBody {
    heading: Option<String>,
    kind: Option<String>,
    body: Option<String>,
    position: Option<i64>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn normalize_tags(input: Option<serde_json::Value>) -> Vec<String> {
    input
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.into_iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Who is asking: their user id and organization (sharing is org-scoped).
#[derive(Clone)]
struct Viewer {
    user_id: String,
    org_id: Option<String>,
}

impl Viewer {
    fn of(user: &AuthUser) -> Self {
        Viewer {
            user_id: user.user_id.clone(),
            org_id: crate::computer_routes::resolve_org_id(user),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Access {
    owner: bool,
    can_edit: bool,
}

/// The owner has full access; members of the org an artifact is shared with can
/// read it, and change it when it's shared for editing. Everyone else: none.
fn artifact_access(
    conn: &rusqlite::Connection,
    artifact_id: &str,
    viewer: &Viewer,
) -> Result<Option<Access>, rusqlite::Error> {
    let row: Option<(String, String, String, Option<String>)> = conn
        .query_row(
            "SELECT user_id, visibility, org_access, org_id FROM artifacts WHERE id = ?1",
            params![artifact_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((owner, visibility, org_access, org_id)) = row else {
        return Ok(None);
    };
    if owner == viewer.user_id {
        return Ok(Some(Access { owner: true, can_edit: true }));
    }
    let shared_with_viewer = visibility == "org"
        && org_id.is_some()
        && viewer.org_id.is_some()
        && org_id == viewer.org_id;
    Ok(shared_with_viewer.then_some(Access { owner: false, can_edit: org_access == "edit" }))
}

fn can_read(conn: &rusqlite::Connection, id: &str, viewer: &Viewer) -> bool {
    matches!(artifact_access(conn, id, viewer), Ok(Some(_)))
}

fn can_edit(conn: &rusqlite::Connection, id: &str, viewer: &Viewer) -> bool {
    matches!(artifact_access(conn, id, viewer), Ok(Some(Access { can_edit: true, .. })))
}

fn fetch_artifact_with_related(
    conn: &rusqlite::Connection,
    artifact_id: &str,
    viewer: &Viewer,
) -> Result<Option<ArtifactRow>, rusqlite::Error> {
    let Some(access) = artifact_access(conn, artifact_id, viewer)? else {
        return Ok(None);
    };
    let artifact: Option<(String, String, String, String, String, String, Option<String>, Option<String>, String, String)> = conn.query_row(
        "SELECT id, user_id, workspace_id, title, type, status, summary, tags, created_at, updated_at
         FROM artifacts WHERE id = ?1",
        params![artifact_id],
        |row| Ok((
            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
            row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
        )),
    ).ok();

    let artifact = match artifact {
        Some(a) => a,
        None => return Ok(None),
    };
    let (visibility, org_access) = sharing_of(conn, artifact_id)?;
    let owner_name = owner_name_of(conn, &artifact.1);

    let mut stmt = conn.prepare(
        "SELECT id, artifact_id, heading, kind, body, position, created_at, updated_at
         FROM artifact_sections WHERE artifact_id = ?1 ORDER BY position ASC, created_at ASC",
    )?;
    let sections: Vec<SectionRow> = stmt
        .query_map(params![artifact_id], |row| {
            Ok(SectionRow {
                id: row.get(0)?,
                artifact_id: row.get(1)?,
                heading: row.get(2)?,
                kind: row.get(3)?,
                body: row.get(4)?,
                position: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut stmt = conn.prepare(
        "SELECT id, artifact_id, reason, snapshot, created_at
         FROM artifact_revisions WHERE artifact_id = ?1 ORDER BY created_at DESC",
    )?;
    let revisions: Vec<RevisionRow> = stmt
        .query_map(params![artifact_id], |row| {
            let snapshot_str: String = row.get(3)?;
            let snapshot = serde_json::from_str(&snapshot_str).unwrap_or(json!({}));
            Ok(RevisionRow {
                id: row.get(0)?,
                artifact_id: row.get(1)?,
                reason: row.get(2)?,
                snapshot,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(ArtifactRow {
        id: artifact.0,
        user_id: artifact.1,
        workspace_id: artifact.2,
        title: artifact.3,
        artifact_type: artifact.4,
        status: artifact.5,
        summary: artifact.6,
        tags: artifact
            .7
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default(),
        created_at: artifact.8,
        updated_at: artifact.9,
        visibility,
        org_access,
        is_owner: access.owner,
        can_edit: access.can_edit,
        owner_name,
        sections,
        revisions,
    }))
}

fn sharing_of(conn: &rusqlite::Connection, artifact_id: &str) -> Result<(String, String), rusqlite::Error> {
    conn.query_row(
        "SELECT visibility, org_access FROM artifacts WHERE id = ?1",
        params![artifact_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

fn owner_name_of(conn: &rusqlite::Connection, user_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT COALESCE(NULLIF(name, ''), email) FROM users WHERE id = ?1",
        params![user_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

fn serialize_snapshot(artifact: &ArtifactRow) -> String {
    json!({
        "title": artifact.title,
        "type": artifact.artifact_type,
        "status": artifact.status,
        "summary": artifact.summary,
        "tags": artifact.tags,
        "sections": artifact.sections.iter().map(|s| json!({
            "id": s.id,
            "artifactId": s.artifact_id,
            "heading": s.heading,
            "kind": s.kind,
            "body": s.body,
            "position": s.position,
            "createdAt": s.created_at,
            "updatedAt": s.updated_at,
        })).collect::<Vec<_>>(),
        "updatedAt": artifact.updated_at,
    })
    .to_string()
}

fn create_revision(
    tx: &Transaction,
    artifact_id: &str,
    reason: &str,
    artifact: &ArtifactRow,
) -> Result<(), rusqlite::Error> {
    let rev_id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO artifact_revisions (id, artifact_id, reason, snapshot, created_at)
         VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
        params![rev_id, artifact_id, reason, serialize_snapshot(artifact)],
    )?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_artifacts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);
    let shared = q.scope.as_deref() == Some("shared");

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let mut sql = String::from(
            "SELECT id, user_id, workspace_id, title, type, status, summary, tags, created_at, updated_at
             FROM artifacts WHERE ",
        );
        let mut params_vec: Vec<String> = Vec::new();
        if shared {
            // Shared with you: others' artifacts shared with your organization.
            let Some(org) = viewer.org_id.clone() else {
                return Ok::<_, rusqlite::Error>(Vec::new());
            };
            sql.push_str("visibility = 'org' AND org_id = ? AND user_id != ?");
            params_vec.push(org);
            params_vec.push(viewer.user_id.clone());
        } else {
            sql.push_str("user_id = ?");
            params_vec.push(viewer.user_id.clone());
        }

        if let Some(ws) = &q.workspace_id {
            sql.push_str(" AND workspace_id = ?");
            params_vec.push(ws.clone());
        }
        if let Some(st) = &q.status {
            sql.push_str(" AND status = ?");
            params_vec.push(st.clone());
        }
        if let Some(tp) = &q.artifact_type {
            sql.push_str(" AND type = ?");
            params_vec.push(tp.clone());
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let params_ref: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<(String, String, String, String, String, String, Option<String>, Option<String>, String, String)> = stmt.query_map(rusqlite::params_from_iter(params_ref), |row| {
            Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
            ))
        })?.collect::<Result<Vec<_>, _>>()?;

        let mut artifacts = Vec::new();
        for row in rows {
            let artifact_id = &row.0;
            let mut stmt = conn.prepare(
                "SELECT id, artifact_id, heading, kind, body, position, created_at, updated_at
                 FROM artifact_sections WHERE artifact_id = ?1 ORDER BY position ASC, created_at ASC"
            )?;
            let sections: Vec<SectionRow> = stmt.query_map(params![artifact_id], |row| {
                Ok(SectionRow {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    heading: row.get(2)?,
                    kind: row.get(3)?,
                    body: row.get(4)?,
                    position: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?;

            let mut stmt = conn.prepare(
                "SELECT id, artifact_id, reason, snapshot, created_at
                 FROM artifact_revisions WHERE artifact_id = ?1 ORDER BY created_at DESC"
            )?;
            let revisions: Vec<RevisionRow> = stmt.query_map(params![artifact_id], |row| {
                let snapshot_str: String = row.get(3)?;
                let snapshot = serde_json::from_str(&snapshot_str).unwrap_or(json!({}));
                Ok(RevisionRow {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    reason: row.get(2)?,
                    snapshot,
                    created_at: row.get(4)?,
                })
            })?.collect::<Result<Vec<_>, _>>()?;

            let access = artifact_access(&conn, &row.0, &viewer)?.unwrap_or(Access { owner: false, can_edit: false });
            let (visibility, org_access) = sharing_of(&conn, &row.0)?;
            let owner_name = if access.owner { None } else { owner_name_of(&conn, &row.1) };
            artifacts.push(ArtifactRow {
                id: row.0,
                user_id: row.1,
                workspace_id: row.2,
                title: row.3,
                artifact_type: row.4,
                status: row.5,
                summary: row.6,
                tags: row.7.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
                created_at: row.8,
                updated_at: row.9,
                visibility,
                org_access,
                is_owner: access.owner,
                can_edit: access.can_edit,
                owner_name,
                sections,
                revisions,
            });
        }

        Ok::<_, rusqlite::Error>(artifacts)
    }).await;

    match result {
        Ok(Ok(artifacts)) => Json(json!({"artifacts": artifacts})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing artifacts: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POST /artifacts
// ═══════════════════════════════════════════════════════════════════════════════

async fn create_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> impl IntoResponse {
    let workspace_id = body.workspace_id.trim().to_string();
    let title = body.title.trim().to_string();
    if workspace_id.is_empty() || title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "workspace_id and title are required"})),
        )
            .into_response();
    }

    let db = state.db.clone();
    let viewer = Viewer::of(&user);
    let artifact_type = body.artifact_type.unwrap_or_else(|| "document".to_string());
    let status = body.status.unwrap_or_else(|| "draft".to_string());
    let summary = body
        .summary
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let tags = normalize_tags(body.tags);
    let _tags_json = serde_json::to_string(&tags).unwrap_or_default();
    let sections_input = body.sections.unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        let artifact_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        tx.execute(
            "INSERT INTO artifacts (id, user_id, workspace_id, title, type, status, summary, tags, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![&artifact_id, &viewer.user_id, workspace_id, title, artifact_type, status, summary, serde_json::to_string(&tags).unwrap_or_default(), &now],
        )?;

        for (index, section) in sections_input.iter().enumerate() {
            let section_id = uuid::Uuid::new_v4().to_string();
            let heading = section.heading.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Section {}", index + 1));
            let kind = section.kind.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "document/markdown".to_string());
            let body_text = section.body.as_ref().map(|s| s.to_string()).unwrap_or_default();
            let position = section.position.unwrap_or(index as i64);

            tx.execute(
                "INSERT INTO artifact_sections (id, artifact_id, heading, kind, body, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![&section_id, &artifact_id, &heading, &kind, &body_text, position, &now],
            )?;
        }

        // Create initial revision
        let artifact = fetch_artifact_with_related(&tx, &artifact_id, &viewer)?.unwrap();
        create_revision(&tx, &artifact_id, "created", &artifact)?;

        tx.commit()?;
        Ok::<_, rusqlite::Error>(artifact)
    }).await;

    match result {
        Ok(Ok(artifact)) => {
            (StatusCode::CREATED, Json(json!({"artifact": artifact}))).into_response()
        }
        Ok(Err(e)) => {
            warn!("DB error creating artifact: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts/:id
// ═══════════════════════════════════════════════════════════════════════════════

async fn get_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        fetch_artifact_with_related(&conn, &id, &viewer)
    })
    .await;

    match result {
        Ok(Ok(Some(artifact))) => Json(json!({"artifact": artifact})).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error getting artifact: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PATCH /artifacts/:id
// ═══════════════════════════════════════════════════════════════════════════════

async fn update_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        // Verify ownership
        let exists: bool = can_edit(&tx, &id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(None);
        }

        let mut updates = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(title) = body.title {
            let t = title.trim();
            if !t.is_empty() {
                updates.push("title = ?".to_string());
                params_vec.push(Box::new(t.to_string()));
            }
        }
        if let Some(tp) = body.artifact_type {
            updates.push("type = ?".to_string());
            params_vec.push(Box::new(tp));
        }
        if let Some(st) = body.status {
            updates.push("status = ?".to_string());
            params_vec.push(Box::new(st));
        }
        if body.summary.is_some() {
            updates.push("summary = ?".to_string());
            params_vec.push(Box::new(
                body.summary
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            ));
        }
        if body.tags.is_some() {
            updates.push("tags = ?".to_string());
            params_vec.push(Box::new(
                serde_json::to_string(&normalize_tags(body.tags)).unwrap_or_default(),
            ));
        }

        if !updates.is_empty() {
            updates.push("updated_at = CURRENT_TIMESTAMP".to_string());
            let sql = format!("UPDATE artifacts SET {} WHERE id = ?", updates.join(", "));
            params_vec.push(Box::new(id.clone()));
            let params_ref: Vec<&dyn rusqlite::ToSql> =
                params_vec.iter().map(|p| p.as_ref()).collect();
            tx.execute(&sql, rusqlite::params_from_iter(params_ref))?;
        }

        let artifact = fetch_artifact_with_related(&tx, &id, &viewer)?.unwrap();
        create_revision(&tx, &id, "updated", &artifact)?;

        tx.commit()?;
        Ok(Some(artifact))
    })
    .await;

    match result {
        Ok(Ok(Some(artifact))) => Json(json!({"artifact": artifact})).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error updating artifact: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DELETE /artifacts/:id
// ═══════════════════════════════════════════════════════════════════════════════

async fn delete_artifact(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        let rows = conn.execute(
            "DELETE FROM artifacts WHERE id = ?1 AND user_id = ?2",
            params![&id, &user_id],
        )?;
        Ok::<_, rusqlite::Error>(rows)
    })
    .await;

    match result {
        Ok(Ok(0)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Ok(_)) => Json(json!({"ok": true})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting artifact: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts/search
// ═══════════════════════════════════════════════════════════════════════════════

async fn search_artifacts(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let query = q.q.unwrap_or_default().trim().to_lowercase();
    if query.is_empty() {
        return Json(json!({"artifacts": []})).into_response();
    }

    let db = state.db.clone();
    let user_id = user.user_id;
    let workspace_id = q.workspace_id;
    let like = format!("%{}%", query);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Search artifacts + sections via JOIN, then dedupe
        let mut sql = String::from(
            "SELECT DISTINCT a.id, a.user_id, a.workspace_id, a.title, a.type, a.status, a.summary, a.tags, a.created_at, a.updated_at
             FROM artifacts a
             LEFT JOIN artifact_sections s ON a.id = s.artifact_id
             WHERE a.user_id = ?1 AND (
                LOWER(a.title) LIKE ?2 OR
                LOWER(COALESCE(a.summary, '')) LIKE ?2 OR
                LOWER(COALESCE(a.tags, '')) LIKE ?2 OR
                LOWER(s.heading) LIKE ?2 OR
                LOWER(s.body) LIKE ?2
             )"
        );
        let mut params_vec: Vec<String> = vec![user_id, like.clone()];
        if let Some(ws) = &workspace_id {
            sql.push_str(" AND a.workspace_id = ?");
            params_vec.push(ws.clone());
        }
        sql.push_str(" ORDER BY a.updated_at DESC");

        let params_ref: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows: Vec<(String, String, String, String, String, String, Option<String>, Option<String>, String, String)> = stmt.query_map(rusqlite::params_from_iter(params_ref), |row| {
            Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
            ))
        })?.collect::<Result<Vec<_>, _>>()?;

        let mut artifacts = Vec::new();
        for row in rows {
            if let Some(artifact) = fetch_artifact_with_related(&conn, &row.0, &Viewer { user_id: row.1.clone(), org_id: None })? {
                artifacts.push(artifact);
            }
        }

        Ok::<_, rusqlite::Error>(artifacts)
    }).await;

    match result {
        Ok(Ok(artifacts)) => Json(json!({"artifacts": artifacts})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error searching artifacts: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts/stats
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ArtifactStats {
    workspace_id: String,
    total: i64,
    drafts: i64,
    #[serde(rename = "final")]
    final_count: i64,
    updated_at: String,
}

async fn get_artifact_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
) -> impl IntoResponse {
    let db = state.db.clone();
    let user_id = user.user_id;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        let mut stmt = conn.prepare(
            "SELECT workspace_id,
                    COUNT(*) as total,
                    SUM(CASE WHEN status = 'draft' THEN 1 ELSE 0 END) as drafts,
                    SUM(CASE WHEN status = 'final' THEN 1 ELSE 0 END) as finals,
                    MAX(updated_at) as latest
             FROM artifacts WHERE user_id = ?1
             GROUP BY workspace_id
             ORDER BY latest DESC",
        )?;
        let stats: Vec<ArtifactStats> = stmt
            .query_map(params![user_id], |row| {
                Ok(ArtifactStats {
                    workspace_id: row.get(0)?,
                    total: row.get(1)?,
                    drafts: row.get(2)?,
                    final_count: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok::<_, rusqlite::Error>(stats)
    })
    .await;

    match result {
        Ok(Ok(stats)) => Json(json!({"stats": stats})).into_response(),
        Ok(Err(e)) => {
            warn!("DB error getting artifact stats: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts/:id/revisions
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_revisions(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        // Verify ownership
        let exists: bool = can_read(&conn, &id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(None);
        }

        let mut stmt = conn.prepare(
            "SELECT id, artifact_id, reason, snapshot, created_at
             FROM artifact_revisions WHERE artifact_id = ?1 ORDER BY created_at DESC",
        )?;
        let revisions: Vec<RevisionRow> = stmt
            .query_map(params![&id], |row| {
                let snapshot_str: String = row.get(3)?;
                let snapshot = serde_json::from_str(&snapshot_str).unwrap_or(json!({}));
                Ok(RevisionRow {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    reason: row.get(2)?,
                    snapshot,
                    created_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(revisions))
    })
    .await;

    match result {
        Ok(Ok(Some(revisions))) => Json(json!({"revisions": revisions})).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing revisions: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GET /artifacts/:id/sections
// ═══════════════════════════════════════════════════════════════════════════════

async fn list_sections(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;

        let exists: bool = can_read(&conn, &id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(None);
        }

        let mut stmt = conn.prepare(
            "SELECT id, artifact_id, heading, kind, body, position, created_at, updated_at
             FROM artifact_sections WHERE artifact_id = ?1 ORDER BY position ASC, created_at ASC",
        )?;
        let sections: Vec<SectionRow> = stmt
            .query_map(params![&id], |row| {
                Ok(SectionRow {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    heading: row.get(2)?,
                    kind: row.get(3)?,
                    body: row.get(4)?,
                    position: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Some(sections))
    })
    .await;

    match result {
        Ok(Ok(Some(sections))) => Json(json!({"sections": sections})).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error listing sections: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POST /artifacts/:id/sections
// ═══════════════════════════════════════════════════════════════════════════════

async fn add_section(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SectionBody>,
) -> impl IntoResponse {
    let heading = body.heading.unwrap_or_default().trim().to_string();
    if heading.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "heading is required"})),
        )
            .into_response();
    }

    let db = state.db.clone();
    let viewer = Viewer::of(&user);
    let kind = body.kind.unwrap_or_else(|| "document/markdown".to_string());
    let body_text = body.body.unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        let exists: bool = can_edit(&tx, &id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(None);
        }

        let position = if let Some(pos) = body.position {
            pos
        } else {
            tx.query_row(
                "SELECT COUNT(*) FROM artifact_sections WHERE artifact_id = ?1",
                params![&id],
                |row| row.get::<_, i64>(0),
            ).unwrap_or(0)
        };

        let section_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        tx.execute(
            "INSERT INTO artifact_sections (id, artifact_id, heading, kind, body, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![&section_id, &id, &heading, &kind, &body_text, position, &now],
        )?;

        tx.execute(
            "UPDATE artifacts SET updated_at = ?1 WHERE id = ?2",
            params![&now, &id],
        )?;

        let artifact = fetch_artifact_with_related(&tx, &id, &viewer)?.unwrap();
        create_revision(&tx, &id, &format!("section:{}:created", section_id), &artifact)?;

        tx.commit()?;

        let section = SectionRow {
            id: section_id,
            artifact_id: id.clone(),
            heading,
            kind,
            body: body_text,
            position,
            created_at: now.clone(),
            updated_at: now,
        };

        Ok(Some(section))
    }).await;

    match result {
        Ok(Ok(Some(section))) => {
            (StatusCode::CREATED, Json(json!({"section": section}))).into_response()
        }
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Artifact not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error adding section: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PATCH /artifacts/:id/sections/:section_id
// ═══════════════════════════════════════════════════════════════════════════════

async fn update_section(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path((artifact_id, section_id)): Path<(String, String)>,
    Json(body): Json<SectionBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        let exists: bool = can_edit(&tx, &artifact_id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(None);
        }

        let has_section: bool = tx
            .query_row(
                "SELECT 1 FROM artifact_sections WHERE id = ?1 AND artifact_id = ?2",
                params![&section_id, &artifact_id],
                |_row| Ok(true),
            )
            .unwrap_or(false);
        if !has_section {
            return Ok(None);
        }

        let mut updates = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(h) = body.heading {
            let trimmed = h.trim();
            if !trimmed.is_empty() {
                updates.push("heading = ?".to_string());
                params_vec.push(Box::new(trimmed.to_string()));
            }
        }
        if let Some(k) = body.kind {
            updates.push("kind = ?".to_string());
            params_vec.push(Box::new(k));
        }
        if let Some(b) = body.body {
            updates.push("body = ?".to_string());
            params_vec.push(Box::new(b));
        }
        if let Some(p) = body.position {
            updates.push("position = ?".to_string());
            params_vec.push(Box::new(p));
        }

        if !updates.is_empty() {
            updates.push("updated_at = CURRENT_TIMESTAMP".to_string());
            let sql = format!(
                "UPDATE artifact_sections SET {} WHERE id = ?",
                updates.join(", ")
            );
            params_vec.push(Box::new(section_id.clone()));
            let params_ref: Vec<&dyn rusqlite::ToSql> =
                params_vec.iter().map(|p| p.as_ref()).collect();
            tx.execute(&sql, rusqlite::params_from_iter(params_ref))?;
        }

        let now = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE artifacts SET updated_at = ?1 WHERE id = ?2",
            params![&now, &artifact_id],
        )?;

        let artifact = fetch_artifact_with_related(&tx, &artifact_id, &viewer)?.unwrap();
        create_revision(
            &tx,
            &artifact_id,
            &format!("section:{}:updated", section_id),
            &artifact,
        )?;

        tx.commit()?;

        let mut stmt = conn.prepare(
            "SELECT id, artifact_id, heading, kind, body, position, created_at, updated_at
             FROM artifact_sections WHERE id = ?1",
        )?;
        let section: Option<SectionRow> = stmt
            .query_map(params![&section_id], |row| {
                Ok(SectionRow {
                    id: row.get(0)?,
                    artifact_id: row.get(1)?,
                    heading: row.get(2)?,
                    kind: row.get(3)?,
                    body: row.get(4)?,
                    position: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .next()
            .transpose()?;

        Ok(section)
    })
    .await;

    match result {
        Ok(Ok(Some(section))) => Json(json!({"section": section})).into_response(),
        Ok(Ok(None)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Section not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error updating section: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// DELETE /artifacts/:id/sections/:section_id
// ═══════════════════════════════════════════════════════════════════════════════

async fn delete_section(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    _headers: HeaderMap,
    Path((artifact_id, section_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let mut conn = db.connect()?;
        let tx = conn.transaction()?;

        let exists: bool = can_edit(&tx, &artifact_id, &viewer);
        if !exists {
            return Ok::<_, rusqlite::Error>(false);
        }

        let has_section: bool = tx
            .query_row(
                "SELECT 1 FROM artifact_sections WHERE id = ?1 AND artifact_id = ?2",
                params![&section_id, &artifact_id],
                |_row| Ok(true),
            )
            .unwrap_or(false);
        if !has_section {
            return Ok(false);
        }

        tx.execute(
            "DELETE FROM artifact_sections WHERE id = ?1",
            params![&section_id],
        )?;

        let now = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE artifacts SET updated_at = ?1 WHERE id = ?2",
            params![&now, &artifact_id],
        )?;

        let artifact = fetch_artifact_with_related(&tx, &artifact_id, &viewer)?.unwrap();
        create_revision(
            &tx,
            &artifact_id,
            &format!("section:{}:deleted", section_id),
            &artifact,
        )?;

        tx.commit()?;
        Ok(true)
    })
    .await;

    match result {
        Ok(Ok(true)) => Json(json!({"ok": true})).into_response(),
        Ok(Ok(false)) => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Section not found"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error deleting section: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal error"})),
            )
                .into_response()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PATCH /artifacts/:id/sharing
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, PartialEq)]
enum SharingError {
    NotFound,
    NotOwner,
    Invalid(&'static str),
    NoOrganization,
}

/// Set who an artifact is shared with (owner only): `private`, or `org` with
/// `view`/`edit` access for the owner's current organization.
fn set_sharing(
    conn: &rusqlite::Connection,
    artifact_id: &str,
    viewer: &Viewer,
    visibility: &str,
    access: Option<&str>,
) -> Result<Result<(), SharingError>, rusqlite::Error> {
    let Some(found) = artifact_access(conn, artifact_id, viewer)? else {
        return Ok(Err(SharingError::NotFound));
    };
    if !found.owner {
        return Ok(Err(SharingError::NotOwner));
    }
    let access = access.unwrap_or("view");
    if !matches!(access, "view" | "edit") {
        return Ok(Err(SharingError::Invalid("access must be view or edit")));
    }
    match visibility {
        "private" => {
            conn.execute(
                "UPDATE artifacts SET visibility = 'private', org_id = NULL, org_access = ?2 WHERE id = ?1",
                params![artifact_id, access],
            )?;
        }
        "org" => {
            let Some(org) = viewer.org_id.as_deref() else {
                return Ok(Err(SharingError::NoOrganization));
            };
            conn.execute(
                "UPDATE artifacts SET visibility = 'org', org_id = ?2, org_access = ?3 WHERE id = ?1",
                params![artifact_id, org, access],
            )?;
        }
        _ => return Ok(Err(SharingError::Invalid("visibility must be private or org"))),
    }
    Ok(Ok(()))
}

async fn update_sharing(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<SharingBody>,
) -> impl IntoResponse {
    let db = state.db.clone();
    let viewer = Viewer::of(&user);

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        match set_sharing(&conn, &id, &viewer, body.visibility.trim(), body.access.as_deref().map(str::trim))? {
            Ok(()) => Ok::<_, rusqlite::Error>(Ok(fetch_artifact_with_related(&conn, &id, &viewer)?)),
            Err(e) => Ok(Err(e)),
        }
    })
    .await;

    match result {
        Ok(Ok(Ok(Some(artifact)))) => Json(json!({"artifact": artifact})).into_response(),
        Ok(Ok(Ok(None))) | Ok(Ok(Err(SharingError::NotFound))) => {
            (StatusCode::NOT_FOUND, Json(json!({"error": "Artifact not found"}))).into_response()
        }
        Ok(Ok(Err(SharingError::NotOwner))) => (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Only the owner can change who it's shared with"})),
        )
            .into_response(),
        Ok(Ok(Err(SharingError::Invalid(msg)))) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
        }
        Ok(Ok(Err(SharingError::NoOrganization))) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "You're not in an organization, so there's no one to share with"})),
        )
            .into_response(),
        Ok(Err(e)) => {
            warn!("DB error updating sharing: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))).into_response()
        }
        Err(e) => {
            warn!("DB task panicked: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "internal error"}))).into_response()
        }
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V1__baseline_schema.sql")).unwrap();
        conn.execute_batch(include_str!("../migrations/V196__artifact_sharing.sql")).unwrap();
        conn.execute(
            "INSERT INTO artifacts (id, user_id, workspace_id, title, type) VALUES ('a1', 'owner', 'w', 'Field messaging', 'document')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO users (id, email, name) VALUES ('owner', 'o@x.test', 'Jenn')", []).unwrap();
        conn
    }

    fn who(user: &str, org: Option<&str>) -> Viewer {
        Viewer { user_id: user.into(), org_id: org.map(Into::into) }
    }

    #[test]
    fn private_artifacts_are_the_owners_alone() {
        let conn = db();
        assert_eq!(
            artifact_access(&conn, "a1", &who("owner", Some("acme"))).unwrap(),
            Some(Access { owner: true, can_edit: true })
        );
        assert_eq!(artifact_access(&conn, "a1", &who("teammate", Some("acme"))).unwrap(), None);
        assert_eq!(artifact_access(&conn, "missing", &who("owner", None)).unwrap(), None);
    }

    #[test]
    fn sharing_with_the_org_gives_members_view_or_edit_and_no_one_else() {
        let conn = db();
        let owner = who("owner", Some("acme"));
        assert_eq!(set_sharing(&conn, "a1", &owner, "org", Some("view")).unwrap(), Ok(()));
        assert_eq!(
            artifact_access(&conn, "a1", &who("teammate", Some("acme"))).unwrap(),
            Some(Access { owner: false, can_edit: false })
        );
        assert_eq!(artifact_access(&conn, "a1", &who("stranger", Some("other"))).unwrap(), None);
        assert_eq!(artifact_access(&conn, "a1", &who("no-org", None)).unwrap(), None);

        assert_eq!(set_sharing(&conn, "a1", &owner, "org", Some("edit")).unwrap(), Ok(()));
        assert!(can_edit(&conn, "a1", &who("teammate", Some("acme"))));

        let row = fetch_artifact_with_related(&conn, "a1", &who("teammate", Some("acme"))).unwrap().unwrap();
        assert_eq!((row.visibility.as_str(), row.org_access.as_str()), ("org", "edit"));
        assert!(!row.is_owner && row.can_edit);
        assert_eq!(row.owner_name.as_deref(), Some("Jenn"));

        assert_eq!(set_sharing(&conn, "a1", &owner, "private", None).unwrap(), Ok(()));
        assert!(!can_read(&conn, "a1", &who("teammate", Some("acme"))));
    }

    #[test]
    fn only_the_owner_changes_sharing_and_only_within_an_org() {
        let conn = db();
        let owner = who("owner", Some("acme"));
        set_sharing(&conn, "a1", &owner, "org", Some("edit")).unwrap().unwrap();
        assert_eq!(
            set_sharing(&conn, "a1", &who("teammate", Some("acme")), "private", None).unwrap(),
            Err(SharingError::NotOwner)
        );
        assert_eq!(
            set_sharing(&conn, "a1", &who("stranger", Some("other")), "private", None).unwrap(),
            Err(SharingError::NotFound)
        );
        assert_eq!(
            set_sharing(&conn, "a1", &who("owner", None), "org", None).unwrap(),
            Err(SharingError::NoOrganization)
        );
        assert!(matches!(set_sharing(&conn, "a1", &owner, "public", None).unwrap(), Err(SharingError::Invalid(_))));
        assert!(matches!(set_sharing(&conn, "a1", &owner, "org", Some("admin")).unwrap(), Err(SharingError::Invalid(_))));
    }
}
