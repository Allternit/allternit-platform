/**
 * Turn routing (Agency Kernel WP-R1: O1, O2, O14). SHADOW-FIRST.
 *
 * At the top of every chat/cowork/bot turn:
 *   1. The incumbent decides, exactly as before: `Provider.resolveAuto`
 *      (request-scorer tiers) for providerID "auto", else the user's model.
 *   2. The kernel router (one routing authority, O2) is asked for the turn's
 *      class/plan over the same pool snapshot (`POST {ALLTERNIT_API_URL}/api/v1/
 *      kernel/turn-route`, the Kernel UI backend's existing auth). Unreachable
 *      or failing → the incumbent stands (fail-safe, logged).
 *   3. S1 runs in shadow (O14) on `/v1/decision` with backend "auto": the ROUTE
 *      bank (what kind of turn) and the ROUTE_MODEL bank (which class). The
 *      answers are logged with their x-decision_id and never acted on.
 *   4. Outcome labels come from what the turn actually did (tools used,
 *      errors, a command template run → "template") and from the next turn
 *      (user retry / model switch), posted to `/v1/decision/outcome`. The
 *      session's last turn gets its ROUTE_MODEL label when the session ends
 *      (deleted, or idle for ALLTERNIT_TURN_ROUTE_IDLE_MS, default 30 min).
 *
 * `ALLTERNIT_TURN_ROUTE_SHADOW=0` turns steps 2–4 off. With
 * `ALLTERNIT_TURN_ROUTE_AUTHORITY=kernel` the kernel's pick replaces the
 * incumbent for "auto" turns (off by default until ROUTE_MODEL passes Q26).
 */
import { Log } from "@/shared/util/log"
import type { ModelPoolEntryV1 } from "@/runtime/model-pool/pool"
import { callTypeClass, escalate, genClassOf, GEN_CLASSES, type GenClass } from "@/runtime/model-pool/classes"

const log = Log.create({ service: "turn-router" })

export const ROUTE_BANK = "bank.route.v0"
export const ROUTE_MODEL_BANK = "bank.route_model.v0"
export const ROUTE_OPTIONS = [
  "answer_from_memory",
  "retrieval",
  "single_tool",
  "agent_run",
  "coding",
  "computer_use",
  "template",
  "clarify",
] as const
export type RouteOption = (typeof ROUTE_OPTIONS)[number]

export type ModelRef = { providerID: string; modelID: string }
type FetchLike = (url: string, init?: RequestInit) => Promise<Response>

export interface TurnRouterDeps {
  fetch: FetchLike
  /** Pool snapshot (gizzi's `GET /model-pool` entries). */
  pool: () => Promise<ModelPoolEntryV1[]>
  env: Record<string, string | undefined>
}

const defaultDeps = (): TurnRouterDeps => ({
  fetch: (u, i) => fetch(u, i),
  pool: async () => {
    const { Provider } = await import("@/runtime/providers/provider")
    const { buildModelPool, viewsFromProviders } = await import("@/runtime/model-pool/pool")
    return buildModelPool(viewsFromProviders((await Provider.list()) as any))
  },
  env: process.env,
})

let deps: TurnRouterDeps | undefined
/** Tests inject fetch/pool/env. */
export function setDeps(d: Partial<TurnRouterDeps> | undefined) {
  deps = d ? { ...defaultDeps(), ...d } : undefined
  pending.clear()
  templateNext.clear()
  for (const t of idleTimers.values()) clearTimeout(t)
  idleTimers.clear()
}
const D = () => (deps ??= defaultDeps())

export interface KernelTurnRoute {
  gen_class: GenClass | null
  backend_id: string
  max_output_tokens: number | null
  estimated_cost: number | null
  model_ref: string | null
}

export interface TurnRecord {
  sessionID: string
  text: string
  requested: ModelRef
  incumbent: ModelRef
  incumbentClass: GenClass | null
  /** Incumbent's pool cost (USD / 1k tokens); for the cost ledger's savings figure. */
  incumbentCost: number | null
  kernel: KernelTurnRoute | null
  kernelError?: string
  routeDecisionId?: string
  routeModelDecisionId?: string
  routeChoice?: string
  routeModelChoice?: string
  routeModelLabeled?: boolean
  /** The turn's user message came from a command template (WP-S1U-3). */
  template?: boolean
}

const pending = new Map<string, TurnRecord>()
/** Sessions whose next turn runs a command template (set by `SessionPrompt.command`). */
const templateNext = new Set<string>()
/** Idle timers that label a session's last ROUTE_MODEL when no next turn comes. */
const idleTimers = new Map<string, ReturnType<typeof setTimeout>>()
const DEFAULT_IDLE_MS = 30 * 60 * 1000

