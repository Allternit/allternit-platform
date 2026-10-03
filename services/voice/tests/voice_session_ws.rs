//! WebSocket integration: a real tokio-tungstenite client against the
//! `/v1/voice/session` route backed by the mock engine.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use voice_service::session::mock::{MockConfig, MockEngine};
use voice_service::session::ws::{
    router_with, SessionRouteState, TicketClaims, TicketVerifier, VerifyFuture,
};

async fn serve(state: SessionRouteState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router_with(state);
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

fn engine() -> Arc<MockEngine> {
    Arc::new(MockEngine::new(MockConfig {
        transcripts: vec!["what time is it".into()],
        ..Default::default()
    }))
}

fn speech_frame(amp: f32) -> Vec<u8> {
    (0..320)
        .flat_map(|i| {
            let s = (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 16_000.0).sin() * amp;
            ((s * 32767.0) as i16).to_le_bytes()
        })
        .collect()
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Next JSON event, plus the speech audio bytes received before it.
async fn next_json(ws: &mut Ws) -> (Value, usize) {
    let mut audio = 0usize;
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("ws error");
        match msg {
            Message::Text(t) => return (serde_json::from_str::<Value>(&t).unwrap(), audio),
            Message::Binary(b) => audio += b.len(),
            Message::Close(_) => return (json!({"type": "<closed>"}), audio),
            _ => {}
        }
    }
}

#[tokio::test]
async fn full_session_flow_over_websocket() {
    let addr = serve(SessionRouteState::new(engine(), Some("s3cret".into()))).await;
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/v1/voice/session?token=s3cret"))
            .await
            .expect("connect");

    ws.send(Message::Text(
        json!({"type": "session.start", "turn": {"mode": "smart"}}).to_string(),
    ))
    .await
    .unwrap();
    let (ready, _) = next_json(&mut ws).await;
    assert_eq!(ready["type"], "session.ready");
    assert_eq!(ready["protocol"], 1);
    assert_eq!(ready["engine"], "device");
    assert_eq!(ready["outputSampleRate"], 24000);
    assert_eq!(ready["models"]["turn"], "mock-turn");

    // 500 ms of speech then 300 ms of silence, as 20 ms binary frames.
    for i in 0..40 {
        let amp = if i < 25 { 0.3 } else { 0.0 };
        ws.send(Message::Binary(speech_frame(amp))).await.unwrap();
    }
    let mut seen = Vec::new();
    let turn = loop {
        let (ev, _) = next_json(&mut ws).await;
        if ev["type"] == "turn.ended" {
            break ev;
        }
        seen.push(ev["type"].as_str().unwrap().to_string());
    };
    assert_eq!(turn["text"], "what time is it");
    for kind in [
        "speech.started",
        "transcript.delta",
        "speech.stopped",
        "transcript.final",
    ] {
        assert!(seen.iter().any(|k| k == kind), "missing {kind} in {seen:?}");
    }

    // An unknown message is a non-fatal error; the session continues.
    ws.send(Message::Text(json!({"type": "nonsense"}).to_string()))
        .await
        .unwrap();
    let (err, _) = next_json(&mut ws).await;
    assert_eq!(
        (
            err["type"].as_str(),
            err["code"].as_str(),
            err["fatal"].as_bool()
        ),
        (Some("error"), Some("bad_message"), Some(false))
    );

    // The app streams the reply back.
    for text in ["It is ", "noon. ", "Anything else?"] {
        ws.send(Message::Text(
            json!({"type": "speak.delta", "id": "r1", "text": text}).to_string(),
        ))
        .await
        .unwrap();
    }
    ws.send(Message::Text(
        json!({"type": "speak.done", "id": "r1"}).to_string(),
    ))
    .await
    .unwrap();
    let (started, _) = next_json(&mut ws).await;
    assert_eq!(started, json!({"type": "speak.started", "id": "r1"}));
    let (ended, audio) = next_json(&mut ws).await;
    assert_eq!(ended, json!({"type": "speak.ended", "id": "r1"}));
    let chars = "It is noon.".len() + "Anything else?".len();
    assert_eq!(audio, chars * 20 * 24 * 2, "speech audio bytes");

    ws.send(Message::Text(json!({"type": "session.end"}).to_string()))
        .await
        .unwrap();
    let (closed, _) = next_json(&mut ws).await;
    assert_eq!(closed["type"], "<closed>");
}

async fn http_status(url: String) -> u16 {
    match tokio_tungstenite::connect_async(url).await {
        Ok(_) => 101,
        Err(WsError::Http(resp)) => resp.status().as_u16(),
        Err(e) => panic!("unexpected error {e}"),
    }
}

#[tokio::test]
async fn auth_rules() {
    // Token configured: required and must match.
    let addr = serve(SessionRouteState::new(engine(), Some("s3cret".into()))).await;
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session")).await,
        401
    );
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session?token=wrong")).await,
        401
    );
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session?token=s3cret")).await,
        101
    );
    // Tickets are rejected by the default verifier, never faked.
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session?ticket=abc")).await,
        401
    );

    // No token configured: loopback peers are allowed.
    let addr = serve(SessionRouteState::new(engine(), None)).await;
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session")).await,
        101
    );

    // A plugged-in verifier decides tickets.
    struct AcceptGood;
    impl TicketVerifier for AcceptGood {
        fn verify<'a>(&'a self, ticket: &'a str) -> VerifyFuture<'a> {
            Box::pin(async move {
                if ticket == "good" {
                    Ok(TicketClaims {
                        sub: "u".into(),
                        plan: "pro".into(),
                        max_seconds: 60,
                    })
                } else {
                    Err("bad ticket".into())
                }
            })
        }
    }
    let mut state = SessionRouteState::new(engine(), Some("s3cret".into()));
    state.verifier = Arc::new(AcceptGood);
    let addr = serve(state).await;
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session?ticket=good")).await,
        101
    );
    assert_eq!(
        http_status(format!("ws://{addr}/v1/voice/session?ticket=bad")).await,
        401
    );
}
