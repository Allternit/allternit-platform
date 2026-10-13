//! Trained flywheel heads (Agency Descent B2/B3, C1): datasets, artifacts
//! and the admin endpoints that build them. CPU only, deterministic.
//!
//! - `POST /v1/decisions/heads/train/:kind` queues a job that fits
//!   `head.embed` (`embed_head`) on the pooled opted-in dataset (`harvest`).
//! - `POST /v1/decisions/heads/compile/:kind` queues a job that compiles
//!   `head.rules` (`rules_head`), stored only when it is clean.
//! - Both answer `202` with the job; `GET /v1/decisions/heads/jobs/:id`
//!   follows it (queued, running, succeeded, failed; texts embedded of the
//!   total; the version's metrics or the error). One queued or running job
//!   per (head, kind): `409` otherwise. Jobs run on the runtime (embedding)
//!   and the blocking pool (fit, compile), not in the request; job state is
//!   in `decision_head_jobs`, and a job left running by a restart is failed.
//! - `GET /v1/decisions/heads/:head/:kind/versions` lists the stored versions
//!   with their metrics (never the weights).
//!
//! Each new version is registered for its kind in **shadow** through the
//! flywheel's audited `transition`, so it has to earn promotion on live
//! traffic like any head; a retrain sends a live head back to shadow.
//!
//! Dataset: every labelled example is keyed by the sha256 of its content
//! (state, options in order, label) and sorted by it. Examples whose key's
//! first byte is a multiple of 5 (about 20%) are the validation split: the
//! split is stable across retrains and exact duplicates never straddle it.
//! Metrics compare the head with the labels (the teacher's or the outcome's)
//! and with baselines on the same validation split: `head.lookup` (exact
//! match against the training split), the prior (most frequent label id) and,
//! for `head.embed`, the untrained cosine choice.

