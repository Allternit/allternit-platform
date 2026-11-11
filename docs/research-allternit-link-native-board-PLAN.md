# Plan: Allternit Link Phase 1 without boards (native board, Wokwi, QEMU)

Root for every path below: `/Users/joe/Desktop/allternit-workspace/allternit-integ-link-phase1` (branch `integ/link-phase1`).

**Goal.** Run the unchanged firmware C++ on this Mac as a "native board", drive it with the real Desktop code, and compare it with `ReferenceBoard`. Wokwi files are prepared to run once a token exists.

**Out of scope.** Any merge to main, deploy, purchase, migration or eFuse burn. New product features. The iPhone side (spec step 1 stays the merge gate).

## Surface Matrix

| Surface | Status | Reason |
|---|---|---|
| Desktop | BUILD | New test kit and four test files in `surfaces/allternit-desktop/src/main/`. No shipped behaviour changes unless a test finds a bug; `ReferenceBoard` fixes land here. |
| ai.allternit.com + phone layout | N/A | The harness adds no state, field or control. `LinkBoardStatus` (contract section 6) does not change. If a firmware fix changes a state field, this row becomes BUILD in that fix's commit. |
| m.allternit.com PWA | N/A | Same reason; it reads the same status shape. |
| iOS | N/A | Same reason. Nothing in this plan can emulate the iPhone side either. |
| Android | N/A | Same reason; it gets board rows through the PWA. |
| Gizzi Code | N/A | `gizzi phone` reads the same status routes; none change. |
| Backend contract | HAS | USBA/2, `link_boards.rs` and `phone_usba.rs` are on the branch (contract sections 2 and 4). Step 12 adds a loopback example binary and may change one visibility keyword. No route or wire change. |
| Docs | BUILD | `hardware/usb-agent/README.md`, new `hardware/usb-agent/native/README.md`, `TEST-LOG.md`, two Mintlify guides, the existing plan's deferred table, the dependency map. |

## Design decisions

1. **Seam: shim headers, no `#ifdef`.** A wrapper `board_main.cpp` includes `Arduino.h` and then `../firmware/usb_agent/usb_agent.ino`. The firmware's includes resolve to `native/shim/`. Rejected: `#ifdef NATIVE` in the firmware, which changes the firmware and forces an image rebuild.
2. **Compile flags match the device where a Mac can.** `-std=gnu++2b -funsigned-char`. Xtensa `char` is unsigned and Apple arm64 `char` is signed; the firmware compares `char` with `' '` and `'~'`. `millis()` returns `uint32_t`. Shim signatures copy the real parameter types (for example `int8_t` in `USBHIDMouse::move`), so truncation is the same. `long` stays 64-bit on the host (see risks).
3. **Own `String`, `IPAddress`, `Serial`, `Preferences`.** Small and backed by `std::string`. Rejected: compiling the core's `WString.cpp` (LGPL-2.1), because CI has no esp32 core and the file pulls in core internals.
4. **Crypto and TLS: OpenSSL 3 (Apache-2.0) behind shim `mbedtls/*.h` headers.** It is on this Mac and on GitHub Ubuntu runners. The adapter is written from the mbedTLS signatures in the installed core, not from how the firmware calls them. Rejected: vendored mbedTLS (Apache-2.0 OR GPL-2.0-or-later). It adds about 15 files or a network fetch in CI, and every handshake and `typex` is already checked against Node's crypto by the real Desktop code. Real mbedTLS is covered by Wokwi.
5. **One process per boot.** `ESP.restart()` and the watchdog call `execv` on the same binary, with the reset reason in the environment. All globals and `static` locals reset, as on a board. The pty and stdio stay open. Rejected: `longjmp` back to `setup()`, which keeps stale statics.
6. **Clock: real time plus a skew the test can add.** `delay()` sleeps for real, so 8 ms pacing is real. `clock +<ms>` jumps `millis()` for the 10 s lease, 60 s unlinked, 30 min idle, 15 s Wi-Fi and relay back-off timers. The watchdog uses real time. Rejected: a fully virtual clock, which cannot work with real sockets and the real `UsbAgentLink` timers.
7. **Serial is a pty.** The real production `serialPort()` (with its `stty` call) opens the slave path. Rejected: a pipe, which would skip that code.
8. **Control on stdin, events on stdout (JSON lines).** This is the contract between steps 2 to 9:
   - Commands: `pin <n> <0|1>` (fires the attached interrupt on a change, from the control thread, so a press latches while `loop()` is blocked), `usb mounted|asleep|none`, `usb-wake on|off`, `clock +<ms>`, `wifi air <ssidhex> <passhex|->`, `wifi clear`, `mdns set <ip> <port> <deskId>`, `mdns clear`, `mdns stall <ms>`, `epd busy <ms>`, `power-cycle`, `quit`.
   - Events: `boot {reason,id,pty}`, `hid {t,dev,…}` (mouse: buttons, x, y, wheel, pan; keyboard: the call and the character), `pin {n,v}` (LED), `frame {full,hex}`, `wifi {state,ssid}`, `wdt`, `nvs-violation`.
   - Environment: `LINK_NATIVE_MAC`, `LINK_NATIVE_STATE` (NVS file), `LINK_NATIVE_HOSTS` (`relay.test=127.0.0.1:<port>`; the firmware hard-codes port 443), `LINK_NATIVE_CA_FILE`.
