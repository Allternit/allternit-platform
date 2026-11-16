//! Freeweights entitlement claim (freeweights `spec/accounts-v1.md` section 3).
//!
//! Freeweights (freeweights.allternit.com) signs people in with their Allternit Clerk account
//! and accepts Allternit `alt_` keys. Its Worker never holds a plan, perk or price: it asks this
//! route for a short-lived signed claim and verifies it with the public key bundled in the
//! freeweights repo (`spec/keys/entitlement-v1.pub`).
//!
//! - `GET /api/v1/freeweights/entitlement` — the caller's Clerk session JWT or `alt_` key in
//!   `Authorization: Bearer` (the usual `auth::resolve_user` path; a revoked or expired key is
//!   refused there). `200` with `Content-Type: application/jose` and a compact EdDSA JWS:
//!   header `{"alg":"EdDSA","typ":"JWT","kid":"entitlement-v1"}`, payload [`EntitlementClaim`].
//!   No credential → `401`. Signing key unset or unreadable →
//!   `503 {"error":"accounts_not_configured"}` (the Worker reads it as "accounts not set up
//!   yet"); the server still boots. The plan lookup or Clerk failing →
//!   `503 {"error":"entitlement_unavailable"}` with `Retry-After` ("cloud-api down").
//!
//! Configuration (env):
//! - `FREEWEIGHTS_ENTITLEMENT_SIGNING_KEY` — the Ed25519 private key: a PKCS#8 PEM
//!   (`openssl genpkey -algorithm ed25519`; literal `\n` escapes are accepted) or the raw
//!   32-byte seed as base64 or hex. Its public half is `entitlement-v1.pub` in the freeweights
//!   repo (`openssl pkey -in key.pem -pubout`).
//! - `FREEWEIGHTS_PERKS_ENABLED` — `1`/`true` turns perks on. Default off: every `perks` is
//!   `{}` and every `addon_bytes` is zero.
//! - `FREEWEIGHTS_PLAN_PERKS` — JSON object, plan id → `{perk name: number | bool | string}`.
//!   When set to a valid object it replaces the built-in draft table ([`DRAFT_PLAN_PERKS`])
//!   for both the claim and the `/billing/plans` listing; unset or invalid keeps the draft
//!   table. The Worker's signed-in rate limit reads the perk `api_requests_5m` (a number); with
//!   perks off it is absent and the Worker applies no limit of its own.
//!
//! Perk listing (plan U13): `GET /api/v1/billing/plans` carries a `freeweights` block built by
//! [`PerkConfig::listing`] from the same table: `status` (`offered` only while perks are on,
//! else `not_offered`) and, per plan, lines of `{key, label, value, unit}` with `unit` one of
//! `bytes`, `count` or `bool`. The draft numbers are listed, not offered: while perks are off
//! the claim's `perks` stays `{}`.
//! - `CLERK_SECRET_KEY` / `CLERK_API_BASE` — the org list comes from Clerk's
//!   `GET /v1/users/{id}/organization_memberships` (same config as `services::user_trust`).
//!   No secret → `orgs: []`.
//!
//! The plan is `routes::billing_subscriptions::current_plan_id`, the same answer as
//! `GET /api/v1/billing/subscription`, so the claim names plans the way cloud-api does
//! (`free`, `plus`, `super` = Max, `ultra`). Add-on storage has no Stripe products yet, so
//! `addon_bytes` is zero until it does. No org has a team plan yet: org `plan` is `free` and
//! `seats` is left out.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::services::api_keys::{
    parse_freeweights_scope, FREEWEIGHTS_READ_SCOPE, FREEWEIGHTS_WRITE_SCOPE,
};
use crate::ApiState;

pub const SIGNING_KEY_ENV: &str = "FREEWEIGHTS_ENTITLEMENT_SIGNING_KEY";
pub const PERKS_ENABLED_ENV: &str = "FREEWEIGHTS_PERKS_ENABLED";
pub const PLAN_PERKS_ENV: &str = "FREEWEIGHTS_PLAN_PERKS";

pub const ENTITLEMENT_PATH: &str = "/api/v1/freeweights/entitlement";
pub const ENTITLEMENT_KID: &str = "entitlement-v1";
pub const ENTITLEMENT_ISSUER: &str = "https://api.allternit.com";
pub const ENTITLEMENT_AUDIENCE: &str = "freeweights";
/// Claim lifetime: `exp = iat + 600` (contract: 10 minutes).
pub const ENTITLEMENT_TTL_SECS: i64 = 600;
pub const ENTITLEMENT_CONTENT_TYPE: &str = "application/jose";
/// Exact JWS protected header the contract names.
const JWS_HEADER_JSON: &str = r#"{"alg":"EdDSA","typ":"JWT","kid":"entitlement-v1"}"#;
/// Verify-side clock skew (contract: 60 seconds).
const LEEWAY_SECS: i64 = 60;
const RETRY_AFTER_SECS: &str = "60";

