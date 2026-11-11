#!/usr/bin/env bash
# Builds the REAL USB Agent firmware for Wokwi into wokwi/build/ (git-ignored).
# Never writes to surfaces/allternit-desktop/resources/phone/.
#
#   A  the shipped FQBN (USBMode=default, CDCOnBoot=cdc: serial is TinyUSB CDC
#      on the USB-OTG port, next to the HID mouse and keyboard)
#   B  CDCOnBoot=default (serial on UART0)
#   C  USBMode=hwcdc, CDCOnBoot=cdc (serial on the S3's hardware USB
#      Serial/JTAG, the interface Wokwi's XIAO part declares)
#
# The Wokwi XIAO part does not expose GPIO 0 (BOOT) or GPIO 21 (user LED) as
# wirable pins, so both flavours are built with the arm button on GPIO 1 (D0)
# and the arm LED on GPIO 2 (D1). The firmware honours these through #ifndef
# guards (usb_agent.ino lines 72-77), so the firmware source is not edited.
# These three images are NOT the shipped image; they only run in the simulator.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
fw="$here/../firmware/usb_agent"
out="$here/build"
FLAGS="-DARM_BUTTON_PIN=1 -DARM_LED_PIN=2"

command -v arduino-cli >/dev/null || { echo "arduino-cli is not installed" >&2; exit 2; }
mkdir -p "$out"

build_one() { # name fqbn
  local name="$1" fqbn="$2"
  rm -rf "$out/$name" "$out/tmp-$name"
  arduino-cli compile -b "$fqbn" \
    --build-path "$out/tmp-$name" --output-dir "$out/$name" \
    --build-property "compiler.cpp.extra_flags=$FLAGS" \
    --build-property "compiler.c.extra_flags=$FLAGS" \
    "$fw" >/dev/null
  rm -rf "$out/tmp-$name"
  echo "$out/$name/usb_agent.ino.merged.bin"
}

build_one a "esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=cdc"
build_one b "esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=default"
build_one c "esp32:esp32:esp32s3:USBMode=hwcdc,CDCOnBoot=cdc"
