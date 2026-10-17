//! The ledger: everything a person builds in Allternit, kept as plain files
//! with history beside their Memory Drive (ledger program phase 4, step 1:
//! a mirror; the database stays the source of truth for now).
//!
//! `collect` reads the person's rows and folders into files; `snapshot`
//! commits them to `ledger_repo` when something changed. Secrets never go
//! in: sealed-token columns are never read, credential-looking JSON values are
//! replaced, and any file the secret scanner still flags is left out and
//! listed in the README.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::{types::ValueRef, Connection};
use serde_json::{Map, Value};

use crate::db::DbHandle;
use crate::ledger_repo::LedgerRepo;
use crate::memory_drive_service::{Result, ServiceError};

const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_FOLDER_FILES: usize = 200;
const MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const SNAPSHOT_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);
pub(crate) const LEFT_OUT: &str = "[left out: looks like a credential]";

/// One kind of thing the ledger holds, read from one table.
pub(crate) struct Kind {
    pub(crate) dir: &'static str,
    pub(crate) table: &'static str,
    pub(crate) id: &'static str,
    /// Column with a human name (`None`: read `name` from the `template` JSON).
    pub(crate) name: Option<&'static str>,
    /// Extra WHERE clause (after the owner check).
    pub(crate) filter: &'static str,
    /// Columns never exported: credentials, links to credentials, runtime state.
    pub(crate) drop: &'static [&'static str],
}

pub(crate) const KINDS: &[Kind] = &[
    Kind { dir: "bots", table: "agents", id: "id", name: Some("name"), filter: "AND is_bot = 1 AND archived_at IS NULL", drop: &["identity_key", "principal_id", "status", "last_run_at", "user_id"] },
    Kind { dir: "agents", table: "agents", id: "id", name: Some("name"), filter: "AND COALESCE(is_bot, 0) = 0 AND archived_at IS NULL", drop: &["identity_key", "principal_id", "status", "last_run_at", "user_id"] },
    Kind { dir: "bot-templates", table: "bot_templates", id: "id", name: None, filter: "", drop: &["user_id"] },
    Kind { dir: "routines", table: "routines", id: "id", name: Some("name"), filter: "", drop: &["user_id", "next_run_at", "last_run_at", "gizzi_job_id"] },
    Kind { dir: "routines/scheduled-messages", table: "cowork_routines", id: "id", name: Some("name"), filter: "", drop: &["user_id", "next_run_at", "last_run_at", "principal"] },
    Kind { dir: "workflows", table: "workflows", id: "id", name: Some("title"), filter: "", drop: &["user_id"] },
    Kind {
        dir: "tasks",
        table: "tasks",
        id: "id",
        name: Some("title"),
        filter: "AND lower(replace(COALESCE(status,'todo'),'-','_')) IN ('backlog','todo','pending','open','ready','in_progress','in_review')",
        drop: &["user_id"],
    },
    Kind { dir: "tasks/long-running", table: "long_running_tasks", id: "id", name: Some("title"), filter: "AND status IN ('pending','running','paused')", drop: &["user_id", "result", "error"] },
    Kind { dir: "skills/recipes", table: "skills", id: "id", name: Some("name"), filter: "", drop: &["user_id", "run_count"] },
    Kind { dir: "projects", table: "cowork_projects", id: "id", name: Some("title"), filter: "", drop: &["user_id"] },
    Kind { dir: "schedules", table: "cowork_scheduled_tasks", id: "id", name: Some("title"), filter: "", drop: &["user_id", "next_run_at", "last_run_at", "last_run_session_id", "last_error"] },
    Kind { dir: "schedules/loops", table: "loops", id: "id", name: Some("name"), filter: "", drop: &["user_id"] },
    Kind { dir: "schedules/deployments", table: "beta_deployments", id: "id", name: Some("id"), filter: "AND status <> 'archived'", drop: &["user_id", "next_run_at", "last_run_at"] },
];

/// A file-name-safe slug.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(48).collect();
    if out.is_empty() { "untitled".into() } else { out.trim_end_matches('-').to_string() }
}

fn file_name(name: &str, id: &str) -> String {
    let short: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    format!("{}-{}", slug(name), if short.is_empty() { "x".into() } else { short })
}

