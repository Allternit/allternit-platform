//! Public A2A v1.0 edge: `/a2a/...` on the edge host (`a2a.allternit.com` / `mcp.allternit.com`).
//!
//! * `GET  /a2a/.well-known/jwks.json` — the public JWK that signs Agent Cards.
//! * `GET  /a2a/<owner>/<bot>/.well-known/agent-card.json` — a **signed** v1.0 Agent Card
//!   (JWS, EdDSA, key from `A2A_CARD_SIGNING_KEY`, a base64 32-byte seed). Only a bot the owner
//!   switched on ("Reachable by other agents") has one; everything else is a plain 404.
//! * `POST /a2a/<owner>/<bot>/rpc` — JSON-RPC `SendMessage` / `GetTask` / `CancelTask`
//!   (the 0.3 names work too). Streaming is off and parts are text only.
//!
//! The caller is a Clerk OAuth bearer (the user), or an outside agent: `X-A2A-Caller: <card URL>`
//! plus a bearer JWT signed (EdDSA) by a key in that card site's `/.well-known/jwks.json`
//! (`iss` = the card URL, `aud` = this rpc URL, short `exp`). It is classified `team` (the
//! owner, or the same Clerk organization), `contact` (an accepted `agent_contacts` row whose
//! card hash still matches) or `stranger`, and the call is relayed signed to the owner's runtime
//! (`/webhooks/a2a-edge/*`) with the class in `x-a2a-peer-*`. The runtime applies the peer
//! policy (team send, contact hold, stranger refuse; the owner can override).
//!
//! Contacts: `GET/POST/DELETE /api/v1/agent-contacts` (migration 097).
//!
//! Inert (503 `a2a_not_configured`) until `A2A_CARD_SIGNING_KEY` is set. `A2A_PUBLIC_URL`
//! (default `https://mcp.allternit.com/a2a`) is the public base.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine as _};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::runtime_relay::{connected_runtime_ids, relay_signed_request_to_runtime_with, RelayRequest};
use crate::ApiState;

const PEER_CLASS: &str = "x-a2a-peer-class";
const PEER_ID: &str = "x-a2a-peer-id";
const PEER_NAME: &str = "x-a2a-peer-name";
const CALLER_HEADER: &str = "x-a2a-caller";
const MAX_BODY: usize = 128 * 1024;
const REFUSED: &str = "This agent accepts messages from contacts only.";

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/a2a/.well-known/jwks.json", get(jwks_route))
        .route("/a2a/:owner/:bot/.well-known/agent-card.json", get(card_route))
        .route("/a2a/:owner/:bot/rpc", post(rpc_route))
        .route("/api/v1/agent-contacts", get(contacts_list).post(contacts_add))
        .route("/api/v1/agent-contacts/:id", delete(contacts_delete))
}

// ─── config + signing ─────────────────────────────────────────────────────────

fn public_base() -> String {
    std::env::var("A2A_PUBLIC_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| "https://mcp.allternit.com/a2a".to_string()).trim_end_matches('/').to_string()
}

fn signing_key() -> Option<SigningKey> {
    let raw = std::env::var("A2A_CARD_SIGNING_KEY").ok()?;
    let seed: [u8; 32] = STANDARD.decode(raw.trim()).ok()?.try_into().ok()?;
    Some(SigningKey::from_bytes(&seed))
}

fn key_id(vk: &VerifyingKey) -> String {
    hex::encode(&Sha256::digest(vk.as_bytes())[..8])
}

fn not_configured() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "a2a_not_configured" }))).into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
}

pub fn jwks_for(vk: &VerifyingKey) -> Value {
    json!({ "keys": [{ "kty": "OKP", "crv": "Ed25519", "x": URL_SAFE_NO_PAD.encode(vk.as_bytes()), "kid": key_id(vk), "use": "sig", "alg": "EdDSA" }] })
}

