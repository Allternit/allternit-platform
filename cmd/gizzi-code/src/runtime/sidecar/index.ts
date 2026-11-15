/**
 * Embedded Model Sidecar
 *
 * Ships a local quantized model (Qwen 3.5 4B Q4_K_M) with gizzi-code.
 * On first run, auto-pulls weights to ~/.local/share/gizzi-code/models/.
 * Starts an Ollama-compatible inference server as a daemon process.
 * Used for background tasks: title generation, compaction, summaries.
 */

import { spawn, type ChildProcess } from "child_process"
import fs from "fs/promises"
import path from "path"
import os from "os"
import { Log } from "@/shared/util/log"
import { GlobalPaths } from "@/runtime/context/global/paths"
import { Filesystem } from "@/shared/util/filesystem"
import { freeweightsIds } from "@/runtime/services/freeweightsCatalog"
import { killProcessTree, ProcessRegistry } from "@/runtime/process-registry"

const log = Log.create({ service: "sidecar" })

const SIDECAR_PORT = 11435
const SIDECAR_HOST = "127.0.0.1"
/**
 * If ALLTERNIT_SIDECAR_URL is set (e.g. "http://my-vps.example.com:11434"),
 * the sidecar skips local Ollama startup and points directly at the remote server.
 */
const REMOTE_SIDECAR_URL = process.env["ALLTERNIT_SIDECAR_URL"]?.replace(/\/$/, "")
/**
 * Set ALLTERNIT_SIDECAR_DISABLED=1 to skip sidecar startup entirely.
 * Useful on low-resource VMs or environments where local Ollama is unavailable/undesirable.
 */
const SIDECAR_DISABLED = process.env["ALLTERNIT_SIDECAR_DISABLED"] === "1" || process.env["ALLTERNIT_SIDECAR_DISABLED"] === "true"

// Default embedded model — shipped with gizzi-code
const EMBEDDED_MODEL = {
  id: "qwen3.5:4b",
  name: "Qwen 3.5 4B (Embedded)",
  // Ollama model identifier for pulling
  ollamaTag: "qwen3:4b",
  contextLength: 32768,
  outputLimit: 4096,
}

export namespace Sidecar {
  export const Port = SIDECAR_PORT
  export const Host = SIDECAR_HOST
  export const BaseURL = REMOTE_SIDECAR_URL
    ? `${REMOTE_SIDECAR_URL}/v1`
    : `http://${SIDECAR_HOST}:${SIDECAR_PORT}/v1`
  export const Model = EMBEDDED_MODEL

  const paths = {
    get root() {
      return path.join(GlobalPaths.data, "sidecar")
    },
    get pid() {
      return path.join(GlobalPaths.data, "sidecar", "sidecar.pid")
    },
    get log() {
      return path.join(GlobalPaths.data, "sidecar", "sidecar.log")
    },
    get ready() {
      return path.join(GlobalPaths.data, "sidecar", "ready")
    },
  }

  /**
   * Check if Ollama is installed on the system
   */
  async function findOllama(): Promise<string | null> {
    const candidates = [
      "/usr/local/bin/ollama",
      "/opt/homebrew/bin/ollama",
      path.join(os.homedir(), ".ollama", "bin", "ollama"),
      "ollama", // PATH fallback
    ]

    for (const candidate of candidates) {
      try {
        const proc = Bun.spawn(["which", candidate], { stdout: "pipe", stderr: "pipe" })
        const code = await proc.exited
        if (code === 0) {
          const out = await new Response(proc.stdout).text()
          return out.trim() || candidate
        }
      } catch {
        // which/spawn failed — try the next candidate.
      }

      // Direct existence check for absolute paths
      if (candidate.startsWith("/")) {
        if (await Filesystem.exists(candidate)) return candidate
      }
    }

    return null
  }

