//! Section 3: decision types. Built-ins mirror the System One motif library
//! (tools/system-one-local/src/decision/motifs.ts); calibration comes from the
//! manifests file (ALLTERNIT_S1_MANIFESTS) and the shadow ledger
//! (ALLTERNIT_S1_SHADOW_DIR: decisions/*.jsonl joined to outcomes/*.jsonl).
//! A type is `live` only when an explicit promotion exists AND a gate-passing
//! manifest bound to the current S1 backend exists; changing the backend
//! clears every promotion (everything returns to shadow).

use super::*;
use axum::extract::{Path, Query, State};
use axum::Extension;
use serde::Deserialize;
use std::collections::HashSet;

/// Q22 gate minimum held-out rows (tools/system-one-local/src/decision/gate.ts DEFAULT_MIN.held_out_n).
pub const NEEDED_ROWS: u64 = 300;
const AUTHORITY: &str = "advisory: cannot grant or close";

const MOTIFS: &[(&str, &str, &str, &str)] = &[
    ("ROUTE", "classify", "CHOICE", "pick one destination from a closed set"),
    ("TRIAGE", "classify", "CHOICE", "assign an urgency or category bucket"),
    ("RANK", "select", "RANK", "order candidates best-first"),
    ("GATE", "gate", "GATE", "yes/no: may this proceed"),
    ("JUDGE", "score", "SCORE", "ordinal quality rating against a rubric"),
    ("LABEL", "classify", "CHOICE", "assign one label from a closed vocabulary"),
    ("REFLEX", "reflex", "CHOICE", "fast lane: event to pre-authorized action"),
    ("CONFIDENCE_GATE", "gate", "GATE", "is the upstream answer trustworthy enough to keep"),
    ("BRANCH_PRUNE", "select", "SUBSET", "keep a subset of candidate branches"),
    ("COMPLETION_GATE", "gate", "VERIFY", "is the task done per the stated criteria"),
];

fn output_of(op: &str) -> &'static str {
    match op {
        "CHOICE" => "one candidate id", "RANK" => "ordered candidate ids", "SUBSET" => "subset of candidate ids",
        "GATE" => "yes or no", "SCORE" => "ordinal level", "VERIFY" => "pass or fail", _ => "value",
    }
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace('-', "_")
}

