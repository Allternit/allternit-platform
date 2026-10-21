import { describe, expect, test, spyOn, afterEach, beforeEach } from "bun:test"
import {
  ArtifactClaimTool,
  ArtifactCommentTool,
  ArtifactCreateTool,
  ArtifactReadTool,
  ArtifactReleaseTool,
  ArtifactSuggestTool,
  ArtifactUpdateTool,
  MAX_MODEL_BODY,
  buildArtboardChange,
  buildControls,
  claimOf,
  heartbeat,
  parseCanvas,
  pinAnchor,
  resetClaims,
  sameArtboard,
  shapeId,
} from "../../src/runtime/tools/builtins/artifact"
import * as BotTurn from "../../src/runtime/bots/bot-turn"
import { Artifacts } from "../../src/runtime/integrations/artifacts"
import { Pairing } from "../../src/runtime/services/pairing/pairing"

type Ctx = Parameters<Awaited<ReturnType<typeof ArtifactCreateTool.init>>["execute"]>[1]

function ctxWith(messages: any[] = []): Ctx {
  return {
    sessionID: "ses_test",
    messageID: "msg_test",
    callID: "call_test",
    agent: "test-agent",
    abort: AbortSignal.any([]),
    messages,
    metadata: () => {},
    ask: async () => {},
  } as unknown as Ctx
}

/** A transcript holding one completed artifact tool part. */
function transcriptWith(artifact: Artifacts.Payload) {
  return [
    {
      info: { id: "msg_1", role: "assistant" },
      parts: [
        {
          type: "tool",
          tool: "artifact_create",
          callID: "c1",
          state: { status: "completed", input: {}, output: "", title: "", metadata: { artifact }, time: { start: 0, end: 1 } },
        },
      ],
    },
  ]
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })

const ENV = [
  "ALLTERNIT_API_KEY",
  "ALLTERNIT_API_TOKEN",
  "ALLTERNIT_CLOUD_API_URL",
  "GIZZI_ARTIFACTS_OFFLINE",
  "GIZZI_ARTIFACT_PRESENCE",
] as const
const saved: Record<string, string | undefined> = {}

