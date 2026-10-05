/**
 * Bot routines (Bot Mode, phase B3 — D3/D4, see docs/programs/gizzi/GIZZI_BOT_MODE_SPEC.md).
 *
 * A routine is a cron job of type `agent` whose config carries `bot: <name>`.
 * The cron agent executor delivers the prompt into the bot's canonical chat
 * (resume + one turn) instead of minting an ephemeral session, and the job is
 * namespaced `[bot:<name>] <label>` so `gizzi workflows wake jobs list` shows ownership.
 *
 * This module owns:
 * - the `[bot:<name>]` naming convention (pure functions),
 * - routine CRUD over the cron job store (CronService is the same service
 *   `gizzi workflows wake jobs add` talks to — the HTTP API in the daemon delegates to it;
 *   heavy imports are dynamic so this file stays drivable from plain bun
 *   tests, mirroring canonical-chat.ts),
 * - the executor delivery seam (deliverBotRoutine): resolve bot, open its
 *   canonical session, run one marked turn. The persona injection hook in
 *   src/runtime/session/prompt.ts keys off the session id, so the turn
 *   carries the bot's identity/SOUL/memory automatically.
 */
import { join } from "node:path"
import { BotStoreError, getBot, type Bot } from "@/runtime/bots/bot-store"
import { openCanonicalChat } from "@/runtime/bots/canonical-chat"
import {
  annotateFailureReason,
  classifyFailure,
  classifyRetry,
} from "@/runtime/bots/failure-reasons"
import type { CreateJobInput, CronJob } from "@/runtime/automation/cron/types"

/* -------------------------------------------------------------------------- */
/* `[bot:<name>]` namespace (D3)                                              */
/* -------------------------------------------------------------------------- */

/** Ownership prefix stamped on every routine job name. */
export function botRoutinePrefix(botName: string): string {
  return `[bot:${botName}]`
}

/** Canonical routine job name: `[bot:<name>] <label>`. */
export function botRoutineJobName(botName: string, label: string): string {
  return `${botRoutinePrefix(botName)} ${label}`
}

/**
 * Split a job name into its bot owner + routine label. Returns null for jobs
 * that are not bot routines. The prefix regex anchors on the closing `]`, so
 * `[bot:alice2]` never parses as bot `alice`.
 */
export function parseBotRoutineJobName(
  jobName: string,
): { bot: string; label: string } | null {
  const match = /^\[bot:([^\]]+)\]\s*(.*)$/.exec(jobName)
  if (!match) return null
  return { bot: match[1]!, label: match[2] ?? "" }
}

/** Routine label for the delivery marker: job name minus the bot namespace. */
export function botRoutineLabel(jobName: string): string {
  return parseBotRoutineJobName(jobName)?.label || jobName
}

/** Turn-text prefix that marks a routine delivery (`[routine: <label>] …`). */
export const ROUTINE_MARKER_PREFIX = "[routine: "

/** True when the turn text carries the routine delivery marker. */
export function isRoutineTurnText(text: string): boolean {
  return text.startsWith(ROUTINE_MARKER_PREFIX)
}

/* -------------------------------------------------------------------------- */
/* Cron job store seam                                                        */
/* -------------------------------------------------------------------------- */

export interface BotRoutineCronDeps {
  create(input: CreateJobInput): CronJob
  list(): CronJob[]
  get(id: string): CronJob | null
  remove(id: string): boolean
}

let initializedDbPath: string | null = null

/**
 * Real deps over the SQLite-backed CronService — the same service the cron
 * daemon's HTTP API (`gizzi workflows wake jobs add`) delegates to. The daemon re-reads the
 * jobs table on every check tick, so jobs created here are picked up without
 * a daemon restart.
 *
 * GIZZI_CRON_DB_PATH overrides the database location (test harnesses); the
 * default matches src/cli/commands/cron.ts (CRON_DB_PATH).
 */
export async function cronServiceDeps(dbPath?: string): Promise<BotRoutineCronDeps> {
  const [{ CronService }, { Global }] = await Promise.all([
    import("@/runtime/automation/cron/service"),
    import("@/runtime/context/global"),
  ])
  const resolved =
    dbPath ?? process.env.GIZZI_CRON_DB_PATH ?? join(Global.Path.data, "cron.db")
  if (initializedDbPath !== resolved) {
    CronService.initialize({ dbPath: resolved })
    initializedDbPath = resolved
  }
  return {
    create: (input) => CronService.create(input),
    list: () => CronService.list(),
    get: (id) => CronService.get(id),
    remove: (id) => CronService.delete(id),
  }
}

