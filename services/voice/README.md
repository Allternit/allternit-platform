# Voice Service

Local speech-to-text sidecar for Gizzi Code and Allternit Desktop.

STT is [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (`whisper-cli` +
`ggml-tiny.en.bin`, MIT). No Python, no pyinstaller, no cloud STT.

TTS endpoints exist for contract compatibility and return metadata only.
A **full TTS product** (real audio, speak button, Gizzi `/speak`, packaging)
is specified in [`docs/specs/tts-product.md`](../../docs/specs/tts-product.md)
and is **not implemented**. Next agent: bake-off, then ship. Do not add a stub.

## Running

```bash
# From the repo root
cargo run -p voice-service

# Or
./services/voice/start.sh
```

Binds `127.0.0.1:${PORT:-8001}`. Build `whisper-cli` with `./build-whisper.sh`
(macOS deployment target 13.0). It is linked statically (`BUILD_SHARED_LIBS=OFF`,
`GGML_NATIVE=OFF`) because Desktop ships the binary alone in `resources/bin`; the
script fails if the result still loads `@rpath` dylibs. The ggml model downloads on first
`POST /v1/stt` into `~/.allternit/models/whisper/` unless `WHISPER_MODEL` is set.

```bash
cargo test -p voice-service
```

## API

```
GET  /health
GET  /v1/voices
POST /v1/stt          multipart: audio, language
POST /v1/tts          JSON stub
```

See [spec/API.md](./spec/API.md) for the full contract.
