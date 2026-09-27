import { getEmptyToolPermissionContext } from '../Tool.js'
import { queryModelWithoutStreaming } from '../services/api/claude.js'
import type { Message } from '../types/message.js'
import { logForDebugging } from '../utils/debug.js'
import { createUserMessage, getAssistantMessageText } from '../utils/messages.js'
import { getSmallFastModel } from '../utils/model/model.js'
import { asSystemPrompt } from '../utils/systemPromptType.js'

/**
 * One-shot small-model call for the companion (hatching its soul, reacting
 * to a turn). Returns null on any failure — the companion is decoration and
 * must never surface an error or block the REPL.
 */
export async function askCompanionModel(
  messages: Message[],
  system: string,
  signal: AbortSignal,
  querySource: string,
): Promise<string | null> {
  try {
    const response = await queryModelWithoutStreaming({
      messages,
      systemPrompt: asSystemPrompt([system]),
      thinkingConfig: { type: 'disabled' },
      tools: [],
      signal,
      options: {
        getToolPermissionContext: async () => getEmptyToolPermissionContext(),
        model: getSmallFastModel(),
        toolChoice: undefined,
        isNonInteractiveSession: false,
        hasAppendSystemPrompt: false,
        agents: [],
        querySource,
        mcpTools: [],
        skipCacheWrite: true,
      },
    })
    if (response.isApiErrorMessage) {
      logForDebugging(`[buddy] ${querySource} API error: ${getAssistantMessageText(response)}`)
      return null
    }
    return getAssistantMessageText(response)?.trim() || null
  } catch (err) {
    if (!signal.aborted) logForDebugging(`[buddy] ${querySource} failed: ${err}`)
    return null
  }
}

export { createUserMessage }
