//! Developer routes for "Sign in with Allternit" apps under a platform project
//! (CONTRACT-oauth-linked-billing-2026-11.md, Part A). Clerk session only, and
//! the same access as the project's API keys: the project's owner, or an admin
//! of the project's Clerk org (`projects::get_project`). Anyone else gets 404,
//! so a project's apps are never confirmed to a stranger.
//!
//! - `GET/POST   /api/v1/platform/projects/:id/oauth-apps`
//! - `GET/PATCH/DELETE …/oauth-apps/:app`
//! - `POST       …/:app/secret/rotate`: the new `client_secret` is in this reply only.
//! - `GET/PUT    …/:app/test-users` (`PUT {"emails": [...]}` makes the list exactly that),
//!   `POST …/test-users {"email"}`, `DELETE …/test-users/:email`
//! - `POST       …/:app/redirect-uris {"uri"}`, `DELETE …/:app/redirect-uris?uri=`
//! - `POST       …/:app/submit`: `testing` to `in_review`; `POST …/:app/withdraw` back.
//!
//! The app JSON is `OAuthApp` plus `redirect_uris` (`services::oauth_apps::AppView`):
//! `{id, project_id, owner_user_id, client_id, name, logo_url, homepage_url,
//! privacy_url, client_type, secret_prefix, state, scopes, created_at,
//! updated_at, approved_at, approved_by, redirect_uris}`. Create and rotate add
//! `client_secret` (confidential apps only), once. Lists are
//! `{"object":"list","data":[…]}`.
//!
//! Every write adds an `oauth_audit_log` row from the service it calls. Going
//! live, suspending and reinstating are admin moves (`routes::oauth_grants`).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    console::session,
    console_governance::ConsoleSession,
    projects::{self, Project},
    ApiJson, ApiQuery, PlatformError,
};
use crate::services::oauth_apps::{
    self, AppState, AuditCtx, ClientType, CreateOAuthAppInput, OAuthApp, StateActor, UpdateOAuthAppInput,
};
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/platform/projects/:id/oauth-apps", get(list_apps).post(create_app))
        .route(
            "/api/v1/platform/projects/:id/oauth-apps/:app",
            get(get_app).patch(update_app).delete(delete_app),
        )
        .route("/api/v1/platform/projects/:id/oauth-apps/:app/secret/rotate", post(rotate_secret))
        .route("/api/v1/platform/projects/:id/oauth-apps/:app/test-users", get(list_test_users).put(set_test_users).post(add_test_user))
        .route("/api/v1/platform/projects/:id/oauth-apps/:app/test-users/:email", delete(remove_test_user))
        .route(
            "/api/v1/platform/projects/:id/oauth-apps/:app/redirect-uris",
            post(add_redirect_uri).delete(remove_redirect_uri),
        )
        .route("/api/v1/platform/projects/:id/oauth-apps/:app/submit", post(submit_app))
        .route("/api/v1/platform/projects/:id/oauth-apps/:app/withdraw", post(withdraw_app))
}

// ─── shared ───────────────────────────────────────────────────────────────────

/// The session and the project it may manage (404 otherwise).
async fn open_project(
    state: &ApiState,
    headers: &HeaderMap,
    project_id: &str,
) -> Result<(ConsoleSession, Project), PlatformError> {
    let session = session(headers).await?;
    let project = projects::get_project(&state.db, &session.who, project_id).await?;
    Ok((session, project))
}

/// Like [`open_project`], for writes: an archived project takes none.
async fn open_project_for_write(
    state: &ApiState,
    headers: &HeaderMap,
    project_id: &str,
) -> Result<(ConsoleSession, Project), PlatformError> {
    let (session, project) = open_project(state, headers, project_id).await?;
    if project.archived_at.is_some() {
        return Err(PlatformError::conflict("project_archived", "This project is archived."));
    }
    Ok((session, project))
}

