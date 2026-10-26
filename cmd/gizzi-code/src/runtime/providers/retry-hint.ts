/**
 * Client-side consumption of the Allternit LLM gateway's `allternit.retry_hint`
 * SSE event (gateway PR #728, `cmd/allternit-api/src/llm_gateway/failover.rs`).
 *
 * Wire contract: on a mid-flight upstream streaming failure the gateway emits,
 * before the terminal error frame and `[DONE]`:
 *
 *   event: allternit.retry_hint
 *   data: {"retryable":true,"reason":"rate_limit_error","next_fallback":{"provider_id":"...","model_id":"..."}}
 *
 * Terminal failures send `{"retryable":false,"reason":"...","next_fallback":null}`.
 *
 * This module does two things:
 *
 * 1. `tapRetryHint` wraps a streaming chat-completions `Response` body with an
 *    SSE-frame-aware pass-through that strips `allternit.retry_hint` frames
 *    (the AI SDK's chunk parser must never see them) and records the parsed
 *    hint against the gizzi session that issued the request.
 * 2. `consumeRetryHint` lets the session processor take the hint exactly once
 *    and decide whether to re-drive the request. Combined with the
 *    processor-local "already consumed a hint" flag, this bounds the re-drive
 *    to a single retry per hint and prevents retry loops.
 */

import { Log } from "@/shared/util/log"
import { frameHandoff, headerHandoff, HANDOFF_HEADER, recordHandoff, type Handoff } from "./handoff"

const log = Log.create({ service: "provider.retry-hint" })

export const RETRY_HINT_EVENT = "allternit.retry_hint"

export interface RetryHintFallback {
  provider_id: string
  model_id: string
}

export interface RetryHint {
  retryable: boolean
  reason: string
  next_fallback: RetryHintFallback | null
}

export function parseRetryHint(data: string): RetryHint | null {
  try {
    const raw = JSON.parse(data) as Partial<RetryHint> | null
    if (!raw || typeof raw !== "object") return null
    if (typeof raw.retryable !== "boolean") return null
    const fallback = raw.next_fallback
    return {
      retryable: raw.retryable,
      reason: typeof raw.reason === "string" ? raw.reason : "unknown",
      next_fallback:
        fallback &&
        typeof fallback === "object" &&
        typeof fallback.provider_id === "string" &&
        typeof fallback.model_id === "string"
          ? { provider_id: fallback.provider_id, model_id: fallback.model_id }
          : null,
    }
  } catch {
    return null
  }
}

// Per-session store. A hint is consumed at most once (`consume` deletes), so a
// recorded hint can never trigger more than one re-drive on its own.
const hints = new Map<string, RetryHint>()

export function recordRetryHint(sessionID: string, hint: RetryHint): void {
  hints.set(sessionID, hint)
}

export function consumeRetryHint(sessionID: string): RetryHint | undefined {
  const hint = hints.get(sessionID)
  hints.delete(sessionID)
  return hint
}

export function clearRetryHints(): void {
  hints.clear()
}

function frameHint(frame: string): RetryHint | null {
  let isHint = false
  const data: string[] = []
  for (const line of frame.split(/\r?\n/)) {
    if (line === `event: ${RETRY_HINT_EVENT}` || line === `event:${RETRY_HINT_EVENT}`) {
      isHint = true
    } else if (line.startsWith("data:")) {
      data.push(line.slice(5).replace(/^ /, ""))
    }
  }
  if (!isHint || data.length === 0) return null
  return parseRetryHint(data.join("\n"))
}

/**
 * Pass-through byte transform that strips `allternit.retry_hint` SSE frames
 * and reports each one to `onHint`. All other frames (including their blank-line
 * terminators and CRLF style) are forwarded byte-identical, flushed as soon as
 * a frame boundary arrives so streaming latency is unaffected.
 */
export function createRetryHintStream(
  onHint: (hint: RetryHint) => void,
  // Allternit Cloud's open-lane handoff chunk (see handoff.ts) is stripped
  // and reported the same way.
  onHandoff?: (handoff: Handoff) => void,
): TransformStream<Uint8Array, Uint8Array> {
  const decoder = new TextDecoder()
  const encoder = new TextEncoder()
  let pending = ""
  // A hint or handoff frame is consumed; true when it was.
  const consumed = (frame: string) => {
    const hint = frameHint(frame)
    if (hint) {
      onHint(hint)
      return true
    }
    const handoff = onHandoff ? frameHandoff(frame) : null
    if (handoff) {
      onHandoff!(handoff)
      return true
    }
    return false
  }
  const flushFrames = (controller: TransformStreamDefaultController<Uint8Array>, final: boolean) => {
    let boundary = pending.search(/\r?\n\r?\n/)
    while (boundary !== -1) {
      const frame = pending.slice(0, boundary)
      const rest = pending.slice(boundary)
      const terminator = rest.match(/^\r?\n\r?\n/)![0]
      pending = rest.slice(terminator.length)
      if (!consumed(frame)) controller.enqueue(encoder.encode(frame + terminator))
      boundary = pending.search(/\r?\n\r?\n/)
    }
    if (final && pending.length > 0) {
      if (!consumed(pending)) controller.enqueue(encoder.encode(pending))
      pending = ""
    }
  }
  return new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      pending += decoder.decode(chunk, { stream: true })
      flushFrames(controller, false)
    },
    flush(controller) {
      pending += decoder.decode()
      flushFrames(controller, true)
    },
  })
}

export type RetryHintTapArgs = {
  response: Response
  method: string | undefined
  url: string
  sessionID: string | null
  npm: string
}

/**
 * Decide whether this response is a gateway chat-completions SSE stream worth
 * tapping, and if so return a Response whose body strips + records retry hints.
 * Returns the original response untouched otherwise. Scoped to
 * OpenAI-compatible providers (the gateway's surface); opt out with
 * GIZZI_DISABLE_RETRY_HINT=1.
 */
export function tapRetryHint(args: RetryHintTapArgs): Response {
  if (process.env.GIZZI_DISABLE_RETRY_HINT) return args.response
  if (!args.npm.includes("@ai-sdk/openai-compatible")) return args.response
  if ((args.method ?? "GET").toUpperCase() !== "POST") return args.response
  if (!args.url.includes("/chat/completions")) return args.response
  if (!args.sessionID) return args.response
  if (!args.response.ok || !args.response.body) return args.response
  const sessionID = args.sessionID
  const onHandoff = (handoff: Handoff) => {
    log.info("open-lane handoff", { sessionID, from: handoff.from, to: handoff.to })
    recordHandoff(sessionID, handoff)
  }
  // A non-streamed reply names its handoff in a header.
  const headed = headerHandoff(args.response.headers.get(HANDOFF_HEADER))
  if (headed) onHandoff(headed)
  const contentType = args.response.headers.get("content-type") ?? ""
  if (!contentType.includes("text/event-stream")) return args.response
  const tapped = args.response.body.pipeThrough(
    createRetryHintStream((hint) => {
      log.info("gateway retry hint", { sessionID, retryable: hint.retryable, reason: hint.reason })
      recordRetryHint(sessionID, hint)
    }, onHandoff),
  )
  return new Response(tapped, args.response)
}
