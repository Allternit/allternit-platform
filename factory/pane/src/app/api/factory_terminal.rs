//! `factory.terminal.*` on the app thread (Allternit addition; the tap and the
//! output stream are in `src/factory_terminal.rs`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::api::schema::{
    LayoutApplyParams, LayoutNode, LayoutPane, WorkspaceCloseParams, WorkspaceCreateParams,
};
use crate::app::App;
use crate::factory_terminal::{
    self as ft, FactoryTerminalCreateParams, FactoryTerminalResizeParams, FactoryTerminalTarget,
    FactoryTerminalWriteParams, Tap,
};
use crate::layout::PaneId;

/// Largest single write handed to the PTY actor's queue.
const WRITE_CHUNK: usize = 4096;

fn parse(response: &str) -> Value {
    serde_json::from_str(response).unwrap_or(Value::Null)
}

impl App {
    /// The live pane behind a tap: (workspace index, pane id).
    fn factory_terminal_pane(&self, tap: &Tap) -> Option<(usize, PaneId)> {
        let pane_id = tap.pane_id();
        self.find_pane(pane_id).map(|(ws_idx, _)| (ws_idx, pane_id))
    }

    fn factory_terminal_info(&self, tap: &Tap) -> Value {
        match self.factory_terminal_pane(tap) {
            Some((ws_idx, pane_id)) => tap.info(
                self.public_pane_id(ws_idx, pane_id),
                Some(self.public_workspace_id(ws_idx)),
                self.state.terminal_id_for_pane(ws_idx, pane_id).map(|t| t.to_string()),
            ),
            None => tap.info(None, None, None),
        }
    }

    fn factory_terminal_apply_size(&mut self, tap: &Tap, cols: u16, rows: u16) -> bool {
        let Some((ws_idx, pane_id)) = self.factory_terminal_pane(tap) else {
            return false;
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return false;
        };
        runtime.resize(rows, cols, 0, 0);
        // The client owns this terminal's size: layout passes must not
        // resize it back to its slot in the agent wall.
        if let Some(terminal_id) = self.state.terminal_id_for_pane(ws_idx, pane_id) {
            self.state.direct_attach_resize_locks.insert(terminal_id);
        }
        true
    }

    pub(super) fn handle_factory_terminal_create(
        &mut self,
        id: String,
        params: FactoryTerminalCreateParams,
    ) -> String {
        if !ft::valid_id(&params.terminal_id) {
            return ft::failure(&id, "invalid_terminal_id", "terminal_id must be 1-64 of [A-Za-z0-9_-]");
        }
        if ft::is_running(&params.terminal_id) {
            return ft::failure(
                &id,
                "terminal_exists",
                format!("terminal {} already exists", params.terminal_id),
            );
        }
        let label = params
            .label
            .clone()
            .unwrap_or_else(|| format!("{}{}", ft::LABEL_PREFIX, params.terminal_id));
        let created = parse(&self.handle_workspace_create(
            id.clone(),
            WorkspaceCreateParams {
                source_workspace_id: None,
                cwd: params.cwd.clone(),
                focus: false,
                label: Some(label),
                env: Default::default(),
            },
        ));
        if created.get("error").is_some() {
            return created.to_string();
        }
        let workspace_id = created["result"]["workspace"]["workspace_id"].as_str().unwrap_or_default().to_string();
        let tab_id = created["result"]["tab"]["tab_id"].as_str().unwrap_or_default().to_string();

        let mut env = params.env.clone();
        env.insert(ft::TERMINAL_ENV_VAR.to_string(), params.terminal_id.clone());
        let applied = parse(&self.handle_layout_apply(
            id.clone(),
            LayoutApplyParams {
                workspace_id: None,
                tab_id: Some(tab_id),
                tab_label: None,
                focus: false,
                root: LayoutNode::Pane {
                    pane: LayoutPane {
                        pane_id: None,
                        label: None,
                        cwd: params.cwd.clone(),
                        command: (!params.command.is_empty()).then(|| params.command.clone()),
                        env,
                    },
                },
            },
        ));
        let tap: Option<Arc<Tap>> = ft::get(&params.terminal_id);
        if applied.get("error").is_some() || tap.is_none() {
            let _ = self.handle_workspace_close(
                id.clone(),
                WorkspaceCloseParams { workspace_id, close_group: true },
            );
            ft::remove(&params.terminal_id);
            if applied.get("error").is_some() {
                return applied.to_string();
            }
            return ft::failure(&id, "terminal_create_failed", "the terminal's pane did not start");
        }
        let tap = tap.expect("checked above");
        self.factory_terminal_apply_size(&tap, params.cols, params.rows);
        ft::success(&id, json!({ "terminal": self.factory_terminal_info(&tap) }))
    }

