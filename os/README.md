# AllternitOS boot image (Stage 2)

A bare Linux kernel plus our own Rust PID 1, no distro. This directory is a
standalone Cargo workspace (PLAN.md decision 10); the platform root
`Cargo.toml` excludes `os/`.

## What is here

- `init/` (crate `allternit-init`, binary `init`): PID 1. Linux only.
  `src/config.rs` holds the pure logic (config parser, service ordering,
  restart policy) and its unit tests; `src/linux.rs` holds the syscalls.
- `authority-stub/` (binary `authority`): a **stub**. It answers a JSON `check`
  on `/run/allternit/authority.sock` with a fixed rule. Plan step 4's
  `allternit-authority` crate replaces it.

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

## Build and boot

Nothing here was compiled locally (the train guard blocks cargo in coding
sessions). Unit tests: `cargo test --manifest-path os/Cargo.toml`. The real
musl build, kernel, initramfs and QEMU boot are step 27 (CI). Binaries
`model` and `shell` are supplied by later steps (29 and the gizzi rootfs
decision in `FINDINGS-stage2.md`); until then init logs the failed start.
