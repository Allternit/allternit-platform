# Native board

The USB Agent firmware (`../firmware/usb_agent/`), compiled unchanged as a program for macOS or Linux. Tests drive it with the real Desktop code. No ESP32 board is needed.

Plan: `docs/research-allternit-link-native-board-PLAN.md`. This page is the control contract for anything that starts a native board.

## What it is and is not

- The firmware source is not edited. `board_main.cpp` includes `Arduino.h` and then `usb_agent.ino`. The firmware's includes resolve to `shim/`.
- Stand-ins: Wi-Fi and mDNS are tables, TCP sockets are real, flash storage is a file, USB HID records every report, serial is a pty, crypto is OpenSSL 3 behind `mbedtls/*.h` headers.
- It does not run the chip's own Wi-Fi, USB or TLS code, and it has no memory limit. `long` is 64-bit here and 32-bit on the board.
- It has no iPhone on the other end. It proves what the board sends, not what iOS does with it.
- Licences: OpenSSL 3 (Apache-2.0) is linked, not copied. No GPL code. No arduino-esp32 source is copied; `check-signatures.sh` compares the shim signatures with the installed core headers.

## Build and check

```
make -C hardware/usb-agent/native all          # build/link-board, link-board-dock, link-board-epd
bash hardware/usb-agent/native/smoke.sh        # ends with: smoke ok: prov info usba-aabbccddeeff 1.1.0 provisioned=0
bash hardware/usb-agent/native/check-signatures.sh   # signatures ok (needs the esp32 core installed)
```

`make FIRMWARE_DIR=<dir> OUT=<dir> link-board` builds another copy of the firmware (for example a mutant, or the USBA/1 firmware from main).

Variants: `link-board` is the shipped configuration (no panel, no dock or stop pin). `link-board-dock` sets `DOCK_DETECT_PIN=1` and `STOP_PIN=2`. `link-board-epd` sets `LINK_DISPLAY_SSD1681` with SCK 7, MOSI 9, CS 3, DC 4, RST 5, BUSY 6.

## Environment

| Variable | Meaning |
|---|---|
| `LINK_NATIVE_MAC` | The board's MAC as 12 hex characters. The board id is `usba-<mac>`. Default `aabbccddeeff`. |
| `LINK_NATIVE_STATE` | The flash storage file. Optional. Without it a temporary file is used and deleted on `quit`. |
| `LINK_NATIVE_HOSTS` | `name=ip:port` pairs, comma separated, for the relay host (the firmware always dials port 443; the pair also rewrites the port). |
| `LINK_NATIVE_CA_FILE` | The CA file the TLS client trusts. |

Variables that start with `LINK_NATIVE_RESET`, `_PTY`, `_WORLD`, `_SOCKETS` or `_STATE_TEMP` are internal. They carry state across a restart.

## Control lines (stdin)

One command per line. Closing stdin is the same as `quit`.

| Command | Effect |
|---|---|
| `pin <n> <0\|1>` | Sets an input pin. A change fires the attached interrupt from the control thread, so a press latches while `loop()` is blocked. Pin 0 is the arm button, active low. |
| `usb mounted\|asleep\|none` | The phone's USB state. Starts `mounted`. |
| `usb-wake on\|off` | Whether the host allows remote wake. Starts `off`. A woken host resumes after 20 ms. |
| `clock +<ms>` | Jumps `millis()` forward. The watchdog uses real time. |
| `wifi air <ssidhex> <passhex\|->` | Puts a network on the air. Replaces an entry with the same SSID. A join takes 500 ms of board time. |
| `wifi clear` | Removes every network. |
| `mdns set <ip> <port> <deskId>` | Adds an mDNS answer. Replaces an entry with the same desk id. |
| `mdns clear` | Removes every answer. |
| `mdns stall <ms>` | The next query blocks for `<ms>` instead of 3000, once. A query does not feed the watchdog. |
| `epd busy <ms>` | How long the panel holds BUSY after a refresh. |
| `power-cycle` | Restarts with reason `power-on`. Flash storage is kept. |
| `quit` | Stops the board. |

## Events (stdout)

One JSON object per line: `{"ev":"<name>","t":<board millis>,…}`.

| Event | Fields |
|---|---|
| `boot` | `reason`, `id`, `pty` (the serial port path) |
| `hid` (mouse) | `dev:"mouse"`, `buttons`, `x`, `y`, `wheel`, `pan`, `sent`. One per report. |
| `hid` (keyboard) | `dev:"keyboard"`, `call` (`press`, `release`, `write`, `releaseAll`), `ch`, `sent`. One per call. |
| `pin` | `n`, `v`. Sent at `pinMode(OUTPUT)` and on every change. Read the latest value. Pin 21 is the armed LED, active low. |
| `frame` | `full`, `hex`. One per panel refresh (epd variant). |
| `wifi` | `state` (`joining`, `connected`, `disconnected`), `ssid` (hex) |
| `wdt` | The watchdog fired. A `boot` with reason `watchdog` follows. |
| `nvs-violation` | `op`, `ns`, `key`. A key or namespace over 15 characters. |
| `ctl-error` | `line`, `error`. A control line was not understood. |

`sent` is 0 when the phone would not get the report (USB `none` or `asleep`). Count only `sent:1`.

## Restart

`ESP.restart()` and the watchdog start the same binary again with `execv`. Every global and `static` resets, as on a board. The pty stays open. Open sockets are not closed cleanly: the peer sees a reset on its next packet, as with a real board.
