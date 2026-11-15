# Zoom sidecar (Meeting SDK for Linux)

`zoom-sidecar` is a small C++ program around the Zoom Meeting SDK for Linux. The Node adapter (`../index.ts`) spawns it once per meeting and talks to it over fd 3. It joins with an OBF token, reports the waiting room and admission, reads and posts chat, polls participants and who is talking, sends mixed call audio out, plays reply audio into a virtual mic and sends the Gizzi tile as the camera.

Why native: since 2026-03-02 Zoom requires apps that join meetings hosted by other accounts to use the Meeting SDK with an OBF token (or a ZAK). Anonymous bots are refused ([OBF FAQ](https://developers.zoom.us/docs/meeting-sdk/obf-faq/), [OBF auth](https://developers.zoom.us/docs/meeting-sdk/auth/obf/)).

## Status

- **Built against the real SDK on 2026-11-13:** Meeting SDK for Linux x86_64 **7.1.5.4432**, Ubuntu 24.04 (inside the `allternit-cloud-computer` image, so glibc matches the pod). `ldd` finds every library, and the binary initializes the SDK and exits cleanly when fd 3 closes. The installed tree is 534 MB. Not yet run in a real call.
- The real headers rejected three things that the stub headers had accepted. All are fixed:
  - `RawDataOptions` spells the fields `audioRawdataMemoryMode` / `shareRawdataMemoryMode` (lower-case "data").
  - `setExternalVideoSource` takes a second argument, `FrameDataFormat`. It is probed as `HAVE_FrameDataFormat_I420_LIMITED`, and we send limited-range I420.
- 7.1.5 has no `MEETING_FAIL_NETWORK_ERR` or `EndMeetingReason_NetworkBroken`. Those codes are reported by number only.
- Where the SDK lives: Zoom licenses it for use inside our app only, so it is not in git. The tarball is in `/root/meetings-build/` on allternit-standby, together with the built tree (`zoom-sidecar-7.1.5.4432.tgz`, which unpacks to `zoom/`). The meeting-bot image script installs that tree with `ZOOM_SIDECAR_TGZ`.
- The chat, virtual mic and `IUserInfo::IsTalking` calls come from the SDK reference, not the sample. Check them on the first real build.
- Event interfaces gain pure virtual methods between SDK versions. If the build says a sink class is abstract, add the missing method as an empty override in `src/bot.h`.
- Enum names that differ between versions (fail codes, end reasons, recording-request states) are probed by CMake and turned into `HAVE_<NAME>` macros. A missing name is reported by number only and does not break the build.

## Getting the SDK (once per SDK version)

1. Sign in at https://marketplace.zoom.us as `allternitpbc@gmail.com`. The password is in the Keychain item `allternit-zoom-account`. Open **Develop > Manage > General app 780** (id `Xie3OlJ3RBalFDC8IZCIfA`) > **Embed > Meeting SDK > Linux > Download**. Pick x86_64 or arm64 to match the pod hosts.
2. You get a TAR file, `zoom-meeting-sdk-linux_<arch>-<version>.tar.xz` (layout `h/`, `libmeetingsdk.so`, `qt_libs/`). It holds no credentials.
3. Keep it out of git. Zoom licenses the SDK for use inside our app, not for redistribution. Put it in private storage (an R2 bucket private to the build) and in `vendor/` locally. `vendor/` is git-ignored.
4. Use version **6.6.10 or later**. Zoom's minimum is 5.17.5, and 6.6.10 adds clearer OBF error messages ([OBF FAQ](https://developers.zoom.us/docs/meeting-sdk/obf-faq/)).
5. Never paste the client secret into a shell, a log or this repo. The sidecar never needs it: it receives a signed SDK JWT at run time (see `../auth.ts`).

## Building

Linux only, Ubuntu 22.04 or later, x86_64 or arm64. Emulation (QEMU, Rosetta) is not supported by Zoom.

```bash
sudo apt-get install -y build-essential cmake pkgconf libglib2.0-dev nlohmann-json3-dev
bash build.sh vendor/zoom-meeting-sdk-linux_x86_64-<ver>.tar.xz            # installs /opt/allternit/zoom
bash build.sh vendor/zoom-meeting-sdk-linux_x86_64-<ver>.tar.xz /tmp/zoom  # or another prefix
```

To build inside the meeting-bot image, use the `Dockerfile` here as a builder stage. Set `BASE_IMAGE` to the meeting-bot runtime base, then `COPY --from=zoom-sidecar /opt/allternit/zoom /opt/allternit/zoom`. Also install the runtime packages from its "runtime" list in the meeting-bot image.

Installed layout: `/opt/allternit/zoom/zoom-sidecar` with `lib/` (the SDK `.so` files and `qt_libs/`), found through RPATH `$ORIGIN/lib`. Override the binary path with `ZOOM_SIDECAR_BIN`.

## Running (pod requirements)

- **PulseAudio must run**, or raw audio never arrives. The meeting-bot pod already starts it (spike `setup-linux.sh` uses a null sink as default). The sidecar writes `~/.config/zoomus.conf` with `system.audio.type=default` if it is missing ([raw-data guide](https://developers.zoom.us/docs/meeting-sdk/linux/custom-ui/advanced-features/raw-data/)).
- No X display is needed for the SDK itself. `QT_LOGGING_RULES` is set by the adapter to quiet Qt.
- The SDK writes its own logs to `~/.zoom/logs`. stdout and stderr go to the adapter's log as plain lines.
- One sidecar per meeting. It exits after `shutdown`, when fd 3 closes (the Node process died), or on SIGTERM. Each time it leaves the meeting first.

## Raw audio permission

Raw audio needs local-recording permission in the meeting. The sidecar handles it like this:

1. It calls `CanStartRawRecording()`. If that passes, recording starts right away. It passes when the join carried a local-recording join token (`app_privilege_token`) or the host's settings allow it.
2. Otherwise it calls `RequestLocalRecordingPrivilege()`. The host sees a request from "Gizzi (Allternit)". The sidecar reports `recording: requested`, then `granted`, `denied` or `timeout`.
3. If the host says no, Gizzi stays in the call and chat works, but there is no transcript. The runner should post a chat line saying so.

The local-recording join token comes from `GET /v2/meetings/{meetingId}/jointoken/local_recording`. Our app has that scope, `meeting:read:local_recording_token`. The token lives 120 s. Forum reports say it works only for meetings the authorising user hosts, so for other people's meetings expect step 2. The adapter takes it through the `freshTokens` option, so it can be fetched right before join.

## Protocol

Transport is fd 3 of the sidecar, a socketpair Node creates (`stdio: ["ignore","pipe","pipe","pipe"]`). `ZOOM_IPC_FD` overrides the fd number.

Frame: `[u32 big-endian N][u8 type][N-1 body bytes]`, where `1 <= N <= 16 MiB`. A bad length means a corrupt stream: the reader drops the connection, and the sidecar leaves and exits.

| Type | Direction | Body |
|---|---|---|
| `0x01` JSON | both | UTF-8 JSON object |
| `0x02` AUDIO_OUT | sidecar → node | `u32 BE sampleRate`, `u8 channels`, s16le PCM (mixed call audio; Zoom sends 32 kHz mono) |
| `0x03` AUDIO_IN | node → sidecar | same layout. PCM for the virtual mic, 8 to 48 kHz, mono or stereo (stereo is downmixed). Queued and sent in 20 ms steps at real time. A new sample rate replaces the queue. |
| `0x04` VIDEO_IN | node → sidecar | `u16 BE width`, `u16 BE height`, I420 planes (even sizes). Sent only while Zoom has the camera started. |

Unknown frame types are skipped. The sidecar drops AUDIO_OUT frames when more than 4 MiB is queued (Node fell behind) and prints the count on exit. JSON frames are never dropped.

Commands (node → sidecar), each with an integer `id`. Each gets exactly one `{"type":"result","id","ok","error?","data?"}`.

| `op` | Fields | Result |
|---|---|---|
| `join` | `sdkJwt`, `obfToken`, `displayName`, `meetingNumber` or `vanityId`, `passcode?`, `appPrivilegeToken?` | `ok` once `SDKAuth` is accepted. The outcome follows as `auth` and `status` events. It may be sent again after a `failed` or `ended` status: the adapter retries while the authorising user is absent. The second attempt reuses the SDK auth and calls `Join` directly. |
| `leave` | | `ok`. `status: ended` follows. |
| `chat_send` | `text` | `ok` if `SendChatMsgTo` succeeded (to everyone) |
| `participants` | | `data.list` = participants (see below) |
| `tile_state` | `state` | `ok` (informational; the picture comes as VIDEO_IN) |
| `audio_clear` | | `ok`, queued mic audio dropped |
| `shutdown` | | `ok`, then leave and exit (after Zoom confirms the leave, or 3 s) |

Events (sidecar → node):

| `type` | Fields |
|---|---|
| `ready` | `sdkVersion`: sent once after `InitSDK` |
| `auth` | `ok`, `code` (`AuthResult`) |
| `status` | `status` ∈ `idle, connecting, waiting_for_host, in_waiting_room, in_meeting, disconnecting, reconnecting, failed, ended`, `code` (the SDK `iResult`: `MeetingFailCode` when failed, `EndMeetingReason` when ended), `name?` (symbolic name when the SDK has it). Other SDK statuses (locked, breakout moves) are logged, not sent. |
| `chat` | `msgId`, `senderId`, `sender`, `text`, `ts` (epoch s), `self` |
| `participants` | `list: [{id, name, self, host}]`: sent at most once a second, on change |
| `active_speaker` | `ids`: SDK user ids currently talking (not the bot). Polled every 250 ms, sent on change. |
| `recording` | `state` ∈ `granted, requested, denied, timeout, started, failed`, `code?` |
| `mic` | `state` ∈ `ready, started, stopped, gone` |
| `log` | `level`, `msg` |

How the adapter maps this to the contract (`../index.ts`):
- `in_waiting_room` or `waiting_for_host` → phase `waiting_admit`; `in_meeting` → `live`.
- `ended` while waiting → `not_admitted`.
- `MEETING_FAIL_AUTHORIZED_USER_NOT_INMEETING` (the person who connected Zoom is not in the call) → phase `waiting_admit`. The join is sent again every 20 s (`ownerRetryMs`) within `admitTimeoutMs`. If the budget runs out → `not_admitted`. The same budget then covers the waiting room. This needs the SDK to have that enum name; CMake reports it if missing, and the code is then reported by number only and not retried.
- Other `ON_BEHALF`, `OBF`, `RESTRICTED`, `BLOCKED`, `SIGN_IN` or `PRIVILEGE_TOKEN` names → `guest_blocked`. Anything else → `join_failed`.
- After live, `ended` → `removed`, unless the adapter asked to leave (`left_by_user`). `failed` or `EndMeetingReason_NetworkBroken` → `error`.

## Tests

`services/meeting-bot/test/zoom/zoom.test.ts` drives the real adapter against `test/zoom/fake-sidecar.ts`. The fake is a Node process speaking this protocol on fd 3, with frames sent one byte at a time and several frames in one write. The test covers:
- link parsing and framing
- the waiting room, a decline and the admit timeout
- the owner-present retry: the owner joins late, the owner never joins, and an OBF token conflict is not retried
- a removal and a corrupt stream
- chat, roster, speaker and audio in both directions

The tests live outside `src/`, so the package build never emits them. They compile into `dist/test/` (git-ignored with `dist/`).

```bash
cd services/meeting-bot
tsc -p test/zoom/tsconfig.json && node --test dist/test/test/zoom/zoom.test.js
```
