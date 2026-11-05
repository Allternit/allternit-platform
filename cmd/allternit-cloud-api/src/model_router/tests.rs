//! Unit tests for the model router.

use super::catalog::{starter_catalog, ModelAliasEntry, ModelAliasMap};
use super::{ChatCompletionRequest, Message, ModelRouter};

#[test]
fn starter_catalog_resolves_known_aliases() {
    let catalog = starter_catalog();

    assert!(catalog.resolve("llama-3.1-8b").is_some());
    assert!(catalog.resolve("llama3.1-8b").is_some()); // alternate alias
    assert!(catalog.resolve("gpt-4o").is_some());
    assert!(catalog.resolve("claude-sonnet-4.6").is_some());
    // Retired upstream id: the old alias resolves to the current Sonnet.
    assert_eq!(
        catalog.resolve("claude-sonnet-4").map(|e| e.upstream_id.as_str()),
        Some("anthropic/claude-sonnet-4.6")
    );
    assert!(catalog.resolve("deepseek-v4.1-flash").is_some());
    assert!(catalog.resolve("glm-5.3-flash").is_some());
    assert!(catalog.resolve("deepseek-v4-flash-fireworks").is_none());
    assert!(catalog.resolve("qwen-2.5-72b").is_some());
    assert!(catalog.resolve("mistral-large").is_some());
    assert!(catalog.resolve("qwen3.8-27b-groq").is_some());
    assert!(catalog.resolve("gpt-oss-120b-groq").is_some());
}

#[test]
fn unknown_model_returns_none() {
    let catalog = starter_catalog();
    assert!(catalog.resolve("not-a-real-model").is_none());
}

#[test]
fn catalog_entries_are_primary_aliases_only() {
    let catalog = starter_catalog();
    // Primary models only (one per model, not per alias):
    // 8 OpenRouter + 2 Together AI + 4 Fireworks + 3 DeepInfra + 3 Groq
    // + 2 open lane = 22.
    assert_eq!(catalog.entries().len(), 22);
    let open: Vec<_> = catalog
        .entries()
        .iter()
        .filter(|e| super::catalog::is_open_lane(e))
        .map(|e| e.alias.as_str())
        .collect();
    assert_eq!(open, vec!["deepseek-v4.1-flash-open", "glm-5.3-flash-open"]);
}

#[test]
fn model_alias_map_supports_multiple_aliases() {
    let catalog = ModelAliasMap::new(vec![ModelAliasEntry {
        alias: "primary".to_string(),
        provider: "openrouter".to_string(),
        upstream_id: "upstream/primary".to_string(),
        aliases: Some(vec!["alt1".to_string(), "alt2".to_string()]),
        created: 12345,
        name: "Primary Model".to_string(),
        prompt_price: 0.01,
        completion_price: 0.02,
        context_length: 128_000,
    }]);

    let primary = catalog.resolve("primary").unwrap();
    assert_eq!(primary.upstream_id, "upstream/primary");

    let alt1 = catalog.resolve("alt1").unwrap();
    assert_eq!(alt1.alias, "primary");

    let alt2 = catalog.resolve("alt2").unwrap();
    assert_eq!(alt2.alias, "primary");
}

#[test]
fn disabled_router_reports_not_enabled() {
    let router = ModelRouter::disabled(starter_catalog());
    assert!(!router.is_enabled());
}

#[tokio::test]
async fn disabled_router_list_models_returns_static_catalog() {
    let router = ModelRouter::disabled(starter_catalog());
    let models = router.list_models().await;
    // 22 catalog entries minus the 2 open-lane models, hidden while the
    // open-lane provider is unset.
    assert_eq!(models.len(), 20);
    assert!(models.iter().any(|m| m.id == "llama-3.1-8b"));
    assert!(!models.iter().any(|m| m.id.ends_with("-open")));

    let llama = models.iter().find(|m| m.id == "llama-3.1-8b").unwrap();
    assert_eq!(llama.extra.get("name").and_then(|v| v.as_str()), Some("Llama 3.1 8B Instruct"));
    assert!(llama.extra.get("prompt_price").and_then(|v| v.as_f64()).is_some());
    assert!(llama.extra.get("completion_price").and_then(|v| v.as_f64()).is_some());
    assert_eq!(llama.extra.get("context_length").and_then(|v| v.as_u64()), Some(128_000));
    assert_eq!(llama.extra.get("lane").and_then(|v| v.as_str()), Some("standard"));
}

