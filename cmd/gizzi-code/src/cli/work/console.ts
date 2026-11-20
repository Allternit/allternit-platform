/**
 * Computers console from the terminal (contract:
 * surfaces/docs/dev/computers-console-contract.mdx). Reads go to the cloud
 * routes `GET /api/v1/computers/console` and `/api/v1/computers/:id/host/*`.
 * Until the backend is live a 404/501 is "not available yet": the commands
 * print one plain line and exit 0, never fake data.
 */
import { cloudBase, pick } from "@/cli/work/api"

export type ConsoleRow = {
  id: string
  kind: string
  name: string
  provider?: string
  region?: string
  os?: string
  status?: string
  lastSample?: HostSnapshot
  servicesCount?: number
  lastBackupAt?: string
}
export type HostSnapshot = {
  sampledAt: string
  uptimeSec: number
  os: { name: string; version: string; family: string; arch: string }
  cpu: { percent: number; cores: number; load1?: number }
  memory: { usedBytes: number; totalBytes: number }
  disk: { mount: string; usedBytes: number; totalBytes: number }[]
  net?: { rxBytesPerSec: number; txBytesPerSec: number }
}
export type HostService = { name: string; unit: string; state: string; enabled: boolean; pid?: number; memoryBytes?: number }
export type HostPort = { port: number; protocol: string; address: string; process?: string; pid?: number; exposure: string }
export type HostUpdate = { id: string; name: string; current?: string; available: string; security: boolean; kind: string }
export type LogLine = { ts: string; unit: string; level: string; message: string }

export const NOT_AVAILABLE = "Not available on your account yet."

/** The new subcommands show in `--help` only with GIZZI_COMPUTERS_CONSOLE=1 (no account flag is readable from the CLI yet). */
export function consoleEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  return /^(1|true|on)$/i.test((env.GIZZI_COMPUTERS_CONSOLE ?? "").trim())
}

export class Unavailable extends Error {
  constructor() {
    super(NOT_AVAILABLE)
  }
}
export class Asleep extends Error {
  constructor(public lastSample?: HostSnapshot) {
    super("This computer is asleep. Run `gizzi computers wake <computer>` to start it.")
  }
}

async function authHeaders(): Promise<Record<string, string>> {
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (env) return { Authorization: `Bearer ${env}` }
  const { Pairing } = await import("@/runtime/services/pairing/pairing")
  const device = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(device) ? { Authorization: `Bearer ${device!.deviceToken!}` } : {}
}

async function request(path: string, init: RequestInit = {}): Promise<Response> {
  const res = await fetch(`${cloudBase()}${path}`, {
    ...init,
    headers: { ...(await authHeaders()), ...(init.headers as Record<string, string>) },
    signal: init.signal ?? AbortSignal.timeout(15_000),
  })
  if (res.status === 404 || res.status === 501) throw new Unavailable()
  if (res.status === 401 || res.status === 403) throw new Error("Not signed in, or this computer is not yours. Run `gizzi login`.")
  if (res.status === 409) {
    const body = (await res.json().catch(() => ({}))) as { error?: string; lastSample?: HostSnapshot }
    if (body.error === "asleep") throw new Asleep(body.lastSample)
  }
  if (!res.ok) throw new Error(`${path}: ${res.status}`)
  return res
}

export async function listConsole(): Promise<ConsoleRow[]> {
  const body = (await (await request("/api/v1/computers/console")).json()) as ConsoleRow[] | { computers?: ConsoleRow[] }
  return Array.isArray(body) ? body : (body.computers ?? [])
}

export async function resolveRow(key: string): Promise<ConsoleRow> {
  const row = pick(await listConsole(), key)
  if (!row) throw new Error(`No computer named or with id "${key}". Run \`gizzi computers ls\`.`)
  return row
}

export async function hostGet<T>(id: string, path: string): Promise<T> {
  return (await (await request(`/api/v1/computers/${encodeURIComponent(id)}/host/${path}`)).json()) as T
}

