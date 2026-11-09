// @ts-nocheck
import { feature } from 'bun:bundle'
import { useEffect, useRef } from 'react'
import {
  getTerminalFocusState,
  subscribeTerminalFocus,
} from '../ink/terminal-focus-state.js'
import { getFeatureValue_CACHED_MAY_BE_STALE } from '../services/analytics/growthbook.js'
import {
  generateAwaySummary,
  getRecapFileChanges,
} from '../services/awaySummary.js'
import { useAppStateStore, useSetAppState } from '../state/AppState.js'
import type { Message } from '../types/message.js'
import { createAwaySummaryMessage } from '../utils/messages.js'

// GIZZI_AWAY_SUMMARY_DELAY_MS shortens the wait for testing.
const BLUR_DELAY_MS =
  Number(process.env.GIZZI_AWAY_SUMMARY_DELAY_MS) > 0
    ? Number(process.env.GIZZI_AWAY_SUMMARY_DELAY_MS)
    : 5 * 60_000

type SetMessages = (updater: (prev: Message[]) => Message[]) => void

function hasSummarySinceLastUserTurn(messages: readonly Message[]): boolean {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!
    if (m.type === 'user' && !m.isMeta && !m.isCompactSummary) return false
    if (m.type === 'system' && m.subtype === 'away_summary') return true
  }
  return false
}

/**
 * Appends a "while you were away" summary message after the terminal has been
 * blurred for 5 minutes. Fires only when (a) 5min since blur, (b) no turn in
 * progress, and (c) no existing away_summary since the last user message.
 *
 * Focus state 'unknown' (terminal doesn't support DECSET 1004) is a no-op.
 */
export function useAwaySummary(
  messages: readonly Message[],
  setMessages: SetMessages,
  isLoading: boolean,
  sessionModel?: string,
): void {
  const setAppState = useSetAppState()
  const store = useAppStateStore()
  const sessionModelRef = useRef(sessionModel)
  sessionModelRef.current = sessionModel
  const blurredAtRef = useRef<number | null>(null)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const abortRef = useRef<AbortController | null>(null)
  const messagesRef = useRef(messages)
  const isLoadingRef = useRef(isLoading)
  const pendingRef = useRef(false)
  const generateRef = useRef<(() => Promise<void>) | null>(null)

  messagesRef.current = messages
  isLoadingRef.current = isLoading

  // 3P default: false
  const gbEnabled = getFeatureValue_CACHED_MAY_BE_STALE(
    'tengu_sedge_lantern',
    false,
  )

  useEffect(() => {
    if (!feature('AWAY_SUMMARY')) return
    if (!gbEnabled) return

    function clearTimer(): void {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current)
        timerRef.current = null
      }
    }

    function abortInFlight(): void {
      abortRef.current?.abort()
      abortRef.current = null
    }

    async function generate(): Promise<void> {
      pendingRef.current = false
      if (hasSummarySinceLastUserTurn(messagesRef.current)) return
      abortInFlight()
      const controller = new AbortController()
      abortRef.current = controller
      const messagesNow = messagesRef.current
      const text = await generateAwaySummary(
        messagesNow,
        controller.signal,
        sessionModelRef.current,
      )
      if (controller.signal.aborted || text === null) return
      // Files changed since the previous recap, counted from the edit tool
      // results (not by the model).
      let since = 0
      for (let i = messagesNow.length - 1; i >= 0; i--) {
        const m = messagesNow[i]!
        if (m.type === 'system' && m.subtype === 'away_summary') {
          since = i + 1
          break
        }
      }
      const files = getRecapFileChanges(messagesNow.slice(since))
      const awayMs =
        blurredAtRef.current !== null ? Date.now() - blurredAtRef.current : undefined
      setMessages(prev => [
        ...prev,
        createAwaySummaryMessage(text, { files, awayMs }),
      ])
      // The recap's "Next:" line becomes the ghost suggestion in the empty
      // prompt (tab to accept), when suggestions are on.
      const next = /^Next:\s*(.+)$/im.exec(text)?.[1]?.trim()
      if (next && store.getState().promptSuggestionEnabled) {
        setAppState(prev => ({
          ...prev,
          promptSuggestion: {
            text: next,
            promptId: 'user_intent',
            shownAt: 0,
            acceptedAt: 0,
            generationRequestId: null,
          },
        }))
      }
    }

    function onBlurTimerFire(): void {
      timerRef.current = null
      if (isLoadingRef.current) {
        pendingRef.current = true
        return
      }
      void generate()
    }

    function onFocusChange(): void {
      const state = getTerminalFocusState()
      if (state === 'blurred') {
        if (blurredAtRef.current === null) blurredAtRef.current = Date.now()
        clearTimer()
        timerRef.current = setTimeout(onBlurTimerFire, BLUR_DELAY_MS)
      } else if (state === 'focused') {
        blurredAtRef.current = null
        clearTimer()
        abortInFlight()
        pendingRef.current = false
      }
      // 'unknown' → no-op
    }

    const unsubscribe = subscribeTerminalFocus(onFocusChange)
    // Handle the case where we're already blurred when the effect mounts
    onFocusChange()
    generateRef.current = generate

    return () => {
      unsubscribe()
      clearTimer()
      abortInFlight()
      generateRef.current = null
    }
  }, [gbEnabled, setMessages, setAppState, store])

  // Timer fired mid-turn → fire when turn ends (if still blurred)
  useEffect(() => {
    if (isLoading) return
    if (!pendingRef.current) return
    if (getTerminalFocusState() !== 'blurred') return
    void generateRef.current?.()
  }, [isLoading])
}
