import { z } from 'zod/v4'
import { buildTool, type ToolDef } from '../../Tool.js'
import { listAllCronTasks, removeCronTasks } from '../../utils/cronTasks.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  buildCronDeletePrompt,
  CRON_DELETE_DESCRIPTION,
  CRON_DELETE_TOOL_NAME,
  isDurableCronEnabled,
  isKairosCronEnabled,
} from './prompt.js'
import { getCronOwnerAgentId } from './shared.js'
import { renderDeleteResultMessage, renderDeleteToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    id: z.string().describe('Job ID returned by CronCreate.'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() => z.object({ id: z.string() }))
type OutputSchema = ReturnType<typeof outputSchema>
export type DeleteOutput = z.infer<OutputSchema>

export const CronDeleteTool = buildTool({
  name: CRON_DELETE_TOOL_NAME,
  searchHint: 'cancel a scheduled cron job',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Unschedule',
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
    return input.id
  },
  async validateInput({ id }) {
    const task = (await listAllCronTasks()).find(t => t.id === id)
    if (!task) {
      return { result: false, message: `No scheduled job with id '${id}'.`, errorCode: 1 }
    }
    // A teammate may only cancel its own jobs.
    const agentId = getCronOwnerAgentId()
    if (agentId && task.agentId !== agentId) {
      return {
        result: false,
        message: `Job '${id}' belongs to another agent.`,
        errorCode: 2,
      }
    }
    return { result: true }
  },
  async description() {
    return CRON_DELETE_DESCRIPTION
  },
  async prompt() {
    return buildCronDeletePrompt(isDurableCronEnabled())
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: `Cancelled job ${output.id}.`,
    }
  },
  renderToolUseMessage: renderDeleteToolUseMessage,
  renderToolResultMessage: renderDeleteResultMessage,
  async call({ id }) {
    await removeCronTasks([id])
    return { data: { id } }
  },
} satisfies ToolDef<InputSchema, DeleteOutput>)
