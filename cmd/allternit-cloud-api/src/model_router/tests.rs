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
    assert!(catalog.resolve("mistral-large").is_none());
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
    // 9 OpenRouter + 3 Together AI + 4 Fireworks + 3 DeepInfra + 3 Groq
    // + 2 open lane = 24.
    assert_eq!(catalog.entries().len(), 24);
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
        ..Default::default()
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
    // 24 catalog entries minus the 2 open-lane models, hidden while the
    // open-lane provider is unset. Unlisted aliases are still returned.
    assert_eq!(models.len(), 22);
    assert!(models.iter().any(|m| m.id == "llama-3.1-8b"));
    assert!(!models.iter().any(|m| m.id.ends_with("-open")));

    let llama = models.iter().find(|m| m.id == "llama-3.1-8b").unwrap();
    assert_eq!(llama.extra.get("name").and_then(|v| v.as_str()), Some("Llama 3.1 8B Instruct"));
    assert!(llama.extra.get("prompt_price").and_then(|v| v.as_f64()).is_some());
    assert!(llama.extra.get("completion_price").and_then(|v| v.as_f64()).is_some());
    assert_eq!(llama.extra.get("context_length").and_then(|v| v.as_u64()), Some(128_000));
    assert_eq!(llama.extra.get("lane").and_then(|v| v.as_str()), Some("standard"));
    assert_eq!(llama.extra.get("default").and_then(|v| v.as_bool()), Some(false));
}

/// Stale and duplicate-by-upstream aliases: returned by `GET /v1/models`
/// flagged `hidden`, so pickers skip them and saved ids keep resolving.
const UNLISTED: [&str; 7] = [
    "llama-3.1-8b",
    "llama-3.1-70b",
    "claude-sonnet-4.6",
    "gpt-4o",
    "qwen-2.5-72b",
    "kimi-k3-fireworks",
    "glm-5.3-fireworks",
];

#[test]
fn unlisted_aliases_still_resolve() {
    // Saved bots and artifact_runtime's `llama-3.1-8b` fallback depend on it.
    let catalog = starter_catalog();
    for alias in UNLISTED.iter().chain(&["gpt4o", "llama3.1-8b", "claude-sonnet", "claude-sonnet-4"]) {
        assert!(catalog.resolve(alias).is_some(), "unlisted alias no longer resolves: {alias}");
    }
    assert_eq!(
        catalog.resolve("gpt-4o").map(|e| e.upstream_id.as_str()),
        Some("openai/gpt-4o")
    );
    assert_eq!(
        catalog.resolve("llama-3.1-8b").map(|e| e.upstream_id.as_str()),
        Some("meta-llama/llama-3.1-8b-instruct")
    );
}

#[tokio::test]
async fn model_list_flags_hidden_rows_last_and_kimi_k3_first_as_default() {
    let router = ModelRouter::disabled(starter_catalog());
    let models = router.list_models().await;
    let flag = |m: &super::ModelInfo, key: &str| m.extra.get(key).and_then(|v| v.as_bool());

    // Default first, flagged, and the only default.
    assert_eq!(models[0].id, "kimi-k3");
    assert_eq!(flag(&models[0], "default"), Some(true));
    assert!(models.iter().all(|m| flag(m, "default").is_some()), "every row carries `default`");
    assert_eq!(models.iter().filter(|m| flag(*m, "default") == Some(true)).count(), 1);

    // Hidden rows: exactly the unlisted set, last, in catalog order.
    let hidden: Vec<&str> = models
        .iter()
        .filter(|m| flag(*m, "hidden") == Some(true))
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(hidden, UNLISTED.to_vec());
    let tail: Vec<&str> = models[models.len() - UNLISTED.len()..]
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(tail, UNLISTED.to_vec());
    // Listed rows carry no `hidden` key; they keep catalog order after the default.
    assert!(models[..models.len() - UNLISTED.len()]
        .iter()
        .all(|m| m.extra.get("hidden").is_none()));
    assert_eq!(models[1].id, "claude-sonnet-5.5");
    assert!(models.iter().any(|m| m.id == "qwen3.8-max-fireworks" && flag(m, "hidden").is_none()));
}

