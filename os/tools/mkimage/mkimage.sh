#!/bin/sh
# AllternitOS image builder (plan step 27). POSIX sh plus common tools.
#
#   mkimage.sh [--out DIR] [--kernel-src DIR] [--skip-kernel] [--llama F --model F]
#
# Steps: static musl build of os/ -> initramfs file list -> kernel (tinyconfig +
# image/kernel/x86_64-vm.config, initramfs embedded) -> two read-only system slot images
# (ext4 + dm-verity hash tree, steps 30a/30b: same tree, each with its own salt and root
# hash) -> UKI with ukify when present, both slots' verity params on its command line (else
# bzImage is booted directly) -> GPT raw disk (UKI path only):
#   p1 esp (UKI + A/B boot state /allternit/bootstate) | p2 system-a | p3 system-b
#   | p4 state (LUKS2, TEST ONLY key baked into the initramfs; step 30b)
#
# Outputs in --out (default os/target/image):
#   bzImage  allternit-os.efi (UKI, if ukify)  allternit-os.img (GPT disk, if UKI)  boot.env
#   boot.env says MODE=uki or MODE=direct; qemu-boot.sh reads it. The UKI path needs
#   veritysetup and cryptsetup (cryptsetup-bin), mke2fs, and root or passwordless sudo
#   (cryptsetup open fills the LUKS2 state through dm-crypt); it never writes an
#   unverified slot or an unencrypted state partition.
#
#   --out DIR       output directory
#   --kernel-src D  use an existing, already-extracted kernel source tree (skips download)
#   --llama FILE    static llama-server (os/model/build-llama.sh). With --model it is put in the image.
#   --model FILE    the GGUF named in os/model/MODEL (sha256 checked); goes on the state partition.
#   --skip-kernel   do not run make; reuse OUT/bzImage from an earlier run. Valid only
#                   when the initramfs contents are unchanged (the initramfs is embedded).
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OS_DIR=$(cd "$SCRIPT_DIR/../.." && pwd)
OUT="$OS_DIR/target/image"
KSRC=""
SKIP_KERNEL=0
LLAMA=""
MODEL=""
TARGET=x86_64-unknown-linux-musl
CMDLINE="console=ttyS0,115200 rdinit=/sbin/init panic=-1 loglevel=4"

die() { echo "mkimage: $*" >&2; exit 1; }
say() { echo "mkimage: $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || die "--out needs a value"; OUT=$2; shift 2 ;;
    --kernel-src) [ $# -ge 2 ] || die "--kernel-src needs a value"; KSRC=$2; shift 2 ;;
    --llama) [ $# -ge 2 ] || die "--llama needs a value"; LLAMA=$2; shift 2 ;;
    --model) [ $# -ge 2 ] || die "--model needs a value"; MODEL=$2; shift 2 ;;
    --skip-kernel) SKIP_KERNEL=1; shift ;;
    -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
    *) die "unknown flag: $1" ;;
  esac
done

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
[ -z "$KSRC" ] || KSRC=$(cd "$KSRC" && pwd)
ROOT="$OUT/initramfs-root"
LIST="$OUT/initramfs.list"
KB="$OUT/kbuild"
JOBS=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)

