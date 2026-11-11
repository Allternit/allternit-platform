//! Allternit Link boards through the cloud relay (hardware/link/README.md
//! sections 4.4 and 6).
//!
//! # The board socket (relayed)
//!
//! `WS /api/v1/phones/usba?board=<boardId>`. A Link board off the LAN buys a
//! socket ticket from cloud-api with its relay token (`link_boards.rs`) and
//! opens the account's relay socket; the relay tunnels it here, with the
//! Desktop's access token like every relayed socket, so the auth middleware's
//! AuthUser is the account.
//!
//! * `board` must be `usba-<12 lowercase hex>`, else close 4400.
//! * The node dials Desktop's USB Agent listener on `127.0.0.1:<port>`
//!   (`ALLTERNIT_USBA_PORT`, default 7300). No listener: close 4503.
//! * Then it copies both ways: one text frame = one USBA line. A frame
//!   becomes that text plus `\n` on TCP; a TCP line becomes one text frame. A
//!   binary frame, a frame or line over 512 bytes, or a frame carrying a line
//!   break closes 4400. Either side closing closes the other.
//! * One socket per account and board: a new one closes the old with 4409.
//!   At most 8 per account; a ninth board closes 4429.
//!
//! The node does not read, check or change USBA lines and holds no key: the
//! handshake under the board key runs end to end between the board and
//! Desktop's `UsbAgentLink`, which drops a board it did not set up. It logs
//! the board id and close codes, never line contents.
//!
//! # Board state for other surfaces
//!
//! Through Desktop's phone gateway (`~/.allternit/phone-gateway.json`, or
//! `ALLTERNIT_PHONE_GATEWAY_FILE`; loopback, token header):
//!
//! * `GET /api/v1/phones/boards` → `{ desktop: true, boards: LinkBoardStatus[] }`,
//!   or `{ desktop: false, boards: [] }` when the gateway is not running.
//! * `POST /api/v1/phones/boards/:boardId/stop` → 200 `{ ok: true }`; 409
//!   `board_offline`; 502 `uncertain`; 503 `desktop_offline`.
//! * `DELETE /api/v1/phones/boards/:boardId` → 200 `{ ok: true }`; 503
//!   `desktop_offline`.
//!
//! Error bodies are `{ error, message }`. A gateway that answers but cannot be
//! understood is 502 `desktop_error`.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tracing::info;

use crate::auth::AuthUser;
use crate::AppState;

/// Longest USBA line, without its line ending (USBA/1 and USBA/2).
pub const MAX_LINE: usize = 512;
pub const CLOSE_BAD_REQUEST: u16 = 4400;
pub const CLOSE_REPLACED: u16 = 4409;
pub const CLOSE_TOO_MANY: u16 = 4429;
pub const CLOSE_DESKTOP_OFFLINE: u16 = 4503;
const CLOSE_NORMAL: u16 = 1000;
const MAX_BOARDS_PER_ACCOUNT: usize = 8;
const DEFAULT_USBA_PORT: u16 = 7300;
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// Frames above this are refused by the WebSocket layer itself; frames
/// between `MAX_LINE` and this get the contract's 4400.
const MAX_WS_MESSAGE: usize = 64 * 1024;
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
/// A disarm over the relay waits for the board's reply (lease 10 s).
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
struct UsbaConfig {
    port: u16,
    gateway_file: Option<PathBuf>,
}

impl UsbaConfig {
    fn from_env() -> Self {
        let port = std::env::var("ALLTERNIT_USBA_PORT")
            .ok()
            .and_then(|v| v.trim().parse::<u16>().ok())
            .filter(|p| *p != 0)
            .unwrap_or(DEFAULT_USBA_PORT);
        let gateway_file = std::env::var_os("ALLTERNIT_PHONE_GATEWAY_FILE")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".allternit").join("phone-gateway.json")));
        Self { port, gateway_file }
    }
}

/// Merged under `/api/v1` on the protected router, after
/// `phone_app::phone_app_router`.
pub fn phone_usba_router() -> Router<Arc<AppState>> {
    router(UsbaConfig::from_env())
}

