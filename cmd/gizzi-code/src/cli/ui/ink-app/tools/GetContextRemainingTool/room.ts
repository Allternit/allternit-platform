// @ts-nocheck
/**
 * Context room in the terminal (Primitives Wave B, lane L6), shared by the
 * get_context_remaining tool and the context notice (utils/attachments.ts).
 *
 * Uses the same numbers as Ink's own auto-compact and its "% until
 * auto-compact" warning (services/compact/autoCompact.ts): tokens in use are
 * tokenCountWithEstimation (the last API usage plus anything added since),
 * and the room before compaction is the auto-compact threshold, or the
 * effective window when auto-compact is off.
 */
import { getSdkBetas } from '../../bootstrap/state.js'
import {
  getAutoCompactThreshold,
  getEffectiveContextWindowSize,
  isAutoCompactEnabled,
} from '../../services/compact/autoCompact.js'
import type { Message } from '../../types/message.js'
import { getContextWindowForModel } from '../../utils/context.js'
import { tokenCountWithEstimation } from '../../utils/tokens.js'

export type ContextRoom = {
  window: number
  used: number
  remaining: number
  until_compaction: number
  /** The room before compaction (the threshold the notice measures against). */
  budget: number
}

export function contextRoom(messages: Message[], model: string): ContextRoom {
  const window = Math.max(0, getContextWindowForModel(model, getSdkBetas()))
  const used = Math.max(0, Math.round(tokenCountWithEstimation(messages)))
  const budget = Math.max(
    1,
    isAutoCompactEnabled() ? getAutoCompactThreshold(model) : getEffectiveContextWindowSize(model),
  )
  return {
    window,
    used,
    remaining: Math.max(0, window - used),
    until_compaction: Math.max(0, budget - used),
    budget,
  }
}
