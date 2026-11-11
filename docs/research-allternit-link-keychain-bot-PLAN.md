# Allternit Link — Phase 1 build plan

Written 2026-11-10 against origin/main `976ef690d`. Spec: `Research/specs/allternit-link-keychain-bot.md` in the Brain (approved by name). Contract: `hardware/link/README.md`. Read the contract before any lane starts.

Goal: build every Phase 1 software step now against the simulated board, on `integ/link-phase1`, so that the day a real board and Eoj's iPhone are free, only the hardware tests remain.

Out of scope: a custom circuit board, the power bank, bank screen firmware, moulded shells, phone-only Bluetooth setup, compliance testing, prices, any sale. Spec steps 1, 7 and 8 wait for a board. Nothing merges to main.

## Surface Matrix

| Surface | Status | What |
|---|---|---|
| Desktop | BUILD | Board card shows link (Wi-Fi or cloud), face, progress, dock, relay state. A picker pairs a board with an app iPhone. A paired app iPhone shows "Screen and taps". Setup, pairing and calibration stay here (they need USB and the board key). |
| ai.allternit.com web + phone layout | BUILD | Board rows with state, Stop and Forget in the phones list and live view, from `GET /api/v1/phones/boards`. Phone sizing in the phone layout. Setup and pairing: N/A, a browser has no USB path to the board key. |
| m.allternit.com PWA | BUILD | The same rows, Stop and Forget, coarse-pointer sizing. Setup: N/A, phone browsers have no USB serial. |
| iOS | BUILD | Existing Settings → Phones screen (`AppPhonesSettingsView.swift`), no new rail tab: board attached, armed or not, AssistiveTouch steps, Stop. Hold review: HAS (`PhoneHoldReviewView.swift`). |
| Android | HAS (via PWA) | Rows arrive with the PWA. Taps: HAS in `PhoneLinkAccessibilityService.kt`, so board hands are N/A there (Android already taps in software). The disarm-stops-the-run rule is built in the node and covers Android. One check on a real Android phone is in Deferred acceptance. |
| Gizzi Code | BUILD | `gizzi phone boards` lists each board's armed, link, dock and face state. `gizzi phone stop` disarms one. Holds: HAS (`cmd/gizzi-code/src/cli/commands/phone.ts`). |
| Backend | BUILD | USBA/2 and `ReferenceBoard`; cloud-api `link_boards` routes and table; allternit-api `phone_usba.rs`; Desktop phone gateway routes; the node task loop ends a run on `disarmed`. The contract lands first. |
| Docs | BUILD | USB Agent guide, new Allternit Link guide, new API page, the two API route tables, nav, dependency map. `check_links.py` at 0. |

platform.allternit.com and the websites: N/A, nothing here touches accounts, billing or public pages.

The dependency map does have entries for `hardware/usb-agent` (`docs/dependency-map/features.json` line 2128 area). The spec said it had none. The docs lane updates it.

## Hard rules (every lane)

1. Do not edit `cmd/allternit-api/src/phone_guard.rs`, `surfaces/allternit-desktop/src/main/phone-hold-client.ts`, or any hold screen. The Phone and Messages lane owns them. Read `~/Desktop/allternit-workspace/LANES.md` first. Use `surfaces/docs/api/phone-holds.mdx`. Every tap goes through the existing tool runner, so the guard applies with no new code.
2. The board boots disarmed. Only its button arms it. Software can only disarm. No protocol message may arm a board. Face, progress and label messages are allowed while disarmed because they move nothing.
3. No GPL code in firmware. `ReferenceBoard` changes in the same commit as the firmware. When firmware changes, rebuild the shipped image with `hardware/usb-agent/build-firmware.sh` and commit both files.
4. No merge to main, no deploy, no parts bought, no eFuse burn. Lanes branch from `integ/link-phase1` and merge back into it. Draft pull requests are allowed.
5. Smoke-boot allternit-api before any route merge. Production cloud-api does not apply migrations itself (`ALLTERNIT_SKIP_MIGRATIONS=1`): the new file is applied at deploy from the `ops_applied_migrations` ledger, or by hand. Nothing is applied in this phase.
6. Tests must be able to fail and must assert what the user gets. No filler tests. Do not mock our own code.
7. Rust is capped machine-wide. In the Agent Tree run `eval "$(gizzi agents tree env)"`. Use `cargo check` while iterating. Only the relay lane runs cargo. The orchestrator runs cargo only after that lane has stopped.
8. Stop every dev server and test watcher you start. Delete build output when you finish.

## Design decisions

