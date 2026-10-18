//! Typed failures. `ErrorCode` is the `code` of `{"event":"error"}`; Desktop maps
//! each one to a user-readable message (phone-link-ios.ts `signRunner`).

use isideload::SideloadError;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Apple rejected the Apple ID or password.
    BadCredentials,
    /// No password was sent and there is no valid saved session.
    PasswordRequired,
    /// The verification code was wrong, never arrived, or 2FA was cancelled.
    TwoFactorFailed,
    /// Free accounts: 10 new App IDs per 7 days.
    AppIdLimit,
    /// Too many development certificates on the account (we never revoke others').
    CertificateLimit,
    /// Too many registered devices on the account.
    DeviceLimit,
    /// The anisette server couldn't be reached or refused us.
    AnisetteUnavailable,
    /// Desktop sent something we can't use.
    InvalidRequest,
    /// Anything else.
    SigningFailed,
}

impl ErrorCode {
    pub fn default_message(self) -> &'static str {
        match self {
            ErrorCode::BadCredentials => "Apple didn't accept that Apple ID or password.",
            ErrorCode::PasswordRequired => "Sign in with your Apple ID again.",
            ErrorCode::TwoFactorFailed => "The verification code didn't work.",
            ErrorCode::AppIdLimit => "This Apple ID has reached Apple's limit of new App IDs for this week.",
            ErrorCode::CertificateLimit => "This Apple ID has too many development certificates.",
            ErrorCode::DeviceLimit => "This Apple ID has too many registered devices.",
            ErrorCode::AnisetteUnavailable => "Apple's sign-in check didn't go through.",
            ErrorCode::InvalidRequest => "The signing request was invalid.",
            ErrorCode::SigningFailed => "Signing the helper failed.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Failure { code, message: message.into() }
    }

    pub fn code(code: ErrorCode) -> Self {
        Failure::new(code, code.default_message())
    }

    /// `code`'s default message plus a short technical detail.
    pub fn with_detail(code: ErrorCode, detail: &str) -> Self {
        let detail = short_detail(detail);
        if detail.is_empty() {
            Failure::code(code)
        } else {
            Failure::new(code, format!("{} ({detail})", code.default_message()))
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for Failure {}

/// First line, trimmed, at most 300 chars.
/// The whole cause chain on one line ("outer — cause — root"): isideload wraps
/// Apple's real answer under contexts like "Failed to log in to Apple ID", and
/// keeping only the first line hid it.
pub fn short_detail(text: &str) -> String {
    let parts: Vec<String> = text
        .lines()
        .map(|l| l.trim_start_matches(|c: char| c.is_whitespace() || "●├╰│─|".contains(c)).trim().to_string())
        .map(|l| l.trim_matches('"').to_string())
        // Source locations (isideload's file:line) say nothing to a person.
        .filter(|l| !l.is_empty() && !l.starts_with("at ") && !l.starts_with("Backtrace") && !l.contains(".rs:"))
        .collect();
    let mut seen = Vec::new();
    for p in parts {
        if !seen.contains(&p) {
            seen.push(p);
        }
    }
    seen.join(" — ").chars().take(300).collect()
}

// Apple GrandSlam (`ec`) and developer services (`resultCode`) codes. The
// developer-services ones are the codes AltServer/SideStore map (ALTAppleAPIError).
const GSA_BAD_CREDENTIALS: &[i64] = &[-20101, -22406, -20283, -20970];
const GSA_BAD_CODE: &[i64] = &[-21669];
// "This action cannot be completed at this time": Apple refused the anisette
// headers before looking at the password (mac_anisette.rs).
const GSA_ANISETTE_REFUSED: &[i64] = &[-80045];
const DEV_CERT_LIMIT: &[i64] = &[7460];
const DEV_APP_ID_LIMIT: &[i64] = &[9120];
const DEV_DEVICE_LIMIT: &[i64] = &[];

/// Map one of isideload's typed errors.
pub fn classify_sideload_error(err: &SideloadError) -> Option<ErrorCode> {
    match err {
        SideloadError::AuthWithMessage(code, msg) => {
            if GSA_BAD_CREDENTIALS.contains(code) {
                Some(ErrorCode::BadCredentials)
            } else if GSA_BAD_CODE.contains(code) {
                Some(ErrorCode::TwoFactorFailed)
            } else if GSA_ANISETTE_REFUSED.contains(code) {
                Some(ErrorCode::AnisetteUnavailable)
            } else {
                classify_text(msg)
            }
        }
        SideloadError::DeveloperError(code, msg) => {
            if DEV_CERT_LIMIT.contains(code) {
                Some(ErrorCode::CertificateLimit)
            } else if DEV_APP_ID_LIMIT.contains(code) {
                Some(ErrorCode::AppIdLimit)
            } else if DEV_DEVICE_LIMIT.contains(code) {
                Some(ErrorCode::DeviceLimit)
            } else {
                classify_text(msg)
            }
        }
        _ => None,
    }
}

/// Fallback on message text (isideload bails with plain strings in places).
pub fn classify_text(text: &str) -> Option<ErrorCode> {
    let t = text.to_ascii_lowercase();
    let has = |s: &str| t.contains(s);
    if has("not enough available app ids") || (has("app id") && (has("maximum") || has("limit"))) {
        Some(ErrorCode::AppIdLimit)
    } else if has("maximum number of certificates") || (has("certificate") && has("maximum")) {
        Some(ErrorCode::CertificateLimit)
    } else if has("device") && (has("maximum number") || has("limit")) {
        Some(ErrorCode::DeviceLimit)
    } else if has("2fa") || has("verification code") || has("security code") {
        Some(ErrorCode::TwoFactorFailed)
    } else if has("password") && (has("incorrect") || has("invalid") || has("wrong")) {
        Some(ErrorCode::BadCredentials)
    } else if has("anisette") {
        Some(ErrorCode::AnisetteUnavailable)
    } else {
        None
    }
}

/// Classify an isideload report: typed errors anywhere in the chain win, then
/// the rendered text, then `fallback`.
pub fn classify_report(report: &rootcause::Report, fallback: ErrorCode) -> Failure {
    let rendered = format!("{report:?}");
    let typed = report
        .iter_reports()
        .filter_map(|node| node.downcast_current_context::<SideloadError>())
        .find_map(classify_sideload_error);
    let code = typed.or_else(|| classify_text(&rendered)).unwrap_or(fallback);
    Failure::with_detail(code, &rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_apple_auth_codes() {
        let e = SideloadError::AuthWithMessage(-22406, "Your Apple ID or password was incorrect.".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::BadCredentials));
        let e = SideloadError::AuthWithMessage(-20101, "x".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::BadCredentials));
        let e = SideloadError::AuthWithMessage(-21669, "Incorrect verification code".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::TwoFactorFailed));
        let e = SideloadError::AuthWithMessage(-80045, "This action cannot be completed at this time.".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::AnisetteUnavailable));
    }

