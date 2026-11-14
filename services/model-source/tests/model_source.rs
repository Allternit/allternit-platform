//! Offline tests for the tricky parts: signature checks, source fall-through,
//! catalog-down fallback and the no-hash rule. Real Freeweights manifests and
//! signatures from testdata/; only the network edge is a local HTTP server.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use allternit_model_source::{
    manifest::key_id_of, verify_manifest, Client, Error, FetchOptions, Source, SourceKind,
    VerificationState, FREEWEIGHTS_KEY_ID, FREEWEIGHTS_PUBLIC_KEY,
};
use axum::{extract::State, http::Uri, response::IntoResponse, Router};
use sha2::{Digest, Sha256};

const FIXTURES: [&str; 3] = [
    "Qwen__Qwen3-Embedding-0.6B",
    "openai__openai_humaneval",
    "Qwen__Qwen3-TTS-Tokenizer-12Hz",
];
const QWEN: &str = "Qwen/Qwen3-Embedding-0.6B";
const QWEN_COMMIT: &str = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3";

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name),
    )
    .unwrap()
}
fn text(name: &str) -> String {
    String::from_utf8(testdata(name)).unwrap()
}
fn config_bytes() -> Vec<u8> {
    // Real bytes of Qwen3-Embedding-0.6B config.json at the pinned commit.
    testdata("Qwen3-Embedding-0.6B.config.json")
}

/// Serves fixed bodies by URL path; anything else is 404.
async fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    async fn handler(
        State(r): State<Arc<HashMap<String, Vec<u8>>>>,
        uri: Uri,
    ) -> impl IntoResponse {
        match r.get(uri.path()) {
            Some(b) => (axum::http::StatusCode::OK, b.clone()),
            None => (axum::http::StatusCode::NOT_FOUND, b"not found".to_vec()),
        }
    }
    let app = Router::new().fallback(handler).with_state(Arc::new(routes));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

fn manifest_routes(fixture: &str, repo: &str, body: Vec<u8>) -> HashMap<String, Vec<u8>> {
    HashMap::from([
        (format!("/manifests/{repo}.json"), body),
        (
            format!("/manifests/{repo}.json.minisig"),
            testdata(&format!("{fixture}.json.minisig")),
        ),
    ])
}

/// Flip one hex digit of the first file sha256: the JSON stays canonical, so
/// only the signature can catch it.
fn flip_byte(bytes: &[u8]) -> Vec<u8> {
    let s = String::from_utf8(bytes.to_vec()).unwrap();
    let at = s.find("\"sha256\": \"").unwrap() + "\"sha256\": \"".len();
    let mut out = bytes.to_vec();
    out[at] = if out[at] == b'0' { b'1' } else { b'0' };
    out
}

#[tokio::test]
async fn good_signature_resolves_signed_sources_in_order() {
    assert_eq!(
        key_id_of(FREEWEIGHTS_PUBLIC_KEY).as_deref(),
        Some(FREEWEIGHTS_KEY_ID)
    );
    for f in FIXTURES {
        let m = verify_manifest(
            &testdata(&format!("{f}.json")),
            &text(&format!("{f}.json.minisig")),
        )
        .unwrap();
        assert_eq!(m.id.replace('/', "__"), f);
    }

    let base = serve(manifest_routes(
        FIXTURES[0],
        QWEN,
        testdata(&format!("{}.json", FIXTURES[0])),
    ))
    .await;
    let client = Client::with_urls(&base, "http://127.0.0.1:1").unwrap();
    let res = client.resolve(QWEN, None).await.unwrap();
    assert_eq!(res.basis, VerificationState::FreeweightsSigned);
    assert_eq!(res.revision, QWEN_COMMIT);
    let kinds: Vec<_> = res.sources.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [SourceKind::FreeweightsWorker, SourceKind::ManifestSource]
    );
    assert_eq!(
        res.sources[0].file_url("1_Pooling/config.json"),
        format!("{base}/{QWEN}/resolve/{QWEN_COMMIT}/1_Pooling/config.json")
    );
    assert_eq!(
        res.sources[1].base_url,
        format!("https://huggingface.co/{QWEN}/resolve/{QWEN_COMMIT}/")
    );
}