9. **Three build variants.** `link-board` (as shipped: `NullDisplay`, no dock or stop pin), `link-board-dock` (`-DDOCK_DETECT_PIN -DSTOP_PIN`), `link-board-epd` (`-DLINK_DISPLAY_SSD1681` plus pins). The dock, stop and panel code is compiled out of the shipped image, so only the variants run it. Each binary is written to a temporary name and renamed, so parallel test workers can build safely.
10. **Differential test compares parsed lines, not bytes.** Nonces are random on both sides. A test-side "desk" built from `protocol.ts` runs the same script on both boards and compares `parseReply` and `parseState` results (both check the tags), drop or no drop, the input log against the folded HID reports, and the last frame. Rejected: seeding both random sources, which needs changes to `ReferenceBoard` and a hook in the firmware.
11. **mDNS and Wi-Fi are tables, sockets are real.** Rejected: real Bonjour, which Linux CI lacks.
12. **Relay test fakes cloud-api plus the node at the network edge.** One TLS server with the ticket route, a `ws` socket, the ready frame, and a line bridge to the real `UsbAgentLink` listener. The real Rust node socket goes in the loop once, on demand (step 12). The real cloud relay does not: it needs Postgres and a running cloud-api.

## Steps

Rules for every step:
- Do not edit `cmd/allternit-api/src/phone_guard.rs`, `surfaces/allternit-desktop/src/main/phone-hold-client.ts` or any hold screen.
- Do not edit `hardware/usb-agent/firmware/**` or `surfaces/allternit-desktop/resources/phone/**`, except in step 11.
- No GPL code. Do not read GxEPD2 or the Waveshare examples.
- Commit on `integ/link-phase1` only.

### Rung 1: native board

**1. Preflight (haiku).** Changes nothing.
- Run `c++ --version`, `PKG_CONFIG_PATH=/opt/homebrew/opt/openssl@3/lib/pkgconfig pkg-config --modversion openssl`, `which openssl stty make`, and `git status --short | wc -l`.
- Run `git show origin/main:surfaces/allternit-desktop/resources/phone/usb-agent/firmware.json` to learn which version main ships.
- Done: OpenSSL prints `3.x`, status prints `0`, and the report states main's firmware version.