export function shadowEnabled(): boolean {
  return D().env.ALLTERNIT_TURN_ROUTE_SHADOW !== "0"
}

function s1Base(): { url: string; token?: string } {
  const env = D().env
  const url = (env.ALLTERNIT_S1_URL || env.SYSTEM_ONE_URL || "http://127.0.0.1:7717").replace(/\/+$/, "")
  return { url, token: env.SYSTEM_ONE_TOKEN || undefined }
}

function entryFor(pool: ModelPoolEntryV1[], ref: ModelRef) {
  return pool.find((e) => e.extensions?.["x-model_ref"] === `${ref.providerID}/${ref.modelID}`)
}

/** Ask the kernel router (allternit-api) for this turn's plan. null + reason on any failure. */
export async function kernelRoute(
  entries: ModelPoolEntryV1[],
  callType = "answer",
): Promise<{ route: KernelTurnRoute | null; error?: string }> {
  const env = D().env
  const origin = (env.ALLTERNIT_API_URL || "").trim().replace(/\/+$/, "")
  const token = env.ALLTERNIT_API_TOKEN?.trim() || env.ALLTERNIT_API_KEY?.trim()
  if (!origin || !token) return { route: null, error: "kernel router not configured (ALLTERNIT_API_URL / token)" }
  if (entries.length === 0) return { route: null, error: "empty model pool" }
  try {
    const res = await D().fetch(`${origin}/api/v1/kernel/turn-route`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ entries, call_type: callType }),
      signal: AbortSignal.timeout(1500),
    })
    if (!res.ok) return { route: null, error: `kernel router HTTP ${res.status}` }
    const b: any = await res.json()
    const chosen = entries.find((e) => e.backend_id === b?.backend_id)
    return {
      route: {
        gen_class: (GEN_CLASSES as string[]).includes(b?.gen_class) ? b.gen_class : null,
        backend_id: String(b?.backend_id ?? ""),
        max_output_tokens: typeof b?.max_output_tokens === "number" ? b.max_output_tokens : null,
        estimated_cost: typeof b?.estimated_cost === "number" ? b.estimated_cost : null,
        model_ref: typeof chosen?.extensions?.["x-model_ref"] === "string" ? (chosen.extensions["x-model_ref"] as string) : null,
      },
    }
  } catch (e) {
    return { route: null, error: `kernel router unreachable: ${(e as Error).message}` }
  }
}

/** One S1 shadow decision. Returns {decision_id, choice} or undefined. Never throws. */
export async function s1Decide(
  bank: string,
  primitive: string,
  instructions: string,
  options: readonly string[],
  state: string,
  runID: string,
): Promise<{ decision_id?: string; choice?: string } | undefined> {
  const { url, token } = s1Base()
  const candidates = [
    ...options.map((o) => ({ candidate_id: o, label: o })),
    { candidate_id: "unknown", label: "unknown", is_unknown: true },
  ]
  const body = {
    state: state.slice(-2000),
    reversible: true,
    backend: "auto",
    request: {
      envelope: {
        abi_version: "1.0.0",
        schema_id: "allternit.kernel.DecisionRequestV1",
        schema_version: "1.0.0",
        run_id: runID,
        node_id: bank,
      },
      operation: "CHOICE",
      state_projection_ref: `turn:${runID}`,
      instructions,
      decision_bank_id: bank,
      candidates,
      calibration_domain: primitive,
    },
  }
  try {
    const res = await D().fetch(`${url}/v1/decision`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(3000),
    })
    if (!res.ok) return undefined
    const r: any = await res.json()
    const choice = typeof r?.answer === "string" ? r.answer : (r?.answer?.candidate_id ?? r?.answer?.value)
    return { decision_id: r?.extensions?.["x-decision_id"], choice: typeof choice === "string" ? choice : undefined }
  } catch {
    return undefined
  }
}

export async function reportOutcome(decisionID: string | undefined, truth: string, source: string, extra: Record<string, unknown> = {}) {
  if (!decisionID) return false
  const { url, token } = s1Base()
  try {
    const res = await D().fetch(`${url}/v1/decision/outcome`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      // Extra x- fields ride along for the cost ledger (ignored by the S1 runtime today).
      body: JSON.stringify({ decision_id: decisionID, truth, source, ...extra }),
      signal: AbortSignal.timeout(1500),
    })
    return res.ok
  } catch {
    return false
  }
}

