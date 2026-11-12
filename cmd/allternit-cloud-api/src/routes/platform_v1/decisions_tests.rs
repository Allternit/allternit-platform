//! Lane 1 decisions: request normaliser, question → node mapping, answer
//! mapping, and the fan-out against a stand-in agency node.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};

use super::*;

fn err_code(r: Result<DecisionRequest, PlatformError>) -> String {
    r.err().expect("expected an error").code
}

#[test]
fn normalizes_openai_shape() {
    let req = normalize(&json!({
        "input": [{ "role": "user", "content": [
            { "type": "input_text", "text": "Box arrived crushed." },
            { "type": "input_json", "json": { "order": 7 } }
        ]}],
        "instructions": "You triage support tickets.",
        "questions": [
            { "name": "is_damaged", "type": "predicate", "instructions": "Did it arrive damaged?" },
            { "name": "dept", "type": "choice", "instructions": "Which team?", "choices": [{ "value": "billing", "description": "Money" }, { "value": "shipping" }] },
            { "name": "severity", "type": "score", "instructions": "How bad?", "levels": [{ "label": "none" }, { "label": "high", "description": "Broken" }] }
        ],
        "latency_budget_ms": 2500,
        "allow_abstain": true,
        "metadata": { "ticket": "t_1" }
    }))
    .unwrap();
    assert_eq!(req.input.text, "Box arrived crushed.\n\n{\"order\":7}");
    assert_eq!(req.instructions, "You triage support tickets.");
    assert_eq!(req.budget_ms, 2500);
    assert!(req.allow_abstain);
    assert_eq!(req.metadata, json!({ "ticket": "t_1" }));
    let types: Vec<QType> = req.questions.iter().map(|q| q.qtype).collect();
    assert_eq!(types, vec![QType::Predicate, QType::Choice, QType::Score]);
    assert_eq!(req.questions[1].options[0], Opt { value: "billing".into(), description: Some("Money".into()) });
    assert_eq!(req.questions[2].options[1].value, "high");
}

#[test]
fn normalizes_google_shape() {
    let req = normalize(&json!({
        "input": [{ "text": "Refund please" }],
        "questions": {
            "is_refund": { "type": "noul", "instructions": "Is this a refund request?" },
            "dept": { "type": "choice", "instructions": "Which team?", "criteria": { "billing": "Money", "shipping": "Parcels" } },
            "urgency": { "type": "score", "instructions": "How urgent?", "criteria": [{ "label": "low" }, { "label": "mid" }, { "label": "high" }] },
            "tone": { "type": "score", "instructions": "How angry?", "criteria": { "10": "furious", "2": "calm" } }
        }
    }))
    .unwrap();
    assert_eq!(req.input.text, "Refund please");
    // Default budget: 5000 + 15 * 13 bytes + 3000 * 4 questions.
    assert_eq!(req.budget_ms, 17_195);
    let q = |n: &str| req.questions.iter().find(|q| q.name == n).unwrap().clone();
    assert_eq!(q("is_refund").qtype, QType::Predicate);
    let dept = q("dept");
    assert_eq!(dept.qtype, QType::Choice);
    assert!(dept.options.contains(&Opt { value: "shipping".into(), description: Some("Parcels".into()) }));
    let urgency: Vec<String> = q("urgency").options.into_iter().map(|o| o.value).collect();
    assert_eq!(urgency, vec!["low", "mid", "high"]);
    // Numeric keys sort numerically, not as strings ("10" > "2").
    let tone: Vec<String> = q("tone").options.into_iter().map(|o| o.value).collect();
    assert_eq!(tone, vec!["2", "10"]);
}

#[test]
fn level_objects_without_numbers_are_rejected() {
    // serde_json here does not keep member order, so named level keys have no order.
    let r = normalize(&json!({ "input": "x", "questions": { "s": { "type": "score", "instructions": "?", "criteria": { "low": "", "high": "" } } } }));
    assert_eq!(err_code(r), "unordered_levels");
}