/// JSON with sorted keys and no whitespace, the signed form of a card.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<_, _> = m.iter().collect();
            format!("{{{}}}", sorted.iter().map(|(k, v)| format!("{}:{}", Value::String((*k).clone()), canonical(v))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// Adds `signatures: [{protected, signature}]`: a detached JWS over the canonical card.
pub fn sign_card(card: &mut Value, sk: &SigningKey, jku: &str) {
    card.as_object_mut().map(|m| m.remove("signatures"));
    let vk = sk.verifying_key();
    let protected = URL_SAFE_NO_PAD.encode(json!({ "alg": "EdDSA", "typ": "JOSE", "kid": key_id(&vk), "jku": jku }).to_string());
    let payload = URL_SAFE_NO_PAD.encode(canonical(card));
    let sig = sk.sign(format!("{protected}.{payload}").as_bytes());
    card["signatures"] = json!([{ "protected": protected, "signature": URL_SAFE_NO_PAD.encode(sig.to_bytes()) }]);
}

// ─── caller identity ──────────────────────────────────────────────────────────

/// Public https URLs only: no localhost, private ranges or bare hostnames (the edge fetches caller cards).
pub fn safe_https_url(raw: &str) -> Option<reqwest::Url> {
    let u = reqwest::Url::parse(raw.trim()).ok()?;
    if u.scheme() != "https" || !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let host = u.host_str()?.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            if ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) {
                return None;
            }
        }
        Ok(std::net::IpAddr::V6(_)) => return None,
        Err(_) => {
            if !host.contains('.') || host == "localhost" || host.ends_with(".local") || host.ends_with(".internal") || host.ends_with(".localhost") {
                return None;
            }
        }
    }
    Some(u)
}

fn http() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder().timeout(Duration::from_secs(8)).redirect(reqwest::redirect::Policy::none()).build()
}

async fn fetch_bytes(url: &reqwest::Url) -> Option<Vec<u8>> {
    let r = http().ok()?.get(url.clone()).header(header::ACCEPT, "application/json").send().await.ok()?;
    if !r.status().is_success() || r.content_length().is_some_and(|n| n > 256 * 1024) {
        return None;
    }
    let b = r.bytes().await.ok()?;
    (b.len() <= 256 * 1024).then(|| b.to_vec())
}

pub fn card_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Verify an EdDSA JWT against a JWKS value: `Some(claims)` when the signature and `exp` hold.
pub fn verify_eddsa_jwt(token: &str, jwks: &Value) -> Option<Value> {
    let mut parts = token.split('.');
    let (h, p, s) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(h).ok()?).ok()?;
    if header["alg"] != "EdDSA" {
        return None;
    }
    let kid = header["kid"].as_str();
    let key = jwks["keys"].as_array()?.iter().find(|k| k["kty"] == "OKP" && k["crv"] == "Ed25519" && (kid.is_none() || k["kid"].as_str() == kid))?;
    let x: [u8; 32] = URL_SAFE_NO_PAD.decode(key["x"].as_str()?).ok()?.try_into().ok()?;
    let vk = VerifyingKey::from_bytes(&x).ok()?;
    let sig = ed25519_dalek::Signature::from_slice(&URL_SAFE_NO_PAD.decode(s).ok()?).ok()?;
    vk.verify(format!("{h}.{p}").as_bytes(), &sig).ok()?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p).ok()?).ok()?;
    let now = chrono::Utc::now().timestamp();
    (claims["exp"].as_i64()? > now).then_some(claims)
}

pub struct Caller {
    /// `team`, `contact`, `stranger` or `blocked`.
    pub class: &'static str,
    pub id: String,
    pub name: String,
}

async fn owner_org(state: &ApiState, owner: &str) -> Option<String> {
    sqlx::query_scalar("SELECT organization_id FROM paired_computers WHERE user_id = $1 AND organization_id IS NOT NULL LIMIT 1").bind(owner).fetch_optional(&state.db).await.ok().flatten()
}

async fn contact_class(state: &ApiState, owner: &str, peer: &str, pinned_hash_now: Option<&str>) -> &'static str {
    let row: Option<(String, Option<String>, bool)> =
        sqlx::query_as("SELECT status, card_hash, card_changed FROM agent_contacts WHERE owner_user_id = $1 AND peer_card_url = $2").bind(owner).bind(peer).fetch_optional(&state.db).await.ok().flatten();
    match row {
        Some((s, _, _)) if s == "blocked" => "blocked",
        Some((s, pinned, changed)) if s == "accepted" => {
            let moved = changed || matches!((pinned.as_deref(), pinned_hash_now), (Some(a), Some(b)) if a != b);
            if moved {
                let _ = sqlx::query("UPDATE agent_contacts SET card_changed = true WHERE owner_user_id = $1 AND peer_card_url = $2").bind(owner).bind(peer).execute(&state.db).await;
                "stranger"
            } else {
                "contact"
            }
        }
        _ => "stranger",
    }
}

