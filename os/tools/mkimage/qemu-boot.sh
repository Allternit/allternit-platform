#!/bin/sh
# Boot the image built by mkimage.sh in QEMU and judge the serial log (gate G2a).
#   qemu-boot.sh [--out DIR]                    exit 0 only if the log has every required line
#   qemu-boot.sh [--out DIR] --corrupt-slot a   negative case (step 30a), see below
# Required lines: "kernel-check ok", "ALLTERNIT-OS READY kernel-interface=1", no panic.
# Step 29 (gate G2c) adds "shell-tool-call ok" (the shell got one tool call from the model
# and the authority answered); tok/s is printed as "model tok/s=".
# Step 30a adds, on the UKI disk: "verity: ok slot=a" (PID 1 opened system-a through
# dm-verity before it switched root).
# --corrupt-slot X boots a copy of the disk with one byte flipped inside slot X's data and
# passes only if the log shows "verity: FAIL slot=X" and never READY (the slot did not boot).
# KVM when /dev/kvm is usable (timeout 300 s), else TCG (timeout 1500 s).
set -u
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT="$SCRIPT_DIR/../../target/image"
CORRUPT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    --corrupt-slot) CORRUPT=$2; shift 2 ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done
OUT=$(cd "$OUT" && pwd)
LOG="$OUT/serial.log"
# shellcheck disable=SC1091
. "$OUT/boot.env"

if [ -n "$CORRUPT" ]; then
  [ "$MODE" = uki ] || { echo "--corrupt-slot needs the UKI disk (MODE=uki)" >&2; exit 2; }
  case "$CORRUPT" in
    a) OFF_MB=$SYS_A_OFF_MB ;;
    b) OFF_MB=$SYS_B_OFF_MB ;;
    *) echo "--corrupt-slot takes a or b" >&2; exit 2 ;;
  esac
  BAD="$OUT/allternit-os-bad-$CORRUPT.img"
  cp --sparse=always "$DISK" "$BAD" 2>/dev/null || cp "$DISK" "$BAD"
  # 1 MiB into the slot: inside the ext4 data area (the slot data is at least 8 MiB).
  OFF=$(( (OFF_MB + 1) * 1048576 ))
  B=$(od -An -tu1 -j "$OFF" -N1 "$BAD" | tr -d ' ')
  NB=$(( B ^ 255 ))
  # shellcheck disable=SC2059
  printf "$(printf '\\%03o' "$NB")" | dd of="$BAD" bs=1 seek="$OFF" conv=notrunc 2>/dev/null
  echo "boot: flipped byte at $OFF (slot $CORRUPT) $B -> $NB in $BAD"
  DISK=$BAD
  LOG="$OUT/serial-bad-$CORRUPT.log"
fi

if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  ACCEL="-accel kvm -cpu host"; LIMIT=300; echo "boot: KVM, ${LIMIT}s limit"
else
  ACCEL="-accel tcg -cpu max"; LIMIT=1500; echo "boot: TCG (no usable /dev/kvm), ${LIMIT}s limit"
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
  set -- qemu-system-x86_64 -machine q35 $ACCEL -m 2G -smp 2 -nographic -serial mon:stdio -no-reboot \
    $FW -drive if=virtio,format=raw,file="$DISK" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
else
  # shellcheck disable=SC2086
  set -- qemu-system-x86_64 -machine q35 $ACCEL -m 2G -smp 2 -nographic -serial mon:stdio -no-reboot \
    -kernel "$KERNEL" -append "$CMDLINE" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
fi

# Lines a good boot must show. The verity line exists only on the UKI disk (direct boot has
# no system slot).
set_required() {
  REQUIRED="kernel-check ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok"
  [ "$MODE" = uki ] && REQUIRED="verity: ok slot=a|$REQUIRED"
}
set_required

# 0 when the log holds the verdict the mode wants.
judged_ok() {
  if [ -n "$CORRUPT" ]; then
    grep -q "verity: FAIL slot=$CORRUPT" "$LOG"
    return
  fi
  OLDIFS=$IFS; IFS='|'
  for line in $REQUIRED; do
    grep -qF "$line" "$LOG" || { IFS=$OLDIFS; return 1; }
  done
  IFS=$OLDIFS
}

: > "$LOG"
"$@" < /dev/null > "$LOG" 2>&1 &
QPID=$!
T=0; RESULT=timeout
while [ "$T" -lt "$LIMIT" ]; do
  if grep -q 'Kernel panic' "$LOG"; then RESULT=panic; break; fi
  if judged_ok; then RESULT=ok; break; fi
  kill -0 "$QPID" 2>/dev/null || { RESULT=exited; break; }
  sleep 1; T=$((T + 1))
done
# Negative case: give a wrongly booting slot a moment to print READY before judging.
[ -n "$CORRUPT" ] && [ "$RESULT" = ok ] && sleep 5
kill "$QPID" 2>/dev/null; sleep 1; kill -9 "$QPID" 2>/dev/null; wait "$QPID" 2>/dev/null
cat "$LOG"
echo "boot: result=$RESULT after ${T}s"
# a final recheck: the process may have exited (powered off) right after printing
if [ "$RESULT" = exited ] && judged_ok; then RESULT=ok; fi
grep -q 'Kernel panic' "$LOG" && RESULT=panic
if [ -n "$CORRUPT" ]; then
  if grep -q 'verity: ok' "$LOG" || grep -q 'ALLTERNIT-OS READY' "$LOG"; then RESULT=booted-corrupt-slot; fi
  rm -f "$DISK"
  echo "boot: corrupt slot $CORRUPT verdict=$RESULT"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
    echo "### os-image corrupt slot $CORRUPT (step 30a)"; echo; echo "- result: $RESULT after ${T}s";
    grep 'verity:' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
  [ "$RESULT" = ok ]
  exit
fi
TPS=$(grep -o 'model tok/s=[0-9.]*' "$LOG" | tail -1)
echo "boot: ${TPS:-model tok/s=unknown}"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
  echo "### os-image boot (steps 29, 30a)"; echo; echo "- result: $RESULT after ${T}s"; echo "- ${TPS:-model tok/s=unknown}";
  grep -E 'verity:|shell-tool-call' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
[ "$RESULT" = ok ]
