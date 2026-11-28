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
   `slot: booting slot=<x> confirmed=<0|1> tries-left=<n>`. Open that slot (`a` =
   `/dev/vda2`, `b` = `/dev/vda3`) as a read-only dm-verity device with its own root hash
   from `allternit.verity.<slot>=<root>:<salt>:<data blocks>`, read it all once, and print
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
allternit-update --image system-b.img [--tries 3] [--uki new.efi]
```

1. Reads the boot state; refuses unless the running slot is confirmed (the other slot is
   the fallback until then).
2. Writes the image to the inactive slot device (refuses an image larger than the slot).
3. With `--uki`, replaces the UKI on the ESP. The new slot's root hash is on the UKI
   command line, never in the boot state, so an update that changes the slot image ships a
   UKI that carries it. TODO(31): one signed UKI per slot so a bad UKI can fall back too.
4. Writes the boot state: new slot active, unconfirmed, N tries. The next boot tries it.

`--device PATH --offset BYTES --slot-bytes N --state FILE` let CI write a slot inside a raw
disk image with the same code (`qemu-boot.sh --bad-update`).

## LUKS2 state: TEST KEY WARNING

**The state key is a TEST ONLY key and protects nothing.** mkimage.sh makes a random 64-byte
key (`os/target/image/TEST-ONLY-state.key`, kept across runs so `--skip-kernel` still
matches), uses it as the LUKS2 volume key and keyslot 0 passphrase, and bakes it into the
initramfs at `/etc/allternit/TEST-ONLY-state.key`. The initramfs is inside the UKI on the
unencrypted ESP, so anyone with the disk image can open the state partition. Never ship an
image built this way to a user. TODO(31): bind the key to the TPM / Secure Boot state and
remove the baked key; also check the key against the LUKS2 digest (today a wrong key is
caught by the missing ext4 superblock behind dm-crypt).

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
  by its dm-verity hash tree (`veritysetup format --no-superblock`, sha256, 4 KiB blocks, random
  salt per slot). Each slot's root hash, salt and data block count go on the UKI command
  line (`allternit.slot=a allternit.verity.a=... allternit.verity.b=...`); a signed manifest
  is step 31. The UKI path stops if `veritysetup`, `cryptsetup` or `mke2fs` is missing, or
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
