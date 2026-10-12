use super::backends::{parse_choice, Answer, DecisionBackend, Query};
use super::route;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

struct Fake {
    name: &'static str,
    probs: Vec<f64>,
    delay_ms: u64,
    vision: bool,
}

#[async_trait]
impl DecisionBackend for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn vision(&self) -> bool {
        self.vision
    }
    fn enabled(&self) -> bool {
        true
    }
    async fn decide(&self, _q: &Query<'_>) -> Result<Answer, String> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(Answer { probs: self.probs.clone(), abstain: false, detail: Value::Null })
    }
}

fn q(allow_abstain: bool, image: Option<&'static str>) -> Query<'static> {
    Query { kind: "element", context: "c", question: None, image, ids: vec!["a", "b"], texts: vec!["A", "B"], allow_abstain }
}

fn chain(local: Vec<f64>, oracle_delay: u64) -> Vec<Box<dyn DecisionBackend>> {
    vec![
        Box::new(Fake { name: "local", probs: local, delay_ms: 0, vision: false }),
        Box::new(Fake { name: "oracle", probs: vec![0.0, 1.0], delay_ms: oracle_delay, vision: true }),
    ]
}

#[tokio::test]
async fn confident_local_answer_stops_the_chain() {
    let r = route(&chain(vec![0.9, 0.1], 0), &q(true, None), 0.6, Duration::from_secs(1)).await;
    assert_eq!((r.choice, r.backend, r.escalated, r.abstained), (Some(0), Some("local"), false, false));
}

#[tokio::test]
async fn low_confidence_escalates_and_the_budget_caps_it() {
    // Escalates to the oracle, which overrides the local answer.
    let r = route(&chain(vec![0.55, 0.45], 0), &q(true, None), 0.6, Duration::from_secs(1)).await;
    assert_eq!((r.choice, r.backend, r.escalated), (Some(1), Some("oracle"), true));
    // The oracle is slower than the budget: the local answer stands, as an
    // abstention when allowed, as the best guess when not.
    let r = route(&chain(vec![0.55, 0.45], 500), &q(true, None), 0.6, Duration::from_millis(50)).await;
    assert_eq!((r.choice, r.backend, r.abstained, r.escalated), (None, Some("local"), true, true));
    assert_eq!(r.attempts[1]["error"], "latency budget spent");
    let r = route(&chain(vec![0.55, 0.45], 500), &q(false, None), 0.6, Duration::from_millis(50)).await;
    assert_eq!((r.choice, r.abstained), (Some(0), false));
}

#[tokio::test]
async fn image_skips_blind_backends() {
    let r = route(&chain(vec![0.9, 0.1], 0), &q(false, Some("data:image/png;base64,AA==")), 0.6, Duration::from_secs(1)).await;
    assert_eq!(r.backend, Some("oracle"));
    assert_eq!(r.attempts[0]["skipped"], "cannot see the image");
}

#[test]
fn oracle_replies_parse_only_to_known_ids() {
    let ids = ["el_7", "done", "__abstain__"];
    assert_eq!(parse_choice(r#"{"choice":"el_7"}"#, &ids), Some("el_7"));
    assert_eq!(parse_choice("Sure: {\"choice\": \"done\"}", &ids), Some("done"));
    assert_eq!(parse_choice("`__abstain__`", &ids), Some("__abstain__"));
    assert_eq!(parse_choice(r#"{"choice":"el_9"}"#, &ids), None);
}

/// A one-shot SystemOne server: answers `reply` and hands back the request body.
async fn systemone_stub(reply: Value) -> (String, tokio::sync::oneshot::Receiver<Value>) {
    use axum::{routing::post, Json, Router};
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let app = Router::new().route(
        "/v1/systemone",
        post(move |Json(body): Json<Value>| {
            let (tx, reply) = (tx.clone(), reply.clone());
            async move {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(body);
                }
                Json(reply)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), rx)
}

#[tokio::test]
async fn open_weights_speaks_systemone() {
    use super::backends::{backend, OpenWeights};
    let reply = serde_json::json!({
        "model": "LiquidAI/d1-3B",
        "answers": { "decision": { "choice": "b", "confidence": 0.82, "probabilities": { "a": 0.18, "b": 0.82 } } },
        "usage": { "input_tokens": 40, "output_tokens": 0 },
    });
    let (url, rx) = systemone_stub(reply).await;
    // SAFETY of env mutation: only this test reads ALLTERNIT_DECISIONS_OPEN_*.
    std::env::set_var("ALLTERNIT_DECISIONS_OPEN_URL", &url);
    let b = backend("open").expect("open backend is registered");
    assert!(b.enabled());
    let a = b.decide(&q(false, None)).await.unwrap();
    std::env::remove_var("ALLTERNIT_DECISIONS_OPEN_URL");
    assert_eq!(a.probs, vec![0.18, 0.82]);
    assert_eq!(a.detail["model"], "LiquidAI/d1-3B");
    let sent = rx.await.unwrap();
    assert_eq!(sent["model"], OpenWeights::DEFAULT_MODEL);
    assert_eq!(sent["state"], "c");
    assert_eq!(sent["questions"]["decision"]["type"], "choice");
    assert_eq!(sent["questions"]["decision"]["criteria"]["b"], "B");
    assert!(!OpenWeights.enabled());
}
