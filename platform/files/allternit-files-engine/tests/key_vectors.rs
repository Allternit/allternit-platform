//! The Files key vectors (`contracts/files-drive/vectors/keys-v1.json`) were
//! made from the SPEC section 13 construction with independent code
//! (node:crypto X25519, @noble/hashes, @noble/ciphers), the same libraries the
//! web client uses. The engine must open them, or a key wrapped on one
//! surface would not open on another.

use allternit_files_core::KeyEnvelope;
use allternit_files_engine::crypto::ChunkSealer;
use allternit_files_engine::replica::{self, Head};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../../../../contracts/files-drive/vectors/keys-v1.json")).unwrap()
}

fn b32(v: &serde_json::Value) -> [u8; 32] {
    URL_SAFE_NO_PAD.decode(v.as_str().unwrap()).unwrap().try_into().unwrap()
}

fn bytes(v: &serde_json::Value) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn confirm_code_matches_the_vector() {
    let v = vectors();
    assert_eq!(replica::confirm_code(&b32(&v["confirm_code"]["device_pub"])), v["confirm_code"]["code"].as_str().unwrap());
    let e = &v["envelope"];
    assert_eq!(replica::confirm_code(&b32(&e["device_pub"])), e["confirm_code"].as_str().unwrap());
}

#[test]
fn an_envelope_wrapped_by_the_web_client_opens_and_the_replica_reads() {
    let v = vectors();
    let e = &v["envelope"];
    let env: KeyEnvelope = serde_json::from_value(e["envelope"].clone()).unwrap();
    assert_eq!(env.wrap_alg, replica::WRAP_ALG);
    let frk = replica::unwrap_root_key(
        &env.key_id,
        &b32(&e["shared"]),
        &b32(&serde_json::json!(env.ephemeral_pub)),
        &b32(&e["device_pub"]),
        &env.device_id,
        &env.nonce,
        &env.wrapped_key,
    )
    .expect("the vector envelope opens");
    // Wrong device id = wrong KEK info and AAD.
    assert!(replica::unwrap_root_key(&env.key_id, &b32(&e["shared"]), &b32(&serde_json::json!(env.ephemeral_pub)), &b32(&e["device_pub"]), "dev_other_phone", &env.nonce, &env.wrapped_key).is_err());

    let r = &v["replica"];
    let rid = r["replica_id"].as_str().unwrap();
    assert_eq!(
        replica::replica_id(r["source_computer_id"].as_str().unwrap(), r["root_id"].as_str().unwrap(), r["host_device_id"].as_str().unwrap()),
        rid
    );
    let sealer = ChunkSealer::for_replica(&frk, rid);
    let hash = r["plaintext_hash"].as_str().unwrap();
    assert_eq!(sealer.locator(hash).unwrap(), r["locator"].as_str().unwrap());
    let blob = bytes(&r["blob_b64"]);
    assert_eq!(replica::open_blob(&sealer, &blob, hash).unwrap(), bytes(&r["plaintext_b64"]));
    let head = replica::open_head(&frk, rid, &bytes(&r["head_blob_b64"])).unwrap();
    let want: Head = serde_json::from_value(r["head"].clone()).unwrap();
    assert_eq!(head, want);

    // One flipped ciphertext byte fails.
    let mut bad = blob.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(replica::open_blob(&sealer, &bad, hash).is_err());
}
