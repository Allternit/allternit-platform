# Third-Party Notices: Voice Service

All inference runs on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).
Model packs download on first use from the k2-fsa sherpa-onnx release assets
(configurable via `ALLTERNIT_VOICE_MODEL_BASE`); their sha256 hashes are
pinned and verified in `src/models.rs`. No model file ships in the installer.

## Native libraries linked into the binary

The `sherpa-onnx-sys` crate links these static libraries (list from its
`build.rs`, version 1.13.8) into `allternit-voice-service`:

| Component | Licence | Role |
|---|---|---|
| sherpa-onnx (`sherpa-onnx-c-api`, `sherpa-onnx-core`, `sherpa-onnx-fst*`, `sherpa-onnx-kaldifst-core`) and the `sherpa-onnx` / `sherpa-onnx-sys` crates | Apache-2.0 | Speech engine: VAD, offline ASR, TTS, resampler |
| onnxruntime | MIT | Neural network inference runtime |
| kaldi-native-fbank | Apache-2.0 | Audio features |
| kaldi-decoder | Apache-2.0 | Decoding |
| OpenFst (vendored in sherpa-onnx as `sherpa-onnx-fst`) | Apache-2.0 | Text normalisation FSTs |
| kissfft | BSD-3-Clause | FFT |
| sentencepiece (`ssentencepiece_core`) | Apache-2.0 | Tokenisation |
| piper-phonemize | MIT | Phonemiser glue for Kokoro |
| **espeak-ng** (+ its `ucd` library) | **GPL-3.0-or-later** | Grapheme-to-phoneme for Kokoro TTS. Its `espeak-ng-data` ships inside the Kokoro model archive. |
| bzip2 (via `bzip2-sys`, static) | bzip2 licence (BSD-style) | Unpacking model archives |

**espeak-ng is GPL-3.0-or-later and is statically linked into this
binary.** Distributing `allternit-voice-service` (for example inside
Allternit Desktop) therefore carries GPL-3.0 obligations for this binary,
including offering its corresponding source. The Desktop app and other
sidecars talk to it over HTTP as a separate program. This is an open
licensing decision recorded in `docs/VOICE_ENGINE_PHASE_1_NOTES.md`.

Rust crate dependencies (axum, tokio, reqwest, serde, sha2, tar, …) are
MIT/Apache-2.0; see `Cargo.lock` for the full list.

## Models

| Model | Pack | Upstream | Licence | Attribution required |
|---|---|---|---|---|
| Silero VAD (`silero_vad.onnx`) | small | https://github.com/snakers4/silero-vad | MIT | No |
| Moonshine tiny EN, quantized (sherpa-onnx export `sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27`) | small | https://github.com/moonshine-ai/moonshine | MIT (English models) | No |
| Kokoro-82M v0.19, int8 EN (`kokoro-int8-en-v0_19`: `model.int8.onnx`, `voices.bin`) | small | https://huggingface.co/hexgrad/Kokoro-82M | Apache-2.0 | No |
| Smart Turn v3.2 (`smart-turn-v3.2-cpu.onnx`, rev f766f81) | small | https://huggingface.co/pipecat-ai/smart-turn-v3 | BSD-2-Clause | Notice only (keep this line) |
| Parakeet TDT 0.6B v3, int8 (sherpa-onnx export `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8`) | accurate | https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3 | **CC-BY-4.0** | **Yes** |

### Parakeet attribution (CC-BY-4.0)

> Speech recognition by **Parakeet TDT 0.6B v3** by NVIDIA,
> https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3, licensed under
> [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/). Converted to
> ONNX and int8-quantised by the k2-fsa sherpa-onnx project.

Keep this notice in any documentation, settings screen, About screen or
marketing page that offers the "accurate" STT option, and with any
redistribution of the `accurate` pack.

### Kokoro voices

The 11 voices served by `GET /v1/voices` and their speaker ids follow the
sherpa-onnx `kokoro-en-v0_19` model card: sid 0–10 = `af`, `af_bella`,
`af_nicole`, `af_sarah`, `af_sky`, `am_adam`, `am_michael`, `bf_emma`,
`bf_isabella`, `bm_george`, `bm_lewis` (the order of `voices.bin`). These are
the stock voices of the model, not clones of any person.
