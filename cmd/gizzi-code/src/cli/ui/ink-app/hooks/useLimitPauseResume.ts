import { useEffect } from 'react'
import { useAppState, useSetAppState } from '../state/AppState.js'
import { CONTINUE_PROMPT, RESUME_GRACE_MS } from '../utils/limitPause.js'
import { enqueuePendingNotification } from '../utils/messageQueueManager.js'

const MAX_TIMER_MS = 2_147_483_647

/**
 * At the reset (plus a minute of grace), clear the pause; a turn that the
 * limit cut off continues on its own ("Continue where you left off."),
 * queued like a scheduled prompt so it runs between turns.
 */
export function useLimitPauseResume(): void {
  const pause = useAppState(s => s.replPause)
  const setAppState = useSetAppState()
  useEffect(() => {
    if (!pause) return
    const wait = Math.min(MAX_TIMER_MS, Math.max(0, pause.until + RESUME_GRACE_MS - Date.now()))
    const timer = setTimeout(() => {
      setAppState(prev => (prev.replPause === pause ? { ...prev, replPause: undefined } : prev))
      if (pause.reason === 'limit_hit') enqueuePendingNotification({ value: CONTINUE_PROMPT, mode: 'prompt', priority: 'later' })
    }, wait)
    return () => clearTimeout(timer)
  }, [pause, setAppState])
}
