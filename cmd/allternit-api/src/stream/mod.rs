//! WebSocket Stream Gateway
//!
//! Provides real-time event streaming from the Rails Ledger via WebSocket.
//! Supports replay, filtering, and multi-client subscriptions.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::AppState;
use allternit_factory_engine::{project_dag, LedgerQuery};

// ============================================================================
// Routes
// ============================================================================

pub fn stream_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(stream_status))
        .route("/ws/ledger", get(ledger_stream_handler))
        .route(
            "/ws/workspace/:workspace_id/ledger",
            get(workspace_ledger_stream),
        )
        .route("/ws/dag/:dag_id/events", get(dag_event_stream))
}

async fn stream_status() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "stream",
        "streams": ["ws/ledger", "ws/workspace/:id/ledger", "ws/dag/:id/events"],
    }))
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize)]
struct StreamQueryParams {
    /// Resume from this cursor (event ID)
    cursor: Option<String>,
    /// Filter by event types (comma-separated)
    event_types: Option<String>,
    /// Replay last N events before subscribing
    replay: Option<usize>,
    /// Client identifier
    client_id: Option<String>,
}

// ============================================================================
// WebSocket Messages
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum ServerMessage {
    #[serde(rename = "connected")]
    Connected { client_id: String, cursor: String },

    #[serde(rename = "event")]
    Event {
        event_id: String,
        event_type: String,
        timestamp: String,
        payload: serde_json::Value,
        scope: Option<EventScopeResponse>,
    },

    #[serde(rename = "replay_complete")]
    ReplayComplete {
        events_replayed: usize,
        cursor: String,
    },

    #[serde(rename = "error")]
    Error { message: String },

    #[serde(rename = "heartbeat")]
    Heartbeat { timestamp: i64 },
}

#[derive(Debug, Serialize)]
struct EventScopeResponse {
    dag_id: Option<String>,
    wih_id: Option<String>,
    run_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClientMessage {
    #[serde(rename = "subscribe")]
    Subscribe { dag_id: Option<String> },

    #[serde(rename = "unsubscribe")]
    Unsubscribe,

    #[serde(rename = "seek")]
    Seek { cursor: String },

    #[serde(rename = "ping")]
    Ping,
}

// ============================================================================
// Handlers
// ============================================================================

/// Global ledger stream - all events
async fn ledger_stream_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<StreamQueryParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!(?params, "New ledger stream connection");
    ws.on_upgrade(move |socket| handle_ledger_socket(socket, state, params, None))
}

/// Workspace-scoped ledger stream
async fn workspace_ledger_stream(
    ws: WebSocketUpgrade,
    Path(workspace_id): Path<String>,
    Query(params): Query<StreamQueryParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!(
        workspace_id,
        ?params,
        "New workspace ledger stream connection"
    );
    ws.on_upgrade(move |socket| handle_ledger_socket(socket, state, params, Some(workspace_id)))
}

/// DAG-specific event stream
async fn dag_event_stream(
    ws: WebSocketUpgrade,
    Path(dag_id): Path<String>,
    Query(params): Query<StreamQueryParams>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    info!(dag_id, ?params, "New DAG event stream connection");
    ws.on_upgrade(move |socket| handle_dag_socket(socket, state, dag_id, params))
}

// ============================================================================
// Socket Handlers
// ============================================================================