/// Browser origins of the Freeweights site and its preview host. `lib.rs` lets them call only
/// [`is_freeweights_cors_path`] routes, not the whole API.
pub const FREEWEIGHTS_CORS_ORIGINS: &[&str] = &[
    "https://freeweights.allternit.com",
    "https://fw-preview.allternit.com",
];

/// Routes the Freeweights origins may call from a browser: this namespace, and the public plan
/// catalog its `/plans/` page reads at view time (plan D10).
pub fn is_freeweights_cors_path(path: &str) -> bool {
    path.starts_with("/api/v1/freeweights/") || path == "/api/v1/billing/plans"
}

/// RFC 8410 PKCS#8 prefix for an Ed25519 private key; the 32-byte seed follows.
const PKCS8_ED25519_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new().route(ENTITLEMENT_PATH, get(entitlement))
}

/// Add-on storage bought on top of the plan, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonBytes {
    pub public: u64,
    pub private: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitlementOrg {
    pub clerk_org_id: String,
    pub slug: String,
    /// `admin` or `member`.
    pub role: String,
    pub plan: String,
    pub perks: Map<String, Value>,
    pub addon_bytes: AddonBytes,
    /// Only when the org has a team plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<u32>,
}

/// The signed payload. Every field is required by the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitlementClaim {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub plan: String,
    pub orgs: Vec<EntitlementOrg>,
    pub perks: Map<String, Value>,
    pub addon_bytes: AddonBytes,
    pub scopes: Vec<String>,
    /// `session` or `key`.
    pub cred: String,
    pub iat: i64,
    pub exp: i64,
}

// ── Signing ─────────────────────────────────────────────────────────────────

pub struct EntitlementSigner {
    signing: SigningKey,
}

impl EntitlementSigner {
    pub fn from_signing_key(signing: SigningKey) -> Self {
        Self { signing }
    }

