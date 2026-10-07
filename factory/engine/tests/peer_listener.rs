//! The peer listener over HTTP: only a valid ticket for this computer gets
//! in, and the caller is always the ticket's user.

use std::collections::HashMap;
use std::sync::Arc;

use allternit_factory_engine::api::peer::{peer_router, PeerOptions, PeerVerifier, PEER_SCOPE};
use allternit_factory_engine::api::service::ServiceState;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use tempfile::TempDir;

fn mint(key: &SigningKey, aud: &str) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let h = B64.encode(serde_json::to_vec(&json!({ "alg": "EdDSA", "typ": "JWT", "kid": "k1" })).unwrap());
    let p = B64.encode(
        serde_json::to_vec(&json!({ "iss": "allternit-cloud-api", "sub": "u_mate", "aud": aud, "iat": now, "nbf": now,
            "exp": now + 300, "scope": PEER_SCOPE, "jti": "j" }))
        .unwrap(),
    );
    let sig = key.sign(format!("{h}.{p}").as_bytes());
    format!("{h}.{p}.{}", B64.encode(sig.to_bytes()))
}

#[tokio::test]
async fn only_a_ticket_for_this_computer_gets_in() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(ServiceState::new(dir.path().to_path_buf()).await.unwrap());
    let key = SigningKey::from_bytes(&[5u8; 32]);
    let opts = PeerOptions {
        port: 0,
        computer_id: "pc_1".into(),
        jwks_url: "http://unused.invalid/jwks".into(),
        issuer: "allternit-cloud-api".into(),
    };
    let verifier = Arc::new(PeerVerifier::with_keys(opts, HashMap::from([("k1".to_string(), key.verifying_key())])));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, peer_router(state, verifier)).await.unwrap() });
    let http = reqwest::Client::new();
    let hello = format!("{base}/api/factory/peer/hello");

    // A good ticket: the caller is the ticket's user, whatever header was sent.
    let r = http
        .get(&hello)
        .bearer_auth(mint(&key, "pc_1"))
        .header("x-allternit-user", "u_spoofed")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!((body["computerId"].as_str(), body["user"].as_str()), (Some("pc_1"), Some("u_mate")), "{body}");

    // No ticket, or one for another computer: refused in the contract's shape.
    let r = http.get(&hello).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = http.get(&hello).bearer_auth(mint(&key, "pc_2")).send().await.unwrap();
    assert_eq!(r.status(), 403);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["fact"], "ticket is for another computer");

    // The loopback API is not served on the peer port.
    let r = http.get(format!("{base}/api/factory/agents")).bearer_auth(mint(&key, "pc_1")).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

/// The verifier fetches cloud-api's key set, and fetches again when a ticket
/// names a key it hasn't seen (rotation).
#[tokio::test]
async fn keys_come_from_the_jwks_url_and_follow_rotation() {
    use std::sync::Mutex;
    let old = SigningKey::from_bytes(&[6u8; 32]);
    let new = SigningKey::from_bytes(&[7u8; 32]);
    let jwk = |kid: &str, key: &SigningKey| {
        json!({ "alg": "EdDSA", "crv": "Ed25519", "kid": kid, "kty": "OKP", "use": "sig", "x": B64.encode(key.verifying_key().to_bytes()) })
    };
    let served = Arc::new(Mutex::new(json!({ "keys": [jwk("k1", &old)] })));
    let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (s2, f2) = (served.clone(), fetches.clone());
    let app = axum::Router::new().route(
        "/api/v1/auth/dp-jwks",
        axum::routing::get(move || {
            let (s, f) = (s2.clone(), f2.clone());
            async move {
                f.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::Json(s.lock().unwrap().clone())
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cloud = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let verifier = PeerVerifier::new(PeerOptions {
        port: 0,
        computer_id: "pc_1".into(),
        jwks_url: format!("{cloud}/api/v1/auth/dp-jwks"),
        issuer: "allternit-cloud-api".into(),
    });
    assert_eq!(verifier.verify(&mint(&old, "pc_1")).await.unwrap().user_id, "u_mate");
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);

    // cloud-api rotates to k2. Inside the refetch floor a k2 ticket is
    // refused as an unknown key (a burst can't hammer cloud-api).
    *served.lock().unwrap() = json!({ "keys": [jwk("k2", &new)] });
    let rotated = {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let h = B64.encode(serde_json::to_vec(&json!({ "alg": "EdDSA", "typ": "JWT", "kid": "k2" })).unwrap());
        let p = B64.encode(
            serde_json::to_vec(&json!({ "iss": "allternit-cloud-api", "sub": "u_mate", "aud": "pc_1", "iat": now, "nbf": now,
                "exp": now + 300, "scope": PEER_SCOPE, "jti": "j" }))
            .unwrap(),
        );
        let sig = new.sign(format!("{h}.{p}").as_bytes());
        format!("{h}.{p}.{}", B64.encode(sig.to_bytes()))
    };
    assert_eq!(verifier.verify(&rotated).await.unwrap_err().to_string(), "unknown signing key");
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1, "no refetch inside the floor");

    // Past the floor, the unknown key makes it fetch again and accept.
    let verifier = verifier.with_refetch_floor(std::time::Duration::ZERO);
    assert_eq!(verifier.verify(&rotated).await.unwrap().user_id, "u_mate");
    assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 2);
}
