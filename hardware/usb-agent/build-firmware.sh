#!/usr/bin/env bash
# Builds the USB Agent firmware image that Allternit Desktop ships and
# flashes from Settings → Computers → Phones → Connect an iPhone.
#
# Output (committed, so Desktop releases don't need the ESP32 toolchain):
#   surfaces/allternit-desktop/resources/phone/usb-agent/usb-agent-firmware.bin
#   surfaces/allternit-desktop/resources/phone/usb-agent/firmware.json
#
# The image is one merged file (bootloader, partition table, boot_app0, app)
# written at offset 0. It links the arduino-esp32 core (LGPL-2.1); its full
# source is this folder, so anyone can rebuild and relink it.
# Needs arduino-cli + esp32 core 3.x (see flash.sh). Run after any firmware change.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$repo/surfaces/allternit-desktop/resources/phone/usb-agent"
build="$(mktemp -d)"
trap 'rm -rf "$build"' EXIT
FQBN="esp32:esp32:esp32s3:USBMode=default,CDCOnBoot=cdc"

arduino-cli compile -b "$FQBN" --warnings all --output-dir "$build" "$here/firmware/usb_agent" >/dev/null
mkdir -p "$out"
# Trailing 0xFF is erased flash: trim it (flashing writes the same result).
python3 -c 'import sys; d=open(sys.argv[1],"rb").read().rstrip(b"\xff"); d+=b"\xff"*(-len(d)%4096); open(sys.argv[2],"wb").write(d)' \
  "$build/usb_agent.ino.merged.bin" "$out/usb-agent-firmware.bin"
version="$(sed -n 's/^#define FIRMWARE_VERSION "\(.*\)"/\1/p' "$here/firmware/usb_agent/usb_agent.ino")"
sha="$(shasum -a 256 "$out/usb-agent-firmware.bin" | cut -d' ' -f1)"
size="$(wc -c < "$out/usb-agent-firmware.bin" | tr -d ' ')"
core="$(arduino-cli core list | awk '$1=="esp32:esp32"{print $2}')"
cat > "$out/firmware.json" <<JSON
{
  "version": "$version",
  "chip": "esp32s3",
  "offset": 0,
  "size": $size,
  "sha256": "$sha",
  "fqbn": "$FQBN",
  "core": "esp32:esp32@$core",
  "source": "hardware/usb-agent/firmware/usb_agent"
}
JSON
echo "Built USB Agent firmware $version ($size bytes, sha256 $sha)"
