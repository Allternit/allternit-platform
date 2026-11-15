# Spike S2: iOS memory limit (os_proc_available_memory)

Plan: PLAN-on-device-models-2026-11-15.md, S2. Run 2026-11-15.

## Device
- iPhone 15 (iPhone15,4), physical, iOS 27.2
- ProcessInfo.physicalMemory = 5688 MB (6 GB class)
- Not checked on iPhone 17 Pro (device unavailable).

## Numbers (cold start, foreground, Debug build, no debugger attached)

| Variant | Entitlements | os_proc_available_memory() | Step test (256 MB steps, touched pages) |
|---|---|---|---|
| A | none | 3063 MB (3,212,441,976 B) | reached 2560 MB, no jetsam; stopped at avail - 300 |
| B | increased-memory-limit | 4088 MB (4,287,117,688 B) | reached 3584 MB, no jetsam; stopped at avail - 300 |
| C | increased-memory-limit + extended-virtual-addressing | 4088 MB (4,287,117,664 B) | reached 3584 MB, no jetsam |

- Entitlement gain: +1025 MB (+33%). Extended-virtual-addressing does not change the number (it widens address space, not the memory limit).
- The number is real: in every step, available + footprint stayed about equal to the cold number (A: 2801+270 at 256 MB, 495+2576 at 2560 MB; B: 494+3601 at 3584 MB). Allocations were malloc'd and every page written with non-zero data. The limit itself was not crossed on purpose, so no crash test was run.
- A cold app has footprint of about 7 MB, so the cold number is the practical ceiling for model weights + KV + app overhead.

## Method
Standalone probe app (SwiftUI, own App IDs, team MQ85968PQP, automatic signing, Debug), three targets from one project (`spikes/ios-memprobe/project.yml`, XcodeGen): `com.allternit.spike.memprobe`, `.inc`, `.inceva`. Built and installed in one session with xcodebuild and devicectl; numbers read from stdout via `devicectl device process launch --console`. The value is read in the Probe initializer at app start, before any allocation. Raw logs: `spikes/ios-memprobe/out/`.

Automatic signing registered both entitlements on the new throwaway dev App IDs without any portal action. The production Allternit App ID and iOS project were not touched. Production still needs this entitlement enabled on its own App ID (Eoj decision) and App Review notes for it.

## Phone budget (0.75 x number, per fit-table rule)

| Variant | Budget |
|---|---|
| Without entitlement | 0.75 x 3063 = 2297 MB (about 2.24 GB) |
| With increased-memory-limit | 0.75 x 4088 = 3066 MB (about 2.99 GB) |

## What fits
- Qwen3.5-4B Q4 (about 2.7 GB weights) + KV: does NOT fit without the entitlement (2.7 GB alone exceeds 2.24 GB). With the entitlement the budget is 2.99 GB, so weights fit but only about 0.3 GB is left for KV cache, activations and app; practically too tight at any real context length. Treat as not admitted on a 6 GB iPhone; marginal at best with a very short context.
- Without entitlement: models up to about 1.5 to 1.8 GB total fit (e.g. 1.5B to 2B class Q4, 1 to 1.5 GB, plus KV).
- With entitlement: up to about 2.2 to 2.5 GB weights leave workable KV (e.g. 3B Q4 around 2 GB; 4B at Q3 or smaller only with short context).
- 8 GB iPhones (15 Pro and later) should be measured separately; this result covers only a 6 GB device. The 0.75 factor held up: the real ceiling is the cold number, and the factor leaves headroom for page-cache and OS pressure that this foreground-only test does not exercise.
