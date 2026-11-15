// Gizzi's copy of the one on-device fit function. Canonical spec and vectors:
// spec/on-device/fit.md and spec/on-device/fit-vectors.json (deviceFit.test.ts runs all of them).
// Same logic as allternit-ai src/lib/on-device/fit.ts; keep the two in step with the vectors.
import { getHardware, getStatus, type HardwareResponse, type StatusResponse } from "./localEngine"

export type DeviceBudgetKind = "discrete" | "unified" | "cpu"
export interface DeviceBudget {
  kind: DeviceBudgetKind
  fast_bytes: number
  spill_bytes: number
  apple?: boolean
  browser?: boolean
  webgpu?: boolean
}
export type QuantEngine = "llama.cpp" | "mlx" | "webgpu"
export interface Quant {
  name: string
  format: "gguf" | "mlx" | "onnx"
  engine: QuantEngine
  files?: string[]
  size: number
  ram_est?: number
  best_for?: string[]
  /** Other Freeweights id that holds this build. */
  repo?: string
}
export type FitKind = "ok" | "slow" | "no" | "na" | "unknown"
export interface FitResult {
  k: FitKind
  label: string
  why: string
  quant: Quant | null
}

const GB = 1e9

/**
 * Memory a quant needs: ram_est, else size x 1.2 + 0.4 GB for native engines.
 * webgpu quants skip the 0.4 GB: it is native process overhead, and the browser
 * budget is a GPU buffer cap.
 */
export function needBytes(q: Quant): number {
  if (q.ram_est != null) return q.ram_est;
  return q.engine === "webgpu" ? q.size * 1.2 : q.size * 1.2 + 0.4 * GB;
}

/**
 * Budget for a computer from what it reports. Apple unified memory 0.65 x RAM,
 * other unified or CPU-only 0.60 x RAM; a discrete GPU is its VRAM with RAM as spill.
 * Mirrors engine `GET /hardware` (C3), used when an older engine has only `/status`.
 */
export function budgetFromHardware(h: { ramBytes: number; apple?: boolean; gpuBytes?: number }): DeviceBudget {
  if (h.apple) return { kind: "unified", fast_bytes: Math.floor(h.ramBytes * 0.65), spill_bytes: 0, apple: true };
  if (h.gpuBytes && h.gpuBytes > 0) return { kind: "discrete", fast_bytes: h.gpuBytes, spill_bytes: h.ramBytes };
  return { kind: "cpu", fast_bytes: Math.floor(h.ramBytes * 0.6), spill_bytes: 0 };
}

/** Bytes to GB, one decimal, ".0" dropped (used in `why` text). */
function gb(bytes: number): string {
  return (Math.round((bytes / GB) * 10) / 10).toString();
}
function gbFixed(bytes: number): string {
  return (Math.round((bytes / GB) * 10) / 10).toFixed(1);
}

type Verdict = "ok" | "slow" | "no";

function verdictOf(d: DeviceBudget, q: Quant): Verdict {
  const need = needBytes(q);
  if (d.kind === "discrete") {
    if (need * 10 <= d.fast_bytes * 9) return "ok";
    if (need * 2 <= d.fast_bytes * 2 + d.spill_bytes) return "slow";
    return "no";
  }
  if (d.kind === "cpu") {
    if (need > d.fast_bytes) return "no";
    return q.size < 1.5 * GB ? "ok" : "slow";
  }
  if (need * 5 <= d.fast_bytes * 4) return "ok";
  if (need <= d.fast_bytes) return "slow";
  return "no";
}

/** Quants this device can run at all: webgpu for browsers, MLX else GGUF on Apple, GGUF elsewhere. */
export function candidates(d: DeviceBudget, quants: Quant[]): Quant[] {
  if (d.browser) return quants.filter((q) => q.engine === "webgpu");
  const gguf = quants.filter((q) => q.engine === "llama.cpp");
  if (d.apple) {
    const mlx = quants.filter((q) => q.engine === "mlx");
    return mlx.length > 0 ? mlx : gguf;
  }
  return gguf;
}

function naResult(d: DeviceBudget): FitResult {
  if (d.browser && d.webgpu === false) {
    return {
      k: "na",
      label: "No WebGPU",
      why: "This browser cannot run models. Use Desktop or the phone app",
      quant: null,
    };
  }
  if (d.browser) {
    return {
      k: "na",
      label: "No browser build",
      why: "Run it in Desktop or the phone app",
      quant: null,
    };
  }
  return {
    k: "na",
    label: "No build for this device",
    why: "Pick another model or device",
    quant: null,
  };
}