| Decision | Reason | Rejected |
|---|---|---|
| USBA/2 binds the version into the handshake labels (`srv2`, `brd2`, `ses2`). | A relay that rewrites `usba/2` to `usba/1` would strip the new protections. | Same labels as USBA/1. |
| USBA/2 signs replies and state lines. | USBA/1 leaves them unsigned (`phone-usb-agent-protocol.ts` lines 87 to 99). With a relay in the path, a forged `r … ok` would make Desktop believe a tap happened. | Sign commands only, as the spec text implied. |
| Typed text goes as `typex` (AES-256-GCM) on every USBA/2 session. | The relay reads USBA lines. Text a bot types can be a password. | Seal everything (lines no longer fit 512 bytes and the protocol stops being an extension); or seal nothing. |
| The node pipes the relayed socket to Desktop's existing TCP listener on loopback. | A relayed socket ends in allternit-api (the relay dials `ws://127.0.0.1:8013{path}`). The pipe needs no new transport in `UsbAgentLink`, and the handshake stays end to end. | A second WebSocket from Desktop to the node carrying many boards (new transport, reconnect logic). |
| `link_boards` is keyed by `(account, board_id)`. | Board ids come from the MAC address and can be guessed. | `board_id` as a global key with a 409, as `app_phones` does with random ids. |
| The pairing of a board with an app phone lives in Desktop's stored board (`pairedPhone`). | Desktop runs the lane. A board with no cloud row can still be paired. The cloud learns less. | A column on the cloud row. |
| The composite lane reuses `UsbAgentManager` with a different frame source. | One pointer, one calibration, one set of safety blockers. | A second pointer implementation for app phones. |
| Other surfaces read board state from the node, which asks Desktop's phone gateway. | `GET /api/v1/phones` lists no USB boards today, and the UI reads them only through Desktop IPC. A pull has no stale copy. | Desktop pushing state to the node on a timer. |
| A disarm ends a run in the node task loop (`failed`, "stopped: the board was disarmed"). | The loop stops only on `device_locked` today. Without this a model retries until the step limit. | A new run status (it would change the hold contract page another lane owns). |
| Our own SSD1681-class driver behind `LinkDisplay`; faces rendered by one pure function in C++ and TypeScript. | Common e-paper libraries are GPL or have no licence. The panel may change. With no panel on hand, the twin renderer is the only thing that can be tested. | A third-party display library. |

## What can run now, and what waits

| Step | What | Now, on the simulated board | Waits for |
|---|---|---|---|
| 1 | Real-iPhone test of the shipped USB Agent | no | the board, and Eoj with his iPhone |
| 2 | Real-iPhone test of the app's screen share | no | Eoj with his iPhone (no board) |
| 3 | Contract | done: `hardware/link/README.md` | |
| 4 | Composite lane | yes: lane C against `ReferenceBoard` and a fake app stream | the alarm task on the real iPhone: board + iPhone |
| 5 | Board through the cloud relay | yes: cloud-api, node, Desktop; firmware client is compiled only | the hotspot test: board |
| 6 | Face | yes: fake display and the host renderer test | the photo: panel + board (parts) |
| 7 | Bluetooth mouse spike | no | board + iPhone |
| 8 | Hardening | no | a sacrificial board and Eoj's OK for it |
| 9 | Desktop, web and PWA rows | yes: lane U with fixtures | screenshots with a real board: board |
| 10 | iOS rows | yes: lane I in the simulator | |
| 11 | Gizzi Code | yes: lane G | |
| 12 | Docs | yes: lane DOC | |
| 13 | Hardware notes | skeleton done in the contract, every line `open` | measured values: parts |
| 14 | Final checks | yes, for software: the orchestrator's combined check | acceptance boxes that need hardware |

## Lane 0 — scaffold (types only)

- Branch: `link/p1-scaffold`. Model: sonnet. Contract: sections 2.10, 5.2, 5.3, 6, 4.5.
- May change: `surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts` (types and the three new `encodeCommand` cases only).
- May create: `surfaces/allternit-desktop/src/main/phone-link-board.ts` (types only: `LinkFrame`, `LinkFrameSource`, `LinkBoardHands`, `LinkBoardState`, `LinkBoardStatus`), `surfaces/allternit-desktop/src/main/phone-usb-agent-cloud.ts` (the `LinkBoardCloud` interface only).
- Must not touch: anything else.
- Tasks:
  1. Add `UsbaVersion`, `FaceName`, `FACE_NAMES`, `HidTransport`, `LinkKind`; the optional `BoardState` fields; the three `BoardCommand` members; `encodeCommand` cases (`face <name>`, `prog <n>`, `label <hex>` or `label -`) with the bounds in contract section 2.6.
  2. Create the two new files with the exact names and fields from the contract. No behaviour, no stubs that return fake data.
  3. Add bounds assertions for the three new commands to the existing test "bounds every command" in `phone-usb-agent-protocol.test.ts`.
