//! Console announcement routes
//!
//! Published banners surfaced in the web/desktop console. Read access is any
//! authenticated user (same bar as neighboring console surfaces; the local-dev
//! fallback user qualifies). Publish/unpublish is org-admin gated, following
//! the `admin_spend_limit_routes` gating (`admin_org` + `rbac::is_org_admin`).

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::AuthUser, AppState};

pub fn console_announcement_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/console/announcements", get(list_announcements))
        .route(
            "/admin/console/announcements",
            post(publish_announcement),
        )
        .route(
            "/admin/console/announcements/:id",
            delete(unpublish_announcement),
        )
}

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(json!({"error": code, "message": message.into()})),
    )
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::warn!(error = %err, "console announcements query failed");
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        err.to_string(),
    )
}

/// Same gating as `admin_spend_limit_routes::admin_org`: the caller must
/// belong to an organization and hold an owner/admin role in it.
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
            "Only organization owners/admins can manage announcements.",
        ));
    }
    Ok(org.to_string())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A version is a dotted numeric string ("1", "1.2", "1.2.3"). Loose
/// validation so the min_client_version filter never matches garbage.
fn is_valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 32
        && v.split('.').all(|part| {
            !part.is_empty() && part.len() <= 8 && part.chars().all(|c| c.is_ascii_digit())
        })
}

#[derive(Debug, Deserialize)]
struct ListAnnouncementsQuery {
    /// Optional client version ("1.2.3"). When provided, announcements with a
    /// min_client_version newer than the caller's version are hidden.
    #[serde(default)]
    client_version: Option<String>,
}

async fn list_announcements(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ListAnnouncementsQuery>,
) -> impl IntoResponse {
    if let Some(v) = &query.client_version {
        if !is_valid_version(v) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_version",
                "client_version must be a dotted numeric version like 1.2.3.",
            ));
        }
    }

    let conn = state.db.connect().map_err(internal)?;
    let now = now_rfc3339();
    // Audience: platform-wide ('all') plus the caller's own organization.
    // min_client_version filtering happens in Rust below (version_gte) so the
    // comparison gets numeric per-part semantics ("1.10.0" >= "1.9.0").
    let sql = "SELECT id, title, body, dismissible, published_at, min_client_version
         FROM console_announcements
         WHERE published_at <= ?1
           AND (expires_at IS NULL OR expires_at > ?1)
           AND (audience = 'all' OR audience = ?2)
         ORDER BY published_at DESC";
    let mut stmt = conn.prepare(sql).map_err(internal)?;
    let org = user.organization_id.clone().unwrap_or_default();
    let rows = stmt
        .query_map(params![now, org], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;

    let items: Vec<Value> = rows
        .into_iter()
        .filter(|(_, _, _, _, _, min_version)| {
            match (&query.client_version, min_version) {
                (Some(client), Some(min)) => version_gte(client, min),
                _ => true,
            }
        })
        .map(|(id, title, body, dismissible, published_at, _)| {
            json!({
                "id": id,
                "title": title,
                "body": body,
                "dismissible": dismissible != 0,
                "published_at": published_at,
            })
        })
        .collect();

    Ok::<Json<Value>, ApiError>(Json(json!({ "items": items })))
}

/// Dotted-numeric version comparison with per-part numeric semantics
/// ("1.10.0" >= "1.9.0"). Parts missing on one side count as 0.
fn version_gte(a: &str, b: &str) -> bool {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return true,
            (Some(x), None) => return x.parse::<u64>().unwrap_or(0) > 0 || a_parts.next().is_some(),
            (None, Some(y)) => return y.parse::<u64>().unwrap_or(0) == 0 && version_rest_zero(&mut b_parts),
            (Some(x), Some(y)) => {
                let xn = x.parse::<u64>().unwrap_or(0);
                let yn = y.parse::<u64>().unwrap_or(0);
                if xn != yn {
                    return xn > yn;
                }
            }
        }
    }
}

fn version_rest_zero(parts: &mut std::str::Split<'_, char>) -> bool {
    parts.all(|p| p.parse::<u64>().unwrap_or(0) == 0)
}

#[derive(Debug, Deserialize)]
struct PublishAnnouncement {
    title: String,
    body: String,
    /// 'all' (default, platform-wide) or an organization id. An org-scoped
    /// audience must be the caller's own organization.
    #[serde(default)]
    audience: Option<String>,
    #[serde(default)]
    min_client_version: Option<String>,
    #[serde(default)]
    dismissible: Option<bool>,
    /// RFC3339; defaults to now.
    #[serde(default)]
    published_at: Option<String>,
    /// RFC3339; must be after published_at when set.
    #[serde(default)]
    expires_at: Option<String>,
}

