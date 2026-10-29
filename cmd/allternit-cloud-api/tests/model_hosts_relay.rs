//! Phone model-host relay end to end over a real socket: a fake phone mints a
//! ticket, connects, says `hello`, and answers the requests gizzi sends
//! through `/api/v1/model-hosts/v1/*` with the owner's runtime device token.

mod common;

use common::TestApp;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Phone = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn user(app: &TestApp, id: &str) {
    sqlx::query("INSERT INTO users (id, email, status) VALUES ($1, $2, 'active') ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(format!("{id}@example.test"))
        .execute(&app.db)
        .await
        .unwrap();
}

/// An `allternit_*` API token with the `inference` scope.
async fn api_token(app: &TestApp, owner: &str) -> String {
    let token = format!("allternit_{owner}_api_token");
    sqlx::query("INSERT INTO api_tokens (id, token_hash, name, user_id, permissions) VALUES ($1, $2, 'test', $3, '[\"inference\"]')")
        .bind(format!("tok_{owner}"))
        .bind(hex::encode(Sha256::digest(token.as_bytes())))
        .bind(owner)
        .execute(&app.db)
        .await
        .unwrap();
    token
}

/// A paired runtime's device token (what gizzi holds).
async fn device_token(app: &TestApp, id: &str, owner: &str) -> String {
    let token = format!("allternit_runtime_{id}_token");
    sqlx::query(
        "INSERT INTO runtime_devices (id, user_id, name, runtime_type, capabilities, public_key_fingerprint,
                                      credential_hash, credential_expires_at, status, last_seen_at)
         VALUES ($1, $2, $1, 'desktop', '[]', 'fp', $3, now() + interval '1 day', 'online', now())",
    )
    .bind(id)
    .bind(owner)
    .bind(hex::encode(Sha256::digest(token.as_bytes())))
    .execute(&app.db)
    .await
    .unwrap();
    token
}

async fn next_json(phone: &mut Phone) -> Value {
    loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(10), phone.next())
            .await
            .expect("frame in time")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(text) = message {
            let frame: Value = serde_json::from_str(&text).unwrap();
            if frame["type"] != "ping" {
                return frame;
            }
        }
    }
}

/// The fake phone answers the next request with "Hel" + "lo" and `end`,
/// returning the request frame it got.
async fn answer(phone: &mut Phone) -> Value {
    let request = next_json(phone).await;
    assert_eq!(request["type"], "request", "{request}");
    let id = request["request_id"].as_str().unwrap().to_string();
    for text in ["Hel", "lo"] {
        let chunk = json!({"id":"chatcmpl-p","object":"chat.completion.chunk","created":1,"model":"lfm2.5-350m",
                           "choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]});
        let frame = json!({"type":"chunk","request_id":id,"data":chunk});
        phone.send(Message::Text(frame.to_string())).await.unwrap();
    }
    phone.send(Message::Text(json!({"type":"end","request_id":id}).to_string())).await.unwrap();
    request
}

#[tokio::test]
async fn phone_serves_its_owner_through_the_relay_and_nobody_else() {
    let app = TestApp::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = app.router.clone();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let owner = format!("u_mh_{}", uuid::Uuid::new_v4().simple());
    let stranger = format!("u_mh_{}", uuid::Uuid::new_v4().simple());
    let host_id = format!("phone-{}", uuid::Uuid::new_v4().simple());
    user(&app, &owner).await;
    user(&app, &stranger).await;
    let owner_api = api_token(&app, &owner).await;
    let gizzi = device_token(&app, &format!("rt_{owner}"), &owner).await;
    let stranger_gizzi = device_token(&app, &format!("rt_{stranger}"), &stranger).await;
    let http = reqwest::Client::new();

    // 1. Ticket → socket → hello → ready.
    let ticket: Value = http
        .post(format!("{base}/api/v1/model-hosts/ws-ticket"))
        .bearer_auth(&owner_api)
        .json(&json!({"host_id": host_id, "name": "Eoj's iPhone"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ws_url = ticket["ws_url"].as_str().unwrap_or_else(|| panic!("no ws_url: {ticket}"));
    assert!(ws_url.starts_with("ws://127.0.0.1:"), "{ws_url}");
    let (mut phone, _) = connect_async(ws_url).await.unwrap();
    let hello = json!({"type":"hello","models":[{"id":"lfm2.5-350m","name":"LFM2.5 350M","context_length":8192}]});
    phone.send(Message::Text(hello.to_string())).await.unwrap();
    assert_eq!(next_json(&mut phone).await, json!({"type":"ready","host_id": host_id}));

    // 2. Gizzi (device token) sees the phone's model.
    let public_id = format!("{host_id}/lfm2.5-350m");
    let models: Value = http
        .get(format!("{base}/api/v1/model-hosts/v1/models"))
        .bearer_auth(&gizzi)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = models["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [public_id.as_str()], "{models}");

    // 3. Streaming: the phone gets the bare model id, the caller gets SSE.
    let chat = |stream: bool| {
        http.post(format!("{base}/api/v1/model-hosts/v1/chat/completions"))
            .bearer_auth(&gizzi)
            .json(&json!({"model": public_id, "stream": stream, "messages":[{"role":"user","content":"hi"}]}))
            .send()
    };
    let (response, request) = tokio::join!(chat(true), answer(&mut phone));
    assert_eq!(request["body"]["model"], "lfm2.5-350m");
    assert_eq!(request["body"]["messages"][0]["content"], "hi");
    let response = response.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = response.text().await.unwrap();
    let data: Vec<&str> = body.lines().filter_map(|l| l.strip_prefix("data: ")).collect();
    assert_eq!(data.len(), 3, "{body}");
    let deltas: Vec<String> = data[..2]
        .iter()
        .map(|d| serde_json::from_str::<Value>(d).unwrap()["choices"][0]["delta"]["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deltas, ["Hel", "lo"]);
    assert_eq!(data[2], "[DONE]");

    // 4. Non-streaming folds into one completion under the public id.
    let (response, _) = tokio::join!(chat(false), answer(&mut phone));
    let response = response.unwrap();
    assert_eq!(response.status(), 200);
    let completion: Value = response.json().await.unwrap();
    assert_eq!(completion["object"], "chat.completion");
    assert_eq!(completion["model"], public_id.as_str());
    assert_eq!(completion["choices"][0]["message"]["content"], "Hello");

    // 5. Another user's credential sees nothing and can't call the model.
    let response = http
        .post(format!("{base}/api/v1/model-hosts/v1/chat/completions"))
        .bearer_auth(&stranger_gizzi)
        .json(&json!({"model": public_id, "messages":[{"role":"user","content":"hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let error: Value = response.json().await.unwrap();
    assert_eq!(error["error"]["type"], "model_not_found");
    let models: Value = http
        .get(format!("{base}/api/v1/model-hosts/v1/models"))
        .bearer_auth(&stranger_gizzi)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"], json!([]), "{models}");
}