/* -------------------------------------------------------------------------- */
/* Routine CRUD                                                               */
/* -------------------------------------------------------------------------- */

export interface AddBotRoutineInput {
  /** Routine label; defaults to the prompt text. */
  label?: string
  /** Cron expression or natural language (e.g. "daily at 9am"). */
  schedule: string
  /** Text delivered into the bot's canonical chat on every run. */
  prompt: string
}

/**
 * Create a cron job (type agent, config.bot set) that delivers `prompt` into
 * the bot's canonical chat. Throws BotStoreError when the bot does not exist
 * or the schedule does not parse (CronService.create throws "Invalid
 * schedule: …").
 */
export async function addBotRoutine(
  botName: string,
  input: AddBotRoutineInput,
  deps?: BotRoutineCronDeps,
): Promise<CronJob> {
  const store = deps ?? (await cronServiceDeps())
  const bot = await getBot(botName)
  if (!bot) throw new BotStoreError(`bot '${botName}' not found`)
  const label = (input.label ?? input.prompt).trim()
  if (!label) throw new BotStoreError("routine label/prompt must not be empty")
  return store.create({
    name: botRoutineJobName(bot.name, label),
    description: `Bot routine for '${bot.name}'`,
    type: "agent",
    schedule: input.schedule,
    config: { prompt: input.prompt, bot: bot.name },
    tags: ["bot", botRoutinePrefix(bot.name)],
    // Missed-due catch-up is standard cron behavior for routines: when the
    // daemon was off at the scheduled time, the fire runs on daemon start
    // (triggeredBy "wake") with the same [routine: <label>] marker.
    catchUpMissed: true,
  })
}

/** All routine jobs owned by the bot, sorted by job name. */
export async function listBotRoutines(
  botName: string,
  deps?: BotRoutineCronDeps,
): Promise<CronJob[]> {
  const store = deps ?? (await cronServiceDeps())
  const bot = await getBot(botName)
  if (!bot) throw new BotStoreError(`bot '${botName}' not found`)
  const prefix = botRoutinePrefix(bot.name).toLowerCase()
  return store
    .list()
    .filter((job) => job.name.toLowerCase().startsWith(prefix))
    .sort((a, b) => a.name.localeCompare(b.name))
}

/**
 * Remove a routine job, refusing to touch jobs owned by another bot (D3
 * prefix scoping). Throws BotStoreError when the job is missing or foreign.
 */
export async function removeBotRoutine(
  botName: string,
  jobId: string,
  deps?: BotRoutineCronDeps,
): Promise<CronJob> {
  const store = deps ?? (await cronServiceDeps())
  const bot = await getBot(botName)
  if (!bot) throw new BotStoreError(`bot '${botName}' not found`)
  const job = store.get(jobId)
  if (!job) throw new BotStoreError(`cron job '${jobId}' not found`)
  const owner = parseBotRoutineJobName(job.name)?.bot.toLowerCase()
  if (owner !== bot.name.toLowerCase()) {
    throw new BotStoreError(
      `cron job '${jobId}' (${job.name}) does not belong to bot '${bot.name}'`,
    )
  }
  store.remove(jobId)
  return job
}

/* -------------------------------------------------------------------------- */
/* Executor delivery (D3)                                                     */
/* -------------------------------------------------------------------------- */

export interface BotRoutineTurnInput {
  sessionID: string
  parts: Array<{ type: "text"; text: string }>
  model?: { providerID: string; modelID: string }
  agent?: string
}

/**
 * Pluggable delivery seams — the cron agent executor injects the real
 * implementation, tests inject in-memory fakes (no session store, no model).
 */
export interface BotRoutineDeliveryDeps {
  getBot(name: string): Promise<Bot | null>
  openCanonicalSession(
    bot: Bot,
    projectPath: string,
  ): Promise<{ projectPath: string; sessionId: string; created: boolean }>
  runTurn(input: BotRoutineTurnInput): Promise<void>
  lastAssistant(sessionID: string): Promise<{ text: string; tokensUsed: number } | null>
  /**
   * In-place compaction of the canonical session (B4/D4): used for the
   * `after_compact` retry policy (context_overflow). Optional so lightweight
   * fakes that never classify to after_compact need not implement it.
   */
  compactSession?(sessionID: string): Promise<void>
}

