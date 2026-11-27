/**
 * Start a session on this computer or another one (Start anywhere, D8),
 * shared by the StartSession tool and `gizzi start`.
 *
 * - Devices: cloud `GET /api/v1/start-options` + `GET /api/v1/runtime-presence`.
 * - This computer: `POST /api/v1/session-start` on allternit-api (8013) when it
 *   answers, else the engine's own route.
 * - Another computer: the cloud relay `POST /api/v1/runtime-devices/:id/proxy`
 *   with the Allternit API key (ALLTERNIT_API_TOKEN, `compute` scope), the same
 *   auth `gizzi run --on` uses; the device token from `gizzi pair` is refused there.
 */
import { randomUUID } from "node:crypto"
import { hostname } from "node:os"
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"
import { cloudBase, engineBase, listRuntimes, pick, relayKeyProblem, type Runtime } from "@/cli/work/api"
import { type StartMode } from "@/cli/work/start-guards"

export type StartDevice = {
  runtimeId: string
  name: string
  kind: string
  remoteStart: boolean
  online: boolean
  lastSeen?: string
  folders: { path: string; name: string }[]
  modes: Record<string, { default: string; supported: string[] }>
}

export type StartResult = {
  kind: "engine" | "terminal"
  id: string
  directory: string
  device: string
  runtime: string
  line: string
}

/** Depth and parent of the session this process runs in (set by whoever started it; 0 = top level). */
export function parentDepth(): number {
  const n = Number(process.env.ALLTERNIT_SESSION_START_DEPTH)
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : 0
}

async function authHeaders(): Promise<Record<string, string>> {
  const env = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (env) return { Authorization: `Bearer ${env}` }
  const { Pairing } = await import("@/runtime/services/pairing/pairing")
  const device = await Pairing.load().catch(() => undefined)
  return Pairing.tokenUsable(device) ? { Authorization: `Bearer ${device!.deviceToken!}` } : {}
}

async function cloudGet<T>(path: string): Promise<T> {
  const res = await fetch(`${cloudBase()}${path}`, { headers: await authHeaders(), signal: AbortSignal.timeout(10_000) })
  if (res.status === 401 || res.status === 403) throw new Error("Not signed in. Run `gizzi login`.")
  if (!res.ok) throw new Error(`${path}: ${res.status}`)
  return (await res.json()) as T
}

/** Every computer on the account with its folders, whether it is online and when it was last seen. */
export async function listStartDevices(): Promise<StartDevice[]> {
  const [options, presence, runtimes] = await Promise.all([
    cloudGet<{ devices?: any[] }>("/api/v1/start-options"),
    cloudGet<any>("/api/v1/runtime-presence").catch(() => ({})),
    listRuntimes().catch(() => [] as Runtime[]),
  ])
  const rows: any[] = Array.isArray(presence) ? presence : (presence.presence ?? presence.runtimes ?? [])
  return (options.devices ?? []).map((d) => {
    const p = rows.find((r) => r.runtimeId === d.runtimeId || r.id === d.runtimeId)
    const rt = runtimes.find((r) => r.id === d.runtimeId)
    const status = String(p?.status ?? rt?.status ?? "").toLowerCase()
    return {
      runtimeId: d.runtimeId,
      name: d.name ?? rt?.name ?? d.runtimeId,
      kind: d.kind ?? p?.kind ?? "desktop",
      remoteStart: p?.remoteStart ?? d.remoteStart ?? true,
      online: p?.online ?? (status === "online" || status === "connected"),
      lastSeen: p?.lastSeenAt ?? rt?.lastSeenAt ?? d.updatedAt,
      folders: d.folders ?? [],
      modes: d.modes ?? {},
    }
  })
}

/** "online" or "asleep · last seen 5m ago". */
export function presenceText(d: Pick<StartDevice, "online" | "lastSeen">, now = Date.now()): string {
  if (d.online) return "online"
  if (!d.lastSeen) return "asleep"
  const mins = Math.max(0, Math.round((now - new Date(d.lastSeen).getTime()) / 60_000))
  const ago = mins < 1 ? "just now" : mins < 60 ? `${mins}m ago` : mins < 1440 ? `${Math.round(mins / 60)}h ago` : `${Math.round(mins / 1440)}d ago`
  return `asleep · last seen ${ago}`
}

export function formatStartDevices(devices: StartDevice[]): string {
  if (!devices.length) return "No computers yet. Pair one with `gizzi pair`."
  return devices.map((d) => `${d.name}  ${presenceText(d)}  ${d.kind}${d.remoteStart ? "" : "  (remote start off)"}`).join("\n")
}