# --- 1. static userland -----------------------------------------------------
# The repo's .cargo/config.toml sets target-dir, so ask cargo where output goes.
TARGET_DIR=$(cargo metadata --manifest-path "$OS_DIR/Cargo.toml" --no-deps --format-version 1 \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[ -n "$TARGET_DIR" ] || TARGET_DIR="$OS_DIR/target"
BIN="$TARGET_DIR/$TARGET/release"
say "building os/ for $TARGET (release)"
cargo build --manifest-path "$OS_DIR/Cargo.toml" --target "$TARGET" --release
[ -x "$BIN/init" ] || die "missing $BIN/init"
[ -x "$BIN/authority" ] || die "missing $BIN/authority"
[ -x "$BIN/shell" ] || die "missing $BIN/shell"
[ -x "$BIN/allternit-update" ] || die "missing $BIN/allternit-update"
if [ -n "$LLAMA" ]; then
  [ -x "$LLAMA" ] || die "no llama-server at $LLAMA"
  if command -v file >/dev/null 2>&1; then
    file "$LLAMA" | grep -qi 'statically linked' || die "llama-server is not static: $(file "$LLAMA")"
  fi
fi
if [ -n "$MODEL" ]; then
  [ -n "$LLAMA" ] || die "--model needs --llama"
  [ -f "$MODEL" ] || die "no model file at $MODEL"
  # shellcheck disable=SC1091
  . "$OS_DIR/model/MODEL"
  if command -v sha256sum >/dev/null 2>&1; then MSUM=$(sha256sum "$MODEL" | cut -d' ' -f1)
  else MSUM=$(shasum -a 256 "$MODEL" | cut -d' ' -f1); fi
  [ "$MSUM" = "$MODEL_SHA256" ] || die "model sha256 mismatch: got $MSUM want $MODEL_SHA256"
fi
if command -v file >/dev/null 2>&1; then
  file "$BIN/init" | grep -qi 'static' || die "init is not statically linked: $(file "$BIN/init")"
fi

# --- 2. initramfs layout ----------------------------------------------------
rm -rf "$ROOT"
mkdir -p "$ROOT/sbin" "$ROOT/usr/lib/allternit" "$ROOT/etc/allternit"
cp "$BIN/init" "$ROOT/sbin/init"
cp "$BIN/authority" "$ROOT/usr/lib/allternit/authority"
# init.toml = compiled-in default from config.rs with the authority path moved.
awk '/DEFAULT_CONFIG: &str = r#"/{f=1;next} f&&/^"#;/{exit} f' "$OS_DIR/init/src/config.rs" \
  | sed 's#/usr/bin/authority#/usr/lib/allternit/authority#' > "$ROOT/etc/allternit/init.toml"
grep -q '/usr/lib/allternit/authority' "$ROOT/etc/allternit/init.toml" || die "could not derive init.toml"
cp "$BIN/shell" "$ROOT/usr/lib/allternit/shell"
cp "$BIN/allternit-update" "$ROOT/usr/lib/allternit/allternit-update"
# TEST ONLY: LUKS2 volume key of the state partition, baked into the initramfs so PID 1 can
# open the state with dm-crypt. Anyone with the image has it. TODO(31): TPM / Secure Boot
# key binding, then this file goes away. Kept across runs so --skip-kernel (which reuses
# the bzImage with its embedded initramfs) still matches the state partition.
STATE_KEY="$OUT/TEST-ONLY-state.key"
if [ ! -s "$STATE_KEY" ] || [ "$(wc -c < "$STATE_KEY")" -ne 64 ]; then
  dd if=/dev/urandom of="$STATE_KEY" bs=64 count=1 2>/dev/null
fi
cp "$STATE_KEY" "$ROOT/etc/allternit/TEST-ONLY-state.key"
chmod 400 "$ROOT/etc/allternit/TEST-ONLY-state.key"
[ -z "$LLAMA" ] || cp "$LLAMA" "$ROOT/usr/lib/allternit/llama-server"
chmod 755 "$ROOT/sbin/init" "$ROOT/usr/lib/allternit/authority" "$ROOT/usr/lib/allternit/shell" \
  "$ROOT/usr/lib/allternit/allternit-update"

# File list for the kernel's gen_init_cpio: no root needed for device nodes.
cat > "$LIST" <<LIST_EOF
dir /dev 755 0 0
nod /dev/console 600 0 0 c 5 1
nod /dev/null 666 0 0 c 1 3
dir /proc 555 0 0
dir /sys 555 0 0
dir /run 755 0 0
dir /sysroot 755 0 0
dir /state 755 0 0
dir /tmp 1777 0 0
dir /sbin 755 0 0
file /sbin/init $ROOT/sbin/init 755 0 0
dir /usr 755 0 0
dir /usr/bin 755 0 0
dir /usr/lib 755 0 0
dir /usr/lib/allternit 755 0 0
file /usr/lib/allternit/authority $ROOT/usr/lib/allternit/authority 755 0 0
file /usr/lib/allternit/shell $ROOT/usr/lib/allternit/shell 755 0 0
file /usr/lib/allternit/allternit-update $ROOT/usr/lib/allternit/allternit-update 755 0 0
dir /etc 755 0 0
dir /etc/allternit 755 0 0
file /etc/allternit/init.toml $ROOT/etc/allternit/init.toml 644 0 0
file /etc/allternit/TEST-ONLY-state.key $ROOT/etc/allternit/TEST-ONLY-state.key 400 0 0
LIST_EOF
# llama-server goes in only when the image carries a model (step 29).
if [ -n "$LLAMA" ]; then
  chmod 755 "$ROOT/usr/lib/allternit/llama-server"
  echo "file /usr/lib/allternit/llama-server $ROOT/usr/lib/allternit/llama-server 755 0 0" >> "$LIST"
fi

# --- 3. kernel --------------------------------------------------------------
if [ "$SKIP_KERNEL" -eq 1 ]; then
  [ -f "$OUT/bzImage" ] || die "--skip-kernel but $OUT/bzImage does not exist"
  say "skipping kernel build, reusing $OUT/bzImage"
else
  # shellcheck disable=SC1091
  . "$OS_DIR/image/kernel/VERSION"
  if [ -z "$KSRC" ]; then
    DL="$OUT/cache"
    mkdir -p "$DL"
    TARBALL="$DL/linux-$KERNEL_VERSION.tar.xz"
    if [ ! -f "$TARBALL" ]; then
      say "downloading $KERNEL_URL"
      curl -fsSL -o "$TARBALL.part" "$KERNEL_URL"
      mv "$TARBALL.part" "$TARBALL"
    fi
    if command -v sha256sum >/dev/null 2>&1; then SUM=$(sha256sum "$TARBALL" | cut -d' ' -f1)
    else SUM=$(shasum -a 256 "$TARBALL" | cut -d' ' -f1); fi
    [ "$SUM" = "$KERNEL_SHA256" ] || { rm -f "$TARBALL"; die "kernel tarball sha256 mismatch: got $SUM want $KERNEL_SHA256"; }
    KSRC="$DL/linux-$KERNEL_VERSION"
    if [ ! -f "$KSRC/Makefile" ]; then
      say "extracting kernel source"
      tar -C "$DL" -xf "$TARBALL"
    fi
  fi
  [ -f "$KSRC/Makefile" ] || die "no kernel source at $KSRC"

  mkdir -p "$KB"
  FRAG="$OUT/kernel.frag"
  cp "$OS_DIR/image/kernel/x86_64-vm.config" "$FRAG"
  echo "CONFIG_INITRAMFS_SOURCE=\"$LIST\"" >> "$FRAG"
  say "configuring kernel (tinyconfig + x86_64-vm.config)"
  make -C "$KSRC" O="$KB" ARCH=x86_64 tinyconfig >/dev/null
  "$KSRC/scripts/kconfig/merge_config.sh" -m -O "$KB" "$KB/.config" "$FRAG" >/dev/null
  make -C "$KSRC" O="$KB" ARCH=x86_64 olddefconfig >/dev/null
  # merge_config does not fail on options that lose a dependency: check the ones we need.
  for opt in VIRTIO_PCI VIRTIO_BLK VIRTIO_NET VIRTIO_CONSOLE SERIAL_8250_CONSOLE DEVTMPFS \
             TMPFS EXT4_FS NET_NS USER_NS UNIX EFI_STUB BLK_DEV_INITRD ACPI BINFMT_ELF \
             MD BLK_DEV_DM DM_VERITY CRYPTO_SHA256 DM_CRYPT CRYPTO_AES CRYPTO_XTS \
             FAT_FS VFAT_FS NLS_CODEPAGE_437 NLS_ISO8859_1; do
    grep -q "^CONFIG_$opt=y" "$KB/.config" || die "CONFIG_$opt is not =y after merge (missing dependency?)"
  done
  grep -q '^CONFIG_MODULES=y' "$KB/.config" && die "modules must be off"
  say "building kernel with $JOBS jobs"
  make -C "$KSRC" O="$KB" ARCH=x86_64 -j"$JOBS" bzImage
  cp "$KB/arch/x86/boot/bzImage" "$OUT/bzImage"
fi

# --- 4. system slots (ext4 + dm-verity) + UKI + LUKS2 state + GPT disk -------
# Plan steps 30a/30b. A slot image is the userland as a read-only ext4 image with its
# dm-verity hash tree appended (veritysetup --no-superblock), so the tree starts at block
# <data blocks>. Slot B is built from the same tree as A but is its own ext4 image with its
# own salt, so each slot has its own root hash on the UKI command line. PID 1 picks the slot
# from the ESP boot state, opens it through dm-verity, reads it once (any bad block fails
# it and PID 1 falls back to the other slot), then switches root into it.
rm -f "$OUT/allternit-os.efi" "$OUT/allternit-os.img"
if command -v ukify >/dev/null 2>&1 && command -v sgdisk >/dev/null 2>&1 \
   && command -v mkfs.vfat >/dev/null 2>&1 && command -v mcopy >/dev/null 2>&1; then
  command -v mke2fs >/dev/null 2>&1 || die "mke2fs not found (e2fsprogs): needed for the system and state partitions"
  command -v veritysetup >/dev/null 2>&1 || die "veritysetup not found (cryptsetup-bin): the system slots must carry dm-verity"
  command -v cryptsetup >/dev/null 2>&1 || die "cryptsetup not found (cryptsetup-bin): the state partition is LUKS2"
  SUDO=""
  if [ "$(id -u)" -ne 0 ]; then
    sudo -n true 2>/dev/null || die "the LUKS2 state needs root or passwordless sudo (cryptsetup open)"
    SUDO="sudo -n"
  fi

  # 4a. the slot tree
  SEED="$OUT/system-seed"
  rm -rf "$SEED"
  # Mountpoints init moves its mounts onto (/proc /sys /dev /run) or mounts (/state /tmp).
  mkdir -p "$SEED/proc" "$SEED/sys" "$SEED/dev" "$SEED/run" "$SEED/state" "$SEED/tmp" \
           "$SEED/usr/bin" "$SEED/usr/lib/allternit" "$SEED/etc/allternit"
  cp -p "$ROOT/usr/lib/allternit/"* "$SEED/usr/lib/allternit/"
  cp -p "$ROOT/etc/allternit/init.toml" "$SEED/etc/allternit/init.toml"
  chmod 755 "$SEED" "$SEED/usr" "$SEED/usr/bin" "$SEED/usr/lib" "$SEED/usr/lib/allternit" "$SEED/etc" "$SEED/etc/allternit"
  chmod 1777 "$SEED/tmp"
  SEED_MB=$(du -sm "$SEED" | cut -f1)
  DATA_MB=$((SEED_MB + SEED_MB / 10 + 8))

  # 4b. one slot image per slot: build_slot a|b writes $OUT/system-<x>.img and sets
  # VERITY_<x> (<root>:<salt>:<data blocks>), ROOT_<x> and SLOT_MB_<x>.
  build_slot() {
    n=$1
    data="$OUT/system-$n-data.img"; hash="$OUT/system-$n-hash.img"; img="$OUT/system-$n.img"
    rm -f "$data" "$hash" "$img"
    truncate -s "${DATA_MB}M" "$data"
    # No journal: the slot is never written. Contents keep the build user's uid; files are
    # 755/644 and the device is read-only, so the services (uid 900-902) can run them.
    mke2fs -q -t ext4 -b 4096 -O ^has_journal -L "system-$n" -E root_owner=0:0 -d "$SEED" "$data"
    blocks=$(( $(wc -c < "$data") / 4096 ))
    # Hash tree: about blocks/127 blocks; give the file room so veritysetup never has to
    # grow it. Slack past the tree is ignored (the kernel table names the exact layout).
    truncate -s $(( (blocks / 127 + 16) * 4096 )) "$hash"
    salt=$(od -An -tx1 -N32 /dev/urandom | tr -d ' \n')
    say "dm-verity: slot $n, formatting the hash tree ($blocks data blocks)"
    veritysetup format --no-superblock --hash=sha256 --data-block-size=4096 --hash-block-size=4096 \
      --data-blocks="$blocks" --salt="$salt" "$data" "$hash" > "$OUT/verity-$n.txt"
    root=$(sed -n 's/^Root hash:[[:space:]]*//p' "$OUT/verity-$n.txt" | tr -d ' \r')
    [ ${#root} -eq 64 ] || die "veritysetup printed no sha256 root hash: $(cat "$OUT/verity-$n.txt")"
    veritysetup verify --no-superblock --hash=sha256 --data-block-size=4096 --hash-block-size=4096 \
      --data-blocks="$blocks" --salt="$salt" "$data" "$hash" "$root" \
      || die "veritysetup verify failed on the fresh slot $n image"
    hash_mb=$(( ($(wc -c < "$hash") + 1048575) / 1048576 ))
    cat "$data" "$hash" > "$img"
    rm -f "$data" "$hash" "$OUT/verity-$n.txt"
    eval "VERITY_$n=\$root:\$salt:\$blocks; ROOT_$n=\$root; SLOT_MB_$n=\$((DATA_MB + hash_mb))"
    say "dm-verity: slot $n root hash $root"
  }
  build_slot a
  build_slot b
  rm -rf "$SEED"
  [ "$ROOT_a" != "$ROOT_b" ] || die "slots a and b got the same root hash (salt not applied?)"
  SLOT_MB=$SLOT_MB_a
  [ "$SLOT_MB_b" -le "$SLOT_MB" ] || SLOT_MB=$SLOT_MB_b

  # 4c. UKI: both slots' verity params on the command line inside the UKI (a signed manifest
  # is step 31). allternit.slot=a is only the default when the ESP has no boot state.
  UKI_CMDLINE="$CMDLINE allternit.slot=a allternit.verity.a=$VERITY_a allternit.verity.b=$VERITY_b"
  say "making UKI with ukify"
  ukify build --linux "$OUT/bzImage" --cmdline "$UKI_CMDLINE" --output "$OUT/allternit-os.efi"

  # 4d. GPT disk. The GGUF lives on the state partition (plan step 29), so the disk grows with
  # the model. The state partition is LUKS2: a 16 MiB header, then the encrypted ext4.
  ESP_MB=64; LUKS_MB=16; STATE_DATA_MB=64
  if [ -n "$MODEL" ]; then
    MODEL_MB=$(( ($(wc -c < "$MODEL") / 1048576) + 1 ))
    STATE_DATA_MB=$((MODEL_MB + 128))
  fi
  STATE_MB=$((STATE_DATA_MB + LUKS_MB))
  DISK="$OUT/allternit-os.img"
  ESP_OFF_MB=1
  SYS_A_OFF_MB=$((ESP_OFF_MB + ESP_MB))
  SYS_B_OFF_MB=$((SYS_A_OFF_MB + SLOT_MB))
  STATE_OFF_MB=$((SYS_B_OFF_MB + SLOT_MB))
  TOTAL=$((STATE_OFF_MB + STATE_MB + 1))
  truncate -s "${TOTAL}M" "$DISK"
  # p1 ESP (1 MiB..) = vda1, p2 system-a = vda2, p3 system-b = vda3, p4 state = vda4 (init default)
  sgdisk -o "$DISK" >/dev/null
  sgdisk -n "1:2048:+${ESP_MB}M" -t 1:ef00 -c 1:esp \
         -n "2:0:+${SLOT_MB}M" -t 2:8300 -c 2:system-a \
         -n "3:0:+${SLOT_MB}M" -t 3:8300 -c 3:system-b \
         -n "4:0:+${STATE_MB}M" -t 4:8300 -c 4:state "$DISK" >/dev/null

  # ESP: the UKI plus the A/B boot state (os/init/src/slot.rs). A fresh image boots slot a
  # unconfirmed with 3 tries; the first healthy boot confirms it.
  ESP_IMG="$OUT/esp.img"; STATE_IMG="$OUT/state.img"; PLAIN_IMG="$OUT/state-plain.img"
  rm -f "$ESP_IMG" "$STATE_IMG" "$PLAIN_IMG"
  truncate -s "${ESP_MB}M" "$ESP_IMG"
  mkfs.vfat -F 32 -n ESP "$ESP_IMG" >/dev/null
  mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT ::/allternit
  mcopy -i "$ESP_IMG" "$OUT/allternit-os.efi" ::/EFI/BOOT/BOOTX64.EFI
  printf '# AllternitOS A/B boot state (plan step 30b). Written by PID 1 and allternit-update.\nactive=a\ntries=3\nconfirmed=0\n' \
    > "$OUT/bootstate"
  mcopy -i "$ESP_IMG" "$OUT/bootstate" ::/allternit/bootstate
  rm -f "$OUT/bootstate"
  dd if="$ESP_IMG" of="$DISK" bs=1M seek="$ESP_OFF_MB" conv=notrunc 2>/dev/null
  dd if="$OUT/system-a.img" of="$DISK" bs=1M seek="$SYS_A_OFF_MB" conv=notrunc 2>/dev/null
  dd if="$OUT/system-b.img" of="$DISK" bs=1M seek="$SYS_B_OFF_MB" conv=notrunc 2>/dev/null
  rm -f "$OUT/system-a.img" "$OUT/system-b.img"

  # State: plaintext ext4 first (mke2fs -d needs no root), then LUKS2 around it.
  truncate -s "${STATE_DATA_MB}M" "$PLAIN_IMG"
  if [ -n "$MODEL" ]; then
    # The GGUF is 644 so the model service (uid 901) reads it.
    SEED="$OUT/state-seed"
    rm -rf "$SEED"; mkdir -p "$SEED/model"
    cp "$MODEL" "$SEED/model/model.gguf"
    chmod 644 "$SEED/model/model.gguf"
    mke2fs -q -t ext4 -L state -E root_owner=0:0 -d "$SEED" "$PLAIN_IMG"
    rm -rf "$SEED"
  else
    mke2fs -q -t ext4 -L state "$PLAIN_IMG"
  fi
  truncate -s "${STATE_MB}M" "$STATE_IMG"
  # aes-xts-plain64, 512-byte sectors, data at 16 MiB. The TEST ONLY key is both the volume
  # key (what PID 1 loads into dm-crypt) and keyslot 0's passphrase (for host tools). The
  # keyslot PBKDF is cheap on purpose: the key is public anyway (step 31 replaces it).
  say "LUKS2: formatting the state partition (TEST ONLY key)"
  $SUDO cryptsetup luksFormat --batch-mode --type luks2 --cipher aes-xts-plain64 --key-size 512 \
    --sector-size 512 --offset $((LUKS_MB * 2048)) --pbkdf pbkdf2 --pbkdf-force-iterations 1000 \
    --volume-key-file "$STATE_KEY" --key-file "$STATE_KEY" "$STATE_IMG" \
    || die "cryptsetup luksFormat failed"
  MAP="allternit-mkimage-state-$$"
  # Open with the volume key, the way PID 1 does: cryptsetup checks it against the LUKS2 digest.
  $SUDO cryptsetup open --type luks2 --volume-key-file "$STATE_KEY" "$STATE_IMG" "$MAP" \
    || die "cryptsetup open with the volume key failed"
  if ! $SUDO dd if="$PLAIN_IMG" of="/dev/mapper/$MAP" bs=1M conv=fsync 2>/dev/null; then
    $SUDO cryptsetup close "$MAP"; die "could not fill the LUKS2 state"
  fi
  $SUDO cryptsetup close "$MAP" || die "cryptsetup close failed"
  rm -f "$PLAIN_IMG"
  dd if="$STATE_IMG" of="$DISK" bs=1M seek="$STATE_OFF_MB" conv=notrunc 2>/dev/null
  rm -f "$ESP_IMG" "$STATE_IMG"
  printf 'MODE=uki\nDISK=%s\nSLOT_MB=%s\nESP_OFF_MB=%s\nSYS_A_OFF_MB=%s\nSYS_B_OFF_MB=%s\nSTATE_OFF_MB=%s\nROOT_HASH=%s\nROOT_HASH_B=%s\nUPDATE_BIN=%s\n' \
    "$OUT/allternit-os.img" "$SLOT_MB" "$ESP_OFF_MB" "$SYS_A_OFF_MB" "$SYS_B_OFF_MB" "$STATE_OFF_MB" \
    "$ROOT_a" "$ROOT_b" "$BIN/allternit-update" > "$OUT/boot.env"
  say "UKI path: wrote $OUT/allternit-os.img"
else
  say "ukify/sgdisk/mkfs.vfat/mcopy not all available: FALLBACK, no UKI and no disk image."
  say "no system slot either: init runs the userland from the initramfs (no dm-verity)."
  say "the bzImage (initramfs embedded) is booted directly with: -kernel bzImage -append '$CMDLINE'"
  printf 'MODE=direct\nKERNEL=%s\nCMDLINE=%s\n' "$OUT/bzImage" "$CMDLINE" > "$OUT/boot.env"
fi
cat "$OUT/boot.env"
say "done: $OUT"