**2. Native board core (opus; many files, new runtime).**
- Files, all new under `hardware/usb-agent/native/`: `Makefile`, `board_main.cpp`, `shim/` (`Arduino.h`, `WiFi.h`, `ESPmDNS.h`, `Preferences.h`, `esp_mac.h`, `esp_random.h`, `esp_task_wdt.h`, `driver/gpio.h`, `mbedtls/{md,gcm,base64,sha1}.h`, `USB.h`, `USBHIDMouse.h`, `USBHIDKeyboard.h`, `tusb.h`), `runtime/*.cpp` (host main and control thread, clock, restart, watchdog, GPIO, sockets, NVS file, HID recorder, OpenSSL crypto adapter), `smoke.sh`, `check-signatures.sh`.
- Also add stubs for `shim/NetworkClientSecure.h` (connect always fails) and `shim/SPI.h`, so all three variants link now.
- The Makefile takes `FIRMWARE_DIR=` (default `../firmware/usb_agent`) and `OUT=`, and globs `runtime/*.cpp`.
- The NVS shim enforces real limits: key and namespace 15 characters at most; a breach emits `nvs-violation`.
- `check-signatures.sh` greps each shimmed signature in the installed core headers (core 3.3.12). It runs only where the core is installed.
- Done:
  - `make -C hardware/usb-agent/native all && bash hardware/usb-agent/native/smoke.sh` ends with `smoke ok: prov info usba-aabbccddeeff 1.1.0 provisioned=0`.
  - `bash hardware/usb-agent/native/check-signatures.sh` prints `signatures ok`.
  - `git diff --stat HEAD -- hardware/usb-agent/firmware surfaces/allternit-desktop/resources/phone` prints nothing.

**3. TLS client shim (sonnet).** After 2.
- Files: `native/shim/NetworkClientSecure.h`, `native/runtime/tls_openssl.cpp`, `native/smoke-tls.sh`.
- Real TLS with hostname and CA checks. Trust comes from `LINK_NATIVE_CA_FILE`, the address from `LINK_NATIVE_HOSTS`. Timeouts are honoured.
- Do not touch: `Makefile`, other shims.
- Done: `bash hardware/usb-agent/native/smoke-tls.sh` prints `tls ok: 200` and then `tls refused: untrusted CA`. The second line proves the check can fail.

**4. SSD1681 panel model (sonnet).** After 2.
- Files: `native/shim/SPI.h`, `native/runtime/epd_model.cpp`, `native/smoke-epd.sh`.
- Written from the SSD1681 command table only. It decodes the SPI stream with the DC and CS pins, keeps both RAM planes, emits a `frame` event on `MASTER_ACTIVATE` with full or partial from the `0x22` value, and holds BUSY high for `epd busy` ms.
- Done: `bash hardware/usb-agent/native/smoke-epd.sh` prints `epd ok: boot frame equals face_dump default 0 0, full`.

**5. Test kit and link tests (sonnet).** After 2.
- Files: `surfaces/allternit-desktop/src/main/phone-usb-agent.native.testkit.ts`, `surfaces/allternit-desktop/src/main/phone-usb-agent-native-link.test.ts`. This step owns the test kit; steps 6 to 9 do not edit it.
- `NativeBoard` API: `start({variant, mac, env})`, `ptyPath`, `pin()`, `pressButton(ms)`, `setUsb()`, `advance(ms)`, `wifiAir()`, `mdns()`, `powerCycle()`, `events`, `hid()`, `hidTotals()` (dx, dy, clicks, typed text), `frames()`, `led()`, `stop()`. An adapter feeds HID reports into `SimPhone`. A failed build fails the test with the compiler output; it never skips.
- Tests use the real `UsbAgentLink`, plus a raw socket built from `protocol.ts` where the desk must be silent or hostile:
  - Boots disarmed. No command arms it. `move` answers `disarmed`.
  - A 60 ms button press arms and the LED pin goes low.
  - `move 37 -12` gives reports that sum to (37, −12), each step at most 10, at least 8 ms apart.
  - `type` and `typex` give the same typed text.
  - A button press mid-swipe: `err disarmed`, a partial sum, and a release report.
  - `usb asleep` and `usb none` answer `phone usb asleep`.
  - A wrong server proof, a bad tag, and a line over 512 bytes drop the link with no HID report. A replay answers `err replay`.
  - A silent desk plus `advance(10100)`: the socket closes and held buttons release.
  - `advance` 60 s unlinked and 30 min idle: both disarm with face `stopped`.
  - `mdns stall 9000`: watchdog, and the next `hello` says `watchdog` and the board is disarmed.
  - A wrong host with an mDNS entry still connects. The board joins the second saved network after 15 s.
  - Dock variant: dock=1 after 100 ms; stop low for 40 ms disarms and never arms.
  - `move 4000 4000` replies inside `COMMAND_TIMEOUT_MS`; record the time.