- Proof: `pnpm install` at the tree root (first run only; a few minutes), then in `surfaces/allternit-desktop`: `pnpm run typecheck` and `npx vitest run src/main/phone-usb-agent-protocol.test.ts src/main/phone-usb-agent-link.test.ts`. Memory note `allternit-typecheck-tsrs.md` says typecheck uses tsrs; read it before running.
- Done: typecheck clean, both test files pass, merged into `integ/link-phase1`. After this, lane F owns `phone-usb-agent-protocol.ts`, lane C owns `phone-link-board.ts`, lane D owns `phone-usb-agent-cloud.ts`.

## Lane F — USBA/2, face, firmware (spec steps 6 and the firmware half of 5)

- Branch: `link/p1-usba2-face`. Model: opus (protocol authentication, more than four files). Contract: sections 2, 3, 4.3, 7.1.
- Owns: `hardware/usb-agent/firmware/usb_agent/usb_agent.ino`, `phone-usb-agent-protocol.ts`, `phone-usb-agent-link.ts`, `phone-usb-agent.testkit.ts`. No other lane edits these four.
- May create: `hardware/usb-agent/firmware/usb_agent/face.h`, `display.h`, `relay_link.h`; `hardware/usb-agent/firmware/host/face_dump.cpp`; `surfaces/allternit-desktop/src/main/phone-usb-agent-face.ts`; `surfaces/allternit-desktop/src/main/phone-usb-agent-face.test.ts`.
- May change: `phone-usb-agent-protocol.test.ts`, `phone-usb-agent-link.test.ts`, `surfaces/allternit-desktop/resources/phone/usb-agent/firmware.json`, `usb-agent-firmware.bin`, `hardware/usb-agent/README.md`, `hardware/usb-agent/NOTICE`, `hardware/usb-agent/build-firmware.sh` (only if the new headers need it).
- Must not touch: `phone-usb-agent.ts`, `phone-app-store.ts`, `phone-tools.ts`, `phone-link-board.ts`, any Rust.
- Tasks, in order:
  1. Check tools: `arduino-cli version`, `arduino-cli core list` (needs `esp32:esp32` 3.3.12, the version in `firmware.json`), `c++ --version`. If arduino-cli or the core is missing, keep going with tasks 2 to 5 and tell the orchestrator before task 6 (contract O-10).
  2. `phone-usb-agent-face.ts`, `face.h`, `host/face_dump.cpp`: the renderer twice, and the host check. Read `GIZZI-CHARACTER.md` first and report if the geometry in contract section 3.2 breaks it (O-9).
  3. `phone-usb-agent-protocol.ts`: USBA/2 parse and key functions, `sealType`, and `ReferenceBoard` at version 2 with faces, progress, label, signed replies and state, `typex`, `pressButton()`, `pressStop()`, `FakeDisplay`. Keep a version 1 `ReferenceBoard` working.
  4. `phone-usb-agent-link.ts`: session `version`; version 2 proofs; verify reply and state tags; ignore a state line whose `n` did not rise; swap `type` for `typex` on version 2; refuse `face`, `prog`, `label` on version 1 without sending; end reason `bad_tag`.
  5. Firmware: version 2 hello and labels; signed replies and state with the new fields; `face`, `prog`, `label` above the armed check in `run()`; `typex`; `display.h` with `NullDisplay` and `Ssd1681Display` and a refresh that never blocks the loop; `DOCK_DETECT_PIN` and `STOP_PIN` (default none; stop only disarms); the faces the firmware sets itself; the `prov` keys; saved networks; `relay_link.h` (ticket request, our own WebSocket client, `link=cloud`). `FIRMWARE_VERSION "1.1.0"`. Tasks 3 and 5 land in one commit.
  6. Rebuild the image with `hardware/usb-agent/build-firmware.sh`. Commit `usb-agent-firmware.bin` and `firmware.json` with the source.
  7. `hardware/usb-agent/README.md`: protocol section and safety table rows for USBA/2. `NOTICE` only if code was taken from somewhere.
- Tests that must exist and must be able to fail:
  - Protocol: after a fresh `hello`, no sequence of commands leaves the board armed; `face`, `prog`, `label` answer `ok` while disarmed and record nothing in `log`; a reply with a changed byte fails `parseReply`; a recorded state line replayed into a new session fails; `typex` opens to the same text and a wrong key answers `err bad text`; a version 1 board still completes the handshake and runs a command.
  - Link: a version 1 and a version 2 `SimBoard` are both accepted by one `UsbAgentLink`; a forged state line ends the session and a pending command resolves `uncertain`; `face` on a version 1 session returns `invalid` and the board receives no line.
  - Face: for every face, prog 0 to 5, armed and not, the C++ output equals `renderFace`; the keep-out area is white; `prog 3` on `working` has three solid squares.
  - Firmware: the existing `phone-usb-agent-firmware.test.ts` passes with the new image.
