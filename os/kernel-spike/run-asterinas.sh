#!/bin/sh
# AllternitOS Stage 3 kernel spike (plan step 34): boot the Stage 2 userland on Asterinas.
#
#   run-asterinas.sh initramfs [--bin DIR] [--llama F] [--model F] [--out DIR]
#       Build OUT/initramfs.cpio.gz (newc): init, authority stub, shell, llama-server,
#       init.toml and the GGUF at /model/model.gguf. Host side; run under `fakeroot` so the
#       /dev nodes can be made (CI does). Needs cpio and gzip.
#   run-asterinas.sh boot [--asterinas DIR] [--out DIR]
#       `cargo osdk run` the Asterinas kernel with that initramfs, serial to OUT/serial.log.
#       Must run where OSDK works: Ubuntu x86_64, i.e. inside the Asterinas Docker dev image.
#   run-asterinas.sh judge [--out DIR]
#       Print the gate G3 lines found in OUT/serial.log. Exit 0 only for READY + kernel-check.
#
# Defaults: --bin os/target/x86_64-unknown-linux-musl/release, --llama os/target/llama/llama-server,
# --model os/target/model/model.gguf, --out os/target/kernel-spike, --asterinas os/target/asterinas.
# Env: BOOT_LIMIT (seconds, default 1500), KCMD_ARGS (replaces the default kernel args).
set -u
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OS_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
TARGET=x86_64-unknown-linux-musl
# The repo's .cargo/config.toml sets target-dir, so ask cargo where the binaries go
# (same fix as mkimage.sh, f3851a8e6).
TARGET_DIR=$(cargo metadata --manifest-path "$OS_DIR/Cargo.toml" --no-deps --format-version 1 2>/dev/null \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[ -n "$TARGET_DIR" ] || TARGET_DIR="$OS_DIR/target"
BIN="$TARGET_DIR/$TARGET/release"
LLAMA="$OS_DIR/target/llama/llama-server"
MODEL="$OS_DIR/target/model/model.gguf"
OUT="$OS_DIR/target/kernel-spike"
AST="$OS_DIR/target/asterinas"
die() { echo "run-asterinas: $*" >&2; exit 1; }
say() { echo "run-asterinas: $*"; }

[ $# -ge 1 ] || { sed -n '2,17p' "$0"; exit 2; }
CMD=$1; shift
while [ $# -gt 0 ]; do
  [ $# -ge 2 ] || die "$1 needs a value"
  case "$1" in
    --bin) BIN=$2 ;; --llama) LLAMA=$2 ;; --model) MODEL=$2 ;;
    --out) OUT=$2 ;; --asterinas) AST=$2 ;;
    *) die "unknown flag: $1" ;;
  esac
  shift 2
done
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)

