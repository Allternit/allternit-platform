/**
 * Provider plan quotas — how much of a subscription's rolling windows (e.g.
 * 5-hour, weekly) a provider has used. Read-only: uses credentials the user
 * already configured (a provider CLI's stored sign-in, or the env API key
 * models.dev declares for the provider), never prompts or refreshes them.
 * Fetchers only exist for providers with a real quota source — a documented
 * quota/credits endpoint, or the limits a CLI records locally for its own
 * sessions (Codex rollout `rate_limits`, Claude Code `quotaLimits`);
 * everything else returns { status: "unsupported" } so the client can render
 * an explicit "quota n/a" — never a fabricated window.
 */

import { readFile } from "node:fs/promises"
import { homedir } from "node:os"
import path from "node:path"
import { Log } from "@/shared/util/log"
import { CLOUD_URLS } from "@/shared/constants/cloudUrls"

const log = Log.create({ service: "provider.quota" })

export interface QuotaWindow {
  /** Stable id: "5h" | "7d" | "7d-opus" | "7d-sonnet" | "month" | "month-code" | …. */
  id: string
  label: string
  /** 0–1 share of the window already used. */
  usedRatio: number
  /** ISO time the window resets, when the provider says. */
  resetAt?: string
}

export interface ProviderQuota {
  providerID: string
  source: string
  windows: QuotaWindow[]
  fetchedAt: number
}

export type QuotaResult =
  | { status: "ok"; quota: ProviderQuota }
  | { status: "unsupported" }
  | { status: "signed-out" | "expired" | "error"; message: string }

type Fetcher = () => Promise<QuotaResult>

const CACHE_MS = 60_000
const cache = new Map<string, { at: number; result: QuotaResult }>()

// ── Kimi For Coding (kimi-cli) ────────────────────────────────────────────
// Same endpoint and sign-in the Kimi CLI uses for its own usage view.
function kimiHome(): string {
  return process.env.KIMI_CODE_HOME ?? path.join(homedir(), ".kimi-code")
}

function kimiBaseUrl(): string {
  return (process.env.KIMI_CODE_BASE_URL ?? "https://api.kimi.com/coding/v1").replace(/\/+$/, "")
}

function ratio(value: unknown): number | undefined {
  const n = typeof value === "string" ? Number(value) : value
  return typeof n === "number" && Number.isFinite(n) ? Math.min(1, Math.max(0, n)) : undefined
}

export function parseKimiUsages(payload: any): QuotaWindow[] {
  const usages = payload?.usages ?? {}
  const entries: Array<[string, string, string]> = [
    ["limit_5h", "5h", "5-hour"],
    ["limit_7d", "7d", "Weekly"],
    ["limit_month_total", "month", "Monthly"],
  ]
  const windows: QuotaWindow[] = []
  for (const [key, id, label] of entries) {
    const entry = usages[key]
    const usedRatio = ratio(entry?.used_ratio)
    if (usedRatio === undefined) continue
    const resetAt = typeof entry?.reset_time === "string" && entry.reset_time ? entry.reset_time : undefined
    windows.push({ id, label, usedRatio, ...(resetAt ? { resetAt } : {}) })
  }
  return windows
}

