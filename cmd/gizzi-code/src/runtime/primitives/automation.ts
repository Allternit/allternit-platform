/**
 * Scheduled tasks the agent makes for its user: reminders, follow-ups and
 * recurring checks ("heartbeats").
 *
 * Shared core for the `automation` tool (runtime builtin and Ink adapter);
 * both share tools/builtins/automation.txt. This file checks and builds every
 * request; allternit-api stores and runs them as local routines
 * (POST /api/v1/bot-routing/automation/{create,list,update,delete,run}).
 * With no platform API or credential the call returns `no_platform` with the
 * built request, and the caller falls back to this computer's scheduler (the
 * runtime CronService behind /cron) through `cronFallback` below.
 *
 * Schedules: exactly one of
 *   at             local wall time "YYYY-MM-DDTHH:mm" read in the user's zone
 *                  (clock.ts); never an offset the model worked out itself.
 *                  A time the clocks skip is refused.
 *   delay_minutes  from now
 *   rrule          RFC 5545 subset, turned into cron (rrule.ts)
 *   cron           5 fields, read in the user's zone
 * One-shots fall 1 minute to 365 days ahead; recurring runs are at least
 * 15 minutes apart.
 *
 * Free of runtime state and UI imports so the Ink adapter and bun tests can
 * use it; tests stub only `fetch`.
 */
import z from "zod/v4"
import {
  AppRoutingError,
  createAutomation,
  deleteAutomation,
  listAutomations,
  runAutomation,
  updateAutomation,
  type AutomationCreateRequest,
  type AutomationInfo,
  type AutomationKind,
  type AutomationRunResult,
  type AutomationStatus,
  type AutomationTarget,
  type AutomationSchedule,
  type AutomationScheduleInput,
  type AutomationUpdateRequest,
} from "@/runtime/bots/app-bot-routing"
import { Clock } from "@/runtime/primitives/clock"
import { RRule } from "@/runtime/primitives/rrule"
import { ROUTINE_MARKER_PREFIX } from "@/runtime/bots/bot-routines"
import type { AgentJob, CreateJobInput, CronJob, CronRun, UpdateJobInput } from "@/runtime/automation/cron/types"

export namespace Automation {
  export const MAX_TITLE_CHARS = 80
  export const MAX_PROMPT_CHARS = 4000
  export const MIN_ONE_SHOT_MINUTES = 1
  export const MAX_ONE_SHOT_DAYS = 365
  export const MIN_INTERVAL_MINUTES = RRule.MIN_INTERVAL_MINUTES
  /** Active agent-made tasks per user (enforced by allternit-api). */
  export const MAX_ACTIVE = 25

  /** The one sentence every create and update result carries about approvals. */
  export const APPROVAL_SENTENCE = "Runs unattended: a step that needs your approval stops the run, and you get a notice to handle it yourself."

  /**
   * User-message metadata key a scheduler sets on every turn it starts
   * (`{automation_run: {id, runId?}}`). A turn carrying it (or a bot routine's
   * "[routine: …]" marker) can't create automations.
   */
  export const RUN_METADATA_KEY = "automation_run"

  export type Action = "create" | "list" | "update" | "delete" | "run_now"

  export class AutomationError extends Error {
    constructor(
      readonly code: "invalid" | "automation_run" | "nonexistent_time" | "out_of_window" | RRule.ErrorCode,
      message: string,
    ) {
      super(message)
      this.name = "AutomationError"
    }
  }

  export const schedule = z.object({
    at: z
      .string()
      .optional()
      .describe('One time, as local wall time "YYYY-MM-DDTHH:mm" in the user\'s zone (no offset, no Z). Prefer this for reminders.'),
    delay_minutes: z.number().int().optional().describe("One time, this many minutes from now (1 to 525600)"),
    rrule: z
      .string()
      .optional()
      .describe('Recurring, as an RRULE such as "FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=9;BYMINUTE=0"; COUNT or UNTIL end it'),
    cron: z.string().optional().describe('Recurring, as 5 cron fields read in the user\'s zone, such as "0 9 * * 1-5"'),
    timezone: z.string().max(64).optional().describe("Same as the top-level timezone"),
  })

