//! `allternit-agents-server admin ...`: operator commands.
//!
//! ```text
//! admin setup      [--name N] [--force-new-admin]
//! admin migrate
//! admin keys       list | create --role R [--name N] | revoke <id>
//! admin env-keys   list --environment E | create --environment E [--name N]
//!                  | revoke --environment E <id>
//! admin rotate-key
//! admin doctor     [--offline]
//! ```
//!
//! Every command takes `--config <path>` (or `ALLTERNIT_SELFHOST_CONFIG`).
//! Secrets (new keys) go to stdout once; errors go to stderr. Exit codes: 0
//! success, 1 failure, 2 usage error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;

use super::auth::{API_KEY_PREFIX, ENV_KEY_PREFIX};
use super::egress_guard::EgressGuard;
use super::routes_keys::mint;
use super::secrets::{load_keyring, rotate_credentials, SecretResolver};
use super::settings::{config_path, Provider, Settings, SettingsError};
use super::store::pg::PgStore;
use super::store::sqlite::SqliteStore;
use super::store::{DynStore, NewApiKey, NewEnvironmentKey, Role, StoreError};

pub const USAGE: &str = "\
allternit-agents-server admin <command> [--config <path>]

COMMANDS:
    setup [--name N] [--force-new-admin]
        Run migrations, mint the first admin API key and print it once.
        Refuses when a key exists, unless --force-new-admin.
    migrate
        Apply database migrations. Safe to run again.
    keys list
    keys create --role admin|developer|viewer [--name N]
    keys revoke <key id>
    env-keys list --environment <env id>
    env-keys create --environment <env id> [--name N]
    env-keys revoke --environment <env id> <key id>
    rotate-key
        Re-seal every stored credential with secrets.encryption_key_ref.
        Keep the old key in secrets.previous_key_refs until this finishes.
    doctor [--offline]
        Check settings, database, gizzi, inference, egress and OpenTelemetry.
";

const VALUE_FLAGS: &[&str] = &["--config", "-c", "--role", "--name", "--environment"];
const BOOL_FLAGS: &[&str] = &["--force-new-admin", "--offline"];

/// The SQLite file the server uses when no database URL is set.
pub fn sqlite_path(settings: &Settings) -> PathBuf {
    settings.data_dir.join("agents-server.db")
}

#[derive(Debug)]
enum Fail {
    Usage(String),
    Run(String),
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Self {
        Fail::Run(match e {
            StoreError::Conflict("last_admin_key") => {
                "that is the last active admin key; create another admin key first".into()
            }
            StoreError::Conflict("setup_already_done") => {
                "an API key already exists; pass --force-new-admin".into()
            }
            StoreError::NotFound => "not found".into(),
            other => other.to_string(),
        })
    }
}

impl From<SettingsError> for Fail {
    fn from(e: SettingsError) -> Self {
        Fail::Run(e.to_string())
    }
}

struct Args {
    raw: Vec<String>,
    positional: Vec<String>,
    values: BTreeMap<String, String>,
    bools: Vec<String>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, Fail> {
        let mut positional = Vec::new();
        let mut values = BTreeMap::new();
        let mut bools = Vec::new();
        let mut it = raw.iter();
        while let Some(a) = it.next() {
            if a.starts_with('-') {
                let (flag, inline) = match a.split_once('=') {
                    Some((f, v)) => (f.to_string(), Some(v.to_string())),
                    None => (a.clone(), None),
                };
                if VALUE_FLAGS.contains(&flag.as_str()) {
                    let v = match inline {
                        Some(v) => v,
                        None => it
                            .next()
                            .cloned()
                            .ok_or_else(|| Fail::Usage(format!("{flag} needs a value")))?,
                    };
                    let key = if flag == "-c" { "--config".to_string() } else { flag };
                    values.insert(key, v);
                } else if BOOL_FLAGS.contains(&flag.as_str()) && inline.is_none() {
                    bools.push(flag);
                } else {
                    return Err(Fail::Usage(format!("unknown option {a}")));
                }
            } else {
                positional.push(a.clone());
            }
        }
        Ok(Self {
            raw,
            positional,
            values,
            bools,
        })
    }

    fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    fn flag(&self, flag: &str) -> bool {
        self.bools.iter().any(|b| b == flag)
    }

    fn settings(&self) -> Result<(PathBuf, Settings), Fail> {
        let path = config_path(&self.raw).ok_or_else(|| Fail::Run(SettingsError::NoConfig.to_string()))?;
        let s = Settings::load(&path)?;
        Ok((path, s))
    }
}

