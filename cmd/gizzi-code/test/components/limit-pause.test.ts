import { describe, expect, test } from "bun:test"
import { limitInTurn, providerOfModel } from "../../src/cli/ui/ink-app/utils/limitPause"

const assistant = (text: string) => ({ type: "assistant", message: { content: [{ type: "text", text }] } }) as never
const user = (text: string) => ({ type: "user", message: { content: text } }) as never

describe("REPL limit pause (P3.17)", () => {
  const now = Date.UTC(2026, 8, 28, 12, 0)

  test("a turn cut off by a stated limit pauses until the reset", () => {
    const hit = limitInTurn([user("go"), assistant("You've hit your session limit · resets in 2 hours 13 minutes")], now)
    expect(hit).toEqual({ until: now + (2 * 60 + 13) * 60_000, limit: "5-hour limit" })
  })

  test("an epoch-stamped limit reply", () => {
    expect(limitInTurn([assistant("Claude AI usage limit reached|1790600000")], now)).toEqual({ until: 1790600000_000, limit: "usage limit" })
  })

  test("an ordinary reply, or a long one that merely mentions limits, doesn't pause", () => {
    expect(limitInTurn([assistant("Done. The rate limiter is configured.")], now)).toBeUndefined()
    expect(limitInTurn([assistant("limit ".repeat(200) + "resets in 1 hour")], now)).toBeUndefined()
    expect(limitInTurn([user("resets in 1 hour limit")], now)).toBeUndefined()
  })

  test("provider of a gizzi model id", () => {
    expect(providerOfModel("openrouter/z-ai/glm-4.7-flash")).toBe("openrouter")
    expect(providerOfModel("claude-sonnet-5")).toBeUndefined()
    expect(providerOfModel(null)).toBeUndefined()
  })
})

describe("/resume-now", () => {
  test("switches to the suggested model, clears the pause and continues", async () => {
    const { call } = await import("../../src/cli/ui/ink-app/commands/resume-now/resume-now")
    const { getCommandQueue, removeByFilter } = await import("../../src/cli/ui/ink-app/utils/messageQueueManager")
    let state: any = {
      mainLoopModel: "claude-cli/claude-sonnet-5",
      replPause: { until: Date.now() + 3_600_000, limit: "5-hour limit", providerID: "claude-cli", reason: "limit_hit", suggest: { providerID: "kimi", modelID: "k3", label: "Kimi · K3", headroom: 0.6 } },
    }
    const context = { getAppState: () => state, setAppState: (f: (s: any) => any) => { state = f(state) } } as never
    const result = await call("", context)
    expect(result).toEqual({ type: "text", value: "Continuing on Kimi · K3." })
    expect(state.replPause).toBeUndefined()
    expect(state.mainLoopModel).toBe("kimi/k3")
    expect(getCommandQueue().some((c: any) => c.value === "Continue where you left off.")).toBe(true)
    removeByFilter((c: any) => c.value === "Continue where you left off.")
  })

  test("nothing paused", async () => {
    const { call } = await import("../../src/cli/ui/ink-app/commands/resume-now/resume-now")
    const context = { getAppState: () => ({}), setAppState: () => {} } as never
    expect(await call("", context)).toEqual({ type: "text", value: "Nothing is paused." })
  })
})