#[test]
fn enforces_limits() {
    let pred = |name: &str| json!({ "name": name, "type": "predicate", "instructions": "?" });
    let seventeen: Vec<Value> = (0..17).map(|i| pred(&format!("q{i}"))).collect();
    assert_eq!(err_code(normalize(&json!({ "input": "x", "questions": seventeen }))), "invalid_question_count");
    let sixteen: Vec<Value> = (0..16).map(|i| pred(&format!("q{i}"))).collect();
    assert!(normalize(&json!({ "input": "x", "questions": sixteen })).is_ok());
    assert_eq!(err_code(normalize(&json!({ "input": "x", "questions": [] }))), "invalid_question_count");
    assert_eq!(err_code(normalize(&json!({ "input": "x", "questions": [pred("a"), pred("a")] }))), "duplicate_question_name");
    assert_eq!(
        err_code(normalize(&json!({ "input": "x", "questions": [{ "name": "c", "type": "choice", "instructions": "?", "choices": [{ "value": "only" }] }] }))),
        "invalid_choices"
    );
    let levels: Vec<Value> = (0..11).map(|i| json!({ "label": format!("l{i}") })).collect();
    assert_eq!(
        err_code(normalize(&json!({ "input": "x", "questions": [{ "name": "s", "type": "score", "instructions": "?", "levels": levels }] }))),
        "invalid_levels"
    );
    assert_eq!(err_code(normalize(&json!({ "input": "x".repeat(4 * 1024 + 1), "questions": [pred("a")] }))), "input_too_large");
    // Shared instructions count toward the same context cap as the input.
    assert_eq!(
        err_code(normalize(&json!({ "input": "x".repeat(3 * 1024), "instructions": "y".repeat(1024 + 1), "questions": [pred("a")] }))),
        "input_too_large"
    );
    // A question's own tail (instructions plus choice texts) is capped too.
    let long_choices: Vec<_> = (0..3).map(|i| json!({ "value": format!("c{i}"), "description": "d".repeat(1500) })).collect();
    assert_eq!(
        err_code(normalize(&json!({ "input": "x", "questions": [{ "name": "q", "type": "choice", "instructions": "Pick one.", "choices": long_choices }] }))),
        "question_too_large"
    );
    assert!(normalize(&json!({ "input": "x".repeat(4 * 1024), "questions": [pred("a")] })).is_ok());
    // The scorer has no vision: any image part, in either shape, is refused.
    let image = json!([{ "role": "user", "content": [{ "type": "input_text", "text": "hi" }, { "type": "input_image", "image_url": "data:image/png;base64,AA" }] }]);
    assert_eq!(err_code(normalize(&json!({ "input": image, "questions": [pred("a")] }))), "image_not_supported");
    let inline = json!([{ "inline_data": { "mime_type": "image/jpeg", "data": "QUJD" } }]);
    assert_eq!(err_code(normalize(&json!({ "input": inline, "questions": [pred("a")] }))), "image_not_supported");
    assert_eq!(err_code(normalize(&json!({ "input": "x", "questions": [pred("a")], "latency_budget_ms": 0 }))), "invalid_latency_budget");
    assert_eq!(
        err_code(normalize(&json!({ "input": "x", "questions": [{ "name": "a", "type": "predicate", "instructions": " " }] }))),
        "invalid_question_instructions"
    );
}

#[test]
fn default_budget_scales_with_input_and_questions() {
    let pred = |name: &str| json!({ "name": name, "type": "predicate", "instructions": "?" });
    let qs = |n: usize| (0..n).map(|i| pred(&format!("q{i}"))).collect::<Vec<_>>();
    // Small requests get the 10 s floor.
    assert_eq!(normalize(&json!({ "input": "x", "questions": qs(1) })).unwrap().budget_ms, 10_000);
    // 5000 + 15 * 4096 + 3000 * 16 = 114_440.
    assert_eq!(normalize(&json!({ "input": "x".repeat(4096), "questions": qs(16) })).unwrap().budget_ms, 114_440);
    // Shared instructions count: the scorer reads them as part of the context.
    let with_instr = normalize(&json!({ "input": "x".repeat(1000), "instructions": "y".repeat(998), "questions": qs(3) })).unwrap();
    assert_eq!(with_instr.budget_ms, 5_000 + 15 * 2_000 + 9_000);
    // The caller's value wins.
    assert_eq!(normalize(&json!({ "input": "x".repeat(4096), "questions": qs(16), "latency_budget_ms": 3000 })).unwrap().budget_ms, 3000);
    assert_eq!(default_budget_ms(1_000_000, 16), 120_000);
}

#[test]
fn agency_body_is_detected() {
    assert!(is_agency_body(&json!({ "context": "x", "options": [{ "id": "a", "text": "A" }, { "id": "b", "text": "B" }] })));
    assert!(!is_agency_body(&json!({ "input": "x", "questions": [] })));
}

fn question(qtype: QType, options: &[(&str, Option<&str>)]) -> Question {
    Question {
        name: "q".into(),
        qtype,
        instructions: "Rate it".into(),
        options: options.iter().map(|(v, d)| Opt { value: v.to_string(), description: d.map(str::to_string) }).collect(),
    }
}