/// Run an admin command. Returns the process exit code.
pub async fn run(args: Vec<String>) -> u8 {
    match dispatch(args).await {
        Ok(()) => 0,
        Err(Fail::Usage(m)) => {
            eprintln!("allternit-agents-server admin: {m}\n\n{USAGE}");
            2
        }
        Err(Fail::Run(m)) => {
            eprintln!("allternit-agents-server admin: {m}");
            1
        }
    }
}

async fn dispatch(raw: Vec<String>) -> Result<(), Fail> {
    let args = Args::parse(raw)?;
    let pos: Vec<&str> = args.positional.iter().map(String::as_str).collect();
    match pos.as_slice() {
        [] | ["help"] => {
            print!("{USAGE}");
            Ok(())
        }
        ["setup"] => setup(&args).await,
        ["migrate"] => migrate(&args).await,
        ["keys", "list"] => keys_list(&args).await,
        ["keys", "create"] => keys_create(&args).await,
        ["keys", "revoke", id] => keys_revoke(&args, id).await,
        ["env-keys", "list"] => env_keys_list(&args).await,
        ["env-keys", "create"] => env_keys_create(&args).await,
        ["env-keys", "revoke", id] => env_keys_revoke(&args, id).await,
        ["rotate-key"] => rotate_key(&args).await,
        ["doctor"] => {
            if doctor(&args).await {
                Ok(())
            } else {
                Err(Fail::Run("doctor found failures".into()))
            }
        }
        other => Err(Fail::Usage(format!("unknown command {:?}", other.join(" ")))),
    }
}

// ─── Store ──────────────────────────────────────────────────────────────────

pub struct OpenedStore {
    pub store: DynStore,
    /// "postgres" or "sqlite".
    pub kind: &'static str,
    /// Where it is, without credentials.
    pub location: String,
    /// `(applied, latest)` migration versions, Postgres only.
    pub level: Option<(i64, i64)>,
}

/// Open the persistent store the server would use. `migrate` applies
/// pending Postgres migrations (SQLite applies its schema on every open).
/// Ephemeral mode has no persistent store and is refused.
pub async fn open_store(settings: &Settings, migrate: bool) -> Result<OpenedStore, String> {
    if let Some(url) = settings
        .database_url_resolved()
        .await
        .map_err(|e| e.to_string())?
    {
        let pg = PgStore::connect(&url, settings.database.max_connections.min(4))
            .await
            .map_err(|e| format!("cannot connect to PostgreSQL: {e}"))?;
        if migrate {
            pg.migrate()
                .await
                .map_err(|e| format!("cannot migrate PostgreSQL: {e}"))?;
        }
        let level = pg.migration_level().await.map_err(|e| e.to_string())?;
        let location = url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(|h| format!("{h}{}", u.path())))
            .unwrap_or_else(|| "postgres".into());
        return Ok(OpenedStore {
            store: Arc::new(pg),
            kind: "postgres",
            location,
            level: Some(level),
        });
    }
    if settings.ephemeral {
        return Err(
            "the settings use ephemeral mode (in-memory store); set database.url or ephemeral: false"
                .into(),
        );
    }
    let path = sqlite_path(settings);
    let store = SqliteStore::open(&path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    Ok(OpenedStore {
        store: Arc::new(store),
        kind: "sqlite",
        location: path.display().to_string(),
        level: None,
    })
}

async fn store_for(args: &Args, migrate: bool) -> Result<(Settings, OpenedStore), Fail> {
    let (_, settings) = args.settings()?;
    let opened = open_store(&settings, migrate).await.map_err(Fail::Run)?;
    if let Some((applied, latest)) = opened.level {
        if applied < latest {
            return Err(Fail::Run(format!(
                "the database is at migration {applied}, this server needs {latest}: run `admin migrate`"
            )));
        }
    }
    Ok((settings, opened))
}

// ─── Commands ───────────────────────────────────────────────────────────────

async fn migrate(args: &Args) -> Result<(), Fail> {
    let (_, opened) = store_for(args, true).await?;
    opened.store.health().await?;
    match opened.level {
        Some((applied, latest)) => println!(
            "migrations applied: postgres {} at version {applied} (latest {latest})",
            opened.location
        ),
        None => println!("schema is current: sqlite {}", opened.location),
    }
    Ok(())
}

