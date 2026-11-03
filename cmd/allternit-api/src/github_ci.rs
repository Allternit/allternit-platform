//! Live PR cards: model-owned identity, provider-owned facts, revision-bound writes.
use crate::{db::DbHandle, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

type Error = (StatusCode, Json<Value>);
fn error(status: StatusCode, message: &str) -> Error {
    (
        status,
        Json(json!({"error":"ci_unavailable","message":message})),
    )
}
fn internal(_: impl std::fmt::Display) -> Error {
    error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not save the CI action.",
    )
}
fn stale() -> Error {
    error(
        StatusCode::CONFLICT,
        "The PR or workflow changed. Refresh and review the action again.",
    )
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn repository(owner: &str, name: &str, number: u64) -> Result<String, Error> {
    if !segment(owner) || !segment(name) || number == 0 || number > i64::MAX as u64 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Invalid GitHub PR identity.",
        ));
    }
    Ok(format!("{owner}/{name}"))
}
fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().chars().take(8000).collect()
}
fn link(v: &Value) -> Option<String> {
    let s = v.as_str()?;
    let u = reqwest::Url::parse(s).ok()?;
    (u.scheme() == "https"
        && u.host_str() == Some("github.com")
        && u.username().is_empty()
        && u.password().is_none())
    .then(|| s.to_string())
}

