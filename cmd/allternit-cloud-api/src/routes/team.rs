//! Team roster (agent-network phase 2, WP13).
//!
//! * Runtimes report live CLI sessions (`LiveSession`, contract C6) in the
//!   presence heartbeat; [`store_sessions`] keeps them in `team_sessions`
//!   (pruned after 24 h by the presence sweeper).
//! * `GET /api/v1/team/roster`: people → machines → sessions/bots of the
//!   caller's Clerk org (just the caller's own machines with no org).
//! * `POST /api/v1/team/send {to, message, replyTo?}`: resolves the address to
//!   the owner's runtime and relays it signed to allternit-api
//!   `POST /bot-routing/message` with `from_kind: "team"`. Returns C1
//!   `{hopId, state, tier, note}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use super::runtime_relay::{relay_signed_request_to_runtime_with, RelayRequest};
use crate::{ApiError, ApiState};

const MAX_SESSIONS: usize = 100;
const MAX_BODY: usize = 64 * 1024;
const PRUNE_HOURS: i32 = 24;
const ONLINE_WITHIN_SECS: i64 = 180;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/api/v1/team/roster", get(roster))
        .route("/api/v1/team/send", post(send))
}

/// Keep only well-formed sessions: an address and a harness, `doing` ≤ 120 chars.
pub fn clean_sessions(parsed: &Value) -> Vec<(String, Value)> {
    parsed["sessions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let address = s["address"].as_str().filter(|x| !x.is_empty() && x.len() <= 200 && x.contains('/'))?;
                    s["harness"].as_str().filter(|x| !x.is_empty() && x.len() <= 32)?;
                    let mut v = s.clone();
                    if let Some(d) = s["doing"].as_str() {
                        v["doing"] = Value::from(d.chars().take(120).collect::<String>());
                    }
                    Some((address.to_string(), v))
                })
                .take(MAX_SESSIONS)
                .collect()
        })
        .unwrap_or_default()
}

