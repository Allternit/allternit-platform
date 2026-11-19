#!/usr/bin/env bash
# Build the Linux x64 sidecars so they run on every glibc >= GLIBC_FLOOR.
# One recipe, used by CI (.github/workflows/release-desktop.yml, build-linux)
# and by the standby runtime builder. Plain `cargo build` links against the
# build host's glibc (Ubuntu 24.04 = 2.39), which broke Debian 12 / Ubuntu 22.04.
#
# Why the floor is 2.27 (Ubuntu 18.04, glibc 2.27): it is the oldest glibc on which
# the Electron shell itself starts. The shell binary needs GLIBC_2.25; Ubuntu 18.04
# (2.27) and Debian 10 (2.28) were tested in docker and reach the onboarding window;
# no supported distro ships 2.25/2.26. The floor is therefore set by what users run,
# not by our build hosts. scripts/check-elf-floor.mjs fails CI if any packaged ELF,
# the Electron shell included, needs more, so an Electron upgrade cannot raise it
# silently.
#
# usage: scripts/build-linux-sidecars.sh <out-dir> [component ...]
#   components: api voice files-mount factory cua-driver   (default: api voice files-mount)
#   api -> allternit-api   voice -> allternit-voice-service + allternit-tts
#   files-mount -> allternit-files-mount   factory -> allternit-factory
#   cua-driver -> cua-driver (trycua/cua built from source at the version and commit
#     pinned in surfaces/allternit-desktop/scripts/prepare-cua-driver.cjs; the
#     upstream Linux prebuilt needs glibc 2.30). Needs libx11-dev libxi-dev
#     libxtst-dev libxkbcommon-dev on the build host.
#
# Two build modes, chosen per component:
#   api, files-mount, factory, cua-driver: cargo-zigbuild with a `.<glibc>` target
#     suffix pins the symbol versions at ZIG_GLIBC (2.17, older than any distro we
#     could meet, at no cost; factory and cua-driver at GLIBC_FLOOR, see below). Zig +
#     cargo-zigbuild pinned below, Zig checksum-verified.
#   voice: built inside ubuntu:18.04 (glibc 2.27 = GLIBC_FLOOR) because zig cannot link
#     it: the prebuilt sherpa-onnx static lib (GCC 11) needs libstdc++ symbols zig's
#     libc++ lacks (`std::string::_Rep::_S_empty_rep_storage`,
#     `__throw_bad_array_new_length`). The container compiles and links with the
#     toolchain PPA's newest g++ (12, else 11) and links its libstdc++ statically
#     (bionic ships only GLIBCXX 3.4.25), plus a source-built OpenSSL 3 statically
#     (bionic has 1.1.1, soname libssl.so.1.1, absent on Debian 12 / Ubuntu 22.04).
# env: REPO_DIR=checkout to build (default: the repo holding this script)
#      ZIG=/path/to/zig (reused if it reports the pinned version, else installed)
#      CARGO_TARGET_DIR, CARGO_BUILD_JOBS as usual (the docker build mounts
#      CARGO_TARGET_DIR/docker as its target dir; cua-driver uses CARGO_TARGET_DIR/cua)
#      CUA_SRC_DIR=existing trycua/cua checkout (default: cloned under CARGO_TARGET_DIR)
set -euo pipefail

# The ONE floor constant. scripts/check-elf-floor.mjs reads this line.
GLIBC_FLOOR=2.27
# glibc symbol version the zigbuild components are pinned to (<= GLIBC_FLOOR).
ZIG_GLIBC=2.17
# factory and cua-driver link at GLIBC_FLOOR itself, because zig at 2.17 fails on symbols
# glibc added in 2.27: factory's fat-LTO std references copy_file_range, and cua-driver
# calls memfd_create (Wayland shm buffers): `ld.lld: error: undefined symbol: ...`.

ZIG_VERSION=0.15.2
# Same version the macOS job in release-desktop.yml pins; linux-x86_64 tarball sha256
# from https://ziglang.org/download/index.json.
ZIG_SHA256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
CARGO_ZIGBUILD_VERSION=0.20.1
RUST_VERSION=1.94.1            # same toolchain as the workflow's dtolnay/rust-toolchain pin
PROTOC_VERSION=28.3
PROTOC_SHA256=0ad949f04a6a174da83cdcbdb36dee0a4925272a5b6d83f79a6bf9852076d53f
OPENSSL_VERSION=3.0.15
OPENSSL_SHA256=23c666d0edf20f14249b3d8f0368acaee9ab585b09e1de82107c66e1f3ec9533
TARGET=x86_64-unknown-linux-gnu