/**
 * Top of a turn. `incumbent` is what gizzi's existing router decided.
 * Returns the model to use: the incumbent, unless authority=kernel and the
 * kernel answered for an "auto" turn.
 */
export async function startTurn(input: {
  sessionID: string
  userMessageID: string
  text: string
  requested: ModelRef
  incumbent: ModelRef
}): Promise<{ model: ModelRef; record?: TurnRecord }> {
  const template = templateNext.delete(input.sessionID)
  clearIdle(input.sessionID)
  if (!shadowEnabled()) return { model: input.incumbent }
  let entries: ModelPoolEntryV1[] = []
  try {
    entries = await D().pool()
  } catch (e) {
    log.warn("pool unavailable", { error: (e as Error).message })
  }
  // The previous turn's ROUTE_MODEL label comes from what the user did next.
  const next = entryFor(entries, input.requested)
  labelPreviousRouteModel(input.sessionID, input.text, input.requested, next ? genClassOf(next) : undefined)
  const inc = entryFor(entries, input.incumbent)
  const record: TurnRecord = {
    sessionID: input.sessionID,
    text: input.text,
    requested: input.requested,
    incumbent: input.incumbent,
    incumbentClass: inc ? genClassOf(inc) : null,
    incumbentCost: inc ? inc.cost : null,
    kernel: null,
    template,
  }
  pending.set(input.sessionID, record)

  const kernelP = kernelRoute(entries, "answer").then((k) => {
    record.kernel = k.route
    if (k.error) record.kernelError = k.error
    return k
  })
  const runID = `${input.sessionID}:${input.userMessageID}`
  const s1P = Promise.all([
    s1Decide(ROUTE_BANK, "route.describe_cognitive_requirement", "classify what this turn needs", ROUTE_OPTIONS, input.text, runID),
    s1Decide(ROUTE_MODEL_BANK, "route.select_logical_model", "pick the smallest model class that can answer this turn", GEN_CLASSES, input.text, runID),
  ]).then(([route, routeModel]) => {
    record.routeDecisionId = route?.decision_id
    record.routeChoice = route?.choice
    record.routeModelDecisionId = routeModel?.decision_id
    record.routeModelChoice = routeModel?.choice
  })
  const logIt = () =>
    log.info("turn-route shadow", {
      sessionID: input.sessionID,
      incumbent: `${input.incumbent.providerID}/${input.incumbent.modelID}`,
      incumbent_class: record.incumbentClass,
      incumbent_cost: record.incumbentCost,
      kernel_class: record.kernel?.gen_class ?? null,
      kernel_cost: record.kernel?.estimated_cost ?? null,
      kernel_fallback: record.kernelError ?? null,
      route: record.routeChoice ?? null,
      route_model: record.routeModelChoice ?? null,
      "x-decision_id": [record.routeDecisionId, record.routeModelDecisionId].filter(Boolean),
    })

  const authority = D().env.ALLTERNIT_TURN_ROUTE_AUTHORITY === "kernel" && input.requested.providerID === "auto"
  if (!authority) {
    // Shadow: never block the turn on the kernel or S1.
    void Promise.allSettled([kernelP, s1P]).then(logIt)
    return { model: input.incumbent, record }
  }
  const k = await kernelP
  void s1P.finally(logIt)
  const ref = k.route?.model_ref
  if (!ref) {
    log.warn("kernel router fallback to incumbent", { reason: k.error ?? "no model_ref" })
    return { model: input.incumbent, record }
  }
  const [providerID, ...rest] = ref.split("/")
  return { model: { providerID, modelID: rest.join("/") }, record }
}

/** A tool that runs a template (e.g. an MCP `run_template` / `templates.run`). */
const TEMPLATE_TOOL = /(run|use|apply|exec|execute|start)[_.\-]?templates?($|[_.\-])|templates?[_.\-](run|use|apply|exec|execute|start)/

/**
 * What the turn actually needed, from its tool calls. "template" when the turn
 * ran a command template (`opts.template`) or called a template tool.
 */
export function routeLabel(tools: string[], opts: { template?: boolean } = {}): RouteOption {
  const t = tools.map((x) => x.toLowerCase())
  const has = (re: RegExp) => t.some((x) => re.test(x))
  if (opts.template || has(TEMPLATE_TOOL)) return "template"
  if (t.length === 0) return "answer_from_memory"
  if (has(/^(question|ask_?user)/)) return "clarify"
  if (has(/computer|browser_|desktop|screenshot|mouse|keyboard/)) return "computer_use"
  if (has(/^(edit|write|multiedit|patch|apply_patch)$/)) return "coding"
  if (has(/^(task|agent|subagent)$/) || t.length > 3) return "agent_run"
  if (t.length === 1 && !has(/^(read|grep|glob|list|ls|webfetch|websearch|search|memory)/)) return "single_tool"
  if (t.every((x) => /^(read|grep|glob|list|ls|webfetch|websearch|search|memory|codesearch)/.test(x))) return "retrieval"
  return t.length === 1 ? "single_tool" : "agent_run"
}