    /// `Ok(None)` = the env var is unset or blank; `Err` = set but unreadable.
    pub fn from_env() -> Result<Option<Self>, String> {
        match std::env::var(SIGNING_KEY_ENV) {
            Ok(raw) if !raw.trim().is_empty() => parse_signing_key(&raw).map(|k| Some(Self::from_signing_key(k))),
            _ => Ok(None),
        }
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// Compact JWS: `b64url(header).b64url(payload).b64url(signature)`.
    pub fn sign(&self, claim: &EntitlementClaim) -> String {
        let header = URL_SAFE_NO_PAD.encode(JWS_HEADER_JSON);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claim).expect("claim serializes"));
        let signature: Signature = self.signing.sign(format!("{header}.{payload}").as_bytes());
        format!("{header}.{payload}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }
}

/// Read the signing key: PKCS#8 PEM, or a 32-byte seed as standard/url-safe base64 or hex.
pub fn parse_signing_key(raw: &str) -> Result<SigningKey, String> {
    let raw = raw.trim().replace("\\n", "\n");
    let seed: Vec<u8> = if raw.starts_with("-----BEGIN") {
        if !raw.starts_with("-----BEGIN PRIVATE KEY-----") {
            return Err(format!("{SIGNING_KEY_ENV}: expected a PKCS#8 'PRIVATE KEY' PEM"));
        }
        let body: String = raw
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .map(str::trim)
            .collect();
        let der = STANDARD
            .decode(body)
            .map_err(|_| format!("{SIGNING_KEY_ENV}: PEM body is not base64"))?;
        if der.len() != 48 || der[..16] != PKCS8_ED25519_PREFIX {
            return Err(format!("{SIGNING_KEY_ENV}: PEM is not an Ed25519 PKCS#8 key"));
        }
        der[16..].to_vec()
    } else if raw.len() == 64 && raw.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(&raw).map_err(|_| format!("{SIGNING_KEY_ENV}: bad hex"))?
    } else {
        STANDARD
            .decode(&raw)
            .or_else(|_| URL_SAFE_NO_PAD.decode(raw.trim_end_matches('=')))
            .map_err(|_| format!("{SIGNING_KEY_ENV}: not a PEM, hex or base64 seed"))?
    };
    let seed: [u8; 32] = seed
        .as_slice()
        .try_into()
        .map_err(|_| format!("{SIGNING_KEY_ENV}: an Ed25519 seed is 32 bytes"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The Worker-side checks of section 3 "Verify", for tests and as the reference for clients:
/// header (`alg` EdDSA, `kid`), signature, `iss`, `aud`, and `exp` with 60 seconds of skew.
pub fn verify_entitlement(token: &str, key: &VerifyingKey, now: i64) -> Result<EntitlementClaim, String> {
    let mut parts = token.split('.');
    let (h, p, s) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) => (h, p, s),
        _ => return Err("malformed".into()),
    };
    let header: Value = URL_SAFE_NO_PAD
        .decode(h)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("malformed header")?;
    if header["alg"] != "EdDSA" || header["kid"] != ENTITLEMENT_KID {
        return Err("unexpected alg or kid".into());
    }
    let sig = URL_SAFE_NO_PAD
        .decode(s)
        .ok()
        .and_then(|b| Signature::from_slice(&b).ok())
        .ok_or("malformed signature")?;
    key.verify(format!("{h}.{p}").as_bytes(), &sig)
        .map_err(|_| "signature mismatch")?;
    let claim: EntitlementClaim = URL_SAFE_NO_PAD
        .decode(p)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("malformed payload")?;
    if claim.iss != ENTITLEMENT_ISSUER || claim.aud != ENTITLEMENT_AUDIENCE {
        return Err("wrong issuer or audience".into());
    }
    if claim.exp + LEEWAY_SECS <= now {
        return Err("expired".into());
    }
    Ok(claim)
}

// ── Perks ───────────────────────────────────────────────────────────────────

/// `freeweights.status` in `/billing/plans` while perks are on.
pub const PERKS_STATUS_OFFERED: &str = "offered";
/// `freeweights.status` in `/billing/plans` while perks are off (the default).
pub const PERKS_STATUS_NOT_OFFERED: &str = "not_offered";

const GB: u64 = 1_000_000_000;
const TB: u64 = 1_000_000_000_000;

/// Known perks in listing order: key, label, unit (`bytes`, `count` or `bool`).
const PERK_KEYS: &[(&str, &str, &str)] = &[
    ("public_storage_bytes", "Public upload storage", "bytes"),
    ("private_storage_bytes", "Private storage", "bytes"),
    ("api_requests_5m", "Hub API requests per 5 minutes", "count"),
    ("webhooks", "Webhooks", "count"),
    ("private_collections", "Private collections", "bool"),
    ("private_dataset_viewer", "Private dataset viewer", "bool"),
];

/// One plan's row of the draft perk table.
struct DraftPerks {
    plan: &'static str,
    public_storage_bytes: u64,
    private_storage_bytes: u64,
    api_requests_5m: u64,
    webhooks: u64,
    /// `private_collections` and `private_dataset_viewer`.
    private_features: bool,
}

/// DRAFT perk table (freeweights plan of record, 2026-11-15, section 3; decimal bytes). Every
/// row still needs Eoj's sign-off. It is listed on `/billing/plans` as `not_offered` and feeds
/// the claim only once `FREEWEIGHTS_PERKS_ENABLED` is on. `FREEWEIGHTS_PLAN_PERKS` replaces it.
const DRAFT_PLAN_PERKS: &[DraftPerks] = &[
    DraftPerks { plan: "free", public_storage_bytes: 10 * GB, private_storage_bytes: 0, api_requests_5m: 1_000, webhooks: 2, private_features: false },
    DraftPerks { plan: "plus", public_storage_bytes: 250 * GB, private_storage_bytes: 100 * GB, api_requests_5m: 2_500, webhooks: 10, private_features: true },
    DraftPerks { plan: "super", public_storage_bytes: TB, private_storage_bytes: 500 * GB, api_requests_5m: 5_000, webhooks: 50, private_features: true },
    DraftPerks { plan: "ultra", public_storage_bytes: 2 * TB, private_storage_bytes: TB, api_requests_5m: 10_000, webhooks: 100, private_features: true },
];

/// The draft table as plan id → perk map, the same shape `FREEWEIGHTS_PLAN_PERKS` takes.
pub fn draft_plan_perks() -> Map<String, Value> {
    DRAFT_PLAN_PERKS
        .iter()
        .map(|row| {
            let perks: Map<String, Value> = [
                ("public_storage_bytes", Value::from(row.public_storage_bytes)),
                ("private_storage_bytes", Value::from(row.private_storage_bytes)),
                ("api_requests_5m", Value::from(row.api_requests_5m)),
                ("webhooks", Value::from(row.webhooks)),
                ("private_collections", Value::Bool(row.private_features)),
                ("private_dataset_viewer", Value::Bool(row.private_features)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            (row.plan.to_string(), Value::Object(perks))
        })
        .collect()
}

/// One listed perk: `{key, label, value, unit}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerkLine {
    pub key: String,
    pub label: String,
    pub value: Value,
    /// `bytes`, `count` or `bool`.
    pub unit: &'static str,
}

/// The `freeweights` block of `GET /api/v1/billing/plans`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerkListing {
    /// `offered` or `not_offered`.
    pub status: &'static str,
    /// Plan id (`free`, `plus`, `super`, `ultra`) → lines.
    pub plans: Map<String, Value>,
}

/// Lines for one plan's perk map: known perks first in [`PERK_KEYS`] order, then any other
/// number or boolean perk by name (label = key). A value whose type does not fit its unit, or
/// a string, is left out of the listing.
fn perk_lines(perks: &Map<String, Value>) -> Vec<PerkLine> {
    let fits = |unit: &str, v: &Value| match unit {
        "bool" => v.is_boolean(),
        _ => v.is_number(),
    };
    let mut lines: Vec<PerkLine> = PERK_KEYS
        .iter()
        .filter_map(|(key, label, unit)| {
            let value = perks.get(*key).filter(|v| fits(unit, v))?;
            Some(PerkLine { key: key.to_string(), label: label.to_string(), value: value.clone(), unit: *unit })
        })
        .collect();
    let mut extra: Vec<PerkLine> = perks
        .iter()
        .filter(|(k, _)| !PERK_KEYS.iter().any(|(known, _, _)| known == k))
        .filter_map(|(k, v)| {
            let unit = if v.is_boolean() { "bool" } else if v.is_number() { "count" } else { return None };
            Some(PerkLine { key: k.clone(), label: k.clone(), value: v.clone(), unit })
        })
        .collect();
    extra.sort_by(|a, b| a.key.cmp(&b.key));
    lines.extend(extra);
    lines
}

/// Perk configuration. Off by default; while off every claim perk map is empty, but the table
/// is still listed on `/billing/plans` with status `not_offered`.
#[derive(Debug, Clone)]
pub struct PerkConfig {
    enabled: bool,
    plans: Map<String, Value>,
}

impl Default for PerkConfig {
    fn default() -> Self {
        Self { enabled: false, plans: draft_plan_perks() }
    }
}

impl PerkConfig {
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var(PERKS_ENABLED_ENV).ok().as_deref(),
            std::env::var(PLAN_PERKS_ENV).ok().as_deref(),
        )
    }

    /// `enabled` is `FREEWEIGHTS_PERKS_ENABLED`, `plans` is `FREEWEIGHTS_PLAN_PERKS` (a valid
    /// JSON object replaces the draft table; unset, blank or invalid keeps it).
    pub fn from_vars(enabled: Option<&str>, plans: Option<&str>) -> Self {
        let enabled = matches!(enabled.map(str::trim), Some("1") | Some("true"));
        let plans = match plans.map(str::trim).filter(|raw| !raw.is_empty()).map(serde_json::from_str::<Value>) {
            Some(Ok(Value::Object(map))) => map,
            Some(_) => {
                tracing::warn!("{PLAN_PERKS_ENV} is not a JSON object; using the built-in draft perk table");
                draft_plan_perks()
            }
            None => draft_plan_perks(),
        };
        Self { enabled, plans }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Perks for a plan id. Values that are not a number, boolean or string are dropped.
    pub fn perks_for(&self, plan: &str) -> Map<String, Value> {
        if !self.enabled {
            return Map::new();
        }
        self.plans
            .get(plan)
            .and_then(Value::as_object)
            .map(|perks| {
                perks
                    .iter()
                    .filter(|(_, v)| v.is_number() || v.is_boolean() || v.is_string())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `/billing/plans` block: every plan in the table, listed whether perks are on or off.
    pub fn listing(&self) -> PerkListing {
        PerkListing {
            status: if self.enabled { PERKS_STATUS_OFFERED } else { PERKS_STATUS_NOT_OFFERED },
            plans: self
                .plans
                .iter()
                .filter_map(|(plan, perks)| {
                    let lines = perk_lines(perks.as_object()?);
                    Some((plan.clone(), serde_json::to_value(lines).expect("perk lines serialize")))
                })
                .collect(),
        }
    }

    /// Add-on bytes: none are sold yet, so zero whether perks are on or off.
    pub fn addon_bytes_for(&self, _owner: &str) -> AddonBytes {
        AddonBytes::default()
    }
}

// ── Orgs ────────────────────────────────────────────────────────────────────

/// One Clerk org membership, before the claim adds plan and perks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgMembership {
    pub clerk_org_id: String,
    pub slug: String,
    pub role: String,
}

/// Parse Clerk's `organization_memberships` list. `org:admin` (or `admin`) is `admin`, every
/// other role is `member`. An org without a slug keeps an empty slug.
pub fn parse_org_memberships(body: &Value) -> Vec<OrgMembership> {
    body.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let clerk_org_id = row.pointer("/organization/id")?.as_str()?.to_string();
            let slug = row
                .pointer("/organization/slug")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let role = match row.get("role").and_then(Value::as_str) {
                Some("org:admin") | Some("admin") => "admin",
                _ => "member",
            };
            Some(OrgMembership { clerk_org_id, slug, role: role.to_string() })
        })
        .collect()
}

