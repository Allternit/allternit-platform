/**
 * HTTP client for the self-hosted Agents Server's worker routes
 * (cmd/allternit-api/openapi/selfhost-v1.yaml: Work, Tool calls).
 *
 * Credentials:
 *   - environment key (`aek_...`) as a Bearer token on claim / heartbeat /
 *     renew / complete / release;
 *   - the claim's session secret as `X-Session-Secret` on tool_calls/next
 *     and tool_calls/{id}/result.
 * Neither is ever logged; errors carry only status, code and message.
 */
import os from "node:os"
import { Installation } from "@/shared/installation"
import type { EnvironmentPolicy } from "./egress"

export interface WorkClaim {
  work_id: string
  session_id: string
  session_secret: string
  lease_generation: number
  lease_expires_at: string
  environment: EnvironmentPolicy
}

export interface Lease {
  work_id: string
  lease_generation: number
  lease_expires_at: string | null
  status: "claimed" | "ready" | "completed"
}

export interface ToolCall {
  tool_call_id: string
  name: string
  input: Record<string, unknown>
}

export type NextToolCall = { kind: "call"; call: ToolCall } | { kind: "idle" } | { kind: "closed" }

/** Response header on a 204 from tool_calls/next when the session is finished. */
export const SESSION_CLOSED_HEADER = "x-session-closed"

export class SelfHostHttpError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | undefined,
    message: string,
    readonly path: string,
  ) {
    super(`${status}${code ? ` ${code}` : ""} ${path}: ${message}`)
    this.name = "SelfHostHttpError"
  }
  /** 409 lease_stale, or a session secret the server no longer accepts. */
  get leaseLost(): boolean {
    return this.status === 409 || this.status === 401 || this.status === 403
  }
}

export interface WorkerClientOptions {
  server: string
  environmentKey?: string
  fetchImpl?: typeof fetch
}

export function createWorkerClient(opts: WorkerClientOptions) {
  const base = opts.server.trim().replace(/\/+$/, "")
  const doFetch = opts.fetchImpl ?? ((input: any, init?: any) => fetch(input, init))

  async function request(
    path: string,
    init: { body?: unknown; auth: "env" | { secret: string }; signal?: AbortSignal },
  ): Promise<Response> {
    const headers: Record<string, string> = { "Content-Type": "application/json", Accept: "application/json" }
    if (init.auth === "env") {
      if (!opts.environmentKey) throw new Error("an environment key is required for this call")
      headers.Authorization = `Bearer ${opts.environmentKey}`
      // Shown on the server's worker list (claim, heartbeat, renew).
      headers["X-Worker-Hostname"] = os.hostname()
      headers["X-Worker-Version"] = Installation.VERSION
    } else {
      headers["X-Session-Secret"] = init.auth.secret
    }
    const res = await doFetch(`${base}${path}`, {
      method: "POST",
      headers,
      body: JSON.stringify(init.body ?? {}),
      signal: init.signal,
    })
    if (res.ok) return res
    const text = await res.text().catch(() => "")
    let code: string | undefined
    let message = text.slice(0, 300)
    try {
      const parsed = JSON.parse(text) as { error?: { code?: string; message?: string } }
      code = parsed.error?.code
      message = parsed.error?.message ?? message
    } catch {
      // not JSON
    }
    throw new SelfHostHttpError(res.status, code, message || res.statusText, path)
  }

  const enc = encodeURIComponent

  return {
    base,

    /** null when the server answered 204 (no work). */
    async claim(
      environmentId: string,
      body: { worker_id: string; block_ms?: number; reclaim_older_than_ms?: number },
      signal?: AbortSignal,
    ): Promise<WorkClaim | null> {
      const res = await request(`/v1/environments/${enc(environmentId)}/work/claim`, { body, auth: "env", signal })
      if (res.status === 204) return null
      return (await res.json()) as WorkClaim
    },

    async heartbeat(workId: string, leaseGeneration: number): Promise<Lease> {
      const res = await request(`/v1/work/${enc(workId)}/heartbeat`, { body: { lease_generation: leaseGeneration }, auth: "env" })
      return (await res.json()) as Lease
    },

    async renew(workId: string, leaseGeneration: number): Promise<Lease> {
      const res = await request(`/v1/work/${enc(workId)}/renew`, { body: { lease_generation: leaseGeneration }, auth: "env" })
      return (await res.json()) as Lease
    },

    async complete(
      workId: string,
      leaseGeneration: number,
      outcome: "success" | "failed" | "cancelled" = "success",
      errorMessage?: string,
    ): Promise<Lease> {
      const body: Record<string, unknown> = { lease_generation: leaseGeneration, outcome }
      if (errorMessage) body.error_message = errorMessage.slice(0, 2000)
      const res = await request(`/v1/work/${enc(workId)}/complete`, { body, auth: "env" })
      return (await res.json()) as Lease
    },

    async release(workId: string, leaseGeneration: number): Promise<Lease> {
      const res = await request(`/v1/work/${enc(workId)}/release`, { body: { lease_generation: leaseGeneration }, auth: "env" })
      return (await res.json()) as Lease
    },

    /**
     * Long-poll for the session's next tool call. A 204 is `idle`, unless it
     * carries `X-Session-Closed: true`, which means the session is finished
     * and the worker should complete the item.
     */
    async nextToolCall(workId: string, secret: string, blockMs: number, signal?: AbortSignal): Promise<NextToolCall> {
      const res = await request(`/v1/work/${enc(workId)}/tool_calls/next`, {
        body: { block_ms: Math.max(0, Math.min(30_000, Math.floor(blockMs))) },
        auth: { secret },
        signal,
      })
      if (res.status === 204) {
        return (res.headers.get(SESSION_CLOSED_HEADER) ?? "").toLowerCase() === "true" ? { kind: "closed" } : { kind: "idle" }
      }
      const body = (await res.json().catch(() => undefined)) as (Partial<ToolCall> & { session_closed?: boolean }) | undefined
      if (body?.session_closed === true && !body.tool_call_id) return { kind: "closed" }
      if (!body || typeof body.tool_call_id !== "string" || typeof body.name !== "string") return { kind: "idle" }
      return {
        kind: "call",
        call: { tool_call_id: body.tool_call_id, name: body.name, input: (body.input ?? {}) as Record<string, unknown> },
      }
    },

    async submitResult(toolCallId: string, secret: string, output: string, isError: boolean): Promise<void> {
      await request(`/v1/tool_calls/${enc(toolCallId)}/result`, {
        body: { output, is_error: isError },
        auth: { secret },
      })
    },
  }
}

export type WorkerClient = ReturnType<typeof createWorkerClient>
