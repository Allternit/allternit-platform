//! The peer listener: how another computer's Factory engine reaches this one
//! (bots on another computer, phase 1; design agreed 2026-10-07).
//!
//! A paired computer runs `serve --peer-port 3019`. The listener binds
//! 127.0.0.1 only; `allternit computers serve` forwards the port over the
//! Allternit mesh beside VNC, so nothing is exposed outside the mesh.
//!
//! Every call carries a peer ticket: a data-plane JWT minted by cloud-api
//! (`POST /api/v1/computers/paired/:id/peer-ticket`) for the computer's owner
//! or a member of its organization. The engine verifies it offline against
//! cloud-api's published key (`GET /api/v1/auth/dp-jwks`): EdDSA signature,
//! `aud` = this computer's id, scope `factory:peer`, and the time window.
//! The loopback API (`/api/factory`) is unchanged and never served here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::service::ServiceState;

/// The only capability a peer ticket may carry.
pub const PEER_SCOPE: &str = "factory:peer";
/// The port a paired computer's engine takes peer calls on.
pub const DEFAULT_PEER_PORT: u16 = 3019;
/// Clock skew allowed on `exp` / `nbf`, as cloud-api allows.
const LEEWAY_SECS: u64 = 60;
/// How long fetched keys are trusted before a refresh; an unknown `kid`
/// refreshes at once (key rotation), at most once a minute.
const KEYS_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const REFETCH_FLOOR: Duration = Duration::from_secs(60);

/// What the peer listener needs: who this computer is and where to get keys.
#[derive(Debug, Clone)]
pub struct PeerOptions {
    pub port: u16,
    pub computer_id: String,
    /// `https://api.allternit.com/api/v1/auth/dp-jwks` by default.
    pub jwks_url: String,
    /// Expected `iss`; cloud-api's default is `allternit-cloud-api`.
    pub issuer: String,
}

/// The paired computer's config, written by `allternit computer pair`
/// (`~/.allternit/computer/paired.json`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairedConfig {
    computer_id: String,
    cloud_url: String,
}

pub fn paired_config_path(home: &Path) -> PathBuf {
    home.join(".allternit/computer/paired.json")
}

impl PeerOptions {
    /// Options for `port`: the computer id and cloud from
    /// `$ALLTERNIT_FACTORY_COMPUTER_ID` / `$ALLTERNIT_CLOUD_URL`, else from
    /// this computer's pairing config. Not paired: an error saying so.
    pub fn resolve(port: u16, home: Option<&Path>) -> Result<Self> {
        let paired = home.map(paired_config_path).and_then(|p| {
            let text = std::fs::read_to_string(&p).ok()?;
            serde_json::from_str::<PairedConfig>(&text).ok()
        });
        let computer_id = std::env::var("ALLTERNIT_FACTORY_COMPUTER_ID")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| paired.as_ref().map(|p| p.computer_id.clone()))
            .ok_or_else(|| {
                anyhow!("this computer is not paired, so it has no computer id for peer calls (run `allternit computer pair <code>`)")
            })?;
        let cloud = std::env::var("ALLTERNIT_CLOUD_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .or_else(|| paired.as_ref().map(|p| p.cloud_url.clone()))
            .unwrap_or_else(|| "https://api.allternit.com".to_string());
        let issuer = std::env::var("ALLTERNIT_DP_JWT_ISSUER").unwrap_or_else(|_| "allternit-cloud-api".to_string());
        Ok(Self {
            port,
            computer_id,
            jwks_url: format!("{}/api/v1/auth/dp-jwks", cloud.trim_end_matches('/')),
            issuer,
        })
    }
}

/// The caller a valid ticket names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCaller {
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: String,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    scope: String,
}