/** End of a turn: label the ROUTE decision from what the turn did. */
export async function finishTurn(sessionID: string, observed: { tools: string[]; errored: boolean }) {
  const rec = pending.get(sessionID)
  if (!rec) return
  const truth = routeLabel(observed.tools, { template: rec.template })
  await reportOutcome(rec.routeDecisionId, truth, rec.template ? "turn_template" : "turn_tools", { "x-tools": observed.tools.length })
  if (observed.errored && rec.incumbentClass && !rec.routeModelLabeled) {
    rec.routeModelLabeled = true
    await reportOutcome(rec.routeModelDecisionId, escalate(rec.incumbentClass), "turn_error", costExtra(rec))
  }
  // The last turn of a session has no next turn to label its ROUTE_MODEL: if
  // none comes within the idle window, label it accepted (see endSession).
  if (!rec.routeModelLabeled) armIdle(sessionID)
}

/** `SessionPrompt.command` marks the session's next turn as a template run. */
export function markTemplate(sessionID: string) {
  templateNext.add(sessionID)
}

function idleMs(): number {
  const n = Number(D().env.ALLTERNIT_TURN_ROUTE_IDLE_MS)
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_IDLE_MS
}

function clearIdle(sessionID: string) {
  const t = idleTimers.get(sessionID)
  if (t) clearTimeout(t)
  idleTimers.delete(sessionID)
}

function armIdle(sessionID: string) {
  clearIdle(sessionID)
  const t = setTimeout(() => void endSession(sessionID, "session_idle"), idleMs())
  ;(t as any).unref?.()
  idleTimers.set(sessionID, t)
}

/**
 * Session end (deleted, or idle with no next turn): the last turn's
 * ROUTE_MODEL is labelled with the incumbent's class (the person did not
 * retry or switch), and the session's record is dropped.
 */
export async function endSession(sessionID: string, source: "session_idle" | "session_deleted" | "session_end" = "session_end") {
  clearIdle(sessionID)
  templateNext.delete(sessionID)
  const rec = pending.get(sessionID)
  pending.delete(sessionID)
  if (!rec || rec.routeModelLabeled || !rec.incumbentClass) return
  rec.routeModelLabeled = true
  await reportOutcome(rec.routeModelDecisionId, rec.incumbentClass, source, costExtra(rec))
}

function costExtra(rec: TurnRecord) {
  // TODO(WP-C1 #1126): the cost ledger reads these for the S1 savings figure.
  return { "x-incumbent_cost": rec.incumbentCost, "x-kernel_cost": rec.kernel?.estimated_cost ?? null }
}

/** ROUTE_MODEL truth for the previous turn, from the user's next move. */
export function labelPreviousRouteModel(sessionID: string, nextText: string, nextRequested: ModelRef, nextClass?: GenClass) {
  const rec = pending.get(sessionID)
  if (!rec || rec.routeModelLabeled || !rec.incumbentClass) return
  rec.routeModelLabeled = true
  const switched = nextRequested.providerID !== rec.requested.providerID || nextRequested.modelID !== rec.requested.modelID
  const retried = nextText.trim() !== "" && nextText.trim() === rec.text.trim()
  let truth: GenClass = rec.incumbentClass
  let source = "turn_accepted"
  if (switched) {
    source = "user_model_switch"
    // The switched-to model's class, when the pool knows it; else one class up.
    truth = nextClass ?? escalate(rec.incumbentClass)
  } else if (retried) {
    source = "user_retry"
    truth = escalate(rec.incumbentClass)
  }
  void reportOutcome(rec.routeModelDecisionId, truth, source, costExtra(rec))
}

/** O5 hook: the kernel plan's output cap for this session's current turn, if any. */
export function outputCap(sessionID: string): number | undefined {
  const cap = pending.get(sessionID)?.kernel?.max_output_tokens
  return typeof cap === "number" && cap > 0 ? cap : undefined
}

export function current(sessionID: string): TurnRecord | undefined {
  return pending.get(sessionID)
}

/** Class a call type should run on (titles/summaries/extraction → gen.small). */
export { callTypeClass }
