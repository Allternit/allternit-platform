import { describe, expect, test, spyOn, afterEach, beforeEach } from "bun:test"
import {
  ArtifactCreateTool,
  ArtifactReadTool,
  ArtifactUpdateTool,
  MAX_MODEL_BODY,
} from "../../src/runtime/tools/builtins/artifact"
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

const ENV = ["ALLTERNIT_API_KEY", "ALLTERNIT_API_TOKEN", "ALLTERNIT_CLOUD_API_URL", "GIZZI_ARTIFACTS_OFFLINE"] as const
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