  export const parameters = z.object({
    action: z.enum(["create", "list", "update", "delete", "run_now"]).describe("list before create, so you don't make a duplicate"),
    id: z.string().optional().describe("The scheduled task's id (update, delete, run_now)"),
    kind: z
      .enum(["message", "heartbeat"])
      .optional()
      .describe("message (default): the prompt runs at the scheduled time. heartbeat: a recurring check that tells the user only when something matters"),
    title: z.string().max(MAX_TITLE_CHARS).optional().describe("Short name the user sees in their scheduled tasks"),
    prompt: z
      .string()
      .max(MAX_PROMPT_CHARS)
      .optional()
      .describe("What to do when it runs. A heartbeat runs in a fresh session, so its prompt must stand alone"),
    target: z
      .enum(["this_thread", "new_session"])
      .optional()
      .describe("this_thread (default): results post into this conversation. new_session: each run gets its own session"),
    schedule: schedule.optional().describe("Exactly one of at, delay_minutes, rrule or cron (create; update to reschedule)"),
    timezone: z
      .string()
      .max(64)
      .optional()
      .describe('IANA zone such as "America/Chicago" for at, rrule and cron; leave it out for the user\'s own zone'),
    initiation: z
      .enum(["user_request", "own_followup"])
      .optional()
      .describe("user_request (default): the user asked for it. own_followup: your own follow-up, only after the user agreed to it"),
    status: z.enum(["active", "paused"]).optional().describe("update: pause or resume"),
  })

  export type Params = z.infer<typeof parameters>
  export type ScheduleParams = z.infer<typeof schedule>

  export type Resolved = {
    schedule: AutomationSchedule
    scheduleInput: AutomationScheduleInput
    recurring: boolean
    maxRuns?: number
    until?: string
  }

  /* ---------------------------------------------------------------------- */
  /* Zone and schedule                                                       */
  /* ---------------------------------------------------------------------- */

  export type Zone = Clock.Resolved

  /**
   * The zone for a schedule. A zone the model named must be valid: unlike
   * current_time, a wrong zone here moves the run, so it is refused rather
   * than replaced.
   */
  export async function zoneFor(input: { timezone?: string; turnZone?: string }): Promise<Zone> {
    if (input.timezone !== undefined && input.timezone.trim() !== "") {
      const zone = Clock.validZone(input.timezone)
      if (!zone) {
        throw new AutomationError("invalid", `"${input.timezone}" is not an IANA time zone (for example "America/Chicago"). Leave timezone out to use the user's zone.`)
      }
      return { zone, source: "argument" }
    }
    return Clock.resolveTimeZone({ turnZone: input.turnZone })
  }

  /** "Wed Nov 18 2026, 09:00 America/Chicago" */
  export function formatInZone(at: Date | string, zone: string): string {
    const date = typeof at === "string" ? new Date(at) : at
    const now = Clock.now(zone, date)
    return `${Clock.dateString(zone, date)}, ${now.iso.slice(11, 16)} ${zone}`
  }

  function rruleError(error: unknown): never {
    if (error instanceof RRule.RRuleError) throw new AutomationError(error.code, error.message)
    throw error
  }

  /**
   * Check a schedule and turn it into what the scheduler stores. `zone` is a
   * valid IANA zone; `now` is injectable for tests.
   */
  export function resolveSchedule(input: ScheduleParams, zone: string, now: Date = new Date()): Resolved {
    const given = (["at", "delay_minutes", "rrule", "cron"] as const).filter((key) => input[key] !== undefined && input[key] !== "")
    if (given.length !== 1) {
      throw new AutomationError(
        "invalid",
        given.length
          ? `Give exactly one of at, delay_minutes, rrule or cron in schedule; got ${given.join(" and ")}.`
          : "schedule needs one of at (local time), delay_minutes, rrule or cron.",
      )
    }
    const minAhead = MIN_ONE_SHOT_MINUTES * 60_000
    const maxAhead = MAX_ONE_SHOT_DAYS * 24 * 60 * 60_000
    const oneShot = (at: Date, scheduleInput: AutomationScheduleInput): Resolved => {
      const ahead = at.getTime() - now.getTime()
      if (ahead < minAhead) {
        throw new AutomationError(
          "out_of_window",
          `That time is ${ahead <= 0 ? "in the past" : "less than a minute away"}. It is now ${formatInZone(now, zone)}; pick a time at least ${MIN_ONE_SHOT_MINUTES} minute ahead.`,
        )
      }
      if (ahead > maxAhead) {
        throw new AutomationError("out_of_window", `One-time scheduled tasks can be at most ${MAX_ONE_SHOT_DAYS} days ahead.`)
      }
      return { schedule: { type: "once", at: at.toISOString() }, scheduleInput, recurring: false }
    }

    if (input.delay_minutes !== undefined) {
      const minutes = input.delay_minutes
      if (!Number.isInteger(minutes) || minutes < MIN_ONE_SHOT_MINUTES || minutes > MAX_ONE_SHOT_DAYS * 24 * 60) {
        throw new AutomationError(
          "out_of_window",
          `delay_minutes must be a whole number from ${MIN_ONE_SHOT_MINUTES} to ${MAX_ONE_SHOT_DAYS * 24 * 60}.`,
        )
      }
      return oneShot(new Date(now.getTime() + minutes * 60_000), { delayMinutes: minutes })
    }
    if (input.at !== undefined) {
      const at = input.at.trim()
      const instant = RRule.localToInstant(at, zone)
      if (!instant.ok) {
        throw new AutomationError(instant.code === "nonexistent" ? "nonexistent_time" : "invalid", instant.error)
      }
      return oneShot(instant.at, { at })
    }
    if (input.rrule !== undefined) {
      let converted: RRule.Converted
      try {
        converted = RRule.rruleToCron(input.rrule, zone)
      } catch (error) {
        rruleError(error)
      }
      if (converted.until && new Date(converted.until).getTime() <= now.getTime()) {
        throw new AutomationError("out_of_window", `UNTIL is already past (it is now ${formatInZone(now, zone)}).`)
      }
      const out: Resolved = {
        schedule: { type: "cron", expression: converted.cron },
        scheduleInput: { rrule: input.rrule.trim() },
        recurring: true,
      }
      if (converted.maxRuns !== undefined) out.maxRuns = converted.maxRuns
      if (converted.until !== undefined) out.until = converted.until
      return out
    }
    let expression: string
    try {
      expression = RRule.validateCron(input.cron!)
    } catch (error) {
      rruleError(error)
    }
    return { schedule: { type: "cron", expression }, scheduleInput: { cron: expression }, recurring: true }
  }

