#!/bin/sh
# AllternitOS image builder (plan step 27). POSIX sh plus common tools.
#
#   mkimage.sh [--out DIR] [--kernel-src DIR] [--skip-kernel]
#
# Steps: static musl build of os/ -> initramfs file list -> kernel (tinyconfig +
# image/kernel/x86_64-vm.config, initramfs embedded) -> UKI with ukify when present
# (else bzImage is booted directly) -> GPT raw disk with an ESP (UKI path only).
#
# Outputs in --out (default os/target/image):
#   bzImage  allternit-os.efi (UKI, if ukify)  allternit-os.img (GPT disk, if UKI)  boot.env
#   boot.env says MODE=uki or MODE=direct; qemu-boot.sh reads it.
#
#   --out DIR       output directory
#   --kernel-src D  use an existing, already-extracted kernel source tree (skips download)
#   --skip-kernel   do not run make; reuse OUT/bzImage from an earlier run. Valid only
#                   when the initramfs contents are unchanged (the initramfs is embedded).
set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
OS_DIR=$(cd "$SCRIPT_DIR/../.." && pwd)
OUT="$OS_DIR/target/image"
KSRC=""
SKIP_KERNEL=0
TARGET=x86_64-unknown-linux-musl
CMDLINE="console=ttyS0,115200 rdinit=/sbin/init panic=-1 loglevel=4"

die() { echo "mkimage: $*" >&2; exit 1; }
say() { echo "mkimage: $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || die "--out needs a value"; OUT=$2; shift 2 ;;
    --kernel-src) [ $# -ge 2 ] || die "--kernel-src needs a value"; KSRC=$2; shift 2 ;;
    --skip-kernel) SKIP_KERNEL=1; shift ;;
    -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
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
BIN="$OS_DIR/target/$TARGET/release"
say "building os/ for $TARGET (release)"
cargo build --manifest-path "$OS_DIR/Cargo.toml" --target "$TARGET" --release
[ -x "$BIN/init" ] || die "missing $BIN/init"
[ -x "$BIN/authority" ] || die "missing $BIN/authority"
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
chmod 755 "$ROOT/sbin/init" "$ROOT/usr/lib/allternit/authority"

# File list for the kernel's gen_init_cpio: no root needed for device nodes.
cat > "$LIST" <<LIST_EOF
dir /dev 755 0 0
nod /dev/console 600 0 0 c 5 1
nod /dev/null 666 0 0 c 1 3
dir /proc 555 0 0
dir /sys 555 0 0
dir /run 755 0 0
dir /state 755 0 0
dir /tmp 1777 0 0
dir /sbin 755 0 0
file /sbin/init $ROOT/sbin/init 755 0 0
dir /usr 755 0 0
dir /usr/bin 755 0 0
dir /usr/lib 755 0 0
dir /usr/lib/allternit 755 0 0
file /usr/lib/allternit/authority $ROOT/usr/lib/allternit/authority 755 0 0
dir /etc 755 0 0
dir /etc/allternit 755 0 0
file /etc/allternit/init.toml $ROOT/etc/allternit/init.toml 644 0 0
LIST_EOF

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
             TMPFS EXT4_FS NET_NS USER_NS UNIX EFI_STUB BLK_DEV_INITRD ACPI BINFMT_ELF; do
    grep -q "^CONFIG_$opt=y" "$KB/.config" || die "CONFIG_$opt is not =y after merge (missing dependency?)"
  done
  grep -q '^CONFIG_MODULES=y' "$KB/.config" && die "modules must be off"
  say "building kernel with $JOBS jobs"
  make -C "$KSRC" O="$KB" ARCH=x86_64 -j"$JOBS" bzImage
  cp "$KB/arch/x86/boot/bzImage" "$OUT/bzImage"
fi

# --- 4. UKI + GPT disk ------------------------------------------------------
rm -f "$OUT/allternit-os.efi" "$OUT/allternit-os.img"
if command -v ukify >/dev/null 2>&1 && command -v sgdisk >/dev/null 2>&1 \
   && command -v mkfs.vfat >/dev/null 2>&1 && command -v mcopy >/dev/null 2>&1; then
  say "making UKI with ukify"
  ukify build --linux "$OUT/bzImage" --cmdline "$CMDLINE" --output "$OUT/allternit-os.efi"

  ESP_MB=64; SYS_MB=16; STATE_MB=64
  DISK="$OUT/allternit-os.img"
  TOTAL=$((1 + ESP_MB + SYS_MB + STATE_MB + 1))
  truncate -s "${TOTAL}M" "$DISK"
  # p1 ESP (1 MiB..), p2 reserved system slot (step 30), p3 state (the init default /dev/vda3)
  sgdisk -o "$DISK" >/dev/null
  sgdisk -n "1:2048:+${ESP_MB}M" -t 1:ef00 -c 1:esp \
         -n "2:0:+${SYS_MB}M" -t 2:8300 -c 2:system-a \
         -n "3:0:+${STATE_MB}M" -t 3:8300 -c 3:state "$DISK" >/dev/null
  ESP_IMG="$OUT/esp.img"; STATE_IMG="$OUT/state.img"
  rm -f "$ESP_IMG" "$STATE_IMG"
  truncate -s "${ESP_MB}M" "$ESP_IMG"
  mkfs.vfat -F 32 -n ESP "$ESP_IMG" >/dev/null
  mmd -i "$ESP_IMG" ::/EFI ::/EFI/BOOT
  mcopy -i "$ESP_IMG" "$OUT/allternit-os.efi" ::/EFI/BOOT/BOOTX64.EFI
  dd if="$ESP_IMG" of="$DISK" bs=1M seek=1 conv=notrunc 2>/dev/null
  if command -v mke2fs >/dev/null 2>&1; then
    truncate -s "${STATE_MB}M" "$STATE_IMG"
    mke2fs -q -t ext4 -L state "$STATE_IMG"
    dd if="$STATE_IMG" of="$DISK" bs=1M seek=$((1 + ESP_MB + SYS_MB)) conv=notrunc 2>/dev/null
  else
    say "mke2fs not found: state partition left unformatted (init skips the state mount)"
  fi
  rm -f "$ESP_IMG" "$STATE_IMG"
  printf 'MODE=uki\nDISK=%s\n' "$OUT/allternit-os.img" > "$OUT/boot.env"
  say "UKI path: wrote $OUT/allternit-os.img"
else
  say "ukify/sgdisk/mkfs.vfat/mcopy not all available: FALLBACK, no UKI and no disk image."
  say "the bzImage (initramfs embedded) is booted directly with: -kernel bzImage -append '$CMDLINE'"
  printf 'MODE=direct\nKERNEL=%s\nCMDLINE=%s\n' "$OUT/bzImage" "$CMDLINE" > "$OUT/boot.env"
fi
cat "$OUT/boot.env"
say "done: $OUT"
