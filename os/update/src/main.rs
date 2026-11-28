//! allternit-update (plan step 30b): write a new system slot image into the INACTIVE slot,
//! then make that slot active, unconfirmed, with N tries. PID 1 boots it on the next boot,
//! spends one try per boot, and switches back to the old slot if it fails dm-verity or runs
//! out of tries before the health check confirms it.
//!
//!   allternit-update --image FILE --manifest FILE --sig FILE [--pubkey FILE]
//!                    [--slot a|b] [--tries N] [--uki FILE]
//!                    [--state FILE] [--device PATH [--offset BYTES --slot-bytes N]]
//!
//! Step 31: nothing is written unless the signed manifest (Ed25519, public key in the slot
//! image at /etc/allternit/update-pub.key) verifies, is for the target slot, and describes
//! this image (sha256 and size) and, with --uki, this UKI. Unsigned or mismatched updates are
//! refused with the reason, before the slot is touched. On success the manifest and its
//! signature are installed next to the boot state (the ESP's `allternit/` directory), which
//! is where PID 1 reads the slot's dm-verity parameters from.
//!
//! On the device the defaults are right: the boot state is `/run/esp/allternit/bootstate`
//! and the slot device is /dev/vda2 or /dev/vda3. `--device/--offset/--slot-bytes` let CI
//! (qemu-boot.sh --bad-update) write a slot inside a raw disk image with the same code.
//!
//! The new slot's dm-verity root hash is in its signed manifest, not in the UKI, so a new
//! slot image needs no new UKI. `--uki` is only for a kernel update; it must match the
//! manifest's uki_sha256.

use allternit_init::manifest::{self, Manifest};
use allternit_init::slot::{self, BootState, DEFAULT_TRIES};
use allternit_init::verity::Slot;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use sha2::{Digest, Sha256};

struct Args {
    image: String,
    manifest: String,
    sig: String,
    pubkey: String,
    slot: Option<Slot>,
    tries: u32,
    uki: Option<String>,
    state: String,
    device: Option<String>,
    offset: u64,
    slot_bytes: Option<u64>,
}

fn usage() -> ! {
    eprintln!(
        "usage: allternit-update --image FILE --manifest FILE --sig FILE [--pubkey FILE] [--slot a|b]\n\
         \x20                       [--tries N] [--uki FILE] [--state FILE]\n\
         \x20                       [--device PATH [--offset BYTES --slot-bytes N]]"
    );
    std::process::exit(2)
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        image: String::new(),
        manifest: String::new(),
        sig: String::new(),
        pubkey: manifest::PUBKEY_PATH.to_string(),
        slot: None,
        tries: DEFAULT_TRIES,
        uki: None,
        state: format!("{}/{}", slot::ESP_MOUNT, slot::STATE_FILE),
        device: None,
        offset: 0,
        slot_bytes: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            usage();
        }
        let val = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let num = |v: &str| v.parse::<u64>().map_err(|_| format!("{flag}: not a number: {v}"));
        match flag.as_str() {
            "--image" => a.image = val,
            "--manifest" => a.manifest = val,
            "--sig" => a.sig = val,
            "--pubkey" => a.pubkey = val,
            "--slot" => a.slot = Some(Slot::parse(&val).ok_or("--slot takes a or b")?),
            "--tries" => {
                a.tries = num(&val)? as u32;
                if a.tries == 0 {
                    return Err("--tries must be at least 1".into());
                }
            }
            "--uki" => a.uki = Some(val),
            "--state" => a.state = val,
            "--device" => a.device = Some(val),
            "--offset" => a.offset = num(&val)?,
            "--slot-bytes" => a.slot_bytes = Some(num(&val)?),
            _ => return Err(format!("unknown flag {flag}")),
        }
    }
    if a.image.is_empty() {
        return Err("--image is required".into());
    }
    if a.manifest.is_empty() || a.sig.is_empty() {
        return Err("refusing an unsigned update: --manifest and --sig are required".into());
    }
    if a.offset > 0 && a.slot_bytes.is_none() {
        return Err("--offset needs --slot-bytes (the slot size), so the write cannot run past the slot".into());
    }
    Ok(a)
}