do_initramfs() {
  for f in "$BIN/init" "$BIN/authority" "$BIN/shell" "$LLAMA" "$MODEL"; do
    [ -f "$f" ] || die "missing $f"
  done
  # shellcheck disable=SC1091
  . "$OS_DIR/model/MODEL"
  if command -v sha256sum >/dev/null 2>&1; then MSUM=$(sha256sum "$MODEL" | cut -d' ' -f1)
  else MSUM=$(shasum -a 256 "$MODEL" | cut -d' ' -f1); fi
  [ "$MSUM" = "$MODEL_SHA256" ] || die "model sha256 mismatch: got $MSUM want $MODEL_SHA256"

  R="$OUT/initramfs-root"
  rm -rf "$R"
  mkdir -p "$R/dev" "$R/proc" "$R/sys" "$R/run" "$R/tmp" "$R/sbin" "$R/usr/bin" \
           "$R/usr/lib/allternit" "$R/etc/allternit" "$R/model"
  chmod 1777 "$R/tmp"
  cp "$BIN/init" "$R/sbin/init"
  cp "$BIN/authority" "$R/usr/lib/allternit/authority"
  cp "$BIN/shell" "$R/usr/lib/allternit/shell"
  cp "$LLAMA" "$R/usr/lib/allternit/llama-server"
  cp "$MODEL" "$R/model/model.gguf"
  chmod 755 "$R/sbin/init" "$R/usr/lib/allternit/"* "$R/model"
  chmod 644 "$R/model/model.gguf"

  # init.toml: the compiled-in default (same derivation as mkimage.sh) with no state
  # partition, the model on the initramfs and the shell home on tmpfs. The cmdline switches
  # allternit.state=none / allternit.model= say the same thing when /proc/cmdline exists.
  awk '/DEFAULT_CONFIG: &str = r#"/{f=1;next} f&&/^"#;/{exit} f' "$OS_DIR/init/src/config.rs" \
    | sed -e 's#/usr/bin/authority#/usr/lib/allternit/authority#' \
          -e '/^state_device = /d' \
          -e 's#/state/model/model.gguf#/model/model.gguf#' \
          -e 's#HOME=/state/shell#HOME=/tmp/shell#' > "$R/etc/allternit/init.toml"
  grep -q '/usr/lib/allternit/authority' "$R/etc/allternit/init.toml" || die "could not derive init.toml"
  grep -q '/model/model.gguf' "$R/etc/allternit/init.toml" || die "init.toml model path not rewritten"

  mknod -m 600 "$R/dev/console" c 5 1 2>/dev/null || say "warning: no /dev/console node (run under fakeroot)"
  mknod -m 666 "$R/dev/null" c 1 3 2>/dev/null || say "warning: no /dev/null node (run under fakeroot)"
  ( cd "$R" && find . | cpio -o -H newc --quiet ) | gzip -1 > "$OUT/initramfs.cpio.gz"
  say "wrote $OUT/initramfs.cpio.gz ($(wc -c < "$OUT/initramfs.cpio.gz") bytes)"
}

do_boot() {
  [ -f "$OUT/initramfs.cpio.gz" ] || die "no $OUT/initramfs.cpio.gz (run: initramfs)"
  [ -d "$AST/kernel" ] || die "no Asterinas checkout at $AST (git clone --branch v0.18.0)"
  command -v cargo-osdk >/dev/null 2>&1 || cargo osdk --help >/dev/null 2>&1 \
    || die "cargo-osdk not found: run inside the Asterinas Docker dev image"
  LIMIT=${BOOT_LIMIT:-1500}
  ARGS=${KCMD_ARGS:-"console=ttyS0 rdinit=/sbin/init init=/sbin/init allternit.state=none allternit.model=/model/model.gguf"}
  LOG="$OUT/serial.log"
  : > "$LOG"
  say "cargo osdk run, initramfs $OUT/initramfs.cpio.gz, limit ${LIMIT}s"
  ( cd "$AST/kernel" && exec cargo osdk run --target-arch x86_64 --boot-method qemu-direct \
      --initramfs "$OUT/initramfs.cpio.gz" --kcmd-args "$ARGS" ) < /dev/null > "$LOG" 2>&1 &
  QPID=$!
  T=0; RESULT=timeout
  while [ "$T" -lt "$LIMIT" ]; do
    if grep -q 'Kernel panic\|KERNEL PANIC\|panicked at' "$LOG"; then RESULT=panic; break; fi
    if grep -q 'shell-tool-call ok' "$LOG"; then RESULT=model-ok; break; fi
    kill -0 "$QPID" 2>/dev/null || { RESULT=exited; break; }
    sleep 1; T=$((T + 1))
  done
  # qemu is a grandchild: stop it by name as well as the cargo process.
  kill "$QPID" 2>/dev/null; pkill -f qemu-system-x86_64 2>/dev/null; sleep 1
  kill -9 "$QPID" 2>/dev/null; pkill -9 -f qemu-system-x86_64 2>/dev/null; wait "$QPID" 2>/dev/null
  say "boot result=$RESULT after ${T}s"
}

do_judge() {
  LOG="$OUT/serial.log"
  [ -f "$LOG" ] || die "no $LOG"
  grep -E 'kernel-check|ALLTERNIT-OS READY|shell-tool-call|model tok/s=' "$LOG" || say "none of the G3 lines are in the log"
  grep -q 'kernel-check ok' "$LOG" && grep -q 'ALLTERNIT-OS READY kernel-interface=1' "$LOG"
}

case "$CMD" in
  initramfs) do_initramfs ;;
  boot) do_boot ;;
  judge) do_judge ;;
  *) die "unknown command: $CMD" ;;
esac