- Done: `cd surfaces/allternit-desktop && npx vitest run src/main/phone-usb-agent-native-link.test.ts` prints `Test Files  1 passed`; `pnpm run typecheck` exits 0.

**6. Setup, cloud and status tests (sonnet).** After 5.
- File: `surfaces/allternit-desktop/src/main/phone-usb-agent-native-setup.test.ts`.
- Uses the real `UsbAgentManager.setUp` with `openSerial: serialPort` (the production function) and `listSerial: () => [board.ptyPath]`. The `LinkBoardCloud` is faked (the network edge).
- Asserts:
  - `prov info` identity, every `prov set` line, commit, restart, and the board dialling in.
  - Settings survive a power cycle.
  - `prov wipe` returns the board to unprovisioned.
  - Relay settings are stored only when both are set.
  - Forget disarms and revokes.
  - `LinkBoardLane.status()` text for each state.
  - A tap aimed by the real pointer code lands on the `SimPhone` button.
- Done: `npx vitest run src/main/phone-usb-agent-native-setup.test.ts` prints `1 passed`.

**7. Differential test against `ReferenceBoard` (sonnet).** After 5.
- File: `surfaces/allternit-desktop/src/main/phone-usb-agent-native-diff.test.ts`.
- A fixed script table covers every command and error in contract 2.6. A seeded generator adds 200 random sequences with small moves. Both boards get the same script and are compared as in decision 10.
- Every mismatch is a failing case with the script printed. Do not edit `ReferenceBoard` or the firmware here; hand mismatches to step 11.
- Done: `npx vitest run src/main/phone-usb-agent-native-diff.test.ts` prints `1 passed`, or the step reports the failing scripts to step 11. Both outcomes close the step.

**8. Relay tests (sonnet).** After 3 and 5.
- File: `surfaces/allternit-desktop/src/main/phone-usb-agent-native-relay.test.ts`.
- The certificate for `relay.test` is made at test time with the `openssl` CLI. No key is committed.
- Asserts:
  - The session reports `link=cloud`.
  - The relay transcript never holds the typed text.
  - A wrong token backs off 5 s, 10 s, 20 s (with `advance`).
  - No ready frame: dropped within 5 s.
  - Ping gets a pong.
  - A binary frame or a server close drops the link and releases keys.
  - A wrong `Sec-WebSocket-Accept` is refused.
  - A chunked ticket answer still works.
  - A button press during a stalled TLS handshake disarms.
  - The LAN is tried first.
- Done: `npx vitest run src/main/phone-usb-agent-native-relay.test.ts` prints `1 passed`.

**9. Panel tests (sonnet).** After 4 and 5.
- File: `surfaces/allternit-desktop/src/main/phone-usb-agent-native-panel.test.ts`.
- Asserts:
  - Each face and prog frame equals `renderFace` bytes.
  - The refresh is full at boot, on `stopped`, and on the 21st change.
  - With `epd busy 2000`, five quick `face` commands end on the newest face, and a button press still disarms within 100 ms.
  - The armed mark follows the button.
- Done: `npx vitest run src/main/phone-usb-agent-native-panel.test.ts` prints `1 passed`.

