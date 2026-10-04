# Call worker: `allternit-voice-service worker`

The call worker answers phone calls on a bot's own number. It registers with LiveKit as agent `allternit-voice` and joins each `call-*` room that LiveKit SIP creates for an incoming call. It speaks the fixed AI disclosure and the bot's greeting, then holds the conversation: caller audio goes to the Voice Session core, each finished caller turn goes to the bot's runtime through cloud-api, and the reply is spoken back.

Contract: `HANDOFF-realtime-voice-2026-10-02.md` §4.1 (frozen between the voice session and the phone/channels session). Source: `services/voice/src/call_worker/`.

## Build and run

The LiveKit media binding pulls libwebrtc, so it sits behind a cargo feature. The Desktop sidecar build (`cargo build -p voice-service`) is unchanged and its `worker` subcommand prints a rebuild hint.

```bash
cargo build --release -p voice-service --features call-worker
allternit-voice-service            # Voice Session / STT / TTS service on 127.0.0.1:8001
allternit-voice-service worker     # call worker (separate process, same binary)
```

### Environment

| Variable | Required | Meaning |
|---|---|---|
| `LIVEKIT_URL` | yes | LiveKit server, `http(s)://` or `ws(s)://`. On allternit-standby: `http://127.0.0.1:7880`. |
| `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` | yes | The server's key pair (the standby's `livekit.yaml`). |
| `ALLTERNIT_CLOUD_API_URL` | yes | cloud-api base URL. |
| `ALLTERNIT_VOICE_WORKER_TOKEN` | yes | Bearer service token for the worker routes on cloud-api. |
| `VOICE_SESSION_URL` | no | Voice Session WebSocket. Default `ws://127.0.0.1:8001/v1/voice/session`. |
| `VOICE_SESSION_TOKEN` | no | Token for that socket, if it requires one. |
| `CALL_WORKER_MAX_CALLS` | no | Concurrent calls before the worker declines offers. Default 8. |
| `CALL_WORKER_START_TIMEOUT_MS` | no | Longest wait for cloud-api's call-start answer before the fallback opening plays. Default 900. |
| `ALLTERNIT_OUTBOUND_TRUNK_ID` | for warm transfer | LiveKit **outbound** SIP trunk id (`ST_…`). Without it a warm transfer answers `ok:false` with the reason. |
| `ALLTERNIT_RECORDING_S3_ENDPOINT` / `_BUCKET` / `_ACCESS_KEY` / `_SECRET` / `_REGION` | for recording | S3-compatible bucket (Cloudflare R2: region `auto`) the egress service uploads call audio to. All five, or recording is unavailable. |

Secrets are never logged; `WorkerConfig`'s `Debug` output leaves them out.

## Flow

1. **Dispatch.** The worker opens `<LIVEKIT_URL>/agent?protocol=1` with a JWT carrying the `video.agent` grant and sends `RegisterWorkerRequest {type: JT_ROOM, agent_name: "allternit-voice"}`. For each `AvailabilityRequest` it accepts if the room starts with `call-` and it is under `CALL_WORKER_MAX_CALLS`. On `JobAssignment` it reports `JS_RUNNING`, joins the room with the job token, and reports `JS_SUCCESS` or `JS_FAILED` when the call ends. It pings every 10 s and reconnects with backoff if pongs stop. Calls in progress survive a dispatch reconnect.
2. **Join.** It publishes the bot audio track (48 kHz mono) and waits up to 15 s for the SIP participant. It reads `sip.phoneNumber`, `sip.callID`, `sip.trunkPhoneNumber`, and the dispatch rule's attributes `{botId, ownerId, numberId, to}`. An outbound call with no `consentRef` is hung up at once.
3. **Call start, in parallel with opening the voice core.** `POST /api/v1/voice/calls` `{botId, numberId, from, to, direction, room, ownerId?, sipCallId?, consentRef?}` returns `{callId, bot: {name?, persona, voiceId, greeting, recording}}` from cloud-api's bot-config cache. The worker never waits on the user's runtime.
4. **Speak first.** `"Hi, you've reached {bot name}. I'm an AI assistant, and this call may be recorded."` (or `"…, and this call isn't recorded."`), then the greeting. Bot config can't remove or reword it.
   - If cloud-api fails or takes longer than the start timeout, the bot still says `"Hi, I'm an AI assistant, and this call isn't recorded. I can't reach my tools right now; I'll have someone follow up."` and hangs up.
   - If the voice core can't be reached at all, the worker deletes the room so the caller isn't left in silence.
