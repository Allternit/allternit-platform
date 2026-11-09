/**
 * Per-turn signal extraction over the ink-app message list, kept out of
 * REPL.tsx so the assembly points stay one-liners. Everything here is
 * defensive: missing fields mean "not reported", never a guess.
 */
import type { Message } from '../../types/message.js'
import { getCurrentUsage, getTokenUsage } from '../tokens.js'

/**
 * True when any assistant message appended during this turn (from index
 * `fromIndex` on) carries the runtime's estimated-usage marker
 * (`usageEstimated` / `tokensEstimated` — see
 * src/runtime/session/context-event.ts and message-v2.ts). Providers that
 * report real usage never set it.
 */
export function turnUsageEstimated(
  messages: Message[],
  fromIndex: number,
): boolean {
  for (let i = Math.max(0, fromIndex); i < messages.length; i++) {
    const m = messages[i]
    if (!m || m.type !== 'assistant') continue
    const nested = m.message as {
      usageEstimated?: boolean
      tokensEstimated?: boolean
    }
    if (
      nested?.usageEstimated === true ||
      nested?.tokensEstimated === true ||
      (m as { usageEstimated?: boolean }).usageEstimated === true
    ) {
      return true
    }
  }
  return false
}

/**
 * 0–1 share of the context window in use, from the last usage-bearing
 * message (same basis as the footer and /usage: input + cache write +
 * cache read over the model's window). Null when nothing was reported.
 */
export function contextRatioFromMessages(
  messages: Message[],
  contextWindow: number,
): number | null {
  if (!Number.isFinite(contextWindow) || contextWindow <= 0) return null
  const usage = getCurrentUsage(messages)
  if (!usage) return null
  const used =
    usage.input_tokens +
    usage.cache_creation_input_tokens +
    usage.cache_read_input_tokens
  if (used <= 0) return null
  return Math.min(1, used / contextWindow)
}

/**
 * Reasoning tokens on the most recent usage-bearing message, when the
 * provider reports them (`reasoning_tokens` / `reasoning_output_tokens`).
 * Zero when absent.
 */
export function reasoningTokensFromMessages(messages: Message[]): number {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]
    const usage = m ? getTokenUsage(m) : undefined
    if (!usage) continue
    const extra = usage as typeof usage & {
      reasoning_tokens?: number
      reasoning_output_tokens?: number
    }
    const n = extra.reasoning_tokens ?? extra.reasoning_output_tokens ?? 0
    return typeof n === 'number' && Number.isFinite(n) ? n : 0
  }
  return 0
}

export type TurnFileChange = { path: string; added: number; removed: number }

/**
 * How the turn ended, for the closing line: failed (the last assistant
 * message is an API error), empty (no text and no tool call), and the files
 * changed by Edit/Write results (path + lines added/removed).
 */
export function turnOutcome(
  messages: Message[],
  fromIndex: number,
  cwd: string = process.cwd(),
): { failed: boolean; empty: boolean; cause?: string; files: TurnFileChange[]; tools: number } {
  let lastAssistant: Message | undefined
  let produced = false
  let tools = 0
  const files: TurnFileChange[] = []
  for (let i = Math.max(0, fromIndex); i < messages.length; i++) {
    const m = messages[i] as any
    if (!m) continue
    if (m.type === 'assistant') {
      lastAssistant = m
      if (m.isApiErrorMessage) continue
      const content = Array.isArray(m.message?.content) ? m.message.content : []
      if (content.some((b: any) => (b?.type === 'text' && String(b.text ?? '').trim()) || b?.type === 'tool_use')) produced = true
      tools += content.filter((b: any) => b?.type === 'tool_use').length
      continue
    }
    const r = m.type === 'user' ? m.toolUseResult : undefined
    if (!r || typeof r !== 'object' || typeof r.filePath !== 'string') continue
    const patch = Array.isArray(r.structuredPatch) ? r.structuredPatch : null
    if (!patch) continue
    let added = 0
    let removed = 0
    for (const h of patch) for (const l of h?.lines ?? []) {
      if (typeof l !== 'string') continue
      if (l.startsWith('+')) added++
      else if (l.startsWith('-')) removed++
    }
    if (patch.length === 0 && typeof r.content === 'string') added = r.content.replace(/\n$/, '').split('\n').length
    const rel = r.filePath.startsWith(cwd + '/') ? r.filePath.slice(cwd.length + 1) : r.filePath
    files.push({ path: rel, added, removed })
  }
  const failed = (lastAssistant as any)?.isApiErrorMessage === true
  let cause: string | undefined
  if (failed) {
    const content = (lastAssistant as any).message?.content
    cause = Array.isArray(content) ? content.find((b: any) => b?.type === 'text')?.text : undefined
  }
  return { failed, empty: !failed && !produced, cause, files, tools }
}
