//! Masking for log lines. Every line goes through here before it leaves the
//! node. Nothing in this module stores anything.
//!
//! Masked: IPv4 and IPv6 addresses (loopback and unspecified addresses stay;
//! the mesh range 100.64.0.0/10 stays when [`MaskOptions::keep_mesh`] is set),
//! the local part of email addresses, JWTs, `Bearer` / `Basic` credentials,
//! `password=` / `secret=` / `token=` / `api_key=` style values, AWS-style
//! access keys, private key blocks, and long (32+ characters) hex or base64
//! values in `key=value` context.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

use regex::{Captures, Regex};

/// What to mask. Secrets, tokens, keys and emails are always masked; only the
/// IP rules are optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskOptions {
    /// Mask IPv4 and IPv6 addresses. Default true.
    pub ips: bool,
    /// Leave mesh addresses (100.64.0.0/10) readable. Default false.
    pub keep_mesh: bool,
}

impl Default for MaskOptions {
    fn default() -> Self {
        MaskOptions { ips: true, keep_mesh: false }
    }
}

struct Patterns {
    key_inline: Regex,
    key_begin: Regex,
    key_end: Regex,
    jwt: Regex,
    bearer: Regex,
    aws: Regex,
    kv: Regex,
    long_kv: Regex,
    email: Regex,
    v6: Regex,
    v4: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("static mask pattern");
        Patterns {
            key_inline: re(r"-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----"),
            key_begin: re(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
            key_end: re(r"-----END [A-Z ]*PRIVATE KEY-----"),
            jwt: re(r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*"),
            bearer: re(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}"),
            aws: re(r"\b(?:AKIA|ASIA|AGPA|AIDA|AROA)[A-Z0-9]{16}\b"),
            kv: re(
                r#"(?i)(password|passwd|pwd|secret[_-]?key|client[_-]?secret|secret|token|api[_-]?key|access[_-]?key|private[_-]?key)(["']?\s*[:=]\s*["']?)([^\s"',;&]+)"#,
            ),
            long_kv: re(r#"\b([A-Za-z_][A-Za-z0-9_.-]*)(\s*[:=]\s*["']?)([A-Za-z0-9+/_-]{32,}={0,2})"#),
            email: re(r"[A-Za-z0-9._%+-]+@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})"),
            v6: re(r"[0-9A-Fa-f:.]{3,}"),
            v4: re(r"\b(?:\d{1,3}\.){3}\d{1,3}\b"),
        }
    })
}

/// Domains that are really systemd unit names (`user@1000.service`).
const UNIT_SUFFIXES: [&str; 8] = [
    ".service", ".scope", ".slice", ".socket", ".mount", ".timer", ".target", ".path",
];

fn is_mesh_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// Finds an IPv6 address inside a candidate token (which may carry a leading
/// `:` or trailing `.` / `:` from the sentence around it). Returns the byte
/// range and the parsed address.
fn find_v6(s: &str) -> Option<(usize, usize, Ipv6Addr)> {
    if s.matches(':').count() < 2 {
        return None;
    }
    let starts: &[usize] = if s.starts_with(':') { &[0, 1] } else { &[0] };
    for &a in starts {
        let body = &s[a..];
        let t1 = body.trim_end_matches('.');
        let t2 = t1.strip_suffix(':').unwrap_or(t1);
        for cand in [body, t1, t2] {
            if let Ok(ip) = cand.parse::<Ipv6Addr>() {
                return Some((a, a + cand.len(), ip));
            }
        }
    }
    None
}

fn mask_plain(line: &str, opts: &MaskOptions) -> String {
    let p = patterns();
    let mut s = p.jwt.replace_all(line, "[token]").into_owned();
    s = p.bearer.replace_all(&s, "${1} [token]").into_owned();
    s = p.aws.replace_all(&s, "[key]").into_owned();
    s = p
        .kv
        .replace_all(&s, |c: &Captures| {
            if c[3].starts_with('[') {
                c[0].to_string()
            } else {
                format!("{}{}[secret]", &c[1], &c[2])
            }
        })
        .into_owned();
    s = p
        .long_kv
        .replace_all(&s, |c: &Captures| {
            let v = &c[3];
            let has_digit = v.bytes().any(|b| b.is_ascii_digit());
            let has_alpha = v.bytes().any(|b| b.is_ascii_alphabetic());
            if has_digit && has_alpha && !v.starts_with('/') {
                format!("{}{}[secret]", &c[1], &c[2])
            } else {
                c[0].to_string()
            }
        })
        .into_owned();
    s = p
        .email
        .replace_all(&s, |c: &Captures| {
            let domain = c[1].to_ascii_lowercase();
            if UNIT_SUFFIXES.iter().any(|suf| domain.ends_with(suf)) {
                c[0].to_string()
            } else {
                format!("[email]@{}", &c[1])
            }
        })
        .into_owned();
    if opts.ips {
        s = p
            .v6
            .replace_all(&s, |c: &Captures| {
                let tok = &c[0];
                match find_v6(tok) {
                    Some((a, b, ip)) if !ip.is_loopback() && !ip.is_unspecified() => {
                        format!("{}[ip]{}", &tok[..a], &tok[b..])
                    }
                    _ => tok.to_string(),
                }
            })
            .into_owned();
        let keep_mesh = opts.keep_mesh;
        s = p
            .v4
            .replace_all(&s, |c: &Captures| match c[0].parse::<Ipv4Addr>() {
                Ok(ip) if ip.is_loopback() || ip.is_unspecified() || (keep_mesh && is_mesh_v4(ip)) => {
                    c[0].to_string()
                }
                Ok(_) => "[ip]".to_string(),
                Err(_) => c[0].to_string(),
            })
            .into_owned();
    }
    s
}

/// Stateful masker for one stream. It remembers whether the previous line was
/// inside a private key block, so the lines of the key body are masked too.
#[derive(Debug, Clone, Default)]
pub struct Masker {
    opts: MaskOptions,
    in_key: bool,
}

impl Masker {
    pub fn new(opts: MaskOptions) -> Self {
        Masker { opts, in_key: false }
    }

