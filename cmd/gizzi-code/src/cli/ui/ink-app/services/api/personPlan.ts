/**
 * The person's own plans in the TUI: `chatgpt/…` (ChatGPT signed in on this
 * computer), `plan/<vendor>/<model>` and `copilot-plan/<model>` (cloud-api,
 * CONTRACT-connections-2026-11 §9.3, §9.5, §9.9).
 *
 * Chat only. A turn runs only from the composer: the REPL main thread of an
 * interactive session, not a sub-agent, not a side call (compaction, away
 * recap, memory extraction, classifier) and not print mode. Everything else
 * gets the plain refusal. Tools and the agent system prompt are never sent.
 */
import { randomUUID } from 'crypto'
import { personTurn } from '@/runtime/providers/person-plan/turn'
import { refusal } from '@/runtime/providers/person-plan/rules'
import type { ChatMessage } from '@/runtime/providers/person-plan/stream'
import { createAssistantAPIErrorMessage } from '../../utils/apiErrorMessage.js'
import type { AssistantMessage, Message, StreamEvent, SystemAPIErrorMessage } from '../../types/message.js'
import type { Options } from './claude.js'

/** The composer's own turn: REPL main thread, interactive, not a sub-agent. */
export function isComposerTurn(options: Pick<Options, 'querySource' | 'isNonInteractiveSession' | 'agentId'>): boolean {
  return (
    typeof options.querySource === 'string' &&
    options.querySource.startsWith('repl_main_thread') &&
    !options.isNonInteractiveSession &&
    !options.agentId
  )
}

function textOf(content: unknown): string {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content
    .filter((b: any) => b && b.type === 'text' && typeof b.text === 'string')
    .map((b: any) => b.text)
    .join('')
}

/** User and assistant text from the transcript; meta (reminders, attachments) and tool traffic left out. */
export function chatHistory(messages: Message[]): ChatMessage[] {
  const out: ChatMessage[] = []
  for (const m of messages as any[]) {
    if (m?.type !== 'user' && m?.type !== 'assistant') continue
    if (m.isMeta || m.isApiErrorMessage) continue
    const text = textOf(m.message?.content)
    if (!text.trim()) continue
    const role = m.type as 'user' | 'assistant'
    const last = out[out.length - 1]
    if (last && last.role === role) last.content += `\n\n${text}`
    else out.push({ role, content: text })
  }
  return out
}

function splitRef(model: string): { providerID: string; modelID: string } {
  const slash = model.indexOf('/')
  return { providerID: model.slice(0, slash), modelID: model.slice(slash + 1) }
}

export async function* queryPersonPlanWithStreaming({
  messages,
  signal,
  options,
}: {
  messages: Message[]
  signal: AbortSignal
  options: Options
}): AsyncGenerator<StreamEvent | AssistantMessage | SystemAPIErrorMessage, void> {
  if (!isComposerTurn(options)) {
    const kind = options.agentId ? 'subagent' : options.isNonInteractiveSession ? 'headless' : 'helper'
    yield createAssistantAPIErrorMessage({ content: refusal(options.model, kind), apiError: 'person_only', error: 'person_only' })
    return
  }
  const { providerID, modelID } = splitRef(options.model)
  const history = chatHistory(messages)
  if (!history.some(m => m.role === 'user')) {
    yield createAssistantAPIErrorMessage({ content: 'Nothing to send: the message has no text.', apiError: 'person_only', error: 'person_only' })
    return
  }
  const start = Date.now()
  let text = ''
  let started = false
  let usage = { input_tokens: 0, output_tokens: 0 }
  for await (const ev of personTurn({ providerID, modelID, messages: history, signal })) {
    if (ev.type === 'delta') {
      if (!started) {
        started = true
        yield { type: 'stream_event', event: { type: 'message_start', message: { usage } }, ttftMs: Date.now() - start } as unknown as StreamEvent
        yield { type: 'stream_event', event: { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } } } as unknown as StreamEvent
      }
      text += ev.text
      yield { type: 'stream_event', event: { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: ev.text } } } as unknown as StreamEvent
      continue
    }
    if (started) yield { type: 'stream_event', event: { type: 'content_block_stop', index: 0 } } as unknown as StreamEvent
    if (ev.type === 'error') {
      yield createAssistantAPIErrorMessage({ content: ev.message, apiError: 'person_plan_error', error: ev.code ?? 'person_plan_error' })
      return
    }
    usage = { input_tokens: ev.inputTokens ?? 0, output_tokens: ev.outputTokens ?? 0 }
    yield {
      type: 'assistant',
      message: {
        role: 'assistant',
        content: [{ type: 'text', text }],
        usage,
        stop_reason: ev.incomplete ? 'max_tokens' : 'end_turn',
      },
      model: options.model,
      usage,
      requestId: `person-plan-${Date.now()}`,
      uuid: randomUUID(),
      timestamp: new Date().toISOString(),
    } as unknown as AssistantMessage
    return
  }
}
