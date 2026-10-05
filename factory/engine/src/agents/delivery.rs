//! Full-bot delivery into a Terminal bot's pane (SPEC §9 "A Terminal bot is a
//! full Bot"). Run at pane start and again on re-entry (after compaction or a
//! restart); every run converges the same files to the same content.
//!
//! Per harness (`claude`, `codex`, `gizzi`, `kimi`, `grok`, `agy`; anything
//! else gets every field `unavailable` and no file is touched):
//!
//! | field | claude | codex | gizzi | kimi / grok | agy |
//! |---|---|---|---|---|---|
//! | persona | `CLAUDE.md` block → guidance | `AGENTS.md` → guidance | `GIZZI.md` → guidance | `AGENTS.md` → guidance | `AGENTS.md` → guidance |
//! | memory | context pack + `LEARNED.md` → delivered (partial without a WIH pack path) | same | same | same | same |
//! | skills | `.claude/skills/<name>` | `.codex/skills/<name>` | `.gizzi/skills/<name>` | unavailable (no project skills folder) | unavailable |
//! | tools | `.mcp.json` `mcpServers.allternit` | `.codex/config.toml` `[mcp_servers.allternit]` | `.gizzi/gizzi.json` `mcp.allternit` | unavailable (no project MCP config) | unavailable |
//! | permissions | `.claude/settings.json` PreToolUse Gate hook → delivered | `-c hooks.PreToolUse=…` argv → delivered | partial (policy in block) | partial | partial |
//! | model | `--model X` | `--model X` | `--model X` | `--model X` | unavailable |
//!
//! Secrets: no credential value is ever written into a pane file. MCP auth is
//! an env var NAME (`${NAME}` for claude, `bearer_token_env_var` for codex,
//! `{env:NAME}` for gizzi). As a backstop every write is checked against the
//! profile's `secret_values`; a file that would contain one is not written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::gate::hook::{codex_hook_override, hook_command, HookFlavor, HookTarget};

/// Start / end markers of the managed block in the harness instruction file.
pub const BLOCK_BEGIN: &str = "<!-- allternit:bot BEGIN -->";
pub const BLOCK_END: &str = "<!-- allternit:bot END -->";
/// MCP server key the Allternit connector is registered under.
pub const MCP_SERVER_NAME: &str = "allternit";
/// Per-bot folder in the pane workdir: `.allternit/bot/<slug>/`.
pub const BOT_DIR: &str = ".allternit/bot";
/// Delivery record file name inside the per-bot folder.
pub const DELIVERY_RECORD: &str = "delivery.json";
const LEARNED_BASELINE: &str = ".learned.baseline";

/// API.md `Agent.fields` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    Persona,
    Memory,
    Skills,
    Tools,
    Model,
    Permissions,
}

pub const FIELDS: [Field; 6] = [Field::Persona, Field::Memory, Field::Skills, Field::Tools, Field::Model, Field::Permissions];

/// API.md `Agent.fields` values: what actually happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    Delivered,
    Guidance,
    Partial,
    Unavailable,
}

/// V233 autonomy policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Autonomy {
    Draft,
    #[default]
    Ask,
    Tell,
    Limits,
}

impl Autonomy {
    fn policy_text(self) -> &'static str {
        match self {
            Autonomy::Draft => "draft: prepare changes and outputs as drafts; a person approves before anything is applied, sent or merged.",
            Autonomy::Ask => "ask: ask the owner before any consequential action (writes outside your node, deploys, money, client messages).",
            Autonomy::Tell => "tell: act within your node, and tell the owner what you did.",
            Autonomy::Limits => "limits: act freely inside the limits on your WIH and leases; stop and ask at any limit.",
        }
    }
}

/// Everything a bot carries into its pane.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotProfile {
    pub id: String,
    pub slug: String,
    pub team: Option<String>,
    pub role: Option<String>,
    pub persona: String,
    pub role_instructions: Option<String>,
    /// Active twin persona (V234), if the owner has one on.
    pub twin_persona: Option<String>,
    /// Bot memory + active twin memory, one item per entry.
    pub memory_items: Vec<String>,
    /// Current `LEARNED.md` text the next session inherits.
    pub learned: Option<String>,
    /// Skill folders (each holding `SKILL.md`); copied by folder name.
    pub skills_dirs: Vec<PathBuf>,
    pub model: Option<String>,
    /// Allternit MCP connector URL (`https://mcp.allternit.com/mcp/bots/<id>` or local).
    pub mcp_url: Option<String>,
    /// NAME of the env var holding the connector's bearer token, if it needs
    /// one. Only the name is ever written.
    pub mcp_auth_env: Option<String>,
    pub autonomy: Autonomy,
    /// Secret values the pane process may receive through its environment.
    /// Never written; any file that would contain one is refused.
    #[serde(skip)]
    pub secret_values: Vec<String>,
}

