//! Test harness for integration tests
//!
//! Provides a TestApp struct for setting up test environment
//! with Postgres database and HTTP client.

use allternit_cloud_api::{
    create_rate_limiter, create_router, model_router, runtime, routes, services, ApiState, RateLimitConfig,
    db::cowork_models::*,
};
use axum::{body::Body, http::Request, http::StatusCode, response::Response};
use sqlx::PgPool;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tower::ServiceExt;

/// Test application wrapper
pub struct TestApp {
    pub db: PgPool,
    pub router: axum::Router,
    pub temp_dir: TempDir,
    pub event_tx: broadcast::Sender<allternit_cloud_api::DeploymentEvent>,
    /// The API state behind `router`, for tests that drive background
    /// services (e.g. the scheduler) directly.
    pub state: Arc<ApiState>,
    /// Per-test schema this app's tables live in. Dropped (CASCADE) when the
    /// TestApp is dropped so test runs do not accumulate leftover schemas.
    schema: String,
}

impl TestApp {
    /// Create a new test application with Postgres database
    pub async fn new() -> Self {
        Self::with_mesh(None).await
    }

    /// [`Self::new`] with a mesh enrollment service (e.g. over a fake
    /// Headscale), for routes that hand out mesh keys.
    pub async fn with_mesh(mesh_service: Option<Arc<routes::mesh::MeshService>>) -> Self {
        // Create temp directory for any file-based test artifacts
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        // Initialize database
        let (db, schema) = Self::init_test_db().await;

        // Create broadcast channel for events
        let (event_tx, _event_rx) = broadcast::channel::<allternit_cloud_api::DeploymentEvent>(100);

        // Run tests in development mode so the auth middleware lets requests
        // through without tokens.
        std::env::set_var("Allternit_API_DEVELOPMENT_MODE", "true");

        // Create shared services (mirrors the wiring in main.rs)
        let event_store: Arc<dyn services::EventStore> =
            Arc::new(services::EventStoreImpl::new(db.clone()));
        let session_manager = Arc::new(runtime::session_manager::SessionManager::new(db.clone()));
        let run_service: Arc<dyn services::RunService> =
            Arc::new(services::RunServiceImpl::new(db.clone()).with_event_store(event_store.clone()));
        let cost_service: Arc<dyn services::CostService> =
            Arc::new(services::CostServiceImpl::new(db.clone()));
        let quota_service = Arc::new(services::QuotaService::new(db.clone()));

        // Generous rate limits so test traffic is never throttled
        let rate_limit_config = RateLimitConfig {
            requests_per_minute: 100_000,
            window: std::time::Duration::from_secs(60),
        };
        let rate_limiter = create_rate_limiter(rate_limit_config.clone());
        let public_rate_limiter = create_rate_limiter(rate_limit_config.clone());
        let free_inference_rate_limiter = create_rate_limiter(rate_limit_config);

        // Create API state
        let state = Arc::new(ApiState {
            db: db.clone(),
            ssh_executor: allternit_cloud_ssh::SshExecutor::new(),
            event_tx: event_tx.clone(),
            event_store,
            run_service,
            session_manager,
            rate_limiter,
            public_rate_limiter,
            free_inference_rate_limiter,
            cost_service,
            quota_service: quota_service.clone(),
            contabo_runtime_service: Arc::new(services::ContaboRuntimeService::new(
                db.clone(),
                None,
                "https://api.allternit.com".to_string(),
            )),
            data_plane_gateway: Arc::new(routes::data_plane::PgDataPlaneGateway::new(
                db.clone(),
                Arc::new(services::ContaboRuntimeService::new(
                    db.clone(),
                    None,
                    "https://api.allternit.com".to_string(),
                )),
                quota_service.clone(),
                Arc::new(services::ProvisioningService::new(db.clone())),
            )),
            provisioning_service: Arc::new(services::ProvisioningService::new(db.clone())),
            mesh_service,
            credential_cipher: None,
            inference_key_service: None,
            metrics_state: Arc::new(allternit_cloud_api::middleware::metrics::MetricsState::new()),
            model_router: model_router::ModelRouter::disabled(model_router::catalog::starter_catalog()),
            inference_pool_service: Arc::new(services::InferencePoolService::new(db.clone())),
        });

        // Create router
        let router = create_router(state.clone());

        Self {
            db,
            router,
            temp_dir,
            event_tx,
            state,
            schema,
        }
    }

