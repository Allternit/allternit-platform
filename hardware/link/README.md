# Allternit Link — Phase 1 contract

Status: draft contract for the integration branch `integ/link-phase1`. Nothing here is merged to main or deployed. No board and no e-paper panel has been tested. Every hardware value is `open`.

Spec: `Research/specs/allternit-link-keychain-bot.md` in the Brain (approved 2026-11-10). Build plan: `docs/research-allternit-link-keychain-bot-PLAN.md`.

Builders work from this page alone. If the code and this page disagree, stop and tell the orchestrator. Do not pick one silently.

## Hard rules

1. Do not edit `cmd/allternit-api/src/phone_guard.rs`, `surfaces/allternit-desktop/src/main/phone-hold-client.ts`, or any hold screen. The "Phone and Messages" lane owns them. Use the contract in `surfaces/docs/api/phone-holds.mdx`. Every tap goes through the existing tool runner (`PhoneToolRunner.dispatch` in `phone-tools.ts`), so the guard applies with no new code.
2. The board boots disarmed. Only its button arms it. Software can only disarm. No message in USBA/2 may arm a board. `face`, `prog` and `label` are allowed while disarmed because they move nothing.
3. No GPL code in firmware. `ReferenceBoard` changes in the same commit as the firmware. When firmware changes, rebuild the shipped image with `hardware/usb-agent/build-firmware.sh` and commit both files (`usb-agent-firmware.bin`, `firmware.json`).
4. No merge to main, no deploy, no parts bought, no eFuse burn. Everything lands on `integ/link-phase1`. Draft pull requests are allowed.
5. Smoke-boot allternit-api before any route merge. Production cloud-api runs with `ALLTERNIT_SKIP_MIGRATIONS=1`: the binary does not apply a new migration. It is applied at deploy time from the `ops_applied_migrations` ledger, or by hand. Nothing is applied in this phase.
6. Tests must be able to fail and must assert what the user gets. No filler tests. Do not mock our own code; fake only the edges (network, clock, serial port, the panel).
7. Rust builds are capped machine-wide. In an Agent Tree run `eval "$(gizzi agents tree env)"` first. Use `cargo check` while iterating. Only the relay lane runs cargo (see the plan).

## 1. Scope and non-goals

In scope for Phase 1:

- USBA/2, a strict extension of USBA/1 (section 2).
- A face on a 1-bit display, built against a fake display (section 3).
- A board that reaches the user's node through the cloud relay (section 4).
- The composite lane: board hands and app-phone eyes (section 5).
- One status shape for every surface (section 6).
- The dock contact list and the hardware notes skeleton (section 7).

Not in scope: a custom circuit board, the power bank, bank screen firmware, moulded shells, phone-only Bluetooth setup, compliance testing, prices, any sale. Bluetooth mouse mode (spec step 7) and hardening (spec step 8) wait for a real board; this page only reserves their names.

## 2. USBA/2 wire protocol

USBA/1 is defined by `surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts` and `hardware/usb-agent/firmware/usb_agent/usb_agent.ino`. USBA/2 keeps every USBA/1 line and rule and adds to them. A USBA/1 session behaves exactly as today.

### 2.1 Unchanged constants

Port 7300. mDNS type `_allternit-usba._tcp`. Longest line 512 bytes. Lease 10 s. Desktop heartbeat 3 s. Handshake limit 5 s. Board id `usba-<12 lowercase hex>`. Key K: 32 bytes, set only over USB serial.

### 2.2 Version negotiation

- The board sends `hello usba/<v> <boardId> <fw> <resetReason> <bn>`. `v` is `1` or `2`. A board speaks the one version its firmware was built with.
- Desktop accepts `usba/1` and `usba/2`. `parseHello` returns `version: 1 | 2`. Any other version is `bad_hello` and the connection is dropped.
- There is no downgrade message. A USBA/2 board that meets an old Desktop is dropped at `hello`.
- The version is bound into the USBA/2 handshake (section 2.3). A party in the middle that rewrites `usba/2` to `usba/1` makes the handshake fail.
- Desktop sends USBA/2-only commands (`face`, `prog`, `label`, `typex`) only on a USBA/2 session. On a USBA/1 session `UsbAgentLink.send` answers `{ ok: false, error: 'invalid', message }` for `face`, `prog` and `label` without sending anything. Reason: USBA/1 firmware resets its idle timer before it looks at the command name (`usb_agent.ino` line 286), so an unknown command would keep an armed board armed.

### 2.3 Handshake

| Step | USBA/1 | USBA/2 |
|---|---|---|
| board → desk | `hello usba/1 <id> <fw> <reason> <bn>` | `hello usba/2 <id> <fw> <reason> <bn>` |
| desk → board | `challenge <sn> <HMAC(K, "srv\|id\|bn\|sn")>` | `challenge <sn> <HMAC(K, "srv2\|id\|bn\|sn")>` |
| board → desk | `auth <HMAC(K, "brd\|id\|bn\|sn")>` | `auth <HMAC(K, "brd2\|id\|bn\|sn")>` |
| desk → board | `ready` | `ready` |
| session key SK | `HMAC(K, "ses\|bn\|sn")` | `HMAC(K, "ses2\|bn\|sn")` |
| text key EK | none | `HMAC(K, "enc2\|bn\|sn")` |

All HMACs are HMAC-SHA-256 written as 64 lowercase hex characters. `bn` and `sn` are 16 random bytes as 32 hex characters. Line shapes and lengths are the same in both versions.

### 2.4 Lines after `ready`

Desk → board, both versions:

```
c <seq> <command> <tag>        tag = first 32 hex of HMAC(SK, "c|<seq>|<command>")
```

Board → desk, USBA/1 (unchanged, not signed):

```
r <seq> ok
r <seq> err <reason>
s armed=<0|1> usb=<mounted|asleep|none>
```

Board → desk, USBA/2 (signed):

```
r <seq> <result> <tag>         result = "ok" or "err <reason>"
                               tag = first 32 hex of HMAC(SK, "r|<seq>|<result>")
s <fields> n=<count> <tag>     tag = first 32 hex of HMAC(SK, "s|<fields> n=<count>")
```

