import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Config } from "@/runtime/context/config/config"
import { Flag } from "@/runtime/context/flag/flag"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Database, eq } from "@/runtime/session/storage/db"
import { PermissionTable, SessionTable } from "@/runtime/session/session.sql"
import { fn } from "@/shared/util/fn"
import { Log } from "@/shared/util/log"
import { Wildcard } from "@/shared/util/wildcard"
import os from "os"
import z from "zod/v4"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"

export namespace PermissionNext {
  const log = Log.create({ service: "permission" })

  function expand(pattern: string): string {
    if (pattern.startsWith("~/")) return os.homedir() + pattern.slice(1)
    if (pattern === "~") return os.homedir()
    if (pattern.startsWith("$HOME/")) return os.homedir() + pattern.slice(5)
    if (pattern.startsWith("$HOME")) return os.homedir() + pattern.slice(5)
    return pattern
  }

  export const Action = z.enum(["allow", "deny", "ask"])
  export type Action = z.infer<typeof Action>

  export const Rule = z
    .object({
      permission: z.string(),
      pattern: z.string(),
      action: Action,
      source: z.enum(["default", "agent", "user", "session", "project"]).optional(),
    })

  export type Rule = z.infer<typeof Rule>

  export const Ruleset = Rule.array()
  export type Ruleset = z.infer<typeof Ruleset>

  export function fromConfig(permission: Config.Permission, source?: Rule["source"]) {
    const ruleset: Ruleset = []
    for (const [key, value] of Object.entries(permission)) {
      if (typeof value === "string") {
        ruleset.push({
          permission: key,
          action: value,
          pattern: "*",
          ...(source ? { source } : {}),
        })
        continue
      }
      ruleset.push(
        ...Object.entries(value).map(([pattern, action]) => ({
          permission: key,
          pattern: expand(pattern),
          action,
          ...(source ? { source } : {}),
        })),
      )
    }
    return ruleset
  }

  export function merge(...rulesets: Ruleset[]): Ruleset {
    return rulesets.flat()
  }

  export const Request = z
    .object({
      id: Identifier.schema("permission"),
      sessionID: Identifier.schema("session"),
      permission: z.string(),
      patterns: z.string().array(),
      metadata: z.record(z.string(), z.any()),
      always: z.string().array(),
      tool: z
        .object({
          messageID: z.string(),
          callID: z.string(),
        })
        .optional(),
    })
    

  export type Request = z.infer<typeof Request>

  export const Reply = z.enum(["once", "always", "reject"])
  export type Reply = z.infer<typeof Reply>

  /**
   * Permission classes that always ask a person, in every mode — auto, yolo,
   * bypassPermissions and GIZZI_SKIP_PERMISSIONS included — and are never
   * remembered by an "always" reply. `subscription` gates Subscription Fabric
   * work no one sent from the chat composer (a task an agent, tool or bot
   * prepared; a provider's mid-task question): D16 makes each one a human act.
   * Only a configured deny, plan mode or dontAsk (nobody there to ask) turn
   * the ask into a refusal; nothing turns it into an approval.
   */
  export const ALWAYS_ASK = new Set(["subscription"])

  /**
   * What the person's reply carried beyond the decision. For `subscription`
   * asks the platform relays the human action it minted when the person
   * approved (`humanAction`) and their answer to a provider question.
   */
  export interface ReplyData {
    humanAction?: string
    answer?: string
  }

  export const Mode = z.enum(["default", "manual", "plan", "acceptEdits", "dontAsk", "auto", "yolo", "bypassPermissions"])
  export type Mode = z.infer<typeof Mode>

  export const Approval = z.object({
    projectID: z.string(),
    patterns: z.string().array(),
  })

  export const Event = {
    Asked: BusEvent.define("permission.asked", Request),
    Replied: BusEvent.define(
      "permission.replied",
      z.object({
        sessionID: z.string(),
        requestID: z.string(),
        reply: Reply,
      }),
    ),
  }

  const state = Instance.state(() => {
    const projectID = Instance.project.id
    const row = Database.use((db) =>
      db.select().from(PermissionTable).where(eq(PermissionTable.project_id, projectID)).get(),
    )
    const stored = row?.data ?? ([] as Ruleset)

    const pending: Record<
      string,
      {
        info: Request
        ruleset: Ruleset
        resolve: (data?: ReplyData) => void
        reject: (e: any) => void
      }
    > = {}

    return {
      pending,
      approved: stored,
      modes: {} as Record<string, Mode>,
    }
  })

