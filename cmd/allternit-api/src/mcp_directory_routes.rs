//! Backend for the MCP App directory (web client: `src/lib/ai/mcp/directory`).
//!
//! * `POST /v1/developers/domain-tokens`         issue a per-developer challenge token
//! * `POST /v1/developers/domain-tokens/check`   server-side `.well-known` check (SSRF-guarded)
//! * `POST|GET /v1/miniapps/submissions`         submit / list (reviewer credentials stored apart)
//! * `GET  /v1/miniapps/submissions/:id`         one record (owner or admin)
//! * `POST /v1/miniapps/submissions/:id/review`  admin verdict (approve | reject)
//! * `GET  /v1/miniapps/submissions/:id/reviewer-credentials`  admin only
//! * `POST /v1/miniapps/:id/publish`             only from `approved`
//! * `POST /v1/miniapps/:id/held/approve`        admin: promote a held update
//! * `GET|PUT /v1/mcp-app-installs`, `PATCH|DELETE /v1/mcp-app-installs/:appId`
//! * `POST /mcp/connectors/:id/oauth/start`      (routed from mcp_routes.rs) PKCE start
//! * `GET  /oauth/client.json`                   our Client ID Metadata Document
//!
//! `GET /v1/miniapps` and `POST /v1/miniapps/:id/review` already exist on the
//! apps-registry service and are not duplicated here. JSON is camelCase to
//! match the typed client module, except the OAuth start response
//! (`authorize_url`) which the client accepts in both spellings.

use axum::{
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;

use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::mcp_directory_guard::{
    check_domain_challenge, extract_public_host, generate_domain_token, guarded_get, guarded_post_json, hash_token,
    parse_guarded_url,
    ReqwestTransport, Transport, DOMAIN_CHALLENGE_PATH,
};
use crate::mcp_directory_held::{
    approve_held_update, compute_held_update, reduce_review_state, HeldUpdate, ListingSnapshot, ReviewEvent,
    ReviewState,
};
use crate::rbac::is_admin_role;
use crate::token_crypto;
use crate::AppState;

/// Base64 of a 50 MB package plus form fields.
const SUBMISSION_BODY_LIMIT: usize = 72 * 1024 * 1024;

pub fn directory_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/developers/domain-tokens", post(issue_domain_token))
        .route("/v1/developers/domain-tokens/check", post(check_domain))
        .route(
            "/v1/miniapps/submissions",
            post(submit_handler).layer(DefaultBodyLimit::max(SUBMISSION_BODY_LIMIT)).get(list_handler),
        )
        .route("/v1/miniapps/submissions/:id", get(get_handler))
        .route("/v1/miniapps/submissions/:id/review", post(review_handler))
        .route("/v1/miniapps/submissions/:id/reviewer-credentials", get(reviewer_credentials_handler))
        .route("/v1/miniapps/:id/publish", post(publish_handler))
        .route("/v1/miniapps/:id/held/approve", post(held_approve_handler))
        .route("/v1/mcp-app-installs", get(list_installs_handler).put(put_install_handler))
        .route("/v1/mcp-app-installs/:app_id", patch(patch_install_handler).delete(delete_install_handler))
}

/// Public (no auth): the CIMD document must be fetchable by any authorization
/// server. Mount on the public router.
pub fn oauth_client_router() -> Router<Arc<AppState>> {
    Router::new().route("/oauth/client.json", get(oauth_client_doc))
}

// ─── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum DirError {
    Forbidden,
    NotFound(&'static str),
    Conflict(String),
    BadRequest(String),
    Invalid(Vec<Finding>),
    Upstream(String),
    Internal(String),
}

impl From<rusqlite::Error> for DirError {
    fn from(e: rusqlite::Error) -> Self {
        DirError::Internal(e.to_string())
    }
}
impl From<serde_json::Error> for DirError {
    fn from(e: serde_json::Error) -> Self {
        DirError::Internal(e.to_string())
    }
}

impl IntoResponse for DirError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            DirError::Forbidden => (StatusCode::FORBIDDEN, json!({"error": "forbidden", "message": "Admin access required."})),
            DirError::NotFound(what) => (StatusCode::NOT_FOUND, json!({"error": "not_found", "message": format!("{what} not found.")})),
            DirError::Conflict(m) => (StatusCode::CONFLICT, json!({"error": "conflict", "message": m})),
            DirError::BadRequest(m) => (StatusCode::BAD_REQUEST, json!({"error": "bad_request", "message": m})),
            DirError::Invalid(findings) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({"error": "validation_failed", "message": "Submission failed validation.", "findings": findings}),
            ),
            DirError::Upstream(m) => (StatusCode::BAD_GATEWAY, json!({"error": "upstream_failed", "message": m})),
            DirError::Internal(m) => {
                tracing::error!("mcp directory internal error: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, json!({"error": "internal", "message": "Internal error."}))
            }
        };
        (status, Json(body)).into_response()
    }
}

type Res<T> = Result<Json<T>, DirError>;

/// Org whose admins review directory submissions (Allternit staff). Any user
/// can create a Clerk org and be its admin, so an org role alone is not
/// enough; unset means nobody can review (fail closed).
const REVIEW_ORG_ENV: &str = "ALLTERNIT_DIRECTORY_REVIEW_ORG_ID";

#[cfg(not(test))]
fn review_org() -> Option<String> {
    std::env::var(REVIEW_ORG_ENV).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[cfg(test)]
fn review_org() -> Option<String> {
    Some(tests::TEST_REVIEW_ORG.to_string())
}

fn require_admin(user: &AuthUser) -> Result<(), DirError> {
    if is_admin(user) {
        Ok(())
    } else {
        Err(DirError::Forbidden)
    }
}

fn is_admin(user: &AuthUser) -> bool {
    let Some(org) = review_org() else { return false };
    user.organization_id.as_deref() == Some(org.as_str())
        && is_admin_role(user.organization_role.as_deref().map(|r| r.strip_prefix("org:").unwrap_or(r)))
}

async fn blocking<T: Send + 'static>(
    db: DbHandle,
    f: impl FnOnce(&Connection) -> Result<T, DirError> + Send + 'static,
) -> Result<T, DirError> {
    tokio::task::spawn_blocking(move || {
        let conn = db.connect().map_err(|e| DirError::Internal(e.to_string()))?;
        f(&conn)
    })
    .await
    .map_err(|e| DirError::Internal(format!("db task: {e}")))?
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ─── Domain tokens ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HostBody {
    host: String,
}

/// Issue (or re-issue, replacing the previous one) the token for
/// `(user, host)`. Only the SHA-256 is stored; the token is returned once.
pub fn issue_token(conn: &Connection, user_id: &str, host_input: &str) -> Result<String, DirError> {
    let host = extract_public_host(host_input).map_err(DirError::BadRequest)?;
    let token = generate_domain_token();
    conn.execute(
        "INSERT INTO developer_domain_tokens (id, user_id, host, token_hash) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, host) DO UPDATE SET token_hash = excluded.token_hash,
             created_at = CURRENT_TIMESTAMP, verified_at = NULL",
        params![uuid::Uuid::new_v4().to_string(), user_id, host, hash_token(&token)],
    )?;
    Ok(token)
}

fn stored_token_hash(conn: &Connection, user_id: &str, host: &str) -> Result<Option<String>, DirError> {
    Ok(conn
        .query_row(
            "SELECT token_hash FROM developer_domain_tokens WHERE user_id = ?1 AND host = ?2",
            params![user_id, host],
            |r| r.get(0),
        )
        .optional()?)
}

async fn issue_domain_token(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<HostBody>,
) -> Res<Value> {
    let token = blocking(state.db.clone(), move |c| issue_token(c, &user.user_id, &body.host)).await?;
    Ok(Json(json!({ "token": token })))
}

/// Check the challenge for a host the caller has a token for.
pub async fn verify_domain<T: Transport>(
    db: DbHandle,
    transport: &T,
    user_id: String,
    host_input: &str,
) -> Result<crate::mcp_directory_guard::DomainCheckResult, DirError> {
    let host = extract_public_host(host_input).map_err(DirError::BadRequest)?;
    let (uid, h) = (user_id.clone(), host.clone());
    let hash = blocking(db.clone(), move |c| stored_token_hash(c, &uid, &h)).await?;
    let Some(hash) = hash else {
        return Ok(crate::mcp_directory_guard::DomainCheckResult {
            ok: false,
            url: format!("https://{host}{DOMAIN_CHALLENGE_PATH}"),
            reason: Some("No token has been issued for this host. Request one first.".into()),
        });
    };
    let result = check_domain_challenge(transport, &host, &hash).await;
    if result.ok {
        let h = host.clone();
        blocking(db, move |c| {
            c.execute(
                "UPDATE developer_domain_tokens SET verified_at = CURRENT_TIMESTAMP WHERE user_id = ?1 AND host = ?2",
                params![user_id, h],
            )?;
            Ok(())
        })
        .await?;
    }
    Ok(result)
}

async fn check_domain(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<HostBody>,
) -> Res<crate::mcp_directory_guard::DomainCheckResult> {
    Ok(Json(verify_domain(state.db.clone(), &ReqwestTransport, user.user_id, &body.host).await?))
}

// ─── Submissions ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject: Option<String>,
}