**10. Mutants, CI, dependency map (sonnet).** After 5 to 9.
- Files: `hardware/usb-agent/native/mutants.sh`, `.github/workflows/ci-desktop.yml`, `docs/dependency-map/runtime-links.json`.
- `mutants.sh` copies the firmware to a temp folder and applies one edit at a time: no command tag check; `seq < lastSeq`; no `releaseAll()` in `dropLink`; display commands below the armed check; lease 100 s; `srv2` to `srv`; no button check in `movePaced`; zero IV in `typeSealed`; accept any WebSocket accept value; stop input arms. It builds each with `FIRMWARE_DIR`/`OUT` and runs the native tests.
- CI: add `hardware/usb-agent/**` to both `paths` lists, and `sudo apt-get install -y libssl-dev` before the unit tests.
- Do not touch `release-desktop.yml`.
- Done: `bash hardware/usb-agent/native/mutants.sh` prints `mutants killed: 10/10`; `python3.11 scripts/dependency-map.py --validate` passes.

**11. Triage and fixes (opus; unknown cause, 4+ files per firmware fix).** After 7; repeats until 5 to 9 are green.
- Class each mismatch as a firmware bug, a `ReferenceBoard` bug, a shim bug, or a contract gap. If the code and the contract disagree, stop and tell the orchestrator.
- One fix per commit, starting from the failing test.
- A firmware fix changes `usb_agent.ino` or a header, `ReferenceBoard` in the same commit, then `bash hardware/usb-agent/build-firmware.sh`, and commits `usb-agent-firmware.bin` and `firmware.json`.
- A TS-only fix needs no image rebuild.
- Done: `npx vitest run src/main/phone-` prints 0 failed; `bash hardware/usb-agent/build-firmware.sh` prints `Built USB Agent firmware` when the firmware changed; `mutants.sh` still prints `10/10`.

**12. Real node socket in the loop (sonnet; the only cargo step).** After 8.
- Files: `cmd/allternit-api/examples/usba_node_socket.rs` (serves the real `phone_usba` router on loopback as a fixed test account and prints its port), `cmd/allternit-api/src/phone_usba.rs` (make `router` and `UsbaConfig` `pub`, only if they are not), `hardware/usb-agent/native/e2e-node-socket.ts` (run with `npx tsx`).
- Chain: native board, the TLS front from step 8, the real Rust node socket, the real `UsbAgentLink`.
- Run `eval "$(gizzi agents tree env)"` first. Use one `cargo build -p allternit-api --example usba_node_socket`. Do not set `RUSTC_WRAPPER` or `CARGO_BUILD_JOBS`.
- Done: `npx tsx hardware/usb-agent/native/e2e-node-socket.ts` prints `e2e ok: ping, typex, disarm through the node socket; close code 1000`.

**13a. Hardware docs (sonnet).** After 11.
- Files: `hardware/usb-agent/README.md` (new section "Testing without a board"), `hardware/usb-agent/native/README.md` (what it proves, what it cannot, the control contract, licences: OpenSSL Apache-2.0, linked not copied), `hardware/usb-agent/TEST-LOG.md` (the simulated fault drill, each line marked "native board, not a real iPhone").

**13b. Guides and plan (sonnet).** After 11.
- Files: `surfaces/docs/guides/usb-agent.mdx`, `surfaces/docs/guides/allternit-link.mdx`, `docs/research-allternit-link-keychain-bot-PLAN.md` (update the "Deferred acceptance" table).
- Done for 13a and 13b: `python3 surfaces/docs/scripts/check_links.py` reports 0 problems.

**14. Combined check (haiku).** Last.
- In `surfaces/allternit-desktop`: `pnpm run typecheck`, then `npx vitest run src/main/phone-`.
- Then `bash hardware/usb-agent/native/mutants.sh`, `python3 surfaces/docs/scripts/check_links.py`, `python3.11 scripts/dependency-map.py --validate`.
- Then `git diff --stat origin/integ/link-phase1 -- cmd/allternit-api/src/phone_guard.rs surfaces/allternit-desktop/src/main/phone-hold-client.ts`, which must print nothing.
- Done: one report with each command and its output.

### Rung 2: Wokwi (real ESP32-S3 binary)

