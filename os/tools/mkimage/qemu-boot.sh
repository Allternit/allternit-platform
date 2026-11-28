#!/bin/sh
# Boot the image built by mkimage.sh in QEMU and judge the serial log (gate G2a).
#   qemu-boot.sh [--out DIR]                    good boot: exit 0 only if the log has every required line
#   qemu-boot.sh [--out DIR] --corrupt-slot a   dm-verity negative case (steps 30a/30b)
#   qemu-boot.sh [--out DIR] --bad-update       bad update falls back to the old slot (step 30b)
#
# Good boot, required lines in this order (the UKI disk; direct boot needs only the three
# marked *):
#   slot: booting slot=a          PID 1 read the ESP boot state (fresh image: a, unconfirmed)
#   verity: ok slot=a             system-a opened through dm-verity (step 30a)
#   state: luks open ok           LUKS2 state opened with dm-crypt (step 30b, TEST ONLY key)
#   kernel-check ok *             the authority answered one allow + one deny (step 27)
#   ALLTERNIT-OS READY kernel-interface=1 *
#   shell-tool-call ok *          the local model made one shell tool call (step 29, gate G2c)
#   slot: confirmed slot=a        the health check confirmed the slot in the boot state (30b)
# tok/s is printed as "model tok/s=" and goes to the job summary.
#
# --corrupt-slot X boots a copy of the disk with one byte flipped inside slot X's data and
# the boot state set to "X active, confirmed". Pass: "verity: FAIL slot=X", then
# "slot: fallback from=X to=Y reason=verity", "verity: ok slot=Y", "state: luks open ok" and
# READY on Y; never "verity: ok slot=X" (the corrupted slot never boots).
#
# --bad-update copies the disk, sets the boot state to "a active, confirmed", takes slot B's
# image out of the disk, flips one byte in it, and installs it with the real updater
# (allternit-update --image ... --tries 3: B active, unconfirmed). Pass: "slot: booting
# slot=b", "verity: FAIL slot=b", "slot: fallback from=b to=a reason=verity", "verity: ok
# slot=a", "state: luks open ok", READY, "shell-tool-call ok", "slot: confirmed slot=a", in
# that order, never "verity: ok slot=b", and the boot state left on the disk says a, confirmed.
#
# KVM when /dev/kvm is usable (timeout 300 s), else TCG (timeout 1500 s).
set -u
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT="$SCRIPT_DIR/../../target/image"
CORRUPT=""
BAD_UPDATE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    --corrupt-slot) CORRUPT=$2; shift 2 ;;
    --bad-update) BAD_UPDATE=1; shift ;;
    *) echo "unknown flag $1" >&2; exit 2 ;;
  esac
done
OUT=$(cd "$OUT" && pwd)
LOG="$OUT/serial.log"
# shellcheck disable=SC1091
. "$OUT/boot.env"
export MTOOLS_SKIP_CHECK=1
MIB=1048576

# The ESP inside a disk image, for mtools.
esp() { echo "$1@@$((ESP_OFF_MB * MIB))"; }

# set_state DISK SLOT TRIES CONFIRMED: write the A/B boot state on the disk's ESP.
set_state() {
  printf '# set by qemu-boot.sh\nactive=%s\ntries=%s\nconfirmed=%s\n' "$2" "$3" "$4" > "$OUT/bootstate.tmp"
  mcopy -o -i "$(esp "$1")" "$OUT/bootstate.tmp" ::/allternit/bootstate || { echo "boot: could not write the boot state" >&2; exit 2; }
  rm -f "$OUT/bootstate.tmp"
}

# get_state DISK: print the boot state on the disk's ESP.
get_state() {
  rm -f "$OUT/bootstate.tmp"
  mcopy -i "$(esp "$1")" ::/allternit/bootstate "$OUT/bootstate.tmp" 2>/dev/null && cat "$OUT/bootstate.tmp"
  rm -f "$OUT/bootstate.tmp"
}

