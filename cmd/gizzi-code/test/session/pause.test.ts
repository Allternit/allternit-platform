import { afterEach, describe, expect, test } from "bun:test"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionPause } from "../../src/runtime/session/pause"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { parseClaudeQuotaLimits, parseCodexRateLimits } from "../../src/runtime/providers/quota"
import { Identifier } from "../../src/shared/id/id"
import { tmpdir } from "../fixture/fixture"

afterEach(() => SessionPause.clearTimers())

// 2026-09-27 20:46 in Chicago (CDT, UTC-5) = 2026-09-28T01:46Z.
const NOW = Date.parse("2026-09-28T01:46:00Z")

describe("reading a limit", () => {
  test("Claude CLI session limit with a time and zone", () => {
    const hit = SessionPause.limitFromText("You've hit your session limit · resets 7:40pm (America/Chicago)", NOW)
    // 7:40pm Chicago already passed today → tomorrow 7:40pm CDT = 00:40Z on the 29th.
    expect(new Date(hit!.until).toISOString()).toBe("2026-09-29T00:40:00.000Z")
    expect(hit!.limit).toBe("5-hour limit")
    const soon = SessionPause.limitFromText("You've hit your session limit · resets 1:20am (America/Chicago)", NOW)
    expect(new Date(soon!.until).toISOString()).toBe("2026-09-28T06:20:00.000Z")
  })

  test("weekly limit with a date, epoch form, and relative waits", () => {
    const weekly = SessionPause.limitFromText("You've hit your weekly limit · resets Oct 3, 9am (UTC)", NOW)
    expect(new Date(weekly!.until).toISOString()).toBe("2026-10-03T09:00:00.000Z")
    expect(weekly!.limit).toBe("weekly limit")
    expect(SessionPause.limitFromText("Claude AI usage limit reached|1790576400", NOW)!.until).toBe(1790576400000)
    expect(SessionPause.limitFromText("You've hit your usage limit. Try again in 2 hours 13 minutes.", NOW)!.until).toBe(
      NOW + (2 * 60 + 13) * 60_000,
    )
    expect(SessionPause.limitFromText("The pricing limit is 35% margin, reset later", NOW)).toBeUndefined()
    expect(SessionPause.limitFromText("All-in H100 is $1.94/hr", NOW)).toBeUndefined()
  })

  test("quota windows land at the threshold, on the latest reset", () => {
    const windows = parseCodexRateLimits({
      primary: { used_percent: 98, window_minutes: 300, resets_at: 1790579075 },
      secondary: { used_percent: 56, window_minutes: 10080, resets_at: 1791065004 },
    })
    expect(windows.map((w) => [w.label, w.usedRatio])).toEqual([
      ["5-hour", 0.98],
      ["Weekly", 0.56],
    ])
    const land = SessionPause.windowsLanding("Codex", windows, 0.95, NOW)
    expect(land).toEqual({ until: 1790579075000, limit: "Codex 5-hour limit" })
    expect(SessionPause.windowsLanding("Codex", windows, 0.99, NOW)).toBeUndefined()

    expect(parseClaudeQuotaLimits({ status: "rejected", resetsAt: 1790576400, rateLimitType: "five_hour" }, NOW)).toEqual([
      { id: "5h", label: "5-hour", usedRatio: 1, resetAt: "2026-09-28T06:20:00.000Z" },
    ])
    expect(parseClaudeQuotaLimits({ status: "allowed_warning", resetsAt: 1790576400, rateLimitType: "seven_day" }, NOW)[0]).toMatchObject({
      label: "Weekly",
      usedRatio: 0.95,
    })
    expect(parseClaudeQuotaLimits({ status: "rejected", resetsAt: 1 }, NOW)).toEqual([])
  })
})

async function turn(sessionID: string, role: "user" | "assistant", text: string) {
  const id = Identifier.ascending("message")
  const base = { id, sessionID, time: { created: Date.now() } }
  await Session.updateMessage(
    role === "user"
      ? { ...base, role, agent: "build", model: { providerID: "claude-cli", modelID: "sonnet" } }
      : ({
          ...base,
          role,
          parentID: id,
          mode: "build",
          agent: "build",
          path: { cwd: "/", root: "/" },
          cost: 0,
          tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
          modelID: "sonnet",
          providerID: "claude-cli",
          finish: "stop",
        } as MessageV2.Assistant),
  )
  await Session.updatePart({ id: Identifier.ascending("part"), messageID: id, sessionID, type: "text", text })
  return id
}

describe("pausing and resuming", () => {
  test("a limit reply pauses the session, persists, and re-arms after a restart", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({ title: "Pricing" })
        await turn(s.id, "user", "Keep going on the sheet")
        await turn(s.id, "assistant", "You've hit your session limit · resets 7:40pm (America/Chicago)")
        const msgs = await MessageV2.filterCompacted(MessageV2.stream(s.id))
        const hit = SessionPause.limitFromTurn(msgs.at(-1))
        expect(hit).toMatchObject({ limit: "5-hour limit", reason: "limit_hit" })

        SessionPause.pause(s.id, { until: hit!.until, limit: hit!.limit, reason: hit!.reason, providerID: "claude-cli" })
        const paused = await Session.get(s.id)
        expect(paused.paused).toMatchObject({ limit: "5-hour limit", providerID: "claude-cli" })
        expect(SessionPause.isPaused(paused)).toBe(true)
        expect(SessionPause.scheduled(s.id)).toBe(true)

        // Restart: timers are gone, restore() re-arms them from the database.
        SessionPause.clearTimers()
        expect(SessionPause.scheduled(s.id)).toBe(false)
        SessionPause.restore()
        expect(SessionPause.scheduled(s.id)).toBe(true)
        expect(Session.listPaused().map((x) => x.id)).toEqual([s.id])

        Session.setPaused({ sessionID: s.id, paused: null })
        expect(Session.listPaused()).toEqual([])
        await Session.remove(s.id)
      },
    })
  })

  test("a long rate-limit wait pauses; a short one stays a retry", () => {
    const err = (retryAfter: string) =>
      ({
        info: {
          role: "assistant",
          error: new MessageV2.APIError({
            message: "429 Too Many Requests: rate limit",
            statusCode: 429,
            isRetryable: true,
            responseHeaders: { "retry-after": retryAfter },
          }).toObject(),
        },
        parts: [],
      }) as unknown as MessageV2.WithParts
    expect(SessionPause.limitFromTurn(err("1800"), NOW)).toEqual({ until: NOW + 1_800_000, limit: "rate limit", reason: "rate_limit" })
    expect(SessionPause.limitFromTurn(err("20"), NOW)).toBeUndefined()
  })
})

describe("continuing on another model", () => {
  const c = (providerID: string, headroom: number | null, modelID = "m") => ({ providerID, modelID, label: providerID, headroom })

  test("most limit left first; unknown limits after known; near-limit ones are out", () => {
    const ranked = SessionPause.rankCandidates([c("kimi-cli", 0.4), c("openrouter", null), c("codex-cli", 0.02), c("anthropic", 0.8)], [], 0.95)
    expect(ranked.map((x) => x.providerID)).toEqual(["anthropic", "kimi-cli", "openrouter"])
  })

  test("the configured order wins over headroom", () => {
    const ranked = SessionPause.rankCandidates(
      [c("kimi-cli", 0.4, "kimi-k3"), c("anthropic", 0.8, "sonnet"), c("openrouter", null, "x")],
      ["kimi-cli/kimi-k3", "openrouter"],
      0.95,
    )
    expect(ranked.map((x) => x.providerID)).toEqual(["kimi-cli", "openrouter", "anthropic"])
  })
})
