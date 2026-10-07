use crate::core::types::AllternitEvent;
use crate::wih::types::{LoopPolicy, WihState};

pub fn project_wih(events: &[AllternitEvent], wih_id: &str) -> Option<WihState> {
    let mut wih: Option<WihState> = None;

    for evt in events {
        let payload = &evt.payload;
        let event_wih = payload.get("wih_id").and_then(|v| v.as_str()).unwrap_or("");
        if event_wih != wih_id {
            continue;
        }

        match evt.r#type.as_str() {
            "WIHCreated" => {
                let dag_id = payload
                    .get("dag_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let node_id = payload
                    .get("node_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let loop_policy = payload
                    .get("policy")
                    .and_then(|p| p.get("loop_policy"))
                    .and_then(|val| serde_json::from_value::<LoopPolicy>(val.clone()).ok());
                wih = Some(WihState {
                    wih_id: wih_id.to_string(),
                    dag_id,
                    node_id,
                    status: "CREATED".to_string(),
                    open_signed: false,
                    agent_id: None,
                    role: None,
                    picked_up_at: None,
                    open_signature: None,
                    close_request: None,
                    final_status: None,
                    closed_at: None,
                    execution_mode: payload
                        .get("execution_mode")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                context_pack_path: payload
                    .get("context_pack_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                resolved_prompt_path: payload
                    .get("resolved_prompt_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                loop_policy,
                loop_state: None,
                pending_elicitation: None,
                pending_sampling: None,
                last_heartbeat: None,
                terminal_context: None,
            });
        }
            // A claimed node handed to another agent (`workspace node handoff`).
            "AgentHandoffRequested" => {
                if let Some(state) = wih.as_mut() {
                    if let Some(target) = payload.get("target_agent_id").and_then(|v| v.as_str()) {
                        state.agent_id = Some(target.to_string());
                    }
                }
            }
            "ElicitationRequested" => {
                if let Some(state) = wih.as_mut() {
                    state.pending_elicitation = payload.get("elicitation_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                }
            }
            "ElicitationResponded" => {
                if let Some(state) = wih.as_mut() {
                    state.pending_elicitation = None;
                }
            }
            "SamplingRequested" => {
                if let Some(state) = wih.as_mut() {
                    state.pending_sampling = payload.get("sampling_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                }
            }
            "SamplingResponded" => {
                if let Some(state) = wih.as_mut() {
                    state.pending_sampling = None;
                }
            }
            "WIHPickedUp" => {
                if let Some(state) = wih.as_mut() {
                    state.status = "ACTIVE".to_string();
                    state.agent_id = payload
                        .get("agent_id")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.role = payload
                        .get("role")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.picked_up_at = payload
                        .get("picked_up_at")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            "WIHOpenSigned" => {
                if let Some(state) = wih.as_mut() {
                    state.open_signed = true;
                    state.open_signature = payload
                        .get("signature")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            "WIHCloseRequested" => {
                if let Some(state) = wih.as_mut() {
                    state.status = "CLOSING".to_string();
                    state.close_request = Some(payload.clone());
                }
            }
            "WIHClosedSigned" => {
                if let Some(state) = wih.as_mut() {
                    state.status = "CLOSED".to_string();
                    state.final_status = payload
                        .get("final_status")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.closed_at = payload
                        .get("closed_at")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            "RailsLoopIterationStarted" => {
                if let Some(state) = wih.as_mut() {
                    let mut loop_state = state.loop_state.take().unwrap_or_default();
                    loop_state.current_iteration = payload
                        .get("iteration")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    loop_state.last_started_at = payload
                        .get("started_at")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    loop_state.last_outcome = Some("started".to_string());
                    loop_state.escalated = false;
                    state.loop_state = Some(loop_state);
                }
            }
            "RailsLoopIterationCompleted" => {
                if let Some(state) = wih.as_mut() {
                    let mut loop_state = state.loop_state.take().unwrap_or_default();
                    loop_state.last_completed_at = payload
                        .get("completed_at")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    loop_state.last_outcome = Some(
                        payload
                            .get("outcome")
                            .and_then(|v| v.as_str())
                            .unwrap_or("completed")
                            .to_string(),
                    );
                    state.loop_state = Some(loop_state);
                }
            }
            "RailsLoopIterationEscalated" => {
                if let Some(state) = wih.as_mut() {
                    let mut loop_state = state.loop_state.take().unwrap_or_default();
                    loop_state.escalated = true;
                    loop_state.last_outcome = Some(
                        payload
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("escalated")
                            .to_string(),
                    );
                    state.loop_state = Some(loop_state);
                }
            }
            "GateTurnCloseout" => {
                if let Some(state) = wih.as_mut() {
                    state.last_heartbeat = payload
                        .get("last_heartbeat")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            _ => {}
        }
    }

    wih
}

#[cfg(test)]
mod handoff_tests {
    use super::*;
    use crate::core::types::{Actor, ActorType};
    use serde_json::json;

    fn evt(t: &str, payload: serde_json::Value) -> AllternitEvent {
        AllternitEvent {
            event_id: format!("e-{t}"),
            ts: "2026-10-06T00:00:00Z".into(),
            actor: Actor { r#type: ActorType::Gate, id: "gate".into() },
            scope: None,
            r#type: t.into(),
            payload,
            provenance: None,
        }
    }

    #[test]
    fn a_handoff_survives_reprojection() {
        let events = vec![
            evt("WIHCreated", json!({ "wih_id": "w1", "dag_id": "d", "node_id": "n" })),
            evt("WIHPickedUp", json!({ "wih_id": "w1", "agent_id": "builder@team" })),
            evt("AgentHandoffRequested", json!({ "wih_id": "w1", "source_agent_id": "builder@team", "target_agent_id": "reviewer@team" })),
        ];
        let w = project_wih(&events, "w1").unwrap();
        assert_eq!(w.agent_id.as_deref(), Some("reviewer@team"));
        assert_eq!(w.status, "ACTIVE");
    }
}
