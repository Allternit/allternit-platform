import { describe, expect, test } from "bun:test"
import { readFileSync } from "fs"
import { join } from "path"
import { budgetFromStatus, fit, type DeviceBudget, type Quant, type StreamQuant } from "./deviceFit"

// Canonical vectors from spec/on-device/fit-vectors.json (repo root).
const vectors = JSON.parse(
  readFileSync(join(import.meta.dir, "../../../../../spec/on-device/fit-vectors.json"), "utf8"),
) as Array<{ name: string; budget: DeviceBudget; quants: Quant[] | null; stream_quants?: StreamQuant[]; opts?: { ctx?: number }; expect: { k: string; label: string; quant: string | null } }>

describe("on-device fit vectors", () => {
  test("has the full vector set", () => expect(vectors.length).toBe(80))
  for (const v of vectors) {
    test(v.name, () => {
      const r = fit(v.budget, v.quants, v.opts, v.stream_quants)
      expect({ k: r.k, label: r.label, quant: r.quant?.name ?? null }).toEqual(v.expect as any)
    })
  }
})

describe("budgetFromStatus (older engines without /hardware)", () => {
  const base = { status: "healthy", active_runtimes: 0, cached_models: 0, hardware_id: "x", platform: { os: "", arch: "" }, cpu: { model: "", cores: 1, threads: 1 }, ram: { total_bytes: 32e9, used_bytes: 0, total_mb: 0, used_mb: 0 } }
  test("apple = 0.65 of RAM, unified", () => {
    const b = budgetFromStatus({ ...base, apple_chip: "M1 Pro", unified_memory: true, backends: { metal: true, cuda: false, cpu_fallback: true } } as any)
    expect(b).toMatchObject({ kind: "unified", fast_bytes: 20.8e9, apple: true })
  })
  test("discrete GPU = VRAM with RAM as spill", () => {
    const b = budgetFromStatus({ ...base, unified_memory: false, gpu: [{ name: "RTX", memory_total_mb: 8192 }], backends: { metal: false, cuda: true, cpu_fallback: true } } as any)
    expect(b).toMatchObject({ kind: "discrete", fast_bytes: 8192 * 1048576, spill_bytes: 32e9 })
  })
  test("no GPU = cpu at 0.6 of RAM", () => {
    const b = budgetFromStatus({ ...base, unified_memory: false, backends: { metal: false, cuda: false, cpu_fallback: true } } as any)
    expect(b).toMatchObject({ kind: "cpu", fast_bytes: 19.2e9 })
  })
})