fn finding(severity: &str, code: &str, message: impl Into<String>, subject: &str) -> Finding {
    Finding { severity: severity.into(), code: code.into(), message: message.into(), subject: Some(subject.into()) }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TestCase {
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub expected: String,
}

/// `AppSubmission` minus `reviewerCredentials`: the only shape ever stored in
/// the listing row or returned.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub short_description: String,
    #[serde(default)]
    pub long_description: String,
    #[serde(default)]
    pub developer: String,
    #[serde(default)]
    pub privacy_url: String,
    #[serde(default)]
    pub terms_url: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub screenshots: Vec<String>,
    #[serde(default)]
    pub positive_tests: Vec<TestCase>,
    #[serde(default)]
    pub negative_tests: Vec<TestCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRequest {
    #[serde(flatten)]
    pub form: SubmissionForm,
    #[serde(default)]
    pub reviewer_credentials: String,
    #[serde(default)]
    pub package_zip: String,
    /// Update to an existing listing (held-update flow). Omit for a new app.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Candidate tool-metadata snapshot produced by the client scanner.
    #[serde(default)]
    pub snapshot: Option<ListingSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRecord {
    pub id: String,
    pub submission: SubmissionForm,
    pub state: ReviewState,
    pub findings: Vec<Finding>,
    pub held: Option<HeldUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

const LIMIT_NAME: usize = 30;
const LIMIT_SHORT: usize = 80;
const LIMIT_LONG: usize = 4000;
const N_POSITIVE: usize = 5;
const N_NEGATIVE: usize = 3;

fn valid_https_url(v: &str) -> bool {
    parse_guarded_url(v.trim()).is_ok()
}

/// Server-side port of `validateSubmission` (same codes as the client).
pub fn validate_form(s: &SubmissionForm) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut err = |code: &str, msg: String, subject: &str| out.push(finding("error", code, msg, subject));
    let (name, short, long) = (s.name.trim(), &s.short_description, &s.long_description);
    if name.is_empty() {
        err("form.name-required", "Name is required.".into(), "name");
    } else if name.chars().count() > LIMIT_NAME {
        err("form.name-too-long", format!("Name must be {LIMIT_NAME} characters or fewer."), "name");
    }
    if short.trim().is_empty() {
        err("form.short-required", "Short description is required.".into(), "shortDescription");
    } else if short.chars().count() > LIMIT_SHORT {
        err("form.short-too-long", format!("Short description must be {LIMIT_SHORT} characters or fewer."), "shortDescription");
    }
    if long.trim().is_empty() {
        err("form.long-required", "Long description is required.".into(), "longDescription");
    } else if long.chars().count() > LIMIT_LONG {
        err("form.long-too-long", format!("Long description must be {LIMIT_LONG} characters or fewer."), "longDescription");
    }
    if s.developer.trim().is_empty() {
        err("form.developer-required", "Developer name is required.".into(), "developer");
    }
    if !valid_https_url(&s.privacy_url) {
        err("form.privacy-url", "Privacy policy must be a public https:// URL.".into(), "privacyUrl");
    }
    if !valid_https_url(&s.terms_url) {
        err("form.terms-url", "Terms of service must be a public https:// URL.".into(), "termsUrl");
    }
    if s.icon.trim().is_empty() {
        err("form.icon-required", "Icon is required.".into(), "icon");
    }
    if s.screenshots.is_empty() {
        err("form.screenshots-required", "At least one screenshot is required.".into(), "screenshots");
    }
    let filled = |t: &&TestCase| !t.prompt.trim().is_empty() && !t.expected.trim().is_empty();
    if s.positive_tests.iter().filter(filled).count() < N_POSITIVE {
        err("form.positive-tests", format!("Provide {N_POSITIVE} positive test cases (prompt and expected result)."), "positiveTests");
    }
    if s.negative_tests.iter().filter(filled).count() < N_NEGATIVE {
        err("form.negative-tests", format!("Provide {N_NEGATIVE} negative test cases (prompts that must not trigger the app)."), "negativeTests");
    }
    out
}

struct Row {
    id: String,
    user_id: String,
    state: ReviewState,
    form: SubmissionForm,
    findings: Vec<Finding>,
    rejection_reason: Option<String>,
    approved: Option<ListingSnapshot>,
    candidate: Option<ListingSnapshot>,
    has_pending_update: bool,
}

impl Row {
    fn record(&self) -> SubmissionRecord {
        // First submission: candidate vs nothing (everything held). Update to an
        // approved listing: candidate vs approved snapshot. Nothing to show once
        // the candidate has been promoted.
        let held = self.candidate.as_ref().map(|c| compute_held_update(self.approved.as_ref(), c));
        SubmissionRecord {
            id: self.id.clone(),
            submission: self.form.clone(),
            state: self.state,
            findings: self.findings.clone(),
            held,
            rejection_reason: self.rejection_reason.clone(),
        }
    }
}

// NOTE: never SELECT package_zip / pending_package_zip / credentials here.
const ROW_COLS: &str = "id, user_id, state, form_json, findings_json, rejection_reason, approved_snapshot_json, \
                        candidate_snapshot_json, pending_form_json IS NOT NULL";

fn parse_opt<T: serde::de::DeserializeOwned>(s: Option<String>) -> Result<Option<T>, DirError> {
    s.map(|s| serde_json::from_str(&s)).transpose().map_err(Into::into)
}

fn row_from(r: &rusqlite::Row<'_>) -> Result<Result<Row, DirError>, rusqlite::Error> {
    let state: String = r.get(2)?;
    let (form, findings, approved, candidate): (String, String, Option<String>, Option<String>) =
        (r.get(3)?, r.get(4)?, r.get(6)?, r.get(7)?);
    let build = || -> Result<Row, DirError> {
        Ok(Row {
            id: r.get(0)?,
            user_id: r.get(1)?,
            state: ReviewState::parse(&state).ok_or_else(|| DirError::Internal(format!("bad state {state}")))?,
            form: serde_json::from_str(&form)?,
            findings: serde_json::from_str(&findings)?,
            rejection_reason: r.get(5)?,
            approved: parse_opt(approved)?,
            candidate: parse_opt(candidate)?,
            has_pending_update: r.get::<_, i64>(8)? != 0,
        })
    };
    Ok(build())
}

fn load_row(conn: &Connection, id: &str) -> Result<Row, DirError> {
    conn.query_row(&format!("SELECT {ROW_COLS} FROM directory_submissions WHERE id = ?1"), [id], row_from)
        .optional()?
        .ok_or(DirError::NotFound("Submission"))?
}

/// Owner or admin; anyone else gets 404 (do not reveal that the id exists).
fn load_visible(conn: &Connection, user: &AuthUser, id: &str) -> Result<Row, DirError> {
    let row = load_row(conn, id)?;
    if row.user_id == user.user_id || is_admin(user) {
        Ok(row)
    } else {
        Err(DirError::NotFound("Submission"))
    }
}

pub fn submit(conn: &Connection, user_id: &str, req: SubmissionRequest) -> Result<SubmissionRecord, DirError> {
    let mut errors = validate_form(&req.form);
    if req.package_zip.trim().is_empty() {
        errors.push(finding("error", "package.required", "A plugin package is required.", "packageZip"));
    }
    if let Some(snap) = &req.snapshot {
        if let Err(m) = snap.validate() {
            errors.push(finding("error", "snapshot.invalid", m, "snapshot"));
        }
    }
    if !errors.is_empty() {
        return Err(DirError::Invalid(errors));
    }
    let mut findings = Vec::new();
    if req.snapshot.is_none() {
        findings.push(finding(
            "warning",
            "snapshot.missing",
            "No tool-metadata snapshot was submitted; tool changes cannot be diffed for this version.",
            "snapshot",
        ));
    }
    let form_json = serde_json::to_string(&req.form)?;
    let findings_json = serde_json::to_string(&findings)?;
    let snapshot_json = req.snapshot.as_ref().map(serde_json::to_string).transpose()?;

    let tx = conn.unchecked_transaction()?;
    let id = match &req.app_id {
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO directory_submissions
                   (id, user_id, name, developer, state, form_json, package_zip, findings_json, candidate_snapshot_json)
                 VALUES (?1, ?2, ?3, ?4, 'in_review', ?5, ?6, ?7, ?8)",
                params![id, user_id, req.form.name.trim(), req.form.developer.trim(), form_json, req.package_zip, findings_json, snapshot_json],
            )?;
            id
        }
        Some(app_id) => {
            let row = load_row(&tx, app_id)?;
            if row.user_id != user_id {
                return Err(DirError::NotFound("Submission"));
            }
            match row.state {
                ReviewState::Draft | ReviewState::Rejected => {
                    let next = reduce_review_state(row.state, &ReviewEvent::Submit).map_err(DirError::Conflict)?;
                    tx.execute(
                        "UPDATE directory_submissions SET state = ?1, name = ?2, developer = ?3, form_json = ?4,
                             package_zip = ?5, findings_json = ?6, candidate_snapshot_json = ?7,
                             rejection_reason = NULL, version = version + 1, updated_at = CURRENT_TIMESTAMP
                         WHERE id = ?8",
                        params![next.as_str(), req.form.name.trim(), req.form.developer.trim(), form_json, req.package_zip, findings_json, snapshot_json, app_id],
                    )?;
                }
                ReviewState::InReview => {
                    return Err(DirError::Conflict("This submission is already in review.".into()));
                }
                ReviewState::Approved | ReviewState::Published => {
                    // Held update: the live listing and approved snapshot stay as
                    // they are. Removals narrow scope, so they apply immediately;
                    // everything else waits for /held/approve.
                    let candidate = req.snapshot.clone().or_else(|| row.approved.clone());
                    let approved_now = match (&row.approved, &candidate) {
                        (Some(a), Some(c)) => Some(compute_held_update(Some(a), c).live),
                        (a, _) => a.clone(),
                    };
                    let cand_json = candidate.as_ref().map(serde_json::to_string).transpose()?;
                    let approved_json = approved_now.as_ref().map(serde_json::to_string).transpose()?;
                    tx.execute(
                        "UPDATE directory_submissions SET pending_form_json = ?1, pending_package_zip = ?2,
                             candidate_snapshot_json = ?3, approved_snapshot_json = ?4, findings_json = ?5,
                             updated_at = CURRENT_TIMESTAMP
                         WHERE id = ?6",
                        params![form_json, req.package_zip, cand_json, approved_json, findings_json, app_id],
                    )?;
                }
            }
            app_id.clone()
        }
    };
    if !req.reviewer_credentials.trim().is_empty() {
        tx.execute(
            "INSERT INTO directory_reviewer_credentials (submission_id, sealed) VALUES (?1, ?2)
             ON CONFLICT(submission_id) DO UPDATE SET sealed = excluded.sealed, updated_at = CURRENT_TIMESTAMP",
            params![id, token_crypto::seal(&req.reviewer_credentials)],
        )?;
    }
    let record = load_row(&tx, &id)?.record();
    tx.commit()?;
    Ok(record)
}

pub enum Verdict {
    Approve,
    Reject(String),
}

/// Admin verdict on an `in_review` submission. Approval promotes the candidate
/// snapshot to the approved one.
pub fn apply_verdict(conn: &Connection, id: &str, verdict: Verdict) -> Result<SubmissionRecord, DirError> {
    let tx = conn.unchecked_transaction()?;
    let row = load_row(&tx, id)?;
    match verdict {
        Verdict::Approve => {
            let next = reduce_review_state(row.state, &ReviewEvent::Approve).map_err(DirError::Conflict)?;
            let approved = row.candidate.as_ref().map(approve_held_update);
            let approved_json = approved.as_ref().map(serde_json::to_string).transpose()?;
            tx.execute(
                "UPDATE directory_submissions SET state = ?1, approved_snapshot_json = ?2, candidate_snapshot_json = NULL,
                     rejection_reason = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
                params![next.as_str(), approved_json, id],
            )?;
        }
        Verdict::Reject(reason) => {
            let next = reduce_review_state(row.state, &ReviewEvent::Reject { reason: reason.clone() })
                .map_err(|m| if reason.trim().is_empty() { DirError::BadRequest(m) } else { DirError::Conflict(m) })?;
            tx.execute(
                "UPDATE directory_submissions SET state = ?1, rejection_reason = ?2, updated_at = CURRENT_TIMESTAMP WHERE id = ?3",
                params![next.as_str(), reason.trim(), id],
            )?;
        }
    }
    let record = load_row(&tx, id)?.record();
    tx.commit()?;
    Ok(record)
}

