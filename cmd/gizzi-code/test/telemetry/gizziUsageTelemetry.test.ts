// @ts-nocheck
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import {
  buildSessionSummary,
  countLinesAccepted,
  flushGizziUsageTelemetry,
  isGizziUsageTelemetryEnabled,
  postSessionSummary,
  recordAcceptedEdit,
  recordRejectedToolCall,
  resetGizziUsageTelemetryForTests,
} from "../../src/runtime/services/telemetry/gizziUsageTelemetry.ts"

const TELEMETRY_ENV_VARS = [
  "GIZZI_TELEMETRY",
  "GIZZI_DISABLE_TELEMETRY",
  "DISABLE_TELEMETRY",
  "GIZZI_DISABLE_NONESSENTIAL_TRAFFIC",
  "DO_NOT_TRACK",
  "ALLTERNIT_API_URL",
  "ALLTERNIT_API_BASE_URL",
  "ALLTERNIT_API_TOKEN",
  "ALLTERNIT_USER_ID",
  "NODE_ENV",
]

const SUMMARY_DEPS = {
  sessionId: "sess-test-1",
  models: ["claude-sonnet-4-6"],
  primaryModel: "claude-sonnet-4-6",
  promptTokens: 1234,
  completionTokens: 567,
  costMicrodollars: 890_000,
}