fn audit_ctx(session: &ConsoleSession, headers: &HeaderMap, peer: Option<ConnectInfo<SocketAddr>>) -> AuditCtx {
    AuditCtx {
        actor: session.who.user_id.clone(),
        ip: super::console_governance::request_ip(headers, peer),
        ua: headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|ua| ua.chars().take(512).collect()),
    }
}

fn app_not_found() -> PlatformError {
    PlatformError::not_found("oauth_app_not_found", "No such OAuth app.")
}

fn app_id(raw: &str) -> Result<Uuid, PlatformError> {
    Uuid::parse_str(raw).map_err(|_| app_not_found())
}

/// An app of this project, 404 otherwise.
async fn project_app(state: &ApiState, project: &Project, app: &str) -> Result<OAuthApp, PlatformError> {
    Ok(oauth_apps::get_app(&state.db, &project.id, app_id(app)?).await?)
}

async fn view(state: &ApiState, app: OAuthApp) -> Result<Json<Value>, PlatformError> {
    let view = oauth_apps::app_view(&state.db, app).await?;
    to_json(&view)
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Json<Value>, PlatformError> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|e| PlatformError::api_error("internal_error", e.to_string()))
}

// ─── apps ─────────────────────────────────────────────────────────────────────

async fn list_apps(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, PlatformError> {
    let (_, project) = open_project(&state, &headers, &id).await?;
    let apps = oauth_apps::list_apps(&state.db, &project.id).await?;
    let mut data = Vec::with_capacity(apps.len());
    for app in apps {
        data.push(oauth_apps::app_view(&state.db, app).await?);
    }
    Ok(Json(json!({ "object": "list", "data": data })))
}

#[derive(Debug, Deserialize)]
struct CreateApp {
    name: String,
    /// `confidential` (default) or `public`.
    client_type: Option<String>,
    logo_url: Option<String>,
    homepage_url: Option<String>,
    privacy_url: Option<String>,
    /// Default `["inference"]`.
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

async fn create_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CreateApp>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let client_type = match body.client_type.as_deref() {
        None | Some("confidential") => ClientType::Confidential,
        Some("public") => ClientType::Public,
        Some(_) => {
            return Err(PlatformError::invalid_request("invalid_client_type", "client_type must be 'confidential' or 'public'.")
                .with_param("client_type"))
        }
    };
    let scopes = if body.scopes.is_empty() { vec!["inference".to_string()] } else { body.scopes };
    let created = oauth_apps::create_app(
        &state.db,
        CreateOAuthAppInput {
            project_id: project.id.clone(),
            owner_user_id: session.who.user_id.clone(),
            name: body.name,
            client_type,
            logo_url: body.logo_url,
            homepage_url: body.homepage_url,
            privacy_url: body.privacy_url,
            scopes,
            redirect_uris: body.redirect_uris,
        },
        &audit_ctx(&session, &headers, peer),
    )
    .await?;
    Ok((StatusCode::CREATED, to_json(&created)?))
}

async fn get_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, app)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let (_, project) = open_project(&state, &headers, &id).await?;
    let app = project_app(&state, &project, &app).await?;
    view(&state, app).await
}

#[derive(Debug, Deserialize)]
struct PatchApp {
    name: Option<String>,
    /// `""` clears a URL.
    logo_url: Option<String>,
    homepage_url: Option<String>,
    privacy_url: Option<String>,
    scopes: Option<Vec<String>>,
}

async fn update_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
    ApiJson(body): ApiJson<PatchApp>,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let app_id = app_id(&app)?;
    let updated = oauth_apps::update_app(
        &state.db,
        &project.id,
        app_id,
        UpdateOAuthAppInput {
            name: body.name,
            logo_url: body.logo_url,
            homepage_url: body.homepage_url,
            privacy_url: body.privacy_url,
            scopes: body.scopes,
        },
        &audit_ctx(&session, &headers, peer),
    )
    .await?;
    view(&state, updated).await
}

