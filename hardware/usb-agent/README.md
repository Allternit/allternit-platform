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
| Texts and calls aren't offered over the USB Agent | `phone-usb-agent.ts` |

**Known limit:** the Wi-Fi password and the board key sit in the board's flash (NVS) without encryption. Someone who takes the board and reads its flash can recover both. ESP32 flash encryption closes that, but turning it on burns eFuses, which can't be undone, so it's left to whoever builds production boards. If a board is lost, choose **Forget** in Desktop (its key stops working) and change your Wi-Fi password.

Pressing the button disarms the board at once, including in the middle of a swipe or a long text.

The board can't tell what a tap *means*. A pointer can open a payment sheet as easily as a settings page. Keep the board disarmed when it isn't working for you, and watch the live view when the bot does anything that matters.

## Bluetooth mode (firmware 1.1.0)

By default the board is a USB keyboard and mouse in the iPhone's port. In Bluetooth mode it pairs with the iPhone as a Bluetooth LE keyboard and mouse named **Allternit Keyboard**, so no cable to the iPhone is needed. Everything else is unchanged: the board still dials Desktop over Wi-Fi (power it from the computer, a charger or any USB port), uses the same authenticated USBA/1 link, and still needs its button to arm. It uses the same keyboard and mouse report map as the Android app and the Mac tool, so Desktop's shared core treats every sender alike. This gives Windows and Linux Desktop a Bluetooth route (they have none of their own) and is the fallback if the Mac route stops working.

Pairing:

1. Set the board to Bluetooth mode in Desktop. The board restarts disarmed and starts advertising. Until an iPhone is bonded, Desktop reports **BT_NOT_PAIRED**.
2. On the iPhone: Settings > Bluetooth > **Allternit Keyboard** (within about 10 m of the board). Pairing is Just Works; nothing to type.
3. Press the board's button to arm it. Turn on AssistiveTouch on the iPhone (Settings > Accessibility > Touch > AssistiveTouch) so it shows the pointer.

Rules in Bluetooth mode:

- **One iPhone.** Once bonded, the board only keeps the bonded iPhone connected. To pair a different one, run `bt pair` (Desktop: pair again; opens a 2-minute window), or `bt forget`. Then also choose "Forget This Device" in the iPhone's Bluetooth list.
- **Disarmed means silence.** No Bluetooth report is sent while disarmed, after a lost Wi-Fi link or lease, or when no iPhone is connected (`err bt not connected`). Releasing keys and buttons applies to Bluetooth too.
- **Switching mode disarms** and restarts the board, so a mode change never leaves input live. The mode is stored on the board. `prov wipe` returns it to cable mode.
- Scroll is vertical only (the shared report map has no horizontal wheel). Reports go out at the same pace as the cable. Range is about 10 m in open air.
- The arm button only arms and disarms. It does not switch modes.
- Firmware headroom: the image is about 93% of the app partition. A larger change needs a bigger partition scheme.

Commands (all authenticated, work while disarmed because they move no input): `mode cable|bt`, `bt pair`, `bt forget`. The state line gains the mode and the Bluetooth state: `s armed=<0|1> usb=<mounted|asleep|none> mode=<cable|bt> bt=<off|pairing|bonded|connected>` (older firmware sends only the first two fields; Desktop reads that as cable mode).

Desktop maps the state to its health: in Bluetooth mode, pairing or bonded-but-away is `BT_NOT_PAIRED`, and the USB cable state is ignored.

## Protocol (USBA/1)

The protocol is defined in `surfaces/allternit-desktop/src/main/phone-usb-agent-protocol.ts`. `ReferenceBoard` in that file is this firmware's logic in TypeScript, and the Desktop tests run against it. If you change the firmware, change `ReferenceBoard` in the same commit.

Setup over USB serial:

```
prov info                       → prov info usba-<12 hex> <fw> provisioned=<0|1>
prov set ssid|pass <hex>        → prov ok
prov set host <ipv4> | port <n> | desk <id> | key <64 hex>
prov commit                     → prov ok, then the board restarts (disarmed)
prov wipe                       → forget everything
```

Over Wi-Fi, the board looks for Desktop over mDNS (`_allternit-usba._tcp`, TXT `id=<desk id>`) first, then falls back to the address saved at setup.

## Licenses

The firmware and the pointer and vision code are adapted from esp32-iphone-agent (MIT); see [NOTICE](NOTICE). Bluetooth mode uses the BLE library bundled with the arduino-esp32 core (NimBLE, Apache-2.0); no third-party BLE code was added. UxPlay is GPL-3.0 and is not included: Desktop starts a `uxplay` you installed. Homebrew has no `uxplay` formula, so on macOS UxPlay is built from source: run [install-uxplay.sh](install-uxplay.sh).
