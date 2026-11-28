/**
 * Inbox client over `/api/v1/inbox`, the one runtime endpoint for every surface
 * (AllternitOS CONTRACT.md 6.4, 7.2). Shared by the `inbox` model tool (list,
 * snooze, prepare only), the TUI `/inbox` and `gizzi inbox` (a person's
 * approve and deny). Uses the existing allternitApi config and credentials.
 *
 * The model's tool never imports `resolve` below: approve and deny are for a
 * person at a terminal. The guard also blocks the model's shell from reaching
 * them (guard/permission/catastrophic.ts).
 */
import { apiFetch, getAllternitApiConfigWithDeviceToken } from "@/runtime/services/api/allternitApi"

/**
 * InboxItemV1, copied from spec/Contracts/os/v1/schemas/inbox.schema.json (1.0.0).
 * gizzi-code's build cannot import across the repo's spec folder.
 */
export interface InboxItem {
  id: string
  kind: "approval" | "informational"
  action_class: string
  action_summary: string
  action_payload: Record<string, unknown>
  assignee: string
  deadline: string
  urgency: "low" | "normal" | "high" | "critical"
  run_id: string
  source_store: string
  status: string
  attention_id: string
}

export type InboxScope = "needs_you" | "snoozed" | "all"

export class InboxError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
    this.name = "InboxError"
  }
}

async function call(method: "GET" | "POST", path: string, body?: unknown): Promise<any> {
  const config = await getAllternitApiConfigWithDeviceToken()
  const response = await apiFetch(config, path, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(15_000),
  })
  const text = await response.text().catch(() => "")
  let json: any
  try {
    json = text ? JSON.parse(text) : undefined
  } catch {
    json = { error: text }
  }
  if (!response.ok) {
    if (response.status === 401 || response.status === 403)
      throw new InboxError(response.status, "Allternit would not accept this request. Sign in with `gizzi login`; approving needs a person's own session.")
    if (response.status === 404) throw new InboxError(404, "Not found: that inbox item is gone or was handled elsewhere.")
    if (response.status === 409) throw new InboxError(409, "That item changed since you looked. List it again.")
    throw new InboxError(response.status, typeof json?.error === "string" ? json.error : `HTTP ${response.status}`)
  }
  return json
}

const enc = encodeURIComponent

/** The endpoint answers `{ needsYou: [...] }` (and `snoozed`); accept a bare array too. */
function rows(json: any, scope: InboxScope): InboxItem[] {
  if (Array.isArray(json)) return json
  const needs: InboxItem[] = Array.isArray(json?.needsYou) ? json.needsYou : []
  const snoozed: InboxItem[] = Array.isArray(json?.snoozed) ? json.snoozed : []
  if (scope === "needs_you") return needs
  if (scope === "snoozed") return snoozed
  return [...needs, ...snoozed]
}

export async function listInbox(scope: InboxScope = "needs_you", limit?: number): Promise<InboxItem[]> {
  const json = await call("GET", `/api/v1/inbox?scope=${enc(scope)}`)
  const all = rows(json, scope)
  return limit ? all.slice(0, limit) : all
}

export async function getInboxItems(ids: string[]): Promise<{ found: InboxItem[]; missing: string[] }> {
  const all = await listInbox("all")
  const found = all.filter((i) => ids.includes(i.id))
  return { found, missing: ids.filter((id) => !found.some((i) => i.id === id)) }
}

export async function snoozeInbox(id: string, until: string): Promise<void> {
  const ms = Date.parse(until) - Date.now()
  if (!Number.isFinite(ms)) throw new InboxError(400, `bad_request: "${until}" is not a timestamp.`)
  await call("POST", `/api/v1/inbox/${enc(id)}/snooze`, { minutes: Math.max(1, Math.ceil(ms / 60_000)) })
}

/** Stores a draft the person reviews. Resolves nothing. */
export async function prepareInbox(id: string, draft: Record<string, unknown>): Promise<any> {
  return call("POST", `/api/v1/inbox/${enc(id)}/prepare`, { draft })
}

/** A person's decision. Same wire shape as the app's inbox: deny is approve with decision "deny". */
export async function resolveInbox(id: string, decision: "approve" | "deny"): Promise<void> {
  await call("POST", `/api/v1/inbox/${enc(id)}/approve`, decision === "deny" ? { decision: "deny" } : {})
}

export function inboxLine(i: InboxItem): string {
  return `${i.id}  [${i.urgency}] ${i.action_class}: ${i.action_summary}  (due ${i.deadline})`
}
