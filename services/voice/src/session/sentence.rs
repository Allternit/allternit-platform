//! Streaming sentence splitter for `speak.delta`.
//!
//! Reply text arrives in arbitrary token-sized pieces. The splitter emits a
//! sentence as soon as its end is certain, so TTS can start on the first
//! sentence while the model is still writing the rest. It does not split on:
//! decimals ("3.14"), initialisms ("U.S."), single initials ("J. Smith"),
//! common abbreviations ("Dr.", "e.g."), a period followed by a lowercase
//! word, or an ellipsis that continues in lowercase ("so... um").
//! Over-joining is harmless (the TTS reads the joined text); a wrong split
//! is audible, so ambiguous cases join.

/// Abbreviations (lowercase, without dots) whose period never ends a sentence.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "mx", "dr", "prof", "st", "sr", "jr", "vs", "etc", "eg", "ie", "approx",
    "no", "vol", "dept", "est", "fig", "capt", "lt", "col", "gen", "sen", "rep", "gov", "pres",
    "inc", "ltd", "co", "corp", "mt", "ft", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep",
    "sept", "oct", "nov", "dec", "e.g", "i.e",
];

const TERMINATORS: &[char] = &['.', '!', '?', '…'];
const CLOSERS: &[char] = &['"', '\'', '”', '’', ')', ']'];

/// A sentence longer than this with no boundary is cut at a clause break
/// so first audio is never held hostage by a run-on sentence.
const MAX_SENTENCE_CHARS: usize = 280;

#[derive(Debug, Default)]
pub struct SentenceSplitter {
    buf: String,
}

impl SentenceSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append text and return every sentence that is now complete.
    pub fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        loop {
            let cut = find_boundary(&self.buf).or_else(|| overflow_cut(&self.buf));
            let Some(cut) = cut else { break };
            let rest = self.buf.split_off(cut);
            let sentence = std::mem::replace(&mut self.buf, rest.trim_start().to_string());
            if let Some(s) = clean_for_speech(&sentence) {
                out.push(s);
            }
        }
        out
    }

    /// No more text: return whatever is left.
    pub fn flush(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        clean_for_speech(&rest)
    }

    pub fn is_empty(&self) -> bool {
        self.buf.trim().is_empty()
    }
}

/// Split a complete text in one go (used by tests and one-shot callers).
pub fn split_all(text: &str) -> Vec<String> {
    let mut s = SentenceSplitter::new();
    let mut out = s.push(text);
    out.extend(s.flush());
    out
}

/// Byte index just past the first certain sentence end, if any.
fn find_boundary(buf: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = buf.char_indices().collect();
    let n = chars.len();
    let byte_after = |k: usize| chars.get(k + 1).map(|c| c.0).unwrap_or(buf.len());
    let mut i = 0;
    while i < n {
        let c = chars[i].1;
        if c == '\n' {
            if !buf[..chars[i].0].trim().is_empty() {
                return Some(chars[i].0);
            }
            i += 1;
            continue;
        }
        if !TERMINATORS.contains(&c) {
            i += 1;
            continue;
        }
        // The run of terminators, then any closing quotes/brackets.
        let mut j = i;
        while j + 1 < n && TERMINATORS.contains(&chars[j + 1].1) {
            j += 1;
        }
        let mut k = j;
        while k + 1 < n && CLOSERS.contains(&chars[k + 1].1) {
            k += 1;
        }
        if k + 1 >= n {
            return None; // need to see what follows
        }
        if !chars[k + 1].1.is_whitespace() {
            i = k + 1; // "3.14", "U.S.", "?!x"
            continue;
        }
        let end = byte_after(k);
        let run: String = chars[i..=j].iter().map(|c| c.1).collect();
        if run.contains('!') || run.contains('?') {
            return Some(end);
        }
        // A period or ellipsis: decide from the next word.
        let mut m = k + 1;
        while m < n && chars[m].1.is_whitespace() {
            m += 1;
        }
        if m >= n {
            return None;
        }
        let next = chars[m].1;
        let is_ellipsis = run.contains('…') || run.chars().filter(|&c| c == '.').count() >= 2;
        if is_ellipsis {
            if next.is_uppercase() || next.is_ascii_digit() {
                return Some(end);
            }
        } else if !abbreviation_before(&chars[..i]) && !next.is_lowercase() {
            return Some(end);
        }
        i = k + 1;
    }
    None
}

