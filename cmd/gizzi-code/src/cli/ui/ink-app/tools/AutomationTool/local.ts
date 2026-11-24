/**
 * The scheduled tasks that live in this terminal session (no platform, no
 * engine): the in-memory queue that useScheduledTasks / cronScheduler fire
 * between turns, plus the older entries in <project>/.claude/scheduled_tasks.json.
 *
 * The file store takes no new tasks (utils/cronTasks.ts addCronTask is
 * session-only). Its existing entries still load, fire, list and delete.
 */
import { Automation } from '@/runtime/primitives/automation'
import type { AutomationCreateRequest } from '@/runtime/bots/app-bot-routing'
import { parseCronExpression } from '../../utils/cron.js'
import { listAllCronTasks, nextCronRunMs, type CronTask } from '../../utils/cronTasks.js'
import { getTeammateContext } from '../../utils/teammateContext.js'

/** Where a listed task lives. */
export type Where = 'session' | 'legacy' | 'account' | 'computer'

export type Row = {
  id: string
  title: string
  kind: 'message' | 'heartbeat'
  status: string
  /** ISO time of the next run; null when nothing is due. */
  nextRunAt: string | null
  where: Where
}

export const LEGACY_FILE = '.claude/scheduled_tasks.json'

/** Teammate tasks carry the teammate's agentId so fires reach its queue. */
export function ownerAgentId(): string | undefined {
  return getTeammateContext()?.agentId
}

/** Session and legacy tasks this agent may see: a teammate sees only its own. */
export async function localTasks(): Promise<CronTask[]> {
  const agentId = ownerAgentId()
  return (await listAllCronTasks()).filter(t => !agentId || t.agentId === agentId)
}

export async function findLocalTask(id: string): Promise<CronTask | undefined> {
  return (await localTasks()).find(t => t.id === id)
}

export function isLegacy(task: CronTask): boolean {
  return task.durable !== false
}

function clip(text: string): string {
  const raw = text.trim().split('\n', 1)[0]!.replace(/\s+/g, ' ').trim()
  return raw.length > Automation.MAX_TITLE_CHARS ? raw.slice(0, Automation.MAX_TITLE_CHARS - 1).trimEnd() + '…' : raw
}

export function localRow(task: CronTask, now: number = Date.now()): Row {
  // A one-shot is pinned to one time from when it was made; a passed one was missed.
  const next = task.recurring ? nextCronRunMs(task.cron, now) : nextCronRunMs(task.cron, task.createdAt)
  const missed = !task.recurring && next !== null && next <= now
  return {
    id: task.id,
    title: clip(task.prompt),
    kind: 'message',
    status: missed ? 'missed' : 'active',
    nextRunAt: next !== null && !missed ? new Date(next).toISOString() : null,
    where: isLegacy(task) ? 'legacy' : 'session',
  }
}

/** One line per task for the model, in the same shape as the core's list. */
export function localLine(task: CronTask, zone: string, now: number = Date.now()): string {
  const row = localRow(task, now)
  const next = row.nextRunAt ? `next ${Automation.formatInZone(row.nextRunAt, zone)}` : 'nothing due'
  const where = row.where === 'legacy' ? `older task in ${LEGACY_FILE} (delete only)` : 'this terminal session only'
  return `- ${row.id} · "${row.title}" · message · ${row.status} · cron "${task.cron}" (this machine's zone) · ${task.recurring ? 'repeating' : 'once'} · ${next} · ${where}`
}

/**
 * A create request as the session queue stores it: a 5-field cron read in
 * this machine's zone. A one-time task becomes a pinned cron at its minute
 * (rounded up, never early by a minute). The queue can't read another zone,
 * nor end after COUNT/UNTIL, so those are refused with the way out.
 */
export function toSessionCron(
  request: AutomationCreateRequest,
  processZone: string,
  now: Date = new Date(),
): { cron: string; recurring: boolean; next: number } {
  if (request.maxRuns != null || request.until != null) {
    throw new Automation.AutomationError(
      'invalid',
      "This terminal session's queue can't stop after COUNT or UNTIL. Leave them out (the task ends with the session anyway), or use target new_session.",
    )
  }
  let cron: string
  let recurring: boolean
  if (request.schedule.type === 'once') {
    const at = new Date(Math.ceil(new Date(request.schedule.at).getTime() / 60_000) * 60_000)
    cron = `${at.getMinutes()} ${at.getHours()} ${at.getDate()} ${at.getMonth() + 1} *`
    recurring = false
  } else {
    if (request.timezone !== processZone) {
      throw new Automation.AutomationError(
        'invalid',
        `This terminal session's queue reads repeating schedules in this machine's zone (${processZone}), not ${request.timezone}. Use target new_session for a schedule in ${request.timezone}.`,
      )
    }
    cron = request.schedule.expression
    recurring = true
  }
  if (!parseCronExpression(cron)) {
    throw new Automation.AutomationError('invalid', `This terminal session's queue can't read the schedule "${cron}". Use target new_session.`)
  }
  const next = nextCronRunMs(cron, now.getTime())
  if (next === null) throw new Automation.AutomationError('out_of_window', 'That schedule has no run in the next year.')
  return { cron, recurring, next }
}
