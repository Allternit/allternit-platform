//! One-time codes and magic links in inbound bot email.
//!
//! [`extract`] reads a message's subject, plain text and HTML and returns the
//! verification codes (4–8 digits, or 6–10 letters+digits next to a code
//! keyword), the sign-in / confirm links, and whether the message looks like a
//! verification email at all. Years, phone numbers, prices, dates and
//! unsubscribe links are skipped. The inbound webhook stores the result on the
//! `agent_email_inbound` row; `allternit_mail.read_otp` reads it back.

use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

/// What [`extract`] found. Codes and links are newest-signal first: codes next
/// to a keyword come before bare digit runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Extraction {
    pub codes: Vec<String>,
    pub links: Vec<String>,
    pub is_verification: bool,
}

const MAX_CODES: usize = 5;
const MAX_LINKS: usize = 5;
/// How far (in bytes) a keyword may sit before or after a code to count as "next to" it.
const NEAR_BEFORE: usize = 60;
const NEAR_AFTER: usize = 30;

/// Words that put an alphanumeric token in code position.
const CODE_KEYWORDS: &[&str] = &["code", "otp", "verification", "passcode", "pin", "2fa", "one-time", "one time", "security code", "token"];

/// Phrases that make a message a verification email (with a code or link present).
const VERIFY_KEYWORDS: &[&str] = &[
    "verification", "verify", "otp", "passcode", "one-time", "one time", "2fa", "two-factor", "two factor",
    "security code", "login code", "log in code", "sign-in code", "sign in code", "confirmation code",
    "your code", "the code", "magic link", "confirm your", "activate your", "sign in to", "log in to",
    "sign-in link", "login link",
];

/// Path/query words that make an https link a magic / confirm link.
const LINK_WORDS: &[&str] = &["verify", "verification", "confirm", "login", "log-in", "signin", "sign-in", "magic", "token", "auth", "activate"];
const LINK_EXCLUDE: &[&str] = &["unsubscribe", "unsub", "opt-out", "optout", "email-preferences", "manage-preferences"];
const IMAGE_EXT: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".js"];

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("constant regex"))
}

/// HTML to readable text: drop style/script/head, turn tags into spaces, decode the common entities.
pub fn html_to_text(html: &str) -> String {
    static BLOCKS: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    let no_blocks = re(&BLOCKS, r"(?is)<(style|script|head|title)[^>]*>.*?</(style|script|head|title)\s*>").replace_all(html, " ");
    let no_tags = re(&TAGS, r"(?s)<[^>]*>").replace_all(&no_blocks, " ");
    decode_entities(&no_tags)
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&#61;", "=")
        .replace("&#x3D;", "=")
        .replace("&amp;", "&")
}