describe("gizziUsageTelemetry.ts", () => {
  const realFetch = globalThis.fetch
  let savedEnv: Record<string, string | undefined>

  beforeEach(() => {
    savedEnv = {}
    for (const key of TELEMETRY_ENV_VARS) savedEnv[key] = process.env[key]
    resetGizziUsageTelemetryForTests()
  })

  afterEach(() => {
    globalThis.fetch = realFetch
    for (const key of TELEMETRY_ENV_VARS) {
      if (savedEnv[key] === undefined) delete process.env[key]
      else process.env[key] = savedEnv[key]
    }
    resetGizziUsageTelemetryForTests()
  })

  function enableTelemetry() {
    process.env.GIZZI_TELEMETRY = "1"
    process.env.ALLTERNIT_API_URL = "http://127.0.0.1:9/telem"
    delete process.env.GIZZI_DISABLE_TELEMETRY
    delete process.env.DISABLE_TELEMETRY
    delete process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC
  }

  function captureFetch() {
    const calls: Array<{ url: string; init: RequestInit }> = []
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url, init: init ?? {} })
      return new Response("{}", { status: 200 })
    }) as typeof fetch
    return calls
  }

  describe("enablement", () => {
    test("disabled by default (no posts)", async () => {
      delete process.env.GIZZI_TELEMETRY
      const calls = captureFetch()
      recordAcceptedEdit(10)
      await flushGizziUsageTelemetry(SUMMARY_DEPS)
      expect(calls.length).toBe(0)
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
    })

    test("disabled when GIZZI_TELEMETRY is the upstream kill switch value", () => {
      process.env.GIZZI_TELEMETRY = "off"
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
      process.env.GIZZI_TELEMETRY = "0"
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
    })

    test("disabled when global telemetry opt-outs are set", () => {
      enableTelemetry()
      expect(isGizziUsageTelemetryEnabled()).toBe(true)
      process.env.GIZZI_DISABLE_TELEMETRY = "1"
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
      delete process.env.GIZZI_DISABLE_TELEMETRY
      process.env.GIZZI_DISABLE_NONESSENTIAL_TRAFFIC = "1"
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
    })

    test("disabled in production without an explicit API URL", () => {
      process.env.GIZZI_TELEMETRY = "1"
      delete process.env.ALLTERNIT_API_URL
      delete process.env.ALLTERNIT_API_BASE_URL
      process.env.NODE_ENV = "production"
      expect(isGizziUsageTelemetryEnabled()).toBe(false)
    })

    test("enabled with GIZZI_TELEMETRY=1 and an explicit URL", () => {
      enableTelemetry()
      expect(isGizziUsageTelemetryEnabled()).toBe(true)
    })
  })

  describe("lines-accepted counting", () => {
    test("counts + lines across hunks", () => {
      const patch = [
        { lines: [" context", "-old", "+new", "+added2"] },
        { lines: ["+another", " context2"] },
      ]
      expect(countLinesAccepted(patch)).toBe(3)
    })

    test("new-file content counts all lines", () => {
      expect(countLinesAccepted([], "a\nb\nc")).toBe(3)
    })

    test("pure deletions count zero", () => {
      const patch = [{ lines: ["-gone1", "-gone2", " ctx"] }]
      expect(countLinesAccepted(patch)).toBe(0)
    })

    test("recordAcceptedEdit accumulates counters only when enabled", () => {
      delete process.env.GIZZI_TELEMETRY
      recordAcceptedEdit(5)
      expect(
        buildSessionSummary(SUMMARY_DEPS).lines_accepted,
      ).toBe(0)

      enableTelemetry()
      recordAcceptedEdit(5)
      recordAcceptedEdit(7)
      recordAcceptedEdit(-3) // clamped to 0
      const summary = buildSessionSummary(SUMMARY_DEPS)
      expect(summary.lines_accepted).toBe(12)
      expect(summary.tool_calls_accepted).toBe(3)
    })

    test("recordRejectedToolCall accumulates rejections", () => {
      enableTelemetry()
      recordRejectedToolCall()
      recordRejectedToolCall()
      expect(buildSessionSummary(SUMMARY_DEPS).tool_calls_rejected).toBe(2)
    })
  })

  describe("flush", () => {
    test("enabled flush posts a session summary with lines count", async () => {
      enableTelemetry()
      const calls = captureFetch()
      recordAcceptedEdit(42)
      recordRejectedToolCall()
      await flushGizziUsageTelemetry(SUMMARY_DEPS)

      expect(calls.length).toBe(1)
      expect(calls[0].url).toBe(
        "http://127.0.0.1:9/telem/api/v1/analytics/gizzi-code/events",
      )
      expect(calls[0].init.method).toBe("POST")
      const payload = JSON.parse(calls[0].init.body as string)
      expect(payload.events.length).toBe(1)
      const event = payload.events[0]
      expect(event.session_id).toBe("sess-test-1")
      expect(event.event_type).toBe("session")
      expect(event.model).toBe("claude-sonnet-4-6")
      expect(event.prompt_tokens).toBe(1234)
      expect(event.completion_tokens).toBe(567)
      expect(event.cost_microdollars).toBe(890_000)
      expect(event.lines_accepted).toBe(42)
      expect(event.tool_calls_accepted).toBe(1)
      expect(event.tool_calls_rejected).toBe(1)
      // No PII: no file paths or prompt content fields.
      expect(JSON.stringify(event)).not.toContain("/Users/")
      expect(event.metadata.models_used).toEqual(["claude-sonnet-4-6"])
    })

    test("skips the post when nothing was recorded", async () => {
      enableTelemetry()
      const calls = captureFetch()
      await flushGizziUsageTelemetry(SUMMARY_DEPS)
      expect(calls.length).toBe(0)
    })

    test("flush is idempotent (one post even when called twice)", async () => {
      enableTelemetry()
      const calls = captureFetch()
      recordAcceptedEdit(1)
      await flushGizziUsageTelemetry(SUMMARY_DEPS)
      await flushGizziUsageTelemetry(SUMMARY_DEPS)
      expect(calls.length).toBe(1)
    })

    test("HTTP failure does not throw and does not retry more than once", async () => {
      enableTelemetry()
      let attempts = 0
      globalThis.fetch = (async () => {
        attempts += 1
        return new Response("boom", { status: 500 })
      }) as typeof fetch
      recordAcceptedEdit(3)
      await expect(
        flushGizziUsageTelemetry(SUMMARY_DEPS),
      ).resolves.toBeUndefined()
      expect(attempts).toBe(2) // initial + one retry
    })

    test("network error does not throw", async () => {
      enableTelemetry()
      globalThis.fetch = (async () => {
        throw new Error("connection refused")
      }) as typeof fetch
      recordAcceptedEdit(3)
      await expect(
        flushGizziUsageTelemetry(SUMMARY_DEPS),
      ).resolves.toBeUndefined()
    })
  })

  describe("postSessionSummary", () => {
    test("sends bearer token and user id headers when configured", async () => {
      process.env.ALLTERNIT_API_TOKEN = "tok-123"
      process.env.ALLTERNIT_USER_ID = "user-9"
      const seen: Array<{ url: string; init: RequestInit }> = []
      const fetchImpl = (async (url: string, init?: RequestInit) => {
        seen.push({ url, init: init ?? {} })
        return new Response("{}", { status: 200 })
      }) as typeof fetch
      const ok = await postSessionSummary(
        "http://127.0.0.1:9",
        { session_id: "s" },
        fetchImpl,
      )
      expect(ok).toBe(true)
      const headers = seen[0].init.headers as Record<string, string>
      expect(headers.Authorization).toBe("Bearer tok-123")
      expect(headers["x-allternit-user-id"]).toBe("user-9")
    })
  })
})
