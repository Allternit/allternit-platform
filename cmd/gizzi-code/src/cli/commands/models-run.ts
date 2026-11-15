// gizzi models fit | run | ps | stop: run open-weights models on this computer through the Local Engine.
// Fit uses the one shared rule (runtime/services/deviceFit.ts) and Freeweights device data.
import { createInterface } from "readline"
import { join } from "path"
import { UI } from "@/cli/ui"
import {
  LocalEngineError,
  ensurePacks,
  getHardware,
  getPacks,
  launchRuntime,
  listModels,
  listRuntimes,
  stopRuntime,
  streamChat,
  type CachedModel,
  type RuntimeInstance,
} from "@/runtime/services/localEngine"
import { fetchDeviceEntry, fit, fmtBytes, needBytes, readLocalDevice, type DeviceEntry, type FitResult, type LocalDevice, type Quant } from "@/runtime/services/deviceFit"
import { pullModel } from "@/cli/commands/models-pull"

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))
const style = { ok: UI.Style.TEXT_SUCCESS_BOLD, warn: UI.Style.TEXT_WARNING_BOLD, dim: UI.Style.TEXT_DIM, off: UI.Style.TEXT_NORMAL }

function row(label: string, value: string) {
  UI.println(`  ${style.dim}${label.padEnd(10)}${style.off}${value}`)
}

function deviceLine(d: LocalDevice): string {
  const hw = d.hardware
  const ram = hw ? `${Math.round(hw.ram_total_bytes / 1073741824)} GB` : ""
  return `${d.name}${ram ? ` · ${ram}` : ""} · ${fmtBytes(d.budget.fast_bytes)} for models (${d.budget.kind})`
}

function buildLine(q: Quant, id: string): string {
  const repo = q.repo && q.repo !== id ? ` · from ${q.repo}` : ""
  return `${q.name} · ${q.format} · ${q.engine} · ${fmtBytes(q.size)} file, about ${fmtBytes(needBytes(q))} in memory${repo}`
}

/** Reads the engine and Freeweights; prints a friendly message and returns null on failure. */
async function load(id: string): Promise<{ device: LocalDevice; entry: DeviceEntry | null } | null> {
  let device: LocalDevice
  try {
    device = await readLocalDevice()
  } catch (err) {
    UI.error(`Cannot reach the Local Engine (${err instanceof Error ? err.message : err}). Open Allternit Desktop or start the engine, then try again.`)
    return null
  }
  let entry: DeviceEntry | null
  try {
    entry = await fetchDeviceEntry(id)
  } catch (err) {
    UI.error(`Cannot read device data for ${id} from Freeweights: ${err instanceof Error ? err.message : err}`)
    return null
  }
  return { device, entry }
}

function validId(id: string | undefined, usage: string): id is string {
  if (!id || !id.includes("/")) {
    UI.error(`Usage: ${usage}`)
    return false
  }
  return true
}

/** gizzi models fit <id> [--json] */
export async function fitModel(id: string | undefined, json: boolean): Promise<number> {
  if (!validId(id, "gizzi models fit <org/model> [--json]")) return 1
  const loaded = await load(id)
  if (!loaded) return 1
  const { device, entry } = loaded
  const r = fit(device.budget, entry?.quants ?? null)
  if (json) {
    process.stdout.write(JSON.stringify({ id, device: { name: device.name, budget: device.budget, source: device.source }, ...r }, null, 2) + "\n")
  } else {
    UI.println(UI.Style.TEXT_HIGHLIGHT_BOLD + id + UI.Style.TEXT_NORMAL)
    row("Computer", deviceLine(device))
    row("Fit", (r.k === "ok" ? style.ok : r.k === "unknown" ? style.dim : style.warn) + r.label + style.off)
    if (r.quant) row("Build", buildLine(r.quant, id))
    row("Why", r.why)
    if (device.source === "status") row("Note", "This engine has no /hardware yet; the budget was worked out from /status.")
  }
  return r.k === "ok" || r.k === "slow" ? 0 : 1
}

