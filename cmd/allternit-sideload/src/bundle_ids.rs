//! Bundle ids for a team.
//!
//! App IDs are global across Apple, so the published build's ids
//! (`com.allternit.phonelink.ictrl`, `…ictrl.uitests.xctrunner`) can't be
//! registered by every user. Each team gets one stable prefix derived from its
//! team id, the same rule as the iCTRL fork's Scripts/setup-signing.sh:
//!
//!   com.allternit.phonelink  →  com.allternit.phonelink.<teamid lowercased>
//!
//! Re-signs reuse the same ids, so they reuse the same App IDs and never spend
//! a free account's 10-new-App-IDs-per-week allowance again.

use crate::errors::{ErrorCode, Failure};

pub const DEFAULT_SOURCE_PREFIX: &str = "com.allternit.phonelink";

/// Apple team ids: 10 upper-case letters/digits.
pub fn is_valid_team_id(team_id: &str) -> bool {
    team_id.len() == 10 && team_id.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Reverse-DNS: dot-separated non-empty labels of [A-Za-z0-9-].
pub fn is_valid_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 155
        && id.split('.').all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// The prefix every bundle id of this team uses.
pub fn team_prefix(source_prefix: &str, team_id: &str) -> Result<String, Failure> {
    if !is_valid_team_id(team_id) {
        return Err(Failure::new(ErrorCode::SigningFailed, format!("Unexpected Apple team id {team_id:?}.")));
    }
    if !is_valid_bundle_id(source_prefix) {
        return Err(Failure::new(ErrorCode::InvalidRequest, "Invalid bundle prefix."));
    }
    Ok(format!("{source_prefix}.{}", team_id.to_ascii_lowercase()))
}

/// Move `id` from `source_prefix` to `target_prefix`. Ids already under the
/// target are returned unchanged (idempotent); ids outside the source are None.
pub fn rebase_bundle_id(id: &str, source_prefix: &str, target_prefix: &str) -> Option<String> {
    let under = |prefix: &str| id == prefix || id.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'));
    if under(target_prefix) {
        return Some(id.to_string());
    }
    if under(source_prefix) {
        return Some(format!("{target_prefix}{}", &id[source_prefix.len()..]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_one_prefix_per_team() {
        assert_eq!(team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap(), "com.allternit.phonelink.abcde12345");
        assert!(team_prefix(DEFAULT_SOURCE_PREFIX, "abc").is_err());
        assert!(team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE1234$").is_err());
        assert!(team_prefix("com..x", "ABCDE12345").is_err());
    }

    #[test]
    fn rebases_the_published_ids() {
        let t = team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap();
        assert_eq!(rebase_bundle_id("com.allternit.phonelink.ictrl", DEFAULT_SOURCE_PREFIX, &t).unwrap(), "com.allternit.phonelink.abcde12345.ictrl");
        assert_eq!(
            rebase_bundle_id("com.allternit.phonelink.ictrl.uitests.xctrunner", DEFAULT_SOURCE_PREFIX, &t).unwrap(),
            "com.allternit.phonelink.abcde12345.ictrl.uitests.xctrunner"
        );
    }

    #[test]
    fn rebase_is_idempotent_and_strict() {
        let t = "com.allternit.phonelink.abcde12345";
        let once = rebase_bundle_id("com.allternit.phonelink.ictrl", DEFAULT_SOURCE_PREFIX, t).unwrap();
        assert_eq!(rebase_bundle_id(&once, DEFAULT_SOURCE_PREFIX, t).unwrap(), once);
        // A sibling that only shares characters, not a label boundary, is not ours.
        assert_eq!(rebase_bundle_id("com.allternit.phonelinkx.ictrl", DEFAULT_SOURCE_PREFIX, t), None);
        assert_eq!(rebase_bundle_id("com.other.app", DEFAULT_SOURCE_PREFIX, t), None);
    }

    #[test]
    fn same_team_same_ids() {
        // Stable across runs: nothing random or time-based goes into the ids.
        assert_eq!(team_prefix(DEFAULT_SOURCE_PREFIX, "ZZZZZ99999").unwrap(), team_prefix(DEFAULT_SOURCE_PREFIX, "ZZZZZ99999").unwrap());
    }

    #[test]
    fn validates_bundle_ids() {
        assert!(is_valid_bundle_id("com.allternit.phonelink"));
        assert!(is_valid_bundle_id("a-b.c1"));
        assert!(!is_valid_bundle_id("com..x"));
        assert!(!is_valid_bundle_id(".com"));
        assert!(!is_valid_bundle_id("com.x_y"));
        assert!(!is_valid_bundle_id(""));
    }
}
