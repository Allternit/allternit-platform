/**
 * send_user_message in the Gizzi TUI: a thin adapter over the shared core
 * (runtime/primitives/user-message.ts), the same one the runtime builtin uses.
 *
 * Files may come from the folders the TUI's own file tools work in (the
 * working directories, the current directory, the scratchpad). The push goes
 * through allternit-api `user/notify` when this machine has a platform
 * credential (`gizzi login` or ALLTERNIT_API_TOKEN); otherwise the result
 * says "unavailable" and the message stays in the terminal.
 *
 * Named in snake_case like the runtime tool; the dead KAIROS BriefTool owns
 * the name "SendUserMessage".
 */
import { z } from 'zod/v4'
import { UserMessage } from '@/runtime/primitives/user-message'
import { getSessionId } from '../../bootstrap/state.js'
import { buildTool, type ToolDef } from '../../Tool.js'
import { getCwd } from '../../utils/cwd.js'
import { lazySchema } from '../../utils/lazySchema.js'
import {
  allWorkingDirectories,
  getScratchpadDir,
  isScratchpadEnabled,
} from '../../utils/permissions/filesystem.js'
import { DESCRIPTION, PROMPT, SEND_USER_MESSAGE_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => UserMessage.parameters.strict())
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    text: z.string(),
    files: z.array(
      z.object({
        id: z.string().optional(),
        name: z.string(),
        size: z.number(),
        display: z.enum(['render', 'attach']),
        path: z.string(),
        caption: z.string().optional(),
        not_saved: z.enum(['drive_off', 'unavailable', 'failed']).optional(),
      }),
    ),
    push: z.enum(['sent', 'not_requested', 'rate_limited', 'unpaired', 'unavailable']),
    notify: z.enum(['none', 'push', 'needs_you']),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

/** The folders a file may come from in the TUI. */
function roots(context): string[] {
  const dirs = [getCwd(), ...allWorkingDirectories(context.getAppState().toolPermissionContext)]
  try {
    if (isScratchpadEnabled()) dirs.push(getScratchpadDir())
  } catch {
    // No scratchpad in this session.
  }
  return [...new Set(dirs)]
}

export const SendUserMessageTool = buildTool({
  name: SEND_USER_MESSAGE_TOOL_NAME,
  searchHint: 'hand the user a finished deliverable, or notify their phone',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Message to you',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return [input.notify ?? 'none', ...(input.attachments ?? []).map(a => a.path)].join(' ')
  },
  async validateInput(input, context) {
    // Refuse a bad file before anything is sent (the core checks again in call).
    const ctx = { roots: roots(context), cwd: getCwd() }
    for (const attachment of input.attachments ?? []) {
      try {
        await UserMessage.checkAttachment(attachment, ctx)
      } catch (error) {
        return { result: false, message: error instanceof Error ? error.message : String(error), errorCode: 1 }
      }
    }
    return { result: true }
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return {
      tool_use_id: toolUseID,
      type: 'tool_result',
      content: UserMessage.summary(output, output.notify),
    }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call({ text, attachments, notify }, context) {
    const result = await UserMessage.sendUserMessage({
      sessionID: getSessionId(),
      text,
      attachments,
      notify: notify ?? 'none',
      ctx: { roots: roots(context), cwd: getCwd() },
    })
    return { data: { ...result, notify: notify ?? 'none' } }
  },
} satisfies ToolDef<InputSchema, Output>)