**15. Project files (sonnet).** Any time; needs no token.
- Files under `hardware/usb-agent/wokwi/`: `wokwi.toml`, `diagram.json`, `build.sh`, `run.sh`, `scenarios/{boot,serial-setup,arm-button,wifi-join}.test.yaml`.
- `diagram.json`: a XIAO ESP32-S3, a push button, an LED, and the serial monitor.
- `build.sh` writes to a git-ignored `wokwi/build/` and never touches the shipped image. It builds two flavours: A, the shipped FQBN; B, `CDCOnBoot=default` (serial on UART). If the XIAO part does not expose GPIO 0, it adds `-DARM_BUTTON_PIN` and `-DARM_LED_PIN` build flags; the source supports this.
- Scenarios:
  - Boot: write `prov info`, wait for `1.1.0 provisioned=0`.
  - Serial setup: the `prov set` lines for `Wokwi-GUEST` (open network), commit, then `provisioned=1` after the restart.
  - Arm: press, LED pin low; press again, LED pin high; arm and wait 61 s, LED pin high.
  - Wi-Fi join: written now, but it can only be asserted with the private gateway (see risks).
- Done: `bash hardware/usb-agent/wokwi/build.sh` prints two image paths; `WOKWI_CLI_TOKEN= bash hardware/usb-agent/wokwi/run.sh` exits 2 with `WOKWI_CLI_TOKEN is not set`.

**16. First Wokwi run (haiku).** Gated on Eoj's token and `wokwi-cli`.
- Run boot, serial-setup and arm-button; about 3 CI minutes per run, 50 free per 30 days.
- Record in `TEST-LOG.md` which flavour gives serial and whether GPIO 0 is exposed.
- Done: `bash hardware/usb-agent/wokwi/run.sh` prints `3 scenarios passed`.

**17. Full session through the private gateway (sonnet).** Gated on Eoj buying the plan.
- File: `hardware/usb-agent/wokwi/e2e-desk.ts`. The real `UsbAgentLink` listens on port 7311 on this Mac; the scenario provisions that host and a fixed test key.
- Asserts hello, handshake, `ping`, `face`, `err disarmed`, and an armed state line after the scenario presses the button. This also proves Wi-Fi join and real mbedTLS HMAC.
- The relay stays untested here: the board trusts only the built-in CA bundle, so a local fake relay is refused.

### Rung 3: Espressif QEMU (assessment)

Worth it for spec step 8, but only when step 8 starts; it is not needed for this plan.

- **What it can rehearse for free:** the eFuse burn order and flash and NVS encryption against a file-backed eFuse image; two-slot update and rollback.
- **What it cannot:** Wi-Fi and USB.
- **Open:** S3 secure boot support is reported both ways.
- **Blocker:** secure boot and flash encryption need an ESP-IDF build of the bootloader, which `arduino-cli` cannot produce. Step 8 therefore moves the build to ESP-IDF anyway.

**18. Optional spike (sonnet, 2 hours at most).** Gated on Eoj's OK to install tools.
- Run flavour B from step 15 in `qemu-system-xtensa -machine esp32s3`.
- Done: one line in `native/README.md`: boots yes or no, `prov info` answers yes or no, behaviour at `WiFi.begin`.

## What runs in parallel

- 1, then 2 alone. Step 15 can run beside everything.
- After 2: 3, 4 and 5 together.
- After 5: 6 and 7. After 3 and 5: 8. After 4 and 5: 9. So 6, 7, 8 and 9 run together.
- 11 owns the firmware and `phone-usb-agent-protocol.ts`; nobody else edits them. It starts when 7 reports.
- 10 after 5 to 9. 12 after 8, when no other cargo job runs in the tree.
- 13a and 13b together after 11. 14 last.
- 16, 17 and 18 wait for Eoj.

## What the native board unlocks

