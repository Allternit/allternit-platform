//! Nightly Dream (Memory Drive Phase 2). Once a night per personal drive, a
//! model reads the drive and the last day's conversation turns and proposes
//! merges, contradiction fixes, lessons, prunes and twin proposals. Every
//! proposal is checked here against real evidence before it is applied:
//!
//! - a contradiction fix or lesson must cite turns that were actually shown;
//! - a prune needs an entry unused for 30+ days (checked from index stats,
//!   not taken from the model);
//! - twin facts are only proposed through the twin owner-review path.
//!
//! Accepted changes land as ONE `Dream YYYY-MM-DD` commit with a report built
//! from what was applied. Undo writes a new commit that reverses only the
//! Dream's own lines and refuses (listing files) when later edits touched them.
//! No transcript text is ever written to the drive.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::db::DbHandle;
use crate::memory_drive::{DriveError, Entry, Operation, Snapshot};
use crate::memory_drive_service::{self as service, DriveRecord, Result, ServiceError};

pub const AUTHOR: &str = "Dream via Allternit";
const MAX_ENTRIES_SHOWN: usize = 300;
const MAX_EVIDENCE: usize = 80;
const EVIDENCE_CHARS: usize = 600;
const MAX_LESSONS: usize = 5;
const MAX_PRUNES: usize = 20;
const MAX_PROPOSALS: usize = 3;
const PRUNE_AGE_DAYS: i64 = 30;
const LEASE_MINUTES: i64 = 60;
const MAX_ATTEMPTS: i64 = 3;