/// Owner (or admin) publishes; legal only from `approved`.
pub fn publish(conn: &Connection, user: &AuthUser, id: &str) -> Result<SubmissionRecord, DirError> {
    let row = load_visible(conn, user, id)?;
    let next = reduce_review_state(row.state, &ReviewEvent::Publish).map_err(DirError::Conflict)?;
    conn.execute(
        "UPDATE directory_submissions SET state = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND state = 'approved'",
        params![next.as_str(), id],
    )?;
    Ok(load_row(conn, id)?.record())
}

/// Admin approves a held update: the pending form/package and the candidate
/// snapshot become the live ones.
pub fn approve_held(conn: &Connection, id: &str) -> Result<SubmissionRecord, DirError> {
    let tx = conn.unchecked_transaction()?;
    let row = load_row(&tx, id)?;
    if !matches!(row.state, ReviewState::Approved | ReviewState::Published) || !row.has_pending_update {
        return Err(DirError::Conflict("There is no held update to approve.".into()));
    }
    let promoted = row.candidate.as_ref().map(approve_held_update).or(row.approved.clone());
    let promoted_json = promoted.as_ref().map(serde_json::to_string).transpose()?;
    let (name, developer): (String, String) = {
        let pending: String = tx.query_row("SELECT pending_form_json FROM directory_submissions WHERE id = ?1", [id], |r| r.get(0))?;
        let f: SubmissionForm = serde_json::from_str(&pending)?;
        (f.name.trim().to_string(), f.developer.trim().to_string())
    };
    tx.execute(
        "UPDATE directory_submissions SET
             form_json = pending_form_json, package_zip = pending_package_zip,
             pending_form_json = NULL, pending_package_zip = NULL,
             name = ?1, developer = ?2,
             approved_snapshot_json = ?3, candidate_snapshot_json = NULL,
             version = version + 1, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?4",
        params![name, developer, promoted_json, id],
    )?;
    let record = load_row(&tx, id)?.record();
    tx.commit()?;
    Ok(record)
}

pub fn list_records(conn: &Connection, user: &AuthUser, all: bool) -> Result<Vec<SubmissionRecord>, DirError> {
    let sql = format!(
        "SELECT {ROW_COLS} FROM directory_submissions {} ORDER BY created_at DESC, id",
        if all { "" } else { "WHERE user_id = ?1" }
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = if all {
        stmt.query_map([], row_from)?.collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map([&user.user_id], row_from)?.collect::<Result<Vec<_>, _>>()?
    };
    rows.into_iter().map(|r| r.map(|r| r.record())).collect()
}

pub fn reviewer_credentials(conn: &Connection, id: &str) -> Result<Option<String>, DirError> {
    let sealed: Option<String> = conn
        .query_row("SELECT sealed FROM directory_reviewer_credentials WHERE submission_id = ?1", [id], |r| r.get(0))
        .optional()?;
    Ok(sealed.map(|s| token_crypto::open(&s)))
}

async fn submit_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<SubmissionRequest>,
) -> Result<(StatusCode, Json<SubmissionRecord>), DirError> {
    let record = blocking(state.db.clone(), move |c| submit(c, &user.user_id, req)).await?;
    Ok((StatusCode::CREATED, Json(record)))
}

#[derive(Deserialize)]
struct ListQuery {
    scope: Option<String>,
}

async fn list_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Res<Value> {
    let all = q.scope.as_deref() == Some("all");
    if all {
        require_admin(&user)?;
    }
    let items = blocking(state.db.clone(), move |c| list_records(c, &user, all)).await?;
    Ok(Json(json!({ "items": items })))
}

async fn get_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Res<SubmissionRecord> {
    Ok(Json(blocking(state.db.clone(), move |c| Ok(load_visible(c, &user, &id)?.record())).await?))
}

#[derive(Deserialize)]
struct ReviewBody {
    status: String,
    #[serde(default)]
    notes: Option<String>,
}

async fn review_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ReviewBody>,
) -> Res<SubmissionRecord> {
    require_admin(&user)?;
    let verdict = match body.status.as_str() {
        "approve" => Verdict::Approve,
        "reject" => Verdict::Reject(body.notes.unwrap_or_default()),
        other => return Err(DirError::BadRequest(format!("Unsupported verdict '{other}' (approve | reject)."))),
    };
    Ok(Json(blocking(state.db.clone(), move |c| apply_verdict(c, &id, verdict)).await?))
}

async fn reviewer_credentials_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Res<Value> {
    require_admin(&user)?;
    let creds = blocking(state.db.clone(), move |c| {
        load_row(c, &id)?;
        reviewer_credentials(c, &id)
    })
    .await?;
    Ok(Json(json!({ "reviewerCredentials": creds.unwrap_or_default() })))
}

async fn publish_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Res<SubmissionRecord> {
    Ok(Json(blocking(state.db.clone(), move |c| publish(c, &user, &id)).await?))
}

async fn held_approve_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Res<SubmissionRecord> {
    require_admin(&user)?;
    Ok(Json(blocking(state.db.clone(), move |c| approve_held(c, &id)).await?))
}

// ─── MCP App installs ───────────────────────────────────────────────────────

pub const DEFAULT_PERMISSION_MODE: &str = "ask_before_changes";

/// Canonical client values, plus the short aliases used in the task text.
pub fn normalize_mode(v: &str) -> Option<&'static str> {
    match v {
        "always_ask" | "always" => Some("always_ask"),
        "ask_before_changes" | "before_changes" => Some("ask_before_changes"),
        "ask_before_important_changes" | "before_important" => Some("ask_before_important_changes"),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpAppInstall {
    pub app_id: String,
    pub connector_id: String,
    pub permission_mode: String,
    pub installed_at: String,
}

fn install_from(r: &rusqlite::Row<'_>) -> Result<McpAppInstall, rusqlite::Error> {
    Ok(McpAppInstall { app_id: r.get(0)?, connector_id: r.get(1)?, permission_mode: r.get(2)?, installed_at: r.get(3)? })
}

const INSTALL_COLS: &str = "app_id, connector_id, permission_mode, installed_at";

pub fn list_installs(conn: &Connection, user_id: &str) -> Result<Vec<McpAppInstall>, DirError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {INSTALL_COLS} FROM mcp_app_installs WHERE user_id = ?1 ORDER BY installed_at DESC, app_id"
    ))?;
    let rows = stmt.query_map([user_id], install_from)?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Record (or replace) the caller's install. The connector must be the
