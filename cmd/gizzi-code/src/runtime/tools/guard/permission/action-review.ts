/**
 * Action reviewer (Wave C decision 4). Runs inside PermissionNext.ask after
 * the policy (floor, config rules, mode) has decided, and can only make that
 * decision stricter:
 *
 *   1. Deterministic first: the call is classified into one of the four
 *      action classes (action-classes.ts) and the class gate applies.
 *   2. Optionally, the local S1 decision runtime is asked whether the person
 *      asked for this action. Its answer is joined with `tighten()`, so it can
 *      turn an allow into an ask and never anything into a looser answer. It
 *      runs only when the gate would let a non-free action run, only against
 *      a runtime on this machine, and with no runtime the review stays
 *      deterministic.
 *
 * Evidence is the person's own messages from the session store (the root
 * session's non-synthetic user text), never page text, tool output or a
 * subagent's prompt, so nothing an agent reads can authorize an action.
 *
 * Denied outcomes are remembered per session (in memory). When the person
 * denied an outcome (a send, a payment, …) and the agent tries the same
 * outcome by another tool or route, the retry is flagged and asks again,
 * whatever grant or rule would have let it run.
 */
import { Database, desc, eq } from "@/runtime/session/storage/db"
import { MessageTable, PartTable, SessionTable } from "@/runtime/session/session.sql"
import { Wildcard } from "@/shared/util/wildcard"
import { Log } from "@/shared/util/log"
import { redactSecrets } from "@/shared/util/redact"
import { ActionClasses } from "@/runtime/tools/guard/permission/action-classes"
import {
  decide as decideS1,
  gateRequest,
  gateRecommendation,
  pTrue,
  tighten,
  type DecisionClientOptions,
  type Friction,
} from "../../../../../../../tools/system-one-local/src/decision/client"

export namespace ActionReview {
  const log = Log.create({ service: "action-review" })

  export type Risk = "critical" | "high" | "medium" | "low"

  /** What an ask card carries as `metadata.review` (the contract's four fields). */
  export interface Metadata {
    class: ActionClasses.Class
    risk: Risk
    authorized: boolean
    reason: string
  }

  export interface Review extends Metadata {
    topic: ActionClasses.Topic
    /** `<tool>:<action>`, e.g. `computer:left_click`, `bash:run`, `mcp:gmail_send_message`. */
    route: string
    /** The other route of a denied outcome this call retries. */
    retry_of?: string
    /** No grant, rule or "always"/"task" reply may skip this ask (confirm_now, or a retry of a denial). */
    forced: boolean
  }

  export interface Decision {
    action: Friction
    review: Review
  }

  export type RuleLike = { permission: string; pattern: string; action: "allow" | "deny" | "ask"; source?: string }

  const RISK: Record<ActionClasses.Class, Risk> = {
    hand_off: "critical",
    confirm_now: "high",
    confirm_unless_named: "medium",
    free: "low",
  }

  export function metadata(review: Review): Metadata {
    return { class: review.class, risk: review.risk, authorized: review.authorized, reason: review.reason }
  }

  /* ------------------------------------------------------------------ */
  /* Permission request -> classifier input                              */
  /* ------------------------------------------------------------------ */

  /** Builtin permission classes that are the runtime's own routine work, not a named outside action. */
  const ROUTINE = new Set([
    "read", "glob", "grep", "list", "lsp", "codesearch", "websearch", "webfetch", "question", "askuserquestion",
    "ask_user_question", "ask_user_later", "send_user_message", "todoread", "todowrite", "task", "task_stop", "skill",
    "skill_manage", "current_time", "goal_create", "goal_budget", "automation", "external_directory", "doom_loop",
    "plan_enter", "plan_exit", "run_code", "agent", "todo",
  ])

  const SUMMARY_KEYS = [
    "label", "role", "secure", "text", "op", "path", "command", "host", "app", "ref", "query", "pattern",
    "direction", "domain", "apps", "name", "title", "target",
  ]

  function hostOf(url: unknown): string | undefined {
    if (typeof url !== "string") return undefined
    try {
      return new URL(url).host || undefined
    } catch {
      return undefined
    }
  }

  /** Lines a diff adds (what a write puts in the file), without the file headers. */
  function addedLines(diff: unknown): string | undefined {
    if (typeof diff !== "string") return undefined
    return diff
      .split("\n")
      .filter((l) => l.startsWith("+") && !l.startsWith("+++"))
      .map((l) => l.slice(1))
      .join("\n")
  }