fn router<S: Clone + Send + Sync + 'static>(cfg: UsbaConfig) -> Router<S> {
    Router::new()
        .route("/phones/usba", get(usba_ws))
        .route("/phones/boards", get(list_boards))
        .route("/phones/boards/:board_id/stop", post(stop_board))
        .route("/phones/boards/:board_id", delete(forget_board))
        .layer(Extension(cfg))
}

/// `usba-` followed by exactly 12 lowercase hex characters.
pub fn is_board_id(raw: &str) -> bool {
    raw.strip_prefix("usba-").is_some_and(|hex| {
        hex.len() == 12 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn err(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({ "error": code, "message": message }))).into_response()
}

// ── Live sockets: one per (account, board) ────────────────────────────────

struct Live {
    id: u64,
    kick: Arc<Notify>,
}

fn hub() -> &'static Mutex<HashMap<(String, String), Live>> {
    static HUB: OnceLock<Mutex<HashMap<(String, String), Live>>> = OnceLock::new();
    HUB.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Takes the (account, board) slot, kicking the socket that held it. `None`
/// when the account already has the most boards and this is a new one.
fn register(account: &str, board: &str) -> Option<(u64, Arc<Notify>)> {
    let mut live = hub().lock().unwrap_or_else(|e| e.into_inner());
    let key = (account.to_string(), board.to_string());
    if !live.contains_key(&key) && live.keys().filter(|(a, _)| a == account).count() >= MAX_BOARDS_PER_ACCOUNT {
        return None;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let kick = Arc::new(Notify::new());
    if let Some(old) = live.insert(key, Live { id, kick: kick.clone() }) {
        old.kick.notify_one();
    }
    Some((id, kick))
}

fn unregister(account: &str, board: &str, id: u64) {
    let mut live = hub().lock().unwrap_or_else(|e| e.into_inner());
    let key = (account.to_string(), board.to_string());
    if live.get(&key).map(|l| l.id) == Some(id) {
        live.remove(&key);
    }
}

// ── The board socket ──────────────────────────────────────────────────────

async fn usba_ws(
    ws: WebSocketUpgrade,
    Extension(user): Extension<AuthUser>,
    Extension(cfg): Extension<UsbaConfig>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let board = query.get("board").cloned().unwrap_or_default();
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run_board_socket(socket, user.user_id, board, cfg.port))
}

async fn close(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))
        .await;
}

async fn run_board_socket(mut socket: WebSocket, account: String, board: String, port: u16) {
    if !is_board_id(&board) {
        info!(code = CLOSE_BAD_REQUEST, "Link board socket refused: bad board id");
        close(&mut socket, CLOSE_BAD_REQUEST, "bad_board").await;
        return;
    }
    let tcp = match tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect((Ipv4Addr::LOCALHOST, port))).await {
        Ok(Ok(tcp)) => tcp,
        _ => {
            info!(board = %board, code = CLOSE_DESKTOP_OFFLINE, "Link board socket: Desktop's USB Agent listener is not answering");
            close(&mut socket, CLOSE_DESKTOP_OFFLINE, "desktop_offline").await;
            return;
        }
    };
    let Some((id, kick)) = register(&account, &board) else {
        info!(board = %board, code = CLOSE_TOO_MANY, "Link board socket refused: too many boards on this account");
        close(&mut socket, CLOSE_TOO_MANY, "too_many_boards").await;
        return;
    };
    info!(board = %board, "Link board socket open");
    let code = pipe(socket, tcp, kick).await;
    unregister(&account, &board, id);
    info!(board = %board, code, "Link board socket closed");
}

