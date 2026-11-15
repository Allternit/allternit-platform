# On-device fit spec

One spec for "will this model run on this device". The TypeScript port (`allternit-ai/src/lib/on-device/fit.ts`), the Swift port (`Core/OnDevice/`) and the Rust port (Gizzi / local-engine) all follow this file and pass `fit-vectors.json`. This file and `fit-vectors.json` are canonical. `allternit-ai/src/lib/on-device/fit-vectors.json` is a copy and must match it byte for byte.

## Function

```
fit(budget: DeviceBudget, quants: Quant[] | null, opts?: {ctx?: number}) -> FitResult
FitResult = { k: 'ok'|'slow'|'no'|'na'|'unknown', label: string, why: string, quant: Quant | null }
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

`kind`, `fast_bytes` and `spill_bytes` are the `budget` object of local-engine `GET /hardware`. The other three flags come from the client.

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

`need = ram_est`, or `size x 1.2 + 0.4 GB` when `ram_est` is absent (float64 arithmetic).

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

## Output strings

Labels are exact (ASCII apostrophe in "Won't fit"):

| k | label |
|---|---|
| ok | `Fits` |
| slow, discrete or cpu | `May be slow` |
| slow, unified | `Tight fit` |
| no | `Won't fit` |
| na, browser without WebGPU | `No WebGPU` |
| na, browser, no webgpu build | `No browser build` |
| na, other | `No build for this device` |
| unknown | `Not checked` |

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

`quant` is the chosen Quant for `ok`, `slow` and `no`, and null for `na` and `unknown`. `fit-vectors.json` checks `k`, `label` and the chosen quant `name` (null when `quant` is null). `why` is not in the vectors.

## Vectors

`fit-vectors.json` is an array of `{name, budget, quants, opts?, expect:{k, label, quant}}` where `expect.quant` is the quant name or null. Profiles come from `mockups/data.js` (Mac 31 GB, Windows NVIDIA 12 GB VRAM + 32 GB RAM, Windows Arm 19 GB, Linux AMD 24 GB VRAM + 64 GB RAM, Linux CPU 9.6 GB, browser 2 GB, phone browser 0.45 GB, iPhone 4.5 GB, Android 5 GB) with realistic quants, and each threshold above is tested exactly at, and one byte past, its edge. A port is done when it passes every vector.

## Differences from the mockup `data.js`

The plan table wins where they differ.

- The mockup always derives need from `size x 1.2 + 0.4`; the spec uses `ram_est` first.
- The mockup picks the first matching build; the spec picks the largest `ok` quant, else the smallest.
- The mockup has no `unknown`, and one na text; the spec has four na/unknown labels.
- The mockup treats iPhone, Android and browser as separate kinds; here they are `unified` with a flag.
- The mockup budgets are rounded GB (Mac 31 for 0.65 x 48 = 31.2, Arm 19 for 19.2, browser 2); the vectors use the mockup numbers.
- The mockup's na text for a missing browser build matches ("No browser build", "Run it in Desktop or the phone app").