#[tokio::test]
async fn chat_completion_request_detects_streaming() {
    let non_stream = ChatCompletionRequest {
        model: "gpt-4o".to_string(),
        messages: vec![Message::text("user", "hello")],
        temperature: None,
        max_tokens: None,
        stream: None,
        top_p: None,
        extra: serde_json::Map::new(),
    };
    assert!(!non_stream.is_streaming());

    let stream = ChatCompletionRequest {
        stream: Some(true),
        ..non_stream
    };
    assert!(stream.is_streaming());
}

mod pricing {
    use super::super::catalog::starter_catalog;
    use super::super::{
        ChatCompletionRequest, Message, ModelInfo, ModelRouter, ModelRouterError, UpstreamProvider,
    };
    use axum::{body::Body, http::Response};
    use std::sync::{Arc, Mutex};

    /// Guards `INFERENCE_MARKUP` mutation across these tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Provider stub serving a fixed model list (and recording dispatched
    /// requests for the stream_options test).
    struct StubProvider {
        models: Vec<ModelInfo>,
        last_request: Mutex<Option<ChatCompletionRequest>>,
    }

    impl StubProvider {
        fn with_pricing(prompt_per_token: f64, completion_per_token: f64) -> Arc<Self> {
            let mut extra = serde_json::Map::new();
            extra.insert(
                "prompt_price".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(prompt_per_token).unwrap()),
            );
            extra.insert(
                "completion_price".to_string(),
                serde_json::Value::Number(
                    serde_json::Number::from_f64(completion_per_token).unwrap(),
                ),
            );
            Arc::new(Self {
                models: vec![ModelInfo {
                    id: "openai/gpt-4o".to_string(),
                    object: "model".to_string(),
                    created: 0,
                    owned_by: "openai".to_string(),
                    upstream_id: None,
                    provider: Some("openrouter".to_string()),
                    aliases: None,
                    extra,
                }],
                last_request: Mutex::new(None),
            })
        }

