//! App Maps: a per-app graph of phone screens (contract:
//! `docs/contracts/CONTRACT-phone-app-map-2026-11-18.md`, sections 1-4, 10).
//!
//! * **Screens** are recognised by structure, not text: package + activity +
//!   the stable resource ids and classes outside the largest scrollable
//!   container ([`fingerprint`]). Scroll position never changes the id.
//! * **Edges** are tap, back and open_app steps between two screens of the
//!   same app, stored as a [`phone_replay::Locator`]. Typed text is never
//!   stored; typed steps are not recorded at all.
//! * **Passive growth.** Task and skill runs call [`observe`] after each
//!   acting step that changed the screen. It skips money apps, foreign
//!   packages, password screens and typed steps. A sensitive tap is stored
//!   (so the graph stays connected) but is never routable.
//! * **Routing.** [`route`] is a breadth-first search over routable edges,
//!   at most [`MAX_ROUTE_EDGES`] long. Two failed replays in a row take an
//!   edge out of routing; a later observation brings it back (never for a
//!   sensitive edge).
//! * **Store.** One JSON file per platform and package under
//!   `data_dir/phone-maps/<platform>/<package>.json`, private (0700 dir,
//!   0600 file), written by temp file + rename. Money apps are never
//!   written or read. A file with another `fp_version` is read as empty for
//!   matching and kept on disk until it is rewritten.
use crate::{auth::AuthUser, AppState};
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    io::Write,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};

use crate::phone_guard::{self, El};
use crate::phone_replay::Locator;

/// Map file schema version.
pub const MAP_VERSION: u32 = 1;
/// Fingerprint algorithm version (contract section 2).
pub const FP_VERSION: u32 = 1;
/// Screens kept per app.
pub const MAX_SCREENS: usize = 300;
/// The longest route [`route`] returns.
pub const MAX_ROUTE_EDGES: usize = 12;
/// Key-set overlap that still counts as the same screen.
pub const MATCH_JACCARD: f64 = 0.8;
/// Failed replays in a row that take an edge out of routing.
pub const STALE_AFTER_FAILS: u32 = 2;
/// Below this many resource-id keys a screen is weak (title added, low confidence).
const MIN_ID_KEYS: usize = 3;

// ─── read_ui nodes (contract section 1) ────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    #[default]
    Android,
    Ios,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::Ios => "ios",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "android" => Some(Platform::Android),
            "ios" | "iphone" => Some(Platform::Ios),
            _ => None,
        }
    }
}

/// One element of `read_ui`'s `nodes[]`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub resource_id: String,
    /// `[x1, y1, x2, y2]` in screen pixels.
    #[serde(default)]
    pub bounds: [i64; 4],
    #[serde(default)]
    pub clickable: bool,
    #[serde(default)]
    pub scrollable: bool,
    #[serde(default)]
    pub checkable: bool,
    #[serde(default)]
    pub editable: bool,
    #[serde(default)]
    pub password: bool,
    #[serde(default)]
    pub focused: bool,
}

impl UiNode {
    /// Lenient parse: missing, null or mistyped fields take their defaults.
    pub fn from_value(v: &Value) -> UiNode {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let mut bounds = [0i64; 4];
        if let Some(a) = v.get("bounds").and_then(Value::as_array) {
            for (i, x) in a.iter().take(4).enumerate() {
                bounds[i] = x.as_f64().unwrap_or(0.0) as i64;
            }
        }
        UiNode {
            index: v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
            class: s("class"),
            label: s("label"),
            resource_id: s("resource_id"),
            bounds,
            clickable: b("clickable"),
            scrollable: b("scrollable"),
            checkable: b("checkable"),
            editable: b("editable"),
            password: b("password"),
            focused: b("focused"),
        }
    }

    pub fn el(&self) -> El {
        El { class: self.class.clone(), label: self.label.clone(), id: self.resource_id.clone() }
    }

    fn area(&self) -> i64 {
        (self.bounds[2] - self.bounds[0]).max(0) * (self.bounds[3] - self.bounds[1]).max(0)
    }

    fn inside(&self, outer: &UiNode) -> bool {
        let (a, o) = (&self.bounds, &outer.bounds);
        a[0] >= o[0] && a[1] >= o[1] && a[2] <= o[2] && a[3] <= o[3]
    }
}

/// The map-relevant part of one `read_ui` result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiRead {
    /// Package or bundle id; `""` = unknown (maps off for this read).
    pub app: String,
    pub activity: Option<String>,
    pub app_version: Option<String>,
    pub nodes: Vec<UiNode>,
}

impl UiRead {
    pub fn has_password(&self) -> bool {
        self.nodes.iter().any(|n| n.password)
    }
}

/// Parse a `read_ui` result. `None` when it has no `nodes` (an old Desktop):
/// maps are off for that phone, and callers skip observe and route.
pub fn parse_read(v: &Value) -> Option<UiRead> {
    let nodes = v.get("nodes")?.as_array()?;
    let opt = |k: &str| v.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    Some(UiRead {
        app: opt("app").unwrap_or_default(),
        activity: opt("activity"),
        app_version: opt("app_version"),
        nodes: nodes.iter().map(UiNode::from_value).collect(),
    })
}

// ─── Fingerprint (contract section 2) ──────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    #[default]
    High,
    Low,
}

/// A live screen's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fp {
    /// `"v1:"` + 16 hex.
    pub fingerprint: String,
    pub app: String,
    pub activity: String,
    /// Sorted, de-duplicated key set.
    pub keys: Vec<String>,
    pub confidence: Confidence,
    /// The top title label (used in the hash only for weak screens, and for auto names).
    pub title: Option<String>,
}