5. **Turn loop.**
   - Caller audio is delivered at 16 kHz mono and goes to the core.
   - On `turn.ended`, the text goes to `POST /api/v1/voice/calls/{callId}/turns` (cloud-api → relay → runtime), and the streamed reply text goes into the core's speak path.
   - Bot speech is resampled to 48 kHz and published in 10 ms frames through a playout queue, so the call loop never waits on real-time pacing.
   - **Barge-in:** on `speak.interrupted`, the playout queue and the LiveKit source buffer are cleared at once and the in-flight turn is aborted.
   - **Slow replies:** a session that can't stream (vendor-bound, final reply only) can take 20 s or more. While the caller waits, the voice core plays a short filler ("One moment.") 1.2 s after the turn ends, then another every 6 s, at most 3 per turn. Fillers stop when the reply's first text arrives or the caller speaks again.
   - **Relay unavailable:** if the relay fails, or sends nothing within 60 s (the worker's turn budget; cloud-api's first-byte limit is also 60 s), the bot says the fixed line "I can't reach my tools right now; I'll have someone follow up." and logs the error. There is no scripted or fake reply path outside tests.
6. **End.** When the caller hangs up, a hangup control arrives, a transfer completes, or the room closes, the worker emits `call.ended` and deletes the room, which tears down the SIP leg.

## Events

Each event is posted to `POST /api/v1/voice/calls/{callId}/events` as `{type, idempotencyKey, seq, atMs, payload}`. The `Idempotency-Key` header is set too.

- `idempotencyKey` is `call:<callId>:<type>:<n>`, where `n` counts that event type within the call, from 1.
- `seq` is the order of the event across the whole call.
- Every `payload` includes `callId`.

A single task per call delivers events in order:

- **Transient failures** (network errors, 408, 429, 5xx) block the queue, which retries with backoff from 200 ms up to 10 s, indefinitely.
- **409** counts as delivered.
- **Any other 4xx** is logged and skipped, so one malformed event can't wedge the rest.

| Event | Payload | When |
|---|---|---|
| `call.started` | `direction, from, to, numberId` | Right after the opening is queued. |
| `call.transcript.delta` | `speaker: caller\|bot, text, final, segmentId` | Caller interim (`final:false`, live only) and final segments; bot utterances (final) when spoken or interrupted. |
| `call.dtmf` | `digits, from` | Caller keypad (`from` = caller), or DTMF the UI sent (`from` = bot number, as the ack). |
| `call.state.changed` | `held, mutedBot, mutedCaller, speaker` | Ack for mute, unmute, hold, resume, listen, and invalid controls. |
| `call.transferred` | `to, mode, ok, reason?` | Transfer result. `mode:"warm"` is the consult-and-connect transfer below; `ok:false` carries an honest `reason`. |
| `call.takeover` | `by, active` | Takeover (`active:true`) and release (`active:false`). |
| `call.voicemail.detected` | `action` | Outbound only. `left_message` or `hung_up`. |
| `call.ended` | `durationSec, reason, answered, missed?, recordingRef?` | Always last. `answered` is false when an outbound callee never picked up (reason `no_answer`: the SIP leg never reached `active` before it ended or the 60 s dial timeout); `missed:true` is set on an inbound call the caller dropped before the bot's first audio frame (reason stays `caller_hangup`). `reason` is one of `caller_hangup`, `hangup_control`, `transferred`, `room_closed`, `voice_engine_error`, `bot_hangup` (invite-code call finished), `failed` (invite-code call with a bad `otp`). |
| `call.summary` | `text, followUps[]` | Written by the **runtime** (not the worker) after `call.ended`, for a call with a final transcript segment: who called, what they wanted, what the bot did, follow-ups. Key `call:<callId>:call.summary:0`. |
| `call.transcript.revised` | `segments[{speaker?, text, startMs, endMs}]` | Posted by the worker's post-call job (below), after every event of the call, `seq` = last + 1, key `call:<callId>:call.transcript.revised:1`. The UI replaces the live transcript with it. `speaker` is unset: the recording is a mixed room composite. |

## Controls

cloud-api sends controls with RoomService `SendData` on topic `allternit.call.control`, as JSON `{action, target?, digits?, to?, mode?, by?}`. The worker only accepts packets the server sent. A packet from a participant, including a human who has taken over, is ignored.

