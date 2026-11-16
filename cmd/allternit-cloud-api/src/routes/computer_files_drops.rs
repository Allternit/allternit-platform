//! Guest drops kept while a computer slept, handed back when it wakes (plan
//! step 17; SPEC section 8, `QueuedDrop`).
//!
//! The sharing gateway (`files_gateway::queue_drop`) keeps an `upload_only`
//! small write to a sleeping computer in the owner's R2 space
//! (`files-drops/<computer_id>/<drop_id>`) with a `files_queued_drops` row.
//! The computer pulls them itself once it is awake, signed exactly like its
//! presence heartbeat, so only the computer the drop was for can read it:
//!
//! - `GET  /api/v1/computer-files/drops`: `QueuedDropsResponse`, oldest first, at most 100.
//! - `GET  /api/v1/computer-files/drops/:drop_id/content`: the bytes.
//! - `POST /api/v1/computer-files/drops/:drop_id/ack`: `QueuedDropAck`. The row
//!   becomes `delivered` or `rejected` and the R2 copy is deleted.
//!
//! None of this wakes a computer or touches its idle clock: the computer
//! calls only while it is already awake. Answers `feature_disabled` while the
//! owner's `files_drive` flag is off or migration 144 is not applied.

use std::sync::Arc;

use allternit_files_core::{ErrorCode, QueuedDrop, QueuedDropAck, QueuedDropAckState, QueuedDropState, QueuedDropsResponse};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

use super::computer_files::{self as cf, files_error};
use super::computer_files_snapshot as snapshot;
use crate::ApiState;

pub const DROPS_PATH: &str = "/api/v1/computer-files/drops";
const MAX_LISTED: i64 = 100;

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route(DROPS_PATH, get(list))
        .route("/api/v1/computer-files/drops/:drop_id/content", get(content))
        .route("/api/v1/computer-files/drops/:drop_id/ack", post(ack))
}

async fn table_ready(db: &sqlx::PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('files_queued_drops') IS NOT NULL")
        .fetch_one(db)
        .await
        .unwrap_or(false)
}

/// The signing computer, with its owner's flag on and migration 144 applied.
async fn computer_gate(state: &ApiState, headers: &HeaderMap, method: &str, path: &str, body: &[u8]) -> Result<(String, String), Response> {
    let (user_id, runtime_id) =
        super::runtime_events::authenticate_runtime(&state.db, headers, method, path, body, chrono::Utc::now().timestamp()).await?;
    if !cf::files_drive_on(&user_id) || !table_ready(&state.db).await {
        return Err(cf::disabled());
    }
    Ok((user_id, runtime_id))
}

type DropRow = (String, String, String, String, i64, String, String, chrono::DateTime<chrono::Utc>, String);

fn to_wire(r: DropRow) -> QueuedDrop {
    let (drop_id, share_id, guest_id, name, size, content_hash, r2_key, received_at, state) = r;
    QueuedDrop {
        drop_id,
        share_id,
        guest_id,
        name,
        size: size.max(0) as u64,
        content_hash,
        r2_key,
        received_at: received_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        state: match state.as_str() {
            "delivered" => QueuedDropState::Delivered,
            "rejected" => QueuedDropState::Rejected,
            _ => QueuedDropState::Queued,
        },
    }
}

const COLS: &str = "drop_id, share_id, guest_id, name, size, content_hash, r2_key, received_at, state";

/// One of this computer's drops (another computer's or user's drop is "not found").
async fn own_drop(db: &sqlx::PgPool, user_id: &str, runtime_id: &str, drop_id: &str) -> Result<Option<QueuedDrop>, sqlx::Error> {
    let row: Option<DropRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM files_queued_drops WHERE drop_id = $1 AND computer_id = $2 AND owner_user_id = $3"
    ))
    .bind(drop_id)
    .bind(runtime_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(to_wire))
}

fn drop_not_found() -> Response {
    files_error(StatusCode::NOT_FOUND, ErrorCode::NotFound, "No such drop for this computer", None, None, None)
}

async fn list(State(state): State<Arc<ApiState>>, headers: HeaderMap) -> Response {
    let (user_id, runtime_id) = match computer_gate(&state, &headers, "GET", DROPS_PATH, b"").await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let rows: Vec<DropRow> = match sqlx::query_as(&format!(
        "SELECT {COLS} FROM files_queued_drops WHERE computer_id = $1 AND owner_user_id = $2 AND state = 'queued' \
         ORDER BY received_at, drop_id LIMIT $3"
    ))
    .bind(&runtime_id)
    .bind(&user_id)
    .bind(MAX_LISTED)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => return cf::internal("drops list", error),
    };
    Json(QueuedDropsResponse { drops: rows.into_iter().map(to_wire).collect() }).into_response()
}

async fn content(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(drop_id): Path<String>) -> Response {
    let path = format!("{DROPS_PATH}/{drop_id}/content");
    let (user_id, runtime_id) = match computer_gate(&state, &headers, "GET", &path, b"").await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let drop = match own_drop(&state.db, &user_id, &runtime_id, &drop_id).await {
        Ok(Some(d)) if d.state == QueuedDropState::Queued => d,
        Ok(_) => return drop_not_found(),
        Err(error) => return cf::internal("drop", error),
    };
    let Some(store) = snapshot::snapshot_store() else {
        return files_error(StatusCode::NOT_IMPLEMENTED, ErrorCode::Unsupported, "Drop storage is not configured", None, None, None);
    };
    match store.get(crate::services::user_files::BUCKET, &drop.r2_key).await {
        Ok(Some(bytes)) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Ok(None) => drop_not_found(),
        Err(error) => cf::internal("drop content", format!("{error:?}")),
    }
}

async fn ack(State(state): State<Arc<ApiState>>, headers: HeaderMap, Path(drop_id): Path<String>, body: Bytes) -> Response {
    let path = format!("{DROPS_PATH}/{drop_id}/ack");
    let (user_id, runtime_id) = match computer_gate(&state, &headers, "POST", &path, &body).await {
        Ok(v) => v,
        Err(response) => return response,
    };
    let Ok(ack) = serde_json::from_slice::<QueuedDropAck>(&body) else {
        return files_error(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest, "Body is not a QueuedDropAck", None, None, None);
    };
    let drop = match own_drop(&state.db, &user_id, &runtime_id, &drop_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return drop_not_found(),
        Err(error) => return cf::internal("drop", error),
    };
    let new_state = match ack.state {
        QueuedDropAckState::Delivered => "delivered",
        QueuedDropAckState::Rejected => "rejected",
    };
    // Idempotent: a repeated ack (the computer lost the first answer) is fine.
    if drop.state == QueuedDropState::Queued {
        if let Err(error) = sqlx::query("UPDATE files_queued_drops SET state = $2 WHERE drop_id = $1 AND state = 'queued'")
            .bind(&drop_id)
            .bind(new_state)
            .execute(&state.db)
            .await
        {
            return cf::internal("drop ack", error);
        }
        if ack.state == QueuedDropAckState::Rejected {
            tracing::info!(%drop_id, reason = ack.reason.as_deref().unwrap_or(""), "files drop rejected by the computer");
        }
    }
    if let Some(store) = snapshot::snapshot_store() {
        if let Err(error) = store.delete(crate::services::user_files::BUCKET, &drop.r2_key).await {
            tracing::warn!(%drop_id, "files drop: R2 copy not deleted: {error:?}");
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
#[path = "computer_files_drops_tests.rs"]
mod tests;
