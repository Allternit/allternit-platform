//! LUKS2 state partition (plan step 30b). Pure logic only, so the tests run on any host;
//! `linux.rs` reads the header and makes the dm-crypt device.
//!
//! PID 1 has no cryptsetup. It reads the LUKS2 header (binary header + JSON area), takes the
//! data segment (offset, cipher, sector size, IV tweak), and loads a `crypt` device-mapper
//! table with the volume key. The key source for now is a TEST ONLY volume key file baked into
//! the initramfs by mkimage.sh (`/etc/allternit/TEST-ONLY-state.key`).
//! TODO(31): bind the key to the TPM / Secure Boot state and drop the baked key file.
//! Step 31: the volume key is checked against the LUKS2 digest (PBKDF2-HMAC-SHA256 over the
//! key, as cryptsetup stores it in the header JSON) before dm-crypt is set up; the ext4
//! superblock read behind the new device stays as a second check.

use serde_json::Value;
use sha2::{Digest as _, Sha256};

pub const MAGIC: &[u8; 6] = b"LUKS\xba\xbe";
/// The binary header is 4096 bytes; the JSON area follows it.
pub const BIN_HEADER: usize = 4096;
/// Largest LUKS2 header (binary + JSON) the spec allows.
const MAX_HEADER: u64 = 4 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Byte offset of the encrypted data on the partition.
    pub offset: u64,
    /// Encrypted size in bytes; `None` = to the end of the partition ("dynamic").
    pub size: Option<u64>,
    pub iv_tweak: u64,
    /// dm-crypt cipher spec, e.g. `aes-xts-plain64`.
    pub encryption: String,
    pub sector_size: u64,
    /// Volume key length in bytes (from the keyslots).
    pub key_size: usize,
}

pub fn is_luks(buf: &[u8]) -> bool {
    buf.len() >= 8 && &buf[..6] == MAGIC
}

/// Total header size (binary + JSON) from the first 4096 bytes of the partition.
pub fn header_size(bin: &[u8]) -> Result<u64, String> {
    if bin.len() < 16 || !is_luks(bin) {
        return Err("no LUKS magic".into());
    }
    let version = u16::from_be_bytes([bin[6], bin[7]]);
    if version != 2 {
        return Err(format!("LUKS version {version}, want 2"));
    }
    let size = u64::from_be_bytes(bin[8..16].try_into().unwrap());
    if size <= BIN_HEADER as u64 || size > MAX_HEADER {
        return Err(format!("bad LUKS2 header size {size}"));
    }
    Ok(size)
}

fn num(v: &Value, what: &str) -> Result<u64, String> {
    match v {
        Value::String(s) => s.parse::<u64>().map_err(|_| format!("{what} is not a number: {s}")),
        Value::Number(n) => n.as_u64().ok_or_else(|| format!("{what} is not a whole number")),
        _ => Err(format!("{what} is missing")),
    }
}

fn header_json(hdr: &[u8]) -> Result<Value, String> {
    let size = header_size(hdr)? as usize;
    if hdr.len() < size {
        return Err(format!("header is {size} bytes, have {}", hdr.len()));
    }
    let area = &hdr[BIN_HEADER..size];
    let end = area.iter().position(|&b| b == 0).unwrap_or(area.len());
    serde_json::from_slice(&area[..end]).map_err(|e| format!("LUKS2 JSON: {e}"))
}

/// Data segment 0 of a LUKS2 header (`hdr` = the whole header, `header_size` bytes).
pub fn parse(hdr: &[u8]) -> Result<Segment, String> {
    let json = header_json(hdr)?;
    let seg = json.pointer("/segments/0").ok_or("no data segment 0")?;
    if seg["type"].as_str() != Some("crypt") {
        return Err("segment 0 is not a crypt segment".into());
    }
    let offset = num(&seg["offset"], "segment offset")?;
    let size = match &seg["size"] {
        Value::String(s) if s == "dynamic" => None,
        v => Some(num(v, "segment size")?),
    };
    let iv_tweak = if seg["iv_tweak"].is_null() { 0 } else { num(&seg["iv_tweak"], "iv_tweak")? };
    let encryption = seg["encryption"].as_str().ok_or("segment has no encryption")?.to_string();
    if encryption.is_empty() || encryption.contains(char::is_whitespace) {
        return Err("bad cipher spec".into());
    }
    let sector_size = if seg["sector_size"].is_null() { 512 } else { num(&seg["sector_size"], "sector_size")? };
    if !(512..=4096).contains(&sector_size) || !sector_size.is_power_of_two() {
        return Err(format!("bad sector size {sector_size}"));
    }
    if offset % 512 != 0 || offset == 0 {
        return Err(format!("segment offset {offset} is not a positive multiple of 512"));
    }
    let key_size = json["keyslots"]
        .as_object()
        .and_then(|m| m.values().find_map(|k| k["key_size"].as_u64()))
        .ok_or("no keyslot with a key_size")? as usize;
    if key_size == 0 || key_size > 128 {
        return Err(format!("bad key size {key_size}"));
    }
    Ok(Segment { offset, size, iv_tweak, encryption, sector_size, key_size })
}