/// Copies lines both ways until one side ends. Returns the close code the node
/// sent the board (0 when the board closed first).
async fn pipe(socket: WebSocket, tcp: TcpStream, kick: Arc<Notify>) -> u16 {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (tcp_rd, mut tcp_wr) = tcp.into_split();
    let (line_tx, mut line_rx) = mpsc::channel::<Result<String, ()>>(16);
    let reader = tokio::spawn(read_lines(tcp_rd, line_tx));
    let (code, reason): (u16, &'static str) = loop {
        tokio::select! {
            _ = kick.notified() => break (CLOSE_REPLACED, "replaced"),
            frame = ws_rx.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if text.len() > MAX_LINE || text.contains(['\n', '\r']) {
                        break (CLOSE_BAD_REQUEST, "bad_line");
                    }
                    let mut line = text.into_bytes();
                    line.push(b'\n');
                    if tcp_wr.write_all(&line).await.is_err() {
                        break (CLOSE_NORMAL, "desktop_closed");
                    }
                }
                Some(Ok(Message::Binary(_))) => break (CLOSE_BAD_REQUEST, "binary_frame"),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break (0, ""),
            },
            line = line_rx.recv() => match line {
                Some(Ok(text)) => {
                    if ws_tx.send(Message::Text(text)).await.is_err() {
                        break (0, "");
                    }
                }
                Some(Err(())) => break (CLOSE_BAD_REQUEST, "bad_line"),
                None => break (CLOSE_NORMAL, "desktop_closed"),
            },
        }
    };
    reader.abort();
    let _ = tcp_wr.shutdown().await;
    if code != 0 {
        let _ = ws_tx
            .send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))
            .await;
    }
    code
}

/// Desktop → board: one `Ok(line)` per TCP line (line ending removed), one
/// `Err(())` for a line over [`MAX_LINE`] or not UTF-8, and the channel closes
/// at end of stream.
async fn read_lines(rd: OwnedReadHalf, tx: mpsc::Sender<Result<String, ()>>) {
    let mut reader = BufReader::new(rd);
    loop {
        let mut buf = Vec::new();
        // Room for the longest line, `\r\n`, and one byte to tell "too long".
        match (&mut reader).take((MAX_LINE + 3) as u64).read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if buf.last() != Some(&b'\n') {
            if buf.len() > MAX_LINE + 2 {
                let _ = tx.send(Err(())).await;
            }
            return;
        }
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        if buf.len() > MAX_LINE {
            let _ = tx.send(Err(())).await;
            return;
        }
        let Ok(text) = String::from_utf8(buf) else {
            let _ = tx.send(Err(())).await;
            return;
        };
        if tx.send(Ok(text)).await.is_err() {
            return;
        }
    }
}

// ── Board state through Desktop's phone gateway ───────────────────────────

#[derive(Debug)]
enum GatewayFail {
    /// No gateway file, or nothing listening at its url.
    Offline,
    /// The gateway took too long (a stop may or may not have happened).
    Timeout,
    /// The gateway answered something other than JSON or refused the token.
    Bad(String),
    /// The gateway has no such route (an older Desktop).
    NoRoute,
}

async fn call_gateway(
    cfg: &UsbaConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
    timeout: Duration,
) -> Result<Value, GatewayFail> {
    let file = cfg.gateway_file.as_ref().ok_or(GatewayFail::Offline)?;
    let raw = tokio::fs::read(file).await.map_err(|_| GatewayFail::Offline)?;
    let gw: Value = serde_json::from_slice(&raw).map_err(|_| GatewayFail::Offline)?;
    let (Some(url), Some(token)) = (gw.get("url").and_then(Value::as_str), gw.get("token").and_then(Value::as_str)) else {
        return Err(GatewayFail::Offline);
    };
    let mut req = crate::net_client::client()
        .request(method, format!("{}{path}", url.trim_end_matches('/')))
        .header("x-allternit-phone-token", token)
        .timeout(timeout);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req.send().await.map_err(|e| {
        if e.is_timeout() {
            GatewayFail::Timeout
        } else if e.is_connect() {
            GatewayFail::Offline
        } else {
            GatewayFail::Bad(format!("phone gateway unreachable: {e}"))
        }
    })?;
    let status = res.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(GatewayFail::Bad(format!("phone gateway refused the request ({status})")));
    }
    let parsed = res.json::<Value>().await;
    match parsed {
        // A tool result is the answer whatever the HTTP status.
        Ok(v) if v.get("ok").is_some() || status.is_success() => Ok(v),
        _ if status == reqwest::StatusCode::NOT_FOUND => Err(GatewayFail::NoRoute),
        Ok(_) => Err(GatewayFail::Bad(format!("phone gateway answered {status}"))),
        Err(e) => Err(GatewayFail::Bad(format!("phone gateway answered badly: {e}"))),
    }
}