    #[test]
    fn maps_developer_limits() {
        let e = SideloadError::DeveloperError(7460, "Maximum number of certificates reached".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::CertificateLimit));
        let e = SideloadError::DeveloperError(9120, "Cannot create more than 10 App IDs".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::AppIdLimit));
        let e = SideloadError::DeveloperError(1, "You have reached the maximum number of devices".into());
        assert_eq!(classify_sideload_error(&e), Some(ErrorCode::DeviceLimit));
        let e = SideloadError::DeveloperError(35, "An invalid value was provided".into());
        assert_eq!(classify_sideload_error(&e), None);
    }

    #[test]
    fn maps_isideload_plain_messages() {
        assert_eq!(
            classify_text("Not enough available app IDs. 2 are required, but only 0 are available."),
            Some(ErrorCode::AppIdLimit)
        );
        assert_eq!(classify_text("No 2FA code provided, aborting"), Some(ErrorCode::TwoFactorFailed));
        assert_eq!(classify_text("Failed to request /v3/client_info from anisette server"), Some(ErrorCode::AnisetteUnavailable));
        assert_eq!(classify_text("connection reset"), None);
    }

    #[test]
    fn detail_is_the_whole_chain_on_one_short_line() {
        let f = Failure::with_detail(ErrorCode::SigningFailed, &format!("\n  first line\nsecond {}", "x".repeat(10)));
        assert_eq!(f.message, format!("Signing the helper failed. (first line — second {})", "x".repeat(10)));
        let long = "y".repeat(1000);
        assert!(Failure::with_detail(ErrorCode::SigningFailed, &long).message.len() < 400);
        assert_eq!(Failure::with_detail(ErrorCode::AppIdLimit, "").message, ErrorCode::AppIdLimit.default_message());
    }

    #[test]
    fn codes_serialize_like_desktop_expects() {
        let codes = [
            (ErrorCode::BadCredentials, "bad_credentials"),
            (ErrorCode::PasswordRequired, "password_required"),
            (ErrorCode::TwoFactorFailed, "two_factor_failed"),
            (ErrorCode::AppIdLimit, "app_id_limit"),
            (ErrorCode::CertificateLimit, "certificate_limit"),
            (ErrorCode::DeviceLimit, "device_limit"),
            (ErrorCode::AnisetteUnavailable, "anisette_unavailable"),
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::SigningFailed, "signing_failed"),
        ];
        for (code, s) in codes {
            assert_eq!(serde_json::to_string(&code).unwrap(), format!("\"{s}\""));
        }
    }

    #[test]
    fn classifies_a_report_chain() {
        let report: rootcause::Report = rootcause::report!("Maximum number of certificates reached");
        let f = classify_report(&report, ErrorCode::SigningFailed);
        assert_eq!(f.code, ErrorCode::CertificateLimit, "text fallback: {}", f.message);
        let report: rootcause::Report = rootcause::report!("Failed to retrieve certificate identity");
        assert_eq!(classify_report(&report, ErrorCode::SigningFailed).code, ErrorCode::SigningFailed);
        let report: rootcause::Report = rootcause::report!("socket closed");
        assert_eq!(classify_report(&report, ErrorCode::AnisetteUnavailable).code, ErrorCode::AnisetteUnavailable);
    }
}
