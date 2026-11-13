//! Orgo (orgo.ai) provider for hosted cloud computers (Eoj, 2026-11-13: "use
//! orgo instead of e2b"). We rent computers pay-as-you-go and only own hosts
//! once subscriptions pay for them. A child module of `provisioning`, so it
//! plugs into the same service, registry, state machine and metering.
//!
//! ## Host row
//!
//! An operator registers one `provisioned_hosts` row whose endpoint is
//! `orgo://<workspace_id>` (the existing `POST /api/v1/provisioned-hosts`
//! route). Its capacity numbers are the pool size: paid computers reserve
//! their size in that ledger, free ones only their disk, exactly like an
//! Incus host. The API key comes from `ORGO_API_KEY` (never logged);
//! `ORGO_API_BASE` overrides `https://www.orgo.ai/api`.
//!
//! ## Routing
//!
//! `ALLTERNIT_COMPUTE_PROVIDER_PLATFORM`, `_PAID` and `_FREE` are `incus`
//! (default) or `orgo`. A class routed to Orgo lands only on `orgo://` hosts;
//! every other class never lands on one. Hosted-driver computers
//! (`/v1/computers`) always stay on Incus: they need the egress-filtered
//! network profile, which Orgo has no equivalent of. A paid re-subscribe that
//! restores a cancel snapshot stays on the Incus host holding that image.
//!
//! ## Guards
//!
//! Orgo computers reserve nothing in host memory admission. Instead:
//! `ALLTERNIT_ORGO_MAX_RUNNING` (default 10) computers provisioning, running
//! or waking at once, and `ALLTERNIT_ORGO_DAILY_BUDGET_USD` (default 5) of
//! estimated spend per UTC day: running hours from the metering sessions (plus
//! time spent in first boot) times the hourly price of the Orgo size. Past
//! either: the `computer_capacity` 503 (message starts [`CLOUD_POOL_FULL`]).
//!
//! ## Size mapping
//!
//! Orgo sells cpu in 0.5/1/2/4/6/8/16 and ram in 4/8/16/32/64 GB. A request
//! maps to the smallest allowed cpu and ram at or above it (cpu cores and
//! ceil(memory_mb / 1024) GB, capped at 16 cpu / 64 GB). A Platform API
//! project runtime is always 0.5 cpu / 4 GB (Orgo Small). Price estimate: the
//! smallest named size that covers both numbers (Small 0.5c/4GB $0.023/h,
//! Medium 2c/8GB $0.055, Large 4c/16GB $0.11, XL 6c/32GB $0.202); above XL,
//! XL scaled by the larger of the cpu and ram ratios, and never below
//! `ALLTERNIT_ORGO_MIN_HOURLY_USD` (default $0.055: the pay-as-you-go account
//! creates every computer as Medium whatever is asked, checked live
//! 2026-11-13). Stopped computers (about 5% of the running price) are not
//! counted.
//!
//! ## First boot
//!
//! Orgo computers boot Orgo's desktop image (Ubuntu 24.04, root, XFCE on
//! Xtigervnc `:99`, supervisord already running, outbound HTTPS open; custom
//! templates need Orgo's top plan), have no systemd, and accept shell commands
//! through `POST
//! /computers/{id}/bash` (300 s max per call). So a create is: `POST
//! /computers` (hourly billing, `Idempotency-Key` = our instance id, name = our
//! `incus_name`), wait for `running`, write `/etc/allternit/bootstrap.json`,
//! write `cmd/allternit-computer-cloud/guest/orgo/setup.sh` and start it
//! detached (`setsid nohup`), poll its state file until `done` or `failed`
//! (each poll is one short bash call), then run the ensure script. The script
//! installs the Desktop .deb (`ALLTERNIT_ORGO_DESKTOP_DEB_URL`, sha256 checked
//! when `ALLTERNIT_ORGO_DESKTOP_DEB_SHA256` is set; the 633 MB .deb is too big
//! for Orgo's upload endpoints, so the computer downloads it), swaps in
//! runtime-feed sidecars when the bundled ones need a newer glibc, and runs
//! the app under the image's supervisord. This takes minutes, so it runs in the background
//! ([`ProvisionBackend::provisions_in_background`]): the row stays
//! `provisioning` and the backend reports `Busy` for the computer until first
//! boot ends, so reconcile neither flips it to running nor marks it missing.
//! The bootstrap is written before the script starts, so a cloud-api restart
//! mid-setup still ends with a paired computer. A failure marks the row
//! `error`, stops the computer, and holds off a new attempt for that owner
//! for 15 minutes.
//!
//! ## Files
//!
//! `push_file` writes through the bash API (base64 in 48 KB chunks into a temp
//! file, then chown/chmod/mv) rather than `POST /files/upload`: that endpoint
//! takes a destination directory with no owner or mode, needs multipart (not
//! enabled in our reqwest), and the bootstrap must land at an exact path, root
//! owned, 0600, in one atomic rename.
//!
//! ## Idle and wake
//!
//! After first boot the computer's Orgo auto-stop is set best-effort (paid
//! 30m, platform 15m, free 15m; `ALLTERNIT_ORGO_AUTO_STOP_PAID` / `_PLATFORM` /
//! `_FREE`, empty = leave Orgo's default). Our own sweeps are the real idle
//! control: free and platform computers sleep on the free idle window as
//! before, and paid computers on Orgo sleep after
//! `ALLTERNIT_ORGO_PAID_IDLE_MINUTES` (default 30; Eoj approved) of no
//! relay activity. A sleeping Orgo computer (free or paid) wakes on a relay
//! request; the relay waits up to 90 s for it, because an Orgo start restores
//! the disk from storage. `stop` = Orgo stop (disk kept, memory lost); `start`
//! = Orgo start, then the ensure script brings supervisord and the app back.
//!
//! ## Not supported
//!
//! Cancel snapshots (`snapshot_to_image`) and owner restore points/exports:
//! a cancelled paid computer on Orgo is stopped and kept (it is the only copy
//! of the disk). The 30-day lifecycle never deletes it, because it only
//! deletes suspended computers whose snapshot exists. Delete it by hand (or
//! through account erase) when it should go. Resize works only while the
//! computer runs (Orgo rule); the disk only grows.

use super::*;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::time::{Duration as StdDuration, Instant};

/// Endpoint scheme of an Orgo host row (`orgo://<workspace_id>`).
pub const ORGO_SCHEME: &str = "orgo";
const ENV_ORGO_API_KEY: &str = "ORGO_API_KEY";
const ENV_ORGO_API_BASE: &str = "ORGO_API_BASE";
const DEFAULT_ORGO_API_BASE: &str = "https://www.orgo.ai/api";

/// The first-boot script (see the module docs). Path is relative to this file.
pub const ORGO_SETUP_SCRIPT: &str =
    include_str!("../../../allternit-computer-cloud/guest/orgo/setup.sh");
const SETUP_PATH: &str = "/opt/allternit-orgo/setup.sh";
const SETUP_STATE_PATH: &str = "/var/lib/allternit-orgo/state";
const SETUP_LOG_PATH: &str = "/var/log/allternit-orgo/setup.log";
/// Starts supervisord and the app (installed by the setup script). Exit 3 =
/// first boot never finished on this computer.
const ENSURE_COMMAND: &str = "if [ -x /usr/local/sbin/allternit-orgo-ensure ]; then \
     /usr/local/sbin/allternit-orgo-ensure; else \
     echo 'allternit first-boot setup has not finished on this computer' >&2; exit 3; fi";

/// Start of the error message when the Orgo running cap or daily budget is
/// reached. The routes turn it into a 503 `computer_capacity`.
pub const CLOUD_POOL_FULL: &str = "Cloud computers are at capacity";
/// How long the relay waits for a waking Orgo computer's runtime.
pub const ORGO_WAKE_WAIT_SECONDS: u64 = 90;
/// A failed first boot holds off the next create for the same owner this long.
const FIRST_BOOT_BACKOFF_MINUTES: i64 = 15;
const FIRST_BOOT_FAILED_PREFIX: &str = "first boot failed";

/// True for an `orgo://` host endpoint.
pub fn is_orgo_endpoint(endpoint: &str) -> bool {
    provider_for_endpoint(endpoint) == ORGO_SCHEME
}

// ---------------------------------------------------------------------------
// Routing and guards (env)
// ---------------------------------------------------------------------------

/// Where a class of computers is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeProvider {
    Incus,
    Orgo,
}

impl ComputeProvider {
    fn from_env(name: &str) -> Self {
        match std::env::var(name)
            .ok()
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("") | Some("incus") => Self::Incus,
            Some("orgo") => Self::Orgo,
            Some(other) => {
                tracing::warn!(
                    variable = name,
                    value = other,
                    "unknown compute provider; using incus"
                );
                Self::Incus
            }
        }
    }
}