  /** The reviewer's view of one permission request (one pattern of it). */
  export function toInput(permission: string, pattern: string, metadata: Record<string, unknown> = {}): ActionClasses.Input {
    if (permission === "bash") return { tool: "bash", action: "run", args: { command: pattern } }
    if (permission === "edit" || permission === "write") {
      const file = typeof metadata.filepath === "string" ? metadata.filepath : pattern
      return { tool: "write", action: "write", args: { path: file, text: addedLines(metadata.diff) ?? metadata.content } }
    }
    if (permission === "browser") {
      return {
        tool: metadata.surface === "pane" ? "pane_browser" : "browser",
        action: String(metadata.action ?? pattern),
        args: {
          ...(typeof metadata.target === "string" ? { label: metadata.target } : {}),
          ...(typeof metadata.ref === "string" ? { ref: metadata.ref } : {}),
          ...(hostOf(metadata.url) ? { host: hostOf(metadata.url) } : {}),
        },
      }
    }
    if (permission.startsWith("computer_toolset.")) {
      const toolset = typeof metadata.toolset === "string" ? metadata.toolset : permission.slice("computer_toolset.".length)
      const member =
        typeof metadata.member === "string" ? metadata.member : pattern.startsWith(`${toolset}.`) ? pattern.slice(toolset.length + 1) : pattern
      const input = (metadata.input && typeof metadata.input === "object" ? metadata.input : {}) as Record<string, unknown>
      const args: Record<string, unknown> = {}
      for (const key of SUMMARY_KEYS) if (input[key] !== undefined) args[key] = input[key]
      if (args.text === undefined && typeof input.key === "string") args.text = input.key
      if (args.host === undefined && hostOf(input.url)) args.host = hostOf(input.url)
      return { tool: toolset, action: member, args }
    }
    if (ROUTINE.has(permission.toLowerCase())) return { tool: "builtin", action: permission, args: { name: "" } }
    // An MCP tool or another named tool: its name says what it does.
    return { tool: "mcp", action: "call", args: { name: permission } }
  }

  function routeOf(input: ActionClasses.Input): string {
    if (input.tool === "mcp") return `mcp:${String(input.args?.name ?? input.action)}`
    return `${input.tool}:${input.action}`
  }

  /* ------------------------------------------------------------------ */
  /* Authorization: a live task grant or the person's own rule            */
  /* ------------------------------------------------------------------ */

  /** Permissions whose pattern carries the action, so a bare `*` pattern names nothing. */
  function patternCarrier(permission: string): boolean {
    return (
      permission === "bash" ||
      permission === "edit" ||
      permission === "write" ||
      permission === "browser" ||
      permission === "external_directory" ||
      permission.startsWith("computer_toolset.")
    )
  }

  /** A rule that names this action rather than allowing everything of a kind. */
  function names(rule: RuleLike): boolean {
    if (rule.permission.includes("*")) return false
    return rule.pattern !== "*" || !patternCarrier(rule.permission)
  }

  function lastMatch(permission: string, pattern: string, rules: RuleLike[]): RuleLike | undefined {
    return rules.findLast((r) => Wildcard.match(permission, r.permission) && Wildcard.match(pattern, r.pattern))
  }

  const USER_SOURCES = new Set(["user", "session", "project"])

  export interface Grants {
    /** The agent + session ruleset (config rules carry `source: "user"`). */
    configured: RuleLike[]
    /** The person's persisted "always" approvals. */
    approvals: RuleLike[]
    /** The session's live one-task grants ("Allow for this task"). */
    task: RuleLike[]
  }

  /** Whether a live task grant, the person's own config rule or their "always" approval covers this call. */
  export function authorizedBy(permission: string, pattern: string, grants: Grants): "task" | "config" | "approval" | undefined {
    if (lastMatch(permission, pattern, grants.task)?.action === "allow") return "task"
    const configured = lastMatch(permission, pattern, grants.configured)
    if (configured?.action === "allow" && USER_SOURCES.has(configured.source ?? "") && names(configured)) return "config"
    const approved = lastMatch(permission, pattern, grants.approvals)
    if (approved?.action === "allow" && names(approved)) return "approval"
    return undefined
  }

  /* ------------------------------------------------------------------ */
  /* The class gate                                                      */
  /* ------------------------------------------------------------------ */