fn manifests() -> Vec<Value> {
    std::env::var("ALLTERNIT_S1_MANIFESTS").ok().filter(|p| !p.trim().is_empty())
        .and_then(|p| std::fs::read_to_string(p.trim()).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

fn jsonl(dir: &std::path::Path, sub: &str) -> Vec<Value> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { return out };
    for f in rd.flatten() {
        if let Ok(s) = std::fs::read_to_string(f.path()) {
            out.extend(s.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
        }
    }
    out
}

/// Joined rows (decision with a recorded outcome) per primitive id.
fn shadow_rows() -> std::collections::HashMap<String, u64> {
    let mut counts = std::collections::HashMap::new();
    let Some(dir) = std::env::var("ALLTERNIT_S1_SHADOW_DIR").ok().filter(|d| !d.trim().is_empty()) else { return counts };
    let dir = std::path::PathBuf::from(dir.trim());
    let outcomes = jsonl(&dir, "outcomes");
    let ids: HashSet<&str> = outcomes.iter().filter_map(|o| o["decision_id"].as_str()).collect();
    let subjects: HashSet<&str> = outcomes.iter().filter_map(|o| o["subject_ref"].as_str()).collect();
    for d in jsonl(&dir, "decisions") {
        let joined = d["decision_id"].as_str().is_some_and(|i| ids.contains(i)) || d["subject_ref"].as_str().is_some_and(|s| subjects.contains(s));
        if joined {
            *counts.entry(norm(d["primitive_id"].as_str().unwrap_or_default())).or_insert(0) += 1;
        }
    }
    counts
}

fn entry(id: &str, category: &str, op: &str, inputs: &str, output: &str, fallback: &str, builtin: bool) -> Value {
    json!({ "id": id, "category": category, "operation": op, "tier": "S1", "inputs": inputs, "output": output,
            "authority": AUTHORITY, "fallback": fallback, "builtin": builtin })
}

fn list_all(conn: &Connection, owner: &str, backend: &str) -> Result<Vec<Value>, KErr> {
    let mut types: Vec<Value> = MOTIFS.iter().map(|(id, cat, op, sum)| {
        let mut e = entry(id, cat, op, "state projection + candidates", output_of(op), "explicit deterministic F-node", true);
        e["summary"] = json!(sum);
        e
    }).collect();
    let mut stmt = conn.prepare("SELECT doc_json FROM kernel_ui_decision_types WHERE owner_id = ?1 ORDER BY created_at")?;
    let rows = stmt.query_map(params![owner], |r| r.get::<_, String>(0))?;
    for r in rows.flatten() {
        if let Ok(v) = serde_json::from_str::<Value>(&r) {
            types.push(v);
        }
    }
    let ms = manifests();
    let rows_by = shadow_rows();
    let mut st = conn.prepare("SELECT id, status FROM kernel_ui_decision_status")?;
    let promoted: std::collections::HashMap<String, String> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    for t in &mut types {
        let id = t["id"].as_str().unwrap_or_default().to_string();
        let qualified = ms.iter().any(|m| {
            norm(m["primitive_id"].as_str().unwrap_or_default()) == norm(&id)
                && m["gate"]["passed"] == json!(true)
                && norm(m["scope"]["backend_id"].as_str().unwrap_or_default()) == norm(backend)
        });
        let status = match promoted.get(&id).map(String::as_str) {
            Some("disabled") => "disabled",
            Some("live") if qualified => "live",
            _ => "shadow",
        };
        t["status"] = json!(status);
        t["calibration"] = json!({ "rows": rows_by.get(&norm(&id)).copied().unwrap_or(0), "needed": NEEDED_ROWS, "qualified": qualified });
    }
    Ok(types)
}

#[derive(Deserialize)]
pub struct Q {
    scope: Option<String>,
    parents: Option<String>,
}

fn backend_of(conn: &Connection, u: &AuthUser, q: &Q) -> Result<String, KErr> {
    let scope = q.scope.clone().unwrap_or_else(|| format!("org:{}", u.organization_id.clone().filter(|o| !o.is_empty()).unwrap_or_else(|| "default".into())));
    let ch = chain(&scope, u, q.parents.as_deref())?;
    routing_policy::current_s1(conn, &ch)
}

pub async fn list(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>) -> KRes {
    let ch_check = q.scope.as_deref().map(|s| chain(s, &u, q.parents.as_deref())).transpose()?;
    drop(ch_check);
    Ok(Json(Value::Array(blocking(st.db.clone(), move |c| {
        let backend = backend_of(c, &u, &q)?;
        list_all(c, &u.user_id, &backend)
    }).await?)))
}

pub async fn create(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Query(q): Query<Q>, Json(b): Json<Value>) -> KRes {
    let s = |k: &str| b.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty() && s.len() <= 500).map(str::to_string);
    let id = s("id").filter(|i| i.len() <= 64 && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).ok_or_else(|| KErr::bad("id must be 1-64 chars of [A-Za-z0-9_-]"))?;
    if MOTIFS.iter().any(|m| m.0.eq_ignore_ascii_case(&id)) {
        return Err(KErr::conflict("id collides with a built-in decision type"));
    }
    let op = s("operation").filter(|o| ["BELIEF", "CHOICE", "SCORE", "RANK", "SUBSET", "ESTIMATE", "GATE", "VERIFY", "PAIR_SCORE"].contains(&o.as_str())).ok_or_else(|| KErr::bad("unknown operation"))?;
    if let Some(a) = b.get("authority").and_then(Value::as_str) {
        let l = a.to_lowercase();
        if (l.contains("grant") || l.contains("close")) && !l.contains("cannot") && !l.contains("never") {
            return Err(KErr::bad("a decision type cannot grant or close"));
        }
    }
    let doc = entry(&id, &s("category").unwrap_or_else(|| "custom".into()), &op, &s("inputs").unwrap_or_default(),
        &s("output").unwrap_or_else(|| output_of(&op).into()), &s("fallback").unwrap_or_else(|| "explicit deterministic F-node".into()), false);
    let owner = u.user_id.clone();
    Ok(Json(blocking(st.db.clone(), move |c| {
        let n = c.execute("INSERT OR IGNORE INTO kernel_ui_decision_types (id, owner_id, doc_json, created_at) VALUES (?1, ?2, ?3, ?4)", params![id, owner, doc.to_string(), now()])?;
        if n == 0 {
            return Err(KErr::conflict("a decision type with that id already exists"));
        }
        let backend = backend_of(c, &u, &q)?;
        list_all(c, &owner, &backend)?.into_iter().find(|t| t["id"] == json!(id)).ok_or_else(|| KErr::internal("created type missing"))
    }).await?))
}

/// Promote / demote one type. `live` needs a gate-passing manifest bound to the
/// current backend; it is refused (409) otherwise.
pub async fn set_status(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Path(id): Path<String>, Query(q): Query<Q>, Json(b): Json<Value>) -> KRes {
    let want = b["status"].as_str().filter(|s| ["shadow", "live", "disabled"].contains(s)).ok_or_else(|| KErr::bad("status must be shadow, live or disabled"))?.to_string();
    Ok(Json(blocking(st.db.clone(), move |c| {
        let backend = backend_of(c, &u, &q)?;
        let cur = list_all(c, &u.user_id, &backend)?.into_iter().find(|t| t["id"] == json!(id)).ok_or_else(|| KErr::not_found("decision type not found"))?;
        if want == "live" && cur["calibration"]["qualified"] != json!(true) {
            return Err(KErr::conflict("no gate-passing manifest for the current S1 backend"));
        }
        c.execute("INSERT INTO kernel_ui_decision_status (id, status, updated_at) VALUES (?1, ?2, ?3)
                   ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at", params![id, want, now()])?;
        list_all(c, &u.user_id, &backend)?.into_iter().find(|t| t["id"] == json!(id)).ok_or_else(|| KErr::internal("type missing"))
    }).await?))
}