# flip_byte FILE OFFSET: invert one byte in place.
flip_byte() {
  B=$(od -An -tu1 -j "$2" -N1 "$1" | tr -d ' ')
  NB=$(( B ^ 255 ))
  # shellcheck disable=SC2059
  printf "$(printf '\\%03o' "$NB")" | dd of="$1" bs=1 seek="$2" conv=notrunc 2>/dev/null
  echo "boot: flipped byte at $2 in $1: $B -> $NB"
}

copy_disk() {
  cp --sparse=always "$DISK" "$1" 2>/dev/null || cp "$DISK" "$1"
}

FORBID=""
if [ -n "$CORRUPT" ]; then
  [ "$MODE" = uki ] || { echo "--corrupt-slot needs the UKI disk (MODE=uki)" >&2; exit 2; }
  case "$CORRUPT" in
    a) OFF_MB=$SYS_A_OFF_MB; OTHER=b ;;
    b) OFF_MB=$SYS_B_OFF_MB; OTHER=a ;;
    *) echo "--corrupt-slot takes a or b" >&2; exit 2 ;;
  esac
  BAD="$OUT/allternit-os-bad-$CORRUPT.img"
  copy_disk "$BAD"
  # 1 MiB into the slot: inside the ext4 data area (the slot data is at least 8 MiB).
  flip_byte "$BAD" $(( (OFF_MB + 1) * MIB ))
  set_state "$BAD" "$CORRUPT" 0 1
  DISK=$BAD
  LOG="$OUT/serial-bad-$CORRUPT.log"
  REQUIRED="verity: FAIL slot=$CORRUPT|slot: fallback from=$CORRUPT to=$OTHER reason=verity|verity: ok slot=$OTHER|state: luks open ok|ALLTERNIT-OS READY kernel-interface=1"
  FORBID="verity: ok slot=$CORRUPT"
elif [ "$BAD_UPDATE" -eq 1 ]; then
  [ "$MODE" = uki ] || { echo "--bad-update needs the UKI disk (MODE=uki)" >&2; exit 2; }
  [ -x "${UPDATE_BIN:-}" ] || { echo "no allternit-update at ${UPDATE_BIN:-<unset>} (boot.env UPDATE_BIN)" >&2; exit 2; }
  BAD="$OUT/allternit-os-bad-update.img"
  copy_disk "$BAD"
  set_state "$BAD" a 0 1
  # The "new" system image: slot B as built, with one byte flipped, so it no longer matches
  # B's root hash on the UKI command line.
  NEW_B="$OUT/system-b-bad.img"
  dd if="$DISK" of="$NEW_B" bs=1M skip="$SYS_B_OFF_MB" count="$SLOT_MB" 2>/dev/null
  flip_byte "$NEW_B" "$MIB"
  get_state "$BAD" > "$OUT/bootstate.update"
  "$UPDATE_BIN" --image "$NEW_B" --tries 3 --state "$OUT/bootstate.update" \
    --device "$BAD" --offset $((SYS_B_OFF_MB * MIB)) --slot-bytes $((SLOT_MB * MIB)) \
    || { echo "boot: allternit-update failed" >&2; exit 1; }
  grep -q '^active=b' "$OUT/bootstate.update" && grep -q '^confirmed=0' "$OUT/bootstate.update" \
    || { echo "boot: updater did not set b active + unconfirmed" >&2; cat "$OUT/bootstate.update"; exit 1; }
  mcopy -o -i "$(esp "$BAD")" "$OUT/bootstate.update" ::/allternit/bootstate || exit 2
  rm -f "$NEW_B" "$OUT/bootstate.update"
  DISK=$BAD
  LOG="$OUT/serial-bad-update.log"
  REQUIRED="slot: booting slot=b|verity: FAIL slot=b|slot: fallback from=b to=a reason=verity|verity: ok slot=a|state: luks open ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok|slot: confirmed slot=a"
  FORBID="verity: ok slot=b"