/// Rows added 2026-11-10 (each checked with a real request on the provider
/// account). Sonnet 4.6 is hidden but saved `claude-sonnet*` ids keep it.
#[tokio::test]
async fn new_rows_route_and_are_listed_and_sonnet_4_6_is_hidden() {
    let catalog = starter_catalog();
    for (alias, provider, upstream) in [
        ("claude-sonnet-5.5", "openrouter", "anthropic/claude-sonnet-5.5"),
        ("gpt-6.1-sol", "openrouter", "openai/gpt-6.1-sol"),
        ("kimi-k3.1-pro", "together", "moonshotai/Kimi-K3.1-Pro"),
    ] {
        let entry = catalog.resolve(alias).unwrap_or_else(|| panic!("{alias} does not resolve"));
        assert_eq!((entry.provider.as_str(), entry.upstream_id.as_str()), (provider, upstream), "{alias}");
    }
    for alias in ["claude-sonnet-4.6", "claude-sonnet", "claude-sonnet-4"] {
        assert_eq!(
            catalog.resolve(alias).map(|e| e.upstream_id.as_str()),
            Some("anthropic/claude-sonnet-4.6"),
            "{alias} must keep the model (and price) saved bots were built on"
        );
    }

    let models = ModelRouter::disabled(starter_catalog()).list_models().await;
    let row = |id: &str| models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id} not listed"));
    for id in ["claude-sonnet-5.5", "gpt-6.1-sol", "kimi-k3.1-pro"] {
        assert!(row(id).extra.get("hidden").is_none(), "{id} must be offered in pickers");
        assert_eq!(row(id).extra.get("default").and_then(|v| v.as_bool()), Some(false));
    }
    assert_eq!(row("claude-sonnet-4.6").extra.get("hidden").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(models.iter().filter(|m| m.extra.get("default").and_then(|v| v.as_bool()) == Some(true)).count(), 1);
    assert_eq!(models[0].id, "kimi-k3");
}

