//! Headless UI tests of File ▸ Export from the menus (#382): a menu entry carries no parameters, so
//! the entries whose command needs a `path` ask the host's save panel for it, and Media… opens the
//! Export mode like ⌘M, instead of failing with a missing `path`.

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::state::Mode;
use serde_json::{Value, json};

/// What the save panel was asked: filter label, extensions, suggested name.
type Asked = Arc<Mutex<Vec<(String, Vec<String>, String)>>>;

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    /// `save_to: None` is a cancelled save panel.
    fn new(session: Session, save_to: Option<std::path::PathBuf>, asked: Asked) -> Self {
        let (tx, rx) = channel();
        let mut app = FilmcraftApp::new(session).with_control(rx);
        app.hooks.pick_save_as = Some(Box::new(move |filter, exts, suggested| {
            asked.lock().unwrap().push((filter.to_string(), exts.iter().map(|e| e.to_string()).collect(), suggested.to_string()));
            save_to.as_ref().map(|d| d.join(suggested).to_string_lossy().into_owned())
        }));
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }
}

/// The demo project (sequences, clips) with one clip selected in the Project panel.
fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let clip = s.project.items.iter().find(|(_, it)| it.name.ends_with(".mp4")).map(|(id, _)| id.0).unwrap();
    s.execute("project.select", json!({"items": [clip]})).unwrap();
    s
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("fc-export-menu-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every File ▸ Export entry whose command needs a path, and the extension it saves.
const SAVED: [(&str, &str); 6] = [
    ("file.exportEdl", "edl"),
    ("file.exportFcp7Xml", "xml"),
    ("file.exportFcpxml", "fcpxml"),
    ("file.exportOtio", "otio"),
    ("file.exportAle", "ale"),
    ("file.exportSelectionProject", "fcproj"),
];

#[test]
fn file_export_entries_ask_for_the_destination() {
    let dir = tmp("save");
    let asked = Asked::default();
    let mut d = Driver::new(session(), Some(dir.clone()), asked.clone());
    let menu = d.ok("ui.menu.list", json!({}));
    for (id, ext) in SAVED {
        let entry = menu.as_array().unwrap().iter().find(|i| i["id"] == id).unwrap_or_else(|| panic!("{id} missing from the menus"));
        assert_eq!(entry["path"], json!(["File", "Export"]), "{id}");
        let r = d.ok("ui.menu.invoke", json!({"id": id}));
        d.frames(2);
        let (_, exts, suggested) = asked.lock().unwrap().last().cloned().unwrap_or_else(|| panic!("{id} did not ask where to save"));
        assert_eq!(exts, [ext], "{id}");
        assert!(suggested.ends_with(&format!(".{ext}")) && suggested.len() > ext.len() + 1, "{id}: {suggested}");
        let written = dir.join(&suggested);
        assert!(written.metadata().is_ok_and(|m| m.len() > 0), "{id} wrote nothing at {} ({r})", written.display());
    }
    assert_eq!(asked.lock().unwrap().len(), SAVED.len());
}

#[test]
fn cancelling_the_save_panel_writes_nothing_and_reports_no_error() {
    let asked = Asked::default();
    let mut d = Driver::new(session(), None, asked.clone());
    for (id, _) in SAVED {
        assert_eq!(d.ok("ui.menu.invoke", json!({"id": id})), Value::Null, "{id}");
    }
    assert_eq!(asked.lock().unwrap().len(), SAVED.len());
    assert!(!d.harness.state().ui.status.contains("path"), "{}", d.harness.state().ui.status);
}

#[test]
fn media_from_the_menu_opens_the_export_mode() {
    let mut d = Driver::new(session(), None, Asked::default());
    assert_eq!(d.harness.state().ui.mode, Mode::Edit);
    d.ok("ui.menu.invoke", json!({"id": "file.exportMedia"}));
    d.frames(2);
    assert_eq!(d.harness.state().ui.mode, Mode::Export);
    // with a path it still exports directly, as scripts and the CLI use it
    let out = tmp("media").join("direct.wav");
    let r = d.ok("ui.menu.invoke", json!({"id": "file.exportMedia", "params": {"path": out.to_string_lossy(), "format": "wav", "wait": true}}));
    assert!(out.metadata().is_ok_and(|m| m.len() > 0), "{r}");
}

#[test]
fn file_export_entries_need_an_open_sequence() {
    let mut d = Driver::new(Session::default(), Some(tmp("noseq")), Asked::default());
    for id in ["file.exportMedia", "file.exportEdl", "file.exportFcp7Xml", "file.exportFcpxml", "file.exportOtio"] {
        assert_eq!(d.call("ui.menu.invoke", json!({"id": id}))["ok"], json!(false), "{id}");
        assert_eq!(d.harness.state().ui.mode, Mode::Edit, "{id}");
    }
}