impl Segment {
    /// (length in 512-byte sectors, `crypt` target parameters) over `dev` (`dev_bytes` long).
    pub fn table(&self, dev: &str, dev_bytes: u64, key: &[u8]) -> Result<(u64, String), String> {
        if key.len() != self.key_size {
            return Err(format!("key is {} bytes, the header wants {}", key.len(), self.key_size));
        }
        if dev_bytes <= self.offset {
            return Err(format!("partition ({dev_bytes} bytes) ends before the data segment"));
        }
        let bytes = match self.size {
            Some(s) if self.offset + s <= dev_bytes => s,
            Some(s) => return Err(format!("segment size {s} runs past the partition")),
            None => dev_bytes - self.offset,
        };
        let bytes = bytes - bytes % self.sector_size;
        if bytes == 0 {
            return Err("empty data segment".into());
        }
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        let mut p = format!("{} {hex} {} {dev} {}", self.encryption, self.iv_tweak, self.offset / 512);
        if self.sector_size != 512 {
            p.push_str(&format!(" 1 sector_size:{}", self.sector_size));
        }
        Ok((bytes / 512, p))
    }
}

/// The PBKDF2 digest that vouches for the volume key of data segment 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDigest {
    pub iterations: u32,
    pub salt: Vec<u8>,
    pub digest: Vec<u8>,
}

/// Standard base64 (with padding), which is how LUKS2 stores salts and digests.
pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err("bad base64 in the LUKS2 header".into()),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// The digest entry for segment 0 (type pbkdf2, hash sha256).
pub fn key_digest(hdr: &[u8]) -> Result<KeyDigest, String> {
    let json = header_json(hdr)?;
    let digests = json["digests"].as_object().ok_or("no digests in the LUKS2 header")?;
    let d = digests
        .values()
        .find(|d| {
            d["type"].as_str() == Some("pbkdf2")
                && d["segments"].as_array().map_or(false, |s| s.iter().any(|x| x.as_str() == Some("0")))
        })
        .ok_or("no pbkdf2 digest for data segment 0")?;
    if d["hash"].as_str() != Some("sha256") {
        return Err("LUKS2 digest hash is not sha256".into());
    }
    let iterations = d["iterations"].as_u64().filter(|&n| n > 0 && n <= u32::MAX as u64).ok_or("bad digest iterations")? as u32;
    let salt = b64_decode(d["salt"].as_str().ok_or("digest has no salt")?)?;
    let digest = b64_decode(d["digest"].as_str().ok_or("digest has no digest")?)?;
    if digest.is_empty() || digest.len() > 32 || salt.is_empty() {
        return Err("bad LUKS2 digest".into());
    }
    Ok(KeyDigest { iterations, salt, digest })
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut h = Sha256::new();
    h.update(ipad);
    for p in parts {
        h.update(p);
    }
    let inner = h.finalize();
    let mut h = Sha256::new();
    h.update(opad);
    h.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// PBKDF2-HMAC-SHA256.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 32);
    let mut block = 1u32;
    while out.len() < len {
        let mut u = hmac_sha256(password, &[salt, &block.to_be_bytes()]);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha256(password, &[&u]);
            for (a, b) in t.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        out.extend_from_slice(&t);
        block += 1;
    }
    out.truncate(len);
    out
}

