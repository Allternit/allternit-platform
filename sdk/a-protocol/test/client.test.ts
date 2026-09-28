import { describe, expect, test } from "bun:test"
import { AClient, AError } from "../src"

function fakeFetch(respond: (url: string, init: RequestInit) => { status?: number; body?: unknown }) {
  const calls: Array<{ url: string; init: RequestInit }> = []
  const f = (async (url: string, init: RequestInit) => {
    calls.push({ url, init })
    const r = respond(url, init)
    return new Response(r.body === undefined ? "" : JSON.stringify(r.body), { status: r.status ?? 200 })
  }) as unknown as typeof fetch
  return { f, calls }
}

describe("A:// client", () => {
  test("submits an intent with the bearer token under /api/v1", async () => {
    const { f, calls } = fakeFetch(() => ({ body: { intent_id: "i1", status: "accepted", run_id: "r1" } }))
    const a = new AClient({ baseUrl: "http://127.0.0.1:8013/", token: async () => "atok_1", fetch: f })
    const view = await a.intents.submit({
      version: "a/0.1",
      intent_id: "i1",
      workspace: "ws",
      initiator: "a://workspace/ws/principal/al",
      target: "a://workspace/ws/bot/ledger",
      action: { action_type: "price.compute", description: "Price H100" },
      compute: "local",
    })
    expect(view.run_id).toBe("r1")
    expect(calls[0].url).toBe("http://127.0.0.1:8013/api/v1/fabric/transport/intents")
    expect((calls[0].init.headers as Record<string, string>).Authorization).toBe("Bearer atok_1")
    expect(JSON.parse(String(calls[0].init.body)).compute).toBe("local")
  })

  test("worker lease cycle paths and bodies", async () => {
    const { f, calls } = fakeFetch((url) =>
      url.endsWith("/claim")
        ? { body: { job_id: "j 1", run_id: "r", lease_id: "l", lease_generation: 2, lease_expires_at: "", payload: {}, required_capabilities: [], current_checkpoint_id: null, initiator: null, delegator: null } }
        : { body: { ok: true } },
    )
    const a = new AClient({ baseUrl: "http://x", fetch: f })
    const grant = (await a.jobs.claim({ wait_secs: 5 }))!
    const lease = { lease_id: grant.lease_id, lease_generation: grant.lease_generation }
    await a.jobs.heartbeat(grant.job_id, lease)
    await a.jobs.complete(grant.job_id, { ...lease, success: true, summary: "done" })
    expect(calls.map((c) => c.url)).toEqual([
      "http://x/api/v1/fabric/transport/claim",
      "http://x/api/v1/fabric/transport/jobs/j%201/heartbeat",
      "http://x/api/v1/fabric/transport/jobs/j%201/complete",
    ])
    expect(JSON.parse(String(calls[2].init.body))).toEqual({ lease_id: "l", lease_generation: 2, success: true, summary: "done" })
  })

  test("lists unwrap either a bare array or an envelope; memory is principal-scoped", async () => {
    const { f, calls } = fakeFetch((url) =>
      url.includes("approvals") ? { body: { approvals: [{ id: "a1", status: "pending" }] } } : { body: { memories: [{ id: "m1", content: "35% margin" }] } },
    )
    const a = new AClient({ baseUrl: "http://x", fetch: f })
    expect((await a.approvals.list({ status: "pending" }))[0].id).toBe("a1")
    expect((await a.memory.list({ bot: "ledger" }))[0].content).toBe("35% margin")
    expect(calls[1].url).toBe("http://x/api/v1/cowork/memory?bot=ledger")
  })

  test("errors carry the status and the server's message", async () => {
    const { f } = fakeFetch(() => ({ status: 403, body: { error: "not your approval" } }))
    const a = new AClient({ baseUrl: "http://x", fetch: f })
    const err = await a.approvals.grant("a1").catch((e) => e)
    expect(err).toBeInstanceOf(AError)
    expect(err.status).toBe(403)
    expect(err.message).toContain("not your approval")
  })
})