/// Verify a ticket's shape, claims and signature with `keys` (kid → key).
/// Pure, so tests need no network.
pub fn verify_ticket(
    token: &str,
    keys: &HashMap<String, VerifyingKey>,
    computer_id: &str,
    issuer: &str,
    now: u64,
) -> Result<PeerCaller> {
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        bail!("not a ticket");
    };
    let header: Header = serde_json::from_slice(&B64.decode(h).context("ticket header")?).context("ticket header")?;
    if header.alg != "EdDSA" {
        bail!("ticket algorithm {} is not accepted", header.alg);
    }
    let key = match &header.kid {
        Some(kid) => keys.get(kid).ok_or_else(|| anyhow!("unknown signing key"))?,
        None if keys.len() == 1 => keys.values().next().expect("one key"),
        None => bail!("ticket names no signing key"),
    };
    let signature = Signature::from_slice(&B64.decode(s).context("ticket signature")?).context("ticket signature")?;
    key.verify(format!("{h}.{p}").as_bytes(), &signature).map_err(|_| anyhow!("bad ticket signature"))?;
    let claims: Claims = serde_json::from_slice(&B64.decode(p).context("ticket claims")?).context("ticket claims")?;
    if claims.iss != issuer {
        bail!("ticket from an unexpected issuer");
    }
    if claims.aud != computer_id {
        bail!("ticket is for another computer");
    }
    if claims.scope != PEER_SCOPE {
        bail!("ticket scope {} is not {PEER_SCOPE}", claims.scope);
    }
    if claims.exp + LEEWAY_SECS < now {
        bail!("ticket expired");
    }
    if claims.nbf.is_some_and(|nbf| nbf > now + LEEWAY_SECS) {
        bail!("ticket not valid yet");
    }
    if claims.sub.trim().is_empty() {
        bail!("ticket names no user");
    }
    Ok(PeerCaller { user_id: claims.sub })
}

/// Parse a JWK set (`{"keys":[{"kid","x",...}]}`) into kid → key.
pub fn parse_jwks(body: &serde_json::Value) -> Result<HashMap<String, VerifyingKey>> {
    let mut out = HashMap::new();
    for k in body["keys"].as_array().ok_or_else(|| anyhow!("no keys in the key set"))? {
        if k["kty"].as_str() != Some("OKP") || k["crv"].as_str() != Some("Ed25519") {
            continue;
        }
        let (Some(kid), Some(x)) = (k["kid"].as_str(), k["x"].as_str()) else { continue };
        let bytes: [u8; 32] = B64.decode(x)?.as_slice().try_into().map_err(|_| anyhow!("key {kid} is not 32 bytes"))?;
        out.insert(kid.to_string(), VerifyingKey::from_bytes(&bytes)?);
    }
    if out.is_empty() {
        bail!("the key set has no Ed25519 keys");
    }
    Ok(out)
}

struct KeyCache {
    keys: HashMap<String, VerifyingKey>,
    fetched: Option<Instant>,
}

/// Verifies tickets, fetching cloud-api's keys when needed.
pub struct PeerVerifier {
    opts: PeerOptions,
    cache: RwLock<KeyCache>,
    http: reqwest::Client,
    refetch_floor: Duration,
}

impl PeerVerifier {
    pub fn new(opts: PeerOptions) -> Self {
        Self {
            opts,
            cache: RwLock::new(KeyCache { keys: HashMap::new(), fetched: None }),
            http: reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default(),
            refetch_floor: REFETCH_FLOOR,
        }
    }

    /// The least time between key fetches forced by an unknown `kid`
    /// (default a minute, so a burst of bad tickets can't hammer cloud-api).
    pub fn with_refetch_floor(mut self, floor: Duration) -> Self {
        self.refetch_floor = floor;
        self
    }

    /// For tests: a verifier with fixed keys that never fetches.
    pub fn with_keys(opts: PeerOptions, keys: HashMap<String, VerifyingKey>) -> Self {
        let v = Self::new(opts);
        v.cache.try_write().expect("fresh lock").keys = keys;
        v.cache.try_write().expect("fresh lock").fetched = Some(Instant::now());
        v
    }

    pub fn computer_id(&self) -> &str {
        &self.opts.computer_id
    }

