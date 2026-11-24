//! `/v1/environments`: CRUD, plus `queue_depth`/`worker_count` and the
//! workers seen for an environment.
//!
//! An environment's `git` and `mcp` host lists can only narrow the server's
//! lists in the settings file. A host outside them is 403
//! `host_not_allowed`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::auth::User;
use super::settings::Settings;
use super::store::{EnvironmentPatch, EnvironmentRecord, NewEnvironment, StoreError};
use super::{
    check_hostname, check_len, deleted, double_option, on_delete, on_get, on_patch,
    on_post, page_limit, rfc3339, ApiError, ApiJson, ApiQuery, PageQuery, RouteBook,
    SelfHostState,
};

/// Workers seen this recently count as online.
pub const ONLINE_SECS: i64 = 60;
/// Workers seen this recently are listed.
pub const LISTED_SECS: i64 = 600;

pub fn mount(book: RouteBook) -> RouteBook {
    book.add(
        "/v1/environments",
        vec![on_get(list_environments), on_post(create_environment)],
    )
    .add(
        "/v1/environments/:id",
        vec![
            on_get(get_environment),
            on_patch(update_environment),
            on_delete(delete_environment),
        ],
    )
    .add("/v1/environments/:id/workers", vec![on_get(list_workers)])
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    None,
    Limited,
    Full,
}