  /**
   * Check if sidecar server is already running and healthy
   */
  export async function isRunning(): Promise<boolean> {
    try {
      const resp = await fetch(`http://${SIDECAR_HOST}:${SIDECAR_PORT}/api/tags`, {
        signal: AbortSignal.timeout(2000),
      })
      return resp.ok
    } catch {
      return false
    }
  }

  /**
   * Check if the embedded model is pulled
   */
  async function isModelPulled(): Promise<boolean> {
    try {
      const resp = await fetch(`http://${SIDECAR_HOST}:${SIDECAR_PORT}/api/tags`, {
        signal: AbortSignal.timeout(3000),
      })
      if (!resp.ok) return false
      const data = (await resp.json()) as { models?: Array<{ name: string }> }
      return data.models?.some((m) => m.name.includes(EMBEDDED_MODEL.ollamaTag)) ?? false
    } catch {
      return false
    }
  }

  /**
   * Pull any Ollama-resolvable tag into the sidecar's isolated instance —
   * a built-in Ollama library tag ("llama3.2:3b") or, since Ollama itself
   * added native HuggingFace support, `hf.co/<repo>[:<quant>]` to pull an
   * arbitrary GGUF repo directly. Ollama does the actual HF download/GGUF
   * handling — gizzi-code never touches raw weights or parses GGUF itself.
   */
  async function pullTag(ollamaBin: string, tag: string, onProgress?: (line: string) => void): Promise<boolean> {
    log.info("pulling model", { tag })

    return new Promise((resolve) => {
      const env = {
        ...process.env,
        OLLAMA_HOST: `${SIDECAR_HOST}:${SIDECAR_PORT}`,
      }

      const child = spawn(ollamaBin, ["pull", tag], {
        env,
        stdio: ["ignore", "pipe", "pipe"],
      })

      let stderr = ""
      child.stdout?.on("data", (d) => onProgress?.(d.toString()))
      child.stderr?.on("data", (d) => {
        stderr += d.toString()
        onProgress?.(d.toString())
      })

      child.on("close", (code) => {
        if (code === 0) {
          log.info("model pulled successfully", { tag })
          resolve(true)
        } else {
          log.error("failed to pull model", { tag, code, stderr: stderr.slice(-500) })
          resolve(false)
        }
      })

      child.on("error", (err) => {
        log.error("pull process error", { tag, error: err.message })
        resolve(false)
      })

      // 10 minute timeout for pulling
      setTimeout(() => {
        try {
          child.kill()
        } catch {
          // child may have already exited.
        }
        log.error("pull timed out", { tag })
        resolve(false)
      }, 600_000)
    })
  }

  /** Back-compat name for the one call site that only ever pulls the embedded default. */
  async function pullModel(ollamaBin: string): Promise<boolean> {
    return pullTag(ollamaBin, EMBEDDED_MODEL.ollamaTag)
  }

