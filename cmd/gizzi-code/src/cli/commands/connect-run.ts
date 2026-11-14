/**
 * `gizzi connect` network side: cloud-api `/api/v1/connections` (account rows)
 * plus the local runtime's (allternit-api) when it answers. Shapes are the
 * contract's; wording is the contract's four status words.
 */
import * as fs from "fs"
import * as os from "os"
import * as path from "path"
import { cloudInferenceToken, cloudOrigin } from "@/runtime/providers/discovery/allternit-cloud"
import { Installation } from "@/shared/installation"
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"
import type { Row } from "@/cli/commands/connect-model"

export class ConnectFailure extends Error {}

export type Login = {
  kind: "device_code" | "link_paste" | "oauth" | "paste" | "scan" | "pair"
  code?: string
  url?: string
  url_with_code?: string
  expires_at?: string
  poll?: boolean
}
export type LoginStatus = { status: "pending" | "done" | "expired" | "failed"; row?: Row }

async function token(): Promise<string> {
  const direct = await cloudInferenceToken()
  if (direct) return direct
  try {
    const dir = process.env.GIZZI_CONFIG_DIR ?? path.join(os.homedir(), ".config", "gizzi-code")
    const session = JSON.parse(fs.readFileSync(path.join(dir, "session.json"), "utf8"))
    if (session?.accessToken) return session.accessToken
  } catch {}
  throw new ConnectFailure("Sign in to Allternit first: run `gizzi login`.")
}

function runtimeOrigin(): string {
  return (process.env.ALLTERNIT_API_URL || ALLTERNIT_GATEWAY_BASE).replace(/\/+$/, "")
}

/** Rows with source `runtime:<id>` are answered by the local runtime, everything else by the cloud. */
function base(source?: string): string {
  return source?.startsWith("runtime:") ? runtimeOrigin() : cloudOrigin()
}

async function call(origin: string, method: string, route: string, body?: unknown, timeout = 15_000): Promise<any> {
  const t = await token()
  let res: Response
  try {
    res = await fetch(`${origin}/api/v1/connections${route}`, {
      method,
      headers: {
        Authorization: `Bearer ${t}`,
        Accept: "application/json",
        "X-Allternit-Client": `gizzi-code/${Installation.VERSION}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeout),
    })
  } catch (e) {
    throw new ConnectFailure(`Could not reach ${origin}: ${e instanceof Error ? e.message : String(e)}`)
  }
  if (!res.ok) {
    const b = (await res.json().catch(() => undefined)) as { error?: unknown; message?: string; reason_code?: string } | undefined
    if (res.status === 401) throw new ConnectFailure("Allternit did not accept your sign-in. Run `gizzi login`.")
    if (res.status === 403) throw new ConnectFailure("Connections needs your signed-in account, not an API key.")
    throw new ConnectFailure(b?.message || (typeof b?.error === "string" ? b.error : `Request failed (HTTP ${res.status})`))
  }
  if (res.status === 204) return undefined
  return res.json()
}

/** Cloud rows, plus the local runtime's when it answers (a quiet miss when no computer runs). */
export async function listRows(): Promise<Row[]> {
  const cloud: { connections: Row[] } = await call(cloudOrigin(), "GET", "")
  let runtime: Row[] = []
  try {
    const r = await call(runtimeOrigin(), "GET", "", undefined, 2_500)
    runtime = (r?.connections ?? []).map((x: Row) => ({ ...x, source: x.source ?? "runtime:local" }))
  } catch {}
  const { mergeRows } = await import("@/cli/commands/connect-model")
  return mergeRows(cloud.connections ?? [], runtime)
}

export const startConnect = (id: string, source?: string, way?: string): Promise<{ login?: Login; state?: string }> =>
  call(base(source), "POST", `/${encodeURIComponent(id)}/connect`, way ? { way } : {})
export const submit = (id: string, source: string | undefined, value: string): Promise<Row> =>
  call(base(source), "POST", `/${encodeURIComponent(id)}/submit`, { value })
export const loginStatus = (id: string, source?: string): Promise<LoginStatus> =>
  call(base(source), "GET", `/${encodeURIComponent(id)}/login`)
export const disconnect = (id: string, source?: string): Promise<void> => call(base(source), "DELETE", `/${encodeURIComponent(id)}`)

/** Best effort: the code goes to the clipboard so the next page only needs a paste. */
export async function copyToClipboard(text: string): Promise<boolean> {
  const tools: string[][] =
    process.platform === "darwin" ? [["pbcopy"]] : process.platform === "win32" ? [["clip"]] : [["wl-copy"], ["xclip", "-selection", "clipboard"]]
  for (const cmd of tools) {
    try {
      const p = Bun.spawn({ cmd, stdin: "pipe", stdout: "ignore", stderr: "ignore" })
      p.stdin.write(text)
      await p.stdin.end()
      if ((await p.exited) === 0) return true
    } catch {}
  }
  return false
}

/** Poll `/login` every 2 s until done, expired or failed (or the code's own expiry, 10 min at most). */
export async function pollLogin(
  id: string,
  source: string | undefined,
  expiresAt?: string,
  opts: { interval?: number; onTick?: () => void } = {},
): Promise<LoginStatus> {
  const interval = opts.interval ?? 2_000
  const limit = Math.min(Date.parse(expiresAt ?? "") || Date.now() + 600_000, Date.now() + 600_000)
  while (Date.now() < limit) {
    const s = await loginStatus(id, source)
    if (s.status !== "pending") return s
    opts.onTick?.()
    await new Promise((r) => setTimeout(r, interval))
  }
  return { status: "expired" }
}