fn print_secret(kind: &str, id: &str, secret: &str) {
    println!("{kind} created. The secret is shown once; store it now.\n");
    println!("  id:     {id}");
    println!("  secret: {secret}");
}

fn name_arg(args: &Args, default: &str) -> Result<String, Fail> {
    let name = args.value("--name").unwrap_or(default).to_string();
    let n = name.chars().count();
    if n == 0 || n > 100 {
        return Err(Fail::Usage("--name must be 1 to 100 characters".into()));
    }
    Ok(name)
}

async fn setup(args: &Args) -> Result<(), Fail> {
    let name = name_arg(args, "admin")?;
    let (_, opened) = store_for(args, true).await?;
    let store = &opened.store;
    let existing = store.count_api_keys().await?;
    if existing > 0 && !args.flag("--force-new-admin") {
        return Err(Fail::Run(format!(
            "{existing} API key(s) already exist. Use an admin key, or pass --force-new-admin to mint another admin key."
        )));
    }
    let (secret, prefix, key_hash) = mint(API_KEY_PREFIX);
    let new = NewApiKey {
        name,
        role: Role::Admin,
        prefix,
        key_hash,
        expires_at: None,
    };
    let key = if existing == 0 {
        store.create_first_api_key(new).await?
    } else {
        store.create_api_key(new).await?
    };
    print_secret("Admin API key", &key.id, &secret);
    Ok(())
}

fn status_of(revoked: bool, expired: bool) -> &'static str {
    if revoked {
        "revoked"
    } else if expired {
        "expired"
    } else {
        "active"
    }
}

async fn keys_list(args: &Args) -> Result<(), Fail> {
    let (_, opened) = store_for(args, false).await?;
    let now = Utc::now();
    println!("{:<32} {:<10} {:<9} {:<16} {:<20} NAME", "ID", "ROLE", "STATUS", "PREFIX", "CREATED");
    let mut after: Option<String> = None;
    loop {
        let page = opened.store.list_api_keys(100, after.as_deref()).await?;
        for k in &page {
            let expired = k.expires_at.is_some_and(|e| e <= now);
            println!(
                "{:<32} {:<10} {:<9} {:<16} {:<20} {}",
                k.id,
                k.role.as_str(),
                status_of(k.revoked_at.is_some(), expired),
                k.prefix,
                k.created_at.format("%Y-%m-%d %H:%M:%S"),
                k.name
            );
        }
        match page.last() {
            Some(last) if page.len() == 100 => after = Some(last.id.clone()),
            _ => break,
        }
    }
    Ok(())
}

async fn keys_create(args: &Args) -> Result<(), Fail> {
    let role = args
        .value("--role")
        .ok_or_else(|| Fail::Usage("keys create needs --role admin|developer|viewer".into()))?;
    let role = Role::parse(role)
        .ok_or_else(|| Fail::Usage(format!("unknown role {role:?}: use admin, developer or viewer")))?;
    let name = name_arg(args, "cli")?;
    let (_, opened) = store_for(args, false).await?;
    if opened.store.count_api_keys().await? == 0 {
        return Err(Fail::Run("no API key exists yet: run `admin setup` first".into()));
    }
    let (secret, prefix, key_hash) = mint(API_KEY_PREFIX);
    let key = opened
        .store
        .create_api_key(NewApiKey {
            name,
            role,
            prefix,
            key_hash,
            expires_at: None,
        })
        .await?;
    print_secret(&format!("{} API key", role.as_str()), &key.id, &secret);
    Ok(())
}

async fn keys_revoke(args: &Args, id: &str) -> Result<(), Fail> {
    let (_, opened) = store_for(args, false).await?;
    let key = opened.store.revoke_api_key(id, Utc::now()).await?;
    println!("revoked {}", key.id);
    Ok(())
}

fn env_arg(args: &Args) -> Result<String, Fail> {
    args.value("--environment")
        .map(str::to_string)
        .ok_or_else(|| Fail::Usage("this command needs --environment <env id>".into()))
}

