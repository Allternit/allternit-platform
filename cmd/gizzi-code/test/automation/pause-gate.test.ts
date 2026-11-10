// @ts-nocheck
import { beforeEach, describe, expect, test } from "bun:test"
import { __setPauseFetcher, botsPausedCached, refreshBotPause, waitWhilePaused } from "../../src/runtime/automation/pause-gate"

beforeEach(() => __setPauseFetcher(null))

describe("pause gate", () => {
  test("paused: reported and cached (one fetch inside 10 s)", async () => {
    let n = 0
    __setPauseFetcher(async () => (n++, { paused: true, since: 1, by: "phone" }))
    expect(await refreshBotPause(1000)).toBe(true)
    expect(await refreshBotPause(5000)).toBe(true)
    expect(n).toBe(1)
    expect(botsPausedCached()).toBe(true)
    await refreshBotPause(12000)
    expect(n).toBe(2)
  })
  test("unreachable or 404: not blocked", async () => {
    __setPauseFetcher(async () => { throw new Error("fetch failed") })
    expect(await refreshBotPause(1000)).toBe(false)
    __setPauseFetcher(async () => { throw new Error("GET /api/v1/bots/pause failed: 404") })
    expect(await refreshBotPause(50000)).toBe(false)
    expect(botsPausedCached()).toBe(false)
  })
  test("waitWhilePaused holds, logs once, releases on resume", async () => {
    let calls = 0
    __setPauseFetcher(async () => ({ paused: ++calls < 3, since: null, by: null }))
    let held = 0, slept = 0, t = 0
    const clock = Date.now
    Date.now = () => (t += 20000)
    try {
      await waitWhilePaused(() => false, () => held++, async () => { slept++ })
    } finally { Date.now = clock }
    expect(held).toBe(1)
    expect(slept).toBe(2)
  })
  test("waitWhilePaused returns at once when the loop was stopped", async () => {
    __setPauseFetcher(async () => ({ paused: true, since: null, by: null }))
    await waitWhilePaused(() => true)
  })
})
