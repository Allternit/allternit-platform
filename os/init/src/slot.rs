//! A/B slot state (plan step 30b). Pure logic only, so the tests run on any host; `linux.rs`
//! reads and writes the file, `allternit-update` (os/update) writes it after an update.
//!
//! The record is a small text file on the ESP, `/allternit/bootstate` (mounted by PID 1 at
//! `/run/esp`):
//!
//! ```text
//! active=a
//! tries=3
//! confirmed=0
//! ```
//!
//! Rules:
//! - A confirmed slot boots as is.
//! - An unconfirmed slot with tries left boots and loses one try (written before the boot).
//! - An unconfirmed slot with no tries left is not booted: PID 1 switches to the other slot
//!   (`reason=tries-exhausted`). A slot that fails dm-verity also switches (`reason=verity`).
//!   The slot it switches to is the one that ran before the update, so it is written as
//!   confirmed. At most one switch per boot: if that slot fails too, the machine powers off.
//! - The health check passing (the shell answered its first tool call) writes the running
//!   slot as confirmed.
//! - `allternit-update` writes only the inactive slot, and only from a confirmed slot, then
//!   sets it active, unconfirmed, with N tries.

use crate::verity::Slot;

/// Tries a freshly written slot gets (mkimage.sh and allternit-update use the same number).
pub const DEFAULT_TRIES: u32 = 3;
/// Where PID 1 mounts the ESP.
pub const ESP_MOUNT: &str = "/run/esp";
/// The record, relative to the ESP root.
pub const STATE_FILE: &str = "allternit/bootstate";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootState {
    pub active: Slot,
    pub tries: u32,
    pub confirmed: bool,
}

/// A slot switch decided at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fallback {
    pub from: Slot,
    pub to: Slot,
    pub reason: &'static str,
}

/// What to boot first, and the record to write before trying it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub slot: Slot,
    pub state: BootState,
    pub fallback: Option<Fallback>,
}

impl BootState {
    pub fn parse(text: &str) -> Result<BootState, String> {
        let (mut active, mut tries, mut confirmed) = (None, None, None);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| format!("not key=value: {line}"))?;
            match k.trim() {
                "active" => active = Some(Slot::parse(v.trim()).ok_or_else(|| format!("unknown slot {v}"))?),
                "tries" => tries = Some(v.trim().parse::<u32>().map_err(|_| format!("tries is not a number: {v}"))?),
                "confirmed" => {
                    confirmed = Some(match v.trim() {
                        "1" | "true" => true,
                        "0" | "false" => false,
                        _ => return Err(format!("confirmed is not 0 or 1: {v}")),
                    })
                }
                _ => {} // unknown keys are kept out of the decision, not an error
            }
        }
        Ok(BootState {
            active: active.ok_or("no active=")?,
            tries: tries.ok_or("no tries=")?,
            confirmed: confirmed.ok_or("no confirmed=")?,
        })
    }

    pub fn render(&self) -> String {
        format!(
            "# AllternitOS A/B boot state (plan step 30b). Written by PID 1 and allternit-update.\n\
             active={}\ntries={}\nconfirmed={}\n",
            self.active.name(),
            self.tries,
            self.confirmed as u8
        )
    }
}

/// The slot that ran before an update (or just passed the health check): known good.
pub fn known_good(slot: Slot) -> BootState {
    BootState { active: slot, tries: 0, confirmed: true }
}

/// What `allternit-update` writes after it wrote `slot`.
pub fn pending(slot: Slot, tries: u32) -> BootState {
    BootState { active: slot, tries, confirmed: false }
}

/// Decide the first slot to try from the stored record.
pub fn plan(st: BootState) -> Plan {
    if st.confirmed {
        return Plan { slot: st.active, state: st, fallback: None };
    }
    if st.tries == 0 {
        let to = st.active.other();
        return Plan {
            slot: to,
            state: known_good(to),
            fallback: Some(Fallback { from: st.active, to, reason: "tries-exhausted" }),
        };
    }
    Plan { slot: st.active, state: BootState { tries: st.tries - 1, ..st }, fallback: None }
}

/// The slot `failed` did not verify: switch to the other one.
pub fn after_verity_fail(failed: Slot) -> (Fallback, BootState) {
    let to = failed.other();
    (Fallback { from: failed, to, reason: "verity" }, known_good(to))
}

/// Can `allternit-update` write `target` given the stored record? Ok = the slot to write.
pub fn update_target(st: BootState, target: Option<Slot>) -> Result<Slot, String> {
    if !st.confirmed {
        return Err(format!(
            "slot {} is not confirmed yet; refusing to overwrite slot {} (it is the fallback)",
            st.active.name(),
            st.active.other().name()
        ));
    }
    let inactive = st.active.other();
    match target {
        Some(t) if t == st.active => Err(format!("slot {} is the running slot; only the inactive slot is written", t.name())),
        _ => Ok(inactive),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render_round_trip() {
        let st = BootState { active: Slot::B, tries: 2, confirmed: false };
        assert_eq!(BootState::parse(&st.render()).unwrap(), st);
        let st = BootState::parse("# x\n active = a \ntries=0\nconfirmed=1\nextra=7\n").unwrap();
        assert_eq!(st, known_good(Slot::A));
        assert!(BootState::parse("active=c\ntries=1\nconfirmed=0").is_err());
        assert!(BootState::parse("active=a\nconfirmed=0").is_err());
        assert!(BootState::parse("active=a\ntries=x\nconfirmed=0").is_err());
        assert!(BootState::parse("active=a\ntries=1\nconfirmed=2").is_err());
        assert!(BootState::parse("garbage").is_err());
    }

    #[test]
    fn confirmed_slot_boots_unchanged() {
        let st = known_good(Slot::A);
        assert_eq!(plan(st), Plan { slot: Slot::A, state: st, fallback: None });
    }

    #[test]
    fn unconfirmed_slot_spends_a_try() {
        let p = plan(pending(Slot::B, 3));
        assert_eq!(p.slot, Slot::B);
        assert_eq!(p.state, pending(Slot::B, 2));
        assert_eq!(p.fallback, None);
        assert_eq!(plan(pending(Slot::B, 1)).state, pending(Slot::B, 0));
    }

    #[test]
    fn no_tries_left_falls_back() {
        let p = plan(pending(Slot::B, 0));
        assert_eq!(p.slot, Slot::A);
        assert_eq!(p.state, known_good(Slot::A));
        assert_eq!(p.fallback, Some(Fallback { from: Slot::B, to: Slot::A, reason: "tries-exhausted" }));
    }

    #[test]
    fn verity_failure_falls_back() {
        let (f, st) = after_verity_fail(Slot::B);
        assert_eq!(f, Fallback { from: Slot::B, to: Slot::A, reason: "verity" });
        assert_eq!(st, known_good(Slot::A));
    }

    #[test]
    fn updater_writes_only_the_inactive_slot_from_a_confirmed_one() {
        assert_eq!(update_target(known_good(Slot::A), None), Ok(Slot::B));
        assert_eq!(update_target(known_good(Slot::A), Some(Slot::B)), Ok(Slot::B));
        assert!(update_target(known_good(Slot::A), Some(Slot::A)).is_err());
        assert!(update_target(pending(Slot::B, 2), None).is_err());
    }
}