#[test]
fn predicate_maps_to_yes_no_verify() {
    let q = question(QType::Predicate, &[]);
    let (kind, options) = node_options(&q);
    assert_eq!(kind, "verify");
    assert_eq!(options, json!([{ "id": "yes", "text": "yes" }, { "id": "no", "text": "no" }]));
    let a = map_answer(&q, &json!({ "choice": "no", "probs": { "yes": 0.3, "no": 0.7 }, "confidence": 0.7, "abstained": false, "backend": "oracle" }));
    assert_eq!(a, json!({ "name": "q", "type": "predicate", "probability": 0.3, "confidence": 0.7, "backend": "oracle" }));
}

#[test]
fn choice_maps_values_and_probabilities() {
    let q = question(QType::Choice, &[("billing", Some("Money questions")), ("shipping", None)]);
    let (kind, options) = node_options(&q);
    assert_eq!(kind, "choice");
    assert_eq!(options, json!([{ "id": "billing", "text": "Money questions" }, { "id": "shipping", "text": "shipping" }]));
    let a = map_answer(&q, &json!({ "choice": "billing", "probs": { "billing": 0.96, "shipping": 0.04 }, "abstained": false, "backend": "local" }));
    assert_eq!(a["type"], "choice");
    assert_eq!(a["choice"], "billing");
    assert_eq!(a["probabilities"], json!([{ "value": "billing", "probability": 0.96 }, { "value": "shipping", "probability": 0.04 }]));
    assert_eq!(a["confidence"], json!(0.96));
    // A node choice outside the options falls back to the most likely value.
    let a = map_answer(&q, &json!({ "choice": null, "probs": { "billing": 0.2, "shipping": 0.8 }, "backend": "local" }));
    assert_eq!(a["choice"], "shipping");
}

#[test]
fn score_is_the_probability_weighted_level() {
    let q = question(QType::Score, &[("none", None), ("some", Some("A scratch")), ("high", Some("Broken"))]);
    let (kind, options) = node_options(&q);
    assert_eq!(kind, "score");
    assert_eq!(options, json!([{ "id": "0", "text": "none" }, { "id": "1", "text": "some: A scratch" }, { "id": "2", "text": "high: Broken" }]));
    let a = map_answer(&q, &json!({ "choice": "2", "probs": { "0": 0.1, "1": 0.3, "2": 0.6 }, "backend": "oracle" }));
    // 0.1*0 + 0.3*0.5 + 0.6*1 = 0.75
    assert_eq!(a["score"], json!(0.75));
    assert_eq!(a["confidence"], json!(0.6));
    assert_eq!(
        a["probabilities"],
        json!([
            { "value": 0, "label": "none", "probability": 0.1 },
            { "value": 1, "label": "some", "probability": 0.3 },
            { "value": 2, "label": "high", "probability": 0.6 }
        ])
    );
}

#[test]
fn abstention_is_a_refusal() {
    let q = question(QType::Predicate, &[]);
    let a = map_answer(&q, &json!({ "choice": null, "probs": { "yes": 0.5, "no": 0.5 }, "abstained": true, "backend": "local" }));
    assert_eq!(a, json!({ "name": "q", "type": "refusal", "reason": "abstained", "backend": "local" }));
}

#[test]
fn node_body_puts_shared_instructions_in_the_context_and_never_sets_backends() {
    let req = normalize(&json!({
        "input": [{ "role": "user", "content": [{ "type": "input_text", "text": "hi" }] }],
        "instructions": "Shared.",
        "questions": [
            { "name": "a", "type": "predicate", "instructions": "Own." },
            { "name": "b", "type": "predicate", "instructions": "Other." }
        ]
    }))
    .unwrap();
    let body = node_body(&req, &req.questions[0], "proj_1", 900);
    assert_eq!(body["question"], "Own.");
    assert_eq!(body["context"], "Shared.\n\nhi");
    // Every question of a request sends the same context, so the scorer reuses the prefix.
    assert_eq!(node_body(&req, &req.questions[1], "proj_1", 900)["context"], body["context"]);
    let bare = normalize(&json!({ "input": "hi", "questions": [{ "name": "a", "type": "predicate", "instructions": "Own." }] })).unwrap();
    assert_eq!(node_body(&bare, &bare.questions[0], "proj_1", 900)["context"], "hi");
    assert_eq!(body["project"], "proj_1");
    assert_eq!(body["latency_budget_ms"], 900);
    assert!(body.get("backends").is_none());
}

#[test]
fn summary_counts_answers_and_picks_the_common_backend() {
    let answers = vec![
        json!({ "type": "choice", "backend": "local" }),
        json!({ "type": "predicate", "backend": "oracle" }),
        json!({ "type": "score", "backend": "oracle" }),
        refusal("x", "unavailable"),
    ];
    assert_eq!(summarize(&answers), ("oracle".to_string(), 3));
    assert_eq!(summarize(&[refusal("x", "abstained")]), ("none".to_string(), 0));
}

