//! Golden replay: a redacted Grok ↔ A:// hand-off, replayed through
//! `netops::hops` (admit / advance) and the thread timeline.
//!
//! The fixture is synthesized from the shapes in `agent_hops` (no personal data).
//! Expect one trace id, every receipt at `answered`, no loop-guard hold, and a
//! span tree with the reply nested under the ask.

use std::collections::HashMap;

use allternit_api::netops::hops::{self, Admit, HopIn};
use allternit_api::test_helpers::app_state;
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/golden/grok-allternit-2026-10.jsonl");

#[tokio::test]
async fn grok_to_allternit_handoff_replays_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state(dir.path()).await;
    let conn = state.db.connect().unwrap();

    let mut ids: HashMap<String, String> = HashMap::new();
    let mut headers: HashMap<String, String> = HashMap::new();
    let mut traces: Vec<String> = vec![];
    let mut thread = String::new();

    for line in FIXTURE.lines().filter(|l| !l.trim().is_empty()) {
        let s: Value = serde_json::from_str(line).unwrap();
        match s["step"].as_str().unwrap() {
            "admit" => {
                thread = s["thread"].as_str().unwrap().to_string();
                let hop = HopIn {
                    owner: "golden-owner".into(),
                    via: s["via"].as_str().unwrap().into(),
                    from_kind: s["fromKind"].as_str().unwrap().into(),
                    from_id: s["fromId"].as_str().unwrap().into(),
                    to_bot_id: s["to"].as_str().unwrap().into(),
                    thread_id: Some(thread.clone()),
                    idempotency_key: s["key"].as_str().map(str::to_string),
                    traceparent: s["parent"].as_str().map(|p| headers[p].clone()),
                    text: s["text"].as_str().unwrap().into(),
                };
                match hops::admit(&conn, &hop) {
                    Admit::Go { hop_id, trace, .. } => {
                        traces.push(trace.trace_id.clone());
                        headers.insert(s["name"].as_str().unwrap().into(), trace.header());
                        ids.insert(s["name"].as_str().unwrap().into(), hop_id);
                    }
                    other => panic!("hand-off was not admitted: {other:?}"),
                }
            }
            "advance" => hops::advance(&conn, &ids[s["hop"].as_str().unwrap()], s["state"].as_str().unwrap(), None, Some(&thread), None),
            other => panic!("unknown step {other}"),
        }
    }

    traces.dedup();
    assert_eq!(traces.len(), 1, "one trace id across the hand-off");
    for (name, id) in &ids {
        assert_eq!(hops::receipt(&conn, id)["state"], "answered", "{name}");
    }
    let held: i64 = conn.query_row("SELECT COUNT(*) FROM agent_hops WHERE state = 'held'", [], |r| r.get(0)).unwrap();
    assert_eq!(held, 0, "no loop-guard hold");

    let tl = hops::timeline(&conn, &thread);
    assert_eq!(tl["traceIds"].as_array().unwrap().len(), 1);
    let roots = tl["spans"].as_array().unwrap();
    assert_eq!(roots.len(), 1, "the ask is the only root");
    assert_eq!(roots[0]["kind"], "hop");
    assert_eq!(roots[0]["state"], "answered");
    let kids = roots[0]["children"].as_array().unwrap();
    assert_eq!(kids.len(), 1, "the reply nests under the ask");
    assert_eq!(kids[0]["from"]["id"], "bot-allternit");
    assert_eq!(kids[0]["state"], "answered");
}
