#!/usr/bin/env bash
# Build a colibri engine pack for Linux (x64, arm64) or Windows (x64). No compiling happens here.
# Contract: handoffs CONTRACT-colibri-unkernel.md section 3 (option A: bundled Python gateway).
#
# Usage: build-colibri.sh --os linux|windows --arch x64|arm64 [--out DIR] [--work DIR]
# Output: <out>/colibri-cpu-<os>-<arch>/ with
#   python/                        python-build-standalone (install_only_stripped), pinned below
#   colibri/c/openai_server.py     colibri's gateway .py files at the pin (+ c/tools), from the fork checkout
#   colibri/c/<engine binaries>    from the pinned upstream release archive, verified against SHA256SUMS.txt
#   LICENSE NOTICE THIRD_PARTY_NOTICES.md   from colibri;  LICENSE-python from CPython
#   pack.json                      same manifest as the llama.cpp packs (make-manifest.py)
# The engine pack resolver returns the bundled Python as the entrypoint (python/bin/python3, Windows
# python/python.exe); the manager runs colibri/c/openai_server.py with it on 127.0.0.1 only.
# Why the .py files come from the checkout and not the archive: the engine finds its binaries next to
# openai_server.py (default_engine in openai_server.py), and the archive contents are unverified
# (contract 3.3). Files from the archive are copied as is, then the checkout's .py files overlay c/.
# There are no macOS or Windows Arm colibri archives (contract 3.4), so those hosts get no pack here.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

# --- pins (change together, with a new sha256 each) ---------------------------------------------
COLIBRI_TAG="${COLIBRI_TAG:-v2.2.0}"
COLIBRI_COMMIT="e479104ae01bdc40ce81e675299b1d230891b321"      # Allternit/colibri tag v2.2.0 (dereferenced)
COLIBRI_FORK="https://github.com/Allternit/colibri"
COLIBRI_RELEASE="https://github.com/JustVugg/colibri/releases/download/$COLIBRI_TAG"
PBS_RELEASE="20261110"; PBS_PYTHON="3.12.16"                   # astral-sh/python-build-standalone
PBS_SHA_LINUX_X64="6a9e62dad033790aef722dbd370e0ce76ce17b73627ba3ae679f537049350b39"
PBS_SHA_LINUX_ARM64="b6a41defec4975d94b4ff1950236e7383e5c02ba40ea48c9ec33c66087a35aab"
PBS_SHA_WINDOWS_X64="355ee61b8207b91574491b1c6dc9b0ebb4408fc56bfafe498f3ef439314b52dc"
# -------------------------------------------------------------------------------------------------

OS="" ARCH="" OUT="${ENGINE_PACK_OUT:-$PWD/out}" WORK="${ENGINE_PACK_WORK:-$PWD/work}"
while [ $# -gt 0 ]; do case "$1" in
  --os) OS="$2"; shift 2;; --arch) ARCH="$2"; shift 2;;
  --out) OUT="$2"; shift 2;; --work) WORK="$2"; shift 2;; *) echo "unknown $1" >&2; exit 2;; esac; done
[ -n "$OS" ] && [ -n "$ARCH" ] || { echo "need --os --arch" >&2; exit 2; }

case "$OS-$ARCH" in
  linux-x64)    ASSET="colibri-$COLIBRI_TAG-linux-x86_64.tar.gz";  PBS_TRIPLE="x86_64-unknown-linux-gnu";  PBS_SHA="$PBS_SHA_LINUX_X64";;
  linux-arm64)  ASSET="colibri-$COLIBRI_TAG-linux-aarch64.tar.gz"; PBS_TRIPLE="aarch64-unknown-linux-gnu"; PBS_SHA="$PBS_SHA_LINUX_ARM64";;
  windows-x64)  ASSET="colibri-$COLIBRI_TAG-windows-x86_64.zip";   PBS_TRIPLE="x86_64-pc-windows-msvc";    PBS_SHA="$PBS_SHA_WINDOWS_X64";;
  *) echo "no colibri archive for $OS-$ARCH" >&2; exit 2;;
esac
PBS_ASSET="cpython-$PBS_PYTHON+$PBS_RELEASE-$PBS_TRIPLE-install_only_stripped.tar.gz"
PACK_ID="colibri-cpu-$OS-$ARCH"
PACK="$OUT/$PACK_ID"; W="$WORK/$PACK_ID"
if [ "$OS" = windows ]; then PYBIN="python/python.exe"; else PYBIN="python/bin/python3"; fi
PY="$(command -v python3 || command -v python)"

