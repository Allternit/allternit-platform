// @ts-nocheck
import { describe, expect, test } from "bun:test"
import { Guardrails } from "../../src/runtime/session/guardrails"
import {
  GUARDRAIL_DEFAULTS_VERSION,
  OUTPUT_TOKEN_CAPS,
  PROMPT_SEGMENT_ORDER,
  REASONING_OUTPUT_HEADROOM,
  TURN_LIMIT_CEILING,
  TURN_LIMIT_DEFAULTS,
} from "../../src/runtime/session/guardrail-defaults"
import { ProviderTransform } from "../../src/runtime/providers/adapters/transform"
import { PromptSegments } from "../../src/runtime/session/prompt-segments"
import { SessionUsage } from "../../src/runtime/session/usage"

const limits = { steps: 5, toolCalls: 3, turnWallClockMs: 1_000 }

describe("O12 turn limits", () => {
  test("defaults are finite and resolved per agent kind", () => {
    for (const row of Object.values(TURN_LIMIT_DEFAULTS)) {
      expect(Number.isFinite(row.steps)).toBe(true)
      expect(row.steps).toBeLessThanOrEqual(TURN_LIMIT_CEILING.steps)
    }
    expect(Guardrails.agentKind({ mode: "primary" })).toBe("primary")
    expect(Guardrails.agentKind({ mode: "all" })).toBe("primary")
    expect(Guardrails.agentKind({ mode: "subagent" })).toBe("subagent")
    expect(Guardrails.agentKind({ mode: "primary", hidden: true, native: true })).toBe("internal")
    expect(Guardrails.limits({ mode: "subagent" })).toEqual(TURN_LIMIT_DEFAULTS.subagent)
  })

  test("agent config overrides within the ceiling", () => {
    const resolved = Guardrails.limits({ mode: "primary", steps: 7, maxToolCalls: 10_000_000, turnTimeoutMs: 60_000 })
    expect(resolved.steps).toBe(7)
    expect(resolved.toolCalls).toBe(TURN_LIMIT_CEILING.toolCalls)
    expect(resolved.turnWallClockMs).toBe(60_000)
    expect(Guardrails.limits({ mode: "primary", steps: Infinity }).steps).toBe(TURN_LIMIT_DEFAULTS.primary.steps)
  })

  test("step cap: the limit step is the text-only last step, one past it trips", () => {
    const guard = new Guardrails.TurnGuard(limits)
    expect(guard.checkSteps(5)).toBeUndefined()
    const trip = guard.checkSteps(6)
    expect(trip?.kind).toBe("max_steps")
    expect(trip?.version).toBe(GUARDRAIL_DEFAULTS_VERSION)
  })

  test("tool-call cap counts across steps", () => {
    const guard = new Guardrails.TurnGuard(limits)
    expect(guard.recordToolCall("read")).toBeUndefined()
    expect(guard.recordToolCall("read")).toBeUndefined()
    expect(guard.recordToolCall("bash")).toBeUndefined()
    const trip = guard.recordToolCall("bash")
    expect(trip?.kind).toBe("max_tool_calls")
    expect(trip?.tool).toBe("bash")
    expect(guard.trip).toEqual(trip)
  })

  test("wall clock budget", () => {
    let now = 0
    const guard = new Guardrails.TurnGuard(limits, { now: () => now })
    now = 999
    expect(guard.checkWallClock()).toBeUndefined()
    expect(guard.remainingWallClockMs()).toBe(1)
    now = 1_000
    expect(guard.checkWallClock()?.kind).toBe("wall_clock")
  })

  test("reset starts a fresh slice", () => {
    let now = 0
    const guard = new Guardrails.TurnGuard(limits, { now: () => now })
    for (let i = 0; i < 4; i++) guard.recordToolCall("read")
    expect(guard.trip).toBeDefined()
    now = 5_000
    guard.reset()
    expect(guard.trip).toBeUndefined()
    expect(guard.toolCallCount).toBe(0)
    expect(guard.checkWallClock()).toBeUndefined()
  })
})

