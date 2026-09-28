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
  // The provider layer can keep retrying an unreachable model long after the
  // signal fires, so stop waiting on abort ourselves.
  const aborted = new Promise<null>(resolve => {
    if (signal.aborted) resolve(null)
    else signal.addEventListener('abort', () => resolve(null), { once: true })
  })
  try {
    const request = queryModelWithoutStreaming({
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
    request.catch(() => {})
    const response = await Promise.race([request, aborted])
    if (!response) {
      logForDebugging(`[pet] ${querySource} aborted`)
      return null
    }
    if (response.isApiErrorMessage) {
      logForDebugging(`[pet] ${querySource} API error: ${getAssistantMessageText(response)}`)
      return null
    }
    return getAssistantMessageText(response)?.trim() || null
  } catch (err) {
    if (!signal.aborted) logForDebugging(`[pet] ${querySource} failed: ${err}`)
    return null
  }
}

export { createUserMessage }