elif [ "$MODE" = uki ]; then
  REQUIRED="slot: booting slot=a|verity: ok slot=a|state: luks open ok|kernel-check ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok|slot: confirmed slot=a"
else
  # Direct boot has no disk: no slots, no boot state, no state partition.
  REQUIRED="kernel-check ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok"
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

# 0 when every required line is in the log, in order; 1 otherwise.
judged_ok() {
  last=0
  OLDIFS=$IFS; IFS='|'
  for line in $REQUIRED; do
    n=$(grep -nF "$line" "$LOG" | head -1 | cut -d: -f1)
    if [ -z "$n" ] || [ "$n" -lt "$last" ]; then IFS=$OLDIFS; return 1; fi
    last=$n
  done
  IFS=$OLDIFS
}

# The first required line that is missing or out of order (for the verdict).
first_missing() {
  last=0
  OLDIFS=$IFS; IFS='|'
  for line in $REQUIRED; do
    n=$(grep -nF "$line" "$LOG" | head -1 | cut -d: -f1)
    if [ -z "$n" ]; then echo "missing: $line"; break; fi
    if [ "$n" -lt "$last" ]; then echo "out of order: $line"; break; fi
    last=$n
  done
  IFS=$OLDIFS
}

forbidden() { [ -n "$FORBID" ] && grep -qF "$FORBID" "$LOG"; }

: > "$LOG"
"$@" < /dev/null > "$LOG" 2>&1 &
QPID=$!
T=0; RESULT=timeout
while [ "$T" -lt "$LIMIT" ]; do
  if grep -q 'Kernel panic' "$LOG"; then RESULT=panic; break; fi
  if forbidden; then RESULT=forbidden; break; fi
  if judged_ok; then RESULT=ok; break; fi
  kill -0 "$QPID" 2>/dev/null || { RESULT=exited; break; }
  sleep 1; T=$((T + 1))
done
kill "$QPID" 2>/dev/null; sleep 1; kill -9 "$QPID" 2>/dev/null; wait "$QPID" 2>/dev/null
cat "$LOG"
echo "boot: result=$RESULT after ${T}s"
# a final recheck: the process may have exited (powered off) right after printing
if [ "$RESULT" = exited ] && judged_ok; then RESULT=ok; fi
grep -q 'Kernel panic' "$LOG" && RESULT=panic
forbidden && RESULT="forbidden ($FORBID)"
[ "$RESULT" = ok ] || echo "boot: $(first_missing)"

if [ -n "$CORRUPT" ] || [ "$BAD_UPDATE" -eq 1 ]; then
  if [ "$BAD_UPDATE" -eq 1 ] && [ "$RESULT" = ok ]; then
    AFTER=$(get_state "$DISK")
    echo "boot: boot state after the run:"; echo "$AFTER"
    echo "$AFTER" | grep -q '^active=a' && echo "$AFTER" | grep -q '^confirmed=1' \
      || RESULT="boot state not back on a + confirmed"
  fi
  NAME=${CORRUPT:+corrupt slot $CORRUPT}; NAME=${NAME:-bad update}
  rm -f "$DISK"
  echo "boot: $NAME verdict=$RESULT"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
    echo "### os-image $NAME (step 30b)"; echo; echo "- result: $RESULT after ${T}s";
    grep -E 'slot:|verity:|state:' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
  [ "$RESULT" = ok ]
  exit
fi
TPS=$(grep -o 'model tok/s=[0-9.]*' "$LOG" | tail -1)
echo "boot: ${TPS:-model tok/s=unknown}"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
  echo "### os-image boot (steps 29, 30a, 30b)"; echo; echo "- result: $RESULT after ${T}s"; echo "- ${TPS:-model tok/s=unknown}";
  grep -E 'slot:|verity:|state:|shell-tool-call' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
[ "$RESULT" = ok ]
