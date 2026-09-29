// Subscription Fabric tool belt (SURFACES_PLAN §3 step 5) and the D16
// always-ask `subscription` permission class.
import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { createHash } from "crypto"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"
import {
  CAPABILITY_TOOLS,
  SubscriptionNotConfirmedError,
  subscriptionCapabilityTools,
} from "../../src/runtime/tools/builtins/subscription"
import { MediaGenerateTool } from "../../src/runtime/tools/builtins/media-generate"
import { resetFabricCapabilitiesCache } from "../../src/runtime/providers/fabric/tasks"

const originalFetch = globalThis.fetch

beforeEach(() => {
  process.env.ALLTERNIT_API_URL = "http://api.test"
  process.env.ALLTERNIT_API_TOKEN = "runtime-token"
  resetFabricCapabilitiesCache()
})

afterEach(() => {
  globalThis.fetch = originalFetch
  delete process.env.ALLTERNIT_API_URL
  delete process.env.ALLTERNIT_API_TOKEN
  resetFabricCapabilitiesCache()
})

type Call = { method: string; path: string; headers: Record<string, string>; body: any }

function fakeForwarder(handler: (call: Call) => Response | Promise<Response>) {
  const calls: Call[] = []
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input))
    const headers: Record<string, string> = {}
    new Headers(init?.headers).forEach((v, k) => (headers[k] = v))
    const call: Call = {
      method: init?.method ?? "GET",
      path: url.pathname.replace("/api/v1/subscriptions/gateway", ""),
      headers,
      body: init?.body ? JSON.parse(String(init.body)) : undefined,
    }
    calls.push(call)
    return handler(call)
  }) as typeof fetch
  return calls
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } })

function sse(events: Array<{ event: string; data: unknown }>): Response {
  const enc = new TextEncoder()
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      for (const e of events) c.enqueue(enc.encode(`event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`))
      c.close()
    },
  })
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } })
}

const cap = (capability: string, provider = "chatgpt", available = true) => ({
  capability,
  provider,
  adapter_id: `${provider}-web`,
  status: "stable",
  entitlements: [{ account_id: "acct", pool_key: "k", pool_state: "ok", available }],
})

const PNG = new Uint8Array([137, 80, 78, 71, 1, 2, 3])
const sha = (b: Uint8Array) => createHash("sha256").update(b).digest("hex")

/** A gateway that runs every submitted task to completion with one artifact. */
function gateway(opts: { caps: any[]; artifact?: Uint8Array; mime?: string; recordedSha?: string; status?: string; detail?: string }) {
  return fakeForwarder((c) => {
    if (c.path === "/v1/capabilities") return json(opts.caps)
    if (c.method === "POST" && c.path === "/v1/tasks") return json({ task_id: "t1", status: "queued" }, 201)
    if (c.path === "/v1/tasks/t1/events")
      return sse([
        { event: "progress", data: { label: "Drafting slides" } },
        { event: "task.status", data: { status: opts.status ?? "completed" } },
      ])
    if (c.path === "/v1/tasks/t1")
      return json({
        task_id: "t1",
        status: opts.status ?? "completed",
        status_detail: opts.detail ?? null,
        result: { text: "Done.", artifact_ids: opts.artifact ? ["art_1"] : [] },
        error: null,
      })
    if (c.path === "/v1/artifacts/art_1/download")
      return new Response(opts.artifact! as unknown as BodyInit, {
        status: 200,
        headers: {
          "content-type": opts.mime ?? "image/png",
          "content-disposition": `attachment; filename="art_1.${opts.mime ? "pptx" : "png"}"`,
          "x-artifact-sha256": opts.recordedSha ?? sha(opts.artifact!),
        },
      })
    return json({ error: "not_found" }, 404)
  })
}

function ctx(ask: (req: any) => Promise<any>) {
  const titles: string[] = []
  const asks: any[] = []
  return {
    titles,
    asks,
    ctx: {
      sessionID: "ses_1",
      messageID: "msg_1",
      callID: "call_1",
      agent: "build",
      abort: new AbortController().signal,
      messages: [],
      metadata: (m: { title?: string }) => void (m.title && titles.push(m.title)),
      ask: async (req: any) => {
        asks.push(req)
        return ask(req)
      },
    } as any,
  }
}

