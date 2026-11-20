//! The cycle report, real spend and undo (step 5.5, decision D11).
//!
//! - [`build`] turns the `report_json` that `cycle.rs` writes into the
//!   `GET /memory/sleep/cycles/:id` report: every stage of the cycle's plan with
//!   its outcome (`ran`, `skipped` + reason, `failed`, `held`, `not_run`),
//!   counts, spend, and the links undo needs (Dream id and its two commits,
//!   manifest before and after).
//! - [`summary`] is the one-line text on list rows and on `status.last_cycle`.
//! - **Real spend.** A stage's spend is read from the usage ledger
//!   ([`super::cycle::ledger_spend_usd`]). The cycle reads it as the stage
//!   returns; ledger rows can land after that, so [`build`] reads the stage's
//!   window again and, when more arrived, raises the stage and cycle spend
//!   (`sleep_cycles.spend_usd`, which the daily cap reads).
//! - [`undo`] undoes the Dream part through Dream's own undo
//!   (`memory_dream::undo`, which reverses only the Dream's lines and keeps
//!   later edits) and rolls back the page manifest when one exists. Consolidation
//!   and curation have no undo; the report lists them under `cannot_undo`
//!   instead of pretending.

use chrono::Duration;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::cycle::{ledger_spend_usd, plan_for};
use super::queue;
use super::strength::parse_ts;
use super::types::CycleSummary;
use crate::db::DbHandle;
use crate::memory_dream::UndoError as DreamUndo;

/// How long after a stage ends the report still looks for its ledger rows.
const LATE_SPEND_GRACE_S: i64 = 120;

#[derive(Debug, Clone, Serialize)]
pub struct StageReport {
    pub stage: String,
    /// `ran`, `skipped`, `failed`, `held` (a clean stop left it for the next
    /// signal) or `not_run`.
    pub outcome: &'static str,
    pub reason: Option<String>,
    pub spend_usd: f64,
    pub counts: Value,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Keeps the listed keys of a stage's detail; an array becomes its length.
fn pick(detail: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        if let Some(v) = detail.get(*k) {
            m.insert((*k).to_string(), if let Value::Array(a) = v { json!(a.len()) } else { v.clone() });
        }
    }
    Value::Object(m)
}

