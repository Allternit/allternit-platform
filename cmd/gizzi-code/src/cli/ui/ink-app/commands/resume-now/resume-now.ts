import type { LocalCommandCall } from '../../types/command.js'
import { CONTINUE_PROMPT } from '../../utils/limitPause.js'
import { enqueuePendingNotification } from '../../utils/messageQueueManager.js'

/**
 * "Resume now on …" (P3.17): the only way the chat moves to another model
 * while paused — always the user's choice. Switches to the suggested model
 * (the one with the most limit left) when there is one; otherwise continues
 * on the same model now, which may hit the limit again.
 */
export const call: LocalCommandCall = async (_args, context) => {
  const pause = context.getAppState().replPause
  if (!pause) return { type: 'text', value: 'Nothing is paused.' }
  const s = pause.suggest
  context.setAppState(prev => ({
    ...prev,
    replPause: undefined,
    ...(s ? { mainLoopModel: `${s.providerID}/${s.modelID}`, mainLoopModelForSession: null } : {}),
  }))
  if (pause.reason === 'limit_hit') enqueuePendingNotification({ value: CONTINUE_PROMPT, mode: 'prompt', priority: 'later' })
  return {
    type: 'text',
    value: s
      ? `Continuing on ${s.label}.`
      : `Continuing now on the same model. The ${pause.limit} may still be in effect.`,
  }
}