  /**
   * Start the Ollama sidecar server as a detached daemon
   */
  async function startServer(ollamaBin: string): Promise<boolean> {
    await fs.mkdir(paths.root, { recursive: true })

    // Clean stale ready marker
    await fs.rm(paths.ready, { force: true }).catch(() => {})

    const logFile = Bun.file(paths.log)
    const logFd = logFile.writer()

    const env = {
      ...process.env,
      OLLAMA_HOST: `${SIDECAR_HOST}:${SIDECAR_PORT}`,
      OLLAMA_MODELS: path.join(GlobalPaths.data, "models"),
      // Limit resource usage — this is a background sidecar
      OLLAMA_NUM_PARALLEL: "1",
      OLLAMA_MAX_LOADED_MODELS: "1",
      OLLAMA_KEEP_ALIVE: "5m",
    }

    log.info("starting sidecar", { port: SIDECAR_PORT, bin: ollamaBin })

    const child = spawn(ollamaBin, ["serve"], {
      env,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
    })

    if (!child.pid) {
      log.error("failed to start sidecar — no PID")
      return false
    }

    // Write PID file
    await Filesystem.write(paths.pid, String(child.pid))

    // Pipe output to log file
    child.stdout?.on("data", (d) => logFd.write(d))
    child.stderr?.on("data", (d) => logFd.write(d))
    // Stay attached to the session so close/SIGINT reaps the sidecar. Do not unref.
    ProcessRegistry.track(child, { label: "sidecar-ollama", group: process.platform !== "win32" })

    // Wait for server to be ready (up to 15 seconds)
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => setTimeout(r, 500))
      if (await isRunning()) {
        await Filesystem.write(paths.ready, String(Date.now()))
        log.info("sidecar ready", { pid: child.pid, port: SIDECAR_PORT })
        return true
      }
    }

    log.error("sidecar failed to become ready within 15 seconds")
    return false
  }

  /**
   * Stop the sidecar daemon
   */
  export async function stop(): Promise<void> {
    try {
      const pidStr = await Filesystem.readText(paths.pid)
      const pid = parseInt(pidStr, 10)
      if (!isNaN(pid)) {
        killProcessTree(pid, process.platform !== "win32")
        log.info("sidecar stopped", { pid })
      }
    } catch {
      // No pid file or process already gone — forced cleanup below.
    }

    await fs.rm(paths.pid, { force: true }).catch(() => {})
    await fs.rm(paths.ready, { force: true }).catch(() => {})
  }

  /**
   * Ensure the sidecar is running and the model is available.
   * Called during bootstrap — non-blocking for the main CLI.
   */
  export async function ensure(): Promise<{
    available: boolean
    baseURL: string
    modelID: string
  }> {
    const result = { available: false, baseURL: BaseURL, modelID: EMBEDDED_MODEL.id }

    if (SIDECAR_DISABLED) {
      log.info("sidecar disabled via ALLTERNIT_SIDECAR_DISABLED")
      return result
    }

    // Remote sidecar: skip local Ollama startup, probe the remote endpoint directly.
    if (REMOTE_SIDECAR_URL) {
      try {
        const resp = await fetch(`${REMOTE_SIDECAR_URL}/api/tags`, { signal: AbortSignal.timeout(4000) })
        if (resp.ok) {
          result.available = true
          log.info("remote sidecar connected", { url: REMOTE_SIDECAR_URL })
        } else {
          log.warn("remote sidecar responded with error", { status: resp.status, url: REMOTE_SIDECAR_URL })
        }
      } catch (err) {
        log.warn("remote sidecar unreachable", { url: REMOTE_SIDECAR_URL, error: err })
      }
      return result
    }

    // Check if already running (could be a shared Ollama instance or previous sidecar)
    if (await isRunning()) {
      // Check if our model is available
      if (await isModelPulled()) {
        result.available = true
        log.info("sidecar already running with model")
        return result
      }
    }

    // Find Ollama binary
    const ollamaBin = await findOllama()
    if (!ollamaBin) {
      log.warn("ollama not found — sidecar disabled. Install ollama to enable embedded model.")
      return result
    }

    // Start server if not running
    if (!(await isRunning())) {
      const started = await startServer(ollamaBin)
      if (!started) return result
    }

    // Pull model if not present
    if (!(await isModelPulled())) {
      const pulled = await pullModel(ollamaBin)
      if (!pulled) return result
    }

    result.available = true
    return result
  }

  // ── Arbitrary GGUF models (HuggingFace search + install) ──────────────
  //
  // PocketPal AI parity: the embedded default above stays fixed, but the
  // sidecar can now also host any GGUF the user chooses via HuggingFace
  // search, not just the one shipped model — the actual ask behind this
  // work. Promoted from internal-only (title-gen/compaction) to a real
  // user-facing capability: installed models flow into providerConfig()
  // below as first-class, individually selectable entries.

  export interface HuggingFaceGgufResult {
    /** e.g. "bartowski/Llama-3.2-3B-Instruct-GGUF" */
    repoId: string
    downloads: number
    likes: number
    tags?: string[]
    pipeline_tag?: string
    lastModified?: string
    /** Sum of `.gguf` sibling file sizes from the HF detail API, when fetched in time. */
    sizeBytes?: number
    /** True when the Freeweights catalog lists this repo (signed manifest available). */
    freeweights?: boolean
  }

  /** Max detail fetches per search when resolving real GGUF sizes. */
  const SIZE_FETCH_MAX_MODELS = 12
  /** Overall time budget (ms) for size resolution inside one search. */
  const SIZE_FETCH_BUDGET_MS = 6000

  interface HfModelDetail {
    siblings?: Array<{ rfilename?: string; size?: number }>
  }

  /**
   * Best-effort real sizes: the HF list endpoint does not include file sizes,
   * so query the per-repo detail endpoint (`?blobs=true`) for the top results
   * and sum the `.gguf` siblings. Failures and timeouts leave `sizeBytes`
   * undefined rather than slowing the search down.
   */
  async function attachGgufSizes(models: HuggingFaceGgufResult[]): Promise<void> {
    const deadline = Date.now() + SIZE_FETCH_BUDGET_MS
    const targets = models.slice(0, SIZE_FETCH_MAX_MODELS)
    await Promise.allSettled(
      targets.map(async (model) => {
        const remaining = deadline - Date.now()
        if (remaining <= 0) return
        const path = model.repoId.split("/").map(encodeURIComponent).join("/")
        const url = `https://huggingface.co/api/models/${path}?blobs=true`
        const resp = await fetch(url, {
          signal: AbortSignal.timeout(Math.min(remaining, SIZE_FETCH_BUDGET_MS)),
        })
        if (!resp.ok) return
        const detail = (await resp.json()) as HfModelDetail
        let sizeBytes = 0
        for (const sibling of detail.siblings ?? []) {
          if (sibling.rfilename?.endsWith(".gguf") && typeof sibling.size === "number") {
            sizeBytes += sibling.size
          }
        }
        if (sizeBytes > 0) model.sizeBytes = sizeBytes
      }),
    )
  }

  /**
   * Searches HuggingFace's public model API for GGUF-tagged repos. No auth
   * needed for public repos — same API PocketPal's own HF search uses.
   */
  export async function searchHuggingFace(query: string, limit = 20): Promise<HuggingFaceGgufResult[]> {
    const url = new URL("https://huggingface.co/api/models")
    url.searchParams.set("search", query)
    url.searchParams.set("filter", "gguf")
    url.searchParams.set("sort", "downloads")
    url.searchParams.set("direction", "-1")
    url.searchParams.set("limit", String(limit))

    try {
      const resp = await fetch(url.toString(), { signal: AbortSignal.timeout(8000) })
      if (!resp.ok) {
        log.warn("huggingface search failed", { status: resp.status })
        return []
      }
      const data = (await resp.json()) as Array<{
        id?: string
        modelId?: string
        downloads?: number
        likes?: number
        tags?: string[]
        pipeline_tag?: string
        lastModified?: string
      }>
      const models: HuggingFaceGgufResult[] = data
        .map((m) => ({
          repoId: m.id ?? m.modelId ?? "",
          downloads: m.downloads ?? 0,
          likes: m.likes ?? 0,
          tags: m.tags ?? [],
          pipeline_tag: m.pipeline_tag,
          lastModified: m.lastModified,
        }))
        .filter((m) => m.repoId.length > 0)
      const [, fwIds] = await Promise.all([attachGgufSizes(models), freeweightsIds()])
      for (const m of models) m.freeweights = fwIds.has(m.repoId.toLowerCase())
      return models
    } catch (err) {
      log.warn("huggingface search error", { error: err })
      return []
    }
  }

  /**
   * Installs an arbitrary GGUF model into the sidecar's isolated Ollama
   * instance via Ollama's own `hf.co/<repo>` pull support. `quantTag` picks
   * a specific quantization (e.g. "Q4_K_M") when the repo has more than
   * one GGUF file; omitted, Ollama picks its own default.
   */
  export async function installCustomModel(
    repoId: string,
    quantTag?: string,
    onProgress?: (line: string) => void,
  ): Promise<{ ok: boolean; tag: string; error?: string }> {
    const ollamaBin = await findOllama()
    if (!ollamaBin) {
      return { ok: false, tag: "", error: "Ollama not found — install it to add custom local models." }
    }
    if (!(await isRunning())) {
      const started = await startServer(ollamaBin)
      if (!started) return { ok: false, tag: "", error: "Sidecar failed to start." }
    }

    const tag = quantTag ? `hf.co/${repoId}:${quantTag}` : `hf.co/${repoId}`
    const ok = await pullTag(ollamaBin, tag, onProgress)
    return ok ? { ok: true, tag } : { ok: false, tag, error: "Pull failed — check the model exists and is GGUF-formatted." }
  }

  /** Removes an installed model (custom or the embedded default) from the sidecar. */
  export async function removeModel(tag: string): Promise<boolean> {
    const ollamaBin = await findOllama()
    if (!ollamaBin) return false
    return new Promise((resolve) => {
      const child = spawn(ollamaBin, ["rm", tag], {
        env: { ...process.env, OLLAMA_HOST: `${SIDECAR_HOST}:${SIDECAR_PORT}` },
        stdio: ["ignore", "ignore", "pipe"],
      })
      child.on("close", (code) => resolve(code === 0))
      child.on("error", () => resolve(false))
    })
  }

  /** All models currently installed in the sidecar (embedded default + any custom installs). */
  export async function listInstalledModels(): Promise<Array<{ tag: string; sizeBytes?: number }>> {
    try {
      const resp = await fetch(`http://${SIDECAR_HOST}:${SIDECAR_PORT}/api/tags`, {
        signal: AbortSignal.timeout(3000),
      })
      if (!resp.ok) return []
      const data = (await resp.json()) as { models?: Array<{ name: string; size?: number }> }
      return (data.models ?? []).map((m) => ({ tag: m.name, sizeBytes: m.size }))
    } catch {
      return []
    }
  }

  /**
   * Get the provider config to inject into gizzi-code's provider system.
   * Returns null if sidecar is not available. Includes every installed
   * model, not just the embedded default — each one flows into the same
   * catalog cloud models already use, individually selectable.
   */
  export async function providerConfig(): Promise<{
    providerID: string
    npm: string
    options: Record<string, unknown>
    models: Record<string, unknown>
  } | null> {
    const installed = await listInstalledModels()
    const models: Record<string, unknown> = {
      [EMBEDDED_MODEL.id]: {
        id: EMBEDDED_MODEL.ollamaTag,
        name: EMBEDDED_MODEL.name,
        tool_call: true,
        limit: {
          context: EMBEDDED_MODEL.contextLength,
          output: EMBEDDED_MODEL.outputLimit,
        },
      },
    }
    for (const model of installed) {
      if (model.tag === EMBEDDED_MODEL.ollamaTag || models[model.tag]) continue
      models[model.tag] = {
        id: model.tag,
        name: model.tag,
        tool_call: true,
        // Context/output limits are unknown for an arbitrary custom pull —
        // conservative defaults rather than guessing per-model specs.
        limit: { context: 8192, output: 2048 },
      }
    }

    return {
      providerID: "sidecar",
      npm: "@ai-sdk/openai-compatible",
      options: {
        baseURL: BaseURL,
      },
      models,
    }
  }

  /**
   * Get the small_model identifier for config injection
   */
  export function smallModelID(): string {
    return `sidecar/${EMBEDDED_MODEL.id}`
  }
}
