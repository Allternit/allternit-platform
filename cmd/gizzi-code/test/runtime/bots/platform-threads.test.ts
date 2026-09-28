import { afterAll, beforeEach, describe, expect, test } from "bun:test"

process.env.ALLTERNIT_API_TOKEN = "test-token"
const {
  ensureStandingThread,
  createIncognitoThread,
  followThread,
  parseModelRef,
  sendThreadTurn,
  turnContextTokens,
} = await import("../../../src/runtime/bots/platform-threads")
const { platformToken } = await import("../../../src/runtime/bots/platform-api")

type Call = { method: string; path: string; body?: any; auth?: string | null }
let calls: Call[] = []
let routes: Record<string, (body: any) => unknown> = {}
const realFetch = globalThis.fetch

beforeEach(() => {
  calls = []
  routes = {}
  globalThis.fetch = (async (input: string, init: RequestInit = {}) => {
    const url = new URL(input)
    const key = `${init.method ?? "GET"} ${url.pathname}${url.search}`
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ method: init.method ?? "GET", path: `${url.pathname}${url.search}`, body, auth: (init.headers as any)?.Authorization })
    const handler = routes[key]
    if (!handler) return new Response(JSON.stringify({ error: "nope", message: `no route ${key}` }), { status: 404 })
    return new Response(JSON.stringify(handler(body)), { status: 200 })
  }) as typeof fetch
})
afterAll(() => {
  globalThis.fetch = realFetch
})

const thread = (over: Record<string, unknown> = {}) => ({
  id: "t1",
  botId: "bot-1",
  projectId: null,
  kind: "standing",
  incognito: false,
  title: "Scout",
  status: "idle",
  currentSessionId: "s1",
  generation: 1,
  contextUsed: null,
  lastActivityAt: "2026-09-27T10:00:00Z",
  createdAt: "2026-09-27T09:00:00Z",
  ...over,
})

describe("platform threads", () => {
  test("uses ALLTERNIT_API_TOKEN as the bearer", async () => {
    expect(await platformToken()).toBe("test-token")
  })

  test("the standing thread is the most recently active one", async () => {
    routes["GET /api/v1/threads?botId=bot-1"] = () => ({
      threads: [
        thread({ id: "old", lastActivityAt: "2026-09-01T00:00:00Z" }),
        thread({ id: "task", kind: "task", lastActivityAt: "2026-09-30T00:00:00Z" }),
        thread({ id: "new", lastActivityAt: "2026-09-27T00:00:00Z" }),
      ],
    })
    expect((await ensureStandingThread("bot-1", "Scout")).id).toBe("new")
    expect(calls[0]!.auth).toBe("Bearer test-token")
  })

  test("creates a standing thread when the bot has none", async () => {
    routes["GET /api/v1/threads?botId=bot-1"] = () => ({ threads: [] })
    routes["POST /api/v1/threads"] = body => ({ thread: thread({ id: "made", ...body }) })
    const t = await ensureStandingThread("bot-1", "Scout")
    expect(t.id).toBe("made")
    expect(calls[1]!.body).toEqual({ botId: "bot-1", title: "Scout", kind: "standing", createdBy: "user" })
  })

  test("incognito asks are incognito threads", async () => {
    routes["POST /api/v1/threads"] = body => ({ thread: thread({ id: "inc", ...body }) })
    await createIncognitoThread("bot-1", "Incognito ask")
    expect(calls[0]!.body).toMatchObject({ botId: "bot-1", incognito: true, kind: "task" })
  })

  test("a turn posts to the current session as the bot's model", async () => {
    routes["POST /api/v1/agent-sessions/s1/messages"] = () => ({
      id: "m2",
      role: "assistant",
      content: "Done.",
      metadata: { telemetry: { usage: { inputTokens: 900, outputTokens: 100, cacheReadTokens: 1000 } } },
    })
    const reply = await sendThreadTurn(thread() as never, "hi", { model: { providerID: "anthropic", modelID: "claude-sonnet-5" } })
    expect(reply.content).toBe("Done.")
    expect(calls[0]!.body).toEqual({ text: "hi", source: "terminal", metadata: { model: { providerID: "anthropic", modelID: "claude-sonnet-5" } } })
    expect(turnContextTokens(reply)).toBe(2000)
  })

  test("after a turn, a full window hands off and the caller follows the new session", async () => {
    routes["POST /api/v1/threads/t1/usage"] = () => ({ contextUsed: 0.8, shouldHandoff: true, reason: "budget" })
    routes["POST /api/v1/threads/t1/handoff"] = () => ({ thread: thread({ currentSessionId: "s2", generation: 2 }) })
    const next = await followThread(thread() as never, { tokensUsed: 160_000, model: "anthropic/claude-sonnet-5" })
    expect(next.currentSessionId).toBe("s2")
    expect(calls.map(c => c.path)).toEqual(["/api/v1/threads/t1/usage", "/api/v1/threads/t1/handoff"])
  })

  test("when the server already handed off, just re-read the thread", async () => {
    routes["POST /api/v1/threads/t1/usage"] = () => ({ contextUsed: 0.1, shouldHandoff: false, reason: null, handedOff: true })
    routes["GET /api/v1/threads/t1"] = () => ({ thread: thread({ currentSessionId: "s3" }) })
    expect((await followThread(thread() as never, { tokensUsed: 10 })).currentSessionId).toBe("s3")
  })

  test("a failed follow keeps the thread as it was", async () => {
    expect((await followThread(thread() as never, { tokensUsed: 10 })).currentSessionId).toBe("s1")
  })

  test("parseModelRef", () => {
    expect(parseModelRef("openrouter/qwen/qwen3")).toEqual({ providerID: "openrouter", modelID: "qwen/qwen3" })
    expect(parseModelRef("bare")).toBeUndefined()
    expect(parseModelRef(null)).toBeUndefined()
  })
})
