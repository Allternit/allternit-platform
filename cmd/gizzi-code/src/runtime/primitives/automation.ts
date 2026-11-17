/**
 * Scheduled tasks the agent makes for its user: reminders, follow-ups and
 * recurring checks ("heartbeats").
 *
 * Shared core for the `automation` tool (runtime builtin and Ink adapter);
 * both share tools/builtins/automation.txt. This file checks and builds every
 * request; allternit-api stores and runs them as local routines
 * (POST /api/v1/bot-routing/automation/{create,list,update,delete,run}).
 * With no platform API or credential the call returns `no_platform` with the
 * built request, so the caller can fall back to the runtime's /cron.
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
  type AutomationRunResult,
  type AutomationSchedule,
  type AutomationScheduleInput,
  type AutomationUpdateRequest,
} from "@/runtime/bots/app-bot-routing"
import { Clock } from "@/runtime/primitives/clock"
import { RRule } from "@/runtime/primitives/rrule"
import { ROUTINE_MARKER_PREFIX } from "@/runtime/bots/bot-routines"

export namespace Automation {
  export const MAX_TITLE_CHARS = 80
  export const MAX_PROMPT_CHARS = 4000
  export const MIN_ONE_SHOT_MINUTES = 1
  export const MAX_ONE_SHOT_DAYS = 365
  export const MIN_INTERVAL_MINUTES = RRule.MIN_INTERVAL_MINUTES
  /** Active agent-made tasks per user (enforced by allternit-api). */
  export const MAX_ACTIVE = 25

  /** The one sentence every create and update result carries about approvals. */
  export const APPROVAL_SENTENCE = "Runs unattended: any step that needs your approval stops the run and asks you."

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

  export type Outcome =
    | { status: "ok"; action: "create" | "update"; automation: AutomationInfo; zone: Zone; recurring?: boolean }
    | { status: "ok"; action: "list"; automations: AutomationInfo[] }
    | { status: "ok"; action: "delete"; id: string }
    | { status: "ok"; action: "run_now"; run: AutomationRunResult }
    | { status: "no_platform"; reason: NoPlatformReason; built: Built }

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
        lines.push(`Tell the user in one sentence which approval setting it got: "${APPROVAL_SENTENCE}"`)
        if (outcome.zone.source === "process") lines.push(PROCESS_ZONE_LINE)
        return lines.join("\n")
      }
    }
  }

  /** Short title for the tool card. */
  export function title(outcome: Outcome): string {
    if (outcome.status === "no_platform") return "Scheduled task not created"
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
