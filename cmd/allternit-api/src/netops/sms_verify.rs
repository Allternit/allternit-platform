//! SMS number verification for critical alerts (agent network WP10).
//!
//! A critical alert only texts a number the owner proved is theirs: we text a
//! 6-digit code from the bot's own number (through the same outbound SMS path as
//! the alerts), and the owner types it back. The code is stored hashed, lives 10
//! minutes and allows 5 tries; starting is limited to 3 texts per hour. The state
//! is one `runtime_settings` row (`alerts.smsVerify`), bound to the number it was
//! verified for, so changing the number un-verifies it.
//!
//! * `POST /alerts/sms/verify/start` `{phone?, botId?}`: text a code.
//! * `POST /alerts/sms/verify/confirm` `{code}`: mark the number verified.
//! * `GET  /alerts/sms/verify/status`: `{smsVerified, phone}`.
//!
//! [`is_verified`] is what the alerts sender checks before it texts.

use std::sync::Arc;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::auth::AuthUser;
use crate::AppState;

const VERIFY_KEY: &str = "alerts.smsVerify";
const SMS_KEY: &str = "alerts.criticalSms";
const CODE_TTL: Duration = Duration::minutes(10);
const MAX_TRIES: i64 = 5;
const MAX_STARTS_PER_HOUR: usize = 3;

#[derive(Debug, PartialEq)]
pub enum VerifyError {
    /// Three texts in the last hour already.
    RateLimited,
    /// No code is waiting (never started, or already used).
    NoCode,
    Expired,
    TooManyTries,
    /// Wrong code; this many tries are left.
    Wrong(i64),
}

impl VerifyError {
    pub fn sentence(&self) -> String {
        match self {
            Self::RateLimited => "You've asked for three codes in the last hour. Try again later.".into(),
            Self::NoCode => "There's no code waiting. Ask for a new one.".into(),
            Self::Expired => "That code has expired. Ask for a new one.".into(),
            Self::TooManyTries => "Too many wrong tries. Ask for a new code.".into(),
            Self::Wrong(left) => format!("That code isn't right. {left} {} left.", if *left == 1 { "try" } else { "tries" }),
        }
    }
}

