//! WP-C3b: the `computer:` effect connector (COMPUTER_USE).
//!
//! Drives the run owner's own computer through the computer-use paths the
//! app already has, with no new auth path:
//! * `computer:local` → the computer-use gateway (ACU) the ACI routes use
//!   (`AppConfig::acu_url()`, `POST /v1/computer-use/execute`, mode `direct`);
//! * `computer:<computer id>` → the unified control plane
//!   (`computer_control::execute_computer_tool`), scoped to the owner's
//!   computers (another user's id is "not found").
//!
//! The executor calls [`dispatch`] only inside P1's fenced, idempotent effect
//! path and only after the POLICY node authorized the write set. A
//! consequential action (anything not read-only, by the ACI taxonomy) also
//! needs a human approval first: [`dispatch`] refuses it unless `approved`,
//! and the hash-bound ACI grant it then needs is issued per action here.

use crate::bot_desktop_input::{KeyboardInput, MouseInput, ShellInput};
use crate::computer_control::{classify_control_action, control_action_descriptor, execute_computer_tool, ComputerControlAction};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

/// Gateway kinds that only observe (no approval needed). Everything else is
/// consequential, fail-safe.
const READ_ONLY: &[&str] = &[
    "screenshot", "observe", "inspect", "extract", "ax_snapshot", "find_elements", "cursor_position", "wait", "zoom",
    "scroll", "hover", "mouse_move",
];
const MAX_ACTIONS: usize = 16;

#[cfg(test)]
pub(crate) static ACU_URL_OVERRIDE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn acu_base(st: &crate::AppState) -> String {
    #[cfg(test)]
    if let Some(u) = ACU_URL_OVERRIDE.lock().unwrap().clone() { return u; }
    st.config.acu_url().trim_end_matches('/').to_string()
}

/// Prompt suffix for the node that proposes the action (C02).
pub const ACTION_HINT: &str = "\n\nAnswer with JSON only: {\"actions\": [{\"kind\": \"screenshot\"|\"click\"|\"type\"|\"key\"|\"scroll\"|\"mouse\"|\"keyboard\"|\"shell\", \"target\": {...}, \"input\": {...}}]} (at most 16 actions).";

/// The proposed actions from the candidate text: a JSON object with
/// `actions`, an array, or one action object (fenced code allowed).
pub fn parse_actions(text: &str) -> Result<Vec<Value>> {
    let (a, b) = (text.find(['{', '[']), text.rfind(['}', ']']));
    let (Some(a), Some(b)) = (a, b) else { bail!("no computer action in the proposal (fail closed)") };
    let v: Value = serde_json::from_str(&text[a..=b]).map_err(|e| anyhow!("computer action is not valid JSON: {e}"))?;
    let list = match v {
        Value::Array(l) => l,
        Value::Object(ref o) if o.contains_key("actions") => o["actions"].as_array().cloned().unwrap_or_default(),
        o @ Value::Object(_) => vec![o],
        _ => vec![],
    };
    if list.is_empty() || list.len() > MAX_ACTIONS { bail!("a computer step takes 1..={MAX_ACTIONS} actions, got {}", list.len()); }
    for x in &list {
        if x["kind"].as_str().is_none_or(|k| k.trim().is_empty()) { bail!("every computer action needs a `kind`"); }
    }
    Ok(list)
}

/// The control-plane form of an action (cloud/paired computers).
fn control_action(a: &Value) -> Result<ComputerControlAction> {
    let input = a.get("input").cloned().unwrap_or(json!({}));
    Ok(match a["kind"].as_str().unwrap_or_default() {
        "screenshot" => ComputerControlAction::Screenshot,
        "mouse" => ComputerControlAction::Mouse(serde_json::from_value::<MouseInput>(input)?),
        "keyboard" => ComputerControlAction::Keyboard(serde_json::from_value::<KeyboardInput>(input)?),
        "shell" => ComputerControlAction::Shell(serde_json::from_value::<ShellInput>(input)?),
        k @ ("click" | "double_click" | "right_click") => {
            let act = match k { "click" => "click", "double_click" => "doubleclick", _ => "rightclick" };
            let mut m = input.clone();
            m["action"] = json!(act);
            for c in ["x", "y"] { if m.get(c).is_none() { if let Some(v) = a["target"].get(c) { m[c] = v.clone(); } } }
            ComputerControlAction::Mouse(serde_json::from_value::<MouseInput>(m)?)
        }
        "type" => ComputerControlAction::Keyboard(KeyboardInput { action: "type".into(),
            text: input["text"].as_str().or(a["text"].as_str()).map(String::from), key: None }),
        "key" => ComputerControlAction::Keyboard(KeyboardInput { action: "key".into(), text: None,
            key: input["key"].as_str().or(a["key"].as_str()).map(String::from) }),
        k => bail!("action `{k}` is not supported on this computer"),
    })
}

