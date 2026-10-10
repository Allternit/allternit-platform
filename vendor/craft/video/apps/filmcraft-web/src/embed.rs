//! craft:1 host-page embed adapter (see `vendor/craft/craft-host/PROTOCOL.md`).
//!
//! When the page is loaded with `?embed=1&origin=<parent origin>` in a sandboxed iframe, the app
//! speaks the craft:1 `postMessage` protocol through the `craft-host` bridge instead of exposing
//! the raw, unauthenticated `window.filmcraft` object:
//!
//! - [`init`] (from `start`) detects embed mode and keeps `window.filmcraft` uninstalled, so the
//!   bridge is the only control surface; token + parent-origin validation live in `craft-host`.
//! - [`attach`] (once the eframe app exists) starts the `craft-host` transport: it posts
//!   `craft:ready`, answers `craft:hello`, and dispatches `craft:open` / `craft:command` /
//!   `craft:theme` through [`HostHooks`] (`EmbedHooks`). Commands run in-process through
//!   `filmcraft_ui_egui::menus::invoke` — the exact path behind `window.filmcraft.execute` and
//!   the desktop TCP channel's `engine.execute`.
//! - [`pump`] (every UI frame) runs the save watcher: app writes (`file.save`,
//!   `file.exportMedia`, …) land in the virtual file table as in-memory entries whose browser
//!   download is suppressed in embed mode; new/changed entries are read back and forwarded as
//!   `craft:save-request`s (bytes transferred). It also mirrors the dirty flag as
//!   `craft:document-changed` and a `window.filmcraftLoad.fatal` panic as a
//!   `craft:command-event {event: "fatal"}` — PROTOCOL.md names no `craft:error` type, so the
//!   closest existing envelope carries it (documented adapter deviation).
//!
//! Deviations from PROTOCOL.md (all forced by the `craft-host` crate's fixed API, which is
//! treated as read-only): no `craft:error` message exists, fatal panics ride
//! `command-event{event:"fatal"}`; `craft:save-ack` is never processed (the crate has no
//! `save-acked` handling and `HostToApp` has no `SaveAck` variant — saves are posted once, the
//! host ack is ignored); `craft:document-changed` is posted by this adapter directly because
//! `BridgeTransport::pump` only forwards saves.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use craft_host::wasm::BridgeTransport;
use craft_host::{AppToHost, EmbedConfig, Envelope, HostHooks, SavePayload, Theme};
use serde_json::{Value, json};
use wasm_bindgen::JsValue;

use crate::WebApp;

/// Saves waiting for the transport to forward them. The bridge drops pre-hello saves and the
/// host must ack each one; a full queue means the host is gone, so the oldest are dropped (the
/// document itself stays alive in the editor).
const MAX_QUEUED_SAVES: usize = 8;

thread_local! {
    /// Embed-mode config from the page URL (`None` = a standalone visit), moved out by
    /// [`attach`] when the bridge starts.
    static CONFIG: RefCell<Option<EmbedConfig>> = const { RefCell::new(None) };
    /// Parent origin accepted by the bridge (from `?origin=`); used by [`post`].
    static ORIGIN: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The running app, shared so the bridge hooks — fired by the transport's `message` listener
    /// between UI frames, same thread — can run engine commands in-process. Frames run inside
    /// [`SharedApp`]'s borrow; the hooks use `try_borrow_mut` and return an error instead of
    /// ever double-borrowing (the borrows cannot overlap by construction).
    static APP: RefCell<Option<Rc<RefCell<WebApp>>>> = const { RefCell::new(None) };
    static HOOKS: RefCell<Option<Rc<RefCell<dyn HostHooks>>>> = const { RefCell::new(None) };
    static TRANSPORT: RefCell<Option<BridgeTransport>> = const { RefCell::new(None) };
    /// Save-watcher baseline: virtual file table path → size. Saves are detected by diffing
    /// `fs::list()` (path + size), per the audit's save-channel mapping.
    static FILES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    /// Completed saves not yet forwarded to the host.
    static SAVES: RefCell<VecDeque<SavePayload>> = const { RefCell::new(VecDeque::new()) };
    /// Last dirty flag mirrored to the host.
    static DIRTY: RefCell<Option<bool>> = const { RefCell::new(None) };
    /// Set once the bridge accepted a `craft:hello` (`set_theme` runs only from validated
    /// `hello`/`theme` messages), i.e. the host is live and listening.
    static HELLO: RefCell<bool> = const { RefCell::new(false) };
    /// Set once a fatal state was reported to the host.
    static FATAL_SENT: RefCell<bool> = const { RefCell::new(false) };
    /// In embed mode, written files keep their hidden-anchor browser download only with
    /// `?download=1`; otherwise the host persists them (`craft:save-request`).
    static SUPPRESS_DOWNLOAD: RefCell<bool> = const { RefCell::new(false) };
}