/// Whether the word right before a period is an abbreviation or initial.
fn abbreviation_before(before: &[(usize, char)]) -> bool {
    let word: String = before
        .iter()
        .rev()
        .take_while(|c| !c.1.is_whitespace())
        .map(|c| c.1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let word = word.trim_start_matches(|c: char| !c.is_alphanumeric());
    if word.is_empty() {
        return false;
    }
    if word.contains('.') {
        // Initialism ("U.S", "e.g", "p.m") — but not a number ("3.14").
        return word.split('.').all(|p| {
            !p.is_empty() && p.chars().count() <= 2 && p.chars().all(char::is_alphabetic)
        });
    }
    if word.chars().count() == 1 && word.chars().all(char::is_alphabetic) {
        return true; // initial: "J."
    }
    ABBREVIATIONS.contains(&word.to_lowercase().as_str())
}

/// A clause-break cut for an over-long buffer with no sentence end.
fn overflow_cut(buf: &str) -> Option<usize> {
    if buf.chars().count() <= MAX_SENTENCE_CHARS {
        return None;
    }
    let limit = buf
        .char_indices()
        .nth(MAX_SENTENCE_CHARS)
        .map(|c| c.0)
        .unwrap_or(buf.len());
    let head = &buf[..limit];
    for pat in [", ", "; ", ": ", " - ", " "] {
        if let Some(pos) = head.rfind(pat) {
            if pos > 0 {
                return Some(pos + pat.trim_end().len());
            }
        }
    }
    Some(limit)
}

/// Strip markdown that a TTS would read aloud; `None` if nothing speakable is left.
fn clean_for_speech(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let line = line.trim();
        let line = line.trim_start_matches('#').trim_start();
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line);
        if !out.is_empty() && !line.is_empty() {
            out.push(' ');
        }
        out.extend(line.chars().filter(|c| !matches!(c, '*' | '`')));
    }
    let out = out.trim().to_string();
    if out.chars().any(char::is_alphanumeric) {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_sentences() {
        assert_eq!(
            split_all("Hello there. How are you? Great!"),
            vec!["Hello there.", "How are you?", "Great!"]
        );
    }

    #[test]
    fn keeps_abbreviations_decimals_and_initials() {
        assert_eq!(
            split_all("Dr. Smith paid $3.50 in the U.S. today. Then J. Doe left, e.g. at noon."),
            vec![
                "Dr. Smith paid $3.50 in the U.S. today.",
                "Then J. Doe left, e.g. at noon."
            ]
        );
        assert_eq!(
            split_all("It is approx. five miles. Go."),
            vec!["It is approx. five miles.", "Go."]
        );
    }

    #[test]
    fn handles_ellipses() {
        assert_eq!(
            split_all("Well... maybe. Wait... What?"),
            vec!["Well... maybe.", "Wait...", "What?"]
        );
        assert_eq!(split_all("Hmm… Okay then."), vec!["Hmm…", "Okay then."]);
    }

    #[test]
    fn streams_token_by_token() {
        let mut s = SentenceSplitter::new();
        let mut got = Vec::new();
        for tok in [
            "The value",
            " is 3",
            ".",
            "14",
            ". Next",
            " one",
            " is Dr",
            ". Who",
            "? ",
            "Done",
        ] {
            got.extend(s.push(tok));
        }
        // "3." must not split before "14" arrives; "Dr." must not split.
        assert_eq!(got, vec!["The value is 3.14.", "Next one is Dr. Who?"]);
        assert_eq!(s.flush().as_deref(), Some("Done"));
    }

    #[test]
    fn waits_for_lookahead_then_emits() {
        let mut s = SentenceSplitter::new();
        assert!(s.push("First sentence.").is_empty());
        assert!(s.push(" ").is_empty());
        assert_eq!(s.push("Second"), vec!["First sentence."]);
    }

    #[test]
    fn quotes_newlines_and_markdown() {
        assert_eq!(
            split_all("He said \"stop.\" Then left.\n## Steps\n- **Open** the `app`"),
            vec!["He said \"stop.\"", "Then left.", "Steps", "Open the app"]
        );
    }

    #[test]
    fn cuts_run_on_text() {
        let long = "word, ".repeat(80);
        let parts = split_all(&long);
        assert!(parts.len() >= 2);
        assert!(parts
            .iter()
            .all(|p| p.chars().count() <= MAX_SENTENCE_CHARS));
    }
}
