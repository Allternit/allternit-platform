//! Replay cache for `run_subtask` (driver spec D4): Stagehand's cache and
//! self-healing design over the Allternit Driver's element map.
//!
//! * **Key** = hash(goal, app + window shape, input names). Input *values* are
//!   variables: the same subtask with new values replays the same steps.
//! * **First run** (a miss) runs the decision loop and records each step that
//!   moved the UI: the op, a semantic locator (our element id, role, name,
//!   tree path, window-relative box, 8x8 pixel hash) and a `verify`-syntax
//!   check of what the step changed. The entry is stored only once the
//!   subtask's exact success checks hold.
//! * **Later runs** (a hit) replay the steps with no decisions: one
//!   speculative `run_batch` (each step waits for its element id, acts and
//!   checks its `expect`). A step whose element moved is found again by
//!   name, path or pixels; failing that it is re-inferred once (one decision
//!   for that step only), and the healed entry is rewritten.
//! * A replay that diverges (a step's check fails, or re-inference can't
//!   place it) hands over to the decision loop from that screen; a run that
//!   then succeeds rewrites the entry.
//! * **Secrets**: input values are never stored. Every string that held one
//!   is stored as `{{name}}` and substituted at replay time, and an entry that
//!   still contains a value is refused. `use_credential` never runs inside a
//!   subtask, and element values are never recorded.
//! * A named entry is a saved skill (`run_skill`, `skills`).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::computer_subtask::{name_of, role_of};

/// Values shorter than this are templated only where a whole string equals
/// them (a "7" inside "17 items" is not that input).
const MIN_SUBSTRING: usize = 3;
/// Pixel-hash distance (of 64 bits) that still counts as the same element.
const CROP_TOLERANCE: u32 = 10;
/// Box overlap (intersection over union) for the pixel fallback.
const MIN_IOU: f64 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Refresh,
    Off,
}

impl Mode {
    pub fn parse(s: Option<&str>) -> Mode {
        match s {
            Some("off") => Mode::Off,
            Some("refresh") => Mode::Refresh,
            _ => Mode::Auto,
        }
    }
}

/// How a replayed step finds its element again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Locator {
    /// The driver's element id: hash(role, name, tree path).
    pub id: String,
    /// Normalized role (`role_of`).
    pub role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Window-relative x, y, w, h.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    /// 8x8 average hash of the element's pixels (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// click | press | set_value
    pub op: String,
    /// set_value: the input's name (never its value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// press: the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub locator: Locator,
    /// What the step does, in words (the re-inference question).
    pub label: String,
    /// What the step changed, in verify syntax ({role, name[, gone]}).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Entry {
    pub id: String,
    pub key: String,
    pub name: Option<String>,
    pub goal: String,
    pub app: String,
    pub window: String,
    pub variables: Vec<String>,
    /// The subtask's exact success checks, templated.
    pub success: Vec<Value>,
    pub steps: Vec<Step>,
    pub hits: i64,
    pub heals: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Variables: values out (template) and back in (substitute).
// ---------------------------------------------------------------------------

fn placeholder(name: &str) -> String {
    format!("{{{{{name}}}}}")
}

/// Replace every input value in `s` with its `{{name}}` placeholder.
pub fn template(s: &str, inputs: &[(String, String)]) -> String {
    let mut vals: Vec<&(String, String)> = inputs.iter().filter(|(_, v)| !v.is_empty()).collect();
    vals.sort_by_key(|(_, v)| std::cmp::Reverse(v.chars().count()));
    if let Some((n, _)) = vals.iter().find(|(_, v)| v == s) {
        return placeholder(n);
    }
    let mut out = s.to_string();
    for (n, v) in vals {
        if v.chars().count() >= MIN_SUBSTRING {
            out = out.replace(v.as_str(), &placeholder(n));
        }
    }
    out
}

/// Put the current values back.
pub fn substitute(s: &str, inputs: &[(String, String)]) -> String {
    let mut out = s.to_string();
    for (n, v) in inputs {
        out = out.replace(&placeholder(n), v);
    }
    out
}

fn map_strings(v: &Value, f: &dyn Fn(&str) -> String) -> Value {
    match v {
        Value::String(s) => Value::String(f(s)),
        Value::Array(a) => Value::Array(a.iter().map(|x| map_strings(x, f)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), map_strings(x, f))).collect()),
        other => other.clone(),
    }
}

