//! Decision harvest (Agency Descent, Phase 1): opt-in training data for the
//! flywheel's trained heads.
//!
//! - **Opt-in per scope.** A scope is the caller's organization
//!   (`org:<id>`) or, without one, the user (`user:<id>`). Nothing is
//!   harvested until an org admin (or the user, for a personal scope) opts
//!   in with `PUT /v1/decisions/training`. Opting out deletes the scope's
//!   examples.
//! - **What is kept.** For an opted-in scope every decision stores its
//!   context, question and options, DLP-redacted (`llm_gateway::dlp_patterns`),
//!   plus the teacher's answer: the backend that served it.
//! - **Labels.** An outcome sets the label: its explicit `label`, or the
//!   served choice on `success`. A `failure` without a label records the
//!   choice as rejected. Without an outcome, a confident answer from a
//!   teacher backend (`oracle`, `open`, `vendor`, `typesafe`) is the label.
//! - **Shared heads.** `GET /v1/decisions/datasets/:kind` exports labelled
//!   examples as JSONL in the Jev/SystemOne shape (state, questions,
//!   answers), ready for an Unsloth decision fine-tune. `scope=mine` gives
//!   your scope's examples (org admin); `scope=all` pools every opted-in
//!   scope with no scope or owner ids in the rows (decisions admins only).

use super::super::{ApiError, ApiResult, RequestId};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::llm_gateway::dlp_patterns::{redact_text, scan_text};
use crate::AppState;
use axum::{
    extract::{Path, Query as QueryParams, State},
    http::header,
    response::IntoResponse,
    Extension, Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Backends whose confident answer may label an example with no outcome.
pub const TEACHERS: &[&str] = &["oracle", "open", "vendor", "typesafe"];

pub fn scope_of(user: &AuthUser) -> String {
    match user.organization_id.as_deref().filter(|o| !o.is_empty()) {
        Some(org) => format!("org:{org}"),
        None => format!("user:{}", user.user_id),
    }
}

pub fn opted_in(conn: &Connection, scope: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT opted_in FROM decision_training_optin WHERE scope = ?1", [scope], |r| r.get::<_, bool>(0))
        .optional()?
        .unwrap_or(false))
}

pub fn redact(text: &str) -> String {
    redact_text(text, &scan_text(text, &[]))
}

/// One decision to harvest, as served.
pub struct Example<'a> {
    pub decision_id: &'a str,
    pub scope: &'a str,
    pub kind: &'a str,
    pub created_at: &'a str,
    pub context: &'a str,
    pub question: Option<&'a str>,
    pub options: &'a Value,
    pub backend: &'a str,
    pub choice: Option<&'a str>,
    pub confidence: f64,
    pub threshold: f64,
}

/// Store the example when the scope opted in. Returns whether it was kept.
pub fn record(db: &DbHandle, e: &Example<'_>) -> rusqlite::Result<bool> {
    let conn = db.connect()?;
    if !opted_in(&conn, e.scope)? {
        return Ok(false);
    }
    let options: Vec<Value> = e
        .options
        .as_array()
        .map(|a| a.iter().map(|o| json!({ "id": o["id"], "text": redact(o["text"].as_str().unwrap_or_default()) })).collect())
        .unwrap_or_default();
    let teacher_label = (TEACHERS.contains(&e.backend) && e.confidence >= e.threshold).then_some(e.choice).flatten();
    conn.execute(
        "INSERT OR IGNORE INTO decision_examples (decision_id, scope, kind, created_at, context, question, options_json,
            teacher_backend, teacher_choice, teacher_confidence, label, label_source, labelled_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            e.decision_id,
            e.scope,
            e.kind,
            e.created_at,
            redact(e.context),
            e.question.map(redact),
            Value::from(options).to_string(),
            e.backend,
            e.choice,
            e.confidence,
            teacher_label,
            teacher_label.map(|_| "teacher"),
            teacher_label.map(|_| e.created_at),
        ],
    )?;
    Ok(true)
}