pub(crate) fn credential_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase().replace(['-', ' '], "_");
    if k.contains("max_token") || k.ends_with("_tokens") || k == "tokens" || k.contains("token_count") || k.contains("token_limit") {
        return false;
    }
    ["token", "secret", "password", "passwd", "api_key", "apikey", "credential", "private_key", "client_secret", "authorization", "cookie"]
        .iter()
        .any(|w| k.contains(w))
}

/// Replace credential-looking values (by key) anywhere in a JSON value.
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for (k, val) in o.iter_mut() {
                if credential_key(k) && !val.is_null() && !matches!(val, Value::Bool(_)) {
                    *val = Value::String(LEFT_OUT.into());
                } else {
                    redact(val);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

/// `https://user:token@host/x` → `https://host/x`.
pub fn strip_userinfo(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, String::new())) {
            (host, path) if host.contains('@') => format!("{scheme}://{}{path}", host.rsplit('@').next().unwrap_or("")),
            _ => url.to_string(),
        },
        None => url.to_string(),
    }
}

pub(crate) fn cell(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).to_string();
            let trimmed = s.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                serde_json::from_str(&s).unwrap_or(Value::String(s))
            } else {
                Value::String(s)
            }
        }
        ValueRef::Blob(_) => Value::Null,
    }
}

fn rows(conn: &Connection, kind: &Kind, owner: &str) -> Vec<Map<String, Value>> {
    let sql = format!("SELECT * FROM {} WHERE user_id = ?1 {}", kind.table, kind.filter);
    let Ok(mut stmt) = conn.prepare(&sql) else { return vec![] };
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let Ok(mapped) = stmt.query_map([owner], |r| {
        let mut m = Map::new();
        for (i, c) in cols.iter().enumerate() {
            if !kind.drop.contains(&c.as_str()) {
                m.insert(c.clone(), cell(r.get_ref(i)?));
            }
        }
        Ok(m)
    }) else {
        return vec![];
    };
    mapped.filter_map(|r| r.ok()).collect()
}

/// What `collect` produced: the files, and what was left out and why.
#[derive(Default)]
pub struct Collected {
    pub files: BTreeMap<String, Vec<u8>>,
    pub counts: BTreeMap<&'static str, usize>,
    pub left_out: Vec<String>,
    bytes: usize,
}

/// Token shapes that are credentials wherever they appear.
pub(crate) fn token_like(text: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(sk-[a-z0-9_-]{16,}|gh[pousr]_[a-z0-9_]{16,}|github_pat_[a-z0-9_]+|glpat-[a-z0-9_-]{12,}|xox[baprs]-[a-z0-9-]{10,}|AKIA[A-Z0-9]{16}|ASIA[A-Z0-9]{16}|allternit_git_[a-z0-9]+|atok_[a-z0-9]{16,}|eyJ[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}\.[a-z0-9_-]{8,}|-----BEGIN [A-Z ]*PRIVATE KEY-----)",
        )
        .expect("constant regex")
    })
    .is_match(text)
}

/// `password: x`, `API_TOKEN=x`, `secret = x`: a value written next to a credential name.
pub(crate) fn assigns_secret(text: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?i)\b[a-z_]*(?:token|secret|password|passwd|api_key|apikey)\s*[:=]\s*[^\s]{6,}").expect("constant regex"))
        .is_match(text)
}

impl Collected {
    /// A record already redacted by key: only real token shapes keep it out.
    fn put_json(&mut self, path: String, value: &Value) {
        let bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
        if token_like(&String::from_utf8_lossy(&bytes)) {
            self.left_out.push(format!("{path} (looks like it contains a password or key)"));
            return;
        }
        self.store(path, bytes);
    }

    /// A file copied as-is: the full secret scan applies.
    fn put(&mut self, path: String, bytes: Vec<u8>) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if token_like(text) || assigns_secret(text) {
                self.left_out.push(format!("{path} (looks like it contains a password or key)"));
                return;
            }
        }
        self.store(path, bytes);
    }

    fn store(&mut self, path: String, bytes: Vec<u8>) {
        if !crate::ledger_repo::valid_path(&path) || self.files.contains_key(&path) {
            return;
        }
        if self.bytes + bytes.len() > MAX_TOTAL_BYTES {
            self.left_out.push(format!("{path} (the ledger reached {} MB)", MAX_TOTAL_BYTES / (1024 * 1024)));
            return;
        }
        self.bytes += bytes.len();
        self.files.insert(path, bytes);
    }
}