/// Provider per class plus the Orgo guards, snapshotted at service start.
#[derive(Debug, Clone)]
pub struct ComputeRouting {
    pub platform: ComputeProvider,
    pub paid: ComputeProvider,
    pub free: ComputeProvider,
    /// `ALLTERNIT_ORGO_MAX_RUNNING`: Orgo computers provisioning, running or
    /// waking at once.
    pub orgo_max_running: i64,
    /// `ALLTERNIT_ORGO_DAILY_BUDGET_USD`: estimated Orgo spend per UTC day.
    pub orgo_daily_budget_usd: f64,
    /// `ALLTERNIT_ORGO_PAID_IDLE_MINUTES`: a paid Orgo computer sleeps after
    /// this long with no relay activity.
    pub orgo_paid_idle: Duration,
    /// `ALLTERNIT_ORGO_MIN_HOURLY_USD`: floor of the per-computer price in the
    /// budget estimate. The pay-as-you-go account bills every computer as
    /// Medium ($0.055/h) whatever size is asked for (checked live 2026-11-13).
    pub orgo_min_hourly_usd: f64,
}

impl Default for ComputeRouting {
    fn default() -> Self {
        Self {
            platform: ComputeProvider::Incus,
            paid: ComputeProvider::Incus,
            free: ComputeProvider::Incus,
            orgo_max_running: 10,
            orgo_daily_budget_usd: 5.0,
            orgo_paid_idle: Duration::minutes(30),
            orgo_min_hourly_usd: 0.055,
        }
    }
}

impl ComputeRouting {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            platform: ComputeProvider::from_env("ALLTERNIT_COMPUTE_PROVIDER_PLATFORM"),
            paid: ComputeProvider::from_env("ALLTERNIT_COMPUTE_PROVIDER_PAID"),
            free: ComputeProvider::from_env("ALLTERNIT_COMPUTE_PROVIDER_FREE"),
            orgo_max_running: env_i64("ALLTERNIT_ORGO_MAX_RUNNING", defaults.orgo_max_running)
                .max(0),
            orgo_daily_budget_usd: std::env::var("ALLTERNIT_ORGO_DAILY_BUDGET_USD")
                .ok()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(defaults.orgo_daily_budget_usd),
            orgo_paid_idle: Duration::minutes(
                env_i64("ALLTERNIT_ORGO_PAID_IDLE_MINUTES", 30).max(1),
            ),
            orgo_min_hourly_usd: std::env::var("ALLTERNIT_ORGO_MIN_HOURLY_USD")
                .ok()
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .unwrap_or(defaults.orgo_min_hourly_usd),
        }
    }

    /// Provider for a class. Hosted-driver computers always stay on Incus.
    pub fn provider_for(&self, class: ComputeClass) -> ComputeProvider {
        match class {
            ComputeClass::Platform => self.platform,
            ComputeClass::Paid => self.paid,
            ComputeClass::Free => self.free,
            ComputeClass::HostedDriver => ComputeProvider::Incus,
        }
    }
}

// ---------------------------------------------------------------------------
// Sizes and prices
// ---------------------------------------------------------------------------

/// An Orgo machine size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrgoSize {
    pub cpu: f64,
    pub ram_gb: i64,
}

const ORGO_CPUS: [f64; 7] = [0.5, 1.0, 2.0, 4.0, 6.0, 8.0, 16.0];
const ORGO_RAM_GB: [i64; 5] = [4, 8, 16, 32, 64];
/// Named sizes with their hourly running price (docs.orgo.ai, 2026-11-13).
const ORGO_PRICES: [(f64, i64, f64); 4] = [
    (0.5, 4, 0.023),
    (2.0, 8, 0.055),
    (4.0, 16, 0.11),
    (6.0, 32, 0.202),
];

/// The Orgo size for a computer (see the module docs).
pub fn orgo_size_for(class: ComputeClass, size: ComputerSize) -> OrgoSize {
    if class == ComputeClass::Platform {
        return OrgoSize {
            cpu: 0.5,
            ram_gb: 4,
        };
    }
    let cpu = ORGO_CPUS
        .iter()
        .copied()
        .find(|cpu| *cpu >= size.cpu_cores as f64)
        .unwrap_or(16.0);
    let need_gb = (size.memory_mb.max(0) + 1023) / 1024;
    let ram_gb = ORGO_RAM_GB
        .iter()
        .copied()
        .find(|gb| *gb >= need_gb)
        .unwrap_or(64);
    OrgoSize { cpu, ram_gb }
}

/// Estimated hourly running price of an Orgo size, USD.
pub fn orgo_hourly_usd(size: OrgoSize) -> f64 {
    for (cpu, ram_gb, price) in ORGO_PRICES {
        if size.cpu <= cpu && size.ram_gb <= ram_gb {
            return price;
        }
    }
    let (cpu, ram_gb, price) = ORGO_PRICES[ORGO_PRICES.len() - 1];
    price * (size.cpu / cpu).max(size.ram_gb as f64 / ram_gb as f64)
}

fn cpu_json(cpu: f64) -> serde_json::Value {
    if cpu.fract() == 0.0 {
        serde_json::json!(cpu as i64)
    } else {
        serde_json::json!(cpu)
    }
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// Orgo settings (env), snapshotted when the backend is built.
#[derive(Debug, Clone)]
pub struct OrgoConfig {
    pub deb_url: Option<String>,
    pub deb_sha256: Option<String>,
    /// `ALLTERNIT_ORGO_RESOLUTION`, passed to create as `resolution`.
    pub resolution: Option<String>,
    pub auto_stop_paid: Option<String>,
    pub auto_stop_platform: Option<String>,
    pub auto_stop_free: Option<String>,
    /// How long a start (or stop) may take before it is an error.
    pub start_wait: StdDuration,
    /// How long a new computer may take to reach `running`.
    pub create_wait: StdDuration,
    /// How long the first-boot script may run.
    pub setup_timeout: StdDuration,
    pub poll_interval: StdDuration,
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// "30m" / "1h" / "45" (minutes) → whole minutes for Orgo; unparseable → 30.
fn auto_stop_minutes(idle: &str) -> i64 {
    let t = idle.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(h) = t.strip_suffix('h') { (h, 60) } else { (t.strip_suffix('m').unwrap_or(&t), 1) };
    num.trim().parse::<i64>().map(|n| (n * mult).max(0)).unwrap_or(30)
}

/// Unset = `default`; set but empty = do not set Orgo's auto-stop.
fn auto_stop_env(name: &str, default: &str) -> Option<String> {
    match std::env::var(name) {
        Err(_) => Some(default.to_string()),
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value.trim().to_string()),
    }
}

impl OrgoConfig {
    pub fn from_env() -> Self {
        let seconds = |name: &str, default: i64| {
            StdDuration::from_secs(env_i64(name, default).max(10) as u64)
        };
        Self {
            deb_url: env_opt("ALLTERNIT_ORGO_DESKTOP_DEB_URL"),
            deb_sha256: env_opt("ALLTERNIT_ORGO_DESKTOP_DEB_SHA256"),
            resolution: env_opt("ALLTERNIT_ORGO_RESOLUTION"),
            auto_stop_paid: auto_stop_env("ALLTERNIT_ORGO_AUTO_STOP_PAID", "30m"),
            auto_stop_platform: auto_stop_env("ALLTERNIT_ORGO_AUTO_STOP_PLATFORM", "15m"),
            auto_stop_free: auto_stop_env("ALLTERNIT_ORGO_AUTO_STOP_FREE", "15m"),
            start_wait: seconds("ALLTERNIT_ORGO_START_WAIT_SECONDS", 180),
            create_wait: seconds("ALLTERNIT_ORGO_CREATE_WAIT_SECONDS", 600),
            setup_timeout: seconds("ALLTERNIT_ORGO_SETUP_TIMEOUT_SECONDS", 1500),
            poll_interval: StdDuration::from_secs(5),
        }
    }

    fn auto_stop_for(&self, class: ComputeClass) -> Option<&str> {
        match class {
            ComputeClass::Paid => self.auto_stop_paid.as_deref(),
            ComputeClass::Platform => self.auto_stop_platform.as_deref(),
            ComputeClass::Free | ComputeClass::HostedDriver => self.auto_stop_free.as_deref(),
        }
    }
}

/// Orgo REST adapter. One per `orgo://` host (cached by the registry), so its
/// id cache and first-boot set are shared by create, reconcile and wake.
pub struct OrgoBackend {
    client: reqwest::Client,
    base: String,
    api_key: String,
    workspace_id: String,
    config: OrgoConfig,
    /// Orgo computer id by computer name (our `incus_name`).
    ids: Mutex<HashMap<String, String>>,
    /// Computers between create and the end of first boot, with their class.
    creating: Mutex<HashMap<String, ComputeClass>>,
}

impl std::fmt::Debug for OrgoBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // Never the API key.
        f.debug_struct("OrgoBackend")
            .field("base", &self.base)
            .field("workspace_id", &self.workspace_id)
            .finish_non_exhaustive()
    }
}

fn orgo_error(status: u16, json: &serde_json::Value) -> ProvisionError {
    let message = json
        .get("error")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| json.to_string());
    let message = match json.get("code").and_then(|value| value.as_str()) {
        Some(code) => format!("orgo: {message} ({code})"),
        None => format!("orgo: {message}"),
    };
    if status == 404 {
        ProvisionError::NotFound(message)
    } else {
        ProvisionError::Api { status, message }
    }
}