use super::super::{ApiError, ApiResult, RequestId};
use super::embed_head::{self, Example, TrainConfig};
use super::flywheel::{self, Metrics, SHADOW};
use super::harvest::{self, Labelled};
use super::rules_head::{self, CompileBar};
use crate::auth::AuthUser;
use crate::db::DbHandle;
use crate::memory_index::{EmbedClient, InputType};
use crate::AppState;
use axum::{
    extract::{Path, Query as QueryParams, State},
    response::IntoResponse,
    Extension, Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Fewer labelled examples than this: no training.
pub const MIN_EXAMPLES: usize = 20;
const DEFAULT_LIMIT: usize = 5_000;
/// Small batches: a CPU embedding server answers each well inside the client timeout.
const EMBED_BATCH: usize = 8;
const EMBED_RETRIES: usize = 2;

// ── dataset ─────────────────────────────────────────────────────────────────

/// One labelled decision, ready for the trainers.
#[derive(Clone, Debug)]
pub struct Item {
    pub state: String,
    pub options: Vec<(String, String)>,
    pub label: usize,
    pub source: String,
    /// sha256 of the content; sort order and split.
    pub hash: [u8; 32],
    /// The decision without its label (the lookup baseline's key).
    pub key: [u8; 32],
}

pub fn prepare(rows: Vec<Labelled>) -> Vec<Item> {
    let mut items: Vec<Item> = rows
        .into_iter()
        .filter_map(|r| {
            let label = r.options.iter().position(|(id, _)| *id == r.label)?;
            let state = embed_head::state_text(&r.context, r.question.as_deref());
            let mut h = Sha256::new();
            h.update(state.as_bytes());
            h.update([0u8]);
            for (id, text) in &r.options {
                h.update(id.as_bytes());
                h.update([1u8]);
                h.update(text.as_bytes());
                h.update([0u8]);
            }
            let key: [u8; 32] = h.clone().finalize().into();
            h.update([2u8]);
            h.update(r.label.as_bytes());
            Some(Item { state, options: r.options, label, source: r.source, hash: h.finalize().into(), key })
        })
        .collect();
    items.sort_by(|a, b| a.hash.cmp(&b.hash));
    items
}

pub fn is_validation(item: &Item) -> bool {
    item.hash[0] % 5 == 0
}

pub fn split(items: &[Item]) -> (Vec<&Item>, Vec<&Item>) {
    items.iter().partition(|i| !is_validation(i))
}

pub fn dataset_hash(items: &[Item]) -> String {
    let mut h = Sha256::new();
    for i in items {
        h.update(i.hash);
    }
    hex::encode(h.finalize())
}

/// `head.lookup` (exact match on the training split) and the prior (most
/// frequent label id) on the validation split.
pub fn baselines(train: &[&Item], val: &[&Item]) -> Value {
    let mut seen: HashMap<[u8; 32], BTreeMap<&str, usize>> = HashMap::new();
    let mut prior: BTreeMap<&str, usize> = BTreeMap::new();
    for i in train {
        let id = i.options[i.label].0.as_str();
        *seen.entry(i.key).or_default().entry(id).or_default() += 1;
        *prior.entry(id).or_default() += 1;
    }
    // Most frequent, first in id order on ties.
    let top = |m: &BTreeMap<&str, usize>| m.iter().fold(None::<(&str, usize)>, |b, (k, v)| match b { Some((_, bv)) if bv >= *v => b, _ => Some((k, *v)) }).map(|x| x.0.to_string());
    let prior_id = top(&prior);
    let (mut lookup_right, mut lookup_answered, mut prior_right) = (0usize, 0usize, 0usize);
    for i in val {
        let right = i.options[i.label].0.as_str();
        if let Some(choice) = seen.get(&i.key).and_then(|m| top(m)) {
            lookup_answered += 1;
            lookup_right += (choice == right) as usize;
        }
        prior_right += (prior_id.as_deref() == Some(right)) as usize;
    }
    let ratio = |a: usize| if val.is_empty() { Value::Null } else { json!(a as f64 / val.len() as f64) };
    json!({
        "lookup": { "accuracy": ratio(lookup_right), "coverage": ratio(lookup_answered) },
        "prior": { "accuracy": ratio(prior_right), "label": prior_id },
    })
}

// ── artifacts ───────────────────────────────────────────────────────────────

pub struct Artifact {
    pub id: String,
    pub version: i64,
    pub embed_model: Option<String>,
    pub embed_dim: Option<usize>,
    pub weights: Option<Vec<u8>>,
    pub rules_json: Option<String>,
}

pub struct NewArtifact<'a> {
    pub head: &'a str,
    pub kind: &'a str,
    pub embed_model: Option<&'a str>,
    pub embed_dim: Option<usize>,
    pub weights: Option<&'a [u8]>,
    pub rules_json: Option<&'a str>,
    pub metrics: &'a Value,
    pub dataset_hash: &'a str,
    pub examples: usize,
    pub actor: &'a str,
}

/// Store a new version and register it in shadow (audited). Returns the version.
pub fn store(conn: &Connection, a: &NewArtifact<'_>) -> rusqlite::Result<i64> {
    let version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM decision_head_models WHERE head = ?1 AND kind = ?2",
        params![a.head, a.kind],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO decision_head_models (id, head, kind, version, embed_model, embed_dim, weights, rules_json,
            metrics_json, dataset_hash, examples, actor, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            super::super::store::new_id("dhm"),
            a.head,
            a.kind,
            version,
            a.embed_model,
            a.embed_dim.map(|d| d as i64),
            a.weights,
            a.rules_json,
            a.metrics.to_string(),
            a.dataset_hash,
            a.examples as i64,
            a.actor,
            super::super::store::now(),
        ],
    )?;
    let reason = format!("trained version {version} on {} examples", a.examples);
    flywheel::transition(conn, a.head, a.kind, SHADOW, &reason, a.actor, &Metrics::default())?;
    Ok(version)
}