/// Copy a folder's regular files (no symlinks, no dot-folders) under `prefix`.
fn copy_folder(out: &mut Collected, dir: &Path, prefix: &str) {
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    let mut n = 0;
    while let Some((d, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && name != ".claude" {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if meta.is_dir() {
                stack.push((e.path(), rel_path));
            } else if meta.is_file() && meta.len() <= MAX_FILE_BYTES && n < MAX_FOLDER_FILES {
                if let Ok(bytes) = std::fs::read(e.path()) {
                    n += 1;
                    out.put(format!("{prefix}/{rel_path}"), bytes);
                }
            }
        }
    }
}

/// Folders where the Allternit app keeps installed skills.
pub fn skill_dirs() -> Vec<PathBuf> {
    std::env::var_os("ALLTERNIT_LEDGER_SKILLS_DIR")
        .map(|p| vec![PathBuf::from(p)])
        .unwrap_or_else(|| dirs::home_dir().map(|h| vec![h.join(".allternit").join("skills")]).unwrap_or_default())
}

/// Read everything `owner` built into ledger files.
pub fn collect(conn: &Connection, owner: &str, skill_roots: &[PathBuf], bot_workspace: impl Fn(&str) -> PathBuf) -> Collected {
    let mut out = Collected::default();
    for kind in KINDS {
        let rows = rows(conn, kind, owner);
        for mut row in rows {
            let id = row.get(kind.id).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            let name = match kind.name {
                Some(col) => row.get(col).and_then(|v| v.as_str()).unwrap_or("untitled").to_string(),
                None => row.get("template").and_then(|t| t.get("name")).and_then(|v| v.as_str()).unwrap_or("template").to_string(),
            };
            if let Some(Value::String(url)) = row.get_mut("git_remote") {
                *url = strip_userinfo(url);
            }
            let mut value = Value::Object(row);
            redact(&mut value);
            let stem = file_name(&name, &id);
            if kind.dir == "bots" || kind.dir == "agents" {
                out.put_json(format!("{}/{stem}/{}.json", kind.dir, if kind.dir == "bots" { "bot" } else { "agent" }), &value);
                let ws = bot_workspace(&id);
                if ws.is_dir() {
                    copy_folder(&mut out, &ws, &format!("{}/{stem}/workspace", kind.dir));
                }
            } else {
                out.put_json(format!("{}/{stem}.json", kind.dir), &value);
            }
            *out.counts.entry(kind.dir).or_default() += 1;
        }
    }
    // Connected apps: which accounts are connected, never their tokens.
    if let Ok(mut stmt) = conn.prepare("SELECT connector_id, account, scopes, status, auth_type, created_at FROM connector_connections WHERE user_id = ?1 ORDER BY connector_id") {
        let list: Vec<Value> = stmt
            .query_map([owner], |r| {
                Ok(serde_json::json!({
                    "connector": cell(r.get_ref(0)?), "account": cell(r.get_ref(1)?), "scopes": cell(r.get_ref(2)?),
                    "status": cell(r.get_ref(3)?), "auth_type": cell(r.get_ref(4)?), "connected_at": cell(r.get_ref(5)?),
                }))
            })
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        if !list.is_empty() {
            out.counts.insert("connected-apps", list.len());
            out.put_json("connected-apps.json".into(), &Value::Array(list));
        }
    }
    // Installed skills (SKILL.md folders).
    for root in skill_roots {
        let Ok(rd) = std::fs::read_dir(root) else { continue };
        let mut skills: Vec<_> = rd.flatten().filter(|e| e.path().join("SKILL.md").is_file()).collect();
        skills.sort_by_key(|e| e.file_name());
        for e in skills {
            let name = slug(&e.file_name().to_string_lossy());
            copy_folder(&mut out, &e.path(), &format!("skills/{name}"));
            *out.counts.entry("skills").or_default() += 1;
        }
    }
    let readme = readme(&out);
    out.files.insert("README.md".into(), readme.into_bytes());
    out
}

fn readme(c: &Collected) -> String {
    let names = [
        ("bots", "Bots, each with its settings and workspace files"),
        ("agents", "Agents"),
        ("bot-templates", "Bot templates"),
        ("routines", "Routines"),
        ("routines/scheduled-messages", "Scheduled messages"),
        ("workflows", "Workflows"),
        ("tasks", "Open tasks"),
        ("tasks/long-running", "Long-running tasks still going"),
        ("skills", "Installed skills (SKILL.md folders)"),
        ("skills/recipes", "Skill recipes"),
        ("projects", "Projects"),
        ("schedules", "Scheduled tasks"),
        ("schedules/loops", "Loops"),
        ("schedules/deployments", "Scheduled deployments"),
        ("connected-apps", "Connected apps (accounts only, never tokens)"),
    ];
    let mut s = String::from(
        "# Your Allternit ledger\n\nEverything you've built in Allternit, as plain files with history. Your memories are in your\nMemory Drive; this is everything else. Allternit updates it every hour and when you ask.\n\n",
    );
    for (key, label) in names {
        if let Some(n) = c.counts.get(key) {
            s.push_str(&format!("- `{key}/`: {label} ({n})\n"));
        }
    }
    if c.counts.is_empty() {
        s.push_str("Nothing here yet. Bots, routines, workflows and the rest appear as you make them.\n");
    }
    s.push_str("\nPasswords, keys and sign-in tokens are never written here.\n");
    if !c.left_out.is_empty() {
        s.push_str("\n## Left out\n\n");
        for l in &c.left_out {
            s.push_str(&format!("- {l}\n"));
        }
    }
    s
}

/// The ledger repo for `owner`: beside their personal Memory Drive.
pub fn repo_for(db: &DbHandle, owner: &str) -> Result<LedgerRepo> {
    let drive = crate::memory_drive_writer::ensure(db, owner)?.ok_or(ServiceError::NotFound)?;
    let dir = drive.repo_path.parent().map(Path::to_path_buf).unwrap_or_else(|| drive.repo_path.clone());
    LedgerRepo::open(dir.join("ledger.git")).map_err(|e| ServiceError::Provenance(e.to_string()))
}

/// Collect and commit. Returns the new revision, or `None` when nothing changed.
pub fn snapshot(db: &DbHandle, owner: &str, message: &str) -> Result<Option<String>> {
    let repo = repo_for(db, owner)?;
    let collected = collect(&db.connect()?, owner, &skill_dirs(), crate::agent_workspace_paths::workspace_dir_for);
    repo.write_snapshot(&collected.files, message).map_err(|e| ServiceError::Provenance(e.to_string()))
}

fn owners(conn: &Connection) -> Vec<String> {
    conn.prepare("SELECT scope_id FROM memory_drives WHERE kind = 'personal'")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0)).map(|it| it.filter_map(|r| r.ok()).collect()))
        .unwrap_or_default()
}

