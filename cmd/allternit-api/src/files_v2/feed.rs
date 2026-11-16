//! Change feed (SPEC section 6): the long poll on `GET /roots/:id/changes`
//! (engine `wait_for_change`) and the `GET /subscribe` WebSocket (engine change
//! listener → broadcast). Neither counts as activity or keeps a computer awake.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use allternit_files_core::{
    ChangesQuery, ChangesResponse, ErrorCode, FeedChanges, FeedHeartbeat, FeedMessage, FeedReset, FeedResetReason,
    SubscribeMessage,
};
use allternit_files_engine::FilesError;
use axum::extract::rejection::QueryRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use tokio::sync::broadcast::error::RecvError;

use super::routes::run;
use super::{live, permissions, ApiError, Fail, FilesV2, Inner, Need, Req};

/// Longest long poll (section 6).
pub const MAX_WAIT_S: u64 = 25;
pub const HEARTBEAT: Duration = Duration::from_secs(25);

pub async fn changes(
    Extension(files): Extension<Arc<FilesV2>>,
    req: Req,
    Path(root_id): Path<String>,
    q: Result<Query<ChangesQuery>, QueryRejection>,
) -> Result<Json<ChangesResponse>, ApiError> {
    let Query(q) = q.map_err(ApiError::from)?;
    let limit = q.limit.unwrap_or(500).clamp(1, 1000) as usize;
    let wait = q.wait_s.unwrap_or(0).min(MAX_WAIT_S);
    run(&files, move |v| {
        let root = v.guard(&req, &root_id, Need::Read)?;
        if wait > 0 {
            v.drive.wait_for_change(&root_id, &q.cursor, Duration::from_secs(wait))?;
        }
        let page = v.drive.changes(&root_id, Some(&q.cursor), limit)?;
        Ok(Json(ChangesResponse {
            root_id: page.root_id.clone(),
            changes: page.changes.iter().map(|c| c.to_entry(|i| permissions(&root, i))).collect(),
            cursor: page.cursor,
            has_more: page.has_more,
            reset_required: page.reset_required,
            freshness: live(),
        }))
    })
    .await
}

pub async fn subscribe(
    Extension(files): Extension<Arc<FilesV2>>,
    req: Req,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let inner = files.inner().await?;
    let ws = ws.map_err(|r| ApiError::invalid(r.body_text()))?;
    Ok(ws.on_upgrade(move |socket| socket_loop(inner, req, socket)).into_response())
}

async fn send(socket: &mut WebSocket, msg: &FeedMessage) -> bool {
    let text = serde_json::to_string(msg).expect("feed frames serialize");
    socket.send(Message::Text(text)).await.is_ok()
}

async fn close(mut socket: WebSocket, code: u16, reason: &str) {
    let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.to_string().into() }))).await;
}

/// Checks each subscribed root. `Err` closes the socket with that reason.
fn admit(inner: &Inner, req: &Req, sub: SubscribeMessage) -> Result<(HashMap<String, String>, Vec<FeedMessage>), ApiError> {
    if sub.r#type != "subscribe" {
        return Err(ApiError::invalid("The first frame must be a subscribe message."));
    }
    let mut cursors = HashMap::new();
    let mut resets = Vec::new();
    for r in sub.roots {
        match inner.guard(req, &r.root_id, Need::Read) {
            Ok(_) => {
                cursors.insert(r.root_id, r.cursor);
            }
            Err(Fail::Files(FilesError::RootNotFound(_))) => {
                resets.push(FeedMessage::FeedReset(FeedReset { root_id: r.root_id, reason: FeedResetReason::RootRemoved }))
            }
            Err(e) => return Err(inner.fail(e)),
        }
    }
    Ok((cursors, resets))
}