fn retry_after_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(*key) {
        Some(serde_json::Value::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(serde_json::Value::Number(number)) => Some(number.to_string()),
        _ => None,
    })
}

/// The computer object in a create/get answer (top level or under `computer`).
fn computer_object(json: &serde_json::Value) -> &serde_json::Value {
    json.get("computer")
        .filter(|value| value.is_object())
        .unwrap_or(json)
}

/// `(id, name)` of each computer in a `GET /workspaces/{id}` answer.
fn desktops_of(json: &serde_json::Value) -> Vec<(String, String)> {
    let list = [
        json.get("desktops"),
        json.get("workspace")
            .and_then(|value| value.get("desktops")),
        json.get("data").and_then(|value| value.get("desktops")),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_array());
    list.map(|items| {
        items
            .iter()
            .filter_map(|item| {
                Some((
                    string_field(item, &["id", "computer_id", "desktop_id"])?,
                    string_field(item, &["name"])?,
                ))
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Orgo status → the DevPod enum. `error` reads Busy (with a warning): the
/// row keeps its state and an operator looks; it is not "gone".
fn map_orgo_status(raw: Option<&str>) -> BackendStatus {
    match raw {
        None | Some("deleted") => BackendStatus::NotFound,
        Some("running") => BackendStatus::Running,
        Some("frozen" | "stopped" | "suspended") => BackendStatus::Stopped,
        Some(
            "creating" | "restarting" | "updating" | "suspending" | "resuming" | "starting"
            | "stopping",
        ) => BackendStatus::Busy,
        Some(other) => {
            warn_orgo_state_once(other);
            BackendStatus::Busy
        }
    }
}

fn warn_orgo_state_once(raw: &str) {
    static SEEN: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let first = SEEN
        .get_or_init(Default::default)
        .lock()
        .map(|mut seen| seen.insert(raw.to_string()))
        .unwrap_or(false);
    if first {
        tracing::warn!(state = raw, "orgo: computer state treated as busy");
    }
}

/// Single-quote a string for bash.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Base64 characters per write step (a multiple of 4, so each chunk decodes
/// on its own).
const PUSH_CHUNK: usize = 48 * 1024;
/// Upper bound on one bash command's length.
const PUSH_COMMAND_MAX: usize = 96 * 1024;

/// Shell commands that write `file` exactly: parent created (0700, owned like
/// the file) only when missing, content appended in base64 chunks to a temp
/// file, then owner, mode and an atomic rename. Each command is one bash
/// call; steps are chained with `&&` so the first failure stops.
pub(crate) fn push_commands(file: &InstanceFile) -> Vec<String> {
    let path = sh_quote(&file.path);
    let tmp = sh_quote(&format!("{}.allternit-part", file.path));
    let owner = format!("{}:{}", file.uid, file.gid);
    let mode = if !file.mode.is_empty() && file.mode.chars().all(|ch| ch.is_ascii_digit()) {
        file.mode.clone()
    } else {
        "0600".to_string()
    };
    let mut steps = Vec::new();
    if let Some((parent, _)) = file
        .path
        .rsplit_once('/')
        .filter(|(parent, _)| !parent.is_empty())
    {
        let parent = sh_quote(parent);
        steps.push(format!(
            "{{ [ -d {parent} ] || {{ mkdir -p {parent} && chown {owner} {parent} && chmod 0700 {parent}; }}; }}"
        ));
    }
    steps.push(format!("rm -f {tmp}"));
    steps.push(format!("touch {tmp}"));
    steps.push(format!("chmod 0600 {tmp}"));
    let encoded = BASE64.encode(&file.content);
    for chunk in encoded.as_bytes().chunks(PUSH_CHUNK) {
        let chunk = std::str::from_utf8(chunk).unwrap_or_default();
        steps.push(format!("printf '%s' '{chunk}' | base64 -d >> {tmp}"));
    }
    steps.push(format!("chown {owner} {tmp}"));
    steps.push(format!("chmod {mode} {tmp}"));
    steps.push(format!("mv -f {tmp} {path}"));

    let mut commands = Vec::new();
    let mut current = String::new();
    for step in steps {
        if !current.is_empty() && current.len() + step.len() + 4 > PUSH_COMMAND_MAX {
            commands.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push_str(" && ");
        }
        current.push_str(&step);
    }
    if !current.is_empty() {
        commands.push(current);
    }
    commands
}

fn tail_of(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

impl OrgoBackend {
    /// Build from a host endpoint `orgo://<workspace_id>` and the env.
    pub fn from_endpoint(endpoint: &str) -> Result<Self, ProvisionError> {
        let workspace = endpoint
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .trim_matches('/');
        if workspace.is_empty() {
            return Err(ProvisionError::Request(
                "an Orgo host endpoint must be orgo://<workspace_id>".to_string(),
            ));
        }
        let Some(api_key) = env_opt(ENV_ORGO_API_KEY) else {
            return Err(ProvisionError::Request(format!(
                "{ENV_ORGO_API_KEY} is not set"
            )));
        };
        Self::new(
            &env_string(ENV_ORGO_API_BASE, DEFAULT_ORGO_API_BASE),
            &api_key,
            workspace,
            OrgoConfig::from_env(),
        )
    }

    pub fn new(
        base: &str,
        api_key: &str,
        workspace_id: &str,
        config: OrgoConfig,
    ) -> Result<Self, ProvisionError> {
        // Above the bash API's 300 s command limit.
        let client = reqwest::Client::builder()
            .timeout(StdDuration::from_secs(330))
            .connect_timeout(StdDuration::from_secs(20))
            .build()
            .map_err(|error| ProvisionError::Request(format!("orgo client: {error}")))?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            workspace_id: workspace_id.to_string(),
            config,
            ids: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashMap::new()),
        })
    }

    /// One API call. A 429 is retried (twice) after its `Retry-After`
    /// (capped at 30 s).
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
        idempotency_key: Option<&str>,
    ) -> Result<(u16, serde_json::Value), ProvisionError> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut request = self
                .client
                .request(method.clone(), &url)
                .bearer_auth(&self.api_key);
            if let Some(body) = body {
                request = request.json(body);
            }
            if let Some(key) = idempotency_key {
                request = request.header("Idempotency-Key", key);
            }
            let response = request
                .send()
                .await
                .map_err(|error| ProvisionError::Request(format!("orgo: {error}")))?;
            let status = response.status().as_u16();
            if status == 429 && attempt < 3 {
                let wait = retry_after_seconds(response.headers()).unwrap_or(5).min(30);
                tracing::warn!(path, wait, "orgo: rate limited; retrying");
                tokio::time::sleep(StdDuration::from_secs(wait)).await;
                continue;
            }
            let json = response.json().await.unwrap_or(serde_json::Value::Null);
            return Ok((status, json));
        }
    }

    fn forget(&self, name: &str) {
        self.ids.lock().unwrap().remove(name);
    }

    /// The Orgo id of computer `name`: the cache, else the workspace's
    /// computer list. A failed list is an error, never "not found": a wrong
    /// workspace must not read as every computer being gone.
    async fn lookup_id(&self, name: &str) -> Result<Option<String>, ProvisionError> {
        let cached = self.ids.lock().unwrap().get(name).cloned();
        if let Some(id) = cached {
            return Ok(Some(id));
        }
        let (status, json) = self
            .send(
                reqwest::Method::GET,
                &format!("/workspaces/{}", self.workspace_id),
                None,
                None,
            )
            .await?;
        if !is_success(status) {
            return Err(match orgo_error(status, &json) {
                ProvisionError::NotFound(message) => ProvisionError::Api { status, message },
                other => other,
            });
        }
        let found = desktops_of(&json)
            .into_iter()
            .find(|(_, desktop)| desktop == name)
            .map(|(id, _)| id);
        if let Some(id) = &found {
            self.ids
                .lock()
                .unwrap()
                .insert(name.to_string(), id.clone());
        }
        Ok(found)
    }

    async fn require_id(&self, name: &str) -> Result<String, ProvisionError> {
        self.lookup_id(name).await?.ok_or_else(|| {
            ProvisionError::NotFound(format!("orgo computer {name} is not in the workspace"))
        })
    }

    /// Orgo's status string for a computer id; `None` when it is gone (404).
    async fn computer_state(&self, id: &str) -> Result<Option<String>, ProvisionError> {
        let (status, json) = self
            .send(
                reqwest::Method::GET,
                &format!("/computers/{id}"),
                None,
                None,
            )
            .await?;
        if status == 404 {
            return Ok(None);
        }
        if !is_success(status) {
            return Err(orgo_error(status, &json));
        }
        Ok(Some(
            string_field(computer_object(&json), &["status"])
                .unwrap_or_default()
                .to_ascii_lowercase(),
        ))
    }

    /// `POST /computers/{id}/{verb}`. A 409 (already in that state) is fine;
    /// the caller re-reads the status.
    async fn action(&self, id: &str, verb: &str) -> Result<(), ProvisionError> {
        let (status, json) = self
            .send(
                reqwest::Method::POST,
                &format!("/computers/{id}/{verb}"),
                Some(&serde_json::json!({})),
                None,
            )
            .await?;
        if is_success(status) || status == 409 {
            Ok(())
        } else {
            Err(orgo_error(status, &json))
        }
    }

    /// One shell command: `(exit_code, output)`. A non-zero exit is not an
    /// HTTP error; a timeout reads exit -1.
    async fn bash(
        &self,
        id: &str,
        command: &str,
        timeout_seconds: u64,
    ) -> Result<(i64, String), ProvisionError> {
        let body = serde_json::json!({ "command": command, "timeout": timeout_seconds.min(300) });
        let (status, json) = self
            .send(
                reqwest::Method::POST,
                &format!("/computers/{id}/bash"),
                Some(&body),
                None,
            )
            .await?;
        if !is_success(status) {
            return Err(orgo_error(status, &json));
        }
        let success = json
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let exit = json
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .unwrap_or(if success { 0 } else { -1 });
        let output = json
            .get("output")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        Ok((exit, output))
    }

    async fn push_file_to(&self, id: &str, file: &InstanceFile) -> Result<(), ProvisionError> {
        for command in push_commands(file) {
            let (exit, output) = self.bash(id, &command, 60).await?;
            if exit != 0 {
                return Err(ProvisionError::Api {
                    status: 502,
                    message: format!(
                        "orgo: writing {} failed (exit {exit}): {}",
                        file.path,
                        tail_of(&output, 300)
                    ),
                });
            }
        }
        Ok(())
    }

    /// Best effort: Orgo's own idle auto-stop (`PATCH /computers/{id}/auto-stop`
    /// `{"auto_stop_minutes": N}`, 0 = never). Orgo's free plan fixes it at
    /// 15 minutes and refuses changes, so a failure only logs.
    async fn set_auto_stop(&self, id: &str, idle: &str) {
        let body = serde_json::json!({ "auto_stop_minutes": auto_stop_minutes(idle) });
        match self
            .send(
                reqwest::Method::PATCH,
                &format!("/computers/{id}/auto-stop"),
                Some(&body),
                None,
            )
            .await
        {
            Ok((status, _)) if is_success(status) => {}
            Ok((status, json)) => {
                tracing::warn!(%id, idle, status, error = %orgo_error(status, &json), "orgo: auto-stop not set");
            }
            Err(error) => tracing::warn!(%id, idle, %error, "orgo: auto-stop not set"),
        }
    }

    /// Poll until the computer is `running` (start/resume once if it is
    /// stopped), or `deadline`.
    async fn wait_running(
        &self,
        name: &str,
        id: &str,
        wait: StdDuration,
        wake: bool,
    ) -> Result<(), ProvisionError> {
        let deadline = Instant::now() + wait;
        let mut asked = false;
        loop {
            match self.computer_state(id).await?.as_deref() {
                None | Some("deleted") => {
                    self.forget(name);
                    return Err(ProvisionError::NotFound(format!(
                        "orgo computer {name} is gone"
                    )));
                }
                Some("running") => return Ok(()),
                Some("error") => {
                    return Err(ProvisionError::Api {
                        status: 502,
                        message: format!("orgo: computer {name} is in the error state"),
                    })
                }
                Some("frozen" | "stopped") if wake && !asked => {
                    self.action(id, "start").await?;
                    asked = true;
                }
                Some("suspended") if wake && !asked => {
                    self.action(id, "resume").await?;
                    asked = true;
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(ProvisionError::Timeout);
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn first_boot_inner(
        &self,
        name: &str,
        files: &[InstanceFile],
        class: Option<ComputeClass>,
    ) -> Result<(), ProvisionError> {
        let id = self.require_id(name).await?;
        self.wait_running(name, &id, self.config.create_wait, false)
            .await?;
        if let Some(idle) = class.and_then(|class| self.config.auto_stop_for(class)) {
            self.set_auto_stop(&id, idle).await;
        }
        // Bootstrap first: a setup run that outlives this call still pairs.
        for file in files {
            self.push_file_to(&id, file).await?;
        }
        self.push_file_to(
            &id,
            &InstanceFile {
                path: SETUP_PATH.to_string(),
                content: ORGO_SETUP_SCRIPT.as_bytes().to_vec(),
                uid: 0,
                gid: 0,
                mode: "0700".to_string(),
            },
        )
        .await?;
        let deb_url = self.config.deb_url.clone().unwrap_or_default();
        let deb_sha = self.config.deb_sha256.clone().unwrap_or_default();
        let launch = format!(
            "mkdir -p /var/lib/allternit-orgo /var/log/allternit-orgo && rm -f {SETUP_STATE_PATH} || exit 1; \
             S=$(command -v setsid || true); \
             ALLTERNIT_ORGO_DESKTOP_DEB_URL={} ALLTERNIT_ORGO_DESKTOP_DEB_SHA256={} \
             $S nohup bash {SETUP_PATH} > {SETUP_LOG_PATH} 2>&1 < /dev/null & echo launched",
            sh_quote(&deb_url),
            sh_quote(&deb_sha),
        );
        let (exit, output) = self.bash(&id, &launch, 30).await?;
        if exit != 0 {
            return Err(ProvisionError::Api {
                status: 502,
                message: format!(
                    "orgo: first-boot script did not launch (exit {exit}): {}",
                    tail_of(&output, 300)
                ),
            });
        }
        let deadline = Instant::now() + self.config.setup_timeout;
        let state_command = format!("cat {SETUP_STATE_PATH} 2>/dev/null || echo pending");
        loop {
            tokio::time::sleep(self.config.poll_interval).await;
            match self.bash(&id, &state_command, 30).await {
                Ok((_, output)) => {
                    let state = output.trim();
                    if state == "done" {
                        break;
                    }
                    if state.starts_with("failed") {
                        let log = self.setup_log_tail(&id).await;
                        return Err(ProvisionError::Api {
                            status: 502,
                            message: format!("orgo: first-boot setup {state}; log: {log}"),
                        });
                    }
                }
                // A poll that failed (network, a busy API) is retried until the deadline.
                Err(error) => {
                    tracing::warn!(%name, %error, "orgo: first-boot poll failed; retrying")
                }
            }
            if Instant::now() >= deadline {
                let log = self.setup_log_tail(&id).await;
                return Err(ProvisionError::Api {
                    status: 504,
                    message: format!(
                        "orgo: first-boot setup did not finish in {} s; log: {log}",
                        self.config.setup_timeout.as_secs()
                    ),
                });
            }
        }
        self.ensure_app(name, &id).await
    }

    async fn setup_log_tail(&self, id: &str) -> String {
        match self
            .bash(id, &format!("tail -n 30 {SETUP_LOG_PATH} 2>/dev/null"), 30)
            .await
        {
            Ok((_, output)) => tail_of(&output, 1500),
            Err(error) => format!("(log unavailable: {error})"),
        }
    }

    async fn ensure_app(&self, name: &str, id: &str) -> Result<(), ProvisionError> {
        let (exit, output) = self.bash(id, ENSURE_COMMAND, 120).await?;
        if exit != 0 {
            return Err(ProvisionError::Api {
                status: 502,
                message: format!(
                    "orgo: Allternit did not start on {name} (exit {exit}): {}",
                    tail_of(&output, 600)
                ),
            });
        }
        Ok(())
    }
}

#[async_trait]
impl ProvisionBackend for OrgoBackend {
    fn provider(&self) -> &'static str {
        ORGO_SCHEME
    }

    fn provisions_in_background(&self) -> bool {
        true
    }

    async fn create(&self, spec: &ProvisionSpec) -> Result<(), ProvisionError> {
        // Check before paying for a computer that could not be set up.
        if self.config.deb_url.is_none() {
            return Err(ProvisionError::Api {
                status: 503,
                message:
                    "ALLTERNIT_ORGO_DESKTOP_DEB_URL is not set; Orgo computers cannot be set up"
                        .to_string(),
            });
        }
        let size = orgo_size_for(
            spec.class,
            ComputerSize {
                cpu_cores: spec.cpu_cores,
                memory_mb: spec.memory_mb,
                disk_gb: spec.disk_gb,
            },
        );
        let mut body = serde_json::json!({
            "workspace_id": self.workspace_id,
            "name": spec.name,
            "os": "linux",
            "ram": size.ram_gb,
            "cpu": cpu_json(size.cpu),
            "disk_size_gb": spec.disk_gb,
            "billing_mode": "hourly",
        });
        if let Some(resolution) = &self.config.resolution {
            body["resolution"] = resolution.as_str().into();
        }
        self.creating
            .lock()
            .unwrap()
            .insert(spec.name.clone(), spec.class);
        let key = (!spec.instance_id.is_empty()).then_some(spec.instance_id.as_str());
        let result: Result<(), ProvisionError> = async {
            let (status, json) = self.send(reqwest::Method::POST, "/computers", Some(&body), key).await?;
            if is_success(status) {
                let id = string_field(computer_object(&json), &["id", "computer_id"]).ok_or_else(|| {
                    ProvisionError::Api { status, message: "orgo: create answered without a computer id".to_string() }
                })?;
                self.ids.lock().unwrap().insert(spec.name.clone(), id);
                return Ok(());
            }
            if status == 409 {
                // The name is taken in this workspace: an earlier attempt for
                // this same computer. Adopt it (first boot is idempotent).
                self.forget(&spec.name);
                if let Some(id) = self.lookup_id(&spec.name).await? {
                    tracing::info!(name = %spec.name, %id, "orgo: computer name already taken; adopting it");
                    return Ok(());
                }
            }
            Err(orgo_error(status, &json))
        }
        .await;
        if result.is_err() {
            self.creating.lock().unwrap().remove(&spec.name);
        }
        result
    }

    async fn first_boot(&self, name: &str, files: &[InstanceFile]) -> Result<(), ProvisionError> {
        let class = self.creating.lock().unwrap().get(name).copied();
        let result = self.first_boot_inner(name, files, class).await;
        self.creating.lock().unwrap().remove(name);
        result
    }

    async fn start(&self, name: &str) -> Result<(), ProvisionError> {
        let id = self.require_id(name).await?;
        self.wait_running(name, &id, self.config.start_wait, true)
            .await?;
        self.ensure_app(name, &id).await
    }

    async fn stop(&self, name: &str) -> Result<(), ProvisionError> {
        let id = self.require_id(name).await?;
        match self.computer_state(&id).await?.as_deref() {
            None | Some("deleted") => {
                self.forget(name);
                return Err(ProvisionError::NotFound(format!(
                    "orgo computer {name} is gone"
                )));
            }
            Some("frozen" | "stopped" | "suspended") => return Ok(()),
            _ => {}
        }
        self.action(&id, "stop").await?;
        // Wait until Orgo no longer reports it running, so reconcile does not
        // read the stop as "started outside the wake path".
        let deadline = Instant::now() + self.config.start_wait;
        loop {
            match self.computer_state(&id).await?.as_deref() {
                None | Some("deleted") => {
                    self.forget(name);
                    return Err(ProvisionError::NotFound(format!(
                        "orgo computer {name} is gone"
                    )));
                }
                Some("frozen" | "stopped" | "suspended") => return Ok(()),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(ProvisionError::Timeout);
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn status(&self, name: &str) -> Result<BackendStatus, ProvisionError> {
        let in_first_boot = self.creating.lock().unwrap().contains_key(name);
        if in_first_boot {
            return Ok(BackendStatus::Busy);
        }
        let Some(id) = self.lookup_id(name).await? else {
            return Ok(BackendStatus::NotFound);
        };
        let state = self.computer_state(&id).await?;
        if state.is_none() {
            self.forget(name);
        }
        Ok(map_orgo_status(state.as_deref()))
    }

    async fn delete(&self, name: &str) -> Result<(), ProvisionError> {
        self.creating.lock().unwrap().remove(name);
        let Some(id) = self.lookup_id(name).await? else {
            return Err(ProvisionError::NotFound(format!(
                "orgo computer {name} is not in the workspace"
            )));
        };
        let (status, json) = self
            .send(
                reqwest::Method::DELETE,
                &format!("/computers/{id}"),
                None,
                None,
            )
            .await?;
        self.forget(name);
        if is_success(status) {
            Ok(())
        } else {
            Err(orgo_error(status, &json))
        }
    }

    async fn resize(
        &self,
        name: &str,
        size: ComputerSize,
        _storage_pool: &str,
    ) -> Result<(), ProvisionError> {
        let id = self.require_id(name).await?;
        match self.computer_state(&id).await?.as_deref() {
            Some("running") => {}
            None | Some("deleted") => {
                self.forget(name);
                return Err(ProvisionError::NotFound(format!("orgo computer {name} is gone")));
            }
            Some(other) => {
                return Err(ProvisionError::Api {
                    status: 409,
                    message: format!(
                        "orgo resizes a computer only while it runs (it is {other}); the plan size applies on the next resize while it runs"
                    ),
                })
            }
        }
        let target = orgo_size_for(ComputeClass::Paid, size);
        let body = serde_json::json!({
            "ram": target.ram_gb,
            "cpu": cpu_json(target.cpu),
            "disk_size_gb": size.disk_gb,
        });
        let (status, json) = self
            .send(
                reqwest::Method::PATCH,
                &format!("/computers/{id}/resize"),
                Some(&body),
                None,
            )
            .await?;
        if is_success(status) {
            Ok(())
        } else {
            Err(orgo_error(status, &json))
        }
    }

    async fn snapshot_to_image(
        &self,
        _name: &str,
        _alias: &str,
        _compression: Option<&str>,
    ) -> Result<(), ProvisionError> {
        Err(ProvisionError::Api {
            status: 501,
            message:
                "cancel snapshots are not supported on Orgo; the stopped computer is kept instead"
                    .to_string(),
        })
    }

    async fn delete_image(&self, _alias: &str) -> Result<(), ProvisionError> {
        // Orgo hosts never hold snapshot images.
        Ok(())
    }

    async fn push_file(&self, name: &str, file: &InstanceFile) -> Result<(), ProvisionError> {
        let id = self.require_id(name).await?;
        self.push_file_to(&id, file).await
    }
}

// ---------------------------------------------------------------------------
// Guards and service hooks
// ---------------------------------------------------------------------------

/// True when `host_id` is an `orgo://` host.
pub(crate) async fn host_is_orgo<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    host_id: &str,
) -> Result<bool, ApiError> {
    let endpoint: Option<String> =
        sqlx::query_scalar("SELECT incus_endpoint FROM provisioned_hosts WHERE id = $1")
            .bind(host_id)
            .fetch_optional(executor)
            .await?;
    Ok(endpoint.as_deref().is_some_and(is_orgo_endpoint))
}

/// Orgo computers provisioning, running or waking.
async fn orgo_running(conn: &mut sqlx::PgConnection) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM provisioned_instances i JOIN provisioned_hosts h ON h.id = i.host_id \
         WHERE LOWER(h.incus_endpoint) LIKE 'orgo://%' AND i.status IN ('provisioning', 'running', 'waking')",
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Estimated Orgo spend for the UTC day of `now`: metered running time inside
/// the day plus the time computers spent in first boot, each at the hourly
/// price of its Orgo size, never below `min_hourly_usd` (what the account
/// really bills per computer).
pub(crate) async fn orgo_spend_today_usd(
    conn: &mut sqlx::PgConnection,
    now: DateTime<Utc>,
    min_hourly_usd: f64,
) -> Result<f64, ApiError> {
    let day_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default()
        .and_utc();
    let rows: Vec<(String, String, i32, i64, i64)> = sqlx::query_as(
        r#"
        SELECT i.user_id, i.tier, i.cpu_cores, i.memory_mb,
               COALESCE(SUM(GREATEST(0, EXTRACT(EPOCH FROM (COALESCE(s.ended_at, $2) - GREATEST(s.started_at, $1))))), 0)::BIGINT
        FROM provisioned_instance_usage_sessions s
        JOIN provisioned_instances i ON i.id = s.instance_id
        JOIN provisioned_hosts h ON h.id = i.host_id
        WHERE LOWER(h.incus_endpoint) LIKE 'orgo://%'
          AND COALESCE(s.ended_at, $2) > $1 AND s.started_at < $2
        GROUP BY i.id, i.user_id, i.tier, i.cpu_cores, i.memory_mb
        UNION ALL
        SELECT i.user_id, i.tier, i.cpu_cores, i.memory_mb,
               GREATEST(0, EXTRACT(EPOCH FROM ($2 - GREATEST(i.created_at, $1))))::BIGINT
        FROM provisioned_instances i
        JOIN provisioned_hosts h ON h.id = i.host_id
        WHERE LOWER(h.incus_endpoint) LIKE 'orgo://%' AND i.status = 'provisioning'
        "#,
    )
    .bind(day_start)
    .bind(now)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, tier, cpu, memory_mb, seconds)| {
            let class = ComputeClass::for_owner(&user_id, &tier);
            let size = orgo_size_for(
                class,
                ComputerSize {
                    cpu_cores: i64::from(cpu),
                    memory_mb,
                    disk_gb: 0,
                },
            );
            orgo_hourly_usd(size).max(min_hourly_usd) * seconds as f64 / 3600.0
        })
        .sum())
}

/// The 503 for an Orgo create/wake past the running cap or the daily budget.
/// The reason is logged, not shown (it carries spend).
pub(crate) fn cloud_capacity_error() -> ApiError {
    ApiError::ServiceUnavailable(format!(
        "{CLOUD_POOL_FULL}; retry in {CAPACITY_RETRY_AFTER_SECONDS} seconds"
    ))
}

/// Admit one more running Orgo computer, or the capacity 503. Call inside the
/// transaction holding the host row lock to make it binding.
pub(crate) async fn orgo_admits(
    conn: &mut sqlx::PgConnection,
    routing: &ComputeRouting,
) -> Result<(), ApiError> {
    let running = orgo_running(&mut *conn).await?;
    if running >= routing.orgo_max_running {
        tracing::warn!(
            running,
            limit = routing.orgo_max_running,
            "orgo: running cap reached; refusing"
        );
        return Err(cloud_capacity_error());
    }
    let spent = orgo_spend_today_usd(&mut *conn, Utc::now(), routing.orgo_min_hourly_usd).await?;
    if spent >= routing.orgo_daily_budget_usd {
        tracing::warn!(
            spent,
            budget = routing.orgo_daily_budget_usd,
            "orgo: daily budget reached; refusing"
        );
        return Err(cloud_capacity_error());
    }
    Ok(())
}

/// Background half of an Orgo create failing: the row goes to `error` (only
/// while it is still provisioning/running: a delete meanwhile wins), the
/// allocation is released and the team is told.
pub(crate) async fn fail_background_create(
    db: &PgPool,
    instance_id: &str,
    host_id: &str,
    allocation: ComputerSize,
    message: &str,
) {
    tracing::error!(%instance_id, %host_id, error = %message, "cloud computer first boot failed; marking error");
    let updated = sqlx::query(
        "UPDATE provisioned_instances SET status = 'error', error_message = $1, updated_at = CURRENT_TIMESTAMP \
         WHERE id = $2 AND status IN ('provisioning', 'running')",
    )
    .bind(format!("{FIRST_BOOT_FAILED_PREFIX}: {message}"))
    .bind(instance_id)
    .execute(db)
    .await;
    match updated {
        Ok(done) if done.rows_affected() == 1 => {
            if let Err(error) = record_instance_stopped(db, instance_id, "first_boot_failed").await
            {
                tracing::warn!(%instance_id, %error, "first boot failure: metering not closed");
            }
            release_host_allocation(db, host_id, allocation).await;
            crate::services::ops_alert::send_once(
                &format!("first-boot:{instance_id}"),
                "A cloud computer failed its first boot".to_string(),
                format!(
                    "Instance: {instance_id}\nHost: {host_id}\nError: {message}\n\n\
                     The computer was stopped and the row marked error. The owner's next create retries after \
                     {FIRST_BOOT_BACKOFF_MINUTES} minutes (and deletes this computer first)."
                ),
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%instance_id, %error, "first boot failure: row not marked error")
        }
    }
}

impl ProvisioningService {
    /// True when a computer of this owner/tier failed its first boot in the
    /// last [`FIRST_BOOT_BACKOFF_MINUTES`]: a new Orgo attempt waits, so a
    /// deterministic failure (bad .deb URL) cannot buy a computer per call.
    pub(crate) async fn orgo_first_boot_backoff(
        &self,
        user_id: &str,
        tier: &str,
    ) -> Result<bool, ApiError> {
        let recent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM provisioned_instances \
             WHERE user_id = $1 AND tier = $2 AND error_message LIKE $3 AND updated_at > $4",
        )
        .bind(user_id)
        .bind(tier)
        .bind(format!("{FIRST_BOOT_FAILED_PREFIX}%"))
        .bind(Utc::now() - Duration::minutes(FIRST_BOOT_BACKOFF_MINUTES))
        .fetch_one(&self.db)
        .await?;
        Ok(recent > 0)
    }

    /// How long the relay should wait for the runtime of a waking computer:
    /// [`ORGO_WAKE_WAIT_SECONDS`] on an Orgo host; `None` = the relay default.
    pub async fn wake_wait_for_device(&self, device_id: &str) -> Option<StdDuration> {
        let endpoint: Option<String> = sqlx::query_scalar(
            "SELECT h.incus_endpoint FROM provisioned_instances i JOIN provisioned_hosts h ON h.id = i.host_id \
             WHERE i.device_id = $1 AND i.status <> 'deleted' ORDER BY i.created_at DESC LIMIT 1",
        )
        .bind(device_id)
        .fetch_optional(&self.db)
        .await
        .ok()
        .flatten();
        endpoint
            .as_deref()
            .is_some_and(is_orgo_endpoint)
            .then(|| StdDuration::from_secs(ORGO_WAKE_WAIT_SECONDS))
    }

    /// Paid computers on Orgo sleep after `orgo_paid_idle` with no activity
    /// (a scheduled job about to be due keeps them up, like free ones).
    pub(crate) async fn sleep_idle_orgo_paid(&self, now: DateTime<Utc>) -> Result<(), ApiError> {
        let idle = sqlx::query_as::<_, InstanceRow>(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM provisioned_instances \
             WHERE tier = 'paid' AND status = 'running' \
               AND COALESCE(last_activity_at, last_started_at, created_at) <= $1 \
               AND (next_wake_at IS NULL OR next_wake_at > $2) \
               AND host_id IN (SELECT id FROM provisioned_hosts WHERE LOWER(incus_endpoint) LIKE 'orgo://%')"
        ))
        .bind(now - self.routing.orgo_paid_idle)
        .bind(now + self.free.wake_lead)
        .fetch_all(&self.db)
        .await?;
        for row in idle {
            if let Err(error) = self.sleep_row(&row, "idle").await {
                tracing::warn!(id = %row.id, %error, "orgo sweep: idle paid computer not slept");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap as AxumHeaders, Method, StatusCode, Uri};
    use axum::response::IntoResponse;

    // ---- pure mapping ------------------------------------------------------

    #[test]
    fn sizes_round_up_to_orgo_sizes_and_the_price_covers_the_size() {
        let size = |cpu, mem| ComputerSize {
            cpu_cores: cpu,
            memory_mb: mem,
            disk_gb: 20,
        };
        // Free default (2 vCPU / 2 GB): ram rounds up to Orgo's 4 GB floor.
        assert_eq!(
            orgo_size_for(ComputeClass::Free, size(2, 2048)),
            OrgoSize {
                cpu: 2.0,
                ram_gb: 4
            }
        );
        // Exactly 8 GB stays 8; one MB more is 16.
        assert_eq!(orgo_size_for(ComputeClass::Paid, size(4, 8192)).ram_gb, 8);
        assert_eq!(orgo_size_for(ComputeClass::Paid, size(4, 8193)).ram_gb, 16);
        // 3 cores is not an Orgo size: 4.
        assert_eq!(orgo_size_for(ComputeClass::Paid, size(3, 4096)).cpu, 4.0);
        // Beyond the biggest Orgo size: capped.
        assert_eq!(
            orgo_size_for(ComputeClass::Paid, size(32, 200_000)),
            OrgoSize {
                cpu: 16.0,
                ram_gb: 64
            }
        );
        // A Platform API runtime is always Orgo Small, whatever it asks for.
        assert_eq!(
            orgo_size_for(ComputeClass::Platform, size(2, 2048)),
            OrgoSize {
                cpu: 0.5,
                ram_gb: 4
            }
        );

        // Price: the smallest named size that covers both numbers.
        assert_eq!(
            orgo_hourly_usd(OrgoSize {
                cpu: 0.5,
                ram_gb: 4
            }),
            0.023
        );
        assert_eq!(
            orgo_hourly_usd(OrgoSize {
                cpu: 2.0,
                ram_gb: 4
            }),
            0.055,
            "2 cpu needs Medium"
        );
        assert_eq!(
            orgo_hourly_usd(OrgoSize {
                cpu: 1.0,
                ram_gb: 16
            }),
            0.11,
            "16 GB needs Large"
        );
        // Above XL it scales up, never down.
        let big = orgo_hourly_usd(OrgoSize {
            cpu: 16.0,
            ram_gb: 64,
        });
        assert!(big > 0.202 * 2.6 && big < 0.202 * 2.7, "{big}");
    }

    #[test]
    fn orgo_states_map_to_the_devpod_enum() {
        assert_eq!(map_orgo_status(Some("running")), BackendStatus::Running);
        for stopped in ["frozen", "stopped", "suspended"] {
            assert_eq!(
                map_orgo_status(Some(stopped)),
                BackendStatus::Stopped,
                "{stopped}"
            );
        }
        for busy in [
            "creating",
            "restarting",
            "updating",
            "suspending",
            "resuming",
            "error",
        ] {
            assert_eq!(map_orgo_status(Some(busy)), BackendStatus::Busy, "{busy}");
        }
        assert_eq!(map_orgo_status(Some("deleted")), BackendStatus::NotFound);
        assert_eq!(map_orgo_status(None), BackendStatus::NotFound);
    }

    #[test]
    fn hosted_driver_computers_never_route_to_orgo() {
        let routing = ComputeRouting {
            platform: ComputeProvider::Orgo,
            paid: ComputeProvider::Orgo,
            free: ComputeProvider::Orgo,
            ..ComputeRouting::default()
        };
        assert_eq!(
            routing.provider_for(ComputeClass::HostedDriver),
            ComputeProvider::Incus
        );
        assert_eq!(
            routing.provider_for(ComputeClass::Platform),
            ComputeProvider::Orgo
        );
        assert_eq!(
            ComputeClass::for_owner("platform-drv:cmp_1", TIER_FREE),
            ComputeClass::HostedDriver
        );
        assert_eq!(
            ComputeClass::for_owner("platform:proj_1", TIER_FREE),
            ComputeClass::Platform
        );
        assert_eq!(
            ComputeClass::for_owner("user_1", TIER_PAID),
            ComputeClass::Paid
        );
    }

    // ---- the first-boot script and file writes, run for real ----------------

    #[test]
    fn setup_script_parses_as_bash() {
        let mut child = std::process::Command::new("bash")
            .arg("-n")
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("bash");
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(ORGO_SETUP_SCRIPT.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn push_commands_write_the_exact_bytes_owner_and_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let meta = std::fs::metadata(dir.path()).unwrap();
        // Big enough for several chunks and commands; every byte value,
        // including NUL and quote characters.
        let content: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 256) as u8).collect();
        let path = dir.path().join("new dir/it's here.bin");
        let file = InstanceFile {
            path: path.to_string_lossy().into_owned(),
            content: content.clone(),
            uid: meta.uid(),
            gid: meta.gid(),
            mode: "0640".to_string(),
        };
        let commands = push_commands(&file);
        assert!(
            commands.len() > 1,
            "a 200 KB file needs more than one command"
        );
        for command in &commands {
            assert!(command.len() <= PUSH_COMMAND_MAX + PUSH_CHUNK);
            let output = std::process::Command::new("bash")
                .arg("-c")
                .arg(command)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{command:.200}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), content);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let parent = std::fs::metadata(path.parent().unwrap()).unwrap();
        assert_eq!(
            parent.permissions().mode() & 0o777,
            0o700,
            "a new parent is private"
        );
        assert!(!dir
            .path()
            .join("new dir/it's here.bin.allternit-part")
            .exists());

        // An existing parent keeps its mode (never chmod /etc).
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let small = InstanceFile {
            path: dir.path().join("b.json").to_string_lossy().into_owned(),
            content: b"{\"token\":\"x\"}".to_vec(),
            mode: "0600".to_string(),
            ..file
        };
        for command in push_commands(&small) {
            assert!(std::process::Command::new("bash")
                .arg("-c")
                .arg(&command)
                .status()
                .unwrap()
                .success());
        }
        assert_eq!(
            std::fs::read(dir.path().join("b.json")).unwrap(),
            b"{\"token\":\"x\"}"
        );
        assert_eq!(
            std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    // ---- a fake Orgo API on a local port ------------------------------------

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        auth: Option<String>,
        idempotency: Option<String>,
        body: serde_json::Value,
    }

    #[derive(Debug, Default)]
    struct Fake {
        /// id → (name, status)
        computers: Vec<(String, String, String)>,
        seen: Vec<Seen>,
        bash: Vec<String>,
        /// GETs of a `creating` computer before it reads `running`.
        creating_polls: usize,
        /// State-file polls answered "running" before `setup_result`.
        setup_polls: usize,
        setup_result: String,
        ensure_exit: i64,
        /// Answer the next N requests with 429 (Retry-After: 0).
        rate_limit_next: usize,
        workspace_status: u16,
    }

    type FakeState = Arc<Mutex<Fake>>;

    async fn fake_orgo(
        axum::extract::State(state): axum::extract::State<FakeState>,
        method: Method,
        uri: Uri,
        headers: AxumHeaders,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let json_body: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        let mut fake = state.lock().unwrap();
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        fake.seen.push(Seen {
            method: method.to_string(),
            path: uri.path().to_string(),
            auth: header("authorization"),
            idempotency: header("idempotency-key"),
            body: json_body.clone(),
        });
        if fake.rate_limit_next > 0 {
            fake.rate_limit_next -= 1;
            return (
                StatusCode::TOO_MANY_REQUESTS,
                [("Retry-After", "0")],
                axum::Json(serde_json::json!({ "error": "slow down" })),
            )
                .into_response();
        }
        let reply = |status: StatusCode, value: serde_json::Value| {
            (status, axum::Json(value)).into_response()
        };
        let parts: Vec<&str> = uri.path().trim_start_matches('/').split('/').collect();
        match (method.as_str(), parts.as_slice()) {
            ("GET", ["workspaces", _]) => {
                if fake.workspace_status != 0 && fake.workspace_status != 200 {
                    return reply(
                        StatusCode::from_u16(fake.workspace_status).unwrap(),
                        serde_json::json!({ "error": "no such workspace" }),
                    );
                }
                let desktops: Vec<_> = fake
                    .computers
                    .iter()
                    .map(|(id, name, status)| serde_json::json!({ "id": id, "name": name, "status": status }))
                    .collect();
                reply(
                    StatusCode::OK,
                    serde_json::json!({ "id": "ws_1", "desktops": desktops }),
                )
            }
            ("POST", ["computers"]) => {
                let name = json_body["name"].as_str().unwrap_or("").to_string();
                if fake.computers.iter().any(|(_, n, _)| *n == name) {
                    return reply(
                        StatusCode::CONFLICT,
                        serde_json::json!({ "error": "name taken", "code": "NAME_TAKEN" }),
                    );
                }
                let id = format!("c_{}", fake.computers.len() + 1);
                fake.computers
                    .push((id.clone(), name, "creating".to_string()));
                reply(
                    StatusCode::CREATED,
                    serde_json::json!({ "id": id, "status": "creating" }),
                )
            }
            (verb, ["computers", id, rest @ ..]) => {
                let id = id.to_string();
                let Some(index) = fake.computers.iter().position(|(cid, _, _)| *cid == id) else {
                    return reply(
                        StatusCode::NOT_FOUND,
                        serde_json::json!({ "error": "not found" }),
                    );
                };
                match (verb, rest) {
                    ("GET", []) => {
                        if fake.computers[index].2 == "creating" {
                            if fake.creating_polls == 0 {
                                fake.computers[index].2 = "running".to_string();
                            } else {
                                fake.creating_polls -= 1;
                            }
                        }
                        let status = fake.computers[index].2.clone();
                        reply(
                            StatusCode::OK,
                            serde_json::json!({ "id": id, "status": status }),
                        )
                    }
                    ("DELETE", []) => {
                        fake.computers.remove(index);
                        reply(StatusCode::OK, serde_json::json!({ "deleted": true }))
                    }
                    ("POST", ["stop"]) => {
                        fake.computers[index].2 = "frozen".to_string();
                        reply(StatusCode::OK, serde_json::json!({}))
                    }
                    ("POST", ["start"]) => {
                        fake.computers[index].2 = "running".to_string();
                        reply(StatusCode::OK, serde_json::json!({}))
                    }
                    ("PATCH", ["resize"]) => {
                        if fake.computers[index].2 != "running" {
                            return reply(
                                StatusCode::CONFLICT,
                                serde_json::json!({ "error": "not running" }),
                            );
                        }
                        reply(StatusCode::OK, serde_json::json!({}))
                    }
                    ("PATCH", ["auto-stop"]) => reply(StatusCode::OK, serde_json::json!({})),
                    ("POST", ["bash"]) => {
                        let command = json_body["command"].as_str().unwrap_or("").to_string();
                        fake.bash.push(command.clone());
                        let (exit, output) =
                            if command.starts_with("cat ") && command.contains(SETUP_STATE_PATH) {
                                if fake.setup_polls > 0 {
                                    fake.setup_polls -= 1;
                                    (0, "running\n".to_string())
                                } else {
                                    (0, format!("{}\n", fake.setup_result))
                                }
                            } else if command.contains("allternit-orgo-ensure") {
                                (
                                    fake.ensure_exit,
                                    if fake.ensure_exit == 0 {
                                        "allternit-desktop RUNNING".to_string()
                                    } else {
                                        "no".to_string()
                                    },
                                )
                            } else if command.starts_with("tail ") {
                                (0, "E: Unable to locate package libasound2t64".to_string())
                            } else {
                                (0, String::new())
                            };
                        reply(
                            StatusCode::OK,
                            serde_json::json!({ "success": exit == 0, "output": output, "exit_code": exit }),
                        )
                    }
                    _ => reply(
                        StatusCode::NOT_FOUND,
                        serde_json::json!({ "error": "no route" }),
                    ),
                }
            }
            _ => reply(
                StatusCode::NOT_FOUND,
                serde_json::json!({ "error": "no route" }),
            ),
        }
    }

    async fn serve(fake: Fake) -> (String, FakeState) {
        let state: FakeState = Arc::new(Mutex::new(fake));
        let app = axum::Router::new()
            .fallback(fake_orgo)
            .with_state(state.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (base, state)
    }

    fn test_config() -> OrgoConfig {
        OrgoConfig {
            deb_url: Some("https://downloads.example.com/allternit desktop.deb".to_string()),
            deb_sha256: Some("ab12".to_string()),
            resolution: None,
            auto_stop_paid: Some("30m".to_string()),
            auto_stop_platform: Some("15m".to_string()),
            auto_stop_free: None,
            start_wait: StdDuration::from_secs(5),
            create_wait: StdDuration::from_secs(5),
            setup_timeout: StdDuration::from_secs(5),
            poll_interval: StdDuration::from_millis(5),
        }
    }

    fn backend(base: &str) -> OrgoBackend {
        OrgoBackend::new(base, "sk_live_test", "ws_1", test_config()).unwrap()
    }

    fn paid_spec(name: &str) -> ProvisionSpec {
        ProvisionSpec {
            name: name.to_string(),
            image: "local:allternit-cloud-computer".to_string(),
            cpu_cores: 4,
            cpu_allowance: None,
            cpu_priority: None,
            memory_mb: 16384,
            disk_gb: 40,
            profiles: vec![],
            storage_pool: "default".to_string(),
            user_data: String::new(),
            instance_id: "pi_abc".to_string(),
            class: ComputeClass::Paid,
        }
    }

    fn bootstrap_file() -> InstanceFile {
        InstanceFile {
            path: BOOTSTRAP_PATH.to_string(),
            content: br#"{"token":"t"}"#.to_vec(),
            uid: 0,
            gid: 0,
            mode: "0600".to_string(),
        }
    }

    fn fake_computer(id: &str, name: &str, status: &str) -> (String, String, String) {
        (id.to_string(), name.to_string(), status.to_string())
    }

    #[tokio::test]
    async fn create_then_first_boot_bills_hourly_and_writes_the_bootstrap_before_setup() {
        let (base, fake) = serve(Fake {
            creating_polls: 2,
            setup_polls: 3,
            setup_result: "done".to_string(),
            ..Fake::default()
        })
        .await;
        let orgo = backend(&base);
        orgo.create(&paid_spec("allternit-u-1")).await.unwrap();
        // Orgo already says running, but first boot has not run: Busy, so
        // reconcile neither flips the row to running nor marks it missing.
        fake.lock().unwrap().computers[0].2 = "running".to_string();
        assert_eq!(
            orgo.status("allternit-u-1").await.unwrap(),
            BackendStatus::Busy
        );
        fake.lock().unwrap().computers[0].2 = "creating".to_string();

        orgo.first_boot("allternit-u-1", &[bootstrap_file()])
            .await
            .unwrap();
        assert_eq!(
            orgo.status("allternit-u-1").await.unwrap(),
            BackendStatus::Running
        );

        let fake = fake.lock().unwrap();
        let create = fake
            .seen
            .iter()
            .find(|s| s.method == "POST" && s.path == "/computers")
            .unwrap();
        assert_eq!(create.auth.as_deref(), Some("Bearer sk_live_test"));
        assert_eq!(create.idempotency.as_deref(), Some("pi_abc"));
        assert_eq!(create.body["billing_mode"], "hourly");
        assert_eq!(create.body["workspace_id"], "ws_1");
        assert_eq!(create.body["name"], "allternit-u-1");
        assert_eq!(create.body["ram"], 16);
        assert_eq!(create.body["cpu"], 4);
        assert_eq!(create.body["disk_size_gb"], 40);
        let auto_stop = fake
            .seen
            .iter()
            .find(|s| s.path.ends_with("/auto-stop"))
            .expect("auto-stop set");
        assert_eq!(auto_stop.body["auto_stop_minutes"], 30);

        let position = |needle: &str| {
            fake.bash
                .iter()
                .position(|c| c.contains(needle))
                .unwrap_or_else(|| panic!("no bash call with {needle}"))
        };
        let bootstrap = position(BOOTSTRAP_PATH);
        let launch = position("nohup bash");
        let ensure = fake
            .bash
            .iter()
            .rposition(|c| c.contains("allternit-orgo-ensure"))
            .unwrap();
        assert!(bootstrap < launch && launch < ensure, "{:?}", fake.bash);
        // The .deb URL reaches the script intact (it has a space in it).
        assert!(fake.bash[launch].contains(
            "ALLTERNIT_ORGO_DESKTOP_DEB_URL='https://downloads.example.com/allternit desktop.deb'"
        ));
        assert!(fake.bash[launch].contains("ALLTERNIT_ORGO_DESKTOP_DEB_SHA256='ab12'"));
    }

    #[tokio::test]
    async fn a_failed_setup_reports_its_reason_and_log_and_stops_reading_busy() {
        let (base, fake) = serve(Fake {
            setup_result: "failed: apt could not install the Allternit Desktop .deb".to_string(),
            ..Fake::default()
        })
        .await;
        let orgo = backend(&base);
        orgo.create(&paid_spec("allternit-u-2")).await.unwrap();
        let error = orgo
            .first_boot("allternit-u-2", &[bootstrap_file()])
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("apt could not install"), "{error}");
        assert!(
            error.contains("libasound2t64"),
            "the log tail is in the error: {error}"
        );
        assert_eq!(
            orgo.status("allternit-u-2").await.unwrap(),
            BackendStatus::Running
        );
        assert!(!fake
            .lock()
            .unwrap()
            .bash
            .iter()
            .any(|c| c.contains("allternit-orgo-ensure")));
    }

    #[tokio::test]
    async fn create_without_a_deb_url_buys_nothing() {
        let (base, fake) = serve(Fake::default()).await;
        let orgo = OrgoBackend::new(
            &base,
            "k",
            "ws_1",
            OrgoConfig {
                deb_url: None,
                ..test_config()
            },
        )
        .unwrap();
        assert!(orgo.create(&paid_spec("allternit-u-3")).await.is_err());
        assert!(fake.lock().unwrap().seen.is_empty());
        assert_eq!(
            orgo.status("allternit-u-3").await.unwrap(),
            BackendStatus::NotFound
        );
    }

    #[tokio::test]
    async fn a_restarted_cloud_api_finds_computers_by_name_and_stops_them() {
        let (base, fake) = serve(Fake {
            computers: vec![fake_computer("c_7", "allternit-free-x", "running")],
            ..Fake::default()
        })
        .await;
        // A fresh backend: empty id cache.
        let orgo = backend(&base);
        orgo.stop("allternit-free-x").await.unwrap();
        assert_eq!(
            orgo.status("allternit-free-x").await.unwrap(),
            BackendStatus::Stopped
        );
        assert!(fake
            .lock()
            .unwrap()
            .seen
            .iter()
            .any(|s| s.method == "POST" && s.path == "/computers/c_7/stop"));
        // Stopping a stopped computer is a no-op success.
        orgo.stop("allternit-free-x").await.unwrap();
        assert_eq!(
            fake.lock()
                .unwrap()
                .seen
                .iter()
                .filter(|s| s.path.ends_with("/stop"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn an_unknown_name_is_not_found_but_a_broken_workspace_is_an_error() {
        let (base, fake) = serve(Fake::default()).await;
        let orgo = backend(&base);
        assert_eq!(
            orgo.status("allternit-nope").await.unwrap(),
            BackendStatus::NotFound
        );
        fake.lock().unwrap().workspace_status = 404;
        // Never NotFound: that would mark every computer missing.
        assert!(matches!(
            orgo.status("allternit-nope").await,
            Err(ProvisionError::Api { status: 404, .. })
        ));
    }

    #[tokio::test]
    async fn start_wakes_a_frozen_computer_then_starts_the_app() {
        let (base, fake) = serve(Fake {
            computers: vec![fake_computer("c_1", "allternit-p", "frozen")],
            ..Fake::default()
        })
        .await;
        let orgo = backend(&base);
        orgo.start("allternit-p").await.unwrap();
        {
            let fake = fake.lock().unwrap();
            assert!(fake
                .seen
                .iter()
                .any(|s| s.method == "POST" && s.path == "/computers/c_1/start"));
            assert!(fake.bash.last().unwrap().contains("allternit-orgo-ensure"));
        }
        // The app not coming up is a failed start, not a silent success.
        fake.lock().unwrap().ensure_exit = 3;
        assert!(orgo.start("allternit-p").await.is_err());
    }

    #[tokio::test]
    async fn resize_only_while_running_with_the_mapped_size() {
        let (base, fake) = serve(Fake {
            computers: vec![fake_computer("c_1", "allternit-r", "frozen")],
            ..Fake::default()
        })
        .await;
        let orgo = backend(&base);
        let target = ComputerSize {
            cpu_cores: 4,
            memory_mb: 16384,
            disk_gb: 60,
        };
        let error = orgo
            .resize("allternit-r", target, "default")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("only while it runs"), "{error}");
        assert!(!fake
            .lock()
            .unwrap()
            .seen
            .iter()
            .any(|s| s.path.ends_with("/resize")));

        fake.lock().unwrap().computers[0].2 = "running".to_string();
        orgo.resize("allternit-r", target, "default").await.unwrap();
        let fake = fake.lock().unwrap();
        let resize = fake
            .seen
            .iter()
            .find(|s| s.path == "/computers/c_1/resize")
            .unwrap();
        assert_eq!(resize.method, "PATCH");
        assert_eq!(
            (
                resize.body["cpu"].clone(),
                resize.body["ram"].clone(),
                resize.body["disk_size_gb"].clone()
            ),
            (
                serde_json::json!(4),
                serde_json::json!(16),
                serde_json::json!(60)
            )
        );
    }

    #[tokio::test]
    async fn a_taken_name_is_adopted_and_delete_removes_that_computer() {
        let (base, fake) = serve(Fake {
            computers: vec![fake_computer("c_9", "allternit-u-4", "running")],
            ..Fake::default()
        })
        .await;
        let orgo = backend(&base);
        orgo.create(&paid_spec("allternit-u-4")).await.unwrap();
        orgo.delete("allternit-u-4").await.unwrap();
        assert!(fake.lock().unwrap().computers.is_empty());
        assert!(matches!(
            orgo.delete("allternit-u-4").await,
            Err(ProvisionError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn rate_limits_are_retried() {
        let (base, _fake) = serve(Fake {
            computers: vec![fake_computer("c_1", "allternit-q", "running")],
            rate_limit_next: 2,
            ..Fake::default()
        })
        .await;
        assert_eq!(
            backend(&base).status("allternit-q").await.unwrap(),
            BackendStatus::Running
        );
    }
}
