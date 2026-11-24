//! Meeting link parsing, `url_key` normalisation and the 1:1 rule
//! (docs/meetings/CONTRACT.md section 7). Pure functions, no I/O.
//!
//! The stored `join_url` keeps what the user gave (a Zoom `pwd` stays in it);
//! `url_key` is the normalised key behind the one-bot-per-meeting dedupe, so
//! two spellings of the same call must give the same key.

use std::sync::OnceLock;

use regex::Regex;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Meet,
    Zoom,
    Teams,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Meet => "meet",
            Platform::Zoom => "zoom",
            Platform::Teams => "teams",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "meet" => Some(Platform::Meet),
            "zoom" => Some(Platform::Zoom),
            "teams" => Some(Platform::Teams),
            _ => None,
        }
    }

    /// The label the UI and the MCP tools show.
    pub fn label(self) -> &'static str {
        match self {
            Platform::Meet => "Google Meet",
            Platform::Zoom => "Zoom",
            Platform::Teams => "Microsoft Teams",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLink {
    pub platform: Platform,
    /// The link as given (trimmed), stored as `join_url`.
    pub join_url: String,
    pub url_key: String,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("meeting link pattern compiles"))
}

static MEET: OnceLock<Regex> = OnceLock::new();
static MEET_LOOKUP: OnceLock<Regex> = OnceLock::new();
static ZOOM_NUMBER: OnceLock<Regex> = OnceLock::new();
static ZOOM_WC: OnceLock<Regex> = OnceLock::new();
static ZOOM_MY: OnceLock<Regex> = OnceLock::new();
static TEAMS_MEETUP: OnceLock<Regex> = OnceLock::new();
static TEAMS_MEET: OnceLock<Regex> = OnceLock::new();
static TEAMS_LIVE: OnceLock<Regex> = OnceLock::new();
static URL_IN_TEXT: OnceLock<Regex> = OnceLock::new();

/// Lower-cases the scheme and host (paths keep their case) and adds
/// `https://` to a bare `meet.google.com/...` style paste.
fn normalise(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None => ("https".to_string(), trimmed),
    };
    if scheme != "https" {
        return None;
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    Some(format!("https://{}{}", host.to_ascii_lowercase(), tail))
}

/// Percent-decodes `%XX` escapes so `%3a`, `%3A` and `:` hash the same.
fn percent_decode(path: &str) -> String {
    urlencoding::decode(path)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| path.to_string())
}

