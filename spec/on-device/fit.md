# On-device fit spec

One spec for "will this model run on this device". The TypeScript port (`allternit-ai/src/lib/on-device/fit.ts`), the Swift port (`Core/OnDevice/`) and the Rust port (Gizzi / local-engine) all follow this file and pass `fit-vectors.json`. This file and `fit-vectors.json` are canonical. `allternit-ai/src/lib/on-device/fit-vectors.json` is a copy and must match it byte for byte.

## Function

```
fit(budget: DeviceBudget, quants: Quant[] | null, opts?: {ctx?: number}, stream_quants?: StreamQuant[] | null) -> FitResult
FitResult = { k: 'ok'|'slow'|'stream'|'no'|'na'|'unknown', label: string, why: string, quant: Quant | StreamQuant | null }
```

`opts.ctx` is accepted and ignored in v1 (`ram_est` is measured at the default context).

## Inputs

All sizes are bytes. 1 GB = 1,000,000,000 bytes.

**DeviceBudget**

| Field | Meaning |
|---|---|
| `kind` | `discrete`, `unified` or `cpu` |
| `fast_bytes` | discrete: VRAM. unified and cpu: the usable model budget, already scaled (see below) |
| `spill_bytes` | discrete: system RAM. unified and cpu: 0 |
| `apple` | optional bool. Apple silicon (Mac or iPhone). Prefer MLX |
| `browser` | optional bool. A browser tab. Only webgpu builds run |
| `webgpu` | optional bool, browser only. `false` = no WebGPU adapter. Default true |
| `disk_free_bytes` | optional integer, bytes. Free space on the volume that holds the models folder. Only a host that can run a colibri pack sets it. See "Stream (colibri)" |

`kind`, `fast_bytes` and `spill_bytes` are the `budget` object of local-engine `GET /hardware`. The other flags come from the client.

How `fast_bytes` is built (done by the detector, not by `fit`):

| Device | `kind` | `fast_bytes` |
|---|---|---|
| Discrete GPU (NVIDIA, AMD, Intel Arc) | discrete | VRAM; `spill_bytes` = system RAM |
| Apple silicon Mac | unified | 0.65 x RAM |
| Other unified memory (Arm Windows, iGPU) | unified | 0.60 x RAM |
| No GPU | cpu | 0.6 x RAM |
| iPhone | unified, apple | 0.75 x `os_proc_available_memory()` at cold start |
| Android | unified | min(availMem - threshold, 0.45 x totalMem) |
| Browser | unified, browser | min(WebGPU buffer limits, 2 GB); 0.45 GB on phone browsers |

**Quant** (one entry of `quants` in device.json): `name`, `format` (`gguf`|`mlx`|`onnx`), `engine` (`llama.cpp`|`mlx`|`webgpu`), `size`, optional `ram_est`, plus `files`, `best_for`, `repo` that `fit` does not read. Engine decides the pick; format is informational.

`quants = null` means the model has no device.json entry.

## Need

`need = ram_est` when present. Otherwise, by the quant's `engine` (float64 arithmetic):

- `llama.cpp` and `mlx`: `size x 1.2 + 0.4 GB`.
- `webgpu`: `size x 1.2`, with no 0.4 GB.

Why: the 0.4 GB is native process and runtime overhead. A browser budget is a GPU buffer cap, so that overhead is not counted against it. With the overhead, nothing could ever fit the 0.45 GB phone-browser cap ("Fits" needs at most 0.36 GB), yet LFM2.5-350M (255 MB) ran on iPhone Safari WebGPU at 40+ tok/s in an earlier spike.

## Pick

1. No entry (`quants` null) -> `unknown`.
2. Browser with `webgpu === false` -> `na` (No WebGPU), whatever the quants.
3. Candidates: browser -> engine `webgpu` only. Apple -> engine `mlx`; if there is none, engine `llama.cpp`. Everything else -> engine `llama.cpp`.
4. No candidates (including an empty list) -> `na`.
5. Sort candidates by need ascending. Take the largest whose verdict is `ok`; if none is `ok`, take the smallest.
6. The result uses that quant's verdict, label and `why`, and returns it as `quant`.

## Verdict per kind

Use integer comparisons as written so ports agree at the boundaries.

| Kind | `ok` (label "Fits") | `slow` | `no` (label "Won't fit") |
|---|---|---|---|
| discrete | `need*10 <= fast*9` | `need*2 <= fast*2 + spill`, label "May be slow" | above that |
| unified | `need*5 <= fast*4` | `need <= fast`, label "Tight fit" | `need > fast` |
| cpu | `need <= fast` and `size < 1.5 GB` | `need <= fast` and `size >= 1.5 GB`, label "May be slow" | `need > fast` |

iPhone, Android and browser are `unified` rows; the thresholds are the same, only `fast_bytes` differs.

## Stream (colibri)

Some models are larger than memory but run through colibri, which keeps part of the weights resident and reads the rest from disk. This adds a fourth verdict, `stream`. The pick above is unchanged. The stream step runs after it.

**StreamQuant** (one entry of `stream_quants` in device.json, a sibling of `quants`): `name`, `format` (`colibri`|`gguf`, informational), `engine` (always `colibri`), `size` (download bytes, must be free on disk), `ram_est` (required, integer bytes of resident weights, not file size), `tier` (`chat`|`background`), plus `colibri_model`, `repo`, optional `metal_ok`, optional `best_for` that `fit` does not read. There is no `size x 1.2 + 0.4` fallback for `colibri`. Old clients ignore the key. A `colibri` entry never goes inside `quants`, because shipped iOS builds decode `engine` as a strict enum.

