/**
 * Live CLI model lists
 *
 * A catalog CLI with `liveModels` prints its own model list (`codex debug
 * models`, `agy models`, `grok models`, `ollama list`). The list is parsed,
 * merged with the catalog rows (which keep their context/output limits), and
 * cached on disk for 6 hours so discovery does not spawn every CLI on every
 * start. When the live call fails: a stale cache entry, then the catalog's
 * static `models`.
 *
 * The cache file also holds the sign-in check results (sign-in.ts).
 * `GIZZI_MODELS_REFRESH=1` ignores cached entries (`gizzi models --refresh`).
 */

import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs"
import path from "node:path"
import { GlobalPaths } from "@/runtime/context/global/paths"
import type { DiscoveredModel } from "./index"
import type { CatalogLiveModels, SubprocessSpec } from "./subprocess"

export const LIVE_MODELS_TTL_MS = 6 * 60 * 60 * 1000
const LIVE_MODELS_TIMEOUT_MS = 8000

// ── Parsers (pure) ───────────────────────────────────────────────────────────

/** `codex debug models` JSON: hidden models dropped, vendor priority order. */
export function parseCodexModels(stdout: string): DiscoveredModel[] {
  const catalog = JSON.parse(stdout) as {
    models?: Array<{ slug?: string; display_name?: string; context_window?: number; visibility?: string; priority?: number }>
  }
  return (catalog.models ?? [])
    .filter((model) => model.slug && model.visibility !== "hide")
    .map((model, index) => ({ model, index }))
    // Stable: models without a priority keep their file order, after the ranked ones.
    .sort((a, b) => (a.model.priority ?? Infinity) - (b.model.priority ?? Infinity) || a.index - b.index)
    .map(({ model }) => ({
      id: model.slug!,
      name: model.display_name || model.slug!,
      context: model.context_window || 272000,
      output: 32768,
    }))
}

/** `agy models`: a "Fetching…" line, then `id<TAB>name` lines. */
export function parseTsvModels(stdout: string): DiscoveredModel[] {
  const rows: DiscoveredModel[] = []
  for (const line of stdout.split(/\r?\n/)) {
    if (!line.includes("\t")) continue
    const [id, ...rest] = line.split("\t")
    const trimmed = id.trim()
    if (!trimmed) continue
    rows.push({ id: trimmed, name: rest.join("\t").trim() || trimmed })
  }
  return rows
}

/** `grok models`: `*`/`-` bullets under "Available models:"; `(default)` or "Default model:" marks the default. */
export function parseGrokModels(stdout: string): DiscoveredModel[] {
  const lines = stdout.split(/\r?\n/)
  const named = lines.map((l) => /^\s*Default model:\s*(\S+)/i.exec(l)?.[1]).find(Boolean)
  const start = lines.findIndex((l) => /^\s*Available models:?\s*$/i.test(l))
  if (start < 0) return []
  const rows: DiscoveredModel[] = []
  for (const line of lines.slice(start + 1)) {
    const match = /^\s*[*-]\s+(\S+)(.*)$/.exec(line)
    if (!match) {
      if (line.trim() && rows.length > 0) break // the list ended
      continue
    }
    const [, id, rest] = match
    const marked = /\(default\)/i.test(rest)
    rows.push({ id, name: id, ...(marked ? { default: true } : {}) })
  }
  if (named && !rows.some((r) => r.default)) {
    const row = rows.find((r) => r.id === named)
    if (row) row.default = true
  }
  return rows
}

/** `ollama list`: a header row, then `NAME ID SIZE MODIFIED` columns. */
export function parseOllamaModels(stdout: string): DiscoveredModel[] {
  return stdout
    .split("\n")
    .slice(1)
    .map((line) => line.trim().split(/\s+/)[0])
    .filter(Boolean)
    .map((id) => ({ id, name: id, context: 128000, output: 8192 }))
}

export function parseLiveModels(parser: CatalogLiveModels["parser"], stdout: string): DiscoveredModel[] {
  try {
    switch (parser) {
      case "codex-json":
        return parseCodexModels(stdout)
      case "tsv":
        return parseTsvModels(stdout)
      case "grok":
        return parseGrokModels(stdout)
      case "ollama":
        return parseOllamaModels(stdout)
    }
  } catch {
    return []
  }
  return []
}

/**
 * Live rows in live order, with the catalog's limits for ids it knows. One
 * default: the CLI's own marker, else the catalog default when the CLI still
 * lists it, else the first row.
 */
