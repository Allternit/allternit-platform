//! The craft:1 host-page embed adapter (protocol: `vendor/craft/craft-host/PROTOCOL.md`).
//!
//! When the page was loaded as `?embed=1&origin=<parent-origin>`, the app speaks the craft:1
//! postMessage protocol with the host page instead of behaving like a standalone visit:
//!
//! - `craft:open` bytes land in the same [`Inbox`] the file picker and drag-and-drop feed, so
//!   a host-seeded document takes exactly the app's normal open path.
//! - `craft:command` maps onto the app's existing control channel
//!   ([`photocraft_ui_egui::control`]) — the same `ControlRequest` mpsc channel the desktop TCP
//!   control server feeds — so the host gets the full engine registry plus `ui.*`/`app.*`
//!   parity. Commands are picked up from the postMessage event, queued, and handed to the
//!   channel between frames; the reply arrives asynchronously and is posted as
//!   `craft:command-result`.
//! - Saves: the platform `Services::write` path (native `.pcraft` saves and flat PSD/PNG/…
//!   exports all go through it) queues a [`SavePayload`] for the bridge instead of triggering
//!   the browser download, which is suppressed unless `?download=1`.
//! - `craft:theme` maps onto the app's dark/light `ThemeKind`s (applied on the next frame).
//!
//! One command runs at a time (PROTOCOL's rate limit): the next is handed to the control
//! channel only after the previous one replied.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};

use craft_host::wasm::BridgeTransport;
use craft_host::{AppToHost, CommandHandling, HostHooks, SavePayload, Theme};
use photocraft_ui_egui::control::{self, ControlRequest, ControlResponse};
use photocraft_ui_egui::theme::ThemeKind;
use photocraft_ui_egui::{PhotocraftApp, Services};
use serde_json::{json, Value};

use crate::web::Inbox;

/// Shared adapter state: the hooks (postMessage thread) and the frame pump (UI thread) both
/// hold a clone.
#[derive(Clone)]
pub struct EmbedState {
    inner: Arc<Mutex<EmbedInner>>,
    /// `?download=1`: keep the browser download alongside the host write-back.
    download: bool,
}

struct EmbedInner {
    /// Saves produced by the platform `write` service, waiting for the bridge pump.
    saves: VecDeque<SavePayload>,
    /// `craft:command` messages waiting for a frame (one runs at a time).
    commands: VecDeque<(u64, String, Value)>,
    /// Theme from `craft:hello`/`craft:theme`, applied by the next frame.
    theme: Option<Theme>,
    /// A valid `craft:hello` arrived. `set_theme` is the bridge's only path here and it runs
    /// only for an authenticated hello/theme, so it doubles as the session-established flag:
    /// queued saves would never reach a host that never said hello.
    ready: bool,
    /// The dirty flag as last observed by the frame pump (`HostHooks::dirty` reads it).
    dirty: bool,
}

impl EmbedState {
    fn new(download: bool) -> Self {
        EmbedState {
            inner: Arc::new(Mutex::new(EmbedInner {
                saves: VecDeque::new(),
                commands: VecDeque::new(),
                theme: None,
                ready: false,
                dirty: false,
            })),
            download,
        }
    }

    fn inner(&self) -> MutexGuard<'_, EmbedInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queue a save the platform `write` service produced. Returns false (queueing nothing)
    /// when no craft:1 session is established, so the caller keeps the browser download — a
    /// queued payload would otherwise never leave the page.
    fn queue_save(&self, path: &str, bytes: &[u8]) -> bool {
        let mut inner = self.inner();
        if !inner.ready {
            return false;
        }
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document".into());
        let format = std::path::Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        inner.saves.push_back(SavePayload {
            name,
            format,
            bytes: bytes.to_vec(),
            meta: json!({ "path": path }),
        });
        true
    }
}

/// The bridge's view of the app: queues work for the frame pump, which owns the
/// `PhotocraftApp` (the postMessage event cannot touch it — the UI thread is mid-frame or
/// idle, never re-entrant).
struct EmbedHooks {
    state: EmbedState,
    inbox: Inbox,
}

impl HostHooks for EmbedHooks {
    fn app_id(&self) -> &'static str {
        "image"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn open(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
        // The same intake as the file picker and drops: the frame loop drains the inbox and
        // imports the document, so parse warnings and errors surface as the usual in-app
        // notices. The ack therefore confirms intake; the document state right after is
        // visible to the host through commands (ui.inspect).
        self.inbox
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((name.to_string(), bytes.to_vec()));
        Ok(Vec::new())
    }

    fn handle_command(&mut self, id: u64, cmd: &str, params: Value) -> CommandHandling {
        self.state.inner().commands.push_back((id, cmd.to_string(), params));
        CommandHandling::Deferred
    }

    fn take_save(&mut self) -> Option<SavePayload> {
        self.state.inner().saves.pop_front()
    }

    fn set_theme(&mut self, theme: &Theme) {
        let mut inner = self.state.inner();
        inner.ready = true;
        inner.theme = Some(theme.clone());
    }

    fn dirty(&self) -> bool {
        self.state.inner().dirty
    }
}

/// Map a `craft:command` name onto the control channel. Names from [`control::METHODS`] are
/// control methods; anything else is an engine/UI command id, run the way `engine.execute`
/// runs it (direct registry dispatch, UI-command table as fallback).
fn route_command(cmd: &str, params: Value) -> (String, Value) {
    if control::METHODS.contains(&cmd) {
        (cmd.to_string(), params)
    } else {
        ("engine.execute".to_string(), json!({ "command": cmd, "params": params }))
    }
}