pub fn template_json(v: &Value, inputs: &[(String, String)]) -> Value {
    map_strings(v, &|s| template(s, inputs))
}

pub fn substitute_json(v: &Value, inputs: &[(String, String)]) -> Value {
    map_strings(v, &|s| substitute(s, inputs))
}

/// The locator with the current values in its name.
pub fn substitute_locator(l: &Locator, inputs: &[(String, String)]) -> Locator {
    Locator { name: substitute(&l.name, inputs), ..l.clone() }
}

/// Refuse an entry that would store an input value (case-insensitive), so a
/// templating gap can never leak one into the cache.
pub fn guard(entry: &Entry, inputs: &[(String, String)]) -> Result<(), String> {
    let stored = serde_json::to_string(&(&entry.goal, &entry.window, &entry.success, &entry.steps)).unwrap_or_default().to_lowercase();
    for (n, v) in inputs {
        let v = v.trim().to_lowercase();
        if v.chars().count() >= MIN_SUBSTRING && stored.contains(&v) {
            return Err(format!("the recording still holds the value of input {n}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The key.
// ---------------------------------------------------------------------------

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The window's title as a shape: values templated, digit runs folded
/// ("Invoice 1042 — Ada" and "Invoice 7 — Bo" are the same window).
pub fn window_shape(title: &str, inputs: &[(String, String)]) -> String {
    let t = template(title, inputs);
    let mut out = String::with_capacity(t.len());
    let mut in_digits = false;
    for c in t.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    norm(&out)
}

/// hash(goal, app + window shape, input names) and the window shape. The
/// goal is templated too ("Sign up as {{email}}").
pub fn cache_key(goal: &str, app: &str, title: &str, inputs: &[(String, String)]) -> (String, String) {
    let window = window_shape(title, inputs);
    let mut names: Vec<String> = inputs.iter().map(|(n, _)| norm(n)).collect();
    names.sort();
    let mut h = Sha256::new();
    for part in [norm(&template(goal, inputs)), norm(app), window.clone(), names.join("\u{1f}")] {
        h.update(part.as_bytes());
        h.update(b"\x1e");
    }
    (hex::encode(&h.finalize()[..16]), window)
}

// ---------------------------------------------------------------------------
// Locators.
// ---------------------------------------------------------------------------

fn bbox(e: &Value) -> Option<[f64; 4]> {
    let a = e.get("bbox")?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let mut b = [0.0; 4];
    for (i, v) in a.iter().enumerate() {
        b[i] = v.as_f64()?;
    }
    Some(b)
}

/// The window's top-left: the first element of a read is the window.
pub fn origin<'a>(mut elements: impl Iterator<Item = &'a Value>) -> (f64, f64) {
    elements.next().and_then(bbox).map(|b| (b[0], b[1])).unwrap_or((0.0, 0.0))
}

fn relative(e: &Value, origin: (f64, f64)) -> Option<[f64; 4]> {
    bbox(e).map(|b| [b[0] - origin.0, b[1] - origin.1, b[2], b[3]])
}

/// Record how to find `e` again. Its name is templated; its value is never kept.
pub fn locator(e: &Value, origin: (f64, f64), inputs: &[(String, String)]) -> Locator {
    Locator {
        id: e.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
        role: role_of(e),
        name: template(&name_of(e), inputs),
        path: e.get("path").and_then(Value::as_str).map(str::to_string),
        bbox: relative(e, origin),
        crop: e.get("crop").and_then(Value::as_str).map(str::to_string),
    }
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let (x1, y1) = (a[0].max(b[0]), a[1].max(b[1]));
    let (x2, y2) = ((a[0] + a[2]).min(b[0] + b[2]), (a[1] + a[3]).min(b[1] + b[3]));
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn crop_distance(a: &str, b: &str) -> Option<u32> {
    Some((u64::from_str_radix(a, 16).ok()? ^ u64::from_str_radix(b, 16).ok()?).count_ones())
}

/// How a step's element was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Id,
    Name,
    Path,
    Pixels,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Id => "id",
            Via::Name => "name",
            Via::Path => "path",
            Via::Pixels => "pixels",
        }
    }
}

/// Find a (substituted) locator's element on the current screen: the same id
/// (same role, name and tree path), else the one element of that role with
/// that name, else with that tree path, else the one element of that role in
/// the same place (and, when either side has a name that differs, with a
/// matching pixel hash). Ambiguity is a miss: never a guess.
pub fn resolve<'a>(loc: &Locator, elements: &[&'a Value], origin: (f64, f64)) -> Option<(String, Via)> {
    let id = |e: &Value| e.get("id").and_then(Value::as_str).map(str::to_string);
    let same: Vec<&Value> = elements.iter().copied().filter(|e| role_of(e) == loc.role).collect();
    if same.iter().any(|e| id(*e).as_deref() == Some(loc.id.as_str())) {
        return Some((loc.id.clone(), Via::Id));
    }
    let one = |m: Vec<&Value>, via: Via| if m.len() == 1 { id(m[0]).map(|i| (i, via)) } else { None };
    if !loc.name.is_empty() {
        if let Some(hit) = one(same.iter().copied().filter(|e| name_of(e) == loc.name).collect(), Via::Name) {
            return Some(hit);
        }
    }
    if let Some(p) = &loc.path {
        if let Some(hit) = one(same.iter().copied().filter(|e| e.get("path").and_then(Value::as_str) == Some(p.as_str())).collect(), Via::Path) {
            return Some(hit);
        }
    }
    let b = loc.bbox?;
    let near: Vec<&Value> = same
        .iter()
        .copied()
        .filter(|e| relative(e, origin).is_some_and(|r| iou(r, b) >= MIN_IOU))
        .filter(|e| {
            let crop = e.get("crop").and_then(Value::as_str);
            let close = match (loc.crop.as_deref(), crop) {
                (Some(a), Some(c)) => crop_distance(a, c).is_some_and(|d| d <= CROP_TOLERANCE),
                _ => false,
            };
            let renamed = name_of(e) != loc.name && !(loc.name.is_empty() && name_of(e).is_empty());
            // Same place, same look; or same place and nothing says otherwise.
            close || (!renamed && loc.crop.is_none())
        })
        .collect();
    one(near, Via::Pixels)
}