REPO="${REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
[ $# -ge 1 ] || { sed -n '2,42p' "${BASH_SOURCE[0]}"; exit 2; }
OUT="$1"; shift
COMPONENTS=("$@"); [ ${#COMPONENTS[@]} -gt 0 ] || COMPONENTS=(api voice files-mount)
mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"

# Linking the allternit-api bins passes >3000 objects to zig, which opens them all
# at once: the default 1024 fds fails with ProcessFdQuotaExceeded.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true

# ── inside the ubuntu:18.04 container: `voice-docker` ─────────────────────────────
if [ "${COMPONENTS[*]}" = voice-docker ]; then
  ldd --version | head -1
  [ "$(ldd --version | head -1 | grep -o '[0-9]*\.[0-9]*$')" = "$GLIBC_FLOOR" ] || { echo "container glibc is not $GLIBC_FLOOR" >&2; exit 1; }
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq build-essential curl ca-certificates cmake clang pkg-config perl make unzip software-properties-common >/dev/null
  # Pinned protoc (bionic's is 3.0; prost needs a current one).
  curl -fsSL "https://github.com/protocolbuffers/protobuf/releases/download/v$PROTOC_VERSION/protoc-$PROTOC_VERSION-linux-x86_64.zip" -o /tmp/protoc.zip
  echo "$PROTOC_SHA256  /tmp/protoc.zip" | sha256sum -c -
  unzip -q -o /tmp/protoc.zip -d /opt/protoc; export PROTOC=/opt/protoc/bin/protoc PROTOC_INCLUDE=/opt/protoc/include
  "$PROTOC" --version
  # g++ from the toolchain PPA (built for bionic's glibc 2.27): the prebuilt sherpa-onnx
  # lib was built with GCC 11 and needs GLIBCXX_3.4.29 symbols bionic's gcc-7 lacks.
  # It compiles every C/C++ dependency and drives the link, and its libstdc++.a is
  # linked statically: a -L dir that holds only the .a makes the linker's explicit
  # -lstdc++ pick it over the system .so.
  add-apt-repository -y ppa:ubuntu-toolchain-r/test >/dev/null
  apt-get update -qq
  GXX=; for v in 12 11; do if apt-get install -y -qq "g++-$v" >/dev/null 2>&1; then GXX=$v; break; fi; done
  [ -n "$GXX" ] || { echo "no g++-12 or g++-11 in ppa:ubuntu-toolchain-r/test for $(. /etc/os-release; echo "$VERSION_CODENAME")" >&2; exit 1; }
  echo "using gcc-$GXX: $(g++-$GXX --version | head -1)"
  export CC="gcc-$GXX" CXX="g++-$GXX" CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="gcc-$GXX"
  mkdir -p /opt/stdcxx && cp "$(g++-$GXX -print-file-name=libstdc++.a)" /opt/stdcxx/
  export RUSTFLAGS="${RUSTFLAGS:-} -L native=/opt/stdcxx"
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain "$RUST_VERSION" >/dev/null
  export PATH="$HOME/.cargo/bin:$PATH"
  if [ ! -f /opt/openssl/lib/libssl.a ]; then
    curl -fsSL "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz" -o /tmp/openssl.tgz
    echo "$OPENSSL_SHA256  /tmp/openssl.tgz" | sha256sum -c -
    mkdir /tmp/openssl-src && tar -xzf /tmp/openssl.tgz -C /tmp/openssl-src --strip-components=1
    (cd /tmp/openssl-src && CC="gcc-$GXX" ./Configure no-shared no-tests -fPIC --prefix=/opt/openssl --libdir=lib >/dev/null && make -j"$(nproc)" >/dev/null && make install_sw >/dev/null)
  fi
  export OPENSSL_DIR=/opt/openssl OPENSSL_STATIC=1
  cd /src
  cargo build --release --locked -p voice-service -p allternit-tts --target "$TARGET"
  install -m 0755 "$CARGO_TARGET_DIR/$TARGET/release/voice-service" /out/allternit-voice-service
  install -m 0755 "$CARGO_TARGET_DIR/$TARGET/release/allternit-tts" /out/allternit-tts
  ls -lh /out/allternit-voice-service /out/allternit-tts
  # The container runs as root; hand the outputs and target dir back to the caller
  # (the CI runner user), or the workflow's chmod and rust-cache cleanup hit EPERM.
  if [ -n "${HOST_UID:-}" ]; then
    chown "$HOST_UID:$HOST_GID" /out/allternit-voice-service /out/allternit-tts
    chown -R "$HOST_UID:$HOST_GID" "$CARGO_TARGET_DIR"
  fi
  exit 0
fi

ensure_zig() {
  if [ -n "${ZIG:-}" ] && [ "$("$ZIG" version 2>/dev/null)" = "$ZIG_VERSION" ]; then return; fi
  local dir="${ZIG_INSTALL_DIR:-${RUNNER_TEMP:-$HOME/.cache}/zig-$ZIG_VERSION}"
  if [ "$("$dir/zig" version 2>/dev/null || true)" != "$ZIG_VERSION" ]; then
    local tmp; tmp="$(mktemp -d)"
    curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/zig-x86_64-linux-$ZIG_VERSION.tar.xz" -o "$tmp/zig.tar.xz"
    echo "$ZIG_SHA256  $tmp/zig.tar.xz" | sha256sum -c -
    rm -rf "$dir"; mkdir -p "$dir"
    tar -xJf "$tmp/zig.tar.xz" -C "$dir" --strip-components=1
    rm -rf "$tmp"
  fi
  export ZIG="$dir/zig"
}

setup_zigbuild() {
  ensure_zig
  export PATH="$(dirname "$ZIG"):$PATH"
  echo "zig $("$ZIG" version)"
  if ! cargo install --list | grep -q "^cargo-zigbuild v$CARGO_ZIGBUILD_VERSION:"; then
    cargo install --locked cargo-zigbuild --version "$CARGO_ZIGBUILD_VERSION" --force
  fi
  echo "cargo-zigbuild $CARGO_ZIGBUILD_VERSION  target ${TARGET}.${ZIG_GLIBC}"
}

cd "$REPO"
ZB=(cargo zigbuild --release --target "${TARGET}.${ZIG_GLIBC}")
ZB_FLOOR=(cargo zigbuild --release --target "${TARGET}.${GLIBC_FLOOR}")
REL="${CARGO_TARGET_DIR:-$REPO/target}/$TARGET/release"
put() { install -m 0755 "$REL/$1" "$OUT/$2"; ls -lh "$OUT/$2"; }
ZB_READY=
zb() { [ -n "$ZB_READY" ] || { setup_zigbuild; ZB_READY=1; }; "${ZB[@]}" "$@"; }
zb_floor() { [ -n "$ZB_READY" ] || { setup_zigbuild; ZB_READY=1; }; "${ZB_FLOOR[@]}" "$@"; }

for c in "${COMPONENTS[@]}"; do
  case "$c" in
    api)         zb --manifest-path cmd/allternit-api/Cargo.toml; put allternit-api allternit-api ;;
    files-mount) zb -p allternit-files-mount; put allternit-files-mount allternit-files-mount ;;
    factory)     zb_floor -p allternit-factory; put allternit-factory allternit-factory ;;
    voice)
      TD="${CARGO_TARGET_DIR:-$REPO/target}/docker"; mkdir -p "$TD"
      docker run --rm -v "$REPO":/src -v "$OUT":/out -v "$TD":/target \
        -v "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")":/build-linux-sidecars.sh:ro \
        -e CARGO_TARGET_DIR=/target -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
        -w /src ubuntu:18.04 \
        bash /build-linux-sidecars.sh /out voice-docker ;;
    cua-driver)
      # Version + commit pinned once, in prepare-cua-driver.cjs (which still downloads
      # the macOS and Windows prebuilts).
      PREP="$REPO/surfaces/allternit-desktop/scripts/prepare-cua-driver.cjs"
      CUA_VERSION="$(sed -n "s/^const VERSION = '\(.*\)';/\1/p" "$PREP")"
      CUA_COMMIT="$(sed -n "s/^const LINUX_SOURCE_COMMIT = '\(.*\)';/\1/p" "$PREP")"
      [ -n "$CUA_VERSION" ] && [ -n "$CUA_COMMIT" ] || { echo "VERSION / LINUX_SOURCE_COMMIT not found in $PREP" >&2; exit 1; }
      TDC="${CARGO_TARGET_DIR:-$REPO/target}/cua"; mkdir -p "$TDC"
      CUA_SRC="${CUA_SRC_DIR:-$TDC/src-$CUA_VERSION}"
      [ -d "$CUA_SRC/.git" ] || git clone -q --depth 1 --branch "cua-driver-rs-v$CUA_VERSION" https://github.com/trycua/cua.git "$CUA_SRC"
      [ "$(git -C "$CUA_SRC" rev-parse HEAD)" = "$CUA_COMMIT" ] || { echo "trycua/cua tag cua-driver-rs-v$CUA_VERSION is not commit $CUA_COMMIT" >&2; exit 1; }
      # The workspace pins its own toolchain (rust-toolchain.toml); rustup installs it.
      [ -n "$ZB_READY" ] || { setup_zigbuild; ZB_READY=1; }
      (cd "$CUA_SRC/libs/cua-driver/rust" && CARGO_TARGET_DIR="$TDC/target" "${ZB_FLOOR[@]}" --locked -p cua-driver)
      install -m 0755 "$TDC/target/$TARGET/release/cua-driver" "$OUT/cua-driver"; ls -lh "$OUT/cua-driver" ;;
    *) echo "unknown component: $c" >&2; exit 2 ;;
  esac
done
