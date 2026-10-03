//! A small SentencePiece **unigram** encoder (Viterbi), enough for Pocket
//! TTS's `tokenizer.model`: identity normaliser, dummy prefix, whitespace
//! escaped to `▁`, byte fallback. Written in Rust so the product runtime has
//! no C++ SentencePiece dependency. Unsupported model kinds (BPE, a
//! normalisation charsmap) are refused at load rather than mis-tokenised.

use std::collections::HashMap;

const SPACE: char = '\u{2581}';
const PIECE_NORMAL: u64 = 1;
const PIECE_USER_DEFINED: u64 = 4;
const PIECE_BYTE: u64 = 6;
/// SentencePiece scores an unknown character `min_score - 10`.
const UNK_PENALTY: f32 = 10.0;

#[derive(Debug)]
pub struct SentencePiece {
    pieces: Vec<String>,
    /// Matchable piece → (id, score).
    lookup: HashMap<String, (u32, f32)>,
    max_chars: usize,
    byte_ids: [Option<u32>; 256],
    unk_id: u32,
    unk_score: f32,
    add_dummy_prefix: bool,
}

fn varint(b: &[u8], i: &mut usize) -> Result<u64, String> {
    let (mut r, mut s) = (0u64, 0u32);
    loop {
        let c = *b.get(*i).ok_or("truncated tokenizer.model")?;
        *i += 1;
        r |= u64::from(c & 0x7f) << s;
        s += 7;
        if c & 0x80 == 0 {
            return Ok(r);
        }
        if s > 63 {
            return Err("bad varint in tokenizer.model".into());
        }
    }
}

enum Val<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn fields(b: &[u8]) -> Result<Vec<(u64, Val<'_>)>, String> {
    let (mut i, mut out) = (0usize, Vec::new());
    while i < b.len() {
        let key = varint(b, &mut i)?;
        let v = match key & 7 {
            0 => Val::Int(varint(b, &mut i)?),
            1 => {
                let s = b.get(i..i + 8).ok_or("truncated tokenizer.model")?;
                i += 8;
                Val::Bytes(s)
            }
            2 => {
                let l = varint(b, &mut i)? as usize;
                let s = b.get(i..i + l).ok_or("truncated tokenizer.model")?;
                i += l;
                Val::Bytes(s)
            }
            5 => {
                let s = b.get(i..i + 4).ok_or("truncated tokenizer.model")?;
                i += 4;
                Val::Bytes(s)
            }
            w => return Err(format!("unsupported wire type {w} in tokenizer.model")),
        };
        out.push((key >> 3, v));
    }
    Ok(out)
}

