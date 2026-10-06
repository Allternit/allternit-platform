//! The pane engine as the Factory engine's [`PaneBackend`] (an Allternit
//! addition beside the herdr code, like `factory_host`).
//!
//! `allternit-factory` calls [`install`] at startup; from then on every engine
//! spawn (`workflows drive` executors, `agents up` later), send, capture and
//! kill goes through this pane engine's socket API — the same calls
//! `allternit-factory pane ao spawn|send|status|kill` make. There is no tmux
//! path.
//!
//! [`PaneBackend`]: allternit_factory_engine::backend::PaneBackend

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use allternit_factory_engine::backend::{self, EngineStatus, LivePane, PaneBackend, PaneSend, PaneSpawn, Transport};
use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::api::client::ApiClient;
use crate::api::schema::{
    LayoutApplyParams, LayoutNode, LayoutPane, Method, PaneListParams,
    WorkspaceCloseParams, WorkspaceCreateParams,
};
use crate::cli::ao::{self as ao, CallError};

/// Install this pane engine as the Factory engine's backend.
pub fn install() {
    backend::install(Arc::new(PaneEngine));
}

/// The pane engine reached over its local socket (session `ao`).
pub struct PaneEngine;

fn err(e: CallError) -> anyhow::Error {
    match e {
        CallError::EngineDown => anyhow::Error::new(Transport(format!(
            "the pane engine is not running (socket {})",
            crate::api::socket_path().display()
        ))),
        CallError::Rpc { code, message } => anyhow!("pane engine: {message} ({code})"),
        CallError::Io(e) => anyhow::Error::new(Transport(format!("pane engine socket: {e}"))),
    }
}

fn client() -> ApiClient {
    ao::ensure_ao_session();
    ApiClient::local()
}

/// Agent workspaces: label → workspace id.
fn agent_workspaces(client: &ApiClient) -> Result<Vec<(String, String)>, CallError> {
    Ok(ao::workspaces(client)?
        .into_iter()
        .filter_map(|ws| {
            let label = ws["label"].as_str()?.to_string();
            let id = ws["workspace_id"].as_str()?.to_string();
            label.starts_with("ao-").then_some((label, id))
        })
        .collect())
}

fn first_pane(client: &ApiClient, workspace_id: &str) -> Result<Option<Value>, CallError> {
    let result = ao::call(
        client,
        Method::PaneList(PaneListParams { workspace_id: Some(workspace_id.to_string()) }),
    )?;
    Ok(result["panes"].as_array().and_then(|p| p.first()).cloned())
}

fn live_pane(session: &str, pane: &Value) -> Option<LivePane> {
    Some(LivePane {
        session: session.to_string(),
        pane_id: pane["pane_id"].as_str()?.to_string(),
        cwd: pane["cwd"].as_str().map(str::to_string),
        agent_status: pane["agent_status"]
            .as_str()
            .filter(|s| *s != "unknown")
            .map(str::to_string),
    })
}

/// Session label for an agent pane outside an `ao-` workspace (a pane a
/// person opened and started an agent in): `ao-pane-<paneId>`.
const ADOPTED_PREFIX: &str = "ao-pane-";