/// Snapshot every owner's ledger every hour (only changes are committed).
pub fn spawn(db: DbHandle) {
    let db = Arc::new(db);
    tokio::spawn(async move {
        // Not at boot: start a few minutes after the app is up.
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + std::time::Duration::from_secs(5 * 60), SNAPSHOT_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let db = db.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let list = db.connect().map(|c| owners(&c)).unwrap_or_default();
                for owner in list {
                    if let Err(e) = snapshot(&db, &owner, "Update ledger") {
                        tracing::debug!("ledger snapshot skipped: {e}");
                    }
                    // Then refresh any copies the owner keeps elsewhere.
                    if let Err(e) = crate::memory_copies::run(&db, &owner) {
                        tracing::debug!("memory copies skipped: {e}");
                    }
                }
            })
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_names_are_stable() {
        assert_eq!(slug("  Mattress Matt! "), "mattress-matt");
        assert_eq!(slug("???"), "untitled");
        assert_eq!(file_name("Daily standup", "rt_9f8e7d6c5b4a"), "daily-standup-rt9f8e7d");
    }

    #[test]
    fn credentials_are_redacted_and_urls_stripped() {
        let mut v = serde_json::json!({"api_key": "abc", "max_tokens": 4000, "nested": [{"client_secret": "x", "label": "ok"}], "has_password": false});
        redact(&mut v);
        assert_eq!(v["api_key"], LEFT_OUT);
        assert_eq!(v["max_tokens"], 4000);
        assert_eq!(v["nested"][0]["client_secret"], LEFT_OUT);
        assert_eq!(v["nested"][0]["label"], "ok");
        assert_eq!(v["has_password"], false);
        assert_eq!(strip_userinfo("https://joe:ghp_abc@github.com/a/b.git"), "https://github.com/a/b.git");
        assert_eq!(strip_userinfo("git@github.com:a/b.git"), "git@github.com:a/b.git");
    }

    #[test]
    fn collects_each_kind_without_secrets() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agents (id TEXT, user_id TEXT, name TEXT, system_prompt TEXT, config TEXT, identity_key TEXT, is_bot INTEGER, archived_at TEXT, status TEXT);
             INSERT INTO agents VALUES ('ag_1','u1','Mattress Matt','Be helpful','{\"model\":\"x\",\"api_key\":\"k\"}','secret-key',1,NULL,'idle');
             INSERT INTO agents VALUES ('ag_2','u1','Old','x','{}',NULL,1,'2026-01-01',NULL);
             INSERT INTO agents VALUES ('ag_3','u2','Not mine','x','{}',NULL,1,NULL,NULL);
             CREATE TABLE workflows (id TEXT, user_id TEXT, title TEXT, nodes TEXT, edges TEXT);
             INSERT INTO workflows VALUES ('wf_1','u1','Lead intake','[{\"id\":\"a\"}]','[]');
             CREATE TABLE tasks (id TEXT, user_id TEXT, title TEXT, status TEXT);
             INSERT INTO tasks VALUES ('t1','u1','Call Ann','todo'), ('t2','u1','Done thing','done'), ('t3','u1','Review','in-review');
             CREATE TABLE cowork_projects (id TEXT, user_id TEXT, title TEXT, git_remote TEXT);
             INSERT INTO cowork_projects VALUES ('p1','u1','Site','https://joe:ghp_abcdefgh123@github.com/a/b.git');
             CREATE TABLE connector_connections (id TEXT, connector_id TEXT, user_id TEXT, auth_type TEXT, status TEXT, access_token TEXT, refresh_token TEXT, scopes TEXT, account TEXT, created_at TEXT);
             INSERT INTO connector_connections VALUES ('c1','gmail','u1','oauth','active','ya29.secret','1//refresh','[\"mail\"]','eoj@example.com','2026-10-01');",
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("ag_1")).unwrap();
        std::fs::write(ws.join("ag_1/SOUL.md"), "You sell mattresses.").unwrap();
        std::fs::write(ws.join("ag_1/leak.md"), "token: ghp_abcdefghijklmnop").unwrap();
        let skills = tmp.path().join("skills");
        std::fs::create_dir_all(skills.join("Brand Voice")).unwrap();
        std::fs::write(skills.join("Brand Voice/SKILL.md"), "---\nname: brand-voice\n---\nWrite plainly.").unwrap();

        let c = collect(&conn, "u1", &[skills], |id| ws.join(id));
        let paths: Vec<&str> = c.files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            vec![
                "README.md",
                "bots/mattress-matt-ag1/bot.json",
                "bots/mattress-matt-ag1/workspace/SOUL.md",
                "connected-apps.json",
                "projects/site-p1.json",
                "skills/brand-voice/SKILL.md",
                "tasks/call-ann-t1.json",
                "tasks/review-t3.json",
                "workflows/lead-intake-wf1.json",
            ]
        );
        let all: String = c.files.values().map(|b| String::from_utf8_lossy(b).to_string()).collect();
        for secret in ["secret-key", "\"k\"", "ya29.secret", "1//refresh", "ghp_abcdefgh123", "ghp_abcdefghijklmnop", "idle"] {
            assert!(!all.contains(secret), "{secret} leaked");
        }
        let bot: Value = serde_json::from_slice(&c.files["bots/mattress-matt-ag1/bot.json"]).unwrap();
        assert_eq!(bot["config"]["model"], "x");
        assert_eq!(bot["config"]["api_key"], LEFT_OUT);
        let wf: Value = serde_json::from_slice(&c.files["workflows/lead-intake-wf1.json"]).unwrap();
        assert_eq!(wf["nodes"][0]["id"], "a");
        assert!(String::from_utf8_lossy(&c.files["connected-apps.json"]).contains("eoj@example.com"));
        let readme = String::from_utf8_lossy(&c.files["README.md"]);
        assert!(readme.contains("`bots/`: Bots, each with its settings and workspace files (1)"));
        assert!(readme.contains("leak.md (looks like it contains a password or key)"));
        assert!(!assigns_secret("Reset a password: ask the user first."));
        assert!(assigns_secret("API_TOKEN=abcdef123456"));
    }
}
