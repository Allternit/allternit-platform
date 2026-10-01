//! O2 one routing authority: the kernel router answers "which model class /
//! plan for this turn" for gizzi's chat/cowork/bot turn path.
//!
//! `POST /v1/kernel/turn-route` (and `/api/v1/kernel/turn-route`), same auth
//! as the rest of the Kernel UI backend. gizzi sends the pool snapshot it
//! already owns (`GET /model-pool` shape) so the router never calls back into
//! gizzi; the caller's stored routing policy (org/workspace/project chain)
//! is applied exactly as the agency executor applies it.
//!
//! Shadow-first: gizzi's own `resolveAuto` stays the incumbent and decides;
//! this answer is logged next to it until the router is promoted.

use super::*;
use crate::agency_api::executor::apply_policy;
use allternit_commrails::kernel::classes;
use allternit_commrails::kernel::graph::GraphNode;
use allternit_commrails::kernel::router::{BudgetLedger, PoolEntry, Router, RouterConfig, StaticModelPool};
use axum::extract::State;
use axum::Extension;

/// The synthetic graph node for one conversational turn.
pub fn turn_node(call_type: &str, capability: &str, primitive: &str, max_output_tokens: Option<u64>) -> Result<GraphNode, String> {
    let mut v = json!({
        "node_id": format!("turn.{call_type}"), "primitive_id": primitive, "node_kind": "COMPUTE",
        "cognitive_role": if classes::call_type_class(call_type) == classes::GEN_DEEP { "S3" } else { "S2" },
        "capability_request": { "capability": capability },
        "inputs": [], "outputs": [], "read_set": [], "write_set": [], "lock_scope": [],
        "on_failure": { "strategy": "FAIL" },
        "extensions": { "x-call_type": call_type },
    });
    if let Some(m) = max_output_tokens {
        v["budget"] = json!({ "max_output_tokens": m });
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// Route one turn over a pool snapshot with an effective routing policy.
pub fn route_turn(entries: Vec<PoolEntry>, body: &Value, policy: Option<(Value, Value)>) -> Result<Value, String> {
    let text = |k: &str, d: &str| body[k].as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(d).to_string();
    let call_type = text("call_type", "answer");
    let capability = text("capability", "cap.agent.tool_use");
    let primitive = text("primitive_id", "ctl.compose_final_report");
    let pool = StaticModelPool { entries };
    let (pool, cfg, trace) = match &policy {
        Some((eff, src)) => apply_policy(pool, RouterConfig::default(), eff, src)?,
        None => (pool, RouterConfig::default(), json!({ "policy_source": "default" })),
    };
    let node = turn_node(&call_type, &capability, &primitive, body["max_output_tokens"].as_u64())?;
    let plan = Router::new(&pool, &cfg)
        .route(&node, &BudgetLedger { remaining_cost_units: f64::MAX, remaining_wall_ms: None })
        .map_err(|e| e.to_string())?;
    let chosen = pool.entries.iter().find(|e| e.backend_id == plan.backend_id);
    let x = plan.extensions.clone().unwrap_or_default();
    Ok(json!({
        "object": "kernel.turn_route",
        "call_type": call_type,
        "gen_class": x.get("x-gen_class").cloned().unwrap_or(Value::Null),
        "class_for_call_type": classes::call_type_class(&call_type),
        "max_output_tokens": x.get("x-max_output_tokens").cloned().unwrap_or(Value::Null),
        "backend_id": plan.backend_id,
        // Est. cost per 1k tokens of the routed backend (pool units) — the
        // cost ledger's savings figure compares it with the incumbent's.
        "estimated_cost": chosen.map(|e| e.cost),
        "plan": plan,
        "routing": trace,
    }))
}

pub async fn turn_route(State(st): State<Arc<AppState>>, Extension(u): Extension<AuthUser>, Json(body): Json<Value>) -> KRes {
    let entries: Vec<PoolEntry> = serde_json::from_value(body["entries"].clone())
        .map_err(|e| KErr::bad(format!("entries: {e}")))?;
    if entries.is_empty() {
        return Err(KErr::bad("entries: empty pool"));
    }
    let scope = body["scope"].as_str().map(str::to_string)
        .or_else(|| u.organization_id.as_deref().filter(|o| !o.is_empty()).map(|o| format!("org:{o}")));
    let policy = match scope {
        Some(s) => {
            let ch = chain(&s, &u, body["parents"].as_str())?;
            Some(super::routing_policy::resolve(&st.db, &ch)?)
        }
        None => None,
    };
    route_turn(entries, &body, policy).map(Json).map_err(KErr::bad)
}
