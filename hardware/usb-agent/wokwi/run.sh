#!/usr/bin/env bash
# Runs the boot, serial-setup and arm-button scenarios on Wokwi's simulated
# XIAO ESP32-S3. Needs WOKWI_CLI_TOKEN and wokwi-cli. Run build.sh first.
# FLAVOUR=a (default; the shipped USB mode, TinyUSB CDC), b (UART0) or
# c (hardware USB Serial/JTAG). build.sh explains the three.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
flavour="${FLAVOUR:-a}"

if [ -z "${WOKWI_CLI_TOKEN:-}" ]; then
  echo "WOKWI_CLI_TOKEN is not set"
  exit 2
fi
if ! command -v wokwi-cli >/dev/null 2>&1; then
  echo "wokwi-cli is not installed (see wokwi/README.md)"
  exit 2
fi
bin="$here/build/$flavour/usb_agent.ino.merged.bin"
elf="$here/build/$flavour/usb_agent.ino.elf"
if [ ! -f "$bin" ]; then
  echo "No firmware at $bin. Run: bash hardware/usb-agent/wokwi/build.sh"
  exit 2
fi

# Stage a project directory so each flavour gets its own toml and diagram.
proj="$here/build/project-$flavour"
rm -rf "$proj"
mkdir -p "$proj/scenarios"
cp "$here"/scenarios/*.yaml "$proj/scenarios/"
cp "$bin" "$proj/firmware.bin"
[ -f "$elf" ] && cp "$elf" "$proj/firmware.elf"
if [ "$flavour" = b ]; then cp "$here/diagram.uart.json" "$proj/diagram.json"; else cp "$here/diagram.json" "$proj/diagram.json"; fi
cat > "$proj/wokwi.toml" <<TOML
[wokwi]
version = 1
firmware = 'firmware.bin'
elf = 'firmware.elf'
TOML

passed=0
for s in boot serial-setup arm-button; do
  echo "== $s (flavour $flavour)"
  # The arm scenario waits 61 s of simulated time; give every run room.
  if (cd "$proj" && wokwi-cli . --scenario "scenarios/$s.test.yaml" --timeout 180000); then
    passed=$((passed + 1))
  else
    echo "FAILED: $s"
  fi
done
if [ "$passed" -eq 3 ]; then
  echo "3 scenarios passed"
  exit 0
fi
echo "$passed of 3 scenarios passed"
exit 1
