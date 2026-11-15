# Engine pack spike (S3): signed Metal llama-server + MLX sidecar shape

Date: 2026-11-15. Plan: `PLAN-on-device-models-2026-11-15.md` (decisions 1 and 2, risk "Installer size and notarization").
Status: spike. Nothing published to runtime.allternit.com. No binaries or models in git.

## Result

| Check | Result |
|---|---|
| Metal llama-server built from pinned tag | PASS: llama.cpp `b12414`, commit `278a89cb787c1d2b4cff9748b1df9cfd2aa69976` |
| Signed (Developer ID, hardened runtime, no entitlements) | PASS: `Developer ID Application: Joseph Cartlidge (MQ85968PQP)`, flags `0x10000(runtime)`, secure timestamp |
| Notarized | PASS: notarytool `Accepted` (two submissions, 32 s and 26 s wait) |
| `codesign --verify --deep --strict` | PASS (quarantined copy) |
| `spctl -a -t exec -vvv` | "rejected (the code is valid but does not seem to be an app)", `origin=Developer ID Application: Joseph Cartlidge`. This is the normal answer for a bare CLI Mach-O, not a Gatekeeper block (see "Gatekeeper on a bare binary") |
| `stapler validate` | Not applicable: tickets can only be stapled to .app/.pkg/.dmg. Bare binary reports "does not have a ticket stapled" |
| Run on Test MacBook | **not checked on Test MacBook** (Tailscale shows `joes-macbook-pro-2` offline, last seen 6 h ago; SSH timed out three times) |
| Same checks locally, quarantined copy | PASS, see below |
| Pack size | zip 9.7 MB (9,680,249 B); unpacked 23 MB (one 23.6 MB Mach-O + licence + manifest) |
| MLX sidecar proof | PASS: macOS executable loads `mlx-community/SmolLM2-135M-Instruct-8bit` and generates, also signed with hardened runtime and no entitlements |

### Local quarantined run (fallback for the offline Test MacBook)

Pack unzipped with `ditto -x -k`, then `xattr -w -r com.apple.quarantine "0083;<ts>;Safari;"` on the tree (curl does not set the attribute, so it was simulated).

```
$ codesign --verify --deep --strict --verbose=2 bin/llama-server
bin/llama-server: valid on disk
bin/llama-server: satisfies its Designated Requirement
$ bin/llama-server --version
version: 1.0.0-dev (build 1, commit 278a89c)
built with AppleClang 21.0.0.21000334 for Darwin arm64        (no Gatekeeper prompt, exit 0)
$ bin/llama-server --list-devices
  MTL0: Apple M1 Pro
  BLAS: Accelerate
$ bin/llama-server -m SmolLM2-135M-Instruct-Q8_0.gguf -ngl 99 --port 18777   (145 MB model)
GET /health -> {"status":"ok"}
POST /v1/chat/completions -> "Hello, welcome to our chat about the latest news and trends in the world of technology."
  19 tokens, 199 tok/s prompt, ~97 tok/s generation
```

Machine: this Mac (M1 Pro), not the 8 GB Test MacBook. The server was stopped afterwards; `pgrep llama-server` is empty. Nothing was started on the Test MacBook.

**To close on the Test MacBook** when it is back: `python3 -m http.server` on this Mac, `curl -O` over Tailscale there (add `xattr -w com.apple.quarantine` since curl does not), then the same four commands. Check `ps` first; other sessions use it.

### Gatekeeper on a bare binary

`spctl -t exec` assesses app bundles, so for a loose Mach-O it always says "does not seem to be an app". What matters is that launching the quarantined file did not prompt: the signature is Developer ID with a notarization ticket the system finds online (a ticket cannot be stapled to a bare file). Offline first launch of a quarantined unstapled binary would fall back to the signature only, which still passes for Developer ID code. The Desktop app downloading the pack with its own HTTP client does not set quarantine in the first place (only browsers and some apps do). If we ever want a staple, ship the pack inside a signed `.pkg` or `.dmg`; not worth it for a 10 MB sidecar.

## Pack layout

```
llamacpp-metal-macos-arm64/
  pack.json                # manifest (below)
  LICENSE-llama.cpp
  bin/llama-server         # static, Metal shaders embedded (GGML_METAL_EMBED_LIBRARY=ON)
```

Build choices (`scripts/engine-packs/build-llamacpp-macos.sh`):

