//! Wire envelopes for the Allternit runtime pairing + relay protocols, with
//! golden-JSON round-trip tests (spike decision D5: protocol drift must fail
//! loudly on either side). Shapes are pinned against:
//! - `cmd/allternit-cloud-api/src/routes/runtime_pairing.rs` (server serde)
//! - `cmd/allternit-cloud-api/src/routes/runtime_relay.rs` (`CloudMessage`)
//! - `cmd/agent-daemon/src/index.ts` + `cmd/gizzi-code/.../pairing/pairing.ts`

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Pairing lifecycle (camelCase, per server `#[serde(rename_all = "camelCase")]`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePairingRequest {
    pub name: String,
    pub runtime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub public_key: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairingStart {
    pub pairing_id: String,
    pub device_code: String,
    pub user_code: String,
    pub challenge: String,
    pub verification_url: String,
    pub expires_at: String,
    pub poll_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExchangePairingRequest {
    pub pairing_id: String,
    pub device_code: String,
    pub signature: String,
}

/// 200 response of `POST /runtime-pairings/exchange` (and the shape returned
/// by `/rotate`). `token_type` is always "Bearer" today but kept as a field
/// for fidelity with `RuntimeSessionResponse`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeSessionResponse {
    pub runtime_id: String,
    pub user_id: String,
    pub user_email: String,
    #[serde(default)]
    pub organization_id: Option<String>,
    pub device_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    pub expires_at: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Exchange non-200 statuses (runtime_pairing.rs:517-537).
#[derive(Debug, Clone)]
pub(crate) enum ExchangeOutcome {
    /// 200 — token issued.
    Issued(RuntimeSessionResponse),
    /// 428 — keep polling.
    Pending,
    /// 429 — poll again after this many seconds (`retry_after` header).
    RetryAfter(u64),
    /// 410 — pairing expired; start over.
    Expired,
    /// 403 — denied by the approver.
    Denied,
}

// ---------------------------------------------------------------------------
// Relay protocol (snake_case `type`-tagged, per `CloudMessage`/`RuntimeMessage`)
// ---------------------------------------------------------------------------

/// Frames the node sends to the cloud relay (agent-daemon `index.ts`
/// `socket.send` sites).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum NodeMessage {
    Authenticate {
        runtime_id: String,
        device_token: String,
    },
    ResponseStart {
        request_id: String,
        status: u16,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
    },
    ResponseChunk {
        request_id: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        body_encoding: String,
    },
    ResponseEnd {
        request_id: String,
    },
    SocketReady {
        socket_id: String,
    },
    SocketData {
        socket_id: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        body_encoding: String,
    },
    SocketClose {
        socket_id: String,
        #[serde(default)]
        code: u16,
        #[serde(default)]
        reason: String,
    },
    Pong,
}

/// Frames the cloud relay sends to the node.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CloudMessage {
    Authenticated {
        runtime_id: String,
    },
    Request {
        request_id: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        body_encoding: String,
    },
    SocketOpen {
        socket_id: String,
        path: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
    SocketData {
        socket_id: String,
        #[serde(default)]
        body: String,
        #[serde(default)]
        body_encoding: String,
    },
    SocketClose {
        socket_id: String,
        #[serde(default)]
        code: u16,
        #[serde(default)]
        reason: String,
    },
    Ping,
    /// Any future/unknown frame is ignored, not fatal (mirrors the TS
    /// client's `else`-less dispatch).
    #[serde(other)]
    Unknown,
}

/// Extract a human-readable message from a cloud error JSON body
/// (`{ "error": …, "message": … }` or `{ "message": … }`).
pub(crate) fn cloud_error_message(body: &str, status: u16) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value.get("message").or_else(|| value.get("error")))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    message.unwrap_or_else(|| format!("request failed ({status})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticate_frame_matches_agent_daemon() {
        let frame = NodeMessage::Authenticate {
            runtime_id: "rt_abc123".into(),
            device_token: "allternit_runtime_xyz".into(),
        };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "authenticate",
                "runtime_id": "rt_abc123",
                "device_token": "allternit_runtime_xyz"
            })
        );
        // And it must NOT deserialize as any cloud frame.
        assert!(matches!(
            serde_json::from_value::<CloudMessage>(json).unwrap(),
            CloudMessage::Unknown
        ));
    }

    #[test]
    fn authenticated_and_ping_frames() {
        let msg: CloudMessage =
            serde_json::from_str(r#"{"type":"authenticated","runtime_id":"rt_abc123"}"#).unwrap();
        assert!(matches!(msg, CloudMessage::Authenticated { .. }));
        let ping: CloudMessage = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(ping, CloudMessage::Ping));
        assert_eq!(
            serde_json::to_value(NodeMessage::Pong).unwrap(),
            serde_json::json!({"type": "pong"})
        );
    }

    #[test]
    fn request_frame_golden() {
        let msg: CloudMessage = serde_json::from_str(
            r#"{
                "type": "request",
                "request_id": "req-1",
                "method": "GET",
                "path": "/v1/remote-control/sessions",
                "headers": {"accept": "application/json"},
                "body": "",
                "body_encoding": "utf8"
            }"#,
        )
        .unwrap();
        let CloudMessage::Request {
            request_id,
            method,
            path,
            headers,
            body,
            body_encoding,
        } = msg
        else {
            panic!("expected request");
        };
        assert_eq!(request_id, "req-1");
        assert_eq!(method, "GET");
        assert_eq!(path, "/v1/remote-control/sessions");
        assert_eq!(headers.get("accept").map(String::as_str), Some("application/json"));
        assert_eq!(body, "");
        assert_eq!(body_encoding, "utf8");
    }

    #[test]
    fn response_start_chunk_end_golden() {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        let start = NodeMessage::ResponseStart {
            request_id: "req-1".into(),
            status: 200,
            headers,
        };
        assert_eq!(
            serde_json::to_value(&start).unwrap(),
            serde_json::json!({
                "type": "response_start",
                "request_id": "req-1",
                "status": 200,
                "headers": {"content-type": "application/json"}
            })
        );
        let chunk = NodeMessage::ResponseChunk {
            request_id: "req-1".into(),
            body: "aGVsbG8=".into(),
            body_encoding: "base64".into(),
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            serde_json::json!({
                "type": "response_chunk",
                "request_id": "req-1",
                "body": "aGVsbG8=",
                "body_encoding": "base64"
            })
        );
        let end = NodeMessage::ResponseEnd {
            request_id: "req-1".into(),
        };
        assert_eq!(
            serde_json::to_value(&end).unwrap(),
            serde_json::json!({"type": "response_end", "request_id": "req-1"})
        );
    }

    #[test]
    fn socket_frames_golden() {
        let open: CloudMessage = serde_json::from_str(
            r#"{"type":"socket_open","socket_id":"s1","path":"/v1/remote-control/sessions/x/events","headers":{}}"#,
        )
        .unwrap();
        assert!(matches!(open, CloudMessage::SocketOpen { .. }));
        let ready = NodeMessage::SocketReady { socket_id: "s1".into() };
        assert_eq!(
            serde_json::to_value(&ready).unwrap(),
            serde_json::json!({"type": "socket_ready", "socket_id": "s1"})
        );
        let data = NodeMessage::SocketData {
            socket_id: "s1".into(),
            body: "hello".into(),
            body_encoding: "utf8".into(),
        };
        assert_eq!(
            serde_json::to_value(&data).unwrap(),
            serde_json::json!({
                "type": "socket_data",
                "socket_id": "s1",
                "body": "hello",
                "body_encoding": "utf8"
            })
        );
        let close = NodeMessage::SocketClose {
            socket_id: "s1".into(),
            code: 1012,
            reason: "Relay disconnected".into(),
        };
        assert_eq!(
            serde_json::to_value(&close).unwrap(),
            serde_json::json!({
                "type": "socket_close",
                "socket_id": "s1",
                "code": 1012,
                "reason": "Relay disconnected"
            })
        );
    }

    #[test]
    fn unknown_cloud_frame_is_unknown_not_fatal() {
        let msg: CloudMessage =
            serde_json::from_str(r#"{"type":"some_future_frame","a":1}"#).unwrap();
        assert!(matches!(msg, CloudMessage::Unknown));
    }

    #[test]
    fn create_pairing_request_is_camelcase() {
        let request = CreatePairingRequest {
            name: "macbook ao".into(),
            runtime_type: "ao".into(),
            hostname: Some("macbook".into()),
            platform: Some("macos-arm64".into()),
            version: Some("0.9.0".into()),
            public_key: "PUBKEY".into(),
            capabilities: vec!["runtime:connect".into()],
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({
                "name": "macbook ao",
                "runtimeType": "ao",
                "hostname": "macbook",
                "platform": "macos-arm64",
                "version": "0.9.0",
                "publicKey": "PUBKEY",
                "capabilities": ["runtime:connect"]
            })
        );
    }

    #[test]
    fn exchange_error_message_extraction() {
        assert_eq!(
            cloud_error_message(r#"{"error":"expired_token","message":"Pairing expired"}"#, 410),
            "Pairing expired"
        );
        assert_eq!(
            cloud_error_message(r#"{"error":"authorization_pending"}"#, 428),
            "authorization_pending"
        );
        assert_eq!(cloud_error_message("not json", 500), "request failed (500)");
        assert_eq!(cloud_error_message("", 403), "request failed (403)");
    }
}
