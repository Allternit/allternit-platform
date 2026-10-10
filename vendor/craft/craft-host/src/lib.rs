mod protocol;

pub use protocol::{AppToHost, Envelope, HelloCaps, HostToApp, Theme, PROTOCOL};

/// How the app handled a `craft:command`.
pub enum CommandHandling {
    /// The command ran inline; the bridge replies with this result immediately.
    Finished(Result<serde_json::Value, String>),
    /// The adapter queued the command for execution on its own thread/pump (engines that
    /// must run commands between frames rather than inside the postMessage event). The
    /// adapter posts the `CommandResult` itself, when the engine answers, via
    /// [`wasm::BridgeTransport::post`].
    Deferred,
}

/// App-implemented hooks. The adapter wires these to the engine; the bridge owns
/// validation, framing, and postMessage.
pub trait HostHooks {
    fn app_id(&self) -> &'static str;
    fn version(&self) -> String;
    /// Replace the current document. Return warnings (e.g. dropped layers).
    fn open(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<String>, String>;
    /// Run one engine command (the agent lane). `params` is the raw registry shape.
    fn command(&mut self, cmd: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
        let _ = (cmd, params);
        Err("this app does not support the command channel".into())
    }
    /// How to handle one `craft:command {id, cmd, params}`. The default runs [`Self::command`]
    /// inline. Engines with an async execution model override this, queue the work, and
    /// return [`CommandHandling::Deferred`]; they own posting the `CommandResult` for `id`
    /// once the engine answers.
    fn handle_command(&mut self, _id: u64, cmd: &str, params: serde_json::Value) -> CommandHandling {
        CommandHandling::Finished(self.command(cmd, params))
    }
    /// The app wants to save. The bridge forwards bytes to the host as `craft:save-request`;
    /// the host persists and then MUST reply `craft:save-ack` (delivered here as
    /// [`HostToApp::SaveAck`]).
    fn take_save(&mut self) -> Option<SavePayload>;
    fn set_theme(&mut self, theme: &Theme);
    fn dirty(&self) -> bool;
}

pub struct SavePayload {
    pub name: String,
    pub format: Option<String>,
    pub bytes: Vec<u8>,
    pub meta: serde_json::Value,
}

pub struct EmbedConfig {
    pub parent_origin: String,
}

/// Parse `?embed=1&origin=<urlencoded>` from the current page URL. Returns `None` when
/// not embedded (plain standalone visit).
#[cfg(target_arch = "wasm32")]
pub fn embed_config_from_location() -> Option<EmbedConfig> {
    let win = web_sys::window()?;
    let search = win.location().search().ok()?;
    if !search.contains("embed=1") {
        return None;
    }
    for pair in search[1..].split('&') {
        if let Some(v) = pair.strip_prefix("origin=") {
            let decoded = js_sys::decode_uri_component(v).ok()?.into();
            return Some(EmbedConfig {
                parent_origin: decoded,
            });
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn embed_config_from_location() -> Option<EmbedConfig> {
    None
}

/// The embedded-session state machine. Transport lives in [`wasm`] behind cfg; the state
/// machine itself is platform-independent and unit-tested on host.
pub struct HostBridge {
    pub config: EmbedConfig,
    state: State,
    token: Option<String>,
    next_save_seq: u64,
    last_save_ack: Option<bool>,
}

#[derive(PartialEq, Debug)]
enum State {
    AwaitingHello,
    Ready,
    Failed(String),
}

impl HostBridge {
    pub fn new(config: EmbedConfig) -> Self {
        Self {
            config,
            state: State::AwaitingHello,
            token: None,
            next_save_seq: 1,
            last_save_ack: None,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.state == State::Ready
    }

    pub fn failed(&self) -> Option<&str> {
        match &self.state {
            State::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// Handle a validated message (origin already checked by the transport).
    /// Returns messages to send back to the host, with optional byte payloads.
    pub fn handle_message(
        &mut self,
        body: &str,
        bytes: Option<Vec<u8>>,
        hooks: &mut dyn HostHooks,
    ) -> Vec<(AppToHost, Option<Vec<u8>>)> {
        let (msg, has_bytes) = match Envelope::unpack(body) {
            Ok(v) => v,
            Err(_) => return vec![],
        };
        let host_msg: HostToApp = match serde_json::from_value(msg) {
            Ok(m) => m,
            Err(_) => return vec![],
        };
        if has_bytes && bytes.is_none() {
            return vec![(
                AppToHost::HelloAck {
                    ok: false,
                    error: Some("missing byte payload".into()),
                },
                None,
            )];
        }
        match host_msg {
            HostToApp::Hello {
                protocol,
                token,
                theme,
                chrome: _,
                capabilities: _,
            } => {
                if protocol != PROTOCOL {
                    self.state = State::Failed(format!("protocol {protocol} not supported"));
                    return vec![(
                        AppToHost::HelloAck {
                            ok: false,
                            error: Some(format!("protocol {protocol} not supported")),
                        },
                        None,
                    )];
                }
                self.token = Some(token);
                self.state = State::Ready;
                hooks.set_theme(&theme);
                vec![(AppToHost::HelloAck { ok: true, error: None }, None)]
            }
            HostToApp::Open {
                name,
                format: _,
                bytes: _,
            } => {
                if !self.authed() {
                    return vec![self.reject_auth("open")];
                }
                let payload = bytes.unwrap_or_default();
                let ack = match hooks.open(&name, &payload) {
                    Ok(warnings) => AppToHost::OpenAck {
                        ok: true,
                        error: None,
                        warnings,
                    },
                    Err(e) => AppToHost::OpenAck {
                        ok: false,
                        error: Some(e),
                        warnings: Vec::new(),
                    },
                };
                vec![(ack, None)]
            }
            HostToApp::Command { id, cmd, params } => {
                if !self.authed() {
                    return vec![self.reject_auth("command")];
                }
                match hooks.handle_command(id, &cmd, params) {
                    CommandHandling::Finished(res) => vec![(
                        AppToHost::CommandResult {
                            id,
                            ok: res.is_ok(),
                            result: res.as_ref().ok().cloned(),
                            error: res.err(),
                        },
                        None,
                    )],
                    // The adapter runs the command on its own pump and posts the
                    // CommandResult when the engine answers.
                    CommandHandling::Deferred => vec![],
                }
            }
            HostToApp::SaveAck { ok, error } => {
                if !ok {
                    log::warn!("craft:1 host failed to persist a save: {}", error.as_deref().unwrap_or("no reason given"));
                }
                self.save_acked(ok, error);
                vec![]
            }
            HostToApp::Theme { theme } => {
                if self.authed() {
                    hooks.set_theme(&theme);
                }
                vec![]
            }
            HostToApp::Ping {} => vec![],
        }
    }

    /// Poll for an app-initiated save (adapter calls from its frame pump).
    pub fn poll_save(&mut self, hooks: &mut dyn HostHooks) -> Option<(u64, SavePayload)> {
        if !self.is_ready() {
            return None;
        }
        hooks.take_save().map(|p| {
            let seq = self.next_save_seq;
            self.next_save_seq += 1;
            (seq, p)
        })
    }

    /// Record the host's reply to a `craft:save-request` (called by the transport when a
    /// `craft:save-ack` arrives). Exposed via [`HostBridge::last_save_ack`].
    pub fn save_acked(&mut self, ok: bool, _error: Option<String>) {
        self.last_save_ack = Some(ok);
    }

    pub fn last_save_ack(&self) -> Option<bool> {
        self.last_save_ack
    }

    fn authed(&self) -> bool {
        self.state == State::Ready && self.token.is_some()
    }

    fn reject_auth(&self, what: &str) -> (AppToHost, Option<Vec<u8>>) {
        (
            AppToHost::CommandResult {
                id: 0,
                ok: false,
                result: None,
                error: Some(format!("{what} rejected: no valid craft:1 session")),
            },
            None,
        )
    }
}

#[cfg(target_arch = "wasm32")]
pub mod wasm {
    use super::{AppToHost, EmbedConfig, Envelope, HostBridge, HostHooks, SavePayload};
    use std::cell::RefCell;
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;

    /// postMessage transport + frame pump. Construct once at web-app startup when
    /// [`super::embed_config_from_location`] returns `Some`.
    pub struct BridgeTransport {
        bridge: Rc<RefCell<HostBridge>>,
        window: web_sys::Window,
    }

    impl BridgeTransport {
        pub fn start(
            config: EmbedConfig,
            hooks: Rc<RefCell<dyn HostHooks>>,
        ) -> Result<Self, JsValue> {
            let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
            let bridge = Rc::new(RefCell::new(HostBridge::new(config)));
            let parent = window
                .parent()
                .ok()
                .flatten()
                .ok_or_else(|| JsValue::from_str("no parent window"))?;

            let cb = {
                let bridge = bridge.clone();
                let hooks = hooks.clone();
                let parent = parent.clone();
                Closure::<dyn FnMut(_)>::new(move |e: web_sys::MessageEvent| {
                    let cfg_origin = bridge.borrow().config.parent_origin.clone();
                    if e.origin() != cfg_origin {
                        return;
                    }
                    let body = match e.data().as_string() {
                        Some(s) => s,
                        None => return,
                    };
                    let bytes = e.data().dyn_into::<js_sys::Array>().ok().and_then(|a| {
                        a.get(1)
                            .dyn_into::<js_sys::ArrayBuffer>()
                            .ok()
                            .map(|b| js_sys::Uint8Array::new(&b).to_vec())
                    });
                    let replies = bridge
                        .borrow_mut()
                        .handle_message(&body, bytes, &mut *hooks.borrow_mut());
                    for (msg, payload) in replies {
                        send(&parent, &cfg_origin, &msg, payload.as_deref());
                    }
                })
            };
            window.add_event_listener_with_callback("message", cb.as_ref().unchecked_ref())?;
            cb.forget();

            let transport = Self { bridge, window };
            transport.pump_ready(&hooks, &parent);
            Ok(transport)
        }

        fn pump_ready(&self, hooks: &Rc<RefCell<dyn HostHooks>>, parent: &web_sys::Window) {
            let (app, version) = {
                let h = hooks.borrow();
                (h.app_id().to_string(), h.version())
            };
            let msg = AppToHost::Ready {
                app,
                version,
                protocol: super::PROTOCOL.into(),
            };
            let origin = self.bridge.borrow().config.parent_origin.clone();
            send(parent, &origin, &msg, None);
        }

        /// Call every frame: forwards app-initiated saves and dirty flag changes.
        pub fn pump(&self, hooks: &Rc<RefCell<dyn HostHooks>>) {
            let origin = self.bridge.borrow().config.parent_origin.clone();
            let parent = match self.window.parent().ok().flatten() {
                Some(p) => p,
                None => return,
            };
            let save = {
                let mut h = hooks.borrow_mut();
                self.bridge.borrow_mut().poll_save(&mut *h)
            };
            if let Some((_, SavePayload {
                name,
                format,
                bytes,
                meta,
            })) = save
            {
                let msg = AppToHost::SaveRequest {
                    name,
                    format,
                    meta,
                    bytes: Vec::new(),
                };
                send(&parent, &origin, &msg, Some(&bytes));
            }
            let _ = self.bridge.borrow().is_ready();
        }

        /// Post an app→host message outside the `handle_message` reply flow: async command
        /// results ([`CommandHandling::Deferred`]), engine events, document-changed pings.
        pub fn post(&self, msg: &AppToHost, bytes: Option<&[u8]>) {
            let origin = self.bridge.borrow().config.parent_origin.clone();
            let Some(parent) = self.window.parent().ok().flatten() else {
                return;
            };
            send(&parent, &origin, msg, bytes);
        }
    }

    fn send(parent: &web_sys::Window, origin: &str, msg: &AppToHost, bytes: Option<&[u8]>) {
        let Ok(body) = Envelope::pack(msg, bytes.is_some()) else {
            return;
        };
        if let Some(b) = bytes {
            let buf = js_sys::Uint8Array::from(b).buffer();
            let arr = js_sys::Array::new();
            arr.push(&JsValue::from_str(&body));
            arr.push(&buf);
            let _ = parent.post_message_with_transfer(arr.as_ref(), origin, arr.as_ref());
        } else {
            let _ = parent.post_message(&JsValue::from_str(&body), origin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Noop;
    impl HostHooks for Noop {
        fn app_id(&self) -> &'static str {
            "test"
        }
        fn version(&self) -> String {
            "0".into()
        }
        fn open(&mut self, _: &str, _: &[u8]) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn command(
            &mut self,
            _: &str,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({}))
        }
        fn take_save(&mut self) -> Option<SavePayload> {
            None
        }
        fn set_theme(&mut self, _: &Theme) {}
        fn dirty(&self) -> bool {
            false
        }
    }

    fn hello(bridge: &mut HostBridge, hooks: &mut dyn HostHooks, token: &str) {
        let body = Envelope::pack(
            &HostToApp::Hello {
                protocol: PROTOCOL.into(),
                token: token.into(),
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
            },
            false,
        )
        .unwrap();
        let replies = bridge.handle_message(&body, None, hooks);
        assert!(matches!(&replies[0].0, AppToHost::HelloAck { ok: true, .. }));
    }

    #[test]
    fn rejects_commands_before_hello() {
        let mut b = HostBridge::new(EmbedConfig {
            parent_origin: "https://ai.allternit.com".into(),
        });
        let mut h = Noop;
        let body = Envelope::pack(
            &HostToApp::Command {
                id: 1,
                cmd: "file.save".into(),
                params: serde_json::json!({}),
            },
            false,
        )
        .unwrap();
        let replies = b.handle_message(&body, None, &mut h);
        assert!(matches!(&replies[0].0, AppToHost::CommandResult { ok: false, .. }));
        assert!(!b.is_ready());
    }

    #[test]
    fn rejects_wrong_protocol() {
        let mut b = HostBridge::new(EmbedConfig {
            parent_origin: "x".into(),
        });
        let mut h = Noop;
        let body = Envelope::pack(
            &HostToApp::Hello {
                protocol: "craft:0".into(),
                token: "t".into(),
                theme: Theme {
                    dark: false,
                    accent: None,
                    scale: None,
                },
                chrome: "hidden".into(),
                capabilities: HelloCaps {
                    save: true,
                    command: true,
                },
            },
            false,
        )
        .unwrap();
        let replies = b.handle_message(&body, None, &mut h);
        assert!(matches!(
            &replies[0].0,
            AppToHost::HelloAck { ok: false, .. }
        ));
        assert!(b.failed().is_some());
    }

    #[test]
    fn open_after_hello() {
        let mut b = HostBridge::new(EmbedConfig {
            parent_origin: "x".into(),
        });
        let mut h = Noop;
        hello(&mut b, &mut h, "tok");
        assert!(b.is_ready());
        let body = Envelope::pack(
            &HostToApp::Open {
                name: "doc.pdf".into(),
                format: Some("application/pdf".into()),
                bytes: Vec::new(),
            },
            true,
        )
        .unwrap();
        let replies = b.handle_message(&body, Some(vec![1, 2, 3]), &mut h);
        assert!(matches!(&replies[0].0, AppToHost::OpenAck { ok: true, .. }));
    }

    #[test]
    fn deferred_command_sends_no_immediate_reply() {
        struct AsyncHooks;
        impl HostHooks for AsyncHooks {
            fn app_id(&self) -> &'static str {
                "test"
            }
            fn version(&self) -> String {
                "0".into()
            }
            fn open(&mut self, _: &str, _: &[u8]) -> Result<Vec<String>, String> {
                Ok(Vec::new())
            }
            fn take_save(&mut self) -> Option<SavePayload> {
                None
            }
            fn set_theme(&mut self, _: &Theme) {}
            fn dirty(&self) -> bool {
                false
            }
            fn handle_command(&mut self, _id: u64, _cmd: &str, _params: serde_json::Value) -> CommandHandling {
                CommandHandling::Deferred
            }
        }
        let mut b = HostBridge::new(EmbedConfig {
            parent_origin: "x".into(),
        });
        let mut h = AsyncHooks;
        hello(&mut b, &mut h, "tok");
        let body = Envelope::pack(
            &HostToApp::Command {
                id: 42,
                cmd: "filter.blur".into(),
                params: serde_json::json!({}),
            },
            false,
        )
        .unwrap();
        let replies = b.handle_message(&body, None, &mut h);
        assert!(replies.is_empty(), "a deferred command posts its own CommandResult later");
    }
}
