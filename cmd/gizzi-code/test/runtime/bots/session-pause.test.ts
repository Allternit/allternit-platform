import { afterAll, beforeEach, describe, expect, test } from "bun:test"

process.env.ALLTERNIT_API_TOKEN = "test-token"
const { pausedOf, untilLabel, limitLabel, pausedLine, getSessionPaused, resumeSession } = await import(
  "../../../src/runtime/bots/session-pause"
)

const realFetch = globalThis.fetch
let calls: Array<{ method: string; path: string; body?: unknown }> = []
beforeEach(() => {
  calls = []
})
afterAll(() => {
  globalThis.fetch = realFetch
})

describe("session pause copy (matches Desktop)", () => {
  const now = new Date(2026, 8, 27, 15, 0).getTime() // Sun Sep 27 2026, 3:00 PM local

  test("until: today shows the time, this week the weekday, later the date", () => {
    const today = new Date(2026, 8, 27, 19, 40).getTime()
    const thisWeek = new Date(2026, 9, 3, 9, 0).getTime()
    const later = new Date(2026, 9, 10, 9, 0).getTime()
    expect(untilLabel(today, now)).toBe(today && new Date(today).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }))
    expect(untilLabel(thisWeek, now)).toMatch(/^Sat /)
    expect(untilLabel(later, now)).toMatch(/Oct 10, /)
  })

  test("limit names the provider once, capitalized", () => {
    expect(limitLabel({ until: 1, limit: "5-hour limit", providerID: "claude-cli" })).toBe("Claude 5-hour limit")
    expect(limitLabel({ until: 1, limit: "Kimi For Coding weekly limit", providerID: "kimi" })).toBe("Kimi For Coding weekly limit")
    expect(limitLabel({ until: 1, limit: "rate limit" })).toBe("Rate limit")
  })

  test("one line", () => {
    const until = new Date(2026, 8, 27, 19, 40).getTime()
    expect(pausedLine({ until, limit: "5-hour limit", providerID: "claude-cli" }, now)).toBe(
      `Paused until ${untilLabel(until, now)} · Claude 5-hour limit · resumes on its own`,
    )
  })

  test("pausedOf ignores expired or malformed pauses", () => {
    expect(pausedOf({ paused: { until: now + 1000, limit: "5-hour limit" } }, now)).toMatchObject({ limit: "5-hour limit" })
    expect(pausedOf({ paused: { until: now - 1, limit: "5-hour limit" } }, now)).toBeNull()
    expect(pausedOf({ paused: { limit: "x" } }, now)).toBeNull()
    expect(pausedOf(null, now)).toBeNull()
  })
})

describe("session pause API", () => {
  test("reads the pause from the session's metadata and resumes on request", async () => {
    const until = Date.now() + 3_600_000
    globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
      const url = new URL(input)
      calls.push({ method: init.method ?? "GET", path: url.pathname, body: init.body ? JSON.parse(String(init.body)) : undefined })
      if (url.pathname === "/api/v1/agent-sessions/s1") {
        return Response.json({ id: "s1", metadata: { paused: { until, limit: "5-hour limit", providerID: "claude-cli", reason: "quota" } } })
      }
      return Response.json({ success: true })
    }) as typeof fetch
    expect(await getSessionPaused("s1")).toMatchObject({ until, limit: "5-hour limit" })
    await resumeSession("s1")
    await resumeSession("s1", { providerID: "openrouter", modelID: "z-ai/glm-4.7-flash" })
    expect(calls.slice(1)).toEqual([
      { method: "POST", path: "/api/v1/agent-sessions/s1/resume", body: {} },
      { method: "POST", path: "/api/v1/agent-sessions/s1/resume", body: { model: { providerID: "openrouter", modelID: "z-ai/glm-4.7-flash" } } },
    ])
  })
})
