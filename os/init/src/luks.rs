//! LUKS2 state partition (plan step 30b). Pure logic only, so the tests run on any host;
//! `linux.rs` reads the header and makes the dm-crypt device.
//!
//! PID 1 has no cryptsetup. It reads the LUKS2 header (binary header + JSON area), takes the
//! data segment (offset, cipher, sector size, IV tweak), and loads a `crypt` device-mapper
//! table with the volume key. The key source for now is a TEST ONLY volume key file baked into
//! the initramfs by mkimage.sh (`/etc/allternit/TEST-ONLY-state.key`).
//! TODO(31): bind the key to the TPM / Secure Boot state and drop the baked key file.
//! TODO(31): check the key against the LUKS2 digest (PBKDF2). For now a wrong key is caught
//! by reading the ext4 superblock through the new device before anything mounts it.

use serde_json::Value;

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

/// Data segment 0 of a LUKS2 header (`hdr` = the whole header, `header_size` bytes).
pub fn parse(hdr: &[u8]) -> Result<Segment, String> {
    let size = header_size(hdr)? as usize;
    if hdr.len() < size {
        return Err(format!("header is {size} bytes, have {}", hdr.len()));
    }
    let area = &hdr[BIN_HEADER..size];
    let end = area.iter().position(|&b| b == 0).unwrap_or(area.len());
    let json: Value = serde_json::from_slice(&area[..end]).map_err(|e| format!("LUKS2 JSON: {e}"))?;
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

    #[test]
    fn ext4_magic() {
        let mut b = vec![0u8; 2048];
        assert!(!has_ext4_magic(&b));
        b[1080] = 0x53;
        b[1081] = 0xEF;
        assert!(has_ext4_magic(&b));
    }
}
