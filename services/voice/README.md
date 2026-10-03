# Voice Service

Local speech-to-text **and** text-to-speech sidecar for Gizzi Code and
Allternit Desktop. One Rust binary (`services/voice`, crate `voice-service`),
one engine: [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (Apache-2.0).
No Python, no whisper.cpp, no cloud APIs.

- **STT:** Silero VAD segments the audio; an offline recogniser transcribes
  each segment. Moonshine tiny (English) is the default; Parakeet TDT 0.6B v3
  int8 (CC-BY-4.0, attribution in THIRD_PARTY_NOTICES.md) is the accurate
  option. Any sample rate is accepted (resampled to 16 kHz); 8 kHz phone
  audio incl. G.711 μ-law WAV works.
- **TTS:** Kokoro-82M int8 English (v0.19, Apache-2.0), 11 voices, 24 kHz.
- **Models download on first use** into `~/.allternit/models/voice/<pack>/`
  (sha256-verified, resumable). Nothing model-related ships in the installer.
  Base URL override: `ALLTERNIT_VOICE_MODEL_BASE` (Phase 2: runtime.allternit.com).
- **Resource limits:** inference runs on `ALLTERNIT_VOICE_THREADS` threads
  (default **2**), CPU provider, designed for a ~5-year-old 8 GB laptop.

## Model packs

| Pack | Contents | Download |
|------|----------|----------|
| `small` (default) | Silero VAD + Moonshine tiny EN (quantized) + Kokoro-82M int8 EN | ~134 MB |
| `accurate` | Parakeet TDT 0.6B v3 int8 (EN) | ~487 MB |

`GET /v1/models` reports per-pack state:
`missing | downloading{pct} | ready | error`.

## Running

```bash
# From the repo root
cargo run -p voice-service

# Or
./services/voice/start.sh
```

Binds `127.0.0.1:${PORT:-8001}`.

```bash
cargo test -p voice-service                 # no network, no models required
ALLTERNIT_VOICE_MODEL_TESTS=1 cargo test -p voice-service   # + real model tests
cargo run --release -p voice-service --example voice_bench -- --data ~/.allternit/voice-bench
```

## Native dependencies

The `sherpa-onnx` crate links sherpa-onnx **and** onnxruntime **statically**
(crate default `static` feature; the sys crate downloads the upstream
per-platform archive at build time). There is no dylib/DLL to stage next to
the binary in Desktop packaging, and nothing macOS-specific in the build.

## API

```
GET  /health                 engine + pack states
GET  /v1/voices              real Kokoro voices
GET  /v1/stt/models          moonshine (small) + parakeet (accurate)
GET  /v1/models              pack download states
POST /v1/stt                 multipart audio (wav/pcm16 any rate) -> transcript
POST /v1/stt/stream          chunked pcm16 16kHz -> NDJSON partial/final
POST /v1/tts                 JSON text -> audio bytes (wav | pcm16)
POST /v1/tts/stream          JSON text -> NDJSON per-sentence audio
```

See [spec/API.md](./spec/API.md) for the full contract and
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md) for model/library licences.
