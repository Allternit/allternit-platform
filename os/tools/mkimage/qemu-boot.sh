#!/bin/sh
# Boot the image built by mkimage.sh in QEMU and judge the serial log (gate G2a).
#   qemu-boot.sh [--out DIR]                    good boot: exit 0 only if the log has every required line
#   qemu-boot.sh [--out DIR] --corrupt-slot a   dm-verity negative case (steps 30a/30b)
#   qemu-boot.sh [--out DIR] --bad-update       bad update falls back to the old slot (step 30b)
#   qemu-boot.sh [--out DIR] --refused-update   allternit-update refuses unsigned/mismatched updates (step 31)
#   qemu-boot.sh [--out DIR] --unsigned-uki     enforcing Secure Boot refuses an unsigned UKI (step 31)
#   qemu-boot.sh [--out DIR] --disk-bus ahci|nvme|usb|virtio   PC image only (boot.env TARGET=pc, step 32)
#
# PC image (mkimage --target pc): the good boot runs on emulated PC hardware instead of virtio:
# q35, the disk on AHCI (default) or NVMe or a USB stick, an e1000e NIC, qemu-xhci + usb-kbd +
# usb-mouse, std VGA, 4 GB, OVMF with Secure Boot enforcing like the vm image. PID 1 finds the
# GPT partitions by name and prints "init: partition state: /dev/vda4 -> /dev/<real device>"
# (required, right after "secureboot: enabled"); then the same lines as below. The disk is
# booted from a copy, so one image can be run once per bus. The negative cases stay on the vm
# image (the PC image changes the drivers, not the slot, update or Secure Boot code).
#
# Good boot, required lines in this order (the UKI disk; direct boot needs only the three
# marked *):
#   secureboot: enabled           OVMF ran with Secure Boot enforcing, the test keys enrolled (step 31)
#   slot: booting slot=a          PID 1 read the ESP boot state (fresh image: a, unconfirmed)
#   manifest: ok slot=a version=  the slot's signed manifest verified (step 31)
#   verity: ok slot=a             system-a opened through dm-verity (step 30a)
#   state: luks digest ok         the key matched the LUKS2 digest (step 31)
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
# --refused-update needs no VM: it runs the real updater three times (image that does not match
# its manifest, a manifest changed after signing, a manifest signed by another key) and passes
# only if each is refused with a reason and the boot state is untouched.
#
# --unsigned-uki replaces the UKI on a copy of the disk with the unsigned build and boots it
# under enforcing OVMF. Pass: firmware prints "Access Denied", PID 1 never starts ("slot:
# booting" never appears).
#
# KVM when /dev/kvm is usable (timeout 300 s), else TCG (timeout 1500 s).
set -u
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OUT="$SCRIPT_DIR/../../target/image"
CORRUPT=""
BAD_UPDATE=0
REFUSED=0
UNSIGNED=0
BUS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$2; shift 2 ;;
    --corrupt-slot) CORRUPT=$2; shift 2 ;;
    --bad-update) BAD_UPDATE=1; shift ;;
    --refused-update) REFUSED=1; shift ;;
    --unsigned-uki) UNSIGNED=1; shift ;;
    --disk-bus) BUS=$2; shift 2 ;;
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
IMG_TARGET=${TARGET:-vm}
PCLINE=""
if [ "$IMG_TARGET" = pc ]; then
  [ "$MODE" = uki ] || { echo "the PC image needs the UKI disk (MODE=uki)" >&2; exit 2; }
  [ -n "$BUS" ] || BUS=ahci
  PCLINE="init: partition state: /dev/vda4 -> /dev/|"
elif [ -n "$BUS" ]; then
  echo "--disk-bus is for the PC image (mkimage --target pc)" >&2; exit 2
fi
# Step 31: on the UKI disk OVMF runs with Secure Boot enforcing, and PID 1 says so first.
SB=""
[ "${SECUREBOOT:-0}" = 1 ] && [ "$MODE" = uki ] && SB="secureboot: enabled|"
KEYS_PUB="${KEYS_DIR:-$OUT/keys}/public"

