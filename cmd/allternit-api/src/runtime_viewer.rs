//! Live desktop (VNC) served by this runtime itself, for "Sign in on the cloud
//! computer" (vendor-account sign-in).
//!
//! cloud-api's `POST /api/v1/runtime-devices/:id/viewer-token` mints a viewer
//! token and a relay socket ticket; the browser's WebSocket is tunnelled
//! through this runtime's outbound relay connection to
//! `GET /api/v1/runtime-viewer/vnc?token=…` here, which proxies to the local
//! VNC server with the same RFB pump the Sessions computers use
//! (`computer_ws::pump_vnc`).
//!
//! Auth: the token is the Sessions ws-token format (HS256 `DT`) signed with the
//! per-runtime relay key `sha256_hex(device_token)` that this runtime already
//! holds ([`RelaySecret`]); no shared desktop secret. Accepted only when the
//! signature checks, it has not expired, `purpose` is `"vnc"`, and `user_id` is
//! the owner this runtime is paired as. Public mount (the browser has no Clerk
//! session on the tunnelled hop); the token is the credential. No device token
//! => 503, so it is inert on an unpaired runtime.
//!
//! The VNC server is `ALLTERNIT_RUNTIME_VNC_ADDR` (default `127.0.0.1:5900`).
//!
//! `GET /api/v1/runtime-viewer/vnc-check?token=…` is the automated self-check
//! behind cloud-api's `GET /api/v1/runtime-devices/:id/viewer-check`: same token
//! rules, but instead of upgrading it connects to the local VNC socket, reads the
//! RFB banner and answers `{ok, rfb, latencyMs}` (200) or `{ok:false, reason}`
//! (502). It opens no session and sends nothing to the VNC server.

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::relay_auth::{EnvOrFileRelaySecret, RelaySecret};

const VNC_ADDR_ENV: &str = "ALLTERNIT_RUNTIME_VNC_ADDR";
const DEFAULT_VNC_ADDR: &str = "127.0.0.1:5900";

#[derive(Deserialize)]
struct ViewerQuery {
    #[serde(default)]
    token: String,
}

/// Verify a viewer token against this runtime's relay key and paired owner.
/// Returns the effective `read_only` flag.
pub(crate) fn authorize_viewer(relay_key: &str, owner: &str, token: &str) -> Result<bool, &'static str> {
    let claims = crate::bot_desktop_stream::verify_desktop_token(relay_key, token).map_err(|_| "invalid token")?;
    if claims.purpose.as_deref() != Some("vnc") {
        return Err("invalid token");
    }
    if claims.user_id != owner {
        return Err("token mismatch");
    }
    Ok(claims.read_only)
}

/// What a reachable VNC socket said.
#[derive(Debug, PartialEq)]
pub(crate) struct VncProbe {
    pub rfb: String,
    pub latency_ms: u64,
}

/// Connect to `addr` and read the RFB protocol banner (`RFB 003.008\n`, 12 bytes)
/// the server sends first. `Err` is a short reason: `vnc_unreachable`,
/// `vnc_timeout` or `not_rfb`.
pub(crate) async fn probe_vnc(addr: &str, wait: std::time::Duration) -> Result<VncProbe, &'static str> {
    use tokio::io::AsyncReadExt;
    let started = std::time::Instant::now();
    let work = async {
        let mut stream = tokio::net::TcpStream::connect(addr).await.map_err(|_| "vnc_unreachable")?;
        let mut banner = [0u8; 12];
        stream.read_exact(&mut banner).await.map_err(|_| "not_rfb")?;
        Ok::<_, &'static str>(banner)
    };
    let banner = tokio::time::timeout(wait, work).await.map_err(|_| "vnc_timeout")??;
    let text = String::from_utf8_lossy(&banner).trim_end().to_string();
    if !text.starts_with("RFB ") {
        return Err("not_rfb");
    }
    Ok(VncProbe { rfb: text, latency_ms: started.elapsed().as_millis() as u64 })
}

async fn runtime_vnc_check_handler(Extension(secret): Extension<Arc<dyn RelaySecret>>, Query(query): Query<ViewerQuery>) -> Response {
    let Some((relay_key, owner)) = secret.credentials() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "runtime viewer not configured");
    };
    if let Err(why) = authorize_viewer(&relay_key, &owner, &query.token) {
        return fail(StatusCode::FORBIDDEN, why);
    }
    let addr = std::env::var(VNC_ADDR_ENV).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DEFAULT_VNC_ADDR.to_string());
    match probe_vnc(&addr, std::time::Duration::from_secs(5)).await {
        Ok(p) => Json(json!({ "ok": true, "rfb": p.rfb, "latencyMs": p.latency_ms })).into_response(),
        Err(reason) => (StatusCode::BAD_GATEWAY, Json(json!({ "ok": false, "reason": reason }))).into_response(),
    }
}