impl NetworkMode {
    fn as_str(self) -> &'static str {
        match self {
            NetworkMode::None => "none",
            NetworkMode::Limited => "limited",
            NetworkMode::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitPolicy {
    #[serde(default)]
    pub allowed_hostnames: Vec<String>,
    #[serde(default)]
    pub allowed_push_refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpPolicy {
    #[serde(default)]
    pub allowed_hostnames: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packages {
    #[serde(default)]
    pub apt: Vec<String>,
    #[serde(default)]
    pub pip: Vec<String>,
    #[serde(default)]
    pub npm: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvironmentBody {
    pub name: String,
    #[serde(default)]
    pub network: Option<NetworkMode>,
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    #[serde(default)]
    pub git: Option<GitPolicy>,
    #[serde(default)]
    pub mcp: Option<McpPolicy>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub packages: Option<Packages>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateEnvironmentBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub network: Option<NetworkMode>,
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    #[serde(default)]
    pub git: Option<GitPolicy>,
    #[serde(default)]
    pub mcp: Option<McpPolicy>,
    #[serde(default, deserialize_with = "double_option")]
    pub image: Option<Option<String>>,
    #[serde(default)]
    pub packages: Option<Packages>,
    #[serde(default)]
    pub metadata: Option<Map<String, Value>>,
}

fn check_hosts(field: &str, hosts: &[String]) -> Result<(), ApiError> {
    for h in hosts {
        check_hostname(field, h)?;
    }
    Ok(())
}

/// Each host must be in the server-wide list (case-insensitive, exact).
fn check_narrows(field: &str, hosts: &[String], server: &[String]) -> Result<(), ApiError> {
    check_hosts(field, hosts)?;
    for h in hosts {
        if !server.iter().any(|s| s.eq_ignore_ascii_case(h)) {
            return Err(ApiError::forbidden(
                "host_not_allowed",
                format!("{field}: {h} is not in the server's list, and an environment can only narrow it."),
            ));
        }
    }
    Ok(())
}

fn check_git(settings: &Settings, git: &GitPolicy) -> Result<(), ApiError> {
    check_narrows(
        "git.allowed_hostnames",
        &git.allowed_hostnames,
        &settings.git.allowed_hostnames,
    )?;
    for r in &git.allowed_push_refs {
        check_len("git.allowed_push_refs[]", r, 1, 255)?;
    }
    Ok(())
}

fn check_mcp(settings: &Settings, mcp: &McpPolicy) -> Result<(), ApiError> {
    check_narrows(
        "mcp.allowed_hostnames",
        &mcp.allowed_hostnames,
        &settings.mcp.allowed_hostnames,
    )
}

fn to_value<T: Serialize>(v: &T) -> Result<Value, ApiError> {
    serde_json::to_value(v).map_err(|e| ApiError::internal(e.to_string()))
}

/// The policy JSON a worker gets with a claim (also used by step 7).
pub fn policy_json(e: &EnvironmentRecord) -> Value {
    json!({
        "network": e.network,
        "allowed_hosts": e.allowed_hosts,
        "git": e.git,
        "mcp": e.mcp,
        "image": e.image,
        "packages": e.packages,
    })
}

pub async fn environment_json(st: &SelfHostState, e: &EnvironmentRecord) -> Result<Value, ApiError> {
    let since = Utc::now() - Duration::seconds(ONLINE_SECS);
    let worker_count = st.store.count_workers(&e.id, since).await?;
    let queue_depth = st.store.queue_depth(&e.id).await?;
    Ok(json!({
        "id": e.id,
        "object": "environment",
        "name": e.name,
        "network": e.network,
        "allowed_hosts": e.allowed_hosts,
        "git": e.git,
        "mcp": e.mcp,
        "image": e.image,
        "packages": e.packages,
        "worker_count": worker_count,
        "queue_depth": queue_depth,
        "metadata": e.metadata,
        "created_at": rfc3339(e.created_at),
        "updated_at": rfc3339(e.updated_at),
    }))
}

pub fn not_found_env(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("No environment with that id."),
        other => other.into(),
    }
}

async fn list_environments(
    State(st): State<SelfHostState>,
    _user: User,
    ApiQuery(q): ApiQuery<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = page_limit(q.limit)?;
    let mut rows = st.store.list_environments(limit + 1, q.after.as_deref()).await?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let mut data = Vec::with_capacity(rows.len());
    for e in &rows {
        data.push(environment_json(&st, e).await?);
    }
    let next_cursor = if has_more {
        rows.last().map(|e| Value::String(e.id.clone())).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    // The ListEnvelope, built by hand because each row needs async counts.
    Ok(Json(json!({ "data": data, "has_more": has_more, "next_cursor": next_cursor })))
}

async fn create_environment(
    State(st): State<SelfHostState>,
    _user: User,
    ApiJson(body): ApiJson<CreateEnvironmentBody>,
) -> Result<impl IntoResponse, ApiError> {
    check_len("name", &body.name, 1, 100)?;
    let hosts = body.allowed_hosts.unwrap_or_default();
    check_hosts("allowed_hosts", &hosts)?;
    let git = body.git.unwrap_or_default();
    check_git(&st.settings, &git)?;
    let mcp = body.mcp.unwrap_or_default();
    check_mcp(&st.settings, &mcp)?;
    if let Some(i) = &body.image {
        check_len("image", i, 1, 500)?;
    }
    let env = st
        .store
        .create_environment(NewEnvironment {
            name: body.name,
            network: body.network.unwrap_or(NetworkMode::None).as_str().to_string(),
            allowed_hosts: hosts,
            git: to_value(&git)?,
            mcp: to_value(&mcp)?,
            image: body.image,
            packages: to_value(&body.packages.unwrap_or_default())?,
            metadata: Value::Object(body.metadata.unwrap_or_default()),
        })
        .await?;
    Ok((StatusCode::CREATED, Json(environment_json(&st, &env).await?)))
}

async fn get_environment(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let e = st.store.get_environment(&id).await.map_err(not_found_env)?;
    Ok(Json(environment_json(&st, &e).await?))
}

async fn update_environment(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateEnvironmentBody>,
) -> Result<Json<Value>, ApiError> {
    if let Some(n) = &body.name {
        check_len("name", n, 1, 100)?;
    }
    if let Some(h) = &body.allowed_hosts {
        check_hosts("allowed_hosts", h)?;
    }
    if let Some(g) = &body.git {
        check_git(&st.settings, g)?;
    }
    if let Some(m) = &body.mcp {
        check_mcp(&st.settings, m)?;
    }
    if let Some(Some(i)) = &body.image {
        check_len("image", i, 1, 500)?;
    }
    let patch = EnvironmentPatch {
        name: body.name,
        network: body.network.map(|n| n.as_str().to_string()),
        allowed_hosts: body.allowed_hosts,
        git: body.git.as_ref().map(to_value).transpose()?,
        mcp: body.mcp.as_ref().map(to_value).transpose()?,
        image: body.image,
        packages: body.packages.as_ref().map(to_value).transpose()?,
        metadata: body.metadata.map(Value::Object),
    };
    let e = st
        .store
        .update_environment(&id, patch)
        .await
        .map_err(not_found_env)?;
    Ok(Json(environment_json(&st, &e).await?))
}

async fn delete_environment(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    st.store.delete_environment(&id).await.map_err(not_found_env)?;
    Ok(Json(deleted(&id)))
}

async fn list_workers(
    State(st): State<SelfHostState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    st.store.get_environment(&id).await.map_err(not_found_env)?;
    let now = Utc::now();
    let workers = st
        .store
        .list_workers(&id, now - Duration::seconds(LISTED_SECS))
        .await?;
    let online_since = now - Duration::seconds(ONLINE_SECS);
    let data: Vec<Value> = workers
        .iter()
        .map(|w| {
            json!({
                "worker_id": w.worker_id,
                "last_seen_at": rfc3339(w.last_seen_at),
                "online": w.last_seen_at >= online_since,
                "current_work_id": w.current_work_id,
                "hostname": w.hostname,
                "version": w.version,
            })
        })
        .collect();
    Ok(Json(json!({ "data": data })))
}