describe("O12 stuck detector", () => {
  const big = { steps: 1000, toolCalls: 1000, turnWallClockMs: 1e9 }

  test("same action + observation x4 trips, x3 does not", () => {
    const guard = new Guardrails.TurnGuard(big)
    for (let i = 0; i < 3; i++) expect(guard.recordObservation({ tool: "read", input: { a: 1, b: 2 }, output: "x" })).toBeUndefined()
    // key order doesn't make it a different action
    const trip = guard.recordObservation({ tool: "read", input: { b: 2, a: 1 }, output: "x" })
    expect(trip?.kind).toBe("repeat_action_observation")
  })

  test("same action, different observation does not trip", () => {
    const guard = new Guardrails.TurnGuard(big)
    for (let i = 0; i < 8; i++) expect(guard.recordObservation({ tool: "bash", input: { cmd: "date" }, output: `t${i}` })).toBeUndefined()
  })

  test("same action erroring x3 trips", () => {
    const guard = new Guardrails.TurnGuard(big)
    expect(guard.recordObservation({ tool: "edit", input: { f: 1 }, error: "no match" })).toBeUndefined()
    expect(guard.recordObservation({ tool: "edit", input: { f: 1 }, error: "no match (2)" })).toBeUndefined()
    expect(guard.recordObservation({ tool: "edit", input: { f: 1 }, error: "no match (3)" })?.kind).toBe("repeat_error")
  })

  test("errors on different inputs do not trip", () => {
    const guard = new Guardrails.TurnGuard(big)
    for (let i = 0; i < 5; i++) expect(guard.recordObservation({ tool: "edit", input: { f: i }, error: "no match" })).toBeUndefined()
  })

  test("A/B ping-pong trips at 6 cycles, not 5", () => {
    const guard = new Guardrails.TurnGuard(big)
    const step = (i: number) =>
      guard.recordObservation(i % 2 === 0 ? { tool: "edit", input: { v: "on" }, output: `o${i}` } : { tool: "edit", input: { v: "off" }, output: `o${i}` })
    for (let i = 0; i < 11; i++) expect(step(i)).toBeUndefined()
    expect(step(11)?.kind).toBe("ping_pong")
  })

  test("A/B/C rotation is not ping-pong", () => {
    const guard = new Guardrails.TurnGuard(big)
    for (let i = 0; i < 30; i++) expect(guard.recordObservation({ tool: "t", input: { k: i % 3 }, output: `${i}` })).toBeUndefined()
  })

  test("first trip sticks", () => {
    const guard = new Guardrails.TurnGuard(big)
    for (let i = 0; i < 3; i++) guard.recordObservation({ tool: "edit", input: {}, error: "e" })
    expect(guard.trip?.kind).toBe("repeat_error")
    guard.recordObservation({ tool: "edit", input: {}, error: "e" })
    expect(guard.trip?.kind).toBe("repeat_error")
  })
})

