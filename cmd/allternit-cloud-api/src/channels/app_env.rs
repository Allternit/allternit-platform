//! Env lookups for Allternit's own (shared) channel apps.
//!
//! Each credential has a namespaced name (`ALLTERNIT_<CHANNEL>_*`) that is
//! read first, and the older bare name as a fallback so existing deployments
//! keep working. The bare names (`APP_ID`, `TENANT_ID`, ...) collide easily in
//! a shared `.env`, which is why new setups should use the namespaced ones.

/// First non-blank value among `names`, read through `get`.
pub(crate) fn first_with(get: &dyn Fn(&str) -> Option<String>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| get(name))
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// First non-blank process env value among `names` (namespaced name first).
pub(crate) fn first(names: &[&str]) -> Option<String> {
    first_with(&|name| std::env::var(name).ok(), names)
}

pub(crate) const TEAMS_APP_ID: &[&str] = &["ALLTERNIT_TEAMS_APP_ID", "APP_ID"];
pub(crate) const TEAMS_APP_PASSWORD: &[&str] = &["ALLTERNIT_TEAMS_APP_PASSWORD", "APP_PASSWORD"];
pub(crate) const TEAMS_TENANT_ID: &[&str] = &["ALLTERNIT_TEAMS_TENANT_ID", "TENANT_ID"];

pub(crate) const SLACK_CLIENT_ID: &[&str] = &["ALLTERNIT_SLACK_CLIENT_ID", "SLACK_CLIENT_ID"];
pub(crate) const SLACK_CLIENT_SECRET: &[&str] = &["ALLTERNIT_SLACK_CLIENT_SECRET", "SLACK_CLIENT_SECRET"];
pub(crate) const SLACK_SIGNING_SECRET: &[&str] = &["ALLTERNIT_SLACK_SIGNING_SECRET", "SLACK_SIGNING_SECRET"];

pub(crate) const META_APP_ID: &[&str] = &["ALLTERNIT_META_APP_ID", "META_APP_ID"];
pub(crate) const META_APP_SECRET: &[&str] = &["ALLTERNIT_META_APP_SECRET", "META_APP_SECRET"];
pub(crate) const META_ES_CONFIG_ID: &[&str] = &["ALLTERNIT_META_ES_CONFIG_ID", "META_ES_CONFIG_ID"];
pub(crate) const META_SYSTEM_TOKEN: &[&str] = &["ALLTERNIT_META_SYSTEM_TOKEN", "META_SYSTEM_TOKEN"];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn namespaced_name_wins_over_legacy() {
        let get = env(&[("ALLTERNIT_TEAMS_APP_ID", "new"), ("APP_ID", "old")]);
        assert_eq!(first_with(&get, TEAMS_APP_ID).as_deref(), Some("new"));
    }

    #[test]
    fn falls_back_to_legacy_name() {
        let get = env(&[("SLACK_SIGNING_SECRET", "old")]);
        assert_eq!(first_with(&get, SLACK_SIGNING_SECRET).as_deref(), Some("old"));
        let get = env(&[("META_APP_ID", "old")]);
        assert_eq!(first_with(&get, META_APP_ID).as_deref(), Some("old"));
    }

    #[test]
    fn blank_namespaced_value_falls_through() {
        let get = env(&[("ALLTERNIT_SLACK_CLIENT_ID", "  "), ("SLACK_CLIENT_ID", "old")]);
        assert_eq!(first_with(&get, SLACK_CLIENT_ID).as_deref(), Some("old"));
    }

    #[test]
    fn unset_is_none() {
        assert_eq!(first_with(&env(&[]), META_ES_CONFIG_ID), None);
    }
}
