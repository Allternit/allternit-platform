//! Motion cloud render: MP4 export for browsers that can't encode H.264 and
//! for clips too long for a tab. Child module of `artifacts_v2` so it reuses
//! caller resolution and access checks.
//!
//! * `POST   /api/v2/artifacts/:id/render`            start a job (`{version?}`)
//! * `GET    /api/v2/artifacts/:id/render/:job`       status and progress
//! * `GET    /api/v2/artifacts/:id/render/:job/file`  the MP4 (while it lasts)
//! * `DELETE /api/v2/artifacts/:id/render/:job`       cancel, or drop the file
//!
//! The renderer is the app's own `renderFrame` bundled for Node (see
//! `render/motion/README.md`); each job spawns `node dist/render.mjs` as a
//! child process that draws frames into `@napi-rs/canvas` and pipes them to
//! ffmpeg. The async runtime never waits on it: the child is a
//! `kill_on_drop` tokio process, killed on timeout or cancel. At most
//! [`rules::MAX_CONCURRENT`] children run; further jobs wait in `queued`.
//! Finished files go to R2 when it is configured (otherwise the host temp
//! dir) and are deleted after 24 hours. Rules live in
//! `artifacts::motion_render`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path as FsPath, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{Notify, Semaphore};

use super::{caller, fetch_version, load_with_access, org_settings, parse_json, require, ts, Result};
use crate::artifacts::access::{visible_version, Access};
use crate::artifacts::error::ArtifactError;
use crate::artifacts::motion_render::{self as rules, Refusal};
use crate::services::r2::R2Client;
use crate::services::user_files::BUCKET;
use crate::ApiState;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v2/artifacts/:id/render", post(create_job))
        .route("/api/v2/artifacts/:id/render/:job", get(get_job).delete(cancel_job))
        .route("/api/v2/artifacts/:id/render/:job/file", get(download))
}

// ---------------------------------------------------------------------------
// Host configuration
// ---------------------------------------------------------------------------

/// Where the renderer bundle, Node and ffmpeg are on this host.
#[derive(Debug, Clone)]
struct RunnerConfig {
    node: PathBuf,
    ffmpeg: PathBuf,
    /// The deployed `render/motion` folder (bundle + `node_modules`).
    dir: PathBuf,
    entry: PathBuf,
    work_root: PathBuf,
}

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn work_root() -> PathBuf {
    match std::env::var("ALLTERNIT_MOTION_RENDER_TMP").ok().filter(|v| !v.trim().is_empty()) {
        Some(v) => PathBuf::from(v),
        None => std::env::temp_dir().join("allternit-motion-render"),
    }
}

/// `None` when the bundle, its runtime package, Node or ffmpeg is missing.
fn runner_config() -> Option<RunnerConfig> {
    let dir = env_path("ALLTERNIT_MOTION_RENDER_DIR", "/opt/allternit-cloud-api/render/motion");
    let entry = dir.join("dist").join("render.mjs");
    let node = env_path("ALLTERNIT_NODE_BIN", "/usr/bin/node");
    let ffmpeg = env_path("ALLTERNIT_FFMPEG_BIN", "/usr/bin/ffmpeg");
    let ready = entry.is_file()
        && dir.join("node_modules").join("@napi-rs").is_dir()
        && node.is_file()
        && ffmpeg.is_file();
    ready.then(|| RunnerConfig { node, ffmpeg, dir, entry, work_root: work_root() })
}

// ---------------------------------------------------------------------------
// Process-wide state: slots, cancel handles, background sweeps
// ---------------------------------------------------------------------------

fn slots() -> &'static Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(rules::MAX_CONCURRENT)))
}

fn cancels() -> &'static Mutex<HashMap<String, Arc<Notify>>> {
    static CANCELS: OnceLock<Mutex<HashMap<String, Arc<Notify>>>> = OnceLock::new();
    CANCELS.get_or_init(Default::default)
}

