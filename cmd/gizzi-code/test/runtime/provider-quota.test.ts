import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  ProviderQuotas,
  parseKimiUsages,
  parseOpenRouterCredits,
  parseOpenRouterKeyLimit,
} from "../../src/runtime/providers/quota/index"

/**
 * Network is fully mocked in this suite — every fetch is a stub installed on
 * globalThis.fetch and restored after each test. No provider is ever called.
 */

const originalFetch = globalThis.fetch
const savedEnv: Record<string, string | undefined> = {}

function setEnv(name: string, value: string | undefined) {
  if (!(name in savedEnv)) savedEnv[name] = process.env[name]
  if (value === undefined) delete process.env[name]
  else process.env[name] = value
}

function mockFetch(handler: (url: string) => { status: number; body?: unknown }) {
  globalThis.fetch = (async (input: any) => {
    const url = typeof input === "string" ? input : input.url
    const { status, body } = handler(url)
    return new Response(body === undefined ? "" : JSON.stringify(body), { status })
  }) as typeof fetch
}

beforeEach(() => {
  ProviderQuotas.clearCache()
})

afterEach(() => {
  globalThis.fetch = originalFetch
  for (const [name, value] of Object.entries(savedEnv)) {
    if (value === undefined) delete process.env[name]
    else process.env[name] = value
    delete savedEnv[name]
  }
  ProviderQuotas.clearCache()
})

describe("parseKimiUsages", () => {
  test("reads 5h/7d/month windows with reset times", () => {
    const windows = parseKimiUsages({
      usages: {
        limit_5h: { used_ratio: 0.62, reset_time: "2026-09-26T20:00:00Z" },
        limit_7d: { used_ratio: "0.8" },
        limit_month_total: { used_ratio: 0.1 },
      },
    })
    expect(windows).toHaveLength(3)
    expect(windows[0]).toEqual({ id: "5h", label: "5-hour", usedRatio: 0.62, resetAt: "2026-09-26T20:00:00Z" })
    expect(windows[1]).toEqual({ id: "7d", label: "Weekly", usedRatio: 0.8 })
    expect(windows[2]).toEqual({ id: "month", label: "Monthly", usedRatio: 0.1 })
  })

  test("skips windows without a usable ratio", () => {
    expect(parseKimiUsages({ usages: { limit_5h: {} } })).toEqual([])
    expect(parseKimiUsages({})).toEqual([])
    expect(parseKimiUsages(undefined)).toEqual([])
  })

  test("clamps out-of-range ratios", () => {
    const windows = parseKimiUsages({ usages: { limit_5h: { used_ratio: 1.7 } } })
    expect(windows[0]!.usedRatio).toBe(1)
  })
})

describe("parseOpenRouterCredits", () => {
  test("ratio is usage over total credits, clamped to 0..1", () => {
    const windows = parseOpenRouterCredits({ data: { total_credits: 100.5, total_usage: 25.75 } })
    expect(windows).toHaveLength(1)
    expect(windows[0]!.id).toBe("credits")
    expect(windows[0]!.usedRatio).toBeCloseTo(25.75 / 100.5, 6)
    const over = parseOpenRouterCredits({ data: { total_credits: 10, total_usage: 12 } })
    expect(over[0]!.usedRatio).toBe(1)
  })

  test("no window when credits are zero or fields missing", () => {
    expect(parseOpenRouterCredits({ data: { total_credits: 0, total_usage: 0 } })).toEqual([])
    expect(parseOpenRouterCredits({ data: { total_usage: 5 } })).toEqual([])
    expect(parseOpenRouterCredits({})).toEqual([])
    expect(parseOpenRouterCredits(undefined)).toEqual([])
  })
})

describe("parseOpenRouterKeyLimit", () => {
  test("ratio is per-key usage over the spend cap", () => {
    const windows = parseOpenRouterKeyLimit({ data: { usage: 4, limit: 10 } })
    expect(windows).toEqual([{ id: "key-limit", label: "Key credit limit", usedRatio: 0.4 }])
  })

  test("no window for unlimited (null limit) keys", () => {
    expect(parseOpenRouterKeyLimit({ data: { usage: 4, limit: null, is_free_tier: false } })).toEqual([])
    expect(parseOpenRouterKeyLimit(undefined)).toEqual([])
  })
})

