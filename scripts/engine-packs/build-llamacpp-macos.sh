#!/usr/bin/env bash
# Build a static, Metal-embedded llama-server for macOS arm64 from a pinned llama.cpp tag.
# Usage: build-llamacpp-macos.sh [--tag bNNNN] [--work DIR] [--out DIR]
# Output: <out>/llama-server (unsigned) + <out>/BUILD-INFO.json. Run under the build queue:
#   scripts/build-queue run engine-pack -- scripts/engine-packs/build-llamacpp-macos.sh
set -euo pipefail
TAG="${LLAMACPP_TAG:-b12414}"   # pin: commit 278a89cb787c1d2b4cff9748b1df9cfd2aa69976
WORK="${ENGINE_PACK_WORK:-$HOME/s3-build-tmp}"
OUT="${ENGINE_PACK_OUT:-$WORK/out/llamacpp-macos-arm64}"
MIN_OS="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
while [ $# -gt 0 ]; do case "$1" in --tag) TAG="$2"; shift 2;; --work) WORK="$2"; shift 2;; --out) OUT="$2"; shift 2;; *) echo "unknown $1" >&2; exit 2;; esac; done
SRC="$WORK/src"
[ -d "$SRC/.git" ] || git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/llama.cpp "$SRC"
git -C "$SRC" fetch --depth 1 origin "refs/tags/$TAG:refs/tags/$TAG" 2>/dev/null || true
git -C "$SRC" checkout -q "$TAG"
SHA="$(git -C "$SRC" rev-parse HEAD)"
rm -rf "$WORK/build" && mkdir -p "$OUT"
# Static libs + embedded metallib => one self-contained Mach-O, no default.metallib / dylibs to sign or locate.
cmake -S "$SRC" -B "$WORK/build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES=arm64 \
  -DCMAKE_OSX_DEPLOYMENT_TARGET="$MIN_OS" -DBUILD_SHARED_LIBS=OFF -DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON \
  -DGGML_NATIVE=OFF -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=ON
cmake --build "$WORK/build" --config Release -j"$(sysctl -n hw.ncpu)" --target llama-server
cp "$WORK/build/bin/llama-server" "$OUT/llama-server"
cp "$SRC/LICENSE" "$OUT/LICENSE-llama.cpp"
printf '{"tag":"%s","commit":"%s","min_macos":"%s","built":"%s"}\n' "$TAG" "$SHA" "$MIN_OS" "$(date -u +%FT%TZ)" > "$OUT/BUILD-INFO.json"
otool -L "$OUT/llama-server"; ls -l "$OUT/llama-server"