const kimi: Fetcher = async () => {
  let token: string | undefined
  let expiresAt: number | undefined
  try {
    const raw = JSON.parse(await readFile(path.join(kimiHome(), "credentials", "kimi-code.json"), "utf8"))
    token = typeof raw?.access_token === "string" ? raw.access_token : undefined
    expiresAt = typeof raw?.expires_at === "number" ? raw.expires_at : undefined
  } catch {
    return { status: "signed-out", message: "Sign in to Kimi CLI to see plan usage." }
  }
  if (!token) return { status: "signed-out", message: "Sign in to Kimi CLI to see plan usage." }
  // expires_at is epoch seconds; the CLI refreshes it on its next run.
  if (expiresAt && expiresAt * 1000 < Date.now()) {
    return { status: "expired", message: "Kimi sign-in expired; it refreshes the next time Kimi runs." }
  }
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 8000)
  try {
    const res = await fetch(`${kimiBaseUrl()}/usages`, {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json" },
      signal: controller.signal,
    })
    if (res.status === 401) return { status: "expired", message: "Kimi sign-in expired; it refreshes the next time Kimi runs." }
    if (!res.ok) return { status: "error", message: `Kimi usage request failed (${res.status}).` }
    const payload = await res.json()
    return {
      status: "ok",
      quota: {
        providerID: "kimi-cli",
        source: "Kimi For Coding",
        windows: parseKimiUsages(payload),
        fetchedAt: Date.now(),
      },
    }
  } catch (err) {
    log.warn("kimi quota fetch failed", { error: err instanceof Error ? err.message : String(err) })
    return { status: "error", message: "Couldn't reach Kimi to read plan usage." }
  } finally {
    clearTimeout(timer)
  }
}

// ── OpenRouter ──────────────────────────────────────────────────────────────
// Official endpoints, grounded in the OpenRouter API reference
// (openrouter.ai/docs/api-reference/get-credits and /get-api-key-info):
//   GET /api/v1/credits  → { data: { total_credits, total_usage } }
//   GET /api/v1/auth/key → { data: { usage, limit, is_free_tier, … } }
// Auth is the OPENROUTER_API_KEY the user already set for the provider
// (models.dev declares that env var for "openrouter"). Read-only; never
// prompts or provisions a key.
function openrouterBaseUrl(): string {
  return (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "")
}

function dollars(value: unknown): number | undefined {
  const n = typeof value === "string" ? Number(value) : value
  return typeof n === "number" && Number.isFinite(n) ? n : undefined
}

export function parseOpenRouterCredits(payload: any): QuotaWindow[] {
  const total = dollars(payload?.data?.total_credits)
  const used = dollars(payload?.data?.total_usage)
  if (total === undefined || total <= 0 || used === undefined) return []
  return [
    {
      id: "credits",
      label: "Credits",
      usedRatio: Math.min(1, Math.max(0, used / total)),
    },
  ]
}

export function parseOpenRouterKeyLimit(payload: any): QuotaWindow[] {
  const limit = dollars(payload?.data?.limit)
  const used = dollars(payload?.data?.usage)
  if (limit === undefined || limit <= 0 || used === undefined) return []
  return [
    {
      id: "key-limit",
      label: "Key credit limit",
      usedRatio: Math.min(1, Math.max(0, used / limit)),
    },
  ]
}

const openrouter: Fetcher = async () => {
  const key = process.env.OPENROUTER_API_KEY?.trim()
  if (!key) {
    return { status: "signed-out", message: "Set OPENROUTER_API_KEY to see OpenRouter credits." }
  }
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 8000)
  try {
    const headers = { Authorization: `Bearer ${key}`, Accept: "application/json" }
    const res = await fetch(`${openrouterBaseUrl()}/credits`, {
      headers,
      signal: controller.signal,
    })
    if (res.status === 401 || res.status === 403) {
      return { status: "signed-out", message: "OpenRouter rejected the configured OPENROUTER_API_KEY." }
    }
    if (!res.ok) return { status: "error", message: `OpenRouter credits request failed (${res.status}).` }
    let windows = parseOpenRouterCredits(await res.json())
    if (windows.length === 0) {
      // Free-tier / no-credits keys: the key-info endpoint still carries the
      // optional per-key spend cap (limit null when none is set).
      const keyRes = await fetch(`${openrouterBaseUrl()}/auth/key`, {
        headers,
        signal: controller.signal,
      })
      if (keyRes.ok) windows = parseOpenRouterKeyLimit(await keyRes.json())
    }
    return {
      status: "ok",
      quota: {
        providerID: "openrouter",
        source: "OpenRouter",
        windows,
        fetchedAt: Date.now(),
      },
    }
  } catch (err) {
    log.warn("openrouter quota fetch failed", { error: err instanceof Error ? err.message : String(err) })
    return { status: "error", message: "Couldn't reach OpenRouter to read credits." }
  } finally {
    clearTimeout(timer)
  }
}

