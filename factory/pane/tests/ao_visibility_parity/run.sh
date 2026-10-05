#!/usr/bin/env bash
# Fingerprint parity gate: gizzi native-session fingerprint.ts (TS reference)
# vs the ao Rust port (`src/ao/native/fingerprint.rs`, P5 of the ao v3 build).
#
# Builds one fixture tree in a scratch dir, computes fingerprints with the
# REAL TS implementation via bun, then runs the env-gated Rust parity test
# (`fingerprint_parity_with_ts`) against the same fixture. Any drift in the
# sha256 scheme (path/size/mtime/ino/children) fails the gate.
#
# Usage: run.sh   (no build of the ao binary needed — uses `cargo test`)
set -euo pipefail

PARITY_DIR="$(cd "$(dirname "$0")" && pwd)"
AOE_ROOT="$(cd "$PARITY_DIR/../.." && pwd)"
REPO_ROOT="$(cd "$PARITY_DIR/../../../../.." && pwd)"
FP_TS="$REPO_ROOT/platform/packages/native-sessions/src/fingerprint.ts"

FIXTURE="$(mktemp -d /tmp/ao-visibility-parity.XXXXXX)"
trap 'rm -rf "$FIXTURE"' EXIT
mkdir -p "$FIXTURE/sub/dir"
printf '{}\n' > "$FIXTURE/sub/dir/wire.jsonl"
printf '{"id":"x"}' > "$FIXTURE/state.json"

TS_OUT="$(bun -e "
import { fingerprintPath, fingerprintPaths } from '$FP_TS';
const target = '$FIXTURE/sub/dir';
console.log(fingerprintPath(target));
console.log(fingerprintPaths(['$FIXTURE/state.json', target]));
")"
EXPECTED_SINGLE="$(sed -n 1p <<< "$TS_OUT")"
EXPECTED_MULTI="$(sed -n 2p <<< "$TS_OUT")"

echo "TS  fingerprintPath : $EXPECTED_SINGLE"
echo "TS  fingerprintPaths: $EXPECTED_MULTI"

cd "$REPO_ROOT"
PATH="/opt/homebrew/opt/zig@0.15/bin:$PATH" \
AO_FP_PARITY_SINGLE="$EXPECTED_SINGLE" \
AO_FP_PARITY_MULTI="$EXPECTED_MULTI" \
AO_FP_PARITY_TARGET="$FIXTURE/sub/dir" \
AO_FP_PARITY_TARGET2="$FIXTURE/state.json" \
cargo test -p herdr fingerprint_parity_with_ts -- --exact --nocapture

echo "PARITY OK: Rust port matches fingerprint.ts byte-for-byte"
