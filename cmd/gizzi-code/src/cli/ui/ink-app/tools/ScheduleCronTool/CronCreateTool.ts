import { z } from 'zod/v4'
import { setScheduledTasksEnabled } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { cronToHuman, parseCronExpression } from '../../utils/cron.js'
import { addCronTask, listAllCronTasks, nextCronRunMs } from '../../utils/cronTasks.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  buildCronCreateDescription,
  buildCronCreatePrompt,
  CRON_CREATE_TOOL_NAME,
  DEFAULT_MAX_AGE_DAYS,
  isDurableCronEnabled,
  isKairosCronEnabled,
} from './prompt.js'
import { getCronOwnerAgentId, MAX_CRON_JOBS } from './shared.js'
import { renderCreateResultMessage, renderCreateToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    cron: z
      .string()
      .describe(
        'Standard 5-field cron expression in local time: "M H DoM Mon DoW" (e.g. "*/5 * * * *" = every 5 minutes, "30 14 28 2 *" = Feb 28 at 2:30pm local once).',
      ),
    prompt: z.string().describe('The prompt to enqueue at each fire time.'),
    recurring: z
      .boolean()
      .optional()
      .describe(
        `true (default) = fire on every cron match until deleted or auto-expired after ${DEFAULT_MAX_AGE_DAYS} days. false = fire once at the next match, then auto-delete.`,
      ),
    durable: z
      .boolean()
      .optional()
      .describe(
        'true = persist to .claude/scheduled_tasks.json and survive restarts. false (default) = in-memory only, dies when this session ends.',
      ),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    id: z.string(),
    humanSchedule: z.string(),
    recurring: z.boolean(),
    durable: z.boolean(),
    nextFireAt: z.number().nullable(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type CreateOutput = z.infer<OutputSchema>

export const CronCreateTool = buildTool({
  name: CRON_CREATE_TOOL_NAME,
  searchHint: 'schedule a recurring or one-shot prompt',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Schedule',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  shouldDefer: true,
  isEnabled() {
    return isKairosCronEnabled()
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return `${input.cron} ${input.prompt}`
  },
  async validateInput({ cron }) {
    if (!parseCronExpression(cron)) {
      return {
        result: false,
        message: `Invalid cron expression '${cron}'. Expected 5 fields: minute hour day-of-month month day-of-week.`,
        errorCode: 1,
      }
    }
    if (nextCronRunMs(cron, Date.now()) === null) {
      return {
        result: false,
        message: `Cron expression '${cron}' does not match any date in the next year.`,
        errorCode: 2,
      }
    }
    const existing = await listAllCronTasks()
    if (existing.length >= MAX_CRON_JOBS) {
      return {
        result: false,
        message: `Too many scheduled jobs (max ${MAX_CRON_JOBS}). Delete one with CronDelete first.`,
        errorCode: 3,
      }
    }
    return { result: true }
  },
  async description() {
    return buildCronCreateDescription(isDurableCronEnabled())
  },
  async prompt() {
    return buildCronCreatePrompt(isDurableCronEnabled())
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    const when = output.recurring
      ? `recurring (${output.humanSchedule}); auto-expires after ${DEFAULT_MAX_AGE_DAYS} days`
      : `one-shot (${output.humanSchedule})`
    const where = output.durable
      ? 'persisted to .claude/scheduled_tasks.json'
      : 'session-only (not written to disk, dies when the session ends)'
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: `Scheduled job ${output.id}: ${when}, ${where}. Cancel with CronDelete id=${output.id}.`,
    }
  },
  renderToolUseMessage: renderCreateToolUseMessage,
  renderToolResultMessage: renderCreateResultMessage,
  async call({ cron, prompt, recurring = true, durable = false }) {
    const agentId = getCronOwnerAgentId()
    // Teammate crons and a disabled durable kill switch force session-only.
    const effectiveDurable = durable && !agentId && isDurableCronEnabled()
    const id = await addCronTask(cron, prompt, recurring, effectiveDurable, agentId)
    // Wake the scheduler if this is the session's first job.
    setScheduledTasksEnabled(true)
    return {
      data: {
        id,
        humanSchedule: cronToHuman(cron),
        recurring,
        durable: effectiveDurable,
        nextFireAt: nextCronRunMs(cron, Date.now()),
      },
    }
  },
} satisfies ToolDef<InputSchema, CreateOutput>)

