//! Platform API live phone view (`POST /v1/phones/:serial/live`, `platform_v1::phones`).
//!
//! The live view is served by cloud-api and tunnelled to the owner's Desktop
//! over the runtime relay socket, the same path the web app's phone viewer
//! uses (`runtime_relay`): the Desktop's allternit-api answers
//! `/ws/phones/:serial/stream?token=…` with a view-only `phone_view` token it
//! minted (`POST /api/v1/phones/:serial/viewer-token`).
//!
//! * `GET /phone-live/:view` — a small public page (iframe-able; CSP
//!   `frame-ancestors` from `ALLTERNIT_EMBED_FRAME_ANCESTORS`, default `*`)
//!   that decodes the H.264 stream with WebCodecs. Its script is
//!   `GET /phone-live/viewer.js` (same origin, so `script-src 'self'` holds).
//! * `GET /phone-live/:view/socket` — the stream. Each connect mints a fresh
//!   30 s relay socket ticket for exactly that phone's stream path.
//!
//! The view id (24 random bytes) is the credential and lives 10 minutes, the
//! life of the token behind it; the runtime checks that token again on every
//! connect and drops any input. Views are kept in memory, like socket tickets.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use tokio::sync::Mutex;

use super::runtime_relay::{issue_socket_ticket, take_socket_ticket, upgrade_with_socket_ticket};
use crate::ApiState;

struct LiveView {
    owner: String,
    runtime_id: String,
    serial: String,
    token: String,
    expires_at: Instant,
}

fn views() -> &'static Mutex<HashMap<String, LiveView>> {
    static VIEWS: OnceLock<Mutex<HashMap<String, LiveView>>> = OnceLock::new();
    VIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember a live view; returns its id (the credential in the viewer URL).
pub(crate) async fn register(owner: &str, runtime_id: &str, serial: &str, token: &str, ttl: Duration) -> String {
    let id = hex::encode(rand::random::<[u8; 24]>());
    let mut map = views().lock().await;
    map.retain(|_, v| v.expires_at > Instant::now());
    map.insert(id.clone(), LiveView { owner: owner.into(), runtime_id: runtime_id.into(), serial: serial.into(), token: token.into(), expires_at: Instant::now() + ttl });
    id
}

/// `(owner, runtime, runtime stream path)` of a live view that hasn't expired.
pub(crate) async fn lookup(id: &str) -> Option<(String, String, String, String)> {
    let map = views().lock().await;
    let v = map.get(id).filter(|v| v.expires_at > Instant::now())?;
    let path = format!("/ws/phones/{}/stream?token={}", urlencoding::encode(&v.serial), urlencoding::encode(&v.token));
    Some((v.owner.clone(), v.runtime_id.clone(), v.serial.clone(), path))
}

/// The public base the viewer URLs use (`ALLTERNIT_PUBLIC_API_URL`).
pub(crate) fn public_base() -> String {
    std::env::var("ALLTERNIT_PUBLIC_API_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://api.allternit.com".to_string())
}

pub fn routes() -> Router<Arc<ApiState>> {
    Router::new()
        .route("/phone-live/viewer.js", get(viewer_js))
        .route("/phone-live/:view", get(page))
        .route("/phone-live/:view/socket", get(socket))
}

fn gone() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "live_view_expired", "message": "This live view has expired. Ask for a new one." }))).into_response()
}

fn attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;")
}

fn frame_ancestors() -> String {
    std::env::var("ALLTERNIT_EMBED_FRAME_ANCESTORS").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "*".to_string())
}

async fn page(Path(view): Path<String>) -> Response {
    let Some((_, _, serial, _)) = lookup(&view).await else { return gone() };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Phone live view</title>\
         <style>html,body{{margin:0;height:100%;background:#111;color:#ddd;font:14px system-ui,sans-serif}}\
         #wrap{{display:flex;align-items:center;justify-content:center;height:100%}}\
         canvas{{max-width:100%;max-height:100%}}#status{{position:fixed;left:12px;bottom:10px;opacity:.8}}</style>\
         </head><body><div id=\"wrap\"><canvas id=\"screen\" aria-label=\"Live view of the phone screen\"></canvas></div>\
         <div id=\"status\" role=\"status\">Connecting…</div>\
         <script src=\"/phone-live/viewer.js\" data-view=\"{}\" data-serial=\"{}\"></script></body></html>",
        attr(&view),
        attr(&serial)
    );
    let csp = format!(
        "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self' ws: wss:; frame-ancestors {}",
        frame_ancestors()
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::CONTENT_SECURITY_POLICY, csp),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
        html,
    )
        .into_response()
}

async fn socket(ws: WebSocketUpgrade, State(state): State<Arc<ApiState>>, Path(view): Path<String>) -> Response {
    let Some((owner, runtime_id, _, path)) = lookup(&view).await else { return gone() };
    let ticket = match issue_socket_ticket(&state, &owner, &runtime_id, path).await {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let Some(ticket) = take_socket_ticket(ticket["ticket"].as_str().unwrap_or_default()).await else { return gone() };
    match upgrade_with_socket_ticket(ws, &state, ticket).await {
        Ok(resp) => resp,
        Err(e) => e.into_response(),
    }
}

async fn viewer_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "public, max-age=300")], VIEWER_JS)
}