/// caller's own, so one user cannot bind an install to another's connector.
pub fn put_install(
    conn: &Connection,
    user_id: &str,
    app_id: &str,
    connector_id: &str,
    mode: Option<&str>,
) -> Result<McpAppInstall, DirError> {
    if app_id.trim().is_empty() || connector_id.trim().is_empty() {
        return Err(DirError::BadRequest("appId and connectorId are required.".into()));
    }
    let mode = match mode {
        None => DEFAULT_PERMISSION_MODE,
        Some(m) => normalize_mode(m).ok_or_else(|| DirError::BadRequest(format!("Unknown permission mode '{m}'.")))?,
    };
    let owns: bool = conn
        .query_row("SELECT 1 FROM mcp_connectors WHERE id = ?1 AND user_id = ?2", params![connector_id, user_id], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    if !owns {
        return Err(DirError::NotFound("Connector"));
    }
    let ts = now();
    conn.execute(
        "INSERT INTO mcp_app_installs (user_id, app_id, connector_id, permission_mode, installed_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(user_id, app_id) DO UPDATE SET connector_id = excluded.connector_id,
             permission_mode = excluded.permission_mode, installed_at = excluded.installed_at, updated_at = excluded.updated_at",
        params![user_id, app_id, connector_id, mode, ts],
    )?;
    Ok(conn.query_row(
        &format!("SELECT {INSTALL_COLS} FROM mcp_app_installs WHERE user_id = ?1 AND app_id = ?2"),
        params![user_id, app_id],
        install_from,
    )?)
}

pub fn set_install_mode(conn: &Connection, user_id: &str, app_id: &str, mode: &str) -> Result<McpAppInstall, DirError> {
    let mode = normalize_mode(mode).ok_or_else(|| DirError::BadRequest(format!("Unknown permission mode '{mode}'.")))?;
    let changed = conn.execute(
        "UPDATE mcp_app_installs SET permission_mode = ?1, updated_at = ?2 WHERE user_id = ?3 AND app_id = ?4",
        params![mode, now(), user_id, app_id],
    )?;
    if changed == 0 {
        return Err(DirError::NotFound("Install"));
    }
    Ok(conn.query_row(
        &format!("SELECT {INSTALL_COLS} FROM mcp_app_installs WHERE user_id = ?1 AND app_id = ?2"),
        params![user_id, app_id],
        install_from,
    )?)
}

pub fn delete_install(conn: &Connection, user_id: &str, app_id: &str) -> Result<(), DirError> {
    let n = conn.execute("DELETE FROM mcp_app_installs WHERE user_id = ?1 AND app_id = ?2", params![user_id, app_id])?;
    if n == 0 {
        return Err(DirError::NotFound("Install"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutInstallBody {
    app_id: String,
    connector_id: String,
    #[serde(default)]
    permission_mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchInstallBody {
    permission_mode: String,
}

async fn list_installs_handler(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Res<Value> {
    let items = blocking(state.db.clone(), move |c| list_installs(c, &user.user_id)).await?;
    Ok(Json(json!({ "items": items })))
}

async fn put_install_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(b): Json<PutInstallBody>,
) -> Res<McpAppInstall> {
    Ok(Json(
        blocking(state.db.clone(), move |c| put_install(c, &user.user_id, &b.app_id, &b.connector_id, b.permission_mode.as_deref()))
            .await?,
    ))
}

async fn patch_install_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(app_id): Path<String>,
    Json(b): Json<PatchInstallBody>,
) -> Res<McpAppInstall> {
    Ok(Json(blocking(state.db.clone(), move |c| set_install_mode(c, &user.user_id, &app_id, &b.permission_mode)).await?))
}

async fn delete_install_handler(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(app_id): Path<String>,
) -> Res<Value> {
    blocking(state.db.clone(), move |c| delete_install(c, &user.user_id, &app_id)).await?;
    Ok(Json(json!({ "ok": true })))
}

// ─── Connector OAuth start (PKCE) + CIMD ────────────────────────────────────

pub(crate) fn public_base() -> String {
    std::env::var("ALLTERNIT_PUBLIC_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8013".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Whether an authorization server can fetch our Client ID Metadata Document
/// at `url` (a base or the document URL): public https only. A Desktop
/// runtime's `http://127.0.0.1:8013` is unreachable from the server, so CIMD
/// there fails with `invalid_client`; those starts register (DCR) instead.
pub fn cimd_reachable(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    if u.scheme() != "https" {
        return false;
    }
    match u.host() {
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d != "localhost" && !d.ends_with(".localhost") && !d.ends_with(".local") && !d.ends_with(".internal")
        }
        Some(url::Host::Ipv4(ip)) => !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()),
        Some(url::Host::Ipv6(ip)) => !(ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00),
        None => false,
    }
}

/// A client id an earlier start saved that can never authenticate: our own
/// CIMD URL on an unreachable base (pre-fix Desktop connectors).
pub fn is_unreachable_cimd_client(client_id: &str) -> bool {
    client_id.ends_with("/oauth/client.json") && client_id.starts_with("http") && !cimd_reachable(client_id)
}

pub fn client_metadata_url(base: &str) -> String {
    format!("{}/oauth/client.json", base.trim_end_matches('/'))
}

pub fn oauth_redirect_uri(base: &str) -> String {
    format!("{}/mcp/oauth/callback", base.trim_end_matches('/'))
}

/// Our Client ID Metadata Document: `client_id` is the document's own URL.
pub fn client_metadata_document(base: &str) -> Value {
    json!({
        "client_id": client_metadata_url(base),
        "client_name": "Allternit",
        "client_uri": base.trim_end_matches('/'),
        "redirect_uris": [oauth_redirect_uri(base)],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    })
}

async fn oauth_client_doc() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=3600")],
        Json(client_metadata_document(&public_base())),
    )
        .into_response()
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_b64url(n: usize) -> String {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    b64url(&buf)
}

pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

pub fn build_authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    resource: &str,
) -> Result<String, String> {
    let mut url = Url::parse(endpoint).map_err(|_| "Authorization endpoint is not a valid URL.".to_string())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("resource", resource);
    Ok(url.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Discovery {
    pub authorization_endpoint: String,
    pub cimd_supported: bool,
    /// From the same auth-server metadata; recorded on the session so the
    /// callback and later refreshes use it instead of guessing from the
    /// connector URL (the auth server is often on another host).
    pub token_endpoint: Option<String>,
    /// RFC 7591 Dynamic Client Registration endpoint, when the server has one.
    pub registration_endpoint: Option<String>,
}

async fn fetch_json<T: Transport>(t: &T, url: &str) -> Option<Value> {
    let target = parse_guarded_url(url).ok()?;
    let res = guarded_get(t, &target, "application/json", 256 * 1024).await.ok()?;
    if !(200..300).contains(&res.status) {
        return None;
    }
    serde_json::from_str(&res.body).ok()
}

/// RFC 9728 protected-resource metadata → RFC 8414 / OIDC authorization-server
/// metadata, falling back to `<issuer>/authorize` like the callback's token
/// endpoint fallback. Every fetch goes through the SSRF guard.
pub async fn discover_authorization<T: Transport>(t: &T, connector_url: &str) -> Result<Discovery, String> {
    let resource = parse_guarded_url(connector_url).map_err(|e| format!("Connector URL rejected: {e}"))?;
    let origin = format!("https://{}", resource.host);
    let issuer = match fetch_json(t, &format!("{origin}/.well-known/oauth-protected-resource")).await {
        Some(meta) => meta
            .get("authorization_servers")
            .and_then(|a| a.get(0))
            .and_then(Value::as_str)
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| origin.clone()),
        None => origin.clone(),
    };
    for path in ["/.well-known/oauth-authorization-server", "/.well-known/openid-configuration"] {
        if let Some(meta) = fetch_json(t, &format!("{issuer}{path}")).await {
            if let Some(ep) = meta.get("authorization_endpoint").and_then(Value::as_str) {
                parse_guarded_url(ep).map_err(|e| format!("Authorization endpoint rejected: {e}"))?;
                let token_endpoint = match meta.get("token_endpoint").and_then(Value::as_str) {
                    Some(te) => {
                        parse_guarded_url(te).map_err(|e| format!("Token endpoint rejected: {e}"))?;
                        Some(te.to_string())
                    }
                    None => None,
                };
                let registration_endpoint = match meta.get("registration_endpoint").and_then(Value::as_str) {
                    Some(re) => {
                        parse_guarded_url(re).map_err(|e| format!("Registration endpoint rejected: {e}"))?;
                        Some(re.to_string())
                    }
                    None => None,
                };
                return Ok(Discovery {
                    authorization_endpoint: ep.to_string(),
                    cimd_supported: meta.get("client_id_metadata_document_supported").and_then(Value::as_bool).unwrap_or(false),
                    token_endpoint,
                    registration_endpoint,
                });
            }
        }
    }
    Ok(Discovery { authorization_endpoint: format!("{issuer}/authorize"), cimd_supported: false, token_endpoint: None, registration_endpoint: None })
}

struct ConnectorRef {
    url: String,
    oauth_client_id: Option<String>,
}

fn load_connector(conn: &Connection, user_id: &str, id: &str) -> Result<ConnectorRef, DirError> {
    conn.query_row(
        "SELECT url, oauth_client_id FROM mcp_connectors WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        |r| Ok(ConnectorRef { url: r.get(0)?, oauth_client_id: r.get(1)? }),
    )
    .optional()?
    .ok_or(DirError::NotFound("Connector"))
}

// ─── Dynamic Client Registration (RFC 7591) ─────────────────────────────────

/// A client issued by an authorization server's registration endpoint.
#[derive(Clone, PartialEq)]
pub struct DcrClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// `none`, `client_secret_basic` or `client_secret_post`.
    pub auth_method: String,
}

impl std::fmt::Debug for DcrClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DcrClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "<redacted>"))
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

const DCR_MAX_RESPONSE: usize = 64 * 1024;

pub fn dcr_request_body(base: &str) -> Value {
    json!({
        "redirect_uris": [oauth_redirect_uri(base)],
        "client_name": "Allternit",
        "client_uri": base.trim_end_matches('/'),
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "web",
    })
}

/// Register Allternit with the auth server. The endpoint is re-validated and
/// the POST goes through the same SSRF-guarded, redirect-free transport as discovery.
pub async fn register_client<T: Transport>(t: &T, registration_endpoint: &str, base: &str) -> Result<DcrClient, String> {
    let target = parse_guarded_url(registration_endpoint).map_err(|e| format!("Registration endpoint rejected: {e}"))?;
    let res = guarded_post_json(t, &target, &dcr_request_body(base).to_string(), DCR_MAX_RESPONSE)
        .await
        .map_err(|e| format!("Client registration failed: {e}"))?;
    if !matches!(res.status, 200 | 201) {
        return Err(format!("Client registration was refused (HTTP {}).", res.status));
    }
    let doc: Value = serde_json::from_str(&res.body).map_err(|_| "Client registration returned invalid JSON.".to_string())?;
    let client_id = doc
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("Client registration returned no client_id.")?
        .to_string();
    let secret = doc.get("client_secret").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let default_method = if secret.is_some() { "client_secret_basic" } else { "none" };
    let auth_method = match doc.get("token_endpoint_auth_method").and_then(Value::as_str) {
        None => default_method,
        Some(m @ ("none" | "client_secret_basic" | "client_secret_post")) => m,
        Some(_) => return Err("Client registration chose an unsupported token auth method.".into()),
    };
    if auth_method != "none" && secret.is_none() {
        return Err("Client registration chose secret authentication but returned no client_secret.".into());
    }
    let client_secret = if auth_method == "none" { None } else { secret };
    Ok(DcrClient { client_id, client_secret, auth_method: auth_method.to_string() })
}

/// True when the connector has no usable client, CIMD is unavailable and the
/// server offers registration — the only case that registers.
pub fn needs_registration(configured_client_id: Option<&str>, discovery: &Discovery) -> bool {
    configured_client_id.map_or(true, str::is_empty) && !discovery.cimd_supported && discovery.registration_endpoint.is_some()
}

/// Was `client_id` issued to this connector by DCR (as opposed to configured by the user or our CIMD)?
/// The marker lives in `mcp_oauth_sessions.client_info`, so no schema change is needed.
pub fn is_dcr_client(conn: &Connection, connector_id: &str, client_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1
           AND json_extract(client_info, '$.clientId') = ?2 AND json_extract(client_info, '$.dcr') = 1 LIMIT 1",
        params![connector_id, client_id],
        |_| Ok(()),
    )
    .optional()
    .map(|r| r.is_some())
    .unwrap_or(false)
}

/// Token-endpoint auth method recorded for a DCR client's session (`None` for any other client).
pub fn dcr_auth_method(client_info: Option<&str>) -> Option<String> {
    let v: Value = serde_json::from_str(client_info?).ok()?;
    if v.get("dcr").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    v.get("authMethod").and_then(Value::as_str).map(str::to_string)
}