        fn without_pricing() -> Arc<Self> {
            Arc::new(Self {
                models: vec![],
                last_request: Mutex::new(None),
            })
        }
    }

    #[async_trait::async_trait]
    impl UpstreamProvider for StubProvider {
        fn provider_id(&self) -> &str {
            "openrouter"
        }

        async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelRouterError> {
            Ok(self.models.clone())
        }

        async fn chat_completions(
            &self,
            request: ChatCompletionRequest,
        ) -> Result<Response<Body>, ModelRouterError> {
            *self.last_request.lock().unwrap() = Some(request);
            Response::builder()
                .status(200)
                .body(Body::from("{}"))
                .map_err(|e| ModelRouterError::Internal(e.to_string()))
        }
    }

    fn request(model: &str, stream: bool) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![Message::text("user", "hello")],
            temperature: None,
            max_tokens: None,
            stream: if stream { Some(true) } else { None },
            top_p: None,
            extra: serde_json::Map::new(),
        }
    }

    #[tokio::test]
    async fn live_pricing_is_marked_up_by_the_default_1_5x() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");
        let provider = StubProvider::with_pricing(0.0000001, 0.0000002); // per-token, OpenRouter shape
        let router = ModelRouter::new(vec![provider], starter_catalog());

        let prices = router.retail_prices("gpt-4o").await.unwrap();
        // wholesale per-1M: 0.10 / 0.20; retail = wholesale * 1.5.
        assert!((prices.wholesale_prompt_per_1m.unwrap() - 0.10).abs() < 1e-9);
        assert!((prices.wholesale_completion_per_1m.unwrap() - 0.20).abs() < 1e-9);
        assert!((prices.prompt_per_1m - 0.15).abs() < 1e-9);
        assert!((prices.completion_per_1m - 0.30).abs() < 1e-9);
    }

    #[tokio::test]
    async fn markup_env_is_clamped_to_1_to_5() {
        let _guard = ENV_LOCK.lock().unwrap();
        let provider = StubProvider::with_pricing(0.0000001, 0.0000002);

        std::env::set_var("INFERENCE_MARKUP", "0.1");
        let router = ModelRouter::new(vec![provider.clone()], starter_catalog());
        let prices = router.retail_prices("gpt-4o").await.unwrap();
        assert!((prices.prompt_per_1m - 0.10).abs() < 1e-9, "markup clamps at 1.0");

        std::env::set_var("INFERENCE_MARKUP", "100");
        let prices = router.retail_prices("gpt-4o").await.unwrap();
        assert!((prices.prompt_per_1m - 0.50).abs() < 1e-9, "markup clamps at 5.0");

        std::env::set_var("INFERENCE_MARKUP", "not-a-number");
        let prices = router.retail_prices("gpt-4o").await.unwrap();
        assert!((prices.prompt_per_1m - 0.15).abs() < 1e-9, "unparseable markup falls back to 1.5");

        std::env::remove_var("INFERENCE_MARKUP");
    }

    #[tokio::test]
    async fn catalog_prices_are_the_fallback_without_live_pricing() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");
        let provider = StubProvider::without_pricing();
        let router = ModelRouter::new(vec![provider], starter_catalog());

        let prices = router.retail_prices("gpt-4o").await.unwrap();
        assert_eq!(prices.prompt_per_1m, 2.50, "static catalog prompt price");
        assert_eq!(prices.completion_per_1m, 10.00, "static catalog completion price");
        assert_eq!(prices.wholesale_prompt_per_1m, None);
        assert_eq!(prices.wholesale_completion_per_1m, None);
    }

    fn listed_price(models: &[ModelInfo], alias: &str, key: &str) -> f64 {
        models
            .iter()
            .find(|m| m.id == alias)
            .and_then(|m| m.extra.get(key))
            .and_then(|v| v.as_f64())
            .unwrap()
    }

    #[tokio::test]
    async fn listed_prices_are_the_billed_retail_per_token() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");

        // Live pricing: wholesale per-token × 1.5, never the wholesale itself.
        let router = ModelRouter::new(
            vec![StubProvider::with_pricing(0.0000001, 0.0000002)],
            starter_catalog(),
        );
        let models = router.list_models().await;
        assert!((listed_price(&models, "gpt-4o", "prompt_price") - 0.00000015).abs() < 1e-15);
        assert!((listed_price(&models, "gpt-4o", "completion_price") - 0.0000003).abs() < 1e-15);

        // No live pricing: the static per-1M catalog price, listed per token.
        let router = ModelRouter::new(vec![StubProvider::without_pricing()], starter_catalog());
        let models = router.list_models().await;
        assert!((listed_price(&models, "gpt-4o", "prompt_price") - 0.0000025).abs() < 1e-15);
        assert!((listed_price(&models, "gpt-4o", "completion_price") - 0.00001).abs() < 1e-15);
        for model in &models {
            assert!(
                listed_price(&models, &model.id, "prompt_price") < 1e-4,
                "{} lists a per-1M price as per-token",
                model.id
            );
        }
    }

    #[tokio::test]
    async fn stream_options_include_usage_is_injected_for_streaming() {
        let provider = StubProvider::without_pricing();
        let router = ModelRouter::new(vec![provider.clone()], starter_catalog());

        router.chat_completions(request("gpt-4o", true)).await.unwrap();
        let sent = provider.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(
            sent.extra.get("stream_options"),
            Some(&serde_json::json!({ "include_usage": true })),
            "streaming requests ask the upstream for a final usage chunk"
        );
        assert_eq!(sent.model, "openai/gpt-4o", "alias is rewritten to the upstream id");

        router.chat_completions(request("gpt-4o", false)).await.unwrap();
        let sent = provider.last_request.lock().unwrap().clone().unwrap();
        assert!(
            sent.extra.get("stream_options").is_none(),
            "non-streaming requests are untouched"
        );
    }

    #[tokio::test]
    async fn caller_stream_options_win_over_the_injection() {
        let provider = StubProvider::without_pricing();
        let router = ModelRouter::new(vec![provider.clone()], starter_catalog());

        let mut req = request("gpt-4o", true);
        req.extra.insert(
            "stream_options".to_string(),
            serde_json::json!({ "include_usage": false }),
        );
        router.chat_completions(req).await.unwrap();
        let sent = provider.last_request.lock().unwrap().clone().unwrap();
        assert_eq!(
            sent.extra.get("stream_options"),
            Some(&serde_json::json!({ "include_usage": false })),
            "an explicit caller value is never overwritten"
        );
    }
}