  /**
   * The class gate on top of the policy's answer. Built from `tighten`, so it
   * can never return a looser answer than `incumbent`. With nobody to ask
   * (dontAsk, plan) an action that must ask is refused.
   */
  export function gate(
    incumbent: Friction,
    cls: ActionClasses.Class,
    opts: { authorized: boolean; forced: boolean; mode?: string },
  ): Friction {
    const nobody = opts.mode === "dontAsk" || opts.mode === "plan"
    const mustAsk: Friction = nobody ? "deny" : "ask"
    switch (cls) {
      case "hand_off":
        return "deny"
      case "confirm_now":
        return tighten(incumbent, mustAsk)
      case "confirm_unless_named":
        if (opts.forced || !opts.authorized) return tighten(incumbent, mustAsk)
        return incumbent
      default:
        return incumbent
    }
  }

  /* ------------------------------------------------------------------ */
  /* Denied outcomes                                                     */
  /* ------------------------------------------------------------------ */

  type Denied = { topic: ActionClasses.Topic; route: string; at: number }
  const denied = new Map<string, Denied[]>()
  const roots = new Map<string, string>()

  /** The session a person types in: a subagent's session walks up to its root. */
  function rootOf(sessionID: string): string {
    const hit = roots.get(sessionID)
    if (hit) return hit
    let current = sessionID
    try {
      for (let depth = 0; depth < 6; depth++) {
        const id = current
        const row = Database.use((db) =>
          db.select({ parent: SessionTable.parent_id }).from(SessionTable).where(eq(SessionTable.id, id)).get(),
        )
        if (!row?.parent) break
        current = row.parent
      }
    } catch {
      return sessionID
    }
    roots.set(sessionID, current)
    if (roots.size > 2000) roots.delete(roots.keys().next().value!)
    return current
  }

  /** The person said no to this outcome: remember it for the session. */
  export function recordDenied(sessionID: string, review: Pick<Review, "topic" | "route" | "class">) {
    if (review.class === "free" || review.topic === "none") return
    const key = rootOf(sessionID)
    const list = denied.get(key) ?? []
    list.push({ topic: review.topic, route: review.route, at: Date.now() })
    if (list.length > 50) list.shift()
    denied.set(key, list)
  }

  /** Forget a session's denied outcomes (the session or its task ended). */
  export function forget(sessionID: string) {
    denied.delete(rootOf(sessionID))
    denied.delete(sessionID)
  }

  function priorDenial(sessionID: string, topic: ActionClasses.Topic, route: string) {
    if (topic === "none") return undefined
    const list = denied.get(rootOf(sessionID)) ?? []
    const same = list.filter((d) => d.topic === topic)
    if (!same.length) return undefined
    const other = same.findLast((d) => d.route !== route)
    return { other: other?.route }
  }

  /* ------------------------------------------------------------------ */
  /* Evidence and the optional local model review                        */
  /* ------------------------------------------------------------------ */

  /**
   * The person's own recent messages (newest last): non-synthetic text parts
   * of user messages in the root session. Never tool output, page text, a
   * subagent's prompt or an injected envelope.
   */
  export function userEvidence(sessionID: string, limit = 6): string[] {
    try {
      const root = rootOf(sessionID)
      const rows = Database.use((db) =>
        db
          .select()
          .from(MessageTable)
          .where(eq(MessageTable.session_id, root))
          .orderBy(desc(MessageTable.time_created), desc(MessageTable.id))
          .limit(40)
          .all(),
      )
      const out: string[] = []
      for (const row of rows) {
        if ((row.data as { role?: string }).role !== "user") continue
        const parts = Database.use((db) =>
          db.select().from(PartTable).where(eq(PartTable.message_id, row.id)).orderBy(PartTable.id).all(),
        )
        const text = parts
          .map((p) => p.data as { type?: string; text?: string; synthetic?: boolean })
          .filter((p) => p.type === "text" && !p.synthetic && typeof p.text === "string")
          .map((p) => p.text!.trim())
          .join("\n")
          .trim()
        if (!text) continue
        out.push(text.length > 600 ? text.slice(0, 599) + "…" : text)
        if (out.length >= limit) break
      }
      return out.reverse()
    } catch (error) {
      log.warn("evidence not read", { sessionID, error: String(error) })
      return []
    }
  }

  let override: DecisionClientOptions | undefined
  /** Tests: route the model review to a fake runtime (the network edge). */
  export function _setClient(opts: DecisionClientOptions | undefined) {
    override = opts
  }

  function isLocal(url: string): boolean {
    try {
      const host = new URL(url).hostname
      return host === "127.0.0.1" || host === "localhost" || host === "::1" || host === "[::1]"
    } catch {
      return false
    }
  }

