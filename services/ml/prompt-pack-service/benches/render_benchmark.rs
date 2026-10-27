use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, Criterion};
use prompt_pack_service::models::{
    PackDependency, PackVariable, PromptPack, PromptTemplate, RenderOptions, RenderRequest,
};
use prompt_pack_service::renderer::PromptRenderer;
use prompt_pack_service::storage::StorageManager;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::runtime::Runtime;

struct BenchContext {
    _tmp: TempDir,
    renderer: PromptRenderer,
    pack: PromptPack,
    base_request: RenderRequest,
}

fn hash_string(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn build_pack(pack_id: &str, version: &str, prompt_id: &str, template_path: &str) -> PromptPack {
    let variables = vec![
        PackVariable {
            name: "user".to_string(),
            var_type: "object".to_string(),
            required: true,
            default: None,
            description: Some("User profile fields".to_string()),
        },
        PackVariable {
            name: "items".to_string(),
            var_type: "array".to_string(),
            required: true,
            default: None,
            description: Some("List of items to render".to_string()),
        },
        PackVariable {
            name: "context".to_string(),
            var_type: "object".to_string(),
            required: true,
            default: None,
            description: Some("Execution context".to_string()),
        },
    ];

    let prompt = PromptTemplate {
        id: prompt_id.to_string(),
        template: template_path.to_string(),
        description: "Welcome prompt with footer partial".to_string(),
        partials: Some(vec!["footer".to_string()]),
    };

    let content_hash = hash_string(&format!("{pack_id}:{version}:{template_path}"));

    PromptPack {
        pack_id: pack_id.to_string(),
        name: "Benchmark Pack".to_string(),
        description: "Benchmark pack for render performance".to_string(),
        version: version.to_string(),
        deterministic: true,
        author: "allternit".to_string(),
        tags: vec!["benchmark".to_string(), "render".to_string()],
        dependencies: Vec::<PackDependency>::new(),
        variables,
        prompts: vec![prompt],
        created_at: Utc::now(),
        content_hash,
    }
}

fn build_base_request(pack_id: &str, prompt_id: &str, version: &str) -> RenderRequest {
    let mut variables = HashMap::new();
    variables.insert(
        "user".to_string(),
        json!({
            "name": "Ada",
            "tier": "pro",
            "region": "us-east-1"
        }),
    );
    variables.insert(
        "items".to_string(),
        json!(["alpha", "beta", "gamma", "delta"]),
    );
    variables.insert(
        "context".to_string(),
        json!({
            "run_id": "bench-001",
            "locale": "en-US"
        }),
    );

    RenderRequest {
        pack_id: pack_id.to_string(),
        prompt_id: prompt_id.to_string(),
        version: Some(version.to_string()),
        variables,
        options: Some(RenderOptions {
            trim_whitespace: Some(true),
            validate_variables: Some(false),
        }),
    }
}

fn setup(rt: &Runtime) -> BenchContext {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_str().expect("tempdir path");
    let storage = rt
        .block_on(StorageManager::new(data_dir))
        .expect("storage init");
    let storage = Arc::new(storage);
    let renderer = PromptRenderer::new(storage.clone());

    let pack_id = "bench-pack";
    let version = "1.0.0";
    let prompt_id = "welcome";
    let template_path = "prompts/welcome.j2";

    let partial_name = format!("{}/{}/partials/footer", pack_id, version);
    let template = r#"{% set greeting = "Hello" %}
{{ greeting }} {{ user.name }} ({{ user.tier }})
{% for item in items %}- {{ item }}
{% endfor %}
{% include "__PARTIAL__" %}
"#
    .replace("__PARTIAL__", &partial_name);

    let partial = "Generated at {{ context.run_id }} | locale {{ context.locale }}";

    let pack = build_pack(pack_id, version, prompt_id, template_path);

    rt.block_on(async {
        storage
            .store_template(pack_id, version, template_path, &template)
            .await
            .expect("store template");
        storage
            .store_template(pack_id, version, "partials/footer.j2", partial)
            .await
            .expect("store partial");
        storage.store_pack(&pack).await.expect("store pack");
    });

    let base_request = build_base_request(pack_id, prompt_id, version);

    BenchContext {
        _tmp: tmp,
        renderer,
        pack,
        base_request,
    }
}

fn bench_render(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let ctx = setup(&rt);

    rt.block_on(async {
        ctx.renderer
            .render(&ctx.base_request, &ctx.pack)
            .await
            .expect("warm render");
    });

    let counter = AtomicU64::new(0);

    let mut group = c.benchmark_group("prompt_render");
    group.bench_function("warm_cache", |b| {
        b.to_async(&rt).iter(|| ctx.renderer.render(&ctx.base_request, &ctx.pack));
    });

    group.bench_function("cold_render", |b| {
        let ctx = &ctx;
        b.to_async(&rt).iter(|| {
            let mut request = ctx.base_request.clone();
            let nonce = counter.fetch_add(1, Ordering::Relaxed);
            request
                .variables
                .insert("nonce".to_string(), json!(nonce));
            async move { ctx.renderer.render(&request, &ctx.pack).await }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_render);
criterion_main!(benches);
