//! Config gate for the `dev-api-token` development backdoor (audit finding B1).
//!
//! Historically the cloud API honored a hardcoded `dev-api-token` bearer at
//! every token-validation site (legacy auth layer, DB validator, WebSocket
//! validator, token-info route, dispatch handoff). That left a production
//! backdoor: any client presenting the literal gained the development user.
//!
//! The token is now honored ONLY when the environment variable
//! `ALLTERNIT_ALLOW_DEV_TOKEN` is set to `true` or `1`. The default is OFF,
//! so production rejects the token. The literal itself is intentionally kept
//! (not yet removed) because the iOS app in the field still depends on it;
//! removal happens in a later, coordinated deploy.
//!
//! `main.rs` logs a warn-level line at startup whenever the gate is open so
//! it can never be enabled silently in production.

/// Environment variable that gates the dev token. Default OFF.
pub const ALLOW_DEV_TOKEN_ENV: &str = "ALLTERNIT_ALLOW_DEV_TOKEN";

/// Whether the dev-token backdoor is currently enabled. Reads the env var on
/// each call, matching the existing `Allternit_API_DEVELOPMENT_MODE` pattern.
pub fn dev_token_allowed() -> bool {
    std::env::var(ALLOW_DEV_TOKEN_ENV)
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// The hardcoded development token. Gated by [`dev_token_allowed`] at every
/// acceptance site; do not reference it outside this module's gate.
pub(crate) const DEV_API_TOKEN: &str = "dev-api-token";

/// Pure decision function: is `token` the dev token AND is the gate open?
/// Separated from env reading so the accept/reject semantics are unit-testable.
pub(crate) fn is_allowed_dev_token(token: &str, allowed: bool) -> bool {
    allowed && token == DEV_API_TOKEN
}

#[cfg(test)]
pub(crate) static DEV_TOKEN_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn dev_token_rejected_when_gate_disabled() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);

        assert!(
            !dev_token_allowed(),
            "gate must default to OFF when {ALLOW_DEV_TOKEN_ENV} is unset"
        );
        assert!(
            !is_allowed_dev_token(DEV_API_TOKEN, false),
            "the literal dev token must be rejected when the gate is off"
        );
        assert!(
            !is_allowed_dev_token("dev-api-token", false),
            "rejected for any caller when the gate is off"
        );
    }

    #[test]
    #[serial_test::serial]
    fn dev_token_accepted_only_when_gate_env_set() {
        let _guard = DEV_TOKEN_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "true");
        assert!(
            dev_token_allowed(),
            "gate opens when {ALLOW_DEV_TOKEN_ENV}=true"
        );
        assert!(
            is_allowed_dev_token(DEV_API_TOKEN, dev_token_allowed()),
            "dev token accepted when the gate env is set"
        );

        std::env::set_var(ALLOW_DEV_TOKEN_ENV, "1");
        assert!(dev_token_allowed(), "gate also accepts =1");

        // Cleanup so other tests observe the default.
        std::env::remove_var(ALLOW_DEV_TOKEN_ENV);
    }

    #[test]
    #[serial_test::serial]
    fn non_dev_tokens_never_match_even_with_gate_open() {
        assert!(!is_allowed_dev_token("allternit_not_the_dev_token_0123", true));
        assert!(!is_allowed_dev_token("", true));
        assert!(!is_allowed_dev_token("dev-api-token ", true));
    }
}