Desktop rules on a USBA/2 session:

- A reply or state line with a missing or wrong tag: destroy the socket. The session ends with reason `bad_tag`. A pending command resolves `uncertain`.
- `n` starts at 1 and rises by one for each state line in the session. A state line whose `n` is not higher than the last one applied is ignored.
- Reply parse: `^r (\d{1,10}) (ok|err .{1,200}) ([0-9a-f]{32})$`.

Sequence numbers: one counter per session for every command, old and new. The board accepts only a strictly higher `seq`. A lower or equal `seq` with a valid tag answers `err replay`. A bad tag or a malformed line drops the connection. A `seq` is used up even when the command answers an error.

### 2.5 State line fields (USBA/2)

Fixed order. Desktop ignores unknown `key=value` tokens between `prog=` and `n=`.

```
s armed=1 usb=mounted dock=0 hid=usb link=lan face=working prog=3 n=7 <tag>
```

| Field | Values | Meaning |
|---|---|---|
| `armed` | `0`, `1` | As USBA/1. |
| `usb` | `mounted`, `asleep`, `none` | As USBA/1: the phone's USB host state. |
| `dock` | `0`, `1` | `1` while the dock detect input has been active for 100 ms or more. Always `0` when the build has no detect pin. |
| `hid` | `usb`, `ble`, `none` | The transport the hands use now. `usb` when `usb` is not `none`. `ble` only in a Bluetooth build with a host connected and USB not mounted. Else `none`. Phase 1 images never report `ble`. |
| `link` | `lan`, `cloud` | How this session reached Desktop: a direct TCP dial, or the cloud relay (section 4). The board knows which it dialled. |
| `face` | a face name (section 3) | The face shown now. |
| `prog` | `0` to `5` | Filled progress squares. |

The board sends a state line after `ready`, and whenever any field changes.

### 2.6 Commands

"Armed" = refused with `err disarmed` when disarmed. "Idle timer" = resets the 30 minute armed-idle timer (`lastInputAt`).

| Command | Since | Syntax | Bounds | Armed | Idle timer | Errors |
|---|---|---|---|---|---|---|
| `ping` | 1 | `ping` | | no | no | |
| `disarm` | 1 | `disarm` | | no | no | |
| `home` | 1 | `home` | | yes | yes | `disarmed`, `phone usb asleep` |
| `move` | 1 | `move <dx> <dy>` | each within ±4000 | yes | yes | `bad numbers`, `out of bounds`, `disarmed`, `phone usb asleep` |
| `press` | 1 | `press` | | yes | yes | `disarmed`, `phone usb asleep` |
| `release` | 1 | `release` | | yes | yes | `disarmed`, `phone usb asleep` |
| `click` | 1 | `click` | | yes | yes | `disarmed`, `phone usb asleep` |
| `scroll` | 1 | `scroll <v> <h>` | each within ±50 | yes | yes | `bad numbers`, `out of bounds`, `disarmed`, `phone usb asleep` |
| `type` | 1 | `type <hex>` | 1 to 200 printable ASCII bytes (0x20 to 0x7e), hex-encoded | yes | yes | `bad text`, `disarmed`, `phone usb asleep` |
| `key` | 1 | `key <combo>` | 1 to 4 names joined by `+` | yes | yes | `unknown key`, `disarmed`, `phone usb asleep` |
| `typex` | 2 | `typex <hex>` | sealed form of `type` (section 2.7) | yes | yes | `bad text`, `disarmed`, `phone usb asleep` |
| `face` | 2 | `face <name>` | `default`, `wink`, `working`, `needs_you`, `stopped` | no | no | `unknown face` |
| `prog` | 2 | `prog <n>` | one digit, `0` to `5` | no | no | `bad numbers`, `out of bounds` |
| `label` | 2 | `label <hex>` or `label -` | 1 to 24 printable ASCII bytes, hex-encoded; `-` clears it | no | no | `bad text` |

Other reply errors, both versions: `replay`, `unknown command`.

Rules for the three display commands:

- They are authenticated like every command: tag, sequence number, lease refresh.
- They must not arm, must not send a HID report, must not call `usbReady()`, and must not touch `lastInputAt`. In the firmware they sit next to `ping` and `disarm`, above the `if (!armed)` line in `run()`.
- They answer `ok` as soon as the request is stored. They do not wait for the panel.
- `label` is stored in RAM only. Phase 1 does nothing else with it (it is for the bank).

### 2.7 `typex`: typed text the relay cannot read

USBA lines are not encrypted. Over the relay, the cloud sees them. Text a bot types can be a password, so on a USBA/2 session Desktop always sends `typex` and never `type`.

- Cipher: AES-256-GCM. Key: EK. IV (12 bytes): four zero bytes, then `seq` as an 8-byte big-endian integer. AAD: the ASCII string `c|<seq>|typex`.
- Plaintext: the same 1 to 200 printable ASCII bytes `type` takes.
- `<hex>` = ciphertext followed by the 16-byte GCM tag, hex-encoded (34 to 432 hex characters). The line stays under 512 bytes.
- The board checks the command tag first, then opens the text. A GCM failure answers `err bad text`.
- A USBA/2 board still accepts `type`.

### 2.8 Faces the firmware sets itself

- Boot: face `default`, prog `0`, disarmed.
- Arm (button only): face `default`.
- Any disarm (button, `disarm`, a timer, the stop input): face `stopped`, prog `0`.
- The armed mark on the display (section 3) follows `armed`. No message sets it.

### 2.9 Provisioning additions (USB serial)

`prov info`, `prov wipe`, `prov commit` and the existing `prov set` keys are unchanged. New optional keys:

| Line | Value | Meaning |
|---|---|---|
| `prov set relay <host>` | 1 to 63 characters of `a-z 0-9 . -` | The relay host, for example `api.allternit.com`. |
| `prov set rtok <hex>` | 64 lowercase hex characters | The relay token (section 4.2). |
| `prov set ssid1 <hex>` … `ssid3`, `prov set pass1 <hex>` … `pass3` | as `ssid` and `pass` | Up to three more saved networks. |