  /* ---------------------------------------------------------------------- */
  /* Requests                                                                */
  /* ---------------------------------------------------------------------- */

  function clipTitle(title: string | undefined, prompt: string): string {
    const raw = (title?.trim() || prompt.trim().split("\n", 1)[0]!).replace(/\s+/g, " ").trim()
    return raw.length > MAX_TITLE_CHARS ? raw.slice(0, MAX_TITLE_CHARS - 1).trimEnd() + "…" : raw
  }

  function checkPrompt(prompt: string | undefined, required: boolean): string | undefined {
    if (prompt === undefined) {
      if (required) throw new AutomationError("invalid", "prompt is required: say what to do when it runs.")
      return undefined
    }
    const text = prompt.trim()
    if (!text) throw new AutomationError("invalid", "prompt is empty.")
    if (text.length > MAX_PROMPT_CHARS) throw new AutomationError("invalid", `prompt is ${text.length} characters; the limit is ${MAX_PROMPT_CHARS}.`)
    return text
  }

  function checkTitle(title: string | undefined): void {
    if (title !== undefined && title.trim().length > MAX_TITLE_CHARS) {
      throw new AutomationError("invalid", `title is longer than ${MAX_TITLE_CHARS} characters.`)
    }
  }

  function requireId(params: Params): string {
    const id = params.id?.trim()
    if (!id) throw new AutomationError("invalid", `${params.action} needs the scheduled task's id (use action list to find it).`)
    return id
  }

  /** The schedule's own timezone field, or the top-level one; both may not disagree. */
  function timezoneParam(params: Params): string | undefined {
    const inner = params.schedule?.timezone?.trim() || undefined
    const outer = params.timezone?.trim() || undefined
    if (inner && outer && inner !== outer) {
      throw new AutomationError("invalid", `timezone "${outer}" and schedule.timezone "${inner}" disagree; give one.`)
    }
    return inner ?? outer
  }

  export type Built =
    | { action: "create"; request: AutomationCreateRequest; zone: Zone; recurring: boolean }
    | { action: "update"; request: AutomationUpdateRequest; zone?: Zone; recurring?: boolean }
    | { action: "delete"; request: { id: string } }
    | { action: "run_now"; request: { id: string } }
    | { action: "list" }