async fn publish_announcement(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<PublishAnnouncement>,
) -> impl IntoResponse {
    let title = body.title.trim();
    let body_text = body.body.trim();
    if title.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_title",
            "title is required.",
        ));
    }
    if body_text.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_body",
            "body is required.",
        ));
    }
    if let Some(v) = &body.min_client_version {
        if !is_valid_version(v) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_version",
                "min_client_version must be a dotted numeric version like 1.2.3.",
            ));
        }
    }

    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let audience = body.audience.unwrap_or_else(|| "all".to_string());
    if audience != "all" && audience != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "audience_forbidden",
            "Org-scoped announcements can only target your own organization.",
        ));
    }

    let published_at = body.published_at.unwrap_or_else(now_rfc3339);
    if chrono::DateTime::parse_from_rfc3339(&published_at).is_err() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "invalid_published_at",
            "published_at must be an RFC3339 timestamp.",
        ));
    }
    if let Some(expires) = &body.expires_at {
        let expires_parsed = chrono::DateTime::parse_from_rfc3339(expires).map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_expires_at",
                "expires_at must be an RFC3339 timestamp.",
            )
        })?;
        let published_parsed = chrono::DateTime::parse_from_rfc3339(&published_at).map_err(|_| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_published_at",
                "published_at must be an RFC3339 timestamp.",
            )
        })?;
        if expires_parsed <= published_parsed {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_expires_at",
                "expires_at must be after published_at.",
            ));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO console_announcements
         (id, title, body, audience, min_client_version, dismissible, published_at, expires_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            title,
            body_text,
            audience,
            body.min_client_version,
            body.dismissible.unwrap_or(true) as i64,
            published_at,
            body.expires_at,
            user.user_id,
        ],
    )
    .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({
        "status": "ok",
        "id": id,
    })))
}

