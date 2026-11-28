# AllternitOS boot image (Stage 2)

A bare Linux kernel plus our own Rust PID 1, no distro. This directory is a
standalone Cargo workspace (PLAN.md decision 10); the platform root
`Cargo.toml` excludes `os/`.

## What is here

- `init/` (crate `allternit-init`, binary `init`): PID 1. Linux only.
  `src/config.rs` holds the pure logic (config parser, service ordering,
  restart policy) and its unit tests; `src/verity.rs` holds the slot/dm-verity logic
  (command line, verity table, device-mapper ioctl buffer) and its tests; `src/slot.rs` the
  A/B boot state rules and `src/luks.rs` the LUKS2 header and dm-crypt table (both with
  tests); `src/linux.rs` holds the syscalls.
- `update/` (binary `allternit-update`, step 30b): writes a system image into the inactive
  slot and sets it active, unconfirmed, with N tries. See "A/B slots and updates".
- `shell/` (binary `shell`): the boot shell loop. Waits for the model, sends one prompt with a
  `shell` tool, sends the tool call to the authority as a framed `check`, prints
  `shell-tool-call ok action=<a> decision=<d>` and `model tok/s=<n>`, exits 0.
- `model/`: `build-llama.sh` (static llama-server at the tag in `LLAMA`), `MODEL` (the baked
  GGUF: URL, size, sha256).
- `authority-stub/` (binary `authority`): a **stub**. It answers a JSON `check`
  on `/run/allternit/authority.sock` with a fixed rule. Plan step 4's
  `allternit-authority` crate replaces it.

## Boot sequence

1. Mount `/proc`, `/sys`, `/dev` (devtmpfs), `/run` (tmpfs).
2. Open `/dev/console` as stdio and set `ttyS0` to 115200 8N1.
2b. System slot (steps 30a, 30b). If the kernel command line names `allternit.slot=<a|b>`
   (the default slot), read the TEST ONLY state key from the initramfs, mount the ESP
   (`/dev/vda1`, vfat) at `/run/esp`, read the A/B boot state `/run/esp/allternit/bootstate`
   (see "A/B slots and updates") and pick the slot; print
   `secureboot: enabled|disabled|unknown` (first line, from the EFI variable) and
   `slot: booting slot=<x> confirmed=<0|1> tries-left=<n>`. Open that slot (`a` =
   `/dev/vda2`, `b` = `/dev/vda3`) as a read-only dm-verity device with its own root hash
   from that slot's signed manifest on the ESP (step 31: `manifest: ok slot=<x>` after the
   Ed25519 check; verity root, salt, data blocks), read it all once, and print
   `verity: ok slot=<x>`. Then mount it read-only, move `/proc /sys /dev /run` (with the ESP
   under it) into it, switch root, and mount a tmpfs on `/tmp`. A slot that fails prints
   `verity: FAIL slot=<x> (<reason>)` and `slot: fallback from=<x> to=<y> reason=verity`, and
   PID 1 tries the other slot; if that fails too it powers off. No `allternit.slot` (direct
   boot): stay on the initramfs.
3. Read `/etc/allternit/init.toml` (compiled-in default if absent), set the hostname.
4. Mount the state partition at `/state` if the device exists (default `/dev/vda4`, ext4).
   When it starts with a LUKS2 header (step 30b), PID 1 parses the header, loads a
   dm-crypt table (`/dev/mapper/state`) with the TEST ONLY volume key, checks for an ext4
   superblock behind it, prints `state: luks open ok`, then mounts it.
5. Start services in `after` order: `authority`, `model`, `shell`.
6. Self-check: one `check` that must be allowed, one (class `money`) that must be
   denied. On success init prints `kernel-check ok`, then exactly
   `ALLTERNIT-OS READY kernel-interface=1` (the step 27 CI gate).
7. Supervise: reap zombies, restart per policy. A clean (code 0) `shell` exit means the
   shell answered its first tool call (`shell-tool-call ok`): PID 1 writes the running slot
   as confirmed and prints `slot: confirmed slot=<x>`. On SIGTERM/SIGINT or that clean
   `shell` exit: SIGTERM all, sync, unmount, `reboot(RB_POWER_OFF)`.

## Disk layout (UKI disk)

