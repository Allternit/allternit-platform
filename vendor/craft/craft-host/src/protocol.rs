use serde::{Deserialize, Serialize};

pub const PROTOCOL: &str = "craft:1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Theme {
    pub dark: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HelloCaps {
    #[serde(default = "bool_true")]
    pub save: bool,
    #[serde(default = "bool_true")]
    pub command: bool,
}

const fn bool_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HostToApp {
    Hello {
        protocol: String,
        token: String,
        theme: Theme,
        #[serde(default = "default_chrome")]
        chrome: String,
        #[serde(default)]
        capabilities: HelloCaps,
    },
    Open {
        name: String,
        format: Option<String>,
        #[serde(skip)]
        bytes: Vec<u8>,
    },
    Command {
        id: u64,
        cmd: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    /// The host's reply to a `craft:save-request` (PROTOCOL: the host persists, then MUST
    /// reply with `craft:save-ack`).
    SaveAck {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Theme { theme: Theme },
    Ping {},
}

fn default_chrome() -> String {
    "hidden".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AppToHost {
    Ready {
        app: String,
        version: String,
        protocol: String,
    },
    HelloAck {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    OpenAck {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
    },
    DocumentChanged { dirty: bool },
    SaveRequest {
        name: String,
        format: Option<String>,
        #[serde(skip)]
        bytes: Vec<u8>,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        meta: serde_json::Value,
    },
    /// Unrecoverable editor failure (e.g. a fatal wasm panic): the host shows an error
    /// surface; the document is not trustworthy from here on.
    Error {
        message: String,
    },
    CommandResult {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    CommandEvent {
        event: String,
        #[serde(default)]
        data: serde_json::Value,
    },
}

/// Wire envelope: the JSON `postMessage` body. Byte payloads travel beside the JSON as a
/// transferred `ArrayBuffer` in the second postMessage argument, never base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub msg: serde_json::Value,
    pub has_bytes: bool,
}

impl Envelope {
    pub fn pack<T: Serialize>(msg: &T, has_bytes: bool) -> serde_json::Result<String> {
        serde_json::to_string(&Envelope {
            msg: serde_json::to_value(msg)?,
            has_bytes,
        })
    }

    pub fn unpack(body: &str) -> serde_json::Result<(serde_json::Value, bool)> {
        let env: Envelope = serde_json::from_str(body)?;
        Ok((env.msg, env.has_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trip() {
        let h = HostToApp::Hello {
            protocol: PROTOCOL.into(),
            token: "tok".into(),
            theme: Theme {
                dark: true,
                accent: None,
                scale: None,
            },
            chrome: "hidden".into(),
            capabilities: HelloCaps {
                save: true,
                command: true,
            },
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["type"], "hello");
        let back: HostToApp = serde_json::from_value(v).unwrap();
        assert!(matches!(back, HostToApp::Hello { .. }));
    }

    #[test]
    fn envelope_round_trip() {
        let s = AppToHost::Ready {
            app: "image".into(),
            version: "0.1.0".into(),
            protocol: PROTOCOL.into(),
        };
        let body = Envelope::pack(&s, false).unwrap();
        let (msg, has_bytes) = Envelope::unpack(&body).unwrap();
        assert!(!has_bytes);
        let back: AppToHost = serde_json::from_value(msg).unwrap();
        assert!(matches!(back, AppToHost::Ready { .. }));
    }

    #[test]
    fn command_result_omits_absent_fields() {
        let r = AppToHost::CommandResult {
            id: 7,
            ok: false,
            result: None,
            error: Some("nope".into()),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("result").is_none());
        assert_eq!(v["type"], "command-result");
    }

    #[test]
    fn save_ack_round_trip() {
        let body = Envelope::pack(
            &HostToApp::SaveAck {
                ok: false,
                error: Some("artifact storage failed".into()),
            },
            false,
        )
        .unwrap();
        let (msg, has_bytes) = Envelope::unpack(&body).unwrap();
        assert!(!has_bytes);
        let back: HostToApp = serde_json::from_value(msg).unwrap();
        assert!(matches!(back, HostToApp::SaveAck { ok: false, .. }));
    }
}