/// Only Fireworks rows (whose model list has no pricing) treat the static
/// catalog price as wholesale; every other row bills its static price.
#[test]
fn only_fireworks_rows_mark_up_their_static_price() {
    let catalog = starter_catalog();
    for entry in catalog.entries() {
        assert_eq!(
            entry.static_price_is_wholesale,
            entry.provider == "fireworks",
            "{} ({})",
            entry.alias,
            entry.provider
        );
    }
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
        id: &'static str,
        models: Vec<ModelInfo>,
        last_request: Mutex<Option<ChatCompletionRequest>>,
    }

    impl StubProvider {
        /// A provider `id` listing `upstream_ids`, with per-token pricing
        /// extras when `pricing` is set (none when it is not, like Fireworks).
        fn serving(id: &'static str, upstream_ids: &[&str], pricing: Option<(f64, f64)>) -> Arc<Self> {
            let mut extra = serde_json::Map::new();
            if let Some((prompt, completion)) = pricing {
                for (key, value) in [("prompt_price", prompt), ("completion_price", completion)] {
                    extra.insert(
                        key.to_string(),
                        serde_json::Value::Number(serde_json::Number::from_f64(value).unwrap()),
                    );
                }
            }
            Arc::new(Self {
                id,
                models: upstream_ids
                    .iter()
                    .map(|upstream| ModelInfo {
                        id: upstream.to_string(),
                        object: "model".to_string(),
                        created: 0,
                        owned_by: id.to_string(),
                        upstream_id: None,
                        provider: Some(id.to_string()),
                        aliases: None,
                        extra: extra.clone(),
                    })
                    .collect(),
                last_request: Mutex::new(None),
            })
        }

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
            let model = |id: &str, owned_by: &str| ModelInfo {
                id: id.to_string(),
                object: "model".to_string(),
                created: 0,
                owned_by: owned_by.to_string(),
                upstream_id: None,
                provider: Some("openrouter".to_string()),
                aliases: None,
                extra: extra.clone(),
            };
            Arc::new(Self {
                id: "openrouter",
                // Both unlisted (still billed): gpt-4o and Sonnet 4.6.
                models: vec![
                    model("openai/gpt-4o", "openai"),
                    model("anthropic/claude-sonnet-4.6", "anthropic"),
                ],
                last_request: Mutex::new(None),
            })
        }

        fn without_pricing() -> Arc<Self> {
            Arc::new(Self {
                id: "openrouter",
                models: vec![],
                last_request: Mutex::new(None),
            })
        }
    }

    #[async_trait::async_trait]
    impl UpstreamProvider for StubProvider {
        fn provider_id(&self) -> &str {
            self.id
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

    /// The Fireworks rows as served today: listed, no pricing in the list.
    fn fireworks_without_pricing() -> std::sync::Arc<StubProvider> {
        StubProvider::serving(
            "fireworks",
            &[
                "accounts/fireworks/models/kimi-k3",
                "accounts/fireworks/models/glm-5p3",
                "accounts/fireworks/models/qwen3p8-max",
            ],
            None,
        )
    }

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-9
    }

    #[tokio::test]
    async fn fireworks_static_price_is_marked_up_and_recorded_as_wholesale() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");
        let router = ModelRouter::new(vec![fireworks_without_pricing()], starter_catalog());

        // (alias, static prompt, static completion, billed prompt, billed completion)
        for (alias, wp, wc, rp, rc) in [
            ("kimi-k3", 3.00, 15.00, 4.50, 22.50),
            ("kimi-k3-fireworks", 3.00, 15.00, 4.50, 22.50),
            ("glm-5.3-fireworks", 1.40, 4.40, 2.10, 6.60),
            ("qwen3.8-max-fireworks", 2.00, 6.00, 3.00, 9.00),
        ] {
            let prices = router.retail_prices(alias).await.unwrap();
            assert!(close(prices.prompt_per_1m, rp), "{alias} prompt {}", prices.prompt_per_1m);
            assert!(close(prices.completion_per_1m, rc), "{alias} completion {}", prices.completion_per_1m);
            assert_eq!(prices.wholesale_prompt_per_1m, Some(wp), "{alias}");
            assert_eq!(prices.wholesale_completion_per_1m, Some(wc), "{alias}");
        }

        // Follows INFERENCE_MARKUP and its clamp, like live pricing.
        for (markup, expected_prompt) in [("2.0", 6.00), ("100", 15.00), ("0.1", 3.00)] {
            std::env::set_var("INFERENCE_MARKUP", markup);
            let prices = router.retail_prices("kimi-k3").await.unwrap();
            assert!(close(prices.prompt_per_1m, expected_prompt), "markup {markup}: {}", prices.prompt_per_1m);
            assert_eq!(prices.wholesale_prompt_per_1m, Some(3.00));
        }
        std::env::set_var("INFERENCE_MARKUP", "2.0");
        let prices = router.retail_prices("kimi-k3").await.unwrap();
        assert!(close(prices.completion_per_1m, 30.00));
        std::env::remove_var("INFERENCE_MARKUP");
    }

    #[tokio::test]
    async fn live_fireworks_pricing_still_wins_over_the_static_price() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");
        let provider = StubProvider::serving(
            "fireworks",
            &["accounts/fireworks/models/kimi-k3"],
            Some((0.000001, 0.000004)), // per token: 1.00 / 4.00 per 1M
        );
        let router = ModelRouter::new(vec![provider], starter_catalog());
        let prices = router.retail_prices("kimi-k3").await.unwrap();
        assert!(close(prices.wholesale_prompt_per_1m.unwrap(), 1.00));
        assert!(close(prices.wholesale_completion_per_1m.unwrap(), 4.00));
        assert!(close(prices.prompt_per_1m, 1.50));
        assert!(close(prices.completion_per_1m, 6.00));
    }

    #[tokio::test]
    async fn open_lane_prices_are_retail_and_not_marked_up() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("INFERENCE_MARKUP");
        let open_lane = StubProvider::serving(
            super::super::catalog::OPEN_LANE_PROVIDER,
            &["deepseek-v4.1-flash-open", "glm-5.3-flash-open"],
            None,
        );
        let router = ModelRouter::new(vec![open_lane], starter_catalog());
        for (alias, prompt, completion) in
            [("deepseek-v4.1-flash-open", 0.10, 2.00), ("glm-5.3-flash-open", 0.40, 1.20)]
        {
            let prices = router.retail_prices(alias).await.unwrap();
            assert_eq!((prices.prompt_per_1m, prices.completion_per_1m), (prompt, completion), "{alias}");
            assert_eq!(prices.wholesale_prompt_per_1m, None, "{alias}");
        }
        let models = router.list_models().await;
        assert!((listed_price(&models, "deepseek-v4.1-flash-open", "prompt_price") - 0.0000001).abs() < 1e-15);
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
        assert!((listed_price(&models, "claude-sonnet-4.6", "prompt_price") - 0.00000015).abs() < 1e-15);
        assert!((listed_price(&models, "claude-sonnet-4.6", "completion_price") - 0.0000003).abs() < 1e-15);

        // No live pricing: the static per-1M catalog price, listed per token.
        let router = ModelRouter::new(vec![StubProvider::without_pricing()], starter_catalog());
        let models = router.list_models().await;
        assert!((listed_price(&models, "claude-sonnet-4.6", "prompt_price") - 0.000003).abs() < 1e-15);
        assert!((listed_price(&models, "claude-sonnet-4.6", "completion_price") - 0.000015).abs() < 1e-15);
        for model in &models {
            assert!(
                listed_price(&models, &model.id, "prompt_price") < 1e-4,
                "{} lists a per-1M price as per-token",
                model.id
            );
        }

        // Fireworks has no pricing in its list: kimi-k3 lists the billed
        // static × 1.5, not Fireworks' own price.
        let router = ModelRouter::new(vec![fireworks_without_pricing()], starter_catalog());
        let models = router.list_models().await;
        assert!((listed_price(&models, "kimi-k3", "prompt_price") - 0.0000045).abs() < 1e-15);
        assert!((listed_price(&models, "kimi-k3", "completion_price") - 0.0000225).abs() < 1e-15);
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
    // Both ids stay primary catalog entries so existing app brains still
    // resolve. `GET /v1/models` returns both; kimi-k3-fireworks (same
    // upstream) is flagged `hidden: true`.
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