const DESKTOP_OFFLINE: &str = "Allternit Desktop isn't running on this computer, or its phone gateway is off.";

fn desktop_error(why: &str) -> Response {
    err(StatusCode::BAD_GATEWAY, "desktop_error", why)
}

async fn list_boards(Extension(_user): Extension<AuthUser>, Extension(cfg): Extension<UsbaConfig>) -> Response {
    match call_gateway(&cfg, reqwest::Method::GET, "/phone/boards", None, LIST_TIMEOUT).await {
        Ok(v) => {
            let boards = v.get("boards").and_then(Value::as_array).cloned().unwrap_or_default();
            Json(json!({ "desktop": true, "boards": boards })).into_response()
        }
        Err(GatewayFail::Offline) => Json(json!({ "desktop": false, "boards": [] })).into_response(),
        // Desktop runs but is older than Link boards: it has none to report.
        Err(GatewayFail::NoRoute) => Json(json!({ "desktop": true, "boards": [] })).into_response(),
        Err(GatewayFail::Timeout) => desktop_error("Allternit Desktop did not answer in time"),
        Err(GatewayFail::Bad(why)) => desktop_error(&why),
    }
}

fn result_message(v: &Value, fallback: &str) -> String {
    v.get("message").and_then(Value::as_str).unwrap_or(fallback).to_string()
}

async fn stop_board(
    Extension(_user): Extension<AuthUser>,
    Extension(cfg): Extension<UsbaConfig>,
    Path(board_id): Path<String>,
) -> Response {
    if !is_board_id(&board_id) {
        return err(StatusCode::BAD_REQUEST, "bad_board", "boardId must be usba- followed by 12 lowercase hex characters");
    }
    let body = json!({ "boardId": board_id });
    match call_gateway(&cfg, reqwest::Method::POST, "/phone/board/stop", Some(body), STOP_TIMEOUT).await {
        Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Json(json!({ "ok": true })).into_response(),
        Ok(v) => match v.get("error").and_then(Value::as_str).unwrap_or_default() {
            "device_offline" | "board_offline" | "offline" | "not_found" => err(
                StatusCode::CONFLICT,
                "board_offline",
                &result_message(&v, "The board is not connected to Allternit Desktop."),
            ),
            "uncertain" => err(
                StatusCode::BAD_GATEWAY,
                "uncertain",
                &result_message(&v, "The board did not confirm the stop. Press its button to be sure."),
            ),
            other => err(
                StatusCode::BAD_GATEWAY,
                if other.is_empty() { "desktop_error" } else { other },
                &result_message(&v, "Allternit Desktop could not stop the board."),
            ),
        },
        Err(GatewayFail::Offline) => err(StatusCode::SERVICE_UNAVAILABLE, "desktop_offline", DESKTOP_OFFLINE),
        Err(GatewayFail::Timeout) => err(
            StatusCode::BAD_GATEWAY,
            "uncertain",
            "Allternit Desktop did not confirm the stop in time. Press the board's button to be sure.",
        ),
        Err(GatewayFail::NoRoute) => desktop_error("This Allternit Desktop is too old to stop a Link board"),
        Err(GatewayFail::Bad(why)) => desktop_error(&why),
    }
}

