import { afterAll, beforeEach, describe, expect, test } from "bun:test"

process.env.ALLTERNIT_API_TOKEN = "test-token"
const { openSyncStream, abortSession, replyPermission, replyQuestion, rejectQuestion } = await import(
  "../../../src/runtime/bots/bot-chat-client"
)
const { PlatformApiError } = await import("../../../src/runtime/bots/platform-api")

type Call = { method: string; path: string; body?: any; accept?: string }
let calls: Call[] = []
let respond: (call: Call) => Response
const realFetch = globalThis.fetch

beforeEach(() => {
  calls = []
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const call = {
      method: init.method ?? "GET",
      path: `${url.pathname}${url.search}`,
      body: init.body ? JSON.parse(String(init.body)) : undefined,
      accept: (init.headers as any)?.Accept,
    }
    calls.push(call)
    return respond(call)
  }) as typeof fetch
})
afterAll(() => {
  globalThis.fetch = realFetch
})

const sse = (body: string) => new Response(body, { status: 200, headers: { "Content-Type": "text/event-stream" } })

describe("openSyncStream", () => {
  test("delivers events, then reconnects from the last id when the stream ends", async () => {
    const stop = new AbortController()
    const events: string[] = []
    const statuses: string[] = []
    const bodies = ['id: 1\ndata: {"type":"a"}\n\n', 'id: 2\ndata: {"type":"b"}\n\n']
    respond = () => {
      const body = bodies.shift()
      if (body === undefined) {
        stop.abort()
        return new Response("", { status: 502 })
      }
      return sse(body)
    }
    await openSyncStream({
      signal: stop.signal,
      onEvent: (e) => events.push(e.type),
      onStatus: (s) => statuses.push(s),
      sleep: async () => {},
    })
    expect(events).toEqual(["a", "b"])
    expect(calls.map((c) => c.path)).toEqual([
      "/api/v1/agent-sessions/sync",
      "/api/v1/agent-sessions/sync?since=1",
      "/api/v1/agent-sessions/sync?since=2",
    ])
    expect(calls[0]!.accept).toBe("text/event-stream")
    expect(statuses).toEqual(["open", "open"])
  })

  test("retries a failed connection with growing backoff", async () => {
    const stop = new AbortController()
    const waits: number[] = []
    respond = () => new Response("down", { status: 502 })
    await openSyncStream({
      signal: stop.signal,
      onEvent: () => {},
      sleep: async (ms) => {
        waits.push(ms)
        if (waits.length === 3) stop.abort()
      },
    })
    expect(waits).toEqual([500, 1000, 2000])
  })

  test("a rejected credential is final", async () => {
    respond = () => new Response("no", { status: 401 })
    const error = await openSyncStream({ signal: new AbortController().signal, onEvent: () => {}, sleep: async () => {} }).catch(
      (e) => e,
    )
    expect(error).toBeInstanceOf(PlatformApiError)
    expect(calls).toHaveLength(1)
  })
})

describe("turn actions", () => {
  test("abort, permission and question replies hit the gateway routes", async () => {
    respond = () => new Response(JSON.stringify({ success: true }), { status: 200 })
    await abortSession("ses/1")
    await replyPermission("per_1", "always")
    await replyPermission("per_2", "reject", "not that file")
    await replyQuestion("que_1", [["A"], ["B", "C"]])
    await rejectQuestion("que_2")
    expect(calls.map((c) => [c.method, c.path, c.body])).toEqual([
      ["POST", "/api/v1/agent-sessions/ses%2F1/abort", {}],
      ["POST", "/api/v1/permissions/per_1/reply", { reply: "always" }],
      ["POST", "/api/v1/permissions/per_2/reply", { reply: "reject", message: "not that file" }],
      ["POST", "/api/v1/questions/que_1/reply", { answers: [["A"], ["B", "C"]] }],
      ["POST", "/api/v1/questions/que_2/reject", {}],
    ])
  })
})