async fn handle_ledger_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    params: StreamQueryParams,
    _workspace_filter: Option<String>,
) {
    let client_id = params
        .client_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (mut sender, mut receiver) = socket.split();

    // Channel for sending messages to the client
    let (tx, mut rx) = mpsc::channel::<Message>(100);

    // Send connected message
    let connected_msg = ServerMessage::Connected {
        client_id: client_id.clone(),
        cursor: params.cursor.clone().unwrap_or_else(|| "0".to_string()),
    };

    if let Ok(json) = serde_json::to_string(&connected_msg) {
        let _ = tx.send(Message::Text(json)).await;
    }

    // Spawn heartbeat task
    let heartbeat_tx = tx.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let heartbeat = ServerMessage::Heartbeat {
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            if let Ok(json) = serde_json::to_string(&heartbeat) {
                if heartbeat_tx.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Handle replay if requested
    if let Some(replay_count) = params.replay {
        if let Some(ref cursor) = params.cursor {
            debug!(
                client_id,
                replay_count,
                cursor,
                event_types = ?params.event_types,
                "Replaying events"
            );

            // Query ledger for events since cursor
            let query = allternit_factory_engine::LedgerQuery {
                r#type: None,
                types: None,
                scope: None,
                since: Some(cursor.clone()),
                until: None,
                limit: Some(replay_count),
            };

            match state.rails.ledger.query(query).await {
                Ok(events) => {
                    let events_replayed = events.len();

                    for event in &events {
                        let msg = ServerMessage::Event {
                            event_id: event.event_id.clone(),
                            event_type: event.r#type.clone(),
                            timestamp: event.ts.clone(),
                            payload: event.payload.clone(),
                            scope: event.scope.as_ref().map(|s| EventScopeResponse {
                                dag_id: s.dag_id.clone(),
                                wih_id: s.wih_id.clone(),
                                run_id: s.run_id.clone(),
                            }),
                        };

                        if let Ok(json) = serde_json::to_string(&msg) {
                            if tx.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                    }

                    // Send replay complete
                    let last_cursor = events
                        .last()
                        .map(|e| e.event_id.clone())
                        .unwrap_or_else(|| cursor.clone());

                    let complete_msg = ServerMessage::ReplayComplete {
                        events_replayed,
                        cursor: last_cursor,
                    };

                    if let Ok(json) = serde_json::to_string(&complete_msg) {
                        let _ = tx.send(Message::Text(json)).await;
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to query ledger for replay");
                    let error_msg = ServerMessage::Error {
                        message: format!("Replay failed: {}", e),
                    };
                    if let Ok(json) = serde_json::to_string(&error_msg) {
                        let _ = tx.send(Message::Text(json)).await;
                    }
                }
            }
        }
    }

    // Spawn task to forward messages from channel to WebSocket
    let forward_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Active DAG filter set by Subscribe messages (None = all events)
    let mut dag_filter: Option<String> = None;

    // Handle incoming messages from client
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        match client_msg {
                            ClientMessage::Ping => {
                                let pong = ServerMessage::Heartbeat {
                                    timestamp: chrono::Utc::now().timestamp_millis(),
                                };
                                if let Ok(json) = serde_json::to_string(&pong) {
                                    let _ = tx.send(Message::Text(json)).await;
                                }
                            }
                            ClientMessage::Seek { cursor } => {
                                debug!(
                                    client_id,
                                    cursor,
                                    "Client seeking to cursor — replaying from new position"
                                );
                                let query = LedgerQuery {
                                    r#type: None,
                                    types: None,
                                    scope: None,
                                    since: Some(cursor.clone()),
                                    until: None,
                                    limit: Some(100),
                                };
                                match state.rails.ledger.query(query).await {
                                    Ok(events) => {
                                        let events_replayed = events.len();
                                        for event in &events {
                                            // Honour active DAG filter during replay
                                            if let Some(ref filter_dag) = dag_filter {
                                                if event
                                                    .scope
                                                    .as_ref()
                                                    .and_then(|s| s.dag_id.as_ref())
                                                    .map(|id| id != filter_dag)
                                                    .unwrap_or(true)
                                                {
                                                    continue;
                                                }
                                            }
                                            let msg = ServerMessage::Event {
                                                event_id: event.event_id.clone(),
                                                event_type: event.r#type.clone(),
                                                timestamp: event.ts.clone(),
                                                payload: event.payload.clone(),
                                                scope: event.scope.as_ref().map(|s| {
                                                    EventScopeResponse {
                                                        dag_id: s.dag_id.clone(),
                                                        wih_id: s.wih_id.clone(),
                                                        run_id: s.run_id.clone(),
                                                    }
                                                }),
                                            };
                                            if let Ok(json) = serde_json::to_string(&msg) {
                                                if tx.send(Message::Text(json)).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                        let last_cursor = events
                                            .last()
                                            .map(|e| e.event_id.clone())
                                            .unwrap_or(cursor);
                                        let complete_msg = ServerMessage::ReplayComplete {
                                            events_replayed,
                                            cursor: last_cursor,
                                        };
                                        if let Ok(json) = serde_json::to_string(&complete_msg) {
                                            let _ = tx.send(Message::Text(json)).await;
                                        }
                                    }
                                    Err(e) => {
                                        error!(error = %e, "Seek ledger query failed");
                                        let error_msg = ServerMessage::Error {
                                            message: format!("Seek failed: {}", e),
                                        };
                                        if let Ok(json) = serde_json::to_string(&error_msg) {
                                            let _ = tx.send(Message::Text(json)).await;
                                        }
                                    }
                                }
                            }
                            ClientMessage::Subscribe { dag_id } => {
                                debug!(client_id, ?dag_id, "Client subscribing to DAG filter");
                                dag_filter = dag_id;
                            }
                            ClientMessage::Unsubscribe => {
                                debug!(client_id, "Client unsubscribing from DAG filter");
                                dag_filter = None;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Failed to parse client message");
                        let error_msg = ServerMessage::Error {
                            message: format!("Invalid message format: {}", e),
                        };
                        if let Ok(json) = serde_json::to_string(&error_msg) {
                            let _ = tx.send(Message::Text(json)).await;
                        }
                    }
                }
            }
            Message::Close(_) => {
                info!(client_id, "Client disconnected");
                break;
            }
            _ => {}
        }
    }

    // Cleanup
    heartbeat_handle.abort();
    forward_handle.abort();

    info!(client_id, "Stream connection closed");
}

async fn handle_dag_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    dag_id: String,
    params: StreamQueryParams,
) {
    let client_id = params
        .client_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let (mut sender, mut receiver) = socket.split();

    // Channel for sending messages
    let (tx, mut rx) = mpsc::channel::<Message>(100);

    // Send connected message
    let connected_msg = ServerMessage::Connected {
        client_id: client_id.clone(),
        cursor: params.cursor.clone().unwrap_or_else(|| "0".to_string()),
    };

    if let Ok(json) = serde_json::to_string(&connected_msg) {
        let _ = tx.send(Message::Text(json)).await;
    }

    // Query current DAG state
    match state.rails.ledger.query(LedgerQuery::default()).await {
        Ok(events) => {
            let dag = project_dag(&events, &dag_id);

            let status = dag
                .nodes
                .values()
                .find(|n| n.parent_node_id.is_none())
                .map(|n| n.status.clone())
                .unwrap_or_else(|| "UNKNOWN".to_string());

            // Send current DAG state as initial event
            let state_msg = ServerMessage::Event {
                event_id: format!("state-{}", dag_id),
                event_type: "dag.state".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                payload: serde_json::json!({
                    "dag_id": dag.dag_id,
                    "status": status,
                    "node_count": dag.nodes.len(),
                }),
                scope: Some(EventScopeResponse {
                    dag_id: Some(dag_id.clone()),
                    wih_id: None,
                    run_id: None,
                }),
            };

            if let Ok(json) = serde_json::to_string(&state_msg) {
                let _ = tx.send(Message::Text(json)).await;
            }
        }
        Err(e) => {
            error!(dag_id, error = %e, "Failed to get events for DAG stream");
        }
    }

    // Spawn heartbeat
    let heartbeat_tx = tx.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let heartbeat = ServerMessage::Heartbeat {
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            if let Ok(json) = serde_json::to_string(&heartbeat) {
                if heartbeat_tx.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    // Forward task
    let forward_handle = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    match client_msg {
                        ClientMessage::Ping => {
                            let pong = ServerMessage::Heartbeat {
                                timestamp: chrono::Utc::now().timestamp_millis(),
                            };
                            if let Ok(json) = serde_json::to_string(&pong) {
                                let _ = tx.send(Message::Text(json)).await;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::Close(_) => {
                info!(client_id, dag_id, "DAG stream client disconnected");
                break;
            }
            _ => {}
        }
    }

    // Cleanup
    heartbeat_handle.abort();
    forward_handle.abort();

    info!(client_id, dag_id, "DAG stream connection closed");
}
