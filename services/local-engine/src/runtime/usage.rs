//! Real token numbers for the chat proxy.
//!
//! llama-server and allternit-mlx-server both send OpenAI `usage`
//! (`prompt_tokens`, `completion_tokens`) on the last stream chunk when the
//! request sets `stream_options.include_usage`, plus llama.cpp-style
//! `timings` (`predicted_per_second`, `prompt_ms`). The proxy always asks for
//! usage upstream, reads it here, adds an `allternit` object
//! `{tok_per_sec, first_token_ms, ctx_used, ctx_max, mem_bytes}` to that
//! chunk, and drops the usage-only chunk again when the client did not ask for
//! it (OpenAI clients that index `choices[0]` would trip on its empty list).

use serde::Serialize;
use serde_json::{json, Value};

/// Numbers from one completed request.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct UsageStats {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub tok_per_sec: Option<f64>,
    pub first_token_ms: Option<u64>,
}

impl UsageStats {
    pub fn ctx_used(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// The `allternit` extension object.
pub fn extension(stats: &UsageStats, ctx_max: Option<u64>, mem_bytes: Option<u64>) -> Value {
    json!({
        "tok_per_sec": stats.tok_per_sec.map(|v| (v * 10.0).round() / 10.0),
        "first_token_ms": stats.first_token_ms,
        "ctx_used": stats.ctx_used(),
        "ctx_max": ctx_max,
        "mem_bytes": mem_bytes,
    })
}

/// Stats from a chunk or reply that carries `usage`. `first_token_ms` is what
/// the proxy measured (falls back to `timings.prompt_ms`); `elapsed_ms` is the
/// whole request, used for tok/s when the backend sent no `timings`.
pub fn stats_from(body: &Value, first_token_ms: Option<u64>, elapsed_ms: u64) -> Option<UsageStats> {
    let usage = body.get("usage").filter(|u| u.is_object())?;
    let prompt_tokens = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let completion_tokens = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    let timings = body.get("timings");
    let num = |k: &str| timings.and_then(|t| t.get(k)).and_then(Value::as_f64);
    let first_token_ms = first_token_ms.or_else(|| num("prompt_ms").map(|v| v.round() as u64));
    let tok_per_sec = num("predicted_per_second").filter(|v| v.is_finite() && *v > 0.0).or_else(|| {
        let gen_ms = elapsed_ms.saturating_sub(first_token_ms.unwrap_or(0));
        (completion_tokens > 0 && gen_ms > 0).then(|| completion_tokens as f64 * 1000.0 / gen_ms as f64)
    });
    Some(UsageStats { prompt_tokens, completion_tokens, tok_per_sec, first_token_ms })
}

/// Make sure the upstream request asks for usage on streams. Returns
/// `(stream, client_asked_for_usage)`.
pub fn prepare_request(req: &mut Value) -> (bool, bool) {
    let stream = req.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let asked = req
        .pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if stream {
        if let Some(obj) = req.as_object_mut() {
            let opts = obj.entry("stream_options").or_insert_with(|| json!({}));
            if !opts.is_object() {
                *opts = json!({});
            }
            opts["include_usage"] = Value::Bool(true);
        }
    }
    (stream, asked)
}

/// What to do with one SSE event from upstream.
pub enum SseOut {
    /// Forward these bytes.
    Emit(String),
    /// Usage-only chunk the client did not ask for.
    Drop,
}

/// Rewrites an upstream SSE stream event by event.
pub struct SseRewriter {
    buf: Vec<u8>,
    client_wants_usage: bool,
    pub first_token_ms: Option<u64>,
    pub stats: Option<UsageStats>,
}

impl SseRewriter {
    pub fn new(client_wants_usage: bool) -> Self {
        Self { buf: Vec::new(), client_wants_usage, first_token_ms: None, stats: None }
    }

    /// Feed bytes; returns complete events (each ends in a blank line).
    /// `ext` builds the `allternit` object once usage is known.
    pub fn push(
        &mut self,
        bytes: &[u8],
        elapsed_ms: u64,
        ext: &dyn Fn(&UsageStats) -> Value,
    ) -> Vec<SseOut> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, sep)) = find_event_end(&self.buf) {
            let event: Vec<u8> = self.buf.drain(..end + sep).collect();
            let text = String::from_utf8_lossy(&event[..end]).into_owned();
            out.push(self.rewrite(&text, elapsed_ms, ext));
        }
        out
    }

    /// Whatever is left when upstream closes (an event with no blank line after it).
    pub fn finish(&mut self, elapsed_ms: u64, ext: &dyn Fn(&UsageStats) -> Value) -> Option<SseOut> {
        if self.buf.iter().all(|b| b.is_ascii_whitespace()) {
            return None;
        }
        let text = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).trim_end().to_string();
        Some(self.rewrite(&text, elapsed_ms, ext))
    }

    fn rewrite(&mut self, event: &str, elapsed_ms: u64, ext: &dyn Fn(&UsageStats) -> Value) -> SseOut {
        let data: Vec<&str> = event
            .lines()
            .filter_map(|l| l.strip_prefix("data:").map(|d| d.strip_prefix(' ').unwrap_or(d)))
            .collect();
        let payload = data.join("\n");
        let Ok(mut chunk) = serde_json::from_str::<Value>(&payload) else {
            // [DONE], comments, keep-alives: untouched.
            return SseOut::Emit(format!("{event}\n\n"));
        };
        if self.first_token_ms.is_none() && has_content(&chunk) {
            self.first_token_ms = Some(elapsed_ms);
        }
        if let Some(stats) = stats_from(&chunk, self.first_token_ms, elapsed_ms) {
            self.stats = Some(stats);
            let choices_empty = chunk.get("choices").and_then(Value::as_array).map_or(true, |c| c.is_empty());
            if !self.client_wants_usage {
                if choices_empty {
                    return SseOut::Drop;
                }
                if let Some(obj) = chunk.as_object_mut() {
                    obj.remove("usage");
                }
            }
            chunk["allternit"] = ext(&stats);
        }
        SseOut::Emit(format!("data: {}\n\n", chunk))
    }
}