async fn delete_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
) -> Result<StatusCode, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    oauth_apps::delete_app(&state.db, &project.id, app_id(&app)?, &audit_ctx(&session, &headers, peer)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rotate_secret(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let rotated =
        oauth_apps::rotate_secret(&state.db, &project.id, app_id(&app)?, &audit_ctx(&session, &headers, peer)).await?;
    let mut out = to_json(&rotated)?;
    out.0["redirect_uris"] = json!(oauth_apps::list_redirect_uris(&state.db, rotated.app.id).await?);
    Ok(out)
}

// ─── states ───────────────────────────────────────────────────────────────────

async fn move_state(
    state: &ApiState,
    headers: &HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    id: &str,
    app: &str,
    to: AppState,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(state, headers, id).await?;
    // 404 unless the app is this project's; set_state then scopes by project again under its lock.
    let app = project_app(state, &project, app).await?;
    let updated = oauth_apps::set_state(
        &state.db,
        app.id,
        to,
        StateActor::Owner { project_id: &project.id },
        &audit_ctx(&session, headers, peer),
    )
    .await?;
    view(state, updated).await
}

async fn submit_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    move_state(&state, &headers, peer, &id, &app, AppState::InReview).await
}

async fn withdraw_app(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    move_state(&state, &headers, peer, &id, &app, AppState::Testing).await
}

// ─── test users ───────────────────────────────────────────────────────────────

async fn list_test_users(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, app)): Path<(String, String)>,
) -> Result<Json<Value>, PlatformError> {
    let (_, project) = open_project(&state, &headers, &id).await?;
    let app = project_app(&state, &project, &app).await?;
    Ok(Json(json!({ "object": "list", "data": oauth_apps::list_test_users(&state.db, app.id).await? })))
}

#[derive(Debug, Deserialize)]
struct EmailBody {
    email: String,
}

async fn add_test_user(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
    ApiJson(body): ApiJson<EmailBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let user =
        oauth_apps::add_test_user(&state.db, &project.id, app_id(&app)?, &body.email, &audit_ctx(&session, &headers, peer)).await?;
    Ok((StatusCode::CREATED, to_json(&user)?))
}

async fn remove_test_user(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app, email)): Path<(String, String, String)>,
) -> Result<StatusCode, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    oauth_apps::remove_test_user(&state.db, &project.id, app_id(&app)?, &email, &audit_ctx(&session, &headers, peer)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct EmailsBody {
    emails: Vec<String>,
}

/// Makes the app's test users exactly `emails`: adds the missing, removes the
/// rest. Each change is its own audited write.
async fn set_test_users(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
    ApiJson(body): ApiJson<EmailsBody>,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let app_id = app_id(&app)?;
    let ctx = audit_ctx(&session, &headers, peer);
    let mut wanted: Vec<String> = body.emails.iter().map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).collect();
    wanted.sort();
    wanted.dedup();
    let current: Vec<String> =
        oauth_apps::list_test_users(&state.db, app_id).await?.into_iter().map(|u| u.email.to_lowercase()).collect();
    for email in wanted.iter().filter(|e| !current.contains(e)) {
        oauth_apps::add_test_user(&state.db, &project.id, app_id, email, &ctx).await?;
    }
    for email in current.iter().filter(|e| !wanted.contains(e)) {
        oauth_apps::remove_test_user(&state.db, &project.id, app_id, email, &ctx).await?;
    }
    Ok(Json(json!({ "object": "list", "data": oauth_apps::list_test_users(&state.db, app_id).await? })))
}

// ─── redirect URIs ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct UriBody {
    uri: String,
}

async fn add_redirect_uri(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
    ApiJson(body): ApiJson<UriBody>,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let uris =
        oauth_apps::add_redirect_uri(&state.db, &project.id, app_id(&app)?, &body.uri, &audit_ctx(&session, &headers, peer)).await?;
    Ok(Json(json!({ "redirect_uris": uris })))
}

