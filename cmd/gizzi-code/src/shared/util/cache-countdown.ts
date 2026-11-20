/** Request cache telemetry. Expiry is a TTL estimate, not a server guarantee. */
export interface CacheCountdownInput {
  cacheTtlSeconds?: number
  cacheExpiresAt?: number
  cacheRecacheTokens?: number
  cacheReadTokens?: number
  cacheWriteTokens?: number
  inputTokens?: number
}

export function cacheCountdown(input: CacheCountdownInput | undefined, now: number) {
  const ttl = input?.cacheTtlSeconds
  const expiry = input?.cacheExpiresAt
  if (!input || !ttl || !expiry || !Number.isFinite(ttl) || !Number.isFinite(expiry) || ttl <= 0) return undefined
  const left = Math.max(0, Math.min(ttl, (expiry - now) / 1000))
  const fraction = left / ttl
  const state = left === 0 ? "cold" : fraction < 0.2 ? "expiring" : "warm"
  const filled = Math.ceil(fraction * 6)
  const bar = "█".repeat(filled) + "░".repeat(6 - filled)
  const tokens = input.cacheRecacheTokens
  const recache = typeof tokens === "number" && Number.isFinite(tokens) && tokens > 0
    ? ` · may re-cache ${(tokens / 1000).toFixed(tokens < 10000 ? 1 : 0)}k tokens` : ""
  const read = input.cacheReadTokens
  const total = (input.inputTokens ?? 0) + (read ?? 0) + (input.cacheWriteTokens ?? 0)
  const hit = typeof read === "number" && total > 0 ? ` · last hit ${Math.round(read / total * 100)}%` : ""
  const time = left < 60 ? `${Math.ceil(left)}s` : `${Math.ceil(left / 60)}m`
  const ttlLabel = ttl >= 3600 ? `${Math.round(ttl / 3600)}h` : `${Math.round(ttl / 60)}m`
  const label = state === "cold"
    ? `cache ○ cold (est.)${recache}`
    : `cache ● ${ttlLabel} ${bar} ~${time} left${hit}`
  return { state, fraction, label }
}

/** Anthropic prompt-cache lifetimes: the default ephemeral TTL and the one-hour opt-in. */
export const PROMPT_CACHE_TTL_S = 300
export const PROMPT_CACHE_1H_TTL_S = 3600

/**
 * The prompt-cache TTL a request source gets, in seconds: one hour when the
 * user is eligible for 1h caching and the source matches the allowlist
 * (entries ending in `*` are prefixes), else five minutes. Same rule as the
 * TUI's `should1hCacheTTL` (services/api/claude.ts); shared by the cache
 * countdown and the Sleep idle timer.
 */
export function promptCacheTtlSeconds(input: {
  eligible: boolean | null | undefined
  allowlist: readonly string[] | null | undefined
  source: string
}): number {
  if (input.eligible !== true) return PROMPT_CACHE_TTL_S
  const oneHour = (input.allowlist ?? []).some((p) =>
    p.endsWith("*") ? input.source.startsWith(p.slice(0, -1)) : p === input.source,
  )
  return oneHour ? PROMPT_CACHE_1H_TTL_S : PROMPT_CACHE_TTL_S
}