    pub(super) fn handle_factory_terminal_write(
        &mut self,
        id: String,
        params: FactoryTerminalWriteParams,
    ) -> String {
        let Some(tap) = ft::get(&params.terminal_id) else {
            return ft::not_found(&id, &params.terminal_id);
        };
        let Some((ws_idx, pane_id)) = self.factory_terminal_pane(&tap) else {
            return ft::failure(&id, "terminal_exited", format!("terminal {} has exited", params.terminal_id));
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return ft::failure(&id, "terminal_exited", format!("terminal {} has exited", params.terminal_id));
        };
        for chunk in params.data.as_bytes().chunks(WRITE_CHUNK) {
            if let Err(err) = runtime.try_send_bytes(Bytes::copy_from_slice(chunk)) {
                return ft::failure(&id, "terminal_write_failed", err.to_string());
            }
        }
        ft::success(&id, json!({}))
    }

    pub(super) fn handle_factory_terminal_resize(
        &mut self,
        id: String,
        params: FactoryTerminalResizeParams,
    ) -> String {
        if params.cols == 0 || params.rows == 0 {
            return ft::failure(&id, "invalid_size", "cols and rows must be at least 1");
        }
        let Some(tap) = ft::get(&params.terminal_id) else {
            return ft::not_found(&id, &params.terminal_id);
        };
        if !self.factory_terminal_apply_size(&tap, params.cols, params.rows) {
            return ft::failure(&id, "terminal_exited", format!("terminal {} has exited", params.terminal_id));
        }
        ft::success(&id, json!({}))
    }

    /// Closes the terminal's pane (its workspace, when the pane is the only
    /// one) and forgets the tap. Closing a terminal that already exited only
    /// forgets it.
    pub(super) fn handle_factory_terminal_close(
        &mut self,
        id: String,
        params: FactoryTerminalTarget,
    ) -> String {
        let Some(tap) = ft::get(&params.terminal_id) else {
            return ft::not_found(&id, &params.terminal_id);
        };
        if let Some((ws_idx, pane_id)) = self.factory_terminal_pane(&tap) {
            if let Some(terminal_id) = self.state.terminal_id_for_pane(ws_idx, pane_id) {
                self.state.direct_attach_resize_locks.remove(&terminal_id);
            }
            let single_pane = self
                .state
                .workspaces
                .get(ws_idx)
                .is_some_and(|ws| ws.public_pane_numbers.len() <= 1);
            let response = if single_pane {
                let workspace_id = self.public_workspace_id(ws_idx);
                parse(&self.handle_workspace_close(
                    id.clone(),
                    WorkspaceCloseParams { workspace_id, close_group: false },
                ))
            } else {
                match self.public_pane_id(ws_idx, pane_id) {
                    Some(pane) => parse(&self.handle_pane_close(
                        id.clone(),
                        crate::api::schema::PaneTarget { pane_id: pane },
                    )),
                    None => Value::Null,
                }
            };
            if response.get("error").is_some() {
                return response.to_string();
            }
        }
        ft::remove(&params.terminal_id);
        ft::success(&id, json!({}))
    }

    pub(super) fn handle_factory_terminal_get(&mut self, id: String, params: FactoryTerminalTarget) -> String {
        match ft::get(&params.terminal_id) {
            Some(tap) => ft::success(&id, json!({ "terminal": self.factory_terminal_info(&tap) })),
            None => ft::not_found(&id, &params.terminal_id),
        }
    }

    pub(super) fn handle_factory_terminal_list(&mut self, id: String) -> String {
        let terminals: Vec<Value> = ft::all().iter().map(|tap| self.factory_terminal_info(tap)).collect();
        ft::success(&id, json!({ "terminals": terminals }))
    }
}