async fn remove_redirect_uri(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    peer: Option<ConnectInfo<SocketAddr>>,
    Path((id, app)): Path<(String, String)>,
    ApiQuery(q): ApiQuery<UriBody>,
) -> Result<Json<Value>, PlatformError> {
    let (session, project) = open_project_for_write(&state, &headers, &id).await?;
    let uris =
        oauth_apps::remove_redirect_uri(&state.db, &project.id, app_id(&app)?, &q.uri, &audit_ctx(&session, &headers, peer)).await?;
    Ok(Json(json!({ "redirect_uris": uris })))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, Router};
    use tower::ServiceExt;

    use super::super::{console::TEST_CLAIMS_HEADER, router_gated, Gate};
    use super::*;
    use crate::routes::test_support::{test_state, MockGateway};

    const OWNER: &str = "user_dev_owner";
    const STRANGER: &str = "user_dev_stranger";

    async fn app() -> (Arc<ApiState>, Router, String) {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        for sql in [
            include_str!("../../../migrations_pg/003_api_keys.sql"),
            include_str!("../../../migrations_pg/050_platform_api_foundation.sql"),
            include_str!("../../../migrations_pg/080_platform_billing.sql"),
            include_str!("../../../migrations_pg/084_platform_payment_methods.sql"),
            include_str!("../../../migrations_pg/126_key_governance.sql"),
        ] {
            sqlx::raw_sql(&sql.replace("public.", "")).execute(&state.db).await.expect("migration applies");
        }
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, email TEXT);")
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../../../migrations_pg/150_oauth_apps.sql")).execute(&state.db).await.unwrap();
        let owner = projects::Principal { user_id: OWNER.to_string(), org_id: None, org_admin: false };
        let project = projects::create_project(&state.db, &owner, "Notes", super::super::ProjectEnv::Sandbox).await.unwrap();
        let router = router_gated(&state, Gate::Forced(true)).with_state(state.clone());
        (state, router, project.id)
    }

    async fn send(router: &Router, user: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let builder = Request::builder().method(method).uri(path).header(TEST_CLAIMS_HEADER, json!({ "sub": user }).to_string());
        let request = match body {
            Some(v) => builder.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn make_app(router: &Router, project: &str, client_type: &str) -> Value {
        let (status, body) = send(
            router,
            OWNER,
            "POST",
            &format!("/api/v1/platform/projects/{project}/oauth-apps"),
            Some(json!({
                "name": "Notes Sync",
                "client_type": client_type,
                "scopes": ["inference", "usage"],
                "redirect_uris": ["https://notes.example.com/cb"],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body
    }

    async fn audit_count(state: &ApiState, event: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM oauth_audit_log WHERE event = $1")
            .bind(event)
            .fetch_one(&state.db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn non_owner_cannot_manage_app() {
        let (state, router, project) = app().await;
        let created = make_app(&router, &project, "confidential").await;
        let base = format!("/api/v1/platform/projects/{project}/oauth-apps");
        let one = format!("{base}/{}", created["id"].as_str().unwrap());
        for (method, path, body) in [
            ("GET", base.clone(), None),
            ("POST", base.clone(), Some(json!({ "name": "Mine now" }))),
            ("GET", one.clone(), None),
            ("PATCH", one.clone(), Some(json!({ "name": "Hijacked" }))),
            ("POST", format!("{one}/secret/rotate"), None),
            ("POST", format!("{one}/submit"), None),
            ("PUT", format!("{one}/test-users"), Some(json!({ "emails": ["a@example.com"] }))),
            ("DELETE", one.clone(), None),
        ] {
            let (status, body) = send(&router, STRANGER, method, &path, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}: {body}");
        }
        // Nothing changed.
        let (name, state_now): (String, String) = sqlx::query_as("SELECT name, state FROM oauth_apps WHERE project_id = $1")
            .bind(&project)
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!((name.as_str(), state_now.as_str()), ("Notes Sync", "testing"));
        assert_eq!(audit_count(&state, "secret_rotated").await, 0);
        // The owner still can.
        let (status, body) = send(&router, OWNER, "GET", &one, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["redirect_uris"], json!(["https://notes.example.com/cb"]));
        assert!(body.get("client_secret").is_none());
        assert!(body.get("secret_hash").is_none());
    }

    #[tokio::test]
    async fn rotate_secret_shown_once() {
        let (state, router, project) = app().await;
        let created = make_app(&router, &project, "confidential").await;
        let first = created["client_secret"].as_str().unwrap().to_string();
        assert!(first.starts_with("alt_ocs_"), "{created}");
        let one = format!("/api/v1/platform/projects/{project}/oauth-apps/{}", created["id"].as_str().unwrap());

        // A read never returns the secret.
        let (_, body) = send(&router, OWNER, "GET", &one, None).await;
        assert!(body.get("client_secret").is_none(), "{body}");

        let (status, rotated) = send(&router, OWNER, "POST", &format!("{one}/secret/rotate"), None).await;
        assert_eq!(status, StatusCode::OK, "{rotated}");
        let second = rotated["client_secret"].as_str().unwrap().to_string();
        assert!(second.starts_with("alt_ocs_") && second != first);

        // Neither a read nor the list shows it again; only the prefix.
        let (_, body) = send(&router, OWNER, "GET", &one, None).await;
        assert!(!body.to_string().contains(&second), "{body}");
        let (_, list) = send(&router, OWNER, "GET", &format!("/api/v1/platform/projects/{project}/oauth-apps"), None).await;
        assert!(!list.to_string().contains(&second), "{list}");
        assert!(body["secret_prefix"].is_string());

        // Only the new secret works.
        let row = oauth_apps::get_app_by_id(&state.db, Uuid::parse_str(created["id"].as_str().unwrap()).unwrap()).await.unwrap();
        assert!(row.verify_secret(&second));
        assert!(!row.verify_secret(&first));
        assert_eq!(audit_count(&state, "secret_rotated").await, 1);
        let detail: String = sqlx::query_scalar("SELECT detail::text FROM oauth_audit_log WHERE event = 'secret_rotated'")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert!(!detail.contains(&second), "no secret in the audit trail");

        // A public app has no secret to rotate.
        let public = make_app(&router, &project, "public").await;
        assert!(public["client_secret"].is_null());
        let path = format!("/api/v1/platform/projects/{project}/oauth-apps/{}/secret/rotate", public["id"].as_str().unwrap());
        assert_eq!(send(&router, OWNER, "POST", &path, None).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn submit_moves_to_in_review() {
        let (state, router, project) = app().await;
        let created = make_app(&router, &project, "public").await;
        assert_eq!(created["state"], "testing");
        let one = format!("/api/v1/platform/projects/{project}/oauth-apps/{}", created["id"].as_str().unwrap());

        let (status, body) = send(&router, OWNER, "POST", &format!("{one}/submit"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "in_review");
        let db_state: String = sqlx::query_scalar("SELECT state FROM oauth_apps WHERE id = $1")
            .bind(Uuid::parse_str(created["id"].as_str().unwrap()).unwrap())
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(db_state, "in_review");
        assert_eq!(audit_count(&state, "app_submitted").await, 1);

        // Submitting twice is a conflict; the owner cannot go live themselves.
        assert_eq!(send(&router, OWNER, "POST", &format!("{one}/submit"), None).await.0, StatusCode::CONFLICT);

        // Withdraw sends it back to testing.
        let (status, body) = send(&router, OWNER, "POST", &format!("{one}/withdraw"), None).await;
        assert_eq!((status, body["state"].as_str()), (StatusCode::OK, Some("testing")), "{body}");

        // Test users: add, list, remove.
        let (status, _) = send(&router, OWNER, "PUT", &format!("{one}/test-users"), Some(json!({ "emails": ["A@Example.com", "b@example.com"] }))).await;
        assert_eq!(status, StatusCode::OK);
        let (_, list) = send(&router, OWNER, "GET", &format!("{one}/test-users"), None).await;
        assert_eq!(list["data"].as_array().unwrap().len(), 2, "{list}");
        let (status, _) = send(&router, OWNER, "DELETE", &format!("{one}/test-users/b@example.com"), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(audit_count(&state, "test_user_added").await, 2);
        assert_eq!(audit_count(&state, "test_user_removed").await, 1);
    }
}