// ── Local CLI records (no credentials, no network) ─────────────────────────
// The newest file under a CLI's session directory, and its last `maxBytes`.
async function newestFile(root: string, match: (name: string) => boolean, depth = 4): Promise<string | undefined> {
  const { readdir, stat } = await import("node:fs/promises")
  let best: { path: string; mtime: number } | undefined
  async function walk(dir: string, d: number) {
    let entries: import("node:fs").Dirent[]
    try {
      entries = await readdir(dir, { withFileTypes: true })
    } catch {
      return
    }
    for (const e of entries) {
      const full = path.join(dir, e.name)
      if (e.isDirectory() && d > 0) await walk(full, d - 1)
      else if (e.isFile() && match(e.name)) {
        const m = (await stat(full).catch(() => undefined))?.mtimeMs ?? 0
        if (!best || m > best.mtime) best = { path: full, mtime: m }
      }
    }
  }
  await walk(root, depth)
  return best?.path
}

async function tail(file: string, maxBytes = 512 * 1024): Promise<string> {
  const { open } = await import("node:fs/promises")
  const fh = await open(file, "r")
  try {
    const { size } = await fh.stat()
    const start = Math.max(0, size - maxBytes)
    const buf = Buffer.alloc(size - start)
    await fh.read(buf, 0, buf.length, start)
    return buf.toString("utf8")
  } finally {
    await fh.close()
  }
}

/** The last JSON object value of `"key": {...}` in a JSONL tail. */
function lastObject(text: string, key: string): any {
  const needle = `"${key}":{`
  let i = text.lastIndexOf(needle)
  while (i >= 0) {
    let depth = 0
    const from = i + needle.length - 1
    for (let j = from; j < text.length; j++) {
      if (text[j] === "{") depth++
      else if (text[j] === "}" && --depth === 0) {
        try {
          return JSON.parse(text.slice(from, j + 1))
        } catch {
          break
        }
      }
    }
    i = text.lastIndexOf(needle, i - 1)
  }
  return undefined
}

// Codex CLI: every turn records `rate_limits` in its rollout log —
// primary = the 5-hour window, secondary = the weekly one.
export function parseCodexRateLimits(limits: any): QuotaWindow[] {
  const out: QuotaWindow[] = []
  for (const [key, fallbackLabel] of [["primary", "5-hour"], ["secondary", "Weekly"]] as const) {
    const w = limits?.[key]
    const used = typeof w?.used_percent === "number" ? w.used_percent / 100 : undefined
    if (used === undefined) continue
    const minutes = typeof w.window_minutes === "number" ? w.window_minutes : undefined
    const label = minutes === 300 ? "5-hour" : minutes === 10080 ? "Weekly" : fallbackLabel
    const resets = typeof w.resets_at === "number" ? new Date(w.resets_at * 1000).toISOString() : undefined
    out.push({ id: label === "Weekly" ? "7d" : "5h", label, usedRatio: Math.min(1, Math.max(0, used)), ...(resets ? { resetAt: resets } : {}) })
  }
  return out
}

const codex: Fetcher = async () => {
  const home = process.env.CODEX_HOME ?? path.join(homedir(), ".codex")
  const file = await newestFile(path.join(home, "sessions"), (n) => n.startsWith("rollout-") && n.endsWith(".jsonl"))
  if (!file) return { status: "signed-out", message: "Run Codex once to see its plan usage." }
  try {
    const limits = lastObject(await tail(file), "rate_limits")
    return {
      status: "ok",
      quota: { providerID: "codex-cli", source: "Codex", windows: limits ? parseCodexRateLimits(limits) : [], fetchedAt: Date.now() },
    }
  } catch (err) {
    log.warn("codex quota read failed", { error: err instanceof Error ? err.message : String(err) })
    return { status: "error", message: "Couldn't read Codex usage." }
  }
}