| Action | Effect | Ack |
|---|---|---|
| `hangup` | Delete the room. | `call.ended` |
| `mute` / `unmute` `target: bot` (default) | Bot audio is dropped or allowed. | `call.state.changed` |
| `mute` / `unmute` `target: caller` | Caller audio stops or resumes reaching the core. | `call.state.changed` |
| `hold` / `resume` | Hold: bot goes silent, in-flight turn aborted, caller audio ignored (a synthesized chord pad plays to the caller). Resume reverses it. | `call.state.changed` |
| `dtmf` `digits` (0-9, `*`, `#`, A-D; up to 32) | Sent to the caller as SIP DTMF. | `call.dtmf` |
| `transfer` `to` (E.164 or `sip:`/`tel:`), `mode: cold` | SIP REFER via `TransferSIPParticipant`. | `call.transferred` |
| `takeover` `by` | Bot goes quiet and stops answering turns; caller transcription continues, and the human's speech is transcribed too. | `call.takeover {active:true}` |
| `release` `by` | Bot resumes with the same transcript context. | `call.takeover {active:false}` |
| `listen` | Nothing changes on the worker side; cloud-api minted the receive-only token. | `call.state.changed` |

## Running against the standby LiveKit

allternit-standby runs livekit-server v1.13.7 and LiveKit SIP. The inbound trunk is `ST_d6Ggxzg3CxoV` and the test number is +1 651-268-6010. The dispatch rule `SDR_jJct8QWKNHaD` sends calls to agent `allternit-voice` in rooms prefixed `call-`. Run both processes on the box, capped like the rest of the voice stack, with:

```bash
LIVEKIT_URL=http://127.0.0.1:7880 LIVEKIT_API_KEY=… LIVEKIT_API_SECRET=… \
ALLTERNIT_CLOUD_API_URL=https://… ALLTERNIT_VOICE_WORKER_TOKEN=… \
RUST_LOG=info,voice_service=debug allternit-voice-service worker
```

You don't need to place a call to check registration. `livekit-cli` shows the worker as registered, and `lk dispatch create --agent-name allternit-voice --room call-test` sends it a job.

## Agent protocol check (livekit-server v1.13.7)

- **Proto definitions.** `livekit_agent.proto` in the `livekit-protocol` 0.8.0 crate (protocol 1.50.4) is byte-identical to the one in protocol revision `a4f4b5c0c23f`, the revision v1.13.7's `go.mod` pins. `TransferSIPParticipantRequest` is identical too. The other SIP and model differences in that range only add fields.
- **Server behaviour, from the v1.13.7 source:**
  - The `/agent` route is registered in `pkg/service/server.go`.
  - The `video.agent` claim check and the `protocol` query parameter are in `pkg/service/agentservice.go`.
  - Binary protobuf framing, with a JSON fallback for text frames, is in `pkg/service/wsprotocol.go`.
  - The 10 s register and availability timeouts are in `pkg/agent/worker.go`. So is the empty `JobAssignment.url`, which means the worker joins its own `LIVEKIT_URL`.
- **Live check, 2026-10-03:** a livekit-server v1.13.7 built from the tagged source and run with `--dev` locally:
  - The worker registered.
  - It accepted a `call-` dispatch and declined a `meeting-` one.
  - It joined the room over WebRTC and published the bot track.
  - With no SIP participant, it deleted the room after 15 s.
  - This check caught a missing `livekit/native` feature, which registers the signalling transport. It's fixed in `Cargo.toml`.

## Invite verification-code call

When a carrier blocks a texted phone-invite code, cloud-api (`routes/phone_invites.rs`, `send_otp_call`) dials the invitee from the bot's number with room metadata and SIP participant attribute `purpose: "invite_code"` and `otp: "<code>"`; `consentRef` is the invitee's own consent row (basis `invite_otp`). The worker (`invite_code.rs`, wired in `room.rs` for outbound calls only) runs a different call:

- **No conversation.** No brain relay, no turns, no controls. A "Hello?" does not interrupt the code.
- **Script** (one utterance, bot name from the call-start config, fallback "Allternit"): "Hi, this is an automated call from {bot}, an AI assistant. This is {bot}'s verification code: 4. 2. 7. 1. 9. 3. ... Again, your code is 4. 2. 7. 1. 9. 3. Goodbye." Then it hangs up once the audio has played out. A call that has not finished after two minutes hangs up.
- **Answer screening still applies.** A person gets the script when they answer; on a machine the worker waits for the beep and leaves the same script (the invitee asked for the code).
- **The code is never logged or stored.** The transcript carries one bot line, "verification code read". The call is not recorded, so the recording cannot hold the code. `call.ended` reason is `bot_hangup`.
- **Bad `otp`** (not 4 to 10 digits): the disclosure and an apology, then hang up with `call.ended` reason `failed`.

The worker does not know which voice backend reads the digits; the full stops between digits are what make the voice pause on each one.

## Known gaps

- **Voice Session core.** The core (`services/voice/src/session/` and the WS route) is being built separately. The worker drives it through the protocol socket in `session_adapter.rs`; an in-process binding would only touch that file.

## Takeover, hold, warm transfer, voicemail, recording