    async fn refresh(&self, force: bool) -> Result<()> {
        {
            let cache = self.cache.read().await;
            if let Some(at) = cache.fetched {
                let age = at.elapsed();
                if (!force && age < KEYS_TTL) || (force && age < self.refetch_floor) {
                    return Ok(());
                }
            }
        }
        let body: serde_json::Value = self
            .http
            .get(&self.opts.jwks_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .with_context(|| format!("fetching {}", self.opts.jwks_url))?
            .json()
            .await?;
        let keys = parse_jwks(&body)?;
        let mut cache = self.cache.write().await;
        cache.keys = keys;
        cache.fetched = Some(Instant::now());
        Ok(())
    }

    pub async fn verify(&self, token: &str) -> Result<PeerCaller> {
        self.refresh(false).await?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        let first = {
            let cache = self.cache.read().await;
            verify_ticket(token, &cache.keys, &self.opts.computer_id, &self.opts.issuer, now)
        };
        match first {
            // A key rotated since the last fetch: fetch again, once.
            Err(e) if e.to_string() == "unknown signing key" => {
                self.refresh(true).await?;
                let cache = self.cache.read().await;
                verify_ticket(token, &cache.keys, &self.opts.computer_id, &self.opts.issuer, now)
            }
            other => other,
        }
    }
}

#[derive(Clone)]
struct PeerState {
    service: Arc<ServiceState>,
    verifier: Arc<PeerVerifier>,
}

fn refusal(status: StatusCode, code: &str, fact: impl Into<String>, action: &str) -> Response {
    (status, Json(json!({ "error": { "code": code, "fact": fact.into(), "action": action } }))).into_response()
}

/// Every peer call needs a valid ticket; the caller rides along as
/// [`PeerCaller`] and the `x-allternit-user` header the engine's handlers read.
async fn require_ticket(State(st): State<PeerState>, mut req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let Some(token) = token else {
        return refusal(StatusCode::UNAUTHORIZED, "refused", "peer calls need a ticket", "Ask cloud-api for one: POST /api/v1/computers/paired/<id>/peer-ticket.");
    };
    match st.verifier.verify(&token).await {
        Ok(caller) => {
            // Whatever the caller sent, the user is the ticket's.
            req.headers_mut().remove("x-allternit-user");
            if let Ok(v) = caller.user_id.parse() {
                req.headers_mut().insert("x-allternit-user", v);
            }
            req.extensions_mut().insert(caller);
            next.run(req).await
        }
        Err(e) if e.to_string().starts_with("fetching ") => refusal(
            StatusCode::BAD_GATEWAY,
            "transport",
            format!("this computer can't check tickets right now: {e:#}"),
            "Check this computer's internet connection, then retry.",
        ),
        Err(e) => refusal(StatusCode::FORBIDDEN, "refused", format!("{e:#}"), "Get a fresh ticket for this computer and retry."),
    }
}

async fn hello(State(st): State<PeerState>, Extension(caller): Extension<PeerCaller>) -> Response {
    let _ = &st.service;
    Json(json!({
        "computerId": st.verifier.computer_id(),
        "engine": env!("CARGO_PKG_VERSION"),
        "user": caller.user_id,
    }))
    .into_response()
}

/// The routes another engine may call: `hello`, then team up/down, send,
/// capture and the bot list, all for the bots that caller placed here.
pub fn peer_router(service: Arc<ServiceState>, verifier: Arc<PeerVerifier>) -> Router {
    let st = PeerState { service, verifier };
    Router::new()
        .route("/api/factory/peer/hello", get(hello))
        .route("/api/factory/peer/teams/:name/up", axum::routing::post(remote::team_up))
        .route("/api/factory/peer/teams/:name/down", axum::routing::post(remote::team_down))
        .route("/api/factory/peer/send", axum::routing::post(remote::send))
        .route("/api/factory/peer/stop", axum::routing::post(remote::stop))
        .route("/api/factory/peer/capture", get(remote::capture))
        .route("/api/factory/peer/agents", get(remote::agents))
        .route("/api/factory/peer/screen", get(remote::screen))
        .route("/api/factory/peer/stream", get(remote::stream))
        .route("/api/factory/peer/input", axum::routing::post(remote::input))
        .layer(middleware::from_fn_with_state(st.clone(), require_ticket))
        .with_state(st)
}

/// Bots another computer's engine runs here (phase 2).
///
/// Each caller (the ticket's user) gets its own workspace per team,
/// `<factory home>/remote-work/<user>/<team>`, and may only touch the panes
/// started in it. A team arrives as its folder's files (team.yaml,
/// CULTURE.md, bots/…); with a repo, the workspace is a clone of it at the
/// team's commit and the bots work there, so results go back as a branch.
pub mod remote {
    use super::*;
    use crate::agents::team;
    use crate::agents::team_apply::{self, ApplyOptions, StepOutcome};
    use crate::agents::team_plan::plan_up;
    use axum::extract::{Path as AxPath, Query};
    use std::collections::BTreeMap;

