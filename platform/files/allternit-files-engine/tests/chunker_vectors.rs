//! The Rust chunker is the normative FastCDC for `files/2`. This test pins its cut
//! points in `contracts/files-drive/vectors/chunker-v1.json`, which the web, Swift and
//! Kotlin chunkers test against so every client cuts the same chunks (cross-client
//! dedup). The file carries the GEAR table and masks, so ports need nothing else.
//!
//! Regenerate (only when the chunker is meant to change, which breaks dedup):
//!   ALLTERNIT_WRITE_CHUNKER_VECTORS=1 cargo test -p allternit-files-engine --test chunker_vectors

use allternit_files_engine::chunker::{self, chunk_bytes};
use serde_json::{json, Value};

/// splitmix64, little-endian u64 words, truncated to `len`. Easy to port exactly.
fn splitmix64_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        out.extend_from_slice(&z.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn vectors() -> Value {
    let (gear, _) = fastcdc::v2020::get_gear_with_seed(0);
    let bits = fastcdc::v2020::logarithm2(chunker::AVG_BYTES);
    let mask_s = fastcdc::v2020::MASKS[(bits + 1) as usize];
    let mask_l = fastcdc::v2020::MASKS[(bits - 1) as usize];
    let cases: Vec<(&str, Value, Vec<u8>)> = vec![
        ("splitmix64 100 KiB (below min: one chunk)", json!({"kind": "splitmix64", "seed": 7, "len": 102_400}), splitmix64_bytes(7, 102_400)),
        ("splitmix64 3 MiB", json!({"kind": "splitmix64", "seed": 1, "len": 3 << 20}), splitmix64_bytes(1, 3 << 20)),
        ("splitmix64 9 MiB + 12345", json!({"kind": "splitmix64", "seed": 2, "len": (9 << 20) + 12_345}), splitmix64_bytes(2, (9 << 20) + 12_345)),
        ("zeros 9 MiB (no content boundary: max cuts)", json!({"kind": "zeros", "len": 9 << 20}), vec![0u8; 9 << 20]),
    ];
    let cases: Vec<Value> = cases
        .into_iter()
        .map(|(name, input, data)| {
            let c = chunk_bytes(&data);
            json!({
                "name": name,
                "input": input,
                "size": c.size,
                "content_hash": c.content_hash,
                "chunks": c.chunks.iter().map(|r| json!({"offset": r.offset, "size": r.size, "hash": r.hash})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "chunker": "fastcdc-2020",
        "hash": "blake3",
        "normative": "allternit-files-engine (Rust, fastcdc crate 3.2.1 v2020)",
        "params": {
            "min_bytes": chunker::MIN_BYTES,
            "avg_bytes": chunker::AVG_BYTES,
            "max_bytes": chunker::MAX_BYTES,
            "normalization_level": 1,
            "gear_seed": 0,
            "mask_s": format!("{mask_s:#018x}"),
            "mask_l": format!("{mask_l:#018x}"),
            "mask_s_ls": format!("{:#018x}", mask_s << 1),
            "mask_l_ls": format!("{:#018x}", mask_l << 1),
        },
        "algorithm": [
            "Cut the next chunk from `src` (the bytes not yet chunked), n = len(src).",
            "If n <= min: the chunk is all n bytes.",
            "center = avg; if n > max then n = max, else if n < center then center = n.",
            "i = min / 2; h = 0 (u64, wrapping arithmetic).",
            "While i < center / 2: a = 2*i; h = (h << 2) + GEAR_LS[src[a]]; if h & mask_s_ls == 0 cut at length a; h = h + GEAR[src[a+1]]; if h & mask_s == 0 cut at length a+1; i += 1.",
            "While i < n / 2: same two steps with mask_l_ls and mask_l.",
            "Otherwise cut at length n.",
            "GEAR_LS[k] = GEAR[k] << 1 (wrapping). Chunk hash = blake3-256 of the chunk bytes, lower-case hex.",
            "Input kinds: splitmix64 = x += 0x9E3779B97F4A7C15; z = x; z = (z ^ z>>30) * 0xBF58476D1CE4E5B9; z = (z ^ z>>27) * 0x94D049BB133111EB; z ^= z>>31; append z as 8 little-endian bytes; truncate to len. zeros = len zero bytes."
        ],
        "gear": gear.iter().map(|g| format!("{g:#018x}")).collect::<Vec<_>>(),
        "cases": cases,
    })
}

#[test]
fn chunker_matches_the_published_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/files-drive/vectors/chunker-v1.json");
    let want = vectors();
    if std::env::var_os("ALLTERNIT_WRITE_CHUNKER_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
    }
    let have: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("vectors file")).unwrap();
    assert_eq!(have, want, "the chunker no longer cuts like the published vectors (cross-client dedup breaks)");
    // The max cut is exercised: the zero input is cut at exactly 4 MiB.
    let zeros = &want["cases"][3]["chunks"];
    assert_eq!(zeros[0]["size"], json!(chunker::MAX_BYTES));
}