  /**
   * Check `params` and build the request for its action. Nothing is sent.
   * `fromAutomation`: this turn was started by a scheduled task, which may not
   * create another one (refused, not asked).
   */
  export async function build(
    params: Params,
    ctx: { turnZone?: string; fromAutomation?: boolean; now?: Date } = {},
  ): Promise<Built> {
    const now = ctx.now ?? new Date()
    switch (params.action) {
      case "list":
        return { action: "list" }
      case "delete":
        return { action: "delete", request: { id: requireId(params) } }
      case "run_now":
        return { action: "run_now", request: { id: requireId(params) } }
      case "create": {
        if (ctx.fromAutomation) {
          throw new AutomationError(
            "automation_run",
            "This run was started by a scheduled task, and a scheduled task can't create scheduled tasks. Tell the user in your reply instead.",
          )
        }
        if (params.id) throw new AutomationError("invalid", "create makes a new scheduled task; leave id out (use update to change one).")
        if (params.status) throw new AutomationError("invalid", "status is for update; a new scheduled task starts active.")
        const prompt = checkPrompt(params.prompt, true)!
        checkTitle(params.title)
        if (!params.schedule) throw new AutomationError("invalid", "create needs a schedule: one of at, delay_minutes, rrule or cron.")
        const zone = await zoneFor({ timezone: timezoneParam(params), turnZone: ctx.turnZone })
        const resolved = resolveSchedule(params.schedule, zone.zone, now)
        const request: AutomationCreateRequest = {
          kind: params.kind ?? "message",
          title: clipTitle(params.title, prompt),
          prompt,
          target: params.target ?? "this_thread",
          initiation: params.initiation ?? "user_request",
          schedule: resolved.schedule,
          timezone: zone.zone,
          scheduleInput: resolved.scheduleInput,
        }
        if (resolved.maxRuns !== undefined) request.maxRuns = resolved.maxRuns
        if (resolved.until !== undefined) request.until = resolved.until
        return { action: "create", request, zone, recurring: resolved.recurring }
      }
      case "update": {
        const id = requireId(params)
        if (params.initiation) throw new AutomationError("invalid", "initiation is set when a scheduled task is created; it can't be changed.")
        checkTitle(params.title)
        const prompt = checkPrompt(params.prompt, false)
        const request: AutomationUpdateRequest = { id }
        if (params.title !== undefined) request.title = clipTitle(params.title, prompt ?? params.title)
        if (prompt !== undefined) request.prompt = prompt
        if (params.kind) request.kind = params.kind
        if (params.target) request.target = params.target
        if (params.status) request.status = params.status
        const tz = timezoneParam(params)
        let zone: Zone | undefined
        let recurring: boolean | undefined
        if (params.schedule) {
          zone = await zoneFor({ timezone: tz, turnZone: ctx.turnZone })
          const resolved = resolveSchedule(params.schedule, zone.zone, now)
          request.schedule = resolved.schedule
          request.timezone = zone.zone
          request.scheduleInput = resolved.scheduleInput
          // A new schedule replaces the old end: COUNT/UNTIL, or none.
          request.maxRuns = resolved.maxRuns ?? null
          request.until = resolved.until ?? null
          recurring = resolved.recurring
        } else if (tz) {
          throw new AutomationError("invalid", "To change the time zone, send the schedule again together with the new timezone.")
        }
        if (Object.keys(request).length === 1) {
          throw new AutomationError("invalid", "update needs something to change: title, prompt, kind, target, schedule or status.")
        }
        return { action: "update", request, zone, recurring }
      }
    }
  }

  /* ---------------------------------------------------------------------- */
  /* Calls                                                                   */
  /* ---------------------------------------------------------------------- */

  /** The HTTP edge (allternit-api). Defaults to the app-bot-routing client. */
  export interface Api {
    create(sessionID: string, input: AutomationCreateRequest): Promise<{ automation: AutomationInfo }>
    list(sessionID: string): Promise<{ automations: AutomationInfo[] }>
    update(sessionID: string, input: AutomationUpdateRequest): Promise<{ automation: AutomationInfo }>
    delete(sessionID: string, input: { id: string }): Promise<{ deleted: true; id: string }>
    run(sessionID: string, input: { id: string }): Promise<AutomationRunResult>
  }

  const defaultApi: Api = {
    create: createAutomation,
    list: listAutomations,
    update: updateAutomation,
    delete: deleteAutomation,
    run: runAutomation,
  }

  /** Why the platform API can't take the call; the caller may fall back to the runtime /cron. */
  export type NoPlatformReason = "no_credential" | "unreachable" | "no_route"

  /** Where an `ok` outcome was stored: allternit-api (default) or this computer's scheduler (`cron`). */
  export type Via = "platform" | "cron"

  export type Outcome =
    | { status: "ok"; action: "create" | "update"; automation: AutomationInfo; zone: Zone; recurring?: boolean; via?: Via }
    | { status: "ok"; action: "list"; automations: AutomationInfo[]; via?: Via }
    | { status: "ok"; action: "delete"; id: string; via?: Via }
    | { status: "ok"; action: "run_now"; run: AutomationRunResult; via?: Via }
    | { status: "no_platform"; reason: NoPlatformReason; built: Built }
    /** No platform, and this computer's scheduler isn't running either. */
    | { status: "no_scheduler"; reason: NoPlatformReason; built: Built }

