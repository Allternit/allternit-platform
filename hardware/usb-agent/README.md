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

## Flash

```bash
hardware/usb-agent/flash.sh            # build + flash the board plugged into this computer
hardware/usb-agent/flash.sh --build-only
```

Then set it up from Desktop: **Settings → Computers → Phones → Connect an iPhone**. Desktop writes your Wi-Fi details and a new 32-byte key to the board over USB. Nothing secret is ever compiled into the firmware.

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

The firmware and the pointer and vision code are adapted from esp32-iphone-agent (MIT); see [NOTICE](NOTICE). UxPlay is GPL-3.0 and is not included: Desktop starts a `uxplay` you installed (`brew install uxplay`).