fn has_content(chunk: &Value) -> bool {
    chunk
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|c| {
            let d = &c["delta"];
            d.get("content").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
                || d.get("reasoning_content").and_then(Value::as_str).is_some_and(|s| !s.is_empty())
        })
}

/// Index of the end of the first event and the separator length.
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(s: &UsageStats) -> Value {
        extension(s, Some(4096), Some(1_000))
    }

    fn emitted(out: Vec<SseOut>) -> Vec<String> {
        out.into_iter()
            .filter_map(|o| match o {
                SseOut::Emit(s) => Some(s),
                SseOut::Drop => None,
            })
            .collect()
    }

    const ROLE: &str = r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#;
    const HI: &str = r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#;
    const USAGE: &str = r#"data: {"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":30,"total_tokens":42},"timings":{"prompt_ms":40.4,"predicted_per_second":97.25}}"#;

    #[test]
    fn usage_chunk_gets_the_extension_even_when_split_across_reads() {
        let mut r = SseRewriter::new(true);
        let stream = format!("{ROLE}\n\n{HI}\n\n{USAGE}\n\ndata: [DONE]\n\n");
        let (a, b) = stream.split_at(stream.len() / 2 + 7);
        let mut out = emitted(r.push(a.as_bytes(), 5, &ext));
        out.extend(emitted(r.push(b.as_bytes(), 300, &ext)));
        assert_eq!(out.len(), 4);
        let last: Value = serde_json::from_str(out[2].trim().strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(last["usage"]["completion_tokens"], 30);
        assert_eq!(last["allternit"]["ctx_used"], 42);
        assert_eq!(last["allternit"]["ctx_max"], 4096);
        assert_eq!(last["allternit"]["tok_per_sec"], 97.3);
        assert_eq!(out[3], "data: [DONE]\n\n");
        // The role chunk has empty content: first token is the "Hi" chunk.
        assert!(r.first_token_ms.is_some());
        assert_eq!(r.stats.unwrap().prompt_tokens, 12);
    }

    #[test]
    fn usage_only_chunk_is_dropped_when_the_client_did_not_ask() {
        let mut r = SseRewriter::new(false);
        let out = r.push(format!("{HI}\r\n\r\n{USAGE}\n\n").as_bytes(), 100, &ext);
        assert_eq!(emitted(out).len(), 1);
        assert_eq!(r.stats.unwrap().ctx_used(), 42);
    }

    #[test]
    fn tok_per_sec_falls_back_to_measured_time_without_timings() {
        let body = json!({"usage": {"prompt_tokens": 5, "completion_tokens": 20}});
        let s = stats_from(&body, Some(200), 1200).unwrap();
        assert_eq!(s.tok_per_sec, Some(20.0));
        assert_eq!(s.first_token_ms, Some(200));
        assert!(stats_from(&json!({"usage": null}), None, 10).is_none());
    }

    #[test]
    fn streams_always_ask_upstream_for_usage() {
        let mut req = json!({"model": "m", "stream": true});
        assert_eq!(prepare_request(&mut req), (true, false));
        assert_eq!(req["stream_options"]["include_usage"], true);
        let mut req = json!({"model": "m", "stream": true, "stream_options": {"include_usage": true}});
        assert_eq!(prepare_request(&mut req), (true, true));
        let mut req = json!({"model": "m"});
        assert_eq!(prepare_request(&mut req), (false, false));
        assert!(req.get("stream_options").is_none());
    }
}