`prov commit` needs the same fields as today. `relay` and `rtok` are stored only when both are set. `prov wipe` clears them.

### 2.10 Desktop types (exports of `phone-usb-agent-protocol.ts`)

```ts
export type UsbaVersion = 1 | 2;
export type FaceName = 'default' | 'wink' | 'working' | 'needs_you' | 'stopped';
export const FACE_NAMES: readonly FaceName[];
export type HidTransport = 'usb' | 'ble' | 'none';
export type LinkKind = 'lan' | 'cloud';

export interface Hello { version: UsbaVersion; boardId: string; firmware: string; resetReason: string; boardNonce: string }

export interface BoardState {
  armed: boolean;
  usb: UsbStatus;
  // Present on USBA/2 sessions only.
  dock?: boolean;
  hid?: HidTransport;
  link?: LinkKind;
  face?: FaceName;
  prog?: number;
}

export type BoardCommand =
  | /* the USBA/1 members, unchanged */
  | { op: 'face'; name: FaceName }
  | { op: 'prog'; n: number }
  | { op: 'label'; text: string };   // '' clears it

export const serverProof: (k: string, id: string, bn: string, sn: string, v?: UsbaVersion) => string;
export const boardProof:  (k: string, id: string, bn: string, sn: string, v?: UsbaVersion) => string;
export const sessionKey:  (k: string, bn: string, sn: string, v?: UsbaVersion) => string;
export const textKey:     (k: string, bn: string, sn: string) => string;            // EK
export const replyTag:    (sk: string, seq: number, result: string) => string;
export const stateTag:    (sk: string, body: string) => string;
export function sealType(ek: string, seq: number, text: string): string;            // "typex <hex>"
export function parseState(line: string, v?: UsbaVersion, sk?: string): (BoardState & { n?: number }) | null;
export function parseReply(line: string, v?: UsbaVersion, sk?: string): Reply | null;
```

`v` defaults to `1`, so existing callers compile unchanged. `BoardSessionInfo` in `phone-usb-agent-link.ts` gains `version: UsbaVersion`. `SendResult` gains no new error code.

`ReferenceBoard`: constructor `(boardId, keyHex, firmware = '1.1.0', version: UsbaVersion = 2)`. New public fields `dock`, `link`, `face`, `prog`, `label`, `display` (a `FakeDisplay`, section 3). New methods `pressButton()` (toggles armed as `serviceButton()` does) and `pressStop()` (disarm only). Both return the state line the firmware would send. `hid` is derived as in section 2.5.

## 3. Face set

### 3.1 Canvas

- 200 × 200 pixels, 1 bit per pixel. `1` = black, `0` = white (the tan comes from the window, not the panel).
- Buffer: 5000 bytes, row-major, 25 bytes per row, most significant bit first.
- The candidate panel is a 1.54 inch 200 × 200 mono module with partial refresh, SSD1681 class. The choice is not final. Art is authored for this canvas. A different panel means new art; the interface below does not change.
- Gizzi's coral `A://` is printed on the window. The panel never draws it. Keep-out area, always white: x 60 to 139, y 118 to 152.

Art must follow `Marketing/Brand Kit And Memory/GIZZI-CHARACTER.md`: two solid black square eyes, no nose, no mouth.

### 3.2 Faces

Every shape is a filled rectangle `x, y, w, h` in canvas pixels. The geometry below is the proposed default (see open point O-9).

| Face | Shows | Rectangles |
|---|---|---|
| `default` | Two open square eyes. | `46,62,36,36` and `118,62,36,36` |
| `wink` | Left eye open, right eye a flat bar. | `46,62,36,36` and `118,76,36,8` |
| `working` | Both eyes narrowed. The progress row is always shown. | `46,72,36,16` and `118,72,36,16` |
| `needs_you` | Both eyes open, and an exclamation mark above them. | eyes as `default`; stem `96,10,8,28`; dot `96,42,8,8` |
| `stopped` | Both eyes closed. | `46,77,36,6` and `118,77,36,6` |

No face changes by a timer. Desktop sends the next face.

### 3.3 Progress squares and the armed mark

- Five squares, 20 × 20, at y 164, x = 26, 58, 90, 122, 154.
- Square `i` (1 to 5) is solid when `i <= prog`. Otherwise it is a 2 pixel outline.
- The row is drawn when `prog > 0` or the face is `working`. Otherwise it is blank.
- Armed mark: solid 8 × 8 square at `184,8`. Drawn only while armed.

### 3.4 Renderer

One pure function, written twice (C++ and TypeScript) with the same integer rules, so both make the same bytes.

Firmware, new file `hardware/usb-agent/firmware/usb_agent/face.h` (plain C++, no Arduino includes):

```cpp
enum FaceName : uint8_t { FACE_DEFAULT, FACE_WINK, FACE_WORKING, FACE_NEEDS_YOU, FACE_STOPPED, FACE_COUNT };
const uint16_t FACE_W = 200, FACE_H = 200;
const size_t FACE_BYTES = 5000;
bool faceFromName(const char *name, FaceName *out);
const char *faceName(FaceName f);
void renderFace(uint8_t out[FACE_BYTES], FaceName face, uint8_t prog, bool armed);
```

Desktop, new file `surfaces/allternit-desktop/src/main/phone-usb-agent-face.ts`:

```ts
export const FACE_W = 200, FACE_H = 200, FACE_BYTES = 5000;
export function renderFace(face: FaceName, prog: number, armed: boolean): Uint8Array;
export function toPbm(bits: Uint8Array): string;   // P1 text, for test output and review
```

Host check: `hardware/usb-agent/firmware/host/face_dump.cpp` (outside the sketch folder, so arduino-cli does not compile it) includes `../usb_agent/face.h` and prints the 5000 bytes as hex for a face, prog and armed value given as arguments. The Desktop test compiles it with the system `c++` and compares its output with `renderFace` for every face, prog 0 to 5, armed and not.

### 3.5 Display interface

Firmware, new file `hardware/usb-agent/firmware/usb_agent/display.h`:

```cpp
class LinkDisplay {
 public:
  virtual bool begin() = 0;                              // false: no panel answered; the firmware keeps running
  virtual uint16_t width() const = 0;
  virtual uint16_t height() const = 0;
  virtual bool busy() = 0;                               // a refresh is still running
  virtual void show(const uint8_t *bits, bool full) = 0; // starts a refresh and returns at once
  virtual void sleep() = 0;                              // panel deep sleep; the picture stays
};
class NullDisplay : public LinkDisplay { /* begin() returns false; the rest do nothing */ };
class Ssd1681Display : public LinkDisplay { /* our own driver, written from the controller datasheet */ };
```

- Built with `-DLINK_DISPLAY_SSD1681` plus `EPD_CS_PIN`, `EPD_DC_PIN`, `EPD_RST_PIN`, `EPD_BUSY_PIN`, `EPD_SCK_PIN`, `EPD_MOSI_PIN`. Without the flag the build uses `NullDisplay`. The shipped image uses `NullDisplay`.
- The main loop never blocks on the panel. It keeps one pending frame (the newest wins) and calls `show` when `busy()` is false. The button is read as often as today.
- Partial refresh for a face or prog change. Full refresh for `stopped`, at boot, and after every 20 partial refreshes.
- The driver is written by us from the SSD1681 datasheet. Do not copy from, or read while writing, GxEPD2 (GPL-3.0) or the Waveshare examples (no licence file). Credit nothing that was not used. Update `hardware/usb-agent/NOTICE` only for code that was really taken.

Desktop twin, in `phone-usb-agent-face.ts`:

```ts
export interface LinkDisplay {
  readonly width: number;
  readonly height: number;
  busy(): boolean;
  show(bits: Uint8Array, full: boolean): void;
  sleep(): void;
}
export class FakeDisplay implements LinkDisplay {
  readonly frames: Array<{ bits: Uint8Array; full: boolean }>;
  last(): Uint8Array | null;
}
```

`ReferenceBoard` calls `display.show(renderFace(face, prog, armed), full)` at the same moments the firmware does.

## 4. Board through the cloud relay

The pattern is the app phone's (`cmd/allternit-cloud-api/src/routes/app_phones.rs`, `runtime_relay.rs`, `cmd/allternit-api/src/phone_app.rs`): a long-lived secret buys a 30 second, single-use socket ticket for one exact node path.

```
board ──HTTPS──▶ cloud-api  POST /api/v1/link-boards/:boardId/socket-ticket   (relay token)
board ──WSS────▶ cloud-api  GET  /api/v1/runtime-devices/<rt>/socket?ticket=…
                 cloud-api ──relay──▶ allternit-api  WS /api/v1/phones/usba?board=<id>
                                      allternit-api ──TCP 127.0.0.1:7300──▶ Desktop UsbAgentLink
```

### 4.1 Table and migration

File `cmd/allternit-cloud-api/migrations_pg/121_link_boards.sql`. 120 is the highest number on origin/main at 976ef690d. Re-check the number against origin/main and open pull requests at merge time and renumber if needed. Never edit a migration file after it has shipped.

```sql
CREATE TABLE IF NOT EXISTS public.link_boards (
    account text NOT NULL,
    board_id text NOT NULL,
    runtime_device_id text NOT NULL,
    name text,
    token_hash text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz,
    revoked_at timestamptz,
    PRIMARY KEY (account, board_id)
);
CREATE INDEX IF NOT EXISTS idx_link_boards_board_id ON public.link_boards (board_id);
```

- `token_hash`: SHA-256 (hex) of the 32-byte relay token. The token is never stored and never logged.
- The key is `(account, board_id)`, not `board_id` alone. Board ids come from the MAC address and can be guessed; a global key would let anyone block a board by registering its id first.
- Add `("link_boards", "account = $1")` to the erase list in `cmd/allternit-cloud-api/src/services/account_erase.rs`, before `runtime_devices` (line 121). The test at lines 667 to 683 fails for any table with a person column that is not classified.

### 4.2 Cloud routes (`cmd/allternit-cloud-api/src/routes/link_boards.rs`, new)

Mounted on the public router next to `app_phones` (`src/lib.rs` line 561, `src/routes/mod.rs` line 82), under the public rate limiter. Errors use `ApiError` as `app_phones.rs` does.

| Method and path | Auth | Request | Answer | Errors |
|---|---|---|---|---|
| `POST /api/v1/link-boards` | `crate::auth::resolve_user_scoped(&state.db, &headers, "compute")` | `{ "boardId", "name"?, "runtimeDeviceId" }` | `{ "boardId", "relayToken", "runtimeDeviceId" }`. `relayToken` is 64 lowercase hex, shown once. | 400 bad `boardId` (must match `^usba-[0-9a-f]{12}$`). 404 runtime not the caller's. |
| `GET /api/v1/link-boards` | same | | `{ "boards": [{ "boardId", "serial", "name", "runtimeDeviceId", "createdAt", "lastSeenAt", "revokedAt" }] }`. `serial` is `usb:<boardId>`. | |
| `DELETE /api/v1/link-boards/:boardId` | same | | `{ "revoked": true }` | 404 no such board for this account. |
| `POST /api/v1/link-boards/:boardId/socket-ticket` | header `x-allternit-link-board-token: <64 hex>` | `{}` or `{ "path" }` | `{ "ticket", "expiresInSeconds": 30, "runtimeDeviceId", "socketPath" }`. `socketPath` is `/api/v1/runtime-devices/<id>/socket?ticket=<ticket>`. | 401 unknown board, revoked board, bad id, wrong or malformed token (one message for all: `Invalid link board token`). 403 `path` given and not exactly `/api/v1/phones/usba?board=<boardId>`. 503 runtime offline or waking (from `issue_socket_ticket`). |

- Re-posting a board id for the same account rotates the token and clears `revoked_at`. `name`: trimmed, control characters removed, 120 characters at most.
- Ticket route: select every row with that `board_id` and `revoked_at IS NULL`, compare the presented hash with each `token_hash` in constant time, and use the matching row's `account` and `runtime_device_id`. Then call `issue_socket_ticket(&state, &account, &runtime_id, path)` and set `last_seen_at`.
- Export `pub(crate) fn is_link_board_socket_path(path: &str, board_id: &str) -> bool`: exact match only, like `is_app_phone_socket_path`.
- No new relay route. The board uses the existing `GET /api/v1/runtime-devices/:id/socket`.

