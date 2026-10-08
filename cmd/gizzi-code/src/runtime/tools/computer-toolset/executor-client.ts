/**
 * Client for the toolset executor in allternit-api, with the user's
 * credentials (ALLTERNIT_API_TOKEN or the `gizzi login` device token, via
 * platformFetch).
 */
import { platformFetch } from "@/runtime/bots/platform-api"
import type { ToolsetRequest, ToolsetResult } from "./contract.gen"

export interface ExecutorReply {
  status: number
  body: ToolsetResult & {
    error?: string
    approval_id?: string
    action_hash?: string
    risk?: string
    message?: string
  }
}

export interface ExecutorClient {
  run(computerId: string, req: ToolsetRequest, signal?: AbortSignal): Promise<ExecutorReply>
  schema(computerId: string, toolset: "computer" | "browser", signal?: AbortSignal): Promise<{ members: Array<{ name: string; enabled: boolean }> } | undefined>
  approve(approvalId: string, signal?: AbortSignal): Promise<boolean>
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
    return (await res.json()) as { members: Array<{ name: string; enabled: boolean }> }
  },
  async approve(approvalId, signal) {
    const res = await platformFetch("POST", `/api/aci/handoff/${enc(approvalId)}/approve`, {}, { signal, timeoutMs: 10_000 })
    return res.ok
  },
}
