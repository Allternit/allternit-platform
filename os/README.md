# AllternitOS boot image (Stage 2)

A bare Linux kernel plus our own Rust PID 1, no distro. This directory is a
standalone Cargo workspace (PLAN.md decision 10); the platform root
`Cargo.toml` excludes `os/`.

## What is here

- `init/` (crate `allternit-init`, binary `init`): PID 1. Linux only.
  `src/config.rs` holds the pure logic (config parser, service ordering,
  restart policy) and its unit tests; `src/linux.rs` holds the syscalls.
- `shell/` (binary `shell`): the boot shell loop. Waits for the model, sends one prompt with a
  `shell` tool, sends the tool call to the authority as a framed `check`, prints
  `shell-tool-call ok action=<a> decision=<d>` and `model tok/s=<n>`, exits 0.
- `model/`: `build-llama.sh` (static llama-server at the tag in `LLAMA`), `MODEL` (the baked
  GGUF: URL, size, sha256).
- `authority/` (crate `allternit-os-authority`, binary `authority`): the authority service. Decisions
  come from the `allternit-authority` crate (`domains/kernel/authority`, path dependency
  `../../domains/kernel/authority`, default features; pure Rust, so it builds for musl). It serves
  `check` only, on `/run/allternit/authority.sock`, in two protocols told apart by the first byte:
  init's JSON line check (`{"op":"check","class":...}`, answers `allow`/`deny`; an `ask` row is deny)
  and the contract section 2.0 framed envelope (answers `ALLOW`/`ASK`/`DENY`). Class comes from
  `extensions["x-os-v1"].action_class` (the older `os.v1` key is still read); a missing
  `autonomy_level` means `ask_first`. Human-only classes are always DENY (no token store yet);
  `app_install` is ASK at every level (G0). Every other call fails closed with `kernel_unavailable`.
  Not yet here: approval tokens, receipts, `SO_PEERCRED` caller binding. This replaced
  `authority-stub/` (PR #3275); the directory was renamed because nothing outside `os/Cargo.toml`
  referenced it (init and mkimage use the binary name `authority`, unchanged).

## Boot sequence

1. Mount `/proc`, `/sys`, `/dev` (devtmpfs), `/run` (tmpfs).
2. Open `/dev/console` as stdio and set `ttyS0` to 115200 8N1.
3. Read `/etc/allternit/init.toml` (compiled-in default if absent), set the hostname.
4. Mount the state partition at `/state` if the device exists (default `/dev/vda3`, ext4).
5. Start services in `after` order: `authority`, `model`, `shell`.
6. Self-check: one `check` that must be allowed, one (class `money`) that must be
   denied. On success init prints `kernel-check ok`, then exactly
   `ALLTERNIT-OS READY kernel-interface=1` (the step 27 CI gate).
7. Supervise: reap zombies, restart per policy. On SIGTERM/SIGINT or a clean
   (code 0) `shell` exit: SIGTERM all, sync, unmount, `reboot(RB_POWER_OFF)`.

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
`gdisk`, `dosfstools`, `mtools`, `e2fsprogs`.

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
  `allternit-os.img` (ESP, `system-a` reserved for step 30, ext4 `state` = `/dev/vda3`),
  booted through OVMF. Without it mkimage says so and `qemu-boot.sh` boots the bzImage
  directly (`boot.env` records which mode).
- Flags: `--out DIR`, `--kernel-src DIR` (existing source tree), `--skip-kernel` (reuse
  `bzImage`; only valid if the initramfs is unchanged).
- Boot gate G2a: `-nographic -serial mon:stdio -no-reboot`, `-accel kvm` when `/dev/kvm`
  is usable (120 s) else `-accel tcg` (600 s). Pass = `kernel-check ok` and
  `ALLTERNIT-OS READY kernel-interface=1` in the log, no `Kernel panic`. `model` and
  `shell` binaries do not exist yet, so init logs their failed start and QEMU is stopped by
  the checker once READY appears.

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