export function fit(
  budget: DeviceBudget,
  quants: Quant[] | null | undefined,
  _opts?: { ctx?: number },
): FitResult {
  if (quants == null) {
    return {
      k: "unknown",
      label: "Size not checked",
      why: "No device data for this model yet",
      quant: null,
    };
  }
  if (budget.browser && budget.webgpu === false) return naResult(budget);
  const cands = candidates(budget, quants);
  if (cands.length === 0) return naResult(budget);

  const bySize = [...cands].sort((a, b) => needBytes(a) - needBytes(b));
  let pick = bySize[0];
  for (let i = bySize.length - 1; i >= 0; i--) {
    if (verdictOf(budget, bySize[i]) === "ok") {
      pick = bySize[i];
      break;
    }
  }

  const v = verdictOf(budget, pick);
  const need = gbFixed(needBytes(pick));
  const unifiedLike = budget.kind === "unified";
  const b = gb(budget.fast_bytes);
  if (v === "ok") {
    const why =
      budget.kind === "discrete"
        ? `Full GPU, ${need} of ${b} GB VRAM`
        : budget.kind === "cpu"
          ? "CPU only"
          : `${need} of ${b} GB for models`;
    return { k: "ok", label: "Fits", why, quant: pick };
  }
  if (v === "slow") {
    if (budget.kind === "discrete") {
      return { k: "slow", label: "May be slow", why: "Part runs on the CPU", quant: pick };
    }
    if (budget.kind === "cpu") {
      return { k: "slow", label: "May be slow", why: "CPU only, about 5 tok/s", quant: pick };
    }
    return { k: "slow", label: "Tight fit", why: "Close other apps first", quant: pick };
  }
  return {
    k: "no",
    label: "Won't fit",
    why: unifiedLike ? `Needs ${need} GB, ${b} GB free for models` : `Needs about ${need} GB`,
    quant: pick,
  };
}

// ---- Device data and hardware ----

const FREEWEIGHTS = (process.env.FREEWEIGHTS_URL?.trim().replace(/\/$/, "")) || "https://freeweights.allternit.com"

export interface DeviceEntry {
  id: string
  quants: Quant[]
  device_formats: string[]
  generated_at?: string
}

/** Device data for a model; null on 404 no_device_data. Throws on network or server errors. */
export async function fetchDeviceEntry(id: string): Promise<DeviceEntry | null> {
  const url = `${FREEWEIGHTS}/api/models/${id.split("/").map(encodeURIComponent).join("/")}/device`
  const res = await fetch(url, { signal: AbortSignal.timeout(10_000) })
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`Freeweights ${res.status} for ${id}`)
  return (await res.json()) as DeviceEntry
}

export interface LocalDevice {
  name: string
  budget: DeviceBudget
  source: "hardware" | "status"
  hardware?: HardwareResponse
}

/** Budget from /status for engines that predate /hardware (same rules as C3). */
export function budgetFromStatus(s: StatusResponse): DeviceBudget {
  const ram = s.ram.total_bytes
  if (s.apple_chip || s.unified_memory) return budgetFromHardware({ ramBytes: ram, apple: Boolean(s.apple_chip) })
  const vram = Math.max(0, ...(s.gpu ?? []).map((g) => (g.memory_total_mb ?? 0) * 1048576))
  return budgetFromHardware({ ramBytes: ram, gpuBytes: vram })
}

/** Reads this computer from the Local Engine: /hardware, else /status. Throws if the engine is down. */
export async function readLocalDevice(): Promise<LocalDevice> {
  const hw = await getHardware()
  if (hw) {
    const apple = Boolean(hw.apple_chip) || hw.gpus.some((g) => g.vendor === "apple")
    const budget: DeviceBudget = { ...hw.budget }
    if (apple) budget.apple = true
    return { name: hw.apple_chip ? `Apple ${hw.apple_chip}` : hw.cpu_model, budget, source: "hardware", hardware: hw }
  }
  const st = await getStatus()
  const budget = budgetFromStatus(st)
  return { name: st.apple_chip ? `Apple ${st.apple_chip}` : st.cpu.model, budget, source: "status" }
}

/** "4.2 GB" / "380 MB". */
export function fmtBytes(n: number): string {
  return n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : `${Math.round(n / 1e6)} MB`
}