sha256_of() { "$PY" -c 'import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"; }
fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }

rm -rf "$W" "$PACK" && mkdir -p "$W/archive" "$W/src-dl" "$PACK"

# 1. colibri release archive, checked against the release's own SHA256SUMS.txt
fetch "$COLIBRI_RELEASE/SHA256SUMS.txt" "$W/SHA256SUMS.txt"
fetch "$COLIBRI_RELEASE/$ASSET" "$W/$ASSET"
WANT="$(awk -v a="$ASSET" '{f=$2; sub(/^\*/,"",f)} f==a {print $1}' "$W/SHA256SUMS.txt")"
[ -n "$WANT" ] || { echo "$ASSET not listed in SHA256SUMS.txt" >&2; exit 1; }
[ "$(sha256_of "$W/$ASSET")" = "$WANT" ] || { echo "sha256 mismatch for $ASSET" >&2; exit 1; }
case "$ASSET" in
  *.zip) "$PY" -m zipfile -e "$W/$ASSET" "$W/archive";;
  *)     tar -xzf "$W/$ASSET" -C "$W/archive";;
esac
ROOT="$W/archive"                                  # descend through a single wrapper folder
while [ "$(ls -A "$ROOT" | wc -l)" -eq 1 ] && [ -d "$ROOT/$(ls -A "$ROOT")" ]; do ROOT="$ROOT/$(ls -A "$ROOT")"; done

# 2. colibri source at the pin (the gateway .py files and the licence files)
git clone -q --branch "$COLIBRI_TAG" "$COLIBRI_FORK" "$W/src"
[ "$(git -C "$W/src" rev-parse HEAD)" = "$COLIBRI_COMMIT" ] || { echo "$COLIBRI_TAG is not $COLIBRI_COMMIT" >&2; exit 1; }
[ -f "$W/src/c/openai_server.py" ] || { echo "c/openai_server.py missing at the pin" >&2; exit 1; }

# 3. lay out colibri/c: archive files first, then the pinned .py files and c/tools on top
mkdir -p "$PACK/colibri/c"
cp -R "$ROOT/." "$PACK/colibri/"
# The engine binaries must sit next to openai_server.py. Archives that put them at the top level
# (or in bin/) get them moved into c/; files already under c/ stay.
find "$PACK/colibri" -type f \( -perm -u+x -o -name '*.exe' -o -name '*.dll' \) -not -path "$PACK/colibri/c/*" \
  -not -name '*.py' -not -name '*.sh' -not -name '*.bat' | while read -r f; do mv -n "$f" "$PACK/colibri/c/"; done
cp "$W"/src/c/*.py "$PACK/colibri/c/"
[ -d "$W/src/c/tools" ] && { mkdir -p "$PACK/colibri/c/tools"; cp -R "$W/src/c/tools/." "$PACK/colibri/c/tools/"; }
find "$PACK/colibri/c" -maxdepth 1 -type f \( -name 'colibri' -o -name 'colibri.exe' \) | grep -q . \
  || { echo "no colibri engine binary in $PACK/colibri/c (archive layout changed?)" >&2; exit 1; }
for f in LICENSE NOTICE THIRD_PARTY_NOTICES.md; do cp "$W/src/$f" "$PACK/$f"; done

# 4. python-build-standalone, checked against the pinned sha256
fetch "https://github.com/astral-sh/python-build-standalone/releases/download/$PBS_RELEASE/$PBS_ASSET" "$W/$PBS_ASSET"
[ "$(sha256_of "$W/$PBS_ASSET")" = "$PBS_SHA" ] || { echo "sha256 mismatch for $PBS_ASSET" >&2; exit 1; }
tar -xzf "$W/$PBS_ASSET" -C "$PACK"                # unpacks to python/
[ -f "$PACK/$PYBIN" ] || { echo "$PYBIN missing after unpack" >&2; exit 1; }
PYLIC="$(find "$PACK/python" -maxdepth 4 -name 'LICENSE.txt' | head -n 1)"
[ -n "$PYLIC" ] || { echo "CPython LICENSE.txt not found in python/" >&2; exit 1; }
cp "$PYLIC" "$PACK/LICENSE-python"

"$PY" "$HERE/make-manifest.py" "$PACK" --engine colibri --backend cpu --version "$COLIBRI_TAG" \
  --os "$OS" --arch "$ARCH" --min-os "" --source-commit "$COLIBRI_COMMIT" --entrypoint "$PYBIN"
du -sh "$PACK" 2>/dev/null || true