| Item | Status |
|---|---|
| Simulated half of the spec step 1 fault drill (button mid-swipe, Wi-Fi drop, lock, unplug, rogue client) | Buildable and testable now |
| Dock detect and the stop line; the bank's `label`; a bank screen controller sketch on the same shims | Buildable and testable now |
| Composite lane and the faces Desktop sends, on the real firmware | Buildable and testable now |
| Step 8 signed update check (signature, version floor, two-slot state, rollback across a restart), written as plain C++ with a file-backed slot | Buildable and testable now |
| USBA/1 firmware from `origin/main`, built with `FIRMWARE_DIR` | Buildable and testable now |
| Whether api.allternit.com's root CA is in the core's bundle (part of O-5) | Checkable now |
| Bluetooth mouse mode (step 7): compiles, reports can be recorded, pairing and iOS acceptance cannot be tested | Buildable, not testable |
| Secure boot, flash encryption and eFuse settings | Buildable, not testable |
| Image size with BLE or update code (the image is at 88% now) | Size is measurable with `arduino-cli`; behaviour is not |
| iOS taking the mouse and keyboard, pointer acceleration, the accessory prompt, the on-screen keyboard | Needs a board and the iPhone |
| USB suspend and remote wake, current draw, an hour locked, AirPlay | Needs a board and the iPhone |
| Real Wi-Fi and multicast mDNS; TLS time and heap against the 3 s step limit and 8 s watchdog | Needs a board |
| The e-paper photo, Android, Bluetooth pairing, the eFuse burn | Needs a board (and the panel or phone) |

## Risks and unknowns

Found while reading; each is a first target for a test:

1. **`ReferenceBoard` does not check input arguments.** `phone-usb-agent-protocol.ts` lines 443 to 453 answer `ok` for `move a b`, `move 5000 0`, `scroll 99 0`, `type zz` and `key bogus`. The firmware (`usb_agent.ino` lines 534 to 554) answers `bad numbers`, `out of bounds`, `bad text` and `unknown key`. Step 7 will fail here first. The fix is TS-only.
2. **`ReferenceBoard` trims each line (line 376); the firmware does not.** `ready ` with a trailing space is accepted by one and dropped by the other.
3. **A `seq` above 4294967295 has three behaviours.** The device saturates (32-bit `unsigned long`), `ReferenceBoard` uses a JS number, and the 64-bit host truncates. Desktop never sends one. Width-of-`long` bugs in general are invisible on the native board.
4. **The ticket answer is parsed without chunked decoding.** It works only while `socketPath` sits in one chunk. Step 8 pins this; behaviour behind Cloudflare is unknown until a deploy.
5. **`move 4000 4000` blocks the loop for about 3.2 s.** Desktop's command timeout is 4 s. Step 5 measures the margin.
6. **The firmware prints nothing on serial except `prov` replies.** On Wokwi's free plan, Wi-Fi join cannot be asserted without a firmware change. I recommend no debug flag.
7. **`prov set host` needs an IPv4 literal.** The numeric address behind `host.wokwi.internal` must be read in the first gateway run.

Not verified; only a first run settles these:

- Whether the `.ino` compiles under clang with the shims. Forward references would need prototypes in the wrapper, not in the firmware.
- Whether `stty raw` works on a pty slave on macOS and Linux.
- Whether TinyUSB CDC gives serial in Wokwi; the XIAO part's pin and LED names; the `wokwi.toml` image field for a merged binary; whether Wokwi has a 200 × 200 SSD1681 part. I know of none, so the panel is covered only by the native model.
- HID call timing: the shim returns at once, but the real call waits for the phone to poll.
- The panel model and the driver come from the same datasheet reading. They prove the wiring and the non-blocking refresh, not that a real panel accepts the sequence.
- The firmware's WebSocket client is tested against the `ws` library, not against the cloud relay's server.
- The dependency map answered "No mapped path: hardware/usb-agent" for my path form. Step 10 must use the `allternit/…` form.
- The native suite's total time. Target: under 90 s.

## Needed from Eoj

1. A free Wokwi account and a `WOKWI_CLI_TOKEN`, kept in his shell or a git-ignored file. This unlocks step 16.
2. Yes or no on the Wokwi paid plan with the private gateway (about EUR 5.6 a month, billed yearly; check the price page first). This unlocks step 17. Agents buy nothing.
3. OK to install `wokwi-cli` on this Mac, and, for the optional spike, Espressif's QEMU build plus its Homebrew libraries.
4. Nothing else for rung 1. The boards and the iPhone test (spec step 1) remain the gate for any merge to main.
