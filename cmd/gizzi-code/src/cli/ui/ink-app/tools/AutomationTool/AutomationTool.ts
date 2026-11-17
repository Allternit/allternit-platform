// @ts-nocheck
/**
 * automation in the Gizzi TUI: a thin adapter over the shared core
 * (runtime/primitives/automation.ts), the same one the runtime builtin uses,
 * with the same description (automation.txt) and permission defaults.
 * Replaces the Ink CronCreate / CronList / CronDelete tools.
 *
 * Where a task goes:
 *   - message + this_thread (the default): this terminal session's own
 *     in-memory queue (utils/cronTasks.ts session tasks, fired between turns
 *     by useScheduledTasks / cronScheduler). It ends with the session, and
 *     the result says so.
 *   - new_session, and every heartbeat: the shared core. The user's Allternit
 *     account (allternit-api) when this machine has a credential; otherwise
 *     this computer's scheduler (the local engine's CronService, through
 *     ./cronBackend.ts and Automation.cronFallback); otherwise the result says
 *     nothing was scheduled and gives the start command.
 *   - Older tasks in .claude/scheduled_tasks.json still fire (the scheduler
 *     loads them), list and delete here; they take no new writes.
 *
 * Permission: `list` is allowed; every other action asks unless a rule
 * `automation(<action>)` allows it (the runtime asks per action the same way,
 * runtime/loop/agent.ts). A turn started by a scheduled task (workload
 * "cron") can't create one: refused, never asked.
 */
import { z } from 'zod/v4'
import { Automation } from '@/runtime/primitives/automation'
import { Clock } from '@/runtime/primitives/clock'
import { getSessionId, setScheduledTasksEnabled } from '../../bootstrap/state.js'
import { getLiveSession } from '../../services/liveSession.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { addCronTask, removeCronTasks } from '../../utils/cronTasks.js'
import { getCwd } from '../../utils/cwd.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { enqueuePendingNotification } from '../../utils/messageQueueManager.js'
import { getRuleByContentsForTool } from '../../utils/permissions/permissions.js'
import { getWorkload, WORKLOAD_CRON } from '../../utils/workloadContext.js'
import { tuiCronBackend } from './cronBackend.js'
import {
  findLocalTask,
  isLegacy,
  LEGACY_FILE,
  localLine,
  localRow,
  localTasks,
  ownerAgentId,
  toSessionCron,
  type Row,
} from './local.js'
import { AUTOMATION_TOOL_NAME, DESCRIPTION, isKairosCronEnabled, PROMPT, SESSION_MAX_AGE_DAYS } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => Automation.parameters.strict())
type InputSchema = ReturnType<typeof inputSchema>

const rowSchema = z.object({
  id: z.string(),
  title: z.string(),
  kind: z.enum(['message', 'heartbeat']),
  status: z.string(),
  nextRunAt: z.string().nullable(),
  where: z.enum(['session', 'legacy', 'account', 'computer']),
})