const SYSTEM: &str = "You are performing a Dream: a nightly reflective pass over one person's memory. \
You receive their memory entries and their conversation turns from the last day. Propose only changes \
that make memory more accurate and easier to use. Never invent facts. Cite turn ids as evidence for \
every contradiction fix, lesson and twin proposal; changes without evidence are discarded. \
Merge only entries that say the same thing. Prefer few, high-value changes. Return JSON only.";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Summary {
    pub merged: usize,
    pub resolved: usize,
    pub lessons: usize,
    pub pruned: usize,
    pub proposals: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DreamRow {
    pub id: String,
    pub date: String,
    pub status: String,
    pub revision: Option<String>,
    pub base_revision: Option<String>,
    pub report: Option<String>,
    pub summary: Summary,
    pub error: Option<String>,
    pub undo_revision: Option<String>,
    pub created_at: String,
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<DreamRow> {
    Ok(DreamRow {
        id: r.get(0)?,
        date: r.get(1)?,
        status: r.get(2)?,
        revision: r.get(3)?,
        base_revision: r.get(4)?,
        report: r.get(5)?,
        summary: r.get::<_, Option<String>>(6)?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        error: r.get(7)?,
        undo_revision: r.get(8)?,
        created_at: r.get(9)?,
    })
}
const COLS: &str = "id,date,status,revision,base_revision,report,summary,error,undo_revision,created_at";

pub fn list(db: &DbHandle, owner: &str, limit: usize) -> Result<Vec<DreamRow>> {
    let conn = db.connect()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM memory_dreams WHERE user_id=?1 ORDER BY date DESC, created_at DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![owner, limit.clamp(1, 100) as i64], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn get(conn: &Connection, owner: &str, id: &str) -> Result<Option<DreamRow>> {
    Ok(conn.query_row(&format!("SELECT {COLS} FROM memory_dreams WHERE id=?1 AND user_id=?2"), params![id, owner], row).optional()?)
}

pub fn dreaming_enabled(conn: &Connection, owner: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT dreaming_enabled FROM memory_drives WHERE kind='personal' AND scope_id=?1", params![owner], |r| r.get::<_, bool>(0))
        .optional()?
        .unwrap_or(true))
}

pub fn set_dreaming(db: &DbHandle, owner: &str, enabled: bool) -> Result<()> {
    let n = db.connect()?.execute(
        "UPDATE memory_drives SET dreaming_enabled=?2 WHERE kind='personal' AND scope_id=?1",
        params![owner, enabled],
    )?;
    if n == 0 {
        return Err(ServiceError::NotFound);
    }
    Ok(())
}

// ─── Model contract ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Plan {
    #[serde(default)]
    pub merges: Vec<Merge>,
    #[serde(default)]
    pub contradictions: Vec<Contradiction>,
    #[serde(default)]
    pub lessons: Vec<Lesson>,
    #[serde(default)]
    pub prune: Vec<Prune>,
    #[serde(default)]
    pub twin_proposals: Vec<Proposal>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Merge {
    pub keep: String,
    pub drop: Vec<String>,
    #[serde(default)]
    pub text: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Contradiction {
    pub keep: String,
    pub drop: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Lesson {
    pub text: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Prune {
    pub id: String,
    #[serde(default)]
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Proposal {
    pub text: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

pub fn schema() -> Value {
    let ids = json!({"type": "array", "items": {"type": "string"}});
    json!({
        "type": "object",
        "properties": {
            "merges": {"type": "array", "items": {"type": "object", "properties": {"keep": {"type": "string"}, "drop": ids, "text": {"type": "string"}}, "required": ["keep", "drop"]}},
            "contradictions": {"type": "array", "items": {"type": "object", "properties": {"keep": {"type": "string"}, "drop": ids, "evidence": ids}, "required": ["keep", "drop", "evidence"]}},
            "lessons": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "evidence": ids}, "required": ["text", "evidence"]}},
            "prune": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "reason": {"type": "string"}}, "required": ["id"]}},
            "twin_proposals": {"type": "array", "items": {"type": "object", "properties": {"text": {"type": "string"}, "kind": {"type": "string"}, "evidence": ids}, "required": ["text", "evidence"]}}
        },
        "required": ["merges", "contradictions", "lessons", "prune", "twin_proposals"]
    })
}

#[derive(Debug, Clone)]
pub struct Evidence {
    pub id: String,
    pub session: Option<String>,
    pub kind: String,
    pub at: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Shown {
    pub path: String,
    pub entry: Entry,
    pub uses: i64,
}

/// Entries the Dream may touch: not twin/, not the questions board.
pub fn shown_entries(conn: &Connection, drive: &DriveRecord, snapshot: &Snapshot) -> Result<Vec<Shown>> {
    let mut out = Vec::new();
    for (path, entry) in service::entries(snapshot)? {
        if path.starts_with("twin/") || path == crate::memory_drive_scopes::QUESTIONS || path.starts_with("imports/kernel") && entry.metadata.is_empty() {
            continue;
        }
        if matches!(entry.metadata.get("kind").map(String::as_str), Some("question" | "answer")) {
            continue;
        }
        let uses: i64 = conn
            .query_row(
                "SELECT COALESCE(f.retrieval_count,0) FROM memory_drive_entries e JOIN memory_facts f ON f.id=e.fact_id WHERE e.drive_id=?1 AND e.entry_id=?2",
                params![drive.id, entry.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        out.push(Shown { path, entry, uses });
    }
    out.sort_by(|a, b| b.entry.added.cmp(&a.entry.added));
    out.truncate(MAX_ENTRIES_SHOWN);
    Ok(out)
}

pub fn evidence(conn: &Connection, owner: &str) -> Result<Vec<Evidence>> {
    let mut stmt = conn.prepare(
        "SELECT o.id,o.session_id,o.kind,o.timestamp,o.content FROM memory_observations o
         WHERE o.user_id=?1 AND o.timestamp >= datetime('now','-1 day')
           AND o.kind IN ('turn_user','turn_assistant','explicit_memory')
           AND (o.session_id IS NULL OR NOT EXISTS(SELECT 1 FROM ephemeral_sessions s WHERE s.session_id=o.session_id))
         ORDER BY o.timestamp DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![owner, MAX_EVIDENCE as i64], |r| {
            let content: String = r.get(4)?;
            Ok(Evidence {
                id: r.get(0)?,
                session: r.get(1)?,
                kind: r.get(2)?,
                at: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                content: content.chars().take(EVIDENCE_CHARS).collect(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn prompt(date: &str, shown: &[Shown], evidence: &[Evidence]) -> String {
    let mut p = format!("# Dream for {date}\n\n## Memory entries\nFormat: [id] (file) text — added DATE — used N times\n");
    for s in shown {
        p.push_str(&format!("[{}] ({}) {} — added {} — used {} times\n", s.entry.id, s.path, s.entry.text, s.entry.added, s.uses));
    }
    p.push_str("\n## Conversation turns from the last day\nFormat: [turn id] (kind, time) text\n");
    if evidence.is_empty() {
        p.push_str("(none)\n");
    }
    for e in evidence {
        p.push_str(&format!("[{}] ({}, {}) {}\n", e.id, e.kind, e.at, e.content.replace('\n', " ")));
    }
    p.push_str(
        "\n## What to return\n\
- merges: entries that say the same thing. keep = the id to keep, drop = ids to remove, text = optional clearer wording for the kept entry.\n\
- contradictions: entries that disagree, where the turns show which is true. keep, drop, evidence = turn ids.\n\
- lessons: at most 5 durable lessons that recur across the turns (how this person likes to work, what failed and why). evidence = turn ids. Convert relative dates to absolute.\n\
- prune: entries that look stale or no longer useful. Only entries never used and older than 30 days will actually be removed.\n\
- twin_proposals: facts about the person that belong in their digital twin. evidence = turn ids. These are only proposed; the person approves them.\n\
Return empty arrays when nothing should change.",
    );
    p
}

// ─── Validation → operations ───────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct Checked {
    pub ops: Vec<Operation>,
    pub proposals: Vec<Proposal>,
    pub summary: Summary,
    pub report: Vec<String>,
}

fn clean(text: &str) -> Option<String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty() && t.chars().count() <= 400).then_some(t)
}

/// Turn the model's plan into drive operations, keeping only what passes the
/// evidence, age and format rules. Pure: unit-tested without a model.
pub fn check(plan: &Plan, shown: &[Shown], evidence: &[Evidence], today: chrono::NaiveDate, session_ok: &dyn Fn(&str) -> bool) -> Checked {
    let by_id: BTreeMap<&str, &Shown> = shown.iter().map(|s| (s.entry.id.as_str(), s)).collect();
    let turns: BTreeMap<&str, &Evidence> = evidence.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = Checked::default();
    let mut take = |id: &str, used: &mut BTreeSet<String>| by_id.contains_key(id) && used.insert(id.to_string());
    let cites = |ids: &[String]| !ids.is_empty() && ids.iter().all(|i| turns.contains_key(i.as_str()));

    for m in &plan.merges {
        let drops: Vec<&String> = m.drop.iter().filter(|d| *d != &m.keep).collect();
        if drops.is_empty() || !by_id.contains_key(m.keep.as_str()) || !drops.iter().all(|d| by_id.contains_key(d.as_str())) {
            out.summary.skipped += 1;
            continue;
        }
        if !take(&m.keep, &mut used) || !drops.iter().all(|d| take(d, &mut used)) {
            out.summary.skipped += 1;
            continue;
        }
        let keep = by_id[m.keep.as_str()];
        let mut entry = keep.entry.clone();
        if let Some(text) = m.text.as_deref().and_then(clean) {
            entry.text = text;
        }
        entry.metadata.insert("dream".into(), today.to_string());
        if entry.render().is_err() {
            out.summary.skipped += 1;
            continue;
        }
        let gone: Vec<String> = drops.iter().map(|d| by_id[d.as_str()].entry.text.clone()).collect();
        out.report.push(format!("- **Merged** into “{}” (`{}`): {}", entry.text, keep.path, gone.iter().map(|g| format!("“{g}”")).collect::<Vec<_>>().join(", ")));
        out.ops.push(Operation::UpsertEntry { path: keep.path.clone(), entry });
        for d in drops {
            out.ops.push(Operation::DeleteEntry { id: d.clone() });
        }
        out.summary.merged += 1;
    }

    for c in &plan.contradictions {
        let drops: Vec<&String> = c.drop.iter().filter(|d| *d != &c.keep).collect();
        if drops.is_empty() || !cites(&c.evidence) || !by_id.contains_key(c.keep.as_str()) || !drops.iter().all(|d| by_id.contains_key(d.as_str())) {
            out.summary.skipped += 1;
            continue;
        }
        if !take(&c.keep, &mut used) || !drops.iter().all(|d| take(d, &mut used)) {
            out.summary.skipped += 1;
            continue;
        }
        let keep = by_id[c.keep.as_str()];
        for d in &drops {
            out.ops.push(Operation::DeleteEntry { id: (*d).clone() });
        }
        out.report.push(format!(
            "- **Resolved** a contradiction: kept “{}”, removed {} (evidence: {})",
            keep.entry.text,
            drops.iter().map(|d| format!("“{}”", by_id[d.as_str()].entry.text)).collect::<Vec<_>>().join(", "),
            c.evidence.join(", ")
        ));
        out.summary.resolved += 1;
    }

    for l in plan.lessons.iter().take(MAX_LESSONS) {
        let Some(text) = clean(&l.text) else {
            out.summary.skipped += 1;
            continue;
        };
        if !cites(&l.evidence) || shown.iter().any(|s| s.entry.text.eq_ignore_ascii_case(&text)) {
            out.summary.skipped += 1;
            continue;
        }
        let first = turns[l.evidence[0].as_str()];
        let source = match first.session.as_deref().filter(|s| session_ok(s)) {
            Some(s) => format!("/?session={s}"),
            None => format!("allternit:observation/{}", first.id),
        };
        let mut entry = Entry { id: crate::memory_drive_writer::new_entry_id(), text, source, added: today.to_string(), metadata: Default::default() };
        entry.metadata.insert("memory_type".into(), "procedure".into());
        entry.metadata.insert("origin".into(), "dream".into());
        entry.metadata.insert("observation".into(), first.id.clone());
        if entry.render().is_err() {
            out.summary.skipped += 1;
            continue;
        }
        out.report.push(format!("- **Lesson** added to `lessons.md`: “{}”", entry.text));
        out.ops.push(Operation::UpsertEntry { path: "lessons.md".into(), entry });
        out.summary.lessons += 1;
    }

    let cutoff = today - chrono::Duration::days(PRUNE_AGE_DAYS);
    for p in plan.prune.iter().take(MAX_PRUNES) {
        let Some(s) = by_id.get(p.id.as_str()) else {
            out.summary.skipped += 1;
            continue;
        };
        let old = chrono::NaiveDate::parse_from_str(&s.entry.added, "%Y-%m-%d").map(|d| d <= cutoff).unwrap_or(false);
        if s.uses > 0 || !old || !take(&p.id, &mut used) {
            out.summary.skipped += 1;
            continue;
        }
        out.report.push(format!("- **Pruned** “{}” (`{}`): never used since {}", s.entry.text, s.path, s.entry.added));
        out.ops.push(Operation::DeleteEntry { id: p.id.clone() });
        out.summary.pruned += 1;
    }

    for p in plan.twin_proposals.iter().take(MAX_PROPOSALS) {
        if clean(&p.text).is_none() || !cites(&p.evidence) || crate::memory_kernel_service::mentions_secret(&p.text) {
            out.summary.skipped += 1;
            continue;
        }
        out.report.push(format!("- **Twin proposal** waiting for your review: “{}”", p.text.trim()));
        out.proposals.push(p.clone());
        out.summary.proposals += 1;
    }
    out
}

fn report_markdown(date: &str, checked: &Checked) -> String {
    let mut r = format!("# Dream {date}\n\n");
    if checked.report.is_empty() {
        r.push_str("Nothing needed to change.\n");
    } else {
        r.push_str(&checked.report.join("\n"));
        r.push('\n');
    }
    if checked.summary.skipped > 0 {
        r.push_str(&format!("\n{} suggested change(s) were skipped because they lacked evidence or broke a rule.\n", checked.summary.skipped));
    }
    r
}

// ─── Run ────────────────────────────────────────────────────────────────────

/// Claim today's run (the UNIQUE(drive,date) row is the lease). Returns the
/// dream id, or None when today's run is done or another worker holds it.
fn claim(conn: &Connection, drive: &DriveRecord, date: &str, base: &str) -> Result<Option<String>> {
    let existing: Option<(String, String, i64, String)> = conn
        .query_row(
            "SELECT id,status,attempts,updated_at FROM memory_dreams WHERE drive_id=?1 AND date=?2",
            params![drive.id, date],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    match existing {
        None => {
            let id = format!("dream_{}", uuid::Uuid::new_v4().simple());
            let n = conn.execute(
                "INSERT OR IGNORE INTO memory_dreams(id,drive_id,user_id,date,status,base_revision) VALUES(?1,?2,?3,?4,'running',?5)",
                params![id, drive.id, drive.user_id, date, base],
            )?;
            Ok((n == 1).then_some(id))
        }
        Some((id, status, attempts, _)) if status == "failed" && attempts < MAX_ATTEMPTS => {
            let n = conn.execute(
                "UPDATE memory_dreams SET status='running',attempts=attempts+1,error=NULL,base_revision=?2,updated_at=CURRENT_TIMESTAMP
                 WHERE id=?1 AND status='failed'",
                params![id, base],
            )?;
            Ok((n == 1).then_some(id))
        }
        Some((id, status, _, _)) if status == "running" => {
            let n = conn.execute(
                &format!(
                    "UPDATE memory_dreams SET attempts=attempts+1,base_revision=?2,updated_at=CURRENT_TIMESTAMP
                     WHERE id=?1 AND status='running' AND updated_at < datetime('now','-{LEASE_MINUTES} minutes')"
                ),
                params![id, base],
            )?;
            Ok((n == 1).then_some(id))
        }
        _ => Ok(None),
    }
}

fn finish(conn: &Connection, id: &str, status: &str, revision: Option<&str>, report: Option<&str>, summary: &Summary, error: Option<&str>) -> Result<()> {
    crate::metrics::inc_memory_drive_event(match status { "applied" => "dream_applied", "failed" => "dream_failed", _ => "dream_no_changes" });
    conn.execute(
        "UPDATE memory_dreams SET status=?2,revision=?3,report=?4,summary=?5,error=?6,updated_at=CURRENT_TIMESTAMP WHERE id=?1",
        params![id, status, revision, report, serde_json::to_string(summary).unwrap_or_default(), error],
    )?;
    Ok(())
}

/// The model call is injectable so tests (and a future local model) can run
/// the exact same validation and commit path.
pub type Complete = Arc<dyn Fn(String) -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>;

pub fn gizzi_completer(owner: String) -> Complete {
    Arc::new(move |prompt: String| {
        let owner = owner.clone();
        Box::pin(async move {
            let model = crate::memory_extraction::extraction_model();
            crate::usage_ledger::scope(
                crate::usage_ledger::LedgerCtx::surface("memory").tenant(None, Some(&owner)),
                crate::gizzi_completion::complete_ephemeral_structured(&prompt, Some(SYSTEM), Some(&model), &schema()),
            )
            .await
        })
    })
}

fn parse_plan(raw: &str) -> Option<Plan> {
    let trimmed = raw.trim();
    let body = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_end_matches("```").trim())
        .unwrap_or(trimmed);
    serde_json::from_str(body).ok()
}

/// Run (or re-run) the Dream for `date`. Idempotent per drive and date.
pub async fn run(db: DbHandle, owner: String, date: chrono::NaiveDate, complete: Complete) -> Result<Option<DreamRow>> {
    let date_s = date.format("%Y-%m-%d").to_string();
    let (drive, id, base, shown, turns) = {
        let (db, owner, date_s) = (db.clone(), owner.clone(), date_s.clone());
        tokio::task::spawn_blocking(move || -> Result<Option<(DriveRecord, String, String, Vec<Shown>, Vec<Evidence>)>> {
            service::repair_index(&db, &owner)?;
            let drive = service::resolve(&db, &owner)?;
            let storage = drive.storage()?;
            let base = storage.head()?.ok_or(DriveError::Uninitialized)?;
            let conn = db.connect()?;
            let Some(id) = claim(&conn, &drive, &date_s, &base)? else { return Ok(None) };
            let snapshot = storage.snapshot(Some(&base))?;
            let shown = shown_entries(&conn, &drive, &snapshot)?;
            let turns = evidence(&conn, &owner)?;
            Ok(Some((drive, id, base, shown, turns)))
        })
        .await
        .map_err(|e| ServiceError::Provenance(format!("dream task failed: {e}")))??
        .map_or((None, String::new(), String::new(), vec![], vec![]), |(d, i, b, s, t)| (Some(d), i, b, s, t))
    };
    let Some(drive) = drive else {
        return Ok(None);
    };
    if shown.is_empty() && turns.is_empty() {
        let conn = db.connect()?;
        finish(&conn, &id, "no_changes", None, Some(&format!("# Dream {date_s}\n\nNo memories or recent conversations to review.\n")), &Summary::default(), None)?;
        return Ok(get(&conn, &owner, &id)?);
    }
    let raw = complete(prompt(&date_s, &shown, &turns)).await;
    let conn = db.connect()?;
    let Some(plan) = raw.as_deref().and_then(parse_plan) else {
        // Memory is untouched; the next sweep retries (bounded).
        let msg = if raw.is_none() { "The model did not answer. Memory was not changed." } else { "The model's answer was not usable. Memory was not changed." };
        finish(&conn, &id, "failed", None, None, &Summary::default(), Some(msg))?;
        return Ok(get(&conn, &owner, &id)?);
    };
    drop(conn);
    let (db2, owner2, id2) = (db.clone(), owner.clone(), id.clone());
    tokio::task::spawn_blocking(move || apply_plan(&db2, &owner2, &drive, &id2, &base, date, &shown, &turns, &plan))
        .await
        .map_err(|e| ServiceError::Provenance(format!("dream task failed: {e}")))??;
    Ok(get(&db.connect()?, &owner, &id)?)
}

#[allow(clippy::too_many_arguments)]
fn apply_plan(db: &DbHandle, owner: &str, drive: &DriveRecord, id: &str, base: &str, date: chrono::NaiveDate, shown: &[Shown], turns: &[Evidence], plan: &Plan) -> Result<()> {
    let date_s = date.format("%Y-%m-%d").to_string();
    let conn = db.connect()?;
    let session_ok = |s: &str| service::session_is_owned(&conn, owner, s).unwrap_or(false);
    let checked = check(plan, shown, turns, date, &session_ok);
    let report = report_markdown(&date_s, &checked);
    // Twin facts are only ever proposed for the owner's review.
    for p in &checked.proposals {
        let body = crate::twin_persona::MemoryBody {
            kind: p.kind.clone().filter(|k| crate::twin_persona::KINDS.contains(&k.as_str())),
            content: Some(p.text.trim().to_string()),
            visibility: Some("owner".into()),
            source_channel: Some("dream".into()),
            ..Default::default()
        };
        if let Err(e) = crate::twin_persona::propose(&conn, owner, &body) {
            tracing::warn!("dream twin proposal skipped: {e}");
        }
    }
    if !checked.proposals.is_empty() {
        crate::memory_drive_twin::sync_logged(db, owner);
    }
    if checked.ops.is_empty() {
        finish(&conn, id, "no_changes", None, Some(&report), &checked.summary, None)?;
        return Ok(());
    }
    let message = format!("Dream {date_s}");
    let mut expected = base.to_string();
    let mut ops = checked.ops.clone();
    for attempt in 0..3 {
        match service::apply_as(db, owner, &expected, &ops, &message, AUTHOR) {
            Ok(r) => {
                conn.execute("UPDATE memory_dreams SET base_revision=?2 WHERE id=?1", params![id, expected])?;
                finish(&conn, id, "applied", Some(&r.revision), Some(&report), &checked.summary, None)?;
                return Ok(());
            }
            Err(ServiceError::Drive(DriveError::Conflict { .. })) if attempt < 2 => {
                // Someone wrote meanwhile: keep only operations whose target
                // lines are unchanged in the new head, then retry on it.
                let storage = drive.storage()?;
                let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
                let now: BTreeMap<String, String> = service::entries(&storage.snapshot(Some(&head))?)?
                    .into_iter()
                    .filter_map(|(_, e)| e.render().ok().map(|l| (e.id, l)))
                    .collect();
                let before: BTreeMap<&str, String> = shown.iter().filter_map(|s| s.entry.render().ok().map(|l| (s.entry.id.as_str(), l))).collect();
                ops.retain(|op| match op {
                    Operation::DeleteEntry { id } => now.get(id) == before.get(id.as_str()),
                    Operation::UpsertEntry { entry, .. } => !before.contains_key(entry.id.as_str()) || now.get(&entry.id) == before.get(entry.id.as_str()),
                    _ => true,
                });
                expected = head;
                if ops.is_empty() {
                    finish(&conn, id, "no_changes", None, Some(&report), &checked.summary, None)?;
                    return Ok(());
                }
            }
            Err(e) => {
                finish(&conn, id, "failed", None, None, &checked.summary, Some(&format!("Memory was not changed: {e}")))?;
                return Ok(());
            }
        }
    }
    finish(&conn, id, "failed", None, None, &checked.summary, Some("Memory kept changing during the Dream; it will retry."))?;
    Ok(())
}

// ─── Undo ───────────────────────────────────────────────────────────────────

fn lines(snapshot: &Snapshot) -> Result<BTreeMap<String, (String, Entry, String)>> {
    Ok(service::entries(snapshot)?
        .into_iter()
        .filter_map(|(path, e)| e.render().ok().map(|l| (e.id.clone(), (path, e, l))))
        .collect())
}

/// Reverse exactly the Dream's own changes on top of the current head.
/// Returns the paths that conflict when later edits touched those lines.
pub fn undo_ops(before: &Snapshot, dream: &Snapshot, head: &Snapshot) -> Result<std::result::Result<Vec<Operation>, Vec<String>>> {
    let (p, d, h) = (lines(before)?, lines(dream)?, lines(head)?);
    let mut ops = Vec::new();
    let mut conflicts = BTreeSet::new();
    for (id, (path, entry, line)) in &p {
        match d.get(id) {
            // Removed by the Dream: bring it back unless it is already back.
            None => {
                if !h.contains_key(id) {
                    ops.push(Operation::UpsertEntry { path: path.clone(), entry: entry.clone() });
                }
            }
            // Rewritten by the Dream: restore if untouched since.
            Some((dpath, _, dline)) if dline != line => match h.get(id) {
                Some((_, _, hline)) if hline == dline => ops.push(Operation::UpsertEntry { path: path.clone(), entry: entry.clone() }),
                Some(_) => {
                    conflicts.insert(dpath.clone());
                }
                None => {}
            },
            _ => {}
        }
    }
    for (id, (path, _, dline)) in &d {
        if p.contains_key(id) {
            continue;
        }
        // Added by the Dream: remove it unless it was edited since.
        match h.get(id) {
            Some((_, _, hline)) if hline == dline => ops.push(Operation::DeleteEntry { id: id.clone() }),
            Some(_) => {
                conflicts.insert(path.clone());
            }
            None => {}
        }
    }
    Ok(if conflicts.is_empty() { Ok(ops) } else { Err(conflicts.into_iter().collect()) })
}

#[derive(Debug)]
pub enum UndoError {
    Service(ServiceError),
    Conflict(Vec<String>),
    NotUndoable(&'static str),
}
impl From<ServiceError> for UndoError {
    fn from(e: ServiceError) -> Self {
        Self::Service(e)
    }
}
impl From<rusqlite::Error> for UndoError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Service(e.into())
    }
}
impl From<DriveError> for UndoError {
    fn from(e: DriveError) -> Self {
        Self::Service(e.into())
    }
}

pub fn undo(db: &DbHandle, owner: &str, dream_id: &str) -> std::result::Result<DreamRow, UndoError> {
    let conn = db.connect()?;
    let dream = get(&conn, owner, dream_id)?.ok_or(UndoError::Service(ServiceError::NotFound))?;
    if dream.status != "applied" {
        return Err(UndoError::NotUndoable(if dream.status == "undone" { "This Dream was already undone." } else { "Only an applied Dream can be undone." }));
    }
    let (Some(base), Some(rev)) = (dream.base_revision.clone(), dream.revision.clone()) else {
        return Err(UndoError::NotUndoable("This Dream has no commit to undo."));
    };
    let drive = service::resolve(db, owner)?;
    let storage = drive.storage()?;
    for _ in 0..4 {
        let head = storage.head()?.ok_or(DriveError::Uninitialized)?;
        let ops = match undo_ops(&storage.snapshot(Some(&base))?, &storage.snapshot(Some(&rev))?, &storage.snapshot(Some(&head))?)? {
            Ok(ops) => ops,
            Err(paths) => return Err(UndoError::Conflict(paths)),
        };
        let applied = if ops.is_empty() {
            Ok(head.clone())
        } else {
            service::apply_as(db, owner, &head, &ops, &format!("Undo Dream {}", dream.date), AUTHOR).map(|r| r.revision)
        };
        match applied {
            Ok(revision) => {
                conn.execute(
                    "UPDATE memory_dreams SET status='undone',undo_revision=?2,updated_at=CURRENT_TIMESTAMP WHERE id=?1",
                    params![dream_id, revision],
                )?;
                return Ok(get(&conn, owner, dream_id)?.ok_or(UndoError::Service(ServiceError::NotFound))?);
            }
            Err(ServiceError::Drive(DriveError::Conflict { .. })) => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(UndoError::Service(DriveError::Conflict { expected: None, actual: None }.into()))
}

// ─── Scheduler ──────────────────────────────────────────────────────────────

/// Nightly sweep: every 15 minutes, Dream each personal drive whose owner's
/// local time (their saved time zone, else server time) is 02:00–06:00 and
/// that has dreaming on and no run for that local date.
pub fn spawn(state: Arc<crate::AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
        loop {
            tick.tick().await;
            sweep_due(&state.db, chrono::Utc::now()).await;
        }
    });
}

/// The owner's local date when it is Dream time (02:00–06:00) for them.
pub fn dream_date(timezone: Option<&str>, now: chrono::DateTime<chrono::Utc>) -> Option<chrono::NaiveDate> {
    use chrono::Timelike;
    let local = match timezone.and_then(|t| t.parse::<chrono_tz::Tz>().ok()) {
        Some(tz) => now.with_timezone(&tz).naive_local(),
        None => now.with_timezone(&chrono::Local).naive_local(),
    };
    (2..6).contains(&local.hour()).then(|| local.date())
}

pub fn set_timezone(db: &DbHandle, owner: &str, timezone: &str) -> Result<()> {
    if timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(ServiceError::Provenance("unknown time zone".into()));
    }
    db.connect()?.execute("UPDATE memory_drives SET timezone=?2 WHERE kind='personal' AND scope_id=?1", params![owner, timezone])?;
    Ok(())
}

pub fn timezone(conn: &Connection, owner: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT timezone FROM memory_drives WHERE kind='personal' AND scope_id=?1", params![owner], |r| r.get(0))
        .optional()?
        .flatten())
}

async fn sweep_due(db: &DbHandle, now: chrono::DateTime<chrono::Utc>) {
    let drives: Vec<(String, Option<String>)> = match db.connect().and_then(|c| {
        // A computer that syncs this drive with another computer (the
        // Desktop app runs the sync) leaves the nightly Dream to the other,
        // always-on one; the results reach it through the sync.
        let mut s = c.prepare(
            "SELECT d.user_id, d.timezone FROM memory_drives d WHERE d.kind='personal' AND d.dreaming_enabled=1
             AND NOT EXISTS(SELECT 1 FROM memory_drive_peers p WHERE p.drive_id=d.id AND p.last_sync_at >= datetime('now','-7 days'))",
        )?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect();
        rows
    }) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("dream sweep: {e}");
            return;
        }
    };
    let mut by_date: BTreeMap<chrono::NaiveDate, Vec<String>> = BTreeMap::new();
    for (owner, tz) in drives {
        if let Some(date) = dream_date(tz.as_deref(), now) {
            by_date.entry(date).or_default().push(owner);
        }
    }
    for (date, owners) in by_date {
        sweep_owners(db, date, Some(&owners)).await;
    }
}

pub async fn sweep(db: &DbHandle, date: chrono::NaiveDate) {
    sweep_owners(db, date, None).await
}

async fn sweep_owners(db: &DbHandle, date: chrono::NaiveDate, only: Option<&[String]>) {
    match service::hide_expired_archives(db) {
        Ok(0) => {}
        Ok(n) => tracing::info!("memory drive: hid {n} imported rows past their 30-day archive window"),
        Err(e) => tracing::warn!("memory drive archive cleanup: {e}"),
    }
    let owners: Vec<String> = match db.connect().and_then(|c| {
        let mut s = c.prepare(
            "SELECT d.user_id FROM memory_drives d WHERE d.kind='personal' AND d.dreaming_enabled=1
             AND NOT EXISTS(SELECT 1 FROM memory_dreams m WHERE m.drive_id=d.id AND m.date=?1 AND m.status IN ('applied','no_changes','undone'))",
        )?;
        let rows = s.query_map(params![date.format("%Y-%m-%d").to_string()], |r| r.get(0))?.collect();
        rows
    }) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("dream sweep: {e}");
            return;
        }
    };
    for owner in owners.into_iter().filter(|o| only.map_or(true, |l| l.contains(o))) {
        if let Err(e) = run(db.clone(), owner.clone(), date, gizzi_completer(owner.clone())).await {
            tracing::warn!("dream for a user failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, text: &str, added: &str) -> Entry {
        Entry { id: id.into(), text: text.into(), source: "/?session=s1".into(), added: added.into(), metadata: Default::default() }
    }
    fn shown(id: &str, text: &str, added: &str, uses: i64) -> Shown {
        Shown { path: "facts.md".into(), entry: entry(id, text, added), uses }
    }
    fn turn(id: &str) -> Evidence {
        Evidence { id: id.into(), session: Some("s1".into()), kind: "turn_user".into(), at: "2026-10-06".into(), content: "x".into() }
    }
    fn today() -> chrono::NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
    }

    #[test]
    fn evidence_age_and_uniqueness_rules_are_enforced_in_code() {
        let shown = vec![
            shown("a", "User lives in Austin.", "2026-10-01", 3),
            shown("b", "User lives in Austin, TX.", "2026-10-02", 0),
            shown("c", "User lives in Denver.", "2026-10-03", 1),
            shown("old", "User likes fax machines.", "2026-01-01", 0),
            shown("recent", "User tried Zig once.", "2026-10-01", 0),
            shown("used", "User uses Rust.", "2025-01-01", 4),
        ];
        let turns = vec![turn("o1")];
        let plan = Plan {
            merges: vec![Merge { keep: "a".into(), drop: vec!["b".into()], text: None }],
            contradictions: vec![
                Contradiction { keep: "c".into(), drop: vec!["a".into()], evidence: vec!["o1".into()] }, // a already used
                Contradiction { keep: "c".into(), drop: vec!["old".into()], evidence: vec![] },          // no evidence
            ],
            lessons: vec![
                Lesson { text: "User prefers small PRs.".into(), evidence: vec!["o1".into()] },
                Lesson { text: "Invented lesson.".into(), evidence: vec!["nope".into()] },
            ],
            prune: vec![
                Prune { id: "old".into(), reason: None },
                Prune { id: "recent".into(), reason: None },
                Prune { id: "used".into(), reason: None },
            ],
            twin_proposals: vec![Proposal { text: "Eoj lives in Denver.".into(), kind: None, evidence: vec!["o1".into()] }],
        };
        let c = check(&plan, &shown, &turns, today(), &|s| s == "s1");
        assert_eq!(c.summary, Summary { merged: 1, resolved: 0, lessons: 1, pruned: 1, proposals: 1, skipped: 5 });
        assert!(c.ops.iter().any(|o| matches!(o, Operation::DeleteEntry { id } if id == "old")));
        assert!(!c.ops.iter().any(|o| matches!(o, Operation::DeleteEntry { id } if id == "recent" || id == "used")));
        let lesson = c.ops.iter().find_map(|o| match o { Operation::UpsertEntry { path, entry } if path == "lessons.md" => Some(entry), _ => None }).unwrap();
        assert_eq!(lesson.source, "/?session=s1");
    }

    #[test]
    fn dream_time_follows_the_owner_time_zone() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-07T08:30:00Z").unwrap().with_timezone(&chrono::Utc);
        // 03:30 in Chicago, 09:30 in London.
        assert_eq!(dream_date(Some("America/Chicago"), now), chrono::NaiveDate::from_ymd_opt(2026, 10, 7));
        assert_eq!(dream_date(Some("Europe/London"), now), None);
    }

    #[test]
    fn malformed_model_output_is_rejected() {
        assert!(parse_plan("not json").is_none());
        assert!(parse_plan("```json\n{\"merges\":[],\"contradictions\":[],\"lessons\":[],\"prune\":[],\"twin_proposals\":[]}\n```").is_some());
    }

    fn snap(es: &[(&str, &str)]) -> Snapshot {
        let mut facts = String::from("# Facts\n\n");
        for (id, text) in es {
            facts.push_str(&entry(id, text, "2026-10-01").render().unwrap());
            facts.push('\n');
        }
        let mut files = BTreeMap::new();
        files.insert("facts.md".to_string(), facts);
        Snapshot { revision: None, files }
    }

    #[test]
    fn undo_restores_dream_lines_and_keeps_later_writes() {
        let before = snap(&[("a", "A one."), ("b", "B one.")]);
        let dream = snap(&[("a", "A merged."), ("l", "Lesson.")]);
        // Later the user added "n"; nothing touched the dream's lines.
        let head = snap(&[("a", "A merged."), ("l", "Lesson."), ("n", "New.")]);
        let ops = undo_ops(&before, &dream, &head).unwrap().unwrap();
        assert_eq!(ops.len(), 3);
        assert!(ops.iter().any(|o| matches!(o, Operation::DeleteEntry { id } if id == "l")));
        assert!(ops.iter().any(|o| matches!(o, Operation::UpsertEntry { entry, .. } if entry.id == "b")));
        assert!(ops.iter().any(|o| matches!(o, Operation::UpsertEntry { entry, .. } if entry.id == "a" && entry.text == "A one.")));
        assert!(!ops.iter().any(|o| matches!(o, Operation::DeleteEntry { id } if id == "n")));
    }

    #[test]
    fn undo_reports_conflict_when_a_dream_line_was_edited_later() {
        let before = snap(&[("a", "A one.")]);
        let dream = snap(&[("a", "A merged.")]);
        let head = snap(&[("a", "A edited by hand.")]);
        assert_eq!(undo_ops(&before, &dream, &head).unwrap().unwrap_err(), vec!["facts.md".to_string()]);
    }
}