/** The pending asks, once `n` are registered (ask() parks asynchronously). */
async function pendingAsks(n = 1) {
  for (let i = 0; i < 400; i++) {
    const list = await PermissionNext.list()
    if (list.length >= n) return list
    await new Promise((r) => setTimeout(r, 10))
  }
  throw new Error("the ask never became pending")
}

const posts = (calls: Call[]) => calls.filter((c) => c.method === "POST" && c.path === "/v1/tasks")

async function presentationTool() {
  const tools = await subscriptionCapabilityTools()
  const info = tools.find((t) => t.id === "subscription_presentation_create")!
  return info.init()
}

describe("D16: `subscription` is always-ask", () => {
  const modes = ["default", "manual", "acceptEdits", "auto", "yolo", "bypassPermissions"]
  for (const mode of modes) {
    test(`${mode}: asks, even with saved approvals and a configured allow`, () => {
      const allow: PermissionNext.Ruleset = [{ permission: "subscription", pattern: "*", action: "allow" }]
      const rule = PermissionNext.evaluatePolicy("subscription", "chatgpt:image.generate", {
        configured: allow,
        approvals: allow,
        mode,
        skipPermissions: mode === "bypassPermissions",
      })
      expect(rule.action).toBe("ask")
    })
  }

  test("only refusals are automatic: configured deny, plan, dontAsk", () => {
    const deny: PermissionNext.Ruleset = [{ permission: "subscription", pattern: "*", action: "deny" }]
    expect(PermissionNext.evaluatePolicy("subscription", "x", { configured: deny, mode: "yolo" }).action).toBe("deny")
    expect(PermissionNext.evaluatePolicy("subscription", "x", { configured: [], mode: "plan" }).action).toBe("deny")
    expect(PermissionNext.evaluatePolicy("subscription", "x", { configured: [], mode: "dontAsk" }).action).toBe("deny")
    // Contrast: an ordinary tool is auto-allowed in yolo.
    expect(PermissionNext.evaluatePolicy("bash", "ls", { configured: [], mode: "yolo" }).action).toBe("allow")
  })

  test("the legacy evaluate() path asks too", () => {
    const allow: PermissionNext.Ruleset = [{ permission: "subscription", pattern: "*", action: "allow" }]
    expect(PermissionNext.evaluate("subscription", "x", allow).action).toBe("ask")
  })

  test("a reply carries the human action back; 'always' counts once and is never saved", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const ask = () =>
          PermissionNext.ask({
            sessionID: "session_sub",
            permission: "subscription",
            patterns: ["chatgpt:image.generate"],
            metadata: {},
            always: ["*"],
            ruleset: [],
            mode: "yolo",
          })
        const first = ask()
        const [pending] = await pendingAsks()
        expect(pending.permission).toBe("subscription")
        await PermissionNext.reply({ requestID: pending.id, reply: "always", humanAction: "ha_1" })
        expect(await first).toEqual({ humanAction: "ha_1" })

        // Nothing was remembered: the next one asks again (still pending).
        const second = ask()
        const again = await pendingAsks()
        expect(again).toHaveLength(1)
        // A reply without a human action grants nothing a tool can use.
        await PermissionNext.reply({ requestID: again[0].id, reply: "once" })
        expect(await second).toBeUndefined()
      },
    })
  })

  test("an ordinary permission never carries a human action", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const p = PermissionNext.ask({
          sessionID: "session_b",
          permission: "bash",
          patterns: ["ls"],
          metadata: {},
          always: [],
          ruleset: [{ permission: "bash", pattern: "*", action: "ask" }],
        })
        const [pending] = await pendingAsks()
        await PermissionNext.reply({ requestID: pending.id, reply: "once", humanAction: "ha_x" })
        expect(await p).toBeUndefined()
      },
    })
  })
})