| GPT | Device | Contents |
|---|---|---|
| `esp` | vda1 | vfat: `EFI/BOOT/BOOTX64.EFI` (the UKI; its command line carries `allternit.slot=a` and both slots' verity params) and `allternit/bootstate` (A/B boot state) |
| `system-a` | vda2 | read-only ext4 userland + dm-verity hash tree, own salt and root hash |
| `system-b` | vda3 | same size; built from the same tree as A but its own ext4 image, salt and root hash |
| `state` | vda4 | LUKS2 (aes-xts-plain64, data at 16 MiB) around ext4: `/state/model/model.gguf`, `/state/shell` |

## A/B slots and updates (step 30b)

The boot state is a text file on the ESP, `allternit/bootstate`:

```text
active=a
tries=3
confirmed=0
```

- Confirmed slot: boots as is.
- Unconfirmed with tries left: boots and loses one try (written before the boot).
- Unconfirmed with no tries left: not booted; PID 1 prints
  `slot: fallback from=<x> to=<y> reason=tries-exhausted` and boots the other slot.
- dm-verity failure: `slot: fallback from=<x> to=<y> reason=verity`, then the other slot.
- The slot PID 1 falls back to is the one that ran before the update, so it is written as
  confirmed. At most one switch per boot.
- Health check: the shell answering its first tool call confirms the running slot
  (`slot: confirmed slot=<x>`). A fresh image starts as `a`, unconfirmed, 3 tries.
- Logic and tests: `init/src/slot.rs`. Writes are temp file, fsync, rename.

Update flow, `update/` (binary `allternit-update`, in the slot at
`/usr/lib/allternit/allternit-update`):

```sh
allternit-update --image system-b.img --manifest manifest-b.json --sig manifest-b.sig [--tries 3] [--uki new.efi]
```

1. Reads the boot state; refuses unless the running slot is confirmed (the other slot is
   the fallback until then).
2. Step 31: verifies the signed manifest (Ed25519, public key `/etc/allternit/update-pub.key`
   in the slot image; `--pubkey` overrides for CI), that it is for the target slot, and that
   the image's sha256 and size (and the UKI's sha256 with `--uki`) match it. Unsigned,
   changed, wrongly signed or mismatched updates are refused with the reason before anything
   is written.
3. Writes the image to the inactive slot device (refuses an image larger than the slot).
4. With `--uki`, replaces the UKI on the ESP (kernel updates only: the slot's dm-verity root
   lives in its manifest, so a new slot image needs no new UKI).
5. Installs `manifest-<slot>.json` + `.sig` next to the boot state on the ESP.
6. Writes the boot state: new slot active, unconfirmed, N tries. The next boot tries it.

`--device PATH --offset BYTES --slot-bytes N --state FILE` let CI write a slot inside a raw
disk image with the same code (`qemu-boot.sh --bad-update`).

## LUKS2 state: TEST KEY WARNING

**The state key is a TEST ONLY key and protects nothing.** mkimage.sh makes a random 64-byte
key (`os/target/image/TEST-ONLY-state.key`, kept across runs so `--skip-kernel` still
matches), uses it as the LUKS2 volume key and keyslot 0 passphrase, and bakes it into the
initramfs at `/etc/allternit/TEST-ONLY-state.key`. The initramfs is inside the UKI on the
unencrypted ESP, so anyone with the disk image can open the state partition. Never ship an
image built this way to a user. Step 31: PID 1 checks the key against the LUKS2 digest
(PBKDF2-HMAC-SHA256 from the header JSON, `luks.rs`) before dm-crypt is set up and prints
`state: luks digest ok`; the ext4 superblock check stays as a second guard. TODO(hardware):
seal the key to the TPM (PCR 7 policy) on real hardware and remove the baked key; CI has no
TPM, so this is documented, not built.

## Provenance, Secure Boot, reproducibility (step 31)

Tooling: `os/provenance/provenance.sh` (`keygen`, `sbom`, `manifest`, `verify`, `retarget`,
`enroll`). Needs openssl 3, jq, sbsign, virt-fw-vars (CI installs them).

- **Keys are TEST keys, made fresh per build, and never committed.** `keygen` writes an
  Ed25519 update-signing key (`update-sign.pem`) and Secure Boot PK/KEK/db keys + RSA certs
  into `os/target/image/keys/` (ignored by git, never uploaded). Only `keys/public/` (certs,
  `update-pub.pem`, raw `update-pub.key`) is copied to `os/target/image/provenance/`. The old
  AllternitOS repo leaked Secure Boot `*.key` files and an `update-seckey.pem` into history;
  `os/provenance/.gitignore` and the CI Verify step (no private key in the upload folder)
  keep that from repeating. A real release key is a later decision (GitHub Actions secret or
  an offline signer): swap `update-sign.pem` and the public half at image build.