fn short_sha(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Parse one meeting link. `None` means `400 unsupported_link`.
pub fn parse_link(input: &str) -> Option<ParsedLink> {
    let url = normalise(input)?;
    let join_url = input.trim().to_string();
    let found = |platform: Platform, url_key: String| {
        Some(ParsedLink { platform, join_url: join_url.clone(), url_key })
    };

    // Meet codes are case-insensitive.
    let meet = re(&MEET, r"(?i)^https://meet\.google\.com/([a-z]{3}-[a-z]{4}-[a-z]{3})(?:[/?#].*)?$");
    if let Some(c) = meet.captures(&url) {
        return found(Platform::Meet, format!("meet:{}", c[1].to_ascii_lowercase()));
    }
    let lookup = re(&MEET_LOOKUP, r"^https://meet\.google\.com/lookup/([A-Za-z0-9_-]+)(?:[/?#].*)?$");
    if let Some(c) = lookup.captures(&url) {
        return found(Platform::Meet, format!("meet:lookup:{}", c[1].to_ascii_lowercase()));
    }

    let zoom_number = re(
        &ZOOM_NUMBER,
        r"^https://(?:[a-z0-9-]+\.)?zoom\.(?:us|com)/(?:j|my|wc/join)/(\d{9,11})(?:[/?#].*)?$",
    );
    if let Some(c) = zoom_number.captures(&url) {
        return found(Platform::Zoom, format!("zoom:{}", &c[1]));
    }
    let zoom_wc = re(&ZOOM_WC, r"^https://(?:[a-z0-9-]+\.)?zoom\.(?:us|com)/wc/(\d{9,11})/join(?:[/?#].*)?$");
    if let Some(c) = zoom_wc.captures(&url) {
        return found(Platform::Zoom, format!("zoom:{}", &c[1]));
    }
    let zoom_my = re(&ZOOM_MY, r"^https://(?:[a-z0-9-]+\.)?zoom\.(?:us|com)/my/([A-Za-z0-9._-]+)(?:[/?#].*)?$");
    if let Some(c) = zoom_my.captures(&url) {
        return found(Platform::Zoom, format!("zoom:my:{}", c[1].to_ascii_lowercase()));
    }

    let meetup = re(&TEAMS_MEETUP, r"^https://teams\.microsoft\.com(/l/meetup-join/[^\s?#]+)(?:[?#]\S*)?$");
    if let Some(c) = meetup.captures(&url) {
        return found(Platform::Teams, format!("teams:{}", short_sha(&percent_decode(&c[1]))));
    }
    let teams_meet = re(&TEAMS_MEET, r"^https://teams\.microsoft\.com/meet/(\d{9,15})(?:\?p=[\w-]+)?$");
    if let Some(c) = teams_meet.captures(&url) {
        return found(Platform::Teams, format!("teams:{}", &c[1]));
    }
    let teams_live = re(&TEAMS_LIVE, r"^https://teams\.live\.com/meet/(\d{9,15})(?:[/?#].*)?$");
    if let Some(c) = teams_live.captures(&url) {
        return found(Platform::Teams, format!("teams:{}", &c[1]));
    }
    None
}

/// Scan free text (a chat paste, a calendar event body or location) for the
/// first supported meeting link. Trailing sentence punctuation is dropped.
pub fn find_in_text(text: &str) -> Option<ParsedLink> {
    let urls = re(&URL_IN_TEXT, r#"(?i)https://[^\s<>"'\]\[(){}]+"#);
    urls.find_iter(text).find_map(|m| {
        let candidate = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        parse_link(candidate)
    })
}

/// 1:1 rule: exactly two attendees (the user and one other). Events with no
/// attendee list are not 1:1; declined invites still count.
pub fn is_one_on_one(attendee_count: Option<i32>) -> bool {
    attendee_count == Some(2)
}

/// The user's auto-join rule (`user_meeting_settings.auto_join`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoJoinRule {
    Off,
    All,
    AllExcept1on1,
}

impl AutoJoinRule {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(AutoJoinRule::Off),
            "all" => Some(AutoJoinRule::All),
            "all_except_1on1" => Some(AutoJoinRule::AllExcept1on1),
            _ => None,
        }
    }
}