/// Does this action need a human approval before it runs?
pub fn consequential(a: &Value) -> bool {
    let k = a["kind"].as_str().unwrap_or_default();
    if READ_ONLY.contains(&k) { return false; }
    match control_action(a) {
        Ok(c) if matches!(k, "mouse" | "keyboard" | "shell") => classify_control_action(&c).requires_confirmation(),
        _ => true,
    }
}

/// Stable hash of one step's actions on one target (approval binding).
pub fn action_hash(target: &str, actions: &[Value]) -> String {
    crate::aci_approvals::hash_action_payload(&json!({ "route": "agency.computer", "target": target, "actions": actions }))
}

/// Prefix of the refusal when a person (or another agent) holds control of
/// this Mac. Callers that answer over HTTP map it to 423 Locked, the same
/// status cloud computers give.
pub const LOCKED_PREFIX: &str = "423 computer_controlled_elsewhere";

/// The agent's control-lease holder on the owner's own Mac. One holder per
/// owner, so consecutive steps of a run renew the same lease.
fn agent_holder(owner: &str) -> crate::computer_control_lease::Holder {
    crate::computer_control_lease::Holder {
        kind: crate::computer_control_lease::HolderKind::Agent,
        id: format!("agency:{owner}"),
        label: Some("Agent".into()),
        device_id: None,
    }
}

