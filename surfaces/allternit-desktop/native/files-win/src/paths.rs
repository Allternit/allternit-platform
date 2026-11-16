//! Where sync roots live and how computer names become folder names.

use std::path::{Path, PathBuf};

/// Folder under the user profile that holds one sync root per computer. Not
/// `~/Allternit`: that is this PC's own runtime home root (SPEC section 3).
pub const PARENT_DIR: &str = "Allternit Files";

/// Sync-root ID prefix (StorageProviderSyncRootManager wants `provider!SID!account`).
pub const PROVIDER_ID_PREFIX: &str = "AllternitFiles";

/// Windows reserved device names (case-insensitive, with or without extension).
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A computer or item name made safe as one Windows path component.
pub fn safe_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let s = s.trim_start().to_string();
    if s.is_empty() {
        return "Computer".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return format!("{s}_");
    }
    s
}

/// `%USERPROFILE%\Allternit Files\<computer name>`.
pub fn default_sync_root(profile: &Path, computer_name: &str) -> PathBuf {
    profile.join(PARENT_DIR).join(safe_component(computer_name))
}

/// Sync-root ID for one computer.
pub fn sync_root_id(user_sid: &str, computer_id: &str) -> String {
    let account: String = computer_id.chars().map(|c| if c == '!' { '_' } else { c }).collect();
    format!("{PROVIDER_ID_PREFIX}!{user_sid}!{account}")
}

/// A stable GUID string per computer for the navigation-pane entry (`{...}` form).
pub fn nav_pane_clsid(computer_id: &str) -> String {
    let h = blake3::hash(format!("allternit-files-nav/{computer_id}").as_bytes());
    let b = h.as_bytes();
    let mut g = [0u8; 16];
    g.copy_from_slice(&b[..16]);
    g[6] = (g[6] & 0x0f) | 0x50; // version 5 style (name based)
    g[8] = (g[8] & 0x3f) | 0x80; // RFC 4122 variant
    format!(
        "{{{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7], g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

/// Relative path (with `\`) of `full` under `root`, or None when outside.
pub fn relative(root: &Path, full: &Path) -> Option<String> {
    let r = full.strip_prefix(root).ok()?;
    let parts: Vec<String> = r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(parts.join("\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Computer names come from users ("Joe's PC: work"); a bad one would make
    // CfRegisterSyncRoot fail or hit a device name like CON.
    #[test]
    fn names_become_legal_windows_components() {
        assert_eq!(safe_component("Mac Studio"), "Mac Studio");
        assert_eq!(safe_component("work: \"main\"/pc"), "work_ _main__pc");
        assert_eq!(safe_component("dots..."), "dots");
        assert_eq!(safe_component("con"), "con_");
        assert_eq!(safe_component("COM1.txt"), "COM1.txt_");
        assert_eq!(safe_component("  "), "Computer");
        assert_eq!(sync_root_id("S-1-5-21-1", "cmp!x"), "AllternitFiles!S-1-5-21-1!cmp_x");
        let a = nav_pane_clsid("cmp_a");
        assert_eq!(a.len(), 38);
        assert_eq!(a, nav_pane_clsid("cmp_a"));
        assert_ne!(a, nav_pane_clsid("cmp_b"));
    }
}
