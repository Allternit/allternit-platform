/**
 * Work views shared by the CLI (`gizzi computers`, `gizzi run --on`,
 * `gizzi artifact share`) and the TUI (`/decisions`, `/pr`, `/projects`,
 * `/routines`). Every call is a route Desktop already uses:
 *
 * - Decisions: the local engine's `GET /v1/question` + `GET /v1/permission`
 *   (what allternit-api `/api/v1/questions` and `/api/v1/permissions` relay to
 *   for Desktop's Tasks → Decisions deck). Bot chats and live terminal
 *   sessions are engine sessions, so their asks are in the same lists.
 * - Computers: allternit-api `/api/v1/computers` (Desktop Computers,
 *   `lib/computers-api.ts`), paired runtimes from cloud-api
 *   `/api/v1/runtime-devices`, sessions on another runtime through the cloud
 *   relay `POST /api/v1/runtime-devices/:id/proxy`.
 * - Projects: allternit-api `/api/v1/cowork/projects` (status page in
 *   `metadata.status`, `lib/projects/project-status.ts`).
 * - Routines: allternit-api `/api/v1/automation/routines` (Automation Tasks,
 *   `lib/automation-api.ts`); pause/resume is `PUT { status }`.
 * - PR checks: the `gh` CLI, the same source the engine's Linked PRs use.
 * - Artifact share: allternit-api `/api/v1/content-artifacts/:id/publish`.
 */
import {
  apiFetch,
  apiFetchJson,
  getAllternitApiConfigWithDeviceToken,
  type AllternitApiConfig,
} from "@/runtime/services/api/allternitApi"
import { ALLTERNIT_GATEWAY_BASE } from "@/shared/constants/allternitGateway"

// ── transport ─────────────────────────────────────────────────────────────

export function engineBase(): string {
  return (process.env.GIZZI_ENGINE_URL || "http://127.0.0.1:4096").replace(/\/+$/, "")
}

async function engine<T>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${engineBase()}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: body === undefined ? undefined : { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(5_000),
  })
  if (!res.ok) throw new Error(`engine ${path}: ${res.status}`)
  return (await res.json()) as T
}

let configPromise: Promise<AllternitApiConfig> | undefined
/**
 * Desktop's gateway on this machine, with this machine's device token. A
 * production build without ALLTERNIT_API_URL refuses to guess a remote
 * origin; the work views then read the local gateway Desktop runs, which is
 * where Desktop itself reads them.
 */
function config(): Promise<AllternitApiConfig> {
  configPromise ??= getAllternitApiConfigWithDeviceToken().catch(async () => {
    const { Pairing } = await import("@/runtime/services/pairing/pairing")
    const device = await Pairing.load().catch(() => undefined)
    return {
      baseUrl: ALLTERNIT_GATEWAY_BASE,
      token: Pairing.tokenUsable(device) ? device!.deviceToken! : process.env.ALLTERNIT_API_TOKEN?.trim(),
      userId: process.env.ALLTERNIT_USER_ID?.trim() || "gizzi-local",
    }
  })
  return configPromise
}

/** allternit-api (Desktop's gateway, 127.0.0.1:8013 unless ALLTERNIT_API_URL says otherwise). */
async function gateway<T>(path: string, init: RequestInit = {}): Promise<T> {
  const cfg = await config()
  try {
    return await apiFetchJson<T>(cfg, path, init)
  } catch (e) {
    // apiFetchJson's own errors carry the HTTP status; anything else is transport.
    if (!String((e as Error)?.message).startsWith("Allternit API "))
      throw new Error(`The Allternit gateway at ${cfg.baseUrl} is not reachable. Open Allternit Desktop (or set ALLTERNIT_API_URL).`)
    throw e
  }
}

export function cloudBase(): string {
  return (process.env.ALLTERNIT_CLOUD_URL || "https://api.allternit.com").replace(/\/+$/, "")
}

// ── decisions ─────────────────────────────────────────────────────────────

export type QuestionOption = { label: string; description?: string }
export type Decision =
  | {
      kind: "question"
      id: string
      sessionID: string
      session?: string
      questions: Array<{ question: string; header?: string; options: QuestionOption[]; multiple?: boolean }>
    }
  | {
      kind: "approval"
      id: string
      sessionID: string
      session?: string
      permission: string
      patterns: string[]
      metadata: Record<string, unknown>
    }

const titles = new Map<string, string>()
async function sessionTitle(id: string): Promise<string | undefined> {
  if (titles.has(id)) return titles.get(id)
  const info = await engine<{ title?: string }>(`/session/${encodeURIComponent(id)}`).catch(() => undefined)
  if (info?.title) titles.set(id, info.title)
  return info?.title
}

