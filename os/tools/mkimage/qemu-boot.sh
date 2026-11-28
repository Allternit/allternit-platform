#!/bin/sh
# Boot the image built by mkimage.sh in QEMU and judge the serial log (gate G2a).
#   qemu-boot.sh [--out DIR]   exit 0 only if the log has both strings and no panic.
# KVM when /dev/kvm is usable (timeout 120 s), else TCG (timeout 600 s).
set -u
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT="$SCRIPT_DIR/../../target/image"
while [ $# -gt 0 ]; do
  case "$1" in --out) OUT=$2; shift 2 ;; *) echo "unknown flag $1" >&2; exit 2 ;; esac
done
OUT=$(cd "$OUT" && pwd)
LOG="$OUT/serial.log"
# shellcheck disable=SC1091
. "$OUT/boot.env"

if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  ACCEL="-accel kvm -cpu host"; LIMIT=120; echo "boot: KVM, ${LIMIT}s limit"
else
  ACCEL="-accel tcg -cpu max"; LIMIT=600; echo "boot: TCG (no usable /dev/kvm), ${LIMIT}s limit"
fi

if [ "$MODE" = uki ]; then
  CODE=""
  for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd /usr/share/ovmf/OVMF.fd; do
    [ -f "$f" ] && { CODE=$f; break; }
  done
  [ -n "$CODE" ] || { echo "no OVMF firmware found" >&2; exit 2; }
  VARS_SRC=""
  for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do
    [ -f "$f" ] && { VARS_SRC=$f; break; }
  done
  if [ -n "$VARS_SRC" ]; then
    cp "$VARS_SRC" "$OUT/OVMF_VARS.fd"
    FW="-drive if=pflash,format=raw,readonly=on,file=$CODE -drive if=pflash,format=raw,file=$OUT/OVMF_VARS.fd"
  else
    FW="-bios $CODE"
  fi
  # shellcheck disable=SC2086
  set -- qemu-system-x86_64 -machine q35 $ACCEL -m 1024 -smp 2 -nographic -serial mon:stdio -no-reboot \
    $FW -drive if=virtio,format=raw,file="$DISK" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
else
  # shellcheck disable=SC2086
  set -- qemu-system-x86_64 -machine q35 $ACCEL -m 1024 -smp 2 -nographic -serial mon:stdio -no-reboot \
    -kernel "$KERNEL" -append "$CMDLINE" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
fi

: > "$LOG"
"$@" < /dev/null > "$LOG" 2>&1 &
QPID=$!
T=0; RESULT=timeout
while [ "$T" -lt "$LIMIT" ]; do
  if grep -q 'Kernel panic' "$LOG"; then RESULT=panic; break; fi
  if grep -q 'kernel-check ok' "$LOG" && grep -q 'ALLTERNIT-OS READY kernel-interface=1' "$LOG"; then RESULT=ok; break; fi
  kill -0 "$QPID" 2>/dev/null || { RESULT=exited; break; }
  sleep 1; T=$((T + 1))
done
kill "$QPID" 2>/dev/null; sleep 1; kill -9 "$QPID" 2>/dev/null; wait "$QPID" 2>/dev/null
cat "$LOG"
echo "boot: result=$RESULT after ${T}s"
# a final recheck: the process may have exited right after printing
if [ "$RESULT" = exited ] && grep -q 'kernel-check ok' "$LOG" && grep -q 'ALLTERNIT-OS READY kernel-interface=1' "$LOG"; then RESULT=ok; fi
grep -q 'Kernel panic' "$LOG" && RESULT=panic
[ "$RESULT" = ok ]