// ---------------------------------------------------------------------------
// Step checks.
// ---------------------------------------------------------------------------

/// The role as the driver's `verify` compares it (AX prefix off).
fn check_role(e: &Value) -> String {
    let r = e.get("role").and_then(Value::as_str).unwrap_or_default();
    r.strip_prefix("AX").unwrap_or(r).to_string()
}

fn secure(e: &Value) -> bool {
    let r = role_of(e);
    r.contains("secure") || r.contains("password")
}

/// What a step changed, as a verify check that was false before it and true
/// after: an element that appeared, else one that went away. Names only
/// (never values), templated; `None` when nothing named changed.
pub fn derive_check(before: &[&Value], after: &[&Value], inputs: &[(String, String)]) -> Option<Value> {
    fn lc(e: &Value) -> String {
        name_of(e).to_lowercase()
    }
    fn has(set: &[&Value], role: &str, name: &str) -> bool {
        set.iter().any(|e| check_role(e).eq_ignore_ascii_case(role) && lc(e).contains(name))
    }
    fn usable(e: &Value) -> bool {
        let n = name_of(e);
        !n.is_empty() && n.chars().count() <= 80 && !secure(e) && !check_role(e).is_empty()
    }
    for &e in after.iter().filter(|e| usable(e)) {
        if !has(before, &check_role(e), &lc(e)) {
            return Some(json!({ "role": check_role(e), "name": template(&name_of(e), inputs) }));
        }
    }
    for &e in before.iter().filter(|e| usable(e)) {
        if !has(after, &check_role(e), &lc(e)) {
            return Some(json!({ "role": check_role(e), "name": template(&name_of(e), inputs), "gone": true }));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Store (`computer_replays`, migration V248).
// ---------------------------------------------------------------------------

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let js = |i: usize| -> rusqlite::Result<Value> { Ok(serde_json::from_str(&r.get::<_, String>(i)?).unwrap_or(Value::Null)) };
    Ok(Entry {
        id: r.get(0)?,
        key: r.get(1)?,
        name: r.get(2)?,
        goal: r.get(3)?,
        app: r.get(4)?,
        window: r.get(5)?,
        variables: serde_json::from_value(js(6)?).unwrap_or_default(),
        success: serde_json::from_value(js(7)?).unwrap_or_default(),
        steps: serde_json::from_value(js(8)?).unwrap_or_default(),
        hits: r.get(9)?,
        heals: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
        last_used_at: r.get(13)?,
    })
}

const COLUMNS: &str =
    "id, cache_key, name, goal, app, window, variables_json, success_json, steps_json, hits, heals, created_at, updated_at, last_used_at";

pub fn by_key(conn: &Connection, owner: &str, key: &str) -> rusqlite::Result<Option<Entry>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM computer_replays WHERE owner = ?1 AND cache_key = ?2"), params![owner, key], row)
        .optional()
}

pub fn by_name(conn: &Connection, owner: &str, name: &str) -> rusqlite::Result<Option<Entry>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM computer_replays WHERE owner = ?1 AND name = ?2"), params![owner, name.trim()], row)
        .optional()
}