/** Every question and approval an agent is waiting on, across all engine sessions. */
export async function listDecisions(): Promise<Decision[]> {
  const [questions, permissions] = await Promise.all([
    engine<any[]>("/v1/question"),
    engine<any[]>("/v1/permission"),
  ])
  const list: Decision[] = [
    ...(Array.isArray(questions) ? questions : []).map(
      (q): Decision => ({ kind: "question", id: q.id, sessionID: q.sessionID, questions: q.questions ?? [] }),
    ),
    ...(Array.isArray(permissions) ? permissions : []).map(
      (p): Decision => ({
        kind: "approval",
        id: p.id,
        sessionID: p.sessionID,
        permission: p.permission,
        patterns: p.patterns ?? [],
        metadata: p.metadata ?? {},
      }),
    ),
  ]
  await Promise.all(list.map(async (d) => (d.session = await sessionTitle(d.sessionID))))
  return list
}

/** Count only (footer badge); never throws. */
export async function countDecisions(): Promise<number | undefined> {
  try {
    const [q, p] = await Promise.all([engine<unknown[]>("/v1/question"), engine<unknown[]>("/v1/permission")])
    return (Array.isArray(q) ? q.length : 0) + (Array.isArray(p) ? p.length : 0)
  } catch {
    return undefined
  }
}

/** One answer per question, each an array of chosen labels (engine Question.Reply). */
export function answerQuestion(id: string, answers: string[][]): Promise<unknown> {
  return engine(`/v1/question/${encodeURIComponent(id)}/reply`, { answers })
}

export function rejectQuestion(id: string): Promise<unknown> {
  return engine(`/v1/question/${encodeURIComponent(id)}/reject`, {})
}

export function answerApproval(id: string, reply: "once" | "always" | "reject"): Promise<unknown> {
  return engine(`/v1/permission/${encodeURIComponent(id)}/reply`, { reply })
}

/** One-line summary of what an approval is for (file, command or URL first). */
export function approvalTarget(d: Extract<Decision, { kind: "approval" }>): string {
  const m = d.metadata as Record<string, any>
  const input = (m.input ?? {}) as Record<string, any>
  const value = m.command ?? input.command ?? m.filepath ?? input.file_path ?? m.url ?? input.url ?? d.patterns[0]
  return typeof value === "string" ? value : d.permission
}

// ── computers ─────────────────────────────────────────────────────────────

export type Computer = {
  id: string
  kind: string
  provider: string
  status: string
  name: string
  os?: string | null
  host?: string | null
  bot_id?: string | null
}

export type Runtime = {
  id: string
  name: string
  runtimeType: string
  status: string
  platform?: string
  lastSeenAt?: string
}

export async function listComputers(): Promise<Computer[]> {
  return (await gateway<{ computers?: Computer[] }>("/api/v1/computers")).computers ?? []
}

export function startComputer(id: string): Promise<{ id: string; status: string }> {
  return gateway(`/api/v1/computers/${encodeURIComponent(id)}/start`, { method: "POST" })
}

export function stopComputer(id: string): Promise<{ id: string; status: string }> {
  return gateway(`/api/v1/computers/${encodeURIComponent(id)}/stop`, { method: "POST" })
}

// ── bots pause (allternit-api GET/PUT /api/v1/bots/pause) ─────────────────

export type BotPause = { paused: boolean; since: number | null; by: string | null }

/** Thrown when the runtime has no pause route (an older runtime answers 404). */
export class BotPauseUnsupported extends Error {
  constructor() {
    super("This computer's Allternit runtime is too old for pause. Update it.")
  }
}

async function botPauseCall(init: RequestInit = {}): Promise<BotPause> {
  try {
    return await gateway<BotPause>("/api/v1/bots/pause", init)
  } catch (e) {
    if (/ failed: 404\b/.test(String((e as Error)?.message))) throw new BotPauseUnsupported()
    throw e
  }
}

export function getBotPause(): Promise<BotPause> {
  return botPauseCall()
}

export function setBotPause(paused: boolean): Promise<BotPause> {
  return botPauseCall({ method: "PUT", body: JSON.stringify({ paused, by: "gizzi" }) })
}

/** One plain sentence for the paused state, with since/by when known. */
export function describeBotPause(p: BotPause, now: number = Date.now()): string {
  if (!p.paused) return "Bots are running."
  const bits: string[] = []
  if (p.since) bits.push(`since ${new Date(p.since * 1000).toLocaleString()}`)
  if (p.by) bits.push(`by ${p.by}`)
  void now
  return `All bots are paused${bits.length ? ` ${bits.join(" ")}` : ""}. Routines and automatic replies are on hold; your own messages still run.`
}

