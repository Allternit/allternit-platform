import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import path from "node:path"
import type { SessionTrace } from "../../src/runtime/session/trace"
import { toTrajectory, WRITE_TOOLS } from "../../src/runtime/eval/trajectory"
import { grade } from "../../src/runtime/eval/graders"

const fixture = JSON.parse(readFileSync(path.join(import.meta.dir, "fixtures/trace-basic.json"), "utf8")) as SessionTrace.Entry[]

describe("toTrajectory", () => {
  test("maps the basic trace to the exact trajectory", () => {
    expect(toTrajectory(fixture)).toEqual({
      status: "completed",
      verifier: null,
      steps: [
        { node: "build", primitive: "edit", ok: true },
        { node: "build", primitive: "bash", ok: false },
      ],
      effects: [
        { node: "build", tool: "edit", committed: true },
        { node: "build", tool: "bash", committed: false },
      ],
      texts: ["Edited a.ts; the test run failed."],
      stopped_at: null,
      missing: [],
      cost_usd: 0.0125,
      tokens: 1550,
      latency_ms: 3100,
    })
  })

  test("a guardrail trip means stopped, with its kind, even after a completed request", () => {
    const trip: SessionTrace.Entry = {
      sequence: 99, sessionID: "ses_1", kind: "guardrail.tripped", time: 1_004_000,
      data: { kind: "max_tool_calls", limit: 2 },
    }
    const t = toTrajectory([...fixture, trip])
    expect(t.status).toBe("stopped")
    expect(t.stopped_at).toBe("max_tool_calls")
    expect(t.latency_ms).toBe(4000)
  })

  test("a failed request without a trip is failed; an empty trace is unknown", () => {
    const fail: SessionTrace.Entry = { sequence: 50, sessionID: "ses_1", kind: "request.failed", time: 1_003_000, data: { error: "x" } }
    expect(toTrajectory([fixture[2]!, fail]).status).toBe("failed")
    expect(toTrajectory([]).status).toBe("unknown")
  })

  test("write set includes mcp__ tools", () => {
    expect(WRITE_TOOLS.has("bash")).toBe(true)
    const t = toTrajectory([
      { sequence: 1, sessionID: "s", kind: "part.updated", time: 1, data: { type: "tool", callID: "a", tool: "mcp__x__y", state: { status: "completed" } } },
    ])
    expect(t.effects).toEqual([{ node: "main", tool: "mcp__x__y", committed: true }])
  })

  test("adapter output is gradeable", () => {
    const t = toTrajectory(fixture)
    expect(grade(t, { type: "effect_count", max: 1 }).pass).toBe(false)
    expect(grade(t, { type: "effect_count", max: 2 }).pass).toBe(true)
    expect(grade(t, { type: "forbidden_step", steps: ["bash"] }).pass).toBe(false)
    expect(grade(toTrajectory([...fixture, { sequence: 99, sessionID: "ses_1", kind: "guardrail.tripped", time: 1_004_000, data: { kind: "ping_pong" } }]), { type: "stopped_at", step: "ping_pong" }).pass).toBe(true)
  })
})