    pub fn mask_line(&mut self, line: &str) -> String {
        let p = patterns();
        if self.in_key {
            return match p.key_end.find(line) {
                Some(m) => {
                    self.in_key = false;
                    format!("[private key]{}", mask_plain(&line[m.end()..], &self.opts))
                }
                None => "[private key]".to_string(),
            };
        }
        if p.key_inline.is_match(line) {
            return mask_plain(&p.key_inline.replace_all(line, "\u{0}"), &self.opts).replace('\u{0}', "[private key]");
        }
        if let Some(m) = p.key_begin.find(line) {
            self.in_key = true;
            return format!("{}[private key]", mask_plain(&line[..m.start()], &self.opts));
        }
        mask_plain(line, &self.opts)
    }
}

/// Mask one line with no state (a private key block that spans lines is only
/// caught on its first line; use [`Masker`] for a stream).
pub fn mask_line(line: &str, opts: &MaskOptions) -> String {
    Masker::new(*opts).mask_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_opts(s: &str) -> MaskOptions {
        let mut o = MaskOptions::default();
        for kv in s.split(',') {
            let (k, v) = kv.split_once('=').expect("opts k=v");
            let v = v.trim() == "true";
            match k.trim() {
                "ips" => o.ips = v,
                "keep_mesh" => o.keep_mesh = v,
                other => panic!("unknown option {other}"),
            }
        }
        o
    }

    #[test]
    fn fixture_table() {
        let text = include_str!("../fixtures/logs/mask-cases.txt");
        let mut sections: Vec<(MaskOptions, Vec<(String, String)>)> = Vec::new();
        let mut cur: Option<(MaskOptions, Vec<(String, String)>)> = None;
        let mut pending: Option<String> = None;
        for raw in text.lines() {
            if raw.starts_with('#') {
                continue;
            }
            if raw.trim().is_empty() {
                if let Some(c) = cur.take() {
                    sections.push(c);
                }
                continue;
            }
            let c = cur.get_or_insert((MaskOptions::default(), Vec::new()));
            if let Some(o) = raw.strip_prefix("OPTS|") {
                c.0 = parse_opts(o);
            } else if let Some(i) = raw.strip_prefix("IN|") {
                assert!(pending.is_none(), "IN without OUT: {raw}");
                pending = Some(i.to_string());
            } else if let Some(o) = raw.strip_prefix("OUT|") {
                let i = pending.take().expect("OUT without IN");
                c.1.push((i, o.to_string()));
            } else {
                panic!("bad fixture line: {raw}");
            }
        }
        if let Some(c) = cur.take() {
            sections.push(c);
        }
        assert!(pending.is_none());
        let mut cases = 0;
        for (opts, rows) in sections {
            let mut m = Masker::new(opts);
            for (input, want) in rows {
                assert_eq!(m.mask_line(&input), want, "input: {input}");
                cases += 1;
            }
        }
        assert!(cases >= 25, "fixture lost cases: {cases}");
    }

    #[test]
    fn stateless_helper_masks_inline() {
        let got = mask_line("from 8.8.8.8", &MaskOptions::default());
        assert_eq!(got, "from [ip]");
    }

    #[test]
    fn masking_is_idempotent() {
        let once = mask_line("user a@b.io password=x 8.8.8.8", &MaskOptions::default());
        assert_eq!(mask_line(&once, &MaskOptions::default()), once);
    }
}