async fn env_keys_list(args: &Args) -> Result<(), Fail> {
    let env = env_arg(args)?;
    let (_, opened) = store_for(args, false).await?;
    opened
        .store
        .get_environment(&env)
        .await
        .map_err(|_| Fail::Run(format!("no environment {env}")))?;
    let now = Utc::now();
    println!("{:<32} {:<9} {:<16} {:<20} NAME", "ID", "STATUS", "PREFIX", "CREATED");
    let mut after: Option<String> = None;
    loop {
        let page = opened
            .store
            .list_environment_keys(&env, 100, after.as_deref())
            .await?;
        for k in &page {
            let expired = k.expires_at.is_some_and(|e| e <= now);
            println!(
                "{:<32} {:<9} {:<16} {:<20} {}",
                k.id,
                status_of(k.revoked_at.is_some(), expired),
                k.prefix,
                k.created_at.format("%Y-%m-%d %H:%M:%S"),
                k.name.as_deref().unwrap_or("")
            );
        }
        match page.last() {
            Some(last) if page.len() == 100 => after = Some(last.id.clone()),
            _ => break,
        }
    }
    Ok(())
}

async fn env_keys_create(args: &Args) -> Result<(), Fail> {
    let env = env_arg(args)?;
    let name = match args.value("--name") {
        Some(_) => Some(name_arg(args, "")?),
        None => None,
    };
    let (_, opened) = store_for(args, false).await?;
    let (secret, prefix, key_hash) = mint(ENV_KEY_PREFIX);
    let key = opened
        .store
        .create_environment_key(NewEnvironmentKey {
            environment_id: env.clone(),
            name,
            prefix,
            key_hash,
            expires_at: None,
        })
        .await
        .map_err(|e| match e {
            StoreError::NotFound => Fail::Run(format!("no environment {env}")),
            other => other.into(),
        })?;
    print_secret("Environment key", &key.id, &secret);
    Ok(())
}

async fn env_keys_revoke(args: &Args, id: &str) -> Result<(), Fail> {
    let env = env_arg(args)?;
    let (_, opened) = store_for(args, false).await?;
    let key = opened
        .store
        .revoke_environment_key(&env, id, Utc::now())
        .await?;
    println!("revoked {}", key.id);
    Ok(())
}

async fn rotate_key(args: &Args) -> Result<(), Fail> {
    let (settings, opened) = store_for(args, false).await?;
    let resolver = SecretResolver::from_settings(&settings)?;
    let ring = load_keyring(&settings, &resolver)
        .await?
        .ok_or_else(|| Fail::Run("secrets.encryption_key_ref is not set".into()))?;
    let ids = ring.key_ids();
    println!(
        "current key id {}; previous key ids: {}",
        ring.current_key_id(),
        if ids.len() > 1 { ids[1..].join(", ") } else { "none".into() }
    );
    let report = rotate_credentials(&opened.store, &ring).await?;
    println!(
        "re-sealed {}, already current {}, failed {}",
        report.rotated.len(),
        report.unchanged,
        report.failed.len()
    );
    for name in &report.rotated {
        println!("  re-sealed {name}");
    }
    if !report.failed.is_empty() {
        for (name, why) in &report.failed {
            eprintln!("  FAILED {name}: {why}");
        }
        return Err(Fail::Run(
            "some credentials did not open with any key; add the key that sealed them to secrets.previous_key_refs and run again".into(),
        ));
    }
    Ok(())
}

// ─── Doctor ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