/// The user's Clerk orgs. No `CLERK_SECRET_KEY` → none. Clerk failing → `Err`, so the claim is
/// not signed with a wrong (empty) org list for ten minutes.
async fn org_memberships(user_id: &str) -> Result<Vec<OrgMembership>, String> {
    let config = crate::services::user_trust::TrustConfig::from_env();
    let Some(secret) = config.clerk_secret_key.as_deref().filter(|s| !s.is_empty()) else {
        tracing::warn!("freeweights entitlement: CLERK_SECRET_KEY is not set; orgs left empty");
        return Ok(Vec::new());
    };
    let url = format!(
        "{}/v1/users/{}/organization_memberships?limit=100",
        config.clerk_api_base.trim_end_matches('/'),
        urlencoding::encode(user_id)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(&url)
        .bearer_auth(secret)
        .send()
        .await
        .map_err(|e| format!("Clerk request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("Clerk answered {}", response.status()));
    }
    let body: Value = response.json().await.map_err(|e| format!("Clerk body: {e}"))?;
    Ok(parse_org_memberships(&body))
}

// ── Claim ───────────────────────────────────────────────────────────────────

/// Freeweights scopes of the credential. A session (no token permissions) and a full-access
/// legacy token (`*`) get read + write; a key gets its own well-formed `freeweights.*` scopes.
pub fn claim_scopes(token_permissions: Option<&[String]>) -> Vec<String> {
    let full = || vec![FREEWEIGHTS_READ_SCOPE.to_string(), FREEWEIGHTS_WRITE_SCOPE.to_string()];
    match token_permissions {
        None => full(),
        Some(perms) if perms.iter().any(|p| p == "*") => full(),
        Some(perms) => {
            let mut out: Vec<String> = Vec::new();
            for scope in perms
                .iter()
                .filter_map(|p| parse_freeweights_scope(p).and_then(Result::ok))
                .map(|s| s.as_scope_string())
            {
                if !out.contains(&scope) {
                    out.push(scope);
                }
            }
            out
        }
    }
}

pub struct ClaimInput<'a> {
    pub sub: &'a str,
    pub plan: &'a str,
    pub orgs: Vec<OrgMembership>,
    pub token_permissions: Option<&'a [String]>,
    pub now: i64,
}

pub fn build_claim(input: ClaimInput<'_>, perks: &PerkConfig) -> EntitlementClaim {
    // No org billing exists yet; org plans arrive with team seats.
    const ORG_PLAN: &str = "free";
    EntitlementClaim {
        iss: ENTITLEMENT_ISSUER.to_string(),
        aud: ENTITLEMENT_AUDIENCE.to_string(),
        sub: input.sub.to_string(),
        plan: input.plan.to_string(),
        orgs: input
            .orgs
            .into_iter()
            .map(|org| EntitlementOrg {
                perks: perks.perks_for(ORG_PLAN),
                addon_bytes: perks.addon_bytes_for(&org.clerk_org_id),
                clerk_org_id: org.clerk_org_id,
                slug: org.slug,
                role: org.role,
                plan: ORG_PLAN.to_string(),
                seats: None,
            })
            .collect(),
        perks: perks.perks_for(input.plan),
        addon_bytes: perks.addon_bytes_for(input.sub),
        scopes: claim_scopes(input.token_permissions),
        cred: if input.token_permissions.is_none() { "session" } else { "key" }.to_string(),
        iat: input.now,
        exp: input.now + ENTITLEMENT_TTL_SECS,
    }
}

fn unavailable(code: &'static str, message: &str, retry: bool) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "error": code, "message": message })),
    )
        .into_response();
    let headers = response.headers_mut();
    if retry {
        headers.insert(header::RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER_SECS));
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response
}

