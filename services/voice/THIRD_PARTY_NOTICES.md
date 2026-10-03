# Third-Party Notices: Voice Service

All inference runs on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).
Model packs download on first use from the k2-fsa sherpa-onnx release assets
(configurable via `ALLTERNIT_VOICE_MODEL_BASE`); their sha256 hashes are
pinned and verified in `src/models.rs`. No model file ships in the installer.

## Native libraries in `allternit-voice-service`

`sherpa-onnx-sys` (1.13.8) links these static libraries; the linker keeps only
what speech-to-text uses:

| Component | Licence | Role |
|---|---|---|
| sherpa-onnx (`sherpa-onnx-c-api`, `sherpa-onnx-core`, `sherpa-onnx-fst*`, `sherpa-onnx-kaldifst-core`) and the `sherpa-onnx` / `sherpa-onnx-sys` crates | Apache-2.0 | Speech engine: VAD, offline ASR, resampler |
| onnxruntime | MIT | Neural network inference runtime |
| kaldi-native-fbank | Apache-2.0 | Audio features |
| kaldi-decoder | Apache-2.0 | Decoding |
| OpenFst (vendored in sherpa-onnx as `sherpa-onnx-fst`) | Apache-2.0 | FSTs |
| kissfft | BSD-3-Clause | FFT |
| sentencepiece (`ssentencepiece_core`) | Apache-2.0 | Tokenisation |
| piper-phonemize (phoneme map tables only) | MIT | Pulled in by sherpa-onnx's lexicon code |
| bzip2 (via `bzip2-sys`, static) | bzip2 licence (BSD-style) | Unpacking model archives |

**No GPL code.** espeak-ng (GPL-3.0-or-later) is kept out of this binary: TTS
runs in the separate program `allternit-tts` (`services/voice-tts`,
GPL-3.0-or-later, its own `LICENSE` and notices), which this service starts as
a child process and talks to over a pipe. `src/main.rs` (`no_espeak`) stops
the linker from pulling espeak-ng in, and `scripts/check-voice-no-gpl.sh`
verifies every release binary.

Rust crate dependencies (axum, tokio, reqwest, serde, sha2, tar, …) are
MIT/Apache-2.0; see `Cargo.lock` for the full list.

## Models

| Model | Pack | Upstream | Licence | Attribution required |
|---|---|---|---|---|
| Silero VAD (`silero_vad.onnx`) | small | https://github.com/snakers4/silero-vad | MIT | No |
| Moonshine tiny EN, quantized (sherpa-onnx export `sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27`) | small | https://github.com/moonshine-ai/moonshine | MIT (English models) | No |
| Kokoro-82M v1.0, fp32 (sherpa-onnx export `kokoro-multi-lang-v1_0`: `model.onnx`, `voices.bin`, lexicons), run by `allternit-tts` | tts | https://huggingface.co/hexgrad/Kokoro-82M | Apache-2.0 (its bundled `espeak-ng-data` is GPL-3.0-or-later) | No |
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

`GET /v1/voices` serves the 28 English voices of Kokoro v1.0, speaker ids
0–27 in the order of the model's `speaker_names` metadata: `af_alloy`,
`af_aoede`, `af_bella`, `af_heart`, `af_jessica`, `af_kore`, `af_nicole`,
`af_nova`, `af_river`, `af_sarah`, `af_sky`, `am_adam`, `am_echo`, `am_eric`,
`am_fenrir`, `am_liam`, `am_michael`, `am_onyx`, `am_puck`, `am_santa`,
`bf_alice`, `bf_emma`, `bf_isabella`, `bf_lily`, `bm_daniel`, `bm_fable`,
`bm_george`, `bm_lewis`. These are the model's stock voices, not clones of
any person.