/** Runtimes paired to this account (this machine's device token reads them). */
export async function listRuntimes(): Promise<Runtime[]> {
  const cfg = await config()
  const res = await fetch(`${cloudBase()}/api/v1/runtime-devices`, {
    headers: cfg.token ? { Authorization: `Bearer ${cfg.token}` } : {},
    signal: AbortSignal.timeout(10_000),
  })
  if (!res.ok) throw new Error(`runtime-devices: ${res.status}`)
  return ((await res.json()) as { runtimes?: Runtime[] }).runtimes ?? []
}

/** Pick one entry by exact id, then case-insensitive name. */
export function pick<T extends { id: string; name: string }>(list: T[], key: string): T | undefined {
  const k = key.toLowerCase()
  return list.find((c) => c.id === key) ?? list.find((c) => c.name.toLowerCase() === k)
}

/**
 * Start a session on another paired runtime through the cloud relay and send
 * it the first message. The relay checks a Clerk session or an Allternit API
 * key with the `compute` scope (ALLTERNIT_API_TOKEN); the device token from
 * `gizzi pair` only identifies this machine and is refused there.
 */
export async function runOnRuntime(
  runtimeId: string,
  text: string,
  opts: { title?: string; model?: { providerID: string; modelID: string } } = {},
): Promise<{ sessionID: string; web: string; phone: string }> {
  const token = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (!token) {
    throw new Error(
      "Running on another computer goes through the Allternit relay, which needs an Allternit API key with the compute scope. Set ALLTERNIT_API_TOKEN (Settings → API keys) and try again.",
    )
  }
  const relay = async (method: string, path: string, body?: unknown) => {
    const res = await fetch(`${cloudBase()}/api/v1/runtime-devices/${encodeURIComponent(runtimeId)}/proxy`, {
      method: "POST",
      headers: { "content-type": "application/json", Authorization: `Bearer ${token}` },
      body: JSON.stringify({
        method,
        path,
        headers: { "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
      }),
      signal: AbortSignal.timeout(60_000),
    })
    const raw = await res.text()
    if (!res.ok) throw new Error(`relay ${path}: ${res.status} ${raw.slice(0, 200)}`)
    return raw ? JSON.parse(raw) : {}
  }
  const created = await relay("POST", "/api/v1/agent-sessions", {
    name: opts.title ?? text.slice(0, 60),
    origin_surface: "code",
    model: opts.model,
    metadata: { sourceRef: { harness: "gizzi-cli" } },
  })
  const sessionID: string = created.id ?? created.session?.id ?? created.sessionId
  if (!sessionID) throw new Error("The computer did not return a session id.")
  await relay("POST", `/api/v1/agent-sessions/${encodeURIComponent(sessionID)}/messages`, { text })
  const query = new URLSearchParams({ runtime: runtimeId, session: sessionID })
  return { sessionID, web: `https://ai.allternit.com/?${query}`, phone: `https://m.allternit.com/?${query}` }
}

// ── projects ──────────────────────────────────────────────────────────────

export type ProjectState = "on_track" | "at_risk" | "blocked" | "needs_decision" | "done"
export type Project = {
  id: string
  title: string
  kind?: string
  lifecycle: "active" | "closed"
  state?: ProjectState
  nextAction?: string
  updatedAt?: string
}

export const PROJECT_STATE_LABELS: Record<ProjectState, string> = {
  on_track: "On track",
  at_risk: "At risk",
  blocked: "Blocked",
  needs_decision: "Needs decision",
  done: "Done",
}

function parseMeta(meta: unknown): Record<string, any> {
  if (typeof meta === "string") {
    try {
      return JSON.parse(meta) ?? {}
    } catch {
      return {}
    }
  }
  return meta && typeof meta === "object" ? (meta as Record<string, any>) : {}
}

export function toProject(row: any): Project {
  const meta = parseMeta(row.metadata)
  const status = meta.status && typeof meta.status === "object" ? meta.status : undefined
  return {
    id: row.id,
    title: row.title || row.name || "Untitled project",
    kind: meta.kind,
    lifecycle: status?.lifecycle === "closed" ? "closed" : "active",
    state: status?.state,
    nextAction: status?.next_action || undefined,
    updatedAt: row.updated_at,
  }
}

export async function listProjects(): Promise<Project[]> {
  const body = await gateway<{ projects?: any[] }>("/api/v1/cowork/projects")
  return (body.projects ?? []).map(toProject)
}

// ── routines ──────────────────────────────────────────────────────────────

export type Routine = {
  id: string
  name: string
  status: string
  agent_id?: string | null
  schedule_type?: string
  schedule_expression?: string
  timezone?: string
  next_run_at?: string | null
  last_run_at?: string | null
}

export async function listRoutines(): Promise<Routine[]> {
  const body = await gateway<Routine[] | { routines?: Routine[] }>("/api/v1/automation/routines")
  const list = Array.isArray(body) ? body : (body.routines ?? [])
  // Active by next run, then paused (their stored next_run_at is stale).
  const key = (r: Routine) => (r.status === "active" && r.next_run_at ? Date.parse(r.next_run_at) : Infinity)
  return list.sort((a, b) => key(a) - key(b))
}

/** Pause or resume, the way Desktop's routine toggle does (`PUT { status }`). */
export function setRoutinePaused(id: string, paused: boolean): Promise<Routine> {
  return gateway(`/api/v1/automation/routines/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: JSON.stringify({ status: paused ? "paused" : "active" }),
  })
}

// ── PR checks (gh, like the engine's Linked PRs) ──────────────────────────

export type PrCheck = { name: string; state: "pass" | "fail" | "pending" | "skip"; url?: string }
export type BranchPR = {
  number: number
  title: string
  url: string
  state: string
  isDraft: boolean
  branch: string
  checks: PrCheck[]
}

export function checkState(c: any): PrCheck["state"] {
  const v = String(c.conclusion || c.state || c.status || "").toUpperCase()
  if (["SUCCESS", "NEUTRAL"].includes(v)) return "pass"
  if (["SKIPPED", "STALE", "CANCELLED"].includes(v)) return "skip"
  if (["FAILURE", "ERROR", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE"].includes(v)) return "fail"
  return "pending"
}

export function toBranchPR(json: any): BranchPR {
  return {
    number: json.number,
    title: json.title,
    url: json.url,
    state: json.state,
    isDraft: !!json.isDraft,
    branch: json.headRefName,
    checks: (json.statusCheckRollup ?? []).map((c: any) => ({
      name: c.name || c.context || "check",
      state: checkState(c),
      url: c.detailsUrl || c.targetUrl,
    })),
  }
}

/** The open PR for the current branch, or null (no PR, no gh, not a repo). */
export async function currentBranchPR(cwd: string = process.cwd()): Promise<BranchPR | null> {
  try {
    const proc = Bun.spawn(
      ["gh", "pr", "view", "--json", "number,title,url,state,isDraft,headRefName,statusCheckRollup"],
      { cwd, stdout: "pipe", stderr: "ignore" },
    )
    const timer = setTimeout(() => proc.kill(), 15_000)
    const out = await new Response(proc.stdout).text()
    clearTimeout(timer)
    if ((await proc.exited) !== 0 || !out.trim()) return null
    return toBranchPR(JSON.parse(out))
  } catch {
    return null
  }
}

export function checkCounts(pr: BranchPR): { pass: number; fail: number; pending: number } {
  return {
    pass: pr.checks.filter((c) => c.state === "pass").length,
    fail: pr.checks.filter((c) => c.state === "fail").length,
    pending: pr.checks.filter((c) => c.state === "pending").length,
  }
}

/** `PR #123 ✓4 ✗1 ●2` — the footer chip. */
export function prChip(pr: BranchPR): string {
  const n = checkCounts(pr)
  return [`PR #${pr.number}`, n.pass && `✓${n.pass}`, n.fail && `✗${n.fail}`, n.pending && `●${n.pending}`]
    .filter(Boolean)
    .join(" ")
}

// ── artifact share ────────────────────────────────────────────────────────

export type PublishStatus = { published: boolean; url?: string; version?: number; routePath?: string; publishedAt?: string }

export function shareArtifact(id: string, version?: number): Promise<PublishStatus> {
  return gateway(`/api/v1/content-artifacts/${encodeURIComponent(id)}/publish`, {
    method: "POST",
    body: JSON.stringify(version != null ? { version } : {}),
  })
}

export function artifactShareStatus(id: string): Promise<PublishStatus> {
  return gateway(`/api/v1/content-artifacts/${encodeURIComponent(id)}/publish`)
}

export async function unshareArtifact(id: string): Promise<void> {
  const res = await apiFetch(await config(), `/api/v1/content-artifacts/${encodeURIComponent(id)}/publish`, {
    method: "DELETE",
  })
  if (!res.ok) throw new Error(`unpublish ${id}: ${res.status}`)
}