/// A Platform agent on `"allternit"` runs on the model `/v1/models` flags as
/// the default; without a flag, the first listed model; never an open-lane one.
#[test]
fn default_alias_is_the_flagged_listed_model() {
    assert_eq!(ModelRouter::disabled(starter_catalog()).default_alias(), Some("kimi-k3"));
    let entry = |alias: &str, provider: &str, listed: bool, default: bool| ModelAliasEntry {
        alias: alias.to_string(),
        provider: provider.to_string(),
        upstream_id: format!("up/{alias}"),
        listed,
        default,
        ..Default::default()
    };
    let open = super::catalog::OPEN_LANE_PROVIDER;
    let unflagged = ModelAliasMap::new(vec![entry("open-1", open, true, false), entry("hidden", "openrouter", false, false), entry("first", "openrouter", true, false)]);
    assert_eq!(ModelRouter::disabled(unflagged).default_alias(), Some("first"));
    let only_open = ModelAliasMap::new(vec![entry("open-1", open, true, true)]);
    assert_eq!(ModelRouter::disabled(only_open).default_alias(), None);
}

/// 2026-11-13: Fireworks had no money, so its rows left `/v1/models`,
/// including the catalog default (Kimi K3), and no row carried
/// `default: true`. The flag must move to the model the server uses instead.
#[tokio::test]
async fn default_flag_moves_when_the_default_provider_is_out_of_credits() {
    use crate::services::upstream_funding::mark_out_of_credits;
    // A provider id used only by this test, so parallel tests are unaffected.
    let unfunded = "test-unfunded-default-provider";
    let entry = |alias: &str, provider: &str, default: bool| ModelAliasEntry {
        alias: alias.to_string(),
        provider: provider.to_string(),
        upstream_id: format!("up/{alias}"),
        default,
        ..Default::default()
    };
    let catalog = ModelAliasMap::new(vec![entry("funded", "openrouter", false), entry("catalog-default", unfunded, true)]);
    mark_out_of_credits(unfunded);
    let router = ModelRouter::disabled(catalog);

    let models = router.list_models().await;
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["funded"]);
    assert_eq!(models[0].extra["default"], true);
    assert_eq!(router.default_alias(), Some("funded"));
}

/// Plan models through the real `/v1/chat/completions` handler (contract 9.3,
/// D7): person-only, chat-only, streamed from the person's runner box (a
/// fake runner program; the box backend is the edge).
mod plan_turns {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{Extension, Router};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use crate::auth::{AuthContext, AuthenticatedUser};
    use crate::routes::connections::TEST_RUNNERS;
    use crate::routes::model_router::{chat_completions, is_person_only_model, plan_model};
    use crate::routes::test_support::{test_state, MockGateway};
    use crate::services::brain_runners::{test_service_with_box, Vendor};
    use crate::ApiState;

    const USER: &str = "user_plan_turn";

    fn app(state: &Arc<ApiState>, token_id: &str) -> Router {
        let auth = AuthContext {
            user: AuthenticatedUser { user_id: USER.into(), token_id: token_id.into(), permissions: vec!["inference".into()] },
            is_development: false,
        };
        Router::new()
            .route("/v1/chat/completions", axum::routing::post(chat_completions))
            .layer(Extension(auth))
            .with_state(state.clone())
    }