/// Check `key` against the LUKS2 digest in the header: a wrong key fails here, before any
/// device-mapper table exists.
pub fn verify_key(hdr: &[u8], key: &[u8]) -> Result<(), String> {
    let d = key_digest(hdr)?;
    let got = pbkdf2_sha256(key, &d.salt, d.iterations, d.digest.len());
    let diff = got.iter().zip(d.digest.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if diff == 0 {
        Ok(())
    } else {
        Err("key does not match the LUKS2 digest (wrong key?)".into())
    }
}

/// True when `start` (the first 2 KiB of a device) holds an ext4 superblock.
pub fn has_ext4_magic(start: &[u8]) -> bool {
    start.len() >= 1024 + 58 && start[1024 + 56] == 0x53 && start[1024 + 57] == 0xEF
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(json: &str) -> Vec<u8> {
        let mut h = vec![0u8; 16384];
        h[..6].copy_from_slice(MAGIC);
        h[6..8].copy_from_slice(&2u16.to_be_bytes());
        h[8..16].copy_from_slice(&16384u64.to_be_bytes());
        h[BIN_HEADER..BIN_HEADER + json.len()].copy_from_slice(json.as_bytes());
        h
    }

    const JSON: &str = r#"{"keyslots":{"0":{"type":"luks2","key_size":64}},
        "segments":{"0":{"type":"crypt","offset":"16777216","size":"dynamic","iv_tweak":"0",
        "encryption":"aes-xts-plain64","sector_size":512}},"digests":{},"config":{}}"#;

    #[test]
    fn parses_cryptsetup_style_header() {
        let h = header(JSON);
        assert_eq!(header_size(&h[..4096]).unwrap(), 16384);
        let s = parse(&h).unwrap();
        assert_eq!(
            s,
            Segment {
                offset: 16 << 20,
                size: None,
                iv_tweak: 0,
                encryption: "aes-xts-plain64".into(),
                sector_size: 512,
                key_size: 64
            }
        );
        let key = [0xabu8; 64];
        let (sectors, p) = s.table("/dev/vda4", 80 << 20, &key).unwrap();
        assert_eq!(sectors, (64u64 << 20) / 512);
        assert_eq!(p, format!("aes-xts-plain64 {} 0 /dev/vda4 32768", "ab".repeat(64)));
        assert!(s.table("/dev/vda4", 80 << 20, &key[..32]).is_err());
        assert!(s.table("/dev/vda4", 16 << 20, &key).is_err());
    }

    #[test]
    fn large_sectors_add_the_option() {
        let h = header(&JSON.replace("\"sector_size\":512", "\"sector_size\":4096"));
        let (_, p) = parse(&h).unwrap().table("/dev/vda4", 80 << 20, &[1u8; 64]).unwrap();
        assert!(p.ends_with(" 32768 1 sector_size:4096"));
    }

    #[test]
    fn rejects_bad_headers() {
        assert!(header_size(&[0u8; 4096]).is_err());
        let mut v1 = header(JSON);
        v1[6..8].copy_from_slice(&1u16.to_be_bytes());
        assert!(parse(&v1).is_err());
        assert!(parse(&header("{not json")).is_err());
        assert!(parse(&header(&JSON.replace("\"crypt\"", "\"linear\""))).is_err());
        assert!(parse(&header(&JSON.replace("\"key_size\":64", "\"x\":1"))).is_err());
    }

    // Made with python hashlib.pbkdf2_hmac('sha256', b"\xab" * 64, b"saltsalt" * 4, 1000, 32).
    const DIGEST_JSON: &str = r#"{"keyslots":{"0":{"type":"luks2","key_size":64}},
        "segments":{"0":{"type":"crypt","offset":"16777216","size":"dynamic","iv_tweak":"0",
        "encryption":"aes-xts-plain64","sector_size":512}},
        "digests":{"0":{"type":"pbkdf2","keyslots":["0"],"segments":["0"],"hash":"sha256",
        "iterations":1000,"salt":"c2FsdHNhbHRzYWx0c2FsdHNhbHRzYWx0c2FsdHNhbHQ=",
        "digest":"0MZkAr02zp2LA/WFQ5u1m+Nydos09ZFXHUIk/s746ro="}},"config":{}}"#;

    #[test]
    fn pbkdf2_matches_the_reference_vector() {
        let got = pbkdf2_sha256(&[0xab; 64], b"saltsaltsaltsaltsaltsaltsaltsalt", 1000, 32);
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "d0c66402bd36ce9d8b03f585439bb59be372768b34f591571d4224fecef8eaba");
    }

    #[test]
    fn checks_the_key_against_the_digest() {
        let h = header(DIGEST_JSON);
        assert!(verify_key(&h, &[0xab; 64]).is_ok());
        assert!(verify_key(&h, &[0xac; 64]).is_err());
        assert!(verify_key(&header(JSON), &[0xab; 64]).is_err(), "no digest = no open");
        assert_eq!(b64_decode("aGk=").unwrap(), b"hi");
        assert!(b64_decode("a*b").is_err());
    }

    #[test]
    fn ext4_magic() {
        let mut b = vec![0u8; 2048];
        assert!(!has_ext4_magic(&b));
        b[1080] = 0x53;
        b[1081] = 0xEF;
        assert!(has_ext4_magic(&b));
    }
}