# --refused-update: three updates the updater must refuse. No VM.
if [ "$REFUSED" -eq 1 ]; then
  [ "$MODE" = uki ] || { echo "--refused-update needs the UKI disk" >&2; exit 2; }
  WORK="$OUT/refuse-work"; rm -rf "$WORK"; mkdir -p "$WORK"
  copy_disk "$WORK/disk.img"
  get_state "$WORK/disk.img" > "$WORK/bootstate"
  cp "$WORK/bootstate" "$WORK/bootstate.orig"
  # An "update" image for slot B that no longer matches the manifest of slot B.
  dd if="$DISK" of="$WORK/new-b.img" bs=1M skip="$SYS_B_OFF_MB" count="$SLOT_MB" 2>/dev/null
  flip_byte "$WORK/new-b.img" "$MIB"
  openssl genpkey -algorithm ED25519 -out "$WORK/other.pem" 2>/dev/null
  cp "$PROV_DIR/manifest-b.json" "$WORK/changed.json"; printf ' ' >> "$WORK/changed.json"
  openssl pkeyutl -sign -inkey "$WORK/other.pem" -rawin -in "$PROV_DIR/manifest-b.json" -out "$WORK/other.sig"
  FAILED=0
  try() { # NAME MANIFEST SIG EXPECT
    "$UPDATE_BIN" --image "$WORK/new-b.img" --manifest "$2" --sig "$3" --pubkey "$KEYS_PUB/update-pub.key" \
      --tries 3 --state "$WORK/bootstate" --device "$WORK/disk.img" \
      --offset $((SYS_B_OFF_MB * MIB)) --slot-bytes $((SLOT_MB * MIB)) > "$WORK/out.txt" 2>&1
    rc=$?
    cat "$WORK/out.txt"
    if [ "$rc" -ne 0 ] && grep -qF "$4" "$WORK/out.txt" && cmp -s "$WORK/bootstate" "$WORK/bootstate.orig"; then
      echo "refused: $1 ($4)"
    else
      echo "NOT refused as expected: $1 (rc=$rc, wanted \"$4\")"; FAILED=1
    fi
  }
  try "image does not match the signed manifest" "$PROV_DIR/manifest-b.json" "$PROV_DIR/manifest-b.sig" "does not match the manifest"
  try "manifest changed after signing" "$WORK/changed.json" "$PROV_DIR/manifest-b.sig" "signature does not match"
  try "manifest signed by another key" "$PROV_DIR/manifest-b.json" "$WORK/other.sig" "signature does not match"
  # No signature at all: the updater refuses before reading anything.
  if "$UPDATE_BIN" --image "$WORK/new-b.img" --state "$WORK/bootstate" > "$WORK/out.txt" 2>&1; then
    echo "NOT refused as expected: no manifest at all"; FAILED=1
  else
    cat "$WORK/out.txt"; grep -qF "refusing an unsigned update" "$WORK/out.txt" && echo "refused: no manifest at all" \
      || { echo "wrong refusal message"; FAILED=1; }
  fi
  rm -rf "$WORK"
  echo "boot: refused-update verdict=$([ "$FAILED" -eq 0 ] && echo ok || echo FAIL)"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "### os-image refused update (step 31): $([ "$FAILED" -eq 0 ] && echo ok || echo FAIL)" >> "$GITHUB_STEP_SUMMARY"
  exit "$FAILED"
fi

if [ "$UNSIGNED" -eq 1 ]; then
  [ "$MODE" = uki ] && [ "${SECUREBOOT:-0}" = 1 ] || { echo "--unsigned-uki needs the UKI disk with Secure Boot (boot.env)" >&2; exit 2; }
  BAD="$OUT/allternit-os-unsigned-uki.img"
  copy_disk "$BAD"
  mcopy -o -i "$(esp "$BAD")" "$UKI_UNSIGNED" ::/EFI/BOOT/BOOTX64.EFI || { echo "boot: could not put the unsigned UKI on the ESP" >&2; exit 2; }
  DISK=$BAD
  LOG="$OUT/serial-unsigned-uki.log"
  REQUIRED="Access Denied"
  FORBID="slot: booting"
  SB=""
