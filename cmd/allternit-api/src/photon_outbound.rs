//! Photon 2.0 (Beta) outbound texts: hold -> approved -> send.
//!
//! * Send: `POST {base}/v1/projects/{projectId}/messages` with header `Idempotency-Key` and body
//!   `{spaceId, senderId, content:{type:"text",format:"plain",text}}`. Photon answers 202 with the
//!   stored message (`id`, `status`) and delivers asynchronously.
//! * A shared iMessage line cannot start a conversation: the person must be added to the line
//!   (`POST /v1/projects/{id}/platforms/imessage/assignments`) and text the line first, which
//!   creates the space and the agent/user ids a send needs.
//! * Policy: the only allowed recipients are the owner's own handles (env
//!   `ALLTERNIT_PHOTON_OWNER_HANDLES`, comma separated E.164; empty means nobody). Every send is held
//!   in `photon_outbound_holds` until the owner approves it; approval sends it once.
//! * Credentials: the sealed vault entries `photon` (API key) and `photon_project` (project id) for
//!   the agent, falling back to env `PHOTON_API_KEY` / `PHOTON_PROJECT_ID`. Base URL: env
//!   `PHOTON_API_BASE` (default `https://api.photon.codes`).

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::db::DbHandle;

pub const DEFAULT_BASE: &str = "https://api.photon.codes";

#[derive(Debug, PartialEq)]
pub enum PhotonError {
    BadRequest(String),
    Forbidden(String),
    NotFound,
    NotConfigured,
    Conflict(String),
    Vendor(String),
    Internal(String),
}

pub fn api_base() -> String {
    std::env::var("PHOTON_API_BASE").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_BASE.to_string())
}

