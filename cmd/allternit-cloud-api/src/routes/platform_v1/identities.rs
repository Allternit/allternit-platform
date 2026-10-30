//! `/v1/identities`: an agent's whole identity in one call.
//!
//! `POST /v1/identities {account_id?, agent_id?, area_code?, number_type?,
//! phone_serial?, local_part?, display_name?}` gets a number (the `/v1/numbers`
//! buy path: simulated in sandbox, bought from the carrier in live, the first
//! available in `area_code` when given), a mailbox (the `/v1/mailboxes` path)
//! and, optionally, binds one of the project owner's linked phones by serial.
//! With `agent_id`, the number answers calls with that agent. A failure after
//! the number is bought releases it again.
//!
//! `GET /v1/identities`, `GET /v1/identities/{id}`; `DELETE` releases the number
//! and removes the mailbox (they can't be deleted on their own while the
//! identity exists).
//!
//! Scopes: `numbers` and `mail`, plus `phones` with `phone_serial` (and then an
//! unbound key, as for `/v1/phones`). Billing: the "Agent identity" bundle
//! ([`super::stripe_plan::AGENT_IDENTITY`], $9 a month) as one
//! `identity_month` row per live identity per month; its number and mailbox
//! record no `number_*_month` / `mailbox_month` of their own.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use super::mailboxes::{self, MailExt};
use super::numbers::{self, ApiNumber};
use super::stripe_plan::AGENT_IDENTITY;
use super::{build_page, new_id, ApiJson, ApiQuery, Page, PageParams, PlatformCaller, PlatformError, RouteTable, UsageEvent};
use crate::carriers::NumberType;
use crate::ApiState;

type VoiceExt = Option<axum::Extension<Arc<super::calls::VoiceDeps>>>;

pub fn register(table: RouteTable) -> RouteTable {
    table
        .add("/v1/identities", &["GET", "POST"], get(list_identities).post(create))
        .add("/v1/identities/:id", &["GET", "DELETE"], get(get_identity).delete(remove))
}

fn require(caller: &PlatformCaller) -> Result<(), PlatformError> {
    caller.require("numbers")?;
    caller.require(mailboxes::SCOPE)
}

#[derive(Debug, Clone, FromRow)]
struct IdentityRow {
    id: String,
    account_id: String,
    agent_id: Option<String>,
    number_id: String,
    mailbox_id: String,
    phone_serial: Option<String>,
    created_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, account_id, agent_id, number_id, mailbox_id, phone_serial, created_at";

/// What the bundle includes, as the API shows it.
pub fn bundle_json() -> Value {
    json!({
        "name": AGENT_IDENTITY.display_name, "meter": AGENT_IDENTITY.meter, "monthly_price_cents": AGENT_IDENTITY.cents,
        "includes": { "numbers": AGENT_IDENTITY.numbers, "mailboxes": AGENT_IDENTITY.mailboxes, "linked_phone_slots": AGENT_IDENTITY.linked_phone_slots, "phone_minutes": AGENT_IDENTITY.phone_minutes },
    })
}

fn to_json(row: &IdentityRow, number: Option<&ApiNumber>, mailbox: Option<&mailboxes::MailboxRow>) -> Value {
    json!({
        "id": row.id, "object": "identity", "account_id": row.account_id, "agent_id": row.agent_id,
        "number": number.map(ApiNumber::to_json), "mailbox": mailbox.map(mailboxes::MailboxRow::to_json),
        "phone_serial": row.phone_serial, "bundle": bundle_json(), "created_at": row.created_at,
    })
}

async fn fetch(db: &PgPool, caller: &PlatformCaller, id: &str) -> Result<IdentityRow, PlatformError> {
    let account = caller.account_filter(None)?;
    sqlx::query_as::<_, IdentityRow>(&format!(
        "SELECT {COLUMNS} FROM platform_identities WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL AND ($3::text IS NULL OR account_id = $3)"
    ))
    .bind(id)
    .bind(&caller.project_id)
    .bind(&account)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| PlatformError::not_found("identity_not_found", "No such identity."))
}

async fn expand(db: &PgPool, caller: &PlatformCaller, row: &IdentityRow) -> Value {
    let number = numbers::api_number(db, caller, &row.number_id).await.ok();
    let mailbox = mailboxes::api_mailbox(db, caller, &row.mailbox_id).await.ok();
    to_json(row, number.as_ref(), mailbox.as_ref())
}

pub(crate) async fn number_in_identity(db: &PgPool, number_id: &str) -> Result<bool, PlatformError> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM platform_identities WHERE number_id = $1 AND deleted_at IS NULL)").bind(number_id).fetch_one(db).await?)
}

