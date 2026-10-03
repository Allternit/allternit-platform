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
| `small` (default) | Silero VAD + Moonshine tiny EN (quantized) + Kokoro-82M int8 EN + Smart Turn v3.2 (end-of-turn model for the voice session layer) | ~142 MB |
| `accurate` | Parakeet TDT 0.6B v3 int8 (25 European languages) | ~487 MB |

`GET /v1/models` reports per-pack state:
`missing | downloading{pct} | ready | error`; `POST /v1/models/<pack>`
starts a download in the background. Unpacked on disk: small ≈ 203 MB,
accurate ≈ 671 MB.

| Env var | Default | Meaning |
|---------|---------|---------|
| `PORT` | `8001` | Listen port (always `127.0.0.1`) |
| `ALLTERNIT_VOICE_THREADS` | `2` | Inference threads per model |
| `ALLTERNIT_VOICE_MODEL_DIR` | `~/.allternit/models/voice` | Pack directory |
| `ALLTERNIT_VOICE_MODEL_BASE` | sherpa-onnx GitHub releases | Download base URL |

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

## Native dependencies and packaging

The `sherpa-onnx` crate (k2-fsa, 1.13.x) links sherpa-onnx **and**
onnxruntime **statically** (crate default `static` feature; the sys crate
downloads the upstream per-platform static archive at build time:
`osx-arm64`/`osx-x64`, `linux-x64`/`linux-aarch64`, `win-x64` MT). bzip2
(model archive extraction) is built in statically as well. The release
binary depends only on OS libraries (macOS: libc++, system frameworks;
Linux: libstdc++/libm/libpthread/libdl; Windows: none beyond the OS, built
with `+crt-static` because the sherpa-onnx Windows libs use the static CRT).
There is no dylib/DLL to stage next to the binary: Desktop packaging copies
the one `allternit-voice-service` binary into `resources/bin`
(`scripts/build-desktop.sh`, `.github/workflows/release-desktop.yml` for
macOS universal, Windows and Linux).

## API

```
GET  /health                 engine + pack states
GET  /v1/voices              real Kokoro voices
GET  /v1/stt/models          moonshine (small) + parakeet (accurate)
GET  /v1/models              pack download states
POST /v1/stt                 multipart audio (wav/pcm16 any rate) -> transcript
POST /v1/models/:pack        start a pack download in the background
POST /v1/stt/stream          chunked pcm16 (?sample_rate=) -> NDJSON partial/final
POST /v1/tts                 JSON text -> audio bytes (wav | pcm16)
POST /v1/tts/stream          JSON text -> NDJSON per-sentence audio
```

## Using the engine from Rust

`SttEngine` (`transcribe`, `decode`/`transcribe_segment`, `stream` →
`SttStream::feed/finish` with its own VAD, `segment_stream` →
`SegmentStream::feed/partial/finish` for callers running their own VAD,
`new_vad_with(VadConfig)` for a tuned Silero VAD),
`TtsEngine` (`synthesize`, `synthesize_stream` with a per-sentence
callback that can stop early, plus `tts::split_sentences`) and `PackManager`
are public, take 16 kHz mono `f32` (STT) and return `f32` + sample rate
(TTS). All engine calls block; run them on `spawn_blocking` threads.

## Bench

`examples/voice_bench.rs` measures WER, finalisation latency, RTF and peak
RSS per STT model and condition, and TTS time-to-first-audio; results for
each host land in `bench/results-<host>.jsonl`. See the doc comment at the
top of the file for the exact definitions.

See [spec/API.md](./spec/API.md) for the full contract and
[THIRD_PARTY_NOTICES.md](./THIRD_PARTY_NOTICES.md) for model/library licences.
