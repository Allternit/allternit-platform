import { afterEach, describe, expect, test } from "bun:test"
import {
  cloudInferenceToken,
  discoverAllternitCloud,
  getCachedAllternitPlan,
  isPaidAllternitPlan,
  refreshAllternitPlan,
} from "../../src/runtime/providers/discovery/allternit-cloud"

const originalFetch = globalThis.fetch

afterEach(() => {
  globalThis.fetch = originalFetch
  delete process.env.ALLTERNIT_API_KEY
  delete process.env.ALLTERNIT_API_TOKEN
})

describe("discoverAllternitCloud", () => {
  test("maps /v1/models into an allternit provider", async () => {
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.endsWith("/v1/models")) {
        return new Response(
          JSON.stringify({
            object: "list",
            data: [
              { id: "llama-3.1-8b", name: "Llama 3.1 8B", owned_by: "allternit" },
              { id: "qwen2.5-7b", extra: { name: "Qwen 2.5 7B", context_length: 32000 } },
            ],
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        )
      }
      return new Response("not found", { status: 404 })
    }) as typeof fetch

    const providers = await discoverAllternitCloud()
    expect(providers).toHaveLength(1)
    expect(providers[0]?.id).toBe("allternit")
    expect(providers[0]?.source).toBe("platform")
    expect(providers[0]?.models.map((m) => m.id)).toEqual(["llama-3.1-8b", "qwen2.5-7b"])
    expect(providers[0]?.models[1]?.context).toBe(32000)
  })
})

describe("refreshAllternitPlan", () => {
  test("caches Free/Plus/Super/Ultra from billing/subscription", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_test"
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.endsWith("/api/v1/billing/subscription")) {
        return new Response(
          JSON.stringify({ plan_id: "plus", label: "Plus", plan_tier: "pro", status: "active" }),
          { status: 200, headers: { "content-type": "application/json" } },
        )
      }
      return new Response("not found", { status: 404 })
    }) as typeof fetch

    const plan = await refreshAllternitPlan()
    expect(plan).toEqual({ id: "plus", label: "Plus", plan_tier: "pro", status: "active" })
    expect(getCachedAllternitPlan()?.label).toBe("Plus")
  })

  test("returns null without an Allternit key", async () => {
    const plan = await refreshAllternitPlan()
    expect(plan).toBeNull()
  })
})

describe("isPaidAllternitPlan", () => {
  test("Plus/Super/Ultra active are paid; Free and canceled are not", () => {
    expect(isPaidAllternitPlan({ id: "plus", label: "Plus", plan_tier: "pro", status: "active" })).toBe(true)
    expect(isPaidAllternitPlan({ id: "free", label: "Free", plan_tier: "free", status: "none" })).toBe(false)
    expect(isPaidAllternitPlan({ id: "plus", label: "Plus", plan_tier: "pro", status: "canceled" })).toBe(false)
  })
})

describe("cloudInferenceToken", () => {
  // Regression: Desktop and cloud computers pass the device token as
  // ALLTERNIT_API_TOKEN; the chat provider read only ALLTERNIT_API_KEY, so
  // every Allternit Cloud chat there went out unsigned ("Unauthorized").
  test("uses the device token Desktop passes as ALLTERNIT_API_TOKEN", async () => {
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_device"
    expect(await cloudInferenceToken()).toBe("allternit_runtime_device")
  })

  test("prefers an explicit ALLTERNIT_API_KEY", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    process.env.ALLTERNIT_API_TOKEN = "allternit_runtime_device"
    expect(await cloudInferenceToken()).toBe("alt_key")
  })
})
