//! Kyutai Pocket TTS (100M, CPU realtime) with zero-shot voice cloning, run
//! from the community ONNX export (`KevinAHM/pocket-tts-onnx`, CC-BY-4.0) on
//! the same onnxruntime sherpa-onnx links. No Python, no espeak: the text
//! goes through a SentencePiece tokenizer straight into the model, so this
//! engine lives in the main (non-GPL) voice service.
//!
//! Pipeline (ported from the export's reference `pocket_tts_onnx.py`):
//! 1. `mimi_encoder`: reference audio (24 kHz) → voice embeddings `[1,N,1024]`.
//! 2. `flow_lm_main` is run once with `bos ++ embeddings` as the text input.
//!    The resulting KV-cache state *is* the voice ([`VoiceState`]).
//! 3. Per sentence: `text_conditioner` → `flow_lm_main` prefill → an
//!    autoregressive loop (`flow_lm_main` → `flow_lm_flow` Euler steps) that
//!    emits one 32-d latent per 80 ms, stopping on the EOS logit.
//! 4. `mimi_decoder` (streaming, stateful) turns latents into 24 kHz audio.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Mutex;

use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};
use serde::Deserialize;

use super::sentencepiece::SentencePiece;
use crate::session::engine::EngineError;

pub const POCKET_SAMPLE_RATE: u32 = 24_000;
/// Longest reference clip the encoder will take (the flow LM's KV cache holds
/// 1000 positions: voice + text + generated frames must fit).
pub const MAX_REF_SECONDS: f32 = 24.0;
pub const MIN_REF_SECONDS: f32 = 3.0;

pub const BUNDLE_FILE: &str = "bundle.json";
pub const TOKENIZER_FILE: &str = "tokenizer.model";
pub const BOS_FILE: &str = "bos_before_voice.npy";
pub const MIMI_ENCODER: &str = "mimi_encoder.onnx";
pub const TEXT_CONDITIONER: &str = "text_conditioner.onnx";
pub const FLOW_MAIN: &str = "flow_lm_main_int8.onnx";
pub const FLOW_FLOW: &str = "flow_lm_flow_int8.onnx";
pub const MIMI_DECODER: &str = "mimi_decoder_int8.onnx";

/// Sampling temperature (the export's default).
const TEMPERATURE: f32 = 0.7;
/// EOS logit above which the sentence is ending.
const EOS_THRESHOLD: f32 = -4.0;
/// Frames decoded for the first audio, then per later piece.
const FIRST_DECODE_FRAMES: usize = 2;
const DECODE_FRAMES: usize = 4;

fn fail(what: &str, e: impl std::fmt::Display) -> EngineError {
    EngineError::failed(format!("pocket tts {what}: {e}"))
}

#[derive(Debug, Deserialize)]
struct StateEntry {
    dtype: String,
    fill: String,
    input_name: String,
    output_name: String,
    shape: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct Bundle {
    conditioning_dim: usize,
    latent_dim: usize,
    frame_rate: f32,
    sample_rate: u32,
    #[serde(default = "default_max_tokens")]
    max_token_per_chunk: usize,
    flow_lm_state_manifest: Vec<StateEntry>,
    mimi_state_manifest: Vec<StateEntry>,
}

fn default_max_tokens() -> usize {
    50
}

/// A tensor in host memory (a model state we can clone and re-upload).
#[derive(Debug, Clone)]
enum Host {
    F32(Vec<usize>, Vec<f32>),
    I64(Vec<usize>, Vec<i64>),
    Bool(Vec<usize>, Vec<bool>),
}

impl Host {
    fn initial(e: &StateEntry) -> Result<Host, EngineError> {
        let shape: Vec<usize> = e.shape.iter().map(|d| (*d).max(0) as usize).collect();
        let n: usize = shape.iter().product();
        Ok(match e.dtype.as_str() {
            "float32" => Host::F32(
                shape,
                vec![if e.fill == "nan" { f32::NAN } else { 0.0 }; n],
            ),
            "int64" => Host::I64(shape, vec![0; n]),
            "bool" => Host::Bool(shape, vec![e.fill == "ones"; n]),
            other => return Err(fail("state", format!("unsupported dtype {other}"))),
        })
    }

