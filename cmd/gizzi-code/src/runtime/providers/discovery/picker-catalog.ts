/**
 * Live picker catalog — GET {cloud}/api/v1/picker-catalog (contract:
 * docs/contracts/picker-catalog/schema.json).
 *
 * Gizzi starts from the generated snapshot (runtime-registry.ts) and lays this
 * live catalog over it, so a catalog fix reaches every installed Gizzi with a
 * cloud-api deploy. Fetch rules: the user's token when signed in, ETag /
 * If-None-Match, a 6-hour disk cache (per signed-in identity, because `access`
 * is computed for the caller), a 4 s timeout, and never an exception: on any
 * failure the cache is used, then the snapshot (callers get `undefined`).
 */

import { createHash } from "node:crypto"
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs"
import path from "node:path"
import { GlobalPaths } from "@/runtime/context/global/paths"
import { cloudInferenceToken, cloudOrigin } from "./allternit-cloud"
import type { DiscoveredModel } from "./index"
import { setSnapshotOverlay, type RuntimeEntry, type RuntimeSnapshot, type SnapshotTool } from "./runtime-registry"

export const PICKER_CATALOG_TTL_MS = 6 * 60 * 60 * 1000
export const PICKER_CATALOG_TIMEOUT_MS = 4000

export type CatalogAccess = { state: "ok" | "locked" | "hidden"; reason?: string; action?: string }

export interface PickerCatalog {
  schema: 1
  generatedAt: string
  runtimes: Array<{
    id: string
    label: string
    vendor: string
    logo: string
    signIn: string
    defaultModel: string
    models: Array<{ id: string; name: string; aliases: string[]; context: number; output: number }>
    installed?: boolean
    signedIn?: boolean | null
  }>
  brains: Array<{ id: string; label: string; kind: string; access: CatalogAccess }>
  cloudModels: Array<{ id: string; name: string; provider: string; access: CatalogAccess }>
  onDevice: Array<{ id: string; name: string; sizeBytes: number; minRamGb: number; platforms: string[] }>
}

/** `unsupported` = the server has no route (404, an older server): use /v1/models. */
export type PickerCatalogResult =
  | { status: "ok"; catalog: PickerCatalog; from: "live" | "cache" | "stale-cache" }
  | { status: "unsupported" }
  | { status: "unavailable" }

type CacheFile = { at: number; key: string; etag?: string; catalog: PickerCatalog }

export type FetchOptions = {
  fetch?: typeof fetch
  now?: number
  cacheFile?: string
  origin?: string
  /** `null` = signed out; omitted = resolve from env / pairing. */
  token?: string | null
  ignoreFresh?: boolean
}

export function pickerCatalogCacheFile(): string {
  return path.join(GlobalPaths.cache, "picker-catalog.json")
}

function identityKey(origin: string, token: string | undefined): string {
  const who = token ? createHash("sha256").update(token).digest("hex").slice(0, 16) : "anon"
  return `${origin}|${who}`
}

function readCache(file: string): CacheFile | undefined {
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as CacheFile
    if (typeof parsed?.at === "number" && typeof parsed.key === "string" && isCatalog(parsed.catalog)) return parsed
  } catch {}
  return undefined
}

function writeCache(file: string, value: CacheFile): void {
  try {
    mkdirSync(path.dirname(file), { recursive: true })
    const tmp = `${file}.${process.pid}.tmp`
    writeFileSync(tmp, JSON.stringify(value))
    renameSync(tmp, file)
  } catch {}
}

function isCatalog(value: unknown): value is PickerCatalog {
  const c = value as PickerCatalog | undefined
  return !!c && c.schema === 1 && Array.isArray(c.runtimes) && Array.isArray(c.cloudModels) && Array.isArray(c.brains)
}

export async function fetchPickerCatalog(opts: FetchOptions = {}): Promise<PickerCatalogResult> {
  const now = opts.now ?? Date.now()
  const file = opts.cacheFile ?? pickerCatalogCacheFile()
  try {
    const origin = opts.origin ?? cloudOrigin()
    const token = opts.token === undefined ? await cloudInferenceToken() : (opts.token ?? undefined)
    const key = identityKey(origin, token)
    const cached = readCache(file)
    const mine = cached && cached.key === key ? cached : undefined
    const refresh = opts.ignoreFresh || process.env.GIZZI_MODELS_REFRESH === "1"
    if (mine && !refresh && now - mine.at < PICKER_CATALOG_TTL_MS) {
      return { status: "ok", catalog: mine.catalog, from: "cache" }
    }
    const headers: Record<string, string> = {}
    if (token) headers.Authorization = `Bearer ${token}`
    if (mine?.etag) headers["If-None-Match"] = mine.etag
    let res: Response
    try {
      res = await (opts.fetch ?? fetch)(`${origin}/api/v1/picker-catalog`, {
        headers,
        signal: AbortSignal.timeout(PICKER_CATALOG_TIMEOUT_MS),
      })
    } catch {
      return mine ? { status: "ok", catalog: mine.catalog, from: "stale-cache" } : { status: "unavailable" }
    }
    if (res.status === 404) return { status: "unsupported" }
    if (res.status === 304 && mine) {
      writeCache(file, { ...mine, at: now })
      return { status: "ok", catalog: mine.catalog, from: "cache" }
    }
    if (res.ok) {
      const json = (await res.json()) as unknown
      if (isCatalog(json)) {
        writeCache(file, { at: now, key, etag: res.headers.get("etag") ?? undefined, catalog: json })
        return { status: "ok", catalog: json, from: "live" }
      }
    }
    return mine ? { status: "ok", catalog: mine.catalog, from: "stale-cache" } : { status: "unavailable" }
  } catch {
    return { status: "unavailable" }
  }
}

