//! Wake word: "Gizzi, …" / "Hey Gizzi, …" in a transcript segment.
//!
//! STT has no idea the name exists, so it writes what it hears: the spike
//! heard "Jizzy"; Moonshine and Parakeet also give "Gizzy", "Gizzie",
//! "Giz-ee", "Jizzie", "Gissy". A token counts as the name when, with
//! hyphens and apostrophes removed, it is `g`/`j` + `i` + one or two `s`/`z` +
//! a vowel ending (`y`, `i`, `ie`, `ee`, `ey`), or the split form "giz ee".
//! That keeps "gizmo", "jazzy", "dizzy", "is he" and "Jesse" out.
//!
//! Where it may stand (meetings talk *about* things all the time):
//! - first word of the segment, or first word after a pause the STT marked
//!   with punctuation (`,` `.` `?` `!` `;` `:` or a dash): "…to Jordan, Jizzy,
//!   please take note" triggers;
//! - anywhere after "hey" / "hi" / "okay" / "ok" / "yo": "so hey gizzi what's next";
//! - never as a possessive ("Gizzi's notes").
//!
//! The question is the text after the name. An empty question ("Gizzi?")
//! means the asker paused: the caller takes the next segment as the question.

/// What was heard and where.
#[derive(Debug, Clone, PartialEq)]
pub struct WakeHit {
    /// The words heard as the name, as written by STT ("Jizzy", "Hey Gizzi").
    pub heard: String,
    /// Everything after the name, trimmed of leading punctuation.
    pub question: String,
    /// 0..1: the exact spelling scores higher than a sound-alike; a "hey"
    /// prefix or a clause start adds to it.
    pub confidence: f32,
}

const GREETINGS: &[&str] = &["hey", "hi", "okay", "ok", "yo", "hello"];

/// A word with its byte span in the original text and whether a pause
/// (punctuation) came right before it.
#[derive(Debug)]
struct Word {
    norm: String,
    start: usize,
    end: usize,
    after_pause: bool,
    possessive: bool,
}

fn is_pause(c: char) -> bool {
    matches!(c, ',' | '.' | '?' | '!' | ';' | ':' | '—' | '–' | '…')
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut pause = true; // segment start counts as a pause
    let mut i = 0;
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    while i < bytes.len() {
        let (pos, c) = bytes[i];
        if c.is_alphanumeric() {
            let start = pos;
            let mut j = i;
            // A word: letters/digits plus inner hyphens and apostrophes.
            while j < bytes.len() {
                let (_, cj) = bytes[j];
                let inner = (cj == '-' || cj == '\'' || cj == '’')
                    && bytes.get(j + 1).is_some_and(|(_, n)| n.is_alphanumeric());
                if cj.is_alphanumeric() || inner {
                    j += 1;
                } else {
                    break;
                }
            }
            let end = bytes.get(j).map_or(text.len(), |(p, _)| *p);
            let raw = &text[start..end];
            let lower = raw.to_lowercase().replace('’', "'");
            let possessive = lower.ends_with("'s");
            let norm: String = lower
                .trim_end_matches("'s")
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            out.push(Word { norm, start, end, after_pause: pause, possessive });
            pause = false;
            i = j;
        } else {
            if is_pause(c) || (c == '-' && bytes.get(i + 1).is_some_and(|(_, n)| n.is_whitespace())) {
                pause = true;
            }
            i += 1;
        }
    }
    out
}

/// Is `w` (normalised, lowercase letters) the name? Returns the spelling score.
fn name_score(w: &str) -> Option<f32> {
    if w == "gizzi" {
        return Some(0.95);
    }
    let b = w.as_bytes();
    if b.len() < 4 || b.len() > 7 || !matches!(b[0], b'g' | b'j') || b[1] != b'i' {
        return None;
    }
    let mut k = 2;
    let mut sibilants = 0;
    while k < b.len() && matches!(b[k], b's' | b'z') {
        sibilants += 1;
        k += 1;
    }
    if !(1..=2).contains(&sibilants) {
        return None;
    }
    let ending = &w[k..];
    match ending {
        "y" | "i" | "ie" | "ee" | "ey" => {
            // "gizzy"/"gizzie"/"gizi" sound right and start right; j- and s- forms
            // are further from the spelling.
            let close = b[0] == b'g' && b[2] == b'z';
            Some(if close { 0.85 } else { 0.75 })
        }
        _ => None,
    }
}

