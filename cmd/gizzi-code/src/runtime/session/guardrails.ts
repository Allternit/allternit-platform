import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"
import {
  GUARDRAIL_DEFAULTS_VERSION,
  STUCK_DEFAULTS,
  TURN_LIMIT_CEILING,
  TURN_LIMIT_DEFAULTS,
  type AgentKind,
  type TurnLimits,
} from "./guardrail-defaults"

/**
 * O12 per-turn guardrails: step cap, tool-call cap, wall-clock budget and the
 * stuck detector. Pure state, no I/O, so it is unit-testable; prompt.ts owns
 * one TurnGuard per turn and the processor feeds it tool calls/observations.
 */
export namespace Guardrails {
  export const TripKind = z.enum([
    "max_steps",
    "max_tool_calls",
    "wall_clock",
    "repeat_action_observation",
    "repeat_error",
    "ping_pong",
  ])
  export type TripKind = z.infer<typeof TripKind>

  export const Trip = z.object({
    kind: TripKind,
    reason: z.string(),
    limit: z.number(),
    observed: z.number(),
    tool: z.string().optional(),
    version: z.string(),
  })
  export type Trip = z.infer<typeof Trip>

  export const Event = {
    Tripped: BusEvent.define(
      "session.guardrail.tripped",
      z.object({
        sessionID: z.string(),
        messageID: z.string().optional(),
        trip: Trip,
      }),
    ),
  }

  type AgentLike = {
    mode: "subagent" | "primary" | "all"
    hidden?: boolean
    native?: boolean
    steps?: number
    maxToolCalls?: number
    turnTimeoutMs?: number
  }

  export function agentKind(agent: AgentLike): AgentKind {
    if (agent.mode === "subagent") return "subagent"
    if (agent.hidden && agent.native) return "internal"
    return "primary"
  }

  function clamp(value: number | undefined, fallback: number, ceiling: number) {
    if (value === undefined || !Number.isFinite(value) || value <= 0) return fallback
    return Math.min(Math.floor(value), ceiling)
  }

  /** Versioned default for the agent's kind, overridden by its config, never above the ceiling. */
  export function limits(agent: AgentLike): TurnLimits {
    const base = TURN_LIMIT_DEFAULTS[agentKind(agent)]
    return {
      steps: clamp(agent.steps, base.steps, TURN_LIMIT_CEILING.steps),
      toolCalls: clamp(agent.maxToolCalls, base.toolCalls, TURN_LIMIT_CEILING.toolCalls),
      turnWallClockMs: clamp(agent.turnTimeoutMs, base.turnWallClockMs, TURN_LIMIT_CEILING.turnWallClockMs),
    }
  }

