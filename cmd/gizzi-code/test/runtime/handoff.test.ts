import { afterEach, describe, expect, test } from "bun:test"
import {
  clearHandoffCache,
  consumeHandoff,
  frameHandoff,
  handoffTarget,
  headerHandoff,
  isRefusal,
} from "@/runtime/providers/handoff"
import { tapRetryHint } from "@/runtime/providers/retry-hint"

// The chunk cloud-api's `handoff_sse_event` sends (services/open_lane.rs).
const HANDOFF_CHUNK =
  'data: {"id":"handoff-1","object":"chat.completion.chunk","created":1,"model":"deepseek-v4.1-flash-open","choices":[],"allternit":{"handoff":{"from":"claude-sonnet-4.6","to":"deepseek-v4.1-flash-open"}}}\n\n'
const TEXT_CHUNK = 'data: {"choices":[{"index":0,"delta":{"content":"Here is the answer."}}]}\n\n'

function streamResponse(body: string, headers: Record<string, string> = {}): Response {
  return new Response(
    new ReadableStream({
      start(c) {
        c.enqueue(new TextEncoder().encode(body))
        c.close()
      },
    }),
    { status: 200, headers: { "content-type": "text/event-stream", ...headers } },
  )
}

function tap(res: Response, sessionID = "ses_handoff") {
  return tapRetryHint({
    response: res,
    method: "POST",
    url: "https://api.allternit.com/v1/chat/completions",
    sessionID,
    npm: "@ai-sdk/openai-compatible",
  })
}

afterEach(() => clearHandoffCache())

describe("cloud handoff marker", () => {
  test("a streamed handoff chunk is stripped and recorded once", async () => {
    const body = await new Response(tap(streamResponse(HANDOFF_CHUNK + TEXT_CHUNK + "data: [DONE]\n\n")).body).text()
    expect(body).toBe(TEXT_CHUNK + "data: [DONE]\n\n")
    expect(consumeHandoff("ses_handoff")).toEqual({ from: "claude-sonnet-4.6", to: "deepseek-v4.1-flash-open" })
    expect(consumeHandoff("ses_handoff")).toBeUndefined()
  })

  test("ordinary chunks and other allternit frames pass through", () => {
    expect(frameHandoff(TEXT_CHUNK.trim())).toBeNull()
    expect(frameHandoff('data: {"allternit":{"note":"x"}}')).toBeNull()
    expect(frameHandoff('event: allternit.retry_hint\ndata: {"allternit":{"handoff":{"from":"a","to":"b"}}}')).toBeNull()
  })

  test("the non-streamed header is read", () => {
    expect(headerHandoff("claude-sonnet-4.6->glm-5.3-flash-open")).toEqual({
      from: "claude-sonnet-4.6",
      to: "glm-5.3-flash-open",
    })
    expect(headerHandoff("handoff")).toBeNull()
    expect(headerHandoff(null)).toBeNull()
    const json = new Response("{}", { headers: { "content-type": "application/json", "x-allternit-handoff": "a->b" } })
    tap(json, "ses_json")
    expect(consumeHandoff("ses_json")).toEqual({ from: "a", to: "b" })
  })
})

describe("isRefusal (same rules as cloud-api services/refusal.rs)", () => {
  test("short refusals", () => {
    expect(isRefusal("I'm sorry, but I can't help with that.")).toBe(true)
    expect(isRefusal("I can’t assist with that request.")).toBe(true)
    expect(isRefusal("", "content_filter")).toBe(true)
  })
  test("answers and partial help are not refusals", () => {
    expect(isRefusal("Here is the script you asked for.")).toBe(false)
    expect(isRefusal("I can't browse the web, but here is what I know: ...")).toBe(false)
    expect(isRefusal("Sure. " + "x".repeat(100) + " I cannot verify this.")).toBe(false)
    expect(isRefusal("I can't " + "x".repeat(1300))).toBe(false)
  })
})

describe("handoffTarget", () => {
  const originalFetch = globalThis.fetch
  const originalKey = process.env.ALLTERNIT_API_KEY
  afterEach(() => {
    globalThis.fetch = originalFetch
    if (originalKey === undefined) delete process.env.ALLTERNIT_API_KEY
    else process.env.ALLTERNIT_API_KEY = originalKey
  })

  function settings(body: object) {
    globalThis.fetch = (async () => new Response(JSON.stringify(body), { status: 200 })) as unknown as typeof fetch
  }

  test("no Allternit Cloud sign-in, no handoff", async () => {
    delete process.env.ALLTERNIT_API_KEY
    delete process.env.ALLTERNIT_API_TOKEN
    expect(await handoffTarget()).toBeUndefined()
  })

  test("needs the open lane serving, the terms and the toggle", async () => {
    process.env.ALLTERNIT_API_KEY = "ak_test"
    settings({ available: true, terms_accepted: true, handoff_enabled: false, default_handoff_model: "glm-5.3-flash-open" })
    expect(await handoffTarget()).toBeUndefined()
    clearHandoffCache()
    settings({ available: true, terms_accepted: true, handoff_enabled: true, default_handoff_model: "glm-5.3-flash-open" })
    expect(await handoffTarget()).toEqual({ providerID: "allternit", modelID: "glm-5.3-flash-open" })
  })
})