/// First request after a start: fail jobs the previous process left
/// `queued`/`running`, then sweep expired files every ten minutes.
fn ensure_background(db: &PgPool) {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let db = db.clone();
    tokio::spawn(async move {
        if let Err(error) = recover(&db).await {
            tracing::warn!(%error, "motion render: recovery failed");
        }
        loop {
            if let Err(error) = sweep(&db).await {
                tracing::warn!(%error, "motion render: sweep failed");
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}

async fn recover(db: &PgPool) -> std::result::Result<(), sqlx::Error> {
    let ids: Vec<String> = sqlx::query_scalar(
        "UPDATE motion_render_jobs SET status = 'failed', error_code = 'render_interrupted', \
         error_message = 'The server restarted while this video was rendering. Start the export again.', \
         finished_at = now() WHERE status IN ('queued', 'running') RETURNING id",
    )
    .fetch_all(db)
    .await?;
    let root = work_root();
    for id in ids {
        if let Some(dir) = rules::job_dir(&root, &id) {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }
    Ok(())
}

#[derive(FromRow)]
struct ExpiredRow {
    id: String,
    storage: Option<String>,
    output_key: Option<String>,
}

/// Delete files past their 24 hours, then any job directory older than a day
/// that no row cleaned up.
async fn sweep(db: &PgPool) -> std::result::Result<(), sqlx::Error> {
    let expired: Vec<ExpiredRow> = sqlx::query_as(
        "SELECT id, storage, output_key FROM motion_render_jobs \
         WHERE output_key IS NOT NULL AND expires_at < now() LIMIT 200",
    )
    .fetch_all(db)
    .await?;
    for row in expired {
        if remove_output(&row.id, row.storage.as_deref(), row.output_key.as_deref()).await {
            sqlx::query("UPDATE motion_render_jobs SET status = 'expired', output_key = NULL WHERE id = $1")
                .bind(&row.id)
                .execute(db)
                .await?;
        }
    }
    let root = work_root();
    if let Ok(mut entries) = tokio::fs::read_dir(&root).await {
        let cutoff = Duration::from_secs(((rules::FILE_TTL_HOURS + 2) * 3600) as u64);
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let old = entry
                .metadata()
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|age| age > cutoff);
            if old && rules::valid_job_id(&name) {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
    Ok(())
}

/// Delete a stored output. `true` when it is gone (or never was there).
async fn remove_output(job_id: &str, storage: Option<&str>, key: Option<&str>) -> bool {
    match (storage, key) {
        (Some("r2"), Some(key)) => match R2Client::from_env() {
            Ok(r2) => r2.delete(BUCKET, key).await.is_ok(),
            Err(_) => false,
        },
        (Some("local"), _) => {
            if let Some(dir) = rules::job_dir(&work_root(), job_id) {
                let _ = tokio::fs::remove_dir_all(dir).await;
            }
            true
        }
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Rows and JSON
// ---------------------------------------------------------------------------

#[derive(Debug, FromRow)]
struct JobRow {
    id: String,
    artifact_id: String,
    version: i32,
    #[allow(dead_code)]
    user_id: String,
    status: String,
    progress: f32,
    width: i32,
    height: i32,
    fps: i32,
    frames: i32,
    error_code: Option<String>,
    error_message: Option<String>,
    storage: Option<String>,
    output_key: Option<String>,
    output_bytes: Option<i64>,
    created_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
}

const JOB_COLUMNS: &str = "id, artifact_id, version, user_id, status, progress, width, height, fps, frames, \
    error_code, error_message, storage, output_key, output_bytes, created_at, started_at, finished_at, expires_at";

fn job_json(job: &JobRow, ahead: i64) -> Value {
    let done = job.status == "done" && job.output_key.is_some();
    json!({
        "id": job.id,
        "artifact_id": job.artifact_id,
        "version": job.version,
        "status": job.status,
        "progress": job.progress,
        "ahead": if job.status == "queued" { ahead } else { 0 },
        "width": job.width,
        "height": job.height,
        "fps": job.fps,
        "frames": job.frames,
        "error": job.error_code.as_ref().map(|code| json!({ "code": code, "message": job.error_message })),
        "file_url": done.then(|| format!("/api/v2/artifacts/{}/render/{}/file", job.artifact_id, job.id)),
        "bytes": job.output_bytes,
        "created_at": ts(job.created_at),
        "started_at": job.started_at.map(ts),
        "finished_at": job.finished_at.map(ts),
        "expires_at": job.expires_at.map(ts),
    })
}

fn refusal(r: Refusal) -> ArtifactError {
    ArtifactError::coded(
        StatusCode::from_u16(r.status).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY),
        r.code,
        r.message,
    )
}

fn job_not_found() -> ArtifactError {
    ArtifactError::Api(crate::ApiError::NotFound("Render job not found".to_string()))
}

async fn fetch_job(db: &PgPool, artifact_id: &str, job_id: &str, user_id: &str) -> Result<JobRow> {
    if !rules::valid_job_id(job_id) {
        return Err(job_not_found());
    }
    sqlx::query_as::<_, JobRow>(&format!(
        "SELECT {JOB_COLUMNS} FROM motion_render_jobs WHERE id = $1 AND artifact_id = $2 AND user_id = $3"
    ))
    .bind(job_id)
    .bind(artifact_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(job_not_found)
}

/// Jobs ahead of this one: queued or running, created earlier.
async fn jobs_ahead(db: &PgPool, job: &JobRow) -> Result<i64> {
    if job.status != "queued" {
        return Ok(0);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM motion_render_jobs WHERE status IN ('queued', 'running') AND created_at < $1",
    )
    .bind(job.created_at)
    .fetch_one(db)
    .await?)
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    version: Option<i32>,
}

async fn create_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>)> {
    ensure_background(&state.db);
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::View, "Rendering")?;
    if row.kind != "motion" {
        return Err(ArtifactError::unprocessable("not_motion", "Only motion artifacts can be rendered to video."));
    }
    let req: CreateRequest =
        if body.iter().all(u8::is_ascii_whitespace) { CreateRequest::default() } else { parse_json(&body)? };

    // Viewers and commenters render the version they can see.
    let visible = visible_version(access, row.current_version, row.shared_version);
    let version = match req.version {
        Some(v) if access.sees_all_versions() || v == visible => v,
        Some(_) => return Err(ArtifactError::Api(crate::ApiError::NotFound("Version not found".to_string()))),
        None => visible,
    };

    let settings = match row.org_id.as_deref() {
        Some(org) => Some(org_settings(&state.db, org).await?),
        None => None,
    };
    rules::org_allows(settings.as_ref()).map_err(refusal)?;
    let plan = crate::routes::billing_subscriptions::open_subscription_plan(&state.db, &caller.id).await?;
    rules::plan_allows(plan.as_deref(), caller.org_id.is_some()).map_err(refusal)?;

    let found = fetch_version(&state.db, &row.id, version)
        .await?
        .ok_or_else(|| ArtifactError::Api(crate::ApiError::NotFound("Version not found".to_string())))?;
    let spec = rules::inspect(&found.body).map_err(refusal)?;
    rules::check_limits(&spec).map_err(refusal)?;

    let Some(cfg) = runner_config() else {
        tracing::error!("motion render: bundle, node or ffmpeg missing on this host");
        return Err(ArtifactError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "render_unavailable",
            "Cloud video export isn’t available right now. Try again later, or export from Allternit Desktop.",
        ));
    };

    // The per-user count and the insert share one transaction behind an
    // advisory lock, so two quick taps can't both pass the limit.
    let job_id = rules::new_job_id();
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("motion-render:{}", caller.id))
        .execute(&mut *tx)
        .await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM motion_render_jobs WHERE user_id = $1 AND status IN ('queued', 'running')",
    )
    .bind(&caller.id)
    .fetch_one(&mut *tx)
    .await?;
    rules::check_queue(active).map_err(refusal)?;
    let job = sqlx::query_as::<_, JobRow>(&format!(
        "INSERT INTO motion_render_jobs (id, artifact_id, version, user_id, width, height, fps, frames) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {JOB_COLUMNS}"
    ))
    .bind(&job_id)
    .bind(&row.id)
    .bind(version)
    .bind(&caller.id)
    .bind(spec.width as i32)
    .bind(spec.height as i32)
    .bind(spec.fps as i32)
    .bind(spec.frames as i32)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let cancel = Arc::new(Notify::new());
    cancels().lock().expect("cancel registry").insert(job_id.clone(), cancel.clone());
    tokio::spawn(run_job(state.db.clone(), cfg, job_id, caller.id.clone(), found.body, cancel));

    let ahead = jobs_ahead(&state.db, &job).await?;
    Ok((StatusCode::ACCEPTED, Json(job_json(&job, ahead))))
}

async fn get_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Json<Value>> {
    ensure_background(&state.db);
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::View, "Rendering")?;
    let job = fetch_job(&state.db, &row.id, &job_id, &caller.id).await?;
    let ahead = jobs_ahead(&state.db, &job).await?;
    Ok(Json(job_json(&job, ahead)))
}

async fn download(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Response> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::View, "Downloading")?;
    let job = fetch_job(&state.db, &row.id, &job_id, &caller.id).await?;
    let gone = || ArtifactError::coded(StatusCode::GONE, "render_expired", "This video was deleted after 24 hours. Export it again.");
    let (Some(key), true) = (job.output_key.as_deref(), job.status == "done") else {
        return Err(if job.status == "expired" {
            gone()
        } else {
            ArtifactError::coded(StatusCode::CONFLICT, "render_not_ready", "This video isn’t ready yet.")
        });
    };
    if job.expires_at.is_some_and(|at| at < Utc::now()) {
        return Err(gone());
    }

    let (body, len) = match job.storage.as_deref() {
        Some("r2") => {
            let r2 = R2Client::from_env().map_err(|_| storage_unavailable())?;
            let bytes = r2.get(BUCKET, key).await.map_err(|_| storage_unavailable())?.ok_or_else(gone)?;
            let len = bytes.len() as u64;
            (Body::from(bytes), len)
        }
        _ => {
            let root = work_root();
            let path = rules::job_dir(&root, &job.id)
                .map(|dir| dir.join(rules::OUTPUT_NAME))
                .filter(|p| rules::is_inside(&root, p))
                .ok_or_else(job_not_found)?;
            let file = tokio::fs::File::open(&path).await.map_err(|_| gone())?;
            let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);
            (stream_file(file), len)
        }
    };
    let mut response = Response::new(body);
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file_name(&row.title))) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