async fn unpublish_announcement(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let conn = state.db.connect().map_err(internal)?;
    let org = admin_org(&conn, &user)?;

    let audience: Option<String> = conn
        .query_row(
            "SELECT audience FROM console_announcements WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(internal)?;
    let audience = audience.ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            "not_found",
            "No such announcement.",
        )
    })?;
    if audience != "all" && audience != org {
        return Err(error(
            StatusCode::FORBIDDEN,
            "audience_forbidden",
            "You can only unpublish announcements from your own organization.",
        ));
    }

    conn.execute(
        "DELETE FROM console_announcements WHERE id = ?1",
        params![id],
    )
    .map_err(internal)?;

    Ok::<Json<Value>, ApiError>(Json(json!({ "status": "ok" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use std::path::Path;
    use tower::ServiceExt;

    fn test_user(user_id: &str, org_id: Option<&str>, role: &str) -> AuthUser {
        AuthUser {
            user_id: user_id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: org_id.map(|s| s.to_string()),
            organization_id: org_id.map(|s| s.to_string()),
            organization_role: Some(role.to_string()),
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
            "INSERT OR IGNORE INTO organizations (id, name) VALUES (?1, 'Other Org')",
            params!["org-2"],
        )
        .unwrap();
        for (uid, role) in [("admin-1", "owner"), ("member-1", "member"), ("admin-2", "owner")] {
            let org = if uid == "admin-2" { "org-2" } else { "org-1" };
            conn.execute(
                "INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?2)",
                params![uid, format!("{}@test.local", uid)],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO organization_members (id, organization_id, user_id, role) VALUES (?1, ?2, ?3, ?4)",
                params![format!("{}:{}", org, uid), org, uid, role],
            )
            .unwrap();
        }
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

    async fn publish(
        app: &axum::Router,
        user: &AuthUser,
        payload: Value,
    ) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/console/announcements")
                    .extension(user.clone())
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        (status, body)
    }

    async fn list(app: &axum::Router, user: &AuthUser, query: &str) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/console/announcements{}", query))
                    .extension(user.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = body_json(resp.into_body()).await;
        (status, body)
    }

    #[tokio::test]
    async fn publish_visible_to_own_org_not_other_org() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let admin1 = test_user("admin-1", Some("org-1"), "org:admin");
        let admin2 = test_user("admin-2", Some("org-2"), "org:admin");
        let payload = json!({
            "title": "Maintenance",
            "body": "Console down at noon.",
            "audience": "org-1",
        });

        let (status, body) = publish(&app, &admin1, payload.clone()).await;
        assert_eq!(status, StatusCode::OK);
        let org_announcement_id = body["id"].as_str().unwrap().to_string();

        let (_, body) = list(&app, &admin1, "").await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Maintenance");
        assert_eq!(items[0]["id"], org_announcement_id);
        // Internal fields never leak.
        assert!(items[0].get("audience").is_none());
        assert!(items[0].get("created_by").is_none());
        assert!(items[0].get("min_client_version").is_none());
        assert_eq!(items[0]["dismissible"], true);

        // Other org does not see org-1's announcement.
        let (_, body) = list(&app, &admin2, "").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn all_audience_visible_to_every_org() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let admin1 = test_user("admin-1", Some("org-1"), "org:admin");
        let admin2 = test_user("admin-2", Some("org-2"), "org:admin");
        publish(
            &app,
            &admin1,
            json!({"title": "Platform notice", "body": "v2 is coming", "audience": "all"}),
        )
        .await;

        for user in [&admin1, &admin2] {
            let (_, body) = list(&app, user, "").await;
            let items = body["items"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["title"], "Platform notice");
        }
    }

    #[tokio::test]
    async fn expired_announcement_hidden() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let admin = test_user("admin-1", Some("org-1"), "org:admin");
        publish(
            &app,
            &admin,
            json!({
                "title": "Old news",
                "body": "Gone tomorrow",
                "published_at": "2026-09-01T00:00:00Z",
                "expires_at": "2026-09-02T00:00:00Z",
            }),
        )
        .await;

        let (_, body) = list(&app, &admin, "").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn min_client_version_filter() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let admin = test_user("admin-1", Some("org-1"), "org:admin");
        publish(
            &app,
            &admin,
            json!({"title": "New feature", "body": "Needs 1.10", "min_client_version": "1.10.0"}),
        )
        .await;
        publish(&app, &admin, json!({"title": "Old feature", "body": "No minimum"})).await;

        // No version header: both visible.
        let (_, body) = list(&app, &admin, "").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 2);

        // 1.9.0 < 1.10.0 → only the unversioned one.
        let (_, body) = list(&app, &admin, "?client_version=1.9.0").await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "Old feature");

        // 1.10.0 meets the minimum → both.
        let (_, body) = list(&app, &admin, "?client_version=1.10.0").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn delete_hides_announcement() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let admin = test_user("admin-1", Some("org-1"), "org:admin");
        let (_, body) = publish(
            &app,
            &admin,
            json!({"title": "Temp", "body": "Soon gone"}),
        )
        .await;
        let id = body["id"].as_str().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/admin/console/announcements/{}", id))
                    .extension(admin.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (_, body) = list(&app, &admin, "").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn non_admin_cannot_publish_or_delete() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);

        let member = test_user("member-1", Some("org-1"), "org:member");
        let (status, _) = publish(
            &app,
            &member,
            json!({"title": "Nope", "body": "Not allowed"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Member list (read) is allowed.
        let (status, _) = list(&app, &member, "").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn publish_validates_required_fields_and_expiry() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);
        let admin = test_user("admin-1", Some("org-1"), "org:admin");

        let (status, _) = publish(&app, &admin, json!({"title": "", "body": "x"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = publish(&app, &admin, json!({"title": "x", "body": " "})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = publish(
            &app,
            &admin,
            json!({
                "title": "x",
                "body": "y",
                "published_at": "2026-09-10T00:00:00Z",
                "expires_at": "2026-09-09T00:00:00Z",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn org_cannot_target_another_org_audience() {
        let temp = tempfile::tempdir().unwrap().keep();
        let state = test_app_state(&temp).await;
        let app = console_announcement_router().with_state(state);
        let admin1 = test_user("admin-1", Some("org-1"), "org:admin");

        let (status, _) = publish(
            &app,
            &admin1,
            json!({"title": "x", "body": "y", "audience": "org-2"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn version_gte_compares_numerically() {
        assert!(version_gte("1.10.0", "1.9.0"));
        assert!(version_gte("1.10.0", "1.10.0"));
        assert!(!version_gte("1.9.0", "1.10.0"));
        assert!(version_gte("2", "1.9"));
        assert!(version_gte("1.2.3", "1.2"));
        assert!(!version_gte("1.2", "1.2.1"));
    }
}