  export async function getMode(sessionID: string): Promise<Mode> {
    const s = await state()
    if (s.modes[sessionID]) return s.modes[sessionID]
    const row = Database.use((db) => db
      .select({ mode: SessionTable.permission_mode })
      .from(SessionTable)
      .where(eq(SessionTable.id, sessionID))
      .get())
    const configured = Mode.safeParse(row?.mode ?? Flag.GIZZI_PERMISSION_MODE ?? "default")
    const mode = configured.success ? configured.data : "default"
    s.modes[sessionID] = mode
    return mode
  }

  export async function setMode(sessionID: string, mode: Mode): Promise<void> {
    const s = await state()
    s.modes[sessionID] = mode
    Database.use((db) => db.update(SessionTable)
      .set({ permission_mode: mode, time_updated: Date.now() })
      .where(eq(SessionTable.id, sessionID))
      .run())
  }

  export const ask = fn(
    Request.partial({ id: true }).extend({
      ruleset: Ruleset,
      // Optional per-call mode override (e.g. the mobile composer's per-message
      // `toolAccess` option). When set, it is used instead of the session's
      // persisted mode for this evaluation only — the session mode is never
      // read or written.
      mode: Mode.optional(),
    }),
    async (input): Promise<ReplyData | undefined> => {
      const s = await state()
      const { ruleset, mode: modeOverride, ...request } = input
      for (const pattern of request.patterns ?? []) {
        const rule = evaluatePolicy(request.permission, pattern, {
          configured: ruleset,
          approvals: s.approved,
          mode: modeOverride ?? (await getMode(request.sessionID)),
        })
        log.info("evaluated", { permission: request.permission, pattern, action: rule })
        if (rule.action === "deny")
          throw new DeniedError(ruleset.filter((r) => Wildcard.match(request.permission, r.permission)))
        if (rule.action === "ask") {
          const id = input.id ?? Identifier.ascending("permission")
          await HookDispatcher.emit({
            name: "PermissionRequest",
            timestamp: Date.now(),
            sessionId: request.sessionID,
            payload: { tool: request.permission, patterns: request.patterns, requestID: id },
          })
          return new Promise<ReplyData | undefined>((resolve, reject) => {
            const info: Request = {
              id,
              ...request,
            }
            s.pending[id] = {
              info,
              ruleset,
              resolve,
              reject,
            }
            Bus.publish(Event.Asked, info)
          })
        }
        if (rule.action === "allow") continue
      }
    },
  )

  /**
   * Take back one open ask without touching the session's other asks — for a
   * turn that stopped while its card was open. `reply(reject)` would reject
   * every pending ask on the session.
   */
  export async function withdraw(requestID: string) {
    const s = await state()
    const existing = s.pending[requestID]
    if (!existing) return
    delete s.pending[requestID]
    Bus.publish(Event.Replied, { sessionID: existing.info.sessionID, requestID, reply: "reject" })
    existing.reject(new RejectedError())
  }

