/**
 * Allternit service status: fetch, parse and summarise the public status feed
 * (https://status.allternit.com/api/v1/status.json, contract allternit.status/v1).
 * Shared by `gizzi status --services`, `gizzi doctor` and the TUI /status model.
 */

export const DEFAULT_STATUS_URL = "https://status.allternit.com/api/v1/status.json"
export const STATUS_TIMEOUT_MS = 5_000

export type ServiceState = "operational" | "degraded" | "down" | "maintenance" | "unknown"

export type ServiceComponent = { id: string; name: string; status: ServiceState; lastChange: string | null }
export type ServiceIncident = { id: string; title: string; impact: string; componentIds: string[] }

export type ServiceStatus = {
  overall: ServiceState
  lastCheckedAt: string
  components: ServiceComponent[]
  incidents: ServiceIncident[]
}

const STATES: ServiceState[] = ["operational", "degraded", "down", "maintenance", "unknown"]

export function statusUrl(env: NodeJS.ProcessEnv = process.env): string {
  return env.ALLTERNIT_STATUS_URL?.trim() || DEFAULT_STATUS_URL
}

const asState = (v: unknown): ServiceState => (STATES.includes(v as ServiceState) ? (v as ServiceState) : "unknown")

/** Parse a raw payload. Throws a plain Error when it is not an allternit.status/v1 document. */
export function parseServiceStatus(raw: unknown): ServiceStatus {
  const doc = raw as Record<string, any> | null
  if (!doc || typeof doc !== "object" || doc.schema !== "allternit.status/v1" || !Array.isArray(doc.components)) {
    throw new Error("not an allternit.status/v1 document")
  }
  return {
    overall: asState(doc.overall),
    lastCheckedAt: typeof doc.last_checked_at === "string" ? doc.last_checked_at : "",
    components: doc.components.map((c: any) => ({
      id: String(c.id),
      name: String(c.name ?? c.id),
      status: asState(c.status),
      lastChange: typeof c.last_change === "string" ? c.last_change : null,
    })),
    incidents: (Array.isArray(doc.incidents) ? doc.incidents : [])
      .filter((i: any) => i && i.state !== "resolved")
      .map((i: any) => ({
        id: String(i.id),
        title: String(i.title ?? ""),
        impact: String(i.impact ?? ""),
        componentIds: Array.isArray(i.component_ids) ? i.component_ids.map(String) : [],
      })),
  }
}

export type FetchedServiceStatus = { raw: unknown; status: ServiceStatus }

/** Fetch + parse. Rejects with a one-line Error on network, HTTP, JSON or schema failure. */
export async function fetchServiceStatus(
  opts: { url?: string; timeoutMs?: number; fetchImpl?: typeof fetch } = {},
): Promise<FetchedServiceStatus> {
  const url = opts.url ?? statusUrl()
  const f = opts.fetchImpl ?? fetch
  let res: Response
  try {
    res = await f(url, { signal: AbortSignal.timeout(opts.timeoutMs ?? STATUS_TIMEOUT_MS), headers: { accept: "application/json" } })
  } catch (e) {
    const timedOut = e instanceof Error && (e.name === "TimeoutError" || e.name === "AbortError")
    throw new Error(timedOut ? "timed out" : e instanceof Error ? e.message : String(e))
  }
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  let raw: unknown
  try {
    raw = await res.json()
  } catch {
    throw new Error("response was not JSON")
  }
  return { raw, status: parseServiceStatus(raw) }
}

/** 0 = all operational, 1 = degraded or maintenance, 2 = down or unknown. */
export function exitCodeFor(s: ServiceStatus): 0 | 1 | 2 {
  const states = [s.overall, ...s.components.map((c) => c.status)]
  if (states.includes("down") || states.includes("unknown")) return 2
  if (states.includes("degraded") || states.includes("maintenance")) return 1
  return 0
}

const SYMBOL: Record<ServiceState, string> = {
  operational: "● operational",
  degraded: "▲ degraded",
  down: "■ down",
  maintenance: "◆ maintenance",
  unknown: "? unknown",
}
export const stateLabel = (s: ServiceState): string => SYMBOL[s]

/** Aligned table: component, status (symbol + text), open incident titles. Footer carries last checked. */
export function renderServiceTable(s: ServiceStatus): string {
  const rows = s.components.map((c) => {
    const titles = s.incidents.filter((i) => i.componentIds.includes(c.id)).map((i) => i.title)
    return [c.name, stateLabel(c.status), titles.join("; ") || "-"]
  })
  const head = ["COMPONENT", "STATUS", "OPEN INCIDENTS"]
  const all = [head, ...rows]
  const w = [0, 1].map((i) => Math.max(...all.map((r) => r[i].length)))
  const line = (r: string[]) => `${r[0].padEnd(w[0])}  ${r[1].padEnd(w[1])}  ${r[2]}`.trimEnd()
  return [...all.map(line), "", `Overall: ${stateLabel(s.overall)}   Last checked: ${s.lastCheckedAt || "unknown"}`].join("\n")
}

/** One-line summary for doctor and the TUI. */
export function summarizeServices(s: ServiceStatus): string {
  const bad = s.components.filter((c) => c.status !== "operational")
  if (bad.length === 0) return "all operational"
  return bad.map((c) => `${c.name} ${c.status}`).join(", ")
}
