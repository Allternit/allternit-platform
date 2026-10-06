//! `team.yaml`: who is on a team, how each bot runs, who reaches it, and how
//! the bots connect (SPEC §9, FOUR-CONCEPTS §3).
//!
//! File: `.allternit/teams/<team>/team.yaml`, with `CULTURE.md` beside it.
//!
//! ```yaml
//! name: product-build            # optional; must match the folder
//! bots:
//!   - { bot: al,       role: coordinator, binding: hosted }
//!   - { bot: builder,  role: build,       binding: terminal, harness: claude, harnesses: [claude, codex] }
//!   - { bot: research, role: research,    binding: vendor,   vendor: chatgpt, lane: official, directed_by: al }
//! reach:   { al: [telegram, push] }
//! edges:   [ { kind: checks, from: checker, to: builder } ]
//! presets: { cheap: { bots: { builder: { harness: codex } } } }
//! default_preset: cheap
//! ```
//!
//! Validation reports every problem at once, each with a path such as
//! `bots[2].harness`. A team file never creates bots; it points at bots by
//! slug and gives each an address `slug@team`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Folder under the Factory root that holds every team.
pub const TEAMS_DIR: &str = ".allternit/teams";
/// The team file name.
pub const TEAM_FILE: &str = "team.yaml";
/// The team's working rules, beside `team.yaml`.
pub const CULTURE_FILE: &str = "CULTURE.md";

/// Reach channels a bot may be given (SPEC §9 "Reach is separate from running").
pub const REACH_CHANNELS: &[&str] = &["telegram", "slack", "sms", "email", "push", "phone"];
/// Vendor lanes (API.md `Agent.binding.lane`).
pub const VENDOR_LANES: &[&str] = &["official", "channel", "ui_bridge", "local"];
/// Vendor modes (API.md `Agent.binding.mode`).
pub const VENDOR_MODES: &[&str] = &["hosted", "linked", "mirror"];
/// Edge kinds (API.md `Team.edges[].kind`).
pub const EDGE_KINDS: &[&str] = &["delegates_to", "checks", "directs"];

/// How a bot runs (SPEC §9 "Three ways a bot runs").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Binding {
    Hosted,
    Terminal,
    Vendor,
}

impl Binding {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hosted" => Some(Self::Hosted),
            "terminal" => Some(Self::Terminal),
            "vendor" => Some(Self::Vendor),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Terminal => "terminal",
            Self::Vendor => "vendor",
        }
    }
}

/// One `bots[]` row exactly as written (strings, so a bad value becomes a
/// validation error with a path instead of a parse failure).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotEntry {
    #[serde(default)]
    pub bot: String,
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Allow-list of harnesses this bot may run on (`harness` and any preset
    /// override must be in it when given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harnesses: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// FOUR-CONCEPTS draft field (`resume_if_possible`, …); carried, not acted on here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<String>,
}

/// An `edges[]` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeEntry {
    pub kind: String,
    pub from: String,
    pub to: String,
}

/// Per-bot overrides a preset may set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetBot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetEntry {
    #[serde(default)]
    pub bots: BTreeMap<String, PresetBot>,
}

/// `team.yaml` as written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Culture file name (default `CULTURE.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub culture: Option<String>,
    #[serde(default)]
    pub bots: Vec<BotEntry>,
    #[serde(default)]
    pub reach: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub edges: Vec<EdgeEntry>,
    #[serde(default)]
    pub presets: BTreeMap<String, PresetEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_preset: Option<String>,
}