/// Where the Gate hook points (see [`HookTarget`]). Without one, permissions
/// fall back to `partial` (policy in the managed block only).
#[derive(Debug, Clone)]
pub struct GateTarget {
    pub bin: PathBuf,
    pub root: PathBuf,
    pub wih_id: Option<String>,
}

/// One delivery run.
#[derive(Debug, Clone)]
pub struct DeliveryRequest<'a> {
    pub profile: &'a BotProfile,
    pub harness: &'a str,
    pub workdir: &'a Path,
    /// WIH `context_pack_path`. `None` → `<workdir>/.allternit/bot/<slug>/context.md`, memory `partial`.
    pub context_pack_path: Option<PathBuf>,
    pub gate: Option<GateTarget>,
}

/// What a delivery run did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReport {
    pub fields: BTreeMap<Field, Label>,
    pub files_written: Vec<PathBuf>,
    /// Extra harness argv (model flag, codex hook override).
    pub argv: Vec<String>,
    pub notes: Vec<String>,
}

/// Persisted at `<workdir>/.allternit/bot/<slug>/delivery.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryRecord {
    pub bot_id: String,
    pub slug: String,
    pub harness: String,
    pub delivered_at: String,
    #[serde(flatten)]
    pub report: DeliveryReport,
}

/// Harnesses delivery knows. Anything else is `unavailable` across the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claude,
    Codex,
    Gizzi,
    Kimi,
    Grok,
    Agy,
}

fn kind_of(harness: &str) -> Option<Kind> {
    match harness.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Some(Kind::Claude),
        "codex" => Some(Kind::Codex),
        "gizzi" | "gizzi-code" => Some(Kind::Gizzi),
        "kimi" | "kimi-code" => Some(Kind::Kimi),
        "grok" => Some(Kind::Grok),
        "agy" => Some(Kind::Agy),
        _ => None,
    }
}

/// Instruction file the harness reads at its workdir root.
pub fn instruction_file(harness: &str) -> Option<&'static str> {
    Some(match kind_of(harness)? {
        Kind::Claude => "CLAUDE.md",
        Kind::Gizzi => "GIZZI.md",
        Kind::Codex | Kind::Kimi | Kind::Grok | Kind::Agy => "AGENTS.md",
    })
}

/// Project skills folder the harness loads, relative to the workdir.
pub fn skills_dir(harness: &str) -> Option<&'static str> {
    match kind_of(harness)? {
        Kind::Claude => Some(".claude/skills"),
        Kind::Codex => Some(".codex/skills"),
        Kind::Gizzi => Some(".gizzi/skills"),
        Kind::Kimi | Kind::Grok | Kind::Agy => None,
    }
}

/// The harness's model flag, if known.
pub fn model_argv(harness: &str, model: &str) -> Option<Vec<String>> {
    match kind_of(harness)? {
        Kind::Claude | Kind::Codex | Kind::Gizzi | Kind::Kimi | Kind::Grok => Some(vec!["--model".into(), model.into()]),
        Kind::Agy => None,
    }
}

/// `<workdir>/.allternit/bot/<slug>`.
pub fn bot_dir(workdir: &Path, slug: &str) -> PathBuf {
    workdir.join(BOT_DIR).join(slug)
}

/// Replace (or append) the managed block in `existing`, keeping every byte
/// outside it.
pub fn upsert_managed_block(existing: &str, body: &str) -> String {
    let block = format!("{BLOCK_BEGIN}\n{}\n{BLOCK_END}", body.trim_end());
    if let Some(start) = existing.find(BLOCK_BEGIN) {
        if let Some(end_rel) = existing[start..].find(BLOCK_END) {
            let end = start + end_rel + BLOCK_END.len();
            return format!("{}{}{}", &existing[..start], block, &existing[end..]);
        }
    }
    if existing.is_empty() {
        format!("{block}\n")
    } else if existing.ends_with("\n\n") {
        format!("{existing}{block}\n")
    } else if existing.ends_with('\n') {
        format!("{existing}\n{block}\n")
    } else {
        format!("{existing}\n\n{block}\n")
    }
}