pub(crate) async fn mailbox_in_identity(db: &PgPool, mailbox_id: &str) -> Result<bool, PlatformError> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM platform_identities WHERE mailbox_id = $1 AND deleted_at IS NULL)").bind(mailbox_id).fetch_one(db).await?)
}

/// `412` → `"412"`; anything but three digits is refused.
pub fn check_area_code(raw: Option<&str>) -> Result<Option<String>, PlatformError> {
    match raw.map(str::trim).filter(|a| !a.is_empty()) {
        None => Ok(None),
        Some(a) if a.len() == 3 && a.bytes().all(|b| b.is_ascii_digit()) => Ok(Some(a.to_string())),
        Some(_) => Err(PlatformError::invalid_request("invalid_area_code", "area_code must be three digits, like 415.").with_param("area_code")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    account_id: Option<String>,
    agent_id: Option<String>,
    area_code: Option<String>,
    number_type: Option<String>,
    phone_serial: Option<String>,
    local_part: Option<String>,
    display_name: Option<String>,
}

/// The serial must be one of the owner's linked phones; the key needs `phones`
/// and must not be bound to one account (phones are the owner's).
async fn check_serial(db: &PgPool, caller: &PlatformCaller, raw: &str) -> Result<String, PlatformError> {
    caller.require("phones")?;
    if caller.account_id.is_some() {
        return Err(PlatformError::permission("account_bound_key", "This API key is bound to one account. Phones belong to the project owner; use an unbound key."));
    }
    let serial = raw.trim();
    if serial.is_empty() || serial.len() > 200 || serial.chars().any(char::is_control) {
        return Err(PlatformError::invalid_request("invalid_serial", "phone_serial must be a serial from GET /v1/phones.").with_param("phone_serial"));
    }
    let linked = crate::routes::registered_phones::phones_for_account(db, &caller.owner_user_id).await?;
    if !linked.iter().any(|p| p.serial == serial) {
        return Err(PlatformError::not_found("phone_not_found", "No linked phone has that serial. GET /v1/phones lists them.").with_param("phone_serial"));
    }
    Ok(serial.to_string())
}

async fn create(
    State(state): State<Arc<ApiState>>,
    caller: PlatformCaller,
    voice: VoiceExt,
    layered: MailExt,
    ApiJson(body): ApiJson<CreateBody>,
) -> Result<(StatusCode, Json<Value>), PlatformError> {
    require(&caller)?;
    let area_code = check_area_code(body.area_code.as_deref())?;
    let kind = numbers::parse_kind(body.number_type.as_deref()).map_err(|e| e.with_param("number_type"))?.unwrap_or(NumberType::Local);
    let serial = match body.phone_serial.as_deref() {
        Some(raw) => Some(check_serial(&state.db, &caller, raw).await?),
        None => None,
    };
    let (account, agent) = mailboxes::resolve_account(&state, &caller, body.account_id.as_deref(), body.agent_id.as_deref()).await?;
    if let Some(serial) = &serial {
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM platform_identities WHERE project_id = $1 AND phone_serial = $2 AND deleted_at IS NULL)")
            .bind(&caller.project_id)
            .bind(serial)
            .fetch_one(&state.db)
            .await?;
        if taken {
            return Err(PlatformError::conflict("phone_in_identity", "That phone is already bound to another identity.").with_param("phone_serial"));
        }
    }
    // Card / spend cap / plan are checked by the number path, before anything is bought.
    let number = numbers::acquire(&state, &caller, &account, None, area_code.as_deref(), kind, true).await?;
    let host = mailboxes::host_for(layered);
    let mailbox = match mailboxes::provision(&state, &caller, host.as_ref(), &account, agent.as_ref(), body.local_part.as_deref(), body.display_name.as_deref()).await {
        Ok(m) => m,
        Err(e) => {
            if let Err(undo) = numbers::release_number(&state, &caller, &number, None).await {
                tracing::error!(number = %number.id, code = %undo.code, "identity: mailbox failed and the number was not released");
            }
            return Err(e);
        }
    };
    let id = new_id("idn_");
    let inserted = sqlx::query(
        "INSERT INTO platform_identities (id, project_id, account_id, agent_id, number_id, mailbox_id, phone_serial) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(&caller.project_id)
    .bind(&account)
    .bind(agent.as_ref().map(|a| a.id.as_str()))
    .bind(&number.id)
    .bind(&mailbox.id)
    .bind(&serial)
    .execute(&state.db)
    .await;
    if let Err(e) = inserted {
        let _ = mailboxes::teardown(&state, host.as_ref(), &mailbox).await;
        let _ = numbers::release_number(&state, &caller, &number, None).await;
        return Err(e.into());
    }
    if let Some(agent) = &agent {
        // Like POST /v1/numbers: the number stays even if voice setup fails
        // (`voice_state: pending`; PATCH /v1/numbers/{id} retries it).
        if let Err(e) = super::calls::bind_agent(&state, &caller, &number, Some(&agent.id), voice).await {
            tracing::warn!(number = %number.id, code = %e.code, "identity: voice setup failed");
        }
    }
    if !number.simulated {
        mailboxes::record_month(&state.db, &caller, &account, AGENT_IDENTITY.meter, &id, "identity").await;
    }
    let row = fetch(&state.db, &caller, &id).await?;
    Ok((StatusCode::CREATED, Json(expand(&state.db, &caller, &row).await)))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(flatten)]
    page: PageParams,
    account_id: Option<String>,
    agent_id: Option<String>,
}

async fn list_identities(State(state): State<Arc<ApiState>>, caller: PlatformCaller, ApiQuery(q): ApiQuery<ListQuery>) -> Result<Json<Page<Value>>, PlatformError> {
    require(&caller)?;
    let limit = q.page.limit()?;
    let (after_at, after_id) = match q.page.cursor()? {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let account = caller.account_filter(q.account_id.as_deref())?;
    let rows = sqlx::query_as::<_, IdentityRow>(&format!(
        "SELECT {COLUMNS} FROM platform_identities WHERE project_id = $1 AND deleted_at IS NULL AND ($2::text IS NULL OR account_id = $2) \
           AND ($3::text IS NULL OR agent_id = $3) AND ($4::timestamptz IS NULL OR (created_at, id) > ($4, $5)) ORDER BY created_at, id LIMIT $6"
    ))
    .bind(&caller.project_id)
    .bind(&account)
    .bind(&q.agent_id)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(&state.db)
    .await?;
    let page = build_page(rows, limit, |r| (r.created_at, r.id.clone()));
    let mut data = Vec::with_capacity(page.data.len());
    for row in &page.data {
        data.push(expand(&state.db, &caller, row).await);
    }
    Ok(Json(Page { data, has_more: page.has_more, next_cursor: page.next_cursor }))
}

async fn get_identity(State(state): State<Arc<ApiState>>, caller: PlatformCaller, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    require(&caller)?;
    let row = fetch(&state.db, &caller, &id).await?;
    Ok(Json(expand(&state.db, &caller, &row).await))
}

/// Release the number, remove the mailbox, then mark the identity deleted.
async fn remove(State(state): State<Arc<ApiState>>, caller: PlatformCaller, voice: VoiceExt, layered: MailExt, Path(id): Path<String>) -> Result<Json<Value>, PlatformError> {
    require(&caller)?;
    let row = fetch(&state.db, &caller, &id).await?;
    if let Ok(number) = numbers::api_number(&state.db, &caller, &row.number_id).await {
        numbers::release_number(&state, &caller, &number, voice).await?;
    }
    if let Ok(mailbox) = mailboxes::api_mailbox(&state.db, &caller, &row.mailbox_id).await {
        mailboxes::teardown(&state, mailboxes::host_for(layered).as_ref(), &mailbox).await?;
    }
    sqlx::query("UPDATE platform_identities SET deleted_at = now() WHERE id = $1").bind(&row.id).execute(&state.db).await?;
    Ok(Json(json!({ "id": row.id, "object": "identity", "deleted": true })))
}

/// Monthly bundle fees: one `identity_month` per live identity of a live
/// project whose number is real (idempotent per identity and month).
pub async fn record_identity_months(db: &PgPool) -> Result<u64, sqlx::Error> {
    let month = Utc::now().format("%Y-%m").to_string();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT i.id, i.project_id, i.account_id FROM platform_identities i JOIN platform_projects p ON p.id = i.project_id \
         JOIN phone_numbers n ON n.id = i.number_id \
         WHERE i.deleted_at IS NULL AND p.archived_at IS NULL AND p.env = 'live' AND n.simulated = false",
    )
    .fetch_all(db)
    .await?;
    let mut n = 0;
    for (id, project, account) in rows {
        let event = UsageEvent {
            project_id: project,
            account_id: Some(account),
            key_id: None,
            meter: AGENT_IDENTITY.meter.to_string(),
            quantity: 1.0,
            unit: Some("month".to_string()),
            ref_id: Some(id.clone()),
            idempotency: Some(format!("identity:{id}:{month}")),
        };
        if matches!(super::record_usage(db, event).await, Ok(Some(_))) {
            n += 1;
        }
    }
    Ok(n)
}