impl SentencePiece {
    pub fn from_model_bytes(bytes: &[u8]) -> Result<Self, String> {
        let (mut pieces, mut types, mut scores) = (Vec::new(), Vec::new(), Vec::new());
        let (mut model_type, mut add_dummy_prefix) = (1u64, true);
        for (f, v) in fields(bytes)? {
            match (f, v) {
                (1, Val::Bytes(p)) => {
                    let (mut text, mut score, mut ty) = (String::new(), 0f32, PIECE_NORMAL);
                    for (pf, pv) in fields(p)? {
                        match (pf, pv) {
                            (1, Val::Bytes(s)) => {
                                text = String::from_utf8(s.to_vec()).map_err(|e| e.to_string())?
                            }
                            (2, Val::Bytes(s)) if s.len() == 4 => {
                                score = f32::from_le_bytes([s[0], s[1], s[2], s[3]])
                            }
                            (3, Val::Int(t)) => ty = t,
                            _ => {}
                        }
                    }
                    pieces.push(text);
                    scores.push(score);
                    types.push(ty);
                }
                (2, Val::Bytes(t)) => {
                    for (tf, tv) in fields(t)? {
                        if let (3, Val::Int(m)) = (tf, tv) {
                            model_type = m;
                        }
                    }
                }
                (3, Val::Bytes(n)) => {
                    for (nf, nv) in fields(n)? {
                        match (nf, nv) {
                            (1, Val::Bytes(name)) if name != b"identity" => {
                                return Err(format!(
                                    "unsupported normaliser '{}' (only identity)",
                                    String::from_utf8_lossy(name)
                                ))
                            }
                            (2, Val::Bytes(map)) if !map.is_empty() => {
                                return Err("unsupported normalisation charsmap".into())
                            }
                            (3, Val::Int(v)) => add_dummy_prefix = v != 0,
                            (4, Val::Int(v)) if v != 0 => {
                                return Err("remove_extra_whitespaces is not supported".into())
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if model_type != 1 {
            return Err("only unigram SentencePiece models are supported".into());
        }
        if pieces.is_empty() {
            return Err("tokenizer.model has no pieces".into());
        }
        let mut lookup = HashMap::new();
        let mut byte_ids = [None; 256];
        let mut max_chars = 1;
        let mut min_score = f32::MAX;
        let mut unk_id = 0;
        for (id, ((p, s), t)) in pieces.iter().zip(&scores).zip(&types).enumerate() {
            match *t {
                PIECE_NORMAL | PIECE_USER_DEFINED => {
                    max_chars = max_chars.max(p.chars().count());
                    if *t == PIECE_NORMAL {
                        min_score = min_score.min(*s);
                    }
                    lookup.insert(p.clone(), (id as u32, *s));
                }
                PIECE_BYTE => {
                    if let Some(h) = p.strip_prefix("<0x").and_then(|h| h.strip_suffix('>')) {
                        if let Ok(b) = u8::from_str_radix(h, 16) {
                            byte_ids[b as usize] = Some(id as u32);
                        }
                    }
                }
                2 => unk_id = id as u32,
                _ => {}
            }
        }
        Ok(Self {
            pieces,
            lookup,
            max_chars,
            byte_ids,
            unk_id,
            unk_score: min_score - UNK_PENALTY,
            add_dummy_prefix,
        })
    }

    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// Token ids for `text` (no BOS/EOS).
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut norm = String::with_capacity(text.len() + 3);
        if self.add_dummy_prefix {
            norm.push(SPACE);
        }
        norm.extend(text.chars().map(|c| if c == ' ' { SPACE } else { c }));
        let chars: Vec<char> = norm.chars().collect();
        let n = chars.len();
        // best[i] = (score, start of last piece, id; None = unknown char).
        let mut best: Vec<(f32, usize, Option<u32>)> = vec![(f32::NEG_INFINITY, 0, None); n + 1];
        best[0].0 = 0.0;
        let mut buf = String::new();
        for i in 0..n {
            if best[i].0 == f32::NEG_INFINITY {
                continue;
            }
            let base = best[i].0;
            let mut single = false;
            buf.clear();
            for len in 1..=self.max_chars.min(n - i) {
                buf.push(chars[i + len - 1]);
                if let Some(&(id, score)) = self.lookup.get(buf.as_str()) {
                    if len == 1 {
                        single = true;
                    }
                    let s = base + score;
                    if s > best[i + len].0 {
                        best[i + len] = (s, i, Some(id));
                    }
                }
            }
            if !single {
                let s = base + self.unk_score;
                if s > best[i + 1].0 {
                    best[i + 1] = (s, i, None);
                }
            }
        }
        let mut out = Vec::new();
        let mut end = n;
        while end > 0 {
            let (_, start, id) = best[end];
            match id {
                Some(id) => out.push(id),
                None => {
                    let ch: String = chars[start..end].iter().collect();
                    let mut ids: Vec<u32> = ch
                        .bytes()
                        .map(|b| self.byte_ids[b as usize].unwrap_or(self.unk_id))
                        .collect();
                    ids.reverse();
                    out.extend(ids);
                }
            }
            end = start;
        }
        out.reverse();
        out
    }

    /// Text for `ids` (inverse of `encode` for normal pieces).
    pub fn decode(&self, ids: &[u32]) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            let Some(p) = self.pieces.get(id as usize) else { continue };
            if let Some(h) = p.strip_prefix("<0x").and_then(|h| h.strip_suffix('>')) {
                if let Ok(b) = u8::from_str_radix(h, 16) {
                    bytes.push(b);
                    continue;
                }
            }
            if p.starts_with('<') && p.ends_with('>') && p.len() <= 7 && !p.contains(SPACE) {
                continue; // control piece (<s>, </s>, <pad>, <unk>)
            }
            bytes.extend_from_slice(p.as_bytes());
        }
        let s = String::from_utf8_lossy(&bytes).replace(SPACE, " ");
        if self.add_dummy_prefix {
            s.strip_prefix(' ').unwrap_or(&s).to_string()
        } else {
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny unigram model protobuf by hand.
    fn model(pieces: &[(&str, f32, u64)]) -> Vec<u8> {
        fn put_varint(out: &mut Vec<u8>, mut v: u64) {
            while v >= 0x80 {
                out.push((v as u8 & 0x7f) | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        }
        let mut out = Vec::new();
        for (p, s, t) in pieces {
            let mut m = Vec::new();
            m.push(0x0a);
            put_varint(&mut m, p.len() as u64);
            m.extend_from_slice(p.as_bytes());
            m.push(0x15);
            m.extend_from_slice(&s.to_le_bytes());
            m.push(0x18);
            put_varint(&mut m, *t);
            out.push(0x0a);
            put_varint(&mut out, m.len() as u64);
            out.extend(m);
        }
        out
    }

    fn tiny() -> SentencePiece {
        let mut p = vec![("<unk>", 0.0, 2u64), ("<s>", 0.0, 3)];
        let bytes: Vec<String> = (0..=255u32).map(|b| format!("<0x{b:02X}>")).collect();
        let mut v: Vec<(String, f32, u64)> = p.drain(..).map(|(a, b, c)| (a.to_string(), b, c)).collect();
        v.extend(bytes.into_iter().map(|b| (b, 0.0, 6u64)));
        for (s, sc) in [
            ("\u{2581}", -3.0),
            ("\u{2581}hello", -2.0),
            ("\u{2581}he", -4.0),
            ("llo", -4.0),
            ("hello", -5.0),
            ("\u{2581}world", -2.5),
            ("w", -6.0),
            ("o", -6.0),
            ("r", -6.0),
            ("l", -6.0),
            ("d", -6.0),
            (".", -5.0),
        ] {
            v.push((s.to_string(), sc, 1));
        }
        let refs: Vec<(&str, f32, u64)> = v.iter().map(|(a, b, c)| (a.as_str(), *b, *c)).collect();
        SentencePiece::from_model_bytes(&model(&refs)).unwrap()
    }

    #[test]
    fn viterbi_prefers_the_best_scoring_segmentation() {
        let sp = tiny();
        let ids = sp.encode("hello world.");
        let text: Vec<&str> = ids.iter().map(|i| sp.pieces[*i as usize].as_str()).collect();
        assert_eq!(text, ["\u{2581}hello", "\u{2581}world", "."]);
        assert_eq!(sp.decode(&ids), "hello world.");
    }

    #[test]
    fn unknown_characters_fall_back_to_utf8_bytes() {
        let sp = tiny();
        let ids = sp.encode("é");
        // "▁" then the two UTF-8 bytes of é.
        assert_eq!(ids.len(), 3);
        assert_eq!(sp.decode(&ids), "é");
    }

    #[test]
    fn rejects_non_unigram_and_unsupported_normalisers() {
        // model_type = 2 (BPE) in trainer_spec (field 2, sub-field 3).
        let mut b = model(&[("<unk>", 0.0, 2)]);
        b.extend([0x12, 0x02, 0x18, 0x02]);
        assert!(SentencePiece::from_model_bytes(&b).unwrap_err().contains("unigram"));
        let mut b = model(&[("<unk>", 0.0, 2)]);
        b.extend([0x1a, 0x05, 0x0a, 0x03, b'n', b'f', b'c']);
        assert!(SentencePiece::from_model_bytes(&b).unwrap_err().contains("normaliser"));
    }
}
