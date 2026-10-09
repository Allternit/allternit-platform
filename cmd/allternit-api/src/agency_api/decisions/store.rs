//! The decision store (`decisions` table, migration V245), in open-systemone's
//! record shape: context hash, options, choice, backend, latency, attempts,
//! and the outcome filled in later through `PATCH /v1/decisions/:id`.

use crate::db::DbHandle;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub struct Row<'a> {
    pub id: &'a str,
    pub owner: &'a str,
    pub created_at: &'a str,
    pub kind: &'a str,
    pub session_id: Option<&'a str>,
    pub task: Option<&'a str>,
    pub context_hash: &'a str,
    pub has_image: bool,
    pub options: &'a Value,
    pub choice: Option<&'a str>,
    pub probs: &'a Value,
    pub confidence: f64,
    pub abstained: bool,
    pub backend: &'a str,
    pub latency_ms: f64,
    pub attempts: &'a Value,
}

pub fn insert(db: &DbHandle, r: &Row<'_>) -> rusqlite::Result<()> {
    db.connect()?.execute(
        "INSERT INTO decisions (id, owner, created_at, kind, session_id, task, context_hash, has_image, options_json,
            choice, probs_json, confidence, abstained, backend, latency_ms, attempts_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            r.id, r.owner, r.created_at, r.kind, r.session_id, r.task, r.context_hash, r.has_image,
            r.options.to_string(), r.choice, r.probs.to_string(), r.confidence, r.abstained, r.backend,
            r.latency_ms, r.attempts.to_string()
        ],
    )?;
    Ok(())
}

/// Record the outcome. `None` when the decision does not exist for `owner`.
pub fn set_outcome(
    db: &DbHandle,
    id: &str,
    owner: &str,
    status: &str,
    label: Option<&str>,
    detail: Option<&str>,
    at: &str,
) -> rusqlite::Result<Option<Value>> {
    let n = db.connect()?.execute(
        "UPDATE decisions SET outcome_status = ?3, outcome_label = ?4, outcome_detail = ?5, outcome_at = ?6
         WHERE id = ?1 AND owner = ?2",
        params![id, owner, status, label, detail, at],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get(db, id, owner)
}

pub fn get(db: &DbHandle, id: &str, owner: &str) -> rusqlite::Result<Option<Value>> {
    db.connect()?
        .query_row(
            "SELECT id, created_at, kind, session_id, task, context_hash, has_image, options_json, choice, probs_json,
                confidence, abstained, backend, latency_ms, attempts_json, outcome_status, outcome_label,
                outcome_detail, outcome_at
             FROM decisions WHERE id = ?1 AND owner = ?2",
            params![id, owner],
            |r| {
                let js = |i: usize| -> rusqlite::Result<Value> {
                    Ok(serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or(Value::Null))
                };
                let status: Option<String> = r.get(15)?;
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "object": "decision",
                    "created_at": r.get::<_, String>(1)?,
                    "kind": r.get::<_, String>(2)?,
                    "session_id": r.get::<_, Option<String>>(3)?,
                    "task": r.get::<_, Option<String>>(4)?,
                    "context_hash": r.get::<_, String>(5)?,
                    "has_image": r.get::<_, bool>(6)?,
                    "options": js(7)?,
                    "choice": r.get::<_, Option<String>>(8)?,
                    "probs": js(9)?,
                    "confidence": r.get::<_, f64>(10)?,
                    "abstained": r.get::<_, bool>(11)?,
                    "backend": r.get::<_, String>(12)?,
                    "latency_ms": r.get::<_, f64>(13)?,
                    "attempts": js(14)?,
                    "outcome": status.map(|s| -> rusqlite::Result<Value> {
                        Ok(json!({ "status": s, "label": r.get::<_, Option<String>>(16)?,
                            "detail": r.get::<_, Option<String>>(17)?, "at": r.get::<_, Option<String>>(18)? }))
                    }).transpose()?,
                }))
            },
        )
        .optional()
}