  function noPlatform(error: unknown): NoPlatformReason | undefined {
    if (error instanceof AppRoutingError) {
      if (error.code === "no_credential") return "no_credential"
      // An allternit-api older than these routes.
      if (error.status === 404 && !error.code) return "no_route"
      return undefined
    }
    const name = (error as { name?: string } | undefined)?.name
    // A timeout may have reached the server: never treat it as "nothing happened".
    if (name === "TimeoutError" || name === "AbortError") return undefined
    return "unreachable"
  }

  function apiError(error: unknown): Error {
    const name = (error as { name?: string } | undefined)?.name
    if (name === "TimeoutError" || name === "AbortError") {
      return new Error("The Allternit API didn't answer in time. Use action list to see whether the change went through before trying again.")
    }
    if (error instanceof AppRoutingError) {
      const prefix = error.code ? `${error.code}: ` : ""
      return new Error(`${prefix}${error.message}`)
    }
    return error instanceof Error ? error : new Error(String(error))
  }

  /** Build, then send. Throws AutomationError for bad input and an Error for a refusal from the server. */
  export async function send(input: { sessionID: string; built: Built; api?: Api }): Promise<Outcome> {
    const api = input.api ?? defaultApi
    const { built, sessionID } = input
    try {
      switch (built.action) {
        case "list": {
          const r = await api.list(sessionID)
          return { status: "ok", action: "list", automations: r.automations ?? [] }
        }
        case "create": {
          const r = await api.create(sessionID, built.request)
          return { status: "ok", action: "create", automation: r.automation, zone: built.zone, recurring: built.recurring }
        }
        case "update": {
          const r = await api.update(sessionID, built.request)
          const zone = built.zone ?? { zone: r.automation.timezone, source: "argument" as const }
          return { status: "ok", action: "update", automation: r.automation, zone, recurring: built.recurring }
        }
        case "delete": {
          const r = await api.delete(sessionID, built.request)
          return { status: "ok", action: "delete", id: r.id ?? built.request.id }
        }
        case "run_now": {
          const r = await api.run(sessionID, built.request)
          return { status: "ok", action: "run_now", run: r }
        }
      }
    } catch (error) {
      const reason = noPlatform(error)
      if (reason) return { status: "no_platform", reason, built }
      throw apiError(error)
    }
  }

  /**
   * True when the turn was started by a scheduled task: the scheduler's
   * metadata key, or a bot routine's text marker.
   */
  export function isAutomationRun(metadata: Record<string, unknown> | undefined, text?: string): boolean {
    if (metadata && metadata[RUN_METADATA_KEY]) return true
    if (metadata?.["origin"] === "automation") return true
    return typeof text === "string" && text.startsWith(ROUTINE_MARKER_PREFIX)
  }

  /* ---------------------------------------------------------------------- */
  /* Fallback: this computer's scheduler (runtime CronService, /cron)        */
  /* ---------------------------------------------------------------------- */

  /** Tag on every job the fallback makes. */
  export const CRON_TAG = "automation"
  /** A run due more than this long ago is logged as missed instead of run (laptop asleep, scheduler off). */
  export const LATE_LIMIT_MS = 12 * 60 * 60_000
  /** How the user starts this computer's scheduler. */
  export const START_COMMAND = "gizzi jobs start"

  /**
   * The CronService calls the fallback uses (the same service the /cron
   * routes use). The runtime builtin passes the in-process service; tests may
   * pass it too, with a temporary database.
   */
  export interface CronBackend {
    running(): boolean
    create(input: CreateJobInput): CronJob
    list(): CronJob[]
    get(id: string): CronJob | null
    update(id: string, input: UpdateJobInput): CronJob
    delete(id: string): boolean
    /** Start a run now. The tool never waits on it: a this_thread run queues behind the turn that asked. */
    run(id: string): Promise<CronRun>
  }

  export function isAutomationJob(job: CronJob): job is AgentJob {
    return job.type === "agent" && !!(job.config as AgentJob["config"]).kind
  }

  const pad2 = (n: number) => String(n).padStart(2, "0")

