/**
 * Pure run-telemetry logic for the TUI — no Ink, no React, no runtime
 * imports. Ported from the desktop RunTelemetry (allternit-ai
 * src/components/chat/RunTelemetry.tsx): the compact/seconds/cost/resetsIn
 * formatters, the context block bar, quota-window selection, and the
 * never-fabricate segment assembly (a segment whose data is absent is
 * omitted, never zero-filled). Kept dependency-free so it is unit-testable
 * on its own.
 */

/** Loose QuotaWindow shape — mirrored from src/runtime/providers/quota. */
export interface TelemetryQuotaWindow {
  /** Stable id: "5h" | "7d" | "month" | … */
  id: string
  label: string
  /** 0–1 share of the window already used. */
  usedRatio: number
  /** ISO time the window resets, when the provider says. */
  resetAt?: string
}

/** Loose QuotaResult shape — mirrored from src/runtime/providers/quota. */
export type TelemetryQuotaResult =
  | { status: 'ok'; quota: { windows: TelemetryQuotaWindow[] } }
  | { status: 'unsupported' }
  | { status: 'signed-out' | 'expired' | 'error'; message: string }

/** Everything one per-turn telemetry line can show; all segments optional. */
export interface RunTelemetryInput {
  model?: string
  durationMs?: number
  inputTokens?: number
  outputTokens?: number
  /** The turn's token counts were estimated (provider reported none). */
  usageEstimated?: boolean
  toolCount?: number
  costUSD?: number
  /** 0–1 share of the context window in use after the turn. */
  contextRatio?: number
  contextEstimated?: boolean
  /** Pre-rendered quota chip (see quotaWindowChip); patched in async. */
  quotaChip?: string
  /** Files the turn changed (Edit/Write results). */
  files?: { path: string; added: number; removed: number }[]
  /** The turn failed or produced nothing; carries the short cause. */
  stopped?: { cause?: string }
}

/** 1234 → "1.2k", 1234567 → "1.2M". Ported from the desktop compact(). */
export function formatCompactTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`
  return String(n)
}

/** 4321 → "4.3s", 45000 → "45s", 125000 → "2m 5s". Ported from seconds(). */
export function formatTelemetrySeconds(ms: number): string {
  const s = ms / 1000
  if (s < 10) return `${s.toFixed(1)}s`
  if (s < 60) return `${Math.round(s)}s`
  const m = Math.floor(s / 60)
  return `${m}m ${Math.round(s % 60)}s`
}

/** Sub-cent costs keep 4 decimals so they don't read as $0.00. */
export function formatTelemetryCost(usd: number): string {
  return usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`
}

/** "3h 12m" / "45m" / "2d 4h" until an ISO reset; undefined when past/absent. */
export function formatResetsIn(
  iso: string | undefined,
  nowMs: number = Date.now(),
): string | undefined {
  if (!iso) return undefined
  const ms = Date.parse(iso) - nowMs
  if (!Number.isFinite(ms) || ms <= 0) return undefined
  const mins = Math.round(ms / 60_000)
  if (mins < 60) return `${mins}m`
  const hours = Math.floor(mins / 60)
  if (hours < 48) return `${hours}h ${mins % 60}m`
  return `${Math.floor(hours / 24)}d ${hours % 24}h`
}

/** Terminal-native block bar: 0.62 of 10 → "██████░░░░". */
export function contextBlockBar(ratio: number, count: number = 10): string {
  const filled = Math.max(
    ratio > 0 ? 1 : 0,
    Math.round(Math.min(1, ratio) * count),
  )
  return '█'.repeat(Math.min(filled, count)) + '░'.repeat(Math.max(0, count - filled))
}

/** Tightest (most-used) reported window, or null — never a fabricated one. */
export function pickTightestQuotaWindow(
  windows: TelemetryQuotaWindow[],
): TelemetryQuotaWindow | null {
  if (!windows.length) return null
  return windows.reduce((a, b) => (b.usedRatio > a.usedRatio ? b : a))
}

/** Short chip label for a window id: "5h" → 5h, "7d" → week, else the id. */
function quotaWindowLabel(id: string): string {
  if (id === '5h') return '5h'
  if (id === '7d') return 'week'
  return id
}

/** "week 20% left" for one window. */
export function quotaWindowChip(window: TelemetryQuotaWindow): string {
  const left = Math.round((1 - window.usedRatio) * 100)
  return `${quotaWindowLabel(window.id)} ${left}% left`
}

/** Chip for the tightest window of an ok result, else null. */
export function quotaChipFromResult(
  result: TelemetryQuotaResult | undefined,
): string | null {
  if (!result || result.status !== 'ok') return null
  const tightest = pickTightestQuotaWindow(result.quota.windows)
  return tightest ? quotaWindowChip(tightest) : null
}

/**
 * Explicit marker for providers with no quota API at all — shown instead of
 * an empty slot so a missing number is never mistaken for "everything fine".
 */
export const QUOTA_NA_CHIP = 'quota n/a'

/**
 * Assemble the one-line per-turn summary:
 *   model · 12.3s · ~1.2k in / ~345 out est. · 3 tools · $0.0123 ·
 *   ctx ██████░░░░ 58% · week 20% left
 * Segments join on " · "; absent data omits the segment. Returns null when
 * there is nothing to say.
 */
export function buildRunTelemetryLine(input: RunTelemetryInput): string | null {
  const segments: string[] = []
  if (input.model) segments.push(input.model)
  if (input.durationMs !== undefined && input.durationMs > 0) {
    segments.push(formatTelemetrySeconds(input.durationMs))
  }
  const inTok = input.inputTokens ?? 0
  const outTok = input.outputTokens ?? 0
  if (inTok + outTok > 0) {
    const approx = input.usageEstimated ? '~' : ''
    segments.push(
      `${approx}${formatCompactTokens(inTok)} in / ${approx}${formatCompactTokens(outTok)} out${input.usageEstimated ? ' est.' : ''}`,
    )
  }
  if (input.toolCount !== undefined && input.toolCount > 0) {
    segments.push(`${input.toolCount} ${input.toolCount === 1 ? 'tool' : 'tools'}`)
  }
  if (input.costUSD !== undefined && input.costUSD > 0) {
    segments.push(formatTelemetryCost(input.costUSD))
  }
  if (input.contextRatio !== undefined && input.contextRatio > 0) {
    const bar = contextBlockBar(input.contextRatio)
    const pct = Math.round(input.contextRatio * 100)
    segments.push(
      `ctx ${bar} ${input.contextEstimated ? '~' : ''}${pct}%`,
    )
  }
  if (input.quotaChip) segments.push(input.quotaChip)
  return segments.length > 0 ? segments.join(' · ') : null
}
