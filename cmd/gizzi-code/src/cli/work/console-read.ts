/**
 * Read-only fetches shared by the TUI `/computers` view and the agent tools.
 * Nothing here changes a computer or wakes one: an asleep computer is answered
 * from its last sample without calling its host routes.
 */
import { Asleep, hostGet, listConsole, resolveRow, streamLogs, type ConsoleRow, type HostPort, type HostService, type HostSnapshot, type LogLine } from "@/cli/work/console"
import { boundLogs, clampLines, isAsleep, MAX_ROWS } from "@/cli/work/console-view"

export type HostDetail = { row: ConsoleRow; asleep: boolean; snapshot?: HostSnapshot; services: HostService[]; ports: HostPort[] }

export async function loadRows(): Promise<ConsoleRow[]> {
  return (await listConsole()).slice(0, MAX_ROWS)
}

export async function loadDetail(row: ConsoleRow): Promise<HostDetail> {
  if (isAsleep(row)) return { row, asleep: true, snapshot: row.lastSample, services: [], ports: [] }
  try {
    const [snapshot, services, ports] = await Promise.all([
      hostGet<HostSnapshot>(row.id, "snapshot"),
      hostGet<{ services?: HostService[] }>(row.id, "services").then((b) => b.services ?? []),
      hostGet<{ ports?: HostPort[] }>(row.id, "ports").then((b) => b.ports ?? []),
    ])
    return { row, asleep: false, snapshot, services, ports }
  } catch (e) {
    if (e instanceof Asleep) return { row, asleep: true, snapshot: e.lastSample ?? row.lastSample, services: [], ports: [] }
    throw e
  }
}

/** The most recent `lines` log lines for a unit (bounded; takes what the stream replays in 8 s). */
export async function tailLogs(key: string, unit: string, lines: unknown, signal?: AbortSignal): Promise<LogLine[]> {
  const row = await resolveRow(key)
  if (isAsleep(row)) throw new Asleep(row.lastSample)
  const limit = clampLines(lines)
  const ctl = new AbortController()
  const stop = () => ctl.abort()
  signal?.addEventListener("abort", stop, { once: true })
  const timer = setTimeout(stop, 8_000)
  const got: LogLine[] = []
  try {
    for await (const l of streamLogs(row.id, unit, ctl.signal)) {
      got.push(l)
      if (got.length >= limit) break
    }
  } catch (e) {
    if (!ctl.signal.aborted) throw e
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener("abort", stop)
    ctl.abort()
  }
  return boundLogs(got, limit)
}