// Claude Code: transcripts carry `quotaLimits` when a limit applies —
// `allowed_warning` as it nears one, `rejected` once hit — with resetsAt.
export function parseClaudeQuotaLimits(q: any, now = Date.now()): QuotaWindow[] {
  if (!q || typeof q.resetsAt !== "number") return []
  const resetAt = new Date(q.resetsAt * 1000)
  if (resetAt.getTime() <= now) return []
  const type = String(q.rateLimitType ?? "")
  const label = /seven_day|weekly/.test(type) ? "Weekly" : /five_hour/.test(type) ? "5-hour" : "Usage"
  const usedRatio =
    typeof q.utilization === "number"
      ? Math.min(1, Math.max(0, q.utilization > 1 ? q.utilization / 100 : q.utilization))
      : q.status === "rejected"
        ? 1
        : q.status === "allowed_warning"
          ? 0.95
          : 0
  return [{ id: label === "Weekly" ? "7d" : label === "5-hour" ? "5h" : "usage", label, usedRatio, resetAt: resetAt.toISOString() }]
}

const claude: Fetcher = async () => {
  const root = path.join(process.env.CLAUDE_CONFIG_DIR ?? path.join(homedir(), ".claude"), "projects")
  const file = await newestFile(root, (n) => n.endsWith(".jsonl"), 1)
  if (!file) return { status: "signed-out", message: "Run Claude Code once to see its plan usage." }
  try {
    const q = lastObject(await tail(file), "quotaLimits")
    return {
      status: "ok",
      quota: { providerID: "claude-cli", source: "Claude", windows: parseClaudeQuotaLimits(q), fetchedAt: Date.now() },
    }
  } catch (err) {
    log.warn("claude quota read failed", { error: err instanceof Error ? err.message : String(err) })
    return { status: "error", message: "Couldn't read Claude usage." }
  }
}

// ── Allternit Cloud ────────────────────────────────────────────────────────
// The plan meter the shell shows (`/api/v1/me/usage`: used vs the plan's
// monthly grant or free allowance). Falls back to the gateway's key budget
// (`/v1/rate-limits`) for a gateway virtual key that doesn't resolve to a
// cloud user. Allternit enforces its own wall, so sessions land at `land_at`
// before it — no overage.
function allternitOrigin(): string {
  const explicit = (process.env.ALLTERNIT_API_URL || process.env.ALLTERNIT_API_BASE_URL || "").trim()
  return (explicit || CLOUD_URLS.api).replace(/\/+$/, "")
}

function nextMonthStartUTC(now = Date.now()): string {
  const d = new Date(now)
  return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)).toISOString()
}

export function parseAllternitUsage(payload: any): QuotaWindow[] {
  // The fields are named weekly* but carry the monthly grant / allowance.
  const limit = dollars(payload?.weeklyLimit)
  const used = dollars(payload?.weeklyUsed)
  if (limit === undefined || limit <= 0 || used === undefined) return []
  const resetAt = typeof payload?.resetsAt === "string" && !Number.isNaN(Date.parse(payload.resetsAt)) ? new Date(payload.resetsAt).toISOString() : undefined
  return [
    {
      id: "month",
      label: payload?.plan && payload.plan !== "free" ? "Monthly" : "Free monthly",
      usedRatio: Math.min(1, Math.max(0, used / limit)),
      ...(resetAt ? { resetAt } : {}),
    },
  ]
}

export function parseGatewayRateLimits(payload: any, now = Date.now()): QuotaWindow[] {
  const limit = dollars(payload?.tokens_limit)
  const remaining = dollars(payload?.tokens_remaining)
  if (limit === undefined || limit <= 0 || remaining === undefined) return []
  // tokens_* are the key's monthly budget in cents.
  return [{ id: "month", label: "Key budget", usedRatio: Math.min(1, Math.max(0, 1 - remaining / limit)), resetAt: nextMonthStartUTC(now) }]
}