  export const reply = fn(
    z.object({
      requestID: Identifier.schema("permission"),
      reply: Reply,
      message: z.string().optional(),
      /** Minted by allternit-api when a person approved a subscription card. */
      humanAction: z.string().optional(),
      /** The person's answer to a provider question (subscription asks). */
      answer: z.string().optional(),
    }),
    async (input) => {
      const s = await state()
      const existing = s.pending[input.requestID]
      if (!existing) return
      delete s.pending[input.requestID]
      // An always-ask class is never remembered: "always" counts as "once".
      if (input.reply === "always" && ALWAYS_ASK.has(existing.info.permission)) {
        input = { ...input, reply: "once" }
      }
      // Only always-ask classes carry reply data (a human action, an answer)
      // back to the asking tool; an ordinary approval never mints one.
      const data: ReplyData | undefined =
        ALWAYS_ASK.has(existing.info.permission) && (input.humanAction || input.answer !== undefined)
          ? {
              ...(input.humanAction ? { humanAction: input.humanAction } : {}),
              ...(input.answer !== undefined ? { answer: input.answer } : {}),
            }
          : undefined
      Bus.publish(Event.Replied, {
        sessionID: existing.info.sessionID,
        requestID: existing.info.id,
        reply: input.reply,
      })
      await HookDispatcher.emit({
        name: "PermissionResult",
        timestamp: Date.now(),
        sessionId: existing.info.sessionID,
        payload: { tool: existing.info.permission, requestID: existing.info.id, reply: input.reply },
      })
      if (input.reply === "reject") {
        existing.reject(input.message ? new CorrectedError(input.message) : new RejectedError())
        // A subscription card is one task's own decision (D16): declining it
        // leaves the session's other cards open.
        if (ALWAYS_ASK.has(existing.info.permission)) return
        // Reject all other pending permissions for this session
        const sessionID = existing.info.sessionID
        for (const [id, pending] of Object.entries(s.pending)) {
          if (pending.info.sessionID === sessionID) {
            delete s.pending[id]
            Bus.publish(Event.Replied, {
              sessionID: pending.info.sessionID,
              requestID: pending.info.id,
              reply: "reject",
            })
            pending.reject(new RejectedError())
          }
        }
        return
      }
      if (input.reply === "once") {
        existing.resolve(data)
        return
      }
      if (input.reply === "always") {
        for (const pattern of existing.info.always) {
          s.approved.push({
            permission: existing.info.permission,
            pattern,
            action: "allow",
          })
        }

        existing.resolve()

        const sessionID = existing.info.sessionID
        const mode = await getMode(sessionID)
        for (const [id, pending] of Object.entries(s.pending)) {
          if (pending.info.sessionID !== sessionID) continue
          const ok = pending.info.patterns.every(
            (pattern) => evaluatePolicy(pending.info.permission, pattern, {
              configured: pending.ruleset,
              approvals: s.approved,
              mode,
            }).action === "allow",
          )
          if (!ok) continue
          delete s.pending[id]
          Bus.publish(Event.Replied, {
            sessionID: pending.info.sessionID,
            requestID: pending.info.id,
            reply: "always",
          })
          pending.resolve()
        }

        // Persist approved permissions to disk so they survive restarts
        Database.use((db) =>
          db
            .insert(PermissionTable)
            .values({ project_id: Instance.project.id, data: s.approved })
            .onConflictDoUpdate({
              target: PermissionTable.project_id,
              set: { data: s.approved },
            })
            .run(),
        )
        return
      }
    },
  )

  const READONLY_PERMISSIONS = new Set([
    "read", "glob", "grep", "list", "websearch", "codesearch", "question", "todoread", "lsp",
  ])

  const EDIT_PERMISSIONS = new Set([
    "edit", "write", "patch", "multiedit",
  ])

  const AUTO_QUESTION_PERMISSIONS = new Set(["question", "askuserquestion", "ask_user_question"])

  export interface PolicyInput {
    configured: Ruleset
    approvals?: Ruleset
    mode?: string
    skipPermissions?: boolean
  }

  /** Ordered runtime policy composition.
   *
   * Rules inside the configured ruleset retain Allternit's documented
   * last-match behavior. Cross-policy precedence is explicit: hard host/plan
   * modes, configured denial, unattended-mode restrictions, session approval,
   * configured ask/allow, mode conveniences, then fallback.
   */
  export function evaluatePolicy(permission: string, pattern: string, input: PolicyInput): Rule {
    const mode = input.mode ?? Flag.GIZZI_PERMISSION_MODE
    const skipPermissions = input.skipPermissions ?? Flag.GIZZI_SKIP_PERMISSIONS

    if (ALWAYS_ASK.has(permission)) return alwaysAsk(permission, pattern, input.configured, mode)

    if (skipPermissions || mode === "bypassPermissions") {
      return { action: "allow", permission, pattern: "*" }
    }

    if (mode === "plan" && !READONLY_PERMISSIONS.has(permission)) {
      return { action: "deny", permission, pattern: "*" }
    }

    const configured = lastMatch(permission, pattern, input.configured)
    if (configured?.action === "deny") return configured

    if (mode === "auto" && AUTO_QUESTION_PERMISSIONS.has(permission.toLowerCase())) {
      return { action: "deny", permission, pattern: "*" }
    }
    if (mode === "auto") return { action: "allow", permission, pattern: "*" }

    const approved = lastMatch(permission, pattern, input.approvals ?? [])
    if (approved?.action === "allow") return approved

    if (configured) {
      if (mode === "dontAsk" && configured.action === "ask") {
        return { action: "deny", permission, pattern: configured.pattern }
      }
      if (
        mode === "yolo" &&
        configured.action === "ask" &&
        configured.source === "default" &&
        configured.permission === "*" &&
        configured.pattern === "*"
      ) {
        return { action: "allow", permission, pattern: "*" }
      }
      return configured
    }

    if (mode === "plan") return { action: "allow", permission, pattern: "*" }

    if (mode === "yolo") return { action: "allow", permission, pattern: "*" }

    if (mode === "acceptEdits" && (READONLY_PERMISSIONS.has(permission) || EDIT_PERMISSIONS.has(permission))) {
      return { action: "allow", permission, pattern: "*" }
    }

    // Non-interactive manual mode must never reinterpret an unresolved prompt
    // as consent. It turns the fallback ask into a deterministic denial.
    if (mode === "dontAsk") return { action: "deny", permission, pattern: "*" }

    return { action: "ask", permission, pattern: "*" }
  }