/// View-only scrcpy H.264 viewer (WebCodecs), the decoding of allternit-ai
/// `scrcpy-codec.ts` cut down to what a viewer needs. It sends only pings.
const VIEWER_JS: &str = r#"(function () {
  var me = document.currentScript;
  var view = me.dataset.view, name = me.dataset.serial;
  var canvas = document.getElementById('screen'), ctx = canvas.getContext('2d');
  var status = document.getElementById('status');
  function say(t) { status.textContent = t; }
  if (typeof VideoDecoder === 'undefined') { say('This browser cannot show the live view (no WebCodecs).'); return; }
  var sps = null, pps = null, decoder = null, pending = [], ts = 0;
  function units(b) {
    var marks = [], i = 0;
    while (i < b.length - 2) {
      if (b[i] === 0 && b[i + 1] === 0 && b[i + 2] === 0 && b[i + 3] === 1) { marks.push([i, i + 4]); i += 4; }
      else if (b[i] === 0 && b[i + 1] === 0 && b[i + 2] === 1) { marks.push([i, i + 3]); i += 3; }
      else i += 1;
    }
    return marks.map(function (m, k) { return b.subarray(m[1], k + 1 < marks.length ? marks[k + 1][0] : b.length); }).filter(function (u) { return u.length > 0; });
  }
  function avcc(b) {
    var us = units(b), total = 0;
    us.forEach(function (u) { total += 4 + u.length; });
    var out = new Uint8Array(total), dv = new DataView(out.buffer), o = 0;
    us.forEach(function (u) { dv.setUint32(o, u.length); out.set(u, o + 4); o += 4 + u.length; });
    return out;
  }
  function hex(n) { return n.toString(16).padStart(2, '0'); }
  function configure() {
    var d = new Uint8Array(11 + sps.length + pps.length), dv = new DataView(d.buffer);
    d[0] = 1; d[1] = sps[1]; d[2] = sps[2]; d[3] = sps[3]; d[4] = 0xff; d[5] = 0xe1;
    dv.setUint16(6, sps.length); d.set(sps, 8);
    var p = 8 + sps.length; d[p] = 1; dv.setUint16(p + 1, pps.length); d.set(pps, p + 3);
    if (decoder) { try { decoder.close(); } catch (e) {} }
    decoder = new VideoDecoder({
      output: function (f) { if (canvas.width !== f.displayWidth) { canvas.width = f.displayWidth; canvas.height = f.displayHeight; } ctx.drawImage(f, 0, 0); f.close(); },
      error: function (e) { say('Video error: ' + e.message); decoder = null; }
    });
    decoder.configure({ codec: 'avc1.' + hex(sps[1]) + hex(sps[2]) + hex(sps[3]), description: d, optimizeForLatency: true });
  }
  function decode(b, key) {
    if (!decoder || decoder.state !== 'configured') return;
    if (decoder.decodeQueueSize > 6 && !key) return;
    decoder.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: (ts++) * 16666, data: avcc(b) }));
  }
  function chunk(buf) {
    var b = new Uint8Array(buf);
    if (b[0] === 0xa0) return; // audio, not requested
    var idr = false, slice = false;
    units(b).forEach(function (u) { var t = u[0] & 0x1f; if (t === 7) sps = u.slice(); else if (t === 8) pps = u.slice(); else if (t === 5) idr = true; if (t >= 1 && t <= 5) slice = true; });
    if (idr && sps && pps && !decoder) { configure(); decode(b, true); pending.splice(0).forEach(function (p) { decode(p[0], p[1]); }); say(''); return; }
    if (!decoder) { if (slice) { if (pending.length >= 60) pending.shift(); pending.push([b, idr]); } return; }
    decode(b, idr);
  }
  var proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  var ws = new WebSocket(proto + '//' + location.host + '/phone-live/' + encodeURIComponent(view) + '/socket');
  ws.binaryType = 'arraybuffer';
  var ping = null;
  ws.onopen = function () { say('Waiting for the phone…'); ping = setInterval(function () { if (ws.readyState === 1) ws.send('{"type":"ping"}'); }, 20000); };
  ws.onmessage = function (ev) {
    if (typeof ev.data === 'string') {
      var f; try { f = JSON.parse(ev.data); } catch (e) { return; }
      if (f.type === 'meta') say('Live: ' + (f.deviceName || name));
      else if (f.type === 'end') say('The live view ended (' + (f.reason || 'closed') + ').');
      else if (f.type === 'status' && f.message) say(f.message);
      return;
    }
    if (ev.data instanceof ArrayBuffer) chunk(ev.data);
  };
  ws.onclose = function (ev) { clearInterval(ping); say(ev.code === 4401 ? 'This live view link expired. Ask for a new one.' : 'The live view ended.'); };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_live_view_names_one_phone_stream_and_expires() {
        let id = register("user-a", "rt-1", "link:7f3a", "tok/1", Duration::from_secs(600)).await;
        assert_eq!(id.len(), 48);
        let (owner, rt, _, path) = lookup(&id).await.unwrap();
        assert_eq!((owner.as_str(), rt.as_str(), path.as_str()), ("user-a", "rt-1", "/ws/phones/link%3A7f3a/stream?token=tok%2F1"));
        let old = register("user-a", "rt-1", "S", "t", Duration::from_millis(0)).await;
        assert!(lookup(&old).await.is_none(), "an expired view opens nothing");
        assert!(lookup("nope").await.is_none());
        let resp = page(Path(old)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let resp = page(Path(id)).await;
        let csp = resp.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().to_string();
        assert!(csp.contains("script-src 'self'") && csp.contains("frame-ancestors"));
    }
}