- Proof: in `surfaces/allternit-desktop`: `npx vitest run src/main/phone-usb-agent-protocol.test.ts src/main/phone-usb-agent-link.test.ts src/main/phone-usb-agent-face.test.ts src/main/phone-usb-agent-firmware.test.ts src/main/phone-usb-agent.test.ts` and `pnpm run typecheck`. `hardware/usb-agent/build-firmware.sh` prints the new version and hash. No cargo.
- Done: the five test files pass, the image is rebuilt, and the lane report says plainly: "firmware compiled, never run on a board".

## Lane R — cloud-api and allternit-api (the Rust half of spec step 5, and the run-stop rule)

- Branch: `link/p1-relay`. Model: opus (auth, a migration, more than four files). Contract: sections 4.1, 4.2, 4.4, 4.6, 5.4 item 2, 6.
- This is the only lane that runs cargo.
- May create: `cmd/allternit-cloud-api/src/routes/link_boards.rs`, `cmd/allternit-cloud-api/migrations_pg/121_link_boards.sql`, `cmd/allternit-cloud-api/tests/link_boards.rs`, `cmd/allternit-api/src/phone_usba.rs`.
- May change: `cmd/allternit-cloud-api/src/routes/mod.rs` (line 82 area), `cmd/allternit-cloud-api/src/lib.rs` (line 561 area), `cmd/allternit-cloud-api/src/services/account_erase.rs` (line 120 area), `cmd/allternit-cloud-api/src/routes/runtime_relay.rs` (only the path rule, only if O-1 needs it), `cmd/allternit-api/src/lib.rs` (line 182 area), `cmd/allternit-api/src/main.rs` (line 1193 area), `cmd/allternit-api/src/phone_task.rs` (the `disarmed` rule only).
- Must not touch: `phone_guard.rs`, `phone_app.rs`, `phone_ws.rs`, `phone_replay.rs`, `app_phones.rs`, anything under `platform_v1/`, any TypeScript.
- Tasks, in order:
  1. Read `is_allowed_runtime_path` and `required_capability` in `runtime_relay.rs` and settle O-1. Read the quota calls and note the limits (O-2).
  2. Check the migration number: `ls cmd/allternit-cloud-api/migrations_pg | tail -3` on fresh origin/main, and open pull requests. Write the migration.
  3. `link_boards.rs`: the four routes, `is_link_board_socket_path`, constant-time token match. Wire it in `mod.rs` and `lib.rs`. Add the erase list entry.
  4. `phone_usba.rs`: the board socket and the TCP pipe; `GET /phones/boards`, `POST /phones/boards/:board_id/stop`, `DELETE /phones/boards/:board_id` through the phone gateway file (read `~/.allternit/phone-gateway.json` or `ALLTERNIT_PHONE_GATEWAY_FILE` in this file; do not edit `phone_replay.rs`). Wire it in `lib.rs` and `main.rs`.
  5. `phone_task.rs`: a step error containing `disarmed` ends the run as `failed` with "stopped: the board was disarmed".
  6. Smoke boot (below).
- Tests that must exist and must be able to fail:
  - Cloud unit: the ticket path matches only the exact path for that board (wrong board, extra query key, upper case, `..` all refused); a 31-byte token is refused.
  - Cloud with a database (`tests/link_boards.rs`, through `tests/common`): register returns a token once; a wrong token answers 401; the right token gets past auth (the answer is 503 "offline", not 401, with no runtime connected); after DELETE the right token answers 401; a second account registering the same board id gets its own row and cannot use the first account's token; account erase removes the row.
  - Node: lines pass both ways through the pipe against a real local TCP listener; a line over 512 bytes closes 4400; no listener closes 4503; a second socket for the same board closes the first with 4409; `GET /phones/boards` with no gateway file answers `{ "desktop": false, "boards": [] }`.
  - Task loop: a step that fails with `disarmed` ends the run with status `failed` and no further step is asked for.