  /** Deterministic JSON (sorted keys) so `{a,b}` and `{b,a}` are the same action. */
  export function stableKey(value: unknown): string {
    if (value === null || typeof value !== "object") return JSON.stringify(value) ?? "undefined"
    if (Array.isArray(value)) return `[${value.map(stableKey).join(",")}]`
    const entries = Object.keys(value as Record<string, unknown>)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableKey((value as Record<string, unknown>)[key])}`)
    return `{${entries.join(",")}}`
  }

  export interface Observation {
    tool: string
    input: unknown
    output?: string
    error?: string
  }

  type Seen = { action: string; tool: string; observation: string; error: boolean }

  export class TurnGuard {
    readonly limits: TurnLimits
    readonly stuck: typeof STUCK_DEFAULTS
    private readonly now: () => number
    private startedAt = 0
    private toolCalls = 0
    private history: Seen[] = []
    private tripped: Trip | undefined

    constructor(limits: TurnLimits, opts?: { stuck?: Partial<typeof STUCK_DEFAULTS>; now?: () => number }) {
      this.limits = limits
      this.stuck = { ...STUCK_DEFAULTS, ...opts?.stuck }
      this.now = opts?.now ?? Date.now
      this.reset()
    }

    /** A new turn (or a durable-goal continuation slice) starts fresh counters. */
    reset() {
      this.startedAt = this.now()
      this.toolCalls = 0
      this.history = []
      this.tripped = undefined
    }

    get trip() {
      return this.tripped
    }

    get toolCallCount() {
      return this.toolCalls
    }

    private fire(trip: Omit<Trip, "version">): Trip {
      this.tripped ??= { ...trip, version: GUARDRAIL_DEFAULTS_VERSION }
      return this.tripped
    }

    /** `stepsThisTurn` counts the step about to run. The step at the limit is the text-only last step; one past it trips. */
    checkSteps(stepsThisTurn: number): Trip | undefined {
      if (stepsThisTurn <= this.limits.steps) return
      return this.fire({
        kind: "max_steps",
        reason: `Stopped: the agent kept calling tools after its last allowed step (${this.limits.steps} steps per turn).`,
        limit: this.limits.steps,
        observed: stepsThisTurn,
      })
    }

    remainingWallClockMs() {
      return Math.max(0, this.startedAt + this.limits.turnWallClockMs - this.now())
    }

    checkWallClock(): Trip | undefined {
      const elapsed = this.now() - this.startedAt
      if (elapsed < this.limits.turnWallClockMs) return
      return this.fire({
        kind: "wall_clock",
        reason: `Stopped: the turn ran past its wall-clock budget (${Math.round(this.limits.turnWallClockMs / 60_000)} min).`,
        limit: this.limits.turnWallClockMs,
        observed: elapsed,
      })
    }

    recordToolCall(tool: string): Trip | undefined {
      this.toolCalls++
      if (this.toolCalls <= this.limits.toolCalls) return
      return this.fire({
        kind: "max_tool_calls",
        reason: `Stopped: the turn reached its tool-call limit (${this.limits.toolCalls} per turn).`,
        limit: this.limits.toolCalls,
        observed: this.toolCalls,
        tool,
      })
    }

    recordObservation(obs: Observation): Trip | undefined {
      const error = obs.error !== undefined
      this.history.push({
        tool: obs.tool,
        action: `${obs.tool}:${stableKey(obs.input)}`,
        observation: error ? `error:${obs.error}` : `ok:${obs.output ?? ""}`,
        error,
      })
      // Keep only what the longest detector needs.
      const keep = Math.max(this.stuck.repeatActionObservation, this.stuck.repeatError, this.stuck.pingPongCycles * 2)
      if (this.history.length > keep) this.history.splice(0, this.history.length - keep)
      return this.detect()
    }

    private detect(): Trip | undefined {
      const h = this.history
      const last = h[h.length - 1]

      const errs = h.slice(-this.stuck.repeatError)
      if (errs.length === this.stuck.repeatError && errs.every((s) => s.error && s.action === last.action)) {
        return this.fire({
          kind: "repeat_error",
          reason: `Stopped: ${last.tool} failed ${this.stuck.repeatError} times in a row with the same input.`,
          limit: this.stuck.repeatError,
          observed: this.stuck.repeatError,
          tool: last.tool,
        })
      }

      const same = h.slice(-this.stuck.repeatActionObservation)
      if (
        same.length === this.stuck.repeatActionObservation &&
        same.every((s) => s.action === last.action && s.observation === last.observation)
      ) {
        return this.fire({
          kind: "repeat_action_observation",
          reason: `Stopped: ${last.tool} was called ${this.stuck.repeatActionObservation} times in a row with the same input and got the same result.`,
          limit: this.stuck.repeatActionObservation,
          observed: this.stuck.repeatActionObservation,
          tool: last.tool,
        })
      }

      const window = this.stuck.pingPongCycles * 2
      const pp = h.slice(-window)
      if (pp.length === window) {
        const a = pp[0].action
        const b = pp[1].action
        if (a !== b && pp.every((s, i) => s.action === (i % 2 === 0 ? a : b))) {
          return this.fire({
            kind: "ping_pong",
            reason: `Stopped: the agent alternated between the same two actions (${pp[0].tool} / ${pp[1].tool}) for ${this.stuck.pingPongCycles} cycles.`,
            limit: this.stuck.pingPongCycles,
            observed: this.stuck.pingPongCycles,
            tool: last.tool,
          })
        }
      }
      return
    }
  }
}
