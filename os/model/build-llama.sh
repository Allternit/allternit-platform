#!/bin/sh
# Build a static, CPU-only llama-server (plan step 29). POSIX sh.
#   build-llama.sh [--out DIR]     writes DIR/llama-server (default os/target/llama)
# Static glibc, not musl: llama.cpp is C++ and a musl C++ toolchain is not packaged on the
# runner. The binary is verified static (no interpreter) because the image has no libc.
set -eu
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT="$SCRIPT_DIR/../target/llama"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || { echo "build-llama: --out needs a value" >&2; exit 2; }; OUT=$2; shift 2 ;;
    *) echo "build-llama: unknown flag $1" >&2; exit 2 ;;
  esac
done
# shellcheck disable=SC1091
. "$SCRIPT_DIR/LLAMA"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
SRC="$OUT/src-$LLAMA_TAG"
JOBS=$(nproc 2>/dev/null || echo 2)

if [ ! -f "$SRC/CMakeLists.txt" ]; then
  git clone --quiet --depth 1 --branch "$LLAMA_TAG" https://github.com/ggml-org/llama.cpp "$SRC"
fi
GOT=$(git -C "$SRC" rev-parse HEAD)
[ "$GOT" = "$LLAMA_COMMIT" ] || { echo "build-llama: $LLAMA_TAG is $GOT, expected $LLAMA_COMMIT" >&2; exit 1; }

cmake -S "$SRC" -B "$OUT/build" -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF -DGGML_BACKEND_DL=OFF -DGGML_NATIVE=OFF -DGGML_OPENMP=OFF \
  -DGGML_CUDA=OFF -DGGML_METAL=OFF -DGGML_VULKAN=OFF -DGGML_BLAS=OFF \
  -DLLAMA_CURL=OFF -DLLAMA_OPENSSL=OFF \
  -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_SERVER=ON \
  -DCMAKE_EXE_LINKER_FLAGS="-static" -DCMAKE_FIND_LIBRARY_SUFFIXES=".a"
cmake --build "$OUT/build" --target llama-server -j"$JOBS"

BIN="$OUT/build/bin/llama-server"
[ -x "$BIN" ] || { echo "build-llama: no $BIN" >&2; exit 1; }
if command -v file >/dev/null 2>&1; then
  file "$BIN" | grep -qi 'statically linked' || { echo "build-llama: not static: $(file "$BIN")" >&2; exit 1; }
fi
strip "$BIN" 2>/dev/null || true
cp "$BIN" "$OUT/llama-server"
echo "build-llama: $OUT/llama-server ($LLAMA_TAG)"
ls -l "$OUT/llama-server"
