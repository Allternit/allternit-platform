# GPU driver packs (plan step 32) - notes, nothing built yet

The PC kernel (`image/kernel/x86_64-pc.config`) has the display drivers built in: simpledrm/efifb
for the firmware framebuffer, i915 and amdgpu for KMS, with their firmware from
`image/firmware/`. That gives a console on the screen. Anything that draws with the GPU (Vulkan,
compute for the local model) needs a userland, which is a separate layer from the slot's base
tree. Nothing below is built or in the image today.

## Layer 1: Vulkan with Mesa (first)

- Intel: `anv`. AMD: `radv`. Both are open source (MIT) and ride on the in-kernel i915/amdgpu
  drivers, so no out-of-tree kernel code and nothing to sign beyond our own build.
- Needs a glibc (or musl-capable) runtime, `libdrm`, `libwayland-client`, `libxcb` stubs only if a
  windowing layer asks, and `libvulkan` (the Khronos loader). That is the same glibc runtime the
  Gizzi TUI needs (see "Boot to the shell" in `os/README.md`): build it once, put it in a layer.
- Build it from a pinned Mesa tag with `-Dvulkan-drivers=amd,intel -Dgallium-drivers=` (no GL
  drivers first), static LLVM off (radv uses ACO), and record the tag, source sha256 and licence
  in the SBOM exactly as llama.cpp is.
- Use: llama.cpp's Vulkan backend (`GGML_VULKAN=ON`) for the local model on GPUs. Today's static
  CPU llama build stays the fallback and the CI path (QEMU has no usable GPU).
- Delivery: a layer is a read-only tree that goes into the slot image at build time, so it is
  covered by the slot's dm-verity root and the signed manifest. No runtime package manager.

## Layer 2: NVIDIA userland (optional, separate, not default)

- The open kernel modules (`nvidia-open`) and the proprietary userland (libcuda, libnvidia-*, the
  GSP firmware) are out of tree. Our PC kernel has **no module loader**, so the open module would
  have to be built into the kernel image (`=y`), which NVIDIA's licence setup (MIT/GPL kernel
  side, proprietary userland and GSP blob) makes awkward to redistribute as one signed UKI. Needs
  a legal read before anything is shipped.
- **Secure Boot.** The UKI is signed by our db key and nothing else loads into the kernel, so an
  NVIDIA driver cannot be added after the fact by a user (no DKMS, no MOK flow). A GPU layer must
  therefore be part of the kernel we sign, or we add module loading with module-signature
  enforcement (`CONFIG_MODULE_SIG_FORCE`, our key) to PID 1 first. That is a decision for Eoj.
- **Licence of the userland.** The proprietary libraries are redistributable only under NVIDIA's
  licence and cannot be mixed into an image we call open. Treat them as a separate download the
  user opts into, never in the base image, with its own SBOM entry and the licence text shown.
- Order of work if it is ever picked up: Nouveau/NVK (open, Mesa, GSP-based for Turing+) as a
  Layer 1 add (`-Dvulkan-drivers=nouveau`), measure it, and only then decide on the proprietary
  stack. NVK needs the GSP firmware from linux-firmware (`nvidia/`), which is redistributable and
  would be one more group in `image/firmware/MANIFEST`.

## What is not covered

- Wayland/X11 or any compositor: the PC image shows a text console only.
- Video decode (VA-API) and ROCm/CUDA compute.
- Hybrid-graphics laptops (two GPUs): first pass is whichever KMS device comes up first.
