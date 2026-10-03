# ChatGPT Voice parity

> TTS product spec (engine shipped 2026-10-02, UI in Phase 2): [`docs/specs/tts-product.md`](../../specs/tts-product.md).
> STT and TTS are live in the Rust service on sherpa-onnx (Silero VAD +
> Moonshine/Parakeet STT, Kokoro TTS). The Python Chatterbox tree was
> removed in PR #194 and whisper.cpp in the voice-engine phase 1.

ChatGPT Voice is a live spoken conversation that can combine microphone input, spoken responses, visual context, and delegated work. Allternit provides self-hosted STT/TTS services and ordinary agent orchestration, but the polished duplex consumer voice UI is still **roadmap**.

## Start talking

Run the optional voice service, then use its STT/TTS endpoints. The API gateway proxies voice discovery and streams at `/api/v1/voice/voices`, `/api/v1/voice/stt/stream`, and `/api/v1/voice/tts/stream`; it returns `503` if the voice service is unavailable.

```bash
# Direct voice service examples
curl -s -F 'audio=@question.wav' -F 'language=en' \
  http://127.0.0.1:8001/v1/stt

# TTS returns audio/wav bytes
curl -s http://127.0.0.1:8001/v1/tts \
  -H 'Content-Type: application/json' \
  -d '{"text":"Here is the result.","voice":"af"}' \
  --output result.wav
```

Both directions run in the single Rust binary (`services/voice`) on
sherpa-onnx; model packs download on first use into
`~/.allternit/models/voice/`. Native microphone capture is optional and is
not bundled in every `gizzi-code` build, so a missing `audio-capture-napi`
package means capture is unavailable even though HTTP STT remains usable.

## Have a conversation

Feed the transcription into an ordinary chat/session and send assistant text to TTS. The current APIs provide the pieces for turn-based spoken conversation; echo cancellation, interruption, and a polished full-duplex voice client remain roadmap.

## Delegate and coordinate work

Speech is an input/output transport, not a separate agent runtime. Transcribe the request, submit it to an agent/session/workflow, stream run progress, and synthesize the final response. Long-running coordination uses the same agent, task, Cowork, workflow, and swarm surfaces as typed chat; voice does not expand tool permissions.

## Show Allternit what you see

Use a file/image upload or ACI's `computer` screenshot action to add visual context. The SDK returns a base64 PNG image content block:

```typescript
const screenshot = await computer.getTool().execute(
  { action: 'screenshot' },
  { callId: 'voice-screen-1' }
);
```

Live mobile camera or screen-share directly inside a duplex voice conversation is **roadmap**. ACI can observe a configured desktop/browser, but it is not ambient camera access and remains governed by host permissions and human approvals.

See the [ACI guide](../aci/index.md) and [voice service API](../../../services/voice/spec/API.md).