- `-DBUILD_SHARED_LIBS=OFF -DGGML_METAL_EMBED_LIBRARY=ON`: one Mach-O. No `ggml-metal.metal`, `default.metallib` or `.dylib` to locate or sign. `otool -L` shows only system frameworks and libc++.
- `-DLLAMA_OPENSSL=OFF -DLLAMA_CURL=OFF`: the first build linked Homebrew `libssl.4.dylib`, which does not exist on a user's Mac. The server needs no TLS client; Allternit downloads models itself.
- `-DCMAKE_OSX_DEPLOYMENT_TARGET=13.0`, `-DGGML_NATIVE=OFF`, `arm64` only. Metal on Apple Silicon needs macOS 13+ at minimum. 13.0 is the declared floor and was **not tested on macOS 13** (built and run on macOS 27).
- Build time 1 m 25 s clean (M1 Pro) through `scripts/build-queue run engine-pack -- ...` (the queue takes any product name).

## Signing and notarization

Identity and credentials are the ones Desktop already uses (`build-local.cjs` picks the `Developer ID Application` cert from the login keychain; `notarize.cjs` uses notarytool keychain profile `allternit`). Nothing was printed or copied.

1. `codesign --force --timestamp --options runtime -s <Developer ID> -i com.allternit.engine.llama-server bin/llama-server` (0 s). **No entitlements.** A static llama-server needs no JIT, unsigned executable memory, library-validation opt-out, network entitlement (not sandboxed) or microphone. Desktop's `entitlements.mac.plist` is far wider (allow-jit, disable-library-validation, virtualization, screen capture) and must not be applied to engines.
2. Write `pack.json` after signing so the sha256 values are of the shipped bytes.
3. `ditto -c -k --keepParent` to a zip, `xcrun notarytool submit --keychain-profile allternit --wait`: Accepted in 26 to 32 s.
4. Verify with `codesign --verify --deep --strict` and `codesign -dvv` (Authority chain ends at Apple Root CA, TeamIdentifier MQ85968PQP).

Scripts: `scripts/engine-packs/build-pack-macos.sh` (whole pipeline), `build-llamacpp-macos.sh`, `sign-notarize.sh`, `make-manifest.py`. Whole pipeline wall time 49 s.

Signing identifiers matter: Keychain prompts on update come from identity changes (see `allternit-mac-signing`), so keep the `com.allternit.engine.*` identifiers and the same team on every release.

## Runtime package format (found; the plan listed it as not verified)

Source: `surfaces/allternit-desktop/src/main/runtime-package.ts` (client) and `surfaces/allternit-desktop/scripts/runtime-package.cjs` (pack and publish).

- Feed root `https://runtime.allternit.com` (R2 bucket `allternit-runtime`).
- `stable/<platform>/latest.json` plus `latest.json.sig`: a signed pointer. `FeedEntry` = `{ format: 2, version, build, shellApi, manifest, manifestSha256 }`. `<platform>` is Node's `process.platform-process.arch` (`darwin-arm64`, `linux-x64`, ...).
- `stable/<platform>/manifests/<version>.json` plus `.sig`: `RuntimeManifest` = `{ version, build, platform, shellApi, files: { "<relpath>": { sha256, size } } }`.
- `objects/<sha256>.gz`: every file once, gzip, content-addressed, shared across versions and platforms. The client copies files it already has and downloads only changed ones.
- Signatures are Ed25519 (private key `~/.config/allternit/runtime-signing-ed25519.pem`, public half `RUNTIME_PUBLIC_KEY` in `runtime-package.ts`). `build` is a monotonic number; higher wins. `shellApi` gates packs that need a newer shell. A new pack is staged, applied on restart as a trial, rolled back if the app does not start on it.
- On disk: `<userData>/runtime/versions/<version>/{manifest.json,manifest.sig,...files}` and `state.json`.

### Engine pack manifest (`pack.json`), a superset

```json
{
  "format": 1, "kind": "engine-pack",
  "pack_id": "llamacpp-metal-macos-arm64",
  "engine": "llamacpp", "backend": "metal",
  "version": "b12414", "build": 1794753442,
  "platform": "darwin-arm64", "shellApi": 1,
  "os": "macos", "arch": "arm64", "min_os": "13.0",
  "entrypoint": "bin/llama-server",
  "source_commit": "278a89cb787c1d2b4cff9748b1df9cfd2aa69976",
  "files": { "bin/llama-server": { "sha256": "...", "size": 23631504 }, "LICENSE-llama.cpp": { ... } }
}
```