async fn identify(state: &ApiState, owner: &str, rpc_url: &str, headers: &HeaderMap) -> Result<Caller, Response> {
    let deny = |msg: &str| (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], Json(json!({ "error": "unauthorized", "message": msg }))).into_response();
    let token = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).filter(|t| !t.is_empty());
    let Some(token) = token else { return Err(deny("Send a Clerk OAuth bearer, or X-A2A-Caller with a bearer JWT signed by your Agent Card key.")) };
    if let Some(card) = headers.get(CALLER_HEADER).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty()) {
        let Some(card_url) = safe_https_url(card) else { return Err(deny("X-A2A-Caller must be a public https Agent Card URL.")) };
        let jwks_url = card_url.join("/.well-known/jwks.json").map_err(|_| deny("bad card url"))?;
        let (Some(card_bytes), Some(jwks_bytes)) = (fetch_bytes(&card_url).await, fetch_bytes(&jwks_url).await) else { return Err(deny("Could not fetch your Agent Card and its JWKS.")) };
        let jwks: Value = serde_json::from_slice(&jwks_bytes).unwrap_or(Value::Null);
        let Some(claims) = verify_eddsa_jwt(token, &jwks) else { return Err(deny("Bearer JWT is not signed by a key in your JWKS, or has expired.")) };
        let aud_ok = match &claims["aud"] {
            Value::String(a) => a == rpc_url,
            Value::Array(a) => a.iter().any(|x| x == rpc_url),
            _ => false,
        };
        if claims["iss"].as_str() != Some(card_url.as_str()) && claims["iss"].as_str() != Some(card) || !aud_ok {
            return Err(deny("JWT iss must be your card URL and aud this rpc URL."));
        }
        let name = serde_json::from_slice::<Value>(&card_bytes).ok().and_then(|c| c["name"].as_str().map(str::to_string)).unwrap_or_default();
        let class = contact_class(state, owner, card, Some(&card_hash(&card_bytes))).await;
        return Ok(Caller { class, id: card.to_string(), name });
    }
    let claims = crate::auth::clerk::verified_claims(token).await.map_err(|_| deny("Invalid or expired token."))?;
    let sub = claims["sub"].as_str().unwrap_or_default().to_string();
    if sub.is_empty() {
        return Err(deny("Token has no subject."));
    }
    let org = claims.get("o").and_then(|o| o.get("id")).and_then(|v| v.as_str()).or_else(|| claims.get("org_id").and_then(|v| v.as_str()));
    let class = if sub == owner || (org.is_some() && owner_org(state, owner).await.as_deref() == org) { "team" } else { contact_class(state, owner, &sub, None).await };
    Ok(Caller { class, id: sub, name: String::new() })
}

// ─── relay ────────────────────────────────────────────────────────────────────

async fn runtimes(state: &ApiState, owner: &str) -> Vec<String> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM runtime_devices WHERE user_id = $1 AND revoked_at IS NULL AND credential_expires_at > CURRENT_TIMESTAMP ORDER BY last_seen_at DESC NULLS LAST, created_at DESC")
        .bind(owner)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let connected = connected_runtime_ids().await;
    let (mut live, asleep): (Vec<String>, Vec<String>) = ids.into_iter().partition(|id| connected.contains(id));
    live.extend(asleep);
    live
}

/// Try the owner's runtimes in order; the first that holds the bot answers. `None` = unreachable.
async fn relay(state: &ApiState, owner: &str, path: &str, body: &[u8], extra: HashMap<String, String>) -> Option<(u16, Vec<u8>)> {
    for runtime_id in runtimes(state, owner).await {
        let r = relay_signed_request_to_runtime_with(
            &state.db,
            &state.contabo_runtime_service,
            &state.quota_service,
            &state.provisioning_service,
            owner,
            &runtime_id,
            RelayRequest { method: "POST".into(), path: path.into(), headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]), body: STANDARD.encode(body), body_encoding: "base64".into() },
            &["content-type"],
            extra.clone(),
        )
        .await;
        let Ok(resp) = r else { continue };
        let status = resp.status().as_u16();
        let Ok(bytes) = axum::body::to_bytes(resp.into_body(), 512 * 1024).await else { continue };
        if status == 404 || status == 503 || status == 504 {
            continue;
        }
        return Some((status, bytes.to_vec()));
    }
    None
}

fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).into_response()
}

// ─── handlers ─────────────────────────────────────────────────────────────────