elif [ -n "$CORRUPT" ]; then
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
  REQUIRED="${SB}verity: FAIL slot=$CORRUPT|slot: fallback from=$CORRUPT to=$OTHER reason=verity|verity: ok slot=$OTHER|state: luks open ok|ALLTERNIT-OS READY kernel-interface=1"
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
  # The update is VALIDLY SIGNED (a manifest for these exact bytes, re-signed with the test
  # key) but its dm-verity root is still the original one, so the updater accepts it and
  # dm-verity rejects it at boot. An update that does not match its manifest is a different
  # case (--refused-update).
  WORK="$OUT/update-work"; rm -rf "$WORK"; mkdir -p "$WORK"
  NEW_SHA=$(sha256sum "$NEW_B" | cut -d' ' -f1); NEW_BYTES=$(wc -c < "$NEW_B" | tr -d ' ')
  sh "$PROV_SH" retarget "$PROV_DIR/manifest-b.json" "${KEYS_DIR:-$OUT/keys}/update-sign.pem" "$WORK/manifest-b" "$NEW_SHA" "$NEW_BYTES" || exit 1
  mv "$OUT/bootstate.update" "$WORK/bootstate"
  "$UPDATE_BIN" --image "$NEW_B" --manifest "$WORK/manifest-b.json" --sig "$WORK/manifest-b.sig" \
    --pubkey "$KEYS_PUB/update-pub.key" --tries 3 --state "$WORK/bootstate" \
    --device "$BAD" --offset $((SYS_B_OFF_MB * MIB)) --slot-bytes $((SLOT_MB * MIB)) \
    || { echo "boot: allternit-update failed" >&2; exit 1; }
  mv "$WORK/bootstate" "$OUT/bootstate.update"
  # The updater installed the new manifest next to the boot state; put it on the ESP too.
  mcopy -o -i "$(esp "$BAD")" "$WORK/manifest-b.json" ::/allternit/manifest-b.json || exit 2
  mcopy -o -i "$(esp "$BAD")" "$WORK/manifest-b.sig" ::/allternit/manifest-b.sig || exit 2
  rm -rf "$WORK"
  grep -q '^active=b' "$OUT/bootstate.update" && grep -q '^confirmed=0' "$OUT/bootstate.update" \
    || { echo "boot: updater did not set b active + unconfirmed" >&2; cat "$OUT/bootstate.update"; exit 1; }
  mcopy -o -i "$(esp "$BAD")" "$OUT/bootstate.update" ::/allternit/bootstate || exit 2
  rm -f "$NEW_B" "$OUT/bootstate.update"
  DISK=$BAD
  LOG="$OUT/serial-bad-update.log"
  REQUIRED="${SB}slot: booting slot=b|manifest: ok slot=b|verity: FAIL slot=b|slot: fallback from=b to=a reason=verity|verity: ok slot=a|state: luks open ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok|slot: confirmed slot=a"
  FORBID="verity: ok slot=b"
elif [ "$MODE" = uki ]; then
  if [ "$IMG_TARGET" = pc ]; then
    # boot a copy: each bus run starts from a fresh image (unconfirmed slot a, 3 tries)
    BAD="$OUT/allternit-os-pc-$BUS.img"
    copy_disk "$BAD"
    DISK=$BAD
    LOG="$OUT/serial-pc-$BUS.log"
  fi
  REQUIRED="${SB}${PCLINE}slot: booting slot=a|manifest: ok slot=a|verity: ok slot=a|state: luks digest ok|state: luks open ok|kernel-check ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok|slot: confirmed slot=a"
else
  # Direct boot has no disk: no slots, no boot state, no state partition.
  REQUIRED="kernel-check ok|ALLTERNIT-OS READY kernel-interface=1|shell-tool-call ok"
fi

if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
  ACCEL="-accel kvm -cpu host"; LIMIT=300; echo "boot: KVM, ${LIMIT}s limit"
else
  ACCEL="-accel tcg -cpu max"; LIMIT=1500; echo "boot: TCG (no usable /dev/kvm), ${LIMIT}s limit"
fi

MACHINE="q35"
SBFLAGS=""
MEM=2G
# Disk and NIC. vm: virtio. pc: emulated PC hardware, so the built-in PC drivers are what runs.
DEVS="-drive if=virtio,format=raw,file=$DISK -netdev user,id=n0 -device virtio-net-pci,netdev=n0"
if [ "$IMG_TARGET" = pc ]; then
  MEM=4G
  case "$BUS" in
    ahci) DISKDEV="-device ide-hd,drive=d0,bus=ide.0" ;;
    nvme) DISKDEV="-device nvme,drive=d0,serial=allternit0" ;;
    usb) DISKDEV="-device usb-storage,drive=d0,bootindex=0" ;;
    virtio) DISKDEV="-device virtio-blk-pci,drive=d0" ;;
    *) echo "--disk-bus takes ahci, nvme, usb or virtio" >&2; exit 2 ;;
  esac
  DEVS="-drive if=none,id=d0,format=raw,file=$DISK $DISKDEV -netdev user,id=n0 -device e1000e,netdev=n0 -device qemu-xhci -device usb-kbd -device usb-mouse -vga std"