export function runtimeBotRoutineDelivery(): BotRoutineDeliveryDeps {
  return {
    getBot: (name) => getBot(name),
    openCanonicalSession: (bot, projectPath) => openCanonicalChat(bot, { projectPath }),
    runTurn: async (input) => {
      const { SessionPrompt } = await import("@/runtime/session/prompt")
      await SessionPrompt.prompt(input)
    },
    lastAssistant: async (sessionID) => {
      const { Session } = await import("@/runtime/session")
      const messages = await Session.messages({ sessionID })
      const last = messages.filter((m) => m.info.role === "assistant").at(-1)
      if (!last) return null
      const textParts = last.parts.filter((p) => p.type === "text") as Array<{ text?: string }>
      return {
        text: textParts.map((p) => p.text ?? "").join(""),
        tokensUsed: (last.info as { tokens?: { output?: number } }).tokens?.output ?? 0,
      }
    },
    compactSession: async (sessionID) => {
      // Programmatic in-place compact (the same primitive the TUI /clear→
      // compact path and the loop's auto-compaction drive): summarize the
      // session in place — the session id never forks, so the canonical pin
      // stays valid.
      const [{ SessionCompaction }, { Session }] = await Promise.all([
        import("@/runtime/session/compaction"),
        import("@/runtime/session"),
      ])
      const messages = await Session.messages({ sessionID })
      const lastUser = messages.findLast((m) => m.info.role === "user")
      if (!lastUser) return
      await SessionCompaction.process({
        messages,
        parentID: lastUser.info.id,
        sessionID,
        abort: new AbortController().signal,
        auto: true,
      })
    },
  }
}

export interface DeliverBotRoutineInput {
  botName: string
  /** Routine label — used for the `[routine: <label>]` delivery marker. */
  label: string
  prompt: string
  context?: string
  model?: { providerID: string; modelID: string }
  agentId?: string
  /** Fallback project for a never-pinned bot. */
  projectPath: string
  deps?: BotRoutineDeliveryDeps
}

export interface BotRoutineDelivery {
  sessionId: string
  createdSession: boolean
  response?: string
  tokensUsed?: number
}

/**
 * Deliver one routine run into the bot's canonical chat: resolve the bot
 * (unknown bot → structured error the cron run record captures), resume the
 * pinned session or create + pin one (D2), then run one turn with the
 * `[routine: <label>]` marker so the bot knows why it woke — including
 * missed-due catch-up fires ("wake").
 *
 * Typed retry (B4/D4): a failed turn is classified into the closed failure
 * taxonomy. Transient classes (rate limit, server error, offline, timeout)
 * retry the same delivery once; `context_overflow` compacts the canonical
 * session in place first (the pin survives — compaction never forks the
 * session id) and retries once; everything else (auth/quota/config/model/
 * blocked/unknown) never auto-retries. Every path that gives up attaches the
 * typed `reason` to the thrown error so the executor's run record can persist
 * it alongside the free-text error.
 */
export async function deliverBotRoutine(
  input: DeliverBotRoutineInput,
): Promise<BotRoutineDelivery> {
  const deps = input.deps ?? runtimeBotRoutineDelivery()
  const bot = await deps.getBot(input.botName)
  if (!bot) {
    throw new BotStoreError(
      `bot '${input.botName}' not found — routine '${input.label}' was not delivered`,
    )
  }
  const opened = await deps.openCanonicalSession(
    bot,
    bot.canonicalSession?.projectPath ?? input.projectPath,
  )
  const body = input.context ? `${input.context}\n\n${input.prompt}` : input.prompt
  const marked = `${ROUTINE_MARKER_PREFIX}${input.label}] ${body}`
  const turn = () =>
    deps.runTurn({
      sessionID: opened.sessionId,
      parts: [{ type: "text", text: marked }],
      ...(input.model && { model: input.model }),
      ...(input.agentId && { agent: input.agentId }),
    })

  try {
    await turn()
  } catch (err) {
    const reason = classifyFailure(err)
    const { retry } = classifyRetry(reason)
    if (retry === "once") {
      await turn().catch((retryErr) => {
        throw annotateFailureReason(retryErr, classifyFailure(retryErr))
      })
    } else if (retry === "after_compact") {
      if (deps.compactSession) await deps.compactSession(opened.sessionId)
      await turn().catch((retryErr) => {
        throw annotateFailureReason(retryErr, classifyFailure(retryErr))
      })
    } else {
      throw annotateFailureReason(err, reason)
    }
  }

  const last = await deps.lastAssistant(opened.sessionId).catch(() => null)
  return {
    sessionId: opened.sessionId,
    createdSession: opened.created,
    ...(last?.text && { response: last.text }),
    ...(last && { tokensUsed: last.tokensUsed }),
  }
}