- Proof: `eval "$(gizzi agents tree env)"`; `cargo check -p allternit-cloud-api`; `cargo test -p allternit-cloud-api link_boards`; `cargo check -p allternit-api`; `cargo test -p allternit-api phone_usba`; `cargo test -p allternit-api phone_task`. Take the package names from each crate's `Cargo.toml`. The database tests need `TEST_DATABASE_URL` pointing at a local Postgres; without one they run in CI on the draft pull request, and the report says so. Cost: the first check of each crate in a new tree can take tens of minutes on the capped Mac (not measured).
- Smoke boot: `cargo build -p allternit-api --bin allternit-api`; run it for about 20 s with a new temp `HOME` and data directory and `ALLTERNIT_API_PORT` unset; confirm no startup panic (an overlapping route panics here, not in unit tests); kill the `gizzi-code fabric-worker` it started.
- Done: all commands pass, the smoke boot output is in the report, a draft pull request is open against `integ/link-phase1`.

## Lane C — composite lane (spec step 4)

- Branch: `link/p1-composite`. Model: opus. Contract: section 5, and section 6 for `LinkBoardStatus`.
- Owns: `surfaces/allternit-desktop/src/main/phone-link-board.ts`, `phone-app-store.ts`, `phone-usb-agent.ts`, `phone-tools.ts` (the one permitted edit), `phone-service.ts`.
- May create: `phone-link-board.test.ts`, `phone-link-board.testkit.ts`.
- May change: `phone-app-store.test.ts`, `phone-usb-agent.test.ts`, `phone-tools.test.ts`, `phone-tools-hold.test.ts`.
- Must not touch: the four files lane F owns, `phone-hold-client.ts`, `phone-gateway.ts`, `phone-ipc.ts`, `src/preload/`, any Rust. In `phone-tools.ts`: only the three parts in contract section 5.5; not `guardCheck`, `screenFacts` or `GUARDED_TOOLS`.
- Tasks, in order:
  1. Read `LANES.md` and the Phone and Messages handoff named there. `git fetch` and confirm `integ/link-phase1` holds the newest origin/main before the `phone-tools.ts` edit.
  2. `phone-usb-agent.ts`: type a board's screen as `LinkFrameSource`; `StoredUsbAgent.pairedPhone`; `pair(boardId, serial | null)` (one to one, clears calibration); `UsbAgentDeps.eyes`; paired boards left out of `serials()` and `has()`; `info()` returns the `LinkBoardStatus` fields.
  3. `phone-app-store.ts`: `frameSource(serial)`, `hands(serial)`, the `call` routing table from contract section 5.3, the screenshot shape for a paired phone, the device note.
  4. `phone-link-board.ts`: `isHandsTool`, `LinkBoardLane` (routing, the stopped mark after a disarm, `status`), and the faces Desktop sends (contract section 5.6), best effort.
  5. `phone-tools.ts`: the one edit.
  6. `phone-service.ts`: pass `hands` to the app backend (line 923) and `eyes` to the USB manager (line 1312).