/// Detect embed mode from the page URL. Called once from `start`, before `api::install()`; the
/// caller skips installing `window.filmcraft` when this returns `true`.
pub fn init() -> bool {
    let Some(config) = craft_host::embed_config_from_location() else { return false };
    SUPPRESS_DOWNLOAD.with(|s| *s.borrow_mut() = !crate::query_flag("download"));
    CONFIG.with(|c| *c.borrow_mut() = Some(config));
    log::info!("craft:1 embed mode: the raw window.filmcraft object stays uninstalled; the authenticated bridge is the only control surface");
    true
}

/// Whether a written file's browser download is suppressed (embed mode without `?download=1`).
pub fn suppress_download() -> bool {
    SUPPRESS_DOWNLOAD.with(|s| *s.borrow())
}

/// The eframe app shared with the bridge hooks. `eframe` owns this wrapper; the hooks hold the
/// same `Rc<RefCell<WebApp>>` and run engine commands between frames (single-threaded wasm, so
/// the borrows never overlap; `try_borrow_mut` guards regardless rather than panicking).
pub struct SharedApp {
    /// The app the bridge hooks also hold (constructed in `start`'s creator closure).
    pub inner: Rc<RefCell<WebApp>>,
}

impl eframe::App for SharedApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        match self.inner.try_borrow_mut() {
            Ok(mut app) => eframe::App::raw_input_hook(&mut *app, ctx, raw_input),
            Err(_) => log::error!("embed: app is already borrowed in raw_input_hook"),
        }
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        match self.inner.try_borrow_mut() {
            Ok(mut app) => eframe::App::logic(&mut *app, ctx, frame),
            Err(_) => log::error!("embed: app is already borrowed in logic"),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        match self.inner.try_borrow_mut() {
            Ok(mut app) => eframe::App::ui(&mut *app, ui, frame),
            Err(_) => log::error!("embed: app is already borrowed in ui"),
        }
    }
}

/// Start the craft:1 bridge. Called once the eframe app exists (inside the runner's creator
/// closure), before the first frame: posts `craft:ready` and from here on answers `craft:hello`.
pub fn attach(app: Rc<RefCell<WebApp>>) {
    let Some(config) = CONFIG.with(|c| c.borrow_mut().take()) else { return };
    ORIGIN.with(|o| *o.borrow_mut() = Some(config.parent_origin.clone()));
    // Baseline the save watcher on the file table as it stands after recovery/demo loading, so
    // pre-existing entries (the recovery snapshot, restored media) are not echoed back as saves.
    sync_files();
    let hooks: Rc<RefCell<EmbedHooks>> = Rc::new(RefCell::new(EmbedHooks { app: app.clone(), theme: None }));
    let hooks_dyn: Rc<RefCell<dyn HostHooks>> = hooks;
    match BridgeTransport::start(config, hooks_dyn.clone()) {
        Ok(transport) => {
            APP.with(|a| *a.borrow_mut() = Some(app));
            HOOKS.with(|h| *h.borrow_mut() = Some(hooks_dyn));
            TRANSPORT.with(|t| *t.borrow_mut() = Some(transport));
        }
        Err(e) => {
            log::error!("craft:1 bridge failed to start: {e:?}");
        }
    }
}

