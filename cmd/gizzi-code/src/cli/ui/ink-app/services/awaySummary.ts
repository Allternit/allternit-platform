// @ts-nocheck
import { APIUserAbortError } from '@allternit/gizzi-sdk/providers/allternit'
import { getEmptyToolPermissionContext } from '../Tool.js'
import type { Message } from '../types/message.js'
import { logForDebugging } from '../utils/debug.js'
import {
  createUserMessage,
  getAssistantMessageText,
} from '../utils/messages.js'
import {
  getDefaultHaikuModel,
  getSmallFastModel,
} from '../utils/model/model.js'
import { asSystemPrompt } from '../utils/systemPromptType.js'
import { queryModelWithoutStreaming } from './api/claude.js'
import { getSessionMemoryContent } from './SessionMemory/sessionMemoryUtils.js'

// Recap only needs recent context — truncate to avoid "prompt too long" on
// large sessions. 30 messages ≈ ~15 exchanges, plenty for "where we left off."
const RECENT_MESSAGE_WINDOW = 30

function buildAwaySummaryPrompt(memory: string | null): string {
  const memoryBlock = memory
    ? `Session memory (broader context):\n${memory}\n\n`
    : ''
  return `${memoryBlock}The user stepped away and is coming back. Write a recap in exactly this plain-text format, no markdown, no other lines:
<one sentence: the high-level task (what they are building or debugging, not implementation details) and where it stands>
Failing: <tests or checks that still fail at the end of the transcript, comma-separated>
Next: <the concrete next step, written as the user would type it to you, 2-12 words>
Leave out the "Failing:" line when nothing is failing or nothing was run. Do not list changed files; they are shown separately.`
}

export type RecapFileChange = { path: string; added: number; removed: number }

/**
 * Files edited or written in `messages`, with +/- line counts, read from the
 * edit/write tool results (no model call).
 */
export function getRecapFileChanges(
  messages: readonly Message[],
): RecapFileChange[] {
  const byPath = new Map<string, RecapFileChange>()
  for (const m of messages) {
    if (m.type !== 'user') continue
    const r = (m as { toolUseResult?: unknown }).toolUseResult as
      | {
          filePath?: unknown
          type?: unknown
          content?: unknown
          structuredPatch?: Array<{ lines: string[] }>
        }
      | undefined
    if (!r || typeof r.filePath !== 'string') continue
    let added = 0
    let removed = 0
    if (Array.isArray(r.structuredPatch) && r.structuredPatch.length > 0) {
      for (const hunk of r.structuredPatch) {
        for (const line of hunk.lines ?? []) {
          if (line.startsWith('+')) added++
          else if (line.startsWith('-')) removed++
        }
      }
    } else if (r.type === 'create' && typeof r.content === 'string') {
      const lines = r.content.split('\n')
      added = lines.at(-1) === '' ? lines.length - 1 : lines.length
    } else {
      continue
    }
    const prev = byPath.get(r.filePath) ?? { path: r.filePath, added: 0, removed: 0 }
    prev.added += added
    prev.removed += removed
    byPath.set(r.filePath, prev)
  }
  return [...byPath.values()]
}

/**
 * Model for the recap: an explicit small model (env / gizzi.json small_model)
 * when one is set; otherwise the session's own model, so a CLI-brain or
 * Allternit Cloud session never falls back to Haiku on an Anthropic key it
 * may not have.
 */
function getRecapModel(sessionModel: string | undefined): string {
  const small = getSmallFastModel()
  if (sessionModel && sessionModel.includes('/') && small === getDefaultHaikuModel()) {
    return sessionModel
  }
  return small
}

/**
 * Generates a short session recap for the "while you were away" card.
 * Returns null on abort, empty transcript, or error.
 */
export async function generateAwaySummary(
  messages: readonly Message[],
  signal: AbortSignal,
  sessionModel?: string,
): Promise<string | null> {
  if (messages.length === 0) {
    return null
  }

  try {
    const memory = await getSessionMemoryContent()
    const recent = messages.slice(-RECENT_MESSAGE_WINDOW)
    recent.push(createUserMessage({ content: buildAwaySummaryPrompt(memory) }))
    const response = await queryModelWithoutStreaming({
      messages: recent,
      systemPrompt: asSystemPrompt([]),
      thinkingConfig: { type: 'disabled' },
      tools: [],
      signal,
      options: {
        getToolPermissionContext: async () => getEmptyToolPermissionContext(),
        model: getRecapModel(sessionModel),
        toolChoice: undefined,
        isNonInteractiveSession: false,
        hasAppendSystemPrompt: false,
        agents: [],
        querySource: 'away_summary',
        mcpTools: [],
        skipCacheWrite: true,
      },
    })

    if (response.isApiErrorMessage) {
      logForDebugging(
        `[awaySummary] API error: ${getAssistantMessageText(response)}`,
      )
      return null
    }
    return getAssistantMessageText(response)
  } catch (err) {
    if (err instanceof APIUserAbortError || signal.aborted) {
      return null
    }
    logForDebugging(`[awaySummary] generation failed: ${err}`)
    return null
  }
}