- Tests that must exist and must be able to fail (real `PhoneToolRunner`, real `UsbAgentManager`, real `UsbAgentLink`, a `SimBoard`, a fake node and stream at the network edge):
  - Paired and armed: `phone.tap` on the `app:` serial returns `ok`, and the board's log shows a move and a click.
  - Not paired: the same call answers `not_supported` with `APP_TAPS_MESSAGE` (today's behaviour stays).
  - Paired and disarmed: `disarmed`. Paired and offline: `device_offline`.
  - Disarm in the middle of a swipe: the call answers `disarmed`; the next hands call answers `disarmed` and the board receives no new line; after arming again, calls work.
  - Guard: with a guard that answers `hold`, the tap returns `approval_required`, the board's log is empty, and the body the guard received carries a screenshot.
  - `phone.devices` lists a paired pair as one phone. `phone.task` on a paired phone is sent to the node task route, not answered `not_supported`.
  - An unpaired `usb:` board with AirPlay behaves as before (the existing `phone-usb-agent.test.ts` passes unchanged in meaning).
- Proof: in `surfaces/allternit-desktop`: `npx vitest run src/main/phone-link-board.test.ts src/main/phone-app-store.test.ts src/main/phone-usb-agent.test.ts src/main/phone-tools.test.ts src/main/phone-tools-hold.test.ts` and `pnpm run typecheck`. No cargo.
- Done: the five files pass; `git diff integ/link-phase1 -- surfaces/allternit-desktop/src/main/phone-tools.ts` shows only `AppPhoneLane`, `dispatch` and the one description line.

## Lane D — Desktop setup, cloud registration and status carriers (the Desktop half of spec step 5)

- Branch: `link/p1-desktop-status`. Model: opus (it handles the relay token). Starts after lanes C and F are merged into `integ/link-phase1`. Contract: sections 2.9, 4.5, 5.1, 6.
- Owns: `phone-usb-agent-cloud.ts`, `phone-usb-agent-provision.ts`, `phone-gateway.ts`, `phone-ipc.ts`, `src/preload/index.ts`. Follows lane C in `phone-usb-agent.ts` and `phone-service.ts` (lane C is finished by then).
- May create: `phone-usb-agent-cloud.test.ts`.
- May change: `phone-gateway.test.ts`, `phone-usb-agent.test.ts`.
- Must not touch: lane F's four files, `phone-tools.ts`, `phone-hold-client.ts`, any Rust.
- Tasks, in order:
  1. Settle O-3 and O-4 by reading `phone-service.ts`.
  2. `httpLinkBoardCloud`.
  3. Provisioning: the `relay`, `rtok` and extra network lines. `setUp` registers the board when signed in and still finishes on the LAN when not. `forget` revokes. At start, forget boards whose cloud row is revoked. The relay token is never written to a log or to disk on the computer.
  4. Gateway routes `GET /phone/boards`, `POST /phone/board/stop`, `POST /phone/board/forget`.
  5. IPC `phone:usb-pair`, preload `usbPair`, and the new fields in the `phone:usb-status` snapshot.
- Tests that must exist and must be able to fail: setup with a cloud that answers writes `relay` and `rtok` to the serial port and marks the board `relay: 'registered'`; setup with a cloud that fails still returns a working board with `relay: 'none'`; forget calls revoke; a board revoked in the cloud is gone after start; the gateway refuses the three routes without the token and returns the board list with it; `httpLinkBoardCloud` against a local HTTP server sends the right method, path and body.
- Proof: `npx vitest run src/main/phone-usb-agent-cloud.test.ts src/main/phone-gateway.test.ts src/main/phone-usb-agent.test.ts` and `pnpm run typecheck`.
- Done: tests pass, merged into `integ/link-phase1`.

## Lane U — Desktop, web, phone layout and PWA rows (spec step 9)

- Repo: `~/Desktop/allternit-workspace/allternit-ai`. The orchestrator creates `integ/link-phase1` there from fresh origin/main, in a clean worktree. Branch: `link/p1-ui`. Model: sonnet. Contract: section 6.
- May change: `src/components/computers/UsbAgentSection.tsx`, `src/components/computers/PhonesSection.tsx`, `src/lib/phones-bridge.ts`, `src/fabric-session/phone/screens/LinkedPhonesSection.tsx`, their test files. A new small client file for `GET /api/v1/phones/boards` is allowed next to the code that already reads `GET /api/v1/phones`.
- Tasks: read `/Users/joe/.agents/skills/libraries-dev/SKILL.md` and the real components first. Add `LinkBoardStatus` to `phones-bridge.ts` (copy the contract shape). Desktop: the new fields on the board row, the pair picker (`usbPair`), "Screen and taps" on a paired app iPhone. Web, phone layout and PWA: board rows with state, Stop and Forget from the node routes; an empty list shows nothing new; `desktop: false` shows "Open Allternit Desktop". White and `--neutral-fill` surfaces, no tan. Actions are buttons, not hover or right-click menus.
- Tests: add cases only where there is logic: the row text for each `status` value, Stop calls the stop route and shows the error text on a 409.
- Proof: `pnpm install`, then `pnpm vitest run src/components/computers/UsbAgentSection.test.tsx src/components/computers/PhonesSection.test.tsx src/fabric-session/phone/screens/LinkedPhonesSection.test.tsx`. Screenshots at desktop size, the web phone layout and PWA size, with fixture data. See memory note `allternit-ui-build-verify.md` for the build and live-check steps.
- Done: tests pass and the three screenshots exist. Not merged to ai main (a merge there is a production deploy).

## Lane I — iOS rows (spec step 10)

- Branch: `link/p1-ios`. Model: sonnet. Contract: section 6.
- May change: `surfaces/allternit-mobile/ios/Features/Phones/AppPhonesSettingsView.swift`, `AppPhoneService.swift`. A new small model file in the same folder is allowed.
- Tasks: in the paired section of the existing screen, show the board paired to this iPhone (from `GET /api/v1/phones/boards` through the runtime the phone is paired with; `phone` equals this phone's serial): attached or not, armed or not, the AssistiveTouch steps, and a Stop button. No new rail tab. No Bluetooth.
- Proof: a simulator build, using the build command in `~/Desktop/HANDOFF-ios-app-2026-10-30.md`, and one screenshot with fixture data.
- Done: the build succeeds and the screenshot exists. Report it as "on the integration branch, not in a store build".

## Lane G — Gizzi Code (spec step 11)

- Branch: `link/p1-gizzi`. Model: sonnet. Contract: section 6.
- May change: `cmd/gizzi-code/src/cli/commands/phone.ts`. May create: `cmd/gizzi-code/test/phone-boards.test.ts`.
- Tasks: `gizzi phone boards` prints one line per board: name, state label, armed, link, dock, face. `gizzi phone stop [--board <id or name>]` calls the stop route; with one board the flag is optional. Plain-language errors for `desktop_offline` and `board_offline`.
- Test: against a local HTTP server that serves the contract JSON: the printed lines contain the armed, link and dock values; stop with two boards and no flag names both and exits non-zero.
- Proof: in `cmd/gizzi-code`: `bun test test/phone-boards.test.ts` and `bun run typecheck`.
- Done: both pass. No gizzi release.

## Lane DOC — docs (spec step 12)

- Branch: `link/p1-docs`. Model: sonnet. Starts after lanes F and R are merged, so it describes real routes.
- May create: `surfaces/docs/guides/allternit-link.mdx`, `surfaces/docs/api/link-boards.mdx`.
- May change: `surfaces/docs/guides/usb-agent.mdx`, `surfaces/docs/docs.json`, `surfaces/docs/api/cloud-api.mdx` (line 161 area), `surfaces/docs/api/phone-stream-relay.mdx` (line 125 area), `docs/dependency-map/features.json`, `docs/dependency-map/runtime-links.json`.
- Must not touch: `surfaces/docs/api/phone-holds.mdx`.
- Tasks: the guide in plain words (what the board does, the three things only a person can do, pairing with the app, Stop, Forget and removing a lost board, what the relay can see). Both new pages carry the "unreleased build" warning, as `phone-holds.mdx` does. No prices, no promises, no claim about a real phone. Quote any bare angle-bracket word in prose (MDX breaks on it).
- Proof: `python3 surfaces/docs/scripts/check_links.py` reports 0 problems; `python3.11 scripts/dependency-map.py --validate` passes.
- Done: both commands pass.

## Order

1. The orchestrator commits the contract and this plan to `integ/link-phase1`, and updates the Allternit Link row in `LANES.md` (the tree is now `allternit-integ-link-phase1`).
2. Lane 0 alone. Merge it.
3. Together: lanes F, R, C and G. Lane R is the only one using cargo.
4. Merge order into `integ/link-phase1`: F, then C. R and G whenever they are done.
5. Together, after step 4: lanes D, U, I and DOC. Run the iOS build when lane R's cargo work has stopped.
6. Combined check, run once by the orchestrator (or a haiku checker) in the tree on `integ/link-phase1`, one command at a time:
   - `pnpm install`; in `surfaces/allternit-desktop`: `pnpm run typecheck`, then `npx vitest run src/main/phone-`.
   - `eval "$(gizzi agents tree env)"`; `cargo check -p allternit-cloud-api`; `cargo check -p allternit-api`; the lane R tests; the smoke boot.
   - `python3 surfaces/docs/scripts/check_links.py`; `python3.11 scripts/dependency-map.py --validate`.
   - In `cmd/gizzi-code`: `bun run typecheck`, `bun test test/phone-boards.test.ts`.
   - In the ai worktree: the lane U tests and screenshots.
   - The report lists each command with its result.
7. Stop. Nothing merges to main until spec step 1 passes on Eoj's iPhone.

## Deferred acceptance

| Spec acceptance line | Needs first |
|---|---|
| `TEST-LOG.md` has a pass or fail line with evidence for every case in steps 1, 2 and 7 | Step 1 and 7: a board and Eoj's iPhone. Step 2: Eoj's iPhone. |
| A bot sets an alarm with the app sharing the screen, a board in the port, no AirPlay, Desktop on another network | A board, Eoj's iPhone, a second network |
| Pressing the button during a swipe stops it at once, the call answers `disarmed`, the run stops | A board and the iPhone. The simulated version is tested in lanes C and R. |
| A fresh boot is disarmed and no command can arm it | Tested now in the protocol suite. On a real board: a board. |
| A wrong board key and a wrong relay token are both refused | Tested now (Desktop and cloud-api). |
| A sensitive tap makes a hold, the face shows "needs you", nothing is tapped until approved | A board and the iPhone. The simulated version is tested in lane C. The face on a real panel: the panel. |
| The e-paper shows default, wink and working with five squares, and keeps the picture with power removed (photo) | The panel, a board, the tinted window sample |
| Protocol tests pass at USBA/2 and a USBA/1 board is still accepted | Tested now. |
| The shipped image matches the source version and the `firmware.json` hash | Tested now, if arduino-cli is installed. |
| On the hardened board: an unsigned image is refused and a flash dump shows no readable secrets | A sacrificial board and Eoj's OK for it |
| Screenshots of the new states on Desktop, the web phone layout, the PWA and the iOS simulator; `gizzi phone` prints them | Now, with fixture data. With a real board: a board. |
| `check_links.py` reports 0 problems | Now. |
| `hardware/link/README.md` lists every audit item as measured, chosen or open | Done: every line is `open`. |
| The board shows up as a mouse on a real Android phone (Surface Matrix) | A board and the old Android phone |
| Current drawn from the phone, surface temperature | A board and the inline power meter (parts) |

## Risks found while reading

Where the real code contradicts or goes beyond the spec:

1. **Replies and state lines are not signed in USBA/1.** `phone-usb-agent-protocol.ts` lines 87 to 99 and `usb_agent.ino` lines 185 and 382. The spec says the relay "cannot forge a command"; true, but it could forge "ok". The contract adds signed replies and state in USBA/2.
2. **USBA lines are clear text.** The relay would read typed text. The contract adds `typex`. Key combos stay readable.
3. **The spec's Desktop file cannot be the relay endpoint.** A relayed socket ends in allternit-api (`runtime_relay.rs` lines 40 to 45). The contract adds a loopback TCP pipe from `phone_usba.rs` to Desktop's listener.
4. **`phone.task` and the guard skip app phones today.** `phone-tools.ts` lines 889 to 893 send every call on an app phone to `apps.call` before the `phone.task` line (898), and `screenFacts` reads no screen for an app phone (line 1422). The permitted edit covers both from inside `dispatch`.
5. **The node task loop does not stop on `disarmed`.** `phone_task.rs` lines 190 to 192 know only `device_locked`. Lane R adds the rule. `phone_task.rs` is not on the forbidden list, but the Phone and Messages lane changed it this week; rebase before editing.
6. **Other surfaces cannot see a USB board at all.** `GET /api/v1/phones` (`phone_ws.rs` line 1227) lists registry rows only, and the UI reads boards through Desktop IPC (`src/preload/index.ts` lines 900 to 908). The spec's "same status rows and Stop" on web, PWA, iOS and gizzi needs the new node and gateway routes. `phone-gateway.ts`, `phone-ipc.ts` and the preload are not in the spec's file list.
7. **USBA/1 firmware resets its idle timer on any command once armed** (`usb_agent.ino` line 286). New commands must sit above the armed check, and Desktop must not send them to a USBA/1 board.
8. **The app stream is H.264, decoded one group of pictures at a time for a still** (`phone-app-store.ts` lines 326 to 766), and its screenshot says `coordinate_space: 'none'`. AirPlay gives 8 JPEG frames a second. Frame rate and delay for pointer aiming are unknown until spec step 2.
9. **The board allows 5 s for the handshake after `hello`** (`usb_agent.ino` line 436). Over the relay that includes the cloud, the node and the TCP hop.
10. **`openapi_matches_router` covers only `/v1/…` platform routes** (`platform_v1/tests.rs` lines 745 to 763). The new `/api/v1/link-boards` routes need no OpenAPI entry; the route tables in two docs pages need them.
11. **The account erase test will fail** until `link_boards` is in the erase list (`account_erase.rs` lines 120 and 667 to 683).
12. **The dependency map has entries for `hardware/usb-agent`** (`features.json` line 2128 area). The spec said it had none.
13. **The lane board is out of date.** `LANES.md` says platform #2215, #2216 and #2217 wait for a merge; all three are merged and are in `976ef690d`. `phone-holds.mdx` changed in #2217. The Allternit Link row still names the old tree.
14. **Migrations in production.** The memory note says the deploy script applies new `migrations_pg` files through a ledger (platform #1892); the brief said "hand-applied". Either way the binary does not apply them, and nothing is applied in this phase.
15. **The panel.** The spec says about 1 inch; the candidate is 1.54 inch, 200 × 200. Face art is tied to that canvas.

Not verified:

- Whether `arduino-cli` and esp32 core 3.3.12 are installed here; whether the core has AES-GCM and a CA bundle (O-5, O-10).
- The relay path rules and quotas (O-1, O-2). Only the top of `validate_proxy_request` and `required_capability` was read.
- Whether Desktop starts its board listener at launch, and which cloud credential its main process holds (O-3, O-4).
- Whether the node task loop works on a screenshot-only phone (O-7). The USB lane has never run on a real phone.
- Whether a cloud computer can run the Desktop phone lane (O-8).
- The cargo package names and the iOS build command.
- Not read: `phone_guard.rs`, `phone-hold-client.ts`, `phone-usb-agent-pointer.ts` and `phone-usb-agent-provision.ts` bodies, `phone-airplay.ts`, most of `phone-service.ts`, `registered_phones.rs`, the quota service, `phone-holds.mdx` after line 150, the Swift and React component bodies, `flash.sh`, `NOTICE`, the guides, the two handoff files beyond a search, and the CI workflows.
