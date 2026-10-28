import { CLOUD_URLS } from '@/shared/constants/cloudUrls.js'
import { CloudContinue } from '@/runtime/session/cloud-continue.js'
import type { LocalCommandCall } from '../../types/command.js'
import { openBrowser } from '../../utils/browser.js'
import { CONTINUE_PROMPT, cloudOfferFor } from '../../utils/limitPause.js'
import { enqueuePendingNotification } from '../../utils/messageQueueManager.js'

/**
 * "Keep going on Allternit Cloud": the terminal twin of the button on the
 * web/Desktop limit strip. Only ever on the user's command. Re-reads the
 * offer (funds may have changed), switches the chat to the Cloud model and
 * continues; with no Cloud funds it opens billing instead. Nothing switches
 * back at the reset: /model picks the subscription model again.
 */
export const call: LocalCommandCall = async (_args, context) => {
  const state = context.getAppState()
  const pause = state.replPause
  if (!pause) return { type: 'text', value: 'Nothing is paused.' }
  if (pause.providerID === CloudContinue.PROVIDER_ID) return { type: 'text', value: 'This chat is already on Allternit Cloud.' }
  const offer = await cloudOfferFor(state.mainLoopModel)
  if (!offer) return { type: 'text', value: 'Sign in to Allternit Cloud to keep going on Cloud credits.' }
  if (offer.funds.kind === 'none') {
    const url = `${CLOUD_URLS.platform}/billing`
    const opened = await openBrowser(url).catch(() => false)
    context.setAppState(prev => (prev.replPause ? { ...prev, replPause: { ...prev.replPause, cloud: offer } } : prev))
    return { type: 'text', value: `No Cloud credits left. Add Cloud credits${opened ? ' in your browser' : ` at ${url}`}, then run /keep-going again.` }
  }
  context.setAppState(prev => ({
    ...prev,
    replPause: undefined,
    mainLoopModel: `${CloudContinue.PROVIDER_ID}/${offer.modelID}`,
    mainLoopModelForSession: null,
  }))
  enqueuePendingNotification({ value: CONTINUE_PROMPT, mode: 'prompt', priority: 'later' })
  return { type: 'text', value: `${CloudContinue.note(offer.label, pause.limit)} ${CloudContinue.fundsLabel(offer.funds)}.` }
}