    const MAX_FILES: usize = 200;
    const MAX_BYTES: usize = 4 * 1024 * 1024;

    fn safe_segment(s: &str) -> String {
        let out: String = s
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' })
            .take(80)
            .collect();
        if out.is_empty() || out.chars().all(|c| c == '.') { "_".into() } else { out }
    }

    /// This caller's workspace for `team` (created by `up`).
    pub fn remote_root(user: &str, team_name: &str) -> PathBuf {
        crate::agents::registry::factory_home().join("remote-work").join(safe_segment(user)).join(safe_segment(team_name))
    }

    /// Where the bots work: the repo clone when the team came with a repo.
    fn work_root(user: &str, team_name: &str) -> PathBuf {
        let base = remote_root(user, team_name);
        let repo = base.join("repo");
        if repo.join(".git").exists() { repo } else { base }
    }

    /// A path inside the team folder: relative, no `..`, no absolute parts.
    pub fn safe_rel(rel: &str) -> Option<PathBuf> {
        let p = Path::new(rel);
        if rel.is_empty() || p.is_absolute() {
            return None;
        }
        let mut out = PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::Normal(x) => out.push(x),
                _ => return None,
            }
        }
        Some(out)
    }

    /// Only repos another computer can reach on its own: https or ssh.
    pub fn allowed_repo_url(url: &str) -> bool {
        (url.starts_with("https://") || url.starts_with("git@") || url.starts_with("ssh://")) && !url.contains(char::is_whitespace)
    }

    fn git(dir: &Path, args: &[&str]) -> Result<()> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .context("running git")?;
        if !out.status.success() {
            bail!("git {}: {}", args.first().copied().unwrap_or(""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    /// Clone `url` into `base/repo` (once) and check out `commit`.
    fn checkout(base: &Path, url: &str, commit: &str) -> Result<PathBuf> {
        if !commit.chars().all(|c| c.is_ascii_hexdigit()) || !(7..=64).contains(&commit.len()) {
            bail!("commit must be a hex commit id");
        }
        let repo = base.join("repo");
        if !repo.join(".git").exists() {
            std::fs::create_dir_all(base)?;
            git(base, &["clone", "--quiet", url, "repo"])?;
        }
        if git(&repo, &["checkout", "--quiet", "--detach", commit]).is_err() {
            git(&repo, &["fetch", "--quiet", "origin", commit])?;
            git(&repo, &["checkout", "--quiet", "--detach", commit])?;
        }
        // The team folder and the engine's state live in the clone; keep them
        // out of what the bots commit.
        let exclude = repo.join(".git/info/exclude");
        let current = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !current.lines().any(|l| l.trim() == ".allternit/") {
            if let Some(dir) = exclude.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&exclude, format!("{current}{}.allternit/\n", if current.is_empty() || current.ends_with('\n') { "" } else { "\n" }))?;
        }
        Ok(repo)
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Repo {
        pub url: String,
        pub commit: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpBody {
        /// The team folder: relative path → text.
        pub files: BTreeMap<String, String>,
        /// The bots placed on this computer (slugs).
        pub bots: Vec<String>,
        #[serde(default)]
        pub preset: Option<String>,
        #[serde(default)]
        pub repo: Option<Repo>,
        #[serde(default)]
        pub dry_run: bool,
    }

    fn bad(fact: impl Into<String>) -> Response {
        refusal(StatusCode::BAD_REQUEST, "usage", fact, "Fix the request and retry.")
    }

    pub async fn team_up(
        Extension(caller): Extension<PeerCaller>,
        AxPath(name): AxPath<String>,
        Json(body): Json<UpBody>,
    ) -> Response {
        if !team::valid_slug(&name) {
            return bad(format!("invalid team name {name:?}"));
        }
        if body.files.len() > MAX_FILES || body.files.values().map(String::len).sum::<usize>() > MAX_BYTES {
            return bad(format!("a team folder may have at most {MAX_FILES} files and {MAX_BYTES} bytes"));
        }
        if !body.files.contains_key(team::TEAM_FILE) {
            return bad(format!("the team folder needs {}", team::TEAM_FILE));
        }
        if body.bots.is_empty() {
            return bad("name the bots placed on this computer");
        }
        if let Some(r) = &body.repo {
            if !allowed_repo_url(&r.url) {
                return bad("the repo must be an https or ssh git URL this computer can clone");
            }
        }
        if let Some(rel) = body.files.keys().find(|k| safe_rel(k).is_none()) {
            return bad(format!("bad file path {rel:?}"));
        }
        let user = caller.user_id.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<Response, Response> {
            let base = remote_root(&user, &name);
            let root = match &body.repo {
                Some(r) if !body.dry_run => checkout(&base, &r.url, &r.commit).map_err(|e| {
                    refusal(StatusCode::CONFLICT, "refused", format!("checking out the team's repo here: {e:#}"), "Check that this computer can clone the repo (its git credentials), then retry.")
                })?,
                _ => work_root(&user, &name),
            };
            let dir = team::team_dir(&root, &name);
            let mut checked = Vec::with_capacity(body.files.len());
            for (rel, text) in &body.files {
                checked.push((safe_rel(rel).ok_or_else(|| bad(format!("bad file path {rel:?}")))?, text));
            }
            for (rel, text) in checked {
                if body.dry_run {
                    continue;
                }
                let path = dir.join(rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| bad(format!("writing the team folder: {e}")))?;
                }
                std::fs::write(&path, text).map_err(|e| bad(format!("writing the team folder: {e}")))?;
            }
            let loaded = if body.dry_run {
                team::parse_team(&name, &body.files[team::TEAM_FILE])
            } else {
                team::load_team(&root, &name)
            }
            .map_err(|e| bad(e.to_string()))?;
            let preset = loaded.resolve_preset(body.preset.as_deref()).map_err(|e| bad(e.to_string()))?;
            let live = team_apply::live_state(&root, &loaded).map_err(|e| {
                refusal(StatusCode::BAD_GATEWAY, "transport", format!("{e:#}"), "Start the pane engine on this computer and retry.")
            })?;
            let wanted: Vec<String> = body.bots.iter().map(|b| team::address(b, &name)).collect();
            // Only the bots placed here, and they run here: no machine.
            let plan: Vec<_> = plan_up(&loaded, preset.as_deref(), None, &live)
                .map_err(|e| bad(e.to_string()))?
                .into_iter()
                .filter(|s| wanted.contains(&s.agent))
                .map(|mut s| {
                    s.machine = None;
                    s
                })
                .collect();
            if body.dry_run {
                return Ok(Json(json!({ "plan": plan, "applied": false, "root": root })).into_response());
            }
            team_apply::check_workdirs(&loaded, preset.as_deref(), &plan, &root, &live)
                .map_err(|fact| refusal(StatusCode::FORBIDDEN, "refused", fact, "Nothing was started here."))?;
            let results = team_apply::apply(&root, &loaded, preset.as_deref(), &plan, &ApplyOptions::default());
            let failed = results.iter().any(|r| r.outcome == StepOutcome::Failed);
            let body = json!({ "plan": plan, "applied": true, "results": results, "root": root });
            Ok(if failed { (StatusCode::CONFLICT, Json(body)).into_response() } else { Json(body).into_response() })
        })
        .await;
        match res {
            Ok(Ok(r)) | Ok(Err(r)) => r,
            Err(e) => refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("team up failed: {e}"), "Retry."),
        }
    }

    pub async fn team_down(Extension(caller): Extension<PeerCaller>, AxPath(name): AxPath<String>) -> Response {
        if !team::valid_slug(&name) {
            return bad(format!("invalid team name {name:?}"));
        }
        let root = work_root(&caller.user_id, &name);
        if !team::team_dir(&root, &name).join(team::TEAM_FILE).exists() {
            return refusal(StatusCode::NOT_FOUND, "not_found", format!("no team {name} from you on this computer"), "Start it with up first.");
        }
        crate::agents::http::down(root, name, None).await
    }

    /// The session of `bot@team` started by this caller here, or why not.
    fn owned_session(user: &str, address: &str) -> std::result::Result<String, Response> {
        let Some((bot, team_name)) = address.split_once('@') else {
            return Err(bad("address a bot as bot@team"));
        };
        let session = crate::agents::spawn::session_name(&team_apply::pane_slug(bot, team_name));
        let root = work_root(user, team_name);
        let entry = crate::agents::registry::Registry::open_default()
            .load()
            .ok()
            .and_then(|f| f.sessions.get(&session).cloned());
        match entry {
            Some(e) if Path::new(&e.cwd).starts_with(&root) => Ok(session),
            _ => Err(refusal(StatusCode::NOT_FOUND, "not_found", format!("no bot {address} from you on this computer"), "Start it with up first.")),
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct SendBody {
        pub to: String,
        pub text: String,
        #[serde(default)]
        pub queue: bool,
    }

    pub async fn send(Extension(caller): Extension<PeerCaller>, Json(body): Json<SendBody>) -> Response {
        let session = match owned_session(&caller.user_id, &body.to) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let team_name = body.to.split_once('@').map(|(_, t)| t.to_string()).unwrap_or_default();
        let root = work_root(&caller.user_id, &team_name);
        let sender = format!("user:{}", caller.user_id);
        let res = crate::agents::backend::blocking(move || {
            crate::agents::backend::backend()?.send(&root, &session, &body.text, &sender, body.queue)
        })
        .await;
        match res {
            Ok(crate::agents::backend::PaneSend::Verified) => Json(json!({ "to": body.to, "state": "verified" })).into_response(),
            Ok(crate::agents::backend::PaneSend::Queued { message_id, depth, reason }) => {
                Json(json!({ "to": body.to, "state": "queued", "messageId": message_id, "depth": depth, "reason": reason })).into_response()
            }
            Err(e) => refusal(StatusCode::BAD_GATEWAY, "transport", format!("{e:#}"), "Start the pane engine on this computer and retry."),
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct StopBody {
        pub to: String,
    }

    /// Stop one of this caller's bots here (close its pane).
    pub async fn stop(Extension(caller): Extension<PeerCaller>, Json(body): Json<StopBody>) -> Response {
        let session = match owned_session(&caller.user_id, &body.to) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let s2 = session.clone();
        match crate::agents::backend::blocking(move || crate::agents::backend::backend()?.kill(&s2)).await {
            Ok(()) => {
                let _ = crate::agents::registry::Registry::open_default().update(|f| {
                    if let Some(e) = f.sessions.get_mut(&session) {
                        e.dead = true;
                        e.lifecycle = Some("dead".into());
                    }
                });
                Json(json!({ "stopped": [body.to] })).into_response()
            }
            Err(e) => refusal(StatusCode::BAD_GATEWAY, "transport", format!("{e:#}"), "Start the pane engine on this computer and retry."),
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct CaptureQuery {
        pub to: String,
        #[serde(default)]
        pub lines: Option<u32>,
    }

    pub async fn capture(Extension(caller): Extension<PeerCaller>, Query(q): Query<CaptureQuery>) -> Response {
        let session = match owned_session(&caller.user_id, &q.to) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let lines = q.lines.unwrap_or(25).min(2000);
        let s2 = session.clone();
        match crate::agents::backend::blocking(move || crate::agents::backend::backend()?.capture(&s2, lines)).await {
            Ok(text) => Json(json!({ "to": q.to, "session": session, "lines": lines, "text": text })).into_response(),
            Err(e) => refusal(StatusCode::NOT_FOUND, "not_found", format!("{e:#}"), "The pane is not live."),
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct ToQuery {
        pub to: String,
    }

    /// One read of the bot's visible screen: `{ansi, revision, at}`.
    pub async fn screen(Extension(caller): Extension<PeerCaller>, Query(q): Query<ToQuery>) -> Response {
        let session = match owned_session(&caller.user_id, &q.to) {
            Ok(s) => s,
            Err(r) => return r,
        };
        match crate::api::factory::read_screen(session).await {
            Ok(s) => Json(json!({ "ansi": s.ansi, "revision": s.revision, "at": chrono::Utc::now().to_rfc3339() })).into_response(),
            Err(e) => refusal(StatusCode::NOT_FOUND, "not_found", format!("{e:#}"), "The pane is not live."),
        }
    }

    /// The bot's live screen as SSE, the same stream `/api/factory/agents/:id/stream`
    /// serves for a local bot (phase 3: remote panes on the wall).
    pub async fn stream(Extension(caller): Extension<PeerCaller>, Query(q): Query<ToQuery>) -> Response {
        match owned_session(&caller.user_id, &q.to) {
            Ok(session) => crate::api::factory::screen_sse(session),
            Err(r) => r,
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct InputBody {
        pub to: String,
        #[serde(flatten)]
        pub(crate) input: crate::api::factory::InputBody,
    }

    /// Keystrokes into the bot's pane, as at the wall. Not a send.
    pub async fn input(Extension(caller): Extension<PeerCaller>, Json(body): Json<InputBody>) -> Response {
        if body.input.text.is_empty() && body.input.keys.is_empty() {
            return bad("nothing to type: text and keys are both empty");
        }
        let session = match owned_session(&caller.user_id, &body.to) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let (text, keys) = (body.input.text, body.input.keys);
        match crate::agents::backend::blocking(move || crate::agents::backend::backend()?.input(&session, &text, &keys)).await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => refusal(StatusCode::BAD_GATEWAY, "transport", format!("{e:#}"), "Start the pane engine on this computer and retry."),
        }
    }

    #[derive(Debug, Deserialize)]
    pub struct AgentsQuery {
        pub team: String,
    }

    /// This caller's bots of `team` here: address, session, live or not.
    pub async fn agents(Extension(caller): Extension<PeerCaller>, Query(q): Query<AgentsQuery>) -> Response {
        let root = work_root(&caller.user_id, &q.team);
        let team_name = q.team.clone();
        let res = crate::agents::backend::blocking(move || {
            let file = crate::agents::registry::Registry::open_default().load()?;
            let live: Vec<String> = crate::agents::backend::backend()
                .and_then(|b| b.list())
                .map(|l| l.into_iter().map(|p| p.session).collect())
                .unwrap_or_default();
            let rows: Vec<Value> = file
                .sessions
                .iter()
                .filter(|(_, e)| Path::new(&e.cwd).starts_with(&root))
                .map(|(session, e)| {
                    let slug = crate::agents::registry::slug_of(session);
                    let bot = slug.strip_suffix(&format!("-{team_name}")).unwrap_or(slug);
                    json!({
                        "session": session,
                        "address": team::address(bot, &team_name),
                        "harness": e.harness,
                        "live": live.contains(session) && !e.dead,
                    })
                })
                .collect();
            Ok(rows)
        })
        .await;
        match res {
            Ok(rows) => Json(json!({ "agents": rows })).into_response(),
            Err(e) => refusal(StatusCode::BAD_GATEWAY, "transport", format!("{e:#}"), "Start the pane engine on this computer and retry."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn opts() -> PeerOptions {
        PeerOptions {
            port: DEFAULT_PEER_PORT,
            computer_id: "pc_1".into(),
            jwks_url: "http://unused.invalid/jwks".into(),
            issuer: "allternit-cloud-api".into(),
        }
    }

    pub(crate) fn mint(key: &SigningKey, kid: &str, claims: serde_json::Value) -> String {
        let h = B64.encode(serde_json::to_vec(&json!({ "alg": "EdDSA", "typ": "JWT", "kid": kid })).unwrap());
        let p = B64.encode(serde_json::to_vec(&claims).unwrap());
        let sig = key.sign(format!("{h}.{p}").as_bytes());
        format!("{h}.{p}.{}", B64.encode(sig.to_bytes()))
    }

    fn claims(aud: &str, scope: &str, exp: u64) -> serde_json::Value {
        json!({ "iss": "allternit-cloud-api", "sub": "u_mate", "aud": aud, "iat": 1000, "nbf": 1000, "exp": exp, "scope": scope, "jti": "j" })
    }

    #[test]
    fn a_ticket_for_this_computer_names_its_user() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let keys = HashMap::from([("k1".to_string(), key.verifying_key())]);
        let t = mint(&key, "k1", claims("pc_1", PEER_SCOPE, 2000));
        assert_eq!(verify_ticket(&t, &keys, "pc_1", "allternit-cloud-api", 1500).unwrap().user_id, "u_mate");
    }

    #[test]
    fn wrong_computer_scope_time_key_or_signature_is_refused() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let other = SigningKey::from_bytes(&[4u8; 32]);
        let keys = HashMap::from([("k1".to_string(), key.verifying_key())]);
        let check = |t: &str| verify_ticket(t, &keys, "pc_1", "allternit-cloud-api", 1500).unwrap_err().to_string();
        assert_eq!(check(&mint(&key, "k1", claims("pc_2", PEER_SCOPE, 2000))), "ticket is for another computer");
        assert!(check(&mint(&key, "k1", claims("pc_1", "runtime:execute", 2000))).contains("scope"));
        assert_eq!(check(&mint(&key, "k1", claims("pc_1", PEER_SCOPE, 1400))), "ticket expired");
        assert_eq!(check(&mint(&key, "k9", claims("pc_1", PEER_SCOPE, 2000))), "unknown signing key");
        assert_eq!(check(&mint(&other, "k1", claims("pc_1", PEER_SCOPE, 2000))), "bad ticket signature");
        assert_eq!(check("a.b"), "not a ticket");
    }

    #[test]
    fn jwks_parses_cloud_api_shape() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let body = json!({ "keys": [{ "alg": "EdDSA", "crv": "Ed25519", "kid": "k1", "kty": "OKP", "use": "sig",
            "x": B64.encode(key.verifying_key().to_bytes()) }] });
        assert_eq!(parse_jwks(&body).unwrap()["k1"], key.verifying_key());
        assert!(parse_jwks(&json!({ "keys": [] })).is_err());
    }

    #[test]
    fn options_come_from_the_pairing_config() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".allternit/computer")).unwrap();
        std::fs::write(
            paired_config_path(home.path()),
            r#"{"computerId":"pc_9","secret":"s","cloudUrl":"https://cloud.test/","meshIp":null}"#,
        )
        .unwrap();
        let o = PeerOptions::resolve(3019, Some(home.path())).unwrap();
        assert_eq!(o.computer_id, "pc_9");
        assert_eq!(o.jwks_url, "https://cloud.test/api/v1/auth/dp-jwks");
        let empty = tempfile::tempdir().unwrap();
        if std::env::var_os("ALLTERNIT_FACTORY_COMPUTER_ID").is_none() {
            assert!(PeerOptions::resolve(3019, Some(empty.path())).unwrap_err().to_string().contains("not paired"));
        }
    }

    #[test]
    fn team_files_stay_inside_and_repos_are_reachable() {
        use super::remote::{allowed_repo_url, safe_rel};
        assert_eq!(safe_rel("bots/a/PERSONA.md"), Some(PathBuf::from("bots/a/PERSONA.md")));
        for bad in ["", "/etc/passwd", "../x", "bots/../../x", "./x/../.."] {
            assert!(safe_rel(bad).is_none(), "{bad}");
        }
        assert!(allowed_repo_url("https://github.com/a/b.git"));
        assert!(allowed_repo_url("git@github.com:a/b.git"));
        for bad in ["file:///etc", "/Users/x/repo", "https://x y", "ext::sh -c x"] {
            assert!(!allowed_repo_url(bad), "{bad}");
        }
    }

    #[test]
    fn verifier_with_keys_checks_offline() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let v = PeerVerifier::with_keys(opts(), HashMap::from([("k1".to_string(), key.verifying_key())]));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let t = mint(&key, "k1", claims("pc_1", PEER_SCOPE, now + 300));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(v.verify(&t)).unwrap().user_id, "u_mate");
    }
}