#[tokio::test]
async fn flipped_byte_fails_and_never_falls_back() {
    for f in FIXTURES {
        let err = verify_manifest(
            &flip_byte(&testdata(&format!("{f}.json"))),
            &text(&format!("{f}.json.minisig")),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Signature(_)), "{f}: {err}");
    }
    // Served by the catalog: resolve must error, not quietly use the publisher.
    let tampered = flip_byte(&testdata(&format!("{}.json", FIXTURES[0])));
    let base = serve(manifest_routes(FIXTURES[0], QWEN, tampered)).await;
    let client = Client::with_urls(&base, &base).unwrap();
    assert!(matches!(
        client.resolve(QWEN, None).await,
        Err(Error::Signature(_))
    ));
}

#[test]
fn wrong_key_id_fails() {
    let err = verify_manifest(
        &testdata("openai__openai_humaneval.json"),
        &text("openai__openai_humaneval.json.otherkey.minisig"),
    )
    .unwrap_err();
    match err {
        Error::KeyId { expected, found } => {
            assert_eq!(expected, FREEWEIGHTS_KEY_ID);
            assert_eq!(found, "9505DB75F0202719");
        }
        other => panic!("want KeyId, got {other}"),
    }
}

#[tokio::test]
async fn hash_mismatch_deletes_and_falls_through_to_next_source() {
    let mut routes = manifest_routes(
        FIXTURES[0],
        QWEN,
        testdata(&format!("{}.json", FIXTURES[0])),
    );
    let mut corrupt = config_bytes();
    corrupt[10] ^= 0x01; // same size, wrong hash
    routes.insert(
        format!("/{QWEN}/resolve/{QWEN_COMMIT}/config.json"),
        corrupt,
    );
    routes.insert("/mirror/config.json".into(), config_bytes());
    let base = serve(routes).await;
    let client = Client::with_urls(&base, "http://127.0.0.1:1").unwrap();
    let mut res = client.resolve(QWEN, None).await.unwrap();
    // Second source = a local mirror instead of huggingface.co, so the test is offline.
    res.sources[1] = Source {
        kind: SourceKind::ManifestHosted,
        base_url: format!("{base}/mirror/"),
    };

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("config.json");
    let out = client
        .fetch_verified(&res, "config.json", &dest, &FetchOptions::default())
        .await
        .unwrap();
    assert_eq!(out.verification.state, VerificationState::FreeweightsSigned);
    assert_eq!(out.verification.source_used, SourceKind::ManifestHosted);
    assert_eq!(
        out.verification.manifest_commit.as_deref(),
        Some(QWEN_COMMIT)
    );
    assert_eq!(out.failed.len(), 1);
    assert_eq!(out.failed[0].source, SourceKind::FreeweightsWorker);
    assert!(
        out.failed[0].reason.starts_with("mismatch"),
        "{}",
        out.failed[0].reason
    );
    assert_eq!(std::fs::read(&dest).unwrap(), config_bytes());
    assert!(!dir.path().join("config.json.part").exists());

    // Every source bad: error, nothing left on disk.
    res.sources.truncate(1);
    let dest2 = dir.path().join("again.json");
    let err = client
        .fetch_verified(&res, "config.json", &dest2, &FetchOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::AllSourcesFailed { .. }), "{err}");
    assert!(!dest2.exists() && !dir.path().join("again.json.part").exists());
}

fn publisher_routes() -> HashMap<String, Vec<u8>> {
    let sha = hex::encode(Sha256::digest(config_bytes()));
    let tree = serde_json::json!([
        {"type": "file", "path": "config.json", "size": 135, "oid": "x",
         "lfs": {"oid": sha, "size": config_bytes().len(), "pointerSize": 135}},
        // Real HF tree entry for this non-LFS file at the pinned commit (git blob SHA-1).
        {"type": "file", "path": "blob/config.json", "size": 727, "oid": "cef2749ee93607b8f9a58ec72f4f6bfaf874e71d"},
        {"type": "file", "path": "flipped/config.json", "size": 727, "oid": "cef2749ee93607b8f9a58ec72f4f6bfaf874e71d"},
        // No usable hash at all.
        {"type": "file", "path": "tokenizer_config.json", "size": 727}
    ]);
    let mut flipped = config_bytes();
    flipped[10] ^= 0x01;
    HashMap::from([
        (
            format!("/api/models/{QWEN}/tree/main"),
            serde_json::to_vec(&tree).unwrap(),
        ),
        (format!("/{QWEN}/resolve/main/config.json"), config_bytes()),
        (
            format!("/{QWEN}/resolve/main/blob/config.json"),
            config_bytes(),
        ),
        (format!("/{QWEN}/resolve/main/flipped/config.json"), flipped),
        (
            format!("/{QWEN}/resolve/main/tokenizer_config.json"),
            config_bytes(),
        ),
    ])
}