const allternit: Fetcher = async () => {
  const token = (process.env.ALLTERNIT_API_KEY || process.env.ALLTERNIT_API_TOKEN || "").trim()
  if (!token) return { status: "signed-out", message: "Sign in to Allternit Cloud to see plan usage." }
  const headers = { Authorization: `Bearer ${token}`, Accept: "application/json" }
  const origin = allternitOrigin()
  try {
    const res = await fetch(`${origin}/api/v1/me/usage`, { headers, signal: AbortSignal.timeout(8000) })
    let windows = res.ok ? parseAllternitUsage(await res.json().catch(() => undefined)) : []
    if (windows.length === 0) {
      const gw = await fetch(`${origin}/v1/rate-limits`, { headers, signal: AbortSignal.timeout(8000) }).catch(() => undefined)
      if (gw?.ok) windows = parseGatewayRateLimits(await gw.json().catch(() => undefined))
      else if (res.status === 401 || res.status === 403) {
        return { status: "signed-out", message: "Allternit Cloud rejected the configured API key." }
      }
    }
    return { status: "ok", quota: { providerID: "allternit", source: "Allternit Cloud", windows, fetchedAt: Date.now() } }
  } catch (err) {
    log.warn("allternit quota fetch failed", { error: err instanceof Error ? err.message : String(err) })
    return { status: "error", message: "Couldn't reach Allternit Cloud to read plan usage." }
  }
}

// ── Response headers (live, per request) ───────────────────────────────────
// Subscription-backed providers report window usage on every response:
// Anthropic's unified 5h/7d headers (Claude plans), Codex's primary /
// secondary windows (ChatGPT plans). Recorded as they arrive so a running
// turn sees its usage between steps, not a minute-old read. Per-minute
// request/token rate limits (`x-ratelimit-*`) are deliberately not windows —
// they reset in seconds and stay ordinary retries.
function epochSecondsToISO(value: string | null): string | undefined {
  const n = value === null ? NaN : Number(value)
  return Number.isFinite(n) && n > 0 ? new Date(n < 1e12 ? n * 1000 : n).toISOString() : undefined
}

function headerRatio(value: string | null, percent = false): number | undefined {
  const n = value === null ? NaN : Number(value)
  if (!Number.isFinite(n)) return
  const r = percent || n > 1 ? n / 100 : n
  return Math.min(1, Math.max(0, r))
}

export function parseLimitHeaders(headers: Headers, now = Date.now()): QuotaWindow[] {
  const out: QuotaWindow[] = []
  for (const [abbrev, id, label] of [
    ["5h", "5h", "5-hour"],
    ["7d", "7d", "Weekly"],
    ["7d_opus", "7d-opus", "Weekly Opus"],
    ["7d_sonnet", "7d-sonnet", "Weekly Sonnet"],
  ] as const) {
    const used = headerRatio(headers.get(`anthropic-ratelimit-unified-${abbrev}-utilization`))
    if (used === undefined) continue
    const resetAt = epochSecondsToISO(headers.get(`anthropic-ratelimit-unified-${abbrev}-reset`))
    out.push({ id, label, usedRatio: used, ...(resetAt ? { resetAt } : {}) })
  }
  // A rejected/warning status names the binding claim even without utilization headers.
  if (out.length === 0) {
    const status = headers.get("anthropic-ratelimit-unified-status")
    const claim = headers.get("anthropic-ratelimit-unified-representative-claim") ?? ""
    const resetAt = epochSecondsToISO(headers.get("anthropic-ratelimit-unified-reset"))
    if ((status === "rejected" || status === "allowed_warning") && resetAt) {
      const weekly = claim.startsWith("seven_day")
      const model = claim === "seven_day_opus" ? "-opus" : claim === "seven_day_sonnet" ? "-sonnet" : ""
      out.push({
        id: weekly ? `7d${model}` : "5h",
        label: weekly ? `Weekly${model === "-opus" ? " Opus" : model === "-sonnet" ? " Sonnet" : ""}` : "5-hour",
        usedRatio: status === "rejected" ? 1 : 0.95,
        resetAt,
      })
    }
  }
  for (const [key, fallback] of [["primary", "5-hour"], ["secondary", "Weekly"]] as const) {
    const used = headerRatio(headers.get(`x-codex-${key}-used-percent`), true)
    if (used === undefined) continue
    const minutes = Number(headers.get(`x-codex-${key}-window-minutes`))
    const label = minutes === 300 ? "5-hour" : minutes === 10080 ? "Weekly" : fallback
    const after = Number(headers.get(`x-codex-${key}-reset-after-seconds`))
    const resetAt =
      epochSecondsToISO(headers.get(`x-codex-${key}-reset-at`)) ??
      (Number.isFinite(after) && after > 0 ? new Date(now + after * 1000).toISOString() : undefined)
    out.push({ id: label === "Weekly" ? "7d" : "5h", label, usedRatio: used, ...(resetAt ? { resetAt } : {}) })
  }
  return out
}

