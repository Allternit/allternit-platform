/**
 * Versioned runtime guardrail defaults (decisions O5, O8, O12; Q15).
 *
 * This is the ONE place these numbers live. Every consumer (the turn loop,
 * the processor, the provider transform, usage metering) imports from here so
 * the values can be surfaced later (settings, the kernel ExecutionPlan, the
 * Usage view) without hunting through call sites. Bump
 * GUARDRAIL_DEFAULTS_VERSION whenever any value below changes.
 *
 * Deliberately import-free so the provider transform can depend on it
 * without pulling session code into the provider layer.
 */

export const GUARDRAIL_DEFAULTS_VERSION = "2026-10-01.1"

// ─── O12: per-turn agent guardrails ──────────────────────────────────────────

/**
 * Which default row applies to an agent:
 * - primary: user-facing agents (mode "primary" or "all", not hidden)
 * - subagent: agents spawned through the task tool (mode "subagent")
 * - internal: hidden native agents (title, summary, compaction)
 */
export type AgentKind = "primary" | "subagent" | "internal"

export interface TurnLimits {
  /** Model calls (loop steps) per turn. The last one is text-only. Never Infinity. */
  steps: number
  /** Tool calls per turn, counted across every step of the turn. */
  toolCalls: number
  /** Wall-clock budget for one turn, in milliseconds. */
  turnWallClockMs: number
}

export const TURN_LIMIT_DEFAULTS: Record<AgentKind, TurnLimits> = {
  primary: { steps: 500, toolCalls: 1_000, turnWallClockMs: 2 * 60 * 60_000 },
  subagent: { steps: 200, toolCalls: 400, turnWallClockMs: 60 * 60_000 },
  internal: { steps: 20, toolCalls: 40, turnWallClockMs: 10 * 60_000 },
}

/**
 * Hard ceiling. An agent config (`steps`, `max_tool_calls`, `turn_timeout_ms`)
 * may raise or lower its own limits, but never above these.
 */
export const TURN_LIMIT_CEILING: TurnLimits = {
  steps: 2_000,
  toolCalls: 5_000,
  turnWallClockMs: 8 * 60 * 60_000,
}

/**
 * Stuck detection, evaluated over the turn's tool observations (call + result),
 * across steps. Complements the older in-message doom_loop ask (3 identical
 * calls in one assistant message), which stays as is.
 */
export const STUCK_DEFAULTS = {
  /** Same tool + same input + same result, this many times in a row. */
  repeatActionObservation: 4,
  /** Same tool + same input failing, this many times in a row. */
  repeatError: 3,
  /** Two distinct actions alternating A,B,A,B… for this many full A/B cycles. */
  pingPongCycles: 6,
} as const

// ─── O5: output token caps per call type ─────────────────────────────────────

/**
 * Call types a caller can declare to `LLM.stream` (`callType`). Unknown or
 * omitted means "answer", which is the previous global default.
 */
export type OutputCallType = "title" | "summary" | "compaction" | "decision" | "extraction" | "patch" | "answer"

/** The global default output cap (the pre-O5 behavior). */
export const DEFAULT_OUTPUT_TOKEN_MAX = 32_000

/**
 * Visible-output cap per call type. Always clamped again by the model's own
 * `limit.output`. `decision` is the LLM fallback for closed-set choices that
 * S1 normally serves at zero output; `patch` is for callers that ask for a
 * diff/code body directly (the main agent loop stays "answer" because its
 * write/edit tool inputs can be large).
 */
export const OUTPUT_TOKEN_CAPS: Record<OutputCallType, number> = {
  title: 64,
  summary: 1_024,
  compaction: 8_192,
  decision: 256,
  extraction: 2_048,
  patch: 8_192,
  answer: DEFAULT_OUTPUT_TOKEN_MAX,
}

/**
 * Reasoning models spend output tokens on thinking before the visible answer,
 * so a 64-token title cap would come back empty. Capped call types get this
 * much extra headroom on models with `capabilities.reasoning`.
 */
export const REASONING_OUTPUT_HEADROOM = 2_048

// ─── O8: prompt caching ──────────────────────────────────────────────────────

/** `promptCacheKey = sessionID` is on unless a provider sets `setCacheKey: false`. */
export const PROMPT_CACHE_KEY_DEFAULT = true

/**
 * The stable-prefix contract: request content is assembled in this order so
 * the cached prefix only grows. Anything that changes step to step (goal
 * reminders, validation reminders, wrap-up notices) belongs in the variable
 * tail, never in the system block. See prompt-segments.ts.
 */
export const PROMPT_SEGMENT_ORDER = ["system", "tools", "pinned", "history", "tail"] as const
export type PromptSegment = (typeof PROMPT_SEGMENT_ORDER)[number]
