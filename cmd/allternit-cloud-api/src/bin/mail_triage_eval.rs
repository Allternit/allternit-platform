//! Measures `routes::mail_triage` against the labelled set in `evals/triage/emails.jsonl`, running the
//! same functions the server uses. Writes `evals/triage/results-<model>.md` (or `results-rules-only.md`).
//!
//!   cargo run -p allternit-cloud-api --features triage-eval --bin mail-triage-eval -- --rules-only
//!   cargo run -p allternit-cloud-api --features triage-eval --bin mail-triage-eval
//!
//! Model mode calls the Allternit model router's OpenAI-compatible `POST {base}/chat/completions`
//! (metered). Environment:
//!   TRIAGE_EVAL_MODEL, then BOT_MODEL   the model name sent to the router (required in model mode)
//!   TRIAGE_EVAL_BASE_URL                default https://api.allternit.com/v1
//!   TRIAGE_EVAL_API_KEY, then ALLTERNIT_API_KEY   bearer key (required in model mode)

use allternit_cloud_api::routes::mail_triage::{rule_verdict, triage, Message, Person, TriageResult, Verdict};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::pin::Pin;

// The morning the set was written for.
const NOW: &str = "2026-10-22T14:30:00Z";

struct Row {
    id: String,
    subject: String,
    label: String,
    why: String,
    result: Result<Option<TriageResult>, String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 { "n/a".into() } else { format!("{:.1}%", 100.0 * n as f64 / d as f64) }
}

fn cell(s: &str) -> String {
    s.replace('|', "/").replace('\n', " ")
}

