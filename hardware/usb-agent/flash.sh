#!/usr/bin/env bash
# Build and flash the Allternit USB Agent firmware onto an ESP32-S3 board.
# Needs arduino-cli and the esp32 core 3.x:
#   brew install arduino-cli
#   arduino-cli config add board_manager.additional_urls https://espressif.github.io/arduino-esp32/package_esp32_index.json
#   arduino-cli core update-index && arduino-cli core install esp32:esp32
# Usage: hardware/usb-agent/flash.sh [serial port]      (build only: --build-only)
#
# A board already running the firmware is a TinyUSB device, so it is first
# kicked into the ROM bootloader with a 1200-baud touch; a blank board is
# flashed directly. After flashing, set it up from Allternit Desktop:
# Settings → Computers → Phones → Connect an iPhone.
set -euo pipefail
cd "$(dirname "$0")/firmware"
FQBN="esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=cdc"
BUILD="${USBA_BUILD_DIR:-build}"
port() { for p in /dev/cu.usbmodem* /dev/ttyACM*; do [[ -e $p ]] && { echo "$p"; return; }; done; }

arduino-cli compile -b "$FQBN" --output-dir "$BUILD" usb_agent
[[ "${1:-}" == "--build-only" ]] && exit 0

PORT=${1:-$(port)}
[[ -n "$PORT" ]] || { echo "No board found. Plug the ESP32-S3 into this computer with a data USB-C cable." >&2; exit 1; }
OLD=$PORT
stty -f "$PORT" 1200 2>/dev/null || stty -F "$PORT" 1200 2>/dev/null || true
for _ in $(seq 20); do sleep 0.5; PORT=$(port); [[ -n "$PORT" && "$PORT" != "$OLD" ]] && break; done
sleep 1  # let the ROM bootloader's serial come up
PORT=$(port)
arduino-cli upload -b "$FQBN" -p "$PORT" --input-dir "$BUILD" usb_agent
echo "Flashed. Now open Allternit Desktop → Settings → Computers → Phones → Connect an iPhone."