OpenAPI: `openapi_matches_router` (`cmd/allternit-cloud-api/src/routes/platform_v1/tests.rs` line 745) compares only `/v1/…` routes registered through the platform route table with `openapi/platform-v1.yaml`. The four routes above are `/api/v1/…` on the public router, so no OpenAPI file changes. Do not put them in `platform_v1`. The route tables that must change are in docs: `surfaces/docs/api/cloud-api.mdx` (line 161 area) and `surfaces/docs/api/phone-stream-relay.mdx` (line 125 area).

### 4.3 The socket the board dials

1. `POST https://<relay>/api/v1/link-boards/<boardId>/socket-ticket` with the token header and body `{}`.
2. Open `wss://<relay><socketPath>`.
3. The relay sends one text frame, `{"type":"allternit_socket_ready"}`, when the node has answered. The board waits for it and discards it. Nothing is sent before it.
4. From then on: one WebSocket text frame = one USBA line, with no line ending. Text frames only. The board sends `hello usba/2 …` first, as on TCP.

Board rules: try the LAN first, as today. Use the relay only when `relay` and `rtok` are set and no LAN session formed within 5 s of joining Wi-Fi. Ask for a ticket no more often than once in 5 s, doubling to 60 s after each failure. Stay on a working link until it drops. Report `link=cloud` on a relay session.

Firmware WebSocket client: our own small client (handshake, masked text frames up to 512 bytes, ping and pong, close) over the core's TLS client. No WebSocket library.

### 4.4 The node socket (`cmd/allternit-api/src/phone_usba.rs`, new)

`pub fn phone_usba_router() -> Router<Arc<AppState>>`, merged in `cmd/allternit-api/src/main.rs` after `phone_app_router()` (line 1193). `pub mod phone_usba;` in `src/lib.rs` (line 182 area).

`WS /api/v1/phones/usba?board=<boardId>`:

- It arrives through the relay with the Desktop's access token, like the app-phone sockets. The route needs an `AuthUser`.
- `board` must match `^usba-[0-9a-f]{12}$`, else close 4400.
- The node opens TCP to `127.0.0.1:<port>`. `<port>` is the env `ALLTERNIT_USBA_PORT` or 7300. If the dial fails, close 4503.
- It then copies both ways: each text frame becomes that text plus `\n` on TCP; each TCP line becomes one text frame. A binary frame, or a line or frame over 512 bytes, closes 4400. Either side closing closes the other.
- One socket per account and board id. A new one closes the old one with 4409. At most 8 per account.
- The node does not read, check or change USBA lines, and holds no key. It logs the board id and close codes, never line contents.

### 4.5 Desktop accepts a relayed board

- `UsbAgentLink` needs no new transport. The node's TCP dial is accepted like any other; the handshake decides. A relayed session is told apart by the board's own signed state field `link=cloud`.
- The key lookup is the same. A relayed board that this Desktop did not set up is dropped as `unknown_board`.
- `UsbAgentLink` must be listening whenever this Desktop has a stored board (see open point O-3).

Desktop setup (`UsbAgentManager.setUp` in `phone-usb-agent.ts`), when signed in: call `POST /api/v1/link-boards`, then write `relay` and `rtok` with the other `prov set` lines. When signed out, or the call fails, setup still finishes with the LAN link only and the board status says `relay: 'none'`.

Forget (`UsbAgentManager.forget`): disarm, drop the session, delete the key and calibration as today, and call `DELETE /api/v1/link-boards/:boardId`. At start, Desktop reads `GET /api/v1/link-boards` and forgets any stored board whose cloud row is revoked, so a Forget made on another surface while Desktop was off still removes the key.

The cloud client is injected, so tests fake only the network:

```ts
// surfaces/allternit-desktop/src/main/phone-usb-agent-cloud.ts (new)
export interface LinkBoardCloud {
  register(boardId: string, name: string): Promise<{ relayToken: string; relayHost: string }>;
  revoke(boardId: string): Promise<void>;
  list(): Promise<Array<{ boardId: string; revokedAt: string | null }>>;
}
export function httpLinkBoardCloud(deps: { cloudUrl: () => string; headers: () => Record<string, string>; runtimeDeviceId: () => string | null; fetchImpl?: typeof fetch }): LinkBoardCloud;
```

`UsbAgentDeps` gains `cloud?: LinkBoardCloud`. `StoredUsbAgent` gains `relay?: 'registered'`.

### 4.6 What the relay can and cannot do

The relay is our cloud-api and anything between the board and it.

| The relay can | The relay cannot |
|---|---|
| See the board id, firmware version, reset reason and timing. | Learn K, SK or EK. They never cross the wire. |
| Read every command except typed text: moves, clicks, scrolls, key combos, faces, progress, labels. | Read text sent with `typex`. |
| Read every reply and state line. | Forge or change a command (tag under SK). |
| Drop, delay or cut the connection. The board then releases every key and button within the 10 s lease, and Desktop reports `uncertain`. | Forge or change a reply or a state line on USBA/2 (tags under SK). On USBA/1 they are unsigned; USBA/1 boards have no relay client. |
| Replay an old line. | Have a replay accepted: a command needs a higher `seq`, a state line a higher `n`, and SK is new for every connection. |
| Refuse a ticket, or keep a revoked board out. | Arm a board. Nothing can, except the button. |
| | Reach a Desktop with a board it did not set up: the Desktop key lookup fails. |

Known limits to keep in the docs: key combos are readable (a bot that enters text one key at a time leaks it); the screen stream is a separate channel with its own sealing (`phone_app.rs`).

## 5. Composite lane: board hands, app-phone eyes

### 5.1 Pairing a board to an app phone

- The record is Desktop's stored board. `StoredUsbAgent` (`phone-usb-agent.ts` line 33) gains `pairedPhone?: string`, the app phone's serial (`app:<uuid>`), or absent.
- Set on Desktop only: IPC `phone:usb-pair` `(boardId: string, serial: string | null)`, preload `usbPair(boardId, serial)`. It returns the board's status.
- One board per app phone and one app phone per board. Pairing a second board to a phone unpairs the first.
- Pairing or unpairing clears the board's calibration.
- Nothing proves the board is in that phone until calibration runs: calibration must find the AssistiveTouch pointer in that phone's shared screen.
- The cloud does not store the pairing.

