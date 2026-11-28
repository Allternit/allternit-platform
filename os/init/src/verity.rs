//! dm-verity for the A/B system slots (plan step 30a). Pure logic only, so the tests run on
//! any host: kernel command line parsing, the verity table line and the device-mapper ioctl
//! buffer. `linux.rs` makes the syscalls.
//!
//! Command line (mkimage.sh writes it into the UKI):
//!   allternit.slot=a allternit.verity.a=<root hex>:<salt hex>:<data blocks> allternit.verity.b=...
//! GPT order is esp, system-a, system-b, state, so slot a is /dev/vda2 and slot b /dev/vda3.
//! Each slot partition holds the ext4 data image followed directly by its hash tree
//! (`veritysetup format --no-superblock`), so the tree starts at block <data blocks>.
//!
//! Step 30b: each slot has its own root hash, salt and block count (`allternit.verity.a=` and
//! `allternit.verity.b=`). `allternit.slot=` is now only the default when the ESP holds no
//! boot state; `slot.rs` picks the slot to boot.

/// Data and hash block size (bytes) used by mkimage.sh.
pub const BLOCK: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn parse(s: &str) -> Option<Slot> {
        match s {
            "a" => Some(Slot::A),
            "b" => Some(Slot::B),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }
    /// Block device of the slot partition.
    pub fn device(self) -> &'static str {
        match self {
            Slot::A => "/dev/vda2",
            Slot::B => "/dev/vda3",
        }
    }
    /// The other slot of the pair.
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    /// Device-mapper name; the node is made at /dev/mapper/<name>.
    pub fn dm_name(self) -> &'static str {
        match self {
            Slot::A => "system-a",
            Slot::B => "system-b",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verity {
    pub slot: Slot,
    pub root_hash: String,
    /// Hex salt, or "-" for none.
    pub salt: String,
    pub data_blocks: u64,
}

/// A slot was named but cannot be opened; `slot` is the name as given on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotError {
    pub slot: String,
    pub reason: String,
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.len() % 2 == 0 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Last `key=value` word on the command line.
fn param<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline
        .split_whitespace()
        .filter_map(|w| w.strip_prefix(key)?.strip_prefix('='))
        .last()
}

/// `None`: no `allternit.slot` (direct boot without a disk): run from the initramfs.
/// `Some(Err)`: a slot is named but its verity parameters are missing or malformed, so that
/// slot must not boot.
pub fn from_cmdline(cmdline: &str) -> Option<Result<Verity, SlotError>> {
    let given = param(cmdline, "allternit.slot")?;
    Some(parse_slot(cmdline, given).map_err(|reason| SlotError { slot: given.to_string(), reason }))
}

/// The slot named by `allternit.slot=` (the default slot), or `None` on a direct boot.
pub fn slot_named(cmdline: &str) -> Option<&str> {
    param(cmdline, "allternit.slot")
}

fn parse_slot(cmdline: &str, given: &str) -> Result<Verity, String> {
    let slot = Slot::parse(given).ok_or("unknown slot name")?;
    for_slot(cmdline, slot)
}

/// Verity parameters of one slot, from `allternit.verity.<slot>=` (step 30b: per slot).
pub fn for_slot(cmdline: &str, slot: Slot) -> Result<Verity, String> {
    let key = format!("allternit.verity.{}", slot.name());
    let val = param(cmdline, &key).ok_or_else(|| format!("no {key} on the command line"))?;
    let parts: Vec<&str> = val.split(':').collect();
    let &[root, salt, blocks] = parts.as_slice() else {
        return Err(format!("{key} is not <root>:<salt>:<blocks>"));
    };
    // sha256 root digest: 32 bytes.
    if root.len() != 64 || !is_hex(root) {
        return Err("root hash is not 64 hex digits".into());
    }
    if salt != "-" && !is_hex(salt) {
        return Err("salt is not hex".into());
    }
    let data_blocks = match blocks.parse::<u64>() {
        Ok(n) if n > 0 => n,
        _ => return Err("data block count is not a positive number".into()),
    };
    Ok(Verity {
        slot,
        root_hash: root.to_ascii_lowercase(),
        salt: salt.to_ascii_lowercase(),
        data_blocks,
    })
}