describe("subscription capability tools", () => {
  test("only capabilities the gateway reports as available become tools", async () => {
    gateway({
      caps: [
        cap("presentation.create"),
        cap("document.create", "claude", false), // no entitled account
        { ...cap("research.deep"), status: "disabled" },
        cap("chat.create"),
      ],
    })
    const tools = await subscriptionCapabilityTools()
    expect(tools.map((t) => t.id)).toEqual(["subscription_presentation_create"])
    expect(CAPABILITY_TOOLS.map((t) => t.capability)).toEqual(["presentation.create", "document.create", "research.deep"])
  })

  test("not configured → no tools and no network", async () => {
    delete process.env.ALLTERNIT_API_URL
    const calls = gateway({ caps: [cap("presentation.create")] })
    expect(await subscriptionCapabilityTools()).toEqual([])
    expect(calls).toHaveLength(0)
  })

  test("D16: a call without a human-approved action never reaches POST v1/tasks", async () => {
    const calls = gateway({ caps: [cap("presentation.create")] })
    const tool = await presentationTool()

    // (1) A context with nobody to ask (auto/yolo sub-contexts, headless) resolves with nothing.
    const none = ctx(async () => undefined)
    await expect(tool.execute({ prompt: "Q3 deck" }, none.ctx)).rejects.toBeInstanceOf(SubscriptionNotConfirmedError)
    expect(none.asks[0]).toMatchObject({ permission: "subscription", patterns: ["chatgpt:presentation.create"], always: [] })

    // (2) An approval without a human action (e.g. a local reply that did not come through the app).
    const bare = ctx(async () => ({}))
    await expect(tool.execute({ prompt: "Q3 deck" }, bare.ctx)).rejects.toBeInstanceOf(SubscriptionNotConfirmedError)

    // (3) The person refuses.
    const refused = ctx(async () => {
      throw new PermissionNext.RejectedError()
    })
    await expect(tool.execute({ prompt: "Q3 deck" }, refused.ctx)).rejects.toBeInstanceOf(PermissionNext.RejectedError)

    expect(posts(calls)).toHaveLength(0)
  })

  test("D16 end to end: in yolo mode the real permission gate holds the call until a person answers", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const calls = gateway({ caps: [cap("presentation.create")], artifact: PNG })
        const tool = await presentationTool()
        const real = ctx((req) =>
          PermissionNext.ask({ ...req, sessionID: "session_yolo", ruleset: [], mode: "yolo" }),
        )
        const run = tool.execute({ prompt: "Q3 deck" }, real.ctx)
        const [pending] = await pendingAsks()
        await new Promise((r) => setTimeout(r, 50))
        expect(posts(calls)).toHaveLength(0)
        expect(pending.permission).toBe("subscription")
        await PermissionNext.reply({ requestID: pending.id, reply: "once", humanAction: "ha_app" })
        const result = await run
        expect(posts(calls)).toHaveLength(1)
        expect(posts(calls)[0].headers["x-allternit-human-action"]).toBe("ha_app")
        expect(result.metadata.ok).toBe(true)
      },
    })
  })

  test("confirmed: submits with the human action, follows progress, returns checksum-verified files", async () => {
    const pptx = new Uint8Array([80, 75, 3, 4, 9, 9])
    const mime = "application/vnd.openxmlformats-officedocument.presentationml.presentation"
    const calls = gateway({ caps: [cap("presentation.create")], artifact: pptx, mime })
    const tool = await presentationTool()
    const c = ctx(async () => ({ humanAction: "ha_42" }))
    const result = await tool.execute({ prompt: "Q3 deck", title: "Q3 review" }, c.ctx)

    const [submit] = posts(calls)
    expect(submit.headers["x-allternit-human-action"]).toBe("ha_42")
    expect(submit.body).toMatchObject({
      capability: "presentation.create",
      prompt: "Q3 deck",
      routing: { provider: "chatgpt" },
      idempotency_key: "gizzi-tool-ses_1-call_1",
    })
    expect(submit.body.initiated_by).toBeUndefined() // stamped by allternit-api, never by gizzi
    expect(c.titles.some((t) => t.includes("Drafting slides"))).toBe(true)
    expect(result.metadata).toMatchObject({ ok: true, taskID: "t1", status: "completed" })
    expect(result.attachments?.[0]).toMatchObject({ mime, filename: "Q3 review.pptx" })
    expect(result.attachments?.[0].url).toBe(`data:${mime};base64,${Buffer.from(pptx).toString("base64")}`)
  })

  test("an artifact that does not match its recorded checksum is refused", async () => {
    gateway({ caps: [cap("presentation.create")], artifact: PNG, recordedSha: "0".repeat(64) })
    const tool = await presentationTool()
    const result = await tool.execute({ prompt: "deck" }, ctx(async () => ({ humanAction: "ha_1" })).ctx)
    expect(result.attachments).toEqual([])
    expect(result.metadata.ok).toBe(false)
    expect(result.output).toContain("did not match its recorded checksum")
  })

  test("a provider question mid-task goes to the person, never answered here", async () => {
    const calls = gateway({ caps: [cap("presentation.create")], status: "needs_user", detail: "Which region should the report cover?" })
    const tool = await presentationTool()
    const result = await tool.execute({ prompt: "deck" }, ctx(async () => ({ humanAction: "ha_1" })).ctx)
    expect(result.metadata.ok).toBe(false)
    expect(result.output).toContain("Which region should the report cover?")
    expect(result.output).toContain("do not try to answer it")
    expect(posts(calls)).toHaveLength(1)
  })

  test("a provider the gateway does not offer is refused before asking", async () => {
    const calls = gateway({ caps: [cap("presentation.create")] })
    const tool = await presentationTool()
    const c = ctx(async () => ({ humanAction: "ha_1" }))
    const result = await tool.execute({ prompt: "deck", provider: "kimi" }, c.ctx)
    expect(result.output).toContain("Kimi cannot do this right now")
    expect(c.asks).toHaveLength(0)
    expect(posts(calls)).toHaveLength(0)
  })
})