describe("tool.artifact_*", () => {
  let fetchSpy: ReturnType<typeof spyOn> | undefined
  let pairingSpy: ReturnType<typeof spyOn> | undefined

  beforeEach(() => {
    for (const k of ENV) {
      saved[k] = process.env[k]
      delete process.env[k]
    }
    process.env.ALLTERNIT_CLOUD_API_URL = "https://cloud.test"
    // These tests count and inspect the store calls; the agent-watch block turns heartbeats back on.
    process.env.GIZZI_ARTIFACT_PRESENCE = "0"
    pairingSpy = spyOn(Pairing, "load").mockResolvedValue(undefined as any)
  })
  afterEach(() => {
    fetchSpy?.mockRestore()
    pairingSpy?.mockRestore()
    fetchSpy = undefined
    for (const k of ENV) {
      if (saved[k] === undefined) delete process.env[k]
      else process.env[k] = saved[k]
    }
  })

  test("create: saves to the cloud store with a client-minted id when signed in", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    let sent: any
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (url: string, init: RequestInit) => {
      sent = { url, init, body: JSON.parse(String(init.body)) }
      return json(201, {
        id: sent.body.id,
        kind: "page",
        title: "Launch plan",
        current_version: 1,
        version: { version: 1, body: sent.body.body, body_format: "text/markdown", meta: {} },
      })
    }) as any)
    const tool = await ArtifactCreateTool.init()
    const result = await tool.execute({ kind: "page", title: "Launch plan", body: "# Plan\n\n- ship" }, ctxWith())

    expect(sent.url).toBe("https://cloud.test/api/v2/artifacts")
    expect(sent.init.method).toBe("POST")
    expect((sent.init.headers as Record<string, string>).Authorization).toBe("Bearer alt_key")
    expect(sent.body).toMatchObject({
      kind: "page",
      title: "Launch plan",
      body_format: "text/markdown",
      runtime_version: 2,
      origin: { surface: "gizzi", session_id: "ses_test", message_id: "msg_test" },
    })
    expect(sent.body.id).toMatch(/^art_[0-9A-HJKMNP-TV-Z]{26}$/)
    expect(result.metadata.artifact).toMatchObject({
      id: sent.body.id,
      version: 1,
      kind: "page",
      body: "# Plan\n\n- ship",
      persisted: true,
    })
    expect(JSON.parse(result.output.split("\n")[0]!)).toMatchObject({ id: sent.body.id, version: 1, persisted: true })
    // The body stays out of the model-visible text.
    expect(result.output).not.toContain("- ship")
  })

  test("create offline: no credential → persisted:false with a stable id and the full payload", async () => {
    fetchSpy = spyOn(globalThis, "fetch")
    const tool = await ArtifactCreateTool.init()
    const result = await tool.execute(
      { kind: "code", title: "fib.py", body: "def fib(n):\n    return n", meta: { language: "python" } },
      ctxWith(),
    )
    expect(fetchSpy).not.toHaveBeenCalled()
    const a = result.metadata.artifact as Artifacts.Payload
    expect(a.id).toMatch(/^art_/)
    expect(a).toMatchObject({
      version: 1,
      kind: "code",
      title: "fib.py",
      body: "def fib(n):\n    return n",
      body_format: "text/plain",
      meta: { language: "python" },
      persisted: false,
    })
    expect(result.output).toContain(a.id)
    expect(JSON.parse(result.output.split("\n")[0]!).id).toBe(a.id)
  })

  test("create offline: network error and missing route both fall back to persisted:false", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    fetchSpy = spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("fetch failed"))
    const tool = await ArtifactCreateTool.init()
    const down = await tool.execute({ kind: "diagram", title: "Flow", body: "graph TD; A-->B" }, ctxWith())
    expect(down.metadata.artifact).toMatchObject({ persisted: false, body_format: "text/vnd.mermaid" })

    fetchSpy.mockRestore()
    fetchSpy = spyOn(globalThis, "fetch").mockResolvedValue(new Response("Not Found", { status: 404 }))
    const missing = await tool.execute({ kind: "diagram", title: "Flow", body: "<svg></svg>" }, ctxWith())
    expect(missing.metadata.artifact).toMatchObject({ persisted: false, body_format: "image/svg+xml" })
  })

  test("create: a rejected credential falls through to the next one, then offline", async () => {
    process.env.ALLTERNIT_API_KEY = "stale"
    process.env.ALLTERNIT_API_TOKEN = "also_stale"
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async () => json(401, { error: "unauthorized" })) as any)
    const tool = await ArtifactCreateTool.init()
    const result = await tool.execute({ kind: "page", title: "P", body: "<p>hi</p>" }, ctxWith())
    expect(fetchSpy).toHaveBeenCalledTimes(2)
    expect(result.metadata.artifact).toMatchObject({ persisted: false, body_format: "text/html" })
  })

  test("create: refuses a format that doesn't fit the kind, and non-JSON for a +json format", async () => {
    const tool = await ArtifactCreateTool.init()
    await expect(
      tool.execute({ kind: "slides", title: "Deck", body: "{}", body_format: "text/html" }, ctxWith()),
    ).rejects.toThrow('isn\'t valid for kind "slides"')
    await expect(tool.execute({ kind: "slides", title: "Deck", body: "# not json" }, ctxWith())).rejects.toThrow(
      "must be JSON",
    )
  })

  test("update: posts base_version to the versions route and returns the new version", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    let sent: any
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (url: string, init: RequestInit) => {
      sent = { url, body: JSON.parse(String(init.body)) }
      return json(201, {
        id: "art_X",
        kind: "page",
        title: "Plan",
        current_version: 3,
        version: { version: 3, body: sent.body.body, body_format: "text/markdown", meta: sent.body.meta },
      })
    }) as any)
    const tool = await ArtifactUpdateTool.init()
    const result = await tool.execute({ id: "art_X", body: "# v3", base_version: 2, note: "tighter" }, ctxWith())
    expect(sent.url).toBe("https://cloud.test/api/v2/artifacts/art_X/versions")
    expect(sent.body).toMatchObject({ base_version: 2, body: "# v3", author: "assistant", meta: { note: "tighter" } })
    expect(result.metadata.artifact).toMatchObject({ id: "art_X", version: 3, persisted: true, body: "# v3" })
    expect(result.output).toContain("base_version 3")
  })

  test("update: 409 stale_version tells the model to read, merge and retry", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async () =>
      json(409, { error: "stale_version", message: "stale", current_version: 5 })) as any)
    const tool = await ArtifactUpdateTool.init()
    const run = tool.execute({ id: "art_X", body: "# new", base_version: 3 }, ctxWith())
    await expect(run).rejects.toThrow("now at version 5")
    await expect(tool.execute({ id: "art_X", body: "# new", base_version: 3 }, ctxWith())).rejects.toThrow(
      'Call artifact_read with id "art_X"',
    )
    await expect(tool.execute({ id: "art_X", body: "# new", base_version: 3 }, ctxWith())).rejects.toThrow(
      "base_version 5",
    )
  })

  test("update offline: builds on the transcript's artifact and refuses a stale base_version", async () => {
    const known: Artifacts.Payload = {
      id: "art_T",
      kind: "page",
      title: "Notes",
      version: 2,
      body: "# v2",
      body_format: "text/markdown",
      meta: {},
      persisted: false,
    }
    const tool = await ArtifactUpdateTool.init()
    const result = await tool.execute({ id: "art_T", body: "# v3", base_version: 2 }, ctxWith(transcriptWith(known)))
    expect(result.metadata.artifact).toMatchObject({
      id: "art_T",
      version: 3,
      base_version: 2,
      title: "Notes",
      kind: "page",
      body_format: "text/markdown",
      body: "# v3",
      persisted: false,
    })
    await expect(
      tool.execute({ id: "art_T", body: "# v3", base_version: 1 }, ctxWith(transcriptWith(known))),
    ).rejects.toThrow("artifact_read")
  })

  test("update: an unknown id the cloud doesn't have is a clear not-found", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    fetchSpy = spyOn(globalThis, "fetch").mockResolvedValue(json(404, { error: "not_found" }))
    const tool = await ArtifactUpdateTool.init()
    await expect(tool.execute({ id: "art_nope", body: "x", base_version: 1 }, ctxWith())).rejects.toThrow(
      "was not found",
    )
  })

  test("read: returns the cloud body, capped for the model but whole in metadata", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    const big = "a".repeat(MAX_MODEL_BODY + 5_000)
    fetchSpy = spyOn(globalThis, "fetch").mockResolvedValue(
      json(200, {
        id: "art_R",
        kind: "code",
        title: "big.txt",
        current_version: 4,
        version: { version: 4, body: big, body_format: "text/plain", meta: { language: "text" } },
      }),
    )
    const tool = await ArtifactReadTool.init()
    const result = await tool.execute({ id: "art_R" }, ctxWith())
    expect((fetchSpy.mock.calls[0] as any[])[0]).toBe("https://cloud.test/api/v2/artifacts/art_R")
    expect(result.metadata.artifact).toMatchObject({ id: "art_R", version: 4, persisted: true })
    expect((result.metadata.artifact as Artifacts.Payload).body.length).toBe(big.length)
    expect(result.output).toContain("…(truncated")
    expect(result.output.length).toBeLessThan(big.length)
  })

  test("read offline: falls back to this session's transcript", async () => {
    const known: Artifacts.Payload = {
      id: "art_T",
      kind: "doc",
      title: "Brief",
      version: 1,
      body: "# Brief",
      body_format: "text/markdown",
      meta: {},
      persisted: false,
    }
    const tool = await ArtifactReadTool.init()
    const result = await tool.execute({ id: "art_T" }, ctxWith(transcriptWith(known)))
    expect(result.output).toContain("# Brief")
    expect(result.metadata.artifact).toMatchObject({ id: "art_T", version: 1 })
    await expect(tool.execute({ id: "art_missing" }, ctxWith(transcriptWith(known)))).rejects.toThrow("was not found")
  })

  test("GIZZI_ARTIFACTS_OFFLINE skips the cloud even with a credential", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    process.env.GIZZI_ARTIFACTS_OFFLINE = "1"
    fetchSpy = spyOn(globalThis, "fetch")
    const tool = await ArtifactCreateTool.init()
    const result = await tool.execute({ kind: "image", title: "Logo", body: "https://files.test/logo.png" }, ctxWith())
    expect(fetchSpy).not.toHaveBeenCalled()
    expect(result.metadata.artifact).toMatchObject({ persisted: false, body_format: "text/uri-list" })
  })
})

