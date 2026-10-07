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
use serde_json::json;
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

/// The routes another engine may call. Phase 1 is `hello`; phase 2 adds
/// team up/down, send and capture for the bots placed on this computer.
pub fn peer_router(service: Arc<ServiceState>, verifier: Arc<PeerVerifier>) -> Router {
    let st = PeerState { service, verifier };
    Router::new()
        .route("/api/factory/peer/hello", get(hello))
        .layer(middleware::from_fn_with_state(st.clone(), require_ticket))
        .with_state(st)
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
    fn verifier_with_keys_checks_offline() {
        let key = SigningKey::from_bytes(&[3u8; 32]);
        let v = PeerVerifier::with_keys(opts(), HashMap::from([("k1".to_string(), key.verifying_key())]));
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let t = mint(&key, "k1", claims("pc_1", PEER_SCOPE, now + 300));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(v.verify(&t)).unwrap().user_id, "u_mate");
    }
}
