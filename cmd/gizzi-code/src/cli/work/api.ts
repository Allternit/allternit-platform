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
  /** A cloud computer the relay can wake when it is asleep. */
  wakeable?: boolean
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
 * The paired runtime `key` names (id, then name), the way `gizzi run --on`
 * and `gizzi mission start --on` look it up; or the line to print when there
 * is none.
 */
export async function findRuntime(key: string, list: () => Promise<Runtime[]> = listRuntimes): Promise<{ runtime: Runtime } | { error: string }> {
  const runtimes = await list().catch((e: Error) => e)
  if (runtimes instanceof Error) return { error: `Couldn't list paired runtimes: ${runtimes.message}` }
  const runtime = pick(runtimes, key)
  if (!runtime) return { error: `No paired runtime named or with id "${key}". Run \`gizzi computers ls\`.` }
  return { runtime }
}

/** `claude-cli/claude-sonnet-5` → `{ providerID, modelID }` (undefined without a provider). */
export function modelRef(model?: string): { providerID: string; modelID: string } | undefined {
  const [providerID, ...rest] = (model ?? "").split("/")
  return providerID && rest.length ? { providerID, modelID: rest.join("/") } : undefined
}

const RELAY_KEY_NEEDED =
  "Running on another computer goes through the Allternit relay, which needs an Allternit API key with the compute scope. Set ALLTERNIT_API_TOKEN (Settings → API keys) and try again."

/** Why the relay can't be used from here (no API key), or undefined. */
export function relayKeyProblem(): string | undefined {
  return process.env.ALLTERNIT_API_TOKEN?.trim() ? undefined : RELAY_KEY_NEEDED
}

/** A relayed request that the runtime (or the relay) answered with an error status; 0 = no answer in time. */
export class RelayError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly path: string,
  ) {
    super(message)
  }
  /** The relay or this client gave up waiting; the runtime may still be working on it. */
  get timedOut(): boolean {
    return this.status === 0 || this.status === 504 || /runtime_timeout/.test(this.message)
  }
}

/** One request to a paired runtime's gateway through the cloud relay; resolves with the parsed JSON body. */
export type RelayCall = (method: string, path: string, body?: unknown, timeoutMs?: number) => Promise<any>

/**
 * Requests to one paired runtime through the cloud relay
 * `POST /api/v1/runtime-devices/:id/proxy`. The relay checks a Clerk session
 * or an Allternit API key with the `compute` scope (ALLTERNIT_API_TOKEN); the
 * device token from `gizzi pair` only identifies this machine and is refused
 * there. The relay waits up to 90 s for the runtime's answer.
 */
export function runtimeRelay(runtimeId: string): RelayCall {
  const token = process.env.ALLTERNIT_API_TOKEN?.trim()
  if (!token) throw new Error(RELAY_KEY_NEEDED)
  return async (method, path, body, timeoutMs = 60_000) => {
    let res: Response
    try {
      res = await fetch(`${cloudBase()}/api/v1/runtime-devices/${encodeURIComponent(runtimeId)}/proxy`, {
        method: "POST",
        headers: { "content-type": "application/json", Authorization: `Bearer ${token}` },
        body: JSON.stringify({
          method,
          path,
          headers: { "content-type": "application/json" },
          body: body === undefined ? "" : JSON.stringify(body),
        }),
        signal: AbortSignal.timeout(timeoutMs),
      })
    } catch (e) {
      const err = e as Error
      const timedOut = err?.name === "TimeoutError" || err?.name === "AbortError"
      throw new RelayError(`relay ${path}: ${timedOut ? "no answer in time" : err?.message ?? String(e)}`, timedOut ? 0 : -1, path)
    }
    const raw = await res.text()
    if (!res.ok) throw new RelayError(`relay ${path}: ${res.status} ${raw.slice(0, 200)}`, res.status, path)
    return raw ? JSON.parse(raw) : {}
  }
}

function followLinks(runtimeId: string, sessionID: string): { web: string; phone: string } {
  const query = new URLSearchParams({ runtime: runtimeId, session: sessionID })
  return { web: `https://ai.allternit.com/?${query}`, phone: `https://m.allternit.com/?${query}` }
}

async function createRuntimeSession(relay: RelayCall, name: string, model?: { providerID: string; modelID: string }): Promise<string> {
  const created = await relay("POST", "/api/v1/agent-sessions", {
    name,
    origin_surface: "code",
    model,
    metadata: { sourceRef: { harness: "gizzi-cli" } },
  })
  const sessionID: string = created.id ?? created.session?.id ?? created.sessionId
  if (!sessionID) throw new Error("The computer did not return a session id.")
  return sessionID
}

/**
 * Start a session on another paired runtime through the cloud relay and send
 * it the first message. Fire-and-forget: it does not wait for the reply.
 */
export async function runOnRuntime(
  runtimeId: string,
  text: string,
  opts: { title?: string; model?: { providerID: string; modelID: string } } = {},
): Promise<{ sessionID: string; web: string; phone: string }> {
  const relay = runtimeRelay(runtimeId)
  const sessionID = await createRuntimeSession(relay, opts.title ?? text.slice(0, 60), opts.model)
  await relay("POST", `/api/v1/agent-sessions/${encodeURIComponent(sessionID)}/messages`, { text })
  return { sessionID, ...followLinks(runtimeId, sessionID) }
}

