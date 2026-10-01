//! Internal batch helper (decision O11).
//!
//! Background, latency-insensitive jobs (weekly memory curation today)
//! submit their LLM calls as one batch through the gateway's batch provider
//! (`llm_gateway::batches`, the same provider the batch worker uses:
//! `NativeBatchProvider` when `ALLTERNIT_BATCH_NATIVE=1`, else
//! `HttpBatchProvider` at `ALLTERNIT_BATCH_PROVIDER_URL`) and poll for the
//! results. Each sub-request carries `x-allternit-batch-id` = an
//! `internal_<uuid>` id, so its usage rows are attributable.
//!
//! Flag: `ALLTERNIT_INTERNAL_BATCH=1` turns it on. Default OFF until it has
//! been live-checked against a configured batch provider (the native provider
//! needs `ALLTERNIT_BATCH_NATIVE_KEY`, a gateway virtual key). When it is off,
//! or the batch fails, callers fall back to direct calls.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::warn;

use crate::llm_gateway::batches::{BatchJobStatus, BatchProvider, HttpBatchProvider, NativeBatchProvider};

/// `ALLTERNIT_INTERNAL_BATCH=1|true` (default off).
pub fn enabled() -> bool {
    std::env::var("ALLTERNIT_INTERNAL_BATCH").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false)
}

/// The provider the gateway batch worker would use.
pub fn provider_from_config(config: &crate::config::AppConfig) -> Arc<dyn BatchProvider> {
    if std::env::var("ALLTERNIT_BATCH_NATIVE").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false) {
        Arc::new(NativeBatchProvider::from_config(config))
    } else {
        Arc::new(HttpBatchProvider::from_config(config))
    }
}

/// Submit `requests` (OpenAI chat-completion bodies) as one batch and poll
/// until it settles or `max_wait` passes. Returns one entry per request, in
/// order: the reply text, or why that request failed. `Err` means the batch
/// as a whole didn't run (callers fall back to direct calls).
pub async fn submit_and_poll(
    provider: &dyn BatchProvider,
    requests: &[Value],
    poll_every: Duration,
    max_wait: Duration,
) -> Result<Vec<Result<String, String>>, String> {
    let batch_id = format!("internal_{}", uuid::Uuid::new_v4().simple());
    let job = provider.submit(&batch_id, requests).await.map_err(|e| format!("{}: {}", e.code, e.message))?;
    let deadline = tokio::time::Instant::now() + max_wait;
    loop {
        match provider.poll(&job.id).await {
            Ok(st) => match st.status {
                BatchJobStatus::Completed => return Ok(align(requests.len(), st.results.unwrap_or_default())),
                BatchJobStatus::Failed | BatchJobStatus::Cancelled => return Err(format!("batch {} ended {:?}", job.id, st.status)),
                _ => {}
            },
            Err(e) if !e.is_transient => return Err(format!("{}: {}", e.code, e.message)),
            Err(e) => warn!(batch = %job.id, error = %e.message, "internal batch poll failed; retrying"),
        }
        if tokio::time::Instant::now() + poll_every > deadline {
            return Err(format!("batch {} did not finish in {:?}", job.id, max_wait));
        }
        tokio::time::sleep(poll_every).await;
    }
}

/// Results by position (an `index` field wins when present).
fn align(n: usize, results: Vec<Value>) -> Vec<Result<String, String>> {
    let mut out: Vec<Result<String, String>> = (0..n).map(|_| Err("no result".to_string())).collect();
    for (pos, r) in results.into_iter().enumerate() {
        let i = r.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(pos);
        if i < n {
            out[i] = completion_text(&r);
        }
    }
    out
}

/// The assistant text of one batch result: a bare chat completion (native
/// provider) or an OpenAI batch line (`response.body`), else its error.
pub fn completion_text(r: &Value) -> Result<String, String> {
    if let Some(e) = r.get("error").filter(|e| !e.is_null()) {
        return Err(e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string()));
    }
    let body = r.pointer("/response/body").or_else(|| r.get("response")).unwrap_or(r);
    body.pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "result has no message content".to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::llm_gateway::batches::{BatchProviderError, ProviderBatchJob, ProviderBatchStatus};
    use serde_json::json;
    use std::sync::Mutex;

    /// Mock batch provider: answers each request with `reply(i)` after
    /// `in_progress_polls` polls.
    pub struct MockBatch {
        pub submitted: Mutex<Vec<(String, Vec<Value>)>>,
        pub polls: Mutex<usize>,
        pub in_progress_polls: usize,
        pub replies: Vec<Value>,
        pub fail_submit: bool,
    }

    impl MockBatch {
        pub fn new(replies: Vec<Value>) -> Self {
            Self { submitted: Mutex::default(), polls: Mutex::default(), in_progress_polls: 1, replies, fail_submit: false }
        }
    }

    #[async_trait::async_trait]
    impl BatchProvider for MockBatch {
        async fn submit(&self, batch_id: &str, requests: &[Value]) -> Result<ProviderBatchJob, BatchProviderError> {
            if self.fail_submit {
                return Err(BatchProviderError::permanent("down", "provider down"));
            }
            self.submitted.lock().unwrap().push((batch_id.to_string(), requests.to_vec()));
            Ok(ProviderBatchJob { id: "pb_1".into(), status: "in_progress".into() })
        }
        async fn poll(&self, _id: &str) -> Result<ProviderBatchStatus, BatchProviderError> {
            let mut p = self.polls.lock().unwrap();
            *p += 1;
            if *p <= self.in_progress_polls {
                return Ok(ProviderBatchStatus { status: BatchJobStatus::InProgress, results: None });
            }
            Ok(ProviderBatchStatus { status: BatchJobStatus::Completed, results: Some(self.replies.clone()) })
        }
    }

    pub fn chat(content: &str) -> Value {
        json!({ "object": "chat.completion", "choices": [{ "message": { "role": "assistant", "content": content } }] })
    }

    #[tokio::test]
    async fn submits_polls_and_aligns_results() {
        let mock = MockBatch::new(vec![
            chat("one"),
            json!({ "index": 2, "response": { "body": chat("three") } }),
            json!({ "index": 1, "error": { "message": "rate limited" } }),
        ]);
        let reqs = vec![json!({"a":1}), json!({"a":2}), json!({"a":3})];
        let out = submit_and_poll(&mock, &reqs, Duration::from_millis(1), Duration::from_secs(5)).await.unwrap();
        assert_eq!(out, vec![Ok("one".into()), Err("rate limited".into()), Ok("three".into())]);
        let sub = mock.submitted.lock().unwrap();
        assert!(sub[0].0.starts_with("internal_"));
        assert_eq!(sub[0].1.len(), 3);
        assert_eq!(*mock.polls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn submit_failure_and_timeout_are_errors() {
        let mut down = MockBatch::new(vec![]);
        down.fail_submit = true;
        assert!(submit_and_poll(&down, &[json!({})], Duration::from_millis(1), Duration::from_secs(1)).await.is_err());
        let mut slow = MockBatch::new(vec![chat("x")]);
        slow.in_progress_polls = usize::MAX;
        assert!(submit_and_poll(&slow, &[json!({})], Duration::from_millis(5), Duration::from_millis(20)).await.is_err());
    }

    #[test]
    fn flag_defaults_off() {
        if std::env::var("ALLTERNIT_INTERNAL_BATCH").is_err() {
            assert!(!enabled());
        }
    }
}