/// One validation problem: `path` like `bots[2].harness`, `edges[0].to`,
/// `presets.cheap.bots.builder.harness`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Errors loading or using a team.
#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("team not found: {0}")]
    NotFound(String),
    #[error("invalid team name {0:?} (use a-z, 0-9, '-' and '_')")]
    BadName(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("team.yaml does not parse: {0}")]
    Parse(String),
    #[error("team.yaml is invalid ({} problem(s)): {}", .0.len(), join_errors(.0))]
    Invalid(Vec<ValidationError>),
    #[error("unknown preset {preset:?} (known: {known})")]
    UnknownPreset { preset: String, known: String },
    #[error("role {role:?} is held by more than one bot: {bots}")]
    RoleConflict { role: String, bots: String },
}

fn join_errors(errs: &[ValidationError]) -> String {
    errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")
}

/// A slug, team name or harness name: non-empty, at most 64 of `[a-z0-9_-]`.
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// `slug@team`.
pub fn address(slug: &str, team: &str) -> String {
    format!("{slug}@{team}")
}

/// A bot with the preset applied and the binding resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveBot {
    pub slug: String,
    pub address: String,
    pub role: String,
    pub binding: Binding,
    pub harness: Option<String>,
    pub vendor: Option<String>,
    pub lane: Option<String>,
    pub mode: Option<String>,
    pub directed_by: Option<String>,
    pub model: Option<String>,
    pub machine: Option<String>,
}

/// A loaded, validated team.
#[derive(Debug, Clone)]
pub struct LoadedTeam {
    pub name: String,
    /// `.allternit/teams/<team>/` (empty for a team parsed from a string).
    pub dir: PathBuf,
    pub file: TeamFile,
    /// `CULTURE.md` contents, if present.
    pub culture: Option<String>,
    /// The exact `team.yaml` bytes.
    pub raw: String,
    /// sha256 hex of `raw`.
    pub content_hash: String,
}

/// sha256 hex of some bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `<root>/.allternit/teams/<team>`.
pub fn team_dir(root: &Path, team: &str) -> PathBuf {
    root.join(TEAMS_DIR).join(team)
}

/// Parse YAML text into a [`TeamFile`] (no validation).
pub fn parse_team_file(text: &str) -> Result<TeamFile, TeamError> {
    if text.trim().is_empty() {
        return Ok(TeamFile::default());
    }
    serde_yaml::from_str(text).map_err(|e| TeamError::Parse(e.to_string()))
}

/// Parse and validate `team.yaml` text for the team `name` (the folder name).
pub fn parse_team(name: &str, text: &str) -> Result<LoadedTeam, TeamError> {
    if !valid_slug(name) {
        return Err(TeamError::BadName(name.to_string()));
    }
    let file = parse_team_file(text)?;
    let errs = validate(name, &file);
    if !errs.is_empty() {
        return Err(TeamError::Invalid(errs));
    }
    Ok(LoadedTeam {
        name: name.to_string(),
        dir: PathBuf::new(),
        file,
        culture: None,
        raw: text.to_string(),
        content_hash: sha256_hex(text.as_bytes()),
    })
}

/// Load `.allternit/teams/<team>/team.yaml` (+ `CULTURE.md`) and validate it.
pub fn load_team(root: &Path, team: &str) -> Result<LoadedTeam, TeamError> {
    if !valid_slug(team) {
        return Err(TeamError::BadName(team.to_string()));
    }
    let dir = team_dir(root, team);
    load_team_dir(&dir, team)
}

/// Load and validate a team from a folder whose name is `team`.
pub fn load_team_dir(dir: &Path, team: &str) -> Result<LoadedTeam, TeamError> {
    let path = dir.join(TEAM_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(TeamError::NotFound(team.to_string())),
        Err(source) => return Err(TeamError::Io { path, source }),
    };
    let mut loaded = parse_team(team, &raw)?;
    loaded.dir = dir.to_path_buf();
    let culture_name = loaded.file.culture.clone().unwrap_or_else(|| CULTURE_FILE.to_string());
    loaded.culture = std::fs::read_to_string(dir.join(culture_name)).ok();
    Ok(loaded)
}

