//! Collation for the harness drift hash.
//!
//! The JS implementation sorts every directory listing inside `hashDir` with
//! `Array.prototype.sort((a, b) => a.name.localeCompare(b.name))` — an ICU
//! locale-aware collation, NOT byte order. On this machine's Node/ICU that
//! ordering is locale-independent for printable ASCII and differs from byte
//! order in two ways that matter for real skill trees:
//!
//!   1. Case-insensitive letter primary: `SKILL.md` sorts AFTER
//!      `compute_hours.py` (s > c), while byte order puts uppercase first.
//!   2. Punctuation and digits sort before letters, each in their own block.
//!
//! This module replicates that collation with a fixed weight table. It was
//! validated exhaustively against `node localeCompare`: all 9,025 single-char
//! pairs over printable ASCII plus 30,000 random multi-char strings produced
//! identical order (see tests/ao_harness_parity evidence in the P4 notes).
//!
//! Known limitation (accepted, documented in docs/programs/ao/AO_HARNESS_PORT_NOTES.md):
//! non-ASCII names fall back to code-point order, which may diverge from ICU
//! for scripts with complex collation. The current 17 skills and their
//! contents are pure ASCII. If a non-ASCII skill/file name ever lands, the
//! drift hash will mismatch JS and every skill will read as drifted.

/// Punctuation block, in ICU primary-weight order (space first). Positions in
/// this string ARE the primary weights 1..=N.
const PUNCT: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

/// Primary weight for one char: lower sorts first.
fn primary(c: char) -> u32 {
    if c == ' ' {
        return 0;
    }
    if let Some(pos) = PUNCT.find(c) {
        return 1 + pos as u32;
    }
    if c.is_ascii_digit() {
        return 100 + (c as u32 - '0' as u32);
    }
    if c.is_ascii_alphabetic() {
        return 200 + (c.to_ascii_lowercase() as u32 - 'a' as u32);
    }
    // Fallback for anything outside the validated ASCII set: code-point
    // order after all ASCII primaries.
    1000 + c as u32
}

/// Tertiary (case) weight: lowercase before uppercase, matching ICU's
/// lowercase-first tertiary for the Latin script in this collation.
fn tertiary(c: char) -> u32 {
    if c.is_ascii_uppercase() {
        1
    } else {
        0
    }
}

/// `localeCompare` replica for the validated charset. Primary sequences are
/// compared element-wise; a strict prefix sorts first; only when the primary
/// sequences are fully equal is the case level compared element-wise.
pub(crate) fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_chars = a.chars();
    let mut b_chars = b.chars();
    loop {
        match (a_chars.next(), b_chars.next()) {
            (Some(x), Some(y)) => {
                let (wx, wy) = (primary(x), primary(y));
                if wx != wy {
                    return wx.cmp(&wy);
                }
            }
            (None, None) => break,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
        }
    }
    let mut a_chars = a.chars();
    let mut b_chars = b.chars();
    loop {
        match (a_chars.next(), b_chars.next()) {
            (Some(x), Some(y)) => {
                let (wx, wy) = (tertiary(x), tertiary(y));
                if wx != wy {
                    return wx.cmp(&wy);
                }
            }
            _ => return std::cmp::Ordering::Equal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Orderings captured live from `node -e localeCompare` on this machine
    /// (see module docs). If ICU ever changes underneath Node these pin the
    /// contract the parity gate proved.
    #[test]
    fn matches_node_localecompare_samples() {
        let cases: &[(&str, &str, std::cmp::Ordering)] = &[
            ("SKILL.md", "compute_hours.py", std::cmp::Ordering::Greater),
            ("scripts", "screenshot.js", std::cmp::Ordering::Greater),
            ("Screenshot.js", "scripts", std::cmp::Ordering::Less),
            ("a_b", "a-b", std::cmp::Ordering::Less),
            ("a-b", "ab", std::cmp::Ordering::Less),
            ("a1", "aa", std::cmp::Ordering::Less),
            ("z10", "z2", std::cmp::Ordering::Less),
            ("AB", "ab", std::cmp::Ordering::Greater),
            ("aB", "Ab", std::cmp::Ordering::Less),
            ("skill.md", "SKILL.md", std::cmp::Ordering::Less),
            ("aB", "ab-", std::cmp::Ordering::Less),
            ("file.md", "file1", std::cmp::Ordering::Less),
        ];
        for (a, b, want) in cases {
            assert_eq!(locale_compare(a, b), *want, "locale_compare({a:?}, {b:?})");
        }
    }

    #[test]
    fn single_char_order_matches_node() {
        // The exact single-char order string `node` produced for printable
        // ASCII; every adjacent pair (and every pair via transitivity of the
        // total order) must hold.
        let order = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ";
        let chars: Vec<char> = order.chars().collect();
        for i in 0..chars.len() {
            for j in 0..chars.len() {
                let want = i.cmp(&j);
                assert_eq!(
                    locale_compare(&chars[i].to_string(), &chars[j].to_string()),
                    want,
                    "chars {i} ({:?}) vs {j} ({:?})",
                    chars[i],
                    chars[j]
                );
            }
        }
    }
}