/// Replace this runtime's session rows with what its heartbeat reported.
pub async fn store_sessions(db: &PgPool, user_id: &str, runtime_id: &str, parsed: &Value) -> Result<(), sqlx::Error> {
    if parsed.get("sessions").is_none() {
        return Ok(());
    }
    let org: String = sqlx::query_scalar("SELECT org_id FROM team_members_seen WHERE user_id = $1 ORDER BY last_seen DESC LIMIT 1")
        .bind(user_id)
        .fetch_optional(db)
        .await?
        .unwrap_or_default();
    let list = clean_sessions(parsed);
    let addresses: Vec<String> = list.iter().map(|(a, _)| a.clone()).collect();
    sqlx::query("DELETE FROM team_sessions WHERE runtime_id = $1 AND NOT (address = ANY($2))")
        .bind(runtime_id)
        .bind(&addresses)
        .execute(db)
        .await?;
    for (address, payload) in list {
        sqlx::query(
            "INSERT INTO team_sessions (runtime_id, user_id, org_id, address, payload, updated_at) VALUES ($1,$2,$3,$4,$5,now()) \
             ON CONFLICT (runtime_id, address) DO UPDATE SET user_id = $2, org_id = $3, payload = $5, updated_at = now()",
        )
        .bind(runtime_id)
        .bind(user_id)
        .bind(&org)
        .bind(address)
        .bind(payload)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Drop sessions not seen for 24 h.
pub async fn prune(db: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM team_sessions WHERE updated_at < now() - make_interval(hours => $1)")
        .bind(PRUNE_HOURS)
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub runtime_id: String,
    pub user_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct MachineRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub online: bool,
}

/// people → machines → sessions + bots. `names` maps user id → display name.
pub fn build_roster(me: &str, machines: &[MachineRow], sessions: &[SessionRow], names: &HashMap<String, String>) -> Value {
    let mut people: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for m in machines {
        let mut list = Vec::new();
        let mut unread = 0i64;
        let mut held = 0i64;
        let mut bots = Vec::new();
        for s in sessions.iter().filter(|s| s.runtime_id == m.id) {
            unread += s.payload["unread"].as_i64().unwrap_or(0);
            held += s.payload["held"].as_i64().unwrap_or(0);
            if s.payload["botId"].as_str().is_some_and(|b| !b.is_empty()) {
                bots.push(s.payload.clone());
            } else {
                list.push(s.payload.clone());
            }
        }
        people.entry(m.user_id.clone()).or_default().push(json!({
            "runtimeId": m.id, "name": m.name, "online": m.online,
            "sessions": list, "bots": bots, "unread": unread, "held": held,
        }));
    }
    let mut out: Vec<Value> = people
        .into_iter()
        .map(|(uid, machines)| {
            let unread: i64 = machines.iter().map(|m| m["unread"].as_i64().unwrap_or(0)).sum();
            let held: i64 = machines.iter().map(|m| m["held"].as_i64().unwrap_or(0)).sum();
            json!({
                "userId": uid, "display": names.get(&uid).cloned().unwrap_or_else(|| "Teammate".into()),
                "me": uid == me, "machines": machines, "unread": unread, "held": held,
            })
        })
        .collect();
    out.sort_by_key(|p| !p["me"].as_bool().unwrap_or(false));
    json!({ "people": out })
}

fn display_of(user: &crate::auth::resolve::ResolvedUser) -> String {
    user.name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or_else(|| user.email.as_ref().and_then(|e| e.split('@').next()).map(str::to_string))
        .unwrap_or_else(|| "Me".into())
}

async fn roster(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let org = user.organization_id.clone().filter(|o| !o.is_empty());
    let display = display_of(&user);
    if let Some(org) = &org {
        sqlx::query(
            "INSERT INTO team_members_seen (org_id, user_id, display, last_seen) VALUES ($1,$2,$3,now()) \
             ON CONFLICT (org_id, user_id) DO UPDATE SET display = $3, last_seen = now()",
        )
        .bind(org)
        .bind(&user.id)
        .bind(&display)
        .execute(&state.db)
        .await?;
    }
    let org_s = org.clone().unwrap_or_default();
    let member_ids: Vec<String> = if org.is_some() {
        sqlx::query_scalar("SELECT user_id FROM team_members_seen WHERE org_id = $1")
            .bind(&org_s)
            .fetch_all(&state.db)
            .await?
    } else {
        vec![]
    };
    let mut ids = member_ids;
    if !ids.contains(&user.id) {
        ids.push(user.id.clone());
    }
    let names: HashMap<String, String> = if org.is_some() {
        sqlx::query_as::<_, (String, String)>("SELECT user_id, display FROM team_members_seen WHERE org_id = $1")
            .bind(&org_s)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect()
    } else {
        HashMap::from([(user.id.clone(), display)])
    };
    let rows: Vec<(String, String, Option<String>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, user_id, name, last_heartbeat_at FROM runtime_devices \
         WHERE user_id = ANY($1) AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let connected = super::runtime_relay::connected_runtime_ids().await;
    let now = Utc::now();
    let machines: Vec<MachineRow> = rows
        .into_iter()
        .map(|(id, user_id, name, beat)| MachineRow {
            online: connected.contains(&id) || beat.is_some_and(|b| (now - b).num_seconds() < ONLINE_WITHIN_SECS),
            name: name.unwrap_or_else(|| "Computer".into()),
            id,
            user_id,
        })
        .collect();
    let srows: Vec<(String, String, Value)> =
        sqlx::query_as("SELECT runtime_id, user_id, payload FROM team_sessions WHERE user_id = ANY($1) ORDER BY address")
            .bind(&ids)
            .fetch_all(&state.db)
            .await?;
    let sessions: Vec<SessionRow> = srows
        .into_iter()
        .filter(|(rid, _, _)| machines.iter().any(|m| &m.id == rid))
        .map(|(runtime_id, user_id, payload)| SessionRow { runtime_id, user_id, payload })
        .collect();
    let mut body = build_roster(&user.id, &machines, &sessions, &names);
    body["org"] = json!(org);
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct SendBody {
    to: String,
    message: String,
    #[serde(default, rename = "replyTo")]
    reply_to: Option<String>,
}

fn failed(note: &str) -> Response {
    Json(json!({ "hopId": null, "state": "failed", "tier": "pull", "note": note })).into_response()
}

async fn send(State(state): State<Arc<ApiState>>, headers: HeaderMap, Json(body): Json<SendBody>) -> Result<Response, ApiError> {
    let user = crate::auth::resolve_user_scoped(&state.db, &headers, "compute").await?;
    let to = body.to.trim().to_string();
    if to.is_empty() || !to.contains('/') || body.message.trim().is_empty() {
        return Err(ApiError::BadRequest("`to` must be person/session and `message` is required".into()));
    }
    if body.message.len() > MAX_BODY {
        return Err(ApiError::BadRequest("Message is over 64 KiB".into()));
    }
    let org = user.organization_id.clone().unwrap_or_default();
    // Same org (or my own machine); an address outside the org does not resolve.
    let target: Option<(String, String)> = sqlx::query_as(
        "SELECT t.runtime_id, t.user_id FROM team_sessions t JOIN runtime_devices d ON d.id = t.runtime_id AND d.revoked_at IS NULL \
         WHERE lower(t.address) = lower($1) AND (t.user_id = $2 OR (t.org_id <> '' AND t.org_id = $3)) \
         ORDER BY t.updated_at DESC LIMIT 1",
    )
    .bind(&to)
    .bind(&user.id)
    .bind(&org)
    .fetch_optional(&state.db)
    .await?;
    let Some((runtime_id, owner)) = target else {
        return Ok(failed("No teammate session with that address is reporting right now."));
    };
    let payload = json!({
        "from_kind": "team", "trust": "team",
        "from": { "userId": user.id, "display": display_of(&user) },
        "target": to, "message": body.message, "replyTo": body.reply_to,
    });
    let jwt = crate::auth::dataplane_jwt::mint(&owner, &runtime_id, "runtime:execute")
        .map_err(|e| ApiError::Internal(format!("data-plane token unavailable: {e}")))?;
    let mut trusted = HashMap::new();
    trusted.insert("authorization".to_string(), format!("Bearer {jwt}"));
    let resp = relay_signed_request_to_runtime_with(
        &state.db,
        &state.contabo_runtime_service,
        &state.quota_service,
        &state.provisioning_service,
        &owner,
        &runtime_id,
        RelayRequest {
            method: "POST".into(),
            path: "/api/v1/bot-routing/message".into(),
            headers: HashMap::new(),
            body: STANDARD.encode(payload.to_string()),
            body_encoding: "base64".into(),
        },
        &[],
        trusted,
    )
    .await;
    let resp = match resp {
        Ok(r) => r,
        Err(_) => return Ok(failed("Their computer is not reachable, so nothing was sent.")),
    };
    let ok = resp.status().is_success();
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024).await.unwrap_or_default();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok(Json(normalize_c1(&v, ok)).into_response())
}

/// Shape an allternit-api answer as C1 `{hopId, state, tier, note}`.
pub fn normalize_c1(v: &Value, ok: bool) -> Value {
    let state = match v["state"].as_str() {
        Some(s @ ("delivered" | "queued" | "held" | "refused" | "failed")) => s,
        _ if ok => "queued",
        _ => "failed",
    };
    let tier = match v["tier"].as_str() {
        Some(t @ ("push" | "turn" | "pull")) => t,
        _ => "pull",
    };
    let note = v["note"].as_str().map(str::to_string).unwrap_or_else(|| match state {
        "delivered" => "Delivered.".into(),
        "queued" => "Queued; it lands when their session next checks in.".into(),
        "held" => "Held for their approval.".into(),
        "refused" => "Their agent refused the message.".into(),
        _ => "Could not be delivered.".into(),
    });
    json!({ "hopId": v.get("hopId").or_else(|| v.get("hop_id")).cloned().unwrap_or(Value::Null), "state": state, "tier": tier, "note": note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_groups_people_machines_and_bots() {
        let machines = vec![
            MachineRow { id: "r1".into(), user_id: "u1".into(), name: "MacBook".into(), online: true },
            MachineRow { id: "r2".into(), user_id: "u2".into(), name: "Studio".into(), online: false },
        ];
        let s = |rt: &str, u: &str, p: Value| SessionRow { runtime_id: rt.into(), user_id: u.into(), payload: p };
        let sessions = vec![
            s("r1", "u1", json!({"address":"eoj/a-claude","harness":"claude","unread":2})),
            s("r1", "u1", json!({"address":"eoj/gizzi","harness":"gizzi","botId":"b1","held":1})),
            s("r2", "u2", json!({"address":"sam/x-codex","harness":"codex"})),
        ];
        let names = HashMap::from([("u1".to_string(), "Eoj".to_string())]);
        let r = build_roster("u2", &machines, &sessions, &names);
        assert_eq!(r["people"][0]["userId"], "u2"); // me first
        assert_eq!(r["people"][1]["display"], "Eoj");
        assert_eq!(r["people"][1]["machines"][0]["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(r["people"][1]["machines"][0]["bots"].as_array().unwrap().len(), 1);
        assert_eq!(r["people"][1]["unread"], 2);
        assert_eq!(r["people"][1]["held"], 1);
        assert_eq!(r["people"][0]["display"], "Teammate");
    }

    #[test]
    fn sessions_are_cleaned_and_c1_normalized() {
        let long = "x".repeat(300);
        let c = clean_sessions(&json!({"sessions":[{"address":"a/b","harness":"claude","doing":long},{"address":"nope","harness":"x"},{"harness":"y"}]}));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1["doing"].as_str().unwrap().len(), 120);
        let n = normalize_c1(&json!({"state":"delivered","tier":"push","hopId":"h1"}), true);
        assert_eq!(n["note"], "Delivered.");
        assert_eq!(normalize_c1(&Value::Null, false)["state"], "failed");
    }
}
