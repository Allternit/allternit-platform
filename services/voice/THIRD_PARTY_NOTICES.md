# Third-Party Notices — Voice Service

All inference runs on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).
Model packs download on first use from the k2-fsa sherpa-onnx release assets
(configurable via `ALLTERNIT_VOICE_MODEL_BASE`). Hashes are pinned and
verified in `src/models.rs`.

## Libraries

| Component | Version | Licence | Notes |
|---|---|---|---|
| sherpa-onnx / sherpa-onnx-sys (crates.io, k2-fsa) | 1.13.x | Apache-2.0 | Speech engine: VAD, offline ASR, TTS. Native code + onnxruntime linked **statically** into the binary. |
| onnxruntime | (via sherpa-onnx-sys) | MIT | Inference runtime, statically linked. |
| espeak-ng (via sherpa-onnx) | (via sherpa-onnx-sys) | GPL-3.0 | Phonemiser used for Kokoro TTS; its code is compiled into the binary and its `espeak-ng-data` ships inside the Kokoro model pack. **Flag:** GPL-3.0 in the binary is incompatible with Apache-2.0 relicensing — sherpa-onnx upstream distributes this combination and uses espeak-ng only at runtime for TTS phonemisation. Tracked for a replacement phonemiser in a later phase. |

## Models

| Model | Pack | Upstream | Licence | Attribution required |
|---|---|---|---|---|
| Silero VAD (`silero_vad.onnx`) | small | https://github.com/snakers4/silero-vad | MIT | No |
| Moonshine tiny EN (quantized, 2026-02-27 export) | small | https://github.com/moonshine-ai/moonshine (sherpa-onnx export) | MIT ("The English-language models are also released under the MIT License") | No |
| Kokoro-82M int8 EN v0.19 (`model.int8.onnx`, `voices.bin`) | small | https://huggingface.co/hexgrad/Kokoro-82M (kLegacy v0.19) | Apache-2.0 | No |
| Parakeet TDT 0.6B v3 int8 | accurate | https://github.com/NVIDIA/NeMo (sherpa-onnx export) | **CC-BY-4.0** | **Yes** |

### Parakeet attribution (CC-BY-4.0)

> Speech recognition by **Parakeet TDT 0.6B v3** (NVIDIA NeMo),
> https://github.com/NVIDIA/NeMo — model licensed under
> [CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/).

Keep this notice in any documentation, marketing page, or About screen that
describes the "accurate" STT option, and in redistribution of the
`accurate` pack.

### Kokoro training-data attribution

Kokoro v1.0 training data includes CC-BY audio (Koniwa `tnc`, SIWIS) per the
[Kokoro-82M model card](https://huggingface.co/hexgrad/Kokoro-82M). The
en-v0.19 pack we ship predates that dataset addition; the notice is recorded
here for completeness.

### Moonshine note

The quantized export used here (sherpa-onnx `sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27`)
is published by k2-fsa under the upstream Moonshine licence (MIT for
English models).

## Voice names

The 11 voice names served by `GET /v1/voices` (`af`, `af_bella`, …) come
from the upstream Kokoro v0.19 release
(https://huggingface.co/hexgrad/kLegacy, folder `v0.19/voices/`), in
alphabetical order matching `voices.bin`.