  /**
   * The next run of a 5-field cron line read in `zone`, strictly after `from`;
   * null when none falls within 5 years. Day-of-month and day-of-week both
   * restricted means either matches (cron's rule). Times the clocks skip are
   * skipped; a repeated time runs at its first occurrence.
   */
  export function nextCronRun(expression: string, zone: string, from: Date = new Date()): Date | null {
    const parsed = RRule.parseCron(expression)
    const fields = expression.trim().split(/\s+/)
    const domAny = fields[2] === "*"
    const dowAny = fields[4] === "*"
    const local = Clock.now(zone, from).iso
    const start = Date.UTC(Number(local.slice(0, 4)), Number(local.slice(5, 7)) - 1, Number(local.slice(8, 10)))
    const months = new Set(parsed.months)
    const doms = new Set(parsed.daysOfMonth)
    const dows = new Set(parsed.daysOfWeek)
    for (let i = 0; i <= 366 * 5; i++) {
      const day = new Date(start + i * 86_400_000)
      if (!months.has(day.getUTCMonth() + 1)) continue
      const domHit = doms.has(day.getUTCDate())
      const dowHit = dows.has(day.getUTCDay())
      const hit = domAny && dowAny ? true : domAny ? dowHit : dowAny ? domHit : domHit || dowHit
      if (!hit) continue
      const date = `${String(day.getUTCFullYear()).padStart(4, "0")}-${pad2(day.getUTCMonth() + 1)}-${pad2(day.getUTCDate())}`
      for (const h of parsed.hours) {
        for (const m of parsed.minutes) {
          const at = RRule.localToInstant(`${date}T${pad2(h)}:${pad2(m)}`, zone)
          if (at.ok && at.at.getTime() > from.getTime()) return at.at
        }
      }
    }
    return null
  }

  /** When the task runs next after `from`; null when nothing more is due (a one-shot that passed, past UNTIL). */
  export function nextFire(config: AgentJob["config"], from: Date = new Date()): Date | null {
    const schedule = config.automationSchedule
    if (!schedule) return null
    let next: Date | null
    if (schedule.type === "once") {
      const at = new Date(schedule.at)
      next = at.getTime() > from.getTime() ? at : null
    } else {
      next = nextCronRun(schedule.expression, config.timezone || "UTC", from)
    }
    if (next && config.until && next.getTime() > new Date(config.until).getTime()) return null
    return next
  }

  /**
   * The schedule the CronService stores for a fallback job: a rolling
   * one-shot interval from `now` to `at`. The service reads cron lines in the
   * process zone only, so the executor re-arms this after every run from the
   * real schedule (`config.automationSchedule` read in `config.timezone`).
   */
  export function intervalUntil(at: Date, now: Date = new Date()): { type: "interval"; seconds: number } {
    return { type: "interval", seconds: Math.max(1, Math.ceil((at.getTime() - now.getTime()) / 1000)) }
  }

  /** A fallback job as the tool and the apps show scheduled tasks. */
  export function jobInfo(job: AgentJob): AutomationInfo {
    const c = job.config
    const status: AutomationStatus = job.status === "active" ? "active" : job.status === "paused" ? "paused" : "completed"
    return {
      id: job.id,
      title: c.title ?? job.name,
      kind: c.kind ?? "message",
      target: c.target ?? (c.delivery === "session" ? "this_thread" : "new_session"),
      status,
      schedule: c.automationSchedule ?? { type: "once", at: job.nextRunAt ?? job.createdAt },
      timezone: c.timezone ?? "UTC",
      nextRunAt: job.status === "active" ? (job.nextRunAt ?? null) : null,
      lastRunAt: job.lastRunAt ?? null,
      lastRunStatus: c.lastRunStatus ?? null,
      maxRuns: job.maxRuns ?? null,
      runCount: job.runCount,
      until: c.until ?? null,
      sessionId: c.targetSessionId ?? null,
      origin: c.origin ?? "agent",
      initiation: c.initiation ?? null,
      createdAt: job.createdAt,
    }
  }

  /** Where runs deliver: a this_thread message is a turn in the thread; heartbeat notices post to the thread that made it. */
  function routing(kind: AutomationKind, target: AutomationTarget, sessionID: string, current?: string) {
    const targetSessionId = target === "this_thread" || kind === "heartbeat" ? (current ?? sessionID) : undefined
    const delivery: "session" | "ephemeral" = kind === "message" && target === "this_thread" ? "session" : "ephemeral"
    return { targetSessionId, delivery }
  }