#[tokio::main]
async fn main() {
    let rules_only = std::env::args().any(|a| a == "--rules-only");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/triage");
    let model = env("TRIAGE_EVAL_MODEL").or_else(|| env("BOT_MODEL"));
    if !rules_only && model.is_none() {
        eprintln!("Set TRIAGE_EVAL_MODEL or BOT_MODEL, or pass --rules-only.");
        std::process::exit(2);
    }
    let model_name = if rules_only { "none".to_string() } else { model.clone().unwrap() };
    let base = env("TRIAGE_EVAL_BASE_URL").unwrap_or_else(|| "https://api.allternit.com/v1".into());
    let key = env("TRIAGE_EVAL_API_KEY").or_else(|| env("ALLTERNIT_API_KEY"));
    if !rules_only && key.is_none() {
        eprintln!("Set TRIAGE_EVAL_API_KEY or ALLTERNIT_API_KEY, or pass --rules-only.");
        std::process::exit(2);
    }

    let text = std::fs::read_to_string(dir.join("emails.jsonl")).expect("read emails.jsonl");
    let set: Vec<(Value, Message)> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("emails.jsonl line is JSON");
            let m = Message::from_json(&v).expect("emails.jsonl line has id and from");
            (v, m)
        })
        .collect();

    let person = Person {
        name: "Maya Okafor".into(),
        email: "maya@marlowceramics.com".into(),
        about: Some("Runs Marlow Ceramics, a small studio that makes mugs and tableware for shops such as Hearth & Co.".into()),
    };

    let client = reqwest::Client::new();
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let model_for_call = model.clone().unwrap_or_default();
    let key_for_call = key.clone().unwrap_or_default();
    let complete = move |prompt: String| -> Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>> {
        let (client, url, model, key) = (client.clone(), url.clone(), model_for_call.clone(), key_for_call.clone());
        Box::pin(async move {
            let res = client
                .post(&url)
                .bearer_auth(&key)
                .json(&json!({ "model": model, "temperature": 0, "messages": [{ "role": "user", "content": prompt }] }))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = res.status();
            let body: Value = res.json().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {body}"));
            }
            body["choices"][0]["message"]["content"].as_str().map(str::to_string).ok_or_else(|| "no message content in the reply".to_string())
        })
    };

    let mut rows: Vec<Row> = Vec::new();
    for (v, m) in &set {
        let result = if rules_only { Ok(rule_verdict(m)) } else { triage(m, &person, &model_name, NOW, &complete).await.map(Some) };
        rows.push(Row {
            id: m.id.clone(),
            subject: m.subject.clone(),
            label: v["label"].as_str().unwrap_or("").to_string(),
            why: v["why"].as_str().unwrap_or("").to_string(),
            result,
        });
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));

    let decided: Vec<(&Row, &TriageResult)> = rows.iter().filter_map(|r| r.result.as_ref().ok().and_then(|x| x.as_ref()).map(|x| (r, x))).collect();
    let count = |label: &str, verdict: Verdict| decided.iter().filter(|(r, x)| r.label == label && x.verdict == verdict).count();
    let (tp, fp) = (count("needs_you", Verdict::NeedsYou), count("can_wait", Verdict::NeedsYou));
    let (fneg, tn) = (count("needs_you", Verdict::CanWait), count("can_wait", Verdict::CanWait));
    let by_rule: Vec<_> = decided.iter().filter(|(_, x)| x.decided_by.starts_with("rule:")).collect();
    let rule_errors = by_rule.iter().filter(|(r, _)| r.label != "can_wait").count();
    let undecided = rows.len() - decided.len();
    let total_needs = set.iter().filter(|(v, _)| v["label"] == "needs_you").count();

    let mut out: Vec<String> = vec![
        format!("# Triage eval{}", if rules_only { " (rules only)" } else { "" }),
        String::new(),
        format!("{} labelled messages ({} needs_you, {} can_wait). Model: {}.", set.len(), total_needs, set.len() - total_needs, model_name),
        String::new(),
        "| Measure | Value |".into(),
        "| --- | --- |".into(),
        format!("| Recall for needs_you | {} ({} of {} that needed her were flagged) |", pct(tp, tp + fneg), tp, tp + fneg),
        format!("| Precision for needs_you | {} ({} of {} flagged were right) |", pct(tp, tp + fp), tp, tp + fp),
        format!("| Accuracy | {} of {} decided |", pct(tp + tn, decided.len()), decided.len()),
        format!("| Decided by rules, no model call | {} of {}, {} wrong |", by_rule.len(), set.len(), rule_errors),
        format!("| Not decided ({}) | {} |", if rules_only { "left for the model" } else { "model error" }, undecided),
        String::new(),
        "## Confusion".into(),
        String::new(),
        "| labelled \\ verdict | needs_you | can_wait |".into(),
        "| --- | --- | --- |".into(),
        format!("| needs_you | {tp} | {fneg} |"),
        format!("| can_wait | {fp} | {tn} |"),
        String::new(),
        "## Disagreements".into(),
        String::new(),
    ];
    let wrong: Vec<_> = decided.iter().filter(|(r, x)| x.verdict.as_str() != r.label).collect();
    if wrong.is_empty() {
        out.push("None.".into());
    }
    for (r, x) in wrong {
        out.push(format!("- **{}** \"{}\": labelled {}, got {} by {}. Its reason: \"{}\". Label's reason: \"{}\"", r.id, r.subject, r.label, x.verdict.as_str(), x.decided_by, x.reason, r.why));
    }
    let errors: Vec<_> = rows.iter().filter_map(|r| r.result.as_ref().err().map(|e| (r, e))).collect();
    if !errors.is_empty() {
        out.push(String::new());
        out.push("## Errors".into());
        for (r, e) in errors {
            out.push(format!("- **{}**: {}", r.id, e));
        }
    }
    out.extend(["".into(), "## Every verdict".into(), "".into(), "| id | label | verdict | decided by | reason |".into(), "| --- | --- | --- | --- | --- |".into()]);
    for r in &rows {
        let (verdict, by, reason) = match &r.result {
            Ok(Some(x)) => (x.verdict.as_str().to_string(), x.decided_by.clone(), x.reason.clone()),
            Ok(None) => ("-".into(), "-".into(), "left for the model".into()),
            Err(e) => ("-".into(), "-".into(), e.clone()),
        };
        out.push(format!("| {} | {} | {} | {} | {} |", r.id, r.label, verdict, cell(&by), cell(&reason)));
    }
    out.push(String::new());

    let file = if rules_only { "results-rules-only.md".to_string() } else { format!("results-{}.md", model_name.replace('/', "-")) };
    std::fs::write(dir.join(&file), out.join("\n")).expect("write results");
    println!("{}", out[..12].join("\n"));
    println!("\nWrote evals/triage/{file}");
}