/// The embedded session: bridge transport + the queues between the postMessage thread and
/// the frame pump. Owned by `WebShell`; `None` on a standalone visit.
pub struct Embed {
    state: EmbedState,
    hooks: Rc<RefCell<dyn HostHooks>>,
    transport: BridgeTransport,
    control_tx: Sender<ControlRequest>,
    control_rx: Option<Receiver<ControlRequest>>,
    /// Command ids in flight, with their control-channel reply receivers.
    pending: Vec<(u64, Receiver<ControlResponse>)>,
    last_dirty: Option<bool>,
}

impl Embed {
    /// Start the bridge when the page was loaded embedded (`?embed=1&origin=…`). Returns None
    /// on a standalone visit, or when the bridge cannot start (the app keeps its normal UI,
    /// like a standalone visit).
    pub fn start(inbox: &Inbox) -> Option<Embed> {
        let config = craft_host::embed_config_from_location()?;
        let download = crate::web::query().contains("download=1");
        let state = EmbedState::new(download);
        let hooks: Rc<RefCell<EmbedHooks>> = Rc::new(RefCell::new(EmbedHooks {
            state: state.clone(),
            inbox: inbox.clone(),
        }));
        let transport = match BridgeTransport::start(config, hooks.clone()) {
            Ok(t) => t,
            Err(e) => {
                log::error!("craft:1 bridge could not start: {e:?}");
                return None;
            }
        };
        let hooks: Rc<RefCell<dyn HostHooks>> = hooks;
        let (control_tx, control_rx) = channel();
        Some(Embed {
            state,
            hooks,
            transport,
            control_tx,
            control_rx: Some(control_rx),
            pending: Vec::new(),
            last_dirty: None,
        })
    }

    /// Route the platform save/export write path into the bridge. The browser download stays
    /// for anything the bridge cannot deliver to the host (no session established) and is kept
    /// alongside the write-back entirely with `?download=1`.
    pub fn hook_saves(&self, services: &mut Services) {
        let state = self.state.clone();
        let download = self.state.download;
        services.write = Some(Box::new(move |path: &str, bytes: &[u8]| {
            let queued = state.queue_save(path, bytes);
            if !queued || download {
                return crate::web::download(path, bytes);
            }
            Ok(())
        }));
    }

    /// Attach the control channel to the app (once, before the first frame).
    pub fn attach(&mut self, app: PhotocraftApp) -> PhotocraftApp {
        match self.control_rx.take() {
            Some(rx) => app.with_control(rx),
            None => app,
        }
    }

    /// Before the frame: apply a pending theme, hand one queued command to the control
    /// channel (only once the previous one replied — one in flight per session).
    pub fn pre_frame(&mut self, app: &mut PhotocraftApp, ctx: &egui::Context) {
        let theme = self.state.inner().theme.take();
        if let Some(theme) = theme {
            // The app's themes are named kinds; dark/light pick the two Photoshop-grammar
            // ones. accent/scale have no token-level equivalent and are ignored.
            let kind = ThemeKind::from_name(if theme.dark { "dark" } else { "light" })
                .unwrap_or(if theme.dark { ThemeKind::Pro } else { ThemeKind::StudioLight });
            app.set_theme(ctx, kind);
        }
        if !self.pending.is_empty() {
            return;
        }
        let next = self.state.inner().commands.pop_front();
        if let Some((id, cmd, params)) = next {
            let (method, params) = route_command(&cmd, params);
            let (req, rx) = ControlRequest::new(method, params);
            if self.control_tx.send(req).is_ok() {
                self.pending.push((id, rx));
            } else {
                self.transport.post(
                    &AppToHost::CommandResult {
                        id,
                        ok: false,
                        result: None,
                        error: Some("the control channel is closed".into()),
                    },
                    None,
                );
            }
        }
    }

    /// After the frame: post the control channel's replies, mirror the dirty flag, and pump
    /// queued saves to the host.
    pub fn post_frame(&mut self, app: &mut PhotocraftApp) {
        // Replies (commands that ran this frame, jobs, settled screenshots, processed
        // synthetic input): exactly one craft:command-result per craft:command.
        let transport = &self.transport;
        self.pending.retain_mut(|(id, rx)| {
            let reply = match rx.try_recv() {
                Ok(reply) => reply,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => {
                    transport.post(
                        &AppToHost::CommandResult {
                            id: *id,
                            ok: false,
                            result: None,
                            error: Some("the control channel closed before replying".into()),
                        },
                        None,
                    );
                    return false;
                }
            };
            let ok = reply.get("ok").and_then(Value::as_bool).unwrap_or(false);
            let result = reply.get("result").cloned().filter(|_| ok);
            let error = reply
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|_| !ok);
            transport.post(&AppToHost::CommandResult { id: *id, ok, result, error }, None);
            false
        });
        // The dirty flag for the host header (also what HostHooks::dirty reports).
        let dirty = app.has_unsaved_work();
        self.state.inner().dirty = dirty;
        if self.last_dirty != Some(dirty) {
            self.last_dirty = Some(dirty);
            self.transport.post(&AppToHost::DocumentChanged { dirty }, None);
        }
        // Saves the write service queued → craft:save-request (bytes as transferables).
        self.transport.pump(&self.hooks);
    }
}