/// Write an entry (insert or rewrite by id). The same key or the same name
/// held by another row is that subtask's older recording: it is replaced.
pub fn save(conn: &mut Connection, owner: &str, e: &Entry) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM computer_replays WHERE owner = ?1 AND cache_key = ?2 AND id != ?3", params![owner, e.key, e.id])?;
    if let Some(name) = &e.name {
        tx.execute("DELETE FROM computer_replays WHERE owner = ?1 AND name = ?2 AND id != ?3", params![owner, name, e.id])?;
    }
    tx.execute(
        "INSERT INTO computer_replays (id, owner, cache_key, name, goal, app, window, variables_json, success_json, steps_json,
            hits, heals, created_at, updated_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(id) DO UPDATE SET cache_key = excluded.cache_key, name = excluded.name, goal = excluded.goal,
            app = excluded.app, window = excluded.window, variables_json = excluded.variables_json,
            success_json = excluded.success_json, steps_json = excluded.steps_json, hits = excluded.hits,
            heals = excluded.heals, updated_at = excluded.updated_at, last_used_at = excluded.last_used_at",
        params![
            e.id,
            owner,
            e.key,
            e.name,
            e.goal,
            e.app,
            e.window,
            json!(e.variables).to_string(),
            json!(e.success).to_string(),
            serde_json::to_string(&e.steps).unwrap_or_else(|_| "[]".into()),
            e.hits,
            e.heals,
            e.created_at,
            e.updated_at,
            e.last_used_at,
        ],
    )?;
    tx.commit()
}

/// A clean replay: count the hit.
pub fn touch(conn: &Connection, owner: &str, id: &str, at: &str) -> rusqlite::Result<()> {
    conn.execute("UPDATE computer_replays SET hits = hits + 1, last_used_at = ?3 WHERE owner = ?1 AND id = ?2", params![owner, id, at])?;
    Ok(())
}

/// The saved skills (named entries), newest use first.
pub fn skills(conn: &Connection, owner: &str) -> rusqlite::Result<Vec<Entry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM computer_replays WHERE owner = ?1 AND name IS NOT NULL
         ORDER BY COALESCE(last_used_at, updated_at) DESC LIMIT 200"
    ))?;
    let rows = stmt.query_map(params![owner], row)?;
    let out: rusqlite::Result<Vec<Entry>> = rows.collect();
    out
}

/// Delete a saved skill. Whether it existed.
pub fn forget(conn: &Connection, owner: &str, name: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM computer_replays WHERE owner = ?1 AND name = ?2", params![owner, name.trim()])? > 0)
}

