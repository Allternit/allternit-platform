import { z } from 'zod/v4'
import { buildTool, type ToolDef } from '../../Tool.js'
import { cronToHuman } from '../../utils/cron.js'
import { listAllCronTasks } from '../../utils/cronTasks.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  buildCronListPrompt,
  CRON_LIST_DESCRIPTION,
  CRON_LIST_TOOL_NAME,
  isDurableCronEnabled,
  isKairosCronEnabled,
} from './prompt.js'
import { getCronOwnerAgentId } from './shared.js'
import { renderListResultMessage, renderListToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => z.strictObject({}))
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    jobs: z.array(
      z.object({
        id: z.string(),
        cron: z.string(),
        humanSchedule: z.string(),
        prompt: z.string(),
        recurring: z.boolean(),
        durable: z.boolean(),
      }),
    ),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type ListOutput = z.infer<OutputSchema>

export const CronListTool = buildTool({
  name: CRON_LIST_TOOL_NAME,
  searchHint: 'list scheduled cron jobs',
  maxResultSizeChars: 50_000,
  userFacingName: () => 'Scheduled jobs',
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
    return true
  },
  isReadOnly() {
    return true
  },
  toAutoClassifierInput() {
    return ''
  },
  async description() {
    return CRON_LIST_DESCRIPTION
  },
  async prompt() {
    return buildCronListPrompt(isDurableCronEnabled())
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    const content =
      output.jobs.length === 0
        ? 'No scheduled jobs.'
        : output.jobs
            .map(
              j =>
                `${j.id} — ${j.humanSchedule} (${j.cron})${j.recurring ? ' recurring' : ' one-shot'}${j.durable ? ' [durable]' : ' [session]'}: ${j.prompt}`,
            )
            .join('\n')
    return { tool_use_id: toolUseID, type: 'tool_result', content }
  },
  renderToolUseMessage: renderListToolUseMessage,
  renderToolResultMessage: renderListResultMessage,
  async call() {
    const agentId = getCronOwnerAgentId()
    const tasks = (await listAllCronTasks()).filter(
      // A teammate sees only its own jobs; the main agent sees everything.
      t => !agentId || t.agentId === agentId,
    )
    return {
      data: {
        jobs: tasks.map(t => ({
          id: t.id,
          cron: t.cron,
          humanSchedule: cronToHuman(t.cron),
          prompt: t.prompt,
          recurring: Boolean(t.recurring),
          durable: t.durable !== false,
        })),
      },
    }
  },
} satisfies ToolDef<InputSchema, ListOutput>)
