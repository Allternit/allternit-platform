import type { LocalCommandCall } from '../../types/command'
import { getLiveSession } from '../../services/liveSession'
import { openBrowser } from '../../utils/browser'

// Desktop's main process turns allternit://session?id=… into its open-session-link event.
export const call: LocalCommandCall = async () => {
  const { sessionID, connected } = getLiveSession()
  if (!sessionID || !connected) {
    return {
      type: 'text',
      value: 'This session is not live yet. Start Allternit Desktop (its engine runs on 127.0.0.1:4096), then run /desktop again.',
    }
  }
  const url = `allternit://session?id=${encodeURIComponent(sessionID)}`
  const opened = await openBrowser(url)
  return {
    type: 'text',
    value: opened ? `Opened this session in Allternit Desktop (${sessionID}).` : `Could not open Desktop. Open this link: ${url}`,
  }
}