async fn jwks_route() -> Response {
    match signing_key() {
        Some(sk) => Json(jwks_for(&sk.verifying_key())).into_response(),
        None => not_configured(),
    }
}

fn valid_handle(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.contains('/')
}

async fn card_route(State(state): State<Arc<ApiState>>, Path((owner, bot)): Path<(String, String)>) -> Response {
    let Some(sk) = signing_key() else { return not_configured() };
    if !valid_handle(&owner) || !valid_handle(&bot) {
        return not_found();
    }
    let Some((200, bytes)) = relay(&state, &owner, &format!("/webhooks/a2a-edge/card/{bot}"), b"{}", HashMap::new()).await else { return not_found() };
    let Ok(mut card) = serde_json::from_slice::<Value>(&bytes) else { return not_found() };
    let base = public_base();
    let rpc = format!("{base}/{owner}/{bot}/rpc");
    if let Some(ifaces) = card["supportedInterfaces"].as_array_mut() {
        for i in ifaces {
            i["url"] = json!(rpc);
        }
    }
    sign_card(&mut card, &sk, &format!("{base}/.well-known/jwks.json"));
    ([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "public, max-age=300")], card.to_string()).into_response()
}

async fn rpc_route(State(state): State<Arc<ApiState>>, Path((owner, bot)): Path<(String, String)>, headers: HeaderMap, body: Bytes) -> Response {
    if signing_key().is_none() {
        return not_configured();
    }
    if !valid_handle(&owner) || !valid_handle(&bot) {
        return not_found();
    }
    if body.len() > MAX_BODY {
        return (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({ "error": "too_large" }))).into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return rpc_error(Value::Null, -32700, "Parse error"),
    };
    let rpc_url = format!("{}/{owner}/{bot}/rpc", public_base());
    let caller = match identify(&state, &owner, &rpc_url, &headers).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let id = req["id"].clone();
    if caller.class == "blocked" {
        return rpc_error(id, -32003, REFUSED);
    }
    let extra = HashMap::from([(PEER_CLASS.to_string(), caller.class.to_string()), (PEER_ID.to_string(), caller.id.clone()), (PEER_NAME.to_string(), caller.name.chars().take(80).collect())]);
    match relay(&state, &owner, &format!("/webhooks/a2a-edge/rpc/{bot}"), &body, extra).await {
        Some((200, bytes)) => ([(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
        Some(_) => rpc_error(id, -32603, "The agent could not take this call."),
        None => rpc_error(id, -32001, "Agent not found, or its computer is offline."),
    }
}

// ─── contacts ─────────────────────────────────────────────────────────────────

async fn user_id(state: &ApiState, headers: &HeaderMap) -> Result<String, Response> {
    crate::auth::resolve_user_scoped(&state.db, headers, "compute").await.map(|u| u.id).map_err(|e| e.into_response())
}

fn internal(e: sqlx::Error) -> Response {
    tracing::error!("a2a contacts: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "internal" }))).into_response()
}

async fn contacts_list(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let owner = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    type Row = (String, String, String, bool, chrono::DateTime<chrono::Utc>);
    let rows: Result<Vec<Row>, _> = sqlx::query_as("SELECT peer_card_url, peer_name, status, card_changed, created_at FROM agent_contacts WHERE owner_user_id = $1 ORDER BY created_at DESC").bind(&owner).fetch_all(&state.db).await;
    match rows {
        Ok(r) => Json(json!({ "contacts": r.into_iter().map(|(url, name, status, changed, at)| json!({ "id": URL_SAFE_NO_PAD.encode(&url), "cardUrl": url, "name": name, "status": status, "cardChanged": changed, "createdAt": at })).collect::<Vec<_>>() })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddBody {
    card_url: String,
    /// `accepted` (default), `pending` or `blocked`.
    status: Option<String>,
}

async fn contacts_add(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(b): Json<AddBody>) -> Response {
    let owner = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let status = b.status.as_deref().unwrap_or("accepted");
    if !["pending", "accepted", "blocked"].contains(&status) {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_status" }))).into_response();
    }
    let Some(url) = safe_https_url(&b.card_url) else { return (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_card_url", "message": "Use the public https URL of the agent's card." }))).into_response() };
    let Some(bytes) = fetch_bytes(&url).await else { return (StatusCode::BAD_GATEWAY, Json(json!({ "error": "card_unreachable", "message": "Could not fetch that Agent Card." }))).into_response() };
    let Ok(card) = serde_json::from_slice::<Value>(&bytes) else { return (StatusCode::BAD_REQUEST, Json(json!({ "error": "not_an_agent_card" }))).into_response() };
    if !card["name"].is_string() {
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": "not_an_agent_card" }))).into_response();
    }
    let name: String = card["name"].as_str().unwrap_or_default().chars().take(80).collect();
    let res = sqlx::query(
        "INSERT INTO agent_contacts (owner_user_id, peer_card_url, peer_name, status, card_hash, card_changed) VALUES ($1,$2,$3,$4,$5,false)
         ON CONFLICT (owner_user_id, peer_card_url) DO UPDATE SET status = EXCLUDED.status, peer_name = EXCLUDED.peer_name, card_hash = EXCLUDED.card_hash, card_changed = false",
    )
    .bind(&owner)
    .bind(b.card_url.trim())
    .bind(&name)
    .bind(status)
    .bind(card_hash(&bytes))
    .execute(&state.db)
    .await;
    match res {
        Ok(_) => (StatusCode::CREATED, Json(json!({ "id": URL_SAFE_NO_PAD.encode(b.card_url.trim()), "cardUrl": b.card_url.trim(), "name": name, "status": status }))).into_response(),
        Err(e) => internal(e),
    }
}

async fn contacts_delete(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let owner = match user_id(&state, &headers).await {
        Ok(u) => u,
        Err(r) => return r,
    };
    let Some(url) = URL_SAFE_NO_PAD.decode(&id).ok().and_then(|b| String::from_utf8(b).ok()) else { return not_found() };
    match sqlx::query("DELETE FROM agent_contacts WHERE owner_user_id = $1 AND peer_card_url = $2").bind(&owner).bind(&url).execute(&state.db).await {
        Ok(r) if r.rows_affected() > 0 => Json(json!({ "ok": true })).into_response(),
        Ok(_) => not_found(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_signature_verifies_and_ssrf_targets_are_refused() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let mut card = json!({ "name": "Ada", "b": 1, "a": [1, 2] });
        sign_card(&mut card, &sk, "https://x.example/a2a/.well-known/jwks.json");
        let sig = &card["signatures"][0];
        let mut unsigned = card.clone();
        unsigned.as_object_mut().unwrap().remove("signatures");
        let signing_input = format!("{}.{}", sig["protected"].as_str().unwrap(), URL_SAFE_NO_PAD.encode(canonical(&unsigned)));
        let s = ed25519_dalek::Signature::from_slice(&URL_SAFE_NO_PAD.decode(sig["signature"].as_str().unwrap()).unwrap()).unwrap();
        assert!(sk.verifying_key().verify(signing_input.as_bytes(), &s).is_ok());
        assert_eq!(canonical(&json!({ "b": 1, "a": 2 })), r#"{"a":2,"b":1}"#);
        for bad in ["http://a.example/c", "https://localhost/c", "https://127.0.0.1/c", "https://10.0.0.5/c", "https://intranet/c", "https://[::1]/c", "https://u:p@a.example/c"] {
            assert!(safe_https_url(bad).is_none(), "{bad}");
        }
        assert!(safe_https_url("https://agent.example.com/card.json").is_some());
    }

    #[test]
    fn caller_jwt_must_be_signed_by_a_key_in_the_jwks_and_unexpired() {
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let jwks = jwks_for(&sk.verifying_key());
        let mk = |exp: i64| {
            let h = URL_SAFE_NO_PAD.encode(json!({ "alg": "EdDSA", "kid": key_id(&sk.verifying_key()) }).to_string());
            let p = URL_SAFE_NO_PAD.encode(json!({ "exp": exp, "iss": "https://p.example/card" }).to_string());
            format!("{h}.{p}.{}", URL_SAFE_NO_PAD.encode(sk.sign(format!("{h}.{p}").as_bytes()).to_bytes()))
        };
        let now = chrono::Utc::now().timestamp();
        assert!(verify_eddsa_jwt(&mk(now + 60), &jwks).is_some());
        assert!(verify_eddsa_jwt(&mk(now - 60), &jwks).is_none(), "expired");
        let other = jwks_for(&SigningKey::from_bytes(&[1u8; 32]).verifying_key());
        assert!(verify_eddsa_jwt(&mk(now + 60), &other).is_none(), "wrong key");
    }
}