struct Writer<'a> {
    secrets: Vec<&'a str>,
    written: Vec<PathBuf>,
    notes: Vec<String>,
}

impl<'a> Writer<'a> {
    fn new(profile: &'a BotProfile) -> Self {
        let mut secrets: Vec<&str> = profile.secret_values.iter().map(|s| s.as_str()).filter(|s| s.len() >= 6).collect();
        secrets.sort();
        secrets.dedup();
        Writer { secrets, written: vec![], notes: vec![] }
    }

    /// Write `content` to `path` unless it carries a secret. Returns whether written.
    fn write(&mut self, path: &Path, content: &[u8]) -> Result<bool> {
        if let Some(_s) = self.secrets.iter().find(|s| contains(content, s.as_bytes())) {
            self.notes.push(format!("refused to write {}: it would contain a secret value", path.display()));
            return Ok(false);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        if std::fs::read(path).ok().as_deref() != Some(content) {
            std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
        }
        self.written.push(path.to_path_buf());
        Ok(true)
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn managed_body(p: &BotProfile, harness: &str, pack: &Path, learned: &Path, permissions_note: Option<&str>) -> String {
    let mut s = String::new();
    let who = match &p.team {
        Some(t) => format!("{}@{t}", p.slug),
        None => p.slug.clone(),
    };
    s.push_str(&format!("# Allternit bot: {who}\n\n"));
    s.push_str("Managed by Allternit Factory. Rewritten at pane start and on re-entry; edits inside this block are replaced.\n\n");
    if let Some(r) = &p.role {
        s.push_str(&format!("Role: {r}\n\n"));
    }
    if !p.persona.trim().is_empty() {
        s.push_str(&format!("## Persona\n\n{}\n\n", p.persona.trim()));
    }
    if let Some(r) = p.role_instructions.as_deref().filter(|r| !r.trim().is_empty()) {
        s.push_str(&format!("## Role instructions\n\n{}\n\n", r.trim()));
    }
    if let Some(t) = p.twin_persona.as_deref().filter(|t| !t.trim().is_empty()) {
        s.push_str(&format!("## Twin persona\n\n{}\n\n", t.trim()));
    }
    s.push_str("## Memory\n\n");
    s.push_str(&format!("- Read your context pack at `{}` before starting work.\n", pack.display()));
    s.push_str(&format!(
        "- Add anything the next session of you should know as new lines in `{}`. New lines go to your owner as proposed memory; only the owner's accept makes them active.\n\n",
        learned.display()
    ));
    s.push_str("## Autonomy\n\n");
    s.push_str(&format!("Policy {}\n", p.autonomy.policy_text()));
    s.push_str("Credentials are never in this workspace: reach them only through the Allternit connector's tools, under a lease.\n");
    if let Some(n) = permissions_note {
        s.push_str(&format!("{n}\n"));
    }
    s.push_str(&format!("\n(harness: {harness})\n"));
    s
}

fn context_pack(p: &BotProfile) -> String {
    let mut s = format!("# Context pack: {}\n\n", p.slug);
    s.push_str("## Memory\n\n");
    if p.memory_items.is_empty() {
        s.push_str("(none)\n");
    }
    for m in &p.memory_items {
        s.push_str(&format!("- {}\n", m.trim().replace('\n', " ")));
    }
    s
}

fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(t) if t.trim().is_empty() => Ok(json!({})),
        Ok(t) => serde_json::from_str(&t).map_err(|e| anyhow!("{} is not valid JSON ({e}); left untouched", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e.into()),
    }
}

fn obj<'v>(v: &'v mut Value, key: &str) -> Result<&'v mut serde_json::Map<String, Value>> {
    let root = v.as_object_mut().ok_or_else(|| anyhow!("top level is not an object"))?;
    let e = root.entry(key.to_string()).or_insert_with(|| json!({}));
    if !e.is_object() {
        return Err(anyhow!("{key} is not an object"));
    }
    Ok(e.as_object_mut().unwrap())
}

fn copy_dir(src: &Path, dst: &Path, w: &mut Writer<'_>) -> Result<()> {
    for ent in std::fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let ent = ent?;
        let ft = ent.file_type()?;
        let to = dst.join(ent.file_name());
        if ft.is_dir() {
            copy_dir(&ent.path(), &to, w)?;
        } else if ft.is_file() {
            let bytes = std::fs::read(ent.path())?;
            w.write(&to, &bytes)?;
        }
        // Symlinks are skipped: a skill folder must not reach outside itself.
    }
    Ok(())
}