/// An outcome landed on a decision: relabel its example, if it has one.
pub fn on_outcome(conn: &Connection, decision_id: &str, status: &str, label: Option<&str>, at: &str) -> rusqlite::Result<()> {
    let Some(choice) = conn
        .query_row("SELECT teacher_choice FROM decision_examples WHERE decision_id = ?1", [decision_id], |r| r.get::<_, Option<String>>(0))
        .optional()?
    else {
        return Ok(());
    };
    let options: Vec<String> = conn.query_row("SELECT options_json FROM decision_examples WHERE decision_id = ?1", [decision_id], |r| {
        let s: String = r.get(0)?;
        Ok(serde_json::from_str::<Vec<Value>>(&s).unwrap_or_default().iter().filter_map(|o| o["id"].as_str().map(str::to_string)).collect())
    })?;
    let label = label.filter(|l| options.iter().any(|o| o == l)).map(str::to_string);
    match (status, label) {
        (_, Some(l)) => {
            conn.execute(
                "UPDATE decision_examples SET label = ?2, label_source = 'outcome', labelled_at = ?3 WHERE decision_id = ?1",
                params![decision_id, l, at],
            )?;
        }
        ("success", None) if choice.is_some() => {
            conn.execute(
                "UPDATE decision_examples SET label = teacher_choice, label_source = 'outcome', labelled_at = ?2 WHERE decision_id = ?1",
                params![decision_id, at],
            )?;
        }
        ("failure", None) => {
            // The served choice was wrong; a teacher label that agrees with it goes too.
            conn.execute(
                "UPDATE decision_examples SET rejected_choice = teacher_choice,
                    label = CASE WHEN label = teacher_choice THEN NULL ELSE label END,
                    label_source = CASE WHEN label = teacher_choice THEN NULL ELSE label_source END,
                    labelled_at = ?2
                 WHERE decision_id = ?1",
                params![decision_id, at],
            )?;
        }
        _ => {}
    }
    Ok(())
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn forbidden(msg: &str, rid: &RequestId) -> ApiError {
    ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", msg, rid)
}

/// Who may change or export a scope: its org's admins, or the user herself
/// for a personal scope.
fn may_manage(conn: &Connection, user: &AuthUser) -> rusqlite::Result<bool> {
    match user.organization_id.as_deref().filter(|o| !o.is_empty()) {
        Some(org) => crate::rbac::is_org_admin(conn, org, &user.user_id)
            .map(|admin| admin || crate::rbac::is_admin_role(user.organization_role.as_deref())),
        None => Ok(true),
    }
}

fn status(conn: &Connection, scope: &str) -> rusqlite::Result<Value> {
    let count = |sql: &str| conn.query_row(sql, [scope], |r| r.get::<_, i64>(0));
    Ok(json!({
        "object": "decision_training",
        "scope": scope,
        "opted_in": opted_in(conn, scope)?,
        "examples": count("SELECT COUNT(*) FROM decision_examples WHERE scope = ?1")?,
        "labelled": count("SELECT COUNT(*) FROM decision_examples WHERE scope = ?1 AND label IS NOT NULL")?,
    }))
}

pub async fn get_training(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
) -> ApiResult {
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    status(&conn, &scope_of(&user)).map(|v| Json(v).into_response()).map_err(|e| ApiError::internal(e, &rid))
}

#[derive(Deserialize)]
pub struct TrainingIn {
    opted_in: bool,
}

pub async fn put_training(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Json(body): Json<TrainingIn>,
) -> ApiResult {
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    if !may_manage(&conn, &user).map_err(|e| ApiError::internal(e, &rid))? {
        return Err(forbidden("only an organization admin can change decision training for the organization", &rid));
    }
    let scope = scope_of(&user);
    conn.execute(
        "INSERT INTO decision_training_optin (scope, opted_in, actor, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(scope) DO UPDATE SET opted_in = excluded.opted_in, actor = excluded.actor, updated_at = excluded.updated_at",
        params![scope, body.opted_in, user.user_id, super::super::store::now()],
    )
    .map_err(|e| ApiError::internal(e, &rid))?;
    if !body.opted_in {
        conn.execute("DELETE FROM decision_examples WHERE scope = ?1", [&scope]).map_err(|e| ApiError::internal(e, &rid))?;
    }
    status(&conn, &scope).map(|v| Json(v).into_response()).map_err(|e| ApiError::internal(e, &rid))
}

#[derive(Deserialize)]
pub struct DatasetQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// One labelled example in the Jev/SystemOne record shape.
pub fn to_record(kind: &str, context: &str, question: Option<&str>, options_json: &str, label: &str, source: &str) -> Value {
    let options: Vec<Value> = serde_json::from_str(options_json).unwrap_or_default();
    let criteria: serde_json::Map<String, Value> =
        options.iter().filter_map(|o| Some((o["id"].as_str()?.to_string(), o["text"].clone()))).collect();
    json!({
        "state": context,
        "questions": { "decision": {
            "type": "choice",
            "instructions": question.map(str::to_string).unwrap_or_else(|| format!("Decision kind: {kind}. Choose the best option.")),
            "criteria": criteria,
        }},
        "answers": { "decision": { "choice": label } },
        "meta": { "kind": kind, "label_source": source },
    })
}

type ExampleRow = (String, Option<String>, String, String, Option<String>, String);

/// Labelled rows of one kind, oldest first: context, question, options JSON,
/// label, label source. `scope: None` pools every opted-in scope.
fn labelled_rows(conn: &Connection, kind: &str, scope: Option<&str>, limit: usize) -> rusqlite::Result<Vec<ExampleRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.context, e.question, e.options_json, e.label, e.label_source, e.decision_id
         FROM decision_examples e JOIN decision_training_optin o ON o.scope = e.scope AND o.opted_in = 1
         WHERE e.kind = ?1 AND e.label IS NOT NULL AND (?2 IS NULL OR e.scope = ?2)
         ORDER BY e.created_at LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![kind, scope, limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
    rows.collect()
}

pub fn export(conn: &Connection, kind: &str, scope: Option<&str>, limit: usize) -> rusqlite::Result<Vec<Value>> {
    Ok(labelled_rows(conn, kind, scope, limit)?
        .iter()
        .map(|(context, question, options, label, source, _)| {
            to_record(kind, context, question.as_deref(), options, label, source.as_deref().unwrap_or_default())
        })
        .collect())
}

/// One labelled example as the trainers read it: the same rows as
/// [`export`], with the options kept in the order they were offered.
#[derive(Clone, Debug)]
pub struct Labelled {
    pub context: String,
    pub question: Option<String>,
    /// (id, text) in offered order.
    pub options: Vec<(String, String)>,
    pub label: String,
    pub source: String,
    /// The decision it was harvested from (never exported).
    pub decision_id: String,
}

/// The rows of [`export`] for training. Rows whose label is not one of their
/// options are dropped.
pub fn labelled(conn: &Connection, kind: &str, scope: Option<&str>, limit: usize) -> rusqlite::Result<Vec<Labelled>> {
    Ok(labelled_rows(conn, kind, scope, limit)?
        .into_iter()
        .filter_map(|(context, question, options, label, source, decision_id)| {
            let options: Vec<(String, String)> = serde_json::from_str::<Vec<Value>>(&options)
                .unwrap_or_default()
                .iter()
                .filter_map(|o| Some((o["id"].as_str()?.to_string(), o["text"].as_str().unwrap_or_default().to_string())))
                .collect();
            options.iter().any(|(id, _)| *id == label).then(|| Labelled { context, question, options, label, source: source.unwrap_or_default(), decision_id })
        })
        .collect())
}

pub async fn get_dataset(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(kind): Path<String>,
    QueryParams(q): QueryParams<DatasetQuery>,
) -> ApiResult {
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    let pooled = q.scope.as_deref() == Some("all");
    let scope = if pooled {
        if !super::flywheel::is_flywheel_admin(&user.user_id) {
            return Err(forbidden("only a decisions admin can export the pooled dataset", &rid));
        }
        None
    } else {
        if !may_manage(&conn, &user).map_err(|e| ApiError::internal(e, &rid))? {
            return Err(forbidden("only an organization admin can export the organization's dataset", &rid));
        }
        Some(scope_of(&user))
    };
    let rows = export(&conn, &kind, scope.as_deref(), q.limit.unwrap_or(10_000).clamp(1, 100_000))
        .map_err(|e| ApiError::internal(e, &rid))?;
    let body: String = rows.iter().map(|r| format!("{r}\n")).collect();
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> DbHandle {
        let path = std::env::temp_dir().join(format!("allternit-harvest-{}.db", uuid::Uuid::new_v4()));
        DbHandle::new(path).unwrap()
    }

    fn opt(conn: &Connection, scope: &str, on: bool) {
        conn.execute(
            "INSERT OR REPLACE INTO decision_training_optin (scope, opted_in, actor, updated_at) VALUES (?1, ?2, 'u', 't')",
            params![scope, on],
        )
        .unwrap();
    }

    fn example<'a>(id: &'a str, scope: &'a str, backend: &'a str, conf: f64, options: &'a Value) -> Example<'a> {
        Example {
            decision_id: id,
            scope,
            kind: "route",
            created_at: "2026-10-12T00:00:00Z",
            context: "Customer jane@example.com was charged twice, card 4111 1111 1111 1111.",
            question: Some("Which team?"),
            options,
            backend,
            choice: Some("billing"),
            confidence: conf,
            threshold: 0.6,
        }
    }

    #[test]
    fn harvests_only_opted_in_scopes_redacted_and_labelled() {
        let db = db();
        let conn = db.connect().unwrap();
        let options = json!([{ "id": "billing", "text": "Charges and refunds" }, { "id": "tech", "text": "App faults" }]);
        opt(&conn, "org:a", true);
        opt(&conn, "org:b", false);

        // Not opted in (explicitly, or never): nothing kept.
        assert!(!record(&db, &example("d0", "org:b", "oracle", 1.0, &options)).unwrap());
        assert!(!record(&db, &example("d0", "org:c", "oracle", 1.0, &options)).unwrap());

        // Opted in: kept, redacted; a confident teacher labels it.
        assert!(record(&db, &example("d1", "org:a", "oracle", 1.0, &options)).unwrap());
        let ctx: String = conn.query_row("SELECT context FROM decision_examples WHERE decision_id = 'd1'", [], |r| r.get(0)).unwrap();
        assert!(!ctx.contains("4111 1111 1111 1111"), "card number must be redacted: {ctx}");
        assert!(ctx.contains("[REDACTED:"), "{ctx}");

        // A non-teacher (local scorer) answer waits for an outcome.
        assert!(record(&db, &example("d2", "org:a", "local", 0.9, &options)).unwrap());
        let rows = export(&conn, "route", Some("org:a"), 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["answers"]["decision"]["choice"], "billing");
        assert_eq!(rows[0]["meta"]["label_source"], "teacher");
        assert_eq!(rows[0]["questions"]["decision"]["criteria"]["tech"], "App faults");

        // Outcomes: success labels d2; failure removes d1's teacher label; an
        // explicit label wins.
        on_outcome(&conn, "d2", "success", None, "t2").unwrap();
        on_outcome(&conn, "d1", "failure", None, "t2").unwrap();
        let labels: Vec<(String, Option<String>, Option<String>)> = conn
            .prepare("SELECT decision_id, label, rejected_choice FROM decision_examples ORDER BY decision_id").unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(labels, vec![
            ("d1".into(), None, Some("billing".into())),
            ("d2".into(), Some("billing".into()), None),
        ]);
        on_outcome(&conn, "d1", "failure", Some("tech"), "t3").unwrap();
        on_outcome(&conn, "d2", "success", Some("not-an-option"), "t3").unwrap();
        let rows = export(&conn, "route", None, 100).unwrap();
        let got: Vec<&str> = rows.iter().map(|r| r["answers"]["decision"]["choice"].as_str().unwrap()).collect();
        assert_eq!(got, vec!["tech", "billing"]);
        assert!(rows.iter().all(|r| r.get("scope").is_none() && r["meta"].get("scope").is_none()));

        // Opting a scope out (row flips to 0) drops it from every export.
        opt(&conn, "org:a", false);
        assert!(export(&conn, "route", None, 100).unwrap().is_empty());
    }
}