async function waitFor<T>(what: string, fn: () => Promise<T | null>, timeoutMs: number): Promise<T | null> {
  const end = Date.now() + timeoutMs
  let last = ""
  while (Date.now() < end) {
    const v = await fn()
    if (v) return v
    process.stderr.write(`\r${what}...   `)
    last = what
    await sleep(1000)
  }
  if (last) process.stderr.write("\n")
  return null
}

/** Makes sure the engine for this build is ready. Returns an error string, or null when ready. */
async function ensureEngine(engineId: "llama.cpp" | "mlx"): Promise<string | null> {
  const state = async () => (await getHardware())?.engines.find((e) => e.id === engineId)?.state
  const s0 = await state()
  if (s0 === "ready" || s0 === "installed" || s0 === "fallback") return null
  if (s0 === "off") return `The ${engineId} engine does not apply on this computer.`
  const packKey = engineId === "mlx" ? "mlx" : "llamacpp"
  let packs
  try {
    packs = await ensurePacks([packKey])
  } catch (err) {
    return `Could not start the ${engineId} engine install: ${err instanceof Error ? err.message : err}`
  }
  if (!packs) return `The ${engineId} engine is ${s0 ?? "not available"} and this Local Engine cannot install engine packs yet. Update Allternit Desktop.`
  const done = await waitFor(`Installing the ${engineId} engine`, async () => {
    const p = await getPacks()
    const jobs = p?.jobs.filter((j) => j.pack_id.startsWith(engineId === "mlx" ? "mlx" : "llamacpp")) ?? []
    if (jobs.some((j) => j.state === "failed")) return { failed: jobs.find((j) => j.state === "failed")!.error ?? "failed" }
    const s = await state()
    return s === "ready" || s === "installed" ? { ok: true } : null
  }, 5 * 60_000)
  process.stderr.write("\n")
  if (!done) return `The ${engineId} engine install did not finish in 5 minutes.`
  if ("failed" in done) return `The ${engineId} engine install failed: ${done.failed}`
  return null
}

function findCached(models: CachedModel[], repo: string): CachedModel | undefined {
  return models.find((m) => m.name === repo && m.status === "ready") ?? models.find((m) => m.id === repo && m.status === "ready")
}

function modelPath(m: CachedModel, q: Quant): string | null {
  if (!m.path) return null
  if (q.engine === "mlx") return m.path
  if (m.path.toLowerCase().endsWith(".gguf")) return m.path
  const want = q.files?.[0]
  const f = want ? (m.files ?? []).find((x) => x.path === want || x.path.endsWith("/" + want)) : (m.files ?? []).find((x) => x.path.toLowerCase().endsWith(".gguf"))
  const rel = f?.path ?? want
  return rel ? join(m.path, rel) : null
}

async function printTurnNumbers(u: { tokPerSec?: number; firstTokenMs?: number; memBytes?: number; completionTokens?: number }) {
  const bits = [
    u.tokPerSec != null ? `${u.tokPerSec.toFixed(1)} tok/s` : null,
    u.firstTokenMs != null ? `first token ${u.firstTokenMs} ms` : null,
    u.completionTokens != null ? `${u.completionTokens} tokens` : null,
    u.memBytes != null ? `${fmtBytes(u.memBytes)} memory` : null,
  ].filter(Boolean)
  if (bits.length) UI.println(`${style.dim}${bits.join(" · ")}${style.off}`)
}

async function turn(id: string, history: Array<{ role: "user" | "assistant"; content: string }>, thinking = true): Promise<void> {
  let reply = ""
  let usage: Parameters<typeof printTurnNumbers>[0] | undefined
  for await (const ev of streamChat(id, history, undefined, { thinking })) {
    if ("token" in ev) {
      reply += ev.token
      process.stdout.write(ev.token)
    } else usage = ev.usage
  }
  process.stdout.write("\n")
  history.push({ role: "assistant", content: reply })
  if (usage) await printTurnNumbers(usage)
}