const outputSchema = lazySchema(() =>
  z.object({
    action: z.enum(['create', 'list', 'update', 'delete', 'run_now']),
    status: z.enum(['ok', 'no_platform', 'no_scheduler']),
    /** Where the change was stored (list: omitted). */
    where: z.enum(['session', 'legacy', 'platform', 'cron']).optional(),
    /** Card title. */
    title: z.string(),
    /** What the model reads back. */
    text: z.string(),
    /** The user's zone, for showing times. */
    zone: z.string(),
    rows: z.array(rowSchema).optional(),
    /** list: why account or computer tasks are missing from the table. */
    note: z.string().optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

/** Max tasks in this terminal session's queue (the account and computer caps live there). */
const MAX_SESSION_TASKS = Automation.MAX_ACTIVE

function fromAutomation(): boolean {
  return getWorkload() === WORKLOAD_CRON
}

/** A message for this thread stays in the terminal; everything else goes to the shared core. */
function goesToSessionQueue(built: Automation.Built): boolean {
  return built.action === 'create' && built.request.kind === 'message' && built.request.target === 'this_thread'
}

/** The session the account or the engine knows: the live engine session when attached, else this terminal's. */
function platformSessionId(): string {
  const live = getLiveSession()
  return live.connected && live.sessionID ? live.sessionID : getSessionId()
}

async function userZone(timezone?: string): Promise<Automation.Zone> {
  try {
    return await Automation.zoneFor({ timezone })
  } catch {
    return Clock.resolveTimeZone()
  }
}

function remoteRow(a: { id: string; title: string; kind: 'message' | 'heartbeat'; status: string; nextRunAt: string | null }, via?: Automation.Via): Row {
  return {
    id: a.id,
    title: a.title,
    kind: a.kind,
    status: a.status,
    nextRunAt: a.nextRunAt,
    where: via === 'cron' ? 'computer' : 'account',
  }
}

/** Send to the account, falling back to this computer's scheduler. */
async function sendRemote(built: Automation.Built): Promise<Automation.Outcome> {
  const sessionID = platformSessionId()
  const outcome = await Automation.send({ sessionID, built })
  if (outcome.status !== 'no_platform') return outcome
  return Automation.cronFallback({
    sessionID,
    built: outcome.built,
    reason: outcome.reason,
    backend: await tuiCronBackend(),
    directory: getCwd(),
  })
}

function remoteOutput(action: Automation.Action, outcome: Automation.Outcome, zone: string): Output {
  const out: Output = {
    action,
    status: outcome.status,
    title: Automation.title(outcome),
    text: Automation.summary(outcome),
    zone,
  }
  if (outcome.status === 'ok') out.where = outcome.via === 'cron' ? 'cron' : 'platform'
  return out
}

async function check(input: Automation.Params): Promise<Automation.Built> {
  const built = await Automation.build(input, { fromAutomation: fromAutomation() })
  if (goesToSessionQueue(built)) {
    if (!isKairosCronEnabled()) {
      throw new Automation.AutomationError(
        'invalid',
        "This terminal session's queue is turned off here (GIZZI_CODE_DISABLE_CRON). Use target new_session.",
      )
    }
    const { cron } = toSessionCron(built.request, Clock.processZone())
    const session = (await localTasks()).filter(t => !isLegacy(t))
    if (session.length >= MAX_SESSION_TASKS) {
      throw new Automation.AutomationError(
        'invalid',
        `This terminal session already has ${MAX_SESSION_TASKS} scheduled tasks. Delete one first (use action list).`,
      )
    }
    const duplicate = session.find(t => t.cron === cron && t.prompt === built.request.prompt)
    if (duplicate) {
      throw new Automation.AutomationError(
        'invalid',
        `A scheduled task like this already exists in this session (id ${duplicate.id}). Don't make another.`,
      )
    }
  }
  if (built.action === 'update' || built.action === 'run_now') {
    const local = await findLocalTask(built.request.id)
    if (local && built.action === 'update') {
      throw new Automation.AutomationError(
        'invalid',
        `Scheduled task ${local.id} is ${isLegacy(local) ? `an older task in ${LEGACY_FILE}` : "in this terminal session's queue"} and can't be changed. Delete it and create a new one.`,
      )
    }
    if (local && local.agentId) {
      throw new Automation.AutomationError('invalid', `Scheduled task ${local.id} belongs to a teammate and runs in its queue; it can't be run from here.`)
    }
  }
  return built
}

async function list(zone: string): Promise<Output> {
  const now = Date.now()
  const tasks = await localTasks()
  const rows: Row[] = tasks.map(t => localRow(t, now))
  const lines: string[] = []
  if (tasks.length) {
    lines.push(`In this terminal (${tasks.length}):`, ...tasks.map(t => localLine(t, zone, now)))
  }
  let note: string | undefined
  try {
    const outcome = await sendRemote({ action: 'list' })
    if (outcome.status === 'ok' && outcome.action === 'list') {
      rows.push(...outcome.automations.map(a => remoteRow(a, outcome.via)))
      if (outcome.automations.length || !tasks.length) lines.push(Automation.summary(outcome))
    } else if (outcome.status !== 'ok') {
      const why = Automation.NO_PLATFORM_TEXT[outcome.reason]
      note =
        outcome.status === 'no_scheduler'
          ? `Tasks on the user's account or this computer's scheduler aren't listed: ${why}, and this computer's scheduler isn't running (\`${Automation.START_COMMAND}\` starts it).`
          : `Tasks on the user's account aren't listed: ${why}.`
    }
  } catch (error) {
    note = `Tasks on the user's account or this computer's scheduler aren't listed: ${error instanceof Error ? error.message : String(error)}`
  }
  if (!lines.length) lines.push('No scheduled tasks in this terminal.')
  if (note) lines.push(note)
  return {
    action: 'list',
    status: 'ok',
    title: `Scheduled tasks (${rows.length})`,
    text: lines.join('\n'),
    zone,
    rows,
    ...(note ? { note } : {}),
  }
}

async function createInSession(built: Extract<Automation.Built, { action: 'create' }>): Promise<Output> {
  const r = built.request
  const { cron, recurring, next } = toSessionCron(r, Clock.processZone())
  const id = await addCronTask(cron, r.prompt, recurring, ownerAgentId())
  // Wake the scheduler if this is the session's first task.
  setScheduledTasksEnabled(true)
  const zone = built.zone.zone
  const lines = [
    `Scheduled task "${r.title}" created (id ${id}, active) in this terminal session's queue. Next run: ${Automation.formatInZone(new Date(next), zone)}.`,
    `It runs here between turns only while this Gizzi session stays open, and it ends with the session${recurring ? `; a repeating task also stops after ${SESSION_MAX_AGE_DAYS} days` : ''}. Tell the user that in one sentence. For a task that must outlast this session, use target new_session.`,
  ]
  if (built.zone.source === 'process') {
    lines.push("The zone used is this machine's, which may not be the user's. Tell the user which zone the time is in.")
  }
  return {
    action: 'create',
    status: 'ok',
    where: 'session',
    title: `Scheduled: ${r.title}`,
    text: lines.join('\n'),
    zone,
  }
}

export const AutomationTool = buildTool({
  name: AUTOMATION_TOOL_NAME,
  searchHint: 'schedule a reminder, follow-up or recurring check; list or cancel scheduled tasks',
  maxResultSizeChars: 50_000,
  userFacingName: () => 'Scheduled tasks',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  shouldDefer: true,
  isReadOnly(input) {
    return input?.action === 'list'
  },
  isConcurrencySafe(input) {
    return input?.action === 'list'
  },
  toAutoClassifierInput(input) {
    return [input.action, input.id, input.target, input.kind, input.prompt].filter(Boolean).join(' ')
  },
  async validateInput(input) {
    try {
      await check(input)
      return { result: true }
    } catch (error) {
      return { result: false, message: error instanceof Error ? error.message : String(error), errorCode: 1 }
    }
  },
  async checkPermissions(input, context) {
    if (input.action === 'list') return { behavior: 'allow', updatedInput: input }
    const permissionContext = context.getAppState().toolPermissionContext
    const ruleContent = input.action
    const deny = getRuleByContentsForTool(permissionContext, AutomationTool, 'deny').get(ruleContent)
    if (deny) {
      return {
        behavior: 'deny',
        message: `${AUTOMATION_TOOL_NAME} ${ruleContent} is denied by a permission rule.`,
        decisionReason: { type: 'rule', rule: deny },
      }
    }
    const message = `Gizzi wants to ${ruleContent === 'run_now' ? 'run' : ruleContent} a scheduled task.`
    const suggestions = [
      {
        type: 'addRules',
        destination: 'localSettings',
        rules: [{ toolName: AUTOMATION_TOOL_NAME, ruleContent }],
        behavior: 'allow',
      },
    ]
    const ask = getRuleByContentsForTool(permissionContext, AutomationTool, 'ask').get(ruleContent)
    if (ask) return { behavior: 'ask', message, decisionReason: { type: 'rule', rule: ask }, suggestions }
    const allow = getRuleByContentsForTool(permissionContext, AutomationTool, 'allow').get(ruleContent)
    if (allow) return { behavior: 'allow', updatedInput: input, decisionReason: { type: 'rule', rule: allow } }
    return { behavior: 'ask', message, suggestions }
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.text }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input) {
    const built = await check(input)
    const zone = (await userZone(input.timezone)).zone
    switch (built.action) {
      case 'list':
        return { data: await list(zone) }
      case 'create':
        if (goesToSessionQueue(built)) return { data: await createInSession(built) }
        return { data: remoteOutput('create', await sendRemote(built), zone) }
      case 'update':
        return { data: remoteOutput('update', await sendRemote(built), zone) }
      case 'delete': {
        const local = await findLocalTask(built.request.id)
        if (!local) return { data: remoteOutput('delete', await sendRemote(built), zone) }
        await removeCronTasks([local.id])
        const legacy = isLegacy(local)
        return {
          data: {
            action: 'delete',
            status: 'ok',
            where: legacy ? 'legacy' : 'session',
            title: 'Scheduled task deleted',
            text: `Scheduled task ${local.id} deleted${legacy ? ` (removed from ${LEGACY_FILE})` : ''}.`,
            zone,
          },
        }
      }
      case 'run_now': {
        const local = await findLocalTask(built.request.id)
        if (!local) return { data: remoteOutput('run_now', await sendRemote(built), zone) }
        // Same queue entry the scheduler makes when the task fires.
        enqueuePendingNotification({
          value: local.prompt,
          mode: 'prompt',
          priority: 'later',
          isMeta: true,
          workload: WORKLOAD_CRON,
        })
        return {
          data: {
            action: 'run_now',
            status: 'ok',
            where: isLegacy(local) ? 'legacy' : 'session',
            title: 'Scheduled task run',
            text: `Scheduled task ${local.id} will run in this session as soon as this turn ends.`,
            zone,
          },
        }
      }
    }
  },
} satisfies ToolDef<InputSchema, Output>)