/// A dependency failed (plan read, Clerk): try again shortly.
fn entitlement_unavailable(message: &str) -> Response {
    unavailable("entitlement_unavailable", message, true)
}

/// No usable signing key on this deployment: accounts are not set up yet.
fn accounts_not_configured() -> Response {
    unavailable(
        "accounts_not_configured",
        "Freeweights entitlement signing is not configured on this server.",
        false,
    )
}

async fn entitlement(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let user = match crate::auth::resolve_user(&state.db, &headers).await {
        Ok(user) => user,
        Err(error) => return error.into_response(),
    };
    let signer = match EntitlementSigner::from_env() {
        Ok(Some(signer)) => signer,
        Ok(None) => return accounts_not_configured(),
        Err(error) => {
            tracing::error!("freeweights entitlement: {error}");
            return accounts_not_configured();
        }
    };
    let plan = match crate::routes::billing_subscriptions::current_plan_id(&state.db, &user.id).await {
        Ok(plan) => plan,
        Err(error) => {
            tracing::error!(user_id = %user.id, "freeweights entitlement: plan lookup failed: {error}");
            return entitlement_unavailable("The plan could not be read. Try again shortly.");
        }
    };
    let orgs = match org_memberships(&user.id).await {
        Ok(orgs) => orgs,
        Err(error) => {
            tracing::warn!(user_id = %user.id, "freeweights entitlement: {error}");
            return entitlement_unavailable("Organizations could not be read. Try again shortly.");
        }
    };
    let claim = build_claim(
        ClaimInput {
            sub: &user.id,
            plan: &plan,
            orgs,
            token_permissions: user.token_permissions.as_deref(),
            now: chrono::Utc::now().timestamp(),
        },
        &PerkConfig::from_env(),
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(ENTITLEMENT_CONTENT_TYPE)),
            (header::CACHE_CONTROL, HeaderValue::from_static("private, no-store")),
        ],
        signer.sign(&claim),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::dev_token::{ALLOW_DEV_TOKEN_ENV, DEV_TOKEN_ENV_LOCK};
    use crate::routes::test_support::{test_state, MockGateway};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    fn throwaway_signer() -> EntitlementSigner {
        EntitlementSigner::from_signing_key(SigningKey::from_bytes(&rand::random::<[u8; 32]>()))
    }

    fn claim_for(token_permissions: Option<&[String]>, perks: &PerkConfig) -> EntitlementClaim {
        build_claim(
            ClaimInput {
                sub: "user_2abc",
                plan: "plus",
                orgs: vec![OrgMembership {
                    clerk_org_id: "org_1".into(),
                    slug: "acme".into(),
                    role: "admin".into(),
                }],
                token_permissions,
                now: 1_700_000_000,
            },
            perks,
        )
    }

    #[test]
    fn claim_signs_and_verifies_with_the_matching_public_key_only() {
        let signer = throwaway_signer();
        let claim = claim_for(None, &PerkConfig::default());
        let token = signer.sign(&claim);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token.split('.').next().unwrap()).unwrap()).unwrap();
        assert_eq!(header, serde_json::json!({"alg": "EdDSA", "typ": "JWT", "kid": "entitlement-v1"}));

        let verified = verify_entitlement(&token, &signer.verifying_key(), 1_700_000_100).unwrap();
        assert_eq!(verified, claim);
        assert_eq!(verified.exp, verified.iat + 600);
        assert_eq!(verified.iss, "https://api.allternit.com");
        assert_eq!(verified.aud, "freeweights");
        assert_eq!(verified.cred, "session");
        assert_eq!(verified.scopes, vec!["freeweights.read", "freeweights.write"]);

        assert!(verify_entitlement(&token, &throwaway_signer().verifying_key(), 1_700_000_100).is_err());
        // Past exp plus the 60 s skew.
        assert!(verify_entitlement(&token, &signer.verifying_key(), 1_700_000_661).is_err());
        let mut tampered = token.clone();
        tampered.insert_str(token.find('.').unwrap() + 1, "e");
        assert!(verify_entitlement(&tampered, &signer.verifying_key(), 1_700_000_100).is_err());
    }

    #[test]
    fn payload_carries_every_contract_field() {
        let token = throwaway_signer().sign(&claim_for(None, &PerkConfig::default()));
        let payload: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap();
        for field in ["sub", "plan", "orgs", "perks", "addon_bytes", "scopes", "iss", "aud", "cred", "iat", "exp"] {
            assert!(payload.get(field).is_some(), "missing {field}");
        }
        let org = &payload["orgs"][0];
        for field in ["clerk_org_id", "slug", "role", "plan", "perks", "addon_bytes"] {
            assert!(org.get(field).is_some(), "org missing {field}");
        }
        assert!(org.get("seats").is_none(), "seats only with a team plan");
        assert_eq!(payload["addon_bytes"], serde_json::json!({"public": 0, "private": 0}));
    }

    #[test]
    fn perks_are_off_by_default_even_with_a_table() {
        let table = r#"{"plus":{"api_requests_5m":1,"private_collections":true},"free":{"webhooks":2}}"#;
        let off = PerkConfig::from_vars(None, Some(table));
        assert!(!off.enabled());
        let claim = claim_for(None, &off);
        assert!(claim.perks.is_empty());
        assert!(claim.perks.get("api_requests_5m").is_none(), "no rate-limit perk while perks are off");
        assert!(claim.orgs[0].perks.is_empty());
        assert_eq!(claim.addon_bytes, AddonBytes::default());
        assert!(PerkConfig::from_vars(Some("0"), Some(table)).perks_for("plus").is_empty());

        let on = PerkConfig::from_vars(Some("1"), Some(table));
        let claim = claim_for(None, &on);
        assert_eq!(claim.perks["private_collections"], Value::Bool(true));
        assert_eq!(claim.perks["api_requests_5m"], serde_json::json!(1));
        assert_eq!(claim.orgs[0].perks["webhooks"], serde_json::json!(2));
        // An override replaces the whole table: a plan it leaves out has no perks.
        assert!(on.perks_for("ultra").is_empty());
        // Unset or invalid override: the draft table feeds the claim once perks are on.
        for raw in [None, Some("not json"), Some("[1]"), Some("  ")] {
            let perks = PerkConfig::from_vars(Some("true"), raw).perks_for("plus");
            assert_eq!(perks["public_storage_bytes"], serde_json::json!(250_000_000_000u64), "{raw:?}");
            assert_eq!(perks["api_requests_5m"], serde_json::json!(2_500), "{raw:?}");
        }
    }

    #[test]
    fn draft_table_is_claimed_only_while_perks_are_on() {
        // Default deployment: no env at all. The draft table exists but the claim stays empty.
        let off = PerkConfig::from_vars(None, None);
        for plan in ["free", "plus", "super", "ultra"] {
            assert!(off.perks_for(plan).is_empty(), "{plan}");
        }
        assert!(claim_for(None, &off).perks.is_empty());
        let on = PerkConfig::from_vars(Some("1"), None);
        let claim = claim_for(None, &on);
        assert_eq!(claim.perks["private_storage_bytes"], serde_json::json!(100_000_000_000u64));
        assert_eq!(claim.perks["private_collections"], Value::Bool(true));
        // Org plans are free.
        assert_eq!(claim.orgs[0].perks["public_storage_bytes"], serde_json::json!(10_000_000_000u64));
        assert_eq!(claim.orgs[0].perks["private_dataset_viewer"], Value::Bool(false));
    }

    fn line(listing: &PerkListing, plan: &str, key: &str) -> Value {
        listing.plans[plan]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["key"] == key)
            .cloned()
            .unwrap_or_else(|| panic!("{plan} has no {key} line"))
    }

    #[test]
    fn listing_shows_the_draft_table_as_not_offered_by_default() {
        let listing = PerkConfig::from_vars(None, None).listing();
        assert_eq!(listing.status, "not_offered");
        let mut plans: Vec<&String> = listing.plans.keys().collect();
        plans.sort();
        assert_eq!(plans, ["free", "plus", "super", "ultra"]);

        let free: Vec<&str> = listing.plans["free"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            free,
            ["public_storage_bytes", "private_storage_bytes", "api_requests_5m", "webhooks", "private_collections", "private_dataset_viewer"]
        );
        assert_eq!(
            line(&listing, "free", "public_storage_bytes"),
            serde_json::json!({"key": "public_storage_bytes", "label": "Public upload storage", "value": 10_000_000_000u64, "unit": "bytes"})
        );
        assert_eq!(line(&listing, "free", "private_storage_bytes")["value"], serde_json::json!(0));
        assert_eq!(line(&listing, "free", "api_requests_5m")["unit"], "count");
        assert_eq!(line(&listing, "free", "webhooks")["value"], serde_json::json!(2));
        assert_eq!(line(&listing, "free", "private_collections")["value"], Value::Bool(false));
        assert_eq!(line(&listing, "free", "private_dataset_viewer")["unit"], "bool");

        let expect = [
            ("plus", 250 * GB, 100 * GB, 2_500, 10),
            ("super", TB, 500 * GB, 5_000, 50),
            ("ultra", 2 * TB, TB, 10_000, 100),
        ];
        for (plan, public, private, requests, hooks) in expect {
            assert_eq!(line(&listing, plan, "public_storage_bytes")["value"], serde_json::json!(public), "{plan}");
            assert_eq!(line(&listing, plan, "private_storage_bytes")["value"], serde_json::json!(private), "{plan}");
            assert_eq!(line(&listing, plan, "api_requests_5m")["value"], serde_json::json!(requests), "{plan}");
            assert_eq!(line(&listing, plan, "webhooks")["value"], serde_json::json!(hooks), "{plan}");
            assert_eq!(line(&listing, plan, "private_collections")["value"], Value::Bool(true), "{plan}");
            assert_eq!(line(&listing, plan, "private_dataset_viewer")["value"], Value::Bool(true), "{plan}");
        }

        assert_eq!(PerkConfig::from_vars(Some("1"), None).listing().status, "offered");
        assert_eq!(PerkConfig::from_vars(Some("0"), None).listing().status, "not_offered");
    }

    #[test]
    fn override_feeds_the_listing_even_while_perks_are_off() {
        let table = r#"{"plus":{"webhooks":3,"private_collections":"yes","zeta":true,"alpha":7,"note":"x"}}"#;
        let off = PerkConfig::from_vars(None, Some(table));
        assert!(off.perks_for("plus").is_empty(), "listed, not claimed");
        let listing = off.listing();
        assert_eq!(listing.status, "not_offered");
        assert_eq!(listing.plans.len(), 1);
        let lines = listing.plans["plus"].as_array().unwrap();
        // Known keys first; a value that does not fit its unit and strings are left out;
        // unknown number/bool perks follow by name with the key as label.
        assert_eq!(
            Value::Array(lines.clone()),
            serde_json::json!([
                {"key": "webhooks", "label": "Webhooks", "value": 3, "unit": "count"},
                {"key": "alpha", "label": "alpha", "value": 7, "unit": "count"},
                {"key": "zeta", "label": "zeta", "value": true, "unit": "bool"}
            ])
        );
    }

    #[test]
    fn key_claims_carry_only_their_freeweights_scopes() {
        let perms: Vec<String> = ["compute", "freeweights.repo:rp_01JD5Z8K3QW2X", "freeweights.read", "freeweights.bogus"]
            .map(String::from)
            .to_vec();
        let claim = claim_for(Some(&perms), &PerkConfig::default());
        assert_eq!(claim.cred, "key");
        assert_eq!(claim.scopes, vec!["freeweights.repo:rp_01JD5Z8K3QW2X", "freeweights.read"]);
        let none: Vec<String> = vec!["inference".into()];
        assert!(claim_scopes(Some(&none)).is_empty());
        let wildcard: Vec<String> = vec!["*".into()];
        assert_eq!(claim_scopes(Some(&wildcard)), vec!["freeweights.read", "freeweights.write"]);
    }

    #[test]
    fn signing_key_reads_pem_base64_and_hex_seeds() {
        let seed = rand::random::<[u8; 32]>();
        let expected = SigningKey::from_bytes(&seed).verifying_key();
        let der = [PKCS8_ED25519_PREFIX.as_slice(), seed.as_slice()].concat();
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", STANDARD.encode(&der));
        for raw in [pem.clone(), pem.replace('\n', "\\n"), STANDARD.encode(seed), hex::encode(seed)] {
            assert_eq!(parse_signing_key(&raw).unwrap().verifying_key(), expected, "{raw}");
        }
        assert!(parse_signing_key("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----").is_err());
        assert!(parse_signing_key(&STANDARD.encode([1u8; 16])).is_err());
        assert!(parse_signing_key("not a key").is_err());
    }

    #[test]
    fn clerk_memberships_map_roles_and_slugs() {
        let body = serde_json::json!({"data": [
            {"role": "org:admin", "organization": {"id": "org_a", "slug": "acme"}},
            {"role": "org:member", "organization": {"id": "org_b", "slug": "beta"}},
            {"role": "org:billing", "organization": {"id": "org_c", "slug": null}},
            {"role": "org:admin"}
        ]});
        assert_eq!(
            parse_org_memberships(&body),
            vec![
                OrgMembership { clerk_org_id: "org_a".into(), slug: "acme".into(), role: "admin".into() },
                OrgMembership { clerk_org_id: "org_b".into(), slug: "beta".into(), role: "member".into() },
                OrgMembership { clerk_org_id: "org_c".into(), slug: "".into(), role: "member".into() },
            ]
        );
    }

    #[test]
    fn cors_paths_cover_only_freeweights_and_the_plan_catalog() {
        assert!(is_freeweights_cors_path(ENTITLEMENT_PATH));
        assert!(is_freeweights_cors_path("/api/v1/billing/plans"));
        assert!(!is_freeweights_cors_path("/api/v1/billing/subscription"));
        assert!(!is_freeweights_cors_path("/api/v1/api-keys"));
        assert!(!is_freeweights_cors_path("/api/v1/freeweightsx"));
    }

    async fn billing_table(db: &sqlx::PgPool) {
        sqlx::query(
            "CREATE TABLE billing_subscriptions (stripe_subscription_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, \
             plan_id TEXT NOT NULL, plan_tier TEXT NOT NULL, status TEXT NOT NULL, stripe_customer_id TEXT, \
             current_period_end TIMESTAMPTZ, updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
        )
        .execute(db)
        .await
        .unwrap();
    }

    fn get(auth: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(ENTITLEMENT_PATH);
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        builder.body(Body::empty()).unwrap()
    }

    /// Through the real router: 401 without a credential, 503 `accounts_not_configured` without a
    /// signing key, and a
    /// verifiable claim (plan from billing_subscriptions) once the key is set.
    #[tokio::test]
    // Same unkeyed serial group as the other dev-token route tests (device_read), so no test
    // flips ALLTERNIT_ALLOW_DEV_TOKEN while another is mid-request.
    #[serial_test::serial]
    async fn route_answers_401_then_503_then_a_signed_claim() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        billing_table(&state.db).await;
        sqlx::query(
            "INSERT INTO billing_subscriptions (stripe_subscription_id, user_id, plan_id, plan_tier, status) \
             VALUES ('sub_1', 'dev-user', 'super', 'team', 'active')",
        )
        .execute(&state.db)
        .await
        .unwrap();
        let app = crate::create_router(state);

        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "1");
        std::env::remove_var(SIGNING_KEY_ENV);

        let res = app.clone().oneshot(get(None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let res = app.clone().oneshot(get(Some("Bearer dev-api-token"))).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = serde_json::from_slice(&to_bytes(res.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(body["error"], "accounts_not_configured");

        // A set but unreadable key is the same misconfiguration, never a panic or a bad claim.
        std::env::set_var(SIGNING_KEY_ENV, "not a key");
        let res = app.clone().oneshot(get(Some("Bearer dev-api-token"))).await.unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: Value = serde_json::from_slice(&to_bytes(res.into_body(), 1 << 16).await.unwrap()).unwrap();
        assert_eq!(body["error"], "accounts_not_configured");

        let seed = rand::random::<[u8; 32]>();
        std::env::set_var(SIGNING_KEY_ENV, STANDARD.encode(seed));
        let res = app.clone().oneshot(get(Some("Bearer dev-api-token"))).await.unwrap();
        // ALLTERNIT_ALLOW_DEV_TOKEN stays set, as device_read leaves it: removing it races
        // dev-token tests that do not take DEV_TOKEN_ENV_LOCK.
        std::env::remove_var(SIGNING_KEY_ENV);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "application/jose");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "private, no-store");
        let token = String::from_utf8(to_bytes(res.into_body(), 1 << 16).await.unwrap().to_vec()).unwrap();
        let public = SigningKey::from_bytes(&seed).verifying_key();
        let claim = verify_entitlement(&token, &public, chrono::Utc::now().timestamp()).unwrap();
        assert_eq!(claim.sub, "dev-user");
        assert_eq!(claim.plan, "super");
        assert_eq!(claim.exp - claim.iat, 600);
        assert!(claim.perks.is_empty(), "perks are off unless FREEWEIGHTS_PERKS_ENABLED is set");
    }
}