impl Verity {
    /// Data size in bytes (the ext4 image, without the hash tree).
    pub fn data_bytes(&self) -> u64 {
        self.data_blocks * BLOCK
    }
    /// (length in 512-byte sectors, `verity` target parameters). Format version 1, data and
    /// hash on the same partition, hash tree right after the data.
    pub fn table(&self) -> (u64, String) {
        let dev = self.slot.device();
        let n = self.data_blocks;
        (
            self.data_bytes() / 512,
            format!(
                "1 {dev} {dev} {BLOCK} {BLOCK} {n} {n} sha256 {} {}",
                self.root_hash, self.salt
            ),
        )
    }
}

// --- device-mapper ioctl (include/uapi/linux/dm-ioctl.h) -------------------------------

/// sizeof(struct dm_ioctl).
pub const DM_HEADER: usize = 312;
/// sizeof(struct dm_target_spec).
const DM_SPEC: usize = 40;
/// Buffer handed to every call (data_size); the kernel writes replies into it.
pub const DM_BUF: usize = 16 * 1024;
pub const DM_READONLY_FLAG: u32 = 1 << 0;
pub const DM_DEV_CREATE: u32 = 3;
pub const DM_DEV_REMOVE: u32 = 4;
pub const DM_DEV_SUSPEND: u32 = 6;
pub const DM_TABLE_LOAD: u32 = 9;

/// _IOWR(0xfd, nr, struct dm_ioctl).
pub fn dm_ioctl_request(nr: u32) -> u64 {
    (3u64 << 30) | ((DM_HEADER as u64) << 16) | (0xfd << 8) | nr as u64
}

fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_ne_bytes());
}

/// One `struct dm_ioctl` (interface 4.0.0) plus, for a table load, one target spec and its
/// NUL-terminated parameter string. `target` = (length in sectors, type, params).
pub fn dm_buffer(name: &str, flags: u32, target: Option<(u64, &str, &str)>) -> Result<Vec<u8>, String> {
    if name.is_empty() || name.len() > 127 {
        return Err("device-mapper name must be 1-127 bytes".into());
    }
    let mut b = vec![0u8; DM_BUF];
    put_u32(&mut b, 0, 4); // version 4.0.0
    put_u32(&mut b, 12, DM_BUF as u32); // data_size
    put_u32(&mut b, 16, DM_HEADER as u32); // data_start
    put_u32(&mut b, 28, flags);
    b[48..48 + name.len()].copy_from_slice(name.as_bytes());
    if let Some((sectors, ttype, params)) = target {
        if ttype.len() > 15 {
            return Err("target type too long".into());
        }
        let spec = DM_HEADER;
        let p = spec + DM_SPEC;
        // next: offset to the following spec, 8-byte aligned past the NUL.
        let next = (DM_SPEC + params.len() + 1 + 7) & !7;
        if spec + next > DM_BUF {
            return Err("target parameters too long".into());
        }
        put_u32(&mut b, 20, 1); // target_count
        b[spec..spec + 8].copy_from_slice(&0u64.to_ne_bytes()); // sector_start
        b[spec + 8..spec + 16].copy_from_slice(&sectors.to_ne_bytes()); // length
        put_u32(&mut b, spec + 20, next as u32);
        b[spec + 24..spec + 24 + ttype.len()].copy_from_slice(ttype.as_bytes());
        b[p..p + params.len()].copy_from_slice(params.as_bytes());
    }
    Ok(b)
}