#[tokio::test]
async fn catalog_down_gives_publisher_hash() {
    let hf = serve(publisher_routes()).await;
    let client = Client::with_urls("http://127.0.0.1:1", &hf).unwrap();
    let res = client.resolve(QWEN, None).await.unwrap();
    assert_eq!(res.basis, VerificationState::PublisherHash);
    assert!(res
        .fallback_reason
        .as_deref()
        .unwrap_or("")
        .contains("unreachable"));

    let dir = tempfile::tempdir().unwrap();
    let out = client
        .fetch_verified(
            &res,
            "config.json",
            &dir.path().join("config.json"),
            &FetchOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.verification.state, VerificationState::PublisherHash);
    assert_eq!(out.verification.source_used, SourceKind::Publisher);
    assert_eq!(out.verification.manifest_commit, None);
}

#[tokio::test]
async fn no_hash_is_an_error_unless_allowed() {
    let hf = serve(publisher_routes()).await;
    let client = Client::with_urls("http://127.0.0.1:1", &hf).unwrap();
    let res = client.resolve(QWEN, None).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("tokenizer_config.json");

    let err = client
        .fetch_verified(
            &res,
            "tokenizer_config.json",
            &dest,
            &FetchOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NoHash(_)), "{err}");
    assert!(!dest.exists());

    let pin = FetchOptions {
        expected_sha256: Some(hex::encode(Sha256::digest(config_bytes()))),
        allow_unverified: false,
    };
    let out = client
        .fetch_verified(&res, "tokenizer_config.json", &dest, &pin)
        .await
        .unwrap();
    assert_eq!(out.verification.state, VerificationState::Pinned);

    let allow = FetchOptions {
        expected_sha256: None,
        allow_unverified: true,
    };
    let out = client
        .fetch_verified(&res, "tokenizer_config.json", &dest, &allow)
        .await
        .unwrap();
    assert_eq!(out.verification.state, VerificationState::Unverified);
}

#[test]
fn contract_fixtures_match_rust_types() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/model-source/fixtures.json");
    let fx: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for case in fx["valid"].as_array().unwrap() {
        let v: allternit_model_source::Verification =
            serde_json::from_value(case["verification"].clone())
                .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            case["verification"],
            "{}",
            case["name"]
        );
    }
    for case in fx["invalid"].as_array().unwrap() {
        let r = serde_json::from_value::<allternit_model_source::Verification>(
            case["verification"].clone(),
        );
        assert!(r.is_err(), "{} should be rejected", case["name"]);
    }
}

#[tokio::test]
async fn catalog_down_non_lfs_file_verifies_by_git_blob_id() {
    let hf = serve(publisher_routes()).await;
    let client = Client::with_urls("http://127.0.0.1:1", &hf).unwrap();
    let res = client.resolve(QWEN, None).await.unwrap();
    let dir = tempfile::tempdir().unwrap();

    let dest = dir.path().join("config.json");
    let out = client
        .fetch_verified(&res, "blob/config.json", &dest, &FetchOptions::default())
        .await
        .unwrap();
    assert_eq!(out.verification.state, VerificationState::PublisherHash);
    assert_eq!(out.verification.source_used, SourceKind::Publisher);
    assert_eq!(std::fs::read(&dest).unwrap(), config_bytes());

    let bad = dir.path().join("flipped.json");
    let err = client
        .fetch_verified(&res, "flipped/config.json", &bad, &FetchOptions::default())
        .await
        .unwrap_err();
    match err {
        Error::AllSourcesFailed { attempts, .. } => {
            assert!(
                attempts[0].reason.contains("git blob"),
                "{}",
                attempts[0].reason
            )
        }
        other => panic!("want AllSourcesFailed, got {other}"),
    }
    assert!(!bad.exists() && !dir.path().join("flipped.json.part").exists());
}
