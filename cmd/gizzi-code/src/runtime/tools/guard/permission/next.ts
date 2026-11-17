import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Config } from "@/runtime/context/config/config"
import { Flag } from "@/runtime/context/flag/flag"
import { Identifier } from "@/shared/id/id"
import { Instance } from "@/runtime/context/project/instance"
import { Database, desc, eq } from "@/runtime/session/storage/db"
import { MessageTable, PartTable, PermissionTable, SessionTable } from "@/runtime/session/session.sql"
import { fn } from "@/shared/util/fn"
import { Log } from "@/shared/util/log"
import { Wildcard } from "@/shared/util/wildcard"
import os from "os"
import z from "zod/v4"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"
import { Catastrophic } from "@/runtime/tools/guard/permission/catastrophic"

import { KernelTurn } from "@/runtime/kernel/compilers/turn-hook"
import { PermissionS1 } from "@/runtime/tools/guard/permission/s1-shadow"
import { ProjectPanels } from "@/runtime/project/panels"
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

  /**
   * `task` = allow this kind of request for the rest of this session only (the
   * bot thread's task). Kept in memory per session, never persisted, cleared by
   * clearTask() when the session ends or its thread resolves/archives. Never
   * applies to ALWAYS_ASK classes or TASK_NEVER (payment, sign, delete): there
   * it counts as "once".
   */
  export const Reply = z.enum(["once", "always", "task", "reject"])
  export type Reply = z.infer<typeof Reply>

  /**
   * Permission classes that always ask a person, in every mode — auto, yolo,
   * bypassPermissions and GIZZI_SKIP_PERMISSIONS included — and are never
   * remembered by an "always" reply. `subscription` gates Subscription Fabric
   * work no one sent from the chat composer (a task an agent, tool or bot
   * prepared; a provider's mid-task question): D16 makes each one a human act.
   * Only a configured deny, plan mode or dontAsk (nobody there to ask) turn
   * the ask into a refusal; nothing turns it into an approval.
   *
   * `mcp_app` gates a model-initiated call to an MCP App tool whose install
   * permission mode (allternit-api) requires the user's confirmation. The mode
   * is the user's own choice, so no session mode or earlier approval waives it.
   *
   * `project_send` gates a bot's main chat sending work into another project:
   * the person approves every send, with the project and full message on the card.
   */
  export const ALWAYS_ASK = new Set(["subscription", "mcp_app", "project_send"])

  /**
   * Classes a one-task grant ("Allow for this task") never covers: money,
   * signatures and deletions are asked every time. Matches allternit-api
   * task_grants::NEVER_GRANTED.
   */
  export const TASK_NEVER = new Set(["payment", "pay", "sign", "signature", "delete", "remove"])

  /** Whether a "task" reply may be remembered for this permission class. */
  export function taskGrantable(permission: string): boolean {
    const p = permission.toLowerCase()
    if (ALWAYS_ASK.has(p)) return false
    for (const never of TASK_NEVER) if (p === never || p.startsWith(`${never}_`) || p.endsWith(`_${never}`)) return false
    return true
  }

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
      /** Session-scoped allows from a "task" reply; never written to disk. */
      task: {} as Record<string, Ruleset>,
      modes: {} as Record<string, Mode>,
      /** Sessions known to be app (non-CLI) sessions; only positives are cached. */
      apps: new Set<string>(),
    }
  })

  /**
   * Whether a session belongs to an app (Desktop, ai.allternit.com, the PWA)
   * rather than the gizzi CLI. Auto mode is stricter there: deletes, payments,
   * signing and the user's own ask rules still ask, and question cards work.
   *
   * The `serve` process allternit-api talks to runs with the default
   * GIZZI_CLIENT ("cli"), so the process flag alone can't tell. The session
   * can: allternit-api stamps a `surface` (chat, cowork, code, browser,
   * design) on every session it creates, and the CLI never sets one. A
   * non-CLI client (GIZZI_CLIENT acp/app/desktop) counts as an app too.
   */
  export async function isAppSession(sessionID: string): Promise<boolean> {
    if (Flag.GIZZI_CLIENT !== "cli") return true
    const s = await state()
    if (s.apps.has(sessionID)) return true
    const row = Database.use((db) => db
      .select({ surface: SessionTable.surface })
      .from(SessionTable)
      .where(eq(SessionTable.id, sessionID))
      .get())
    if (!row?.surface) return false
    s.apps.add(sessionID)
    return true
  }

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
      const app = await isAppSession(request.sessionID)
      const destructive = request.metadata?.destructive === true
      for (const pattern of request.patterns ?? []) {
        const mode = modeOverride ?? (await getMode(request.sessionID))
        let rule = evaluatePolicy(request.permission, pattern, {
          configured: ruleset,
          approvals: approvalsFor(s, request.sessionID),
          mode,
          app,
          destructive,
        })
        // An approved project panel guard (.allternit/panels, agent.guards)
        // turns an allow into an ask, in every mode; with nobody there to
        // ask (dontAsk) it refuses. It never loosens a deny or an ask.
        if (rule.action === "allow") {
          const guarded = await ProjectPanels.guardFor(request.permission, pattern)
          if (guarded) {
            rule = { action: mode === "dontAsk" ? "deny" : "ask", permission: request.permission, pattern, source: "project" }
          }
        }
        log.info("evaluated", { permission: request.permission, pattern, action: rule })
        const callID = request.tool?.callID
        // Q27 shadow S1 GATE: logged after the floor/policy decided; never awaited, never changes `rule`.
        const askID = rule.action === "ask" ? (input.id ?? Identifier.ascending("permission")) : undefined
        PermissionS1.shadow({
          permission: request.permission,
          pattern,
          mode,
          incumbent: rule.action,
          sessionID: request.sessionID,
          subject_ref: askID ? PermissionS1.subjectForRequest(askID) : PermissionS1.subjectForCall(callID, request.permission, pattern),
        })
        if (rule.action === "deny") {
          const floor = Catastrophic.applies(request.permission) ? Catastrophic.check(pattern) : undefined
          // WP9: hand the gate's real decision to the Tool Call Compiler (no-op unless GIZZI_KERNEL_COMPILERS).
          KernelTurn.noteGateDecision(callID, {
            permission: request.permission,
            pattern,
            action: "deny",
            source: floor ? "catastrophic_floor" : (rule.source ?? "default"),
          })
          if (floor) throw new FloorError(floor.reason, pattern)
          throw new DeniedError(ruleset.filter((r) => Wildcard.match(request.permission, r.permission)))
        }
        if (rule.action === "ask") {
          // A turn a scheduler started has nobody to answer: refuse now
          // instead of leaving the run waiting until its timeout, tell the
          // owner once per run, and let the processor end the turn (an
          // UnattendedError is a RejectedError). Never an approval.
          const unattended = await unattendedTurn(request.sessionID, request.tool?.messageID)
          if (unattended) {
            KernelTurn.noteGateDecision(callID, { permission: request.permission, pattern, action: "deny", source: "unattended" })
            raiseNeedsYou(unattended, request)
            throw new UnattendedError()
          }
          const id = askID!
          await HookDispatcher.emit({
            name: "PermissionRequest",
            timestamp: Date.now(),
            sessionId: request.sessionID,
            payload: { tool: request.permission, patterns: request.patterns, requestID: id },
          })
          const decided = (action: "allow" | "deny") =>
            KernelTurn.noteGateDecision(callID, { permission: request.permission, pattern, action, source: "user_reply" })
          KernelTurn.noteGateDecision(callID, { permission: request.permission, pattern, action: "ask", source: rule.source ?? "default" })
          return new Promise<ReplyData | undefined>((resolve, reject) => {
            const info: Request = {
              id,
              ...request,
            }
            s.pending[id] = {
              info,
              ruleset,
              resolve: (v: any) => {
                decided("allow")
                resolve(v)
              },
              reject: (e: any) => {
                decided("deny")
                reject(e)
              },
            }
            Bus.publish(Event.Asked, info)
          })
        }
        if (rule.action === "allow") {
          KernelTurn.noteGateDecision(callID, { permission: request.permission, pattern, action: "allow", source: rule.source ?? "default" })
          continue
        }
      }
    },
  )

  /* ---------------------------------------------------------------------- */
  /* Unattended turns (scheduled tasks, bot routines)                        */
  /* ---------------------------------------------------------------------- */

  /**
   * A turn a scheduler started: the user message carries
   * `metadata.automation_run` (gizzi's /cron fallback, allternit-api's
   * scheduler) or begins with a routine's "[routine: <label>]" marker.
   * `key` is that user message; `sessionID` the session it is in.
   */
  export type UnattendedTurn = { key: string; sessionID: string; jobId?: string; runId?: string; by?: string; label?: string }

  /** After this many refusals in one run its session is stopped (only reached when continue_loop_on_deny is on). */
  export const MAX_UNATTENDED_DENIALS = 5
  const ROUTINE_LABEL = /^\[routine: ([^\]]*)\]/
  const unattendedDenials = new Map<string, number>()
  const unattendedBlocks = new Map<string, string>()

  function remember<V>(map: Map<string, V>, key: string, value: V) {
    map.delete(key)
    map.set(key, value)
    if (map.size > 500) map.delete(map.keys().next().value!)
  }

  type MessageRow = typeof MessageTable.$inferSelect

  /** The user message a tool call answers: the assistant message's parent, else the session's latest user message. */
  function turnUserRow(sessionID: string, messageID?: string): MessageRow | undefined {
    const byId = (id: string) => Database.use((db) => db.select().from(MessageTable).where(eq(MessageTable.id, id)).get())
    if (messageID) {
      const row = byId(messageID)
      const data = row?.data as { role?: string; parentID?: string } | undefined
      if (row && data?.role === "user") return row
      if (data?.role === "assistant" && data.parentID) {
        const parent = byId(data.parentID)
        if (parent && (parent.data as { role?: string }).role === "user") return parent
      }
    }
    const rows = Database.use((db) =>
      db
        .select()
        .from(MessageTable)
        .where(eq(MessageTable.session_id, sessionID))
        .orderBy(desc(MessageTable.time_created), desc(MessageTable.id))
        .limit(20)
        .all(),
    )
    return rows.find((row) => (row.data as { role?: string }).role === "user")
  }

  async function markedTurn(row: MessageRow): Promise<UnattendedTurn | undefined> {
    const metadata = (row.data as { metadata?: Record<string, unknown> }).metadata
    const parts = Database.use((db) =>
      db.select().from(PartTable).where(eq(PartTable.message_id, row.id)).orderBy(PartTable.id).all(),
    )
    const text = parts
      .map((part) => part.data as { type?: string; text?: string; synthetic?: boolean })
      .find((part) => part.type === "text" && !part.synthetic)?.text
    const { Automation } = await import("@/runtime/primitives/automation")
    if (!Automation.isAutomationRun(metadata, text)) return undefined
    const run = metadata?.[Automation.RUN_METADATA_KEY] as { id?: string; runId?: string; by?: string } | undefined
    return {
      key: row.id,
      sessionID: row.session_id,
      ...(typeof run?.id === "string" ? { jobId: run.id } : {}),
      ...(typeof run?.runId === "string" ? { runId: run.runId } : {}),
      ...(typeof run?.by === "string" ? { by: run.by } : {}),
      ...(text && ROUTINE_LABEL.test(text) ? { label: ROUTINE_LABEL.exec(text)![1] } : {}),
    }
  }

  /**
   * Whether the turn that made this tool call was started by a scheduler. A
   * subagent's session has no marker of its own, so its parent's turn
   * decides. Never throws: on a read error the ask goes ahead as usual.
   */
  export async function unattendedTurn(sessionID: string, messageID?: string): Promise<UnattendedTurn | undefined> {
    try {
      let session: string | undefined = sessionID
      let message = messageID
      for (let depth = 0; session && depth < 4; depth++) {
        const row = turnUserRow(session, message)
        if (row) {
          const hit = await markedTurn(row)
          if (hit) return hit
        }
        const current: string = session
        const parent = Database.use((db) =>
          db.select({ parent: SessionTable.parent_id }).from(SessionTable).where(eq(SessionTable.id, current)).get(),
        )
        session = parent?.parent ?? undefined
        message = undefined
      }
    } catch (error) {
      log.warn("unattended check failed; asking as usual", { sessionID, error: String(error) })
    }
    return undefined
  }

  function describeAsk(request: { permission: string; patterns?: string[] }): string {
    const patterns = (request.patterns ?? []).filter((p) => p && p !== "*")
    const text = `use ${request.permission}${patterns.length ? ` (${patterns.join(", ")})` : ""}`
    return text.length > 140 ? text.slice(0, 139).trimEnd() + "…" : text
  }

  /** needs_you once per run; the run's scheduler reads the block back (takeUnattendedBlock). */
  function raiseNeedsYou(turn: UnattendedTurn, request: { sessionID: string; permission: string; patterns?: string[] }) {
    const count = (unattendedDenials.get(turn.key) ?? 0) + 1
    remember(unattendedDenials, turn.key, count)
    const action = describeAsk(request)
    if (turn.runId && !unattendedBlocks.has(turn.runId)) remember(unattendedBlocks, turn.runId, action)
    // allternit-api's scheduler raises its own attention item for this run; one notice, not two.
    if (count === 1 && turn.by !== "scheduler") {
      const who = turn.label ? `Scheduled task "${turn.label}"` : "A scheduled task"
      const preview = `${who} stopped: it needs your approval to ${action}.`
      void import("@/runtime/bots/app-bot-routing")
        .then(({ notifyUser }) =>
          notifyUser(turn.sessionID, { kind: "message", level: "needs_you", preview: preview.slice(0, 200) }),
        )
        .catch((error) => log.warn("needs_you push not sent", { sessionID: turn.sessionID, error: String(error) }))
    }
    if (count >= MAX_UNATTENDED_DENIALS) {
      void import("@/runtime/session/prompt")
        .then(({ SessionPrompt }) => SessionPrompt.cancel(request.sessionID))
        .catch(() => {})
    }
  }

  /** The approval a scheduled run stopped at (its run id from `metadata.automation_run.runId`), read once. */
  export function takeUnattendedBlock(runId: string): string | undefined {
    const action = unattendedBlocks.get(runId)
    unattendedBlocks.delete(runId)
    return action
  }

  function approvalsFor(s: { approved: Ruleset; task: Record<string, Ruleset> }, sessionID: string): Ruleset {
    const task = s.task[sessionID]
    return task?.length ? [...s.approved, ...task] : s.approved
  }

  /** The session's one-task allow rules (what "Allow for this task" added). */
  export async function taskRules(sessionID: string): Promise<Ruleset> {
    const s = await state()
    return s.task[sessionID] ?? []
  }

  /**
   * Drop the session's one-task allows. Called when the session ends and when
   * allternit-api closes the thread (resolve / archive / pause revoke its grants).
   */
  export async function clearTask(sessionID: string): Promise<boolean> {
    const s = await state()
    const had = !!s.task[sessionID]?.length
    delete s.task[sessionID]
    return had
  }

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

  /**
   * An ask raised in another process (a live terminal session, see
   * TerminalLive). It is published like any other ask, so every surface shows
   * its card and answers it through `reply`; the promise settles with the
   * person's decision. Policy was already applied where the tool runs.
   */
  export async function relay(info: Request): Promise<{ allow: boolean; message?: string }> {
    const s = await state()
    return new Promise((resolve) => {
      s.pending[info.id] = {
        info,
        ruleset: [],
        resolve: () => resolve({ allow: true }),
        reject: (e: any) => resolve({ allow: false, message: e instanceof CorrectedError ? e.message : undefined }),
      }
      Bus.publish(Event.Asked, info)
    })
  }

  /** A relayed ask answered where it was raised: close its card everywhere without deciding it again. */
  export async function settle(requestID: string, reply: Reply) {
    const s = await state()
    const existing = s.pending[requestID]
    if (!existing) return
    delete s.pending[requestID]
    Bus.publish(Event.Replied, { sessionID: existing.info.sessionID, requestID, reply })
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
      // The person's answer is the outcome label for this ask's shadow S1 GATE.
      PermissionS1.outcome(input.requestID, input.reply !== "reject")
      // An always-ask class is never remembered: "always" counts as "once".
      if (input.reply === "always" && ALWAYS_ASK.has(existing.info.permission)) {
        input = { ...input, reply: "once" }
      }
      // A one-task grant never covers always-ask classes or payment/sign/delete.
      if (input.reply === "task" && !taskGrantable(existing.info.permission)) {
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
      if (input.reply === "task") {
        const sessionID = existing.info.sessionID
        const rules = (s.task[sessionID] ??= [])
        const patterns = existing.info.always.length ? existing.info.always : existing.info.patterns
        for (const pattern of patterns) {
          rules.push({ permission: existing.info.permission, pattern, action: "allow", source: "session" })
        }
        existing.resolve()
        const mode = await getMode(sessionID)
        const app = await isAppSession(sessionID)
        for (const [id, pending] of Object.entries(s.pending)) {
          if (pending.info.sessionID !== sessionID) continue
          const ok = pending.info.patterns.every(
            (pattern) => evaluatePolicy(pending.info.permission, pattern, {
              configured: pending.ruleset,
              approvals: approvalsFor(s, sessionID),
              mode,
              app,
              destructive: pending.info.metadata?.destructive === true,
            }).action === "allow",
          )
          if (!ok) continue
          delete s.pending[id]
          Bus.publish(Event.Replied, { sessionID, requestID: pending.info.id, reply: "task" })
          pending.resolve()
        }
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
        const app = await isAppSession(sessionID)
        for (const [id, pending] of Object.entries(s.pending)) {
          if (pending.info.sessionID !== sessionID) continue
          const ok = pending.info.patterns.every(
            (pattern) => evaluatePolicy(pending.info.permission, pattern, {
              configured: pending.ruleset,
              approvals: s.approved,
              mode,
              app,
              destructive: pending.info.metadata?.destructive === true,
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
    "read", "glob", "grep", "list", "websearch", "codesearch", "question", "todoread", "lsp", "current_time",
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
    /** An app (non-CLI) session: auto mode keeps asking for risky work. See `isAppSession`. */
    app?: boolean
    /** The tool says it is destructive (MCP `destructiveHint`). */
    destructive?: boolean
  }

  /** Name words that mark a delete, beyond TASK_NEVER's own. */
  const DELETE_WORDS = new Set(["destroy", "erase", "purge", "trash", "unlink"])

  /**
   * Whether a permission class names a payment, a signature or a delete: a
   * TASK_NEVER word (or a delete synonym) as a whole word of the name, so
   * `gmail_delete_message`, `stripe.pay`, `docusign_sign` and `deleteFile`
   * match while `design` or `display` don't.
   */
  export function namesNeverAuto(permission: string): boolean {
    const words = permission
      .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
      .toLowerCase()
      .split(/[^a-z0-9]+/)
      .filter(Boolean)
    return words.some((w) => TASK_NEVER.has(w) || DELETE_WORDS.has(w))
  }

  /**
   * What auto mode still asks a person about in an app session: payments,
   * signing and deletes (TASK_NEVER), a tool marked destructive, and shell
   * commands that delete files.
   */
  function autoAsks(permission: string, pattern: string, destructive: boolean): boolean {
    if (destructive) return true
    if (namesNeverAuto(permission)) return true
    if (Catastrophic.applies(permission) && Catastrophic.deletes(pattern)) return true
    return false
  }

  /** Rules the person set themselves (config, session, project), not built-in defaults. */
  function isUserRule(rule: Rule | undefined): boolean {
    return rule?.source === "user" || rule?.source === "session" || rule?.source === "project"
  }

  /**
   * The hard floor (catastrophic.ts): a shell command no mode can approve.
   * Checked before everything else except ALWAYS_ASK classes (which never
   * allow anyway), so bypassPermissions / GIZZI_SKIP_PERMISSIONS, yolo, auto,
   * session approvals and configured allows all stop here.
   */
  function floorRule(permission: string, pattern: string): Rule | undefined {
    if (!Catastrophic.applies(permission)) return
    if (!Catastrophic.check(pattern)) return
    return { action: "deny", permission, pattern, source: "default" }
  }

  /** Ordered runtime policy composition.
   *
   * Rules inside the configured ruleset retain Allternit's documented
   * last-match behavior. Cross-policy precedence is explicit: the catastrophic
   * floor, configured denial (bypass does not override it), hard host/plan
   * modes, unattended-mode restrictions, session approval, configured
   * ask/allow, mode conveniences, then fallback.
   */
  export function evaluatePolicy(permission: string, pattern: string, input: PolicyInput): Rule {
    const mode = input.mode ?? Flag.GIZZI_PERMISSION_MODE
    const skipPermissions = input.skipPermissions ?? Flag.GIZZI_SKIP_PERMISSIONS

    if (ALWAYS_ASK.has(permission)) return alwaysAsk(permission, pattern, input.configured, mode)

    const floor = floorRule(permission, pattern)
    if (floor) return floor

    // A configured deny is the user's explicit "never": bypass skips asks,
    // it does not override denials.
    const configured = lastMatch(permission, pattern, input.configured)
    if (configured?.action === "deny") return configured

    if (skipPermissions || mode === "bypassPermissions") {
      return { action: "allow", permission, pattern: "*" }
    }

    if (mode === "plan" && !READONLY_PERMISSIONS.has(permission)) {
      return { action: "deny", permission, pattern: "*" }
    }

    // Auto in an app session (Desktop/web/PWA chat): work runs without
    // prompts, but payments, signing, deletes and the person's own ask rules
    // still ask, a session approval ("always") still counts, and question
    // cards keep working because someone is there to answer.
    if (mode === "auto" && input.app) {
      const approved = lastMatch(permission, pattern, input.approvals ?? [])
      if (approved?.action === "allow") return approved
      if (configured?.action === "ask" && isUserRule(configured)) return configured
      if (autoAsks(permission, pattern, input.destructive ?? false)) {
        return { action: "ask", permission, pattern: "*" }
      }
      return { action: "allow", permission, pattern: "*" }
    }

    // Auto in the CLI: unattended, so questions are refused and the rest runs.
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

    const floor = floorRule(permission, pattern)
    if (floor) return floor

    // bypassPermissions skips asks, but never the floor or a configured deny.
    if (Flag.GIZZI_SKIP_PERMISSIONS || mode === "bypassPermissions") {
      const denied = lastMatch(permission, pattern, merge(...rulesets))
      if (denied?.action === "deny") return denied
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

  const EDIT_TOOLS = ["edit", "write", "patch", "multiedit", "symbol_edit"]

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

  /**
   * Tools whose last matching rule is deny `*`. Unlike `disabled`, this has no
   * mode logic (bypass, dontAsk, plan, GIZZI_SKIP_PERMISSIONS): `evaluate` never
   * skips a configured deny, so a tool hidden here could never run anyway.
   * Used to keep denied tool schemas out of the model request.
   */
  export function hidden(tools: string[], ruleset: Ruleset): Set<string> {
    const result = new Set<string>()
    for (const tool of tools) {
      const permission = EDIT_TOOLS.includes(tool) ? "edit" : tool
      const rule = ruleset.findLast((r) => Wildcard.match(permission, r.permission))
      if (rule && rule.pattern === "*" && rule.action === "deny") result.add(tool)
    }
    return result
  }

  /** User rejected without message - halts execution */
  export class RejectedError extends Error {
    constructor() {
      super(`The user rejected permission to use this specific tool call.`)
    }
  }

  /**
   * Refused because nobody is there to ask: the turn was started by a
   * scheduled task or routine. A RejectedError, so the processor ends the
   * turn after this step instead of letting the model keep trying.
   */
  export class UnattendedError extends RejectedError {
    constructor() {
      super()
      this.message =
        "This step needs the owner's approval. Unattended scheduled runs can't ask; the owner has been notified."
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

  /** Refused by the catastrophic floor: no mode or rule can approve it. */
  export class FloorError extends DeniedError {
    constructor(
      public readonly reason: string,
      public readonly command: string,
    ) {
      super([{ permission: "bash", pattern: command, action: "deny", source: "default" }])
      this.message =
        `This command is blocked by the gizzi safety floor (${reason}) and cannot be approved in any permission ` +
        `mode, including bypassPermissions. Do not retry it or work around it; if the user really wants this, ` +
        `they must run it themselves.`
    }
  }

  export async function list() {
    const s = await state()
    return Object.values(s.pending).map((x) => x.info)
  }
}