`version`, `build`, `platform`, `shellApi`, `files` are exactly `RuntimeManifest`, so the existing verifier, delta stager and object store work unchanged. The extra fields (`pack_id`, `engine`, `backend`, `os`, `arch`, `min_os`, `entrypoint`, `source_commit`) are ignored by the current client and read by the future engine installer. Proposed feed path: `engines/<pack_id>/latest.json` with the same pointer and signing, so engine packs update independently of the Desktop runtime and a `stable/<platform>` bump does not re-download 24 MB. Not built: the manifest here is unsigned (signing and `publish` stay in `runtime-package.cjs`; publish was out of scope). Note a version string must match `[A-Za-z0-9._-]+`; `b12414` does.

W-lane follow-up: `local-engine` (`services/local-engine/src/runtime/manager.rs:220-227`) should resolve `llama-server` from `<runtime>/engines/<pack_id>/<entrypoint>` before PATH.

## MLX sidecar shape (decision)

**Decision: the macOS MLX engine is a Swift command-line executable `allternit-mlx-server`, built from the same Swift package as iOS (W7 `ios/Packages/OnDeviceLLM/`), shipped as an engine pack (`mlx-metal-macos-arm64`) with a small OpenAI-compatible HTTP server on loopback. Not a Python mlx-lm process.**

Verified facts:

