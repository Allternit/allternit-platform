/**
 * Client for the toolset executor in allternit-api, with the user's
 * credentials (ALLTERNIT_API_TOKEN or the `gizzi login` device token, via
 * platformFetch).
 *
 * Where the executor runs decides where a `browser` toolset session runs: the
 * executor drives the ACU gateway next to it (allternit-api `build_target`,
 * `acu_url`, 127.0.0.1:8760 by default, spawned by Desktop). So a browser
 * session is on the user's own machine only when the executor is: the API on
 * this Mac, or on the user's own cloud computer when gizzi runs there. Both
 * reach it over loopback. `executorIsLocal` is that check; features that must
 * never run a browser on a company host (`browser_tasks`) require it.
 */
import { platformApiBase, platformFetch } from "@/runtime/bots/platform-api"
import type { ToolsetRequest, ToolsetResult } from "./contract.gen"

export interface ExecutorReply {
  status: number
  body: ToolsetResult & {
    error?: string
    approval_id?: string
    action_hash?: string
    risk?: string
    message?: string
    /** 409/403 credential approval (contract #3112): which login the step would fill. */
    kind?: "action" | "credential" | string
    credential?: { name?: string; domain?: string } | null
  }
}

/** One frame of GET /computers/:id/toolset/events (event name `computer.action`; read `type`). */
export interface ComputerEvent {
  type: string
  ts?: string
  data: Record<string, any>
}

/** The person's answer to a held block (POST /computers/:id/block-resolve). */
export interface BlockResolveReply {
  ok: boolean
  run_id?: string
  rerun_target?: string | null
}

/** GET /computers/:id/toolset/schema: what the target runs right now. */
export interface ToolsetSchema {
  members: Array<{ name: string; enabled: boolean; reason?: string | null }>
  /** False when the target is down (for `browser`: the ACU gateway isn't running). */
  available?: boolean
  /** this_device, guest_linux, browser_gateway, …; null when no target could be built. */
  target?: string | null
  message?: string
}

export interface ExecutorClient {
  run(computerId: string, req: ToolsetRequest, signal?: AbortSignal): Promise<ExecutorReply>
  schema(computerId: string, toolset: "computer" | "browser", signal?: AbortSignal): Promise<ToolsetSchema | undefined>
  /** `scope: "run"` = allow this credential name + domain for this run only. */
  approve(approvalId: string, signal?: AbortSignal, scope?: "run"): Promise<boolean>
  // The calls below are optional so a scripted fake can leave them out; the
  // needs-you watcher (needs-you.ts) does nothing without `events`.
  deny?(approvalId: string, signal?: AbortSignal): Promise<boolean>
  events?(computerId: string, signal: AbortSignal): AsyncIterable<ComputerEvent>
  humanDone?(computerId: string, signal?: AbortSignal): Promise<boolean>
  /** The masked secret box: `{ text }` or `{ cancel: true }`. Never logs the value. */
  supplySecret?(computerId: string, body: { text: string } | { cancel: true }, signal?: AbortSignal): Promise<boolean>
  blockResolve?(
    computerId: string,
    body: { action: "continue" | "stop" | "rerun"; rerun_target?: string },
    signal?: AbortSignal,
  ): Promise<BlockResolveReply | undefined>
}

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "::1", "[::1]"])

/**
 * Whether the executor (and so any `browser` toolset session it opens) runs on
 * this machine: the API base is a loopback address. A remote base (a company
 * API) is never "local", even when this process runs on a cloud computer: the
 * browser would then run wherever that API runs.
 */
export function executorIsLocal(base: string = platformApiBase()): boolean {
  let host: string
  try {
    host = new URL(base).hostname.toLowerCase()
  } catch {
    return false
  }
  return LOOPBACK_HOSTS.has(host) || /^127(?:\.\d{1,3}){3}$/.test(host)
}

const enc = encodeURIComponent

export const httpExecutor: ExecutorClient = {
  async run(computerId, req, signal) {
    const res = await platformFetch("POST", `/api/v1/computers/${enc(computerId)}/toolset`, req, { signal, timeoutMs: 120_000 })
    const text = await res.text()
    let body: any
    try {
      body = JSON.parse(text)
    } catch {
      body = { is_error: true, content: [{ type: "text", text: text || `HTTP ${res.status}` }], screen: { width: 0, height: 0, scale: 1 } }
    }
    return { status: res.status, body }
  },
  async schema(computerId, toolset, signal) {
    const res = await platformFetch("GET", `/api/v1/computers/${enc(computerId)}/toolset/schema?toolset=${toolset}`, undefined, {
      signal,
      timeoutMs: 10_000,
    })
    if (!res.ok) return undefined
    return (await res.json()) as ToolsetSchema
  },
  async approve(approvalId, signal, scope) {
    const res = await platformFetch("POST", `/api/aci/handoff/${enc(approvalId)}/approve`, scope ? { scope } : {}, { signal, timeoutMs: 10_000 })
    return res.ok
  },
  async deny(approvalId, signal) {
    const res = await platformFetch("POST", `/api/aci/handoff/${enc(approvalId)}/deny`, {}, { signal, timeoutMs: 10_000 })
    return res.ok
  },
  async *events(computerId, signal) {
    const res = await platformFetch("GET", `/api/v1/computers/${enc(computerId)}/toolset/events`, undefined, {
      signal,
      accept: "text/event-stream",
    })
    if (!res.ok || !res.body) return
    const reader = res.body.getReader()
    const decoder = new TextDecoder()
    let buf = ""
    try {
      for (;;) {
        const { done, value } = await reader.read()
        if (done) return
        buf += decoder.decode(value, { stream: true })
        let at: number
        while ((at = buf.search(/\r?\n\r?\n/)) >= 0) {
          const frame = buf.slice(0, at)
          buf = buf.slice(at).replace(/^\r?\n\r?\n/, "")
          const data = frame
            .split(/\r?\n/)
            .filter((l) => l.startsWith("data:"))
            .map((l) => l.slice(5).trimStart())
            .join("\n")
          if (!data) continue
          try {
            const ev = JSON.parse(data)
            if (ev && typeof ev.type === "string") yield { type: ev.type, ts: ev.ts, data: ev.data ?? {} }
          } catch {}
        }
      }
    } finally {
      reader.cancel().catch(() => {})
    }
  },
  async humanDone(computerId, signal) {
    const res = await platformFetch("POST", `/api/v1/computers/${enc(computerId)}/human-done`, {}, { signal, timeoutMs: 10_000 })
    return res.ok
  },
  async supplySecret(computerId, body, signal) {
    const res = await platformFetch("POST", `/api/v1/computers/${enc(computerId)}/secret`, body, { signal, timeoutMs: 30_000 })
    return res.ok
  },
  async blockResolve(computerId, body, signal) {
    const res = await platformFetch("POST", `/api/v1/computers/${enc(computerId)}/block-resolve`, body, { signal, timeoutMs: 10_000 })
    if (!res.ok) return undefined
    return (await res.json()) as BlockResolveReply
  },
}