fn deliver_skills(req: &DeliveryRequest<'_>, rel: &str, w: &mut Writer<'_>) -> Label {
    if req.profile.skills_dirs.is_empty() {
        w.notes.push("skills: the bot has no skills".into());
        return Label::Delivered;
    }
    let mut ok = 0usize;
    for src in &req.profile.skills_dirs {
        let Some(name) = src.file_name() else { continue };
        let dst = req.workdir.join(rel).join(name);
        match copy_dir(src, &dst, w) {
            Ok(()) => ok += 1,
            Err(e) => w.notes.push(format!("skills: {} not copied: {e:#}", src.display())),
        }
    }
    if ok == req.profile.skills_dirs.len() {
        Label::Delivered
    } else if ok > 0 {
        Label::Partial
    } else {
        Label::Unavailable
    }
}

fn deliver_tools(req: &DeliveryRequest<'_>, kind: Kind, w: &mut Writer<'_>) -> Result<Label> {
    let p = req.profile;
    let Some(url) = p.mcp_url.as_deref() else {
        w.notes.push("tools: no connector URL for this bot".into());
        return Ok(Label::Unavailable);
    };
    if url_carries_credentials(url) {
        w.notes.push("tools: connector URL carries credentials; not written (use mcp_auth_env)".into());
        return Ok(Label::Unavailable);
    }
    if let Some(n) = &p.mcp_auth_env {
        if !n.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') || n.is_empty() {
            w.notes.push(format!("tools: {n:?} is not an env var name; not written"));
            return Ok(Label::Unavailable);
        }
    }
    let wrote = match kind {
        Kind::Claude => {
            let path = req.workdir.join(".mcp.json");
            let mut v = read_json(&path)?;
            let mut server = json!({ "type": "http", "url": url });
            if let Some(n) = &p.mcp_auth_env {
                server["headers"] = json!({ "Authorization": format!("Bearer ${{{n}}}") });
            }
            obj(&mut v, "mcpServers")?.insert(MCP_SERVER_NAME.into(), server);
            w.write(&path, format!("{}\n", serde_json::to_string_pretty(&v)?).as_bytes())?
        }
        Kind::Gizzi => {
            let path = req.workdir.join(".gizzi/gizzi.json");
            let mut v = read_json(&path)?;
            let mut server = json!({ "type": "remote", "url": url, "enabled": true });
            if let Some(n) = &p.mcp_auth_env {
                server["headers"] = json!({ "Authorization": format!("Bearer {{env:{n}}}") });
            }
            obj(&mut v, "mcp")?.insert(MCP_SERVER_NAME.into(), server);
            w.write(&path, format!("{}\n", serde_json::to_string_pretty(&v)?).as_bytes())?
        }
        Kind::Codex => {
            let path = req.workdir.join(".codex/config.toml");
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let mut doc: toml_edit::DocumentMut = text
                .parse()
                .map_err(|e| anyhow!("{} is not valid TOML ({e}); left untouched", path.display()))?;
            let servers = doc
                .entry("mcp_servers")
                .or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                })
                .as_table_mut()
                .ok_or_else(|| anyhow!("mcp_servers is not a table"))?;
            let mut t = toml_edit::Table::new();
            t["url"] = toml_edit::value(url);
            if let Some(n) = &p.mcp_auth_env {
                t["bearer_token_env_var"] = toml_edit::value(n.as_str());
            }
            servers.insert(MCP_SERVER_NAME, toml_edit::Item::Table(t));
            w.notes.push("tools: codex reads a project .codex/config.toml only for a trusted project".into());
            w.write(&path, doc.to_string().as_bytes())?
        }
        Kind::Kimi | Kind::Grok | Kind::Agy => {
            w.notes.push("tools: this harness has no project MCP config Allternit can write".into());
            return Ok(Label::Unavailable);
        }
    };
    Ok(if wrote { Label::Delivered } else { Label::Unavailable })
}

/// True when a URL has userinfo or a query parameter that looks like a credential.
fn url_carries_credentials(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|x| x.1).unwrap_or(url);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return true;
    }
    let lower = url.to_ascii_lowercase();
    lower
        .split_once('?')
        .map(|(_, q)| {
            q.split('&').any(|kv| {
                let k = kv.split('=').next().unwrap_or("");
                ["token", "key", "secret", "password", "auth", "apikey", "api_key", "access_token"].iter().any(|n| k.contains(n))
            })
        })
        .unwrap_or(false)
}