1. **It builds and runs as a macOS executable.** A proof package (`mlx-swift` 0.30.6 + `mlx-swift-lm` 2.30.6, products `MLXLLM`, `MLXLMCommon`) built with `xcodebuild`, loaded `mlx-community/SmolLM2-135M-Instruct-8bit` and generated ("Hello!"). It also ran after signing with `--options runtime`, no entitlements: MLX's runtime kernel compile does not need the JIT entitlement.
2. **metallib.** `swift build` cannot compile Metal shaders (mlx-swift README says so). `xcodebuild` does, and puts `default.metallib` (3.8 MB) in `mlx-swift_Cmlx.bundle/Contents/Resources/` beside the executable (`Package.swift` sets `SWIFTPM_BUNDLE="mlx-swift_Cmlx"`, `METAL_PATH="default.metallib"`). The pack must ship the executable, `mlx-swift_Cmlx.bundle`, and the other SwiftPM bundles (`swift-transformers_Hub.bundle`, `swift-crypto_Crypto.bundle`) side by side. Build requirements: full Xcode and its Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`) on the CI Mac. CI cannot use plain `swift build`.
3. **Signing.** Sign each `.bundle` and the executable (Developer ID, hardened runtime, no entitlements), notarize the zip, same flow as llama. Bundles are resource-only, so sign them as bundles. Same pipeline as `build-pack-macos.sh` with a different build step.
4. **Version sharing.** iOS KokoroKit pins `mlx-swift` `exact: "0.30.2"` (`ios/Packages/KokoroKit/Package.swift:33`, `Package.resolved` agrees). No `mlx-swift-lm` release matches 0.30.2: 2.29.x needs 0.29.1+, **2.30.3 needs `upToNextMinor(from: 0.30.3)`**, 2.30.6 needs 0.30.6, 2.31.x needs 0.31.3, 3.32.3 needs 0.32.3. So both surfaces must move to the same patch of the 0.30 minor: **bump KokoroKit to `mlx-swift` 0.30.6 and use `mlx-swift-lm` 2.30.6** for iOS W7 and the Mac sidecar. 0.30.2 to 0.30.6 is a patch bump but the Kokoro TTS path (MisakiSwift, `kokoro-ios` commit pinned to 0.30.2) needs a regression listen on iOS before W7 lands; not done here. Newer lines (0.31, 0.32, mlx-swift-lm 3.x, Swift tools 6.2) are a separate upgrade for both at once.
5. **Server library.** Pick **Hummingbird 2** (`hummingbird-project/hummingbird`, Apache-2.0, swift-nio based, async/await, SSE streaming) for `POST /v1/chat/completions` (stream and non-stream), `GET /v1/models`, `GET /health`. Reasons: far smaller than Vapor (no Fluent, Leaf, ORM), no Foundation-networking quirks, built-in streaming response bodies for SSE. Raw swift-nio would save ~1 dependency and cost ~300 lines of HTTP parsing. Not built in this spike: only the load and generate path was proved. The shared package should split into `OnDeviceLLM` (engine, no HTTP, used by iOS) and a Mac-only `allternit-mlx-server` target (Hummingbird plus `OnDeviceLLM`).
6. **Model download.** mlx-swift-lm uses swift-transformers `Hub` to fetch from Hugging Face; the first run downloaded to `HF_HOME` (40 s for the 135 M 8-bit model). Desktop should instead download model files with its own manager into the model store and pass a local directory (`ModelConfiguration(directory:)`), so downloads, disk gates and resume stay in one place.
7. **Why not Python mlx-lm:** needs a bundled Python plus site-packages (hundreds of MB, thousands of files to sign, which already hurt Desktop signing, see the 26k-file note in `allternit-mac-signing`), and does not share code with iOS.

Cost of this path: `swift-transformers` and `Jinja` come with it. The proof's unstripped debug build was not size-measured; the release executable size is not measured. Expect roughly 10 to 25 MB plus the metallib (to be confirmed when W7 builds Release).

## What W6 should build: Windows and Linux packs (plan only, not checked on those OSes)

Same pack layout, same `pack.json`, `platform` = `win32-x64`, `win32-arm64`, `linux-x64`, `linux-arm64`. One pack per backend so a user downloads only what the hardware needs. `components` from C3 hardware detection chooses the pack.

| Pack | Build (llama.cpp tag `b12414`, same pin) | Notes |
|---|---|---|
| `llamacpp-cpu-win32-x64` | `-DGGML_NATIVE=OFF -DGGML_CPU_ALL_VARIANTS=ON -DGGML_BACKEND_DL=ON -DBUILD_SHARED_LIBS=ON` | Ships `llama-server.exe`, `ggml-base.dll`, per-ISA `ggml-cpu-*.dll` (AVX2, AVX512, ...) picked at runtime. Fallback for everyone |
| `llamacpp-vulkan-win32-x64` | `-DGGML_VULKAN=ON` (Vulkan SDK on the build host) | Works on NVIDIA, AMD, Intel with no vendor runtime. Best default GPU pack |
| `llamacpp-cuda-win32-x64` | `-DGGML_CUDA=ON`, CUDA 12.x, `CMAKE_CUDA_ARCHITECTURES` 75;80;86;89 (add 120 for Blackwell) | Ships `cudart64_12.dll`, `cublas64_12.dll`, `cublasLt64_12.dll` (about 400 MB with them; consider the NVIDIA redistributable license) or require the user's driver. Needs the driver (`nvcuda.dll`) already present |
| `llamacpp-cpu-win32-arm64` | MSVC arm64 target | Snapdragon X; Vulkan or OpenCL (Adreno) is a later, separate question |
| `llamacpp-cpu-linux-x64` and `-arm64` | same as Windows CPU, glibc 2.31 baseline (build in an Ubuntu 20.04 or manylinux-style container so cloud computers and old distros run it) | Cloud computers are Linux; this is the no-GPU default |
| `llamacpp-vulkan-linux-x64` | `-DGGML_VULKAN=ON` | Needs Mesa or vendor Vulkan loader; `libvulkan.so.1` is not shipped |
| `llamacpp-cuda-linux-x64` | `-DGGML_CUDA=ON`, CUDA 12.x | Do not bundle `libcuda.so` (comes from the driver); bundle `libcudart`, `libcublas` or document a CUDA runtime dep. Use RPATH `$ORIGIN` |
| `llamacpp-rocm-linux-x64` | `-DGGML_HIP=ON -DAMDGPU_TARGETS=...` (gfx1030, 1100, 1101, 1201, ...) | ROCm runtime large (GBs); treat as opt-in download, require the user's ROCm install, and expect narrow GPU support |

Signing: Windows needs Authenticode (EV or Azure Trusted Signing) on every `.exe` and `.dll`; unsigned `.dll` loaded by an unsigned-then-signed exe is fine technically, but SmartScreen reputation applies to the downloaded exe. Not part of this spike and no cert is known to exist. Linux packs are unsigned; integrity is the Ed25519 manifest plus sha256, same as the runtime packages. Existing runtime guidance applies: Linux must be built on a Linux host (the standby builder, see `allternit-runtime-packages`); no fresh cold builds per agent. All of this is **not checked** (no Windows or Linux build was attempted).

## Files in this PR

- `scripts/engine-packs/build-llamacpp-macos.sh`: pinned tag build.
- `scripts/engine-packs/sign-notarize.sh`: Developer ID signing of every Mach-O in a pack dir.
- `scripts/engine-packs/make-manifest.py`: writes `pack.json`.
- `scripts/engine-packs/build-pack-macos.sh`: build, sign, manifest, zip, notarize, verify.
- This document.

Build products live outside git (`~/s3-build-tmp` during the spike, cleaned afterwards).
