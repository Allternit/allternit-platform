//! UEFI Secure Boot state (plan step 31). Pure logic: `linux.rs` mounts efivarfs and reads
//! the variable.

/// efivarfs file of the EFI `SecureBoot` variable (global variable GUID).
pub const VAR_PATH: &str = "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";
pub const EFIVARFS_MOUNT: &str = "/sys/firmware/efi/efivars";

/// An efivarfs file is 4 attribute bytes followed by the variable data; `SecureBoot` is one
/// byte, 1 = enforcing. `None` when the file is not that shape.
pub fn parse(file: &[u8]) -> Option<bool> {
    if file.len() == 5 {
        Some(file[4] == 1)
    } else {
        None
    }
}

/// The serial line CI looks for.
pub fn line(state: Option<bool>) -> &'static str {
    match state {
        Some(true) => "secureboot: enabled",
        Some(false) => "secureboot: disabled",
        None => "secureboot: unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_variable() {
        assert_eq!(parse(&[6, 0, 0, 0, 1]), Some(true));
        assert_eq!(parse(&[6, 0, 0, 0, 0]), Some(false));
        assert_eq!(parse(&[1]), None);
        assert_eq!(parse(&[]), None);
        assert_eq!(line(Some(true)), "secureboot: enabled");
        assert_eq!(line(None), "secureboot: unknown");
    }
}
