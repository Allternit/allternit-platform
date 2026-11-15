//! Authentication and authorization module
//!
//! Provides middleware for token validation, permission checking,
//! and development mode bypass capabilities.

pub mod clerk;
pub mod dataplane_jwt;
pub mod dev_token;
pub mod es256;
pub mod middleware;
pub mod models;
pub mod permissions;
pub mod resolve;

pub use middleware::{AuthContext, AuthLayer, AuthMiddleware};
pub use models::{ApiToken, AuthenticatedUser, TokenInfo};
pub use permissions::{Permission, PermissionChecker};
pub use resolve::{require_session_user, resolve_user, resolve_user_id, resolve_user_scoped};

/// Clerk user ids listed in `ALLTERNIT_ADMIN_USER_IDS` (comma-separated).
/// Admins operate the platform and are treated as Ultra for Cloud default /
/// subscription reads without a Stripe Checkout row.
pub fn is_admin_user(user_id: &str) -> bool {
    if user_id.trim().is_empty() {
        return false;
    }
    std::env::var("ALLTERNIT_ADMIN_USER_IDS")
        .map(|value| {
            value
                .split(',')
                .any(|entry| entry.trim() == user_id && !entry.trim().is_empty())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod admin_user_tests {
    use super::is_admin_user;

    #[test]
    fn admin_env_match() {
        std::env::set_var("ALLTERNIT_ADMIN_USER_IDS", "user_aaa, user_bbb");
        assert!(is_admin_user("user_aaa"));
        assert!(is_admin_user("user_bbb"));
        assert!(!is_admin_user("user_ccc"));
        assert!(!is_admin_user(""));
        std::env::remove_var("ALLTERNIT_ADMIN_USER_IDS");
    }
}
