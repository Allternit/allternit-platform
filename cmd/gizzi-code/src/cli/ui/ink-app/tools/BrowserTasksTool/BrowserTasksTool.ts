import { z } from 'zod/v4'
import { BrowserTasks } from '@/runtime/primitives/browser-tasks'
import { BrowserTasksTool as RuntimeBrowserTasksTool, browserTasksAvailableNow } from '@/runtime/tools/builtins/browser-tasks'
import { Provider } from '@/runtime/providers/provider'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { BROWSER_TASKS_TOOL_NAME, DESCRIPTION, PROMPT } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => BrowserTasks.Input)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    results: z.array(
      z.object({ id: z.string(), status: BrowserTasks.Status, result: z.string(), final_url: z.string() }),
    ),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

// isEnabled() is sync: it reads the core's last availability check, which runs
// in the background (timer + first use); the tool appears once it reports a
// local executor.
export const BrowserTasksTool = buildTool({
  name: BROWSER_TASKS_TOOL_NAME,
  searchHint: 'run several hidden browser tasks in parallel',
  maxResultSizeChars: 60_000,
  userFacingName: () => 'Browser tasks',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isEnabled() {
    return browserTasksAvailableNow()
  },
  isReadOnly() {
    return false
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return input.tasks.map(t => `${t.id} ${t.start_url ?? ''}`).join('\n')
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  async validateInput(input) {
    const problem = BrowserTasks.validate(input)
    return problem ? { result: false, message: problem, errorCode: 1 } : { result: true }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: BrowserTasks.render(output.results) }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async checkPermissions(input) {
    const hosts = BrowserTasks.hosts(input.tasks).join(', ')
    return { behavior: 'ask', message: `${input.action} browser tasks on ${hosts || 'the pages you named'}`, updatedInput: input }
  },
  async call(input, context) {
    // The runtime core runs the tasks: the user's configured model, the
    // action-class gates, 4 at once / 10 minutes. Consent was asked above.
    const sessionID = String(context.agentId ?? 'tui')
    const ref = await Provider.defaultModelConcrete()
    const model = await Provider.getModel(ref.providerID, ref.modelID)
    const tool = await RuntimeBrowserTasksTool.init()
    const out = await tool.execute(input, {
      sessionID,
      messageID: sessionID,
      agent: 'gizzi',
      abort: context.abortController.signal,
      extra: { model },
      messages: [],
      metadata() {},
      async ask() {},
    })
    return { data: { results: out.metadata.results } }
  },
} satisfies ToolDef<InputSchema, Output>)
