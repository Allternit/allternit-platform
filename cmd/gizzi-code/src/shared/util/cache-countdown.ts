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