A paired board is not listed as its own phone: `UsbAgentManager.serials()` and `has()` leave it out, and `phone.devices` shows the app phone only. An unpaired board works exactly as today (AirPlay eyes, serial `usb:<boardId>`).

### 5.2 Eyes: the frame source

A paired board's pointer and screen-change check read frames from the app phone's stream, not AirPlay. The AirPlay receiver for that board is not started.

```ts
// surfaces/allternit-desktop/src/main/phone-link-board.ts (new)
export interface LinkFrame { jpeg: Buffer; width: number; height: number; at: number }

/** The slice of a screen the board lane needs. AirplayScreen already has this shape. */
export interface LinkFrameSource {
  state(): 'unavailable' | 'stopped' | 'waiting' | 'mirroring';
  latest(): LinkFrame | null;
  nextFrame(since: number, timeoutMs: number): Promise<LinkFrame | null>;
  settled(quietMs: number, leastMs: number, mostMs: number): Promise<void>;
  start(): void;
  stop(): void;
}
```

- `AppPhoneBackend` (`phone-app-store.ts`) gains `frameSource(serial: string): LinkFrameSource`. It is built on the same viewer, `AppFrameKeeper` and `decodeStill` that `phone.screenshot` uses. While started, it decodes a still whenever the keeper has a newer frame and no decode is running, and keeps the newest as `latest()`. `state()` is `mirroring` while the phone shares its screen and a frame newer than 5 s exists, `waiting` while it shares but no frame has come, `stopped` otherwise.
- `UsbAgentManager` types a board's screen as `LinkFrameSource` (today it is the concrete `AirplayScreen`, line 479) and takes the source from `UsbAgentDeps.eyes?: (pairedPhone: string) => LinkFrameSource | null` for a paired board.
- `phone.screenshot` on a paired app phone returns the frame the pointer uses (the `UsbAgentManager.screenshot` shape: `width`, `height`, `ageMs`), without `coordinate_space: 'none'`, so tap coordinates match.

### 5.3 Hands: who answers which tool

```ts
// phone-link-board.ts
export interface LinkBoardHands {
  /** The board paired to this app phone, or null. */
  boardFor(serial: string): string | null;
  /** Runs a hands tool for a paired app phone. */
  call(name: string, serial: string, args: Record<string, unknown>): Promise<ToolResult>;
  status(serial: string): LinkBoardStatus | null;
}
export function isHandsTool(name: string): boolean;
export class LinkBoardLane implements LinkBoardHands {
  constructor(deps: { usb: () => UsbAgentManager | null; now?: () => number });
}
```

`AppPhoneBackendDeps` gains `hands?: LinkBoardHands`. `AppPhoneBackend` gains `hands(serial: string): boolean` (true when a board is paired, armed or not).

`AppPhoneBackend.call(name, serial, args)`:

| Case | Answer |
|---|---|
| `phone.web.*`, `phone.screen.request`, calendar, reminders, contacts | As today. Never the board. |
| No board paired | As today: `not_supported` with `APP_TAPS_MESSAGE` for taps. |
| Board paired, `isHandsTool(name)` | `hands.call(name, serial, args)`. |

`isHandsTool` is true for every tool `UsbAgentManager.call` handles (its `switch`, lines 325 to 395): `phone.screenshot`, `phone.tap`, `phone.swipe`, `phone.type`, `phone.home`, `phone.back`, `phone.key`, `phone.open_app`, and the ones it answers as unsupported.