/// Per-frame duties, called from `WebApp::logic` (the app is passed in, so this never
/// re-borrows the shared refcell): run the save watcher, mirror dirty/fatal state to the host,
/// and forward queued saves over the transport.
pub fn pump(app: &mut WebApp) {
    if TRANSPORT.with(|t| t.borrow().is_none()) {
        return;
    }
    // Save watcher: the engine writes saves/exports into the virtual file table as in-memory
    // entries (and would offer them as downloads — suppressed in embed mode). A new or
    // size-changed in-memory entry outside `/opfs/` is a completed save: read it back and queue
    // it as a `craft:save-request`. This catches both synchronous `file.save` and job-based
    // `file.exportMedia` completions, plus anything the user saves through the UI.
    let mut snapshot = FILES.with(|f| f.borrow().clone());
    let mut queue = SAVES.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for (path, size, kind) in crate::fs::list() {
        let changed = snapshot.insert(path.clone(), size).map_or(true, |old| old != size);
        if !changed || kind != "memory" || path.starts_with("/opfs/") {
            continue;
        }
        let Some(bytes) = crate::fs::mem_bytes(&path) else { continue };
        if queue.len() >= MAX_QUEUED_SAVES {
            log::warn!("craft:1: save queue is full ({MAX_QUEUED_SAVES}); dropping {path}");
            continue;
        }
        log::info!("craft:1: save-request for {path} ({} bytes)", bytes.len());
        queue.push_back(SavePayload {
            name: path.rsplit('/').next().unwrap_or(&path).to_string(),
            format: None,
            bytes,
            meta: json!({ "path": path }),
        });
    }
    FILES.with(|f| *f.borrow_mut() = snapshot);
    SAVES.with(|q| *q.borrow_mut() = queue);

    // Dirty flag → host header (`craft:document-changed`).
    let dirty = app.app.session.is_dirty();
    if HELLO.with(|h| *h.borrow()) && DIRTY.with(|d| *d.borrow()) != Some(dirty) {
        DIRTY.with(|d| *d.borrow_mut() = Some(dirty));
        post(&AppToHost::DocumentChanged { dirty }, None);
    }
    // A fatal panic flips `window.filmcraftLoad.fatal` (web/index.html); mirror it once. PROTOCOL.md
    // defines no `craft:error` type, so the notification rides `command-event {event: "fatal"}`.
    if HELLO.with(|h| *h.borrow())
        && !FATAL_SENT.with(|f| *f.borrow())
        && let Some(msg) = fatal_message()
    {
        FATAL_SENT.with(|f| *f.borrow_mut() = true);
        post(&AppToHost::CommandEvent { event: "fatal".into(), data: json!({ "message": msg }) }, None);
    }
    // Forward queued saves (`craft:save-request`, bytes as transferables).
    if let Some(hooks) = HOOKS.with(|h| h.borrow().clone()) {
        TRANSPORT.with(|t| {
            if let Some(transport) = &*t.borrow() {
                transport.pump(&hooks);
            }
        });
    }
}

/// Rebaseline the save watcher onto the current virtual file table.
fn sync_files() {
    let snapshot: HashMap<String, u64> = crate::fs::list().into_iter().map(|(p, n, _)| (p, n)).collect();
    FILES.with(|f| *f.borrow_mut() = snapshot);
}

/// `window.filmcraftLoad.fatal` (set by web/index.html when a panic kills the app), if any.
fn fatal_message() -> Option<String> {
    let w = web_sys::window()?;
    let load = js_sys::Reflect::get(&w, &"filmcraftLoad".into()).ok()?;
    js_sys::Reflect::get(&load, &"fatal".into()).ok()?.as_string()
}

/// Post an app→host message the transport does not cover (`craft:document-changed`, fatal
/// notifications). Same framing as `craft-host`'s transport: an [`Envelope`] JSON body, with
/// bytes as a transferred `ArrayBuffer` beside it.
fn post(msg: &AppToHost, bytes: Option<&[u8]>) {
    let Some(origin) = ORIGIN.with(|o| o.borrow().clone()) else { return };
    let Some(window) = web_sys::window() else { return };
    let Some(parent) = window.parent().ok().flatten() else { return };
    let Ok(body) = Envelope::pack(msg, bytes.is_some()) else { return };
    if let Some(b) = bytes {
        let buf = js_sys::Uint8Array::from(b).buffer();
        let arr = js_sys::Array::new();
        arr.push(&JsValue::from_str(&body));
        arr.push(&buf);
        let _ = parent.post_message_with_transfer(arr.as_ref(), &origin, arr.as_ref());
    } else {
        let _ = parent.post_message(&JsValue::from_str(&body), &origin);
    }
}

