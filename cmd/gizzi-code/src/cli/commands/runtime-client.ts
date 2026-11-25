/**
 * Client for the user's RUNTIME api (allternit-api: /api/v1/bar/*, /api/v1/team/*).
 * These routes do not exist on cloud-api, so `Flag.GIZZI_PLATFORM_API_URL` is the wrong base.
 *
 *  1. Local runtime first: ALLTERNIT_API_URL (Desktop sets it) or http://127.0.0.1:8013,
 *     used when `/health` answers. Set GIZZI_RUNTIME_VIA=cloud to skip this.
 *  2. Otherwise the cloud relay: pick a device from GET {cloud}/api/v1/runtime-devices, then
 *     POST {cloud}/api/v1/runtime-devices/:id/proxy with {method, path, headers, body}.
 *     The proxy streams the runtime's response, so SSE works through it. No device, or a
 *     428, is `no_runtime`.
 */
import { Flag } from "@/runtime/context/flag/flag"

const CLOUD = () => Flag.GIZZI_PLATFORM_API_URL.replace(/\/+$/, "")

export class RuntimeError extends Error {
  constructor(public status: number, public code: string | undefined, message: string, public data?: any) {
    super(message)
  }
}

export const NO_RUNTIME = "No computer is connected. Run `gizzi connect` to set one up, then try again."

export async function cloudToken(): Promise<string | undefined> {
  if (process.env.ALLTERNIT_API_TOKEN) return process.env.ALLTERNIT_API_TOKEN
  try {
    const fs = await import("fs")
    const path = await import("path")
    const os = await import("os")
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    return JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8"))?.accessToken || undefined
  } catch {
    return undefined
  }
}

/** Reads the four error body shapes (contract section 3). `friendly` maps a code to wording. */
export function parseApiError(
  status: number,
  raw: string,
  friendly: (status: number, code: string | undefined, message?: string) => string = (s, _c, m) => m ?? `Request failed (${s})`,
): RuntimeError {
  let body: any
  try {
    body = JSON.parse(raw)
  } catch {}
  const e = body?.error
  let code: string | undefined = typeof body?.code === "string" ? body.code : undefined
  let message: string | undefined
  if (e && typeof e === "object") {
    code = e.data?.code ?? e.code ?? code
    message = typeof e.message === "string" ? e.message : undefined
  } else if (typeof e === "string") {
    if (code) message = e
    else if (/^[a-z_]+$/.test(e)) code = e
    else message = e
  }
  if (typeof body?.message === "string") message = body.message
  if (status === 428) code = "no_runtime"
  return new RuntimeError(status, code, friendly(status, code, message), e?.data ?? body?.data ?? body)
}

let computerName: string | undefined
export function setComputer(name?: string) {
  computerName = name
}

let localBase: string | null | undefined
async function local(): Promise<string | null> {
  if (localBase !== undefined) return localBase
  localBase = null
  if (process.env.GIZZI_RUNTIME_VIA === "cloud") return localBase
  const base = (process.env.ALLTERNIT_API_URL || "http://127.0.0.1:8013").replace(/\/+$/, "")
  try {
    const r = await fetch(`${base}/health`, { signal: AbortSignal.timeout(700) })
    if (r.ok) localBase = base
  } catch {}
  return localBase
}

let deviceId: string | undefined
async function device(token: string): Promise<string> {
  if (deviceId) return deviceId
  let res: Response
  try {
    res = await fetch(`${CLOUD()}/api/v1/runtime-devices`, { headers: { Authorization: `Bearer ${token}`, Accept: "application/json" } })
  } catch {
    throw new RuntimeError(0, "no_runtime", NO_RUNTIME)
  }
  if (!res.ok) throw parseApiError(res.status, await res.text().catch(() => ""), (s, c, m) => (c === "no_runtime" ? NO_RUNTIME : m ?? `Request failed (${s})`))
  const list: any[] = ((await res.json()) as any)?.runtimes ?? []
  const online = list.filter((d) => d.status === "online" || d.wakeable)
  let pick: any
  if (computerName) {
    pick = online.find((d) => d.name?.toLowerCase() === computerName!.toLowerCase() || d.id === computerName)
    if (!pick) throw new RuntimeError(0, "no_runtime", `No online computer named "${computerName}". Online: ${online.map((d) => d.name).join(", ") || "none"}.`)
  } else {
    pick = online.find((d) => d.preferred) ?? (online.length === 1 ? online[0] : undefined)
    if (!pick && online.length > 1)
      throw new RuntimeError(0, "no_runtime", `More than one computer is online (${online.map((d) => d.name).join(", ")}). Choose one with --computer <name>.`)
  }
  if (!pick) throw new RuntimeError(428, "no_runtime", NO_RUNTIME)
  return (deviceId = pick.id)
}

/** One request to the runtime api. `path` is the full runtime path, e.g. `/api/v1/bar/apps`. Returns the raw Response. */
export async function runtimeFetch(
  method: string,
  path: string,
  opts: { body?: unknown; accept?: string; headers?: Record<string, string>; signal?: AbortSignal } = {},
): Promise<Response> {
  const t = await cloudToken()
  if (!t) throw new RuntimeError(401, undefined, "Not signed in. Run `gizzi auth login` first.")
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    Accept: opts.accept ?? "application/json",
    Authorization: `Bearer ${t}`,
    ...(opts.headers ?? {}),
  }
  const body = opts.body !== undefined ? JSON.stringify(opts.body) : undefined
  const base = await local()
  try {
    if (base) return await fetch(`${base}${path}`, { method, headers, body, signal: opts.signal })
    const id = await device(t)
    return await fetch(`${CLOUD()}/api/v1/runtime-devices/${encodeURIComponent(id)}/proxy`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: opts.accept ?? "application/json", Authorization: `Bearer ${t}` },
      body: JSON.stringify({ method, path, headers: { Accept: headers.Accept, "Content-Type": headers["Content-Type"], Authorization: headers.Authorization }, body: body ?? "" }),
      signal: opts.signal,
    })
  } catch (e: any) {
    if (e instanceof RuntimeError || e?.name === "AbortError") throw e
    throw new RuntimeError(0, "no_runtime", NO_RUNTIME)
  }
}

export async function runtimeJson<T>(
  method: string,
  path: string,
  body?: unknown,
  friendly?: (status: number, code: string | undefined, message?: string) => string,
  headers?: Record<string, string>,
): Promise<T> {
  const res = await runtimeFetch(method, path, { body, headers })
  if (!res.ok) throw parseApiError(res.status, await res.text().catch(() => ""), friendly)
  return (await res.json()) as T
}

/** Server-sent events as parsed JSON `data:` payloads. */
export async function* runtimeSse(path: string, signal: AbortSignal, friendly?: Parameters<typeof parseApiError>[2]): AsyncGenerator<any> {
  const res = await runtimeFetch("GET", path, { accept: "text/event-stream", signal })
  if (!res.ok || !res.body) throw parseApiError(res.status, await res.text().catch(() => ""), friendly)
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ""
  while (true) {
    const { value, done } = await reader.read()
    if (done) return
    buf += dec.decode(value, { stream: true })
    let i: number
    while ((i = buf.search(/\r?\n\r?\n/)) >= 0) {
      const block = buf.slice(0, i)
      buf = buf.slice(i).replace(/^\r?\n\r?\n/, "")
      const data = block.split(/\r?\n/).filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim()).join("\n")
      if (!data) continue
      try {
        yield JSON.parse(data)
      } catch {}
    }
  }
}