describe("ProviderQuotas openrouter fetcher", () => {
  test("signed-out when OPENROUTER_API_KEY is not set — no network", async () => {
    setEnv("OPENROUTER_API_KEY", undefined)
    mockFetch(() => {
      throw new Error("network must not be called")
    })
    const result = await ProviderQuotas.get("openrouter")
    expect(result.status).toBe("signed-out")
    expect((result as any).message).toContain("OPENROUTER_API_KEY")
  })

  test("ok with a credits window from the documented /credits response", async () => {
    setEnv("OPENROUTER_API_KEY", "sk-or-test")
    let urls: string[] = []
    mockFetch((url) => {
      urls.push(url)
      return { status: 200, body: { data: { total_credits: 100, total_usage: 40 } } }
    })
    const result = await ProviderQuotas.get("openrouter")
    expect(result.status).toBe("ok")
    if (result.status !== "ok") return
    expect(result.quota.providerID).toBe("openrouter")
    expect(result.quota.windows).toEqual([{ id: "credits", label: "Credits", usedRatio: 0.4 }])
    expect(urls).toEqual(["https://openrouter.ai/api/v1/credits"])
  })

  test("falls back to the per-key spend cap when there are no credits", async () => {
    setEnv("OPENROUTER_API_KEY", "sk-or-test")
    mockFetch((url) =>
      url.endsWith("/credits")
        ? { status: 200, body: { data: { total_credits: 0, total_usage: 0 } } }
        : { status: 200, body: { data: { usage: 3, limit: 12, is_free_tier: true } } },
    )
    const result = await ProviderQuotas.get("openrouter")
    expect(result.status).toBe("ok")
    if (result.status !== "ok") return
    expect(result.quota.windows).toEqual([{ id: "key-limit", label: "Key credit limit", usedRatio: 0.25 }])
  })

  test("rejected key and server errors map to honest statuses", async () => {
    setEnv("OPENROUTER_API_KEY", "sk-or-test")
    mockFetch(() => ({ status: 401 }))
    const rejected = await ProviderQuotas.get("openrouter")
    expect(rejected.status).toBe("signed-out")

    ProviderQuotas.clearCache()
    mockFetch(() => ({ status: 500 }))
    const failed = await ProviderQuotas.get("openrouter")
    expect(failed.status).toBe("error")
    expect((failed as any).message).toContain("500")
  })

  test("network failure maps to error, never a fabricated window", async () => {
    setEnv("OPENROUTER_API_KEY", "sk-or-test")
    mockFetch(() => {
      throw new Error("offline")
    })
    const result = await ProviderQuotas.get("openrouter")
    expect(result.status).toBe("error")
  })

  test("repeat reads are served from the 60s cache (one network call)", async () => {
    setEnv("OPENROUTER_API_KEY", "sk-or-test")
    let calls = 0
    mockFetch(() => {
      calls++
      return { status: 200, body: { data: { total_credits: 100, total_usage: 10 } } }
    })
    await ProviderQuotas.get("openrouter")
    await ProviderQuotas.get("openrouter")
    expect(calls).toBe(1)
  })
})

describe("ProviderQuotas registry", () => {
  test("only providers with a real fetcher are listed as supported", () => {
    const supported = ProviderQuotas.supported()
    expect(supported).toContain("kimi-cli")
    expect(supported).toContain("openrouter")
    expect(supported).toContain("codex-cli")
    expect(supported).toContain("claude-cli")
    expect(supported).not.toContain("anthropic")
    expect(supported).not.toContain("openai")
  })

  test("providers without a fetcher report unsupported, never a guess", async () => {
    const result = await ProviderQuotas.get("anthropic")
    expect(result.status).toBe("unsupported")
  })
})