/// The newest version's id and number.
pub fn latest(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<Option<(String, i64)>> {
    conn.prepare_cached("SELECT id, version FROM decision_head_models WHERE head = ?1 AND kind = ?2 ORDER BY version DESC LIMIT 1")?
        .query_row(params![head, kind], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
}

pub fn load(conn: &Connection, id: &str) -> rusqlite::Result<Artifact> {
    conn.query_row(
        "SELECT id, version, embed_model, embed_dim, weights, rules_json FROM decision_head_models WHERE id = ?1",
        [id],
        |r| {
            Ok(Artifact {
                id: r.get(0)?,
                version: r.get(1)?,
                embed_model: r.get(2)?,
                embed_dim: r.get::<_, Option<i64>>(3)?.map(|d| d as usize),
                weights: r.get(4)?,
                rules_json: r.get(5)?,
            })
        },
    )
}

/// Every version of a head for a kind, newest first, without weights.
pub fn versions(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(
        "SELECT version, embed_model, embed_dim, metrics_json, dataset_hash, examples, actor, created_at, rules_json
         FROM decision_head_models WHERE head = ?1 AND kind = ?2 ORDER BY version DESC",
    )?;
    let rows = stmt.query_map(params![head, kind], |r| {
        let rules: Option<String> = r.get(8)?;
        Ok(json!({
            "object": "decision_head_version", "head": head, "kind": kind, "version": r.get::<_, i64>(0)?,
            "embed_model": r.get::<_, Option<String>>(1)?, "embed_dim": r.get::<_, Option<i64>>(2)?,
            "metrics": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null),
            "dataset_hash": r.get::<_, String>(4)?, "examples": r.get::<_, i64>(5)?,
            "actor": r.get::<_, String>(6)?, "created_at": r.get::<_, String>(7)?,
            "rules": rules.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
        }))
    })?;
    rows.collect()
}

/// The newest version's summary for the heads list (`None` for untrained heads).
pub fn latest_summary(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<Option<Value>> {
    conn.query_row(
        "SELECT version, embed_model, created_at, examples FROM decision_head_models WHERE head = ?1 AND kind = ?2 ORDER BY version DESC LIMIT 1",
        params![head, kind],
        |r| Ok(json!({ "version": r.get::<_, i64>(0)?, "embed_model": r.get::<_, Option<String>>(1)?, "created_at": r.get::<_, String>(2)?, "examples": r.get::<_, i64>(3)? })),
    )
    .optional()
}

// ── head.embed training ─────────────────────────────────────────────────────

/// Why a training run produced nothing: HTTP status and message.
#[derive(Debug)]
pub struct TrainError(pub u16, pub String);

impl From<rusqlite::Error> for TrainError {
    fn from(e: rusqlite::Error) -> Self {
        TrainError(500, e.to_string())
    }
}

/// Embedded dataset: one vector table, every item's state and option rows.
pub struct Embedded {
    pub model: String,
    pub dim: usize,
    pub vecs: Vec<Vec<f32>>,
    pub examples: Vec<Example>,
}

/// Embed each distinct state (as a query) and option text (as a document)
/// once. The first batch decides the model: the embedding server when it
/// answers, the hash fallback for the whole run when it does not. A later
/// batch that fails (after retries) or comes back from another model fails
/// the run: one dataset is never embedded by two models.
pub async fn embed_items(client: &EmbedClient, items: &[Item]) -> Result<Embedded, TrainError> {
    embed_items_with(client, items, &|_, _| {}).await
}

/// Progress callback: (texts embedded, texts in total).
pub type Progress<'a> = &'a (dyn Fn(usize, usize) + Send + Sync);

pub async fn embed_items_with(client: &EmbedClient, items: &[Item], progress: Progress<'_>) -> Result<Embedded, TrainError> {
    let mut index: HashMap<(bool, &str), usize> = HashMap::new();
    let mut texts: [Vec<String>; 2] = [vec![], vec![]];
    // (is_state, text) → row; rows are assigned in item order: deterministic.
    let mut order: Vec<(bool, &str)> = Vec::new();
    for i in items {
        for (is_state, t) in std::iter::once((true, i.state.as_str())).chain(i.options.iter().map(|(_, t)| (false, t.as_str()))) {
            if !index.contains_key(&(is_state, t)) {
                index.insert((is_state, t), order.len());
                order.push((is_state, t));
                texts[!is_state as usize].push(t.to_string());
            }
        }
    }
    let total = texts[0].len() + texts[1].len();
    let mut done = 0usize;
    progress(0, total);
    let mut model: Option<(String, usize)> = None;
    let mut remote = true;
    let mut by_kind: [Vec<Vec<f32>>; 2] = [vec![], vec![]];
    for (k, input) in [(0usize, InputType::Query), (1usize, InputType::Document)] {
        for batch in texts[k].chunks(EMBED_BATCH) {
            let e = if !remote {
                crate::memory_index::hash_embed(batch)
            } else {
                let mut tries = 0;
                loop {
                    match client.embed_remote(batch, input).await {
                        Ok(e) => break e,
                        Err(err) if model.is_none() => {
                            tracing::info!(error = %err, "decision head training: embedding server unavailable, using the hash fallback");
                            remote = false;
                            break crate::memory_index::hash_embed(batch);
                        }
                        Err(err) if tries >= EMBED_RETRIES => {
                            return Err(TrainError(503, format!("the embedding server failed during training: {err}; retry")));
                        }
                        Err(_) => tries += 1,
                    }
                }
            };
            match &model {
                None => model = Some((e.model.clone(), e.dim)),
                Some((m, d)) if *m != e.model || *d != e.dim => {
                    return Err(TrainError(503, format!("the embedding model changed during training ({m} → {}); retry", e.model)));
                }
                _ => {}
            }
            by_kind[k].extend(e.vectors);
            done += batch.len();
            progress(done, total);
        }
    }
    let (model, dim) = model.unwrap_or_default();
    // Back to first-seen order.
    let (mut si, mut oi) = (0usize, 0usize);
    let mut vecs = Vec::with_capacity(order.len());
    for (is_state, _) in &order {
        let v = if *is_state { si += 1; std::mem::take(&mut by_kind[0][si - 1]) } else { oi += 1; std::mem::take(&mut by_kind[1][oi - 1]) };
        vecs.push(v);
    }
    let examples = items
        .iter()
        .map(|i| Example {
            state: index[&(true, i.state.as_str())],
            options: i.options.iter().map(|(_, t)| index[&(false, t.as_str())]).collect(),
            label: i.label,
        })
        .collect();
    Ok(Embedded { model, dim, vecs, examples })
}

/// Fit on the training split and score on both splits (blocking CPU work).
pub fn fit_embedded(items: &[Item], e: &Embedded, cfg: &TrainConfig) -> (Vec<f32>, Value) {
    let (train, val): (Vec<Example>, Vec<Example>) = {
        let mut t = vec![];
        let mut v = vec![];
        for (i, ex) in items.iter().zip(&e.examples) {
            if is_validation(i) { v.push(ex.clone()) } else { t.push(ex.clone()) }
        }
        (t, v)
    };
    let started = std::time::Instant::now();
    let w = embed_head::fit(&e.vecs, &train, e.dim, cfg);
    let train_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (train_acc, _) = embed_head::evaluate(&w, &e.vecs, &train);
    let (val_acc, val_conf) = embed_head::evaluate(&w, &e.vecs, &val);
    let (titems, vitems) = split(items);
    let teacher: Vec<Example> = items.iter().zip(&e.examples).filter(|(i, _)| is_validation(i) && i.source == "teacher").map(|(_, x)| x.clone()).collect();
    let nonempty = |n: usize, x: f64| if n == 0 { Value::Null } else { json!(x) };
    let mut baselines = baselines(&titems, &vitems);
    baselines["cosine"] = json!({ "accuracy": nonempty(val.len(), embed_head::cosine_accuracy(&e.vecs, &val)) });
    let metrics = json!({
        "examples": items.len(), "train_examples": train.len(), "val_examples": val.len(),
        "embed_model": e.model, "embed_dim": e.dim, "features": "[s*o, |s-o|, cos(s,o)]",
        "config": cfg, "train_ms": (train_ms * 100.0).round() / 100.0,
        "train_accuracy": nonempty(train.len(), train_acc),
        "val_accuracy": nonempty(val.len(), val_acc),
        "val_accuracy_teacher_labels": nonempty(teacher.len(), embed_head::evaluate(&w, &e.vecs, &teacher).0),
        "val_teacher_examples": teacher.len(),
        "val_overconfidence": nonempty(val.len(), val_conf - val_acc),
        "baselines": baselines,
        "weights_sha256": embed_head::weights_sha256(&w),
    });
    (w, metrics)
}

/// Train `head.embed` for `kind` from the pooled opted-in dataset, store the
/// version and register it in shadow. Returns the stored version's summary.
pub async fn train_embed(db: &DbHandle, client: &EmbedClient, kind: &str, actor: &str, limit: usize) -> Result<Value, TrainError> {
    train_embed_with(db, client, kind, actor, limit, &|_, _| {}).await
}

pub async fn train_embed_with(
    db: &DbHandle,
    client: &EmbedClient,
    kind: &str,
    actor: &str,
    limit: usize,
    progress: Progress<'_>,
) -> Result<Value, TrainError> {
    let rows = harvest::labelled(&db.connect()?, kind, None, limit)?;
    let items = prepare(rows);
    if items.len() < MIN_EXAMPLES {
        return Err(TrainError(422, format!("{kind} has {} labelled examples; training needs {MIN_EXAMPLES}", items.len())));
    }
    let embedded = embed_items_with(client, &items, progress).await?;
    let (items, embedded, (w, metrics)) = tokio::task::spawn_blocking(move || {
        let out = fit_embedded(&items, &embedded, &TrainConfig::default());
        (items, embedded, out)
    })
    .await
    .map_err(|e| TrainError(500, e.to_string()))?;
    let conn = db.connect()?;
    let version = store(
        &conn,
        &NewArtifact {
            head: embed_head::NAME,
            kind,
            embed_model: Some(&embedded.model),
            embed_dim: Some(embedded.dim),
            weights: Some(&crate::memory_index::f32_to_bytes(&w)),
            rules_json: None,
            metrics: &metrics,
            dataset_hash: &dataset_hash(&items),
            examples: items.len(),
            actor,
        },
    )?;
    Ok(json!({ "object": "decision_head_version", "head": embed_head::NAME, "kind": kind, "version": version,
               "state": SHADOW, "dataset_hash": dataset_hash(&items), "metrics": metrics }))
}

/// Compile `head.rules` for `kind`; stores it only when the bar is met.
pub fn compile_rules(conn: &Connection, kind: &str, actor: &str, limit: usize, bar: &CompileBar) -> Result<Value, TrainError> {
    let items = prepare(harvest::labelled(conn, kind, None, limit)?);
    let c = rules_head::compile(&items, bar);
    let Some(rules) = c.rules else {
        let why = c.metrics["reason"].as_str().unwrap_or("rejected").to_string();
        return Err(TrainError(422, format!("{kind} does not compile to rules: {why}")));
    };
    let rules_json = serde_json::to_string(&rules).unwrap_or_default();
    let version = store(
        conn,
        &NewArtifact {
            head: rules_head::NAME,
            kind,
            embed_model: None,
            embed_dim: None,
            weights: None,
            rules_json: Some(&rules_json),
            metrics: &c.metrics,
            dataset_hash: &dataset_hash(&items),
            examples: items.len(),
            actor,
        },
    )?;
    Ok(json!({ "object": "decision_head_version", "head": rules_head::NAME, "kind": kind, "version": version,
               "state": SHADOW, "dataset_hash": dataset_hash(&items), "metrics": c.metrics, "rules": rules }))
}

// ── jobs ────────────────────────────────────────────────────────────────────

pub const QUEUED: &str = "queued";
pub const RUNNING: &str = "running";
pub const SUCCEEDED: &str = "succeeded";
pub const FAILED: &str = "failed";

/// Jobs this process owns (queued or running). A queued/running row that is
/// not here was left by a process that ended: it is failed on the next
/// enqueue for its (head, kind) and on read.
fn owned() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static OWNED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    OWNED.get_or_init(Default::default)
}

fn is_owned(id: &str) -> bool {
    owned().lock().map(|o| o.contains(id)).unwrap_or(false)
}

fn fail_orphans(conn: &Connection, head: &str, kind: &str) -> rusqlite::Result<()> {
    let active: Vec<String> = conn
        .prepare("SELECT id FROM decision_head_jobs WHERE head = ?1 AND kind = ?2 AND status IN ('queued', 'running')")?
        .query_map(params![head, kind], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in active.into_iter().filter(|id| !is_owned(id)) {
        let now = super::super::store::now();
        conn.execute(
            "UPDATE decision_head_jobs SET status = 'failed', error = 'interrupted: the server restarted', finished_at = ?2, updated_at = ?2
             WHERE id = ?1 AND status IN ('queued', 'running')",
            params![id, now],
        )?;
    }
    Ok(())
}

/// Queue a job for `head` (`head.embed` trains, `head.rules` compiles) on
/// `kind`. One queued or running job per (head, kind): 409 otherwise.
pub fn enqueue(conn: &Connection, head: &str, kind: &str, actor: &str) -> Result<String, TrainError> {
    fail_orphans(conn, head, kind)?;
    let id = super::super::store::new_id("dhj");
    let now = super::super::store::now();
    // The partial unique index makes the check and the insert one step.
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO decision_head_jobs (id, head, kind, status, actor, created_at, updated_at) VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?5)",
        params![id, head, kind, actor, now],
    )?;
    if inserted == 0 {
        return Err(TrainError(409, format!("a {head} job for {kind} is already queued or running")));
    }
    if let Ok(mut o) = owned().lock() {
        o.insert(id.clone());
    }
    Ok(id)
}

