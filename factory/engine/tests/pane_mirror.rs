//! The pane mirror (`/api/factory/agents/:id/screen|stream|input`): what
//! Desktop's terminal tiles read. A fake pane backend stands in for the pane
//! engine; the router runs on a real socket, as the engine serves it.
//!
//! - the screen is the pane's visible ANSI screen;
//! - the stream sends a `screen` event at once, another only when the
//!   pane's revision changes, then `gone` when the pane closes;
//! - input reaches the pane, records nothing, and refuses empty bodies;
//! - an agent with no live pane is `not_found`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use allternit_factory_engine::api::service::{create_router, ServiceState};
use allternit_factory_engine::backend::{self, LivePane, PaneBackend, PaneScreen, PaneSend, PaneSpawn};
use serde_json::{json, Value};
use tempfile::TempDir;

#[derive(Default)]
struct Panes {
    live: Mutex<Vec<LivePane>>,
    screen: Mutex<(String, u64)>,
    typed: Mutex<Vec<(String, String, Vec<String>)>>,
}

impl PaneBackend for Panes {
    fn spawn(&self, _req: &PaneSpawn) -> anyhow::Result<LivePane> {
        anyhow::bail!("not used")
    }
    fn list(&self) -> anyhow::Result<Vec<LivePane>> {
        Ok(self.live.lock().unwrap().clone())
    }
    fn send(&self, _root: &Path, _s: &str, _t: &str, _f: &str, _q: bool) -> anyhow::Result<PaneSend> {
        anyhow::bail!("not used")
    }
    fn capture(&self, _session: &str, _lines: u32) -> anyhow::Result<String> {
        Ok(String::new())
    }
    fn kill(&self, _session: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn screen(&self, session: &str) -> anyhow::Result<PaneScreen> {
        if !self.live.lock().unwrap().iter().any(|p| p.session == session) {
            anyhow::bail!("no live pane for {session}");
        }
        let (ansi, revision) = self.screen.lock().unwrap().clone();
        Ok(PaneScreen { ansi, revision })
    }
    fn input(&self, session: &str, text: &str, keys: &[String]) -> anyhow::Result<()> {
        self.typed.lock().unwrap().push((session.into(), text.into(), keys.to_vec()));
        Ok(())
    }
}

async fn serve(root: &Path) -> String {
    let state = Arc::new(ServiceState::new(root.to_path_buf()).await.unwrap());
    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn agent_id(base: &str) -> String {
    let body: Value = reqwest::get(format!("{base}/api/factory/agents")).await.unwrap().json().await.unwrap();
    body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["slug"] == "mirror")
        .unwrap_or_else(|| panic!("no mirror agent in {body}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Read SSE events (`event`, `data`) from a streaming response until `n`
/// have arrived.
async fn events(resp: &mut reqwest::Response, n: usize) -> Vec<(String, Value)> {
    let mut buf = String::new();
    let mut out = Vec::new();
    while out.len() < n {
        let chunk = tokio::time::timeout(Duration::from_secs(5), resp.chunk()).await.expect("stream stalled").unwrap();
        let Some(chunk) = chunk else { break };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find("\n\n") {
            let block: String = buf.drain(..end + 2).collect();
            let mut ty = String::new();
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    ty = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.trim());
                }
            }
            if !ty.is_empty() {
                out.push((ty, serde_json::from_str(&data).unwrap()));
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn the_pane_mirror_reads_streams_and_types() {
    let home = TempDir::new().unwrap();
    let root = TempDir::new().unwrap();
    std::env::set_var("ALLTERNIT_FACTORY_HOME", home.path());

    let base = serve(root.path()).await;

    let panes = Arc::new(Panes::default());
    *panes.screen.lock().unwrap() = ("\u{1b}[32mready\u{1b}[0m $ ".into(), 7);
    backend::install(panes.clone());
    panes.live.lock().unwrap().push(LivePane {
        session: "ao-mirror".into(),
        pane_id: "p-1".into(),
        cwd: None,
        agent_status: Some("idle".into()),
    });
    let id = agent_id(&base).await;

    let screen: Value = reqwest::get(format!("{base}/api/factory/agents/{id}/screen")).await.unwrap().json().await.unwrap();
    assert_eq!(screen["ansi"], "\u{1b}[32mready\u{1b}[0m $ ");
    assert_eq!(screen["revision"], 7);
    assert!(screen["at"].is_string());

    // The stream: the screen at once, nothing while it's unchanged, the new
    // screen when the revision moves, `gone` when the pane closes.
    let mut resp = reqwest::get(format!("{base}/api/factory/agents/{id}/stream")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let first = events(&mut resp, 1).await;
    assert_eq!(first[0].0, "screen");
    assert_eq!(first[0].1["revision"], 7);
    *panes.screen.lock().unwrap() = ("ls\r\n".into(), 8);
    let second = events(&mut resp, 1).await;
    assert_eq!((second[0].0.as_str(), second[0].1["ansi"].as_str()), ("screen", Some("ls\r\n")));
    panes.live.lock().unwrap().clear();
    let gone = events(&mut resp, 1).await;
    assert_eq!(gone[0].0, "gone");
    assert!(gone[0].1["reason"].as_str().unwrap().contains("ao-mirror"));

    // Input: the pane is gone now, so not_found …
    let client = reqwest::Client::new();
    let r = client.post(format!("{base}/api/factory/agents/{id}/input")).json(&json!({ "text": "ls" })).send().await.unwrap();
    assert_eq!(r.status(), 404);
    // … and when it's live, the keystrokes reach it as typed.
    panes.live.lock().unwrap().push(LivePane {
        session: "ao-mirror".into(),
        pane_id: "p-1".into(),
        cwd: None,
        agent_status: Some("idle".into()),
    });
    let r = client
        .post(format!("{base}/api/factory/agents/{id}/input"))
        .json(&json!({ "text": "ls", "keys": ["Enter"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({ "ok": true }));
    assert_eq!(*panes.typed.lock().unwrap(), vec![("ao-mirror".to_string(), "ls".to_string(), vec!["Enter".to_string()])]);

    // An empty body is a usage error and types nothing.
    let r = client.post(format!("{base}/api/factory/agents/{id}/input")).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "usage");
    assert_eq!(panes.typed.lock().unwrap().len(), 1);

    // Typing is not a send: nothing in the ledger.
    let deliveries: Value = reqwest::get(format!("{base}/api/factory/deliveries")).await.unwrap().json().await.unwrap();
    assert_eq!(deliveries["deliveries"], json!([]));
}