/// Every https link in the text (and HTML attributes), trailing punctuation trimmed.
fn all_links(text: &str) -> Vec<String> {
    static URL: OnceLock<Regex> = OnceLock::new();
    re(&URL, r#"https://[^\s"'<>()\[\]{}`]+"#)
        .find_iter(text)
        .map(|m| decode_entities(m.as_str()).trim_end_matches(['.', ',', ';', ':', '!', '?', '*']).to_string())
        .collect()
}

fn is_magic_link(link: &str) -> bool {
    let Ok(url) = url::Url::parse(link) else { return false };
    if url.scheme() != "https" {
        return false;
    }
    let path = url.path().to_ascii_lowercase();
    let query = url.query().unwrap_or("").to_ascii_lowercase();
    let whole = link.to_ascii_lowercase();
    if LINK_EXCLUDE.iter().any(|w| whole.contains(w)) {
        return false;
    }
    if IMAGE_EXT.iter().any(|e| path.ends_with(e)) {
        return false;
    }
    LINK_WORDS.iter().any(|w| path.contains(w) || query.contains(w))
}

/// Byte offsets of every keyword hit in `lower` (already lower-cased).
fn keyword_hits(lower: &str, words: &[&str]) -> Vec<(usize, usize)> {
    let mut hits = Vec::new();
    for w in words {
        let mut from = 0;
        while let Some(i) = lower[from..].find(w) {
            let start = from + i;
            let end = start + w.len();
            // Whole words only ("pin" must not match "shipping").
            let before_ok = lower[..start].chars().next_back().map_or(true, |c| !c.is_ascii_alphanumeric());
            let after_ok = lower[end..].chars().next().map_or(true, |c| !c.is_ascii_alphanumeric());
            if before_ok && after_ok {
                hits.push((start, end));
            }
            from = end;
        }
    }
    hits
}

fn near(hits: &[(usize, usize)], start: usize, end: usize) -> bool {
    hits.iter().any(|&(ks, ke)| (ke <= start && start - ke <= NEAR_BEFORE) || (ks >= end && ks - end <= NEAR_AFTER))
}

fn prev_char(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

fn next_char(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}

/// A digit run that is part of a phone number, price, date, decimal, version,
/// time or longer number — anything but a standalone code.
fn digits_embedded(s: &str, start: usize, end: usize) -> bool {
    let before = prev_char(s, start);
    let after = next_char(s, end);
    let before2 = before.and_then(|c| prev_char(s, start - c.len_utf8()));
    let after2 = after.and_then(|c| next_char(s, end + c.len_utf8()));
    let digitish = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    // Currency / phone prefix / percent.
    if matches!(before, Some('$' | '€' | '£' | '¥' | '+' | '#')) || matches!(after, Some('%')) {
        return true;
    }
    // "USD 1234" / "1234 USD".
    let tail = s[end..].trim_start().to_ascii_lowercase();
    if ["usd", "eur", "gbp", "dollars"].iter().any(|c| tail.starts_with(c)) {
        return true;
    }
    let head = s[..start].trim_end().to_ascii_lowercase();
    if ["usd", "eur", "gbp", "$", "€", "£"].iter().any(|c| head.ends_with(c)) {
        return true;
    }
    // Joined to other digits by a separator: 555-123-4567, 12.50, 10/20/2026, 12:30, 1,000.
    for (sep, other) in [(before, before2), (after, after2)] {
        if matches!(sep, Some('-' | '.' | ',' | '/' | ':' | '(' | ')')) && digitish(other) {
            return true;
        }
    }
    // "(555) 1234567" style phone numbers.
    if s[..start].trim_end().ends_with(')') {
        return true;
    }
    // Inside a longer alphanumeric token (an id like "A12345B" is handled by the alnum path).
    before.is_some_and(|c| c.is_ascii_alphanumeric()) || after.is_some_and(|c| c.is_ascii_alphanumeric())
}

fn looks_like_year(code: &str) -> bool {
    code.len() == 4 && code.parse::<u32>().is_ok_and(|y| (1900..=2099).contains(&y))
}

/// Codes in one block of text: `(code, near_keyword)`.
fn codes_in(text: &str) -> Vec<(String, bool)> {
    static DIGITS: OnceLock<Regex> = OnceLock::new();
    static GROUPED: OnceLock<Regex> = OnceLock::new();
    static ALNUM: OnceLock<Regex> = OnceLock::new();
    static URLS: OnceLock<Regex> = OnceLock::new();
    static EMAILS: OnceLock<Regex> = OnceLock::new();
    // Digits inside links and addresses are never codes; blank them out with
    // same-length padding so byte offsets stay valid.
    let blank = |s: &str, r: &Regex| r.replace_all(s, |c: &regex::Captures| " ".repeat(c[0].len())).into_owned();
    let text = blank(text, re(&URLS, r#"(?i)https?://[^\s"'<>]+"#));
    let text = blank(&text, re(&EMAILS, r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"));
    let lower = text.to_ascii_lowercase();
    let hits = keyword_hits(&lower, CODE_KEYWORDS);
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut push = |code: String, kw: bool| {
        if !out.iter().any(|(c, _)| *c == code) {
            out.push((code, kw));
        }
    };

    // "123 456" / "123-456": a split six-to-eight-digit code, only next to a keyword
    // (a bare "555-1234" is a phone number).
    for m in re(&GROUPED, r"\b\d{3,4}[ -]\d{3,4}\b").find_iter(&text) {
        let joined: String = m.as_str().chars().filter(char::is_ascii_digit).collect();
        if (6..=8).contains(&joined.len()) && near(&hits, m.start(), m.end()) && !digits_embedded(&text, m.start(), m.end()) {
            push(joined, true);
        }
    }
    for m in re(&DIGITS, r"\d{4,8}").find_iter(&text) {
        let (s, e) = (m.start(), m.end());
        // The regex is greedy; a longer run shows up as an alnum neighbour and is skipped.
        if digits_embedded(&text, s, e) {
            continue;
        }
        let kw = near(&hits, s, e);
        if looks_like_year(m.as_str()) && !kw {
            continue;
        }
        push(m.as_str().to_string(), kw);
    }
    for m in re(&ALNUM, r"\b[A-Za-z0-9]{6,10}\b").find_iter(&text) {
        let tok = m.as_str();
        let has_digit = tok.bytes().any(|b| b.is_ascii_digit());
        let has_alpha = tok.bytes().any(|b| b.is_ascii_alphabetic());
        if !(has_digit && has_alpha) || !near(&hits, m.start(), m.end()) {
            continue;
        }
        // Ordinal/measurement words ("100px", "2fa", "10th") aren't codes.
        let low = tok.to_ascii_lowercase();
        if ["px", "th", "st", "nd", "rd", "pm", "am", "kb", "mb", "gb"].iter().any(|s| low.ends_with(s) && low[..low.len() - s.len()].bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        if matches!(prev_char(&text, m.start()), Some('#' | '$')) {
            continue;
        }
        push(tok.to_string(), true);
    }
    out
}

/// Extract codes and magic links from one message.
pub fn extract(subject: Option<&str>, text: Option<&str>, html: Option<&str>) -> Extraction {
    let subject = subject.unwrap_or("");
    let text = text.unwrap_or("");
    let html = html.unwrap_or("");
    let html_text = if html.is_empty() { String::new() } else { html_to_text(html) };

    let mut scored: Vec<(String, bool)> = Vec::new();
    for block in [subject, text, html_text.as_str()] {
        for (code, kw) in codes_in(block) {
            match scored.iter_mut().find(|(c, _)| *c == code) {
                Some(existing) => existing.1 |= kw,
                None => scored.push((code, kw)),
            }
        }
    }
    // Keyword-adjacent codes first; order of appearance otherwise.
    scored.sort_by_key(|(_, kw)| !*kw);

    let mut links: Vec<String> = Vec::new();
    for link in all_links(text).into_iter().chain(all_links(html)) {
        if is_magic_link(&link) && !links.contains(&link) {
            links.push(link);
        }
    }
    links.truncate(MAX_LINKS);

    let all_lower = format!("{subject}\n{text}\n{html_text}").to_ascii_lowercase();
    let has_keyword = !keyword_hits(&all_lower, VERIFY_KEYWORDS).is_empty();
    let has_near_code = scored.iter().any(|(_, kw)| *kw);
    // Without a keyword, a bare digit run is likely an order number, not a code.
    if !has_keyword {
        scored.retain(|(_, kw)| *kw);
    }
    let codes: Vec<String> = scored.into_iter().map(|(c, _)| c).take(MAX_CODES).collect();
    let is_verification = (has_keyword && (!codes.is_empty() || !links.is_empty())) || has_near_code;

    Extraction { codes, links, is_verification }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_six_digit_code() {
        let x = extract(Some("Your verification code"), Some("Your code is 482913. It expires in 10 minutes."), None);
        assert_eq!(x.codes, vec!["482913"]);
        assert!(x.is_verification);
    }

    #[test]
    fn split_code_and_subject_code() {
        let x = extract(Some("123 456 is your Acme login code"), Some("Use 123 456 to sign in."), None);
        assert_eq!(x.codes.first().map(String::as_str), Some("123456"));
        assert!(x.is_verification);
    }

    #[test]
    fn alnum_code_next_to_keyword() {
        let x = extract(Some("Sign in"), Some("Your one-time passcode: K7Q2ZP9"), None);
        assert_eq!(x.codes, vec!["K7Q2ZP9"]);
        assert!(x.is_verification);
    }

    #[test]
    fn skips_years_phones_and_prices() {
        let x = extract(
            Some("Your receipt"),
            Some("Thanks for your order in 2026. Total $1299 or 1299 USD, 15% off. Call (555) 123-4567 or +1 5551234567. Ref 10/20/2026 at 12:30."),
            None,
        );
        assert!(x.codes.is_empty(), "{:?}", x.codes);
        assert!(!x.is_verification);
    }

    #[test]
    fn year_next_to_code_keyword_is_kept() {
        let x = extract(None, Some("Your verification code is 2048"), None);
        assert_eq!(x.codes, vec!["2048"]);
    }

    #[test]
    fn magic_link_from_html_without_unsubscribe() {
        let html = r#"<html><head><style>.a{color:#123456}</style></head><body>
            <p>Click to sign in to Acme:</p>
            <a href="https://app.acme.com/auth/magic?token=abc&amp;email=bot%40x.com">Sign in</a>
            <a href="https://acme.com/unsubscribe?u=1">Unsubscribe</a>
            <img src="https://t.acme.com/open/token123.png"></body></html>"#;
        let x = extract(Some("Your sign-in link"), None, Some(html));
        assert_eq!(x.links, vec!["https://app.acme.com/auth/magic?token=abc&email=bot%40x.com"]);
        assert!(x.codes.is_empty(), "{:?}", x.codes);
        assert!(x.is_verification);
    }

    #[test]
    fn confirm_link_in_text() {
        let x = extract(Some("Confirm your email"), Some("Confirm your account: https://example.com/confirm/9f8e7d6c."), None);
        assert_eq!(x.links, vec!["https://example.com/confirm/9f8e7d6c"]);
        assert!(x.is_verification);
    }

    #[test]
    fn ordinary_mail_is_not_verification() {
        let x = extract(
            Some("Lunch on Friday?"),
            Some("Hey, want to grab lunch at 12? Here's the menu: https://cafe.example.com/menu"),
            None,
        );
        assert!(x.codes.is_empty());
        assert!(x.links.is_empty());
        assert!(!x.is_verification);
    }

    #[test]
    fn newsletter_login_link_alone_is_not_verification() {
        let x = extract(Some("This week's news"), Some("Read more: https://news.example.com/login?next=/post/1"), None);
        assert_eq!(x.links.len(), 1);
        assert!(!x.is_verification);
    }

    #[test]
    fn digits_in_urls_and_addresses_are_not_codes() {
        let x = extract(Some("Verify your email"), Some("Write to user123456@example.com or open https://example.com/p/98765432 to verify"), None);
        assert!(x.codes.is_empty(), "{:?}", x.codes);
    }

    #[test]
    fn pin_matches_whole_word_only() {
        let x = extract(None, Some("Shipping 12345678 tracking"), None);
        assert!(x.codes.is_empty(), "{:?}", x.codes);
    }
}