/// Every live agent pane: the first pane of each `ao-` workspace, plus any
/// other pane the pane engine detected an agent in.
fn list_all(client: &ApiClient) -> Result<Vec<LivePane>, CallError> {
    let mut out = Vec::new();
    for (label, id) in agent_workspaces(client)? {
        if let Some(pane) = first_pane(client, &id)?.and_then(|p| live_pane(&label, &p)) {
            out.push(pane);
        }
    }
    let agent_ws: std::collections::HashSet<String> =
        agent_workspaces(client)?.into_iter().map(|(_, id)| id).collect();
    let all = ao::call(client, Method::PaneList(PaneListParams { workspace_id: None }))?;
    for pane in all["panes"].as_array().into_iter().flatten() {
        let in_agent_ws = pane["workspace_id"].as_str().is_some_and(|w| agent_ws.contains(w));
        let has_agent = pane["agent"].as_str().is_some_and(|a| !a.is_empty());
        if in_agent_ws || !has_agent {
            continue;
        }
        if let Some(id) = pane["pane_id"].as_str() {
            if let Some(p) = live_pane(&format!("{ADOPTED_PREFIX}{id}"), pane) {
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn find(client: &ApiClient, session: &str) -> Result<Option<LivePane>, CallError> {
    if let Some(pane_id) = session.strip_prefix(ADOPTED_PREFIX) {
        let all = ao::call(client, Method::PaneList(PaneListParams { workspace_id: None }))?;
        return Ok(all["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["pane_id"].as_str() == Some(pane_id))
            .and_then(|p| live_pane(session, p)));
    }
    let Some(ws) = ao::find_workspace(client, session)? else { return Ok(None) };
    let id = ws["workspace_id"].as_str().unwrap_or_default();
    Ok(first_pane(client, id)?.and_then(|p| live_pane(session, &p)))
}

impl PaneBackend for PaneEngine {
    fn spawn(&self, req: &PaneSpawn) -> Result<LivePane> {
        ao::ensure_ao_session();
        ao::ensure_engine_running().map_err(|e| {
            anyhow::Error::new(Transport(format!("could not start the pane engine: {e}")))
        })?;
        let client = ApiClient::local();
        if ao::find_workspace(&client, &req.session).map_err(err)?.is_some() {
            return Err(anyhow!("session {} already exists", req.session));
        }
        let cwd = req.cwd.to_string_lossy().to_string();
        let created = ao::call(
            &client,
            Method::WorkspaceCreate(WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: Some(cwd.clone()),
                focus: false,
                label: Some(req.session.clone()),
                env: Default::default(),
            }),
        )
        .map_err(err)?;
        let workspace_id = created["workspace"]["workspace_id"].as_str().unwrap_or_default().to_string();
        let tab_id = created["tab"]["tab_id"].as_str().unwrap_or_default().to_string();
        let mut env: std::collections::HashMap<String, String> =
            req.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Some(log) = &req.transcript {
            env.insert(crate::ao::transcript::TRANSCRIPT_ENV_VAR.to_string(), log.to_string_lossy().to_string());
        }
        let applied = ao::call(
            &client,
            Method::LayoutApply(LayoutApplyParams {
                workspace_id: None,
                tab_id: Some(tab_id),
                tab_label: None,
                focus: false,
                root: LayoutNode::Pane {
                    pane: LayoutPane {
                        pane_id: None,
                        label: None,
                        cwd: Some(cwd.clone()),
                        command: Some(req.argv.clone()),
                        env,
                    },
                },
            }),
        );
        if let Err(e) = applied {
            let _ = ao::call(
                &client,
                Method::WorkspaceClose(WorkspaceCloseParams { workspace_id: workspace_id.clone(), close_group: true }),
            );
            return Err(err(e));
        }
        let pane = first_pane(&client, &workspace_id).map_err(err)?;
        // A run that finished instantly may already be gone; report the pane
        // id when it can be read, the workspace otherwise.
        Ok(pane
            .as_ref()
            .and_then(|p| live_pane(&req.session, p))
            .unwrap_or(LivePane { session: req.session.clone(), pane_id: workspace_id, cwd: Some(cwd), agent_status: None }))
    }

    fn list(&self) -> Result<Vec<LivePane>> {
        match list_all(&client()) {
            Ok(panes) => Ok(panes),
            // No pane engine running means no live panes: a fact, not a
            // failure (`status()` reports the engine itself as down).
            Err(CallError::EngineDown) => Ok(Vec::new()),
            Err(e) => Err(err(e)),
        }
    }

    fn find(&self, session: &str) -> Result<Option<LivePane>> {
        match find(&client(), session) {
            Ok(p) => Ok(p),
            Err(CallError::EngineDown) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    fn status(&self) -> EngineStatus {
        match ao::workspaces(&client()) {
            Ok(_) => EngineStatus { running: true, error: None },
            Err(e) => EngineStatus { running: false, error: Some(format!("{:#}", err(e))) },
        }
    }

    fn send(&self, root: &Path, session: &str, text: &str, sender: &str, queue_only: bool) -> Result<PaneSend> {
        let reason = if queue_only {
            "queued on request".to_string()
        } else {
            let client = client();
            match find(&client, session) {
                Ok(None) | Err(CallError::EngineDown) => "the pane is not running".to_string(),
                Err(e) => return Err(err(e)),
                Ok(Some(pane)) => {
                    let Some(marker) = ao::prompt_marker(text) else {
                        return Err(anyhow!("the text has no letters or digits to verify the paste with"));
                    };
                    if !ao::pane_idle(&client, &pane.pane_id).map_err(err)? {
                        "the pane is busy".to_string()
                    } else if ao::paste_and_verify(&client, &pane.pane_id, text, &marker).map_err(err)? {
                        return Ok(PaneSend::Verified);
                    } else {
                        "the paste could not be read back (line cleared, not submitted)".to_string()
                    }
                }
            }
        };
        let (id, depth) = crate::ao::mailbox::enqueue(root, session, sender, text).map_err(|e| anyhow!("mailbox: {e}"))?;
        Ok(PaneSend::Queued { message_id: id.to_string(), depth, reason })
    }

    fn capture(&self, session: &str, lines: u32) -> Result<String> {
        let client = client();
        let pane = find(&client, session).map_err(err)?.ok_or_else(|| anyhow!("no live pane for {session}"))?;
        ao::pane_read_text(&client, &pane.pane_id, lines).map_err(err)
    }

    fn kill(&self, session: &str) -> Result<()> {
        if session.starts_with(ADOPTED_PREFIX) {
            // Not started by the engine: closing its workspace could close a
            // person's other panes. They close it themselves.
            return Err(anyhow!("{session} is a pane someone opened; close it in the agent wall"));
        }
        let client = client();
        let Some(ws) = ao::find_workspace(&client, session).map_err(err)? else { return Ok(()) };
        let id = ws["workspace_id"].as_str().unwrap_or_default().to_string();
        ao::call(&client, Method::WorkspaceClose(WorkspaceCloseParams { workspace_id: id, close_group: true }))
            .map_err(err)?;
        // The engine removes the pane asynchronously; wait briefly so a
        // following `ps` reads it gone.
        for _ in 0..20 {
            if ao::find_workspace(&client, session).map_err(err)?.is_none() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}