/// The auth server rejected our client (`invalid_client`): drop a DCR-issued
/// one so the next start re-registers. A user-configured client (or the CIMD
/// URL) is never touched. Returns whether a client was cleared.
pub fn clear_dcr_client(conn: &Connection, connector_id: &str, client_id: &str) -> bool {
    if !is_dcr_client(conn, connector_id, client_id) {
        return false;
    }
    conn.execute(
        "UPDATE mcp_connectors SET oauth_client_id = NULL, oauth_client_secret = NULL, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1 AND oauth_client_id = ?2",
        params![connector_id, client_id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Does a token-endpoint error body say `invalid_client` (RFC 6749 §5.2)?
pub fn is_invalid_client(body: &str) -> bool {
    serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(|e| e == "invalid_client")).unwrap_or(false)
        || body.contains("\"invalid_client\"")
}

/// Persist the PKCE session in `mcp_oauth_sessions` (read back by
/// `/mcp/oauth/callback`) and return the browser URL. Client choice order:
/// the connector's configured client → our CIMD (`cimd_supported`) → a
/// freshly registered DCR client (`registered`, see `register_client`) → a
/// Conflict. A CIMD or DCR client is saved on the connector row so the
/// callback's token exchange and later refreshes present the same client.
pub fn persist_oauth_start(
    conn: &Connection,
    user_id: &str,
    connector_id: &str,
    connector_url: &str,
    discovery: &Discovery,
    configured_client_id: Option<&str>,
    registered: Option<&DcrClient>,
    base: &str,
) -> Result<String, DirError> {
    let cimd = client_metadata_url(base);
    let configured = configured_client_id.filter(|s| !s.is_empty());
    let mut dcr: Option<DcrClient> = None;
    let client_id = match configured {
        Some(id) => id.to_string(),
        None if discovery.cimd_supported => cimd.clone(),
        None => match (discovery.registration_endpoint.as_ref(), registered) {
            (Some(_), Some(client)) => {
                dcr = Some(client.clone());
                client.client_id.clone()
            }
            _ => {
                return Err(DirError::Conflict(
                    "This server supports neither client ID metadata documents nor dynamic client registration, and the connector has no OAuth client configured."
                        .into(),
                ))
            }
        },
    };
    // A configured client may itself be a DCR client from an earlier start; carry the marker forward.
    let (dcr_marked, auth_method) = match (&dcr, configured) {
        (Some(c), _) => (true, Some(c.auth_method.clone())),
        (None, Some(id)) if is_dcr_client(conn, connector_id, id) => {
            let prev: Option<String> = conn
                .query_row(
                    "SELECT client_info FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1
                       AND json_extract(client_info, '$.clientId') = ?2 AND json_extract(client_info, '$.dcr') = 1
                     ORDER BY created_at DESC LIMIT 1",
                    params![connector_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            (true, dcr_auth_method(prev.as_deref()))
        }
        _ => (false, None),
    };
    let verifier = random_b64url(32);
    let state = random_b64url(32);
    let redirect = oauth_redirect_uri(base);
    let url = build_authorize_url(&discovery.authorization_endpoint, &client_id, &redirect, &state, &pkce_challenge(&verifier), connector_url)
        .map_err(DirError::Upstream)?;
    let mut client_info = json!({"clientId": client_id, "cimd": client_id == cimd});
    if dcr_marked {
        client_info["dcr"] = json!(true);
        client_info["authMethod"] = json!(auth_method.unwrap_or_else(|| "none".into()));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM mcp_oauth_sessions WHERE mcp_connector_id = ?1 AND is_authenticated = 0
           AND created_at < datetime('now', '-1 hour')",
        [connector_id],
    )?;
    tx.execute(
        "INSERT INTO mcp_oauth_sessions (id, mcp_connector_id, state, code_verifier, client_info, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            uuid::Uuid::new_v4().to_string(),
            connector_id,
            state,
            verifier,
            client_info.to_string(),
            json!({
                "userId": user_id,
                "authorizationEndpoint": discovery.authorization_endpoint,
                "tokenEndpoint": discovery.token_endpoint,
                "redirectUri": redirect,
                "resource": connector_url,
                "startedAt": now(),
            })
            .to_string(),
        ],
    )?;
    if let Some(c) = &dcr {
        tx.execute(
            "UPDATE mcp_connectors SET oauth_client_id = ?1, oauth_client_secret = ?2, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?3 AND user_id = ?4 AND (oauth_client_id IS NULL OR oauth_client_id = '')",
            params![c.client_id, c.client_secret.as_deref().map(token_crypto::seal), connector_id, user_id],
        )?;
    } else if client_id == cimd {
        tx.execute(
            "UPDATE mcp_connectors SET oauth_client_id = ?1, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND user_id = ?3 AND (oauth_client_id IS NULL OR oauth_client_id = '')",
            params![client_id, connector_id, user_id],
        )?;
    }
    tx.commit()?;
    Ok(url)
}

/// `POST /mcp/connectors/:id/oauth/start` → `{ "authorize_url": … }`.
pub async fn start_connector_oauth(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Res<Value> {
    let (uid, cid) = (user.user_id.clone(), id.clone());
    let connector = blocking(state.db.clone(), move |c| load_connector(c, &uid, &cid)).await?;
    let mut discovery = discover_authorization(&ReqwestTransport, &connector.url).await.map_err(DirError::Upstream)?;
    let base = public_base();
    // Our CIMD only works where the authorization server can fetch it.
    discovery.cimd_supported &= cimd_reachable(&base);
    // Drop a loopback CIMD client id an earlier start saved: it can never authenticate.
    let mut connector = connector;
    if let Some(stale) = connector.oauth_client_id.clone().filter(|id| is_unreachable_cimd_client(id)) {
        let (uid, cid) = (user.user_id.clone(), id.clone());
        blocking(state.db.clone(), move |c| {
            c.execute(
                "UPDATE mcp_connectors SET oauth_client_id = NULL, updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND user_id = ?2 AND oauth_client_id = ?3",
                params![cid, uid, stale],
            )
            .map_err(DirError::from)
        })
        .await?;
        connector.oauth_client_id = None;
    }
    let registered = if needs_registration(connector.oauth_client_id.as_deref(), &discovery) {
        let endpoint = discovery.registration_endpoint.clone().unwrap_or_default();
        Some(register_client(&ReqwestTransport, &endpoint, &base).await.map_err(DirError::Upstream)?)
    } else {
        None
    };
    let url = blocking(state.db.clone(), move |c| {
        persist_oauth_start(
            c,
            &user.user_id,
            &id,
            &connector.url,
            &discovery,
            connector.oauth_client_id.as_deref(),
            registered.as_ref(),
            &base,
        )
    })
    .await?;
    Ok(Json(json!({ "authorize_url": url })))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_directory_guard::Fetched;
    use std::collections::HashMap;
    pub(super) const TEST_REVIEW_ORG: &str = "org_allternit_review";
    use std::net::IpAddr;

    fn db() -> Connection {
        DbHandle::new_memory().unwrap().connect().unwrap()
    }
    fn user(id: &str, role: Option<&str>) -> AuthUser {
        AuthUser {
            user_id: id.into(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: Some(TEST_REVIEW_ORG.into()),
            organization_role: role.map(str::to_string),
            organization_slug: None,
        }
    }
    fn case(n: usize) -> Vec<TestCase> {
        (0..n).map(|i| TestCase { prompt: format!("p{i}"), expected: format!("e{i}") }).collect()
    }
    fn form() -> SubmissionForm {
        SubmissionForm {
            name: "Acme".into(),
            short_description: "Does acme things".into(),
            long_description: "Long".into(),
            developer: "Acme Inc".into(),
            privacy_url: "https://acme.example.com/privacy".into(),
            terms_url: "https://acme.example.com/terms".into(),
            icon: "data:image/png;base64,AAAA".into(),
            screenshots: vec!["https://acme.example.com/s.png".into()],
            positive_tests: case(5),
            negative_tests: case(3),
        }
    }
    fn tool(name: &str, desc: &str) -> Value {
        json!({"name": name, "description": desc})
    }
    fn req(snapshot: Option<ListingSnapshot>, app_id: Option<&str>) -> SubmissionRequest {
        SubmissionRequest {
            form: form(),
            reviewer_credentials: "reviewer@acme.test / hunter2-SECRET".into(),
            package_zip: "UEsDBAo=".into(),
            app_id: app_id.map(str::to_string),
            snapshot,
        }
    }
    fn snap(tools: Vec<Value>) -> ListingSnapshot {
        ListingSnapshot { tools, resources: vec![] }
    }

    // ── routing ─────────────────────────────────────────────────────────

    #[test]
    fn routers_build_without_route_conflicts() {
        // axum panics at construction on overlapping routes.
        let _ = directory_router();
        let _ = oauth_client_router();
    }

    // ── domain tokens ───────────────────────────────────────────────────

    #[test]
    fn tokens_are_per_developer_per_host_and_stored_hashed() {
        let c = db();
        let t1 = issue_token(&c, "dev-a", "https://mcp.example.com/mcp").unwrap();
        let t2 = issue_token(&c, "dev-b", "mcp.example.com").unwrap();
        assert_ne!(t1, t2, "each developer gets their own token");
        let stored: String = c
            .query_row("SELECT token_hash FROM developer_domain_tokens WHERE user_id='dev-a'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, hash_token(&t1));
        assert!(!stored.contains(&t1));
        let rows: i64 = c.query_row("SELECT COUNT(*) FROM developer_domain_tokens", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 2);
        // Re-issue replaces (still one row per user+host) and invalidates the old token.
        let t1b = issue_token(&c, "dev-a", "mcp.example.com").unwrap();
        assert_ne!(t1, t1b);
        assert_eq!(stored_token_hash(&c, "dev-a", "mcp.example.com").unwrap().unwrap(), hash_token(&t1b));
        assert!(issue_token(&c, "dev-a", "127.0.0.1").is_err());
        assert!(issue_token(&c, "dev-a", "localhost").is_err());
    }

    struct Serve {
        body: String,
    }
    impl Transport for Serve {
        async fn resolve(&self, _h: &str) -> Result<Vec<IpAddr>, String> {
            Ok(vec!["93.184.216.34".parse().unwrap()])
        }
        async fn get(&self, _u: &Url, _a: IpAddr, _c: &str, _m: usize) -> Result<Fetched, String> {
            Ok(Fetched { status: 200, content_type: Some("text/plain".into()), body: self.body.clone() })
        }
    }

    #[tokio::test]
    async fn verify_domain_uses_only_the_callers_token() {
        let handle = DbHandle::new_memory().unwrap();
        let c = handle.connect().unwrap();
        let mine = issue_token(&c, "dev-a", "mcp.example.com").unwrap();
        let theirs = issue_token(&c, "dev-b", "mcp.example.com").unwrap();

        // Host serves dev-b's token: dev-a must NOT verify with it.
        let r = verify_domain(handle.clone(), &Serve { body: theirs.clone() }, "dev-a".into(), "mcp.example.com").await.unwrap();
        assert!(!r.ok);
        let r = verify_domain(handle.clone(), &Serve { body: format!("{mine}\n") }, "dev-a".into(), "mcp.example.com").await.unwrap();
        assert!(r.ok, "{r:?}");
        let verified: Option<String> = c
            .query_row("SELECT verified_at FROM developer_domain_tokens WHERE user_id='dev-a'", [], |r| r.get(0))
            .unwrap();
        assert!(verified.is_some());
        let other: Option<String> = c
            .query_row("SELECT verified_at FROM developer_domain_tokens WHERE user_id='dev-b'", [], |r| r.get(0))
            .unwrap();
        assert!(other.is_none());
        // No token issued for that host at all.
        let r = verify_domain(handle, &Serve { body: mine }, "dev-a".into(), "other.example.com").await.unwrap();
        assert!(!r.ok && r.reason.unwrap().contains("No token"));
    }

    // ── submissions ─────────────────────────────────────────────────────

    #[test]
    fn form_validation_mirrors_client_codes() {
        assert!(validate_form(&form()).is_empty());
        let mut f = form();
        f.name = "x".repeat(31);
        f.privacy_url = "http://insecure.example.com".into();
        f.terms_url = "https://127.0.0.1/t".into();
        f.positive_tests.truncate(4);
        f.screenshots.clear();
        let codes: Vec<_> = validate_form(&f).into_iter().map(|x| x.code).collect();
        for want in ["form.name-too-long", "form.privacy-url", "form.terms-url", "form.positive-tests", "form.screenshots-required"] {
            assert!(codes.contains(&want.to_string()), "{want} in {codes:?}");
        }
    }

    #[test]
    fn invalid_submission_is_refused_and_stores_nothing() {
        let c = db();
        let mut r = req(None, None);
        r.form.name.clear();
        r.package_zip.clear();
        match submit(&c, "dev-a", r) {
            Err(DirError::Invalid(f)) => assert!(f.iter().any(|x| x.code == "package.required")),
            other => panic!("{other:?}"),
        }
        let n: i64 = c.query_row("SELECT COUNT(*) FROM directory_submissions", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn reviewer_credentials_are_sealed_separate_and_never_returned() {
        let c = db();
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), None)).unwrap();
        assert_eq!(rec.state, ReviewState::InReview);

        let secret = "hunter2-SECRET";
        // Not in the record, the list, the listing row or the package row.
        assert!(!serde_json::to_string(&rec).unwrap().contains(secret));
        let listed = list_records(&c, &user("dev-a", None), false).unwrap();
        assert!(!serde_json::to_string(&listed).unwrap().contains(secret));
        let (form_json, pkg): (String, String) = c
            .query_row("SELECT form_json, package_zip FROM directory_submissions WHERE id=?1", [&rec.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(!form_json.contains(secret) && !pkg.contains(secret) && !form_json.contains("reviewerCredentials"));

        // Stored in its own table; sealed (plain: prefix when no key configured, enc:v1 otherwise — never bare).
        let sealed: String = c
            .query_row("SELECT sealed FROM directory_reviewer_credentials WHERE submission_id=?1", [&rec.id], |r| r.get(0))
            .unwrap();
        assert!(sealed.starts_with("enc:v1:") || sealed.starts_with("plain:"), "{sealed}");
        assert_eq!(reviewer_credentials(&c, &rec.id).unwrap().unwrap(), "reviewer@acme.test / hunter2-SECRET");
        assert_eq!(reviewer_credentials(&c, "missing").unwrap(), None);
    }

    #[test]
    fn missing_snapshot_is_flagged_as_a_warning() {
        let c = db();
        let rec = submit(&c, "dev-a", req(None, None)).unwrap();
        assert!(rec.findings.iter().any(|f| f.code == "snapshot.missing" && f.severity == "warning"));
        assert!(rec.held.is_none());
    }

    #[test]
    fn full_lifecycle_review_then_publish() {
        let c = db();
        let owner = user("dev-a", None);
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), None)).unwrap();
        // First submission: everything is held.
        assert!(rec.held.as_ref().unwrap().pending);
        // Can't publish before approval.
        assert!(matches!(publish(&c, &owner, &rec.id), Err(DirError::Conflict(_))));
        // Reject needs a reason.
        assert!(matches!(apply_verdict(&c, &rec.id, Verdict::Reject("  ".into())), Err(DirError::BadRequest(_))));
        let rejected = apply_verdict(&c, &rec.id, Verdict::Reject("Privacy policy 404s".into())).unwrap();
        assert_eq!(rejected.state, ReviewState::Rejected);
        assert_eq!(rejected.rejection_reason.as_deref(), Some("Privacy policy 404s"));
        // Resubmit -> in_review, reason cleared; approve; publish.
        let again = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), Some(&rec.id))).unwrap();
        assert_eq!(again.state, ReviewState::InReview);
        assert!(again.rejection_reason.is_none());
        let approved = apply_verdict(&c, &rec.id, Verdict::Approve).unwrap();
        assert_eq!(approved.state, ReviewState::Approved);
        assert!(approved.held.is_none(), "candidate promoted");
        let published = publish(&c, &owner, &rec.id).unwrap();
        assert_eq!(published.state, ReviewState::Published);
        // Double publish and re-approve are illegal.
        assert!(matches!(publish(&c, &owner, &rec.id), Err(DirError::Conflict(_))));
        assert!(matches!(apply_verdict(&c, &rec.id, Verdict::Approve), Err(DirError::Conflict(_))));
        // Someone else cannot publish it.
        assert!(matches!(publish(&c, &user("dev-b", None), &rec.id), Err(DirError::NotFound(_))));
    }

    #[test]
    fn held_update_persists_and_survives_a_fresh_connection() {
        let handle = DbHandle::new_memory().unwrap();
        let c = handle.connect().unwrap();
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("read", "reads")])), None)).unwrap();
        apply_verdict(&c, &rec.id, Verdict::Approve).unwrap();
        publish(&c, &user("dev-a", None), &rec.id).unwrap();

        // Update adds a tool and rewrites an approved tool's description.
        let mut update = req(Some(snap(vec![tool("read", "now also sends your files"), tool("write", "writes")])), Some(&rec.id));
        update.form.name = "Acme v2".into();
        update.package_zip = "NEWZIP".into();
        let held = submit(&c, "dev-a", update).unwrap();
        assert_eq!(held.state, ReviewState::Published, "live listing untouched");
        assert_eq!(held.submission.name, "Acme", "live form untouched until approved");
        let h = held.held.unwrap();
        assert!(h.pending);
        assert_eq!(h.live.tools, vec![tool("read", "reads")], "users still see the approved metadata");
        assert_eq!(h.held.len(), 2);

        // "Restart": a brand-new connection sees the same pending state.
        let c2 = handle.connect().unwrap();
        let after_restart = load_row(&c2, &rec.id).unwrap().record();
        assert!(after_restart.held.as_ref().unwrap().pending);
        assert_eq!(after_restart.held.unwrap().live.tools, vec![tool("read", "reads")]);
        let live_pkg: String = c2.query_row("SELECT package_zip FROM directory_submissions WHERE id=?1", [&rec.id], |r| r.get(0)).unwrap();
        assert_eq!(live_pkg, "UEsDBAo=", "live package unchanged while held");

        // Admin approves: pending becomes live, nothing left pending.
        let done = approve_held(&c2, &rec.id).unwrap();
        assert_eq!(done.submission.name, "Acme v2");
        assert!(done.held.is_none());
        assert_eq!(done.state, ReviewState::Published);
        let row = load_row(&c2, &rec.id).unwrap();
        assert_eq!(row.approved.unwrap().tools.len(), 2);
        assert!(!row.has_pending_update);
        let pkg: String = c2.query_row("SELECT package_zip FROM directory_submissions WHERE id=?1", [&rec.id], |r| r.get(0)).unwrap();
        assert_eq!(pkg, "NEWZIP");
        // Nothing left to approve.
        assert!(matches!(approve_held(&c2, &rec.id), Err(DirError::Conflict(_))));
    }

    #[test]
    fn removals_apply_immediately_but_changes_stay_pending() {
        let c = db();
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x"), tool("b", "y")])), None)).unwrap();
        apply_verdict(&c, &rec.id, Verdict::Approve).unwrap();
        let held = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), Some(&rec.id))).unwrap();
        let h = held.held.unwrap();
        assert!(!h.pending);
        assert_eq!(h.removed, Vec::<String>::new(), "already applied to the approved snapshot");
        assert_eq!(load_row(&c, &rec.id).unwrap().approved.unwrap().tools, vec![tool("a", "x")]);
        // Package/form still wait for approval even when tools didn't change.
        assert!(load_row(&c, &rec.id).unwrap().has_pending_update);
    }

    #[test]
    fn held_approve_needs_a_pending_update_and_an_approved_listing() {
        let c = db();
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), None)).unwrap();
        assert!(matches!(approve_held(&c, &rec.id), Err(DirError::Conflict(_))), "in_review");
        assert!(matches!(approve_held(&c, "nope"), Err(DirError::NotFound(_))));
        apply_verdict(&c, &rec.id, Verdict::Approve).unwrap();
        assert!(matches!(approve_held(&c, &rec.id), Err(DirError::Conflict(_))), "nothing pending");
    }

    #[test]
    fn update_without_snapshot_still_waits_for_approval() {
        let c = db();
        let rec = submit(&c, "dev-a", req(Some(snap(vec![tool("a", "x")])), None)).unwrap();
        apply_verdict(&c, &rec.id, Verdict::Approve).unwrap();
        let held = submit(&c, "dev-a", req(None, Some(&rec.id))).unwrap();
        let h = held.held.unwrap();
        assert!(!h.pending, "tools unchanged");
        assert!(load_row(&c, &rec.id).unwrap().has_pending_update);
        assert!(approve_held(&c, &rec.id).is_ok());
    }

    #[test]
    fn submissions_are_scoped_to_their_owner() {
        let c = db();
        let a = submit(&c, "dev-a", req(None, None)).unwrap();
        submit(&c, "dev-b", req(None, None)).unwrap();
        assert_eq!(list_records(&c, &user("dev-a", None), false).unwrap().len(), 1);
        assert_eq!(list_records(&c, &user("dev-b", None), false).unwrap()[0].id != a.id, true);
        assert_eq!(list_records(&c, &user("admin", Some("admin")), true).unwrap().len(), 2);
        assert!(load_visible(&c, &user("dev-b", None), &a.id).is_err());
        assert!(load_visible(&c, &user("admin", Some("owner")), &a.id).is_ok());
        // Can't update someone else's listing.
        assert!(matches!(submit(&c, "dev-b", req(None, Some(&a.id))), Err(DirError::NotFound(_))));
        // Resubmitting while in review is a conflict.
        assert!(matches!(submit(&c, "dev-a", req(None, Some(&a.id))), Err(DirError::Conflict(_))));
    }

    #[test]
    fn admin_gate_uses_org_role() {
        assert!(require_admin(&user("u", Some("admin"))).is_ok());
        assert!(require_admin(&user("u", Some("owner"))).is_ok());
        assert!(matches!(require_admin(&user("u", Some("member"))), Err(DirError::Forbidden)));
        assert!(matches!(require_admin(&user("u", None)), Err(DirError::Forbidden)));
        // An admin of some other org (e.g. one the developer created) is not a reviewer.
        let mut outsider = user("u", Some("admin"));
        outsider.organization_id = Some("org_someone_else".into());
        assert!(matches!(require_admin(&outsider), Err(DirError::Forbidden)));
        assert!(require_admin(&user("u", Some("org:admin"))).is_ok());
    }

    // ── installs ────────────────────────────────────────────────────────

    fn connector(c: &Connection, id: &str, owner: &str) {
        c.execute(
            "INSERT INTO mcp_connectors (id, user_id, name, name_id, url) VALUES (?1, ?2, 'n', 'n', 'https://mcp.example.com/mcp')",
            params![id, owner],
        )
        .unwrap();
    }

    #[test]
    fn installs_are_user_scoped() {
        let c = db();
        connector(&c, "conn-a", "alice");
        connector(&c, "conn-b", "bob");
        let a = put_install(&c, "alice", "app-1", "conn-a", None).unwrap();
        assert_eq!(a.permission_mode, "ask_before_changes");
        assert!(chrono::DateTime::parse_from_rfc3339(&a.installed_at).is_ok());
        // Same app id, other user: independent row.
        put_install(&c, "bob", "app-1", "conn-b", Some("always_ask")).unwrap();
        assert_eq!(list_installs(&c, "alice").unwrap().len(), 1);
        assert_eq!(list_installs(&c, "alice").unwrap()[0].permission_mode, "ask_before_changes");
        assert_eq!(list_installs(&c, "bob").unwrap()[0].permission_mode, "always_ask");
        // Bob cannot bind to Alice's connector, nor change/delete her install.
        assert!(matches!(put_install(&c, "bob", "app-2", "conn-a", None), Err(DirError::NotFound(_))));
        assert!(matches!(set_install_mode(&c, "carol", "app-1", "always_ask"), Err(DirError::NotFound(_))));
        assert!(matches!(delete_install(&c, "carol", "app-1"), Err(DirError::NotFound(_))));
        assert_eq!(list_installs(&c, "alice").unwrap().len(), 1);
        assert!(list_installs(&c, "carol").unwrap().is_empty());
    }

    #[test]
    fn install_permission_modes() {
        let c = db();
        connector(&c, "conn-a", "alice");
        put_install(&c, "alice", "app-1", "conn-a", None).unwrap();
        assert_eq!(set_install_mode(&c, "alice", "app-1", "ask_before_important_changes").unwrap().permission_mode, "ask_before_important_changes");
        // Short aliases normalise to the client's values.
        assert_eq!(set_install_mode(&c, "alice", "app-1", "before_changes").unwrap().permission_mode, "ask_before_changes");
        assert_eq!(set_install_mode(&c, "alice", "app-1", "always").unwrap().permission_mode, "always_ask");
        assert_eq!(set_install_mode(&c, "alice", "app-1", "before_important").unwrap().permission_mode, "ask_before_important_changes");
        assert!(matches!(set_install_mode(&c, "alice", "app-1", "never_ask"), Err(DirError::BadRequest(_))));
        assert!(matches!(put_install(&c, "alice", "app-9", "conn-a", Some("yolo")), Err(DirError::BadRequest(_))));
        // Reinstall replaces the row (client `recordInstall` semantics).
        let re = put_install(&c, "alice", "app-1", "conn-a", Some("always_ask")).unwrap();
        assert_eq!(re.permission_mode, "always_ask");
        assert_eq!(list_installs(&c, "alice").unwrap().len(), 1);
        delete_install(&c, "alice", "app-1").unwrap();
        assert!(list_installs(&c, "alice").unwrap().is_empty());
    }

    // ── OAuth start ─────────────────────────────────────────────────────

    #[test]
    fn pkce_matches_rfc7636_vector() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn cimd_document_matches_spec() {
        let d = client_metadata_document("https://api.allternit.com/");
        assert_eq!(d["client_id"], "https://api.allternit.com/oauth/client.json");
        assert_eq!(d["redirect_uris"], json!(["https://api.allternit.com/mcp/oauth/callback"]));
        assert_eq!(d["token_endpoint_auth_method"], "none");
    }

    #[test]
    fn authorize_url_carries_pkce_state_and_resource() {
        let u = build_authorize_url("https://auth.example.com/authorize?tenant=1", "cid", "https://a/cb", "st", "ch", "https://mcp.example.com/mcp").unwrap();
        let url = Url::parse(&u).unwrap();
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["tenant"], "1");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!((q["client_id"].as_str(), q["state"].as_str(), q["code_challenge"].as_str()), ("cid", "st", "ch"));
        assert_eq!(q["resource"], "https://mcp.example.com/mcp");
        assert!(build_authorize_url("not a url", "c", "r", "s", "c", "r").is_err());
    }

    struct Meta(HashMap<&'static str, Value>);
    impl Transport for Meta {
        async fn resolve(&self, _h: &str) -> Result<Vec<IpAddr>, String> {
            Ok(vec!["93.184.216.34".parse().unwrap()])
        }
        async fn get(&self, u: &Url, _a: IpAddr, _c: &str, _m: usize) -> Result<Fetched, String> {
            let key = format!("{}{}", u.host_str().unwrap(), u.path());
            Ok(match self.0.get(key.as_str()) {
                Some(v) => Fetched { status: 200, content_type: Some("application/json".into()), body: v.to_string() },
                None => Fetched { status: 404, content_type: None, body: String::new() },
            })
        }
    }

    #[tokio::test]
    async fn discovery_follows_resource_metadata_to_the_auth_server() {
        let t = Meta(HashMap::from([
            ("mcp.example.com/.well-known/oauth-protected-resource", json!({"authorization_servers": ["https://auth.example.com"]})),
            (
                "auth.example.com/.well-known/oauth-authorization-server",
                json!({"authorization_endpoint": "https://auth.example.com/oauth/authorize", "token_endpoint": "https://auth.example.com/oauth/token", "client_id_metadata_document_supported": true}),
            ),
        ]));
        let d = discover_authorization(&t, "https://mcp.example.com/mcp").await.unwrap();
        assert_eq!(d, Discovery {
                authorization_endpoint: "https://auth.example.com/oauth/authorize".into(),
                cimd_supported: true,
                token_endpoint: Some("https://auth.example.com/oauth/token".into()),
                registration_endpoint: None,
            });
    }

    #[tokio::test]
    async fn discovery_falls_back_and_refuses_unsafe_urls() {
        let d = discover_authorization(&Meta(HashMap::new()), "https://mcp.example.com/mcp").await.unwrap();
        assert_eq!(d, Discovery { authorization_endpoint: "https://mcp.example.com/authorize".into(), cimd_supported: false, token_endpoint: None, registration_endpoint: None });
        for bad in ["http://mcp.example.com/mcp", "https://127.0.0.1/mcp", "https://localhost/mcp", "https://mcp.example.com:9000/mcp"] {
            assert!(discover_authorization(&Meta(HashMap::new()), bad).await.is_err(), "{bad}");
        }
        // An auth server advertising an internal authorization endpoint is refused.
        let t = Meta(HashMap::from([(
            "mcp.example.com/.well-known/oauth-authorization-server",
            json!({"authorization_endpoint": "https://169.254.169.254/authorize"}),
        )]));
        assert!(discover_authorization(&t, "https://mcp.example.com/mcp").await.is_err());
    }

    #[test]
    fn cimd_is_only_used_where_the_server_can_fetch_it() {
        assert!(cimd_reachable("https://api.allternit.com"));
        assert!(cimd_reachable("https://api.allternit.com/oauth/client.json"));
        for unreachable in ["http://127.0.0.1:8013", "https://127.0.0.1:8013", "https://localhost:8013", "https://10.0.0.5", "https://192.168.1.2", "https://box.local", "http://api.allternit.com", "https://[::1]:8013", "not a url"] {
            assert!(!cimd_reachable(unreachable), "{unreachable}");
        }
        assert!(is_unreachable_cimd_client("http://127.0.0.1:8013/oauth/client.json"));
        assert!(!is_unreachable_cimd_client("https://api.allternit.com/oauth/client.json"));
        assert!(!is_unreachable_cimd_client("my-configured-client"));
    }

    #[test]
    fn oauth_start_persists_pkce_session_and_cimd_client() {
        let c = db();
        connector(&c, "conn-a", "alice");
        let disc = Discovery { authorization_endpoint: "https://auth.example.com/authorize".into(), cimd_supported: true, token_endpoint: Some("https://auth.example.com/token".into()), registration_endpoint: None };
        let base = "https://api.allternit.com";
        let url = persist_oauth_start(&c, "alice", "conn-a", "https://mcp.example.com/mcp", &disc, None, None, base).unwrap();
        let q: HashMap<_, _> = Url::parse(&url).unwrap().query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "https://api.allternit.com/oauth/client.json");
        assert_eq!(q["redirect_uri"], "https://api.allternit.com/mcp/oauth/callback");

        // The row the callback reads: same state, verifier hashes to the challenge.
        let (verifier, connector_id, authed): (String, String, i64) = c
            .query_row(
                "SELECT code_verifier, mcp_connector_id, is_authenticated FROM mcp_oauth_sessions WHERE state = ?1",
                [&q["state"]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((connector_id.as_str(), authed), ("conn-a", 0));
        assert_eq!(pkce_challenge(&verifier), q["code_challenge"]);
        let meta: String = c
            .query_row("SELECT metadata FROM mcp_oauth_sessions WHERE state = ?1", [&q["state"]], |r| r.get(0))
            .unwrap();
        let meta: Value = serde_json::from_str(&meta).unwrap();
        assert_eq!(meta["tokenEndpoint"], "https://auth.example.com/token");
        assert_eq!(meta["resource"], "https://mcp.example.com/mcp");
        assert!(verifier.len() >= 43);
        // Connector now presents the CIMD as its (public) client for the token exchange.
        let cid: Option<String> = c.query_row("SELECT oauth_client_id FROM mcp_connectors WHERE id='conn-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(cid.as_deref(), Some("https://api.allternit.com/oauth/client.json"));

        // Two starts never share state or verifier.
        let url2 = persist_oauth_start(&c, "alice", "conn-a", "https://mcp.example.com/mcp", &disc, cid.as_deref(), None, base).unwrap();
        assert_ne!(url, url2);
    }

    #[test]
    fn oauth_start_uses_configured_client_and_refuses_when_no_client_possible() {
        let c = db();
        connector(&c, "conn-a", "alice");
        let no_cimd = Discovery { authorization_endpoint: "https://auth.example.com/authorize".into(), cimd_supported: false, token_endpoint: None, registration_endpoint: None };
        let base = "https://api.allternit.com";
        assert!(matches!(
            persist_oauth_start(&c, "alice", "conn-a", "https://mcp.example.com/mcp", &no_cimd, None, None, base),
            Err(DirError::Conflict(_))
        ));
        let n: i64 = c.query_row("SELECT COUNT(*) FROM mcp_oauth_sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        let url = persist_oauth_start(&c, "alice", "conn-a", "https://mcp.example.com/mcp", &no_cimd, Some("preregistered"), None, base).unwrap();
        assert!(url.contains("client_id=preregistered"));
        let cid: Option<String> = c.query_row("SELECT oauth_client_id FROM mcp_connectors WHERE id='conn-a'", [], |r| r.get(0)).unwrap();
        assert_eq!(cid, None, "a configured client is never overwritten or invented");
    }

    #[test]
    fn oauth_start_is_scoped_to_the_connector_owner() {
        let c = db();
        connector(&c, "conn-a", "alice");
        assert!(load_connector(&c, "alice", "conn-a").is_ok());
        assert!(matches!(load_connector(&c, "bob", "conn-a"), Err(DirError::NotFound(_))));
    }

    // ── Dynamic Client Registration ─────────────────────────────────────────

    /// Serves auth-server metadata on GET and records / answers registration POSTs.
    struct DcrServer {
        meta: Meta,
        reply: Fetched,
        posts: std::sync::Mutex<Vec<(String, String)>>,
    }
    impl DcrServer {
        fn new(reply: Value) -> Self {
            Self {
                meta: Meta(HashMap::new()),
                reply: Fetched { status: 201, content_type: Some("application/json".into()), body: reply.to_string() },
                posts: Default::default(),
            }
        }
        fn count(&self) -> usize {
            self.posts.lock().unwrap().len()
        }
    }
    impl Transport for DcrServer {
        async fn resolve(&self, h: &str) -> Result<Vec<IpAddr>, String> {
            self.meta.resolve(h).await
        }
        async fn get(&self, u: &Url, a: IpAddr, c: &str, m: usize) -> Result<Fetched, String> {
            self.meta.get(u, a, c, m).await
        }
        async fn post_json(&self, u: &Url, _a: IpAddr, body: &str, _m: usize) -> Result<Fetched, String> {
            self.posts.lock().unwrap().push((u.to_string(), body.to_string()));
            Ok(self.reply.clone())
        }
    }

    const BASE: &str = "https://api.allternit.com";
    const MCP: &str = "https://mcp.example.com/mcp";

    fn dcr_discovery() -> Discovery {
        Discovery {
            authorization_endpoint: "https://auth.example.com/authorize".into(),
            cimd_supported: false,
            token_endpoint: Some("https://auth.example.com/token".into()),
            registration_endpoint: Some("https://auth.example.com/register".into()),
        }
    }

    #[tokio::test]
    async fn discovery_parses_and_guards_the_registration_endpoint() {
        let meta = |reg: &str| {
            Meta(HashMap::from([(
                "mcp.example.com/.well-known/oauth-authorization-server",
                json!({"authorization_endpoint": "https://auth.example.com/authorize", "registration_endpoint": reg}),
            )]))
        };
        let d = discover_authorization(&meta("https://auth.example.com/register"), MCP).await.unwrap();
        assert_eq!(d.registration_endpoint.as_deref(), Some("https://auth.example.com/register"));
        for bad in ["http://auth.example.com/register", "https://169.254.169.254/register", "https://localhost/register"] {
            assert!(discover_authorization(&meta(bad), MCP).await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn registration_posts_the_spec_body_and_requires_a_client_id() {
        let t = DcrServer::new(json!({"client_id": "abc123"}));
        let c = register_client(&t, "https://auth.example.com/register", BASE).await.unwrap();
        assert_eq!(c, DcrClient { client_id: "abc123".into(), client_secret: None, auth_method: "none".into() });
        let (url, body) = t.posts.lock().unwrap()[0].clone();
        assert_eq!(url, "https://auth.example.com/register");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["redirect_uris"], json!(["https://api.allternit.com/mcp/oauth/callback"]));
        assert_eq!(body["client_name"], "Allternit");
        assert_eq!(body["client_uri"], BASE);
        assert_eq!(body["grant_types"], json!(["authorization_code", "refresh_token"]));
        assert_eq!(body["response_types"], json!(["code"]));
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["application_type"], "web");

        for reply in [json!({}), json!({"client_id": ""}), json!({"client_id": 7})] {
            assert!(register_client(&DcrServer::new(reply), "https://auth.example.com/register", BASE).await.is_err());
        }
        let mut refused = DcrServer::new(json!({"error": "invalid_redirect_uri"}));
        refused.reply.status = 400;
        assert!(register_client(&refused, "https://auth.example.com/register", BASE).await.is_err());
    }

    #[tokio::test]
    async fn registration_refuses_unsafe_endpoints_and_never_posts() {
        let t = DcrServer::new(json!({"client_id": "x"}));
        for bad in ["http://auth.example.com/register", "https://127.0.0.1/register", "https://auth.example.com:8443/register"] {
            assert!(register_client(&t, bad, BASE).await.is_err(), "{bad}");
        }
        assert_eq!(t.count(), 0);
        // A private resolution is refused by the guard before any POST.
        struct Private;
        impl Transport for Private {
            async fn resolve(&self, _h: &str) -> Result<Vec<IpAddr>, String> {
                Ok(vec!["10.0.0.5".parse().unwrap()])
            }
            async fn get(&self, _u: &Url, _a: IpAddr, _c: &str, _m: usize) -> Result<Fetched, String> {
                unreachable!()
            }
            async fn post_json(&self, _u: &Url, _a: IpAddr, _b: &str, _m: usize) -> Result<Fetched, String> {
                panic!("must not connect")
            }
        }
        assert!(register_client(&Private, "https://auth.example.com/register", BASE).await.is_err());
    }

    #[tokio::test]
    async fn registration_accepts_a_server_issued_secret_and_method() {
        let t = DcrServer::new(json!({"client_id": "c", "client_secret": "s3", "token_endpoint_auth_method": "client_secret_post"}));
        let c = register_client(&t, "https://auth.example.com/register", BASE).await.unwrap();
        assert_eq!((c.client_secret.as_deref(), c.auth_method.as_str()), (Some("s3"), "client_secret_post"));
        let t = DcrServer::new(json!({"client_id": "c", "client_secret": "s3"}));
        assert_eq!(register_client(&t, "https://auth.example.com/register", BASE).await.unwrap().auth_method, "client_secret_basic");
        for bad in [
            json!({"client_id": "c", "token_endpoint_auth_method": "private_key_jwt"}),
            json!({"client_id": "c", "token_endpoint_auth_method": "client_secret_basic"}),
        ] {
            assert!(register_client(&DcrServer::new(bad), "https://auth.example.com/register", BASE).await.is_err());
        }
        assert!(!format!("{c:?}").contains("s3"));
    }

    fn stored(c: &Connection, id: &str) -> (Option<String>, Option<String>) {
        c.query_row("SELECT oauth_client_id, oauth_client_secret FROM mcp_connectors WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[test]
    fn dcr_start_persists_the_client_sealed_and_reuses_it() {
        let c = db();
        connector(&c, "conn-a", "alice");
        let disc = dcr_discovery();
        assert!(needs_registration(None, &disc));
        let issued = DcrClient { client_id: "dcr-1".into(), client_secret: Some("hush".into()), auth_method: "client_secret_basic".into() };
        let url = persist_oauth_start(&c, "alice", "conn-a", MCP, &disc, None, Some(&issued), BASE).unwrap();
        assert!(url.contains("client_id=dcr-1"));

        let (id, secret) = stored(&c, "conn-a");
        assert_eq!(id.as_deref(), Some("dcr-1"));
        let secret = secret.unwrap();
        assert!(!secret.contains("hush") || !token_crypto::encryption_enabled());
        assert_eq!(token_crypto::open(&secret), "hush");

        // Session carries the DCR marker and the auth method for the callback.
        let info: String = c.query_row("SELECT client_info FROM mcp_oauth_sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(dcr_auth_method(Some(&info)).as_deref(), Some("client_secret_basic"));

        // Second start: the stored client is used, nothing is registered again, the marker carries over.
        assert!(!needs_registration(id.as_deref(), &disc));
        let url2 = persist_oauth_start(&c, "alice", "conn-a", MCP, &disc, id.as_deref(), None, BASE).unwrap();
        assert!(url2.contains("client_id=dcr-1"));
        assert!(is_dcr_client(&c, "conn-a", "dcr-1"));
        assert_eq!(stored(&c, "conn-a").0.as_deref(), Some("dcr-1"));
        let infos: i64 = c
            .query_row("SELECT COUNT(*) FROM mcp_oauth_sessions WHERE json_extract(client_info,'$.dcr') = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(infos, 2);
    }

    #[test]
    fn dcr_clients_are_per_connector_row_never_shared_across_users() {
        let c = db();
        connector(&c, "conn-a", "alice");
        connector(&c, "conn-b", "bob");
        let disc = dcr_discovery();
        let alice = DcrClient { client_id: "alice-client".into(), client_secret: None, auth_method: "none".into() };
        persist_oauth_start(&c, "alice", "conn-a", MCP, &disc, None, Some(&alice), BASE).unwrap();
        // Bob's connector has no client of its own, so it needs its own registration.
        let (bob_id, _) = stored(&c, "conn-b");
        assert_eq!(bob_id, None);
        assert!(needs_registration(bob_id.as_deref(), &disc));
        assert!(matches!(persist_oauth_start(&c, "bob", "conn-b", MCP, &disc, None, None, BASE), Err(DirError::Conflict(_))));
        assert!(!is_dcr_client(&c, "conn-b", "alice-client"));
        // A client registered for alice's row cannot be written onto bob's.
        persist_oauth_start(&c, "alice", "conn-b", MCP, &disc, None, Some(&alice), BASE).unwrap();
        assert_eq!(stored(&c, "conn-b").0, None);
    }

    #[test]
    fn client_choice_order_is_configured_then_cimd_then_dcr_then_conflict() {
        let c = db();
        let issued = DcrClient { client_id: "dcr".into(), client_secret: None, auth_method: "none".into() };
        let mut both = dcr_discovery();
        both.cimd_supported = true;
        let cid = |url: &str| Url::parse(url).unwrap().query_pairs().find(|(k, _)| k == "client_id").unwrap().1.into_owned();

        connector(&c, "c1", "alice");
        let u = persist_oauth_start(&c, "alice", "c1", MCP, &both, Some("mine"), Some(&issued), BASE).unwrap();
        assert_eq!(cid(&u), "mine");
        assert!(!needs_registration(Some("mine"), &both));

        connector(&c, "c2", "alice");
        assert!(!needs_registration(None, &both), "CIMD wins over DCR");
        let u = persist_oauth_start(&c, "alice", "c2", MCP, &both, None, Some(&issued), BASE).unwrap();
        assert_eq!(cid(&u), client_metadata_url(BASE));

        connector(&c, "c3", "alice");
        let u = persist_oauth_start(&c, "alice", "c3", MCP, &dcr_discovery(), None, Some(&issued), BASE).unwrap();
        assert_eq!(cid(&u), "dcr");

        connector(&c, "c4", "alice");
        let mut none = dcr_discovery();
        none.registration_endpoint = None;
        assert!(!needs_registration(None, &none));
        assert!(matches!(persist_oauth_start(&c, "alice", "c4", MCP, &none, None, Some(&issued), BASE), Err(DirError::Conflict(_))));
    }

    #[test]
    fn invalid_client_clears_only_a_dcr_issued_client() {
        let c = db();
        connector(&c, "conn-a", "alice");
        let disc = dcr_discovery();
        let issued = DcrClient { client_id: "dcr-1".into(), client_secret: Some("hush".into()), auth_method: "client_secret_basic".into() };
        persist_oauth_start(&c, "alice", "conn-a", MCP, &disc, None, Some(&issued), BASE).unwrap();
        assert!(clear_dcr_client(&c, "conn-a", "dcr-1"));
        assert_eq!(stored(&c, "conn-a"), (None, None));
        assert!(needs_registration(None, &disc), "next start re-registers");

        // A user-configured client is never cleared, even on invalid_client.
        c.execute("UPDATE mcp_connectors SET oauth_client_id = 'mine', oauth_client_secret = 'x' WHERE id = 'conn-a'", []).unwrap();
        persist_oauth_start(&c, "alice", "conn-a", MCP, &disc, Some("mine"), None, BASE).unwrap();
        assert!(!clear_dcr_client(&c, "conn-a", "mine"));
        assert_eq!(stored(&c, "conn-a").0.as_deref(), Some("mine"));

        assert!(is_invalid_client(r#"{"error":"invalid_client"}"#));
        assert!(!is_invalid_client(r#"{"error":"invalid_grant"}"#));
    }
}
