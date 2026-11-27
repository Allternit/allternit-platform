/**
 * Server side of skill growth: the knowledge proposal routes of allternit-api
 * (`/api/v1/knowledge/proposals...`, contract: handoffs/memory-raw-rag-knowledge-factory-2026-11-27/CONTRACT.md,
 * "Knowledge factory"). Skill proposals live there so the curator and the
 * web / iOS / Android review screens see them; the device file in
 * `skill-growth/proposals/` is the offline cache (see ./growth).
 *
 * Uses the shared platform client (ALLTERNIT_API_TOKEN, else the `gizzi login`
 * device token). Each call says whether it worked, was refused for good, or
 * should be retried later (offline, signed out, server error, older runtime).
 */
import { platformFetch } from "@/runtime/bots/platform-api"

export interface ServerProposal {
  id: string
  target_kind: string
  target_ref: string
  title: string
  body: string
  base: string | null
  origin: string
  eval: unknown
  status: "proposed" | "approved" | "rejected" | "active" | "rolled_back"
}

export interface NewServerProposal {
  target_kind: "skill"
  target_ref: string
  title: string
  body: string
  base?: string
  origin: "skill_growth"
  eval?: { score: number; report: string }
}

export type ServerOp = "eval" | "approve" | "reject" | "activated" | "rollback"

/** Try again on the next skill-growth call: offline, signed out, 5xx, auth, or a runtime without the route. */
export class GrowthSyncUnavailable extends Error {
  constructor(message: string) {
    super(message)
    this.name = "GrowthSyncUnavailable"
  }
}

/** The server refused the proposal itself (bad input); retrying the same body will not help. */
export class GrowthSyncRefused extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
    this.name = "GrowthSyncRefused"
  }
}

const TIMEOUT_MS = 8_000

async function call(method: "GET" | "POST", path: string, body?: unknown): Promise<{ status: number; body: any }> {
  let response: Response
  try {
    response = await platformFetch(method, `/api/v1/knowledge/${path}`, body, { timeoutMs: TIMEOUT_MS })
  } catch (error) {
    throw new GrowthSyncUnavailable(error instanceof Error ? error.message : String(error))
  }
  const text = await response.text().catch(() => "")
  let parsed: any
  try {
    parsed = text ? JSON.parse(text) : undefined
  } catch {
    parsed = undefined
  }
  return { status: response.status, body: parsed }
}

function message(r: { status: number; body: any }) {
  return (r.body && (r.body.error || r.body.message)) || `HTTP ${r.status}`
}

export namespace GrowthServer {
  /** `POST /knowledge/proposals`. A 404 here means the runtime has no knowledge routes yet: retry later. */
  export async function create(input: NewServerProposal): Promise<ServerProposal> {
    const r = await call("POST", "proposals", input)
    if (r.status >= 200 && r.status < 300 && r.body?.proposal?.id) return r.body.proposal as ServerProposal
    if (r.status === 400 || r.status === 413 || r.status === 422) throw new GrowthSyncRefused(r.status, message(r))
    throw new GrowthSyncUnavailable(message(r))
  }

  /**
   * One status change or the eval. Returns the HTTP status when the server
   * answered for good: 200, or 400/404/409 (the server already moved on, the
   * proposal is gone, or the route is missing), all of which drop the op.
   */
  export async function send(serverId: string, op: ServerOp, evalBody?: { score: number; report: string }): Promise<number> {
    const id = encodeURIComponent(serverId)
    const r = await call("POST", `proposals/${id}/${op}`, op === "eval" ? { eval: evalBody ?? {} } : {})
    if ((r.status >= 200 && r.status < 300) || r.status === 400 || r.status === 404 || r.status === 409) return r.status
    throw new GrowthSyncUnavailable(message(r))
  }

  /** `GET /knowledge/proposals?status=approved`, skill proposals only. */
  export async function listApproved(): Promise<ServerProposal[]> {
    const r = await call("GET", "proposals?status=approved&limit=200")
    if (r.status < 200 || r.status >= 300 || !Array.isArray(r.body?.proposals)) throw new GrowthSyncUnavailable(message(r))
    return (r.body.proposals as ServerProposal[]).filter((p) => p && p.target_kind === "skill")
  }
}