/// Agent input on this Mac takes the control lease first, the same rule as
/// a person's input (`computer_routes::this_device_input_gate`) and cloud
/// computers. A person who took over keeps control: the agent gets 423.
async fn this_mac_agent_lease(st: &crate::AppState, owner: &str) -> Result<()> {
    let (db, owner) = (st.db.clone(), owner.to_string());
    let verdict = tokio::task::spawn_blocking(move || -> Result<std::result::Result<(), crate::computer_control_lease::Lease>> {
        let conn = db.connect()?;
        let mut stmt = conn.prepare(
            "SELECT id FROM computers WHERE kind = 'local' AND provider = ?1 AND owner_type = 'user'
             AND owner_id = ?2 AND status != 'deleted'",
        )?;
        let ids = stmt
            .query_map(rusqlite::params![crate::computer_routes::THIS_DEVICE_PROVIDER, owner], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        let now = chrono::Utc::now().timestamp();
        match crate::computer_control_lease::take_all_for_agent(&conn, &ids, &agent_holder(&owner), now) {
            Ok(_) => Ok(Ok(())),
            Err(crate::computer_control_lease::TakeError::Held(lease)) => Ok(Err(lease)),
            Err(crate::computer_control_lease::TakeError::Db(e)) => Err(anyhow!(e)),
        }
    })
    .await
    .map_err(|e| anyhow!("control lease check failed: {e}"))??;
    verdict.map_err(|lease| {
        anyhow!(
            "{LOCKED_PREFIX}: {} is controlling this computer. The agent can act again when they hand control back.",
            lease.holder.label.unwrap_or_else(|| "Someone else".into())
        )
    })
}

/// Run the actions on `target`, on behalf of `owner`. `key` is the effect's
/// idempotency key (sent as the gateway run id, so the gateway can dedupe too).
pub async fn dispatch(st: &crate::AppState, owner: &str, target: &str, actions: &[Value], approved: bool, key: &str) -> Result<String> {
    if actions.iter().any(consequential) && !approved {
        bail!("approval required: consequential computer action without an approval (refused)");
    }
    let digest = allternit_factory_engine::receipts::jcs::sha256_tagged(serde_json::to_string(actions)?.as_bytes());
    if target == "local" {
        if actions.iter().any(consequential) {
            this_mac_agent_lease(st, owner).await?;
        }
        let run_id = format!("agency-{}", &allternit_factory_engine::receipts::jcs::sha256_tagged(key.as_bytes()).replace("sha256:", "")[..24]);
        let body = json!({ "mode": "direct", "actions": actions, "run_id": run_id, "session_id": run_id, "target_scope": "desktop" });
        let resp = reqwest::Client::new().post(format!("{}/v1/computer-use/execute", acu_base(st)))
            .timeout(std::time::Duration::from_secs(120)).json(&body).send().await
            .map_err(|e| anyhow!("computer-use gateway unreachable: {e}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() || !matches!(v["status"].as_str(), Some("completed" | "succeeded" | "success")) {
            // The gateway answers 200 with a run-level `status` and per-action
            // outcomes; the top-level `error` is usually null, so surface the
            // per-action errors before falling back to the whole body.
            let why = v["error"].as_str().map(str::to_string)
                .or_else(|| v["result"]["actions"].as_array().and_then(|acts| {
                    let errs: Vec<String> = acts.iter().filter_map(|a| a["error"].as_str()
                        .map(|e| format!("{}: {e}", a["kind"].as_str().unwrap_or("action")))).collect();
                    if errs.is_empty() { None } else { Some(errs.join("; ")) }
                }))
                .unwrap_or_else(|| v.to_string());
            bail!("computer action failed ({status}): {why}");
        }
        return Ok(format!("computer:local:{run_id}:{digest}"));
    }
    if target.is_empty() || target.contains(['/', ' ']) { bail!("bad computer id `{target}`"); }
    for a in actions {
        let c = control_action(a)?;
        let grant = (approved && classify_control_action(&c).requires_confirmation())
            .then(|| crate::aci_approvals::GRANTS.issue(owner, &crate::aci_approvals::hash_action_payload(&control_action_descriptor(&c))));
        execute_computer_tool(st, owner, target, c, grant.as_deref()).await
            .map_err(|(s, b)| anyhow!("computer action refused ({s}): {b}"))?;
    }
    Ok(format!("computer:{target}:{digest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_classifies_actions() {
        let a = parse_actions("```json\n{\"actions\":[{\"kind\":\"screenshot\"},{\"kind\":\"click\",\"target\":{\"x\":1,\"y\":2}}]}\n```").unwrap();
        assert_eq!(a.len(), 2);
        assert!(!consequential(&a[0]), "observing is not consequential");
        assert!(consequential(&a[1]), "a click needs approval");
        assert!(consequential(&json!({ "kind": "shell", "input": { "command": ["rm", "-rf", "/tmp/x"] } })));
        assert!(consequential(&json!({ "kind": "launch_missiles" })), "unknown kinds are consequential (fail safe)");
        assert!(parse_actions("scripted output of C02").is_err(), "prose is not an action");
        assert!(parse_actions("[]").is_err());
        assert_eq!(action_hash("local", &a), action_hash("local", &a));
        assert_ne!(action_hash("local", &a), action_hash("cmp_2", &a));
    }

    /// G2: opt-in live test — drives `computer:local` through the REAL
    /// computer-use gateway (`ALLTERNIT_ACU_URL`, default the Desktop's
    /// `http://127.0.0.1:8760`) with READ-ONLY actions only. Never sends
    /// click/type/key/shell. Run:
    /// `ALLTERNIT_LIVE_COMPUTER_TEST=1 cargo test -p allternit-api --lib -- --ignored --exact agency_api::effects::computer::tests::live_local_read_only_against_the_real_gateway --nocapture`
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "live: drives the real computer-use gateway (ALLTERNIT_LIVE_COMPUTER_TEST=1)"]
    async fn live_local_read_only_against_the_real_gateway() {
        if std::env::var("ALLTERNIT_LIVE_COMPUTER_TEST").as_deref() != Ok("1") {
            eprintln!("ALLTERNIT_LIVE_COMPUTER_TEST!=1; skipping");
            return;
        }
        let t = crate::agency_api::tests::setup().await;
        let url = std::env::var("ALLTERNIT_ACU_URL").unwrap_or_else(|_| "http://127.0.0.1:8760".into());
        let actions = vec![
            json!({ "kind": "screenshot" }),
            json!({ "kind": "observe" }),
            json!({ "kind": "cursor_position" }),
        ];
        for a in &actions {
            assert!(!consequential(a), "the live test is read-only, `{a}` is not");
        }
        *ACU_URL_OVERRIDE.lock().unwrap() = Some(url.clone());
        let r = dispatch(&t.st, "u1", "local", &actions, false, "g2-live-readonly").await;
        // The run-level contract is dispatch's; also show WHICH adapter
        // claimed each read-only action (a "none" adapter_id means the
        // gateway never routed the action — the regression this test pins).
        let probe = reqwest::Client::new().post(format!("{}/v1/computer-use/execute", url.trim_end_matches('/')))
            .json(&json!({ "mode": "direct", "actions": actions, "run_id": "g2-live-readonly-probe",
                           "session_id": "g2-live-readonly-probe", "target_scope": "desktop" }))
            .timeout(std::time::Duration::from_secs(120)).send().await;
        *ACU_URL_OVERRIDE.lock().unwrap() = None;
        match probe {
            Ok(resp) => {
                let v: Value = resp.json().await.unwrap_or(Value::Null);
                for a in v["result"]["actions"].as_array().cloned().unwrap_or_default() {
                    eprintln!("LIVE COMPUTER per-action: kind={} status={} adapter_id={}",
                        a["kind"], a["status"], a["result"]["adapter_id"]);
                    assert_ne!(a["result"]["adapter_id"], "none", "{} was not routed to an adapter", a["kind"]);
                }
            }
            Err(e) => eprintln!("LIVE COMPUTER per-action probe failed against {url}: {e}"),
        }
        match &r {
            Ok(v) => eprintln!("LIVE COMPUTER TEST OK against {url}: {v}"),
            Err(e) => eprintln!("LIVE COMPUTER TEST FAILED against {url}: {e}"),
        }
        let Ok(v) = r else { panic!("live computer test against {url} failed") };
        assert!(v.starts_with("computer:local:"), "{v}");
    }
}
