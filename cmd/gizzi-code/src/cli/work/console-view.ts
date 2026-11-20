/**
 * Pure helpers for the Computers console views (TUI `/computers` and the
 * agent read tools): grouping, "Needs attention", meters, and the bounds on
 * how much a tool may return. No network and no wake: an asleep computer is
 * always shown from its last sample.
 */
import { bytes, type ConsoleRow, type HostSnapshot, type LogLine } from "@/cli/work/console"

export const ATTENTION_GROUP = "Needs attention"
export const MAX_LOG_LINES = 200
export const DEFAULT_LOG_LINES = 50
export const MAX_ROWS = 100
export const MAX_LINE_CHARS = 400

export function isAsleep(row: ConsoleRow): boolean {
  return /^(asleep|sleeping|stopped|suspended|hibernat)/i.test(row.status ?? "")
}

function pct(used: number, total: number): number {
  return total > 0 ? (used / total) * 100 : 0
}

/** Why a computer needs attention. Same rule as web, iOS and Android: status offline or error, or CPU, memory or disk at 90% or more from the last sample. Asleep is never flagged. Empty = fine. */
export function attentionReasons(row: ConsoleRow): string[] {
  const out: string[] = []
  if (/^(offline|error)$/i.test((row.status ?? "").trim())) out.push(`status ${row.status}`)
  const s = row.lastSample
  if (s && !isAsleep(row)) {
    const worstDisk = s.disk.reduce<{ mount: string; p: number } | undefined>((w, d) => {
      const p = pct(d.usedBytes, d.totalBytes)
      return !w || p > w.p ? { mount: d.mount, p } : w
    }, undefined)
    if (worstDisk && worstDisk.p >= 90) out.push(`disk ${worstDisk.mount} ${Math.round(worstDisk.p)}% full`)
    const mem = pct(s.memory.usedBytes, s.memory.totalBytes)
    if (mem >= 90) out.push(`memory ${Math.round(mem)}% used`)
    if (s.cpu.percent >= 90) out.push(`cpu ${Math.round(s.cpu.percent)}%`)
  }
  return out
}

export type RowGroup = { name: string; rows: ConsoleRow[] }

/** "Needs attention" first, then the account's groups (by name), then ungrouped by kind. Rows keep their order inside a group. */
export function groupRows(rows: ConsoleRow[]): RowGroup[] {
  const attention: ConsoleRow[] = []
  const byName = new Map<string, ConsoleRow[]>()
  for (const r of rows) {
    if (attentionReasons(r).length) {
      attention.push(r)
      continue
    }
    const key = r.group?.trim() || (r.kind ? r.kind[0].toUpperCase() + r.kind.slice(1) : "Other")
    byName.set(key, [...(byName.get(key) ?? []), r])
  }
  const named = [...byName.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([name, rs]) => ({ name, rows: rs }))
  return attention.length ? [{ name: ATTENTION_GROUP, rows: attention }, ...named] : named
}

/** Rows in display order (the order the selection moves through). */
export function flatRows(groups: RowGroup[]): ConsoleRow[] {
  return groups.flatMap((g) => g.rows)
}

/** `███░░░░░` style meter; width cells, value clamped to 0..100. */
export function meter(percent: number, width = 8): string {
  const p = Number.isFinite(percent) ? Math.max(0, Math.min(100, percent)) : 0
  const on = Math.round((p / 100) * width)
  return "█".repeat(on) + "░".repeat(width - on)
}

/** One-line meters from the last sample, e.g. `cpu ██░░ 24%  mem ███░ 61%  disk █░░░ 18%`. */
export function meterLine(s: HostSnapshot | undefined): string {
  if (!s) return "no sample yet"
  const mem = pct(s.memory.usedBytes, s.memory.totalBytes)
  const disk = s.disk.reduce((m, d) => Math.max(m, pct(d.usedBytes, d.totalBytes)), 0)
  const cell = (label: string, p: number) => `${label} ${meter(p, 6)} ${Math.round(p)}%`
  return [cell("cpu", s.cpu.percent), cell("mem", mem), cell("disk", disk)].join("  ")
}

/** Clamp a requested log line count to 1..MAX_LOG_LINES (default when missing or not a number). */
export function clampLines(n: unknown): number {
  const v = typeof n === "number" ? n : Number(n)
  if (!Number.isFinite(v)) return DEFAULT_LOG_LINES
  return Math.max(1, Math.min(MAX_LOG_LINES, Math.floor(v)))
}

/** Keep the last `limit` lines, each cut to MAX_LINE_CHARS. */
export function boundLogs(lines: LogLine[], limit: unknown): LogLine[] {
  return lines.slice(-clampLines(limit)).map((l) => (l.message.length > MAX_LINE_CHARS ? { ...l, message: l.message.slice(0, MAX_LINE_CHARS) + "…" } : l))
}

/** Compact JSON-ready row for agent tools: no more than the contract's list fields. */
export function summarizeRow(row: ConsoleRow) {
  return {
    id: row.id,
    name: row.name,
    kind: row.kind,
    group: row.group,
    status: row.status ?? "unknown",
    asleep: isAsleep(row),
    attention: attentionReasons(row),
    servicesCount: row.servicesCount,
    lastBackupAt: row.lastBackupAt,
    lastSample: row.lastSample && {
      sampledAt: row.lastSample.sampledAt,
      cpuPercent: Math.round(row.lastSample.cpu.percent),
      memory: `${bytes(row.lastSample.memory.usedBytes)} of ${bytes(row.lastSample.memory.totalBytes)}`,
    },
  }
}