/** gizzi models run <id> [--quant Q] [--prompt "text"] */
export async function runModel(id: string | undefined, opts: { quant?: string; prompt?: string; allowUnverified?: boolean }): Promise<number> {
  if (!validId(id, "gizzi models run <org/model> [--quant Q4_K_M] [--prompt \"text\"]")) return 1
  const loaded = await load(id)
  if (!loaded) return 1
  const { device, entry } = loaded
  let quants = entry?.quants ?? null
  if (opts.quant && quants) {
    const only = quants.filter((q) => q.name.toLowerCase() === opts.quant!.toLowerCase())
    if (only.length === 0) {
      UI.error(`No ${opts.quant} build of ${id}. Builds: ${quants.map((q) => q.name).join(", ")}`)
      return 1
    }
    quants = only
  }
  let r: FitResult = fit(device.budget, quants)
  if (r.k === "unknown") {
    UI.error(`Size not checked: ${id} has no device data yet, so gizzi cannot tell if it fits. Nothing was downloaded.`)
    return 1
  }
  if (r.k === "no" || r.k === "na" || !r.quant) {
    UI.error(`${r.label}: ${r.why}. Nothing was downloaded. Run \`gizzi models fit ${id}\` for the details.`)
    return 1
  }

  // The engine for the chosen build: install the pack when missing; fall back to GGUF if MLX cannot be made ready.
  let quant: Quant = r.quant
  let err = await ensureEngine(quant.engine === "mlx" ? "mlx" : "llama.cpp")
  if (err && quant.engine === "mlx") {
    const ggufOnly = (quants ?? []).filter((q) => q.engine === "llama.cpp")
    const alt = ggufOnly.length ? fit({ ...device.budget, apple: false }, ggufOnly) : null
    if (alt?.quant && (alt.k === "ok" || alt.k === "slow")) {
      UI.println(`${style.warn}${err} Using the GGUF build instead.${style.off}`)
      r = alt
      quant = alt.quant
      err = await ensureEngine("llama.cpp")
    }
  }
  if (err) {
    UI.error(err)
    return 1
  }

  UI.println(`${style.ok}${r.label}${style.off}  ${id}`)
  row("Build", buildLine(quant, id))

  const repo = quant.repo ?? id
  let { models } = await listModels()
  let cached = findCached(models, repo)
  const have = cached && modelPath(cached, quant)
  if (!have || !cached) {
    const code = await pullModel(repo, quant.engine === "mlx" ? undefined : quant.name, Boolean(opts.allowUnverified))
    if (code !== 0) return code
    ;({ models } = await listModels())
    cached = findCached(models, repo)
  }
  const path = cached && modelPath(cached, quant)
  if (!cached || !path) {
    UI.error(`Downloaded ${repo} but the engine did not report where it is. Run \`gizzi models verify ${repo}\`.`)
    return 1
  }

  // Reuse a model that is already loaded.
  const rts = (await listRuntimes()).runtimes
  let rt: RuntimeInstance | undefined = rts.find((x) => x.model_id === id && (x.status === "running" || x.status === "starting"))
  if (!rt) {
    try {
      const recipe = quant.engine === "mlx" ? ({ backend: "mlx", model_path: path } as const) : ({ backend: "llama_cpp", model_path: path, n_gpu_layers: device.hardware?.gpus.length || device.budget.apple ? 999 : 0 } as const)
      rt = (await launchRuntime(id, recipe)).runtime
    } catch (e) {
      const status = e instanceof LocalEngineError ? e.status : undefined
      UI.error(`The engine could not start ${id}${status ? ` (${status})` : ""}: ${e instanceof Error ? e.message : e}`)
      return 1
    }
  }
  const up = await waitFor(`Loading ${id}`, async () => {
    const x = (await listRuntimes()).runtimes.find((y) => y.id === rt!.id)
    if (!x || x.status === "error" || x.status === "stopped") return { dead: x?.health?.error ?? x?.status ?? "gone" }
    return x.status === "running" && x.health?.reachable !== false ? { rt: x } : null
  }, 180_000)
  process.stderr.write("\n")
  if (!up || "dead" in up) {
    UI.error(`${id} did not start${up && "dead" in up ? `: ${up.dead}` : " in time"}. The engine log is in its logs folder.`)
    return 1
  }
  const via = up.rt.pack_id ? `pack ${up.rt.pack_id} ${up.rt.pack_version ?? ""}` : "engine on PATH (no pack)"
  row("Engine", `${up.rt.backend ?? quant.engine} · ${via}`)

  const history: Array<{ role: "user" | "assistant"; content: string }> = []
  const stopHint = `Model stays loaded until idle for 5 minutes. Unload now: gizzi models stop ${id}`
  try {
    if (opts.prompt) {
      history.push({ role: "user", content: opts.prompt })
      await turn(id, history, false) // one-shot: no thinking, so the token count is the reply
      UI.println(`${style.dim}${stopHint}${style.off}`)
      return 0
    }
    UI.println(`${style.dim}Chatting with ${id} on this computer. Type /exit to leave.${style.off}`)
    const rl = createInterface({ input: process.stdin, output: process.stderr, prompt: "> " })
    rl.prompt()
    for await (const line of rl) {
      const t = line.trim()
      if (t === "/exit" || t === "/quit") break
      if (t) {
        history.push({ role: "user", content: t })
        try {
          await turn(id, history)
        } catch (e) {
          history.pop()
          UI.error(`Chat failed: ${e instanceof Error ? e.message : e}`)
        }
      }
      rl.prompt()
    }
    rl.close()
    UI.println(`${style.dim}${stopHint}${style.off}`)
    return 0
  } catch (e) {
    UI.error(`Chat failed: ${e instanceof Error ? e.message : e}`)
    return 1
  }
}