- **Signed manifest per slot**: `manifest-a.json` / `manifest-b.json` + `.sig`: schema,
  version, slot, image sha256 + size, dm-verity root + salt + data blocks, UKI sha256, SBOM
  sha256. Compact JSON with sorted keys, raw 64-byte Ed25519 signature over those exact
  bytes. They sit on the ESP in `allternit/`; PID 1 verifies the booting slot's manifest with
  the public key baked into the initramfs and takes the verity parameters from it
  (`manifest: ok slot=a version=...`); a bad manifest fails the slot and falls back like a
  verity failure. This replaces the verity params on the UKI command line: there is one UKI,
  it never changes for a slot update, and each slot is vouched for on its own.
  TODO: anti-rollback (monotonic version counter in a UEFI variable / TPM).
- **SBOM**: `provenance/sbom.spdx.json` (SPDX 2.3): kernel version, tarball sha256, kernel
  config hashes; llama.cpp tag b12823 + commit + built binary sha256; the GGUF sha256; each
  Rust binary's sha256 with the crates it links, resolved by `cargo tree` from `Cargo.lock`
  (now committed) with crates.io checksums.
- **Secure Boot**: the UKI is signed with `sbsign` (db key). `qemu-boot.sh` enrolls the TEST
  PK/KEK/db into a copy of the OVMF vars with `virt-fw-vars`, boots the Secure Boot OVMF
  (`OVMF_CODE_4M.secboot.fd`, SMM on, secure pflash), and PID 1 prints `secureboot: enabled`
  from the EFI `SecureBoot` variable (efivarfs; kernel `CONFIG_EFIVAR_FS=y`). Negative case:
  `qemu-boot.sh --unsigned-uki` puts the unsigned UKI on the ESP; firmware must print
  `Access Denied` and PID 1 must never start. Not covered: a real TPM, measured boot,
  shim/MOK, key rotation.
- **Update refusals**: `qemu-boot.sh --refused-update` runs the real updater with an image
  that does not match its manifest, a manifest changed after signing, a manifest signed by
  another key, and no manifest at all; each must be refused with the reason and leave the
  boot state alone. `--bad-update` now installs a VALIDLY signed update whose verity root is
  wrong, so the fallback path is still tested.
- **Reproducibility**: `SOURCE_DATE_EPOCH` = the git commit time; ext4 UUID, directory hash
  seed, verity salt, GPT GUIDs and the FAT volume id derive from the commit; mke2fs uses
  `E2FSPROGS_FAKE_TIME`; tree mtimes are set to `SOURCE_DATE_EPOCH`; Rust paths are remapped;
  the initramfs file list has a fixed order. mkimage builds slot A twice from the same tree
  and fails if the sha256 differs (`--skip-repro-check` to skip; result in
  `provenance/reproducibility.txt`). Scope: two builds of the same commit with the same key
  make identical slot images. Not reproducible, by design or not yet: the TEST keys differ per
  run (the public key is inside the slot, so slots differ across runs unless the key dir is
  kept), the LUKS2 state (random header salts), and the kernel (embeds the initramfs and the
  key; kernel build determinism is not asserted). `mke2fs -d` copies directory entries in
  readdir order; if CI shows that flaking, the fix is building the tree through a sorted tar.

## Targets: vm and pc (step 32)

`mkimage.sh --target vm|pc` (default `vm`). Both targets share the PID 1, services, A/B slots,
dm-verity, LUKS2 state, signed manifests and Secure Boot path; they differ in the kernel config,
the firmware and the console.

| | `vm` | `pc` |
|---|---|---|
| Kernel config | `image/kernel/x86_64-vm.config` | `image/kernel/x86_64-pc.config` (the VM config plus drivers; mkimage fails if a VM line is dropped, except `CONFIG_VT`) |
| Disk, NIC | virtio-blk, virtio-net | NVMe, AHCI/SATA, USB mass storage; e1000e, igb, igc, r8169; Wi-Fi iwlwifi (mvm) and mt7921e |
| Input, display | none (serial) | USB xHCI/EHCI/OHCI/UHCI + HID, PS/2; simpledrm/efifb, i915, amdgpu, bochs (QEMU VGA); VT + framebuffer console |
| Firmware | none | pinned linux-firmware blobs, see below |
| Console | `ttyS0` | `tty0` + `ttyS0` |

**No kernel modules.** PID 1 has no module loader, so every driver in the PC config is built in
(`=y`) and `CONFIG_MODULES` stays off. Adding a module later needs a loader in PID 1 first.