`LinkBoardLane.call` runs the same code path as a `usb:` phone (`UsbAgentManager`'s blocker, pointer and `act`). Errors are the board's own: `device_offline` (board not connected), `device_locked`, `no_screen` (with `APP_SHARE_SCREEN_MESSAGE`), `not_calibrated`, `disarmed`, `uncertain`. A paired phone never answers `not_supported` for a tap.

`AppPhoneBackend.devices()`: for a paired phone the `note` says taps work while the board is armed, and why not when they do not.

### 5.4 Disarm stops the run

1. When a paired board's state goes from armed to disarmed (button, Stop, timer, link loss), `LinkBoardLane` marks that phone stopped. Every hands call then answers `disarmed` at once, without sending anything to the board, until the board reports armed again.
2. `cmd/allternit-api/src/phone_task.rs`: a step error that contains `disarmed` ends the run, in the same places `device_locked` does (`locked()`, line 190, and its uses near lines 407, 524 and 834). Status `failed`, result `stopped: the board was disarmed`. No new status value, so `surfaces/docs/api/phone-holds.mdx` does not change. This is true on both phone systems and for a `usb:` phone too.
3. A direct tool caller just gets `disarmed`.
4. On a hold (`approval_required`) the caller's surface shows the item. Desktop sets the board face to `needs_you` when a hold is made for a phone with a board, and back when it is resolved, from the existing `PhoneToolRunner` hold event (`deps` field at `phone-tools.ts` line 158 area). The board offers Stop only.

### 5.5 The one permitted edit in `phone-tools.ts`

Make it on a fresh origin/main, after reading `LANES.md` and the open pull requests of the Phone and Messages lane. It has three parts and nothing else:

1. `AppPhoneLane` (line 69): add `hands?(serial: string): boolean;`.
2. `PhoneToolRunner.dispatch` (line 858), and only this function:
   - After `resolveSerial`: `const hands = apps?.hands?.(serial) === true;`
   - The guard call (line 885) passes `hands ? { usb: apps, links, apps: null } : { usb, links, apps }`. With that, the unchanged `screenFacts` (line 1403) takes one screenshot for the guard through `lanes.usb.call('phone.screenshot', …)`, as it does for a `usb:` phone. `guardCheck`, `screenFacts` and `GUARDED_TOOLS` are not edited.
   - In the `if (apps?.has(serial))` block (line 889): when `hands`, `phone.task` returns `this.task(serial, a, apps, links)` and `phone.run_skill` returns `this.skillCall(name, { ...a, serial })`. Every other call still goes to `apps.call`; when `hands`, its result is wrapped in `withHints(name, a, …)`.
3. The `phone.devices` description (line 229): one clause saying an app iPhone with a paired, armed Link board also takes taps.

The guard runs before any lane, on line 884, for every tool in `GUARDED_TOOLS`. Do not add a second check and do not skip that one.

### 5.6 Faces Desktop sends

| When | Face | Prog |
|---|---|---|
| A tool call or task step starts on a phone with a board | `working` | step count mapped to 1 to 5, when known |
| A run ends well | `wink`, then `default` after 1 s | `0` |
| A hold is waiting | `needs_you` | unchanged |
| Idle | `default` | `0` |

Face commands are best effort: a failure never fails the tool call. They are sent only on USBA/2 sessions.

## 6. Status model for every surface

One shape. Desktop builds it; every other surface reads it.

```ts
// phone-link-board.ts
export type LinkBoardState =
  | 'board_offline' | 'phone_not_connected' | 'phone_locked'
  | 'no_screen' | 'not_calibrated' | 'disarmed' | 'ready';   // the existing UsbAgentStatus values

export interface LinkBoardStatus {
  boardId: string;                      // "usba-0123456789ab"
  serial: string;                       // "usb:<boardId>"
  name: string;
  online: boolean;                      // a session with Desktop exists
  armed: boolean;
  status: LinkBoardState;
  link: 'lan' | 'cloud' | null;         // null when offline or USBA/1
  usb: 'mounted' | 'asleep' | 'none' | null;
  hid: 'usb' | 'ble' | 'none' | null;
  dock: boolean | null;
  face: FaceName | null;
  prog: number | null;                  // 0 to 5
  eyes: 'airplay' | 'app';
  phone: string | null;                 // paired app phone serial, "app:<uuid>"
  screen: boolean;                      // the eyes have a live picture
  calibrated: boolean;
  firmware: string | null;
  protocol: 1 | 2 | null;
  firmwareUpdate: boolean;
  relay: 'registered' | 'none';
}
```

`UsbAgentInfo` (`phone-usb-agent.ts` line 82) becomes `LinkBoardStatus` plus its Desktop-only fields (`address`, `mirroring`, `receiverName`). All additions are optional for older readers.

Carriers:

| Reader | Carrier |
|---|---|
| Desktop UI | The existing IPC `phone:usb-status` (`phone-ipc.ts` line 329, preload `usbStatus`): `boards` carries the new fields. Stop: existing `phone:usb-stop`. Forget: existing `phone:usb-forget`. Pair: new `phone:usb-pair`. |
| allternit-api | Desktop's phone gateway (`phone-gateway.ts`, loopback, token header, file `~/.allternit/phone-gateway.json`). New routes: `GET /phone/boards` → `{ "boards": LinkBoardStatus[] }`; `POST /phone/board/stop` `{ "boardId" }` → a tool result (`{ "ok": true }` or `{ "ok": false, "error", "message" }`); `POST /phone/board/forget` `{ "boardId" }` → `{ "ok": true }`. `PhoneGatewayDeps` gains `boards?: { list(): LinkBoardStatus[]; stop(boardId: string): Promise<ToolResult>; forget(boardId: string): Promise<void> }`. |
| Web, phone layout, PWA, iOS, `gizzi phone` | allternit-api, new routes in `phone_usba.rs`, reached the way each surface already reaches `GET /api/v1/phones`: `GET /api/v1/phones/boards` → `{ "desktop": true, "boards": LinkBoardStatus[] }`, or `{ "desktop": false, "boards": [] }` when the gateway is not running. `POST /api/v1/phones/boards/:boardId/stop` → 200 `{ "ok": true }`; 409 `{ "error": "board_offline" }`; 502 `{ "error": "uncertain" }`; 503 `{ "error": "desktop_offline" }`. `DELETE /api/v1/phones/boards/:boardId` → 200 `{ "ok": true }`; 503 `desktop_offline`. Error bodies are `{ "error", "message" }`. |
| Forget from any surface | First `DELETE /api/v1/link-boards/:boardId` on the cloud (works with Desktop off). Then, best effort, `DELETE /api/v1/phones/boards/:boardId` on the node. |

`GET /api/v1/phones` does not change. Setup and pairing stay on Desktop.

## 7. Dock contacts and hardware notes

### 7.1 Dock contact list

One recessed block on the flat underside, between the feet. Pads on the bot are never live. Spring pins are on the bank.

| # | Name | Direction | Use |
|---|---|---|---|
| 1 | `VDOCK` | bank → bot | Power in. Off until the bank detects the bot. |
| 2 | `GND` | common | Ground. |
| 3 | `DOCK_TX` | bot → bank | Serial: state, face, prog, label. |
| 4 | `DOCK_RX` | bank → bot | Serial from the bank. Never carries a command that arms or moves. |
| 5 | `STOP_N` | bank → bot | Stop line, active low. Wired to the bank's stop button and, separately, to the bank controller. |

- Detect rule, bank side (proposed, open): the bot carries an identity resistor from `DOCK_RX` to `GND`. The bank reads it through a weak pull-up before it switches `VDOCK` on, and needs a steady in-range reading for 200 ms. No reading, no power.
- Detect rule, bot side: `dock=1` while the `DOCK_DETECT_PIN` input (sensing `VDOCK`) has been active for 100 ms. Build flag, default none.
- Stop line: `STOP_N` low for 40 ms or more disarms, exactly like a button press that disarms. It never arms. Build flag `STOP_PIN`, default none. It works with the bank's software hung, because the button is wired straight to the line.
- Keyed magnets set the direction. Their place and strength are open.

### 7.2 Hardware notes skeleton

Every line is `open`. Nothing has been measured or chosen. Change a line to `chosen` or `measured` only with the evidence next to it. No prices here.

| Area | Item | State | Note |
|---|---|---|---|
| Bot electronics | Controller module (small pre-certified ESP32-S3 with antenna and native USB) | open | Dev stage: XIAO ESP32S3 class. |
| Bot electronics | USB-C plug: device only, CC pull-downs, ESD on the data pins | open | |
| Bot electronics | Plug shoulder length for phone cases | open | |
| Bot electronics | Face display part | open | Candidate: 1.54 inch 200 × 200 mono, SSD1681 class, partial refresh. None on hand. |
| Bot electronics | Tinted anti-glare window and printed `A://` | open | |
| Bot electronics | No battery in the bot | open | Design rule from the spec; to confirm in the audit. |
| Bot electronics | Arm and stop button place (Gizzi: under an ear nub) | open | |
| Bot electronics | Hardware switch that cuts the USB data lines when stopped | open | Required for a product board. |
| Bot electronics | Dock contact block: pad size, pitch, plating, recess | open | Section 7.1. |
| Bot electronics | Dock identity resistor value and bank read window | open | |
| Bot electronics | Antenna place and keep-out; no metallic finish on any shell | open | |
| Bot electronics | Current drawn from the phone | open | Spec step 1 records it. |
| Bot electronics | Surface temperature in a sealed shell | open | |
| Bot electronics | Core cartridge outline (the datum for every shell) | open | |
| Bot electronics | Pin map (button, display, dock detect, stop) | open | Build flags in sections 3.5 and 7.1. |
| Bot electronics | Automatic shell tag | open | Audit item. |
| Power bank | Certified bank from a battery maker as the base | open | |
| Power bank | Cable: charge only in the first design | open | |
| Power bank | Data through the cable (role-swap power-delivery controller) | open | Only if Bluetooth fails spec step 7. |
| Power bank | Screen (about 3 inch capacitive touch) | open | |
| Power bank | Bank controller (second ESP32-S3 class module) | open | |
| Power bank | Bot to bank link: serial docked, Bluetooth undocked; pairing by dock and button | open | |
| Power bank | Stop button wiring: to the controller and straight to `STOP_N` | open | |
| Power bank | Dock pocket place, magnets, keying | open | |
| Compliance | Battery: UN 38.3, IEC 62133-2 class, UL 2056 class, EU battery regulation, labels, airline watt-hour limit | open | |
| Compliance | Radio: module approvals (FCC, ISED, CE RED, UKCA) and emissions tests of the finished bot and bank | open | |
| Compliance | Bluetooth qualification and listing | open | |
| Compliance | USB: our own vendor and product IDs; USB-C and power-delivery behaviour; whether Apple's accessory programme applies | open | |
| Compliance | Product safety: touch temperature, plug as a lever on the port, drop and crush of the e-paper glass, small parts, age grading | open | |
| Compliance | Materials and waste: RoHS, REACH, WEEE, California Proposition 65 | open | |
| Compliance | Stores: App Review and Play policy for an app paired with an accessory that operates the phone | open | |
| Compliance | Names: "Link" next to Allternit Phone Link and other companies' "Phone Link" marks | open | |
| Security | Signed two-slot updates with rollback | open | Spec step 8; waits for a board. |
| Security | Secure boot, flash encryption, encrypted storage | open | Spec step 8; needs Eoj's OK for one board. eFuses cannot be undone. |

## 8. Open points

Each has a default so no builder is blocked. Report what you find to the orchestrator; do not change the contract yourself.

| # | Point | Read | Default until settled |
|---|---|---|---|
| O-1 | Does the relay allow `/api/v1/phones/usba` as a socket path and `/api/v1/phones/boards` as a proxied path, and which capability do they need? | `is_allowed_runtime_path` and `required_capability` in `cmd/allternit-cloud-api/src/routes/runtime_relay.rs` (lines 1432 to 1540) | They fall under the same rule as `/api/v1/phones/app/…`. If not, add the two paths to that rule with the same capability, and add a unit test next to the existing ones. |
| O-2 | Relay socket and bandwidth quotas for a board that stays connected all day with a 3 s heartbeat. | `quota_service.check_relay_socket_allowed` and `open_relay_socket` (called at `runtime_relay.rs` lines 625 to 633) | No change. Record the limits in the plan's risks. |
| O-3 | Does Desktop start `UsbAgentLink` at launch when boards are stored, and is `linkPort` ever set in production? | `ensureUsbReady` in `surfaces/allternit-desktop/src/main/phone-service.ts` (line 1308) | Production uses 7300. Start the listener at launch when a board is stored. |
| O-4 | Which credential Desktop's main process uses for cloud-api calls. | `phone-service.ts`, `routes/registered_phones.rs` | `httpLinkBoardCloud` takes `headers()` from the caller. If Desktop has only a runtime device credential, `link_boards.rs` accepts it the way `registered_phones.rs` does, in addition to `resolve_user_scoped`. |
| O-5 | The TLS trust anchor for the relay host on the board, and whether the core's mbedTLS has AES-GCM for `typex`. | arduino-esp32 core 3.3.12 | Use the core's CA bundle and `mbedtls/gcm.h`. Never run on a board; stays unverified until parts arrive. |
| O-6 | Decode cost and frame rate of the app stream as pointer eyes. | `AppFrameKeeper` and `screenshot` in `phone-app-store.ts` (lines 326 to 766); spec step 2 on the real iPhone | Decode on demand, newest frame wins. Calibration and aiming timeouts stay as they are. |
| O-7 | Does the node task loop drive a screenshot-only phone end to end? | `cmd/allternit-api/src/phone_task.rs` (line 398 loop) | Yes, as for a `usb:` phone. Never proven on a real phone. |
| O-8 | Does a cloud computer run the Desktop phone lane with no screen? | Not found in the files read | No. A board needs Allternit Desktop running. Say so in the guide. |
| O-9 | Face geometry against the character sheet and the real window. | `Marketing/Brand Kit And Memory/GIZZI-CHARACTER.md` | The rectangles in section 3.2. |
| O-10 | Is `arduino-cli` with esp32 core 3.3.12 installed on this Mac? | `hardware/usb-agent/build-firmware.sh`, `flash.sh` | If not, the firmware lane stops before the image rebuild and asks the orchestrator. |
| O-11 | Exact pins for the display, dock detect and stop inputs on the dev board. | The board, when it arrives | All default to none. The shipped image has no display. |