- **Takeover transcription.** While `takeover` is active the call opens a second, transcription-only Voice Session for the human who joined the room (VAD and STT only: no turn-taking, no brain, no TTS). Final text is emitted as `call.transcript.delta` with `speaker:"human"`; the caller's pipeline keeps running. `release` closes it and the bot resumes. If no core can be opened, the worker logs that the human isn't transcribed.
- **Hold music.** `hold` plays a looped chord pad synthesized in code (`hold_music.rs`; no audio files, no licensing). The loop is whole periods of every partial, so it repeats seamlessly. `resume` stops it at once. The bot stays silent during hold.
- **Warm transfer** (`transfer` mode `warm`, `transfer.rs` and `consult.rs`): (1) hold the caller with music; (2) `CreateSIPParticipant` on `ALLTERNIT_OUTBOUND_TRUNK_ID` dials the target into a separate `consult-*` room; (3) the bot briefs the target by voice from a template over the transcript and asks them to press 1 to accept; (4) on accept, `MoveParticipant` moves the target into the caller's room (SIP REFER of the caller is the fallback) and the bot leaves, leaving the room standing; (5) on no answer, decline or error the caller comes off hold and the bot says so. `call.transferred {mode:"warm", ok, reason?}` reports it. Warm transfer dials out, so the control must carry the consent gate's `consentRef`; without it, or without an outbound trunk, it fails with `ok:false` and the reason.
- **Voicemail detection** (outbound with `consentRef`, `voicemail.rs`): for the first seconds after answer, the worker classifies human or machine from STT text ("leave a message", "after the tone"), greeting length, and a Goertzel beep detector (about 1 kHz for more than 300 ms). A person gets the normal opening. A machine: wait for the beep, leave the bot's `voicemailMessage` (or the default: who is calling and the callback number), hang up, and emit `call.voicemail.detected {action:"left_message"}`; a full or unavailable mailbox gives `hung_up`.
- **Recording** (`recording.rs`): only when the bot's `recording` is true. The worker starts a LiveKit Egress audio-only room composite (OGG) into the bucket at `calls/<callId>.ogg` and stops it at the end; `call.ended.recordingRef` is that key. If egress or the bucket settings are missing, the call is **not** recorded, an error is logged, `call.state.changed` carries `recording:false`, and the disclosure says "this call isn't recorded". The disclosure follows what is really happening, never only the config.

- **Revised transcript** (`revise.rs`): after a call that has a `recordingRef`, once its events are delivered, a background job (1) GETs the recording from the bucket with a minimal SigV4 request built from the `ALLTERNIT_RECORDING_S3_*` env (no secrets logged), (2) decodes it to 16 kHz mono with `ffmpeg` (**the voice host needs `ffmpeg` on PATH**), (3) runs the accurate STT (Parakeet, VAD segments) over the whole call and (4) posts `call.transcript.revised`. Bounds: calls over 30 min are skipped, one job at a time, nice 10. No recording, no bucket settings, no ffmpeg or no accurate pack means the job logs and does nothing; the live transcript stays.

### Deploy notes (egress and outbound trunk)

Outbound trunk, created once on LiveKit (`lk sip outbound create`), with the Telnyx credentials joe-07 holds:

```json
{ "trunk": { "name": "allternit-outbound", "address": "sip.telnyx.com",
  "numbers": ["<a Telnyx number owned by the account>"],
  "authUsername": "<telnyx sip user>", "authPassword": "<telnyx sip password>" } }
```

Set the returned `ST_…` id as `ALLTERNIT_OUTBOUND_TRUNK_ID` on the worker. Egress needs its own service next to LiveKit, on the same Redis:

```yaml
  egress:
    image: livekit/egress:latest
    restart: unless-stopped
    network_mode: host
    cpus: "1.0"
    environment:
      EGRESS_CONFIG_BODY: |
        api_key: ${LIVEKIT_API_KEY}
        api_secret: ${LIVEKIT_API_SECRET}
        ws_url: ws://127.0.0.1:7880
        redis: { address: 127.0.0.1:6379 }
        health_port: 9090
        cpu_cost: { room_composite_cpu_cost: 1.0 }
```

The worker passes the bucket settings (`ALLTERNIT_RECORDING_S3_*`) to egress per request, so the bucket keys live only in the worker's environment.

## Known gaps

- **Voice Session core.** The core (`services/voice/src/session/` and the WS route) is being built separately. The worker drives it through the protocol socket in `session_adapter.rs`; an in-process binding would only touch that file.
- **Live verification pending.** The warm-transfer dial-out, `MoveParticipant` of a SIP participant, and egress need the outbound trunk and the egress service above; they are covered by fakes in tests, not yet by a real call.