/** gizzi models ps [--json] */
export async function psModels(json: boolean): Promise<number> {
  let rts: RuntimeInstance[]
  try {
    rts = (await listRuntimes()).runtimes.filter((r) => r.status === "running" || r.status === "starting")
  } catch (e) {
    UI.error(`Cannot reach the Local Engine (${e instanceof Error ? e.message : e}).`)
    return 1
  }
  if (json) {
    process.stdout.write(JSON.stringify(rts, null, 2) + "\n")
    return 0
  }
  if (rts.length === 0) {
    UI.println("No models running. Start one with `gizzi models run <org/model>`.")
    return 0
  }
  UI.println(`  ${"MODEL".padEnd(40)}${"STATUS".padEnd(10)}${"MEMORY".padEnd(10)}${"SPEED".padEnd(12)}${"CONTEXT".padEnd(14)}IDLE`)
  for (const r of rts) {
    const x = r.running
    const mem = x?.mem_bytes != null ? fmtBytes(x.mem_bytes) : "-"
    const speed = x?.tok_per_sec != null ? `${x.tok_per_sec.toFixed(1)} tok/s` : "-"
    const ctx = x?.ctx_max != null ? `${x.ctx_used ?? 0} / ${x.ctx_max}` : "-"
    const idle = x ? `${x.idle_sec}s of ${x.idle_unload_sec}s` : "-"
    UI.println(`  ${r.model_id.padEnd(40)}${r.status.padEnd(10)}${mem.padEnd(10)}${speed.padEnd(12)}${ctx.padEnd(14)}${idle}`)
  }
  return 0
}

/** gizzi models stop <id> */
export async function stopModel(id: string | undefined): Promise<number> {
  if (!id) {
    UI.error("Usage: gizzi models stop <org/model>")
    return 1
  }
  try {
    const rts = (await listRuntimes()).runtimes.filter((r) => (r.model_id === id || r.id === id) && (r.status === "running" || r.status === "starting"))
    if (rts.length === 0) {
      UI.println(`${id} is not running.`)
      return 0
    }
    for (const r of rts) await stopRuntime(r.id)
    UI.println(`${style.ok}Unloaded ${id}${style.off}`)
    return 0
  } catch (e) {
    UI.error(`Could not stop ${id}: ${e instanceof Error ? e.message : e}`)
    return 1
  }
}