    /// Initialize test database with migrations
    ///
    /// Migrations run in a fresh, uniquely-named schema (`it_<uuid>`) on the
    /// shared test database, and every connection is pinned to that schema
    /// via `search_path` — no `public` fallback — so parallel test binaries
    /// each see only their own tables and enum types.
    ///
    /// The embedded `sqlx::migrate!` path is deliberately NOT used here: all
    /// migrations_pg DDL is `public.`-qualified (pg_dump style), which would
    /// land every object in the shared `public` schema regardless of
    /// search_path. Instead [`Self::apply_migrations`] reads the files at
    /// runtime and rewrites them to be schema-agnostic (see its docs). The
    /// legacy SQLite-dialect `migrations/` tree this replaced cannot run
    /// against Postgres at all.
    async fn init_test_db() -> (PgPool, String) {
        let database_url = test_database_url();

        let schema = format!("it_{}", uuid::Uuid::new_v4().simple());
        let schema_for_hook = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |conn, _meta| {
                let schema = schema_for_hook.clone();
                Box::pin(async move {
                    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    // Schema only. The migration DDL is rewritten to drop its
                    // `public.` qualifiers, so tables and enum types are
                    // created inside the per-test schema and unqualified
                    // casts in app queries (`$1::runmode`) resolve there.
                    sqlx::query(&format!("SET search_path TO {}", schema))
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("Failed to connect to test database");

        Self::apply_migrations(&pool).await;

        (pool, schema)
    }

    /// Apply `migrations_pg/` to the per-test schema.
    ///
    /// Each file is read from `CARGO_MANIFEST_DIR/migrations_pg` at runtime
    /// (tests run with the crate root as the working directory's anchor, and
    /// `env!` bakes the path in at compile time), rewritten, split into
    /// statements, and executed in order:
    ///
    ///   * `n.nspname = 'public'` → `n.nspname = current_schema()` — the
    ///     existence guards inside the DO blocks must look in the fresh
    ///     schema, not at the shared `public` objects, or they skip creation
    ///     and unqualified enum casts fail with `type "runmode" does not
    ///     exist`;
    ///   * `public.` qualifiers are stripped, so CREATE TABLE / CREATE TYPE /
    ///     ALTER TABLE land wherever search_path points (the per-test schema).
    ///
    /// No `_sqlx_migrations` bookkeeping: the schema is unique per run and
    /// dropped on teardown, so there is no migration history to track — the
    /// schema itself is the isolation.
    async fn apply_migrations(pool: &PgPool) {
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.push("migrations_pg");
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("read migrations_pg directory")
            .map(|entry| entry.expect("migrations_pg entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        entries.sort();

        for path in entries {
            let sql = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let rewritten = sql
                .replace("n.nspname = 'public'", "n.nspname = current_schema()")
                .replace("public.", "");
            for statement in split_sql_statements(&rewritten) {
                sqlx::query(&statement)
                    .execute(pool)
                    .await
                    .unwrap_or_else(|e| panic!("apply {}: {e}", path.display()));
            }
        }
    }

    /// Make a GET request
    pub async fn get(&self, path: &str) -> Response {
        let request = Request::builder()
            .uri(path)
            .method("GET")
            .body(Body::empty())
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Make a POST request with JSON body
    pub async fn post<T: serde::Serialize>(&self, path: &str, body: T) -> Response {
        let json_body = serde_json::to_string(&body).unwrap();
        let request = Request::builder()
            .uri(path)
            .method("POST")
            .header("Content-Type", "application/json")
            .body(Body::from(json_body))
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Make a PUT request with JSON body
    pub async fn put<T: serde::Serialize>(&self, path: &str, body: T) -> Response {
        let json_body = serde_json::to_string(&body).unwrap();
        let request = Request::builder()
            .uri(path)
            .method("PUT")
            .header("Content-Type", "application/json")
            .body(Body::from(json_body))
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Make a DELETE request
    pub async fn delete(&self, path: &str) -> Response {
        let request = Request::builder()
            .uri(path)
            .method("DELETE")
            .body(Body::empty())
            .unwrap();

        self.router.clone().oneshot(request).await.unwrap()
    }

    /// Parse response body as JSON
    pub async fn parse_json<T: serde::de::DeserializeOwned>(response: Response) -> T {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        serde_json::from_slice(&body)
            .unwrap_or_else(|e| {
                let body_str = String::from_utf8_lossy(&body);
                panic!(
                    "Failed to parse JSON response (status: {}): {}\nBody: {}",
                    status, e, body_str
                )
            })
    }

    /// Assert that response status is success (2xx)
    pub fn assert_success(response: &Response) -> StatusCode {
        let status = response.status();
        assert!(
            status.is_success(),
            "Expected success status, got {:?}",
            status
        );
        status
    }

    /// Assert that response status equals expected
    pub fn assert_status(response: &Response, expected: StatusCode) -> StatusCode {
        let status = response.status();
        assert_eq!(
            status, expected,
            "Expected status {:?}, got {:?}",
            expected, status
        );
        status
    }

    // ============================================================================
    // Run Helpers
    // ============================================================================

    /// Create a new run
    pub async fn create_run(&self, name: &str) -> Run {
        let request = CreateRunRequest {
            name: name.to_string(),
            description: Some(format!("Test run: {}", name)),
            mode: RunMode::Local,
            config: RunConfig {
                command: Some("echo".to_string()),
                args: Some(vec!["hello".to_string()]),
                ..Default::default()
            },
            auto_start: false,
            region_id: None,
        };

        let response = self.post("/api/v1/runs", request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Start a run
    pub async fn start_run(&self, run_id: &str) -> Run {
        let response = self.post(&format!("/api/v1/runs/{}/start", run_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Pause a run
    pub async fn pause_run(&self, run_id: &str) -> Run {
        let response = self.post(&format!("/api/v1/runs/{}/pause", run_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Resume a run
    pub async fn resume_run(&self, run_id: &str) -> Run {
        let response = self.post(&format!("/api/v1/runs/{}/resume", run_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Cancel a run
    pub async fn cancel_run(&self, run_id: &str) -> Run {
        let response = self.post(&format!("/api/v1/runs/{}/cancel", run_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get a run by ID
    pub async fn get_run(&self, run_id: &str) -> Run {
        let response = self.get(&format!("/api/v1/runs/{}", run_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get events for a run
    pub async fn get_run_events(&self, run_id: &str) -> Vec<Event> {
        let response = self.get(&format!("/api/v1/runs/{}/events", run_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    // ============================================================================
    // Approval Helpers
    // ============================================================================

    /// Create an approval request
    pub async fn create_approval(&self, run_id: &str, title: &str) -> ApprovalRequest {
        let request = CreateApprovalRequest {
            run_id: run_id.to_string(),
            step_cursor: None,
            priority: Some(ApprovalPriority::Normal),
            title: title.to_string(),
            description: Some("Test approval request".to_string()),
            action_type: Some("test_action".to_string()),
            action_params: None,
            reasoning: Some("Need approval for testing".to_string()),
            requested_by: Some("test".to_string()),
            timeout_seconds: None,
        };

        let response = self.post("/api/v1/approvals", request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Approve an approval request
    pub async fn approve_request(&self, approval_id: &str, message: Option<&str>) -> ApprovalRequest {
        let body = ApprovalResponse {
            approved: true,
            message: message.map(|m| m.to_string()),
            modified_params: None,
        };

        let response = self.post(&format!("/api/v1/approvals/{}/approve", approval_id), body).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Deny an approval request
    pub async fn deny_request(&self, approval_id: &str, message: Option<&str>) -> ApprovalRequest {
        let body = ApprovalResponse {
            approved: false,
            message: message.map(|m| m.to_string()),
            modified_params: None,
        };

        let response = self.post(&format!("/api/v1/approvals/{}/deny", approval_id), body).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get an approval request
    pub async fn get_approval(&self, approval_id: &str) -> ApprovalRequest {
        let response = self.get(&format!("/api/v1/approvals/{}", approval_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    // ============================================================================
    // Schedule Helpers
    // ============================================================================

    /// Create a schedule
    pub async fn create_schedule(&self, name: &str, cron_expr: &str) -> Schedule {
        let request = serde_json::json!({
            "name": name,
            "description": "Test schedule",
            "cron_expr": cron_expr,
            "natural_lang": "Test schedule",
            "timezone": "UTC",
            "job_template": {
                "command": "echo".to_string(),
                "args": Some(vec!["scheduled".to_string()]),
                "env": None::<serde_json::Value>,
                "working_dir": None::<String>,
                "timeout_seconds": None::<u64>,
            },
            "enabled": true,
        });

        let response = self.post("/api/v1/schedules", request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Trigger a schedule manually
    pub async fn trigger_schedule(&self, schedule_id: &str) -> serde_json::Value {
        let response = self.post(&format!("/api/v1/schedules/{}/trigger", schedule_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get a schedule
    pub async fn get_schedule(&self, schedule_id: &str) -> Schedule {
        let response = self.get(&format!("/api/v1/schedules/{}", schedule_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    // ============================================================================
    // Attachment Helpers
    // ============================================================================

    /// Attach a client to a run
    pub async fn attach_to_run(&self, run_id: &str, client_type: ClientType, user_id: Option<&str>) -> serde_json::Value {
        let request = serde_json::json!({
            "client_type": client_type,
            "user_id": user_id,
        });

        let response = self.post(&format!("/api/v1/runs/{}/attach", run_id), request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Detach a client from a run
    pub async fn detach_from_run(&self, run_id: &str, client_id: &str) -> serde_json::Value {
        let request = serde_json::json!({
            "client_id": client_id,
        });

        let response = self.post(&format!("/api/v1/runs/{}/detach", run_id), request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get attachments for a run
    pub async fn get_run_attachments(&self, run_id: &str) -> Vec<Attachment> {
        let response = self.get(&format!("/api/v1/runs/{}/attachments", run_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    // ============================================================================
    // Job Helpers
    // ============================================================================

    /// Create a job for a run
    pub async fn create_job(&self, run_id: &str, name: &str) -> Job {
        let request = serde_json::json!({
            "name": name,
            "description": "Test job",
            "priority": 0,
            "config": {
                "command": "echo".to_string(),
                "args": Some(vec!["job".to_string()]),
                "env": None::<serde_json::Value>,
                "working_dir": None::<String>,
                "timeout_seconds": None::<u64>,
            },
        });

        let response = self.post(&format!("/api/v1/runs/{}/jobs", run_id), request).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Start a job
    pub async fn start_job(&self, run_id: &str, job_id: &str) -> Job {
        let response = self.post(&format!("/api/v1/runs/{}/jobs/{}/start", run_id, job_id), serde_json::json!({})).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Complete a job
    pub async fn complete_job(&self, run_id: &str, job_id: &str, result: serde_json::Value) -> Job {
        let response = self.post(&format!("/api/v1/runs/{}/jobs/{}/complete", run_id, job_id), result).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }

    /// Get a job
    pub async fn get_job(&self, run_id: &str, job_id: &str) -> Job {
        let response = self.get(&format!("/api/v1/runs/{}/jobs/{}", run_id, job_id)).await;
        Self::assert_success(&response);
        Self::parse_json(response).await
    }
}

/// Test context that cleans up after tests
impl Drop for TestApp {
    fn drop(&mut self) {
        // Best-effort schema teardown on a dedicated thread: Drop is
        // synchronous and the pool's connections are pinned to the schema,
        // so use a short-lived runtime on a fresh connection. Without this,
        // every run leaves an `it_<uuid>` schema behind on the dev database.
        let schema = self.schema.clone();
        let database_url = test_database_url();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Runtime::new() else {
                return;
            };
            let _ = rt.block_on(async move {
                if let Ok(pool) = sqlx::postgres::PgPool::connect(&database_url).await {
                    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {} CASCADE", schema))
                        .execute(&pool)
                        .await;
                }
            });
        })
        .join()
        .ok();
    }
}

fn test_database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://allternit:allternit_pg_2026@localhost:5432/allternit_test".to_string())
}

/// Split a PostgreSQL script into individual statements.
///
/// A statement boundary is a `;` in normal state only. The scanner tracks
/// `'`/`"` quoting (with doubled-character escapes), `--` line comments,
/// nested `/* */` block comments, and dollar-quoted blocks (`$$ … $$`,
/// `$tag$ … $tag$`) — a naive `;` split breaks on the DO blocks and enum
/// literals in 001/002. Comment bodies are replaced with a single space so
/// they cannot bleed tokens into neighboring statements.
fn split_sql_statements(script: &str) -> Vec<String> {
    enum State {
        Normal,
        SingleQuote,
        DoubleQuote,
        LineComment,
        BlockComment(usize),
        DollarQuote(String), // full tag, e.g. "$$" or "$body$"
    }

    let mut statements = Vec::new();
    let mut current = String::new();
    let mut state = State::Normal;
    let mut chars = script.chars().peekable();

    while let Some(c) = chars.next() {
        match state {
            State::Normal => match c {
                ';' => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        statements.push(trimmed.to_string());
                    }
                    current.clear();
                }
                '\'' => {
                    current.push(c);
                    state = State::SingleQuote;
                }
                '"' => {
                    current.push(c);
                    state = State::DoubleQuote;
                }
                '-' if chars.peek() == Some(&'-') => {
                    chars.next();
                    current.push(' ');
                    state = State::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    current.push(' ');
                    state = State::BlockComment(1);
                }
                '$' => {
                    // Opening a dollar quote requires a valid tag: an
                    // optional identifier between two `$`. Otherwise `$` is
                    // just an operator character (e.g. `cost $1`).
                    let mut lookahead = chars.clone();
                    let mut tag = String::from("$");
                    while let Some(&nc) = lookahead.peek() {
                        if nc.is_alphanumeric() || nc == '_' {
                            tag.push(nc);
                            lookahead.next();
                        } else {
                            break;
                        }
                    }
                    if lookahead.peek() == Some(&'$') {
                        tag.push('$');
                        for _ in 0..tag.len() - 1 {
                            chars.next();
                        }
                        current.push_str(&tag);
                        state = State::DollarQuote(tag);
                    } else {
                        current.push(c);
                    }
                }
                _ => current.push(c),
            },
            State::SingleQuote => {
                current.push(c);
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        current.push(chars.next().unwrap());
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::DoubleQuote => {
                current.push(c);
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        current.push(chars.next().unwrap());
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::LineComment => {
                if c == '\n' {
                    current.push(c);
                    state = State::Normal;
                }
            }
            State::BlockComment(depth) => match (c, chars.peek()) {
                ('*', Some(&'/')) => {
                    chars.next();
                    if depth == 1 {
                        state = State::Normal;
                    } else {
                        state = State::BlockComment(depth - 1);
                    }
                }
                ('/', Some(&'*')) => {
                    chars.next();
                    state = State::BlockComment(depth + 1);
                }
                _ => {}
            },
            State::DollarQuote(ref tag) => {
                if c == '$' {
                    // Candidate closing tag starting at this `$`.
                    let ident_len = tag.len() - 2; // tag is `$<idents>$`
                    let mut matches = true;
                    for (i, tc) in tag[1..tag.len() - 1].chars().enumerate() {
                        match chars.clone().nth(i) {
                            Some(nc) if nc == tc => {}
                            _ => {
                                matches = false;
                                break;
                            }
                        }
                    }
                    if matches {
                        for _ in 0..ident_len {
                            chars.next();
                        }
                        // Closing `$` still sits in the iterator.
                        if chars.next() == Some('$') {
                            current.push_str(tag);
                            state = State::Normal;
                            continue;
                        }
                        // Not a real closing tag after all; fall through and
                        // emit the `$` we already consumed.
                    }
                    current.push(c);
                } else {
                    current.push(c);
                }
            }
        }
    }

    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }
    statements
}

#[cfg(test)]
mod tests {
    use super::split_sql_statements;

    #[test]
    fn splits_simple_statements() {
        let sql = "CREATE TABLE a (id INT);\nCREATE TABLE b (id INT);";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("CREATE TABLE a"));
        assert!(statements[1].starts_with("CREATE TABLE b"));
    }

    #[test]
    fn keeps_semicolons_inside_single_quotes() {
        let sql = "INSERT INTO t (v) VALUES ('a;b'); UPDATE t SET v = 'it''s;x';";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("'a;b'"));
        assert!(statements[1].contains("'it''s;x'"));
    }

    #[test]
    fn keeps_semicolons_inside_double_quotes() {
        let sql = r#"CREATE TABLE t ("a;b" TEXT); SELECT 1;"#;
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains(r#""a;b""#));
    }

    #[test]
    fn keeps_semicolons_inside_dollar_quoted_blocks() {
        let sql = "DO $$\nBEGIN\n    IF x THEN RAISE NOTICE 'a;b'; END IF;\nEND\n$$;\nCREATE TYPE mood AS ENUM ('ok');";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("DO $$"));
        assert!(statements[0].ends_with("$$"));
        assert!(statements[1].starts_with("CREATE TYPE mood"));
    }

    #[test]
    fn supports_tagged_dollar_quotes() {
        let sql = "DO $body$\nBEGIN\n    PERFORM 1; PERFORM 2;\nEND\n$body$; SELECT 3;";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("PERFORM 1; PERFORM 2;"));
    }

    #[test]
    fn ignores_semicolons_in_line_and_block_comments() {
        let sql = "SELECT 1; -- trailing; comment\nSELECT 2 /* block; comment */ + 1; /* unterminated-looking; ";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("SELECT 1"));
        assert!(statements[1].starts_with("SELECT 2"));
    }

    #[test]
    fn handles_nested_block_comments() {
        let sql = "SELECT 1 /* outer /* inner ; */ still outer */ + 2; SELECT 3;";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        // Comment bodies (including everything after the nested close) are
        // dropped, but the `;` inside the nesting must not split, and the
        // surviving SQL on both sides of the comment is one statement.
        assert!(statements[0].contains("SELECT 1"));
        assert!(statements[0].contains("+ 2"));
        assert!(!statements[0].contains("outer"));
    }

    #[test]
    fn dollar_operator_is_not_a_dollar_quote() {
        // `$1` is a parameter, not the opening of a dollar-quoted string.
        let sql = "SELECT cost $1 FROM t; SELECT $2;";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("$1"));
    }

    #[test]
    fn drops_empty_and_comment_only_statements() {
        let sql = "; -- just a comment\n;\nSELECT 1;;\n";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0], "SELECT 1");
    }
}