/// Only the vendor HTTP origin is replaced in tests. Production disables redirects.
pub struct CiService {
    db: DbHandle,
    client: reqwest::Client,
    origin: String,
}
impl CiService {
    pub fn new(db: DbHandle) -> Self {
        Self::at(db, "https://api.github.com".into())
    }
    fn at(db: DbHandle, origin: String) -> Self {
        Self {
            db,
            origin,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("GitHub client"),
        }
    }
    async fn request(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(token)
            .header("User-Agent", "Allternit-Live-CI")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            req = req.json(&body);
        }
        let mut response = req.send().await.map_err(|_| {
            error(
                StatusCode::BAD_GATEWAY,
                "GitHub could not be reached. Refresh before trying another action.",
            )
        })?;
        if !response.status().is_success() {
            let status = match response.status().as_u16() {
                401 => StatusCode::UNAUTHORIZED,
                403 => StatusCode::FORBIDDEN,
                404 => StatusCode::NOT_FOUND,
                409 | 422 => StatusCode::CONFLICT,
                429 => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::BAD_GATEWAY,
            };
            return Err(error(status,"GitHub could not complete this request. Check connection access, rate limits and repository rules."));
        }
        // Bound the stream, not just Content-Length. No log downloads or signed redirects.
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            error(
                StatusCode::BAD_GATEWAY,
                "GitHub returned an unreadable response.",
            )
        })? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(error(
                    StatusCode::BAD_GATEWAY,
                    "GitHub returned too much CI data.",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            error(
                StatusCode::BAD_GATEWAY,
                "GitHub returned an unreadable response.",
            )
        })
    }
    async fn read(&self, token: &str, path: &str) -> Result<Value, Error> {
        self.request(token, reqwest::Method::GET, path, None).await
    }
    async fn pages(
        &self,
        token: &str,
        path: &str,
        field: Option<&str>,
    ) -> Result<(Vec<Value>, bool), Error> {
        let mut all = Vec::new();
        for page in 1..=5 {
            let sep = if path.contains('?') { '&' } else { '?' };
            let v = self
                .read(token, &format!("{path}{sep}per_page=100&page={page}"))
                .await?;
            let items = field
                .map(|k| &v[k])
                .unwrap_or(&v)
                .as_array()
                .ok_or_else(|| {
                    error(
                        StatusCode::BAD_GATEWAY,
                        "GitHub returned incomplete CI data.",
                    )
                })?;
            all.extend(items.iter().cloned());
            if items.len() < 100 {
                return Ok((all, true));
            }
        }
        Ok((all, false))
    }
    pub async fn snapshot(&self, token: &str, repo: &str, number: u64) -> Result<Value, Error> {
        let base = format!("/repos/{repo}");
        let pr = self.read(token, &format!("{base}/pulls/{number}")).await?;
        let sha = pr["head"]["sha"]
            .as_str()
            .filter(|s| s.len() >= 40 && s.len() <= 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .ok_or_else(|| {
                error(
                    StatusCode::BAD_GATEWAY,
                    "GitHub did not return the PR revision.",
                )
            })?;
        let checks_path = format!("{base}/commits/{sha}/check-runs?filter=latest");
        let statuses_path = format!("{base}/commits/{sha}/statuses");
        let runs_path = format!("{base}/actions/runs?head_sha={sha}");
        let ((checks, cc), (statuses, sc), (runs, rc)) = tokio::try_join!(
            self.pages(token, &checks_path, Some("check_runs")),
            self.pages(token, &statuses_path, None),
            self.pages(token, &runs_path, Some("workflow_runs"))
        )?;
        let head = self.read(token, &format!("{base}/pulls/{number}")).await?;
        if head["head"]["sha"] != pr["head"]["sha"] {
            return Err(stale());
        }
        let mut jobs:Vec<Value>=checks.iter().filter(|c|c["head_sha"]==sha).map(|c|json!({
   "id":format!("check-{}",c["id"]),"name":text(&c["name"]),"status":c["status"],"conclusion":c["conclusion"],"startedAt":c["started_at"],"completedAt":c["completed_at"],
   "url":link(&c["html_url"]).or_else(||link(&c["details_url"])),"summary":text(&c["output"]["summary"]),"detail":text(&c["output"]["text"]),"suiteId":c["check_suite"]["id"]})).collect();
        let mut contexts = std::collections::HashSet::new();
        jobs.extend(statuses.iter().filter(|s|contexts.insert(text(&s["context"]))).map(|s|json!({
   "id":format!("status-{}",s["id"]),"name":text(&s["context"]),"status":if s["state"]=="pending"{"in_progress"}else{"completed"},
   "conclusion":match s["state"].as_str(){Some("success")=>"success",Some("pending")=>"pending",_=>"failure"},
   "startedAt":s["created_at"],"completedAt":s["updated_at"],"url":link(&s["target_url"]),"summary":text(&s["description"]),"detail":"","suiteId":null})));
        let runs:Vec<Value>=runs.iter().filter(|r|r["head_sha"]==sha).map(|r|json!({"id":r["id"],"attempt":r["run_attempt"],"name":text(&r["name"]),"status":r["status"],"conclusion":r["conclusion"],"suiteId":r["check_suite_id"],"url":link(&r["html_url"])})).collect();
        Ok(
            json!({"repository":repo,"number":number,"title":text(&head["title"]),"headSha":sha,"headBranch":text(&head["head"]["ref"]),"baseBranch":text(&head["base"]["ref"]),
   "state":if head["merged"]==true{"merged"}else{head["state"].as_str().unwrap_or("unknown")},"draft":head["draft"]==true,"mergeable":head["mergeable"],"url":link(&head["html_url"]),
   "checks":jobs,"runs":runs,"complete":cc&&sc&&rc,"observedAt":chrono::Utc::now().to_rfc3339()}),
        )
    }
    pub async fn prepare(
        &self,
        user: &str,
        connection: &str,
        token: &str,
        repo: &str,
        number: u64,
        body: &Action,
    ) -> Result<Value, Error> {
        validate_action(&self.snapshot(token, repo, number).await?, body)?;
        let id = uuid::Uuid::new_v4().to_string();
        let expires = now() + 300;
        self.db.connect().map_err(internal)?.execute("INSERT INTO live_ci_actions (id,user_id,connection_id,repository,pull_number,head_sha,action,run_id,run_attempt,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,user,connection,repo,number,body.head_sha,body.action,body.run_id,body.run_attempt,expires]).map_err(internal)?;
        Ok(
            json!({"id":id,"action":body.action,"repository":repo,"number":number,"headSha":body.head_sha,"runId":body.run_id,"runAttempt":body.run_attempt,"expiresAt":expires}),
        )
    }
    pub async fn execute(
        &self,
        user: &str,
        connection: &str,
        token: &str,
        repo: &str,
        number: u64,
        approval: &str,
    ) -> Result<Value, Error> {
        let action=self.db.connect().map_err(internal)?.query_row("SELECT action,head_sha,run_id,run_attempt FROM live_ci_actions WHERE id=?1 AND user_id=?2 AND connection_id=?3 AND repository=?4 AND pull_number=?5 AND state='pending' AND expires_at>?6",params![approval,user,connection,repo,number,now()],|r|Ok(Action{action:r.get(0)?,head_sha:r.get(1)?,run_id:r.get(2)?,run_attempt:r.get(3)?})).optional().map_err(internal)?.ok_or_else(stale)?;
        validate_action(&self.snapshot(token, repo, number).await?, &action)?;
        if self.db.connect().map_err(internal)?.execute("UPDATE live_ci_actions SET state='dispatching' WHERE id=?1 AND state='pending' AND expires_at>?2",params![approval,now()]).map_err(|_|stale())?!=1{return Err(stale());}
        // Consume before dispatch: a timeout can never automatically repeat the write.
        let result = if action.action == "rerun-failed" {
            let run = self
                .read(
                    token,
                    &format!(
                        "/repos/{repo}/actions/runs/{}",
                        action.run_id.unwrap_or_default()
                    ),
                )
                .await?;
            if run["head_sha"] != action.head_sha
                || run["run_attempt"].as_u64() != action.run_attempt
                || run["status"] != "completed"
            {
                return Err(stale());
            }
            self.request(
                token,
                reqwest::Method::POST,
                &format!(
                    "/repos/{repo}/actions/runs/{}/rerun-failed-jobs",
                    action.run_id.unwrap_or_default()
                ),
                Some(json!({})),
            )
            .await
        } else {
            // GitHub checks sha atomically and retains repository protection rules.
            self.request(
                token,
                reqwest::Method::PUT,
                &format!("/repos/{repo}/pulls/{number}/merge"),
                Some(json!({"sha":action.head_sha,"merge_method":"merge"})),
            )
            .await
        };
        self.db
            .connect()
            .map_err(internal)?
            .execute(
                "UPDATE live_ci_actions SET state=?2 WHERE id=?1",
                params![
                    approval,
                    if result.is_ok() {
                        "submitted"
                    } else {
                        "unknown"
                    }
                ],
            )
            .map_err(internal)?;
        let result = result?;
        if action.action == "merge" && result["merged"] != true {
            return Err(stale());
        }
        Ok(json!({"state":"submitted","action":action.action,"approvalId":approval}))
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub action: String,
    pub head_sha: String,
    pub run_id: Option<u64>,
    pub run_attempt: Option<u64>,
}
fn validate_action(s: &Value, a: &Action) -> Result<(), Error> {
    if s["headSha"] != a.head_sha || s["state"] != "open" || s["complete"] != true {
        return Err(stale());
    }
    match a.action.as_str() {
        "rerun-failed"
            if a.run_id.is_some()
                && s["runs"].as_array().is_some_and(|rs| {
                    rs.iter().any(|r| {
                        r["id"].as_u64() == a.run_id
                            && r["attempt"].as_u64() == a.run_attempt
                            && r["status"] == "completed"
                            && matches!(
                                r["conclusion"].as_str(),
                                Some("failure" | "timed_out" | "cancelled" | "action_required")
                            )
                    })
                }) =>
        {
            Ok(())
        }
        "merge"
            if a.run_id.is_none()
                && a.run_attempt.is_none()
                && s["draft"] == false
                && s["mergeable"] == true
                && s["checks"].as_array().is_some_and(|cs| {
                    !cs.is_empty()
                        && cs.iter().all(|c| {
                            c["status"] == "completed"
                                && matches!(
                                    c["conclusion"].as_str(),
                                    Some("success" | "neutral" | "skipped")
                                )
                        })
                }) =>
        {
            Ok(())
        }
        _ => Err(error(
            StatusCode::CONFLICT,
            "This action is not available for the current checks.",
        )),
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    connection_id: Option<String>,
}
fn credential(
    state: &AppState,
    h: &HeaderMap,
    a: &Account,
) -> Result<(String, String, String), Error> {
    let user = crate::auth::get_user(h)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Sign in to read live CI."))?
        .user_id;
    let conn = state.db.connect().map_err(internal)?;
    let rows = crate::connector_routes::load_accounts(&conn, "github", &user).map_err(internal)?;
    let row = crate::connector_routes::pick_account(&rows, a.connection_id.as_deref(), None)
        .map_err(|_| {
            error(
                StatusCode::CONFLICT,
                "This GitHub account is no longer connected.",
            )
        })?
        .ok_or_else(|| {
            error(
                StatusCode::CONFLICT,
                "Connect GitHub in Settings to view live CI.",
            )
        })?;
    if row.status != "connected" {
        return Err(error(StatusCode::CONFLICT, "Reconnect GitHub in Settings."));
    }
    let sealed: String = conn
        .query_row(
            "SELECT COALESCE(access_token,'') FROM connector_connections WHERE id=?1",
            params![row.id],
            |r| r.get(0),
        )
        .map_err(internal)?;
    let token = crate::token_crypto::open(&sealed);
    if token.is_empty() {
        return Err(error(StatusCode::CONFLICT, "Reconnect GitHub in Settings."));
    }
    Ok((user, row.id.clone(), token))
}
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/connectors/github/ci/:owner/:repo/:number",
            get(read_route),
        )
        .route(
            "/connectors/github/ci/:owner/:repo/:number/review",
            post(review_route),
        )
        .route(
            "/connectors/github/ci/:owner/:repo/:number/execute",
            post(execute_route),
        )
}
async fn read_route(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((o, r, n)): Path<(String, String, u64)>,
    Query(a): Query<Account>,
) -> Result<Json<Value>, Error> {
    let repo = repository(&o, &r, n)?;
    let (_, connection, token) = credential(&s, &h, &a)?;
    let mut value = CiService::new(s.db.clone())
        .snapshot(&token, &repo, n)
        .await?;
    value["connectionId"] = json!(connection);
    Ok(Json(value))
}
async fn review_route(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((o, r, n)): Path<(String, String, u64)>,
    Query(a): Query<Account>,
    Json(body): Json<Action>,
) -> Result<Json<Value>, Error> {
    let repo = repository(&o, &r, n)?;
    let (user, c, t) = credential(&s, &h, &a)?;
    Ok(Json(
        CiService::new(s.db.clone())
            .prepare(&user, &c, &t, &repo, n, &body)
            .await?,
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Confirmation {
    approval_id: String,
}
async fn execute_route(
    State(s): State<Arc<AppState>>,
    h: HeaderMap,
    Path((o, r, n)): Path<(String, String, u64)>,
    Query(a): Query<Account>,
    Json(body): Json<Confirmation>,
) -> Result<Json<Value>, Error> {
    let repo = repository(&o, &r, n)?;
    let (user, c, t) = credential(&s, &h, &a)?;
    Ok(Json(
        CiService::new(s.db.clone())
            .execute(&user, &c, &t, &repo, n, &body.approval_id)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, extract::State, http::Request, response::IntoResponse};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Vendor {
        sha: String,
        fail_write: bool,
        writes: Vec<(String, String)>,
        attempt: u64,
        passed: bool,
        pages: bool,
    }
    async fn github(
        State(state): State<Arc<Mutex<Vendor>>>,
        request: Request<Body>,
    ) -> axum::response::Response {
        let path = request.uri().path();
        let query = request.uri().query().unwrap_or_default();
        let mut vendor = state.lock().unwrap();
        if request.method() != axum::http::Method::GET {
            vendor
                .writes
                .push((request.method().to_string(), path.to_string()));
            return if vendor.fail_write {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"message":"lost acknowledgement"})),
                )
                    .into_response()
            } else if path.ends_with("/merge") {
                Json(json!({"merged":true})).into_response()
            } else {
                StatusCode::CREATED.into_response()
            };
        }
        let sha = vendor.sha.clone();
        let conclusion = if vendor.passed { "success" } else { "failure" };
        let run = json!({"id":12,"run_attempt":vendor.attempt,"name":"Tests","head_sha":sha,"status":"completed","conclusion":conclusion,"check_suite_id":9,"html_url":"https://github.com/acme/app/actions/runs/12"});
        let value = if path.ends_with("/pulls/128") {
            json!({"title":"Session refresh","state":"open","merged":false,"draft":false,"mergeable":true,"head":{"sha":sha,"ref":"fix/session"},"base":{"ref":"main"},"html_url":"https://github.com/acme/app/pull/128"})
        } else if path.ends_with("/check-runs") {
            let count = if vendor.pages && query.split('&').any(|part| part == "page=1") {
                100
            } else {
                1
            };
            let checks:Vec<Value>=(0..count).map(|i|json!({"id":if query.split('&').any(|part| part == "page=2"){100+i}else{i},"name":format!("check-{i}"),"head_sha":sha,"status":"completed","conclusion":conclusion,"started_at":"2026-11-02T12:00:00Z","completed_at":"2026-11-02T12:00:33Z","check_suite":{"id":9},"output":{"summary":"Result","text":"Details"},"html_url":"https://github.com/acme/app/actions/runs/12"})).collect();
            json!({"check_runs":checks})
        } else if path.ends_with("/statuses") {
            json!([])
        } else if path.ends_with("/actions/runs") {
            json!({"workflow_runs":[run]})
        } else if path.ends_with("/actions/runs/12") {
            run
        } else {
            return StatusCode::NOT_FOUND.into_response();
        };
        Json(value).into_response()
    }
    struct Fixture {
        service: CiService,
        vendor: Arc<Mutex<Vendor>>,
        server: tokio::task::JoinHandle<()>,
        _temp: tempfile::TempDir,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    async fn fixture() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let db = DbHandle::new(temp.path().join("ci.db")).unwrap();
        let vendor = Arc::new(Mutex::new(Vendor {
            sha: "a".repeat(40),
            attempt: 1,
            ..Default::default()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(github).with_state(vendor.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Fixture {
            service: CiService::at(db, origin),
            vendor,
            server,
            _temp: temp,
        }
    }
    fn rerun() -> Action {
        Action {
            action: "rerun-failed".into(),
            head_sha: "a".repeat(40),
            run_id: Some(12),
            run_attempt: Some(1),
        }
    }
    #[tokio::test]
    async fn loads_paginated_facts_and_executes_only_one_reviewed_attempt() {
        let f = fixture().await;
        f.vendor.lock().unwrap().pages = true;
        let snapshot = f.service.snapshot("token", "acme/app", 128).await.unwrap();
        assert_eq!(snapshot["checks"].as_array().unwrap().len(), 101);
        assert_eq!(snapshot["complete"], true);
        let approval = f
            .service
            .prepare("user-a", "account-a", "token", "acme/app", 128, &rerun())
            .await
            .unwrap();
        assert!(f.vendor.lock().unwrap().writes.is_empty());
        let id = approval["id"].as_str().unwrap();
        assert_eq!(
            f.service
                .execute("user-b", "account-a", "token", "acme/app", 128, id)
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.service
                .execute("user-a", "account-b", "token", "acme/app", 128, id)
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.service
                .execute("user-a", "account-a", "token", "acme/app", 128, id)
                .await
                .unwrap()["state"],
            "submitted"
        );
        assert_eq!(
            f.service
                .execute("user-a", "account-a", "token", "acme/app", 128, id)
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            f.vendor.lock().unwrap().writes.as_slice(),
            &[(
                "POST".into(),
                "/repos/acme/app/actions/runs/12/rerun-failed-jobs".into()
            )]
        );
    }
    #[tokio::test]
    async fn a_force_push_or_new_run_attempt_invalidates_the_review() {
        let f = fixture().await;
        let approval = f
            .service
            .prepare("u", "c", "token", "acme/app", 128, &rerun())
            .await
            .unwrap();
        f.vendor.lock().unwrap().sha = "b".repeat(40);
        assert_eq!(
            f.service
                .execute(
                    "u",
                    "c",
                    "token",
                    "acme/app",
                    128,
                    approval["id"].as_str().unwrap()
                )
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        f.vendor.lock().unwrap().sha = "a".repeat(40);
        f.vendor.lock().unwrap().attempt = 2;
        assert_eq!(
            f.service
                .execute(
                    "u",
                    "c",
                    "token",
                    "acme/app",
                    128,
                    approval["id"].as_str().unwrap()
                )
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        assert!(f.vendor.lock().unwrap().writes.is_empty());
    }
    #[tokio::test]
    async fn ambiguous_vendor_outcome_is_durable_and_cannot_replay() {
        let f = fixture().await;
        let approval = f
            .service
            .prepare("u", "c", "token", "acme/app", 128, &rerun())
            .await
            .unwrap();
        f.vendor.lock().unwrap().fail_write = true;
        let id = approval["id"].as_str().unwrap();
        assert!(f
            .service
            .execute("u", "c", "token", "acme/app", 128, id)
            .await
            .is_err());
        assert!(f
            .service
            .execute("u", "c", "token", "acme/app", 128, id)
            .await
            .is_err());
        assert_eq!(f.vendor.lock().unwrap().writes.len(), 1);
        let stored: String = f
            .service
            .db
            .connect()
            .unwrap()
            .query_row(
                "SELECT state FROM live_ci_actions WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, "unknown");
    }
    #[tokio::test]
    async fn merge_review_requires_current_successful_checks() {
        let f = fixture().await;
        let merge = Action {
            action: "merge".into(),
            head_sha: "a".repeat(40),
            run_id: None,
            run_attempt: None,
        };
        assert!(f
            .service
            .prepare("u", "c", "token", "acme/app", 128, &merge)
            .await
            .is_err());
        f.vendor.lock().unwrap().passed = true;
        let approval = f
            .service
            .prepare("u", "c", "token", "acme/app", 128, &merge)
            .await
            .unwrap();
        assert_eq!(
            f.service
                .execute(
                    "u",
                    "c",
                    "token",
                    "acme/app",
                    128,
                    approval["id"].as_str().unwrap()
                )
                .await
                .unwrap()["action"],
            "merge"
        );
        assert_eq!(f.vendor.lock().unwrap().writes[0].0, "PUT");
    }
    #[tokio::test]
    async fn signed_out_route_does_not_load_another_users_github_account() {
        use tower::ServiceExt;
        let temp = tempfile::tempdir().unwrap();
        let state = crate::test_helpers::app_state(temp.path()).await;
        let response = router()
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/connectors/github/ci/acme/app/128")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