fn storage_unavailable() -> ArtifactError {
    ArtifactError::coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "render_unavailable",
        "The video couldn’t be fetched right now. Try the download again in a moment.",
    )
}

fn stream_file(file: tokio::fs::File) -> Body {
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buf)), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    });
    Body::from_stream(stream)
}

/// ASCII-only attachment name from the artifact title.
fn file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { ' ' })
        .collect();
    let words = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let base: String = words.chars().take(80).collect();
    format!("{}.mp4", if base.is_empty() { "motion" } else { base.as_str() })
}

async fn cancel_job(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<StatusCode> {
    let caller = caller(&state, &headers).await?;
    let (row, access) = load_with_access(&state.db, &id, &caller).await?;
    require(access, Access::View, "Rendering")?;
    let job = fetch_job(&state.db, &row.id, &job_id, &caller.id).await?;
    match job.status.as_str() {
        "queued" | "running" => {
            sqlx::query(
                "UPDATE motion_render_jobs SET status = 'canceled', finished_at = now() \
                 WHERE id = $1 AND status IN ('queued', 'running')",
            )
            .bind(&job.id)
            .execute(&state.db)
            .await?;
            if let Some(cancel) = cancels().lock().expect("cancel registry").get(&job.id) {
                cancel.notify_one();
            }
        }
        "done" => {
            if remove_output(&job.id, job.storage.as_deref(), job.output_key.as_deref()).await {
                sqlx::query("UPDATE motion_render_jobs SET status = 'expired', output_key = NULL WHERE id = $1")
                    .bind(&job.id)
                    .execute(&state.db)
                    .await?;
            }
        }
        _ => {}
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// The job: wait for a slot, run the renderer, store the file
// ---------------------------------------------------------------------------

enum Outcome {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Canceled,
}

async fn run_job(db: PgPool, cfg: RunnerConfig, job_id: String, user_id: String, body: String, cancel: Arc<Notify>) {
    let result = run_job_inner(&db, &cfg, &job_id, &user_id, &body, &cancel).await;
    cancels().lock().expect("cancel registry").remove(&job_id);
    if let Err(error) = result {
        tracing::error!(%error, job = %job_id, "motion render: job failed");
        fail(&db, &job_id, "render_failed", "The video couldn’t be rendered. Try again, or export from Allternit Desktop.").await;
    }
}

async fn fail(db: &PgPool, job_id: &str, code: &str, message: &str) {
    let _ = sqlx::query(
        "UPDATE motion_render_jobs SET status = 'failed', error_code = $2, error_message = $3, finished_at = now() \
         WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(job_id)
    .bind(code)
    .bind(message)
    .execute(db)
    .await;
    if let Some(dir) = rules::job_dir(&work_root(), job_id) {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

async fn run_job_inner(
    db: &PgPool,
    cfg: &RunnerConfig,
    job_id: &str,
    user_id: &str,
    body: &str,
    cancel: &Notify,
) -> std::result::Result<(), String> {
    // Wait for one of the render slots (FIFO), unless the job is canceled first.
    let _slot = tokio::select! {
        _ = cancel.notified() => return Ok(()),
        permit = slots().clone().acquire_owned() => permit.map_err(|e| e.to_string())?,
    };
    let started = sqlx::query(
        "UPDATE motion_render_jobs SET status = 'running', started_at = now() WHERE id = $1 AND status = 'queued'",
    )
    .bind(job_id)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    if started.rows_affected() == 0 {
        return Ok(()); // canceled while queued
    }

    let dir = rules::job_dir(&cfg.work_root, job_id).ok_or("bad job id")?;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let input = dir.join(rules::INPUT_NAME);
    let output = dir.join(rules::OUTPUT_NAME);
    tokio::fs::write(&input, body).await.map_err(|e| e.to_string())?;

    let mut child = Command::new(&cfg.node)
        .arg("--max-old-space-size=2048")
        .arg(&cfg.entry)
        .arg("--in")
        .arg(&input)
        .arg("--out")
        .arg(&output)
        .arg("--ffmpeg")
        .arg(&cfg.ffmpeg)
        .current_dir(&cfg.dir)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("NODE_ENV", "production")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn node: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let stderr = child.stderr.take().ok_or("no stderr")?;
    // Keep the last few stderr lines for the log.
    let stderr_tail = tokio::spawn(async move {
        let mut tail: VecDeque<String> = VecDeque::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tail.len() == 12 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let outcome = tokio::select! {
        _ = cancel.notified() => Outcome::Canceled,
        r = tokio::time::timeout(Duration::from_secs(rules::JOB_TIMEOUT_SECS), drive(db, job_id, &mut child, stdout)) => match r {
            Ok(status) => Outcome::Exited(status),
            Err(_) => Outcome::TimedOut,
        },
    };

    match outcome {
        Outcome::Canceled => {
            let _ = child.kill().await;
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Ok(());
        }
        Outcome::TimedOut => {
            let _ = child.kill().await;
            fail(
                db,
                job_id,
                "render_timeout",
                "This video took longer than 10 minutes to render, so it was stopped. Shorten it or lower the frame rate (ask Gizzi), or export from Allternit Desktop.",
            )
            .await;
            return Ok(());
        }
        Outcome::Exited(status) => {
            let tail = stderr_tail.await.unwrap_or_default();
            let status = status.map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("renderer exited with {status}: {}", tail.trim()));
            }
        }
    }

    let size = tokio::fs::metadata(&output).await.map_err(|e| format!("no output: {e}"))?.len();
    if size == 0 {
        return Err("renderer wrote an empty file".into());
    }
    if size > rules::MAX_OUTPUT_BYTES {
        fail(db, job_id, "render_too_large", "The finished video is larger than the 256 MB limit. Shorten it or ask Gizzi for a smaller size.").await;
        return Ok(());
    }
    let (storage, key) = store_output(&output, user_id, job_id, &dir).await;
    let done = sqlx::query(
        "UPDATE motion_render_jobs SET status = 'done', progress = 1, storage = $2, output_key = $3, output_bytes = $4, \
         finished_at = now(), expires_at = now() + make_interval(hours => $5) WHERE id = $1 AND status = 'running'",
    )
    .bind(job_id)
    .bind(storage)
    .bind(&key)
    .bind(size as i64)
    .bind(rules::FILE_TTL_HOURS as i32)
    .execute(db)
    .await
    .map_err(|e| e.to_string())?;
    if done.rows_affected() == 0 {
        // Canceled while finishing: nobody will download it.
        remove_output(job_id, Some(storage), Some(&key)).await;
    }
    Ok(())
}

/// Reads the child's `progress <0..1>` lines, writes them to the row about
/// once per 2%, then waits for the child to exit.
async fn drive(
    db: &PgPool,
    job_id: &str,
    child: &mut Child,
    stdout: ChildStdout,
) -> std::io::Result<std::process::ExitStatus> {
    let mut lines = BufReader::new(stdout).lines();
    let mut written = 0.0_f32;
    while let Some(line) = lines.next_line().await? {
        let Some(p) = line.strip_prefix("progress ").and_then(|v| v.trim().parse::<f32>().ok()) else { continue };
        let p = p.clamp(0.0, 0.99);
        if p - written >= 0.02 {
            written = p;
            let _ = sqlx::query("UPDATE motion_render_jobs SET progress = $2 WHERE id = $1 AND status = 'running'")
                .bind(job_id)
                .bind(p)
                .execute(db)
                .await;
        }
    }
    child.wait().await
}

/// Put the finished file in R2 when it is configured; otherwise (or when the
/// upload fails) it stays in the job's temp dir. Returns `(storage, key)`.
async fn store_output(output: &FsPath, user_id: &str, job_id: &str, dir: &FsPath) -> (&'static str, String) {
    if let (Ok(r2), Some(key)) = (R2Client::from_env(), rules::output_key(user_id, job_id)) {
        match tokio::fs::read(output).await {
            Ok(bytes) => match r2.put(BUCKET, &key, bytes, "video/mp4").await {
                Ok(()) => {
                    let _ = tokio::fs::remove_dir_all(dir).await;
                    return ("r2", key);
                }
                Err(error) => tracing::warn!(%error, job = %job_id, "motion render: R2 upload failed; keeping the file on the host"),
            },
            Err(error) => tracing::warn!(%error, job = %job_id, "motion render: couldn’t read the output for R2"),
        }
    }
    ("local", rules::OUTPUT_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_are_ascii_and_bounded() {
        assert_eq!(file_name("Q3 in numbers"), "Q3 in numbers.mp4");
        assert_eq!(file_name("../../etc/passwd"), "etc passwd.mp4");
        assert_eq!(file_name("héllo \"x\"\r\n"), "h llo x.mp4");
        assert_eq!(file_name("🎬"), "motion.mp4");
        assert!(file_name(&"a".repeat(500)).len() <= 84);
    }

    #[test]
    fn job_json_shows_the_file_only_when_done() {
        let now = Utc::now();
        let mut job = JobRow {
            id: rules::new_job_id(),
            artifact_id: "art_X".into(),
            version: 3,
            user_id: "user_1".into(),
            status: "running".into(),
            progress: 0.4,
            width: 1920,
            height: 1080,
            fps: 30,
            frames: 90,
            error_code: None,
            error_message: None,
            storage: None,
            output_key: None,
            output_bytes: None,
            created_at: now,
            started_at: Some(now),
            finished_at: None,
            expires_at: None,
        };
        assert!(job_json(&job, 0)["file_url"].is_null());
        job.status = "done".into();
        job.output_key = Some("out.mp4".into());
        let v = job_json(&job, 0);
        assert_eq!(v["file_url"], format!("/api/v2/artifacts/art_X/render/{}/file", job.id));
        assert!(v["error"].is_null());
    }

    #[test]
    fn refusals_keep_their_status_and_code() {
        let r = rules::check_queue(3).unwrap_err();
        match refusal(r) {
            ArtifactError::Coded { status, code, .. } => {
                assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(code, "render_limit_reached");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