**Budget.** `disk_free_bytes` is set only on Mac arm64, Windows x64, Linux x64 and Linux arm64. If it is missing, no stream verdict is ever returned. Phones, browsers, Windows Arm and Intel Macs never see `stream` for themselves. They see it for another device whose budget has the field.

**When it runs.** All of these must hold:

1. The budget is not a browser (`browser` is not true).
2. `disk_free_bytes` is present.
3. The in-memory result is `no`, or it is `na` because there were no candidates (an empty list counts). `ok`, `slow` and `unknown` are never replaced. `quants = null` with a non-empty `stream_quants` is treated as `quants = []`. `unknown` is returned only when both are null or empty.

**A StreamQuant qualifies** when `engine == 'colibri'`, `ram_est <= stream_ram` and `size <= disk_free_bytes`. `stream_ram` uses integer compares:

| Budget kind | Test |
|---|---|
| unified, cpu | `ram_est <= fast_bytes` |
| discrete | `ram_est * 10 <= spill_bytes * 6` (0.6 x system RAM; VRAM is not used) |

If at least one qualifies, the result is `stream` and `quant` is the chosen StreamQuant. If none qualifies, the original result (`no` with its smallest quant, or `na`) is returned unchanged.

**Sort order.** Verdict order, best first: `ok`, `slow`, `stream`, `no`. Among qualifying stream quants: `tier == 'chat'` before `tier == 'background'`, then larger `size` first, then `name` ascending (byte order). Take the first.

**Strings (exact).**

| Field | Value |
|---|---|
| k | `stream` |
| label | `Streams from disk` |
| why, chat tier | `Keeps N GB in memory, reads the rest from disk` |
| why, background tier | `Slow. Best for jobs you leave running` |

`N` = `ram_est` in GB, one decimal, always shown (`10.0`, `9.6`), rounded half away from zero (same rule as the other `N`). No speed number appears in any string until one is measured. The client shows the disk need from `quant.size` (for example "23.1 GB on disk"); it is not part of `why`.

## Output strings

Labels are exact (ASCII apostrophe in "Won't fit"):

| k | label |
|---|---|
| ok | `Fits` |
| slow, discrete or cpu | `May be slow` |
| slow, unified | `Tight fit` |
| stream | `Streams from disk` |
| no | `Won't fit` |
| na, browser without WebGPU | `No WebGPU` |
| na, browser, no webgpu build | `No browser build` |
| na, other | `No build for this device` |
| unknown | `Size not checked` |

`why` (exact text). `N` = need in GB with one decimal, always shown (`3.6`, `10.0`), rounded half away from zero. `B` = `fast_bytes` in GB, one decimal, trailing `.0` dropped (`12`, `4.5`).

| Case | `why` |
|---|---|
| ok, discrete | `Full GPU, N of B GB VRAM` |
| ok, cpu | `CPU only` |
| ok, unified | `N of B GB for models` |
| slow, discrete | `Part runs on the CPU` |
| slow, cpu | `CPU only, about 5 tok/s` |
| slow, unified | `Close other apps first` |
| no, discrete or cpu | `Needs about N GB` |
| no, unified | `Needs N GB, B GB free for models` |
| na, No WebGPU | `This browser cannot run models. Use Desktop or the phone app` |
| na, No browser build | `Run it in Desktop or the phone app` |
| na, other | `Pick another model or device` |
| unknown | `No device data for this model yet` |
| stream, chat tier | `Keeps N GB in memory, reads the rest from disk` |
| stream, background tier | `Slow. Best for jobs you leave running` |

`quant` is the chosen Quant for `ok`, `slow` and `no` (a StreamQuant for `stream`), and null for `na` and `unknown`. `fit-vectors.json` checks `k`, `label` and the chosen quant `name` (null when `quant` is null). `why` is not in the vectors.

## Vectors

`fit-vectors.json` is an array of `{name, budget, quants, stream_quants?, opts?, expect:{k, label, quant}}` where `expect.quant` is the quant name or null. Profiles come from `mockups/data.js` (Mac 31 GB, Windows NVIDIA 12 GB VRAM + 32 GB RAM, Windows Arm 19 GB, Linux AMD 24 GB VRAM + 64 GB RAM, Linux CPU 9.6 GB, browser 2 GB, phone browser 0.45 GB, iPhone 4.5 GB, Android 5 GB) with realistic quants, and each threshold above is tested exactly at, and one byte past, its edge. The last 15 vectors cover the stream step: the ok and slow verdicts that win over it, the disk and RAM edges (unified and discrete), cpu, a phone with no disk field, empty and null `quants`, a browser, and the tier sort. A port is done when it passes every vector (80).

## Differences from the mockup `data.js`

The plan table wins where they differ.

- The mockup always derives need from `size x 1.2 + 0.4`; the spec uses `ram_est` first, and drops the 0.4 GB for webgpu quants.
- The mockup picks the first matching build; the spec picks the largest `ok` quant, else the smallest.
- The mockup has no `unknown`, and one na text; the spec has four na/unknown labels.
- The mockup treats iPhone, Android and browser as separate kinds; here they are `unified` with a flag.
- The mockup budgets are rounded GB (Mac 31 for 0.65 x 48 = 31.2, Arm 19 for 19.2, browser 2); the vectors use the mockup numbers.
- The mockup's na text for a missing browser build matches ("No browser build", "Run it in Desktop or the phone app").