describe("Artifacts.validate for motion", () => {
  const fmt = "application/vnd.allternit.motion+json"
  test("accepts a body with scenes", () => {
    expect(Artifacts.validate("motion", fmt, JSON.stringify({ scenes: [{ type: "title", duration: 2, props: { text: "Hi" } }] }))).toBeUndefined()
  })
  test("tells the model what is missing", () => {
    expect(Artifacts.validate("motion", fmt, "{}")).toContain('"scenes"')
    expect(Artifacts.validate("motion", fmt, '{"scenes":[]}')).toContain('"scenes"')
    expect(Artifacts.validate("motion", fmt, "null")).toContain('"scenes"')
    expect(Artifacts.validate("motion", fmt, "nope")).toContain("must be JSON")
  })
})

describe("agent watch: artifact_suggest, presence heartbeats, agent comments", () => {
  let fetchSpy: ReturnType<typeof spyOn> | undefined
  let pairingSpy: ReturnType<typeof spyOn> | undefined
  const calls: Array<{ url: string; method: string; body: any }> = []

  beforeEach(() => {
    for (const k of ENV) {
      saved[k] = process.env[k]
      delete process.env[k]
    }
    process.env.ALLTERNIT_CLOUD_API_URL = "https://cloud.test"
    pairingSpy = spyOn(Pairing, "load").mockResolvedValue(undefined as any)
    BotTurn.reset()
    calls.length = 0
  })
  afterEach(() => {
    fetchSpy?.mockRestore()
    pairingSpy?.mockRestore()
    fetchSpy = undefined
    BotTurn.reset()
    for (const k of ENV) {
      if (saved[k] === undefined) delete process.env[k]
      else process.env[k] = saved[k]
    }
  })

  /** Records every call; presence answers 204, suggestions echo pending items. */
  function mockCloud(overrides: Record<string, (body: any) => Response> = {}) {
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (url: string, init: RequestInit) => {
      const body = init.body ? JSON.parse(String(init.body)) : undefined
      calls.push({ url, method: String(init.method), body })
      for (const [suffix, respond] of Object.entries(overrides)) if (url.endsWith(suffix)) return respond(body)
      if (url.endsWith("/presence")) return new Response(null, { status: 204 })
      if (url.endsWith("/suggestions")) {
        return json(201, {
          items: body.changes.map((c: any, i: number) => ({
            id: `sug_${i + 1}`,
            artifact_id: "art_S",
            base_version: body.base_version,
            agent_name: body.agent_name,
            session_id: body.session_id ?? null,
            change: c.change,
            reason: c.reason ?? null,
            cost_usd: body.cost_usd ?? null,
            status: "pending",
            created_at: "2026-10-20T00:00:00Z",
            resolved_at: null,
          })),
        })
      }
      if (url.endsWith("/comments")) return json(201, { id: "cmt_1", author_id: "assistant", agent_name: body.agent_name })
      return json(404, { error: "not_found" })
    }) as any)
  }
  const settle = () => new Promise((r) => setTimeout(r, 20))

  test("suggest: posts the change shapes under the bot's name and returns the ids", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    BotTurn.mark("ses_test", { id: "agt_1", name: "Raven" })
    mockCloud()
    const tool = await ArtifactSuggestTool.init()
    const result = await tool.execute(
      {
        id: "art_S",
        base_version: 4,
        task: "Fixing the dates",
        changes: [
          { kind: "text", block: 2, find: "about 40%", replace: "38%", reason: "Matches the report" },
          { kind: "block", op: "delete", block: 7, markdown: "ignored" },
          { kind: "cell", sheet: null, address: "b4", value: "=SUM(B1:B3)" },
          { kind: "text", find: "Nov 1", replace: "Nov 10" },
        ],
      } as any,
      ctxWith(),
    )
    await settle()
    const post = calls.find((c) => c.url === "https://cloud.test/api/v2/artifacts/art_S/suggestions")!
    expect(post.method).toBe("POST")
    expect(post.body).toMatchObject({ base_version: 4, agent_name: "Raven", session_id: "ses_test" })
    expect(post.body.changes).toEqual([
      { change: { kind: "text", block: 2, find: "about 40%", replace: "38%" }, reason: "Matches the report" },
      { change: { kind: "block", op: "delete", block: 7 } },
      { change: { kind: "cell", sheet: null, address: "B4", value: "=SUM(B1:B3)" } },
      { change: { kind: "text", block: null, find: "Nov 1", replace: "Nov 10" } },
    ])
    expect(JSON.parse(result.output.split("\n")[0]!).suggestion_ids).toEqual(["sug_1", "sug_2", "sug_3", "sug_4"])
    expect(result.output).toContain("accepts or rejects")

    const presence = calls.find((c) => c.url.endsWith("/art_S/presence"))!
    expect(presence.body).toEqual({
      state: "editing",
      agent: { name: "Raven", session_id: "ses_test", task: "Fixing the dates", anchor: { kind: "block", block: 2 } },
    })
  })

  test("suggest without a credential: a clear error that points at artifact_update", async () => {
    fetchSpy = spyOn(globalThis, "fetch")
    const tool = await ArtifactSuggestTool.init()
    await expect(
      tool.execute({ id: "art_S", base_version: 1, changes: [{ kind: "text", block: 0, find: "a", replace: "b" }] } as any, ctxWith()),
    ).rejects.toThrow("artifact_update")
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  test("suggest on a cloud without the route falls back to artifact_update", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    mockCloud({ "/suggestions": () => new Response("Not Found", { status: 404 }) })
    const tool = await ArtifactSuggestTool.init()
    await expect(
      tool.execute({ id: "art_S", base_version: 1, changes: [{ kind: "cell", sheet: "Budget", address: "A1", value: "1" }] } as any, ctxWith()),
    ).rejects.toThrow("artifact_update")
  })

  test("read sends a viewing heartbeat with the default task; a failing heartbeat never fails the tool", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    mockCloud({
      "/art_R": () =>
        json(200, { id: "art_R", kind: "doc", title: "Brief", current_version: 2, version: { version: 2, body: "{}", body_format: "application/vnd.allternit.doc+json", meta: {} } }),
      "/presence": () => {
        throw new TypeError("fetch failed")
      },
    })
    const tool = await ArtifactReadTool.init()
    const result = await tool.execute({ id: "art_R" }, ctxWith())
    await settle()
    expect(result.metadata.artifact).toMatchObject({ id: "art_R", version: 2 })
    const presence = calls.find((c) => c.url.endsWith("/art_R/presence"))!
    expect(presence.body.state).toBe("viewing")
    expect(presence.body.agent).toMatchObject({ name: "Gizzi", task: "Reading", session_id: "ses_test" })
  })

  test("heartbeat resolves false and never throws when presence is off or offline", async () => {
    process.env.GIZZI_ARTIFACT_PRESENCE = "0"
    fetchSpy = spyOn(globalThis, "fetch")
    expect(await heartbeat(ctxWith(), "art_X", { state: "viewing", task: "Reading" })).toBe(false)
    delete process.env.GIZZI_ARTIFACT_PRESENCE
    expect(await heartbeat(ctxWith(), "art_X", { state: "viewing", task: "Reading" })).toBe(false)
    expect(fetchSpy).not.toHaveBeenCalled()
  })

  test("comment posts under the agent's name", async () => {
    process.env.ALLTERNIT_API_KEY = "alt_key"
    BotTurn.mark("ses_test", { id: "agt_2", name: "Jev" })
    mockCloud()
    const tool = await ArtifactCommentTool.init()
    await tool.execute({ id: "art_C", body: "Which date is right?" } as any, ctxWith())
    await settle()
    const post = calls.find((c) => c.url.endsWith("/art_C/comments"))!
    expect(post.body).toMatchObject({ body: "Which date is right?", agent_name: "Jev" })
    const presence = calls.find((c) => c.url.endsWith("/art_C/presence"))!
    expect(presence.body.agent).toMatchObject({ name: "Jev", task: "Asking a question" })
  })
})

