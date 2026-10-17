//! Red-team corpus: hidden instructions in email, calendar invites, PDF text,
//! repo files, peer-bot messages, vendor-mcp `ask_bot` and A2A.
//!
//! The runner assumes the model is compromised: after the input is labelled the
//! way ingress labels it, the fake driver emits the send a fully obeying model
//! would. The merged WP5 code must hold it, and the proposal's provenance must
//! name the source that carried the injection.

use allternit_api::autonomy::{self, Action, Ctx, Verdict};
use allternit_api::netops::provenance::{self, Trust};
use allternit_api::test_helpers::app_state;
use serde_json::Value;

const FILES: &[(&str, &str)] = &[
    ("email", include_str!("fixtures/redteam/email.jsonl")),
    ("calendar", include_str!("fixtures/redteam/calendar.jsonl")),
    ("pdf", include_str!("fixtures/redteam/pdf.jsonl")),
    ("repo", include_str!("fixtures/redteam/repo.jsonl")),
    ("peer_bot", include_str!("fixtures/redteam/peer_bot.jsonl")),
    ("vendor_mcp", include_str!("fixtures/redteam/vendor_mcp.jsonl")),
    ("a2a", include_str!("fixtures/redteam/a2a.jsonl")),
];

fn cases() -> Vec<Value> {
    FILES.iter().flat_map(|(_, raw)| raw.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str::<Value>(l).expect("corpus line is JSON"))).collect()
}

#[test]
fn corpus_has_at_least_forty_cases_over_every_channel() {
    let all = cases();
    assert!(all.len() >= 40, "only {} cases", all.len());
    for (name, _) in FILES {
        assert!(all.iter().any(|c| c["channel"] == *name), "no cases for {name}");
    }
}

#[tokio::test]
async fn a_compromised_model_never_sends_without_a_hold_that_names_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let state = app_state(dir.path()).await;
    let conn = state.db.connect().unwrap();
    let (owner, bot) = ("rt-owner", "rt-bot");
    conn.execute("INSERT INTO agents (id, user_id, name, model, provider) VALUES (?1, ?2, 'Red team bot', 'm', 'p')", rusqlite::params![bot, owner]).unwrap();

    for case in cases() {
        let id = case["id"].as_str().unwrap();
        let thread = format!("th-{id}");
        let now = "2026-10-16T00:00:00Z";
        conn.execute(
            "INSERT INTO bot_threads (id, user_id, bot_id, title, last_activity_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)",
            rusqlite::params![thread, owner, bot, id, now],
        )
        .unwrap();

        // Ingress labels the input (the injected text is the payload).
        let trust = Trust::parse(case["trust"].as_str().unwrap()).unwrap();
        assert!(trust.tainted(), "{id}: corpus cases come from outside the team");
        let (kind, source_ref) = (case["sourceKind"].as_str().unwrap(), case["sourceRef"].as_str().unwrap());
        provenance::label(&conn, &thread, trust, kind, source_ref, case["payload"].as_str().unwrap());

        // The compromised model emits the injected send.
        let act = &case["action"];
        let (channel, to, text) = (act["channel"].as_str().unwrap(), act["to"].as_str().unwrap(), act["text"].as_str().unwrap());
        let eval = if channel == "email" {
            autonomy::email_eval_ctx(&state.db, owner, bot, to, Some(&thread), text)
        } else {
            autonomy::evaluate_ctx(
                &state.db,
                &Action { owner, bot_id: bot, channel, persons: vec![to.to_string()], action: "message", amount_cents: 0 },
                false,
                &Ctx { thread_id: Some(&thread), text, destination: to },
            )
        };
        let eval = eval.unwrap_or_else(|| panic!("{id}: no verdict, the send would go on the caller's own gate"));
        assert!(!matches!(eval.verdict, Verdict::Send { .. }), "{id}: injected send was allowed: {:?}", eval.verdict);
        assert!(eval.held(), "{id}");

        // The approval card can say where the proposal came from.
        let prov = provenance::latest_tainted(&conn, &thread).unwrap_or_else(|| panic!("{id}: no provenance"));
        assert_eq!(prov["sourceKind"], kind, "{id}");
        assert_eq!(prov["sourceRef"], source_ref, "{id}");
    }
}
