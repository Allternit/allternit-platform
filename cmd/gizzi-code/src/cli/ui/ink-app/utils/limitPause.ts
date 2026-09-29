/**
 * The REPL's side of landing before usage limits (spec P3.17). The runtime
 * pauses its own sessions (runtime/session/pause.ts); the REPL runs its own
 * query loop, so it reuses the same parsers and quota readers:
 *  - a turn that ran into a limit ("You've hit your session limit · resets
 *    7:40pm", a quota error) pauses the chat until the reset, then continues;
 *  - after each turn, a provider window at `limits.land_at` (95%) pauses
 *    before the next turn fails.
 * Continuing sooner on another model is only ever the user's choice
 * (`/resume-now`); `limits.fallback` "suggest" offers the model with the most
 * limit left. Anything that can't be read (no quota API, runtime config not
 * loaded) simply means no pause.
 */
import { SessionPause } from '@/runtime/session/pause.js'
import { SessionLimit } from '@/runtime/session/limit.js'
import { ProviderQuotas } from '@/runtime/providers/quota/index.js'
import type { Message } from '../types/message.js'

export type ReplPauseSuggestion = { providerID: string; modelID: string; label: string; headroom?: number }

export type ReplPause = {
  /** Epoch ms when the limit resets. */
  until: number
  /** e.g. "5-hour limit", "Kimi For Coding weekly limit". */
  limit: string
  providerID?: string
  /** limit_hit: a turn was cut off and continues at the reset; quota: landed before the limit. */
  reason: 'limit_hit' | 'quota'
  suggest?: ReplPauseSuggestion
}

/** "openrouter/z-ai/glm-4.7-flash" → "openrouter"; bare model names have no provider. */
export function providerOfModel(model: string | null | undefined): string | undefined {
  const i = model?.indexOf('/') ?? -1
  return i > 0 ? model!.slice(0, i) : undefined
}

function textOf(message: Message): string {
  const content = (message as { message?: { content?: unknown } }).message?.content
  if (typeof content === 'string') return content
  if (Array.isArray(content)) {
    return content
      .map(block => (block && typeof block === 'object' && 'text' in block ? String((block as { text: unknown }).text ?? '') : ''))
      .join(' ')
  }
  return ''
}

/** A limit this turn ran into: its last assistant reply or API error states when it resets. */
export function limitInTurn(turnMessages: readonly Message[], now = Date.now()): { until: number; limit: string } | undefined {
  for (let i = turnMessages.length - 1; i >= 0; i--) {
    const m = turnMessages[i]!
    if (m.type !== 'assistant') continue
    const text = textOf(m).trim()
    if (!text || text.length > 600) return undefined
    return SessionPause.limitFromText(text, now)
  }
  return undefined
}

/** The provider's window is at `limits.land_at`: pause before the next turn fails. */
export async function quotaLandingFor(providerID: string | undefined) {
  if (!providerID) return undefined
  try {
    return await SessionPause.quotaLanding(providerID)
  } catch {
    return undefined
  }
}

/** The model with the most limit left, for "Resume now on …" (never switched to automatically). */
export async function suggestionFor(providerID: string | undefined): Promise<ReplPauseSuggestion | undefined> {
  try {
    const s = await SessionPause.suggestAlternative(providerID)
    return s ? { providerID: s.providerID, modelID: s.modelID, label: s.label, headroom: s.headroom ?? undefined } : undefined
  } catch {
    return undefined
  }
}

/** Resume a little after the stated reset, so the provider has rolled over (same as the runtime). */
export const RESUME_GRACE_MS = 60_000

export const CONTINUE_PROMPT = 'Continue where you left off.'

/** A provider window past `limits.warn_at` (80%): the "Approaching usage limit" line. */
export type ReplLimitWarning = {
  providerID: string
  /** e.g. "Kimi For Coding 5-hour limit". */
  limit: string
  usedRatio: number
  /** Epoch ms when the window resets, when the provider says. */
  resetAt?: number
}

/** The warning for this provider's tightest window, or undefined when under warn_at (or unreadable). */
export async function approachingFor(providerID: string | undefined, modelID?: string): Promise<ReplLimitWarning | undefined> {
  if (!providerID) return undefined
  try {
    const result = await ProviderQuotas.get(providerID)
    if (result.status !== 'ok') return undefined
    const reading = SessionLimit.classify(result.quota.windows, await SessionLimit.thresholds(), modelID)
    if (reading.state !== 'approaching') return undefined
    const w = reading.window
    const limit = SessionLimit.snapshot('approaching', providerID, w, result.quota.source).label
    const resetAt = w.resetAt ? Date.parse(w.resetAt) : undefined
    return { providerID, limit, usedRatio: w.usedRatio, ...(resetAt ? { resetAt } : {}) }
  } catch {
    return undefined
  }
}