    fn to_value(&self) -> Result<DynValue, EngineError> {
        Ok(match self {
            Host::F32(s, d) => Tensor::from_array((s.clone(), d.clone())).map_err(|e| fail("tensor", e))?.into_dyn(),
            Host::I64(s, d) => Tensor::from_array((s.clone(), d.clone())).map_err(|e| fail("tensor", e))?.into_dyn(),
            Host::Bool(s, d) => Tensor::from_array((s.clone(), d.clone())).map_err(|e| fail("tensor", e))?.into_dyn(),
        })
    }

    fn from_value(v: &DynValue, dtype: &str) -> Result<Host, EngineError> {
        let dims = |s: &ort::value::Shape| s.iter().map(|d| (*d).max(0) as usize).collect::<Vec<_>>();
        Ok(match dtype {
            "float32" => {
                let (s, d) = v.try_extract_tensor::<f32>().map_err(|e| fail("state", e))?;
                Host::F32(dims(s), d.to_vec())
            }
            "int64" => {
                let (s, d) = v.try_extract_tensor::<i64>().map_err(|e| fail("state", e))?;
                Host::I64(dims(s), d.to_vec())
            }
            "bool" => {
                let (s, d) = v.try_extract_tensor::<bool>().map_err(|e| fail("state", e))?;
                Host::Bool(dims(s), d.to_vec())
            }
            other => return Err(fail("state", format!("unsupported dtype {other}"))),
        })
    }
}

/// Reference-clip embeddings, `[1, frames, 1024]` row-major.
#[derive(Debug, Clone)]
pub struct VoiceEmbedding {
    pub frames: usize,
    data: Vec<f32>,
}

impl VoiceEmbedding {
    #[cfg(test)]
    pub fn for_test() -> Self {
        Self { frames: 1, data: vec![0.0; 1024] }
    }
}

/// A voice ready to speak: the flow LM state after reading the embeddings.
#[derive(Debug, Clone)]
pub struct VoiceState(Vec<Host>);

struct Models {
    encoder: Session,
    text: Session,
    main: Session,
    flow: Session,
    decoder: Session,
    main_out: Vec<String>,
    rng: u64,
}

pub struct PocketEngine {
    models: Mutex<Models>,
    tokenizer: SentencePiece,
    bundle: Bundle,
    bos: Vec<f32>,
}

/// Parse a float32 `.npy` (v1/v2) body.
fn read_npy_f32(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err("not an npy file".into());
    }
    let header_len = match bytes[6] {
        1 => u16::from_le_bytes([bytes[8], bytes[9]]) as usize + 10,
        _ => u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize + 12,
    };
    let header = std::str::from_utf8(bytes.get(..header_len).ok_or("short npy")?).map_err(|e| e.to_string())?;
    if !header.contains("'<f4'") || header.contains("True") {
        return Err("npy must be little-endian float32, C order".into());
    }
    Ok(bytes[header_len..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

impl PocketEngine {
    pub fn load(dir: &Path, threads: usize) -> Result<Self, EngineError> {
        crate::session::turn::ensure_ort_api()?;
        let read = |name: &str| {
            std::fs::read(dir.join(name))
                .map_err(|e| EngineError::unavailable(format!("custom voice pack file {name}: {e}")))
        };
        let bundle: Bundle = serde_json::from_slice(&read(BUNDLE_FILE)?).map_err(|e| fail("bundle.json", e))?;
        if bundle.sample_rate != POCKET_SAMPLE_RATE {
            return Err(fail("bundle", format!("unexpected sample rate {}", bundle.sample_rate)));
        }
        let tokenizer = SentencePiece::from_model_bytes(&read(TOKENIZER_FILE)?).map_err(|e| fail("tokenizer", e))?;
        let bos = read_npy_f32(&read(BOS_FILE)?).map_err(|e| fail("bos", e))?;
        if bos.len() != bundle.conditioning_dim {
            return Err(fail("bos", "unexpected size"));
        }
        let load = |name: &str| -> Result<Session, EngineError> {
            Session::builder()
                .map_err(|e| fail(name, e))?
                .with_intra_threads(threads.clamp(1, 4))
                .map_err(|e| fail(name, e))?
                .with_inter_threads(1)
                .map_err(|e| fail(name, e))?
                .commit_from_file(dir.join(name))
                .map_err(|e| fail(name, e))
        };
        let main = load(FLOW_MAIN)?;
        let main_out = main.outputs().iter().map(|o| o.name().to_string()).collect();
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        Ok(Self {
            models: Mutex::new(Models {
                encoder: load(MIMI_ENCODER)?,
                text: load(TEXT_CONDITIONER)?,
                main,
                flow: load(FLOW_FLOW)?,
                decoder: load(MIMI_DECODER)?,
                main_out,
                rng: seed,
            }),
            tokenizer,
            bundle,
            bos,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.bundle.sample_rate
    }

    /// Encode a consented reference clip (24 kHz mono) into voice embeddings.
    pub fn embed_clip(&self, pcm24k: &[f32]) -> Result<VoiceEmbedding, EngineError> {
        let secs = pcm24k.len() as f32 / POCKET_SAMPLE_RATE as f32;
        if !(MIN_REF_SECONDS..=MAX_REF_SECONDS + 0.5).contains(&secs) {
            return Err(fail(
                "reference clip",
                format!("must be {MIN_REF_SECONDS}-{MAX_REF_SECONDS} s, got {secs:.1} s"),
            ));
        }
        let mut m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        let input = Tensor::from_array(([1usize, 1, pcm24k.len()], pcm24k.to_vec())).map_err(|e| fail("encoder input", e))?;
        let out = m.encoder.run(vec![(Cow::Borrowed("audio"), SessionInputValue::from(input))]).map_err(|e| fail("encoder", e))?;
        let (_, data) = out[0].try_extract_tensor::<f32>().map_err(|e| fail("encoder output", e))?;
        let dim = self.bundle.conditioning_dim;
        if data.is_empty() || data.len() % dim != 0 {
            return Err(fail("encoder", "unexpected embedding size"));
        }
        Ok(VoiceEmbedding { frames: data.len() / dim, data: data.to_vec() })
    }

    fn init_states(manifest: &[StateEntry]) -> Result<Vec<DynValue>, EngineError> {
        manifest.iter().map(|e| Host::initial(e)?.to_value()).collect()
    }

    /// Run the flow LM once over `text_embeddings` (`[1, n, 1024]`) and
    /// `sequence` (`[1, k, 32]`), updating `states`; returns outputs 0 and 1.
    fn run_main(
        &self,
        m: &mut Models,
        sequence: (usize, &[f32]),
        text_embeddings: (usize, &[f32]),
        states: &mut [DynValue],
    ) -> Result<(Vec<f32>, f32), EngineError> {
        let seq = Tensor::from_array(([1usize, sequence.0, self.bundle.latent_dim], sequence.1.to_vec())).map_err(|e| fail("seq", e))?;
        let txt = Tensor::from_array(([1usize, text_embeddings.0, self.bundle.conditioning_dim], text_embeddings.1.to_vec())).map_err(|e| fail("text", e))?;
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![
            (Cow::Borrowed("sequence"), SessionInputValue::from(seq)),
            (Cow::Borrowed("text_embeddings"), SessionInputValue::from(txt)),
        ];
        for (e, s) in self.bundle.flow_lm_state_manifest.iter().zip(states.iter()) {
            inputs.push((Cow::Borrowed(e.input_name.as_str()), SessionInputValue::from(s)));
        }
        let mut out = m.main.run(inputs).map_err(|e| fail("flow_lm_main", e))?;
        let cond = out[m.main_out[0].as_str()].try_extract_tensor::<f32>().map_err(|e| fail("conditioning", e))?.1.to_vec();
        let eos = out[m.main_out[1].as_str()].try_extract_tensor::<f32>().map_err(|e| fail("eos", e))?.1.first().copied().unwrap_or(f32::MIN);
        for (e, slot) in self.bundle.flow_lm_state_manifest.iter().zip(states.iter_mut()) {
            *slot = out.remove(e.output_name.as_str()).ok_or_else(|| fail("flow_lm_main", "missing state output"))?;
        }
        Ok((cond, eos))
    }

    /// Condition the flow LM on a voice. The result is the voice; keep it for
    /// the session and reuse it for every sentence.
    pub fn condition(&self, emb: &VoiceEmbedding) -> Result<VoiceState, EngineError> {
        let mut m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        let mut states = Self::init_states(&self.bundle.flow_lm_state_manifest)?;
        let mut text = self.bos.clone();
        text.extend_from_slice(&emb.data);
        self.run_main(&mut m, (0, &[]), (emb.frames + 1, &text), &mut states)?;
        let host = self
            .bundle
            .flow_lm_state_manifest
            .iter()
            .zip(&states)
            .map(|(e, v)| Host::from_value(v, &e.dtype))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VoiceState(host))
    }

    /// Split into model-sized chunks: sentence by sentence, long ones at commas.
    fn chunks(&self, text: &str) -> Vec<String> {
        let max = self.bundle.max_token_per_chunk;
        let mut pieces: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in text.chars() {
            cur.push(ch);
            if matches!(ch, '.' | '!' | '?') {
                pieces.push(std::mem::take(&mut cur));
            }
        }
        if !cur.trim().is_empty() {
            pieces.push(cur);
        }
        let mut refined = Vec::new();
        for p in pieces {
            if self.tokenizer.encode(p.trim()).len() <= max {
                refined.push(p);
                continue;
            }
            let mut sub = String::new();
            for ch in p.chars() {
                sub.push(ch);
                if matches!(ch, ',' | ';' | ':') {
                    refined.push(std::mem::take(&mut sub));
                }
            }
            if !sub.trim().is_empty() {
                refined.push(sub);
            }
        }
        let (mut out, mut cur, mut count) = (Vec::<String>::new(), String::new(), 0usize);
        for p in refined {
            let c = self.tokenizer.encode(p.trim()).len();
            if !cur.is_empty() && count + c > max {
                out.push(cur.trim().to_string());
                cur.clear();
                count = 0;
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(p.trim());
            count += c;
        }
        if !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
        }
        out
    }

    /// Text normalisation the model was trained with.
    fn prepare(text: &str) -> Option<(String, usize)> {
        let t = text.trim().replace(['\n', '\r'], " ").replace("  ", " ");
        let mut chars = t.chars();
        let first = chars.next()?;
        let words = t.split_whitespace().count();
        let mut t = first.to_uppercase().collect::<String>() + chars.as_str();
        if t.chars().last().is_some_and(|c| c.is_alphanumeric()) {
            t.push('.');
        }
        Some((t, if words <= 4 { 3 } else { 1 }))
    }

    fn gaussian(rng: &mut u64, std: f32) -> f32 {
        let mut next = || {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            ((*rng >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0)
        };
        let (u1, u2) = (next(), next());
        ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()) as f32 * std
    }

    /// Speak `text` in the conditioned voice. `sink` gets 24 kHz mono f32 as
    /// it is decoded and returns `false` to stop (barge-in).
    pub fn synthesize(
        &self,
        voice: &VoiceState,
        text: &str,
        sink: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<(), EngineError> {
        let mut m = self.models.lock().unwrap_or_else(|e| e.into_inner());
        for chunk in self.chunks(text) {
            let Some((prepared, after_eos)) = Self::prepare(&chunk) else { continue };
            if !self.speak_chunk(&mut m, voice, &prepared, after_eos + 2, sink)? {
                return Ok(());
            }
        }
        Ok(())
    }

    fn decode(
        &self,
        m: &mut Models,
        latents: &[f32],
        frames: usize,
        states: &mut [DynValue],
    ) -> Result<Vec<f32>, EngineError> {
        let lat = Tensor::from_array(([1usize, frames, self.bundle.latent_dim], latents.to_vec())).map_err(|e| fail("latent", e))?;
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> =
            vec![(Cow::Borrowed("latent"), SessionInputValue::from(lat))];
        for (e, s) in self.bundle.mimi_state_manifest.iter().zip(states.iter()) {
            inputs.push((Cow::Borrowed(e.input_name.as_str()), SessionInputValue::from(s)));
        }
        let first_out = m.decoder.outputs()[0].name().to_string();
        let mut out = m.decoder.run(inputs).map_err(|e| fail("mimi_decoder", e))?;
        let audio = out[first_out.as_str()].try_extract_tensor::<f32>().map_err(|e| fail("audio", e))?.1.to_vec();
        for (e, slot) in self.bundle.mimi_state_manifest.iter().zip(states.iter_mut()) {
            *slot = out.remove(e.output_name.as_str()).ok_or_else(|| fail("mimi_decoder", "missing state output"))?;
        }
        Ok(audio)
    }

    /// One chunk of text → audio. `Ok(false)` when the sink asked to stop.
    fn speak_chunk(
        &self,
        m: &mut Models,
        voice: &VoiceState,
        text: &str,
        frames_after_eos: usize,
        sink: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<bool, EngineError> {
        let b = &self.bundle;
        let ids: Vec<i64> = self.tokenizer.encode(text).into_iter().map(i64::from).collect();
        if ids.is_empty() {
            return Ok(true);
        }
        let tok = Tensor::from_array(([1usize, ids.len()], ids.clone())).map_err(|e| fail("tokens", e))?;
        let text_emb = {
            let out = m.text.run(vec![(Cow::Borrowed("token_ids"), SessionInputValue::from(tok))]).map_err(|e| fail("text_conditioner", e))?;
            out[0].try_extract_tensor::<f32>().map_err(|e| fail("text embeddings", e))?.1.to_vec()
        };
        let mut states: Vec<DynValue> = voice.0.iter().map(Host::to_value).collect::<Result<_, _>>()?;
        self.run_main(m, (0, &[]), (text_emb.len() / b.conditioning_dim, &text_emb), &mut states)?;
        let mut dec_states = Self::init_states(&b.mimi_state_manifest)?;

        let max_frames = (((ids.len() as f32 / 3.0) + 2.0) * b.frame_rate).ceil() as usize;
        let mut curr = vec![f32::NAN; b.latent_dim];
        let (mut eos_step, mut pending, mut pending_frames, mut first) = (None::<usize>, Vec::<f32>::new(), 0usize, true);
        let std = TEMPERATURE.sqrt();
        for step in 0..max_frames {
            let (cond, eos) = self.run_main(m, (1, &curr), (0, &[]), &mut states)?;
            if eos > EOS_THRESHOLD && eos_step.is_none() {
                eos_step = Some(step);
            }
            if eos_step.is_some_and(|e| step >= e + frames_after_eos) {
                break;
            }
            // One Euler step of the flow (lsd_steps = 1): x += flow(c, 0, 1, x).
            let mut x: Vec<f32> = (0..b.latent_dim).map(|_| Self::gaussian(&mut m.rng, std)).collect();
            let c = Tensor::from_array(([1usize, cond.len()], cond)).map_err(|e| fail("c", e))?;
            let s = Tensor::from_array(([1usize, 1], vec![0.0f32])).map_err(|e| fail("s", e))?;
            let t = Tensor::from_array(([1usize, 1], vec![1.0f32])).map_err(|e| fail("t", e))?;
            let xt = Tensor::from_array(([1usize, b.latent_dim], x.clone())).map_err(|e| fail("x", e))?;
            let flow = {
                let out = m.flow.run(vec![
                    (Cow::Borrowed("c"), SessionInputValue::from(c)),
                    (Cow::Borrowed("s"), SessionInputValue::from(s)),
                    (Cow::Borrowed("t"), SessionInputValue::from(t)),
                    (Cow::Borrowed("x"), SessionInputValue::from(xt)),
                ]).map_err(|e| fail("flow_lm_flow", e))?;
                out[0].try_extract_tensor::<f32>().map_err(|e| fail("flow", e))?.1.to_vec()
            };
            for (xi, fi) in x.iter_mut().zip(&flow) {
                *xi += *fi;
            }
            pending.extend_from_slice(&x);
            pending_frames += 1;
            curr = x;
            let want = if first { FIRST_DECODE_FRAMES } else { DECODE_FRAMES };
            if pending_frames >= want {
                let audio = self.decode(m, &pending, pending_frames, &mut dec_states)?;
                pending.clear();
                pending_frames = 0;
                first = false;
                if !sink(&audio) {
                    return Ok(false);
                }
            }
        }
        if pending_frames > 0 {
            let audio = self.decode(m, &pending, pending_frames, &mut dec_states)?;
            if !sink(&audio) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