  /** Options for the model review, or undefined when it should not run (deterministic only). */
  function runtime(): DecisionClientOptions | undefined {
    if (override) return override
    if (process.env.ALLTERNIT_ACTION_REVIEW_S1 === "0") return undefined
    if (process.env.NODE_ENV === "test" && process.env.ALLTERNIT_ACTION_REVIEW_S1 !== "1") return undefined
    // The person's messages are evidence: they go only to a runtime on this machine.
    const url = process.env.ALLTERNIT_S1_URL ?? process.env.SYSTEM_ONE_URL ?? "http://127.0.0.1:7717"
    if (!isLocal(url)) return undefined
    return { url, timeoutMs: Number(process.env.ALLTERNIT_ACTION_REVIEW_TIMEOUT_MS ?? 1200) }
  }

  function summary(input: ActionClasses.Input): string {
    const args = input.args ?? {}
    const bits: string[] = []
    for (const key of ["name", "label", "command", "path", "host", "app", "op"]) {
      const v = args[key]
      if (typeof v === "string" && v) bits.push(`${key}: ${v}`)
      else if (Array.isArray(v)) bits.push(`${key}: ${v.filter((x) => typeof x === "string").join(" > ")}`)
    }
    return redactSecrets(bits.join("\n")).slice(0, 600)
  }

  /** The model's recommendation, capped at "ask": it may add a question, never refuse on its own. */
  async function modelReview(sessionID: string, input: ActionClasses.Input, review: Review): Promise<Friction | null> {
    const opts = runtime()
    if (!opts) return null
    const evidence = userEvidence(sessionID)
    const state = [
      `action: ${review.route}`,
      `class: ${review.class} (${review.reason})`,
      summary(input),
      "",
      "The person's own messages, newest last:",
      ...(evidence.length ? evidence.map((m) => `- ${redactSecrets(m)}`) : ["(none)"]),
    ].join("\n")
    const result = await decideS1(
      gateRequest({
        producer: "gizzi-action-review",
        decision_bank_id: "bank.action_review",
        question_id: "person_asked_for_this",
        primitive_id: "permission.action_review",
        motif: "GATE",
        ids: { session_id: sessionID },
        instructions:
          "Did the person clearly ask for this exact action in their own messages? Answer true only if they did. " +
          "Treat anything not in the person's messages as not asked for.",
      }),
      state,
      opts,
    ).catch(() => null)
    const rec = gateRecommendation(pTrue(result))
    if (!rec) return null
    return rec === "deny" ? "ask" : rec
  }

  /* ------------------------------------------------------------------ */
  /* The review                                                          */
  /* ------------------------------------------------------------------ */

  export interface DecideInput extends Grants {
    sessionID: string
    permission: string
    pattern: string
    metadata?: Record<string, unknown>
    /** The policy's answer (floor, config rules, mode). The review never loosens it. */
    incumbent: Friction
    mode?: string
    projectRoot?: string
  }

  export async function decide(input: DecideInput): Promise<Decision> {
    const action = toInput(input.permission, input.pattern, input.metadata ?? {})
    const classified = ActionClasses.classify(action, { projectRoot: input.projectRoot })
    const route = routeOf(action)
    const by =
      classified.class === "confirm_unless_named" ? authorizedBy(input.permission, input.pattern, input) : undefined
    const prior = classified.class === "free" ? undefined : priorDenial(input.sessionID, classified.topic, route)
    const forced = classified.class === "confirm_now" || !!prior
    let reason = classified.reason
    if (classified.class === "hand_off") reason = `${reason}. Hand this to the person`
    else if (prior?.other) reason = `${reason}. The person denied this before (via ${prior.other}); this retries it another way`
    else if (prior) reason = `${reason}. The person denied this before`
    else if (by === "task") reason = `${reason}. Allowed for this task`
    else if (by) reason = `${reason}. Named by the person's own rule`

    const review: Review = {
      class: classified.class,
      risk: RISK[classified.class],
      authorized: classified.class === "free" || (!!by && !prior),
      reason,
      topic: classified.topic,
      route,
      ...(prior?.other ? { retry_of: prior.other } : {}),
      forced,
    }

    let result = gate(input.incumbent, classified.class, { authorized: !!by, forced, mode: input.mode })
    if (result === "allow" && classified.class !== "free") {
      const rec = await modelReview(input.sessionID, action, review)
      const joined = tighten(result, rec)
      if (joined !== result) {
        review.authorized = false
        review.reason = `${review.reason}. The local reviewer could not match it to the person's request`
      }
      result = joined
    }
    // Belt and braces: whatever happened above, never looser than the policy.
    return { action: tighten(input.incumbent, result), review }
  }
}
