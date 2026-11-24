import type { LocalCommandResult } from '../../commands'
import type { ToolUseContext } from '../../Tool'

export const NO_MESSAGE_NOTICE = 'No message to edit yet.'

export async function call(_args: string, context: ToolUseContext): Promise<LocalCommandResult> {
  // The REPL owns the same rewind + composer prefill that /rewind and the
  // message "edit" action use; it returns false when there is no user message.
  const restored = context.restoreLastUserMessage?.() ?? false
  if (!restored) return { type: 'text', value: NO_MESSAGE_NOTICE }
  return { type: 'skip' }
}
