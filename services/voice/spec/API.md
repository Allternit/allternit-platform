# Voice Service API Spec

## Overview

The Rust voice service is the only in-tree voice sidecar. STT and TTS are
real and run on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx):
Silero VAD + Moonshine/Parakeet (STT) and Kokoro-82M (TTS). Models download
on first use into `~/.allternit/models/voice/<pack>/` (sha256-verified).
Inference uses `ALLTERNIT_VOICE_THREADS` threads (default 2), CPU provider.

Response shapes for `/v1/stt`, `/v1/voices`, and `/v1/stt/models` are
unchanged from the previous (whisper-era) contract; `/v1/tts` now returns
audio bytes instead of the old metadata-only descriptor (that shape returned
a dangling `audio_url` and was a documented bug — see
`docs/specs/tts-product.md` acceptance #7).

## Transport

- HTTP/1.1
- JSON request/response bodies, except where noted (binary audio)
- Multipart form data for STT audio uploads
- NDJSON (`application/x-ndjson`, one JSON event per line) for streaming
  endpoints
- Default bind address: `127.0.0.1:8001`

## Resources

### VoiceModel

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Voice identifier (Kokoro id, e.g. `af`, `am_michael`, `bm_george`) |
| `name` | string | Human-readable name |
| `language` | string | Language code (`en-US`, `en-GB`) |
| `gender` | string | Gender tag |
| `sample_rate` | integer | Output sample rate in Hz (24000) |

### SttModel

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | `moonshine-tiny-en` or `parakeet-tdt-0.6b-v3-int8` |
| `name` | string | Human-readable name |
| `language` | string | Supported language |
| `supports_streaming` | boolean | Whether streaming is supported (true) |

### VoiceSession

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | string | UUIDv4 session identifier |
| `created_at` | ISO8601 | Creation timestamp |
| `last_activity` | ISO8601 | Last activity timestamp |
| `mode` | string | `tts`, `stt`, or `both` |
| `language` | string | Session language |

### PackStatus

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | `small` or `accurate` |
| `state` | string | `missing` \| `downloading` \| `ready` \| `error` |
| `pct` | number? | 0..1 download progress while `downloading` |
| `error` | string? | Error message when `state=error` |
| `size_bytes` | integer? | On-disk size once `ready` |

## Endpoints

### `GET /health` / `GET /v1/health`

Service liveness, engine name, inference threads, engine readiness, and
pack states. Never triggers downloads.

**Response 200:**

```json
{
  "service": "voice",
  "status": "healthy",
  "version": "0.1.0",
  "timestamp": 1234567890000,
  "features": ["tts", "stt", "streaming"],
  "engine": "sherpa-onnx",
  "stt_ready": false,
  "tts_ready": false,
  "num_threads": 2,
  "packs": [ /* PackStatus */ ]
}
```

### `GET /v1/models`

Model pack download states (see PackStatus). Never triggers downloads.

**Response 200:** `{ "packs": [ /* PackStatus */ ] }`

### `GET /v1/voices`

List installed Kokoro voices (11 English voices: `af`, `af_bella`,
`af_nicole`, `af_sarah`, `af_sky`, `am_adam`, `am_michael`, `bf_emma`,
`bf_isabella`, `bm_george`, `bm_lewis`).

**Response 200:** `[ /* VoiceModel */ ]`

### `GET /v1/voices/:id`

Get a specific voice.

**Response 200:** `VoiceModel`
**Response 404:** Not found

### `GET /v1/stt/models`

List STT models.

**Response 200:** `[ /* SttModel */ ]`

### `POST /v1/tts`

Synthesise speech. Returns real audio bytes.

**Request body:**

```json
{
  "text": "string",
  "voice": "af",              // optional; aliases: voice_id, "default"
  "language": "en",           // optional (reserved)
  "speed": 1.0,               // optional
  "format": "wav"             // "wav" (default) | "pcm16"
}
```

**Response 200:** `audio/wav` (RIFF) or `application/octet-stream` (raw
s16le mono 24 kHz) with headers `X-Duration-Seconds`, `X-Sample-Rate`,
`X-Format`.

**Response 400:** Empty text. **503:** TTS engine unavailable.

The first request triggers the `small` pack download if not present; poll
`GET /v1/models` for progress.

### `POST /v1/tts/stream`

Sentence-streaming TTS. The text is split into sentences (abbreviation- and
initialism-aware); each sentence is synthesised and emitted as soon as it is
ready.

**Request body:** same as `POST /v1/tts` (`format` ignored; chunks are pcm16).

**Response 200:** `application/x-ndjson` stream of events:

```jsonl
{"type":"audio","index":0,"sample_rate":24000,"format":"pcm16","audio_b64":"<s16le base64>"}
{"type":"audio","index":1,"sample_rate":24000,"format":"pcm16","audio_b64":"<...>"}
{"type":"done","duration_secs":4.32}
```

`audio_b64` decodes to raw s16le mono PCM at `sample_rate`. Concatenating
all chunks yields the full utterance (same samples as non-streaming, split
per sentence).

### `POST /v1/stt`

Transcribe uploaded audio. Accepts WAV (PCM 8/16/24/32-bit, float32, G.711
μ-law/A-law; any sample rate; downmixed to mono) or raw 16 kHz s16le PCM.
Audio is resampled to 16 kHz; Silero VAD finds speech segments; the
recogniser (`model` field or `moonshine` default) transcribes each.

**Request body:** `multipart/form-data` with fields:
- `audio` (required): audio bytes
- `language` (optional): language hint (echoed back)
- `model` (optional): `moonshine` (default, small pack) | `parakeet`
  (accurate pack; triggers its download on first use)

**Response 200:**

```json
{
  "text": "hello world",
  "confidence": 0.9,
  "language": "en",
  "segments": [
    { "start_time": 0.0, "end_time": 1.4, "text": "hello world", "confidence": 0.9 }
  ]
}
```

`segments` carry VAD-derived timestamps. `confidence` is a heuristic
(0.9 when non-empty, 0.0 when empty) — sherpa-onnx offline recognisers do
not emit per-word confidence.

**Response 400:** Missing audio or unknown `model`.
**415:** Unsupported container (e.g. WebM/Opus — decode to WAV client-side).
**503:** STT engine unavailable.

### `POST /v1/stt/transcribe`

Compatibility alias for `POST /v1/stt` used by allternit-ai's
`SpeechToText.ts`: same multipart input, response is `{ "transcript": "..." }`.

### `POST /v1/stt/stream`

Streaming STT. **Request body:** raw chunked s16le mono **16 kHz** PCM
(`Content-Type: application/octet-stream`; `?model=moonshine|parakeet`).
Feed audio in chunks as it is captured; responses arrive as
`application/x-ndjson` events:

```jsonl
{"type":"partial","text":"hello wor"}
{"type":"final","text":"hello world","start":0.0,"end":1.4}
{"type":"done","duration_secs":6.1}
```

- `partial`: throttled (~every 800 ms) interim transcript of the
  in-progress speech.
- `final`: a finished VAD segment with absolute timestamps (seconds).
- `done`: input fully consumed (client closed the request body).

One streaming STT session at a time — a concurrent request gets
**409 Conflict**. Unknown `?model=` gets **400**.

### `GET /v1/sessions`

List active voice sessions.

**Response 200:** `[ /* VoiceSession */ ]`

### `POST /v1/sessions`

Create a new voice session.

**Request body:**

```json
{
  "mode": "tts | stt | both",
  "language": "string?"
}
```

**Response 200:** `VoiceSession`

### `GET /v1/sessions/:id`

Get a session.

**Response 200:** `VoiceSession`
**Response 404:** Not found

### `DELETE /v1/sessions/:id`

Delete a session.

**Response 204:** No content
**Response 404:** Not found

### `GET /v1/stats`

Service metrics.

**Response 200:**

```json
{
  "active_sessions": 0,
  "tts_models": 11,
  "stt_models": 2,
  "total_requests": 0,
  "timestamp": 1234567890000
}
```

## Error Handling

Error responses use standard HTTP status codes: `400` for malformed requests
(missing audio, empty text, unknown model/voice), `404` for missing
resources, `409` for a busy streaming STT session, `415` for unsupported
audio containers, `503` when an engine cannot be initialised (e.g. model
download failure).
