#!/usr/bin/env bash
# build -> sign -> manifest -> zip -> notarize -> verify. Usage: build-pack-macos.sh [outdir]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${ENGINE_PACK_WORK:-$HOME/s3-build-tmp}"
RAW="$WORK/out/llamacpp-macos-arm64"
OUTDIR="${1:-$WORK/out}"
PACK="$OUTDIR/pack/llamacpp-metal-macos-arm64"
ZIP="$OUTDIR/llamacpp-metal-macos-arm64.zip"
PROFILE="${NOTARY_PROFILE:-allternit}"
"$HERE/build-llamacpp-macos.sh"
rm -rf "$OUTDIR/pack" && mkdir -p "$PACK/bin"
cp "$RAW/llama-server" "$PACK/bin/llama-server"; cp "$RAW/LICENSE-llama.cpp" "$PACK/"
TAG=$(python3 -c "import json;print(json.load(open('$RAW/BUILD-INFO.json'))['tag'])")
SHA=$(python3 -c "import json;print(json.load(open('$RAW/BUILD-INFO.json'))['commit'])")
"$HERE/sign-notarize.sh" "$PACK"
python3 "$HERE/make-manifest.py" "$PACK" --engine llamacpp --backend metal --version "$TAG" --os macos --arch arm64 \
  --min-os 13.0 --source-commit "$SHA" --entrypoint bin/llama-server
rm -f "$ZIP"; (cd "$OUTDIR/pack" && ditto -c -k --keepParent llamacpp-metal-macos-arm64 "$ZIP")
T0=$(date +%s)
xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait 2>&1 | grep -vi password
echo "notarize wait $(( $(date +%s)-T0 ))s"
# Bare Mach-O binaries cannot be stapled; Gatekeeper checks the ticket online (and quarantined unzips pass).
codesign --verify --deep --strict --verbose=2 "$PACK/bin/llama-server"
codesign -dvv "$PACK/bin/llama-server" 2>&1 | grep -E 'Identifier|Authority|flags|TeamIdentifier|Timestamp'
ls -l "$ZIP"; shasum -a 256 "$ZIP"
