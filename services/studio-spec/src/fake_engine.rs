//! A tiny engine API v1 server. It validates the posted job spec, then walks
//! the job queued -> preparing -> running -> exporting -> succeeded with
//! events, so other lanes can build a host, a UI or a CLI against it.
//!
//! Bearer token on every route. One job at a time (409 `job_running`).

use crate::{engines, ApiError, Job, JobEvent, JobOutput, JobSpec, JobState};
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Rec {
    job: Job,
    events: Vec<JobEvent>,
}

#[derive(Clone)]
struct App {
    token: Arc<String>,
    delay: Duration,
    jobs: Arc<Mutex<HashMap<String, Rec>>>,
    next: Arc<Mutex<u64>>,
}

fn err(code: StatusCode, error: &str, message: impl Into<String>) -> Response {
    (code, Json(ApiError { error: error.into(), message: message.into() })).into_response()
}

fn push(rec: &mut Rec, kind: &str, message: &str, progress: Option<f64>) {
    rec.job.last_seq += 1;
    rec.job.updated_at = Some(Utc::now());
    rec.events.push(JobEvent {
        seq: rec.job.last_seq,
        at: Utc::now(),
        kind: kind.into(),
        message: message.into(),
        progress,
    });
}

/// Move to `to` (and log it). Returns false when the job already finished
/// (cancelled), which stops the walker.
fn advance(app: &App, id: &str, to: JobState, progress: Option<f64>) -> bool {
    let mut jobs = app.jobs.lock().unwrap();
    let Some(rec) = jobs.get_mut(id) else { return false };
    match rec.job.state.transition(to) {
        Ok(s) => {
            rec.job.state = s;
            push(rec, "state", &format!("{s:?}").to_lowercase(), progress);
            true
        }
        Err(_) => false,
    }
}

fn log(app: &App, id: &str, kind: &str, msg: &str, progress: Option<f64>) {
    if let Some(rec) = app.jobs.lock().unwrap().get_mut(id) {
        if !rec.job.state.is_terminal() {
            push(rec, kind, msg, progress);
        }
    }
}

async fn walk(app: App, id: String) {
    let pause = || tokio::time::sleep(app.delay);
    pause().await;
    if !advance(&app, &id, JobState::Preparing, None) { return; }
    pause().await;
    if !advance(&app, &id, JobState::Running, Some(0.0)) { return; }
    for step in 1..=3u32 {
        pause().await;
        let p = f64::from(step) / 3.0;
        log(&app, &id, "progress", &format!("step {step}/3 loss={:.3}", 2.0 / f64::from(step + 1)), Some(p));
    }
    pause().await;
    if !advance(&app, &id, JobState::Exporting, None) { return; }
    pause().await;
    {
        let mut jobs = app.jobs.lock().unwrap();
        if let Some(rec) = jobs.get_mut(&id) {
            if rec.job.state != JobState::Exporting { return; }
            rec.job.outputs.push(JobOutput { kind: "adapter".into(), path: format!("outputs/{id}/adapter"), bytes: Some(1024) });
            push(rec, "output", "adapter written", None);
        }
    }
    advance(&app, &id, JobState::Succeeded, Some(1.0));
}

async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {}", app.token))
        .unwrap_or(false);
    if ok { next.run(req).await } else { err(StatusCode::UNAUTHORIZED, "unauthorized", "missing or wrong bearer token") }
}

async fn engine_info() -> Response {
    let mut e = engines::find("allternit-tune").expect("catalog has allternit-tune");
    e.id = "fake-engine".into();
    e.name = "Fake engine".into();
    e.install_state = crate::InstallState::Installed;
    Json(e).into_response()
}

async fn create(State(app): State<App>, body: String) -> Response {
    let spec = match JobSpec::parse(&body) {
        Ok(s) => s,
        Err(errs) => {
            let first = &errs[0];
            let message = errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
            return err(StatusCode::UNPROCESSABLE_ENTITY, &first.code, message);
        }
    };
    let mut jobs = app.jobs.lock().unwrap();
    if jobs.values().any(|r| !r.job.state.is_terminal()) {
        return err(StatusCode::CONFLICT, "job_running", "a job is already running on this engine");
    }
    let id = {
        let mut n = app.next.lock().unwrap();
        *n += 1;
        format!("job_{:04}", *n)
    };
    let mut rec = Rec {
        job: Job {
            id: id.clone(), state: JobState::Queued, spec,
            created_at: Utc::now(), updated_at: None, error: None,
            last_seq: 0, outputs: vec![],
        },
        events: vec![],
    };
    push(&mut rec, "state", "queued", Some(0.0));
    let snapshot = rec.job.clone();
    jobs.insert(id.clone(), rec);
    drop(jobs);
    tokio::spawn(walk(app.clone(), id));
    (StatusCode::CREATED, Json(snapshot)).into_response()
}

async fn get_job(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.jobs.lock().unwrap().get(&id) {
        Some(r) => Json(r.job.clone()).into_response(),
        None => err(StatusCode::NOT_FOUND, "not_found", "no such job"),
    }
}

#[derive(Deserialize)]
struct After { after: Option<u64> }

async fn events(State(app): State<App>, Path(id): Path<String>, Query(q): Query<After>) -> Response {
    match app.jobs.lock().unwrap().get(&id) {
        Some(r) => {
            let after = q.after.unwrap_or(0);
            let evs: Vec<&JobEvent> = r.events.iter().filter(|e| e.seq > after).collect();
            Json(serde_json::json!({ "state": r.job.state, "events": evs })).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "not_found", "no such job"),
    }
}

async fn cancel(State(app): State<App>, Path(id): Path<String>) -> Response {
    let mut jobs = app.jobs.lock().unwrap();
    let Some(rec) = jobs.get_mut(&id) else {
        return err(StatusCode::NOT_FOUND, "not_found", "no such job");
    };
    match rec.job.state.transition(JobState::Cancelled) {
        Ok(s) => {
            rec.job.state = s;
            push(rec, "state", "cancelled", None);
            Json(rec.job.clone()).into_response()
        }
        Err(e) => err(StatusCode::CONFLICT, "already_finished", e.to_string()),
    }
}

/// Router for engine API v1. `step_delay` is the pause between steps.
pub fn router(token: &str, step_delay: Duration) -> Router {
    let app = App {
        token: Arc::new(token.to_string()),
        delay: step_delay,
        jobs: Arc::default(),
        next: Arc::default(),
    };
    Router::new()
        .route("/v1/engine", get(engine_info))
        .route("/v1/jobs", post(create))
        .route("/v1/jobs/:id", get(get_job))
        .route("/v1/jobs/:id/events", get(events))
        .route("/v1/jobs/:id/cancel", post(cancel))
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .with_state(app)
}

/// A running fake engine on a free loopback port.
pub struct FakeEngine {
    pub addr: SocketAddr,
    pub token: String,
    task: tokio::task::JoinHandle<()>,
}

impl FakeEngine {
    pub async fn start(token: &str, step_delay: Duration) -> std::io::Result<FakeEngine> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let app = router(token, step_delay);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(FakeEngine { addr, token: token.to_string(), task })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.task.abort();
    }
}
