//! `/api/v1/studio/*` routes (merged into the v1 router, behind the auth
//! middleware). Shapes: `services/studio-spec/openapi/studio-api.yaml`.

use super::{HostError, StudioHost};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

impl IntoResponse for HostError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

type Res = Result<Response, HostError>;

/// Paths are absolute so the router can merge into the `/api/v1` nest.
pub fn router<S: Clone + Send + Sync + 'static>(host: StudioHost) -> Router<S> {
    Router::new()
        .route("/studio/engines", get(list_engines))
        .route("/studio/engines/:id/install", post(install_engine))
        .route("/studio/jobs", get(list_jobs).post(create_job))
        .route("/studio/jobs/:id", get(get_job))
        .route("/studio/jobs/:id/events", get(job_events))
        .route("/studio/jobs/:id/cancel", post(cancel_job))
        .route("/studio/jobs/:id/outputs", get(job_outputs))
        .route("/studio/jobs/:id/outputs/register", post(register_output))
        .with_state(host)
}

/// The auth middleware stamps the caller on every request it lets through;
/// a request without one never reached it (defence in depth, as the other
/// local routes do).
fn require_user(headers: &HeaderMap) -> Result<(), HostError> {
    if crate::auth::get_user(headers).is_some() {
        Ok(())
    } else {
        Err(HostError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to use Studio.",
        ))
    }
}

async fn list_engines(State(host): State<StudioHost>, headers: HeaderMap) -> Res {
    require_user(&headers)?;
    Ok(Json(json!({ "engines": host.engine_views() })).into_response())
}

async fn install_engine(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res {
    require_user(&headers)?;
    let view = host.install(&id)?;
    Ok((StatusCode::ACCEPTED, Json(view)).into_response())
}

async fn create_job(State(host): State<StudioHost>, headers: HeaderMap, body: String) -> Res {
    require_user(&headers)?;
    let job = host.create(&body).await?;
    Ok((StatusCode::CREATED, Json(job)).into_response())
}

async fn list_jobs(State(host): State<StudioHost>, headers: HeaderMap) -> Res {
    require_user(&headers)?;
    Ok(Json(json!({ "jobs": host.jobs() })).into_response())
}

async fn get_job(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res {
    require_user(&headers)?;
    let job = host.job(&id).ok_or_else(HostError::not_found)?;
    Ok(Json(job).into_response())
}

#[derive(Deserialize)]
struct After {
    after: Option<u64>,
}

async fn job_events(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<After>,
) -> Res {
    require_user(&headers)?;
    let (state, events) = host.events(&id, q.after.unwrap_or(0))?;
    Ok(Json(json!({ "state": state, "events": events })).into_response())
}

async fn cancel_job(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res {
    require_user(&headers)?;
    Ok(Json(host.cancel(&id).await?).into_response())
}

async fn job_outputs(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Res {
    require_user(&headers)?;
    Ok(Json(json!({ "outputs": host.outputs(&id)? })).into_response())
}

#[derive(Deserialize)]
struct RegisterBody {
    kind: String,
    #[serde(default)]
    name: Option<String>,
}

async fn register_output(
    State(host): State<StudioHost>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RegisterBody>,
) -> Res {
    require_user(&headers)?;
    let model_id = host.register_output(&id, &body.kind, body.name).await?;
    Ok(Json(json!({ "model_id": model_id })).into_response())
}