  /** Policy for an ALWAYS_ASK class: ask, or refuse — never allow. */
  function alwaysAsk(permission: string, pattern: string, configured: Ruleset, mode: string | undefined): Rule {
    const rule = lastMatch(permission, pattern, configured)
    if (rule?.action === "deny") return rule
    if (mode === "plan" || mode === "dontAsk") return { action: "deny", permission, pattern: "*" }
    return { action: "ask", permission, pattern: "*" }
  }

  function lastMatch(permission: string, pattern: string, ruleset: Ruleset): Rule | undefined {
    return ruleset.findLast(
      (rule) => Wildcard.match(permission, rule.permission) && Wildcard.match(pattern, rule.pattern),
    )
  }

  export function evaluate(permission: string, pattern: string, ...rulesets: Ruleset[]): Rule {
    const mode = Flag.GIZZI_PERMISSION_MODE

    if (ALWAYS_ASK.has(permission)) return alwaysAsk(permission, pattern, merge(...rulesets), mode)

    // bypassPermissions: skip all permission checks entirely
    if (Flag.GIZZI_SKIP_PERMISSIONS || mode === "bypassPermissions") {
      return { action: "allow", permission, pattern: "*" }
    }

    // plan: read-only mode — allow reads, deny all writes
    if (mode === "plan") {
      if (READONLY_PERMISSIONS.has(permission)) {
        return { action: "allow", permission, pattern: "*" }
      }
      return { action: "deny", permission, pattern: "*" }
    }

    // acceptEdits: auto-approve file edits, ask for bash/dangerous
    if (mode === "acceptEdits") {
      if (READONLY_PERMISSIONS.has(permission) || EDIT_PERMISSIONS.has(permission)) {
        return { action: "allow", permission, pattern: "*" }
      }
      // Fall through to normal ruleset evaluation for bash, external_directory, etc.
    }

    // default mode (or acceptEdits fallthrough): evaluate rulesets
    const merged = merge(...rulesets)
    log.info("evaluate", { permission, pattern, ruleset: merged })
    const match = lastMatch(permission, pattern, merged)
    if (!match && mode === "dontAsk") return { action: "deny", permission, pattern: "*" }
    if (!match && mode === "auto") {
      return AUTO_QUESTION_PERMISSIONS.has(permission.toLowerCase())
        ? { action: "deny", permission, pattern: "*" }
        : { action: "allow", permission, pattern: "*" }
    }
    return match ?? { action: "ask", permission, pattern: "*" }
  }

  const EDIT_TOOLS = ["edit", "write", "patch", "multiedit"]

  export function disabled(tools: string[], ruleset: Ruleset): Set<string> {
    const mode = Flag.GIZZI_PERMISSION_MODE
    // In bypass mode nothing is disabled. dontAsk keeps tools visible so
    // explicitly allowed calls still work; unresolved calls are denied at use.
    if (Flag.GIZZI_SKIP_PERMISSIONS || mode === "bypassPermissions" || mode === "dontAsk") {
      return new Set<string>()
    }

    const result = new Set<string>()
    for (const tool of tools) {
      const permission = EDIT_TOOLS.includes(tool) ? "edit" : tool

      // In plan mode, all non-read tools are disabled
      if (mode === "plan" && !READONLY_PERMISSIONS.has(permission)) {
        result.add(tool)
        continue
      }

      const rule = ruleset.findLast((r) => Wildcard.match(permission, r.permission))
      if (!rule) continue
      if (rule.pattern === "*" && rule.action === "deny") result.add(tool)
    }
    return result
  }

  /** User rejected without message - halts execution */
  export class RejectedError extends Error {
    constructor() {
      super(`The user rejected permission to use this specific tool call.`)
    }
  }

  /** User rejected with message - continues with guidance */
  export class CorrectedError extends Error {
    constructor(message: string) {
      super(`The user rejected permission to use this specific tool call with the following feedback: ${message}`)
    }
  }

  /** Auto-rejected by config rule - halts execution */
  export class DeniedError extends Error {
    constructor(public readonly ruleset: Ruleset) {
      super(
        `The user has specified a rule which prevents you from using this specific tool call. Here are some of the relevant rules ${JSON.stringify(ruleset)}`,
      )
    }
  }

  export async function list() {
    const s = await state()
    return Object.values(s.pending).map((x) => x.info)
  }
}