describe("O5 output caps per call type", () => {
  const model = (output: number, reasoning = false) => ({ limit: { output }, capabilities: { reasoning } })

  test("unknown / omitted call type keeps the previous default", () => {
    expect(ProviderTransform.maxOutputTokens(model(64_000))).toBe(ProviderTransform.OUTPUT_TOKEN_MAX)
    expect(ProviderTransform.maxOutputTokens(model(64_000), "nonsense")).toBe(ProviderTransform.OUTPUT_TOKEN_MAX)
    expect(ProviderTransform.maxOutputTokens(model(64_000), "answer")).toBe(ProviderTransform.OUTPUT_TOKEN_MAX)
  })

  test("each row applies and is clamped by model.limit.output", () => {
    expect(ProviderTransform.maxOutputTokens(model(64_000), "title")).toBe(OUTPUT_TOKEN_CAPS.title)
    expect(ProviderTransform.maxOutputTokens(model(64_000), "summary")).toBe(OUTPUT_TOKEN_CAPS.summary)
    expect(ProviderTransform.maxOutputTokens(model(64_000), "decision")).toBe(OUTPUT_TOKEN_CAPS.decision)
    expect(ProviderTransform.maxOutputTokens(model(64_000), "patch")).toBe(OUTPUT_TOKEN_CAPS.patch)
    expect(ProviderTransform.maxOutputTokens(model(4_000), "patch")).toBe(4_000)
    expect(ProviderTransform.maxOutputTokens(model(4_000))).toBe(4_000)
  })

  test("reasoning models get headroom on capped rows", () => {
    expect(ProviderTransform.maxOutputTokens(model(64_000, true), "title")).toBe(OUTPUT_TOKEN_CAPS.title + REASONING_OUTPUT_HEADROOM)
    expect(ProviderTransform.maxOutputTokens(model(64_000, true))).toBe(ProviderTransform.OUTPUT_TOKEN_MAX)
  })
})

describe("O8 stable prefix", () => {
  test("segment order is the contract", () => {
    expect([...PROMPT_SEGMENT_ORDER]).toEqual(["system", "tools", "pinned", "history", "tail"])
  })

  test("tools are ordered deterministically", () => {
    expect(Object.keys(PromptSegments.orderTools({ write: 1, bash: 2, read: 3 }))).toEqual(["bash", "read", "write"])
  })

  test("no tail leaves history untouched", () => {
    const history = [{ role: "user", content: "hi" }]
    expect(PromptSegments.withTail(history, [])).toBe(history)
    expect(PromptSegments.withTail(history, undefined)).toBe(history)
  })

  test("tail after a tool result is its own trailing user message; prefix unchanged", () => {
    const history = [
      { role: "user", content: "do it" },
      { role: "assistant", content: [{ type: "tool-call", toolCallId: "1", toolName: "read", input: {} }] },
      { role: "tool", content: [{ type: "tool-result", toolCallId: "1", toolName: "read", output: { type: "text", value: "x" } }] },
    ]
    const out = PromptSegments.withTail(history, ["goal: 3/10"])
    expect(out.slice(0, 3)).toEqual(history)
    expect(out[3].role).toBe("user")
    expect(out[3].content).toContain("goal: 3/10")
  })

  test("tail joins a trailing user message instead of adding a consecutive one", () => {
    const out = PromptSegments.withTail([{ role: "user", content: "hi" }], ["note"])
    expect(out).toHaveLength(1)
    expect(out[0].content[0]).toEqual({ type: "text", text: "hi" })
    expect(out[0].content[1].text).toContain("note")
  })

  test("a trailing assistant prefill stays last", () => {
    const history = [
      { role: "tool", content: [] },
      { role: "assistant", content: "MAX STEPS" },
    ]
    const out = PromptSegments.withTail(history, ["note"])
    expect(out.map((m) => m.role)).toEqual(["tool", "user", "assistant"])
  })
})

describe("O8 cache hit-rate counter", () => {
  const entry = (input: number, read: number, write: number, callType?: string) => ({
    tokens: { input, output: 10, reasoning: 0, cache: { read, write } },
    callType,
  })

  test("hit rate = cache reads / all prompt tokens, overall and per call type", () => {
    const { total, byCallType } = SessionUsage.cacheHitRate([
      entry(100, 900, 0, "answer"),
      entry(100, 0, 900),
      entry(50, 50, 0, "title"),
    ])
    expect(total.read).toBe(950)
    expect(total.prompt).toBe(2_100)
    expect(total.hitRate).toBeCloseTo(950 / 2_100)
    expect(byCallType.answer.hitRate).toBeCloseTo(900 / 2_000)
    expect(byCallType.title.hitRate).toBeCloseTo(0.5)
  })

  test("empty usage is a 0 hit rate, not NaN", () => {
    expect(SessionUsage.cacheHitRate([]).total.hitRate).toBe(0)
  })
})