/// (major, minor) from the `dev` field of a reply (huge_encode_dev).
pub fn dm_dev(reply: &[u8]) -> (u32, u32) {
    let mut d = [0u8; 8];
    d.copy_from_slice(&reply[40..48]);
    let d = u64::from_ne_bytes(d);
    let major = ((d >> 8) & 0xfff) as u32;
    let minor = ((d & 0xff) | ((d >> 12) & 0xfff00)) as u32;
    (major, minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "4f8a2b1c9d0e3f56a7b8c9d0e1f2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4";

    #[test]
    fn no_slot_means_direct_boot() {
        assert_eq!(from_cmdline("console=ttyS0 rdinit=/sbin/init"), None);
    }

    #[test]
    fn parses_the_active_slot() {
        let line = format!(
            "console=ttyS0 allternit.slot=b allternit.verity.a={ROOT}:00ff:10 allternit.verity.b={}:-:4608\n",
            ROOT.to_uppercase()
        );
        let v = from_cmdline(&line).unwrap().unwrap();
        assert_eq!(v.slot, Slot::B);
        assert_eq!(v.root_hash, ROOT);
        assert_eq!(v.salt, "-");
        assert_eq!(v.data_blocks, 4608);
        let (sectors, params) = v.table();
        assert_eq!(sectors, 4608 * 8);
        assert_eq!(params, format!("1 /dev/vda3 /dev/vda3 4096 4096 4608 4608 sha256 {ROOT} -"));
    }

    #[test]
    fn a_named_slot_with_bad_params_fails() {
        let bad = |line: &str| from_cmdline(line).unwrap().unwrap_err();
        assert_eq!(bad("allternit.slot=c").slot, "c");
        assert_eq!(bad("allternit.slot=a allternit.verity.b=x").slot, "a"); // missing
        assert_eq!(bad(&format!("allternit.slot=a allternit.verity.a={ROOT}:00")).slot, "a");
        assert!(from_cmdline("allternit.slot=a allternit.verity.a=abc:-:1").unwrap().is_err());
        assert!(from_cmdline(&format!("allternit.slot=a allternit.verity.a={ROOT}:0g:1")).unwrap().is_err());
        assert!(from_cmdline(&format!("allternit.slot=a allternit.verity.a={ROOT}:-:0")).unwrap().is_err());
    }

    #[test]
    fn each_slot_has_its_own_params() {
        let other = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let line = format!("allternit.slot=a allternit.verity.a={ROOT}:aa:10 allternit.verity.b={other}:bb:12");
        assert_eq!(slot_named(&line), Some("a"));
        let a = for_slot(&line, Slot::A).unwrap();
        let b = for_slot(&line, Slot::B).unwrap();
        assert_eq!((a.root_hash.as_str(), a.salt.as_str(), a.data_blocks), (ROOT, "aa", 10));
        assert_eq!((b.root_hash.as_str(), b.salt.as_str(), b.data_blocks), (other, "bb", 12));
        assert_eq!(Slot::A.other(), Slot::B);
        assert_eq!(Slot::B.other(), Slot::A);
        assert!(for_slot("allternit.slot=a", Slot::B).is_err());
        assert_eq!(slot_named("console=ttyS0"), None);
    }

    #[test]
    fn dm_buffer_layout() {
        let params = "1 /dev/vda2 /dev/vda2 4096 4096 8 8 sha256 aa -";
        let b = dm_buffer("system-a", DM_READONLY_FLAG, Some((64, "verity", params))).unwrap();
        let u32_at = |o: usize| u32::from_ne_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), u32_at(8)), (4, 0, 0));
        assert_eq!(u32_at(12) as usize, DM_BUF);
        assert_eq!(u32_at(16) as usize, DM_HEADER);
        assert_eq!(u32_at(20), 1);
        assert_eq!(u32_at(28), DM_READONLY_FLAG);
        assert_eq!(&b[48..57], b"system-a\0");
        let spec = DM_HEADER;
        assert_eq!(u64::from_ne_bytes(b[spec + 8..spec + 16].try_into().unwrap()), 64);
        assert_eq!(u32_at(spec + 20) % 8, 0);
        assert_eq!(&b[spec + 24..spec + 31], b"verity\0");
        let p = spec + 40;
        assert_eq!(&b[p..p + params.len()], params.as_bytes());
        assert_eq!(b[p + params.len()], 0);
        assert!(dm_buffer("", 0, None).is_err());
    }

    #[test]
    fn ioctl_numbers_and_dev_decode() {
        assert_eq!(dm_ioctl_request(DM_DEV_CREATE), 0xC138_FD03);
        assert_eq!(dm_ioctl_request(DM_TABLE_LOAD), 0xC138_FD09);
        let mut reply = vec![0u8; DM_HEADER];
        // major 252, minor 0x123 -> (0x23) | (252 << 8) | (0x100 << 12)
        let enc: u64 = 0x23 | (252 << 8) | (0x100 << 12);
        reply[40..48].copy_from_slice(&enc.to_ne_bytes());
        assert_eq!(dm_dev(&reply), (252, 0x123));
    }
}