#[derive(Default)]
struct Report {
    lines: Vec<(Level, &'static str, String)>,
}

impl Report {
    fn add(&mut self, level: Level, check: &'static str, msg: impl Into<String>) {
        let msg = msg.into();
        let tag = match level {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        };
        println!("{tag}  {check}: {msg}");
        self.lines.push((level, check, msg));
    }
    fn ok(&self) -> bool {
        !self.lines.iter().any(|(l, _, _)| *l == Level::Fail)
    }
}

/// Where the gizzi binary would be found: `ALLTERNIT_GIZZI_BIN`, next to
/// this binary, then `PATH` (`gizzi`, then `gizzi-code`).
pub fn find_gizzi() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ALLTERNIT_GIZZI_BIN") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    let names = ["gizzi", "gizzi-code"];
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        for n in names {
            if dir.join(n).is_file() {
                return Some(dir.join(n));
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    for n in names {
        for dir in std::env::split_paths(&path) {
            if dir.join(n).is_file() {
                return Some(dir.join(n));
            }
        }
    }
    None
}

/// One URL on the inference host, for a reachability probe.
pub fn inference_probe_url(settings: &Settings) -> Result<String, String> {
    let creds = settings.inference.credentials.clone().unwrap_or_default();
    match settings.inference.provider {
        Provider::Anthropic => Ok("https://api.anthropic.com/v1/models".into()),
        Provider::Bedrock => creds
            .aws_region
            .filter(|r| !r.is_empty())
            .map(|r| format!("https://bedrock-runtime.{r}.amazonaws.com/"))
            .ok_or_else(|| "inference.credentials.aws_region is not set".into()),
        Provider::Vertex => Ok(match creds.gcp_region.as_deref().filter(|r| !r.is_empty()) {
            Some("global") | None => "https://aiplatform.googleapis.com/".into(),
            Some(r) => format!("https://{r}-aiplatform.googleapis.com/"),
        }),
        Provider::Foundry => creds
            .foundry_endpoint
            .ok_or_else(|| "inference.credentials.foundry_endpoint is not set".into()),
        Provider::Gateway => settings
            .inference
            .gateway
            .as_ref()
            .map(|g| g.base_url.clone())
            .ok_or_else(|| "inference.gateway is not set".into()),
    }
}

/// Prints one line per check. True when nothing failed.
async fn doctor(args: &Args) -> bool {
    let mut r = Report::default();
    let path = match config_path(&args.raw) {
        Some(p) => p,
        None => {
            r.add(Level::Fail, "settings", SettingsError::NoConfig.to_string());
            return false;
        }
    };
    let settings = match Settings::load(&path) {
        Ok(s) => {
            r.add(Level::Pass, "settings", format!("{} is valid", path.display()));
            s
        }
        Err(e) => {
            r.add(Level::Fail, "settings", e.to_string());
            return false;
        }
    };

    // Secrets.
    let resolver = match SecretResolver::from_settings(&settings) {
        Ok(res) => Some(res),
        Err(e) => {
            r.add(Level::Fail, "secrets", e.to_string());
            None
        }
    };
    if let Some(resolver) = &resolver {
        match load_keyring(&settings, resolver).await {
            Ok(Some(ring)) => r.add(
                Level::Pass,
                "encryption key",
                format!(
                    "current key id {}, {} previous",
                    ring.current_key_id(),
                    ring.key_ids().len() - 1
                ),
            ),
            Ok(None) => r.add(
                Level::Warn,
                "encryption key",
                "secrets.encryption_key_ref is not set; stored credentials are refused",
            ),
            Err(e) => r.add(Level::Fail, "encryption key", e.to_string()),
        }
        if let Some(c) = &settings.inference.credentials {
            let refs = [
                ("api_key", &c.api_key),
                ("aws_access_key_id", &c.aws_access_key_id),
                ("aws_secret_access_key", &c.aws_secret_access_key),
                ("aws_session_token", &c.aws_session_token),
                ("gcp_service_account_json", &c.gcp_service_account_json),
                ("foundry_api_key", &c.foundry_api_key),
            ];
            let mut bad = Vec::new();
            let mut n = 0;
            for (field, v) in refs {
                if let Some(v) = v {
                    n += 1;
                    if let Err(e) = resolver.resolve(v).await {
                        bad.push(format!("{field}: {e}"));
                    }
                }
            }
            if bad.is_empty() {
                r.add(Level::Pass, "inference credentials", format!("{n} reference(s) resolve"));
            } else {
                r.add(Level::Fail, "inference credentials", bad.join("; "));
            }
        }
    }

    // Database.
    let has_url = settings.database.url.is_some();
    if !has_url && settings.ephemeral {
        r.add(
            Level::Warn,
            "database",
            "ephemeral mode: in-memory store, all state is lost when the server stops",
        );
    } else if !has_url && !sqlite_path(&settings).exists() {
        // Doctor stays read-only: do not create the file just to check it.
        r.add(
            Level::Warn,
            "database",
            format!(
                "no sqlite database at {} yet (created on first start or `admin migrate`)",
                sqlite_path(&settings).display()
            ),
        );
    } else {
        match open_store(&settings, false).await {
            Err(e) => r.add(Level::Fail, "database", e),
            Ok(o) => match o.store.health().await {
                Err(e) => r.add(Level::Fail, "database", format!("{} {}: {e}", o.kind, o.location)),
                Ok(()) => match o.level {
                    Some((applied, latest)) if applied < latest => r.add(
                        Level::Fail,
                        "database",
                        format!(
                            "postgres {} reachable, at migration {applied} of {latest}: run `admin migrate`",
                            o.location
                        ),
                    ),
                    Some((applied, latest)) if applied > latest => r.add(
                        Level::Warn,
                        "database",
                        format!(
                            "postgres {} is at migration {applied}, newer than this server ({latest})",
                            o.location
                        ),
                    ),
                    Some((applied, _)) => r.add(
                        Level::Pass,
                        "database",
                        format!("postgres {} reachable, migrations current ({applied})", o.location),
                    ),
                    None => r.add(
                        Level::Pass,
                        "database",
                        format!("sqlite {} open, schema current", o.location),
                    ),
                },
            },
        }
    }

    // Gizzi.
    match find_gizzi() {
        Some(p) => r.add(Level::Pass, "gizzi", p.display().to_string()),
        None => r.add(
            Level::Fail,
            "gizzi",
            "binary not found: set ALLTERNIT_GIZZI_BIN or put gizzi on PATH",
        ),
    }

    // Egress allowlist.
    let hosts = EgressGuard::hosts_for(&settings);
    if hosts.is_empty() {
        r.add(Level::Warn, "egress", "allowlist is empty: every outbound host is refused");
    } else {
        let home: Vec<&String> = hosts
            .iter()
            .filter(|h| {
                let host = h.split(':').next().unwrap_or("").to_ascii_lowercase();
                host == "allternit.com" || host.ends_with(".allternit.com")
            })
            .collect();
        if home.is_empty() {
            r.add(Level::Pass, "egress", format!("allowlist: {}", hosts.join(", ")));
        } else {
            r.add(
                Level::Warn,
                "egress",
                format!(
                    "allowlist includes allternit.com host(s) {home:?}; the server can reach them: {}",
                    hosts.join(", ")
                ),
            );
        }
    }

    // Inference reachability.
    if args.flag("--offline") {
        r.add(Level::Warn, "inference", "skipped (--offline)");
    } else {
        match inference_probe_url(&settings) {
            Err(e) => r.add(Level::Fail, "inference", e),
            Ok(url) => {
                let guard = Arc::new(EgressGuard::from_settings(&settings));
                match guard.client() {
                    Err(e) => r.add(Level::Fail, "inference", e.to_string()),
                    Ok(client) => match client.request(reqwest::Method::HEAD, &url) {
                        Err(e) => r.add(Level::Fail, "inference", e.to_string()),
                        Ok(req) => match req.timeout(std::time::Duration::from_secs(10)).send().await {
                            // Any HTTP answer (401, 404, 405) proves the host is reachable.
                            Ok(res) => r.add(
                                Level::Pass,
                                "inference",
                                format!("{url} reachable (HTTP {})", res.status().as_u16()),
                            ),
                            Err(e) => r.add(Level::Fail, "inference", format!("{url}: {e}")),
                        },
                    },
                }
            }
        }
    }

    // OpenTelemetry.
    match std::env::var(crate::otel::ENV_OTLP_ENDPOINT)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
    {
        None => r.add(
            Level::Pass,
            "otel",
            "trace export off (OTEL_EXPORTER_OTLP_ENDPOINT is not set)",
        ),
        Some(ep) => match url::Url::parse(&ep) {
            Ok(u) if u.host_str().is_some() => {
                let attrs = crate::otel::parse_resource_attributes(
                    &std::env::var(crate::otel::ENV_RESOURCE_ATTRIBUTES).unwrap_or_default(),
                );
                let service = crate::otel::resolve_service_name(
                    std::env::var(crate::otel::ENV_SERVICE_NAME).ok().as_deref(),
                    &attrs,
                    super::telemetry::DEFAULT_SERVICE_NAME,
                );
                r.add(
                    Level::Pass,
                    "otel",
                    format!("exporting to {ep}/v1/traces as service {service}"),
                );
            }
            _ => r.add(
                Level::Fail,
                "otel",
                format!("OTEL_EXPORTER_OTLP_ENDPOINT {ep:?} is not a URL"),
            ),
        },
    }

    // Prometheus.
    let prom = &settings.observability.prometheus;
    if !prom.enabled {
        r.add(Level::Pass, "metrics", "Prometheus listener off");
    } else {
        let loopback = prom
            .listen
            .parse::<std::net::SocketAddr>()
            .map(|a| a.ip().is_loopback())
            .unwrap_or(prom.listen.starts_with("localhost:"));
        if loopback {
            r.add(Level::Pass, "metrics", format!("GET /metrics on {}", prom.listen));
        } else {
            r.add(
                Level::Warn,
                "metrics",
                format!("GET /metrics on {} is not loopback and has no auth", prom.listen),
            );
        }
    }

    r.ok()
}
