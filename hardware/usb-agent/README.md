# Allternit USB Agent

A small ESP32-S3 board that lets a bot operate a real iPhone, or anything else that accepts a USB mouse and keyboard. Nothing is installed on the phone. It needs no Mac-only iPhone Mirroring and no Apple developer account.

- **Hands:** the board plugs into the phone's USB-C port and shows up as a standard USB mouse and keyboard. With iOS AssistiveTouch on, the mouse moves an on-screen pointer.
- **Eyes:** the phone mirrors its screen to Allternit Desktop over AirPlay. Desktop receives it with [UxPlay](https://github.com/FDH2/UxPlay), which runs as a separate program.
- **Brain:** whichever bot uses the `phone.*` tools. Desktop translates taps and swipes into pointer moves, calibrates the pointer and checks every tap against the screen.

The user guide is `surfaces/docs/guides/usb-agent.mdx`. Desktop code is in `surfaces/allternit-desktop/src/main/phone-usb-agent*.ts` and `phone-airplay.ts`.

## Hardware

| Part | Notes |
|---|---|
| ESP32-S3 board with native USB | Tested target: Seeed Studio XIAO ESP32S3 (the plain one, not Sense or Plus). Any S3 board whose USB-C port goes to the S3's own USB pins works. Boards that only have a USB-to-serial chip don't. |
| USB-C hub with power pass-through | Optional. Keeps the phone charging while the board is plugged in. |
| iPhone with USB-C | iPhone 15 and later. Lightning iPhones need Apple's Lightning to USB 3 Camera Adapter. |

The XIAO's BOOT button is the **arm button** and its user LED shows the armed state. On other boards, set `ARM_BUTTON_PIN`, `ARM_LED_PIN` and `ARM_LED_ACTIVE_LOW` at build time.

## Install

Users don't flash anything by hand. **Settings → Computers → Phones → Connect an iPhone → Set up** installs the firmware Desktop ships (`surfaces/allternit-desktop/resources/phone/usb-agent/`), then writes your Wi-Fi details and a new 32-byte key over USB. Nothing secret is ever compiled into the firmware.

Developers:

```bash
hardware/usb-agent/flash.sh               # build + flash your own build onto the board plugged into this computer
hardware/usb-agent/build-firmware.sh      # rebuild the image Desktop ships (run after any firmware change, commit both files)
```

The shipped image is reproducible: building the same source with the same core (recorded in `firmware.json`) gives the same SHA-256. A Desktop test fails if `FIRMWARE_VERSION` in the source and the shipped image don't match.

## Safety model

| Rule | Where it's enforced |
|---|---|
| Only a Desktop holding the board's key can drive it, and only a board holding the key is accepted (mutual HMAC handshake, fresh session key per connection) | firmware + `phone-usb-agent-link.ts` |
| A recorded command can't be replayed: commands carry an HMAC tag and a strictly increasing sequence number | firmware |
| The board boots **disarmed**. Only its button arms it. Desktop can disarm it (**Stop**) but never arm it | firmware |
| Disarmed, link lost, or 10 s without authenticated traffic: every key and button is released and input is refused | firmware |
| It disarms itself after 30 min without input, or 1 min without a Desktop session | firmware |
| Every command is bounded: move ±4000, type ≤200 ASCII chars, combos of ≤4 named keys | firmware + protocol |
| A key can only be set over USB, so setup needs the board in hand | firmware |
| Only a phone that entered this computer's 4-digit AirPlay PIN can mirror to it, so nobody else on the Wi-Fi can feed the bot fake screens | `phone-airplay.ts` (`uxplay -pin -reg`) |
| A command whose answer never arrives is reported `uncertain`, never ok | `phone-usb-agent-link.ts` |
| USBA/2: replies and state lines are signed under the session key. A missing or wrong tag ends the session (`bad_tag`) and a pending command is `uncertain`, so a relay in the path cannot forge an "ok" or an armed state | firmware + `phone-usb-agent-link.ts` |
| USBA/2: a replayed state line is ignored (its `n` must rise), and a recorded one fails in a new session (fresh session key) | `phone-usb-agent-link.ts` |
| USBA/2: the version is bound into the handshake labels (`srv2`, `brd2`, `ses2`), so rewriting `usba/2` to `usba/1` breaks the handshake | firmware + protocol |
| USBA/2: typed text goes as `typex` (AES-256-GCM under a per-session text key), so the relay cannot read it. Key combos are still readable | firmware + `phone-usb-agent-link.ts` |
| `face`, `prog` and `label` run while disarmed because they move nothing. They never arm, send no HID report and do not reset the idle timer. Desktop never sends them to a USBA/1 board | firmware + `phone-usb-agent-link.ts` |
| The dock stop line (`STOP_PIN`, default none) only disarms, never arms | firmware |
| Texts and calls aren't offered over the USB Agent | `phone-usb-agent.ts` |

**Known limit:** the Wi-Fi password and the board key sit in the board's flash (NVS) without encryption. Someone who takes the board and reads its flash can recover both. ESP32 flash encryption closes that, but turning it on burns eFuses, which can't be undone, so it's left to whoever builds production boards. If a board is lost, choose **Forget** in Desktop (its key stops working) and change your Wi-Fi password.

Pressing the button disarms the board at once, including in the middle of a swipe or a long text.

The board can't tell what a tap *means*. A pointer can open a payment sheet as easily as a settings page. Keep the board disarmed when it isn't working for you, and watch the live view when the bot does anything that matters.

## Protocol (USBA/1 and USBA/2)

The protocol is defined in `surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts`; the USBA/2 contract is `hardware/link/README.md` section 2. `ReferenceBoard` in that file is this firmware's logic in TypeScript, and the Desktop tests run against it. If you change the firmware, change `ReferenceBoard` in the same commit.

Firmware 1.1.0 speaks USBA/2. Firmware 1.0.0 speaks USBA/1. Desktop accepts both.

```
board → hello usba/2 <boardId> <fw> <resetReason> <bn>
desk  → challenge <sn> <HMAC(K, "srv2|id|bn|sn")>
board → auth <HMAC(K, "brd2|id|bn|sn")>
desk  → ready
desk  → c <seq> <command> <first 32 hex of HMAC(SK, "c|seq|command")>
board → r <seq> <ok | err reason> <first 32 hex of HMAC(SK, "r|seq|result")>
board → s armed=<0|1> usb=<mounted|asleep|none> dock=<0|1> hid=<usb|ble|none> link=<lan|cloud> face=<name> prog=<0-5> n=<count> <first 32 hex of HMAC(SK, "s|<fields> n=<count>")>
```

SK = HMAC(K, "ses2|bn|sn") and EK = HMAC(K, "enc2|bn|sn") are new for every connection. USBA/1 uses the labels without the `2`, and its replies and state lines (`r <seq> ok`, `s armed=… usb=…`) are not signed.

USBA/2 commands added to the USBA/1 set:

| Command | Bounds | Armed | Notes |
|---|---|---|---|
| `typex <hex>` | sealed `type`: AES-256-GCM under EK, IV = 4 zero bytes + seq (8 bytes, big-endian), AAD `c|<seq>|typex` | yes | Desktop always uses it on USBA/2. A GCM failure answers `err bad text`. |
| `face <name>` | `default`, `wink`, `working`, `needs_you`, `stopped` | no | `err unknown face` |
| `prog <n>` | one digit, 0 to 5 | no | `err bad numbers`, `err out of bounds` |
| `label <hex>` / `label -` | 1 to 24 printable ASCII bytes; `-` clears | no | RAM only, for the bank |

The firmware sets faces itself: `default` at boot and on arm, `stopped` with prog 0 on any disarm. The face is drawn by `firmware/usb_agent/face.h` (the same renderer as `phone-usb-agent-face.ts`; a Desktop test compiles `firmware/host/face_dump.cpp` and compares the two byte for byte). The shipped image has no panel (`NullDisplay`). Build with `-DLINK_DISPLAY_SSD1681` and the `EPD_*_PIN` values for a 200 × 200 SSD1681-class panel; that driver is ours and has never run on a panel.

Through the cloud relay (`firmware/usb_agent/relay_link.h`): the board tries the LAN first. When `relay` and `rtok` are set and no LAN session formed within 5 s of joining Wi-Fi, it asks `POST https://<relay>/api/v1/link-boards/<boardId>/socket-ticket` for a ticket (at most every 5 s, doubling to 60 s after a failure), opens the WebSocket, waits for the relay's ready frame, and then sends one USBA line per text frame. The handshake is the same end to end. TLS uses the core's built-in CA bundle. Not yet run on a board.

Optional build flags: `DOCK_DETECT_PIN` (dock=1 after 100 ms active, `DOCK_DETECT_ACTIVE_LEVEL` default HIGH) and `STOP_PIN` (active low, 40 ms disarms). Both default to none.

Setup over USB serial:

```
prov info                       → prov info usba-<12 hex> <fw> provisioned=<0|1>
prov set ssid|pass <hex>        → prov ok
prov set ssid1..3|pass1..3 <hex>   up to three more saved networks (firmware 1.1.0)
prov set relay <host>           1 to 63 of a-z 0-9 . -  (firmware 1.1.0)
prov set rtok <64 hex>          the relay token (firmware 1.1.0); stored only with relay
prov set host <ipv4> | port <n> | desk <id> | key <64 hex>
prov commit                     → prov ok, then the board restarts (disarmed)
prov wipe                       → forget everything
```

Over Wi-Fi, the board looks for Desktop over mDNS (`_allternit-usba._tcp`, TXT `id=<desk id>`) first, then falls back to the address saved at setup. With no saved network joined, it tries the next saved network every 15 s.

## Licenses

The firmware and the pointer and vision code are adapted from esp32-iphone-agent (MIT); see [NOTICE](NOTICE). `face.h`, `display.h` (including the SSD1681 driver) and `relay_link.h` are our own code; no display or WebSocket library is used. UxPlay is GPL-3.0 and is not included: Desktop starts a `uxplay` you installed. Homebrew has no `uxplay` formula, so on macOS UxPlay is built from source: run [install-uxplay.sh](install-uxplay.sh).