fn deliver_claude_hook(req: &DeliveryRequest<'_>, gate: &GateTarget, w: &mut Writer<'_>) -> Result<Label> {
    let target = HookTarget {
        commrails_bin: &gate.bin,
        root: &gate.root,
        workspace: Some(req.workdir),
        wih_id: gate.wih_id.as_deref(),
    };
    let cmd = hook_command(HookFlavor::Claude, target);
    let path = req.workdir.join(".claude/settings.json");
    let mut v = read_json(&path)?;
    let hooks = obj(&mut v, "hooks")?;
    let list = hooks.entry("PreToolUse".to_string()).or_insert_with(|| json!([]));
    let arr = list.as_array_mut().ok_or_else(|| anyhow!("hooks.PreToolUse is not a list"))?;
    // Drop an earlier Allternit gate entry (re-entry, moved binary, new WIH),
    // keep every other hook.
    let marker = format!("hook {}", HookFlavor::Claude.subcommand());
    arr.retain(|e| {
        !e.get("hooks")
            .and_then(|h| h.as_array())
            .map(|hs| hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).map(|c| c.contains(&marker)).unwrap_or(false)))
            .unwrap_or(false)
    });
    arr.push(json!({
        "matcher": "*",
        "hooks": [{ "type": "command", "command": cmd, "timeout": 30 }]
    }));
    let wrote = w.write(&path, format!("{}\n", serde_json::to_string_pretty(&v)?).as_bytes())?;
    Ok(if wrote { Label::Delivered } else { Label::Partial })
}

/// Deliver the bot into its pane. Never fails part-way silently: a field that
/// could not be delivered is labeled and explained in `notes`.
pub fn deliver(req: &DeliveryRequest<'_>) -> Result<DeliveryReport> {
    let p = req.profile;
    if !super::team::valid_slug(&p.slug) {
        return Err(anyhow!("invalid bot slug {:?}", p.slug));
    }
    let mut fields: BTreeMap<Field, Label> = FIELDS.iter().map(|f| (*f, Label::Unavailable)).collect();
    let Some(kind) = kind_of(req.harness) else {
        let report = DeliveryReport {
            fields,
            files_written: vec![],
            argv: vec![],
            notes: vec![format!("harness {:?} is not one Allternit can deliver into", req.harness)],
        };
        return Ok(report);
    };
    let mut w = Writer::new(p);
    let mut argv = vec![];
    let dir = bot_dir(req.workdir, &p.slug);

    // permissions (decided first: its fallback text goes in the managed block)
    let mut permissions_note = None;
    let permissions = match (kind, &req.gate) {
        (Kind::Claude, Some(g)) => match deliver_claude_hook(req, g, &mut w) {
            Ok(l) => l,
            Err(e) => {
                w.notes.push(format!("permissions: {e:#}"));
                Label::Partial
            }
        },
        (Kind::Codex, Some(g)) => {
            let target = HookTarget { commrails_bin: &g.bin, root: &g.root, workspace: Some(req.workdir), wih_id: g.wih_id.as_deref() };
            argv.push("-c".to_string());
            argv.push(codex_hook_override(target));
            Label::Delivered
        }
        _ => {
            permissions_note = Some(if req.gate.is_none() {
                "No Gate hook is installed in this pane: follow the policy above for every tool call."
            } else {
                "This harness has no PreToolUse hook: follow the policy above for every tool call."
            });
            Label::Partial
        }
    };
    if permissions == Label::Partial && permissions_note.is_none() {
        permissions_note = Some("The Gate hook could not be installed: follow the policy above for every tool call.");
    }

    // memory
    let (pack_path, pack_from_wih) = match &req.context_pack_path {
        Some(p) => (p.clone(), true),
        None => (dir.join("context.md"), false),
    };
    let learned_path = dir.join("LEARNED.md");
    let learned_text = p.learned.clone().unwrap_or_else(|| format!("# LEARNED: {}\n", p.slug));
    let pack_ok = w.write(&pack_path, context_pack(p).as_bytes())?;
    // LEARNED.md: the bot owns it between runs, so only seed it when absent
    // or when the owner supplied accepted text.
    let learned_ok = if p.learned.is_some() || !learned_path.exists() {
        w.write(&learned_path, learned_text.as_bytes())?
    } else {
        true
    };
    if learned_ok {
        let current = std::fs::read_to_string(&learned_path).unwrap_or_default();
        w.write(&dir.join(LEARNED_BASELINE), current.as_bytes())?;
    }
    fields.insert(
        Field::Memory,
        match (pack_ok && learned_ok, pack_from_wih) {
            (true, true) => Label::Delivered,
            (true, false) => {
                w.notes.push("memory: no WIH context pack path; wrote the pack beside the bot".into());
                Label::Partial
            }
            (false, _) => Label::Unavailable,
        },
    );

    // persona (managed block)
    let file = req.workdir.join(instruction_file(req.harness).unwrap());
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let body = managed_body(p, req.harness, &pack_path, &learned_path, permissions_note);
    let persona_ok = w.write(&file, upsert_managed_block(&existing, &body).as_bytes())?;
    fields.insert(Field::Persona, if persona_ok { Label::Guidance } else { Label::Unavailable });
    if !persona_ok && permissions == Label::Partial {
        fields.insert(Field::Permissions, Label::Unavailable);
    } else {
        fields.insert(Field::Permissions, permissions);
    }

    // skills
    let skills = match skills_dir(req.harness) {
        Some(rel) => deliver_skills(req, rel, &mut w),
        None => {
            w.notes.push("skills: this harness has no project skills folder".into());
            Label::Unavailable
        }
    };
    fields.insert(Field::Skills, skills);

    // tools
    let tools = deliver_tools(req, kind, &mut w).unwrap_or_else(|e| {
        w.notes.push(format!("tools: {e:#}"));
        Label::Unavailable
    });
    fields.insert(Field::Tools, tools);

    // model
    let model = match p.model.as_deref().filter(|m| !m.trim().is_empty()) {
        None => {
            w.notes.push("model: none set; the harness default applies".into());
            Label::Unavailable
        }
        Some(m) => match model_argv(req.harness, m) {
            Some(mut a) => {
                a.append(&mut argv);
                argv = a;
                Label::Delivered
            }
            None => {
                w.notes.push("model: this harness has no known model flag".into());
                Label::Unavailable
            }
        },
    };
    fields.insert(Field::Model, model);

    let mut report = DeliveryReport { fields, files_written: vec![], argv, notes: std::mem::take(&mut w.notes) };
    let record = DeliveryRecord {
        bot_id: p.id.clone(),
        slug: p.slug.clone(),
        harness: req.harness.to_string(),
        delivered_at: chrono::Utc::now().to_rfc3339(),
        report: DeliveryReport { files_written: w.written.clone(), ..report.clone() },
    };
    let record_path = dir.join(DELIVERY_RECORD);
    w.write(&record_path, format!("{}\n", serde_json::to_string_pretty(&record)?).as_bytes())?;
    report.notes.extend(std::mem::take(&mut w.notes));
    report.files_written = w.written;
    Ok(report)
}