**Partitions by name.** The disk layout is the same, but a PC disk is `/dev/nvme0n1` or
`/dev/sda`, not `/dev/vda`. PID 1 reads each partition's GPT name (`esp`, `system-a`, `system-b`,
`state`) from sysfs and makes `/dev/vda1`..`/dev/vda4` symlinks to the real devices when `/dev/vda1`
does not exist (`link_partition_aliases` in `init/src/linux.rs`; the VM boot is unchanged). It logs
`init: partition state: /dev/vda4 -> /dev/sda4`. With two disks that carry these names, the first
found wins (TODO: match the disk the UKI booted from).

**Firmware** (`image/firmware/`). `LINUX_FIRMWARE` pins one linux-firmware tag (`20261019`) with
the tarball sha256 kernel.org publishes; `MANIFEST` lists the globs per group (iwlwifi, mt7921,
r8169, i915 DMC, amdgpu) and the licence file WHENCE names for each. `fetch.sh` checks the
tarball sha256 before it reads anything, stages each blob xz-compressed
(`CONFIG_FW_LOADER_COMPRESS_XZ`), and writes `firmware.list` (sha256 of each upstream blob),
`groups.tsv` and the licence texts. The tree is in `/lib/firmware` of the initramfs (built-in
drivers probe before the slot is mounted) and of the system slot (loads after the switch), and the
licences are in `/usr/share/licenses/linux-firmware`. The SBOM gets the tag + tarball sha256, one
package per group with a hash of its file list, and each licence as a `LicenseRef` with the text.
The tree is capped by `FW_MAX_MB` (UKI size follows it); the PC ESP is sized to the UKI.
Not included: GuC/HuC, other AMD families, Bluetooth, `regulatory.db` (Wi-Fi uses the built-in
world regulatory domain). Add a line to `MANIFEST` for more.

**Boot to the shell.** On the PC target the shell service is the same boot shell loop as the VM
(`shell/`), writing to `/dev/console`: the screen (tty0) when the PC has no serial port, the
serial line when it has one (and always in CI, where the log is the evidence). The Gizzi TUI is
the next step, not in this build: the Gizzi binary is a Bun-compiled glibc executable
(FINDINGS-stage2.md), and this image has no glibc, no ICU data, no terminfo and no CA bundle in
the slot, plus no keyboard-reading TTY service in init. First step later: a glibc runtime layer
in the slot (built from a pinned source, verity-covered like the rest), `tty1` with a login-less
`[[service]]` that runs `gizzi` on `/dev/tty1`, and a `tty` field in the service table.

**Wi-Fi is driver + firmware only.** Nothing in the image brings a Wi-Fi link up (no wpa_supplicant
or iwd). Wired NICs are probed (the kernel logs the link) but init only brings loopback up, as on the VM: no NIC gets an address yet.

**GPU userland** is separate: see `image/drivers/README.md` (Vulkan Mesa first, NVIDIA optional).

**Real hardware: not checked.** CI boots the PC image on emulated PC devices: `qemu-boot.sh
--disk-bus ahci|nvme` (q35, e1000e, qemu-xhci + usb-kbd + usb-mouse, std VGA, OVMF Secure
Boot enforcing). Real NVMe/SATA/Wi-Fi/GPU behaviour is unverified until a PC runs it.

## Accounts

| Service | uid/gid | Notes |
|---|---|---|
| authority | 900 | owns `/run/allternit`; socket mode 0666 |
| model | 901 | private network namespace (`CLONE_NEWNET`), unix socket only at `/run/allternit/model/model.sock` (ADR-0006) |
| shell | 902 | home `/state/shell`; powers the machine off on a clean exit |

Service fields in `init.toml`: `name`, `path`, `args`, `uid`, `gid`, `env`
(`KEY=VALUE` strings), `restart` (`never`, `on-failure`, `always`), `netns`,
`after`, `poweroff_on_clean_exit`.

## Build the image

Step 27. CI does this in `.github/workflows/os-image.yml` (path-filtered to
`os/**`); the same commands work on any x86_64 Linux host with `musl-tools`, the
kernel build deps, `qemu-system-x86`, `ovmf`, and (for the UKI path) `systemd-ukify`,
`gdisk`, `dosfstools`, `mtools`, `e2fsprogs`, `cryptsetup-bin` (veritysetup, cryptsetup), and
root or passwordless sudo for `cryptsetup open`.

```sh
rustup target add x86_64-unknown-linux-musl
sh os/tools/mkimage/mkimage.sh            # -> os/target/image/
sh os/tools/mkimage/qemu-boot.sh          # boots it, judges the serial log
```