  /**
   * Do a call the platform couldn't take on this computer's scheduler. Same
   * rules as allternit-api: the active cap, the duplicate check, only
   * scheduled tasks (never other cron jobs) can be changed. Returns
   * `no_scheduler` when the scheduler isn't running in this process.
   */
  export async function cronFallback(input: {
    sessionID: string
    built: Built
    reason: NoPlatformReason
    backend: CronBackend | undefined
    /** The folder runs work in (the calling session's project). */
    directory?: string
    now?: Date
  }): Promise<Outcome> {
    const { built, backend, sessionID } = input
    if (!backend || !backend.running()) return { status: "no_scheduler", reason: input.reason, built }
    const now = input.now ?? new Date()
    const via = "cron" as const
    const own = (id: string): AgentJob => {
      const job = backend.get(id)
      if (!job || !isAutomationJob(job)) {
        throw new AutomationError("invalid", `There's no scheduled task with id ${id} on this computer (use action list).`)
      }
      return job
    }
    switch (built.action) {
      case "list":
        return { status: "ok", action: "list", automations: backend.list().filter(isAutomationJob).map(jobInfo), via }
      case "delete": {
        own(built.request.id)
        backend.delete(built.request.id)
        return { status: "ok", action: "delete", id: built.request.id, via }
      }
      case "run_now": {
        const job = own(built.request.id)
        // The run record carries any failure; the tool doesn't wait (a this_thread turn queues behind this one).
        void backend.run(job.id).catch(() => {})
        const queued = job.config.delivery === "session" && job.config.targetSessionId === sessionID
        return { status: "ok", action: "run_now", run: { id: job.id, status: queued ? "queued" : "started" }, via }
      }
      case "create": {
        const r = built.request
        const active = backend.list().filter(isAutomationJob).filter((job) => job.status === "active")
        if (active.length >= MAX_ACTIVE) {
          throw new AutomationError(
            "invalid",
            `There are already ${MAX_ACTIVE} active scheduled tasks on this computer. Delete or pause one first (use action list).`,
          )
        }
        const { targetSessionId, delivery } = routing(r.kind, r.target, sessionID)
        const duplicate = active.find(
          (job) =>
            job.config.prompt === r.prompt &&
            job.config.kind === r.kind &&
            job.config.targetSessionId === targetSessionId &&
            JSON.stringify(job.config.automationSchedule) === JSON.stringify(r.schedule),
        )
        if (duplicate) {
          throw new AutomationError(
            "invalid",
            `A scheduled task like this already exists (id ${duplicate.id}, "${jobInfo(duplicate).title}"). Update it instead of making another.`,
          )
        }
        const config: AgentJob["config"] = {
          prompt: r.prompt,
          kind: r.kind,
          target: r.target,
          delivery,
          origin: "agent",
          initiation: r.initiation,
          title: r.title,
          automationSchedule: r.schedule,
          timezone: r.timezone,
          scheduleInput: r.scheduleInput,
          ...(targetSessionId ? { targetSessionId } : {}),
          ...(r.until ? { until: r.until } : {}),
          ...(input.directory ? { directory: input.directory } : {}),
        }
        const first = nextFire(config, now)
        if (!first) throw new AutomationError("out_of_window", "That schedule has no run left.")
        const job = backend.create({
          name: r.title,
          description: `Scheduled task (${r.kind}) made by the agent`,
          type: "agent",
          schedule: intervalUntil(first, now),
          config,
          ...(r.maxRuns !== undefined ? { maxRuns: r.maxRuns } : {}),
          tags: [CRON_TAG, `${CRON_TAG}:${r.kind}`],
          metadata: { origin: "agent", sourceSessionId: sessionID },
          scope: "persistent",
          catchUpMissed: false,
        })
        return { status: "ok", action: "create", automation: jobInfo(job as AgentJob), zone: built.zone, recurring: built.recurring, via }
      }
      case "update": {
        const r = built.request
        const job = own(r.id)
        const config: AgentJob["config"] = { ...job.config }
        const patch: UpdateJobInput = {}
        if (r.title !== undefined) {
          config.title = r.title
          patch.name = r.title
        }
        if (r.prompt !== undefined) config.prompt = r.prompt
        if (r.kind) config.kind = r.kind
        if (r.target) config.target = r.target
        const kind = config.kind ?? "message"
        const target = config.target ?? "this_thread"
        Object.assign(config, routing(kind, target, sessionID, config.targetSessionId))
        if (r.schedule) {
          config.automationSchedule = r.schedule
          config.timezone = r.timezone
          config.scheduleInput = r.scheduleInput
          config.until = r.until ?? undefined
          // null clears the run limit (the service stores what it is given).
          patch.maxRuns = (r.maxRuns ?? null) as unknown as number
        }
        const wantActive = r.status ? r.status === "active" : job.status === "active" || (!!r.schedule && job.status !== "paused")
        if (wantActive) {
          const next = nextFire(config, now)
          if (!next) throw new AutomationError("out_of_window", "Nothing more is due on that schedule; send a new schedule to turn it back on.")
          config.ended = undefined
          patch.schedule = intervalUntil(next, now)
          patch.status = "active"
        } else if (r.status === "paused") {
          patch.status = "paused"
        }
        patch.config = config
        const updated = backend.update(job.id, patch) as AgentJob
        return {
          status: "ok",
          action: "update",
          automation: jobInfo(updated),
          zone: built.zone ?? { zone: config.timezone ?? "UTC", source: "argument" },
          recurring: built.recurring,
          via,
        }
      }
    }
  }

