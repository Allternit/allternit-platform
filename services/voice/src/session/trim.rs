//! Trim low-energy audio around a segment before the final decode. The
//! turn-taking VAD only closes a segment after ~0.2 s of silence, and Moonshine
//! tiny garbles words next to that tail.

use super::engine::ENGINE_SAMPLE_RATE;

/// Analysis frame: 10 ms.
const FRAME: usize = ENGINE_SAMPLE_RATE as usize / 100;
/// Silence kept on each side of the speech.
pub const PAD_SAMPLES: usize = ENGINE_SAMPLE_RATE as usize / 10;
/// A frame is speech when its RMS exceeds this fraction of the loud-frame RMS...
const REL_THRESHOLD: f32 = 0.1;
/// ...but never below this absolute floor.
const ABS_FLOOR: f32 = 0.005;

fn frame_rms(frame: &[f32]) -> f32 {
    (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt()
}

/// The slice of `samples` from `pad` before the first speech frame to `pad`
/// after the last one. All of `samples` when no frame is above the threshold
/// (nothing to anchor on) or nothing would be cut.
pub fn trim_silence(samples: &[f32], pad: usize) -> &[f32] {
    let rms: Vec<f32> = samples.chunks(FRAME).map(frame_rms).collect();
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let Some(&loud) = sorted.get(sorted.len() * 9 / 10).or(sorted.last()) else {
        return samples;
    };
    let thr = (loud * REL_THRESHOLD).max(ABS_FLOOR);
    let (Some(first), Some(last)) = (
        rms.iter().position(|&r| r > thr),
        rms.iter().rposition(|&r| r > thr),
    ) else {
        return samples;
    };
    let from = (first * FRAME).saturating_sub(pad);
    let to = ((last + 1) * FRAME + pad).min(samples.len());
    &samples[from..to]
}

/// Share of `a`'s words (lowercased, punctuation stripped) that also occur in `b`.
pub fn word_overlap(a: &str, b: &str) -> f32 {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric() || *c == '\'')
                    .collect::<String>()
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return if wa.is_empty() && wb.is_empty() { 1.0 } else { 0.0 };
    }
    let hit = wa.iter().filter(|w| wb.contains(w)).count();
    hit as f32 / wa.len().max(wb.len()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.3).sin()).collect()
    }

    #[test]
    fn trims_leading_and_trailing_silence_to_the_pad() {
        let sr = ENGINE_SAMPLE_RATE as usize;
        let mut v = vec![0.0; sr / 2];
        v.extend(tone(sr, 0.3));
        v.extend(vec![0.0; sr / 2]);
        let t = trim_silence(&v, PAD_SAMPLES);
        assert!(t.len() >= sr + 2 * PAD_SAMPLES - FRAME);
        assert!(t.len() <= sr + 2 * PAD_SAMPLES + FRAME);
    }

    #[test]
    fn handles_noise_floor() {
        let sr = ENGINE_SAMPLE_RATE as usize;
        let mut v = tone(sr / 5, 0.01);
        v.extend(tone(sr, 0.4));
        v.extend(tone(sr / 5, 0.01));
        let t = trim_silence(&v, PAD_SAMPLES);
        assert!(t.len() < v.len() - sr / 10);
        assert!(t.len() > sr);
    }

    #[test]
    fn all_silence_or_empty_is_unchanged() {
        assert_eq!(trim_silence(&[], PAD_SAMPLES).len(), 0);
        let v = vec![0.0; 8000];
        assert_eq!(trim_silence(&v, PAD_SAMPLES).len(), 8000);
    }

    #[test]
    fn speech_filling_the_buffer_is_unchanged() {
        let v = tone(16_000, 0.3);
        assert_eq!(trim_silence(&v, PAD_SAMPLES).len(), 16_000);
    }

    #[test]
    fn overlap() {
        assert!(word_overlap("What time is it in Tokyo?", "what time is it in tokyo") > 0.99);
        assert!(
            word_overlap("A time is at in Tokyo right now.", "What time is it in Tokyo right now?")
                < 0.8
        );
        assert_eq!(word_overlap("", ""), 1.0);
    }
}