// One lookup per process; discovery and the picker share it.
let shared: Promise<PickerCatalogResult> | undefined
export function pickerCatalog(): Promise<PickerCatalogResult> {
  return (shared ??= fetchPickerCatalog().then((result) => {
    installOverlay(result.status === "ok" ? result.catalog : undefined)
    return result
  }))
}

/** After the catalog loads, sync readers (`currentRuntimeSnapshot`) see it; memoized per snapshot object. */
export function installOverlay(catalog: PickerCatalog | undefined): void {
  if (!catalog) return setSnapshotOverlay(undefined)
  let from: RuntimeSnapshot | undefined
  let out: LiveSnapshot | undefined
  setSnapshotOverlay((snapshot) => {
    if (from !== snapshot || !out) {
      from = snapshot
      out = overlaySnapshot(snapshot, catalog)
    }
    return out as unknown as RuntimeSnapshot
  })
}
/** Test hook and `gizzi models --refresh`: forget the shared lookup. */
export function resetPickerCatalog(): void {
  shared = undefined
  setSnapshotOverlay(undefined)
}

// ── Cloud models: apply `access` ─────────────────────────────────────────────

/** The id the Cloud provider ("allternit") knows: the catalog writes `allternit/<id>`. */
function cloudModelId(id: string): string {
  return id.startsWith("allternit/") ? id.slice("allternit/".length) : id
}

/**
 * Catalog cloud models → discovered models. hidden: kept but flagged `hidden`
 * (never offered, saved choices still resolve) and sorted last. locked: listed
 * with `locked` {reason, action} for the picker to grey. ok: normal.
 */
export function cloudModelsFromCatalog(catalog: PickerCatalog): DiscoveredModel[] {
  const offered: DiscoveredModel[] = []
  const hidden: DiscoveredModel[] = []
  for (const row of catalog.cloudModels) {
    const id = cloudModelId(row.id?.trim() ?? "")
    if (!id) continue
    const model: DiscoveredModel = { id, name: row.name || id, context: 128000, output: 8192 }
    if (row.access.state === "hidden") {
      model.hidden = true
      hidden.push(model)
      continue
    }
    if (row.access.state === "locked") {
      model.locked = { reason: row.access.reason ?? "Locked", ...(row.access.action ? { action: row.access.action } : {}) }
    }
    offered.push(model)
  }
  return [...offered, ...hidden].map((model, rank) => ({ ...model, rank }))
}

/** The Allternit Cloud brain row's access, for the picker's Brain layer. */
export function cloudBrainAccess(catalog: PickerCatalog): CatalogAccess | undefined {
  return catalog.brains.find((b) => b.kind === "cloud")?.access
}

// ── Runtime overlay ──────────────────────────────────────────────────────────

export type LiveRuntimeRow = RuntimeEntry & {
  defaultModel?: string
  models?: PickerCatalog["runtimes"][number]["models"]
}

export interface LiveSnapshot extends Omit<RuntimeSnapshot, "runtimes"> {
  runtimes: LiveRuntimeRow[]
  /** Live ids the snapshot does not know: only the fields the catalog has. */
  extraRuntimes: PickerCatalog["runtimes"]
  brains: PickerCatalog["brains"]
  onDevice: PickerCatalog["onDevice"]
  live: boolean
}

function matches(row: { id: string; aliases?: string[] }, id: string): boolean {
  return row.id === id || (row.aliases ?? []).includes(id)
}

/**
 * Lay live rows over the snapshot BY ID (or alias). Snapshot rows keep every
 * field (adapter, look, auth, headless); the live row sets models, default
 * model and labels. Tools (CLI rows) take models and the default flag.
 */
export function overlaySnapshot(snapshot: RuntimeSnapshot, catalog: PickerCatalog | undefined): LiveSnapshot {
  const base: LiveSnapshot = {
    ...snapshot,
    runtimes: snapshot.runtimes.map((r) => ({ ...r })),
    tools: snapshot.tools.map((t) => ({ ...t })),
    extraRuntimes: [],
    brains: [],
    onDevice: [],
    live: false,
  }
  if (!catalog) return base
  base.live = true
  base.brains = catalog.brains
  base.onDevice = catalog.onDevice
  for (const live of catalog.runtimes) {
    const runtime = base.runtimes.find((r) => matches(r, live.id))
    const tool = base.tools.find((t: SnapshotTool) => matches(t, live.id))
    if (!runtime && !tool) {
      base.extraRuntimes.push(live)
      continue
    }
    if (runtime) {
      runtime.name = live.label || runtime.name
      runtime.vendor = live.vendor || runtime.vendor
      runtime.defaultModel = live.defaultModel
      runtime.models = live.models
    }
    if (tool) {
      if (live.models.length > 0) {
        tool.models = live.models.map((m) => ({
          id: m.id,
          name: m.name,
          context: m.context,
          output: m.output,
          ...(m.id === live.defaultModel ? { default: true } : {}),
        }))
      }
    }
  }
  return base
}

/** The snapshot with the live catalog over it (snapshot alone when offline). Never throws, never waits past 4 s. */
export async function livePickerSnapshot(snapshot: RuntimeSnapshot): Promise<LiveSnapshot> {
  const result = await pickerCatalog()
  return overlaySnapshot(snapshot, result.status === "ok" ? result.catalog : undefined)
}