fn load(conn: &Connection, owner: &str) -> Value {
    conn.query_row("SELECT value FROM runtime_settings WHERE user_id = ?1 AND key = ?2", params![owner, VERIFY_KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

fn save(conn: &Connection, owner: &str, v: &Value) {
    let _ = conn.execute(
        "INSERT INTO runtime_settings (user_id, key, value, updated_at) VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(user_id, key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
        params![owner, VERIFY_KEY, v.to_string()],
    );
}

fn hash_code(owner: &str, phone: &str, code: &str) -> String {
    hex::encode(Sha256::digest(format!("{owner}:{phone}:{code}").as_bytes()))
}

/// True when `phone` is the number this owner verified. The alerts sender skips every other number.
pub fn is_verified(conn: &Connection, owner: &str, phone: &str) -> bool {
    let v = load(conn, owner);
    v["verified"].as_bool() == Some(true) && v["phone"].as_str() == Some(phone)
}

/// Start verifying `phone`: returns the 6-digit code to text. Only its hash is kept.
pub fn begin(conn: &Connection, owner: &str, phone: &str, now: DateTime<Utc>) -> Result<String, VerifyError> {
    let prev = load(conn, owner);
    let mut sends: Vec<String> = prev["sends"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .filter(|s| DateTime::parse_from_rfc3339(s).is_ok_and(|t| now - t.with_timezone(&Utc) < Duration::hours(1)))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if sends.len() >= MAX_STARTS_PER_HOUR {
        return Err(VerifyError::RateLimited);
    }
    sends.push(now.to_rfc3339());
    let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000));
    // Asking again for the number that is already verified keeps it verified; any other number starts over.
    let keep = prev["verified"].as_bool() == Some(true) && prev["phone"].as_str() == Some(phone);
    save(
        conn,
        owner,
        &json!({ "phone": phone, "verified": keep, "hash": hash_code(owner, phone, &code), "expiresAt": (now + CODE_TTL).to_rfc3339(), "tries": 0, "sends": sends }),
    );
    Ok(code)
}

/// Check the code the owner typed. On success the number is verified and the code is gone.
pub fn confirm(conn: &Connection, owner: &str, code: &str, now: DateTime<Utc>) -> Result<(), VerifyError> {
    let mut v = load(conn, owner);
    let (Some(hash), Some(phone)) = (v["hash"].as_str().map(str::to_string), v["phone"].as_str().map(str::to_string)) else { return Err(VerifyError::NoCode) };
    if v["expiresAt"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map_or(true, |t| now > t.with_timezone(&Utc)) {
        return Err(VerifyError::Expired);
    }
    let tries = v["tries"].as_i64().unwrap_or(0);
    if tries >= MAX_TRIES {
        return Err(VerifyError::TooManyTries);
    }
    if hash_code(owner, &phone, code.trim()) != hash {
        v["tries"] = json!(tries + 1);
        save(conn, owner, &v);
        return Err(VerifyError::Wrong(MAX_TRIES - tries - 1));
    }
    v["verified"] = json!(true);
    v["hash"] = Value::Null;
    v["tries"] = json!(0);
    save(conn, owner, &v);
    Ok(())
}

/// The number critical texts go to: the one saved with the setting, else the owner's own phone in People.
fn destination(conn: &Connection, owner: &str) -> Option<String> {
    let v: Value = conn
        .query_row("SELECT value FROM runtime_settings WHERE user_id = ?1 AND key = ?2", params![owner, SMS_KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    v["phone"].as_str().filter(|p| !p.trim().is_empty()).map(str::to_string).or_else(|| {
        conn.query_row(
            "SELECT i.external_id FROM person_identities i JOIN people p ON p.id = i.person_id
             WHERE i.owner = ?1 AND i.kind = 'phone' AND p.name_source = 'owner' AND p.merged_into IS NULL ORDER BY i.created_at LIMIT 1",
            params![owner],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten()
    })
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": "SMS_VERIFY" }))).into_response()
}

async fn status_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>) -> Response {
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    let phone = destination(&conn, &user.user_id);
    let verified = phone.as_deref().is_some_and(|p| is_verified(&conn, &user.user_id, p));
    Json(json!({ "smsVerified": verified, "phone": phone })).into_response()
}

async fn start_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, body: Option<Json<Value>>) -> Response {
    let body = body.map(|b| b.0).unwrap_or(Value::Null);
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    let asked = body["phone"].as_str().map(str::trim).filter(|p| !p.is_empty());
    if asked.is_some_and(|p| !crate::channel_phone::is_e164(p)) {
        return err(StatusCode::BAD_REQUEST, "the phone number must look like +14155550123");
    }
    // The code can only go to the number alerts would text: a different one would verify nothing.
    let Some(phone) = destination(&conn, &user.user_id) else { return err(StatusCode::CONFLICT, "add your phone number first") };
    if asked.is_some_and(|p| p != phone) {
        return err(StatusCode::CONFLICT, "save this number first, then verify it");
    }
    let bot = body["botId"].as_str().filter(|b| !b.is_empty());
    let number = match crate::phone_outbound::pick_number(&state.db, &user.user_id, bot, None).or_else(|_| crate::phone_outbound::pick_number(&state.db, &user.user_id, None, None)) {
        Ok(n) => n,
        Err(e) => return err(StatusCode::CONFLICT, &e.sentence()),
    };
    let code = match begin(&conn, &user.user_id, &phone, Utc::now()) {
        Ok(c) => c,
        Err(e) => return err(StatusCode::TOO_MANY_REQUESTS, &e.sentence()),
    };
    let text = format!("Allternit: your verification code is {code}. It expires in 10 minutes.");
    match crate::phone_outbound::text(&state.db, &crate::phone_outbound::runtime(&state), Arc::new(crate::channel_transports::ReqwestSend), &number, &phone, &text).await {
        Ok(_) => Json(json!({ "sent": true, "phone": phone, "expiresInSeconds": CODE_TTL.num_seconds() })).into_response(),
        Err(e) => err(StatusCode::BAD_GATEWAY, &e.sentence()),
    }
}

async fn confirm_route(State(state): State<Arc<AppState>>, Extension(user): Extension<AuthUser>, Json(body): Json<Value>) -> Response {
    let Some(code) = body["code"].as_str().map(str::trim).filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit())) else {
        return err(StatusCode::BAD_REQUEST, "enter the 6-digit code");
    };
    let Ok(conn) = state.db.connect() else { return err(StatusCode::INTERNAL_SERVER_ERROR, "database unavailable") };
    match confirm(&conn, &user.user_id, code, Utc::now()) {
        Ok(()) => Json(json!({ "smsVerified": true })).into_response(),
        Err(e) => err(if e == VerifyError::NoCode { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST }, &e.sentence()),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/alerts/sms/verify/start", post(start_route))
        .route("/alerts/sms/verify/confirm", post(confirm_route))
        .route("/alerts/sms/verify/status", get(status_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE runtime_settings (user_id TEXT, key TEXT, value TEXT, updated_at TEXT, PRIMARY KEY (user_id, key));").unwrap();
        c
    }

    #[test]
    fn code_flow_wrong_expired_and_success_and_number_binding() {
        let c = conn();
        let now = Utc::now();
        assert!(!is_verified(&c, "u", "+14155550123"));
        assert_eq!(confirm(&c, "u", "123456", now), Err(VerifyError::NoCode));

        let code = begin(&c, "u", "+14155550123", now).unwrap();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        assert_eq!(confirm(&c, "u", wrong, now), Err(VerifyError::Wrong(4)));
        assert!(!is_verified(&c, "u", "+14155550123"));
        assert_eq!(confirm(&c, "u", &code, now + Duration::minutes(11)), Err(VerifyError::Expired));
        assert_eq!(confirm(&c, "u", &code, now + Duration::minutes(9)), Ok(()));
        assert!(is_verified(&c, "u", "+14155550123"));
        assert!(!is_verified(&c, "u", "+14155550999"), "a changed number is not verified");
        assert!(!is_verified(&c, "other", "+14155550123"));
        let raw: String = c.query_row("SELECT value FROM runtime_settings", [], |r| r.get(0)).unwrap();
        assert!(!raw.contains(&format!("\"{code}\"")), "the code itself is never stored");
    }

    #[test]
    fn five_tries_then_locked_and_three_starts_per_hour() {
        let c = conn();
        let now = Utc::now();
        let code = begin(&c, "u", "+14155550123", now).unwrap();
        let wrong = if code == "000000" { "000001" } else { "000000" };
        for _ in 0..5 {
            assert!(matches!(confirm(&c, "u", wrong, now), Err(VerifyError::Wrong(_))));
        }
        assert_eq!(confirm(&c, "u", &code, now), Err(VerifyError::TooManyTries), "even the right code is refused after 5 tries");
        begin(&c, "u", "+14155550123", now).unwrap();
        begin(&c, "u", "+14155550123", now).unwrap();
        assert_eq!(begin(&c, "u", "+14155550123", now), Err(VerifyError::RateLimited));
        assert!(begin(&c, "u", "+14155550123", now + Duration::minutes(61)).is_ok(), "the hour rolls over");
    }

    #[test]
    fn alerts_sender_skips_an_unverified_number() {
        // The one line in alerts::sms_pending: `sms_destination(..).filter(is_verified)`. No SMS is sent here.
        let c = conn();
        let now = Utc::now();
        let pick = |c: &Connection| Some("+14155550123".to_string()).filter(|p| is_verified(c, "u", p));
        assert_eq!(pick(&c), None);
        let code = begin(&c, "u", "+14155550123", now).unwrap();
        assert_eq!(pick(&c), None, "a code that was sent but not entered is still unverified");
        confirm(&c, "u", &code, now).unwrap();
        assert_eq!(pick(&c).as_deref(), Some("+14155550123"));
    }
}