fn set_progress(db: &DbHandle, id: &str, done: usize, total: usize) {
    let res = db.connect().and_then(|c| {
        c.execute(
            "UPDATE decision_head_jobs SET progress_done = ?2, progress_total = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, done as i64, total as i64, super::super::store::now()],
        )
    });
    if let Err(e) = res {
        tracing::warn!(job = id, "decision head job progress: {e}");
    }
}

fn finish(db: &DbHandle, id: &str, out: &Result<Value, TrainError>) {
    let now = super::super::store::now();
    let (status, result, error) = match out {
        Ok(v) => (SUCCEEDED, Some(v.to_string()), None),
        Err(e) => (FAILED, None, Some(e.1.clone())),
    };
    let res = db.connect().and_then(|c| {
        c.execute(
            "UPDATE decision_head_jobs SET status = ?2, result_json = ?3, error = ?4, finished_at = ?5, updated_at = ?5 WHERE id = ?1",
            params![id, status, result, error, now],
        )
    });
    if let Err(e) = res {
        tracing::warn!(job = id, "decision head job finish: {e}");
    }
    if let Ok(mut o) = owned().lock() {
        o.remove(id);
    }
    tracing::info!(job = id, status, "decision head job done");
}

/// Run a queued job to the end on the runtime, independent of the request
/// that queued it: embedding is async, the fit and the compile run on the
/// blocking pool.
pub fn spawn(db: DbHandle, client: &'static EmbedClient, id: String, head: String, kind: String, actor: String, limit: usize) {
    tokio::spawn(async move {
        let now = super::super::store::now();
        if let Err(e) = db.connect().and_then(|c| {
            c.execute("UPDATE decision_head_jobs SET status = 'running', started_at = ?2, updated_at = ?2 WHERE id = ?1", params![id, now])
        }) {
            tracing::warn!(job = id, "decision head job start: {e}");
        }
        let out = if head == rules_head::NAME {
            let (db2, kind2, actor2) = (db.clone(), kind.clone(), actor.clone());
            tokio::task::spawn_blocking(move || compile_rules(&db2.connect()?, &kind2, &actor2, limit, &CompileBar::default()))
                .await
                .unwrap_or_else(|e| Err(TrainError(500, e.to_string())))
        } else {
            let (pdb, pid) = (db.clone(), id.clone());
            let progress = move |done: usize, total: usize| set_progress(&pdb, &pid, done, total);
            train_embed_with(&db, client, &kind, &actor, limit, &progress).await
        };
        finish(&db, &id, &out);
    });
}

