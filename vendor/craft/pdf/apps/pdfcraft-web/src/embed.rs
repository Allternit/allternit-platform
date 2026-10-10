//! craft:1 host-page bridge adapter (feature `embed`; wasm only).
//!
//! When the page URL carries `?embed=1&origin=<parent-origin>` the app speaks the postMessage
//! protocol from `vendor/craft/craft-host/PROTOCOL.md` instead of behaving standalone: the host
//! page replaces the current document (`craft:open` → [`PdfCraftApp::open_bytes`]), drives the
//! engine command registry (`craft:command` → [`PdfCraftApp::execute`], the same ids the
//! menus/CLI/MCP expose), and persists saves (the save-bytes that would be a browser download
//! are queued as `craft:save-request` instead, unless the page passed `?download=1`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};

use craft_host::{EmbedConfig, HostHooks, SavePayload, Theme, embed_config_from_location, wasm::BridgeTransport};
use pdfcraft_ui_egui::{PdfCraftApp, PendingSave, SaveOutput};

/// The app wrapped for an embedded session: eframe and the bridge share one `PdfCraftApp`
/// (wasm is single-threaded, so the message handler and `logic` never run concurrently — one
/// borrows the app at a time, and the two RefCells are never locked nested).
pub struct EmbedShell {
    app: Rc<RefCell<PdfCraftApp>>,
    hooks: Rc<RefCell<dyn HostHooks>>,
    /// `None` when the bridge could not start: the editor still runs, saves fall back to
    /// downloads, and the host's "no craft:ready" timeout shows its error surface (PROTOCOL.md).
    transport: Option<BridgeTransport>,
}

impl EmbedShell {
    fn start(app: PdfCraftApp, config: EmbedConfig) -> Self {
        let app = Rc::new(RefCell::new(app));
        let (tx, rx) = channel();
        app.borrow_mut().save_output = SaveOutput::Embed { tx, also_download: query_flag("download") };
        let hooks: Rc<RefCell<dyn HostHooks>> = Rc::new(RefCell::new(EmbedHooks { app: app.clone(), saves: rx, theme: None }));
        let transport = match BridgeTransport::start(config, hooks.clone()) {
            Ok(transport) => Some(transport),
            Err(e) => {
                // Never a panic (AGENTS.md §4): keep the editor usable, restores downloads.
                eframe::web_sys::console::error_1(&format!("Allternit PDF Editor: craft:1 bridge failed to start: {e:?}").into());
                app.borrow_mut().save_output = SaveOutput::Download;
                None
            }
        };
        Self { app, hooks, transport }
    }
}

impl eframe::App for EmbedShell {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.borrow_mut().logic(ctx, frame);
        // One save per frame is plenty: pump after logic so a save queued by this frame's
        // command processing goes out the same frame.
        if let Some(transport) = &self.transport {
            transport.pump(&self.hooks);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.borrow_mut().ui(ui, frame);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.borrow_mut().raw_input_hook(ctx, raw);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.app.borrow_mut().save(storage);
    }
}

/// [`HostHooks`] wiring: engine session and document IO onto the shared bridge.
struct EmbedHooks {
    app: Rc<RefCell<PdfCraftApp>>,
    /// Saves queued by the app's save path, drained by `take_save` (one per bridge pump).
    saves: Receiver<PendingSave>,
    /// Last `craft:theme` received; applying it to the egui theme is a later milestone.
    theme: Option<Theme>,
}

impl HostHooks for EmbedHooks {
    fn app_id(&self) -> &'static str {
        "pdf"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn open(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<String>, String> {
        self.app.borrow_mut().open_bytes(name, None, bytes.to_vec())?;
        Ok(Vec::new())
    }

    fn command(&mut self, cmd: &str, _params: serde_json::Value) -> Result<serde_json::Value, String> {
        if !PdfCraftApp::command_known(cmd) {
            return Err(format!("unknown/disallowed command: {cmd}"));
        }
        // One in flight per session is inherent here: the bridge handles messages to completion,
        // one at a time, on the single wasm thread (PROTOCOL.md §Command channel). The engine's
        // command registry is id-only, so `params` has nothing to bind to in this app (v1).
        if self.app.borrow_mut().execute(cmd) { Ok(serde_json::json!({ "ran": cmd })) } else { Err(format!("{cmd} is disabled right now")) }
    }

    fn take_save(&mut self) -> Option<SavePayload> {
        let save = self.saves.try_recv().ok()?;
        Some(SavePayload {
            name: save.name,
            format: Some("application/pdf".into()),
            bytes: (*save.bytes).clone(),
            meta: serde_json::json!({ "kind": "document" }),
        })
    }

    fn set_theme(&mut self, theme: &Theme) {
        self.theme = Some(theme.clone());
    }

    fn dirty(&self) -> bool {
        self.app.borrow().first_dirty().is_some()
    }
}

/// Start the craft:1 bridge when the page URL asks for it (`?embed=1&origin=…`); otherwise
/// return the app as the plain standalone editor.
pub fn start_embedded(app: PdfCraftApp) -> Box<dyn eframe::App> {
    match embed_config_from_location() {
        Some(config) => Box::new(EmbedShell::start(app, config)),
        None => Box::new(app),
    }
}

/// Whether the page URL carries `?<key>=1` (a value of "0" or empty means off).
fn query_flag(key: &str) -> bool {
    let Some(search) = web_sys::window().and_then(|w| w.location().search().ok()) else { return false };
    web_sys::UrlSearchParams::new_with_str(&search).ok().and_then(|p| p.get(key)).is_some_and(|v| !v.is_empty() && v != "0")
}