async fn forget_board(
    Extension(_user): Extension<AuthUser>,
    Extension(cfg): Extension<UsbaConfig>,
    Path(board_id): Path<String>,
) -> Response {
    if !is_board_id(&board_id) {
        return err(StatusCode::BAD_REQUEST, "bad_board", "boardId must be usba- followed by 12 lowercase hex characters");
    }
    let body = json!({ "boardId": board_id });
    match call_gateway(&cfg, reqwest::Method::POST, "/phone/board/forget", Some(body), STOP_TIMEOUT).await {
        Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Json(json!({ "ok": true })).into_response(),
        Ok(v) => desktop_error(&result_message(&v, "Allternit Desktop could not forget the board.")),
        Err(GatewayFail::Offline) => err(StatusCode::SERVICE_UNAVAILABLE, "desktop_offline", DESKTOP_OFFLINE),
        Err(GatewayFail::Timeout) => desktop_error("Allternit Desktop did not answer in time"),
        Err(GatewayFail::NoRoute) => desktop_error("This Allternit Desktop is too old to forget a Link board"),
        Err(GatewayFail::Bad(why)) => desktop_error(&why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    const BOARD: &str = "usba-0123456789ab";

    fn user(id: &str) -> AuthUser {
        AuthUser {
            user_id: id.to_string(),
            email: None,
            name: None,
            avatar_url: None,
            tenant_id: None,
            organization_id: None,
            organization_role: None,
            organization_slug: None,
        }
    }

    /// The node's routes on a real loopback server, as the given account.
    async fn node(account: &str, port: u16, gateway_file: Option<PathBuf>) -> String {
        let app: Router = router::<()>(UsbaConfig { port, gateway_file }).layer(Extension(user(account)));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("127.0.0.1:{}", addr.port())
    }

    type Board = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

    async fn board_socket(node: &str, board: &str) -> Board {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{node}/phones/usba?board={board}")).await.unwrap();
        ws
    }

    /// The next frame the board gets, waiting at most 5 s.
    async fn next(ws: &mut Board) -> Option<WsMsg> {
        tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("no frame in 5 s").and_then(Result::ok)
    }

    async fn close_code(ws: &mut Board) -> u16 {
        loop {
            match next(ws).await {
                Some(WsMsg::Close(Some(frame))) => return u16::from(frame.code),
                Some(WsMsg::Close(None)) | None => return 0,
                Some(_) => continue,
            }
        }
    }

    /// A stand-in for Desktop's USB Agent listener (the TCP edge).
    async fn desktop_listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    async fn accept(listener: &TcpListener) -> BufReader<TcpStream> {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.expect("node never dialled").unwrap();
        BufReader::new(tcp)
    }

    async fn read_line(desk: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), desk.read_line(&mut line)).await.expect("no line in 5 s").unwrap();
        line
    }

    #[tokio::test]
    async fn lines_pass_both_ways_and_desktop_closing_closes_the_board() {
        let (listener, port) = desktop_listener().await;
        let node = node("acct-pipe", port, None).await;
        let mut board = board_socket(&node, BOARD).await;
        let mut desk = accept(&listener).await;

        let hello = "hello usba/2 usba-0123456789ab 1.1.0 poweron 00112233445566778899aabbccddeeff";
        board.send(WsMsg::Text(hello.into())).await.unwrap();
        assert_eq!(read_line(&mut desk).await, format!("{hello}\n"));

        let challenge = format!("challenge {} {}", "ab".repeat(16), "cd".repeat(32));
        desk.get_mut().write_all(format!("{challenge}\n").as_bytes()).await.unwrap();
        assert_eq!(next(&mut board).await, Some(WsMsg::Text(challenge)));

        // A full 512-byte line still passes, in both directions.
        let longest = "x".repeat(MAX_LINE);
        board.send(WsMsg::Text(longest.clone())).await.unwrap();
        assert_eq!(read_line(&mut desk).await, format!("{longest}\n"));
        desk.get_mut().write_all(format!("{longest}\r\n").as_bytes()).await.unwrap();
        assert_eq!(next(&mut board).await, Some(WsMsg::Text(longest)));

        drop(desk);
        assert_eq!(close_code(&mut board).await, CLOSE_NORMAL);
    }

    #[tokio::test]
    async fn an_overlong_or_binary_frame_closes_4400_and_drops_the_desktop_side() {
        let (listener, port) = desktop_listener().await;
        let node = node("acct-long", port, None).await;

        // Board → Desktop: 513 bytes.
        let mut board = board_socket(&node, BOARD).await;
        let mut desk = accept(&listener).await;
        board.send(WsMsg::Text("x".repeat(MAX_LINE + 1))).await.unwrap();
        assert_eq!(close_code(&mut board).await, CLOSE_BAD_REQUEST);
        assert_eq!(read_line(&mut desk).await, "", "Desktop's side is closed and got nothing");

        // Desktop → board: 513 bytes.
        let mut board = board_socket(&node, BOARD).await;
        let mut desk = accept(&listener).await;
        desk.get_mut().write_all(format!("{}\n", "y".repeat(MAX_LINE + 1)).as_bytes()).await.unwrap();
        assert_eq!(close_code(&mut board).await, CLOSE_BAD_REQUEST);
        assert_eq!(read_line(&mut desk).await, "");

        // A binary frame, and a frame that would smuggle a second line.
        for frame in [WsMsg::Binary(b"c 1 ping".to_vec()), WsMsg::Text("c 1 ping x\nc 2 disarm y".into())] {
            let mut board = board_socket(&node, BOARD).await;
            let mut desk = accept(&listener).await;
            board.send(frame).await.unwrap();
            assert_eq!(close_code(&mut board).await, CLOSE_BAD_REQUEST);
            assert_eq!(read_line(&mut desk).await, "");
        }

        // A malformed board id never dials Desktop.
        for bad in ["usba-0123456789AB", "usba-0123", "x"] {
            let mut board = board_socket(&node, bad).await;
            assert_eq!(close_code(&mut board).await, CLOSE_BAD_REQUEST, "{bad}");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(300), listener.accept()).await.is_err(),
            "a bad board id must not reach Desktop"
        );
    }

    #[tokio::test]
    async fn no_desktop_listener_closes_4503() {
        let (listener, port) = desktop_listener().await;
        drop(listener);
        let node = node("acct-offline", port, None).await;
        let mut board = board_socket(&node, BOARD).await;
        assert_eq!(close_code(&mut board).await, CLOSE_DESKTOP_OFFLINE);
    }

    #[tokio::test]
    async fn a_second_socket_for_the_same_board_closes_the_first_with_4409() {
        let (listener, port) = desktop_listener().await;
        let mine = node("acct-twice", port, None).await;
        let mut first = board_socket(&mine, BOARD).await;
        let mut first_desk = accept(&listener).await;
        let mut second = board_socket(&mine, BOARD).await;
        let mut second_desk = accept(&listener).await;

        assert_eq!(close_code(&mut first).await, CLOSE_REPLACED);
        assert_eq!(read_line(&mut first_desk).await, "", "the first pipe's Desktop side is closed");
        second.send(WsMsg::Text("hello usba/2 again".into())).await.unwrap();
        assert_eq!(read_line(&mut second_desk).await, "hello usba/2 again\n", "the new socket works");

        // Another account's socket for the same board id is its own slot.
        let other = node("acct-twice-other", port, None).await;
        let mut theirs = board_socket(&other, BOARD).await;
        let mut theirs_desk = accept(&listener).await;
        theirs.send(WsMsg::Text("theirs".into())).await.unwrap();
        assert_eq!(read_line(&mut theirs_desk).await, "theirs\n");
        second.send(WsMsg::Text("still mine".into())).await.unwrap();
        assert_eq!(read_line(&mut second_desk).await, "still mine\n");
    }

    async fn get_json(client: &reqwest::Client, url: String) -> (u16, Value) {
        let res = client.get(url).send().await.unwrap();
        (res.status().as_u16(), res.json().await.unwrap())
    }

    #[tokio::test]
    async fn board_routes_answer_from_the_desktop_gateway_or_say_desktop_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();

        // No gateway file: Desktop is off.
        let off = node("acct-gw", 1, Some(dir.path().join("phone-gateway.json"))).await;
        assert_eq!(get_json(&client, format!("http://{off}/phones/boards")).await, (200, json!({ "desktop": false, "boards": [] })));
        let res = client.post(format!("http://{off}/phones/boards/{BOARD}/stop")).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 503);
        assert_eq!(res.json::<Value>().await.unwrap()["error"], "desktop_offline");
        let res = client.delete(format!("http://{off}/phones/boards/{BOARD}")).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 503);

        // A gateway file whose Desktop is gone counts as off too.
        let (gone, gone_port) = desktop_listener().await;
        drop(gone);
        let stale = dir.path().join("stale.json");
        std::fs::write(&stale, json!({ "url": format!("http://127.0.0.1:{gone_port}"), "token": "t" }).to_string()).unwrap();
        let off = node("acct-gw", 1, Some(stale)).await;
        assert_eq!(get_json(&client, format!("http://{off}/phones/boards")).await, (200, json!({ "desktop": false, "boards": [] })));
        let res = client.post(format!("http://{off}/phones/boards/{BOARD}/stop")).send().await.unwrap();
        assert_eq!(res.status().as_u16(), 503);
        let body = res.json::<Value>().await.unwrap();
        assert_eq!((body["error"].as_str(), body["message"].is_string()), (Some("desktop_offline"), true));

        // A running gateway (the loopback HTTP edge Desktop serves).
        let gateway: Router = Router::new()
            .route(
                "/phone/boards",
                get(|h: axum::http::HeaderMap| async move {
                    if h.get("x-allternit-phone-token").and_then(|v| v.to_str().ok()) != Some("gw-token") {
                        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" })));
                    }
                    (StatusCode::OK, Json(json!({ "boards": [{ "boardId": BOARD, "armed": true, "link": "cloud" }] })))
                }),
            )
            .route(
                "/phone/board/stop",
                post(|Json(b): Json<Value>| async move {
                    match b["boardId"].as_str() {
                        Some(BOARD) => Json(json!({ "ok": true })),
                        Some("usba-00000000000f") => Json(json!({ "ok": false, "error": "uncertain", "message": "no reply" })),
                        _ => Json(json!({ "ok": false, "error": "device_offline", "message": "The board is not connected." })),
                    }
                }),
            )
            .route("/phone/board/forget", post(|| async { Json(json!({ "ok": true })) }));
        let gw_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let gw_port = gw_listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(gw_listener, gateway).await.unwrap() });
        let file = dir.path().join("gw.json");
        std::fs::write(&file, json!({ "url": format!("http://127.0.0.1:{gw_port}"), "token": "gw-token" }).to_string()).unwrap();
        let on = node("acct-gw", 1, Some(file.clone())).await;

        assert_eq!(
            get_json(&client, format!("http://{on}/phones/boards")).await,
            (200, json!({ "desktop": true, "boards": [{ "boardId": BOARD, "armed": true, "link": "cloud" }] }))
        );
        let stop = |id: &str| client.post(format!("http://{on}/phones/boards/{id}/stop")).send();
        let res = stop(BOARD).await.unwrap();
        assert_eq!((res.status().as_u16(), res.json::<Value>().await.unwrap()), (200, json!({ "ok": true })));
        let res = stop("usba-000000000001").await.unwrap();
        assert_eq!(res.status().as_u16(), 409);
        assert_eq!(res.json::<Value>().await.unwrap(), json!({ "error": "board_offline", "message": "The board is not connected." }));
        let res = stop("usba-00000000000f").await.unwrap();
        assert_eq!(res.status().as_u16(), 502);
        assert_eq!(res.json::<Value>().await.unwrap()["error"], "uncertain");
        let res = stop("not-a-board").await.unwrap();
        assert_eq!(res.status().as_u16(), 400);
        let res = client.delete(format!("http://{on}/phones/boards/{BOARD}")).send().await.unwrap();
        assert_eq!((res.status().as_u16(), res.json::<Value>().await.unwrap()), (200, json!({ "ok": true })));

        // A wrong gateway token is not "Desktop is off": it is an error.
        std::fs::write(&file, json!({ "url": format!("http://127.0.0.1:{gw_port}"), "token": "wrong" }).to_string()).unwrap();
        let (status, body) = get_json(&client, format!("http://{on}/phones/boards")).await;
        assert_eq!((status, body["error"].as_str()), (502, Some("desktop_error")));
    }
}