fn counts(stage: &str, d: &Value) -> Value {
    match stage {
        "capture" => json!({ "captured": d.get("captured").and_then(Value::as_array).map_or(0, |a| a.len()) }),
        "consolidation" => pick(d, &["scanned", "candidates", "merged", "decayed", "shadow_decisions"]),
        "fit" => pick(d, &["events", "fitted", "kept_owner"]),
        "dream" => d.get("summary").cloned().unwrap_or_else(|| json!({})),
        "rem" => pick(d, &["seeds", "links_proposed", "links_kept", "links_rejected", "lessons_kept", "lessons_rejected", "keep_rate"]),
        "curation" => pick(d, &["bots"]),
        _ => json!({}),
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// Every stage of the cycle's plan, in order.
pub fn stages(kind: &str, state: &str, report: &Value) -> Vec<StageReport> {
    let steps = report.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let error = s(report, "error");
    plan_for(kind)
        .iter()
        .map(|step| {
            let name = step.as_str();
            if let Some(rec) = steps.iter().find(|r| r.get("step").and_then(Value::as_str) == Some(name)) {
                let detail = rec.get("detail").cloned().unwrap_or(Value::Null);
                let done = rec.get("status").and_then(Value::as_str) == Some("done");
                return StageReport {
                    stage: name.into(),
                    outcome: if done { "ran" } else { "skipped" },
                    reason: s(rec, "note"),
                    spend_usd: rec.get("spend_usd").and_then(Value::as_f64).unwrap_or(0.0),
                    counts: if done { counts(name, &detail) } else { json!({}) },
                    started_at: s(rec, "started_at"),
                    finished_at: s(rec, "finished_at"),
                };
            }
            let (outcome, reason) = if error.as_deref().is_some_and(|e| e.starts_with(&format!("{name}:"))) {
                ("failed", error.clone())
            } else if state == "stopped" {
                ("held", s(report, "stop_reason"))
            } else if state == "failed" {
                ("not_run", Some("an earlier stage failed".to_string()))
            } else {
                ("not_run", Some("ran in the cycle before a stop".to_string()))
            };
            StageReport { stage: name.into(), outcome, reason, spend_usd: 0.0, counts: json!({}), started_at: None, finished_at: None }
        })
        .collect()
}

fn count(st: &[StageReport], stage: &str, key: &str) -> i64 {
    st.iter().find(|x| x.stage == stage && x.outcome == "ran").and_then(|x| x.counts.get(key)).and_then(Value::as_i64).unwrap_or(0)
}

fn ran(st: &[StageReport], stage: &str) -> bool {
    st.iter().any(|x| x.stage == stage && x.outcome == "ran")
}

/// What no undo covers in this cycle. Consolidation and curation write
/// through their own paths and have no undo; nothing is invented for them.
fn cannot_undo(st: &[StageReport]) -> Vec<String> {
    let mut out = vec![];
    if count(st, "consolidation", "merged") + count(st, "consolidation", "decayed") > 0 {
        out.push("consolidation: merged or decayed facts have no undo".to_string());
    }
    if ran(st, "curation") {
        out.push("curation: bot memory edits have no undo".to_string());
    }
    out
}

/// The Dream this cycle ran, when it has a commit to undo.
fn dream_link(report: &Value) -> Option<Value> {
    let rec = report.get("steps")?.as_array()?.iter().find(|r| {
        r.get("step").and_then(Value::as_str) == Some("dream") && r.get("status").and_then(Value::as_str) == Some("done")
    })?;
    let d = rec.get("detail")?;
    let id = s(d, "dream_id")?;
    Some(json!({
        "dream_id": id,
        "status": d.get("status").cloned().unwrap_or(Value::Null),
        "revision": d.get("revision").cloned().unwrap_or(Value::Null),
        "base_revision": d.get("base_revision").cloned().unwrap_or(Value::Null),
    }))
}

fn parse(report_json: Option<&str>) -> Value {
    report_json.and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null)
}

fn original_state(report: &Value, column: &str) -> String {
    s(report, "state").unwrap_or_else(|| column.to_string())
}

/// One line for list rows and `status.last_cycle`; `None` while it runs.
pub fn summary(kind: &str, state: &str, spend_usd: f64, undone: bool, report_json: Option<&str>) -> Option<String> {
    if state == "running" {
        return None;
    }
    let report = parse(report_json);
    let state = original_state(&report, state);
    let st = stages(kind, &state, &report);
    let mut bits: Vec<String> = vec![];
    let n = count(&st, "capture", "captured");
    if n > 0 {
        bits.push(format!("{n} captured"));
    }
    if ran(&st, "consolidation") {
        bits.push(format!("consolidation merged {}, decayed {}", count(&st, "consolidation", "merged"), count(&st, "consolidation", "decayed")));
    }
    let n = count(&st, "fit", "fitted");
    if n > 0 {
        bits.push(format!("fitted {n} keys"));
    }
    if let Some(d) = st.iter().find(|x| x.stage == "dream") {
        match d.outcome {
            "ran" => bits.push(format!(
                "Dream {} changes",
                ["merged", "resolved", "lessons", "pruned"].iter().map(|k| d.counts.get(*k).and_then(Value::as_i64).unwrap_or(0)).sum::<i64>()
            )),
            "skipped" if d.reason.as_deref() == Some("spend_cap") => bits.push("Dream skipped (spend cap)".into()),
            _ => {}
        }
    }
    if ran(&st, "curation") {
        bits.push(format!("curated {} bots", count(&st, "curation", "bots")));
    }
    if state == "failed" {
        bits.push(format!("failed: {}", s(&report, "error").unwrap_or_default()));
    } else if state == "stopped" {
        bits.push(format!("stopped ({})", s(&report, "stop_reason").unwrap_or_default()));
    }
    if bits.is_empty() {
        bits.push("nothing to do".into());
    }
    let mut line = format!("{kind} {state}: {}", bits.join("; "));
    if spend_usd > 0.0 {
        line.push_str(&format!("; spent ${spend_usd:.4}"));
    }
    if undone {
        line = format!("Undone. {line}");
    }
    Some(line)
}

/// The full report for one cycle. Also reads the usage ledger again for late
/// spend and records it on the cycle (and in `cycle.spend_usd`). `None` while
/// the cycle has written no report yet.
pub fn build(conn: &Connection, user: &str, cycle: &mut CycleSummary, report_json: Option<&str>) -> rusqlite::Result<Option<Value>> {
    let report = parse(report_json);
    if report.is_null() {
        return Ok(None);
    }
    let state = original_state(&report, &cycle.state);
    let mut st = stages(&cycle.kind, &state, &report);
    let grace = Duration::seconds(LATE_SPEND_GRACE_S);
    for i in 0..st.len() {
        if !(matches!(st[i].stage.as_str(), "dream" | "curation") && st[i].outcome == "ran") {
            continue;
        }
        let (Some(from), Some(end)) = (st[i].started_at.clone(), st[i].finished_at.as_deref().and_then(parse_ts)) else { continue };
        let mut upper = end + grace;
        if let Some(next) = st[i + 1..].iter().find_map(|x| x.started_at.as_deref().and_then(parse_ts)) {
            upper = upper.min(next);
        }
        // The stage's own last second always counts.
        let upper = upper.max(end + Duration::seconds(1));
        let real = ledger_spend_usd(conn, user, &from, &queue::ts(upper))?;
        if real > st[i].spend_usd {
            st[i].spend_usd = real;
        }
    }
    let total: f64 = st.iter().map(|x| x.spend_usd).sum();
    if total > cycle.spend_usd + 1e-9 {
        conn.execute("UPDATE sleep_cycles SET spend_usd = ?2 WHERE id = ?1", params![cycle.id, total])?;
        cycle.spend_usd = total;
    }
    let undone = cycle.undone_at.is_some();
    let available = !undone && matches!(cycle.state.as_str(), "finished" | "stopped");
    let undo_log = report.get("undo").cloned().unwrap_or(Value::Null);
    Ok(Some(json!({
        "cycle_id": cycle.id,
        "kind": cycle.kind,
        "state": state,
        "summary": cycle.summary,
        "spend_usd": cycle.spend_usd,
        "stop_reason": report.get("stop_reason").cloned().unwrap_or(Value::Null),
        "resume_from_stage": report.get("resume_from_stage").cloned().unwrap_or(Value::Null),
        "error": report.get("error").cloned().unwrap_or(Value::Null),
        "stages": st,
        "undo": {
            "available": available,
            "undone_at": cycle.undone_at,
            "dream": dream_link(&report),
            "manifest": { "before": cycle.manifest_before, "after": cycle.manifest_after },
            "cannot_undo": cannot_undo(&st),
            "result": undo_log,
        },
    })))
}

// ── Undo ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum UndoError {
    NotFound,
    AlreadyUndone,
    NotUndoable(String),
    /// Dream lines were edited after the Dream; the paths that conflict.
    Conflict(Vec<String>),
    Failed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoPart {
    pub part: &'static str,
    /// `undone`, `nothing_to_undo`, `recorded_no_op` or `cannot_undo`.
    pub status: &'static str,
    pub detail: String,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoResult {
    pub cycle_id: String,
    pub undone_at: String,
    pub parts: Vec<UndoPart>,
    pub cannot_undo: Vec<String>,
}

fn part(part: &'static str, status: &'static str, detail: impl Into<String>, revision: Option<String>) -> UndoPart {
    UndoPart { part, status, detail: detail.into(), revision }
}

fn failed(e: impl std::fmt::Display) -> UndoError {
    UndoError::Failed(e.to_string())
}

/// Undoes a `finished` or `stopped` cycle of `user`. The cycle is claimed first
/// (`undone_at`, state `undone`) so two undos cannot both run; a Dream
/// conflict or error gives the claim back and leaves everything as it was.
pub fn undo(db: &DbHandle, user: &str, id: &str) -> Result<UndoResult, UndoError> {
    let conn = db.connect().map_err(failed)?;
    let row: Option<(String, String, Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT state, kind, undone_at, report_json, manifest_before FROM sleep_cycles WHERE user_id = ?1 AND id = ?2",
            params![user, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(failed)?;
    let Some((state, kind, undone_at, report_json, manifest_before)) = row else { return Err(UndoError::NotFound) };
    if undone_at.is_some() || state == "undone" {
        return Err(UndoError::AlreadyUndone);
    }
    if !matches!(state.as_str(), "finished" | "stopped") {
        return Err(UndoError::NotUndoable(format!("A {state} cycle cannot be undone; only a finished or stopped one.")));
    }
    let claimed = conn
        .execute(
            "UPDATE sleep_cycles SET state = 'undone', undone_at = CURRENT_TIMESTAMP
             WHERE id = ?1 AND user_id = ?2 AND undone_at IS NULL AND state IN ('finished', 'stopped')",
            params![id, user],
        )
        .map_err(failed)?;
    if claimed == 0 {
        return Err(UndoError::AlreadyUndone);
    }
    let give_back = || {
        let _ = conn.execute("UPDATE sleep_cycles SET state = ?3, undone_at = NULL WHERE id = ?1 AND user_id = ?2", params![id, user, state]);
    };

    let report = parse(report_json.as_deref());
    let st = stages(&kind, &original_state(&report, &state), &report);
    let mut parts = vec![];

    // Dream: its own undo reverses exactly the Dream's lines and keeps later edits.
    match dream_link(&report) {
        None => parts.push(part("dream", "nothing_to_undo", "Dream did not run in this cycle.", None)),
        Some(link) if link["status"] != json!("applied") => {
            parts.push(part("dream", "nothing_to_undo", format!("Dream ended `{}`; it applied nothing.", link["status"].as_str().unwrap_or("unknown")), None));
        }
        Some(link) => {
            let dream_id = link["dream_id"].as_str().unwrap_or_default();
            match crate::memory_dream::undo(db, user, dream_id) {
                Ok(row) => parts.push(part("dream", "undone", format!("Dream of {} undone; later edits kept.", row.date), row.undo_revision)),
                // Undone already from the Dream screen: nothing left to do here.
                Err(DreamUndo::NotUndoable(why)) => parts.push(part("dream", "nothing_to_undo", why, None)),
                Err(DreamUndo::Conflict(paths)) => {
                    give_back();
                    return Err(UndoError::Conflict(paths));
                }
                Err(DreamUndo::Service(e)) => {
                    give_back();
                    return Err(failed(e));
                }
            }
        }
    }

    // Page manifest. Pages and manifests arrive with step 4.1; until a cycle
    // writes one, `manifest_before` is null and this is a recorded no-op.
    // TODO(4.1): restore the manifest named by `manifest_before` here and add
    // the manifest-rollback assertion to the undo test.
    parts.push(match manifest_before {
        None => part("manifest", "recorded_no_op", "This cycle wrote no page manifest.", None),
        Some(m) => part("manifest", "cannot_undo", format!("Manifest `{m}` cannot be restored yet: manifest rollback lands with step 4.1."), None),
    });

    let cannot = cannot_undo(&st);
    for c in &cannot {
        let (name, why) = c.split_once(": ").unwrap_or(("cycle", c.as_str()));
        parts.push(part(if name == "curation" { "curation" } else { "consolidation" }, "cannot_undo", why, None));
    }

    let undone_at: String = conn
        .query_row("SELECT undone_at FROM sleep_cycles WHERE id = ?1", params![id], |r| r.get(0))
        .map_err(failed)?;
    if !report.is_null() {
        let mut report = report;
        report["undo"] = json!({ "undone_at": undone_at, "parts": parts, "cannot_undo": cannot });
        conn.execute("UPDATE sleep_cycles SET report_json = ?2 WHERE id = ?1", params![id, report.to_string()]).map_err(failed)?;
    }
    Ok(UndoResult { cycle_id: id.to_string(), undone_at, parts, cannot_undo: cannot })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_drive_service as service;
    use crate::memory_kernel_service as kernel;
    use crate::sleep::cycle::{dream_detail, CycleReport, Step, StepRecord};

    struct Fixture {
        _dir: tempfile::TempDir,
        db: DbHandle,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let db = DbHandle::new(base.join("test.db")).expect("db");
        crate::memory_drive_writer::configure_root(&db, &base.join("brains")).unwrap();
        Fixture { _dir: dir, db }
    }

    fn obs(db: &DbHandle) -> String {
        kernel::record_observation(db, "u1", None, None, "turn_user", "x", Some("user")).unwrap()
    }

    fn active_facts(db: &DbHandle) -> Vec<String> {
        let mut v: Vec<String> = kernel::list_facts(db, "u1", None, 100).unwrap().into_iter().map(|f| f.fact).collect();
        v.sort();
        v
    }

    fn entry_id(db: &DbHandle, fact: &str) -> String {
        db.connect().unwrap().query_row("SELECT entry_id FROM memory_drive_entries WHERE fact_id=?1", params![fact], |r| r.get(0)).unwrap()
    }

    /// A stage record; each stage starts 10 s after the one before it and lasts 3 s.
    fn rec(step: Step, status: &'static str, note: Option<&str>, detail: Value) -> StepRecord {
        let i = crate::sleep::cycle::NAP.iter().position(|s| *s == step).unwrap();
        StepRecord {
            step, status, note: note.map(str::to_string), spend_usd: 0.0, detail,
            started_at: Some(format!("2026-11-19 12:00:{:02}", i * 10)), finished_at: Some(format!("2026-11-19 12:00:{:02}", i * 10 + 3)),
        }
    }

    /// Writes a closed cycle row the way `cycle::close_cycle` does.
    fn cycle_row(db: &DbHandle, id: &str, user: &str, state: &'static str, steps: Vec<StepRecord>) {
        let report = CycleReport {
            cycle_id: Some(id.into()), unit_id: "unit".into(), user_id: user.into(), kind: "nap".into(), state, steps,
            spend_usd: 0.0, stop_reason: (state == "stopped").then(|| "resume".to_string()),
            resume_from_stage: (state == "stopped").then_some("dream"), held_unit: None, error: None,
        };
        db.connect().unwrap().execute(
            "INSERT INTO sleep_cycles (id, user_id, trigger, kind, state, started_at, finished_at, report_json)
             VALUES (?1, ?2, 'idle', 'nap', ?3, '2026-11-19 12:00:00', '2026-11-19 12:00:09', ?4)",
            params![id, user, state, json!(report).to_string()],
        ).unwrap();
    }

    fn state_of(db: &DbHandle, id: &str) -> (String, Option<String>) {
        db.connect().unwrap().query_row("SELECT state, undone_at FROM sleep_cycles WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    #[tokio::test]
    async fn undo_restores_the_previous_drive_revision_and_keeps_later_user_edits() {
        let f = fixture();
        let o = obs(&f.db);
        let ids: Vec<String> = kernel::persist_facts(&f.db, "u1", None, &o, &["User likes tea.".into(), "User enjoys tea.".into()])
            .unwrap().into_iter().map(|x| x.id).collect();
        let (keep, drop) = (entry_id(&f.db, &ids[0]), entry_id(&f.db, &ids[1]));
        let turn = obs(&f.db);
        let plan = json!({
            "merges": [{"keep": keep, "drop": [drop], "text": "User likes tea."}],
            "contradictions": [], "prune": [], "twin_proposals": [],
            "lessons": [{"text": "User wants short answers.", "evidence": [turn]}]
        }).to_string();
        let complete: crate::memory_dream::Complete = std::sync::Arc::new(move |_p| {
            let plan = plan.clone();
            Box::pin(async move { Some(plan) })
        });
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 19).unwrap();
        // The Dream runs through its real apply path.
        let dream = crate::memory_dream::run(f.db.clone(), "u1".into(), date, complete).await.unwrap().unwrap();
        assert_eq!(dream.status, "applied");
        assert_eq!(active_facts(&f.db), vec!["User likes tea."]);

        cycle_row(&f.db, "cyc_1", "u1", "finished", vec![
            rec(Step::Consolidation, "done", None, json!({ "merged": 0, "decayed": 0 })),
            rec(Step::Dream, "done", None, dream_detail(date, Some(dream.clone()))),
        ]);
        // A later user write survives the undo.
        let o = obs(&f.db);
        kernel::persist_facts(&f.db, "u1", None, &o, &["User moved to Saint Paul.".into()]).unwrap();

        // Another user cannot undo it.
        assert!(matches!(undo(&f.db, "someone-else", "cyc_1"), Err(UndoError::NotFound)));
        let r = undo(&f.db, "u1", "cyc_1").unwrap();
        let by = |p: &str| r.parts.iter().find(|x| x.part == p).unwrap();
        assert_eq!((by("dream").status, by("manifest").status), ("undone", "recorded_no_op"));
        assert!(by("dream").revision.is_some());
        // The Drive is back to the revision before the Dream, plus the later edit.
        assert_eq!(active_facts(&f.db), vec!["User enjoys tea.", "User likes tea.", "User moved to Saint Paul."]);
        let files = service::resolve(&f.db, "u1").unwrap().storage().unwrap().snapshot(None).unwrap().files;
        assert!(!files.values().any(|t| t.contains("short answers")), "the Dream's lesson is removed");
        let (state, undone_at) = state_of(&f.db, "cyc_1");
        assert_eq!(state, "undone");
        assert!(undone_at.is_some());
        let status: String = f.db.connect().unwrap().query_row("SELECT status FROM memory_dreams WHERE id = ?1", [&dream.id], |r| r.get(0)).unwrap();
        assert_eq!(status, "undone");
        // TODO(4.1): when pages and manifests land, assert the manifest is rolled back to `manifest_before` here.

        // The report now shows the undo.
        let conn = f.db.connect().unwrap();
        let (mut c, rj) = (cycle_summary(&conn, "cyc_1"), report_json(&conn, "cyc_1"));
        let rep = build(&conn, "u1", &mut c, rj.as_deref()).unwrap().unwrap();
        assert_eq!(rep["undo"]["available"], json!(false));
        assert_eq!(rep["undo"]["result"]["parts"][0]["part"], json!("dream"));
        assert!(c.summary.unwrap().starts_with("Undone."));
    }

    #[tokio::test]
    async fn a_second_undo_is_refused() {
        let f = fixture();
        cycle_row(&f.db, "cyc_1", "u1", "finished", vec![rec(Step::Consolidation, "done", None, json!({ "merged": 2, "decayed": 1 }))]);
        let r = undo(&f.db, "u1", "cyc_1").unwrap();
        // Consolidation has no undo: said plainly, not invented.
        assert_eq!(r.cannot_undo.len(), 1, "{r:?}");
        assert!(r.parts.iter().any(|p| p.part == "consolidation" && p.status == "cannot_undo"));
        assert!(matches!(undo(&f.db, "u1", "cyc_1"), Err(UndoError::AlreadyUndone)));
        // Only a finished or stopped cycle can be undone.
        cycle_row(&f.db, "cyc_2", "u1", "failed", vec![]);
        assert!(matches!(undo(&f.db, "u1", "cyc_2"), Err(UndoError::NotUndoable(_))));
        assert_eq!(state_of(&f.db, "cyc_2").0, "failed");
    }

    fn cycle_summary(conn: &Connection, id: &str) -> CycleSummary {
        let (state, kind, spend, before, after, started, finished, undone, rj): (String, String, f64, Option<String>, Option<String>, String, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT state, kind, spend_usd, manifest_before, manifest_after, started_at, finished_at, undone_at, report_json FROM sleep_cycles WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
            ).unwrap();
        CycleSummary {
            id: id.into(), trigger: "idle".into(), summary: summary(&kind, &state, spend, undone.is_some(), rj.as_deref()),
            kind, state, stage: None, session_id: None, spend_usd: spend, manifest_before: before, manifest_after: after,
            started_at: started, finished_at: finished, undone_at: undone,
        }
    }

    fn report_json(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row("SELECT report_json FROM sleep_cycles WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn the_report_lists_every_stage_with_its_outcome_and_late_spend() {
        let f = fixture();
        cycle_row(&f.db, "cyc_1", "u1", "finished", vec![
            rec(Step::Capture, "done", None, json!({ "captured": [{ "unit": "a" }, { "unit": "b" }] })),
            rec(Step::Consolidation, "done", None, json!({ "merged": 3, "decayed": 1, "scanned": 40 })),
            rec(Step::Fit, "skipped", Some("stage_off: strengths"), Value::Null),
            rec(Step::Dream, "done", None, json!({ "status": "applied", "dream_id": "d1", "revision": "r2", "base_revision": "r1",
                "summary": { "merged": 1, "resolved": 0, "lessons": 1, "pruned": 0, "proposals": 0, "skipped": 2 } })),
            rec(Step::Curation, "skipped", Some("not_due"), Value::Null),
        ]);
        let conn = f.db.connect().unwrap();
        // The model cost lands in the ledger after the cycle closed.
        conn.execute(
            "INSERT INTO llm_usage_events (id, user_id, surface, cost_microdollars, status, created_at)
             VALUES ('e1', 'u1', 'memory', 30000, 'ok', '2026-11-19 12:00:35')",
            [],
        ).unwrap();
        let mut c = cycle_summary(&conn, "cyc_1");
        let rj = report_json(&conn, "cyc_1");
        let rep = build(&conn, "u1", &mut c, rj.as_deref()).unwrap().unwrap();

        let stages: Vec<(String, String)> = rep["stages"].as_array().unwrap().iter()
            .map(|x| (x["stage"].as_str().unwrap().to_string(), x["outcome"].as_str().unwrap().to_string())).collect();
        let want: Vec<(String, String)> = [
            ("capture", "ran"), ("consolidation", "ran"), ("fit", "skipped"), ("dream", "ran"),
            ("rem", "not_run"), ("curation", "skipped"), ("pages", "not_run"), ("checks", "not_run"), ("sign", "not_run"),
        ].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(stages, want);
        assert_eq!(rep["stages"][0]["counts"]["captured"], json!(2));
        assert_eq!(rep["stages"][1]["counts"]["merged"], json!(3));
        assert_eq!(rep["stages"][2]["reason"], json!("stage_off: strengths"));
        assert_eq!(rep["stages"][3]["counts"]["lessons"], json!(1));
        // Late spend is read from the ledger and recorded on the cycle.
        assert!((rep["stages"][3]["spend_usd"].as_f64().unwrap() - 0.03).abs() < 1e-9);
        assert!((rep["spend_usd"].as_f64().unwrap() - 0.03).abs() < 1e-9);
        let col: f64 = conn.query_row("SELECT spend_usd FROM sleep_cycles WHERE id = 'cyc_1'", [], |r| r.get(0)).unwrap();
        assert!((col - 0.03).abs() < 1e-9);
        // Undo links and the cannot-undo list.
        assert_eq!(rep["undo"]["available"], json!(true));
        assert_eq!(rep["undo"]["dream"], json!({ "dream_id": "d1", "status": "applied", "revision": "r2", "base_revision": "r1" }));
        assert_eq!(rep["undo"]["cannot_undo"].as_array().unwrap().len(), 1);
        let line = summary("nap", "finished", 0.03, false, rj.as_deref()).unwrap();
        assert!(line.starts_with("nap finished: 2 captured; consolidation merged 3, decayed 1; Dream 2 changes"), "{line}");

        // A cycle that stopped keeps its remaining stages visible as held.
        cycle_row(&f.db, "cyc_2", "u1", "stopped", vec![rec(Step::Capture, "done", None, json!({ "captured": [] }))]);
        let mut c2 = cycle_summary(&conn, "cyc_2");
        let rj2 = report_json(&conn, "cyc_2");
        let rep2 = build(&conn, "u1", &mut c2, rj2.as_deref()).unwrap().unwrap();
        assert_eq!((rep2["stages"][1]["outcome"].as_str(), rep2["stages"][1]["reason"].as_str()), (Some("held"), Some("resume")));
    }
}
