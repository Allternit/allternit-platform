#!/usr/bin/env bash
# Fail if the allternit-voice-service binary contains espeak-ng
# (GPL-3.0-or-later). TTS, and with it espeak-ng, lives only in the separate
# allternit-tts program.
#
# The guard looks for espeak-ng CODE, not for the word "espeak". sherpa-onnx
# (Apache-2.0) mentions espeak in its own code ("has_espeak", "use espeak for
# %s", "wespeaker", espeak data file names such as phontab), so a bare
# `grep espeak` is a false positive on every build that links sherpa-onnx.
#
# Evidence (Linux voice-service build, allternit-standby, 2026-10-03): an
# unstripped `nm` had 0 espeak-ng internal functions (espeak_ng_*,
# SpeakNextClause, TranslateClause, LoadVoice, LookupDictList, SetTranslator).
# The only espeak symbols were the 3 shim stubs from the no_espeak module in
# services/voice/src/main.rs (espeak_Initialize, espeak_SetVoiceByName,
# espeak_TextToPhonemesWithTerminator), which are allowed.
#
# Two checks, both portable (macOS, Linux, Windows/MSYS bash; the .exe case is
# a string check only, since MSYS has no reliable nm for PE files):
#   1. espeak-ng-only strings in the bytes: the data directory name, the
#      error strings of its translator, its init banner.
#   2. when the binary still has symbols (nm works): any espeak_ng_ symbol or
#      espeak-ng internal function name.
#
# Usage: scripts/check-voice-no-gpl.sh <path-to-voice-service-binary>
#        scripts/check-voice-no-gpl.sh --self-test   (needs built binaries)
set -euo pipefail

# Strings that only espeak-ng's own code carries.
STRING_MARKERS=(
  'espeak-ng-data'
  'Unknown phoneme'
  'Bad phoneme'
  'eSpeak NG'
  'espeakINITIALIZE'
)
# Symbol names (as a regex over `nm` output) that only espeak-ng defines.
# The three no_espeak shims are exempt by not being listed here.
SYMBOL_REGEX='(^|[ _])(espeak_ng_[A-Za-z0-9_]*|SpeakNextClause|TranslateClause|LoadVoice|LookupDictList|SetTranslator)$'

check() {
  local bin="$1" hit=""
  [ -f "$bin" ] || { echo "check-voice-no-gpl: $bin not found" >&2; return 2; }

  local m
  for m in "${STRING_MARKERS[@]}"; do
    if grep -a -F -q -- "$m" "$bin"; then
      hit="string \"$m\""
      break
    fi
  done

  if [ -z "$hit" ]; then
    case "$bin" in
      *.exe) ;; # string check only on Windows
      *)
        if command -v nm >/dev/null 2>&1; then
          local sym
          sym="$(nm "$bin" 2>/dev/null | grep -E "$SYMBOL_REGEX" | head -n 1 || true)"
          [ -n "$sym" ] && hit="symbol \"${sym##* }\""
        fi
        ;;
    esac
  fi

  if [ -n "$hit" ]; then
    echo "check-voice-no-gpl: FAIL: $bin contains espeak-ng code (GPL-3.0): $hit." >&2
    echo "  See the no_espeak module in services/voice/src/main.rs." >&2
    return 1
  fi
  echo "check-voice-no-gpl: OK: no espeak-ng in $bin"
}

if [ "${1:-}" = "--self-test" ]; then
  # The guard must FAIL on the GPL child and PASS on the voice service.
  root="$(cd "$(dirname "$0")/.." && pwd)"
  tdir="${CARGO_TARGET_DIR:-$root/target}/release"
  tts="${2:-$tdir/allternit-tts}"
  svc="${3:-$tdir/allternit-voice-service}"
  fail=0
  if check "$tts" >/dev/null 2>&1; then
    echo "self-test: FAIL: guard passed allternit-tts ($tts), which links espeak-ng" >&2
    fail=1
  else
    echo "self-test: ok: guard rejects allternit-tts"
  fi
  if check "$svc"; then
    echo "self-test: ok: guard accepts voice-service"
  else
    echo "self-test: FAIL: guard rejected voice-service ($svc)" >&2
    fail=1
  fi
  exit "$fail"
fi

bin="${1:?usage: $0 <voice-service binary> | --self-test}"
check "$bin" || exit $?
