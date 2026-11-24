//! Operator tool for the Meet bot account profiles (docs/meetings/BOT-ACCOUNTS.md).
//! Runs on the cloud-api host, as the user that owns the profile store, with
//! the same env as cloud-api: `MEETINGS_BOT_PROFILE_KEY` (needed by `seal` and
//! `open`) and optionally `MEETINGS_BOT_PROFILE_DIR`.
//!
//! ```text
//! meetings-bot-profile seal <account> <archive.tar.zst>   seal a signed-in profile as the next generation, mark active
//! meetings-bot-profile status [account]                   status, generation, timestamps
//! meetings-bot-profile mark <account> active|needs_reauth|suspended
//! meetings-bot-profile open <account> <out.tar.zst>       decrypt the current generation (for a re-sign-in)
//! ```

use std::io::Write;
use std::process::ExitCode;

use allternit_cloud_api::services::bot_profiles::{
    validate_account, ProfileStatus, ProfileStore, DIR_ENV, KEY_ENV,
};
use chrono::Utc;

const USAGE: &str = "usage:
  meetings-bot-profile seal <account> <archive.tar.zst>
  meetings-bot-profile status [account]
  meetings-bot-profile mark <account> active|needs_reauth|suspended
  meetings-bot-profile open <account> <out.tar.zst>";

/// zstd frame magic: a cheap check that `seal` was handed a .tar.zst.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("meetings-bot-profile: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let store = ProfileStore::from_env();
    let need_key = |store: &ProfileStore| {
        if store.has_key() {
            Ok(())
        } else {
            Err(format!(
                "{KEY_ENV} is not set (or malformed); use the value from cloud-api's env"
            ))
        }
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["seal", account, archive] => {
            need_key(&store)?;
            validate_account(account).map_err(|e| e.to_string())?;
            let bytes = std::fs::read(archive).map_err(|e| format!("reading {archive}: {e}"))?;
            if bytes.len() < 4 || bytes[..4] != ZSTD_MAGIC {
                return Err(format!("{archive} is not a zstd archive (expected the .tar.zst from bot-profile-signin.sh)"));
            }
            let gen = store
                .seal(account, &bytes, Utc::now(), true)
                .map_err(|e| e.to_string())?;
            println!(
                "sealed {account} generation {gen} ({} bytes) in {}; status active",
                bytes.len(),
                store.dir().display()
            );
            println!("delete the plaintext archive now: shred -u {archive}  (or rm -f)");
            Ok(())
        }
        ["status"] => {
            let accounts = store.accounts().map_err(|e| e.to_string())?;
            if accounts.is_empty() {
                println!("no bot accounts in {} ({DIR_ENV})", store.dir().display());
            }
            for account in accounts {
                print_status(&store, &account)?;
            }
            Ok(())
        }
        ["status", account] => print_status(&store, account),
        ["mark", account, status] => {
            let status = ProfileStatus::parse(status)
                .ok_or_else(|| format!("unknown status {status:?}\n{USAGE}"))?;
            let state = store
                .set_status(account, status, Utc::now())
                .map_err(|e| e.to_string())?;
            if status == ProfileStatus::Active && state.gen == 0 {
                eprintln!(
                    "warning: {account} has no sealed profile; pods skip it until `seal` runs"
                );
            }
            println!("{account}: {}", state.status.as_str());
            Ok(())
        }
        ["open", account, out] => {
            need_key(&store)?;
            let bytes = store.open_current(account).map_err(|e| e.to_string())?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(out)
                .map_err(|e| format!("creating {out} (it must not exist yet): {e}"))?;
            file.write_all(&bytes)
                .map_err(|e| format!("writing {out}: {e}"))?;
            println!("wrote {account}'s current profile to {out} (0600, plaintext Google session: delete it after use)");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

fn print_status(store: &ProfileStore, account: &str) -> Result<(), String> {
    let state = store.state(account).map_err(|e| e.to_string())?;
    let gens = store.generations(account).map_err(|e| e.to_string())?;
    let at = |t: Option<chrono::DateTime<Utc>>| {
        t.map(|t| t.to_rfc3339()).unwrap_or_else(|| "-".to_string())
    };
    match state {
        None => println!("{account}: not signed in yet (no state.json)"),
        Some(state) => println!(
            "{account}: {} gen={} on_disk={:?} deliverable={} last_ok_at={} last_signed_out_at={} last_sealed_at={}",
            state.status.as_str(),
            state.gen,
            gens,
            store.deliverable(account),
            at(state.last_ok_at),
            at(state.last_signed_out_at),
            at(state.last_sealed_at),
        ),
    }
    Ok(())
}
