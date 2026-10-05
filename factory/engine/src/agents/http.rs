//! Teams over HTTP (API.md §3 Agents):
//!
//! * `GET  /api/factory/teams` → `{ teams: Team[], invalid: {name, errors}[] }`
//! * `POST /api/factory/teams/:name/up` body `{ preset?, on?, dryRun? }` →
//!   `{ plan: TeamPlanStep[], applied, results? }`
//! * `POST /api/factory/teams/:name/down` body `{ dryRun?, rmWorktree? }` →
//!   `{ stopped: string[], plan, applied, results? }`
//!
//! Errors are `{ error: { code, fact, action } }` with 400 / 404 / 409 / 502 /
//! 504. A partly failed apply answers with the worst step's status and still
//! carries `plan` and `results`, so nothing that did happen is hidden. The
//! pane engine and allternit-api calls block, so they run on
//! `spawn_blocking`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as AxPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::team::{self, TeamError};
use super::team_apply::{self, ApiClient, ApplyOptions, FactoryApi, StepOutcome, StepResult};
use super::team_plan::{plan_down, plan_up};

#[derive(Clone)]
struct TeamsState {
    root: PathBuf,
    /// Injected client (tests); `None` reads allternit-api from the env per call.
    api: Option<Arc<dyn FactoryApi>>,
}

/// The teams router over workspace `root`.
pub fn router(root: PathBuf) -> Router {
    router_with_api(root, None)
}

/// [`router`] with a fixed allternit-api client (tests, embedding).
pub fn router_with_api(root: PathBuf, api: Option<Arc<dyn FactoryApi>>) -> Router {
    Router::new()
        .route("/api/factory/teams", get(list_teams))
        .route("/api/factory/teams/:name/up", post(team_up))
        .route("/api/factory/teams/:name/down", post(team_down))
        .with_state(TeamsState { root, api })
}

