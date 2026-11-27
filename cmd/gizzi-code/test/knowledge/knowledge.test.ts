// The network edge is a fake fetch on globalThis; everything else is real.
import { afterEach, describe, expect, test } from "bun:test"
import { KnowledgeFlagTool } from "../../src/runtime/tools/builtins/knowledge-flag"
import { formatList, formatProposal, runKnowledge } from "../../src/runtime/knowledge/client"

const realFetch = globalThis.fetch
process.env.ALLTERNIT_API_TOKEN = "alt_test"
afterEach(() => {
  globalThis.fetch = realFetch
})

function fakeEdge(respond: (url: string, body: any) => Response) {
  const calls: Array<{ url: string; method: string; body: any }> = []
  globalThis.fetch = (async (url: any, init: any = {}) => {
    const body = init.body ? JSON.parse(String(init.body)) : undefined
    calls.push({ url: String(url), method: init.method ?? "GET", body })
    return respond(String(url), body)
  }) as unknown as typeof fetch
  return calls
}

describe("knowledge_flag", () => {
  test("posts the flag with session and turn ids and says it was flagged", async () => {
    const calls = fakeEdge(() => new Response(JSON.stringify({ flag: { id: "kflag_1" } }), { status: 200 }))
    const tool = await KnowledgeFlagTool.init()
    const out = await tool.execute(
      { kind: "wrong", target_kind: "data_access", target_ref: "github", evidence: " column renamed ", suggested_fix: "use owner_login" },
      { sessionID: "ses_1", messageID: "msg_9", agent: "build" } as any,
    )
    const post = calls.find((c) => c.url.endsWith("/api/v1/knowledge/flags"))
    expect(post).toBeDefined()
    expect(post!.method).toBe("POST")
    expect(post!.body).toEqual({
      kind: "wrong",
      target_kind: "data_access",
      target_ref: "github",
      evidence: "column renamed",
      suggested_fix: "use owner_login",
      session_id: "ses_1",
      turn_id: "msg_9",
      agent_id: "build",
    })
    expect(out.output).toBe("Flagged. The curator will look at it.")
  })

  test("an unreachable API is reported without throwing", async () => {
    globalThis.fetch = (async () => {
      throw new Error("fetch failed")
    }) as unknown as typeof fetch
    const tool = await KnowledgeFlagTool.init()
    const out = await tool.execute({ kind: "missing", target_kind: "skill", evidence: "no how-to" }, { sessionID: "s", messageID: "m", agent: "a" } as any)
    expect(out.output).toContain("Could not send the flag")
  })
})

describe("/knowledge formatting", () => {
  const proposal = {
    id: "kprop_1",
    target_kind: "data_access",
    target_ref: "github",
    title: "Use owner_login",
    body: "owner_login is the column\n",
    base: "owner is the column\n",
    evidence: [{ flag_id: "kflag_1" }],
    origin: "curator",
    status: "proposed",
  }
  test("the list shows a seeded proposal and flag", () => {
    const text = formatList(
      [{ id: "kflag_1", kind: "wrong", target_kind: "data_access", target_ref: "github", evidence: "column renamed", status: "open" }],
      [proposal],
    )
    expect(text).toContain("kprop_1")
    expect(text).toContain("Use owner_login")
    expect(text).toContain("kflag_1")
  })
  test("show renders a diff against base and the evidence", () => {
    const text = formatProposal(proposal)
    expect(text).toContain("-owner is the column")
    expect(text).toContain("+owner_login is the column")
    expect(text).toContain("flag kflag_1")
  })
  test("approving a skill proposal with nobody to ask says how to turn it on", async () => {
    fakeEdge(() => new Response(JSON.stringify({ proposal: { ...proposal, target_kind: "skill", status: "approved" } }), { status: 200 }))
    const out = await runKnowledge(["approve", "kprop_1"])
    expect(out).toBe("kprop_1 approved. Turn it on with /knowledge activate kprop_1 (it asks first).")
  })
})
