//! `/v1/messages`: texts on a project's numbers (scope `messaging`).
//!
//! Sending goes through the same rules as the Allternit app's numbers
//! (`phone::send_sms`): the number's texting must be active (carrier
//! registration approved), the recipient must not have sent STOP, and they must
//! have texted the number first or have consent recorded
//! (`POST /v1/numbers/{id}/consent`). A sandbox (simulated) number checks the
//! same STOP and consent rules but never reaches a carrier; its texts are
//! recorded with status `simulated`.
//!
//! Inbound texts on a live number arrive through the carrier (verified, deduped,
//! STOP/HELP/START answered by `phone::edge_core`), are kept here and sent as a
//! `message.received` event. Usage: one `sms_segment` per segment, each way.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::events::{emit_event, emit_for_number};
use super::numbers::{api_number, phone_error};
use super::{build_page, new_id, record_usage, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable, UsageEvent};
use crate::routes::phone;
use crate::ApiState;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/messages", &["GET", "POST"], get(list_messages).post(send_message))
        .add("/v1/messages/:id", &["GET"], get(get_message))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Message {
    pub id: String,
    pub object: String,
    pub account_id: Option<String>,
    pub number_id: String,
    pub direction: String,
    #[sqlx(rename = "from_e164")]
    pub from: String,
    #[sqlx(rename = "to_e164")]
    pub to: String,
    pub body: String,
    pub segments: i32,
    pub status: String,
    pub error_code: Option<String>,
    pub media: Value,
    pub created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, 'message'::text AS object, account_id, number_id, direction, from_e164, to_e164, body, segments, status, error_code, media, created_at";

/// SMS segments for `text`: GSM-7 is 160 per message (153 when split),
/// anything else (UCS-2) is 70 (67 when split).
pub fn segments(text: &str) -> i32 {
    const GSM: &str = "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞÆæßÉ !\"#¤%&'()*+,-./0123456789:;<=>?¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§¿abcdefghijklmnopqrstuvwxyzäöñüà^{}\\[~]|€";
    let gsm = text.chars().all(|c| GSM.contains(c));
    let n = text.chars().count().max(1) as i32;
    let (single, multi) = if gsm { (160, 153) } else { (70, 67) };
    if n <= single {
        1
    } else {
        (n + multi - 1) / multi
    }
}

#[derive(Debug, Deserialize)]
struct SendBody {
    number_id: String,
    to: String,
    body: String,
}

async fn insert(db: &PgPool, m: &Message, project_id: &str, carrier_id: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO platform_messages (id, project_id, account_id, number_id, direction, from_e164, to_e164, body, segments, status, error_code, carrier_message_id, media, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(&m.id)
    .bind(project_id)
    .bind(&m.account_id)
    .bind(&m.number_id)
    .bind(&m.direction)
    .bind(&m.from)
    .bind(&m.to)
    .bind(&m.body)
    .bind(m.segments)
    .bind(&m.status)
    .bind(&m.error_code)
    .bind(carrier_id)
    .bind(&m.media)
    .bind(m.created_at)
    .execute(db)
    .await?;
    Ok(())
}

async fn send_message(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiJson(body): ApiJson<SendBody>,
) -> Result<(StatusCode, Json<Message>), PlatformError> {
    caller.require("messaging")?;
    let number = api_number(&state.db, &caller, &body.number_id).await?;
    let text = body.body;
    let carrier = if number.simulated { None } else { Some(phone::carrier().map_err(phone_error)?) };
    send_inner(&state.db, &caller, &number, &body.to, &text, carrier.as_deref()).await.map(|m| (StatusCode::CREATED, Json(m)))
}

/// Send and record one text. `carrier` is `None` for a simulated number.
pub async fn send_inner(
    db: &PgPool,
    caller: &PlatformCaller,
    number: &super::numbers::ApiNumber,
    to: &str,
    text: &str,
    carrier: Option<&dyn crate::carriers::Carrier>,
) -> Result<Message, PlatformError> {
    // Card on file for every project (sandbox too: no free usage); a real text
    // is billable, so it is also refused once the project hit its spend cap.
    super::billing::require_payment_method(db, &caller.project_id).await?;
    let (status, carrier_id, parts) = match carrier {
        Some(carrier) => {
            super::spend_allowed(db, &caller.project_id).await?;
            let out = phone::send_sms(db, carrier, &caller.owner_user_id, &number.id, to, text).await.map_err(phone_error)?;
            let parts = out["parts"].as_i64().map(|p| p as i32).unwrap_or_else(|| segments(text));
            ("sent", out["messageId"].as_str().map(str::to_string), parts)
        }
        None => {
            // Same rules a real send enforces, without a carrier.
            if !crate::carriers::is_e164(to) {
                return Err(PlatformError::invalid_request("invalid_to", "to must be an E.164 number like +14155550101.").with_param("to"));
            }
            if text.trim().is_empty() {
                return Err(PlatformError::invalid_request("empty_body", "body is empty.").with_param("body"));
            }
            if phone::is_opted_out(db, &number.id, to).await? {
                return Err(PlatformError::permission("recipient_opted_out", "This person replied STOP to this number."));
            }
            if phone::consent_basis(db, &number.id, to).await?.is_none() {
                return Err(PlatformError::permission("no_consent", "This person hasn't texted this number and has no consent recorded."));
            }
            ("simulated", None, segments(text))
        }
    };
    let m = Message {
        id: new_id("msg_"),
        object: "message".into(),
        account_id: number.account_id.clone(),
        number_id: number.id.clone(),
        direction: "outbound".into(),
        from: number.e164.clone(),
        to: to.to_string(),
        body: text.to_string(),
        segments: parts,
        status: status.into(),
        error_code: None,
        media: json!([]),
        created_at: Utc::now(),
    };
    insert(db, &m, &caller.project_id, carrier_id.as_deref()).await?;
    if !number.simulated {
        let _ = record_usage(
            db,
            UsageEvent {
                project_id: caller.project_id.clone(),
                account_id: m.account_id.clone(),
                key_id: Some(caller.key_id.clone()),
                meter: "sms_segment".into(),
                quantity: f64::from(m.segments),
                unit: Some("segment".into()),
                ref_id: Some(m.id.clone()),
                idempotency: Some(format!("sms:{}", m.id)),
            },
        )
        .await;
    }
    let _ = emit_event(db, &caller.project_id, m.account_id.as_deref(), "message.status", json!({ "message": &m })).await;
    Ok(m)
}

/// Keep an inbound text for a Platform API number (called by the carrier edge
/// after verification) and send `message.received`.
pub async fn record_inbound(db: &PgPool, number_id: &str, normalised: &[u8]) -> Result<(), sqlx::Error> {
    let v: Value = serde_json::from_slice(normalised).unwrap_or(Value::Null);
    let owner: Option<(Option<String>, Option<String>)> = sqlx::query_as("SELECT project_id, account_id FROM phone_numbers WHERE id = $1")
        .bind(number_id)
        .fetch_optional(db)
        .await?;
    let Some((Some(project_id), account_id)) = owner else { return Ok(()) };
    let text = v["text"].as_str().unwrap_or("").to_string();
    let m = Message {
        id: new_id("msg_"),
        object: "message".into(),
        account_id,
        number_id: number_id.to_string(),
        direction: "inbound".into(),
        from: v["from"].as_str().unwrap_or("").to_string(),
        to: v["to"].as_str().unwrap_or("").to_string(),
        segments: segments(&text),
        body: text,
        status: "received".into(),
        error_code: None,
        media: v.get("media").cloned().unwrap_or_else(|| json!([])),
        created_at: Utc::now(),
    };
    insert(db, &m, &project_id, v["messageId"].as_str()).await?;
    let simulated: bool = sqlx::query_scalar("SELECT simulated FROM phone_numbers WHERE id = $1").bind(number_id).fetch_one(db).await?;
    if !simulated {
        let _ = record_usage(
            db,
            UsageEvent {
                project_id: project_id.clone(),
                account_id: m.account_id.clone(),
                key_id: None,
                meter: "sms_segment".into(),
                quantity: f64::from(m.segments),
                unit: Some("segment".into()),
                ref_id: Some(m.id.clone()),
                idempotency: Some(format!("sms:{}", m.id)),
            },
        )
        .await;
    }
    emit_for_number(db, number_id, "message.received", json!({ "message": &m })).await;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    number_id: Option<String>,
    direction: Option<String>,
    account_id: Option<String>,
}

async fn list_messages(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> Result<Json<Page<Message>>, PlatformError> {
    caller.require("messaging")?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(q.account_id.as_deref())?;
    if let Some(d) = q.direction.as_deref() {
        if d != "inbound" && d != "outbound" {
            return Err(PlatformError::invalid_request("invalid_direction", "direction must be inbound or outbound.").with_param("direction"));
        }
    }
    let rows = sqlx::query_as::<_, Message>(&format!(
        "SELECT {COLUMNS} FROM platform_messages WHERE project_id = $1 \
           AND ($2::text IS NULL OR account_id = $2) AND ($3::text IS NULL OR number_id = $3) AND ($4::text IS NULL OR direction = $4) \
           AND ($5::timestamptz IS NULL OR (created_at, id) > ($5, $6)) \
         ORDER BY created_at, id LIMIT $7"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.number_id)
    .bind(&q.direction)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(build_page(rows, limit, |m| (m.created_at, m.id.clone()))))
}

async fn get_message(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    Path(id): Path<String>,
) -> Result<Json<Message>, PlatformError> {
    caller.require("messaging")?;
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, Message>(&format!("SELECT {COLUMNS} FROM platform_messages WHERE id = $1 AND project_id = $2 AND ($3::text IS NULL OR account_id = $3)"))
        .bind(&id)
        .bind(&caller.project_id)
        .bind(&account)
        .fetch_optional(&state.db)
        .await?
        .map(Json)
        .ok_or_else(|| PlatformError::not_found("message_not_found", "No such message."))
}