/// Status for a CLI error code.
pub fn status_for(code: &str) -> StatusCode {
    match code {
        "usage" => StatusCode::BAD_REQUEST,
        "not_found" => StatusCode::NOT_FOUND,
        "refused" | "needs_person" => StatusCode::CONFLICT,
        "timeout" => StatusCode::GATEWAY_TIMEOUT,
        "transport" => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `{ error: { code, fact, action } }` plus any extra fields.
pub fn error_response(code: &str, fact: impl Into<String>, action: &str, extra: Value) -> Response {
    let mut body = json!({ "error": { "code": code, "fact": fact.into(), "action": action } });
    if let (Some(b), Value::Object(e)) = (body.as_object_mut(), extra) {
        b.extend(e);
    }
    (status_for(code), Json(body)).into_response()
}

fn team_error(e: &TeamError) -> Response {
    match e {
        TeamError::NotFound(t) => error_response(
            "not_found",
            format!("team {t} not found"),
            "List teams with GET /api/factory/teams.",
            Value::Null,
        ),
        TeamError::Io { .. } => error_response("internal", e.to_string(), "Check the team folder's permissions.", Value::Null),
        _ => error_response("usage", e.to_string(), "Fix team.yaml (every problem is listed) and retry.", Value::Null),
    }
}

async fn list_teams(State(st): State<TeamsState>) -> Response {
    let root = st.root.clone();
    let res = tokio::task::spawn_blocking(move || {
        let mut teams = vec![];
        let mut invalid = vec![];
        for name in team::list_teams(&root) {
            match team::load_team(&root, &name).and_then(|t| t.to_contract(None)) {
                Ok(t) => teams.push(serde_json::to_value(t).unwrap_or_default()),
                Err(e) => invalid.push(json!({ "name": name, "errors": e.to_string() })),
            }
        }
        json!({ "teams": teams, "invalid": invalid })
    })
    .await;
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response("internal", format!("team listing failed: {e}"), "Retry.", Value::Null),
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct UpBody {
    preset: Option<String>,
    on: Option<String>,
    dry_run: bool,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DownBody {
    dry_run: bool,
    rm_worktree: bool,
}

fn api_for(st: &TeamsState) -> Result<Option<Arc<dyn FactoryApi>>, Response> {
    if let Some(api) = &st.api {
        return Ok(Some(api.clone()));
    }
    match ApiClient::from_env() {
        Ok(c) => Ok(c.map(|c| Arc::new(c) as Arc<dyn FactoryApi>)),
        Err(e) => Err(error_response("transport", e.fact, team_apply::API_ENV_ACTION, Value::Null)),
    }
}

/// 200 with `body` when every step is ok/skipped, else the worst step's
/// status with an error and the same body.
fn applied_response(results: &[StepResult], body: Value) -> Response {
    match team_apply::worst_code(results) {
        None => Json(body).into_response(),
        Some(code) => {
            let failed = results.iter().filter(|r| r.outcome == StepOutcome::Failed).count();
            let first = results.iter().find(|r| r.outcome == StepOutcome::Failed).map(|r| format!("{}: {}", r.step.agent, r.fact)).unwrap_or_default();
            error_response(&code, format!("{failed} step(s) failed; first: {first}"), "Read each result's fact, fix the cause, and run up again (it skips what is running).", body)
        }
    }
}

async fn team_up(State(st): State<TeamsState>, AxPath(name): AxPath<String>, body: Option<Json<UpBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let api = match api_for(&st) {
        Ok(a) => a,
        Err(r) if body.dry_run => {
            drop(r);
            None
        }
        Err(r) => return r,
    };
    let root = st.root.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<Response, Response> {
        let team = team::load_team(&root, &name).map_err(|e| team_error(&e))?;
        let preset = team.resolve_preset(body.preset.as_deref()).map_err(|e| team_error(&e))?;
        let live = team_apply::live_state(&root, &team)
            .map_err(|e| error_response("transport", format!("{e:#}"), "Start the pane engine (allternit-factory pane) and retry.", Value::Null))?;
        let plan = plan_up(&team, preset.as_deref(), body.on.as_deref(), &live).map_err(|e| team_error(&e))?;
        if body.dry_run {
            return Ok(Json(json!({ "plan": plan, "applied": false })).into_response());
        }
        let opts = ApplyOptions { api, ..Default::default() };
        let results = team_apply::apply(&root, &team, preset.as_deref(), &plan, &opts);
        Ok(applied_response(&results, json!({ "plan": plan, "applied": true, "results": results })))
    })
    .await;
    match res {
        Ok(Ok(r)) | Ok(Err(r)) => r,
        Err(e) => error_response("internal", format!("team up failed: {e}"), "Retry.", Value::Null),
    }
}

async fn team_down(State(st): State<TeamsState>, AxPath(name): AxPath<String>, body: Option<Json<DownBody>>) -> Response {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let root = st.root.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<Response, Response> {
        let team = team::load_team(&root, &name).map_err(|e| team_error(&e))?;
        let live = team_apply::live_state(&root, &team)
            .map_err(|e| error_response("transport", format!("{e:#}"), "Start the pane engine (allternit-factory pane) and retry.", Value::Null))?;
        let plan = plan_down(&team, &live);
        if body.dry_run {
            return Ok(Json(json!({ "plan": plan, "applied": false, "stopped": [] })).into_response());
        }
        let opts = ApplyOptions { rm_worktree: body.rm_worktree, ..Default::default() };
        let results = team_apply::apply(&root, &team, None, &plan, &opts);
        let stopped: Vec<&str> = results.iter().filter(|r| r.outcome == StepOutcome::Ok).map(|r| r.step.agent.as_str()).collect();
        Ok(applied_response(&results, json!({ "stopped": stopped, "plan": plan, "applied": true, "results": results })))
    })
    .await;
    match res {
        Ok(Ok(r)) | Ok(Err(r)) => r,
        Err(e) => error_response("internal", format!("team down failed: {e}"), "Retry.", Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::team::tests::GOOD;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let st = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn workspace() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let dir = team::team_dir(root.path(), "product-build");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(team::TEAM_FILE), GOOD).unwrap();
        let bad = team::team_dir(root.path(), "broken");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join(team::TEAM_FILE), "bots:\n  - { bot: a, role: r, binding: cloud }\n").unwrap();
        root
    }

    #[tokio::test]
    async fn teams_list_and_dry_run_up() {
        let root = workspace();
        // A pane engine binary that lists no sessions keeps the test hermetic.
        let fake = root.path().join("fake-factory");
        std::fs::write(&fake, "#!/bin/sh\necho 'no ao-* sessions'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var(team_apply::ENV_FACTORY_BIN, &fake);
        let app = router(root.path().to_path_buf());

        let (st, v) = call(&app, "GET", "/api/factory/teams", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["teams"][0]["name"], "product-build");
        assert_eq!(v["teams"][0]["agents"][1], "builder@product-build");
        assert_eq!(v["invalid"][0]["name"], "broken");

        let (st, v) = call(&app, "POST", "/api/factory/teams/product-build/up", Some(json!({ "dryRun": true, "preset": "cheap" }))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["applied"], false);
        assert_eq!(v["plan"][1]["action"], "spawn");
        assert_eq!(v["plan"][1]["harness"], "codex");
        let (_, again) = call(&app, "POST", "/api/factory/teams/product-build/up", Some(json!({ "dryRun": true, "preset": "cheap" }))).await;
        assert_eq!(v, again, "the dry run is deterministic");

        let (st, v) = call(&app, "POST", "/api/factory/teams/nope/up", Some(json!({ "dryRun": true }))).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["code"], "not_found");
        let (st, v) = call(&app, "POST", "/api/factory/teams/broken/up", Some(json!({ "dryRun": true }))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"]["fact"].as_str().unwrap().contains("bots[0].binding"), "{v}");
        let (st, v) = call(&app, "POST", "/api/factory/teams/product-build/up", Some(json!({ "dryRun": true, "preset": "zzz" }))).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");

        let (st, v) = call(&app, "POST", "/api/factory/teams/product-build/down", Some(json!({ "dryRun": true }))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["stopped"], json!([]));
        std::env::remove_var(team_apply::ENV_FACTORY_BIN);
    }
}