describe("media_generate subscription lane", () => {
  test("image.generate through the forwarder: confirmed, verified, attached", async () => {
    const calls = gateway({ caps: [cap("image.generate")], artifact: PNG })
    const tool = await MediaGenerateTool.init()
    const c = ctx(async () => ({ humanAction: "ha_img" }))
    const result = await tool.execute({ kind: "image", prompt: "a lighthouse at dawn", title: "Lighthouse", lane: "subscription" }, c.ctx)
    expect(c.asks[0]).toMatchObject({ permission: "subscription", patterns: ["chatgpt:image.generate"] })
    expect(posts(calls)[0].body).toMatchObject({ capability: "image.generate", prompt: "a lighthouse at dawn", routing: { provider: "chatgpt" } })
    expect(posts(calls)[0].headers["x-allternit-human-action"]).toBe("ha_img")
    expect(result.metadata).toMatchObject({ ok: true, lane: "subscription", provider: "chatgpt", taskID: "t1" })
    expect(result.attachments?.[0]).toMatchObject({ mime: "image/png", filename: "Lighthouse.png" })
  })

  test("D16: no human action → nothing submitted", async () => {
    const calls = gateway({ caps: [cap("image.generate")], artifact: PNG })
    const tool = await MediaGenerateTool.init()
    await expect(
      tool.execute({ kind: "image", prompt: "x", lane: "subscription" }, ctx(async () => undefined).ctx),
    ).rejects.toBeInstanceOf(SubscriptionNotConfirmedError)
    expect(posts(calls)).toHaveLength(0)
  })

  test("no subscription can make images → says so, asks nothing, sends nothing", async () => {
    const calls = gateway({ caps: [cap("chat.create")] })
    const tool = await MediaGenerateTool.init()
    const c = ctx(async () => ({ humanAction: "ha" }))
    const result = await tool.execute({ kind: "image", prompt: "x", lane: "subscription" }, c.ctx)
    expect(result.metadata).toMatchObject({ ok: false, lane: "subscription" })
    expect(result.output).toContain("Nothing was sent")
    expect(c.asks).toHaveLength(0)
    expect(posts(calls)).toHaveLength(0)
  })
})