const live = new Map<string, { at: number; windows: QuotaWindow[] }>()
/** Header reads older than this are dropped (the window may have rolled). */
const LIVE_MS = 6 * 60 * 60_000

/** Merge `extra` into `base` by window id; `extra` wins. */
function mergeWindows(base: QuotaWindow[], extra: QuotaWindow[]): QuotaWindow[] {
  const byId = new Map(base.map((w) => [w.id, w]))
  for (const w of extra) byId.set(w.id, w)
  return [...byId.values()]
}

function withLive(providerID: string, result: QuotaResult | undefined, now = Date.now()): QuotaResult | undefined {
  const hit = live.get(providerID)
  const fresh = hit && now - hit.at < LIVE_MS ? hit.windows.filter((w) => !w.resetAt || Date.parse(w.resetAt) > now) : []
  if (fresh.length === 0) return result
  if (result?.status === "ok") {
    return { status: "ok", quota: { ...result.quota, windows: mergeWindows(result.quota.windows, fresh) } }
  }
  return { status: "ok", quota: { providerID, source: providerID, windows: fresh, fetchedAt: hit!.at } }
}

const FETCHERS: Record<string, Fetcher> = {
  "kimi-cli": kimi,
  openrouter,
  "codex-cli": codex,
  "claude-cli": claude,
  allternit,
}

export namespace ProviderQuotas {
  export async function get(providerID: string): Promise<QuotaResult> {
    const fetcher = FETCHERS[providerID]
    if (!fetcher) return withLive(providerID, undefined) ?? { status: "unsupported" }
    const hit = cache.get(providerID)
    if (hit && Date.now() - hit.at < CACHE_MS) return withLive(providerID, hit.result)!
    const result = await fetcher()
    cache.set(providerID, { at: Date.now(), result })
    return withLive(providerID, result)!
  }

  /**
   * What is already known, without fetching: the last read plus any live
   * header windows. For a running turn that must not wait on the network.
   */
  export function peek(providerID: string): QuotaResult | undefined {
    return withLive(providerID, cache.get(providerID)?.result)
  }

  /** Record windows a provider reported on a response (see parseLimitHeaders). */
  export function record(providerID: string, windows: QuotaWindow[], now = Date.now()): void {
    if (windows.length === 0) return
    const prev = live.get(providerID)
    live.set(providerID, { at: now, windows: mergeWindows(prev && now - prev.at < LIVE_MS ? prev.windows : [], windows) })
  }

  /** Record limit windows from a response's headers, if it carries any. */
  export function recordHeaders(providerID: string, headers: Headers): void {
    try {
      record(providerID, parseLimitHeaders(headers))
    } catch (err) {
      log.warn("limit header parse failed", { providerID, error: err instanceof Error ? err.message : String(err) })
    }
  }

  export function supported(): string[] {
    return Object.keys(FETCHERS)
  }

  /** Test/maintenance hook: drop every cached read. */
  export function clearCache(): void {
    cache.clear()
    live.clear()
  }
}
