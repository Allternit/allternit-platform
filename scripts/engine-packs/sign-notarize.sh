#!/usr/bin/env bash
# Sign (Developer ID, hardened runtime), notarize and verify an engine pack on macOS.
# Usage: sign-notarize.sh <pack-dir>   (signs only; build-pack-macos.sh then writes the manifest, zips and notarizes)
# Identity + notary profile match Desktop (build-local.cjs / notarize.cjs): "Developer ID Application" cert
# found in the login keychain, notarytool keychain profile "allternit". No secrets are read or printed.
# Entitlements: none. A static llama-server needs no JIT, no unsigned memory, no library-validation opt-out
# (Metal shaders are embedded and compiled by the system Metal driver, which hardened runtime allows).
set -euo pipefail
PACK="$1"
IDENT="${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}"
[ -n "$IDENT" ] || { echo "no Developer ID Application identity in keychain" >&2; exit 1; }
T0=$(date +%s)
# 1. sign every Mach-O (identifier is explicit so it does not change with the file name)
find "$PACK" -type f | while read -r f; do
  if file "$f" | grep -q 'Mach-O'; then
    id="com.allternit.engine.$(basename "$f")"
    codesign --force --timestamp --options runtime -s "$IDENT" -i "$id" "$f"
    codesign --verify --strict --verbose=2 "$f"
  fi
done
T1=$(date +%s)
echo "signed in $((T1-T0))s"