export function mergeLiveModels(spec: Pick<SubprocessSpec, "models">, live: DiscoveredModel[]): DiscoveredModel[] {
  const known = new Map(spec.models.map((m) => [m.id, m]))
  const catalogDefault = spec.models.find((m) => m.default)?.id
  const defaultId =
    live.find((m) => m.default)?.id ??
    (catalogDefault && live.some((m) => m.id === catalogDefault) ? catalogDefault : live[0]?.id)
  return live.map((row, rank) => {
    const k = known.get(row.id)
    const model: DiscoveredModel = { id: row.id, name: row.name || k?.name || row.id, rank }
    const context = k?.context ?? row.context
    const output = k?.output ?? row.output
    if (context !== undefined) model.context = context
    if (output !== undefined) model.output = output
    if (row.id === defaultId) model.default = true
    return model
  })
}

// ── Disk cache (shared with sign-in.ts) ──────────────────────────────────────

type CacheEntry = { at: number; value: unknown }
type CacheFile = { entries: Record<string, CacheEntry> }

export function defaultCacheFile(): string {
  return path.join(GlobalPaths.cache, "cli-models.json")
}

export function refreshRequested(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.GIZZI_MODELS_REFRESH === "1"
}

function readCacheFile(file: string): CacheFile {
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as CacheFile
    return parsed && typeof parsed.entries === "object" && parsed.entries ? parsed : { entries: {} }
  } catch {
    return { entries: {} }
  }
}

export function cacheGet(key: string, file = defaultCacheFile()): CacheEntry | undefined {
  return readCacheFile(file).entries[key]
}

/** Read-modify-write with sync fs (no interleaving in this process) and an atomic rename. */
export function cachePut(key: string, value: unknown, at: number, file = defaultCacheFile()): void {
  try {
    const data = readCacheFile(file)
    data.entries[key] = { at, value }
    mkdirSync(path.dirname(file), { recursive: true })
    const tmp = `${file}.${process.pid}.tmp`
    writeFileSync(tmp, JSON.stringify(data))
    renameSync(tmp, file)
  } catch {
    // A cache we can't write only costs a spawn next time.
  }
}

/** Forget every cached model list and sign-in result. */
export function clearLiveModelCache(file = defaultCacheFile()): void {
  if (existsSync(file)) rmSync(file, { force: true })
}

// ── Running the CLI ──────────────────────────────────────────────────────────

/** stdout of `argv`, or null on a spawn error or after `timeoutMs` (the process is killed). */
export async function runForStdout(
  argv: string[],
  timeoutMs: number,
  env: NodeJS.ProcessEnv = process.env,
): Promise<{ stdout: string; exitCode: number | null } | null> {
  let proc: ReturnType<typeof Bun.spawn>
  try {
    proc = Bun.spawn(argv, { stdin: "ignore", stdout: "pipe", stderr: "ignore", env })
  } catch {
    return null
  }
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), timeoutMs)
  })
  // Race the read: a killed CLI's children can keep the stdout pipe open.
  const out = await Promise.race([new Response(proc.stdout as ReadableStream).text().catch(() => null), timeout])
  clearTimeout(timer)
  if (out === null) {
    try {
      proc.kill(9)
    } catch {
      // already exited
    }
    return null
  }
  const exitCode = await Promise.race([proc.exited, new Promise<null>((r) => setTimeout(() => r(null), 1000))])
  return { stdout: out, exitCode }
}

export interface LiveModelsOptions {
  /** Skip a fresh cache entry (GIZZI_MODELS_REFRESH=1 does the same). */
  refresh?: boolean
  now?: () => number
  cacheFile?: string
}

/** The CLI's own model list merged with the catalog; see the file header for the fallbacks. */
export async function liveModels(
  spec: SubprocessSpec,
  binPath: string,
  options: LiveModelsOptions = {},
): Promise<DiscoveredModel[]> {
  if (!spec.liveModels) return spec.models
  const now = options.now ?? Date.now
  const file = options.cacheFile ?? defaultCacheFile()
  const key = `models:${spec.id}:${binPath}`
  const cached = cacheGet(key, file)
  const cachedRows = Array.isArray(cached?.value) ? (cached!.value as DiscoveredModel[]) : undefined
  const refresh = options.refresh || refreshRequested()

  if (!refresh && cached && cachedRows?.length && now() - cached.at < LIVE_MODELS_TTL_MS) {
    return mergeLiveModels(spec, cachedRows)
  }

  const result = await runForStdout([binPath, ...spec.liveModels.args], LIVE_MODELS_TIMEOUT_MS)
  const parsed = result ? parseLiveModels(spec.liveModels.parser, result.stdout) : []
  if (parsed.length > 0) {
    cachePut(key, parsed, now(), file)
    return mergeLiveModels(spec, parsed)
  }
  if (cachedRows?.length) return mergeLiveModels(spec, cachedRows)
  return spec.models
}