pub fn job(conn: &Connection, id: &str) -> rusqlite::Result<Option<Value>> {
    let row = conn
        .query_row(
            "SELECT head, kind, status, progress_done, progress_total, result_json, error, actor, created_at, started_at, finished_at
             FROM decision_head_jobs WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((head, kind, status, done, total, result, error, actor, created, started, finished)) = row else { return Ok(None) };
    if (status == QUEUED || status == RUNNING) && !is_owned(id) {
        fail_orphans(conn, &head, &kind)?;
        return job(conn, id);
    }
    Ok(Some(json!({
        "object": "decision_head_job", "id": id, "head": head, "kind": kind, "status": status,
        "progress": { "done": done, "total": total, "unit": "texts embedded" },
        "result": result.and_then(|r| serde_json::from_str::<Value>(&r).ok()), "error": error,
        "actor": actor, "created_at": created, "started_at": started, "finished_at": finished,
    })))
}

// ── HTTP ────────────────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
pub struct TrainQuery {
    #[serde(default)]
    limit: Option<usize>,
}

fn admin(user: &AuthUser, rid: &RequestId) -> Result<(), ApiError> {
    if !flywheel::is_flywheel_admin(&user.user_id) {
        return Err(ApiError::new(403, "PERMISSION", "ERR_PERMISSION_DENIED", "only a decisions admin can train a head", rid));
    }
    Ok(())
}

fn admin_and_kind(user: &AuthUser, kind: &str, rid: &RequestId) -> Result<(), ApiError> {
    admin(user, rid)?;
    if kind.is_empty() || kind.len() > 64 || !kind.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) {
        return Err(ApiError::new(400, "INPUT", "ERR_INPUT_INVALID", "kind must be 1-64 characters of [A-Za-z0-9_.-]", rid).param(Some("kind".into())));
    }
    Ok(())
}

