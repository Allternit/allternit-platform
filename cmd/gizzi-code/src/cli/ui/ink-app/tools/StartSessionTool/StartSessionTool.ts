/**
 * start_session in the Gizzi TUI (Start anywhere, D8): start a session on this
 * or another of the user's computers. Always asks (bypass-immune, like
 * folder_access); the mode never goes above this session's own; depth is the
 * parent's + 1 and refused above 3; every call carries a fresh Idempotency-Key.
 */
import { z } from 'zod/v4'
import { hostname } from 'os'
import { buildTool, type ToolDef } from '../../Tool.js'
import { getSessionId } from '../../bootstrap/state.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  callerStartMode,
  checkStart,
  isStartMode,
  START_MODES,
} from '@/cli/work/start-guards'
import { parentDepth, startSession } from '@/cli/work/start-session'
import { DESCRIPTION, PROMPT, START_SESSION_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() =>
  z.strictObject({
    device: z
      .string()
      .optional()
      .describe('Name or runtime id of the computer (see `gizzi start --list`); leave out for this computer'),
    folder: z.string().min(1).describe('Absolute folder path on that computer'),
    runtime: z.string().optional().describe('Coding tool to run; default "allternit" (Gizzi)'),
    mode: z.enum(START_MODES).optional().describe('plan, default, auto or bypass; never above this session\'s own mode'),
    prompt: z.string().optional().describe('The first task for the new session'),
  }),
)
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    kind: z.enum(['engine', 'terminal']),
    id: z.string(),
    directory: z.string(),
    device: z.string(),
    runtime: z.string(),
    line: z.string(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>
export type Output = z.infer<OutputSchema>

export const StartSessionTool = buildTool({
  name: START_SESSION_TOOL_NAME,
  searchHint: 'start a coding session on this or another computer',
  maxResultSizeChars: 10_000,
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  userFacingName() {
    return 'Start session'
  },
  shouldDefer: true,
  isConcurrencySafe() {
    return false
  },
  isReadOnly() {
    return false
  },
  requiresUserInteraction() {
    return true
  },
  toAutoClassifierInput(input) {
    return `${input.device ?? 'this computer'} ${input.folder} ${input.runtime ?? 'allternit'} ${input.mode ?? ''}`
  },
  async validateInput(input, context) {
    const caller = callerStartMode(context.getAppState().toolPermissionContext.mode)
    const guard = checkStart({
      callerMode: caller,
      requestedMode: input.mode && isStartMode(input.mode) ? input.mode : undefined,
      parentDepth: parentDepth(),
    })
    return guard.ok ? { result: true } : { result: false, message: guard.message, errorCode: 1 }
  },
  async checkPermissions(input, context) {
    const caller = callerStartMode(context.getAppState().toolPermissionContext.mode)
    const mode = input.mode ?? caller
    return {
      behavior: 'ask',
      message: `Start a ${input.runtime ?? 'allternit'} session on ${input.device ?? `this computer (${hostname()})`} in ${input.folder}, mode ${mode}?${input.prompt ? ` First task: ${input.prompt.slice(0, 200)}` : ''}`,
      decisionReason: {
        type: 'safetyCheck',
        reason: `start_session ${input.device ?? 'local'} ${input.folder} ${mode}`,
        classifierApprovable: false,
      },
    }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    const caller = callerStartMode(context.getAppState().toolPermissionContext.mode)
    const guard = checkStart({ callerMode: caller, requestedMode: input.mode, parentDepth: parentDepth() })
    if (!guard.ok) throw new Error(guard.message)
    const result = await startSession({
      device: input.device,
      folder: input.folder,
      runtime: input.runtime,
      mode: guard.mode,
      prompt: input.prompt,
      parentSessionId: String(getSessionId()),
      depth: guard.depth,
    })
    return { data: result }
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      type: 'tool_result',
      content: JSON.stringify({ [output.kind === 'terminal' ? 'terminalId' : 'sessionId']: output.id, message: output.line }),
      tool_use_id: toolUseID,
    }
  },
} satisfies ToolDef<InputSchema, Output>)