/// Replace `path` with `data`: temp file, fsync, rename.
fn replace_file(path: &str, data: &[u8]) -> Result<(), String> {
    let tmp = format!("{path}.new");
    let mut f = File::create(&tmp).map_err(|e| format!("create {tmp}: {e}"))?;
    f.write_all(data).and_then(|_| f.sync_all()).map_err(|e| format!("write {tmp}: {e}"))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {tmp} -> {path}: {e}"))?;
    if let Some(dir) = Path::new(path).parent() {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// SHA-256 (lowercase hex) and length of a file.
fn sha256_file(path: &str) -> Result<(String, u64), String> {
    let mut f = File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("read {path}: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((h.finalize().iter().map(|b| format!("{b:02x}")).collect(), total))
}

/// Step 31: the update must be signed, for this slot, and describe these bytes. Returns the
/// manifest and its raw bytes (installed on the ESP afterwards).
fn check_release(a: &Args, target: Slot) -> Result<(Manifest, Vec<u8>, Vec<u8>), String> {
    let pubkey = std::fs::read(&a.pubkey).map_err(|e| format!("read public key {}: {e}", a.pubkey))?;
    let json = std::fs::read(&a.manifest).map_err(|e| format!("read manifest {}: {e}", a.manifest))?;
    let sig = std::fs::read(&a.sig).map_err(|e| format!("read signature {}: {e}", a.sig))?;
    let m = manifest::verified(&json, &sig, &pubkey).map_err(|e| format!("refusing the update: {e}"))?;
    if m.slot != target.name() {
        return Err(format!("refusing the update: the manifest is for slot {}, the target is slot {}", m.slot, target.name()));
    }
    let (sum, bytes) = sha256_file(&a.image)?;
    m.check_image(&sum, bytes).map_err(|e| format!("refusing the update: {e}"))?;
    if let Some(uki) = &a.uki {
        let (usum, _) = sha256_file(uki)?;
        if usum != m.uki_sha256 {
            return Err(format!("refusing the update: UKI sha256 {usum} does not match the manifest ({})", m.uki_sha256));
        }
    }
    Ok((m, json, sig))
}

fn write_slot(image: &str, device: &str, offset: u64, room: Option<u64>) -> Result<u64, String> {
    let mut src = File::open(image).map_err(|e| format!("open {image}: {e}"))?;
    let len = src.metadata().map_err(|e| format!("{image}: {e}"))?.len();
    if len == 0 {
        return Err(format!("{image} is empty"));
    }
    let mut dst = OpenOptions::new().write(true).open(device).map_err(|e| format!("open {device}: {e}"))?;
    // A block device reports its size through seek(End).
    let room = match room {
        Some(r) => r,
        None => dst.seek(SeekFrom::End(0)).map_err(|e| format!("size of {device}: {e}"))?,
    };
    if len > room {
        return Err(format!("{image} is {len} bytes, the slot holds {room}"));
    }
    dst.seek(SeekFrom::Start(offset)).map_err(|e| format!("seek {device}: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let n = src.read(&mut buf).map_err(|e| format!("read {image}: {e}"))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).map_err(|e| format!("write {device} at {}: {e}", offset + done))?;
        done += n as u64;
    }
    dst.sync_all().map_err(|e| format!("sync {device}: {e}"))?;
    Ok(done)
}

fn run() -> Result<(), String> {
    let a = parse_args()?;
    let text = std::fs::read_to_string(&a.state).map_err(|e| format!("read boot state {}: {e}", a.state))?;
    let st = BootState::parse(&text).map_err(|e| format!("{}: {e}", a.state))?;
    let target = slot::update_target(st, a.slot)?;
    let (man, man_json, man_sig) = check_release(&a, target)?;
    println!("update: manifest ok slot={} version={}", man.slot, man.version);
    let device = a.device.clone().unwrap_or_else(|| target.device().to_string());
    let bytes = write_slot(&a.image, &device, a.offset, a.slot_bytes)?;
    println!("update: wrote slot={} bytes={bytes} to {device}", target.name());
    if let Some(uki) = &a.uki {
        let data = std::fs::read(uki).map_err(|e| format!("read {uki}: {e}"))?;
        let esp = Path::new(&a.state).parent().and_then(Path::parent).unwrap_or(Path::new(slot::ESP_MOUNT));
        let dest = esp.join("EFI/BOOT/BOOTX64.EFI");
        replace_file(&dest.to_string_lossy(), &data)?;
        println!("update: replaced {}", dest.display());
    }
    // The manifest goes in last, right before the boot state: the old manifest vouches for
    // the old image until then. A cut in between leaves a slot PID 1 refuses (verity or
    // manifest FAIL) and falls back from.
    let mdir = Path::new(&a.state).parent().unwrap_or(Path::new("."));
    replace_file(&mdir.join(format!("manifest-{}.json", man.slot)).to_string_lossy(), &man_json)?;
    replace_file(&mdir.join(format!("manifest-{}.sig", man.slot)).to_string_lossy(), &man_sig)?;
    println!("update: installed manifest-{}.json + .sig in {}", man.slot, mdir.display());
    let next = slot::pending(target, a.tries);
    replace_file(&a.state, next.render().as_bytes())?;
    println!(
        "update: active={} confirmed=0 tries={} (reboot to try it; falls back to slot {} if it fails)",
        target.name(),
        a.tries,
        st.active.name()
    );
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("allternit-update: {e}");
        std::process::exit(1);
    }
}
