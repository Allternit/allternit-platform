# Voice Service API Spec

## Overview

The Rust voice service is the only in-tree voice sidecar. STT and TTS are
real and run on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx):
Silero VAD + Moonshine/Parakeet (STT) and Kokoro-82M (TTS). Models download
on first use into `~/.allternit/models/voice/<pack>/` (sha256-verified).
Inference uses `ALLTERNIT_VOICE_THREADS` threads (default 2), CPU provider,
and runs one model at a time service-wide (STT segments and TTS sentences
interleave), so voice never uses more than that many inference threads.
Silero VAD runs on 1 thread per active request or stream.

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
| `label` | string | Same as `name` (field the allternit-ai voice pickers read) |
| `engine` | string | `kokoro` |
| `assetReady` | boolean | `true` once the `small` pack is installed |

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
| `pct` | number? | 0..1 progress across the whole pack while `downloading` |
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
  "stt_ready": false,           // Moonshine loaded
  "stt_accurate_ready": false,  // Parakeet loaded
  "tts_ready": false,
  "num_threads": 2,
  "packs": [ /* PackStatus */ ]
}
```

### `GET /v1/models`

Model pack download states (see PackStatus). Never triggers downloads.

**Response 200:** `{ "packs": [ /* PackStatus */ ] }`

Packs: `small` (default, ~39 MB download: Silero VAD + Moonshine tiny EN +
Smart Turn v3.2, the end-of-turn model the voice
session layer uses; fetched from its pinned Hugging Face revision unless
`ALLTERNIT_VOICE_MODEL_BASE` points at a mirror), `tts` (~350 MB: Kokoro-82M
v1.0 fp32, downloaded on the first TTS request) and `accurate` (~487 MB:
Parakeet TDT 0.6B v3 int8).
Files are sha256-checked against hashes pinned in `src/models.rs`, downloaded
to `<file>.part` (resumed with HTTP Range after an interruption) and only
moved/extracted into place after the hash matches. Base URL:
`ALLTERNIT_VOICE_MODEL_BASE` (default: the k2-fsa sherpa-onnx GitHub release
assets). Model dir: `ALLTERNIT_VOICE_MODEL_DIR` (default
`~/.allternit/models/voice`).

### `POST /v1/models/:pack`

Start downloading `small` or `accurate` in the background (idempotent; does
nothing if installed). For a settings screen that pre-fetches packs; normal
requests download on first use.

**Response 202:** the pack's `PackStatus`. **404:** unknown pack.

### `GET /v1/voices`

List the Kokoro v1.0 English voices (28: `af_*`/`am_*` US, `bf_*`/`bm_*` UK;
default `af_heart`). The old ids `af`, `default`, `en-us-female` map to
`af_heart`, `en-us-male` to `am_adam`.

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
  "voice": "af_heart",        // optional; aliases: voice_id, "default"
  "language": "en",           // optional (reserved)
  "speed": 1.0,               // optional
  "format": "wav"             // "wav" (default) | "pcm16"
}
```

**Response 200:** `audio/wav` (RIFF) or `application/octet-stream` (raw
s16le mono 24 kHz) with headers `X-Duration-Seconds`, `X-Sample-Rate`,
`X-Format`.

**Response 400:** empty text, unknown `voice`, or unsupported `format`.
**503:** TTS engine unavailable (e.g. the model download failed). Error
bodies are `{ "error": "..." }`.

Long text is synthesised sentence by sentence and concatenated.

The first request triggers the `small` pack download if not present; poll
`GET /v1/models` for progress.

### `POST /v1/tts/stream`

Streaming TTS. The text is split into sentences (abbreviation- and
initialism-aware), and the first sentence is cut short (at its first clause,
or about half of it) so first audio arrives early. Each piece is emitted as
soon as Kokoro renders it. Kokoro runs in the `allternit-tts` child process
(GPL-3.0, `services/voice-tts`); the first TTS request downloads the `tts`
pack and starts it (a few seconds to load the model).

**Request body:** same as `POST /v1/tts` (`format` ignored; chunks are pcm16).

**Response 200:** `application/x-ndjson` stream of events:

```jsonl
{"type":"audio","index":0,"text":"Hello there.","sample_rate":24000,"format":"pcm16","audio_b64":"<s16le base64>"}
{"type":"audio","index":1,"text":"How can I help?","sample_rate":24000,"format":"pcm16","audio_b64":"<...>"}
{"type":"done","duration_secs":4.32}
```

An unknown voice is rejected up front with 400. A failure mid-stream sends
`{"type":"error","error":"..."}` and ends the stream. If the client
disconnects, synthesis stops after the current sentence.

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
  (accurate pack; triggers its download on first use). The ids from
  `GET /v1/stt/models` and the pack names `small`/`accurate` also work.
- `sample_rate` (optional): only for raw PCM uploads (no WAV header);
  default 16000. 8 kHz phone audio works.

Moonshine v2 tiny cannot decode segments longer than ~9 s, so longer VAD
segments are split at the quietest point between 5 and 8 s before decoding.

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
`SpeechToText.ts`: same multipart input, response is
`{ "transcript": "...", "text": "...", "segments": [...] }`. Note that
`SpeechToText.ts` uploads WebM/Opus, which gets 415 (no WebM decoder in the
service); the client must send WAV or PCM.

### `POST /v1/stt/stream`

Streaming STT over one chunked HTTP request.

**Request:** body = raw s16le mono PCM, sent in chunks as it is captured
(any `Content-Type`; the API gateway forces `application/json`, which is
ignored). Query: `?sample_rate=` (default 16000, 4000–192000; resampled to
16 kHz internally), `?model=moonshine|parakeet`.

**Response 200:** `application/x-ndjson` events:

```jsonl
{"type":"partial","text":"hello wor"}
{"type":"final","text":"hello world","start":0.0,"end":1.4}
{"type":"done","duration_secs":6.1}
```

- `partial`: interim transcript of the speech in progress, re-decoded every
  0.6 s of audio while the VAD hears speech, or less often when decoding is
  slow (partials never take more than ~50% of real time; with Parakeet on a
  slow CPU they become sparse). Each replaces the last.
- `final`: a finished speech segment (the VAD heard 0.3 s of silence), with
  start/end in seconds from the start of the stream.
- `done`: the client closed the request body and all audio is transcribed.
- `error`: model download/load failed; the stream ends.

Several streams can run at once (each has its own VAD); inference is shared
and serialised. **400** for an unknown `model` or bad `sample_rate`.

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

Errors use standard HTTP status codes with a `{ "error": "..." }` body:
`400` for malformed requests (missing audio, empty text, unknown
model/voice/format), `404` for missing resources, `415` for unsupported audio
containers, `503` when an engine cannot be initialised (e.g. model download
failure; `GET /v1/models` then shows the pack in `error` state).