  /* ---------------------------------------------------------------------- */
  /* What the model reads back                                               */
  /* ---------------------------------------------------------------------- */

  function scheduleText(a: AutomationInfo): string {
    if (a.schedule.type === "once") return `once at ${formatInZone(a.schedule.at, a.timezone)}`
    return `cron "${a.schedule.expression}" (${a.timezone})`
  }

  function line(a: AutomationInfo): string {
    const next = a.nextRunAt ? `next ${formatInZone(a.nextRunAt, a.timezone)}` : "nothing due"
    const target = a.target === "this_thread" ? (a.sessionId ? `thread ${a.sessionId}` : "this thread") : "new session each run"
    return `- ${a.id} · "${a.title}" · ${a.kind} · ${a.status} · ${scheduleText(a)} · ${next} · ${target} · made by ${a.origin}`
  }

  const PROCESS_ZONE_LINE =
    "The zone used is this machine's, which may not be the user's. Tell the user which zone the time is in, or ask where they are and update it."

  /** Said once on create/update when the task went to this computer's scheduler. */
  const CRON_NOTE =
    "It is saved on this computer's scheduler, not the user's Allternit account: it runs only while Gizzi is running here, and the user's other devices don't list it."

  export const NO_PLATFORM_TEXT: Record<NoPlatformReason, string> = {
    no_credential: "this runtime isn't connected to an Allternit account",
    unreachable: "the Allternit API on this computer can't be reached",
    no_route: "the Allternit API on this computer is too old for scheduled tasks",
  }

  /** The text the model reads back. Create and update say which approval setting runs get, and the next run in the user's zone. */
  export function summary(outcome: Outcome): string {
    if (outcome.status === "no_platform") {
      return `Not scheduled: ${NO_PLATFORM_TEXT[outcome.reason]}. Tell the user plainly; don't say it is scheduled.`
    }
    if (outcome.status === "no_scheduler") {
      return (
        `Not scheduled: ${NO_PLATFORM_TEXT[outcome.reason]}, and this computer's scheduler isn't running. ` +
        `Tell the user plainly; don't say it is scheduled. They can start the scheduler with \`${START_COMMAND}\` ` +
        `(or keep Gizzi Code open, which runs it), then ask again.`
      )
    }
    switch (outcome.action) {
      case "list":
        return outcome.automations.length
          ? `${outcome.automations.length} scheduled task${outcome.automations.length === 1 ? "" : "s"}:\n${outcome.automations.map(line).join("\n")}`
          : "No scheduled tasks."
      case "delete":
        return `Scheduled task ${outcome.id} deleted.`
      case "run_now":
        return outcome.run.status === "queued"
          ? `Scheduled task ${outcome.run.id} will run as soon as its thread is free.`
          : `Scheduled task ${outcome.run.id} started.`
      case "create":
      case "update": {
        const a = outcome.automation
        const zone = a.timezone || outcome.zone.zone
        const next = a.nextRunAt ? `Next run: ${formatInZone(a.nextRunAt, zone)}.` : "Nothing more is due."
        const verb = outcome.action === "create" ? "created" : "updated"
        const lines = [`Scheduled task "${a.title}" ${verb} (id ${a.id}, ${a.status}). ${next}`]
        if (a.kind === "heartbeat") {
          lines.push("Each run is a fresh session that tells the user only on NOTIFY or DONE.")
        }
        if (outcome.via === "cron") lines.push(CRON_NOTE)
        lines.push(`Tell the user in one sentence which approval setting it got: "${APPROVAL_SENTENCE}"`)
        if (outcome.zone.source === "process") lines.push(PROCESS_ZONE_LINE)
        return lines.join("\n")
      }
    }
  }

  /** Short title for the tool card. */
  export function title(outcome: Outcome): string {
    if (outcome.status === "no_platform" || outcome.status === "no_scheduler") return "Scheduled task not created"
    switch (outcome.action) {
      case "list":
        return `Scheduled tasks (${outcome.automations.length})`
      case "create":
        return `Scheduled: ${outcome.automation.title}`
      case "update":
        return `Updated: ${outcome.automation.title}`
      case "delete":
        return "Scheduled task deleted"
      case "run_now":
        return "Scheduled task run"
    }
  }
}