/// Find the first wake word in a segment's text.
pub fn detect(text: &str) -> Option<WakeHit> {
    let ws = words(text);
    for (i, w) in ws.iter().enumerate() {
        if w.possessive {
            continue;
        }
        // Single-token name, or the split "giz ee" / "gizz e" / "jiz ee".
        let (score, last) = match name_score(&w.norm) {
            Some(s) => (s, i),
            None => match ws.get(i + 1) {
                Some(n) if !n.after_pause && !n.possessive && matches!(n.norm.as_str(), "ee" | "e" | "y" | "ey") => {
                    match name_score(&format!("{}{}", w.norm, n.norm)) {
                        Some(s) => (s - 0.05, i + 1),
                        None => continue,
                    }
                }
                _ => continue,
            },
        };
        // "Hey Gizzi" greets; "Hey, Gizzi" is a pause, so the name stands alone.
        let greeted = i > 0 && !w.after_pause && GREETINGS.contains(&ws[i - 1].norm.as_str());
        if !(w.after_pause || greeted) {
            continue;
        }
        let first = if greeted { i - 1 } else { i };
        let heard = text[ws[first].start..ws[last].end].to_string();
        let question = text[ws[last].end..]
            .trim_start_matches(|c: char| is_pause(c) || c.is_whitespace() || c == '-')
            .trim()
            .to_string();
        let mut confidence = score;
        if greeted {
            confidence += 0.04;
        }
        if ws[first].after_pause && first == 0 {
            confidence += 0.01;
        }
        return Some(WakeHit { heard, question, confidence: (confidence.min(0.99) * 100.0).round() / 100.0 });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(text: &str) -> WakeHit {
        detect(text).unwrap_or_else(|| panic!("no wake word in {text:?}"))
    }

    #[test]
    fn spike_sentence_triggers_after_the_comma() {
        let h = hit("Agreed, let's move the pricing decision to Thursday and assign it to Jordan, Jizzy, please take note of that action item.");
        assert_eq!(h.heard, "Jizzy");
        assert_eq!(h.question, "please take note of that action item.");
        assert!(h.confidence >= 0.7);
    }

    #[test]
    fn start_of_segment_and_greetings() {
        let h = hit("Gizzi, what did we decide about pricing?");
        assert_eq!((h.heard.as_str(), h.question.as_str()), ("Gizzi", "what did we decide about pricing?"));
        assert!(h.confidence >= 0.95);

        let h = hit("Hey Gizzi, summarize the last ten minutes.");
        assert_eq!(h.heard, "Hey Gizzi");
        assert_eq!(h.question, "summarize the last ten minutes.");

        // A greeting makes mid-clause OK.
        let h = hit("so hey gizzy what's next on the agenda");
        assert_eq!(h.heard, "hey gizzy");
        assert_eq!(h.question, "what's next on the agenda");
    }

    #[test]
    fn sound_alike_spellings() {
        for (text, heard) in [
            ("Gizzy, make a doc.", "Gizzy"),
            ("Gizzie, make a doc.", "Gizzie"),
            ("Giz-ee, make a doc.", "Giz-ee"),
            ("Giz ee, make a doc.", "Giz ee"),
            ("Jizzie, make a doc.", "Jizzie"),
            ("Gissy, make a doc.", "Gissy"),
            ("Okay Jizzy make a doc", "Okay Jizzy"),
        ] {
            let h = hit(text);
            assert_eq!(h.heard, heard, "{text}");
            assert_eq!(h.question.trim_end_matches('.'), "make a doc", "{text}");
        }
    }

    #[test]
    fn no_false_positives() {
        for text in [
            "The gizmo is broken again.",
            "Is he joining the call later?",
            "It is, he said, a jazzy design.",
            "I feel dizzy after that demo.",
            "Jesse, can you share your screen?",
            "We sent the notes to Jizzy yesterday.", // mid-clause, no pause, no greeting
            "Gizzi's notes were good.",              // possessive
            "Let's look at the gizzard numbers.",
            "",
        ] {
            assert_eq!(detect(text), None, "{text:?}");
        }
    }

    #[test]
    fn bare_name_leaves_the_question_for_the_next_segment() {
        let h = hit("Gizzi?");
        assert_eq!(h.question, "");
        let h = hit("Hey, Gizzi.");
        assert_eq!(h.heard, "Gizzi");
        assert_eq!(h.question, "");
    }
}
