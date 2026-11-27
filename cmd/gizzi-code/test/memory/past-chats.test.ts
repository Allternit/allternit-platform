import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { PastChats, type RawRequest } from "../../src/runtime/memory/past-chats"

const turn = (n: number, speaker = "user") => ({
  observation_id: `obs_${n}`,
  session_id: `ses_old${n}`,
  speaker,
  timestamp: `2026-11-${10 + n}T14:05:00Z`,
  text: `we picked bge for reranking ${n}`,
})

function fake(turns: unknown[], calls: { path: string; body: any }[] = []): RawRequest {
  return async (_m, path, body) => {
    calls.push({ path, body })
    return { turns }
  }
}

beforeEach(() => {
  delete process.env.GIZZI_MEMORY_DRIVE
  process.env.GIZZI_MEMORY_KERNEL = "1"
  PastChats.resetForTests()
})
afterEach(() => {
  delete process.env.GIZZI_MEMORY_KERNEL
})

describe("PastChats.build", () => {
  test("normal turn: dated, role-labelled block; current session excluded; refs are past_chat", async () => {
    const calls: { path: string; body: any }[] = []
    const out = await PastChats.build({
      sessionID: "ses_now",
      text: "which reranker did we pick",
      request: fake([turn(1, "user"), turn(2, "assistant")], calls),
    })
    expect(out!.block).toContain("<past-chats")
    expect(out!.block).toContain("[2026-11-11 · user] we picked bge for reranking 1")
    expect(out!.block).toContain("[2026-11-12 · assistant]")
    expect(calls[0].path).toBe("/api/v1/memory/v2/raw-search")
    expect(calls[0].body.mode).toBe("auto")
    expect(calls[0].body.exclude_sessions).toEqual(["ses_now"])
    expect(calls[0].body.calling_session).toBe("ses_now")
    expect(out!.refs.map((r) => r.source)).toEqual(["past_chat", "past_chat"])
    expect(out!.refs[0]).toMatchObject({ id: "obs_1", session_id: "ses_old1", date: "2026-11-11T14:05:00Z" })
  })

  test("at most 4 refs", async () => {
    const out = await PastChats.build({ sessionID: "s", text: "one two three four", request: fake([1, 2, 3, 4, 5, 6].map((n) => turn(n))) })
    expect(out!.refs).toHaveLength(4)
  })

  test("no block for incognito, memory_search false, short query, no results, drive off", async () => {
    const base = { sessionID: "s", text: "which reranker did we pick", request: fake([turn(1)]) }
    expect(await PastChats.build({ ...base, incognito: true })).toBeUndefined()
    expect(await PastChats.build({ ...base, memorySearch: false })).toBeUndefined()
    expect(await PastChats.build({ ...base, text: "hello there" })).toBeUndefined()
    expect(await PastChats.build({ ...base, request: fake([]) })).toBeUndefined()
    process.env.GIZZI_MEMORY_DRIVE = "0"
    expect(await PastChats.build(base)).toBeUndefined()
  })

  test("a slow route sends no block and never throws", async () => {
    const slow: RawRequest = () => new Promise(() => {})
    const t0 = Date.now()
    const out = await PastChats.build({ sessionID: "s", text: "which reranker did we pick", request: slow, timeoutMs: 50 })
    expect(out).toBeUndefined()
    expect(Date.now() - t0).toBeLessThan(500)
    const boom: RawRequest = async () => {
      throw new Error("503")
    }
    expect(await PastChats.build({ sessionID: "s", text: "which reranker did we pick", request: boom })).toBeUndefined()
  })

  test("one lookup per user message; refs readable for the processor", async () => {
    const calls: { path: string; body: any }[] = []
    const input = { sessionID: "s", text: "which reranker did we pick", request: fake([turn(1)], calls) }
    await PastChats.forUserMessage("msg_u1", input)
    await PastChats.forUserMessage("msg_u1", input)
    expect(calls).toHaveLength(1)
    expect((await PastChats.refsFor("msg_u1"))[0].id).toBe("obs_1")
    expect(await PastChats.refsFor("msg_other")).toEqual([])
  })
})