/** Server-sent events: yields one parsed `LogLine` per `data:` line. */
export async function* streamLogs(id: string, unit: string, signal?: AbortSignal): AsyncGenerator<LogLine> {
  const res = await request(`/api/v1/computers/${encodeURIComponent(id)}/host/logs/stream?unit=${encodeURIComponent(unit)}`, {
    headers: { accept: "text/event-stream" },
    signal: signal ?? undefined,
  })
  if (!res.body) return
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ""
  for (;;) {
    const { done, value } = await reader.read()
    if (done) return
    buf += dec.decode(value, { stream: true })
    let nl: number
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).trim()
      buf = buf.slice(nl + 1)
      if (!line.startsWith("data:")) continue
      try {
        yield JSON.parse(line.slice(5).trim()) as LogLine
      } catch {}
    }
  }
}

// ── formatting ────────────────────────────────────────────────────────────

const pad = (s: string, n: number) => (s.length >= n ? s.slice(0, n - 1) + "…" : s.padEnd(n))

export function bytes(n: number): string {
  const u = ["B", "KB", "MB", "GB", "TB"]
  let i = 0
  let v = n
  while (v >= 1024 && i < u.length - 1) (v /= 1024), i++
  return `${i === 0 ? v : v.toFixed(v >= 10 ? 0 : 1)} ${u[i]}`
}

export function uptime(sec: number): string {
  const d = Math.floor(sec / 86400)
  const h = Math.floor((sec % 86400) / 3600)
  const m = Math.floor((sec % 3600) / 60)
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`
}

export function formatStatus(row: ConsoleRow, snap: HostSnapshot | undefined, asleep = false): string {
  const lines = [
    `${row.name} (${row.id})`,
    `  Kind:      ${row.kind}${row.provider ? ` on ${row.provider}` : ""}${row.region ? `, ${row.region}` : ""}`,
    `  Status:    ${asleep ? "asleep (last sample shown)" : (row.status ?? "unknown")}`,
  ]
  if (!snap) return [...lines, "  No health sample yet."].join("\n")
  lines.push(
    `  OS:        ${snap.os.name} ${snap.os.version} (${snap.os.arch})`,
    `  Uptime:    ${uptime(snap.uptimeSec)}`,
    `  CPU:       ${Math.round(snap.cpu.percent)}% of ${snap.cpu.cores} cores`,
    `  Memory:    ${bytes(snap.memory.usedBytes)} of ${bytes(snap.memory.totalBytes)}`,
    ...snap.disk.map((d) => `  Disk ${d.mount}: ${bytes(d.usedBytes)} of ${bytes(d.totalBytes)}`),
    `  Sampled:   ${snap.sampledAt}`,
  )
  return lines.join("\n")
}

export function formatServices(rows: HostService[]): string {
  if (!rows.length) return "No services found."
  return [`${pad("STATE", 9)} ${pad("UNIT", 38)} ${pad("ENABLED", 8)} MEMORY`]
    .concat(rows.map((s) => `${pad(s.state, 9)} ${pad(s.unit, 38)} ${pad(s.enabled ? "yes" : "no", 8)} ${s.memoryBytes ? bytes(s.memoryBytes) : "-"}`))
    .join("\n")
}

export function formatPorts(rows: HostPort[]): string {
  if (!rows.length) return "No listening ports found."
  return [`${pad("PORT", 7)} ${pad("PROTO", 6)} ${pad("EXPOSURE", 9)} ${pad("ADDRESS", 22)} PROCESS`]
    .concat(rows.map((p) => `${pad(String(p.port), 7)} ${pad(p.protocol, 6)} ${pad(p.exposure, 9)} ${pad(p.address, 22)} ${p.process ?? "-"}`))
    .join("\n")
}

export function formatUpdates(rows: HostUpdate[]): string {
  if (!rows.length) return "Up to date."
  return [`${pad("SECURITY", 9)} ${pad("NAME", 30)} ${pad("CURRENT", 16)} AVAILABLE`]
    .concat(rows.map((u) => `${pad(u.security ? "yes" : "no", 9)} ${pad(u.name, 30)} ${pad(u.current ?? "-", 16)} ${u.available}`))
    .join("\n")
}

export function formatLogLine(l: LogLine): string {
  return `${l.ts} ${pad(l.level, 5)} ${l.message}`
}
