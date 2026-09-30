/**
 * The thought stream for a subscription turn: what the Sessions computer is
 * doing while the provider works, as reasoning parts, so the chat shows
 * live activity instead of a silent wait. It closes when the reply text
 * starts (or the turn ends) and never reopens in the same turn.
 *
 * Every line states something the gateway reported; nothing here guesses
 * at what the provider is thinking.
 */

import type { LanguageModelV2StreamPart } from "@ai-sdk/provider"

const REASONING_ID = "fabric-thoughts"
/** The SDK's own reply-growth labels ("streaming (+12 chars)") — the reply text already shows that. */
const STREAMING_LABEL = /^streaming \(/i
/** First "still working" line after this many seconds, then at most one per interval. */
const STILL_WORKING_AFTER_S = 30

export interface ThoughtStream {
  sending(): void
  status(status: string): void
  submitted(): void
  progress(label: string): void
  heartbeat(elapsedS: number): void
  /** Close the thought stream (reply text is starting, or the turn ended). */
  end(): void
}

export function createThoughtStream(
  enqueue: (part: LanguageModelV2StreamPart) => void,
  providerName: string,
): ThoughtStream {
  let state: "idle" | "open" | "closed" = "idle"
  let last = ""
  const seen = new Set<string>()
  let nextStillWorkingS = STILL_WORKING_AFTER_S

  const line = (text: string) => {
    if (state === "closed" || !text || text === last) return
    if (state === "idle") {
      state = "open"
      enqueue({ type: "reasoning-start", id: REASONING_ID })
    }
    last = text
    enqueue({ type: "reasoning-delta", id: REASONING_ID, delta: `${text}\n` })
  }

  return {
    sending: () => line(`Sending your message to ${providerName} on your Sessions computer`),
    status: (status) => {
      if (status === "queued") line(`Waiting for your ${providerName} account to be free`)
      else if (status === "running") line(`Opening ${providerName}`)
    },
    submitted: () => line(`${providerName} has your message and is working on it`),
    progress: (label) => {
      const clean = label.trim()
      if (!clean || STREAMING_LABEL.test(clean) || seen.has(clean)) return
      seen.add(clean)
      line(clean)
    },
    heartbeat: (elapsedS) => {
      if (elapsedS < nextStillWorkingS) return
      nextStillWorkingS = elapsedS + STILL_WORKING_AFTER_S
      line(`Still working (${Math.round(elapsedS)} s)`)
    },
    end: () => {
      if (state === "open") enqueue({ type: "reasoning-end", id: REASONING_ID })
      state = "closed"
    },
  }
}
