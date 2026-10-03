//! Cloud tickets: redeem against a mock cloud-api, enforce `maxSeconds`, and
//! report Cloud Voice minutes once on close.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use voice_service::session::mock::{MockConfig, MockEngine};
use voice_service::session::ws::{router_with, CloudApi, SessionRouteState, UsageReporter};

const WORKER_TOKEN: &str = "worker-secret";

#[derive(Clone, Default)]
struct Mock {
    max_seconds: u64,
    usage: Arc<Mutex<Vec<Value>>>,
    /// Usage posts to fail with 503 before accepting.
    usage_failures: Arc<Mutex<u32>>,
}

fn authed(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {WORKER_TOKEN}"))
}

async fn redeem(
    State(m): State<Mock>,
    h: HeaderMap,
    Json(b): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if !authed(&h) || b["ticket"] != "good" {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid"})));
    }
    (
        StatusCode::OK,
        Json(json!({"sub": "user-1", "plan": "pro", "maxSeconds": m.max_seconds})),
    )
}

async fn usage(State(m): State<Mock>, h: HeaderMap, Json(b): Json<Value>) -> StatusCode {
    if !authed(&h) {
        return StatusCode::UNAUTHORIZED;
    }
    {
        let mut f = m.usage_failures.lock().unwrap();
        if *f > 0 {
            *f -= 1;
            return StatusCode::SERVICE_UNAVAILABLE;
        }
    }
    m.usage.lock().unwrap().push(b);
    StatusCode::OK
}

async fn spawn_router(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

async fn mock_cloud(m: Mock) -> String {
    let app = Router::new()
        .route("/api/v1/voice/tickets/redeem", post(redeem))
        .route("/api/v1/voice/usage", post(usage))
        .with_state(m);
    format!("http://{}", spawn_router(app).await)
}

async fn voice(cloud_url: &str, token: Option<&str>) -> SocketAddr {
    let engine = Arc::new(MockEngine::new(MockConfig::default()));
    let mut state = SessionRouteState::new(engine, token.map(Into::into))
        .with_cloud(CloudApi::new(cloud_url, WORKER_TOKEN));
    state.usage = state
        .usage
        .map(|u: UsageReporter| u.with_backoff(vec![Duration::from_millis(50); 3]));
    spawn_router(router_with(state)).await
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("ws error");
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Close(_) => return json!({"type": "<closed>"}),
            _ => {}
        }
    }
}

async fn start(ws: &mut Ws) -> Value {
    ws.send(Message::Text(json!({"type": "session.start"}).to_string()))
        .await
        .unwrap();
    next_json(ws).await
}

async fn refusal(url: String) -> (u16, String) {
    match tokio_tungstenite::connect_async(url).await {
        Err(WsError::Http(resp)) => {
            let body = resp.body().clone().unwrap_or_default();
            (
                resp.status().as_u16(),
                String::from_utf8_lossy(&body).into_owned(),
            )
        }
        Ok(_) => (101, String::new()),
        Err(e) => panic!("unexpected {e}"),
    }
}

async fn wait_usage(m: &Mock, n: usize) {
    for _ in 0..100 {
        if m.usage.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn good_ticket_opens_a_cloud_session_and_usage_posts_once_on_close() {
    let m = Mock {
        max_seconds: 600,
        ..Default::default()
    };
    let addr = voice(&mock_cloud(m.clone()).await, Some("s3cret")).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice/session?ticket=good"))
            .await
            .unwrap();
    let ready = start(&mut ws).await;
    assert_eq!(ready["type"], "session.ready");
    assert_eq!(ready["engine"], "cloud");
    tokio::time::sleep(Duration::from_millis(300)).await;
    ws.send(Message::Text(json!({"type": "session.end"}).to_string()))
        .await
        .unwrap();
    while next_json(&mut ws).await["type"] != "<closed>" {}

    wait_usage(&m, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await; // no duplicate arrives
    let posts = m.usage.lock().unwrap().clone();
    assert_eq!(posts.len(), 1, "{posts:?}");
    assert_eq!(posts[0]["sub"], "user-1");
    assert_eq!(posts[0]["sessionId"], ready["sessionId"]);
    assert_eq!(posts[0]["engine"], "cloud");
    assert_eq!(posts[0]["seconds"], 1); // 0.3 s rounds up
}

#[tokio::test]
async fn usage_retries_after_a_failed_post() {
    let m = Mock {
        max_seconds: 600,
        usage_failures: Arc::new(Mutex::new(2)),
        ..Default::default()
    };
    let addr = voice(&mock_cloud(m.clone()).await, None).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice/session?ticket=good"))
            .await
            .unwrap();
    start(&mut ws).await;
    drop(ws);
    wait_usage(&m, 1).await;
    assert_eq!(m.usage.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unauthorized_ticket_is_refused() {
    let addr = voice(&mock_cloud(Mock::default()).await, None).await;
    let (status, _) = refusal(format!("ws://{addr}/v1/voice/session?ticket=bad")).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn unreachable_cloud_is_refused_with_a_clear_message() {
    // A port nothing listens on.
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let addr = voice(&url, None).await;
    let (status, body) = refusal(format!("ws://{addr}/v1/voice/session?ticket=good")).await;
    assert_eq!(status, 401);
    assert!(body.contains("cannot reach Allternit Cloud"), "{body}");
    assert!(!body.contains("good") && !body.contains(WORKER_TOKEN));
}

#[tokio::test]
async fn max_seconds_closes_the_session_with_session_limit() {
    let m = Mock {
        max_seconds: 1,
        ..Default::default()
    };
    let addr = voice(&mock_cloud(m.clone()).await, None).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice/session?ticket=good"))
            .await
            .unwrap();
    assert_eq!(start(&mut ws).await["type"], "session.ready");
    let err = next_json(&mut ws).await;
    assert_eq!(err["type"], "error");
    assert_eq!(err["code"], "session_limit");
    assert_eq!(err["fatal"], true);
    assert_eq!(next_json(&mut ws).await["type"], "<closed>");
    wait_usage(&m, 1).await;
    let secs = m.usage.lock().unwrap()[0]["seconds"].as_u64().unwrap();
    assert!((1..=2).contains(&secs), "{secs}");
}

#[tokio::test]
async fn token_sessions_post_no_usage() {
    let m = Mock {
        max_seconds: 600,
        ..Default::default()
    };
    let addr = voice(&mock_cloud(m.clone()).await, Some("s3cret")).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice/session?token=s3cret"))
            .await
            .unwrap();
    let ready = start(&mut ws).await;
    assert_eq!(ready["engine"], "device");
    ws.send(Message::Text(json!({"type": "session.end"}).to_string()))
        .await
        .unwrap();
    while next_json(&mut ws).await["type"] != "<closed>" {}
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(m.usage.lock().unwrap().is_empty());
}