/// Everything after `cursor` in one root, as frames; returns the new cursor
/// (None when the root is gone).
fn pull(inner: &Inner, root_id: &str, cursor: &str) -> (Vec<FeedMessage>, Option<String>) {
    let mut frames = Vec::new();
    let mut cursor = cursor.to_string();
    let root = match inner.drive.root(root_id) {
        Ok(r) => r,
        Err(_) => {
            frames.push(FeedMessage::FeedReset(FeedReset { root_id: root_id.into(), reason: FeedResetReason::RootRemoved }));
            return (frames, None);
        }
    };
    loop {
        match inner.drive.changes(root_id, Some(&cursor), 500) {
            Ok(page) if page.reset_required => {
                frames.push(FeedMessage::FeedReset(FeedReset { root_id: root_id.into(), reason: FeedResetReason::CursorExpired }));
                return (frames, Some(page.cursor));
            }
            Ok(page) => {
                if !page.changes.is_empty() {
                    frames.push(FeedMessage::FeedChanges(FeedChanges {
                        root_id: root_id.into(),
                        changes: page.changes.iter().map(|c| c.to_entry(|i| permissions(&root, i))).collect(),
                        cursor: page.cursor.clone(),
                    }));
                }
                cursor = page.cursor;
                if !page.has_more {
                    return (frames, Some(cursor));
                }
            }
            Err(FilesError::CursorExpired) => {
                frames.push(FeedMessage::FeedReset(FeedReset { root_id: root_id.into(), reason: FeedResetReason::CursorExpired }));
                let now = inner.drive.current_cursor(root_id).ok();
                return (frames, now);
            }
            Err(e) => {
                tracing::warn!(error = %e, root_id, "files v2: feed read failed");
                return (frames, Some(cursor));
            }
        }
    }
}

async fn pull_all(inner: &Arc<Inner>, cursors: &mut HashMap<String, String>, only: Option<&str>) -> Vec<FeedMessage> {
    let wanted: Vec<(String, String)> = cursors
        .iter()
        .filter(|(r, _)| only.map_or(true, |o| o == r.as_str()))
        .map(|(r, c)| (r.clone(), c.clone()))
        .collect();
    let i = inner.clone();
    let results = tokio::task::spawn_blocking(move || wanted.into_iter().map(|(r, c)| (r.clone(), pull(&i, &r, &c))).collect::<Vec<_>>())
        .await
        .unwrap_or_default();
    let mut frames = Vec::new();
    for (root, (f, cursor)) in results {
        frames.extend(f);
        match cursor {
            Some(c) => {
                cursors.insert(root, c);
            }
            None => {
                cursors.remove(&root);
            }
        }
    }
    frames
}

async fn read_subscribe(inner: &Arc<Inner>, req: &Req, text: &str) -> Result<(HashMap<String, String>, Vec<FeedMessage>), ApiError> {
    let sub: SubscribeMessage = serde_json::from_str(text).map_err(|e| ApiError::invalid(format!("subscribe frame: {e}")))?;
    let (i, r) = (inner.clone(), req.clone());
    tokio::task::spawn_blocking(move || admit(&i, &r, sub)).await.map_err(|_| ApiError::internal())?
}

fn code_str(e: &ApiError) -> String {
    serde_json::to_value(&e.body.code).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "internal".into())
}

async fn socket_loop(inner: Arc<Inner>, req: Req, mut socket: WebSocket) {
    let mut rx = inner.feed.subscribe();
    let first = match tokio::time::timeout(Duration::from_secs(30), socket.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => t,
        _ => return close(socket, 1008, "expected a subscribe frame").await,
    };
    let mut cursors = match read_subscribe(&inner, &req, &first).await {
        Ok((c, resets)) => {
            for f in &resets {
                if !send(&mut socket, f).await {
                    return;
                }
            }
            c
        }
        Err(e) => return close(socket, if e.body.code == ErrorCode::InvalidRequest { 1003 } else { 1008 }, &code_str(&e)).await,
    };
    for f in pull_all(&inner, &mut cursors, None).await {
        if !send(&mut socket, &f).await {
            return;
        }
    }
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    loop {
        let frames = tokio::select! {
            r = rx.recv() => match r {
                Ok(root) if cursors.contains_key(&root) => pull_all(&inner, &mut cursors, Some(&root)).await,
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => pull_all(&inner, &mut cursors, None).await,
                Err(RecvError::Closed) => break,
            },
            _ = beat.tick() => vec![FeedMessage::FeedHeartbeat(FeedHeartbeat {
                at: inner.drive.now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            })],
            m = socket.recv() => match m {
                Some(Ok(Message::Text(t))) => match read_subscribe(&inner, &req, &t).await {
                    Ok((c, resets)) => {
                        cursors = c;
                        let mut f = resets;
                        f.extend(pull_all(&inner, &mut cursors, None).await);
                        f
                    }
                    Err(e) => return close(socket, 1008, &code_str(&e)).await,
                },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => continue,
            },
        };
        for f in &frames {
            if !send(&mut socket, f).await {
                return;
            }
        }
    }
}