/// Team folder names under `<root>/.allternit/teams` that hold a `team.yaml`,
/// sorted. Folders with an invalid name are skipped.
pub fn list_teams(root: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(root.join(TEAMS_DIR)) {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if valid_slug(&name) && ent.path().join(TEAM_FILE).is_file() {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

fn err(errs: &mut Vec<ValidationError>, path: impl Into<String>, message: impl Into<String>) {
    errs.push(ValidationError { path: path.into(), message: message.into() });
}

/// Per-bot checks shared by base rows and preset-applied rows.
fn check_bot(
    errs: &mut Vec<ValidationError>,
    path: &str,
    bot: &BotEntry,
    known: &BTreeSet<&str>,
    allow: Option<&Vec<String>>,
) {
    let binding = match bot.binding.as_deref() {
        None => {
            err(errs, format!("{path}.binding"), "missing binding (hosted | terminal | vendor)");
            return;
        }
        Some(b) => match Binding::parse(b) {
            Some(b) => b,
            None => {
                err(errs, format!("{path}.binding"), format!("unknown binding {b:?} (hosted | terminal | vendor)"));
                return;
            }
        },
    };
    if let Some(h) = bot.harness.as_deref() {
        if !valid_slug(h) {
            err(errs, format!("{path}.harness"), format!("invalid harness name {h:?}"));
        } else if let Some(allow) = allow {
            if !allow.iter().any(|a| a == h) {
                err(errs, format!("{path}.harness"), format!("harness {h:?} is not in harnesses [{}]", allow.join(", ")));
            }
        }
    }
    match binding {
        Binding::Terminal => {
            if bot.harness.as_deref().map(str::is_empty).unwrap_or(true) {
                err(errs, format!("{path}.harness"), "a terminal bot needs a harness");
            }
        }
        Binding::Vendor => {
            if bot.vendor.as_deref().map(str::is_empty).unwrap_or(true) {
                err(errs, format!("{path}.vendor"), "a vendor bot needs a vendor");
            }
            match bot.directed_by.as_deref() {
                None | Some("") => err(errs, format!("{path}.directed_by"), "a vendor bot needs directed_by (the bot that directs it)"),
                Some(d) if d == bot.bot => err(errs, format!("{path}.directed_by"), "a bot cannot direct itself"),
                _ => {}
            }
            if let Some(l) = bot.lane.as_deref() {
                if !VENDOR_LANES.contains(&l) {
                    err(errs, format!("{path}.lane"), format!("unknown lane {l:?} ({})", VENDOR_LANES.join(" | ")));
                }
            }
            if let Some(m) = bot.mode.as_deref() {
                if !VENDOR_MODES.contains(&m) {
                    err(errs, format!("{path}.mode"), format!("unknown mode {m:?} ({})", VENDOR_MODES.join(" | ")));
                }
            }
        }
        Binding::Hosted => {}
    }
    if let Some(d) = bot.directed_by.as_deref() {
        if !d.is_empty() && !known.contains(d) {
            err(errs, format!("{path}.directed_by"), format!("unknown bot {d:?}"));
        }
    }
}

fn apply_preset(base: &BotEntry, p: &PresetBot) -> BotEntry {
    let mut b = base.clone();
    let swap_binding = p.binding.is_some() && p.binding != base.binding;
    if let Some(v) = &p.binding {
        b.binding = Some(v.clone());
    }
    if swap_binding {
        // A binding swap starts the run fields fresh so a terminal harness
        // never leaks into a vendor row (or a vendor lane into a terminal row).
        b.harness = None;
        b.vendor = None;
        b.lane = None;
        b.mode = None;
        if base.binding.as_deref() == Some("vendor") {
            b.directed_by = None;
        }
    }
    macro_rules! over {
        ($f:ident) => {
            if let Some(v) = &p.$f {
                b.$f = Some(v.clone());
            }
        };
    }
    over!(harness);
    over!(vendor);
    over!(lane);
    over!(mode);
    over!(directed_by);
    over!(model);
    over!(machine);
    b
}

/// Validate a parsed team file for team `name`. Returns every problem found.
pub fn validate(name: &str, file: &TeamFile) -> Vec<ValidationError> {
    let mut errs = vec![];
    if !valid_slug(name) {
        err(&mut errs, "name", format!("invalid team name {name:?} (use a-z, 0-9, '-' and '_')"));
    }
    if let Some(n) = file.name.as_deref() {
        if n != name {
            err(&mut errs, "name", format!("name {n:?} does not match the team folder {name:?}"));
        }
    }
    if file.bots.is_empty() {
        err(&mut errs, "bots", "a team needs at least one bot");
    }
    let known: BTreeSet<&str> = file.bots.iter().map(|b| b.bot.as_str()).filter(|s| !s.is_empty()).collect();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut base_ok: BTreeMap<&str, bool> = BTreeMap::new();
    for (i, bot) in file.bots.iter().enumerate() {
        let path = format!("bots[{i}]");
        let before = errs.len();
        if bot.bot.is_empty() {
            err(&mut errs, format!("{path}.bot"), "missing bot slug");
        } else if !valid_slug(&bot.bot) {
            err(&mut errs, format!("{path}.bot"), format!("invalid bot slug {:?} (use a-z, 0-9, '-' and '_')", bot.bot));
        } else if let Some(first) = seen.get(bot.bot.as_str()) {
            err(&mut errs, format!("{path}.bot"), format!("duplicate bot {:?} (first at bots[{first}])", bot.bot));
        } else {
            seen.insert(bot.bot.as_str(), i);
        }
        if bot.role.trim().is_empty() {
            err(&mut errs, format!("{path}.role"), "missing role");
        }
        if let Some(allow) = &bot.harnesses {
            for (j, h) in allow.iter().enumerate() {
                if !valid_slug(h) {
                    err(&mut errs, format!("{path}.harnesses[{j}]"), format!("invalid harness name {h:?}"));
                }
            }
        }
        check_bot(&mut errs, &path, bot, &known, bot.harnesses.as_ref());
        base_ok.entry(bot.bot.as_str()).or_insert(errs.len() == before);
    }
    for (bot, chans) in &file.reach {
        if !known.contains(bot.as_str()) {
            err(&mut errs, format!("reach.{bot}"), format!("unknown bot {bot:?}"));
        }
        for (j, c) in chans.iter().enumerate() {
            if !REACH_CHANNELS.contains(&c.as_str()) {
                err(&mut errs, format!("reach.{bot}[{j}]"), format!("unknown channel {c:?} ({})", REACH_CHANNELS.join(" | ")));
            }
        }
    }
    for (i, e) in file.edges.iter().enumerate() {
        if !EDGE_KINDS.contains(&e.kind.as_str()) {
            err(&mut errs, format!("edges[{i}].kind"), format!("unknown edge kind {:?} ({})", e.kind, EDGE_KINDS.join(" | ")));
        }
        for (field, v) in [("from", &e.from), ("to", &e.to)] {
            if !known.contains(v.as_str()) {
                err(&mut errs, format!("edges[{i}].{field}"), format!("unknown bot {v:?}"));
            }
        }
    }
    for (pname, preset) in &file.presets {
        if !valid_slug(pname) {
            err(&mut errs, format!("presets.{pname}"), "invalid preset name (use a-z, 0-9, '-' and '_')");
        }
        for (slug, over) in &preset.bots {
            let path = format!("presets.{pname}.bots.{slug}");
            let Some(base) = file.bots.iter().find(|b| &b.bot == slug) else {
                err(&mut errs, path, format!("unknown bot {slug:?}"));
                continue;
            };
            if let Some(b) = over.binding.as_deref() {
                if Binding::parse(b).is_none() {
                    err(&mut errs, format!("{path}.binding"), format!("unknown binding {b:?} (hosted | terminal | vendor)"));
                    continue;
                }
            }
            // Only check the preset result when the base row is clean, so one
            // mistake isn't reported twice.
            if base_ok.get(slug.as_str()).copied().unwrap_or(false) {
                let eff = apply_preset(base, over);
                check_bot(&mut errs, &path, &eff, &known, base.harnesses.as_ref());
            }
        }
    }
    if let Some(d) = file.default_preset.as_deref() {
        if !file.presets.contains_key(d) {
            err(&mut errs, "default_preset", format!("unknown preset {d:?}"));
        }
    }
    errs
}

/// API.md `Team`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub name: String,
    pub preset: Option<String>,
    pub presets: Vec<String>,
    /// Bot addresses `slug@team`, in file order.
    pub agents: Vec<String>,
    pub edges: Vec<TeamEdge>,
}

/// API.md `Team.edges[]` (`from` / `to` are bot addresses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamEdge {
    pub kind: String,
    pub from: String,
    pub to: String,
}

impl LoadedTeam {
    /// The preset in effect: the one asked for, else `default_preset`, else none.
    pub fn resolve_preset(&self, preset: Option<&str>) -> Result<Option<String>, TeamError> {
        match preset.or(self.file.default_preset.as_deref()) {
            None => Ok(None),
            Some(p) if self.file.presets.contains_key(p) => Ok(Some(p.to_string())),
            Some(p) => Err(TeamError::UnknownPreset {
                preset: p.to_string(),
                known: self.file.presets.keys().cloned().collect::<Vec<_>>().join(", "),
            }),
        }
    }

    /// Every bot, in file order, with the preset applied.
    pub fn effective_bots(&self, preset: Option<&str>) -> Result<Vec<EffectiveBot>, TeamError> {
        let preset = self.resolve_preset(preset)?;
        let overrides = preset.as_deref().and_then(|p| self.file.presets.get(p));
        Ok(self
            .file
            .bots
            .iter()
            .map(|b| {
                let e = match overrides.and_then(|o| o.bots.get(&b.bot)) {
                    Some(o) => apply_preset(b, o),
                    None => b.clone(),
                };
                let binding = e.binding.as_deref().and_then(Binding::parse).unwrap_or(Binding::Hosted);
                EffectiveBot {
                    address: address(&e.bot, &self.name),
                    slug: e.bot,
                    role: e.role,
                    binding,
                    harness: if binding == Binding::Vendor { None } else { e.harness },
                    vendor: if binding == Binding::Vendor { e.vendor } else { None },
                    lane: if binding == Binding::Vendor { e.lane } else { None },
                    mode: if binding == Binding::Vendor { e.mode } else { None },
                    directed_by: e.directed_by,
                    model: e.model,
                    machine: e.machine,
                }
            })
            .collect())
    }

    /// `role -> "bot:<slug>"` for template `executor: role:<role>` resolution.
    /// Two bots holding one role is an error naming both.
    pub fn role_executors(&self, preset: Option<&str>) -> Result<BTreeMap<String, String>, TeamError> {
        let mut by_role: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in self.effective_bots(preset)? {
            by_role.entry(b.role).or_default().push(b.slug);
        }
        let mut out = BTreeMap::new();
        for (role, bots) in by_role {
            if bots.len() > 1 {
                return Err(TeamError::RoleConflict { role, bots: bots.join(", ") });
            }
            out.insert(role, format!("bot:{}", bots[0]));
        }
        Ok(out)
    }

    /// Channels people reach `slug` on (file order, deduped).
    pub fn reach(&self, slug: &str) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.file
            .reach
            .get(slug)
            .map(|v| v.iter().filter(|c| seen.insert(c.as_str())).cloned().collect())
            .unwrap_or_default()
    }

    /// API.md `Team` JSON for this team under `preset`.
    pub fn to_contract(&self, preset: Option<&str>) -> Result<Team, TeamError> {
        let preset = self.resolve_preset(preset)?;
        Ok(Team {
            name: self.name.clone(),
            preset,
            presets: self.file.presets.keys().cloned().collect(),
            agents: self.file.bots.iter().map(|b| address(&b.bot, &self.name)).collect(),
            edges: self
                .file
                .edges
                .iter()
                .map(|e| TeamEdge {
                    kind: e.kind.clone(),
                    from: address(&e.from, &self.name),
                    to: address(&e.to, &self.name),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const GOOD: &str = r#"
name: product-build
bots:
  - { bot: al,       role: coordinator, binding: hosted }
  - { bot: builder,  role: build,       binding: terminal, harness: claude, harnesses: [claude, codex] }
  - { bot: checker,  role: check,       binding: terminal, harness: codex }
  - { bot: research, role: research,    binding: vendor,   vendor: chatgpt, lane: official, directed_by: al }
reach:
  al: [telegram, push]
edges:
  - { kind: checks, from: checker, to: builder }
  - { kind: delegates_to, from: al, to: builder }
presets:
  cheap:
    bots:
      builder: { harness: codex }
  vendor-build:
    bots:
      builder: { binding: vendor, vendor: chatgpt, lane: official, directed_by: al }
"#;

    fn errs_of(text: &str) -> Vec<ValidationError> {
        match parse_team("product-build", text) {
            Err(TeamError::Invalid(e)) => e,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    fn has(errs: &[ValidationError], path: &str, needle: &str) -> bool {
        errs.iter().any(|e| e.path == path && e.message.contains(needle))
    }

    #[test]
    fn good_team_parses() {
        let t = parse_team("product-build", GOOD).unwrap();
        assert_eq!(t.file.bots.len(), 4);
        let c = t.to_contract(None).unwrap();
        assert_eq!(c.agents[1], "builder@product-build");
        assert_eq!(c.presets, vec!["cheap", "vendor-build"]);
        assert_eq!(c.edges[0].from, "checker@product-build");
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("presets").is_some() && json.get("preset").is_some());
        let roles = t.role_executors(None).unwrap();
        assert_eq!(roles["build"], "bot:builder");
        assert_eq!(t.reach("al"), vec!["telegram", "push"]);
    }

    #[test]
    fn each_error_case_has_a_path() {
        let cases: &[(&str, &str, &str)] = &[
            ("bots:\n  - { bot: a, role: r, binding: cloud }\n", "bots[0].binding", "unknown binding"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\n  - { bot: a, role: s, binding: hosted }\n", "bots[1].bot", "duplicate bot"),
            ("bots:\n  - { bot: a, role: r, binding: terminal }\n", "bots[0].harness", "needs a harness"),
            ("bots:\n  - { bot: a, role: r, binding: terminal, harness: kimi, harnesses: [claude] }\n", "bots[0].harness", "not in harnesses"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\n  - { bot: v, role: s, binding: vendor, directed_by: a }\n", "bots[1].vendor", "needs a vendor"),
            ("bots:\n  - { bot: v, role: s, binding: vendor, vendor: chatgpt }\n", "bots[0].directed_by", "needs directed_by"),
            ("bots:\n  - { bot: v, role: s, binding: vendor, vendor: chatgpt, directed_by: ghost }\n", "bots[0].directed_by", "unknown bot"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\nedges:\n  - { kind: checks, from: a, to: ghost }\n", "edges[0].to", "unknown bot"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\nedges:\n  - { kind: likes, from: a, to: a }\n", "edges[0].kind", "unknown edge kind"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\nreach:\n  ghost: [sms]\n", "reach.ghost", "unknown bot"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\nreach:\n  a: [fax]\n", "reach.a[0]", "unknown channel"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\npresets:\n  p:\n    bots:\n      ghost: { harness: claude }\n", "presets.p.bots.ghost", "unknown bot"),
            ("bots:\n  - { bot: Bad.Slug, role: r, binding: hosted }\n", "bots[0].bot", "invalid bot slug"),
            ("name: other\nbots:\n  - { bot: a, role: r, binding: hosted }\n", "name", "does not match"),
            ("bots:\n  - { bot: a, role: r, binding: hosted }\ndefault_preset: nope\n", "default_preset", "unknown preset"),
            ("bots:\n  - { bot: a, role: r, binding: terminal, harness: claude, harnesses: [claude] }\npresets:\n  p:\n    bots:\n      a: { harness: kimi }\n", "presets.p.bots.a.harness", "not in harnesses"),
            ("bots:\n  - { bot: a, role: r, binding: terminal, harness: claude }\npresets:\n  p:\n    bots:\n      a: { binding: vendor, vendor: chatgpt }\n", "presets.p.bots.a.directed_by", "needs directed_by"),
            ("bots:\n  - { bot: a, binding: hosted }\n", "bots[0].role", "missing role"),
        ];
        for (text, path, needle) in cases {
            let errs = errs_of(text);
            assert!(has(&errs, path, needle), "case {path}/{needle}: got {errs:?}");
        }
        assert!(matches!(parse_team("Bad Team", "bots: []"), Err(TeamError::BadName(_))));
    }

    #[test]
    fn all_errors_reported_at_once() {
        let text = "bots:\n  - { bot: a, role: r, binding: cloud }\n  - { bot: b, role: r, binding: terminal }\n  - { bot: b, role: s, binding: vendor }\nreach:\n  a: [fax]\nedges:\n  - { kind: checks, from: a, to: zz }\n";
        let errs = errs_of(text);
        for (p, n) in [
            ("bots[0].binding", "unknown binding"),
            ("bots[1].harness", "needs a harness"),
            ("bots[2].bot", "duplicate bot"),
            ("bots[2].vendor", "needs a vendor"),
            ("bots[2].directed_by", "needs directed_by"),
            ("reach.a[0]", "unknown channel"),
            ("edges[0].to", "unknown bot"),
        ] {
            assert!(has(&errs, p, n), "missing {p}: {errs:?}");
        }
        assert!(errs.len() >= 7);
    }

    #[test]
    fn role_conflict_is_an_error() {
        let t = parse_team("t", "bots:\n  - { bot: a, role: build, binding: hosted }\n  - { bot: b, role: build, binding: hosted }\n").unwrap();
        match t.role_executors(None) {
            Err(TeamError::RoleConflict { role, bots }) => {
                assert_eq!(role, "build");
                assert_eq!(bots, "a, b");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn preset_swaps_binding() {
        let t = parse_team("product-build", GOOD).unwrap();
        let b = t.effective_bots(Some("vendor-build")).unwrap();
        assert_eq!(b[1].binding, Binding::Vendor);
        assert_eq!(b[1].harness, None);
        assert_eq!(b[1].vendor.as_deref(), Some("chatgpt"));
        assert!(matches!(t.effective_bots(Some("nope")), Err(TeamError::UnknownPreset { .. })));
    }

    #[test]
    fn list_and_load_from_disk() {
        let root = tempfile::tempdir().unwrap();
        let dir = team_dir(root.path(), "product-build");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TEAM_FILE), GOOD).unwrap();
        std::fs::write(dir.join(CULTURE_FILE), "# Rules\n").unwrap();
        std::fs::create_dir_all(team_dir(root.path(), "empty")).unwrap();
        assert_eq!(list_teams(root.path()), vec!["product-build"]);
        let t = load_team(root.path(), "product-build").unwrap();
        assert_eq!(t.culture.as_deref(), Some("# Rules\n"));
        assert_eq!(t.content_hash, sha256_hex(GOOD.as_bytes()));
        assert!(matches!(load_team(root.path(), "missing"), Err(TeamError::NotFound(_))));
    }
}
