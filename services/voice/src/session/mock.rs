//! Scripted engines for tests and the protocol examples. They need no
//! models: the VAD is an energy gate, STT returns scripted text, TTS
//! returns a tone whose length follows the text.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::engine::{
    EngineError, EngineFactory, EngineInfo, StreamingStt, SttOptions, Tts, TurnDetector, Vad,
    VadEvent, ENGINE_SAMPLE_RATE,
};
use super::protocol::EngineKind;

/// Knobs for [`MockEngine`].
#[derive(Debug, Clone)]
pub struct MockConfig {
    /// Final transcripts handed out one per speech segment, in order.
    pub transcripts: Vec<String>,
    /// Smart Turn probability; `None` means no turn model.
    pub turn_probability: Option<f32>,
    /// RMS above which a frame counts as speech.
    pub vad_threshold: f32,
    /// Quiet audio needed before the VAD reports speech stop.
    pub vad_hangover_ms: u32,
    pub tts_sample_rate: u32,
    /// Speech length per character of text.
    pub tts_ms_per_char: u32,
    /// TTS chunk size and the compute time to simulate per chunk.
    pub tts_chunk_ms: u32,
    pub tts_delay_per_chunk: Duration,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            transcripts: Vec::new(),
            turn_probability: Some(0.9),
            vad_threshold: 0.02,
            vad_hangover_ms: 100,
            tts_sample_rate: 24_000,
            tts_ms_per_char: 20,
            tts_chunk_ms: 100,
            tts_delay_per_chunk: Duration::ZERO,
        }
    }
}

/// Factory for mock engines. `spoken()` records every sentence synthesized.
#[derive(Clone)]
pub struct MockEngine {
    pub config: MockConfig,
    transcripts: Arc<Mutex<VecDeque<String>>>,
    spoken: Arc<Mutex<Vec<String>>>,
}

impl MockEngine {
    pub fn new(config: MockConfig) -> Self {
        Self {
            transcripts: Arc::new(Mutex::new(config.transcripts.iter().cloned().collect())),
            spoken: Arc::default(),
            config,
        }
    }

    /// Sentences passed to TTS so far.
    pub fn spoken(&self) -> Vec<String> {
        self.spoken.lock().expect("spoken lock").clone()
    }
}

impl EngineFactory for MockEngine {
    fn info(&self, stt: &SttOptions) -> EngineInfo {
        EngineInfo {
            kind: EngineKind::Device,
            stt_model: format!("mock-stt-{}", stt.model),
            tts_model: "mock-tts".into(),
            vad_model: "mock-vad".into(),
            turn_model: "mock-turn".into(),
            voices: vec!["mock".into(), "mock-2".into()],
            default_voice: "mock".into(),
        }
    }

    fn stt(&self, _: &SttOptions) -> Result<Box<dyn StreamingStt>, EngineError> {
        Ok(Box::new(MockStt {
            script: self.transcripts.clone(),
            heard: 0,
            interims: 0,
        }))
    }

    fn tts(&self) -> Result<Box<dyn Tts>, EngineError> {
        Ok(Box::new(MockTts {
            config: self.config.clone(),
            spoken: self.spoken.clone(),
        }))
    }

    fn vad(&self) -> Result<Box<dyn Vad>, EngineError> {
        Ok(Box::new(MockVad {
            threshold: self.config.vad_threshold,
            hangover: (self.config.vad_hangover_ms as usize) * ENGINE_SAMPLE_RATE as usize / 1000,
            speaking: false,
            quiet: 0,
        }))
    }

    fn turn_detector(&self) -> Result<Option<Box<dyn TurnDetector>>, EngineError> {
        Ok(self
            .config
            .turn_probability
            .map(|p| Box::new(MockTurn(p)) as Box<dyn TurnDetector>))
    }
}

pub struct MockVad {
    threshold: f32,
    hangover: usize,
    speaking: bool,
    quiet: usize,
}

impl Vad for MockVad {
    fn accept(&mut self, frame: &[f32]) -> Option<VadEvent> {
        if frame.is_empty() {
            return None;
        }
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        if rms >= self.threshold {
            self.quiet = 0;
            if !self.speaking {
                self.speaking = true;
                return Some(VadEvent::SpeechStart);
            }
        } else if self.speaking {
            self.quiet += frame.len();
            if self.quiet >= self.hangover {
                self.speaking = false;
                return Some(VadEvent::SpeechStop);
            }
        }
        None
    }

    fn reset(&mut self) {
        self.speaking = false;
        self.quiet = 0;
    }
}

pub struct MockStt {
    script: Arc<Mutex<VecDeque<String>>>,
    heard: usize,
    interims: usize,
}

impl StreamingStt for MockStt {
    fn begin(&mut self) {
        self.heard = 0;
        self.interims = 0;
    }

    fn accept(&mut self, samples: &[f32]) -> Result<Option<String>, EngineError> {
        self.heard += samples.len();
        // One interim per 200 ms heard: the first word of the upcoming final.
        let due = self.heard / (ENGINE_SAMPLE_RATE as usize / 5);
        if due > self.interims {
            self.interims = due;
            let next = self.script.lock().expect("script lock").front().cloned();
            return Ok(next.and_then(|t| t.split_whitespace().next().map(str::to_string)));
        }
        Ok(None)
    }

    fn finish(&mut self) -> Result<String, EngineError> {
        Ok(self
            .script
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_default())
    }

    fn abort(&mut self) {
        self.heard = 0;
    }
}

pub struct MockTts {
    config: MockConfig,
    spoken: Arc<Mutex<Vec<String>>>,
}

impl Tts for MockTts {
    fn sample_rate(&self) -> u32 {
        self.config.tts_sample_rate
    }

    fn synthesize(
        &mut self,
        text: &str,
        _voice: &str,
        sink: &mut dyn FnMut(&[f32]) -> bool,
    ) -> Result<(), EngineError> {
        self.spoken
            .lock()
            .expect("spoken lock")
            .push(text.to_string());
        let rate = self.config.tts_sample_rate as usize;
        let total = text.chars().count() * self.config.tts_ms_per_char as usize * rate / 1000;
        let chunk = (self.config.tts_chunk_ms as usize * rate / 1000).max(1);
        let mut produced = 0;
        while produced < total {
            if !self.config.tts_delay_per_chunk.is_zero() {
                std::thread::sleep(self.config.tts_delay_per_chunk);
            }
            let n = chunk.min(total - produced);
            let samples: Vec<f32> = (produced..produced + n)
                .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / rate as f32).sin() * 0.3)
                .collect();
            produced += n;
            if !sink(&samples) {
                break;
            }
        }
        Ok(())
    }
}

pub struct MockTurn(pub f32);

impl TurnDetector for MockTurn {
    fn predict(&mut self, _audio: &[f32]) -> Result<f32, EngineError> {
        Ok(self.0)
    }
}
