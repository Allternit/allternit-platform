//! Item names: validation, keep-both conflict copies and duplicate names (SPEC section 4).

use chrono::NaiveDateTime;
use unicode_normalization::UnicodeNormalization;

use crate::error::{FilesError, Result};

/// Longest name in UTF-8 bytes (`Item.name`).
pub const MAX_NAME_BYTES: usize = 255;

/// Prefix of the engine's own temp files. The scanner and watcher skip them.
pub const TEMP_PREFIX: &str = ".allternit-";

/// Checks a display name and returns it in NFC. Refuses empty, `.`, `..`, `/`, NUL,
/// more than 255 bytes, and the engine's temp prefix.
pub fn validate_name(name: &str) -> Result<String> {
    let nfc: String = name.nfc().collect();
    if nfc.is_empty() || nfc == "." || nfc == ".." {
        return Err(FilesError::InvalidName(format!("{name:?} is not a file name")));
    }
    if nfc.contains('/') || nfc.contains('\0') {
        return Err(FilesError::InvalidName(format!("{name:?} contains / or NUL")));
    }
    if nfc.len() > MAX_NAME_BYTES {
        return Err(FilesError::InvalidName(format!("{name:?} is longer than 255 bytes")));
    }
    if nfc.starts_with(TEMP_PREFIX) {
        return Err(FilesError::InvalidName(format!("{name:?} uses a reserved prefix")));
    }
    Ok(nfc)
}

/// Splits `Lease.pdf` into (`Lease`, `.pdf`). A leading dot (`.env`) or a trailing dot is
/// part of the stem, so dotfiles and `name.` have no extension.
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Builds `<stem> (<label>)<ext>`, shortening the stem (at a char boundary) so the
/// result stays within 255 bytes.
fn with_label(stem: &str, label: &str, ext: &str) -> String {
    let fixed = label.len() + ext.len() + 3; // " (" + ")"
    let mut stem = stem;
    if stem.len() + fixed > MAX_NAME_BYTES {
        let mut cut = MAX_NAME_BYTES.saturating_sub(fixed);
        while cut > 0 && !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        stem = &stem[..cut];
    }
    format!("{stem} ({label}){ext}")
}

fn clean_device_name(device: &str) -> String {
    let cleaned: String = device
        .nfc()
        .map(|c| if c == '/' || c == '\0' || c == '(' || c == ')' { '-' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "another device".to_string()
    } else {
        cleaned.chars().take(64).collect()
    }
}

/// Keep-both conflict copy name, exactly as SPEC section 4:
/// `<stem> (conflict <device name> <YYYY-MM-DD HHmm>).<ext>`; when that is taken,
/// ` 2`, ` 3`… goes before the `)`.
///
/// `when` is the wall-clock time to show (the drive's configured offset).
pub fn conflict_copy_name(
    name: &str,
    device_name: &str,
    when: NaiveDateTime,
    taken: impl Fn(&str) -> bool,
) -> String {
    let (stem, ext) = split_name(name);
    let base = format!("conflict {} {}", clean_device_name(device_name), when.format("%Y-%m-%d %H%M"));
    let first = with_label(stem, &base, ext);
    if !taken(&first) {
        return first;
    }
    let mut n = 2u32;
    loop {
        let candidate = with_label(stem, &format!("{base} {n}"), ext);
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// A create whose name exists (no `base_rev`), `keep_both`: `<stem> (2).<ext>`, then `(3)`…
pub fn duplicate_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = split_name(name);
    let mut n = 2u32;
    loop {
        let candidate = with_label(stem, &n.to_string(), ext);
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}
