#!/usr/bin/env bash
# Regenerate TypeScript + Rust types from the AllternitOS os/v1 schemas (same tools and
# versions as spec/Contracts/kernel/v1/scripts/regenerate.sh).
# Requires: python3, npx (node), cargo + cargo-typify 0.8.0, rustfmt.
# The Rust half replaces the hand-written generated/rust/src/generated.rs written in the
# coding session (cargo is blocked there by the train guard); the build train runs this.
set -euo pipefail
V1="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
python3 "$V1/scripts/bundle_for_codegen.py" "$TMP/os-contracts.bundle.json"

mkdir -p "$V1/generated/ts"
npx --yes json-schema-to-typescript@15.0.4 -i "$TMP/os-contracts.bundle.json" \
  -o "$V1/generated/ts/os-contracts.d.ts" --unreachableDefinitions --additionalProperties false \
  --bannerComment "/* Generated from spec/Contracts/os/v1/schemas (os contracts 1.0.0) by scripts/regenerate.sh. DO NOT EDIT. */"

if [ "${SKIP_RUST:-0}" != "1" ]; then
  cargo typify --version | grep -q "0.8.0" || { echo "need cargo-typify 0.8.0 (cargo install cargo-typify --version 0.8.0 --locked)"; exit 1; }
  cargo typify "$TMP/os-contracts.bundle.json" -o "$V1/generated/rust/src/generated.rs" --no-builder
  rustfmt --edition 2021 "$V1/generated/rust/src/generated.rs"
fi
python3 "$V1/scripts/hash_manifest.py"
echo "regenerated: generated/ts/os-contracts.d.ts${SKIP_RUST:+ (rust skipped)}"