fi
if [ "$MODE" = uki ]; then
  CODE=""
  if [ "${SECUREBOOT:-0}" = 1 ]; then
    # Enforcing: the Secure Boot capable OVMF code (SMM on, secure pflash), vars with the TEST
    # PK/KEK/db enrolled. Only the UKI signed with the db key boots.
    for f in /usr/share/OVMF/OVMF_CODE_4M.secboot.fd /usr/share/OVMF/OVMF_CODE.secboot.fd; do
      [ -f "$f" ] && { CODE=$f; break; }
    done
    [ -n "$CODE" ] || { echo "no Secure Boot capable OVMF code (OVMF_CODE_4M.secboot.fd) found" >&2; exit 2; }
  else
    for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd /usr/share/ovmf/OVMF.fd; do
      [ -f "$f" ] && { CODE=$f; break; }
    done
    [ -n "$CODE" ] || { echo "no OVMF firmware found" >&2; exit 2; }
  fi
  VARS_SRC=""
  for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do
    [ -f "$f" ] && { VARS_SRC=$f; break; }
  done
  if [ "${SECUREBOOT:-0}" = 1 ]; then
    [ -n "$VARS_SRC" ] || { echo "no OVMF_VARS to enroll the test keys into" >&2; exit 2; }
    sh "$PROV_SH" enroll "$VARS_SRC" "$OUT/OVMF_VARS_sb.fd" "${KEYS_DIR:-$OUT/keys}" || exit 2
    MACHINE="q35,smm=on"
    SBFLAGS="-global driver=cfi.pflash01,property=secure,value=on"
    FW="-drive if=pflash,format=raw,unit=0,readonly=on,file=$CODE -drive if=pflash,format=raw,unit=1,file=$OUT/OVMF_VARS_sb.fd"
  elif [ -n "$VARS_SRC" ]; then
    cp "$VARS_SRC" "$OUT/OVMF_VARS.fd"
    FW="-drive if=pflash,format=raw,readonly=on,file=$CODE -drive if=pflash,format=raw,file=$OUT/OVMF_VARS.fd"
  else
    FW="-bios $CODE"
  fi
  # shellcheck disable=SC2086
  set -- qemu-system-x86_64 -machine "$MACHINE" $SBFLAGS $ACCEL -m $MEM -smp 2 -nographic -serial mon:stdio -no-reboot \
    $FW $DEVS
else
  # shellcheck disable=SC2086
  set -- qemu-system-x86_64 -machine q35 $ACCEL -m 2G -smp 2 -nographic -serial mon:stdio -no-reboot \
    -kernel "$KERNEL" -append "$CMDLINE" -netdev user,id=n0 -device virtio-net-pci,netdev=n0
fi
[ "$UNSIGNED" -eq 1 ] && [ "$LIMIT" -gt 240 ] && LIMIT=240

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

if [ -n "$CORRUPT" ] || [ "$BAD_UPDATE" -eq 1 ] || [ "$UNSIGNED" -eq 1 ]; then
  if [ "$BAD_UPDATE" -eq 1 ] && [ "$RESULT" = ok ]; then
    AFTER=$(get_state "$DISK")
    echo "boot: boot state after the run:"; echo "$AFTER"
    echo "$AFTER" | grep -q '^active=a' && echo "$AFTER" | grep -q '^confirmed=1' \
      || RESULT="boot state not back on a + confirmed"
  fi
  NAME=${CORRUPT:+corrupt slot $CORRUPT}
  [ "$UNSIGNED" -eq 1 ] && NAME="unsigned UKI refused"
  NAME=${NAME:-bad update}
  rm -f "$DISK"
  echo "boot: $NAME verdict=$RESULT"
  [ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
    echo "### os-image $NAME (steps 30b, 31)"; echo; echo "- result: $RESULT after ${T}s";
    grep -E 'secureboot:|manifest:|slot:|verity:|state:|Access Denied' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
  [ "$RESULT" = ok ]
  exit
fi
[ "$IMG_TARGET" != pc ] || rm -f "$DISK"
TPS=$(grep -o 'model tok/s=[0-9.]*' "$LOG" | tail -1)
echo "boot: ${TPS:-model tok/s=unknown}"
[ -z "${GITHUB_STEP_SUMMARY:-}" ] || {
  echo "### os-image boot (steps 29, 30a, 30b, 31${BUS:+, 32: pc/$BUS})"; echo; echo "- result: $RESULT after ${T}s"; echo "- ${TPS:-model tok/s=unknown}";
  grep -E 'secureboot:|partition |manifest:|slot:|verity:|state:|shell-tool-call' "$LOG" | sed 's/^/- /'; } >> "$GITHUB_STEP_SUMMARY"
[ "$RESULT" = ok ]