    async fn call(app: &Router, headers: &[(&str, &str)], body: Value) -> (StatusCode, String) {
        let mut builder = Request::builder().method("POST").uri("/v1/chat/completions").header("content-type", "application/json");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = app.clone().oneshot(builder.body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    fn code(body: &str) -> Option<String> {
        serde_json::from_str::<Value>(body).ok()?["error"]["code"].as_str().map(str::to_string)
    }

    fn turn(model: &str, stream: bool) -> Value {
        json!({ "model": model, "stream": stream, "messages": [{ "role": "user", "content": "Hello" }],
                "tools": [{ "type": "function", "function": { "name": "x" } }], "tool_choice": "auto" })
    }

    #[test]
    fn plan_ids_parse_in_wire_and_in_app_forms() {
        assert_eq!(plan_model("plan/claude/sonnet"), Some((Vendor::Claude, "sonnet")));
        assert_eq!(plan_model("plan-kimi/kimi-code/k3"), Some((Vendor::Kimi, "kimi-code/k3")));
        assert_eq!(plan_model("plan/nope/x"), None);
        assert!(is_person_only_model("copilot/gpt-5") && is_person_only_model("plan-claude/opus"));
        assert!(!is_person_only_model("kimi-k3") && !is_person_only_model("xai/grok-4"));
    }

    #[tokio::test]
    async fn plan_turns_are_person_only_chat_only_and_stream_from_the_runner() {
        let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        sqlx::raw_sql(include_str!("../../migrations_pg/140_brain_runners.sql")).execute(&state.db).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS api_keys (id TEXT PRIMARY KEY, project_id TEXT, kind TEXT NOT NULL DEFAULT 'standard');
             INSERT INTO api_keys (id, project_id, kind) VALUES ('key_platform', 'proj_1', 'standard'), ('key_play', 'proj_1', 'playground');",
        )
        .execute(&state.db)
        .await
        .unwrap();
        // The fake runner's hello: ChatGPT signed in, Claude signed out.
        let (service, _box) = test_service_with_box(state.db.clone(), USER).await;
        let person = ("x-allternit-turn", "person");

        TEST_RUNNERS
            .scope(service, async {
                let session = app(&state, "sess_1");
                // D7: no person header, a device token, a Platform key, a playground key.
                let (status, body) = call(&session, &[], turn("plan/chatgpt/gpt-6-luna", false)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "{body}");
                let (status, body) =
                    call(&session, &[person, ("authorization", "Bearer allternit_runtime_abc")], turn("plan/chatgpt/gpt-6-luna", false)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "device token: {body}");
                for key in ["key_platform", "key_play"] {
                    let (status, body) = call(&app(&state, key), &[person], turn("plan/chatgpt/gpt-6-luna", false)).await;
                    assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "{key}: {body}");
                }
                // Copilot ids take the same rule.
                let (status, body) = call(&session, &[], turn("copilot/gpt-5", false)).await;
                assert_eq!(code(&body).as_deref(), Some("plan_person_only"), "{status} {body}");

                // Chat only: a tool-role message is a 400.
                let mut with_tool = turn("plan/chatgpt/gpt-6-luna", false);
                with_tool["messages"] = json!([{ "role": "user", "content": "x" }, { "role": "tool", "content": "y", "tool_call_id": "c1" }]);
                let (status, body) = call(&session, &[person], with_tool).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::BAD_REQUEST, Some("plan_no_tools")));

                // A plan that is not signed in.
                let (status, body) = call(&session, &[person], turn("plan/claude/sonnet", false)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_not_connected")), "{body}");

                // The person's own send: the runner's reply, tools dropped.
                let (status, body) = call(&session, &[person], turn("plan/chatgpt/gpt-6-luna", false)).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                let reply: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(reply["choices"][0]["message"]["content"], "Hi there");
                assert_eq!(reply["model"], "plan/chatgpt/gpt-6-luna");
                assert_eq!(reply["usage"]["completion_tokens"], 3);

                let (status, body) = call(&session, &[person], turn("plan-chatgpt/gpt-6-luna", true)).await;
                assert_eq!(status, StatusCode::OK);
                assert!(body.contains(r#""content":"Hi""#) && body.contains(r#""content":" there""#), "{body}");
                assert!(body.trim_end().ends_with("data: [DONE]"), "{body}");
            })
            .await;

        // Other routes that take a model refuse a plan id.
        let mut open = super::ChatCompletionRequest {
            model: "plan/claude/sonnet".into(),
            messages: vec![super::Message::text("user", "hi")],
            temperature: None,
            max_tokens: None,
            stream: None,
            top_p: None,
            extra: serde_json::Map::new(),
        };
        let refused = crate::routes::open_lane::guard_open_request(&state, USER, true, &open).await.unwrap().expect("refused");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        open.model = "copilot/gpt-5".into();
        assert!(crate::routes::open_lane::guard_open_request(&state, USER, true, &open).await.unwrap().is_some());
        let platform = crate::routes::platform_v1::responses::refuse_person_only_model(Some("plan/grok/grok-4.6")).unwrap_err();
        assert_eq!(platform.status, StatusCode::FORBIDDEN);
        assert!(crate::routes::platform_v1::responses::refuse_person_only_model(Some("kimi-k3")).is_ok());
    }

    /// Copilot turns (contract 9.5): the same D7 rules, then one streamed
    /// turn through the (fake) Copilot API with the person's sealed token;
    /// only role and text reach Copilot.
    #[tokio::test]
    async fn copilot_turns_are_person_only_and_stream_from_copilot() {
        use crate::services::copilot::{fake, TEST_CONFIG};
        let mut state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
        let cipher = Arc::new(allternit_cloud_core::CredentialCipher::new("test cipher key material"));
        Arc::get_mut(&mut state).expect("fresh state").credential_cipher = Some(cipher.clone());
        sqlx::raw_sql(include_str!("../../migrations_pg/142_copilot_grants.sql")).execute(&state.db).await.unwrap();
        let person = ("x-allternit-turn", "person");
        let session = app(&state, "sess_1");

        // No App configured: not connected.
        TEST_CONFIG
            .scope(None, async {
                let (status, body) = call(&session, &[person], turn("copilot/gpt-5", false)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_not_connected")), "{body}");
            })
            .await;

        let github = fake::start().await;
        TEST_CONFIG
            .scope(Some(github.config()), async {
                let (status, body) = call(&session, &[person], turn("copilot/gpt-5", true)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_not_connected")), "no grant yet: {body}");
                sqlx::query("INSERT INTO copilot_grants (user_id, github_login, sealed_access, access_expires_at, models) VALUES ($1, 'octo', $2, now() + interval '8 hours', '[\"gpt-5\"]')")
                    .bind(USER)
                    .bind(cipher.encrypt("ghu_fake_user_token").unwrap())
                    .execute(&state.db)
                    .await
                    .unwrap();

                // D7 still holds: no person header, a device token, a tool message.
                let (status, body) = call(&session, &[], turn("copilot/gpt-5", true)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "{body}");
                let (status, body) =
                    call(&session, &[person, ("authorization", "Bearer allternit_runtime_abc")], turn("copilot/gpt-5", true)).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::FORBIDDEN, Some("plan_person_only")), "{body}");
                let mut with_tool = turn("copilot/gpt-5", true);
                with_tool["messages"] = json!([{ "role": "user", "content": "x" }, { "role": "tool", "content": "y", "tool_call_id": "c1" }]);
                let (status, body) = call(&session, &[person], with_tool).await;
                assert_eq!((status, code(&body).as_deref()), (StatusCode::BAD_REQUEST, Some("plan_no_tools")));
                assert!(github.calls.lock().unwrap().chat_bodies.is_empty(), "refused turns never reach Copilot");

                // The person's own send streams Copilot's reply.
                let (status, body) = call(&session, &[person], turn("copilot/gpt-5", true)).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                assert!(body.contains(r#""content":"Hi""#) && body.contains(r#""content":" there""#), "{body}");
                assert!(body.contains(r#""model":"copilot/gpt-5""#) && body.trim_end().ends_with("data: [DONE]"), "{body}");
                let sent = github.calls.lock().unwrap().chat_bodies[0].clone();
                assert_eq!(sent["model"], "gpt-5");
                assert!(sent.get("tools").is_none() && sent.get("tool_choice").is_none(), "{sent}");
                assert_eq!(sent["messages"], json!([{ "role": "user", "content": "Hello" }]));

                // Not streamed: one JSON body with usage.
                let (status, body) = call(&session, &[person], turn("copilot/gpt-5", false)).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                let reply: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(reply["choices"][0]["message"]["content"], "Hi there");
                assert_eq!(reply["usage"]["completion_tokens"], 2);
            })
            .await;
    }
}