/** A message as the runtime's `/api/v1/agent-sessions/:id/messages` returns it. */
type RuntimeMessage = { id?: string; role?: string; content?: string; metadata?: { error?: unknown } | null }

const NO_TEXT = "[No text content]"

/**
 * Whether the runtime is running a turn in `sessionID`: allternit-api's
 * `GET /api/v1/agent-sessions/:id/status`, else gizzi's own
 * `GET /v1/session/status` (a BYO box whose gizzi serves the app protocol
 * itself). Throws when the runtime answers neither.
 */
async function runtimeSessionBusy(relay: RelayCall, sessionID: string): Promise<boolean> {
  let first: unknown
  try {
    const s = await relay("GET", `/api/v1/agent-sessions/${encodeURIComponent(sessionID)}/status`)
    if (typeof s?.busy === "boolean") return s.busy
    if (typeof s?.status === "string") return s.status !== "idle"
  } catch (e) {
    if (e instanceof RelayError && e.timedOut) return true // busy enough not to answer; look again
    first = e
  }
  try {
    const all = await relay("GET", "/v1/session/status")
    const type = all?.[sessionID]?.type
    return typeof type === "string" && type !== "idle"
  } catch (e) {
    if (e instanceof RelayError && e.timedOut) return true
    throw new Error(
      `the computer has no way to say when the turn is done (${(first as Error)?.message ?? "no status"}; ${(e as Error).message})`,
    )
  }
}

function replyText(msg: RuntimeMessage): string {
  const text = (msg.content ?? "").trim()
  if (msg.metadata?.error) throw new Error(text && text !== NO_TEXT ? text : "the turn on the computer failed")
  if (!text || text === NO_TEXT) throw new Error("the computer's turn replied with nothing")
  return text
}

/**
 * One turn on a paired runtime that waits for the reply: sends `prompt` to
 * `sessionID` (or a new session), then returns the assistant's reply text.
 *
 * The runtime's `POST /api/v1/agent-sessions/:id/messages` blocks until the
 * turn ends and answers with the assistant message. A turn longer than the
 * relay waits (90 s) comes back as a timeout while it keeps running there, so
 * the reply is then read the way the app reads it: poll the session's status
 * until it is idle, then take the newest assistant message that was not there
 * before the send. Throws (never runs anything locally) when the turn fails,
 * times out or the reply can't be read.
 */
export async function turnOnRuntime(
  runtimeId: string,
  prompt: string,
  opts: {
    sessionID?: string
    /** `provider/model`, e.g. `claude-cli/claude-sonnet-5`. */
    model?: string
    title?: string
    relay?: RelayCall
    pollMs?: number
    timeoutMs?: number
    sleep?: (ms: number) => Promise<void>
  } = {},
): Promise<{ sessionID: string; web: string; phone: string; text: string }> {
  const relay = opts.relay ?? runtimeRelay(runtimeId)
  const model = modelRef(opts.model)
  const pollMs = opts.pollMs ?? 5_000
  const deadline = Date.now() + (opts.timeoutMs ?? 15 * 60_000)
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
  const sessionID = opts.sessionID ?? (await createRuntimeSession(relay, opts.title ?? prompt.slice(0, 60), model))
  const messagesPath = `/api/v1/agent-sessions/${encodeURIComponent(sessionID)}/messages`
  const list = async (): Promise<RuntimeMessage[]> => {
    const body = await relay("GET", messagesPath)
    return Array.isArray(body) ? body : (body?.messages ?? [])
  }
  const before = new Set(opts.sessionID ? (await list()).map((m) => m.id) : [])
  const done = (msg: RuntimeMessage) => ({ sessionID, ...followLinks(runtimeId, sessionID), text: replyText(msg) })

  let sent: RuntimeMessage | undefined
  try {
    sent = await relay("POST", messagesPath, { text: prompt, ...(model ? { metadata: { model } } : {}) }, 100_000)
  } catch (e) {
    if (!(e instanceof RelayError && e.timedOut)) throw e
  }
  if (sent?.role === "assistant") return done(sent)

  for (;;) {
    await sleep(pollMs)
    if (await runtimeSessionBusy(relay, sessionID)) {
      if (Date.now() > deadline) throw new Error(`the turn on the computer is still running after ${Math.round((opts.timeoutMs ?? 15 * 60_000) / 60_000)} min`)
      continue
    }
    const fresh = (await list()).filter((m) => m.role === "assistant" && m.id && !before.has(m.id))
    const last = fresh[fresh.length - 1]
    if (!last) throw new Error("the turn on the computer ended without a reply")
    return done(last)
  }
}

/**
 * PATCH the runtime engine's own session record (`/v1/session/:id`, which
 * gizzi serves on every runtime and the relay allows), e.g. the `mission`
 * label and `glance` line Remote Control shows.
 */
export async function patchRuntimeSession(runtimeId: string, sessionID: string, body: unknown, relay: RelayCall = runtimeRelay(runtimeId)): Promise<void> {
  await relay("PATCH", `/v1/session/${encodeURIComponent(sessionID)}`, body, 10_000)
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