/// The persisted delivery record for `slug` in `workdir`, if any.
pub fn load_record(workdir: &Path, slug: &str) -> Option<DeliveryRecord> {
    let text = std::fs::read_to_string(bot_dir(workdir, slug).join(DELIVERY_RECORD)).ok()?;
    serde_json::from_str(&text).ok()
}

/// `Agent.fields` for `slug` from its last delivery in `workdir`.
pub fn load_fields(workdir: &Path, slug: &str) -> Option<BTreeMap<Field, Label>> {
    load_record(workdir, slug).map(|r| r.report.fields)
}

/// Lines the bot added to `LEARNED.md` since the last delivery, in order,
/// trimmed, deduped. The caller posts them as `proposed` memory. Call it
/// before re-delivering on re-entry: [`deliver`] resets the baseline.
pub fn collect_learned(workdir: &Path, slug: &str) -> Vec<String> {
    let dir = bot_dir(workdir, slug);
    let current = std::fs::read_to_string(dir.join("LEARNED.md")).unwrap_or_default();
    let base = std::fs::read_to_string(dir.join(LEARNED_BASELINE)).unwrap_or_default();
    let known: std::collections::BTreeSet<&str> = base.lines().map(str::trim).collect();
    let mut seen = std::collections::BTreeSet::new();
    current
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !known.contains(l) && seen.insert(*l))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "sk-live-SECRET-0123456789abcdef";

    fn skill(root: &Path) -> PathBuf {
        let d = root.join("skills-src/review");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "---\nname: review\n---\nReview carefully.\n").unwrap();
        d
    }

    fn profile(root: &Path) -> BotProfile {
        BotProfile {
            id: "bot_123".into(),
            slug: "builder".into(),
            team: Some("product-build".into()),
            role: Some("build".into()),
            persona: "You are Builder.".into(),
            role_instructions: Some("Build the node, then hand to checker.".into()),
            twin_persona: Some("Speak like Eoj.".into()),
            memory_items: vec!["Repo uses pnpm".into()],
            learned: None,
            skills_dirs: vec![skill(root)],
            model: Some("opus".into()),
            mcp_url: Some("https://mcp.allternit.com/mcp/bots/bot_123".into()),
            mcp_auth_env: Some("ALLTERNIT_MCP_TOKEN".into()),
            autonomy: Autonomy::Ask,
            secret_values: vec![TOKEN.into()],
        }
    }

    fn gate(root: &Path) -> GateTarget {
        GateTarget { bin: PathBuf::from("/opt/allternit/bin/allternit-factory"), root: root.to_path_buf(), wih_id: Some("wih_1".into()) }
    }

    #[test]
    fn claude_gets_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(wd.join(".claude")).unwrap();
        std::fs::write(wd.join(".claude/settings.json"), r#"{"theme":"dark","hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo keep"}]}]}}"#).unwrap();
        std::fs::write(wd.join(".mcp.json"), r#"{"mcpServers":{"other":{"type":"stdio","command":"x"}}}"#).unwrap();
        let p = profile(tmp.path());
        let req = DeliveryRequest { profile: &p, harness: "claude", workdir: &wd, context_pack_path: Some(wd.join("ctx/pack.md")), gate: Some(gate(tmp.path())) };
        let r = deliver(&req).unwrap();
        assert_eq!(r.fields[&Field::Persona], Label::Guidance);
        for f in [Field::Memory, Field::Skills, Field::Tools, Field::Model, Field::Permissions] {
            assert_eq!(r.fields[&f], Label::Delivered, "{f:?}: {:?}", r.notes);
        }
        assert_eq!(r.argv, vec!["--model", "opus"]);
        assert!(wd.join(".claude/skills/review/SKILL.md").is_file());
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(wd.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["theme"], "dark");
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert!(settings["hooks"]["PreToolUse"][1]["hooks"][0]["command"].as_str().unwrap().contains("hook claude-pretool"));
        let mcp: Value = serde_json::from_str(&std::fs::read_to_string(wd.join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["other"]["command"], "x");
        assert_eq!(mcp["mcpServers"]["allternit"]["headers"]["Authorization"], "Bearer ${ALLTERNIT_MCP_TOKEN}");
        // Re-entry converges: one gate hook, not two.
        let r2 = deliver(&req).unwrap();
        assert_eq!(r2.fields, r.fields);
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(wd.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(load_fields(&wd, "builder").unwrap(), r.fields);
    }

    #[test]
    fn codex_hook_via_argv_and_toml_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(wd.join(".codex")).unwrap();
        std::fs::write(wd.join(".codex/config.toml"), "# mine\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n").unwrap();
        let p = profile(tmp.path());
        let r = deliver(&DeliveryRequest { profile: &p, harness: "codex", workdir: &wd, context_pack_path: None, gate: Some(gate(tmp.path())) }).unwrap();
        assert_eq!(r.fields[&Field::Permissions], Label::Delivered);
        assert_eq!(r.fields[&Field::Memory], Label::Partial);
        assert_eq!(r.fields[&Field::Tools], Label::Delivered);
        assert!(r.argv.iter().any(|a| a.starts_with("hooks.PreToolUse=")));
        let toml = std::fs::read_to_string(wd.join(".codex/config.toml")).unwrap();
        assert!(toml.contains("# mine") && toml.contains("[mcp_servers.other]"));
        assert!(toml.contains("[mcp_servers.allternit]") && toml.contains("bearer_token_env_var = \"ALLTERNIT_MCP_TOKEN\""));
        assert!(wd.join("AGENTS.md").is_file());
    }

    #[test]
    fn kimi_partial_and_unknown_harness_unavailable() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        let p = profile(tmp.path());
        let r = deliver(&DeliveryRequest { profile: &p, harness: "kimi", workdir: &wd, context_pack_path: None, gate: None }).unwrap();
        assert_eq!(r.fields[&Field::Permissions], Label::Partial);
        assert_eq!(r.fields[&Field::Skills], Label::Unavailable);
        assert_eq!(r.fields[&Field::Tools], Label::Unavailable);
        assert!(std::fs::read_to_string(wd.join("AGENTS.md")).unwrap().contains("No Gate hook"));

        let wd2 = tmp.path().join("wd2");
        let r = deliver(&DeliveryRequest { profile: &p, harness: "mystery", workdir: &wd2, context_pack_path: None, gate: Some(gate(tmp.path())) }).unwrap();
        assert!(r.fields.values().all(|l| *l == Label::Unavailable));
        assert!(r.files_written.is_empty() && r.argv.is_empty());
        assert!(!wd2.exists());
    }

    #[test]
    fn managed_block_replaced_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        std::fs::create_dir_all(&wd).unwrap();
        let before = format!("# Project\n\nKeep this.\n\n{BLOCK_BEGIN}\nold stuff\n{BLOCK_END}\n\n## After\nAlso keep.\n");
        std::fs::write(wd.join("GIZZI.md"), &before).unwrap();
        let p = profile(tmp.path());
        deliver(&DeliveryRequest { profile: &p, harness: "gizzi", workdir: &wd, context_pack_path: None, gate: None }).unwrap();
        let after = std::fs::read_to_string(wd.join("GIZZI.md")).unwrap();
        assert!(after.starts_with("# Project\n\nKeep this.\n\n<!-- allternit:bot BEGIN -->\n"));
        assert!(after.ends_with(&format!("{BLOCK_END}\n\n## After\nAlso keep.\n")));
        assert!(!after.contains("old stuff"));
        assert!(after.contains("You are Builder.") && after.contains("Speak like Eoj."));
        assert_eq!(after.matches(BLOCK_BEGIN).count(), 1);
        let gizzi: Value = serde_json::from_str(&std::fs::read_to_string(wd.join(".gizzi/gizzi.json")).unwrap()).unwrap();
        assert_eq!(gizzi["mcp"]["allternit"]["type"], "remote");
        assert!(wd.join(".gizzi/skills/review/SKILL.md").is_file());
    }

    fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                all_files(&e.path(), out);
            } else {
                out.push(e.path());
            }
        }
    }

    #[test]
    fn no_secret_in_any_pane_file() {
        let tmp = tempfile::tempdir().unwrap();
        for h in ["claude", "codex", "gizzi", "kimi", "grok", "agy"] {
            let wd = tmp.path().join(h);
            let p = profile(tmp.path());
            let r = deliver(&DeliveryRequest { profile: &p, harness: h, workdir: &wd, context_pack_path: None, gate: Some(gate(tmp.path())) }).unwrap();
            let mut files = vec![];
            all_files(&wd, &mut files);
            assert!(!files.is_empty());
            for f in files {
                let text = std::fs::read(&f).unwrap();
                assert!(!contains(&text, TOKEN.as_bytes()), "{h}: secret in {}", f.display());
            }
            assert!(!r.argv.iter().any(|a| a.contains(TOKEN)));
        }
        // A secret smuggled into the persona is refused, not written.
        let wd = tmp.path().join("leak");
        let mut p = profile(tmp.path());
        p.persona = format!("token is {TOKEN}");
        let r = deliver(&DeliveryRequest { profile: &p, harness: "claude", workdir: &wd, context_pack_path: None, gate: None }).unwrap();
        assert_eq!(r.fields[&Field::Persona], Label::Unavailable);
        assert!(!wd.join("CLAUDE.md").exists());
        // A credential in the connector URL is refused.
        let mut p = profile(tmp.path());
        p.mcp_url = Some("https://mcp.allternit.com/mcp?token=abc".into());
        let r = deliver(&DeliveryRequest { profile: &p, harness: "claude", workdir: &tmp.path().join("u"), context_pack_path: None, gate: None }).unwrap();
        assert_eq!(r.fields[&Field::Tools], Label::Unavailable);
    }

    #[test]
    fn learned_lines_come_back() {
        let tmp = tempfile::tempdir().unwrap();
        let wd = tmp.path().join("wd");
        let mut p = profile(tmp.path());
        p.learned = Some("# LEARNED\n- old fact\n".into());
        deliver(&DeliveryRequest { profile: &p, harness: "claude", workdir: &wd, context_pack_path: None, gate: None }).unwrap();
        assert!(collect_learned(&wd, "builder").is_empty());
        let lp = bot_dir(&wd, "builder").join("LEARNED.md");
        let mut t = std::fs::read_to_string(&lp).unwrap();
        t.push_str("- tests need --nocapture\n\n- tests need --nocapture\n");
        std::fs::write(&lp, t).unwrap();
        assert_eq!(collect_learned(&wd, "builder"), vec!["- tests need --nocapture"]);
    }
}