/// Whether Gizzi will join this meeting on its own: a per-meeting
/// `auto_join` always wins; otherwise the user's rule decides.
pub fn will_auto_join(per_meeting: Option<bool>, rule: AutoJoinRule, one_on_one: bool) -> bool {
    match per_meeting {
        Some(on) => on,
        None => match rule {
            AutoJoinRule::Off => false,
            AutoJoinRule::All => true,
            AutoJoinRule::AllExcept1on1 => !one_on_one,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(url: &str) -> Option<String> {
        parse_link(url).map(|p| p.url_key)
    }

    #[test]
    fn meet_codes_normalise_case_and_ignore_query() {
        assert_eq!(key("https://meet.google.com/abc-defg-hij").as_deref(), Some("meet:abc-defg-hij"));
        assert_eq!(key("https://MEET.google.com/ABC-DEFG-HIJ?authuser=1").as_deref(), Some("meet:abc-defg-hij"));
        assert_eq!(key("  meet.google.com/abc-defg-hij/  ").as_deref(), Some("meet:abc-defg-hij"));
        assert_eq!(key("https://meet.google.com/lookup/AbC123").as_deref(), Some("meet:lookup:abc123"));
        let parsed = parse_link("https://meet.google.com/abc-defg-hij#x").unwrap();
        assert_eq!(parsed.platform, Platform::Meet);
        assert_eq!(parsed.join_url, "https://meet.google.com/abc-defg-hij#x");
    }

    #[test]
    fn meet_rejects_wrong_shapes() {
        for bad in [
            "https://meet.google.com/abcd-efg-hij",
            "https://meet.google.com/",
            "http://meet.google.com/abc-defg-hij",
            "https://meet.google.com.evil.com/abc-defg-hij",
            "https://evilmeet.google.com/abc-defg-hij",
            "ftp://meet.google.com/abc-defg-hij",
        ] {
            assert_eq!(key(bad), None, "{bad}");
        }
    }

    #[test]
    fn zoom_keys_drop_password_and_subdomain() {
        assert_eq!(key("https://us02web.zoom.us/j/85512345678?pwd=abc").as_deref(), Some("zoom:85512345678"));
        assert_eq!(key("https://zoom.us/j/855123456").as_deref(), Some("zoom:855123456"));
        assert_eq!(key("https://acme.zoom.com/wc/join/85512345678").as_deref(), Some("zoom:85512345678"));
        assert_eq!(key("https://zoom.us/wc/85512345678/join?pwd=x").as_deref(), Some("zoom:85512345678"));
        assert_eq!(key("https://zoom.us/my/Jordan.Lee").as_deref(), Some("zoom:my:jordan.lee"));
        // Same meeting, password or not: same key; the pwd stays in join_url.
        let with_pwd = parse_link("https://us02web.zoom.us/j/85512345678?pwd=abc").unwrap();
        assert_eq!(with_pwd.join_url, "https://us02web.zoom.us/j/85512345678?pwd=abc");
        assert_eq!(key("https://zoom.us/j/85512345678").unwrap(), with_pwd.url_key);
    }

    #[test]
    fn zoom_rejects_short_numbers_and_other_hosts() {
        for bad in ["https://zoom.us/j/12345", "https://zoom.us/s/85512345678", "https://zoomus.com/j/85512345678"] {
            assert_eq!(key(bad), None, "{bad}");
        }
    }

    #[test]
    fn teams_meetup_join_hashes_the_decoded_path() {
        let a = "https://teams.microsoft.com/l/meetup-join/19%3ameeting_NzE2@thread.v2/0?context=%7b%22Tid%22%7d";
        let b = "https://teams.microsoft.com/l/meetup-join/19%3Ameeting_NzE2@thread.v2/0?context=other";
        let c = "https://teams.microsoft.com/l/meetup-join/19:meeting_NzE2@thread.v2/0";
        let ka = key(a).unwrap();
        assert!(ka.starts_with("teams:") && ka.len() == "teams:".len() + 16, "{ka}");
        assert_eq!(key(b).unwrap(), ka, "escape case and query don't change the key");
        assert_eq!(key(c).unwrap(), ka, "decoded path is the same meeting");
        assert_ne!(key("https://teams.microsoft.com/l/meetup-join/19:meeting_Other@thread.v2/0").unwrap(), ka);
    }

    #[test]
    fn teams_meet_numbers() {
        assert_eq!(key("https://teams.microsoft.com/meet/2345678901234?p=AbC-12").as_deref(), Some("teams:2345678901234"));
        assert_eq!(key("https://teams.microsoft.com/meet/2345678901234").as_deref(), Some("teams:2345678901234"));
        assert_eq!(key("https://teams.live.com/meet/9876543210?p=x").as_deref(), Some("teams:9876543210"));
        assert_eq!(key("https://teams.microsoft.com/meet/12345"), None);
        assert_eq!(key("https://teams.microsoft.com/meet/2345678901234?x=1"), None);
    }

    #[test]
    fn anything_else_is_unsupported() {
        for bad in ["", "hello", "https://example.com/meet", "https://webex.com/meet/xyz", "https://meet.google.com/abc-defg-hij extra"] {
            assert_eq!(parse_link(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn free_text_scan_takes_the_first_supported_link() {
        let text = "Agenda: https://docs.example.com/x. Join: https://meet.google.com/abc-defg-hij, or dial in.";
        assert_eq!(find_in_text(text).unwrap().url_key, "meet:abc-defg-hij");
        let text = "Zoom (https://zoom.us/j/85512345678?pwd=Q1) then https://meet.google.com/abc-defg-hij";
        let parsed = find_in_text(text).unwrap();
        assert_eq!(parsed.url_key, "zoom:85512345678");
        assert_eq!(parsed.join_url, "https://zoom.us/j/85512345678?pwd=Q1");
        assert_eq!(find_in_text("no link here https://example.com"), None);
    }

    #[test]
    fn one_on_one_is_exactly_two_attendees() {
        assert!(is_one_on_one(Some(2)));
        for n in [None, Some(0), Some(1), Some(3)] {
            assert!(!is_one_on_one(n), "{n:?}");
        }
    }

    #[test]
    fn per_meeting_override_beats_the_rule() {
        use AutoJoinRule::*;
        assert!(!will_auto_join(None, Off, false));
        assert!(will_auto_join(None, All, true));
        assert!(will_auto_join(None, AllExcept1on1, false));
        assert!(!will_auto_join(None, AllExcept1on1, true), "1:1 skipped by the rule");
        assert!(will_auto_join(Some(true), AllExcept1on1, true), "override on wins");
        assert!(!will_auto_join(Some(false), All, false), "override off wins");
    }
}
