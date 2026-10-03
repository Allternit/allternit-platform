//! End-to-end for bot email that replies: a fake mailflare and a fake remote
//! runtime stand in for the real services, and the real webhook → turn →
//! reply pipeline runs against them on a temp data dir.
//!
//! Covers: approval-gated reply with threading headers and a quoted excerpt,
//! the strip-quoted-history behavior on the turn text, loop guards (no turn,
//! no send), the `auto` mode's admin skip-approval send, and the kill switch.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use allternit_api::agent_email_routes::agent_email_webhook_router_with;

const RELAY_TOKEN: &str = "runtime-device-token";
const RELAY_OWNER: &str = "user-a";
const INBOUND_PATH: &str = "/api/v1/agent-email/inbound";
use allternit_api::test_helpers::app_state;
use axum::extract::Path as AxumPath;
use axum::routing::{get as aget, post as apost};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tower::ServiceExt;

#[derive(Default)]
struct Recorded {
    /// Bodies posted to mailflare /api/v1/send.
    sends: Mutex<Vec<Value>>,
    /// POSTs to the fake runtime's session-create endpoint.
    session_creates: Mutex<u32>,
    /// Bodies posted to the fake runtime's turn endpoint.
    turns: Mutex<Vec<Value>>,
}

fn sign_webhook(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// POST a signed message.inbound webhook and return the response status/body.
async fn post_inbound(state: &Arc<allternit_api::AppState>, data: Value) -> (u16, Value) {
    let body = json!({"type": "message.inbound", "data": data});
    let bytes = body.to_string().into_bytes();
    let signature = sign_webhook("test-webhook-secret", &bytes);
    let secret = Arc::new(allternit_api::relay_auth::StaticRelaySecret { token: RELAY_TOKEN.into(), owner: RELAY_OWNER.into() });
    let app = agent_email_webhook_router_with(secret).with_state(state.clone());
    let mut request = axum::http::Request::post(INBOUND_PATH)
        .header("content-type", "application/json")
        .header("x-email-platform-signature", signature);
    for (k, v) in allternit_api::relay_auth::signed_headers(RELAY_TOKEN, RELAY_OWNER, "POST", INBOUND_PATH, &bytes) {
        request = request.header(k, v);
    }
    let response = app.oneshot(request.body(axum::body::Body::from(bytes)).unwrap()).await.unwrap();
    let status = response.status().as_u16();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// Poll until `sends` holds at least `n` entries; returns the full vec.
async fn wait_for_sends(recorded: &Arc<Recorded>, n: usize) -> Vec<Value> {
    for _ in 0..150 {
        let sends = recorded.sends.lock().unwrap().clone();
        if sends.len() >= n {
            return sends;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {n} mailflare sends; got {:?}", recorded.sends.lock().unwrap().clone());
}

/// Poll until the latest inbound row from `from` has a reply_status/guard set.
fn inbound_row(state: &Arc<allternit_api::AppState>, from: &str) -> Option<(Option<String>, Option<String>)> {
    let conn = state.db.connect().unwrap();
    conn.query_row(
        "SELECT guard_reason, reply_status FROM agent_email_inbound
         WHERE from_address = ?1 ORDER BY created_at DESC LIMIT 1",
        [from],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .ok()
}

async fn wait_for_row(state: &Arc<allternit_api::AppState>, from: &str) -> (Option<String>, Option<String>) {
    for _ in 0..100 {
        if let Some(row) = inbound_row(state, from) {
            if row.0.is_some() || row.1.is_some() {
                return row;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for the inbound row of {from} to settle");
}

#[tokio::test]
async fn inbound_turns_into_one_threaded_reply_and_guards_hold() {
    let recorded = Arc::new(Recorded::default());

    // Fake mailflare: records sends; approval-gated unless skipApproval is set.
    let mf_rec = recorded.clone();
    let gw_rec = recorded.clone();
    let mailflare = Router::new()
        .route(
            "/api/v1/send",
            apost(move |Json(body): Json<Value>| {
                let rec = mf_rec.clone();
                async move {
                    let skip = body
                        .get("skipApproval")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    rec.sends.lock().unwrap().push(body);
                    if skip {
                        Json(json!({"messageId": "mf-auto", "jobId": "job-auto", "status": "sent"}))
                    } else {
                        Json(json!({"messageId": "mf-1", "jobId": "job-1", "status": "pending_approval"}))
                    }
                }
            }),
        )
        .route(
            "/api/domains",
            aget(|| async { Json(json!({"domains": [{"id": "dom1", "hostname": "agents.test"}]})) }),
        );
    let mailflare_url = serve(mailflare).await;

    // Fake remote runtime: the placed bot's session lives here.
    let rt_rec = recorded.clone();
    let runtime = Router::new()
        .route(
            "/api/v1/agent-sessions",
            apost(move |Json(_body): Json<Value>| {
                let rec = rt_rec.clone();
                async move {
                    let n = {
                        let mut count = rec.session_creates.lock().unwrap();
                        *count += 1;
                        *count
                    };
                    // Session ids are unique per thread, like the real runtime's.
                    Json(json!({"id": format!("ses_remote{n}")}))
                }
            }),
        )
        .route(
            "/api/v1/agent-sessions/:id/messages",
            apost(
                move |AxumPath(id): AxumPath<String>, Json(body): Json<Value>| {
                    let rec = gw_rec.clone();
                    async move {
                        rec.turns.lock().unwrap().push(json!({"session": id, "body": body}));
                        Json(json!({"id": "m2", "role": "assistant", "content": "The bot's reply"}))
                    }
                },
            ),
        );
    let runtime_url = serve(runtime).await;

    std::env::set_var("ALLTERNIT_MAILFLARE_URL", &mailflare_url);
    std::env::set_var("ALLTERNIT_MAILFLARE_ADMIN_KEY", "ep_admin_test");
    std::env::set_var("ALLTERNIT_BOT_EMAIL_DOMAIN", "agents.test");
    std::env::set_var("ALLTERNIT_MAILFLARE_WEBHOOK_SECRET", "test-webhook-secret");

    let temp = tempfile::tempdir().unwrap();
    let state = app_state(temp.path()).await;
    {
        let conn = state.db.connect().unwrap();
        conn.execute(
            "INSERT INTO agents (id, user_id, name, model, provider, is_bot, config)
             VALUES ('bot-1', 'user-a', 'ledger', 'm', 'p', 1, '{}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_identity_channels
                 (id, agent_id, user_id, email_address, email_provider,
                  email_send_enabled, email_receive_enabled, email_mailbox_id, email_api_key_sealed)
             VALUES ('ch-1', 'bot-1', 'user-a', 'bot@agents.test', 'mailflare', 1, 1, 'mb-1', ?1)",
            [allternit_api::token_crypto::seal("ep_mailbox_key")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO remote_backend_targets (id, user_id, name, status, gateway_url, encrypted_gateway_token)
             VALUES ('tgt-1', 'user-a', 'Fake runtime', 'ready', ?1, ?2)",
            [runtime_url, allternit_api::token_crypto::seal("atok_1")],
        )
        .unwrap();
        conn.execute(
            "UPDATE agents SET config = json_set(COALESCE(config, '{}'), '$.placement', json_object('targetId', 'tgt-1')) WHERE id = 'bot-1'",
            [],
        )
        .unwrap();
    }

    // The route only accepts cloud-api's relay: no signature is 401, a valid
    // mailflare HMAC from a runtime owned by someone else is 403, and neither
    // reaches the agent.
    {
        let body = json!({"type": "message.inbound", "data": {"to": "bot@agents.test", "from": "eve@evil.test", "textBody": "hi", "messageId": "evt-x"}});
        let bytes = body.to_string().into_bytes();
        let hmac = sign_webhook("test-webhook-secret", &bytes);
        let secret = Arc::new(allternit_api::relay_auth::StaticRelaySecret { token: RELAY_TOKEN.into(), owner: "user-b".into() });
        let post = |signed_as: Option<&str>| {
            let app = agent_email_webhook_router_with(secret.clone()).with_state(state.clone());
            let (bytes, hmac) = (bytes.clone(), hmac.clone());
            let owner = signed_as.map(str::to_string);
            async move {
                let mut request = axum::http::Request::post(INBOUND_PATH).header("x-email-platform-signature", hmac);
                if let Some(owner) = owner {
                    for (k, v) in allternit_api::relay_auth::signed_headers(RELAY_TOKEN, &owner, "POST", INBOUND_PATH, &bytes) {
                        request = request.header(k, v);
                    }
                }
                app.oneshot(request.body(axum::body::Body::from(bytes)).unwrap()).await.unwrap().status().as_u16()
            }
        };
        assert_eq!(post(None).await, 401, "unsigned relay");
        assert_eq!(post(Some("user-b")).await, 403, "the agent belongs to user-a");
        assert!(inbound_row(&state, "eve@evil.test").is_none());
    }

    // Scenario 1 (mode approve, the default): full pipeline.
    let (status, body) = post_inbound(
        &state,
        json!({
            "to": "bot@agents.test",
            "from": "dana@acme.com",
            "subject": "H100 pricing",
            "textBody": "What's your H100 rate?\n\nOn Mon, Jan 1, 2024 at 9:00 AM Someone <a@b.c> wrote:\n\n> old stuff\n> more old",
            "messageId": "evt-in-1",
            "headers": {
                "from": "dana@acme.com",
                "to": "bot@agents.test",
                "subject": "H100 pricing",
                "messageId": "<in-1@acme.com>",
                "references": "<root@acme.com>",
            },
            "authResults": "dmarc=pass header.from=acme.com",
        }),
    )
    .await;
    assert_eq!(status, 200, "webhook rejected: {body}");
    assert_eq!(body["accepted"], true);

    let sends = wait_for_sends(&recorded, 1).await;
    let send = &sends[0];
    assert_eq!(send["from"], "bot@agents.test");
    assert_eq!(send["to"], "dana@acme.com");
    assert_eq!(send["subject"], "Re: H100 pricing");
    assert_eq!(send["mailboxId"], "mb-1");
    assert!(
        send.get("skipApproval").is_none(),
        "approval mode must not skip the gate: {send}"
    );
    assert_eq!(send["headers"]["In-Reply-To"], "<in-1@acme.com>");
    assert_eq!(send["headers"]["References"], "<root@acme.com> <in-1@acme.com>");
    assert_eq!(send["headers"]["Auto-Submitted"], "auto-replied");
    let reply_text = send["text"].as_str().unwrap();
    assert!(reply_text.contains("The bot's reply"), "reply text: {reply_text}");
    assert!(reply_text.contains("> What's your H100 rate?"), "quoted excerpt: {reply_text}");
    assert!(!reply_text.contains("old stuff"), "quoted excerpt must be the stripped body: {reply_text}");

    // The turn the bot saw is the de-quoted body.
    let turns = recorded.turns.lock().unwrap().clone();
    assert_eq!(turns.len(), 1, "one turn so far: {turns:?}");
    let turn_text = turns[0]["body"]["text"].as_str().unwrap();
    assert!(turn_text.contains("What's your H100 rate?"), "turn text: {turn_text}");
    assert!(!turn_text.contains("old stuff"), "turn text must be stripped: {turn_text}");

    // The reply is waiting on human approval.
    let row = wait_for_row(&state, "dana@acme.com").await;
    assert_eq!(row.0, None, "no guard fired");
    assert_eq!(row.1.as_deref(), Some("pending_approval"));
    let outbound: (String, Option<String>) = state
        .db
        .connect()
        .unwrap()
        .query_row(
            "SELECT status, reply_inbound_id FROM agent_email_outbound ORDER BY created_at DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(outbound.0, "pending_approval");
    assert!(outbound.1.is_some(), "outbound row marked as a reply");

    // Scenario 2 (loop guard): an auto-response gets no turn and no send.
    let (status, _body) = post_inbound(
        &state,
        json!({
            "to": "bot@agents.test",
            "from": "eve@acme.com",
            "subject": "Out of office",
            "textBody": "I am away",
            "headers": {"autoSubmitted": "auto-replied"},
        }),
    )
    .await;
    assert_eq!(status, 200);
    let row = wait_for_row(&state, "eve@acme.com").await;
    assert_eq!(row.0.as_deref(), Some("auto_submitted"));
    assert_eq!(row.1.as_deref(), Some("skipped"));
    assert_eq!(*recorded.session_creates.lock().unwrap(), 1, "guarded mail must not start a turn");
    assert_eq!(recorded.sends.lock().unwrap().len(), 1, "guarded mail must not be answered");

    // Scenario 3 (loop guard): DMARC fail is never answered.
    let (status, _body) = post_inbound(
        &state,
        json!({
            "to": "bot@agents.test",
            "from": "mallory@evil.com",
            "subject": "Wire me money",
            "textBody": "Urgent",
            "authResults": "dmarc=fail header.from=evil.com",
        }),
    )
    .await;
    assert_eq!(status, 200);
    let row = wait_for_row(&state, "mallory@evil.com").await;
    assert_eq!(row.0.as_deref(), Some("dmarc_fail"));
    assert_eq!(row.1.as_deref(), Some("skipped"));
    assert_eq!(*recorded.session_creates.lock().unwrap(), 1);
    assert_eq!(recorded.sends.lock().unwrap().len(), 1);

    // Scenario 4 (mode auto on a verified domain): direct send, admin key.
    {
        let conn = state.db.connect().unwrap();
        conn.execute(
            "UPDATE agent_identity_channels SET email_reply_mode = 'auto', email_domain_verified = 1 WHERE agent_id = 'bot-1'",
            [],
        )
        .unwrap();
    }
    let (status, _body) = post_inbound(
        &state,
        json!({
            "to": "bot@agents.test",
            "from": "frank@other.com",
            "subject": "Status?",
            "textBody": "How is it going?",
            "headers": {"messageId": "<in-4@other.com>"},
            "authResults": "dmarc=pass",
        }),
    )
    .await;
    assert_eq!(status, 200);
    let sends = wait_for_sends(&recorded, 2).await;
    let direct = &sends[1];
    assert_eq!(
        direct["skipApproval"],
        true,
        "auto mode on a verified domain sends through the admin skip-approval path: {direct}"
    );
    assert_eq!(direct["headers"]["In-Reply-To"], "<in-4@other.com>");
    let row = wait_for_row(&state, "frank@other.com").await;
    assert_eq!(row.0, None);
    assert_eq!(row.1.as_deref(), Some("sent"));

    // Scenario 5 (kill switch): the turn runs, the reply does not go out.
    {
        let conn = state.db.connect().unwrap();
        conn.execute(
            "UPDATE agent_identity_channels SET email_reply_enabled = 0 WHERE agent_id = 'bot-1'",
            [],
        )
        .unwrap();
    }
    let (status, _body) = post_inbound(
        &state,
        json!({
            "to": "bot@agents.test",
            "from": "grace@x.com",
            "subject": "Ping",
            "textBody": "ping",
            "authResults": "dmarc=pass",
        }),
    )
    .await;
    assert_eq!(status, 200);
    let row = wait_for_row(&state, "grace@x.com").await;
    assert_eq!(row.0, None);
    assert_eq!(row.1.as_deref(), Some("disabled"));
    assert_eq!(*recorded.session_creates.lock().unwrap(), 3, "the turn still runs under the kill switch");
    assert_eq!(recorded.sends.lock().unwrap().len(), 2, "the kill switch blocks the reply");
}
