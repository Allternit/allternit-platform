# Wokwi: the real firmware on a simulated XIAO ESP32-S3

Runs the unchanged USB Agent firmware image in Wokwi's simulator, driven by serial lines and a simulated button.

## What it proves

- The real compiled image boots on an ESP32-S3 and answers `prov info`.
- Serial setup (`prov set ...`, `prov commit`) saves settings and the board restarts with `provisioned=1`.
- The arm button toggles the arm LED, and an armed board with no session disarms after 60 s.

## What it cannot prove

- No USB mouse or keyboard (HID): Wokwi has no USB host.
- No Bluetooth.
- The free plan cannot reach a computer on your network, so Wi-Fi join and the session handshake cannot be asserted (the firmware prints nothing for them). `scenarios/wifi-join.test.yaml` is written for the private gateway plan and is not run by `run.sh`.
- The e-paper panel: no matching Wokwi part is known.

## What is needed

1. A Wokwi account and a CI token, exported as `WOKWI_CLI_TOKEN`.
2. `wokwi-cli` installed (not installed by these files).
3. `arduino-cli` with the esp32 core 3.x (for `build.sh`).

## Use

```
bash hardware/usb-agent/wokwi/build.sh        # prints three image paths (flavours a, b and c)
export WOKWI_CLI_TOKEN=...                     # never commit it
bash hardware/usb-agent/wokwi/run.sh           # FLAVOUR=b for UART serial, FLAVOUR=c for hardware USB serial
```

`run.sh` exits 2 when the token or `wokwi-cli` is missing, and prints `3 scenarios passed` only when boot, serial-setup and arm-button all passed.

Images are written only to `wokwi/build/` (git-ignored), never to `surfaces/allternit-desktop/resources/phone/`. They are built with the arm button on GPIO 1 and the arm LED on GPIO 2 (`-DARM_BUTTON_PIN=1 -DARM_LED_PIN=2`), because the Wokwi XIAO board definition wires neither GPIO 0 nor a free GPIO 21 (GPIO 21 is only a virtual pin on its built-in LED). The firmware honours these overrides through `#ifndef` guards; it is not edited. So these are not the shipped image bytes, but the same source.

The test key in the scenarios is 64 hex zeros, a fake. `Wokwi-GUEST` is Wokwi's open network.

## What the docs confirmed

- `wokwi.toml`: `[wokwi] version = 1`, `firmware` (required, relative to the toml), optional `elf` (project-config page). ESP32 accepts `.bin`, `.uf2`, `.elf`, `flasher_args.json`.
- ESP32 page: a single `.bin` with bootloader, partition table and app (esptool `merge_bin`) is the recommended image; without a detectable offset Wokwi assumes an app-only image at 0x10000. S3 serial is over USB Serial/JTAG via `"serialInterface": "USB_SERIAL_JTAG"`.
- Scenario page: top-level `name`, `version: 1`, `author`, `steps`; steps `delay` (units needed), `expect-pin` (`part-id`, `name`, `value`), `set-control` (`part-id`, `name`, `value`), `wait-serial`, `write-serial`, `take-screenshot`. Pushbutton control is `pressed` (1/0), pins `1.l 1.r 2.l 2.r`. The page calls scenarios alpha.
- CLI page: `wokwi-cli <dir>` with `--scenario`, `--timeout` (default 30000 ms), `--elf`, `--diagram-file`; token env `WOKWI_CLI_TOKEN`.
- Diagram page: connections are `[ "part:pin", "part:pin", "color", [] ]`; LED pins are `A` and `C`; `serialMonitor` has `display` and `newline` (serial-monitor page).
- Not on the docs pages: the XIAO board's board.json in github.com/wokwi/wokwi-boards (`boards/xiao-esp32-s3`) shows pins D0..D10, 3V3, GND, 5V, `serialInterface` USB_SERIAL_JTAG, and a virtual pin `$gpio21` on the user LED. No GPIO 0 pin.

## Unknown until the first run

- The exact part type string. The ESP32 page lists "XIAO ESP32-S3" without a slug; `board-xiao-esp32-s3` (the `board-<folder>` pattern) is a guess. Run `wokwi-cli lint` first.
- Whether `expect-pin` accepts the name `D1` for the XIAO part, or needs `GPIO2`.
- Whether the firmware's `Serial` (arduino-esp32 `CDCOnBoot=cdc`, `USBMode=default` = TinyUSB CDC on the USB-OTG port, not the hardware USB Serial/JTAG that the XIAO part declares) reaches the simulator's serial, i.e. whether flavour A works. Flavour C (`USBMode=hwcdc`) puts serial on the hardware USB Serial/JTAG and is the most likely to work; try it first if A is silent. Flavour B's UART wiring (`esp:D6`/`esp:D7` to `$serialMonitor`) is also unverified.
- Whether `wait-serial` matches substrings, consumes output in order, and has a per-step timeout (none documented; `--timeout` bounds the run).
- Whether the full-flash merged `.bin` is accepted as `firmware` with the default flash size (the XIAO definition says 8 MB), and whether NVS survives `ESP.restart()` inside one simulation (serial-setup depends on it).
- Whether 61 s of simulated time counts against the free plan's CI minutes more than expected.
- Whether the empty-password line `prov set pass` (no value) is delivered as written.
- Whether `--timeout` is simulated or wall time, and the exit codes other than timeout (42).