fn train_error(e: TrainError, rid: &RequestId) -> ApiError {
    match e.0 {
        500 => ApiError::internal(e.1, rid),
        409 => ApiError::new(409, "CONFLICT", "ERR_CONFLICT", e.1, rid),
        422 => ApiError::new(422, "INPUT", "ERR_TRAINING_REJECTED", e.1, rid),
        s => ApiError::new(s, "SYSTEM", "ERR_TRAINING_UNAVAILABLE", e.1, rid),
    }
}

/// Queue a job and answer 202 with it; the job outlives the request.
pub fn start(st: &AppState, client: &'static EmbedClient, user: &AuthUser, rid: &RequestId, head: &str, kind: &str, limit: Option<usize>) -> ApiResult {
    admin_and_kind(user, kind, rid)?;
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 100_000);
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, rid))?;
    let id = enqueue(&conn, head, kind, &user.user_id).map_err(|e| train_error(e, rid))?;
    spawn(st.db.clone(), client, id.clone(), head.into(), kind.into(), user.user_id.clone(), limit);
    let v = job(&conn, &id).map_err(|e| ApiError::internal(e, rid))?;
    Ok((axum::http::StatusCode::ACCEPTED, Json(v)).into_response())
}

pub async fn train_head(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(kind): Path<String>,
    QueryParams(q): QueryParams<TrainQuery>,
) -> ApiResult {
    start(&st, crate::memory_index::global(), &user, &rid, embed_head::NAME, &kind, q.limit)
}

pub async fn compile_head(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(kind): Path<String>,
    QueryParams(q): QueryParams<TrainQuery>,
) -> ApiResult {
    start(&st, crate::memory_index::global(), &user, &rid, rules_head::NAME, &kind, q.limit)
}

pub async fn get_job(
    State(st): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path(id): Path<String>,
) -> ApiResult {
    admin(&user, &rid)?;
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    match job(&conn, &id).map_err(|e| ApiError::internal(e, &rid))? {
        Some(v) => Ok(Json(v).into_response()),
        None => Err(ApiError::not_found("decision head job", &rid)),
    }
}

pub async fn list_versions(
    State(st): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Extension(rid): Extension<RequestId>,
    Path((head, kind)): Path<(String, String)>,
) -> ApiResult {
    let conn = st.db.connect().map_err(|e| ApiError::internal(e, &rid))?;
    let data = versions(&conn, &head, &kind).map_err(|e| ApiError::internal(e, &rid))?;
    Ok(Json(json!({ "object": "list", "data": data })).into_response())
}
