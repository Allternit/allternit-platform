//! `send` to bots that aren't on this computer, against a mock allternit-api
//! serving the routes the engine calls (the real paths, so a route move in
//! allternit-api shows up here): hosted → a turn in the thread's session,
//! vendor → a ticket, channel → the thread's channel relay, unknown → 404,
//! allternit-api unusable → transport. Nothing is recorded for a send that
//! can't be attempted.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use allternit_factory_engine::registry::Registry;
use allternit_factory_engine::send::{self, ApiLink, SendCtx, SendRequest};
use allternit_factory_engine::ledger::ledger::LedgerOptions;
use allternit_factory_engine::{Ledger, LedgerQuery};
use axum::extract::Path as P;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<(String, Value, Option<String>)>>>;

async fn mock(seen: Seen) -> String {
    let s1 = seen.clone();
    let s2 = seen.clone();
    let s3 = seen.clone();
    let app = Router::new()
        .route(
            "/api/v1/gateway/bots/:id/execution-binding",
            get(|P(id): P<String>| async move {
                match id.as_str() {
                    "vend" => (StatusCode::OK, Json(json!({ "binding": { "type": "vendor", "botId": "vend" } }))),
                    "broken" => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "db down" }))),
                    _ => (StatusCode::NOT_FOUND, Json(json!({ "error": "execution binding not found" }))),
                }
            }),
        )
        .route(
            "/api/v1/threads",
            get(|q: axum::extract::Query<std::collections::HashMap<String, String>>| async move {
                let bot = q.get("botId").cloned().unwrap_or_default();
                let threads = if bot == "al" { json!([{ "id": "thr_task", "kind": "task" }, { "id": "thr_main", "kind": "standing" }]) } else { json!([]) };
                Json(json!({ "threads": threads }))
            }),
        )
        .route("/api/v1/threads/:id", get(|P(id): P<String>| async move { Json(json!({ "id": id, "currentSessionId": format!("ses_{id}") })) }))
        .route(
            "/api/v1/agent-sessions/:id/messages",
            post(move |P(id): P<String>, h: HeaderMap, Json(b): Json<Value>| {
                let seen = s1.clone();
                async move {
                    seen.lock().unwrap().push((format!("session {id}"), b, h.get("authorization").map(|v| v.to_str().unwrap().to_string())));
                    Json(json!({ "id": "msg_1" }))
                }
            }),
        )
        .route(
            "/api/v1/vendor-bots/:id/tickets",
            post(move |P(id): P<String>, Json(b): Json<Value>| {
                let seen = s2.clone();
                async move {
                    seen.lock().unwrap().push((format!("ticket {id}"), b, None));
                    (StatusCode::CREATED, Json(json!({ "ticket": { "id": "vt_1", "n": 14 } })))
                }
            }),
        )
        .route(
            "/api/v1/gateway/threads/:id/channel-send",
            post(move |P(id): P<String>, Json(b): Json<Value>| {
                let seen = s3.clone();
                async move {
                    seen.lock().unwrap().push((format!("channel {id}"), b, None));
                    (StatusCode::ACCEPTED, Json(json!({ "state": "unconfirmed" })))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn ctx(root: &Path, base: Option<String>) -> SendCtx {
    SendCtx {
        root: root.to_path_buf(),
        registry: Registry::at(root.join("registry.json")),
        api: ApiLink { base, authorization: Some("Bearer t0k".into()), desktop: None },
        sender: "user:eoj".into(),
    }
}

async fn count(root: &Path) -> usize {
    Ledger::new(LedgerOptions { root_dir: Some(root.to_path_buf()), ledger_dir: Some(PathBuf::from(".allternit/ledger")) })
        .query(LedgerQuery::default())
        .await
        .unwrap()
        .len()
}

fn req(to: &str) -> SendRequest {
    SendRequest { to: to.into(), text: "do the thing".into(), ..Default::default() }
}

#[tokio::test]
async fn remote_bindings_deliver_the_way_their_binding_allows() {
    let tmp = tempfile::Builder::new().prefix("f3snd").tempdir_in("/tmp").unwrap();
    let root = tmp.path().to_path_buf();
    let seen: Seen = Arc::default();
    let c = ctx(&root, Some(mock(seen.clone()).await));

    // Hosted: the standing thread's session gets a turn, as the caller.
    let d = send::send(&c, &req("al")).await.unwrap();
    assert_eq!((d.via.as_str(), d.state.as_str(), d.to.as_str()), ("session", "verified", "al"), "{d:?}");
    {
        let s = seen.lock().unwrap();
        assert_eq!(s[0].0, "session ses_thr_main");
        assert_eq!(s[0].1["text"], "do the thing");
        assert_eq!(s[0].2.as_deref(), Some("Bearer t0k"));
    }

    // Vendor: a ticket in the given thread; queued until the lane answers.
    let d = send::send(&c, &SendRequest { thread_id: Some("thr_v".into()), ..req("vend") }).await.unwrap();
    assert_eq!((d.via.as_str(), d.state.as_str(), d.ticket.as_deref()), ("vendor_ticket", "queued", Some("T-14")), "{d:?}");
    assert_eq!(d.thread_id.as_deref(), Some("thr_v"));
    assert_eq!(seen.lock().unwrap()[1].1, json!({ "instructions": "do the thing", "threadId": "thr_v" }));
    // A vendor ticket without its thread is a recorded failure, not a guess.
    let d = send::send(&c, &req("vend")).await.unwrap();
    assert_eq!(d.state, "failed");

    // Channel: unconfirmed by the provider → best_effort, never "verified".
    let d = send::send(&c, &req("channel:thr_tg")).await.unwrap();
    assert_eq!((d.via.as_str(), d.state.as_str()), ("channel", "best_effort"), "{d:?}");
    assert_eq!(seen.lock().unwrap()[2].0, "channel thr_tg");

    // Unknown bot, broken api, no api at all: refused before anything is written.
    let n = count(&root).await;
    assert_eq!(send::send(&c, &req("ghost")).await.unwrap_err().code, "not_found");
    assert_eq!(send::send(&c, &req("broken")).await.unwrap_err().code, "transport");
    assert_eq!(send::send(&ctx(&root, None), &req("al")).await.unwrap_err().code, "not_found");
    assert_eq!(count(&root).await, n, "a send that couldn't be attempted wrote to the ledger");

    // Dry runs name the delivery the binding decides.
    assert_eq!(send::plan(&c, &req("al")).await.unwrap().via, "session");
    assert_eq!(send::plan(&c, &req("vend")).await.unwrap().via, "vendor_ticket");
}