fn fail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

async fn runtime_vnc_ws_handler(
    ws: WebSocketUpgrade,
    Extension(secret): Extension<Arc<dyn RelaySecret>>,
    Query(query): Query<ViewerQuery>,
) -> Response {
    let Some((relay_key, owner)) = secret.credentials() else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "runtime viewer not configured");
    };
    let read_only = match authorize_viewer(&relay_key, &owner, &query.token) {
        Ok(read_only) => read_only,
        Err(why) => {
            warn!(why, "runtime viewer token rejected");
            return fail(StatusCode::FORBIDDEN, why);
        }
    };
    let addr = std::env::var(VNC_ADDR_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_VNC_ADDR.to_string());
    ws.on_upgrade(move |socket| async move {
        crate::computer_ws::pump_vnc(socket, "runtime", &addr, None, read_only).await;
    })
}

pub fn runtime_viewer_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    runtime_viewer_router_with(Arc::new(EnvOrFileRelaySecret::from_process_env()))
}

pub fn runtime_viewer_router_with<S: Clone + Send + Sync + 'static>(secret: Arc<dyn RelaySecret>) -> Router<S> {
    Router::new()
        .route("/api/v1/runtime-viewer/vnc", get(runtime_vnc_ws_handler))
        .route("/api/v1/runtime-viewer/vnc-check", get(runtime_vnc_check_handler))
        .layer(Extension(secret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_desktop_stream::sign_computer_token;
    use crate::relay_auth::{relay_key_from_device_token, StaticRelaySecret, UnconfiguredRelaySecret};
    use futures::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message;

    const TOKEN: &str = "allternit_runtime_test_token";
    const OWNER: &str = "user-a";

    fn key() -> String {
        relay_key_from_device_token(TOKEN)
    }

    fn token_for(key: &str, user: &str, purpose: &str, read_only: bool, ttl: u64) -> String {
        sign_computer_token(key, "rt_1", "rt_1", user, ttl, purpose, read_only)
    }

    #[test]
    fn owner_token_signed_with_the_relay_key_is_accepted() {
        let t = token_for(&key(), OWNER, "vnc", false, 60);
        assert_eq!(authorize_viewer(&key(), OWNER, &t), Ok(false));
        let ro = token_for(&key(), OWNER, "vnc", true, 60);
        assert_eq!(authorize_viewer(&key(), OWNER, &ro), Ok(true));
    }

    #[test]
    fn wrong_key_owner_purpose_or_expiry_is_rejected() {
        let good = token_for(&key(), OWNER, "vnc", false, 60);
        assert!(authorize_viewer("other-key", OWNER, &good).is_err());
        assert_eq!(authorize_viewer(&key(), "user-b", &good), Err("token mismatch"));
        let embed = token_for(&key(), OWNER, "embed", true, 60);
        assert!(authorize_viewer(&key(), OWNER, &embed).is_err());
        // The raw device token is not the key: cloud-api only has the digest.
        let raw = token_for(TOKEN, OWNER, "vnc", false, 60);
        assert!(authorize_viewer(&key(), OWNER, &raw).is_err());
        assert!(authorize_viewer(&key(), OWNER, "").is_err());
        assert!(authorize_viewer(&key(), OWNER, "a.b.c").is_err());
    }

    #[test]
    fn token_minted_like_cloud_api_verifies() {
        // cloud-api's `sign_viewer_token`: same header/claims, key = credential_hash.
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let exp = chrono::Utc::now().timestamp() as u64 + 60;
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"DT"}"#);
        let claims = json!({"bot_id":"","computer_id":"rt_1","purpose":"vnc","read_only":false,"sandbox_id":"rt_1","user_id":OWNER,"exp":exp});
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let input = format!("{header}.{payload}");
        let token = format!("{input}.{}", crate::bot_desktop_stream::hmac_sign(&key(), &input));
        assert_eq!(authorize_viewer(&key(), OWNER, &token), Ok(false));
    }

    async fn serve(secret: Arc<dyn RelaySecret>) -> std::net::SocketAddr {
        let app: Router = runtime_viewer_router_with(secret);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn tunnels_a_valid_token_to_the_vnc_server_and_refuses_the_rest() {
        // Fake VNC server: greets with an RFB banner.
        let vnc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        std::env::set_var(VNC_ADDR_ENV, vnc.local_addr().unwrap().to_string());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = vnc.accept().await.unwrap();
                tokio::spawn(async move {
                    s.write_all(b"RFB 003.008\n").await.unwrap();
                    let mut buf = [0u8; 12];
                    let _ = s.read_exact(&mut buf).await;
                });
            }
        });
        let addr = serve(Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() })).await;

        let good = token_for(&key(), OWNER, "vnc", false, 60);
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/runtime-viewer/vnc?token={good}"))
            .await
            .expect("valid token upgrades");
        match ws.next().await.unwrap().unwrap() {
            Message::Binary(b) => assert_eq!(&b[..], b"RFB 003.008\n"),
            other => panic!("expected the RFB banner, got {other:?}"),
        }
        ws.send(Message::Binary(b"RFB 003.008\n".to_vec())).await.unwrap();

        for bad in [
            String::new(),
            token_for("other-key", OWNER, "vnc", false, 60),
            token_for(&key(), "user-b", "vnc", false, 60),
        ] {
            let err = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/runtime-viewer/vnc?token={bad}"))
                .await
                .expect_err("bad token must not upgrade");
            match err {
                tokio_tungstenite::tungstenite::Error::Http(r) => assert_eq!(r.status(), 403),
                other => panic!("unexpected {other:?}"),
            }
        }
        std::env::remove_var(VNC_ADDR_ENV);
    }

    #[tokio::test]
    async fn unpaired_runtime_answers_503() {
        let addr = serve(Arc::new(UnconfiguredRelaySecret)).await;
        let err = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/runtime-viewer/vnc?token=x"))
            .await
            .expect_err("unpaired runtime refuses");
        match err {
            tokio_tungstenite::tungstenite::Error::Http(r) => assert_eq!(r.status(), 503),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_check_reads_the_rfb_banner_and_says_why_when_it_cannot() {
        let wait = std::time::Duration::from_millis(500);
        // A VNC server that greets.
        let vnc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = vnc.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = vnc.accept().await.unwrap();
                tokio::spawn(async move { let _ = s.write_all(b"RFB 003.008\n").await; });
            }
        });
        let ok = probe_vnc(&addr, wait).await.unwrap();
        assert_eq!(ok.rfb, "RFB 003.008");
        // Something else listening on the port: reachable but not VNC.
        let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let web_addr = web.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = web.accept().await.unwrap();
                tokio::spawn(async move { let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await; });
            }
        });
        assert_eq!(probe_vnc(&web_addr, wait).await, Err("not_rfb"));
        // A socket that accepts and says nothing.
        let mute = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mute_addr = mute.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let mut held = vec![];
            loop {
                held.push(mute.accept().await.unwrap());
            }
        });
        assert_eq!(probe_vnc(&mute_addr, std::time::Duration::from_millis(150)).await, Err("vnc_timeout"));
        // Nothing listening.
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_addr = closed.local_addr().unwrap().to_string();
        drop(closed);
        assert_eq!(probe_vnc(&closed_addr, wait).await, Err("vnc_unreachable"));
    }

    #[tokio::test]
    async fn the_check_route_wants_the_same_token_as_the_viewer() {
        let addr = serve(Arc::new(StaticRelaySecret { token: TOKEN.into(), owner: OWNER.into() })).await;
        let http = reqwest::Client::new();
        let status = |q: String| {
            let http = http.clone();
            async move { http.get(format!("http://{addr}/api/v1/runtime-viewer/vnc-check?token={q}")).send().await.unwrap().status().as_u16() }
        };
        assert_eq!(status(String::new()).await, 403);
        assert_eq!(status(token_for("other-key", OWNER, "vnc", false, 60)).await, 403);
        // A valid token reaches the probe: 200 with a VNC server, 502 without one (never 403).
        let good = token_for(&key(), OWNER, "vnc", true, 60);
        let unreachable = status(good).await;
        assert!(matches!(unreachable, 200 | 502), "{unreachable}");
        let unpaired = serve(Arc::new(UnconfiguredRelaySecret)).await;
        let r = http.get(format!("http://{unpaired}/api/v1/runtime-viewer/vnc-check?token=x")).send().await.unwrap();
        assert_eq!(r.status().as_u16(), 503);
    }
}