/// A resource id that changes between runs: its name is digits, ends in a
/// separator plus digits (`item_3`), or holds a hex run of 8 or more.
fn dynamic_id(id: &str) -> bool {
    let name = id.rsplit(|c: char| c == '/' || c == ':').next().unwrap_or(id);
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if stem.len() < name.len() && (stem.is_empty() || stem.ends_with(|c: char| matches!(c, '_' | '-' | '.'))) {
        return true;
    }
    let mut run = 0;
    for c in name.chars() {
        if c.is_ascii_hexdigit() {
            run += 1;
            if run >= 8 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn hash16(input: &str) -> String {
    let h = hex::encode(Sha256::digest(input.as_bytes()));
    format!("v{FP_VERSION}:{}", &h[..16])
}

/// The structural fingerprint of an Android screen (contract section 2).
pub fn fingerprint(app: &str, activity: Option<&str>, nodes: &[UiNode]) -> Fp {
    let activity = activity.unwrap_or("").trim().to_string();
    // 1. Drop the largest scrollable container and everything inside it.
    let scroller = nodes.iter().filter(|n| n.scrollable && n.area() > 0).fold(None::<&UiNode>, |best, n| match best {
        Some(b) if b.area() >= n.area() => Some(b),
        _ => Some(n),
    });
    let kept: Vec<&UiNode> = nodes.iter().filter(|n| !n.password && scroller.map_or(true, |s| !n.inside(s))).collect();
    // 2-4. Stable keys: resource id, else class; dynamic ids dropped.
    let (mut keys, mut id_keys) = (BTreeSet::new(), BTreeSet::new());
    for n in &kept {
        let id = n.resource_id.trim();
        if !id.is_empty() {
            if !dynamic_id(id) {
                keys.insert(id.to_string());
                id_keys.insert(id.to_string());
            }
        } else if !n.class.trim().is_empty() {
            keys.insert(n.class.trim().to_string());
        }
    }
    let title = kept
        .iter()
        .filter(|n| !n.label.trim().is_empty())
        .min_by_key(|n| (n.bounds[1], n.bounds[0]))
        .map(|n| n.label.trim().chars().take(80).collect::<String>());
    let keys: Vec<String> = keys.into_iter().collect();
    let weak = id_keys.len() < MIN_ID_KEYS;
    let mut input = format!("{}\n{}\n{}", app.trim(), activity, keys.join("\n"));
    if weak {
        if let Some(t) = &title {
            input.push_str("\ntitle:");
            input.push_str(t);
        }
    }
    Fp {
        fingerprint: hash16(&input),
        app: app.trim().to_string(),
        activity,
        keys,
        confidence: if weak { Confidence::Low } else { Confidence::High },
        title,
    }
}

/// The iPhone (OCR) fingerprint: words from the top and bottom 12% of the
/// frame. Always low confidence (exact match only).
pub fn fingerprint_ios(app: &str, nodes: &[UiNode]) -> Fp {
    let height = nodes.iter().map(|n| n.bounds[3]).max().unwrap_or(0).max(1) as f64;
    let mut words = BTreeSet::new();
    for n in nodes {
        let (top, bottom) = (n.bounds[1] as f64 / height, n.bounds[3] as f64 / height);
        if bottom > 0.12 && top < 0.88 {
            continue;
        }
        for w in n.label.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            let w: String = w.chars().filter(|c| !c.is_ascii_digit()).collect();
            if w.chars().count() >= 3 {
                words.insert(w);
            }
        }
    }
    let title = nodes.iter().filter(|n| !n.label.trim().is_empty()).min_by_key(|n| (n.bounds[1], n.bounds[0])).map(|n| n.label.trim().chars().take(80).collect::<String>());
    let keys: Vec<String> = words.into_iter().collect();
    Fp {
        fingerprint: hash16(&format!("{}\n\n{}", app.trim(), keys.join("\n"))),
        app: app.trim().to_string(),
        activity: String::new(),
        keys,
        confidence: Confidence::Low,
        title,
    }
}

/// The fingerprint of one read on its platform.
pub fn fingerprint_for(platform: Platform, read: &UiRead) -> Fp {
    match platform {
        Platform::Android => fingerprint(&read.app, read.activity.as_deref(), &read.nodes),
        Platform::Ios => fingerprint_ios(&read.app, &read.nodes),
    }
}

fn jaccard(a: &[String], b: &[String]) -> f64 {
    let (a, b): (HashSet<&String>, HashSet<&String>) = (a.iter().collect(), b.iter().collect());
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// The known screen a live screen is: an exact fingerprint, else (high
/// confidence only) the best key-set overlap of at least [`MATCH_JACCARD`]
/// among screens of the same app and activity. `None` = a new screen.
pub fn match_screen(map: &AppMap, fp: &Fp) -> Option<String> {
    if map.fp_version != FP_VERSION || (!map.app.is_empty() && map.app != fp.app) {
        return None;
    }
    if let Some(s) = map.screens.values().find(|s| s.fingerprint == fp.fingerprint) {
        return Some(s.id.clone());
    }
    if fp.confidence == Confidence::Low {
        return None;
    }
    let mut best: Option<(&Screen, f64)> = None;
    for s in map.screens.values().filter(|s| s.confidence == Confidence::High && s.activity == fp.activity) {
        let j = jaccard(&s.keys, &fp.keys);
        if j >= MATCH_JACCARD && best.map_or(true, |(_, bj)| j > bj) {
            best = Some((s, j));
        }
    }
    best.map(|(s, _)| s.id.clone())
}

// ─── Map file (contract section 3) ─────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub id: String,
    pub fingerprint: String,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub name: String,
    /// `brain`, `auto` or `user`.
    #[serde(default = "auto")]
    pub named_by: String,
    #[serde(default)]
    pub activity: String,
    #[serde(default)]
    pub confidence: Confidence,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub last_seen: String,
}

fn auto() -> String {
    "auto".into()
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeAction {
    Tap,
    Back,
    OpenApp,
}

impl EdgeAction {
    /// The edge kind of a phone tool or task action; `None` for anything a
    /// map never stores (type, swipe, key, home, …).
    pub fn of(action: &str) -> Option<Self> {
        match action.trim().trim_start_matches("phone.") {
            "tap" | "find_and_tap" => Some(EdgeAction::Tap),
            "back" => Some(EdgeAction::Back),
            "open_app" => Some(EdgeAction::OpenApp),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub action: EdgeAction,
    /// Required for `tap`; `None` for `back` and `open_app`. `crop` is never stored.
    #[serde(default)]
    pub locator: Option<Locator>,
    #[serde(default)]
    pub ok: u32,
    #[serde(default)]
    pub fail: u32,
    #[serde(default)]
    pub consecutive_fail: u32,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub last_ok: Option<String>,
    #[serde(default)]
    pub last_fail: Option<String>,
    #[serde(default = "yes")]
    pub routable: bool,
    #[serde(default)]
    pub sensitive: bool,
}

/// What makes an edge unique: `(from, to, action, locator id or label)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EdgeKey {
    pub from: String,
    pub to: String,
    pub action: EdgeAction,
    pub target: String,
}

fn locator_target(l: Option<&Locator>) -> String {
    match l {
        None => String::new(),
        Some(l) if !l.resource_id.is_empty() => l.resource_id.clone(),
        Some(l) if !l.label.is_empty() => l.label.clone(),
        // Neither: class + place, so two unnamed buttons stay two edges.
        Some(l) => match l.rel {
            Some(r) => format!("{}@{:.2},{:.2}", l.class, r[0], r[1]),
            None => l.class.clone(),
        },
    }
}

impl Edge {
    pub fn key(&self) -> EdgeKey {
        EdgeKey { from: self.from.clone(), to: self.to.clone(), action: self.action, target: locator_target(self.locator.as_ref()) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Truncated {
    /// `time_budget`, `screen_cap`, `stopped`, `step_watchdog`, `foreign_app`.
    pub reason: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMap {
    pub version: u32,
    pub fp_version: u32,
    pub platform: Platform,
    pub app: String,
    /// The app's label when known (summary `name`); else the package is shown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub truncated: Option<Truncated>,
    #[serde(default)]
    pub screens: BTreeMap<String, Screen>,
    #[serde(default)]
    pub edges: Vec<Edge>,
    /// The next `s<n>`: screen ids are never reused, even after the cap drops one.
    #[serde(default)]
    pub next_screen: u64,
}

impl AppMap {
    pub fn new(platform: Platform, app: &str) -> Self {
        AppMap {
            version: MAP_VERSION,
            fp_version: FP_VERSION,
            platform,
            app: app.trim().to_string(),
            name: String::new(),
            app_version: String::new(),
            truncated: None,
            screens: BTreeMap::new(),
            edges: Vec::new(),
            next_screen: 0,
        }
    }

    pub fn edge_index(&self, key: &EdgeKey) -> Option<usize> {
        self.edges.iter().position(|e| &e.key() == key)
    }

    fn new_screen_id(&mut self) -> String {
        let floor = self.screens.keys().filter_map(|k| k.strip_prefix('s')?.parse::<u64>().ok()).max().map_or(0, |n| n + 1);
        let n = self.next_screen.max(floor);
        self.next_screen = n + 1;
        format!("s{n}")
    }

    /// The known screen for `fp`, or a new one. Updates `last_seen`. Returns `(id, new)`.
    pub fn ensure_screen(&mut self, fp: &Fp, now: &str) -> (String, bool) {
        if let Some(id) = match_screen(self, fp) {
            if let Some(s) = self.screens.get_mut(&id) {
                s.last_seen = now.to_string();
            }
            return (id, false);
        }
        let id = self.new_screen_id();
        self.screens.insert(
            id.clone(),
            Screen {
                id: id.clone(),
                fingerprint: fp.fingerprint.clone(),
                keys: fp.keys.clone(),
                name: auto_name(&fp.activity, fp.title.as_deref(), &id),
                named_by: auto(),
                activity: fp.activity.clone(),
                confidence: fp.confidence,
                first_seen: now.to_string(),
                last_seen: now.to_string(),
            },
        );
        (id, true)
    }

    /// Keep at most [`MAX_SCREENS`]: drop the least recently seen screen
    /// with no edges first, then the least recently seen; never one in `keep`.
    pub fn enforce_cap(&mut self, keep: &[&str]) {
        while self.screens.len() > MAX_SCREENS {
            let linked: HashSet<&str> = self.edges.iter().flat_map(|e| [e.from.as_str(), e.to.as_str()]).collect();
            let oldest = |skip_linked: bool| -> Option<String> {
                self.screens
                    .values()
                    .filter(|s| !keep.contains(&s.id.as_str()) && !(skip_linked && linked.contains(s.id.as_str())))
                    .min_by(|a, b| a.last_seen.cmp(&b.last_seen))
                    .map(|s| s.id.clone())
            };
            let victim = oldest(true).or_else(|| oldest(false));
            let Some(id) = victim else { break };
            self.screens.remove(&id);
            self.edges.retain(|e| e.from != id && e.to != id);
        }
    }
}

/// With no brain: activity short name + top title, one short line.
fn auto_name(activity: &str, title: Option<&str>, id: &str) -> String {
    let short = activity.rsplit('.').next().unwrap_or("").rsplit('$').next().unwrap_or("");
    let name = format!("{} {}", short, title.unwrap_or("")).trim().chars().take(80).collect::<String>();
    if name.is_empty() {
        id.to_string()
    } else {
        name
    }
}

// ─── Store ─────────────────────────────────────────────────────────────────

fn maps_dir(data_dir: &FsPath) -> PathBuf {
    data_dir.join("phone-maps")
}

/// A package or bundle id as a file name: characters outside
/// `[A-Za-z0-9._-]` become `_`. Empty, too long or dot-led names are refused.
pub fn file_stem(app: &str) -> Result<String, String> {
    let app = app.trim();
    if app.is_empty() || app.chars().count() > 200 {
        return Err("an app is a package or bundle id of 1-200 characters".into());
    }
    let stem: String = app.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if stem.starts_with('.') {
        return Err("an app id can't start with a dot".into());
    }
    Ok(stem)
}

pub fn map_path(data_dir: &FsPath, platform: Platform, app: &str) -> Result<PathBuf, String> {
    Ok(maps_dir(data_dir).join(platform.as_str()).join(format!("{}.json", file_stem(app)?)))
}

/// The map file as stored (any `fp_version`). Money apps: always `None`.
pub fn load(data_dir: &FsPath, platform: Platform, app: &str) -> Option<AppMap> {
    if phone_guard::is_money_app(app) {
        return None;
    }
    serde_json::from_slice(&std::fs::read(map_path(data_dir, platform, app).ok()?).ok()?).ok()
}

/// The map to match and route against: empty when missing, unreadable or
/// written with another `fp_version` (that file stays until it is rewritten).
pub fn load_for_matching(data_dir: &FsPath, platform: Platform, app: &str) -> AppMap {
    match load(data_dir, platform, app) {
        Some(m) if m.fp_version == FP_VERSION && m.app == app.trim() => m,
        _ => AppMap::new(platform, app),
    }
}

fn private_dir(dir: &FsPath) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Write a map: temp file (0600) + fsync + rename. Money apps are refused.
pub fn save(data_dir: &FsPath, map: &AppMap) -> Result<(), String> {
    if phone_guard::is_money_app(&map.app) {
        return Err("money_app".into());
    }
    let path = map_path(data_dir, map.platform, &map.app)?;
    private_dir(&maps_dir(data_dir))?;
    private_dir(path.parent().ok_or("bad map path")?)?;
    let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    let tmp = path.with_file_name(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("map"), uuid::Uuid::new_v4().simple()));
    let written = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|e| e.to_string())
}

/// One read-modify-write at a time in this process (task and Explore runs
/// may observe the same app at once).
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Load the map for matching, change it, and save it when `f` says so.
fn with_map<R>(data_dir: &FsPath, platform: Platform, app: &str, f: impl FnOnce(&mut AppMap) -> (R, bool)) -> Result<R, String> {
    if phone_guard::is_money_app(app) {
        return Err("money_app".into());
    }
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_for_matching(data_dir, platform, app);
    let (out, changed) = f(&mut map);
    if changed {
        save(data_dir, &map)?;
    }
    Ok(out)
}

/// Load, change and save one app's map (step 5's `record_ok` / `record_fail` path).
pub fn update<R>(data_dir: &FsPath, platform: Platform, app: &str, f: impl FnOnce(&mut AppMap) -> R) -> Result<R, String> {
    with_map(data_dir, platform, app, |m| (f(m), true))
}

/// Every stored map (money apps never), optionally one platform.
pub fn list(data_dir: &FsPath, platform: Option<Platform>) -> Vec<AppMap> {
    let platforms: Vec<Platform> = match platform {
        Some(p) => vec![p],
        None => vec![Platform::Android, Platform::Ios],
    };
    let mut out = Vec::new();
    for p in platforms {
        for entry in std::fs::read_dir(maps_dir(data_dir).join(p.as_str())).into_iter().flatten().filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(m) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<AppMap>(&b).ok()) {
                if !phone_guard::is_money_app(&m.app) {
                    out.push(m);
                }
            }
        }
    }
    out.sort_by(|a, b| last_seen(b).cmp(&last_seen(a)).then_with(|| a.app.cmp(&b.app)));
    out
}

/// Delete one map file. `true` when a file was removed.
pub fn forget(data_dir: &FsPath, platform: Platform, app: &str) -> bool {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    map_path(data_dir, platform, app).map(|p| std::fs::remove_file(p).is_ok()).unwrap_or(false)
}

fn last_seen(m: &AppMap) -> String {
    m.screens.values().map(|s| s.last_seen.clone()).max().unwrap_or_default()
}

/// The summary row (contract section 4). `stale_edges` = routable edges
/// verified on another version than the map's latest known one.
pub fn summary(m: &AppMap) -> Value {
    let routable = m.edges.iter().filter(|e| e.routable && !e.sensitive);
    let stale = routable.clone().filter(|e| !m.app_version.is_empty() && !e.app_version.is_empty() && e.app_version != m.app_version).count();
    let last = last_seen(m);
    json!({
        "app": m.app,
        "platform": m.platform,
        "app_version": m.app_version,
        "name": if m.name.is_empty() { m.app.clone() } else { m.name.clone() },
        "screens": m.screens.len(),
        "edges": m.edges.len(),
        "routable_edges": routable.count(),
        "stale_edges": stale,
        "truncated": m.truncated,
        "last_seen": if last.is_empty() { Value::Null } else { json!(last) },
    })
}

// ─── Passive observation ───────────────────────────────────────────────────

/// One acting step that may have changed the screen.
pub struct Observation<'a> {
    pub platform: Platform,
    /// The read before the step.
    pub prev: &'a UiRead,
    /// The phone tool or task action (`phone.tap`, `tap`, `back`, `open_app`, `type`, …).
    pub action: &'a str,
    /// The tapped element (taps only).
    pub locator: Option<&'a Locator>,
    /// The guard or the model called this step sensitive.
    pub sensitive: bool,
    /// The read after the step.
    pub new: &'a UiRead,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observed {
    /// Nothing written. Reasons: `money_app`, `no_app`, `foreign_app`,
    /// `password_screen`, `not_mapped` (type, swipe, key, …), `no_locator`, `same_screen`.
    Skipped(&'static str),
    Recorded { from: String, to: String, new_screens: usize, sensitive: bool },
}

/// Whether the guard's own rules call this tap sensitive.
fn sensitive_tap(l: &Locator, app: &str) -> bool {
    let el = El { class: l.class.clone(), label: l.label.clone(), id: l.resource_id.clone() };
    let args = Map::new();
    phone_guard::label_effect(&el).is_some()
        || phone_guard::is_secret_field(&el)
        || phone_guard::rule_effect(&phone_guard::Action { tool: "phone.tap", args: &args, el: Some(&el), app: Some(app) }).is_some()
}

/// The skip rules that need no map. `Ok((action, locator, sensitive))` to record.
fn precheck(o: &Observation<'_>) -> Result<(EdgeAction, Option<Locator>, bool), &'static str> {
    let (prev_app, new_app) = (o.prev.app.trim(), o.new.app.trim());
    if phone_guard::is_money_app(prev_app) || phone_guard::is_money_app(new_app) {
        return Err("money_app");
    }
    let Some(action) = EdgeAction::of(o.action) else { return Err("not_mapped") };
    if prev_app.is_empty() || new_app.is_empty() {
        return Err("no_app");
    }
    if prev_app != new_app {
        return Err("foreign_app");
    }
    if o.prev.has_password() || o.new.has_password() {
        return Err("password_screen");
    }
    let locator = match action {
        EdgeAction::Tap => {
            let Some(l) = o.locator else { return Err("no_locator") };
            Some(Locator { crop: None, ..l.clone() })
        }
        EdgeAction::Back | EdgeAction::OpenApp => None,
    };
    let sensitive = o.sensitive || locator.as_ref().is_some_and(|l| sensitive_tap(l, prev_app));
    Ok((action, locator, sensitive))
}

/// Record `prev --action--> new` into an in-memory map. Pure (no files).
pub fn observe_into(map: &mut AppMap, o: &Observation<'_>, now: &str) -> Observed {
    let (action, locator, sensitive) = match precheck(o) {
        Ok(v) => v,
        Err(why) => return Observed::Skipped(why),
    };
    if !map.app.is_empty() && map.app != o.prev.app.trim() {
        return Observed::Skipped("foreign_app");
    }
    let (fp_prev, fp_new) = (fingerprint_for(o.platform, o.prev), fingerprint_for(o.platform, o.new));
    if fp_prev.fingerprint == fp_new.fingerprint {
        return Observed::Skipped("same_screen");
    }
    if map.app.is_empty() {
        map.app = o.prev.app.trim().to_string();
    }
    if let Some(v) = o.new.app_version.as_deref().filter(|v| !v.is_empty()) {
        map.app_version = v.to_string();
    }
    let (from, from_new) = map.ensure_screen(&fp_prev, now);
    let (to, to_new) = map.ensure_screen(&fp_new, now);
    let new_screens = from_new as usize + to_new as usize;
    if from == to {
        return Observed::Skipped("same_screen");
    }
    let key = EdgeKey { from: from.clone(), to: to.clone(), action, target: locator_target(locator.as_ref()) };
    let version = map.app_version.clone();
    let sensitive = match map.edge_index(&key) {
        Some(i) => {
            let e = &mut map.edges[i];
            e.sensitive |= sensitive;
            e.ok += 1;
            e.consecutive_fail = 0;
            e.last_ok = Some(now.to_string());
            e.app_version = version;
            // A seen-working edge is routable again, never a sensitive one.
            e.routable = !e.sensitive;
            e.sensitive
        }
        None => {
            map.edges.push(Edge {
                from: from.clone(),
                to: to.clone(),
                action,
                locator,
                ok: 1,
                fail: 0,
                consecutive_fail: 0,
                app_version: version,
                last_ok: Some(now.to_string()),
                last_fail: None,
                routable: !sensitive,
                sensitive,
            });
            sensitive
        }
    };
    map.enforce_cap(&[from.as_str(), to.as_str()]);
    Observed::Recorded { from, to, new_screens, sensitive }
}

/// Passive observation into the stored map. Money apps, foreign packages,
/// password screens and typed steps are skipped before any file is touched.
pub fn observe(data_dir: &FsPath, o: &Observation<'_>) -> Result<Observed, String> {
    if let Err(why) = precheck(o) {
        return Ok(Observed::Skipped(why));
    }
    let now = chrono::Utc::now().to_rfc3339();
    with_map(data_dir, o.platform, o.prev.app.trim(), |m| {
        let r = observe_into(m, o, &now);
        let changed = matches!(r, Observed::Recorded { .. });
        (r, changed)
    })
}

// ─── Edge stats (contract section 10) ──────────────────────────────────────

/// A replayed edge worked. `false` when the edge is not in the map.
pub fn record_ok(map: &mut AppMap, key: &EdgeKey, now: &str) -> bool {
    let version = map.app_version.clone();
    let Some(i) = map.edge_index(key) else { return false };
    let e = &mut map.edges[i];
    e.ok += 1;
    e.consecutive_fail = 0;
    e.last_ok = Some(now.to_string());
    if !version.is_empty() {
        e.app_version = version;
    }
    e.routable = !e.sensitive;
    true
}

/// A replayed edge failed. Two in a row take it out of routing.
pub fn record_fail(map: &mut AppMap, key: &EdgeKey, now: &str) -> bool {
    let Some(i) = map.edge_index(key) else { return false };
    let e = &mut map.edges[i];
    e.fail += 1;
    e.consecutive_fail += 1;
    e.last_fail = Some(now.to_string());
    if e.consecutive_fail >= STALE_AFTER_FAILS || e.sensitive {
        e.routable = false;
    }
    true
}

// ─── Routing ───────────────────────────────────────────────────────────────

/// The shortest path of routable, non-sensitive edges from `from` to `to`,
/// at most [`MAX_ROUTE_EDGES`] long. `Some(vec![])` when already there;
/// `None` when there is no such path (or the map is from another `fp_version`).
pub fn route(map: &AppMap, from: &str, to: &str) -> Option<Vec<Edge>> {
    if map.fp_version != FP_VERSION || !map.screens.contains_key(from) || !map.screens.contains_key(to) {
        return None;
    }
    if from == to {
        return Some(Vec::new());
    }
    // Adjacency: per screen, its usable edges, the most proven first.
    let mut adj: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in map.edges.iter().enumerate() {
        if e.routable && !e.sensitive && e.from != e.to && map.screens.contains_key(&e.to) && (e.action != EdgeAction::Tap || e.locator.is_some()) {
            adj.entry(e.from.as_str()).or_default().push(i);
        }
    }
    for v in adj.values_mut() {
        v.sort_by(|a, b| map.edges[*b].ok.cmp(&map.edges[*a].ok).then(a.cmp(b)));
    }
    let mut prev: HashMap<&str, usize> = HashMap::new();
    let mut depth: HashMap<&str, usize> = HashMap::from([(from, 0)]);
    let mut queue = VecDeque::from([from]);
    while let Some(here) = queue.pop_front() {
        let d = depth[here];
        if d >= MAX_ROUTE_EDGES {
            continue;
        }
        for &i in adj.get(here).map(Vec::as_slice).unwrap_or_default() {
            let next = map.edges[i].to.as_str();
            if depth.contains_key(next) {
                continue;
            }
            depth.insert(next, d + 1);
            prev.insert(next, i);
            if next == to {
                let mut path = Vec::new();
                let mut at = to;
                while at != from {
                    let i = prev[at];
                    path.push(map.edges[i].clone());
                    at = map.edges[i].from.as_str();
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(next);
        }
    }
    None
}

// ─── Routes (contract section 4) ───────────────────────────────────────────

fn err(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({ "ok": false, "error": code, "message": message.into() }))).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct MapQuery {
    #[serde(default)]
    pub platform: Option<String>,
}

/// `?platform=`: `Ok(None)` when absent, 400 when unknown.
fn platform_of(q: &MapQuery) -> Result<Option<Platform>, Response> {
    match q.platform.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        None => Ok(None),
        Some(p) => Platform::parse(p).map(Some).ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid_args", "platform is android or ios")),
    }
}

/// The `:app` path value checked: 400 when malformed, 403 for a money app.
fn app_of(app: &str) -> Result<String, Response> {
    file_stem(app).map_err(|e| err(StatusCode::BAD_REQUEST, "invalid_args", e))?;
    if phone_guard::is_money_app(app) {
        return Err(err(StatusCode::FORBIDDEN, "money_app", "maps are never kept for bank, wallet or broker apps"));
    }
    Ok(app.trim().to_string())
}

async fn list_maps(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Query(q): Query<MapQuery>) -> Response {
    let platform = match platform_of(&q) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let dir = state.data_dir.clone();
    let maps = tokio::task::spawn_blocking(move || list(&dir, platform)).await.unwrap_or_default();
    Json(json!({ "ok": true, "maps": maps.iter().map(summary).collect::<Vec<_>>() })).into_response()
}

async fn get_map(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Path(app): Path<String>, Query(q): Query<MapQuery>) -> Response {
    let (platform, app) = match (platform_of(&q), app_of(&app)) {
        (Ok(p), Ok(a)) => (p, a),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let order = match platform {
        Some(p) => vec![p],
        None => vec![Platform::Android, Platform::Ios],
    };
    match order.into_iter().find_map(|p| load(&state.data_dir, p, &app)) {
        Some(m) => Json(json!({ "ok": true, "map": m })).into_response(),
        None => err(StatusCode::NOT_FOUND, "map_not_found", format!("no app map for {app}")),
    }
}

async fn delete_map(State(state): State<Arc<AppState>>, Extension(_user): Extension<AuthUser>, Path(app): Path<String>, Query(q): Query<MapQuery>) -> Response {
    let (platform, app) = match (platform_of(&q), app_of(&app)) {
        (Ok(p), Ok(a)) => (p, a),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let order = match platform {
        Some(p) => vec![p],
        None => vec![Platform::Android, Platform::Ios],
    };
    // Every platform's file goes when none is named; deleting twice is fine.
    let deleted = order.into_iter().fold(false, |any, p| forget(&state.data_dir, p, &app) || any);
    Json(json!({ "ok": true, "deleted": deleted })).into_response()
}

/// `GET /phones/maps`, `GET|DELETE /phones/maps/:app`. Static `maps` sits
/// beside the `/phones/:serial/...` routes the same way `/phones/skills` does.
pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/phones/maps", get(list_maps)).route("/phones/maps/:app", get(get_map).delete(delete_map))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(class: &str, id: &str, label: &str, bounds: [i64; 4]) -> UiNode {
        UiNode { class: class.into(), resource_id: id.into(), label: label.into(), bounds, clickable: true, ..Default::default() }
    }

    fn scroller(bounds: [i64; 4]) -> UiNode {
        UiNode { class: "androidx.recyclerview.widget.RecyclerView".into(), resource_id: "com.x:id/list".into(), bounds, scrollable: true, ..Default::default() }
    }

    /// A settings-like screen: a toolbar outside the list, rows inside it.
    fn list_screen(app: &str, activity: &str, rows: &[&str], top: i64) -> UiRead {
        let mut nodes = vec![
            node("android.widget.FrameLayout", "", "", [0, 0, 1080, 2400]),
            node("android.widget.TextView", "com.x:id/toolbar_title", "Settings", [40, 80, 600, 180]),
            node("android.widget.ImageButton", "com.x:id/search", "Search", [900, 80, 1040, 180]),
            node("android.widget.ImageButton", "com.x:id/up", "Navigate up", [0, 80, 120, 180]),
            scroller([0, 200, 1080, 2300]),
        ];
        for (i, r) in rows.iter().enumerate() {
            let y = top + i as i64 * 150;
            nodes.push(node("android.widget.TextView", &format!("com.x:id/row_{i}"), r, [0, y.max(200), 1080, (y + 150).min(2300)]));
            nodes.push(node("android.widget.TextView", "android:id/title", r, [40, y.max(200), 800, (y + 100).min(2300)]));
        }
        UiRead { app: app.into(), activity: Some(activity.into()), app_version: Some("14".into()), nodes }
    }

    fn detail_screen(app: &str, title: &str, button: &str) -> UiRead {
        UiRead {
            app: app.into(),
            activity: Some(format!("{app}.DetailActivity")),
            app_version: Some("14".into()),
            nodes: vec![
                node("android.widget.TextView", "com.x:id/detail_title", title, [40, 80, 600, 180]),
                node("android.widget.ImageButton", "com.x:id/up", "Navigate up", [0, 80, 120, 180]),
                node("android.widget.Button", "com.x:id/action", button, [40, 400, 1040, 520]),
                node("android.widget.Switch", "com.x:id/toggle", "", [900, 600, 1040, 700]),
            ],
        }
    }

    fn tap_on(label: &str, id: &str) -> Locator {
        Locator { class: "android.widget.TextView".into(), label: label.into(), resource_id: id.into(), rel: Some([0.1, 0.2, 0.8, 0.07]), crop: Some("ffff".into()) }
    }

    fn obs<'a>(prev: &'a UiRead, action: &'a str, locator: Option<&'a Locator>, new: &'a UiRead) -> Observation<'a> {
        Observation { platform: Platform::Android, prev, action, locator, sensitive: false, new }
    }

    const SETTINGS: &str = "com.android.settings";

    #[test]
    fn read_ui_without_nodes_means_maps_off() {
        assert!(parse_read(&json!({ "ok": true, "text": "Button \"OK\"" })).is_none());
        let r = parse_read(&json!({ "ok": true, "text": "", "app": SETTINGS, "activity": null, "nodes": [
            { "index": 0, "class": "android.widget.TextView", "label": "Wi-Fi", "resource_id": "android:id/title", "bounds": [0, 210.0, 1080, 330], "clickable": true },
            { "label": null, "bounds": "junk" }
        ] }))
        .unwrap();
        assert_eq!(r.app, SETTINGS);
        assert_eq!(r.activity, None);
        assert_eq!(r.nodes.len(), 2);
        assert_eq!(r.nodes[0].bounds, [0, 210, 1080, 330]);
        assert!(r.nodes[0].clickable && !r.nodes[1].clickable);
    }

    #[test]
    fn two_scroll_positions_are_one_screen() {
        let act = "com.android.settings.Settings";
        let top = list_screen(SETTINGS, act, &["Network & internet", "Connected devices", "Apps", "Notifications"], 200);
        let scrolled = list_screen(SETTINGS, act, &["Apps", "Notifications", "Battery", "Storage", "Sound"], 120);
        let (a, b) = (fingerprint_for(Platform::Android, &top), fingerprint_for(Platform::Android, &scrolled));
        assert_eq!(a, Fp { title: a.title.clone(), ..b.clone() }, "only the row contents differ, and they are inside the list");
        assert_eq!(a.fingerprint, b.fingerprint);
        assert!(a.fingerprint.starts_with("v1:") && a.fingerprint.len() == 3 + 16);
        assert_eq!(a.confidence, Confidence::High);
        assert!(!a.keys.iter().any(|k| k.contains("row_")), "{:?}", a.keys);
        let mut map = AppMap::new(Platform::Android, SETTINGS);
        let (id1, new1) = map.ensure_screen(&a, "2026-11-19T00:00:00Z");
        let (id2, new2) = map.ensure_screen(&b, "2026-11-19T00:00:01Z");
        assert_eq!((id1.as_str(), new1, new2), (id2.as_str(), true, false));
        // A different activity is a different screen.
        let other = fingerprint_for(Platform::Android, &list_screen(SETTINGS, "com.android.settings.SubSettings", &["Apps"], 200));
        assert_ne!(match_screen(&map, &other), Some(id1));
    }

    #[test]
    fn overlap_matches_high_confidence_but_not_weak_screens() {
        let mut nodes = vec![
            node("A", "com.x:id/a", "", [0, 0, 10, 10]),
            node("B", "com.x:id/b", "", [0, 10, 10, 20]),
            node("C", "com.x:id/c", "", [0, 20, 10, 30]),
            node("D", "com.x:id/d", "", [0, 30, 10, 40]),
            node("E", "com.x:id/e", "", [0, 40, 10, 50]),
        ];
        let base = fingerprint("com.x", Some("Main"), &nodes);
        let mut map = AppMap::new(Platform::Android, "com.x");
        let (id, _) = map.ensure_screen(&base, "t");
        // One extra key of six: overlap 5/6 >= 0.8.
        nodes.push(node("F", "com.x:id/f", "", [0, 50, 10, 60]));
        let more = fingerprint("com.x", Some("Main"), &nodes);
        assert_ne!(more.fingerprint, base.fingerprint);
        assert_eq!(match_screen(&map, &more), Some(id));
        // Weak (fewer than 3 resource ids): exact only, title in the hash.
        let weak = |title: &str| fingerprint("com.y", Some("Main"), &[node("android.view.View", "", title, [0, 0, 100, 50]), node("android.view.View", "", "x", [0, 60, 100, 90])]);
        assert_eq!(weak("Inbox").confidence, Confidence::Low);
        assert_ne!(weak("Inbox").fingerprint, weak("Sent").fingerprint);
        // Dynamic ids are not keys.
        assert!(dynamic_id("com.x:id/item_12") && dynamic_id("com.x:id/1234") && dynamic_id("com.x:id/row_deadbeef99"));
        assert!(!dynamic_id("android:id/button1") && !dynamic_id("com.x:id/title"));
    }

    #[test]
    fn a_foreign_package_is_not_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = list_screen(SETTINGS, "Settings", &["Help"], 200);
        let browser = detail_screen("com.android.chrome", "Help center", "Search");
        let l = tap_on("Help", "com.x:id/row_0");
        assert_eq!(observe(tmp.path(), &obs(&prev, "phone.tap", Some(&l), &browser)).unwrap(), Observed::Skipped("foreign_app"));
        assert!(!maps_dir(tmp.path()).exists());
        // An in-memory map of the first app also refuses it.
        let mut map = AppMap::new(Platform::Android, SETTINGS);
        assert_eq!(observe_into(&mut map, &obs(&prev, "tap", Some(&l), &browser), "t"), Observed::Skipped("foreign_app"));
        assert!(map.screens.is_empty() && map.edges.is_empty());
    }

    #[test]
    fn typed_steps_and_password_screens_are_not_recorded() {
        let mut map = AppMap::new(Platform::Android, SETTINGS);
        let prev = list_screen(SETTINGS, "Settings", &["Wi-Fi"], 200);
        let next = detail_screen(SETTINGS, "Wi-Fi", "Add network");
        assert_eq!(observe_into(&mut map, &obs(&prev, "phone.type", None, &next), "t"), Observed::Skipped("not_mapped"));
        assert_eq!(observe_into(&mut map, &obs(&prev, "swipe", None, &next), "t"), Observed::Skipped("not_mapped"));
        let mut secret = next.clone();
        secret.nodes.push(UiNode { class: "android.widget.EditText".into(), editable: true, password: true, ..Default::default() });
        assert_eq!(observe_into(&mut map, &obs(&prev, "tap", Some(&tap_on("Wi-Fi", "android:id/title")), &secret), "t"), Observed::Skipped("password_screen"));
        assert!(map.screens.is_empty());
    }

    #[test]
    fn a_money_app_is_never_observed_or_written() {
        let tmp = tempfile::tempdir().unwrap();
        let app = "com.chase.sig.android";
        let prev = list_screen(app, "Home", &["Accounts"], 200);
        let next = detail_screen(app, "Accounts", "Transfer");
        let l = tap_on("Accounts", "com.x:id/row_0");
        assert_eq!(observe(tmp.path(), &obs(&prev, "phone.tap", Some(&l), &next)).unwrap(), Observed::Skipped("money_app"));
        assert!(!maps_dir(tmp.path()).exists(), "no file and no folder");
        assert!(save(tmp.path(), &AppMap::new(Platform::Android, app)).is_err());
        assert!(update(tmp.path(), Platform::Android, app, |_| ()).is_err());
        assert!(load(tmp.path(), Platform::Android, app).is_none());
    }

    #[test]
    fn a_sensitive_edge_is_stored_but_never_routable() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = detail_screen(SETTINGS, "Storage", "Free up space");
        // A dialog on its own activity (same ids as `prev` would overlap 0.8 and merge).
        let mut next2 = detail_screen(SETTINGS, "Confirm", "OK");
        next2.activity = Some("com.android.settings.ConfirmDialog".into());
        next2.nodes.push(node("android.widget.TextView", "com.x:id/warning", "This can't be undone", [40, 800, 1040, 900]));
        let del = tap_on("Delete all", "com.x:id/delete");
        let r = observe(tmp.path(), &obs(&prev, "phone.tap", Some(&del), &next2)).unwrap();
        let Observed::Recorded { from, to, sensitive, .. } = r.clone() else { panic!("expected a record, got {r:?}") };
        assert!(sensitive);
        let map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert_eq!(map.edges.len(), 1);
        let e = &map.edges[0];
        assert!(e.sensitive && !e.routable);
        assert!(e.locator.as_ref().unwrap().crop.is_none(), "crop is never stored in maps");
        assert_eq!(route(&map, &from, &to), None);
        // Seen again: still not routable. A replay "ok" doesn't restore it either.
        observe(tmp.path(), &obs(&prev, "phone.tap", Some(&del), &next2)).unwrap();
        let mut map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert_eq!((map.edges.len(), map.edges[0].ok, map.edges[0].routable), (1, 2, false));
        let key = map.edges[0].key();
        assert!(record_ok(&mut map, &key, "t"));
        assert!(!map.edges[0].routable);
        // The model's own flag also makes an edge sensitive.
        let mut m2 = AppMap::new(Platform::Android, SETTINGS);
        let plain = tap_on("Storage", "com.x:id/storage");
        let o = Observation { sensitive: true, ..obs(&prev, "tap", Some(&plain), &next2) };
        assert!(matches!(observe_into(&mut m2, &o, "t"), Observed::Recorded { sensitive: true, .. }));
    }

    fn screen(id: &str) -> Screen {
        Screen { id: id.into(), fingerprint: format!("v1:{id:0>16}"), name: id.into(), named_by: auto(), ..Default::default() }
    }

    fn edge(from: &str, to: &str, routable: bool) -> Edge {
        Edge {
            from: from.into(),
            to: to.into(),
            action: EdgeAction::Tap,
            locator: Some(Locator { class: "Button".into(), label: format!("to {to}"), ..Default::default() }),
            ok: 1,
            fail: 0,
            consecutive_fail: 0,
            app_version: String::new(),
            last_ok: None,
            last_fail: None,
            routable,
            sensitive: false,
        }
    }

    fn graph(n: usize, edges: Vec<Edge>) -> AppMap {
        let mut m = AppMap::new(Platform::Android, "com.x");
        for i in 0..n {
            let id = format!("s{i}");
            m.screens.insert(id.clone(), screen(&id));
        }
        m.edges = edges;
        m
    }

    #[test]
    fn route_is_shortest_and_skips_unroutable_edges() {
        let mut back = edge("s3", "s0", true);
        back.action = EdgeAction::Back;
        back.locator = None;
        let m = graph(
            6,
            vec![
                edge("s0", "s1", true),
                edge("s1", "s2", true),
                edge("s2", "s3", true),
                edge("s0", "s3", false), // shortest, but stale
                edge("s0", "s4", true),
                edge("s4", "s3", true),
                back,
            ],
        );
        let path = route(&m, "s0", "s3").unwrap();
        assert_eq!(path.iter().map(|e| (e.from.as_str(), e.to.as_str())).collect::<Vec<_>>(), vec![("s0", "s4"), ("s4", "s3")]);
        assert_eq!(route(&m, "s3", "s0").unwrap().len(), 1);
        assert_eq!(route(&m, "s0", "s0"), Some(vec![]));
        assert_eq!(route(&m, "s5", "s0"), None, "no edges out of s5");
        assert_eq!(route(&m, "s0", "s9"), None, "unknown screen");
        // A sensitive edge is skipped even when marked routable by hand.
        let mut m2 = graph(2, vec![edge("s0", "s1", true)]);
        m2.edges[0].sensitive = true;
        assert_eq!(route(&m2, "s0", "s1"), None);
        // At most 12 edges.
        let chain = |n: usize| graph(n + 1, (0..n).map(|i| edge(&format!("s{i}"), &format!("s{}", i + 1), true)).collect());
        assert_eq!(route(&chain(12), "s0", "s12").unwrap().len(), 12);
        assert_eq!(route(&chain(13), "s0", "s13"), None);
    }

    #[test]
    fn two_fails_take_an_edge_out_and_a_later_observation_restores_it() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = list_screen(SETTINGS, "com.android.settings.Settings", &["Network & internet"], 200);
        let next = detail_screen(SETTINGS, "Network & internet", "Wi-Fi");
        let l = tap_on("Network & internet", "android:id/title");
        let Observed::Recorded { from, to, new_screens, .. } = observe(tmp.path(), &obs(&prev, "phone.tap", Some(&l), &next)).unwrap() else { panic!() };
        assert_eq!(new_screens, 2);
        let map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert_eq!(route(&map, &from, &to).unwrap().len(), 1);
        let key = map.edges[0].key();
        update(tmp.path(), Platform::Android, SETTINGS, |m| assert!(record_fail(m, &key, "t1"))).unwrap();
        let map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert!(map.edges[0].routable, "one fail keeps it");
        update(tmp.path(), Platform::Android, SETTINGS, |m| assert!(record_fail(m, &key, "t2"))).unwrap();
        let map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert_eq!((map.edges[0].fail, map.edges[0].consecutive_fail, map.edges[0].routable), (2, 2, false));
        assert_eq!(map.edges[0].last_fail.as_deref(), Some("t2"));
        assert_eq!(route(&map, &from, &to), None);
        // Seen working again in a later run.
        observe(tmp.path(), &obs(&prev, "tap", Some(&l), &next)).unwrap();
        let map = load(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert_eq!((map.edges.len(), map.edges[0].ok, map.edges[0].consecutive_fail, map.edges[0].routable), (1, 2, 0, true));
        assert_eq!(route(&map, &from, &to).unwrap().len(), 1);
        assert_eq!(summary(&map)["routable_edges"], 1);
    }

    #[test]
    fn another_fp_version_is_empty_for_matching_and_kept_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let read = list_screen(SETTINGS, "Settings", &["Apps"], 200);
        let fp = fingerprint_for(Platform::Android, &read);
        let mut old = AppMap::new(Platform::Android, SETTINGS);
        old.ensure_screen(&fp, "t");
        old.fp_version = 0;
        save(tmp.path(), &old).unwrap();
        let path = map_path(tmp.path(), Platform::Android, SETTINGS).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(load_for_matching(tmp.path(), Platform::Android, SETTINGS).screens.is_empty());
        assert_eq!(match_screen(&old, &fp), None);
        assert_eq!(route(&old, "s0", "s0"), None);
        // GET still shows the file as stored, and nothing rewrote it.
        assert_eq!(load(tmp.path(), Platform::Android, SETTINGS).unwrap().screens.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn store_is_private_atomic_and_listable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut m = AppMap::new(Platform::Android, SETTINGS);
        m.ensure_screen(&fingerprint_for(Platform::Android, &list_screen(SETTINGS, "Settings", &["Apps"], 200)), "2026-11-19T00:00:00Z");
        save(tmp.path(), &m).unwrap();
        let path = map_path(tmp.path(), Platform::Android, SETTINGS).unwrap();
        assert!(path.ends_with("phone-maps/android/com.android.settings.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
        let maps = list(tmp.path(), None);
        assert_eq!(maps.len(), 1);
        let s = summary(&maps[0]);
        assert_eq!((s["app"].as_str(), s["name"].as_str(), s["screens"].as_u64()), (Some(SETTINGS), Some(SETTINGS), Some(1)));
        assert!(list(tmp.path(), Some(Platform::Ios)).is_empty());
        assert!(forget(tmp.path(), Platform::Android, SETTINGS));
        assert!(!forget(tmp.path(), Platform::Android, SETTINGS), "idempotent");
        // Path tricks are refused; odd characters become `_`.
        assert!(file_stem("..").is_err() && file_stem("").is_err() && file_stem(".hidden").is_err());
        assert_eq!(file_stem("com.x/../y").unwrap(), "com.x_.._y");
    }

    #[test]
    fn screen_ids_are_never_reused_and_the_cap_holds() {
        let mut m = AppMap::new(Platform::Android, "com.x");
        for i in 0..(MAX_SCREENS + 5) {
            let fp = Fp { fingerprint: format!("v1:{i:016}"), app: "com.x".into(), activity: String::new(), keys: vec![], confidence: Confidence::Low, title: None };
            m.ensure_screen(&fp, &format!("2026-11-19T00:{:02}:{:02}Z", i / 60, i % 60));
            m.enforce_cap(&[]);
        }
        assert_eq!(m.screens.len(), MAX_SCREENS);
        assert!(!m.screens.contains_key("s0"), "the oldest went first");
        let (id, _) = m.ensure_screen(&Fp { fingerprint: "v1:fresh".into(), app: "com.x".into(), activity: String::new(), keys: vec![], confidence: Confidence::Low, title: None }, "z");
        assert_eq!(id, format!("s{}", MAX_SCREENS + 5));
    }
}