/// Store bytes at an unused virtual path under `dir` (`name (2).ext` on collisions, like
/// `fs::register_blob`), returning the path. The loop terminates: candidates are distinct and
/// the occupied set is finite.
fn put_unique(dir: &str, name: &str, bytes: &[u8]) -> String {
    let clean: String = name.chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    let taken: Vec<String> = crate::fs::list().into_iter().map(|(p, _, _)| p).collect();
    let (stem, ext) = match clean.rfind('.') {
        Some(i) if i > 0 => (&clean[..i], &clean[i..]),
        _ => (clean.as_str(), ""),
    };
    let mut n = 1usize;
    loop {
        let candidate = if n == 1 { format!("{dir}/{clean}") } else { format!("{dir}/{stem} ({n}){ext}") };
        if !taken.iter().any(|p| p == &candidate) {
            crate::fs::put(&candidate, bytes.to_vec());
            return candidate;
        }
        n += 1;
    }
}

/// The app's craft:1 hooks: protocol messages mapped onto the in-process editor.
struct EmbedHooks {
    app: Rc<RefCell<WebApp>>,
    /// Last `craft:theme`. Rendering is a deliberate no-op in v1 (stored until the embed chrome
    /// work picks it up).
    #[allow(dead_code)]
    theme: Option<Theme>,
}

impl HostHooks for EmbedHooks {
    fn app_id(&self) -> &'static str {
        "video"
    }

    fn version(&self) -> String {
        // The app's version source: `version.workspace = true` (same source the desktop and
        // CLI shells report via CARGO_PKG_VERSION).
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn open(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
        let mut app = self.app.try_borrow_mut().map_err(|_| "the editor is busy (a command is running)".to_string())?;
        // The bytes arrive as a transferred ArrayBuffer; register them in the virtual file
        // table (in-memory entries read synchronously, so no async prewarm is needed) and open
        // them through the same engine commands as the pickers/drag-and-drop (`import.rs`).
        let warnings = if name.to_ascii_lowercase().ends_with(".fcproj") {
            let path = put_unique("/projects", name, bytes);
            app.app
                .session
                .execute("file.open", json!({ "path": path }))
                .map_err(|e| e.to_string())?;
            Vec::new()
        } else {
            let path = put_unique("/files", name, bytes);
            let bin = app.app.import_bin().0;
            let r = app.app
                .session
                .execute("file.import", json!({ "paths": [path], "bin": bin }))
                .map_err(|e| e.to_string())?;
            r.get("errors")
                .and_then(Value::as_array)
                .map(|e| e.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default()
        };
        // The opened/imported bytes are not a save: rebaseline the watcher so they are not
        // echoed back to the host as a `craft:save-request`.
        sync_files();
        Ok(warnings)
    }

    fn command(&mut self, cmd: &str, params: Value) -> Result<Value, String> {
        // The same in-process path as `window.filmcraft.execute` and the desktop TCP channel's
        // `engine.execute` (`ui-egui/src/control.rs`): the full UI + engine command registry.
        // Commands run synchronously between frames, so craft:1's one-in-flight rule holds for
        // everything except job-based commands (`file.exportMedia`), which return immediately by
        // design — the same contract as the JS API.
        let mut app = self.app.try_borrow_mut().map_err(|_| "the editor is busy (a command is running)".to_string())?;
        let ctx = crate::context().ok_or_else(|| "the editor is not ready yet".to_string())?;
        filmcraft_ui_egui::menus::invoke(&mut app.app, &ctx, cmd, params)
    }

    fn take_save(&mut self) -> Option<SavePayload> {
        SAVES.with(|q| q.borrow_mut().pop_front())
    }

    fn set_theme(&mut self, theme: &Theme) {
        // Reached only from a validated `craft:hello` / `craft:theme`, i.e. the host is live.
        HELLO.with(|h| *h.borrow_mut() = true);
        self.theme = Some(theme.clone());
    }

    fn dirty(&self) -> bool {
        self.app.try_borrow().map(|a| a.app.session.is_dirty()).unwrap_or(false)
    }
}