fn vault_value(db: &DbHandle, user_id: &str, agent_id: &str, provider: &str) -> Option<String> {
    let conn = db.connect().ok()?;
    let sealed: Option<String> = conn
        .query_row(
            "SELECT encrypted_value FROM allternit_vault_credentials WHERE user_id = ?1 AND provider = ?2 AND agent_id = ?3 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) ORDER BY updated_at DESC LIMIT 1",
            params![user_id, provider, agent_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    sealed.map(|s| crate::token_crypto::open(&s)).filter(|v| !v.is_empty())
}

/// (project id, API key) from the vault, else env.
pub fn credentials(db: &DbHandle, user_id: &str, agent_id: &str) -> Result<(String, String), PhotonError> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let key = vault_value(db, user_id, agent_id, "photon").or_else(|| env("PHOTON_API_KEY"));
    let project = vault_value(db, user_id, agent_id, "photon_project").or_else(|| env("PHOTON_PROJECT_ID"));
    match (project, key) {
        (Some(p), Some(k)) => Ok((p, k)),
        _ => Err(PhotonError::NotConfigured),
    }
}

/// Send one plain-text message. Returns Photon's stored message (`id`, `status`, ...).
/// `idempotency_key` makes a retry reuse the same message.
pub async fn send_photon_outbound_message(
    base: &str,
    project_id: &str,
    api_key: &str,
    space_id: &str,
    sender_id: &str,
    text: &str,
    idempotency_key: &str,
) -> Result<Value, String> {
    let url = format!("{}/v1/projects/{}/messages", base.trim_end_matches('/'), project_id);
    let resp = crate::net_client::client()
        .post(url)
        .bearer_auth(api_key)
        .header("Idempotency-Key", idempotency_key)
        .json(&json!({
            "spaceId": space_id,
            "senderId": sender_id,
            "content": { "type": "text", "format": "plain", "text": text },
        }))
        .send()
        .await
        .map_err(|e| format!("Photon API error: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        resp.json::<Value>().await.map_err(|e| format!("Photon answered {status} with an unreadable body: {e}"))
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!("Photon API rejected message ({status}): {}", body.chars().take(300).collect::<String>()))
    }
}

fn owner_handles() -> Vec<String> {
    std::env::var("ALLTERNIT_PHOTON_OWNER_HANDLES").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Queue a text for approval. Refuses any recipient that is not one of the owner's handles.
pub fn request_hold(db: &DbHandle, user_id: &str, agent_id: &str, to: &str, space_id: &str, sender_id: &str, text: &str) -> Result<String, PhotonError> {
    if text.trim().is_empty() || text.len() > 2000 {
        return Err(PhotonError::BadRequest("The text must be 1 to 2000 characters.".into()));
    }
    if !space_id.starts_with("pho_spc_") || !sender_id.starts_with("pho_usr_") {
        return Err(PhotonError::BadRequest("spaceId and senderId must be Photon ids (pho_spc_..., pho_usr_...). They come from the person's first message to the line.".into()));
    }
    if !owner_handles().iter().any(|h| h == to) {
        return Err(PhotonError::Forbidden("Photon texts may only go to the owner's own handles.".into()));
    }
    let id = format!("pho_hold_{}", uuid::Uuid::new_v4().simple());
    let conn = db.connect().map_err(|e| PhotonError::Internal(e.to_string()))?;
    conn.execute(
        "INSERT INTO photon_outbound_holds (id, agent_id, user_id, to_handle, space_id, sender_id, content) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![id, agent_id, user_id, to, space_id, sender_id, text],
    )
    .map_err(|e| PhotonError::Internal(e.to_string()))?;
    Ok(id)
}

/// Owner approves (or rejects) a held text. Approving sends it once; a second approve is a 409.
pub async fn decide(db: &DbHandle, user_id: &str, hold_id: &str, approve: bool, base: &str) -> Result<Value, PhotonError> {
    let internal = |e: rusqlite::Error| PhotonError::Internal(e.to_string());
    let conn = db.connect().map_err(|e| PhotonError::Internal(e.to_string()))?;
    let row: Option<(String, String, String, String, String)> = conn
        .query_row(
            "SELECT agent_id, space_id, sender_id, content, state FROM photon_outbound_holds WHERE id = ?1 AND user_id = ?2",
            params![hold_id, user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(internal)?;
    let Some((agent_id, space_id, sender_id, content, state)) = row else { return Err(PhotonError::NotFound) };
    if state != "pending" {
        return Err(PhotonError::Conflict(format!("This text is already {state}.")));
    }
    if !approve {
        conn.execute("UPDATE photon_outbound_holds SET state='rejected', decided_at=CURRENT_TIMESTAMP WHERE id=?1", params![hold_id]).map_err(internal)?;
        return Ok(json!({ "id": hold_id, "state": "rejected" }));
    }
    let (project, key) = credentials(db, user_id, &agent_id)?;
    // Claim it before the network call so two approvals cannot both send.
    let claimed = conn
        .execute("UPDATE photon_outbound_holds SET state='approved', decided_at=CURRENT_TIMESTAMP WHERE id=?1 AND state='pending'", params![hold_id])
        .map_err(internal)?;
    if claimed == 0 {
        return Err(PhotonError::Conflict("This text was just decided.".into()));
    }
    match send_photon_outbound_message(base, &project, &key, &space_id, &sender_id, &content, hold_id).await {
        Ok(v) => {
            let (mid, st) = (v["id"].as_str().unwrap_or(""), v["status"].as_str().unwrap_or("dispatched"));
            conn.execute("UPDATE photon_outbound_holds SET state='sent', vendor_message_id=?2, vendor_status=?3 WHERE id=?1", params![hold_id, mid, st]).map_err(internal)?;
            Ok(json!({ "id": hold_id, "state": "sent", "vendorMessageId": mid, "vendorStatus": st }))
        }
        Err(e) => {
            conn.execute("UPDATE photon_outbound_holds SET state='failed', error=?2 WHERE id=?1", params![hold_id, e]).map_err(internal)?;
            Err(PhotonError::Vendor(e))
        }
    }
}


// ---------------------------------------------------------------- inbound webhook

/// Standard Webhooks check: HMAC-SHA256 over `id.timestamp.body` with the base64-decoded
/// `whsec_...` secret, any one of the space-separated `v1,<base64>` signatures, a 5-minute
/// timestamp window, constant-time compare. An empty secret rejects everything.
pub fn verify_webhook(secret: &str, id: &str, timestamp: &str, signature: &str, body: &[u8], now_unix: i64) -> Result<(), &'static str> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let secret = secret.trim();
    if secret.is_empty() {
        return Err("webhook secret not configured");
    }
    if id.is_empty() || signature.is_empty() {
        return Err("missing webhook headers");
    }
    let ts: i64 = timestamp.parse().map_err(|_| "bad webhook-timestamp")?;
    if (now_unix - ts).abs() > 300 {
        return Err("webhook-timestamp outside the 5 minute window");
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let key = b64.decode(secret.strip_prefix("whsec_").unwrap_or(secret)).map_err(|_| "webhook secret is not base64")?;
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).map_err(|_| "bad webhook secret")?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    // `verify_slice` style compare: constant time against each offered signature.
    let ok = signature.split(' ').filter_map(|p| p.strip_prefix("v1,")).filter_map(|v| b64.decode(v).ok()).fold(false, |acc, sig| {
        use subtle_eq::ct_eq;
        acc | ct_eq(&sig, expected.as_slice())
    });
    if ok { Ok(()) } else { Err("signature mismatch") }
}

mod subtle_eq {
    /// Constant-time equality for byte slices (length is not secret).
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
    }
}

/// Store a verified `message.received` event in the agent's inbox (idempotent on the event id).
/// Returns the agent id it was routed to, or None when the event is another type or no agent
/// owns the receiving number.
pub fn ingest_event(db: &DbHandle, event_id: &str, event: &Value) -> Result<Option<String>, PhotonError> {
    if event["type"] != "message.received" {
        return Ok(None);
    }
    let m = &event["data"]["object"];
    let (to, from) = (m["receivedBy"]["handle"].as_str().unwrap_or(""), m["sentBy"]["handle"].as_str().unwrap_or(""));
    let text = m["content"]["text"].as_str().unwrap_or("");
    if to.is_empty() || from.is_empty() {
        return Err(PhotonError::BadRequest("event has no sender or receiver handle".into()));
    }
    let conn = db.connect().map_err(|e| PhotonError::Internal(e.to_string()))?;
    let agent: Option<String> = conn
        .query_row("SELECT agent_id FROM agent_identity_channels WHERE phone_provider = 'photon' AND phone_number = ?1", params![to], |r| r.get(0))
        .optional()
        .map_err(|e| PhotonError::Internal(e.to_string()))?;
    let Some(agent) = agent else { return Ok(None) };
    // Content for non-text messages (images) is kept as a short marker, not the raw object.
    let content = if text.is_empty() { format!("[{} message]", m["content"]["type"].as_str().unwrap_or("non-text")) } else { text.to_string() };
    conn.execute(
        "INSERT OR IGNORE INTO agent_photon_inbox (id, agent_id, from_id, to_id, content, surface, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![event_id, agent, from, to, content, m["platform"].as_str().unwrap_or("imessage"), chrono::Utc::now().to_rfc3339()],
    )
    .map_err(|e| PhotonError::Internal(e.to_string()))?;
    Ok(Some(agent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// The vendor, mocked at the network edge only: a local server on the real path.
    async fn mock_photon(status: u16) -> (String, Seen) {
        let seen: Seen = Arc::default();
        async fn h(State((seen, status)): State<(Seen, u16)>, headers: HeaderMap, Json(b): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
            let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let idem = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            seen.lock().unwrap().push((auth, idem, b));
            let code = axum::http::StatusCode::from_u16(status).unwrap();
            if status == 202 { (code, Json(json!({ "id": "pho_msg_mock", "status": "dispatched" }))) } else { (code, Json(json!({ "error": "NO_SPACE" }))) }
        }
        let app = Router::new().route("/v1/projects/:id/messages", post(h)).with_state((seen.clone(), status));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (base, seen)
    }

    fn db() -> (DbHandle, tempfile::TempDir) {
        let t = tempfile::tempdir().unwrap();
        let db = DbHandle::new(t.path().join("t.db")).unwrap();
        (db, t)
    }

    const SPC: &str = "pho_spc_01m76dje8pfqbakph2g1dpr4cq";
    const USR: &str = "pho_usr_01m76dje8pfqbakph2g1dpr4cq";

    #[tokio::test]
    async fn hold_approve_send_hits_the_real_beta_endpoint_once() {
        std::env::set_var("ALLTERNIT_PHOTON_OWNER_HANDLES", "+17735520158");
        std::env::set_var("PHOTON_API_KEY", "test-key");
        std::env::set_var("PHOTON_PROJECT_ID", "pho_prj_test");
        let (db, _t) = db();
        let (base, seen) = mock_photon(202).await;
        let id = request_hold(&db, "u1", "a1", "+17735520158", SPC, USR, "Allternit test: reply OK").unwrap();
        assert!(seen.lock().unwrap().is_empty(), "nothing is sent while held");
        let out = decide(&db, "u1", &id, true, &base).await.unwrap();
        assert_eq!(out["state"], "sent");
        assert_eq!(out["vendorMessageId"], "pho_msg_mock");
        let s = seen.lock().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, "Bearer test-key");
        assert_eq!(s[0].1, id);
        assert_eq!(s[0].2, json!({ "spaceId": SPC, "senderId": USR, "content": { "type": "text", "format": "plain", "text": "Allternit test: reply OK" } }));
        drop(s);
        assert!(matches!(decide(&db, "u1", &id, true, &base).await, Err(PhotonError::Conflict(_))));
        assert_eq!(seen.lock().unwrap().len(), 1, "a second approval does not resend");
    }

    #[tokio::test]
    async fn only_the_owner_handle_other_users_and_rejects_never_send() {
        std::env::set_var("ALLTERNIT_PHOTON_OWNER_HANDLES", "+17735520158");
        std::env::set_var("PHOTON_API_KEY", "test-key");
        std::env::set_var("PHOTON_PROJECT_ID", "pho_prj_test");
        let (db, _t) = db();
        let (base, seen) = mock_photon(202).await;
        assert!(matches!(request_hold(&db, "u1", "a1", "+12025550123", SPC, USR, "hi"), Err(PhotonError::Forbidden(_))));
        let id = request_hold(&db, "u1", "a1", "+17735520158", SPC, USR, "hi").unwrap();
        assert_eq!(decide(&db, "u2", &id, true, &base).await, Err(PhotonError::NotFound));
        assert_eq!(decide(&db, "u1", &id, false, &base).await.unwrap()["state"], "rejected");
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_vendor_refusal_is_recorded_as_failed() {
        std::env::set_var("ALLTERNIT_PHOTON_OWNER_HANDLES", "+17735520158");
        std::env::set_var("PHOTON_API_KEY", "test-key");
        std::env::set_var("PHOTON_PROJECT_ID", "pho_prj_test");
        let (db, _t) = db();
        let (base, _seen) = mock_photon(422).await;
        let id = request_hold(&db, "u1", "a1", "+17735520158", SPC, USR, "hi").unwrap();
        assert!(matches!(decide(&db, "u1", &id, true, &base).await, Err(PhotonError::Vendor(_))));
        let st: String = db.connect().unwrap().query_row("SELECT state FROM photon_outbound_holds WHERE id=?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(st, "failed");
    }

    fn sign(secret_b64: &str, id: &str, ts: i64, body: &str) -> String {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&b64.decode(secret_b64).unwrap()).unwrap();
        mac.update(format!("{id}.{ts}.{body}").as_bytes());
        format!("v1,{}", b64.encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn webhook_signature_accepts_valid_and_rejects_bad_old_or_unsigned() {
        use base64::Engine;
        let enc = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let secret_b64 = enc(b"fixture bytes one");
        let secret_b64 = secret_b64.as_str();
        let secret = format!("whsec_{secret_b64}");
        let (id, now, body) = ("evt_1", 1_800_000_000i64, r#"{"type":"message.received"}"#);
        let good = sign(secret_b64, id, now, body);
        assert_eq!(verify_webhook(&secret, id, &now.to_string(), &good, body.as_bytes(), now + 10), Ok(()));
        // one valid among several offered signatures (rotation)
        assert_eq!(verify_webhook(&secret, id, &now.to_string(), &format!("v1,AAAA {good}"), body.as_bytes(), now), Ok(()));
        // tampered body, wrong secret, wrong id
        assert!(verify_webhook(&secret, id, &now.to_string(), &good, b"{}", now).is_err());
        assert!(verify_webhook(&format!("whsec_{}", enc(b"fixture bytes two")), id, &now.to_string(), &good, body.as_bytes(), now).is_err());
        assert!(verify_webhook(&secret, "evt_2", &now.to_string(), &good, body.as_bytes(), now).is_err());
        // older or newer than 5 minutes
        assert!(verify_webhook(&secret, id, &now.to_string(), &good, body.as_bytes(), now + 301).is_err());
        assert!(verify_webhook(&secret, id, &now.to_string(), &good, body.as_bytes(), now - 301).is_err());
        // unsigned, and no secret configured
        assert!(verify_webhook(&secret, id, &now.to_string(), "", body.as_bytes(), now).is_err());
        assert!(verify_webhook("", id, &now.to_string(), &good, body.as_bytes(), now).is_err());
    }

    #[test]
    fn inbound_event_lands_in_the_agents_inbox_once() {
        let (db, _t) = db();
        let conn = db.connect().unwrap();
        conn.execute("INSERT INTO agent_identity_channels (id, agent_id, user_id, phone_provider, phone_number) VALUES ('c1','a1','u1','photon','+16289979661')", []).unwrap();
        let ev = json!({ "id": "evt_9", "type": "message.received", "source": "imessage.photon", "apiVersion": "2026-07-01",
            "data": { "projectId": "pho_prj_x", "object": { "platform": "imessage", "status": "received",
                "content": { "type": "text", "format": "plain", "text": "ping" },
                "sentBy": { "handle": "+17735520158", "kind": "human" }, "receivedBy": { "handle": "+16289979661", "kind": "agent" } } } });
        assert_eq!(ingest_event(&db, "evt_9", &ev).unwrap().as_deref(), Some("a1"));
        assert_eq!(ingest_event(&db, "evt_9", &ev).unwrap().as_deref(), Some("a1"));
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM agent_photon_inbox WHERE id='evt_9' AND content='ping' AND from_id='+17735520158'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(ingest_event(&db, "e2", &json!({"type":"message.delivered"})).unwrap(), None);
    }
}
