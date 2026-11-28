# AllternitOS kernel spike (Stage 3, plan step 34)

Boots our Stage 2 userland (init, authority stub, shell, static llama-server, Qwen2.5-0.5B
GGUF) on the Asterinas kernel instead of the pinned Linux. It decides gate G3.

## Gate G3

- **Adopt Asterinas** if the serial log shows `kernel-check ok` and
  `ALLTERNIT-OS READY kernel-interface=1` AND the model produces at least one inference
  token (`shell-tool-call ok`, then `model tok/s=`).
- **Otherwise stay on Linux** and file the gap list (what failed, from the serial log).

READY alone is progress: the CI job "Boot and check READY" gates on it. The "Model lines"
step is `continue-on-error`. If it fails, that is information for the gap list, not a
blocker for this PR.

## Run

CI: `.github/workflows/os-kernel-spike.yml` (workflow_dispatch, or a PR touching
`os/kernel-spike/**`). It builds the userland, llama-server and GGUF with the same cache keys as
`os-image.yml`, clones Asterinas at the tag in `ASTERINAS` (v0.19.0), and boots in the
`asterinas/asterinas:<DOCKER_IMAGE_VERSION>` dev image (the version file at the root of that tag).
The serial log is the `os-kernel-spike-serial-log` artifact.

By hand (OSDK needs Ubuntu x86_64, so use the Asterinas Docker image), after building
`os/` for `x86_64-unknown-linux-musl`, plus `os/target/llama/llama-server` and
`os/target/model/model.gguf`:

    fakeroot sh os/kernel-spike/run-asterinas.sh initramfs
    git clone --depth 1 --branch v0.19.0 https://github.com/asterinas/asterinas os/target/asterinas
    # inside the dev image:
    sh os/kernel-spike/run-asterinas.sh boot
    sh os/kernel-spike/run-asterinas.sh judge

## What differs from the Linux image (fixes B1 and B2)

- The GGUF is inside the initramfs at `/model/model.gguf`. No ext4 state partition; the Linux
  image and `mkimage.sh` are unchanged.
- `init.toml` is derived from the compiled-in default with `state_device` removed, the model
  path moved, and the shell HOME on tmpfs. The kernel args `allternit.state=none` and
  `allternit.model=/model/model.gguf` (handled in `os/init/src/cmdline.rs`) say the same
  thing when `/proc/cmdline` exists. Without them init behaves exactly as before.
- Kernel args also carry `rdinit=/sbin/init init=/sbin/init`.

## Gaps still open (FINDINGS-stage3.md)

- Not verified: whether the static glibc llama-server starts on Asterinas; mmap of a ~490 MB
  GGUF from the initramfs; the token rate under TCG; whether OSDK accepts this cpio size.
- The `cargo osdk run` flags (`--initramfs`, `--kcmd-args`, `--boot-method qemu-direct`) are
  written from the OSDK docs and not yet run. The first CI log decides.
- Asterinas may lack `/proc/cmdline`, devtmpfs, or the netlink/ioctl pieces init uses
  (loopback up, chown, setuid). The first boot log lists them.
- QEMU memory and KVM come from OSDK defaults; the container only passes `/dev/kvm` through.
- B3 is handled by the Docker image, not removed: any host without it cannot run OSDK.
- The step 30a verity code (PR #3292) is not involved here.
