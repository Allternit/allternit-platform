import { describe, expect, test } from "bun:test"
import {
  ProviderQuotas,
  parseAllternitUsage,
  parseGatewayRateLimits,
  parseLimitHeaders,
} from "../../src/runtime/providers/quota"
import { SessionLimit } from "../../src/runtime/session/limit"

const NOW = Date.parse("2026-09-28T12:00:00Z")

describe("limit windows from response headers", () => {
  test("Anthropic unified 5h / 7d / per-model weekly", () => {
    const h = new Headers({
      "anthropic-ratelimit-unified-5h-utilization": "0.97",
      "anthropic-ratelimit-unified-5h-reset": "1790604000",
      "anthropic-ratelimit-unified-7d-utilization": "0.41",
      "anthropic-ratelimit-unified-7d-reset": "1791000000",
      "anthropic-ratelimit-unified-7d_opus-utilization": "0.88",
    })
    expect(parseLimitHeaders(h, NOW)).toEqual([
      { id: "5h", label: "5-hour", usedRatio: 0.97, resetAt: new Date(1790604000_000).toISOString() },
      { id: "7d", label: "Weekly", usedRatio: 0.41, resetAt: new Date(1791000000_000).toISOString() },
      { id: "7d-opus", label: "Weekly Opus", usedRatio: 0.88 },
    ])
  })

  test("a rejected status names its binding claim", () => {
    const h = new Headers({
      "anthropic-ratelimit-unified-status": "rejected",
      "anthropic-ratelimit-unified-representative-claim": "seven_day_sonnet",
      "anthropic-ratelimit-unified-reset": "1791000000",
    })
    expect(parseLimitHeaders(h, NOW)).toEqual([
      { id: "7d-sonnet", label: "Weekly Sonnet", usedRatio: 1, resetAt: new Date(1791000000_000).toISOString() },
    ])
  })

  test("Codex primary / secondary windows; per-minute rate limits are not windows", () => {
    const h = new Headers({
      "x-codex-primary-used-percent": "62",
      "x-codex-primary-window-minutes": "300",
      "x-codex-primary-reset-after-seconds": "600",
      "x-codex-secondary-used-percent": "12.5",
      "x-codex-secondary-window-minutes": "10080",
      "x-ratelimit-remaining-requests": "0",
      "x-ratelimit-limit-requests": "60",
    })
    expect(parseLimitHeaders(h, NOW)).toEqual([
      { id: "5h", label: "5-hour", usedRatio: 0.62, resetAt: new Date(NOW + 600_000).toISOString() },
      { id: "7d", label: "Weekly", usedRatio: 0.125 },
    ])
    expect(parseLimitHeaders(new Headers({ "x-ratelimit-remaining-tokens": "0" }), NOW)).toEqual([])
  })

  test("recorded headers make a provider readable and merge over fetched windows", async () => {
    ProviderQuotas.clearCache()
    expect(await ProviderQuotas.get("anthropic")).toEqual({ status: "unsupported" })
    ProviderQuotas.recordHeaders(
      "anthropic",
      new Headers({ "anthropic-ratelimit-unified-5h-utilization": "0.5", "anthropic-ratelimit-unified-5h-reset": String(Math.floor(Date.now() / 1000) + 3600) }),
    )
    const got = await ProviderQuotas.get("anthropic")
    expect(got.status).toBe("ok")
    expect(got.status === "ok" && got.quota.windows.map((w) => [w.id, w.usedRatio])).toEqual([["5h", 0.5]])
    expect(ProviderQuotas.peek("anthropic")?.status).toBe("ok")
    ProviderQuotas.clearCache()
  })
})

describe("Allternit Cloud plan usage", () => {
  test("/me/usage → a monthly window (the weekly* fields carry the monthly grant)", () => {
    expect(parseAllternitUsage({ plan: "plus", weeklyUsed: 19, weeklyLimit: 20, resetsAt: "2026-10-01T00:00:00+00:00" })).toEqual([
      { id: "month", label: "Monthly", usedRatio: 0.95, resetAt: "2026-10-01T00:00:00.000Z" },
    ])
    expect(parseAllternitUsage({ plan: "free", weeklyUsed: 0.5, weeklyLimit: 2 })[0]).toMatchObject({ label: "Free monthly", usedRatio: 0.25 })
    // The local proxy's fail-soft default (no limit) is not a window.
    expect(parseAllternitUsage({ plan: "free", weeklyUsed: 0, weeklyLimit: 0 })).toEqual([])
  })

  test("gateway key budget fallback", () => {
    expect(parseGatewayRateLimits({ tokens_remaining: 250, tokens_limit: 1000 }, NOW)).toEqual([
      { id: "month", label: "Key budget", usedRatio: 0.75, resetAt: "2026-10-01T00:00:00.000Z" },
    ])
    expect(parseGatewayRateLimits({ tokens_remaining: 0, tokens_limit: 0 }, NOW)).toEqual([])
  })
})

describe("classifying a session against its windows", () => {
  const t = { warnAt: 0.8, landAt: 0.95, wrapUpSteps: 3 }
  const reset = (h: number) => new Date(NOW + h * 3_600_000).toISOString()

  test("ok → approaching → land, landing on the latest reset among spent windows", () => {
    expect(SessionLimit.classify([{ id: "5h", label: "5-hour", usedRatio: 0.5, resetAt: reset(1) }], t, undefined, NOW).state).toBe("ok")
    expect(SessionLimit.classify([{ id: "5h", label: "5-hour", usedRatio: 0.85, resetAt: reset(1) }], t, undefined, NOW).state).toBe("approaching")
    const land = SessionLimit.classify(
      [
        { id: "5h", label: "5-hour", usedRatio: 0.99, resetAt: reset(1) },
        { id: "7d", label: "Weekly", usedRatio: 0.96, resetAt: reset(50) },
      ],
      t,
      undefined,
      NOW,
    )
    expect(land).toMatchObject({ state: "land", window: { id: "7d" } })
    // Without a reset time there is nothing to resume at: it stays a warning.
    expect(SessionLimit.classify([{ id: "credits", label: "Credits", usedRatio: 0.99 }], t, undefined, NOW).state).toBe("approaching")
  })

  test("per-model weekly windows only bind their model family", () => {
    const windows = [{ id: "7d-opus", label: "Weekly Opus", usedRatio: 0.99, resetAt: reset(20) }]
    expect(SessionLimit.classify(windows, t, "claude-opus-5-5", NOW).state).toBe("land")
    expect(SessionLimit.classify(windows, t, "claude-sonnet-5-5", NOW).state).toBe("ok")
  })
})
