#!/usr/bin/env bash
# Regenerate TypeScript + Rust types from the frozen Kernel ABI 1.0.0 schemas.
# Requires: python3, npx (node), cargo + cargo-typify (`cargo install cargo-typify --locked`), rustfmt.
set -euo pipefail
V1="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
python3 "$V1/scripts/bundle_for_codegen.py" "$TMP/kernel-abi.bundle.json"

# TypeScript: types only, no runtime deps.
mkdir -p "$V1/generated/ts"
npx --yes json-schema-to-typescript@15.0.4 -i "$TMP/kernel-abi.bundle.json" \
  -o "$V1/generated/ts/kernel-abi.d.ts" --unreachableDefinitions --additionalProperties false \
  --bannerComment "/* Generated from spec/Contracts/kernel/v1/schemas (ABI 1.0.0) by scripts/regenerate.sh. DO NOT EDIT. */"

# Rust: serde derive only.
cargo typify --version | grep -q "0.8.0" || { echo "need cargo-typify 0.8.0 (cargo install cargo-typify --version 0.8.0 --locked)"; exit 1; }
cargo typify "$TMP/kernel-abi.bundle.json" -o "$V1/generated/rust/src/generated.rs" --no-builder
rustfmt --edition 2021 "$V1/generated/rust/src/generated.rs"
python3 "$V1/scripts/hash_manifest.py"
echo "regenerated: generated/ts/kernel-abi.d.ts, generated/rust/src/generated.rs"