/// A skill as `skills` lists it.
pub fn skill_summary(e: &Entry) -> Value {
    json!({
        "name": e.name,
        "goal": e.goal,
        "app": e.app,
        "inputs": e.variables,
        "steps": e.steps.len(),
        "runs": e.hits,
        "healed": e.heals,
        "updated_at": e.updated_at,
        "last_used_at": e.last_used_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> Vec<(String, String)> {
        vec![("email".into(), "ada@example.com".into()), ("digit".into(), "7".into())]
    }

    #[test]
    fn values_become_placeholders_and_come_back() {
        let i = inputs();
        assert_eq!(template("Signed in as ada@example.com", &i), "Signed in as {{email}}");
        assert_eq!(template("7", &i), "{{digit}}", "a short value only where the whole string is it");
        assert_eq!(template("17 items", &i), "17 items");
        let other = vec![("email".into(), "bo@example.org".into()), ("digit".into(), "9".into())];
        assert_eq!(substitute("Signed in as {{email}}", &other), "Signed in as bo@example.org");
        let check = template_json(&json!({ "role": "StaticText", "name": "Welcome ada@example.com" }), &i);
        assert_eq!(substitute_json(&check, &other)["name"], "Welcome bo@example.org");
    }

    #[test]
    fn key_follows_goal_app_window_and_input_names_not_values() {
        let a = vec![("email".to_string(), "ada@example.com".to_string())];
        let b = vec![("email".to_string(), "bo@example.org".to_string())];
        let (k1, w1) = cache_key("Sign up", "Safari", "Invoice 1042 — ada@example.com", &a);
        let (k2, w2) = cache_key("  sign   UP ", "safari", "Invoice 7 — bo@example.org", &b);
        assert_eq!(k1, k2, "new values and another record number replay the same steps");
        assert_eq!(w1, w2);
        assert_eq!(w1, "invoice # — {{email}}");
        // A different goal, app, window or input name is a different subtask.
        assert_ne!(k1, cache_key("Sign in", "Safari", "Invoice 1 — x", &a).0);
        assert_eq!(cache_key("Sign up as ada@example.com", "Safari", "", &a).0, cache_key("Sign up as bo@example.org", "Safari", "", &b).0);
        assert_ne!(k1, cache_key("Sign up", "Chrome", "Invoice 1042 — ada@example.com", &a).0);
        assert_ne!(k1, cache_key("Sign up", "Safari", "Settings", &a).0);
        assert_ne!(k1, cache_key("Sign up", "Safari", "Invoice 1042 — ada@example.com", &[("mail".into(), "ada@example.com".into())]).0);
    }

    fn el(id: &str, role: &str, name: &str, path: &str, bbox: [f64; 4], crop: Option<&str>) -> Value {
        let mut e = json!({ "id": id, "role": role, "name": name, "path": path, "bbox": bbox });
        if let Some(c) = crop {
            e["crop"] = json!(c);
        }
        e
    }

    #[test]
    fn resolve_tiers_id_name_path_pixels_and_never_guesses() {
        let win = el("w", "AXWindow", "Form", "/Window:0", [100.0, 100.0, 800.0, 600.0], None);
        let submit = el("e1", "AXButton", "Submit", "/Window:0/Button:0", [500.0, 600.0, 80.0, 30.0], Some("ffff0000ffff0000"));
        let loc = locator(&submit, (100.0, 100.0), &[]);
        assert_eq!(loc.bbox, Some([400.0, 500.0, 80.0, 30.0]));
        assert_eq!(resolve(&loc, &[&win, &submit], (100.0, 100.0)), Some(("e1".into(), Via::Id)));
        // Moved in the tree (new id) but still the one "Submit" button.
        let moved = el("e9", "AXButton", "Submit", "/Window:0/Group:0/Button:0", [10.0, 10.0, 80.0, 30.0], None);
        assert_eq!(resolve(&loc, &[&win, &moved], (100.0, 100.0)), Some(("e9".into(), Via::Name)));
        // Renamed in place: the same tree path.
        let renamed = el("e5", "AXButton", "Send", "/Window:0/Button:0", [0.0, 0.0, 1.0, 1.0], None);
        assert_eq!(resolve(&loc, &[&win, &renamed], (100.0, 100.0)), Some(("e5".into(), Via::Path)));
        // Renamed and moved in the tree, same place and look: pixels. The window moved too.
        let look = el("e6", "AXButton", "Go", "/Window:0/Group:1/Button:0", [602.0, 701.0, 80.0, 30.0], Some("ffff0000ffff0001"));
        let win2 = el("w", "AXWindow", "Form", "/Window:0", [200.0, 200.0, 800.0, 600.0], None);
        assert_eq!(resolve(&loc, &[&win2, &look], (200.0, 200.0)), Some(("e6".into(), Via::Pixels)));
        // Same place but another look, or two candidates: a miss, not a guess.
        let other = el("e7", "AXButton", "Go", "/x", [600.0, 700.0, 80.0, 30.0], Some("0000ffff0000ffff"));
        assert_eq!(resolve(&loc, &[&win2, &other], (200.0, 200.0)), None);
        let twin_a = el("a", "AXButton", "Submit", "/a", [0.0, 0.0, 1.0, 1.0], None);
        let twin_b = el("b", "AXButton", "Submit", "/b", [0.0, 0.0, 1.0, 1.0], None);
        assert_eq!(resolve(&loc, &[&win, &twin_a, &twin_b], (100.0, 100.0)), None);
        // Another role never matches.
        let link = el("e1", "AXLink", "Submit", "/Window:0/Button:0", [500.0, 600.0, 80.0, 30.0], None);
        assert_eq!(resolve(&loc, &[&win, &link], (100.0, 100.0)), None);
    }

    #[test]
    fn checks_come_from_what_appeared_or_went_away_names_only() {
        let i = inputs();
        let field = json!({ "id": "f", "role": "AXTextField", "name": "Email", "value": "ada@example.com" });
        let dialog = json!({ "id": "d", "role": "AXSheet", "name": "Confirm" });
        let thanks = json!({ "id": "t", "role": "AXStaticText", "name": "Thanks, ada@example.com" });
        let pw = json!({ "id": "p", "role": "AXSecureTextField", "name": "Password" });
        assert_eq!(derive_check(&[&field], &[&field, &pw, &thanks], &i), Some(json!({ "role": "StaticText", "name": "Thanks, {{email}}" })));
        assert_eq!(derive_check(&[&field, &dialog], &[&field], &i), Some(json!({ "role": "Sheet", "name": "Confirm", "gone": true })));
        assert_eq!(derive_check(&[&field], &[&field], &i), None);
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V248__computer_replays.sql")).unwrap();
        conn
    }

    fn entry(id: &str, key: &str, name: Option<&str>) -> Entry {
        Entry {
            id: id.into(),
            key: key.into(),
            name: name.map(str::to_string),
            goal: "Sign up".into(),
            app: "Safari".into(),
            variables: vec!["email".into()],
            success: vec![json!({ "name": "Thanks, {{email}}" })],
            steps: vec![Step {
                op: "set_value".into(),
                input: Some("email".into()),
                key: None,
                locator: Locator { id: "e1".into(), role: "textfield".into(), name: "Email".into(), ..Default::default() },
                label: "type the email into the text field \"Email\"".into(),
                check: None,
            }],
            created_at: "t0".into(),
            updated_at: "t0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn store_round_trips_replaces_by_key_and_name_and_lists_skills() {
        let mut conn = db();
        save(&mut conn, "u1", &entry("r1", "k1", None)).unwrap();
        let got = by_key(&conn, "u1", "k1").unwrap().unwrap();
        assert_eq!(got.steps, entry("r1", "k1", None).steps);
        assert!(by_key(&conn, "u2", "k1").unwrap().is_none(), "per owner");
        // A newer recording of the same subtask replaces the old one.
        save(&mut conn, "u1", &entry("r2", "k1", Some("signup"))).unwrap();
        assert!(by_key(&conn, "u1", "k1").unwrap().is_some_and(|e| e.id == "r2"));
        // save_as a taken name overwrites that skill.
        save(&mut conn, "u1", &entry("r3", "k3", Some("signup"))).unwrap();
        assert_eq!(by_name(&conn, "u1", "signup").unwrap().unwrap().id, "r3");
        assert!(by_key(&conn, "u1", "k1").unwrap().is_none());
        touch(&conn, "u1", "r3", "t1").unwrap();
        let list = skills(&conn, "u1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(skill_summary(&list[0])["runs"], 1);
        assert!(forget(&conn, "u1", "signup").unwrap());
        assert!(!forget(&conn, "u1", "signup").unwrap());
    }

    #[test]
    fn guard_refuses_a_stored_value() {
        let i = vec![("email".to_string(), "ada@example.com".to_string())];
        assert!(guard(&entry("r", "k", None), &i).is_ok());
        let mut leaky = entry("r", "k", None);
        leaky.steps[0].locator.name = "ADA@example.com".into();
        assert!(guard(&leaky, &i).is_err());
    }
}