const ERRORS: Record<string, string> = {
  remote_start_off: "Remote start is turned off on that computer. Turn it on in Settings there, or start the session there.",
  mode_not_allowed: "That permission mode is higher than you are allowed to grant.",
  depth_exceeded: "Sessions started from sessions can only go three levels deep.",
  folder_missing: "That folder no longer exists on that computer.",
  runtime_missing: "That coding tool is not installed on that computer.",
  runtime_signed_out: "That coding tool is signed out on that computer. Sign in there, then try again.",
  rate_limited: "That computer has started too many sessions in the last minute. Wait a minute, then try again.",
  clone_failed: "The cloud computer could not copy the repository.",
}

function explain(status: number, raw: string): Error {
  let code = ""
  try {
    code = JSON.parse(raw).error ?? ""
  } catch {}
  return new Error(ERRORS[code] ?? `Could not start the session (${status}${code ? ` ${code}` : ""}).`)
}

export type StartArgs = {
  /** Device name or runtime id; undefined = this computer. */
  device?: string
  folder: string
  runtime?: string
  mode: StartMode
  prompt?: string
  parentSessionId?: string
  depth: number
}

/** Resolve the device a name or id points at, or undefined for this computer. */
export async function resolveDevice(key: string | undefined): Promise<StartDevice | undefined> {
  if (!key) return undefined
  const devices = await listStartDevices()
  const found = pick(devices.map((d) => ({ ...d, id: d.runtimeId })), key)
  if (!found) throw new Error(`No computer named or with id "${key}". Run \`gizzi start --list\`.`)
  return found
}

function parseResult(json: any, device: string, runtime: string, folder: string): StartResult {
  if (json?.kind === "terminal") {
    const id = json.terminalId
    return { kind: "terminal", id, directory: json.cwd ?? folder, device, runtime, line: `Started ${json.agent ?? runtime} on ${device} in ${json.cwd ?? folder} (terminal ${id}).` }
  }
  const id = json?.sessionId
  if (!id) throw new Error("The computer did not return a session id.")
  return { kind: "engine", id, directory: json.directory ?? folder, device, runtime, line: `Started a ${runtime} session on ${device} in ${json.directory ?? folder} (session ${id}).` }
}

export async function startSession(args: StartArgs): Promise<StartResult> {
  const runtime = args.runtime ?? "allternit"
  const device = await resolveDevice(args.device)
  if (device && !device.remoteStart) throw new Error(ERRORS.remote_start_off)
  const deviceName = device?.name ?? hostname()
  const body = {
    folder: { path: args.folder },
    runtime,
    mode: args.mode,
    ...(args.prompt ? { prompt: args.prompt } : {}),
    origin: { surface: "gizzi", deviceName: hostname(), parentSessionId: args.parentSessionId, depth: args.depth },
  }
  const key = randomUUID()

  if (device) {
    const problem = relayKeyProblem()
    if (problem) throw new Error(problem)
    const res = await fetch(`${cloudBase()}/api/v1/runtime-devices/${encodeURIComponent(device.runtimeId)}/proxy`, {
      method: "POST",
      headers: { "content-type": "application/json", Authorization: `Bearer ${process.env.ALLTERNIT_API_TOKEN!.trim()}` },
      body: JSON.stringify({
        method: "POST",
        path: "/api/v1/session-start",
        headers: { "content-type": "application/json", "Idempotency-Key": key },
        body: JSON.stringify(body),
        bodyEncoding: "utf8",
      }),
      signal: AbortSignal.timeout(90_000),
    })
    const raw = await res.text()
    if (!res.ok) throw explain(res.status, raw)
    return parseResult(raw ? JSON.parse(raw) : {}, deviceName, runtime, args.folder)
  }

  // This computer: allternit-api first, the engine's own route when the gateway does not answer.
  const init = (token?: string): RequestInit => ({
    method: "POST",
    headers: { "content-type": "application/json", "Idempotency-Key": key, ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(30_000),
  })
  const token = (await authHeaders()).Authorization?.replace(/^Bearer /, "")
  let res: Response | undefined
  try {
    res = await fetch(`${ALLTERNIT_GATEWAY_BASE}/api/v1/session-start`, init(token))
    if (res.status === 404) res = undefined
  } catch {
    res = undefined
  }
  res ??= await fetch(`${engineBase()}/api/v1/session-start`, init()).catch(() => {
    throw new Error("Neither Allternit Desktop nor the Gizzi engine answered on this computer. Start one, or name another computer with --device.")
  })
  const raw = await res.text()
  if (!res.ok) throw explain(res.status, raw)
  return parseResult(raw ? JSON.parse(raw) : {}, deviceName, runtime, args.folder)
}
