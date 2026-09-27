import { feature } from 'bun:bundle'
import type { Message } from '../types/message.js'
import { getGlobalConfig } from '../utils/config.js'
import { getAssistantMessageText } from '../utils/messages.js'
import { getCompanion } from './companion.js'
import { logForDebugging } from '../utils/debug.js'
import { askCompanionModel, createUserMessage } from './soul.js'

// Speak after roughly one turn in four unless addressed by name, and never
// more than once a minute — a quip every turn gets tuned out (and costs a
// model call per turn).
const SPEAK_CHANCE = 0.25
const MIN_GAP_MS = 60_000
const MAX_QUIP_CHARS = 80
// Generous: reasoning-heavy small models can take tens of seconds.
const TIMEOUT_MS = 30_000

let lastSpokeAt = 0
let inFlight: AbortController | null = null

function lastText(messages: readonly Message[], type: 'user' | 'assistant'): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!
    if (m.type !== type || m.isMeta) continue
    const content = m.message?.content
    const text =
      type === 'assistant'
        ? getAssistantMessageText(m)
        : typeof content === 'string'
          ? content
          : Array.isArray(content)
            ? content
                .filter((b: { type: string }) => b.type === 'text')
                .map((b: { text: string }) => b.text)
                .join(' ')
            : ''
    if (text?.trim()) return text.trim()
  }
  return ''
}

/**
 * After each completed turn, maybe let the companion react in its speech
 * bubble. Fire-and-forget: errors and timeouts resolve to silence.
 */
export async function fireCompanionObserver(
  messages: readonly Message[],
  onReaction: (reaction: string) => void,
): Promise<void> {
  if (!feature('BUDDY')) return
  const companion = getCompanion()
  if (!companion || getGlobalConfig().companionMuted) return

  const userText = lastText(messages, 'user')
  const addressed = new RegExp(`\\b${companion.name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\b`, 'i').test(userText)
  const now = Date.now()
  if (!addressed && (now - lastSpokeAt < MIN_GAP_MS || Math.random() > SPEAK_CHANCE)) return

  inFlight?.abort()
  const controller = new AbortController()
  inFlight = controller
  const timer = setTimeout(() => controller.abort(), TIMEOUT_MS)
  try {
    const assistantText = lastText(messages, 'assistant').slice(-1500)
    const system = `You are ${companion.name}, a tiny ${companion.species} companion who sits beside a developer's terminal and watches them work with a coding agent. Personality: ${companion.personality}
Reply with ONE short in-character remark (under ${MAX_QUIP_CHARS} characters), no quotes, no emoji, no hashtags. ${addressed ? 'The user just spoke to you by name — answer them.' : 'React to what just happened.'} Never give instructions or code.`
    const reply = await askCompanionModel(
      [
        createUserMessage({
          content: `User said: ${userText.slice(-600) || '(nothing)'}\nAgent replied: ${assistantText || '(tool work only)'}`,
        }),
      ],
      system,
      controller.signal,
      'companion_observer',
    )
    if (controller.signal.aborted) {
      logForDebugging('[buddy] observer timed out')
      return
    }
    if (!reply) return
    const quip = reply.replace(/^["'“]|["'”]$/g, '').split('\n')[0]!.slice(0, MAX_QUIP_CHARS)
    if (!quip) return
    lastSpokeAt = Date.now()
    onReaction(quip)
  } finally {
    clearTimeout(timer)
    if (inFlight === controller) inFlight = null
  }
}
