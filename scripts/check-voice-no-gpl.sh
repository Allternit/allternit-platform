#!/usr/bin/env bash
# Fail if the allternit-voice-service binary contains espeak-ng
# (GPL-3.0-or-later). TTS, and with it espeak-ng, lives only in the separate
# allternit-tts program. espeak-ng's code carries its name in error strings
# and data paths, so any linked espeak code shows up as "espeak" bytes.
# Usage: scripts/check-voice-no-gpl.sh <path-to-voice-service-binary>
set -euo pipefail
bin="${1:?usage: $0 <voice-service binary>}"
[ -f "$bin" ] || { echo "check-voice-no-gpl: $bin not found" >&2; exit 2; }
if grep -a -i -q 'espeak' "$bin"; then
  echo "check-voice-no-gpl: FAIL: $bin contains espeak-ng code (GPL-3.0)." >&2
  echo "  See the no_espeak module in services/voice/src/main.rs." >&2
  exit 1
fi
echo "check-voice-no-gpl: OK: no espeak-ng in $bin"