#[test]
fn outcomes_are_checked_and_labels_translated() {
    let qs = vec![
        Question { name: "dept".into(), ..question(QType::Choice, &[("a", None), ("b", None)]) },
        Question { name: "sev".into(), ..question(QType::Score, &[("low", None), ("high", None)]) },
    ];
    let out = parse_outcomes(&json!({ "outcomes": { "sev": { "status": "success", "label": "high" } } }), &qs).unwrap();
    assert_eq!(out["sev"], json!({ "status": "success", "label": "high" }));
    assert_eq!(node_label(&qs[1], "high"), "1");
    assert_eq!(node_label(&question(QType::Predicate, &[]), "true"), "yes");
    let code = |b: Value| parse_outcomes(&b, &qs).err().unwrap().code;
    assert_eq!(code(json!({ "outcomes": { "nope": { "status": "success" } } })), "unknown_question");
    assert_eq!(code(json!({ "outcomes": { "dept": { "status": "great" } } })), "invalid_outcome_status");
    assert_eq!(code(json!({ "outcomes": {} })), "outcomes_required");
    assert_eq!(code(json!({ "outcomes": { "dept": { "status": "success" } }, "extra": 1 })), "unknown_parameter");
}

#[test]
fn stored_questions_round_trip() {
    let q = question(QType::Score, &[("low", Some("fine")), ("high", None)]);
    assert_eq!(Question::from_stored(&q.to_json()), Some(q));
}

/// A stand-in agency node: answers by question text. "slow" sleeps past the
/// budget, "down" answers 503, anything else picks the first option.
async fn spawn_node(seen: Arc<Mutex<Vec<(Option<String>, Value)>>>) -> String {
    let app = Router::new().route(
        "/v1/decisions",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                let user = headers.get("x-allternit-user-id").and_then(|v| v.to_str().ok()).map(str::to_string);
                seen.lock().unwrap().push((user, body.clone()));
                let q = body["question"].as_str().unwrap_or("").to_string();
                if q.contains("slow") {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                if q.contains("down") {
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": { "message": "no backend" } })));
                }
                let first = body["options"][0]["id"].as_str().unwrap().to_string();
                let probs: serde_json::Map<String, Value> = body["options"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| {
                        let id = o["id"].as_str().unwrap().to_string();
                        let p = if id == first { 0.9 } else { 0.1 / (body["options"].as_array().unwrap().len() - 1) as f64 };
                        (id, json!(p))
                    })
                    .collect();
                (
                    axum::http::StatusCode::OK,
                    Json(json!({ "id": format!("dec_node_{}", body["question"].as_str().unwrap_or("")), "choice": first, "probs": probs, "confidence": 0.9, "abstained": false, "backend": "local" })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn fan_out_answers_each_question_within_the_budget() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let url = spawn_node(seen.clone()).await;
    let fwd = AgencyForward::new(url, Some("peer-secret".into()));
    let node = Node::from_forward(&fwd).unwrap();
    let req = normalize(&json!({
        "input": "Parcel was crushed",
        "questions": [
            { "name": "ok", "type": "choice", "instructions": "fast", "choices": [{ "value": "a" }, { "value": "b" }] },
            { "name": "late", "type": "predicate", "instructions": "slow" },
            { "name": "broken", "type": "predicate", "instructions": "down" },
            { "name": "sev", "type": "score", "instructions": "rate", "levels": [{ "label": "low" }, { "label": "high" }] }
        ],
        "latency_budget_ms": 800
    }))
    .unwrap();
    let started = std::time::Instant::now();
    let out = fan_out(&node, "proj_1", &req).await;
    assert!(started.elapsed() < Duration::from_secs(3), "budget not enforced: {:?}", started.elapsed());

    let answers: Vec<&Value> = out.iter().map(|(a, _)| a).collect();
    assert_eq!(answers[0]["choice"], "a");
    assert_eq!(out[0].1.as_deref(), Some("dec_node_fast"));
    assert_eq!(*answers[1], refusal("late", "budget_exhausted"));
    assert_eq!(*answers[2], refusal("broken", "unavailable"));
    assert_eq!(answers[3]["type"], "score");
    // p(low)=0.9, p(high)=0.1 → 0.1
    assert_eq!(answers[3]["score"], json!(0.1));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    for (user, body) in seen.iter() {
        assert_eq!(user.as_deref(), Some("platform:proj_1"));
        assert_eq!(body["project"], "proj_1");
        assert!(body.get("backends").is_none());
    }
}

#[test]
fn missing_node_secret_is_agency_not_configured() {
    let fwd = AgencyForward::new("http://127.0.0.1:1", None);
    let e = Node::from_forward(&fwd).err().unwrap();
    assert_eq!(e.code, "agency_not_configured");
    assert_eq!(e.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
}