- Kernel: one pinned longterm release in `image/kernel/VERSION` (version, URL,
  sha256 from kernel.org's `sha256sums.asc`; the download is verified). Config:
  `make tinyconfig` plus `image/kernel/x86_64-vm.config` (virtio only, no modules).
- initramfs: `/sbin/init`, `/usr/lib/allternit/authority`, `/etc/allternit/init.toml`
  (derived from the default in `init/src/config.rs`), embedded in the kernel through a
  `gen_init_cpio` file list, so no root is needed.
- With `ukify` present: one UKI (`allternit-os.efi`) on the ESP of a GPT raw disk
  `allternit-os.img`, booted through OVMF. Partitions: see "Disk layout". A system slot is the userland (`/usr/lib/allternit/*`,
  `/etc/allternit/init.toml`) as a read-only ext4 image without a journal, followed directly
  by its dm-verity hash tree (`veritysetup format --no-superblock`, sha256, 4 KiB blocks, salt
  derived from the commit per slot). Each slot's root hash, salt and data block count were on the UKI command
  line in step 30; since step 31 they live in per-slot signed manifests on the ESP and the UKI
  (Secure Boot signed with the TEST db key) only carries `allternit.slot=a`. The UKI path also needs `sbsign`, `jq` and `openssl`. The UKI path stops if `veritysetup`, `cryptsetup` or `mke2fs` is missing, or
  if it has neither root nor passwordless sudo (`cryptsetup open` fills the LUKS2 state
  through dm-crypt), so it never writes an unverified slot or an unencrypted state. The initramfs still carries the userland for the direct-boot fallback. Without it mkimage says so and `qemu-boot.sh` boots the bzImage
  directly (`boot.env` records which mode).
- Flags: `--out DIR`, `--kernel-src DIR` (existing source tree), `--skip-kernel` (reuse
  `bzImage`; only valid if the initramfs is unchanged).
- Negative case (steps 30a, 30b): `qemu-boot.sh --corrupt-slot a` boots a copy of the disk
  with one byte flipped 1 MiB into `system-a` and the boot state set to `a`, confirmed. It
  passes on `verity: FAIL slot=a`, `slot: fallback from=a to=b reason=verity`,
  `verity: ok slot=b`, `state: luks open ok` and READY, and fails on any
  `verity: ok slot=a`.
- Bad update (step 30b): `qemu-boot.sh --bad-update` sets the boot state to `a` confirmed,
  takes slot B out of the disk, flips one byte, installs it with `allternit-update`
  (`b` active, unconfirmed, 3 tries) and boots. It passes on `slot: booting slot=b`,
  `verity: FAIL slot=b`, `slot: fallback from=b to=a reason=verity`, `verity: ok slot=a`,
  `state: luks open ok`, READY, `shell-tool-call ok`, `slot: confirmed slot=a` in that
  order, no `verity: ok slot=b`, and the boot state left on the disk saying `a`, confirmed.
- Boot gate G2a: `-nographic -serial mon:stdio -no-reboot`, `-accel kvm` when `/dev/kvm`
  is usable (300 s) else `-accel tcg` (1500 s). Pass = in this order `slot: booting slot=a`,
  `verity: ok slot=a`, `state: luks open ok` (those three on the UKI disk only),
  `kernel-check ok`, `ALLTERNIT-OS READY kernel-interface=1`, `shell-tool-call ok` and
  `slot: confirmed slot=a` (UKI disk only), no `Kernel panic`; QEMU is stopped by the
  checker once they all appear.

## Build and boot

Nothing here was compiled locally (the train guard blocks cargo in coding
sessions). Unit tests: `cargo test --manifest-path os/Cargo.toml`. The real
musl build, kernel, initramfs and QEMU boot are step 27 (see "Build the image"). Binaries
`model` and `shell` are supplied by later steps (29 and the gizzi rootfs
decision in `FINDINGS-stage2.md`); until then init logs the failed start.

## Local model (step 29)

Decision: **llama.cpp** for the small baked model, built static and CPU-only; colibri stays
for the big MoE models later. Why: a small dense GGUF is llama.cpp's sweet spot, and colibri
targets MoE models larger than RAM.

- `model` service: `/usr/lib/allternit/llama-server -m /state/model/model.gguf`, uid 901, on
  `127.0.0.1:8080`. First version runs in the root network namespace (init brings `lo` up).
  TODO(ADR-0006): private netns plus a socket proxy.
- The GGUF sits on the state partition (`mke2fs -d`), so the initramfs stays small.
- Gate G2c: CI boots with `-m 2G` and needs `shell-tool-call ok` in the serial log; tok/s is
  in the job summary. Build the pieces by hand with `model/build-llama.sh` then
  `mkimage.sh --llama os/target/llama/llama-server --model <gguf>`.