describe("agents on one canvas: artboard changes, artifact_claim, artifact_release", () => {
  let fetchSpy: ReturnType<typeof spyOn> | undefined
  let pairingSpy: ReturnType<typeof spyOn> | undefined
  const calls: Array<{ url: string; method: string; body: any }> = []
  const DESIGN = "application/vnd.allternit.design+json"

  const layer = (id: string, artboard: string | null, text = "Hi") => ({
    id,
    type: "text",
    name: id,
    artboard,
    x: 0,
    y: 0,
    w: 100,
    h: 40,
    props: { text, size: "m", color: "black", w: 100 },
  })
  /** A model-written canvas: ids without tldraw's shape: prefix. */
  const canvas = (heroText = "Hi", pricingText = "Plans") =>
    JSON.stringify({
      v: 2,
      kind: "canvas",
      designSystem: null,
      artboards: [
        { id: "hero", name: "Hero", x: 0, y: 0, w: 1440, h: 900, fill: "#ffffff" },
        { id: "pricing", name: "Pricing", x: 1600, y: 0, w: 1440, h: 900, fill: "#ffffff" },
      ],
      layers: [layer("title", "hero", heroText), layer("plans", "pricing", pricingText)],
    })
  const artifact = (version: number, body: string) =>
    json(200, { id: "art_D", kind: "design", title: "Landing", current_version: version, version: { version, body, body_format: DESIGN, meta: {} } })

  beforeEach(() => {
    for (const k of ENV) {
      saved[k] = process.env[k]
      delete process.env[k]
    }
    process.env.ALLTERNIT_CLOUD_API_URL = "https://cloud.test"
    process.env.ALLTERNIT_API_KEY = "alt_key"
    pairingSpy = spyOn(Pairing, "load").mockResolvedValue(undefined as any)
    BotTurn.reset()
    BotTurn.mark("ses_test", { id: "agt_1", name: "Oki" })
    calls.length = 0
  })
  afterEach(() => {
    fetchSpy?.mockRestore()
    pairingSpy?.mockRestore()
    fetchSpy = undefined
    BotTurn.reset()
    resetClaims()
    for (const k of ENV) {
      if (saved[k] === undefined) delete process.env[k]
      else process.env[k] = saved[k]
    }
  })

  /** The design is at version 3; version 2 is `v2body`. Suggestions echo pending items. */
  function mockCloud(opts: { v2body?: string; overrides?: Record<string, (body: any) => Response> } = {}) {
    fetchSpy = spyOn(globalThis, "fetch").mockImplementation((async (url: string, init: RequestInit) => {
      const body = init.body ? JSON.parse(String(init.body)) : undefined
      calls.push({ url, method: String(init.method), body })
      for (const [suffix, respond] of Object.entries(opts.overrides ?? {})) if (url.endsWith(suffix)) return respond(body)
      if (url.endsWith("/art_D")) return artifact(3, canvas())
      if (url.endsWith("/art_D/versions/2")) return json(200, { version: 2, body: opts.v2body ?? canvas(), body_format: DESIGN, meta: {} })
      if (url.endsWith("/presence")) return new Response(null, { status: 204 })
      if (url.endsWith("/suggestions") && init.method === "POST") {
        return json(201, {
          items: body.changes.map((c: any, i: number) => ({
            id: `sug_${i + 1}`,
            artifact_id: "art_D",
            base_version: body.base_version,
            agent_name: body.agent_name,
            session_id: body.session_id ?? null,
            change: c.change,
            reason: c.reason ?? null,
            cost_usd: null,
            status: "pending",
            created_at: "2026-10-20T00:00:00Z",
            resolved_at: null,
          })),
        })
      }
      return json(404, { error: "not_found" })
    }) as any)
  }
  const settle = () => new Promise((r) => setTimeout(r, 20))
  const heroChange = {
    kind: "artboard",
    artboard: "hero",
    frame: { fill: "#0b0b0f" },
    layers: [{ ...layer("title", null, "Plan your week"), props: { text: "Plan your week", color: "white" } }, layer("eyebrow", null, "New")],
    reason: "Made the eyebrow and heading dark",
  }

  test("pure: build an artboard change with shape ids and before read from the canvas", () => {
    const doc = parseCanvas(canvas(), DESIGN)!
    expect(shapeId("hero")).toBe("shape:hero")
    expect(shapeId("shape:hero")).toBe("shape:hero")
    const change = buildArtboardChange(doc, heroChange as any)
    expect(change).toMatchObject({ kind: "artboard", artboard: "shape:hero", name: "Hero", frame: { fill: "#0b0b0f" } })
    expect(change.layers.map((l) => [l.id, l.artboard])).toEqual([
      ["shape:title", "shape:hero"],
      ["shape:eyebrow", "shape:hero"],
    ])
    expect(change.before.frame).toEqual({ fill: "#ffffff" })
    expect(change.before.layers).toEqual([{ ...layer("title", "hero"), id: "shape:title", artboard: "shape:hero" }])

    expect(() => buildArtboardChange(doc, { artboard: "footer", layers: [] })).toThrow("Hero (hero), Pricing (pricing)")
    expect(() => buildArtboardChange(doc, { artboard: "hero", layers: [layer("x", "pricing")] })).toThrow("one artboard's layers")
    expect(() => buildArtboardChange(doc, { artboard: "hero", layers: [layer("plans", null)] })).toThrow("already used on another artboard")
    expect(() => buildArtboardChange(doc, { artboard: "hero", layers: [layer("a", null), layer("shape:a", null)] })).toThrow("used twice")

    // An HTML design or a non-canvas body has no artboards.
    expect(parseCanvas("<main/>", "text/html")).toBeUndefined()
    expect(parseCanvas('{"html":"<p/>"}', DESIGN)).toBeUndefined()
    // Frame equality ignores where the frame sits and key order.
    const moved = parseCanvas(canvas().replace('"x":1600', '"x":1700'), DESIGN)!
    expect(sameArtboard(doc, moved, "shape:pricing")).toBe(true)
    expect(sameArtboard(doc, parseCanvas(canvas("Changed"), DESIGN)!, "hero")).toBe(false)
  })

  test("suggest: one artboard change, before filled from the canvas, posted at the current version", async () => {
    mockCloud({ v2body: canvas("Hi", "Old plans") })
    const tool = await ArtifactSuggestTool.init()
    // Pricing changed since version 2, Hero didn't: the Hero change goes through at version 3.
    const result = await tool.execute({ id: "art_D", base_version: 2, task: "Dark restyle", changes: [heroChange] } as any, ctxWith())
    await settle()
    const post = calls.find((c) => c.url === "https://cloud.test/api/v2/artifacts/art_D/suggestions" && c.method === "POST")!
    expect(post.body).toMatchObject({ base_version: 3, agent_name: "Oki", session_id: "ses_test" })
    expect(post.body.changes).toHaveLength(1)
    expect(post.body.changes[0].reason).toBe("Made the eyebrow and heading dark")
    const change = post.body.changes[0].change
    expect(change).toMatchObject({ kind: "artboard", artboard: "shape:hero", name: "Hero", frame: { fill: "#0b0b0f" } })
    expect(change.before.layers[0]).toMatchObject({ id: "shape:title", artboard: "shape:hero", props: { text: "Hi" } })
    expect(result.output).toContain("In review")
    expect(result.output).toContain("artifact_release")
    const presence = calls.find((c) => c.url.endsWith("/art_D/presence"))!
    expect(presence.body.agent).toMatchObject({ name: "Oki", task: "Dark restyle", anchor: { kind: "artboard", artboard: "shape:hero" } })
  })

  test("suggest: a frame changed since base_version must be read again", async () => {
    mockCloud({ v2body: canvas("Old heading") })
    const tool = await ArtifactSuggestTool.init()
    await expect(tool.execute({ id: "art_D", base_version: 2, changes: [heroChange] } as any, ctxWith())).rejects.toThrow(
      "Hero changed since version 2",
    )
    expect(calls.some((c) => c.url.endsWith("/suggestions") && c.method === "POST")).toBe(false)
  })

  test("suggest: a held frame comes back as a plain error naming the holder", async () => {
    mockCloud({
      overrides: {
        "/suggestions": () =>
          json(409, { error: "artboard_claimed", code: "artboard_claimed", message: "x", holder: { name: "Ash", session_id: "ses_ash" } }),
      },
    })
    const tool = await ArtifactSuggestTool.init()
    await expect(tool.execute({ id: "art_D", base_version: 3, changes: [heroChange] } as any, ctxWith())).rejects.toThrow(
      "Hero is being worked on by Ash; pick another frame or wait.",
    )
  })

  test("suggest: artboard changes need a canvas design", async () => {
    mockCloud({
      overrides: {
        "/art_D": () => json(200, { id: "art_D", kind: "design", title: "Old", current_version: 1, version: { version: 1, body: "<main/>", body_format: "text/html", meta: {} } }),
      },
    })
    const tool = await ArtifactSuggestTool.init()
    await expect(tool.execute({ id: "art_D", base_version: 1, changes: [heroChange] } as any, ctxWith())).rejects.toThrow("Convert to canvas")
  })

  test("claim: claims through presence, heartbeats carry the frame, release clears it", async () => {
    mockCloud()
    const claim = await ArtifactClaimTool.init()
    const result = await claim.execute({ artifact_id: "art_D", artboard: "hero", task: "Dark restyle" } as any, ctxWith())
    const posted = calls.find((c) => c.url.endsWith("/art_D/presence"))!
    expect(posted.body).toEqual({
      state: "editing",
      agent: { name: "Oki", session_id: "ses_test", task: "Dark restyle", anchor: { kind: "artboard", artboard: "shape:hero" } },
    })
    expect(JSON.parse(result.output.split("\n")[0]!)).toMatchObject({ artifact_id: "art_D", artboard: "shape:hero", name: "Hero", version: 3 })
    expect(result.output).toContain("base_version 3")
    expect(claimOf("ses_test", "art_D")).toMatchObject({ artboard: "shape:hero", frame: "Hero" })

    // A later read keeps the claim: its heartbeat carries the anchor and the frame's task.
    calls.length = 0
    const read = await ArtifactReadTool.init()
    await read.execute({ id: "art_D" }, ctxWith())
    await settle()
    const beat = calls.find((c) => c.url.endsWith("/art_D/presence"))!
    expect(beat.body.agent).toMatchObject({ task: "Dark restyle", anchor: { kind: "artboard", artboard: "shape:hero" } })

    // Releasing another frame is a no-op; releasing sends no anchor.
    const release = await ArtifactReleaseTool.init()
    const wrong = await release.execute({ artifact_id: "art_D", artboard: "pricing" } as any, ctxWith())
    expect(wrong.output).toContain("Nothing was released")
    expect(claimOf("ses_test", "art_D")).toBeDefined()
    calls.length = 0
    const done = await release.execute({ artifact_id: "art_D" } as any, ctxWith())
    expect(done.output).toContain("Released Hero")
    expect(claimOf("ses_test", "art_D")).toBeUndefined()
    const cleared = calls.find((c) => c.url.endsWith("/art_D/presence"))!
    expect(cleared.body.agent.anchor).toBeUndefined()
  })

  test("claim: a held frame is a plain tool error; unknown frames list the artboards", async () => {
    mockCloud({
      overrides: {
        "/presence": () =>
          json(409, { error: "artboard_claimed", code: "artboard_claimed", message: "x", holder: { name: "Ash", session_id: "ses_ash" } }),
      },
    })
    const claim = await ArtifactClaimTool.init()
    await expect(claim.execute({ artifact_id: "art_D", artboard: "shape:hero" } as any, ctxWith())).rejects.toThrow(
      "Hero is being worked on by Ash; pick another frame or wait.",
    )
    expect(claimOf("ses_test", "art_D")).toBeUndefined()
    await expect(claim.execute({ artifact_id: "art_D", artboard: "footer" } as any, ctxWith())).rejects.toThrow("Its artboards: Hero")
  })

  test("update refuses a design with other agents' pending frame changes", async () => {
    const pending = (session: string) =>
      json(200, {
        items: [
          {
            id: "sug_1",
            artifact_id: "art_D",
            base_version: 3,
            agent_name: "Ash",
            session_id: session,
            change: { kind: "artboard", artboard: "shape:pricing", name: "Pricing", layers: [], before: { layers: [] } },
            reason: null,
            cost_usd: null,
            status: "pending",
            created_at: "2026-10-20T00:00:00Z",
            resolved_at: null,
          },
        ],
      })
    mockCloud({ overrides: { "/suggestions?status=pending": () => pending("ses_ash") } })
    const update = await ArtifactUpdateTool.init()
    await expect(update.execute({ id: "art_D", body: canvas("New"), base_version: 3 }, ctxWith())).rejects.toThrow(
      "Ash on Pricing",
    )
    expect(calls.some((c) => c.url.endsWith("/versions"))).toBe(false)

    // This session's own pending change doesn't block it.
    fetchSpy?.mockRestore()
    calls.length = 0
    mockCloud({
      overrides: {
        "/suggestions?status=pending": () => pending("ses_test"),
        "/versions": (body) => json(201, { id: "art_D", kind: "design", title: "Landing", current_version: 4, version: { version: 4, body: body.body, body_format: DESIGN, meta: {} } }),
      },
    })
    const written = await update.execute({ id: "art_D", body: canvas("New"), base_version: 3 }, ctxWith())
    expect(written.metadata.artifact).toMatchObject({ id: "art_D", version: 4 })
  })
  // ----- Phase 2: controls and pinned questions (SPEC-agent-canvas-phase2-2026-10-20) -----

  const radius = { id: "radius", label: "Corner radius", type: "range", layer: "title", prop: "radius", min: 0, max: 32, step: 1, value: 12 }
  const density = { id: "density", label: "Density", type: "select", layer: null, prop: "frame.padding", options: ["compact", "cozy"], value: "cozy" }

  test("pure: controls get shape ids and are checked against the change's layers", () => {
    const doc = parseCanvas(canvas(), DESIGN)!
    const change = buildArtboardChange(doc, { artboard: "hero", controls: [radius, density] })
    // A controls-only change keeps the frame's current layers.
    expect(change.layers).toEqual(change.before.layers)
    expect(change.controls).toEqual([
      { ...radius, layer: "shape:title" },
      { id: "density", label: "Density", type: "select", layer: null, prop: "frame.padding", options: ["compact", "cozy"], value: "cozy" },
    ] as any)
    const layers = change.layers
    expect(() => buildControls([{ ...radius, layer: "plans" }], layers, "Hero")).toThrow("isn't in Hero's layers")
    expect(() => buildControls([{ ...density, layer: "title" }], layers, "Hero")).toThrow("its layer must be null")
    expect(() => buildControls([{ ...radius, value: 40 }], layers, "Hero")).toThrow("outside 0 to 32")
    expect(() => buildControls([{ ...radius, min: 32, max: 0 }], layers, "Hero")).toThrow("min must be less than max")
    expect(() => buildControls([{ ...density, value: "roomy" }], layers, "Hero")).toThrow("one of compact, cozy")
    expect(() => buildControls([radius, radius], layers, "Hero")).toThrow("use the id")
    expect(() => buildControls(Array.from({ length: 13 }, (_, i) => ({ ...radius, id: `r${i}` })), layers, "Hero")).toThrow("at most 12")
    // Nothing to change at all is refused.
    expect(() => buildArtboardChange(doc, { artboard: "hero" })).toThrow("no layers, frame patch or controls")
  })

  test("suggest: posts controls with the artboard change", async () => {
    mockCloud()
    const tool = await ArtifactSuggestTool.init()
    await tool.execute(
      { id: "art_D", base_version: 3, changes: [{ kind: "artboard", artboard: "hero", controls: [radius, density], reason: "Pick a radius" }] } as any,
      ctxWith(),
    )
    const post = calls.find((c) => c.url.endsWith("/art_D/suggestions") && c.method === "POST")!
    const change = post.body.changes[0].change
    expect(change.controls.map((c: any) => [c.id, c.layer])).toEqual([
      ["radius", "shape:title"],
      ["density", null],
    ])
    expect(change.layers[0]).toMatchObject({ id: "shape:title", artboard: "shape:hero", props: { text: "Hi" } })
  })

  test("pure: a pin resolves the frame and layer to shape ids", () => {
    const doc = parseCanvas(canvas(), DESIGN)!
    expect(pinAnchor(doc, { artboard: "hero" })).toEqual({ kind: "element", shapeId: "shape:hero", artboard: "shape:hero", label: "Hero" })
    expect(pinAnchor(doc, { anchor: { kind: "element", shapeId: "title", extra: 1 } })).toEqual({
      kind: "element",
      shapeId: "shape:title",
      artboard: "shape:hero",
      label: "title",
      extra: 1,
    })
    expect(() => pinAnchor(doc, { artboard: "footer" })).toThrow("Its artboards: Hero")
    expect(() => pinAnchor(doc, { anchor: { kind: "element", shapeId: "nope" }, artboard: "hero" })).toThrow('no layer "nope"')
    expect(() => pinAnchor(doc, { anchor: { kind: "element", shapeId: "plans" }, artboard: "hero" })).toThrow("is on artboard pricing")
    expect(() => pinAnchor(doc, {})).toThrow("pass artboard")
  })

  test("comment: an agent's question about a frame posts as a pin under its name", async () => {
    mockCloud({ overrides: { "/art_D/comments": (body) => json(201, { id: "cmt_1", author_id: "assistant", agent_name: body.agent_name, anchor: body.anchor }) } })
    const tool = await ArtifactCommentTool.init()
    const result = await tool.execute({ id: "art_D", body: "Dark or light hero?", artboard: "hero" } as any, ctxWith())
    const post = calls.find((c) => c.url.endsWith("/art_D/comments") && c.method === "POST")!
    expect(post.body).toEqual({
      body: "Dark or light hero?",
      agent_name: "Oki",
      anchor: { kind: "element", shapeId: "shape:hero", artboard: "shape:hero", label: "Hero" },
    })
    expect(result.output).toContain("pinned to Hero")
    // artboard on something that isn't a canvas is a clear error.
    fetchSpy?.mockRestore()
    calls.length = 0
    mockCloud({
      overrides: {
        "/art_D": () =>
          json(200, { id: "art_D", kind: "doc", title: "Notes", current_version: 1, version: { version: 1, body: "# Hi", body_format: "text/markdown", meta: {} } }),
      },
    })
    await expect(tool.execute({ id: "art_D", body: "q", artboard: "hero" } as any, ctxWith())).rejects.toThrow("artboard is for design canvases")
    expect(calls.some((c) => c.url.endsWith("/comments"))).toBe(false)
  })
})