#[test]
fn messages_accept_openai_content_parts_and_keep_tool_fields() {
    let body = serde_json::json!({
        "model": "deepseek-v4-pro",
        "messages": [
            { "role": "system", "content": "You are helpful." },
            { "role": "user", "content": [
                { "type": "text", "text": "Make a poster" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AA==" } }
            ] },
            { "role": "assistant", "content": null, "tool_calls": [
                { "id": "call_1", "type": "function", "function": { "name": "media_generate", "arguments": "{\"kind\":\"image\"}" } }
            ] },
            { "role": "tool", "tool_call_id": "call_1", "content": "Rendered" }
        ],
        "tools": [{ "type": "function", "function": { "name": "media_generate", "parameters": { "type": "object" } } }]
    });
    let req: ChatCompletionRequest = serde_json::from_value(body).expect("OpenAI-shaped request parses");
    assert_eq!(req.messages.len(), 4);
    assert_eq!(req.messages[1].content_chars(), "Make a poster".len());
    assert_eq!(req.messages[2].content_chars(), r#"{"kind":"image"}"#.len());
    assert!(req.extra.contains_key("tools"));

    // Round-trips to the upstream unchanged: parts, null content, tool fields.
    let out = serde_json::to_value(&req).unwrap();
    assert_eq!(out["messages"][1]["content"][1]["type"], "image_url");
    assert!(out["messages"][2]["content"].is_null());
    assert_eq!(out["messages"][2]["tool_calls"][0]["id"], "call_1");
    assert_eq!(out["messages"][3]["tool_call_id"], "call_1");
    assert_eq!(out["tools"][0]["function"]["name"], "media_generate");
}

#[test]
fn out_of_credits_provider_is_hidden_and_reported_clearly() {
    use crate::services::upstream_funding::{mark_out_of_credits, provider_out_of_credits};
    // A provider id used only by this test, so parallel tests are unaffected.
    let provider = "test-unfunded-provider";
    assert!(!provider_out_of_credits(provider));
    mark_out_of_credits(provider);
    assert!(provider_out_of_credits(provider));

    let err: crate::ApiError = super::ModelRouterError::ModelUnavailable {
        model: "Claude Sonnet 4.6".to_string(),
    }
    .into();
    assert!(matches!(err, crate::ApiError::ServiceUnavailable(ref m) if m.starts_with("Claude Sonnet 4.6 is unavailable")));

    let upstream_402: crate::ApiError = super::ModelRouterError::UpstreamError {
        status: 402,
        message: "{\"error\":\"Insufficient credits\"}".to_string(),
    }
    .into();
    assert!(matches!(upstream_402, crate::ApiError::ServiceUnavailable(ref m) if !m.contains("credits")));
}

#[test]
fn retired_together_dedicated_only_models_are_not_offered() {
    let catalog = starter_catalog();
    for alias in ["llama-3.3-70b-turbo", "llama-3.3-70b", "qwen2.5-72b-together", "qwen2.5-72b-t"] {
        assert!(catalog.resolve(alias).is_none(), "retired model still offered: {alias}");
    }
}

#[test]
fn unavailable_routes_are_removed_and_saved_kimi_chats_use_live_host() {
    let catalog = starter_catalog();
    for alias in ["qwen3.6-27b-groq", "deepseek-v4-pro-fireworks", "qwen3.8-2.4t-a95b", "qwen3.8-2.4t"] {
        assert!(catalog.resolve(alias).is_none(), "unavailable route still offered: {alias}");
    }
    // Runtime discovery lists primary ids, not alternate aliases. Both ids
    // must stay in the list so existing app brains still resolve locally.
    for id in ["kimi-k3", "kimi-k3-fireworks"] {
        assert!(catalog.entries().iter().any(|entry| entry.alias == id));
    }
    let kimi = catalog.resolve("kimi-k3").unwrap();
    assert_eq!(kimi.provider, "fireworks");
    assert_eq!(kimi.upstream_id, "accounts/fireworks/models/kimi-k3");
}

/// Regression (2026-11-05): gizzi sends `prompt_cache_key`; Groq answered 400
/// "property 'prompt_cache_key' is unsupported", so every Cloud chat failed.
#[test]
fn openai_only_fields_are_dropped_for_other_upstreams() {
    let body = r#"{"model":"allternit/gpt-oss-120b-groq","messages":[{"role":"user","content":"hi"}],"prompt_cache_key":"ses_1","tools":[]}"#;
    let mut groq: ChatCompletionRequest = serde_json::from_str(body).unwrap();
    groq.strip_openai_only_fields("groq");
    let sent = serde_json::to_value(&groq).unwrap();
    assert!(sent.get("prompt_cache_key").is_none());
    assert!(sent.get("tools").is_some(), "other extra fields still go upstream");

    let mut openai: ChatCompletionRequest = serde_json::from_str(body).unwrap();
    openai.strip_openai_only_fields("openai");
    assert_eq!(serde_json::to_value(&openai).unwrap()["prompt_cache_key"], "ses_1");
}
