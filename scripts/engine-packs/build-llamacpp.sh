#!/usr/bin/env bash
# Build a llama-server engine pack for Linux or Windows from the pinned llama.cpp tag.
# macOS uses build-llamacpp-macos.sh + build-pack-macos.sh (signed, notarized).
#
# Usage: build-llamacpp.sh --backend cpu|vulkan|cuda|rocm --os linux|windows --arch x64|arm64 [--out DIR]
# Output: <out>/<pack_id>/ with bin/llama-server[.exe] (+ shared libs), LICENSE-llama.cpp and pack.json.
# Runs in CI (.github/workflows/engine-packs.yml). Windows runs under Git Bash with MSVC on PATH.
# Backends (spec/on-device/engine-pack-spike.md, "Windows and Linux packs"):
#   cpu     GGML_CPU_ALL_VARIANTS + GGML_BACKEND_DL: per-ISA ggml-cpu-* libraries picked at runtime
#   vulkan  needs the Vulkan SDK (glslc) to build; users need only the loader (driver)
#   cuda    CUDA 12.x, archs 75;80;86;89;120. libcuda comes from the user's driver, never shipped
#   rocm    HIP; the user's ROCm install provides the runtime (opt-in, Linux only)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
TAG="${LLAMACPP_TAG:-b12414}"   # same pin as macOS: commit 278a89cb787c1d2b4cff9748b1df9cfd2aa69976
BACKEND="" OS="" ARCH="" OUT="${ENGINE_PACK_OUT:-$PWD/out}" WORK="${ENGINE_PACK_WORK:-$PWD/work}"
while [ $# -gt 0 ]; do case "$1" in
  --backend) BACKEND="$2"; shift 2;; --os) OS="$2"; shift 2;; --arch) ARCH="$2"; shift 2;;
  --out) OUT="$2"; shift 2;; --work) WORK="$2"; shift 2;; *) echo "unknown $1" >&2; exit 2;; esac; done
[ -n "$BACKEND" ] && [ -n "$OS" ] && [ -n "$ARCH" ] || { echo "need --backend --os --arch" >&2; exit 2; }
PACK_ID="llamacpp-$BACKEND-$OS-$ARCH"
SRC="$WORK/src"; BUILD="$WORK/build-$PACK_ID"; PACK="$OUT/$PACK_ID"
[ -d "$SRC/.git" ] || git clone --depth 1 --branch "$TAG" https://github.com/ggml-org/llama.cpp "$SRC"
git -C "$SRC" checkout -q "$TAG"
SHA="$(git -C "$SRC" rev-parse HEAD)"

FLAGS=(-DCMAKE_BUILD_TYPE=Release -DGGML_NATIVE=OFF -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF
       -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=ON
       -DBUILD_SHARED_LIBS=ON -DGGML_BACKEND_DL=ON -DGGML_CPU_ALL_VARIANTS=ON)
case "$BACKEND" in
  cpu) ;;
  vulkan) FLAGS+=(-DGGML_VULKAN=ON) ;;
  cuda) FLAGS+=(-DGGML_CUDA=ON "-DCMAKE_CUDA_ARCHITECTURES=75;80;86;89;120") ;;
  rocm) FLAGS+=(-DGGML_HIP=ON "-DAMDGPU_TARGETS=${AMDGPU_TARGETS:-gfx1030;gfx1100;gfx1101;gfx1102;gfx1200;gfx1201;gfx90a;gfx942}") ;;
  *) echo "unknown backend $BACKEND" >&2; exit 2 ;;
esac
if [ "$OS" = linux ]; then
  # Shared libs next to the binary: RPATH $ORIGIN so the pack runs from any folder.
  FLAGS+=("-DCMAKE_INSTALL_RPATH=\$ORIGIN" -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON)
fi
if [ "$OS" = windows ] && [ "$ARCH" = arm64 ]; then
  FLAGS+=(-A ARM64)
  # The all-variants CPU build is x86 only.
  FLAGS=("${FLAGS[@]/-DGGML_CPU_ALL_VARIANTS=ON/-DGGML_CPU_ALL_VARIANTS=OFF}")
fi
cmake -S "$SRC" -B "$BUILD" "${FLAGS[@]}"
cmake --build "$BUILD" --config Release -j "$(nproc 2>/dev/null || echo 4)" --target llama-server

rm -rf "$PACK" && mkdir -p "$PACK/bin"
BIN_DIR="$BUILD/bin"; [ -d "$BUILD/bin/Release" ] && BIN_DIR="$BUILD/bin/Release"
EXE=llama-server; [ "$OS" = windows ] && EXE=llama-server.exe
cp "$BIN_DIR/$EXE" "$PACK/bin/"
# ggml-base, ggml-cpu-*, ggml-vulkan / ggml-cuda / ggml-hip, libllama, libmtmd ...
find "$BIN_DIR" "$BUILD" -maxdepth 3 \( -name '*.so' -o -name '*.so.*' -o -name '*.dll' \) -exec cp -n {} "$PACK/bin/" \; 2>/dev/null || true
cp "$SRC/LICENSE" "$PACK/LICENSE-llama.cpp"
python3 "$HERE/make-manifest.py" "$PACK" --engine llamacpp --backend "$BACKEND" --version "$TAG" \
  --os "$OS" --arch "$ARCH" --min-os "" --source-commit "$SHA" --entrypoint "bin/$EXE"
ls -la "$PACK/bin"
