import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { discoverSubscriptionFabric, providersFromCatalog } from "../../src/runtime/providers/fabric/discovery"
import { createHash } from "node:crypto"
import {
  FABRIC_INLINE_LIMIT_BYTES,
  SubscriptionFabricLanguageModel,
  progressFromEvent,
} from "../../src/runtime/providers/fabric/language-model"

const originalFetch = globalThis.fetch

beforeEach(() => {
  process.env.ALLTERNIT_API_URL = "http://api.test"
  process.env.ALLTERNIT_API_TOKEN = "runtime-token"
})

afterEach(() => {
  globalThis.fetch = originalFetch
  delete process.env.ALLTERNIT_API_URL
  delete process.env.ALLTERNIT_API_TOKEN
})

const catalog = [
  { id: "subs/chatgpt:fast", name: "x", provider: "chatgpt", tier: "fast", health: "ready", fabric: { capability: "chat.create", options: { model_class: "fast" } } },
  { id: "subs/chatgpt:reasoning", name: "x", provider: "chatgpt", tier: "flagship", health: "ready", fabric: { capability: "chat.create", options: { model_class: "reasoning" } } },
  // a second account with the same class does not duplicate the model
  { id: "subs/chatgpt:fast", name: "y", provider: "chatgpt", tier: "fast", health: "ready", fabric: { capability: "chat.create", options: { model_class: "fast" } } },
  { id: "subs/claude:standard", name: "z", provider: "claude", tier: "standard", health: "ready", fabric: { capability: "chat.create", options: { model_class: "standard" } } },
]

function sse(events: Array<{ event: string; data: unknown }>, opts: { holdOpen?: boolean } = {}): Response {
  const enc = new TextEncoder()
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(enc.encode(": connected\n\n"))
      for (const e of events) c.enqueue(enc.encode(`event: ${e.event}\ndata: ${JSON.stringify(e.data)}\n\n`))
      if (!opts.holdOpen) c.close()
    },
  })
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } })
}

type Call = { method: string; path: string; headers: Record<string, string>; body: any }

/** A fake allternit-api forwarder in front of a fake gateway. */
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

async function run(model: SubscriptionFabricLanguageModel, options: any) {
  const { stream } = await model.doStream(options)
  const parts: any[] = []
  const reader = stream.getReader()
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    parts.push(value)
  }
  return parts
}

const userTurn = (text: string) => ({ role: "user", content: [{ type: "text", text }] })
const headers = (extra: Record<string, string> = {}) => ({
  "x-gizzi-session": "ses_1",
  "x-gizzi-request": "msg_1",
  "x-allternit-human-action": "ha_1",
  ...extra,
})

describe("subscription fabric discovery", () => {
  test("catalog → one subs-<provider> per subscription, one model per class, no tool calls", () => {
    const providers = providersFromCatalog(catalog as any)
    expect(providers.map((p) => p.id)).toEqual(["subs-chatgpt", "subs-claude"])
    expect(providers[0].models.map((m) => m.id)).toEqual(["fast", "reasoning"])
    expect(providers[0].name).toBe("ChatGPT (subscription)")
    expect(providers[0].options).toEqual({ runtime: "fabric", fabricProvider: "chatgpt" })
  })

  test("reads the catalog through the forwarder with the runtime token; unbound → no models", async () => {
    const calls = fakeForwarder((c) => (c.path === "/v1/catalog" ? json(catalog) : json({}, 404)))
    const found = await discoverSubscriptionFabric()
    expect(found.map((p) => p.id)).toEqual(["subs-chatgpt", "subs-claude"])
    expect(calls[0].headers["authorization"]).toBe("Bearer runtime-token")

    fakeForwarder(() => json({ error: "sessions_computer_not_bound" }, 409))
    expect(await discoverSubscriptionFabric()).toEqual([])
  })
})

describe("SubscriptionFabricLanguageModel", () => {
  test("first turn: chat.create on the session thread, deltas stream, done.text fills the rest", async () => {
    const calls = fakeForwarder((c) => {
      if (c.method === "POST" && c.path === "/v1/tasks") return json({ task_id: "t1", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t1/events")
        return sse([
          { event: "task.status", data: { status: "running" } },
          { event: "reply", data: { event: { type: "reply.text.delta", delta: "Hello" } } },
          { event: "reply", data: { event: { type: "reply.text.delta", delta: " there" } } },
          { event: "task.status", data: { status: "completed" } },
        ])
      if (c.path === "/v1/tasks/t1") return json({ task_id: "t1", status: "completed", result: { text: "Hello there, friend", artifact_ids: [] } })
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })

    const submit = calls.find((c) => c.method === "POST")!
    expect(submit.body).toMatchObject({
      capability: "chat.create",
      prompt: "hi",
      thread_id: "ses_1",
      options: { model_class: "fast" },
      routing: { provider: "chatgpt" },
      idempotency_key: "gizzi-ses_1-msg_1",
    })
    expect(submit.headers["x-allternit-human-action"]).toBe("ha_1")
    const deltas = parts.filter((p) => p.type === "text-delta").map((p) => p.delta)
    expect(deltas).toEqual(["Hello", " there", ", friend"])
    expect(parts.at(-1)).toMatchObject({ type: "finish", finishReason: "stop" })
  })

  test("later turn: chat.continue; an unmapped thread falls back to chat.create with the same key + action", async () => {
    const posts: any[] = []
    fakeForwarder((c) => {
      if (c.method === "POST" && c.path === "/v1/tasks") {
        posts.push({ body: c.body, action: c.headers["x-allternit-human-action"] })
        if (c.body.capability === "chat.continue") return json({ error: "thread_not_mapped" }, 409)
        return json({ task_id: "t2", status: "queued" }, 201)
      }
      if (c.path === "/v1/tasks/t2/events") return sse([{ event: "task.status", data: { status: "completed" } }])
      if (c.path === "/v1/tasks/t2") return json({ task_id: "t2", status: "completed", result: { text: "ok", artifact_ids: [] } })
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, {
      prompt: [userTurn("a"), { role: "assistant", content: [{ type: "text", text: "b" }] }, userTurn("c")],
      headers: headers(),
    })
    expect(posts.map((p) => p.body.capability)).toEqual(["chat.continue", "chat.create"])
    expect(new Set(posts.map((p) => p.body.idempotency_key)).size).toBe(1)
    expect(posts.every((p) => p.action === "ha_1")).toBe(true)
    expect(posts[1].body.prompt).toBe("c")
    expect(parts.filter((p) => p.type === "text-delta").map((p) => p.delta).join("")).toBe("ok")
  })

  test("D16: no human action → no task is submitted, a plain error is shown", async () => {
    const calls = fakeForwarder(() => json({}, 500))
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, {
      prompt: [userTurn("hi")],
      headers: { "x-gizzi-session": "ses_1" },
    })
    expect(calls).toHaveLength(0)
    const error = parts.find((p) => p.type === "error")
    expect(String(error.error.message)).toContain("only run when you send a message yourself")
  })

  test("disclosure_required from the forwarder surfaces in plain words", async () => {
    fakeForwarder(() => json({ error: "disclosure_required", provider: "chatgpt", version: 1 }, 403))
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    const error = parts.find((p) => p.type === "error")
    expect(String(error.error.message)).toContain("acknowledge the subscription disclosure")
  })

  test("a dropped event stream reconnects until the task is terminal", async () => {
    let streams = 0
    let polls = 0
    fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t3", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t3/events") {
        streams++
        return streams === 1
          ? sse([{ event: "reply", data: { event: { type: "reply.text.delta", delta: "part one" } } }])
          : sse([
              { event: "reply", data: { event: { type: "reply.text.delta", delta: " part two" } } },
              { event: "task.status", data: { status: "completed" } },
            ])
      }
      if (c.path === "/v1/tasks/t3") {
        polls++
        return json(
          polls === 1
            ? { task_id: "t3", status: "streaming", result: null }
            : { task_id: "t3", status: "completed", result: { text: "part one part two", artifact_ids: [] } },
        )
      }
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    expect(streams).toBe(2)
    expect(parts.filter((p) => p.type === "text-delta").map((p) => p.delta).join("")).toBe("part one part two")
  })

  test("needs_user ends the turn with what the person has to do", async () => {
    fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t4", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t4/events") return sse([{ event: "task.status", data: { status: "needs_user" } }])
      return json({ task_id: "t4", status: "needs_user", status_detail: "not logged in", result: null, error: null })
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    expect(String(parts.find((p) => p.type === "error").error.message)).toBe("ChatGPT needs you: not logged in.")
  })

  test("gateway progress events become raw progress parts (label, fraction, heartbeat)", async () => {
    fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t6", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t6/events")
        return sse([
          { event: "submitted", data: { t: "submitted", provider_url: null } },
          { event: "progress", data: { t: "progress", label: "Searching the web", fraction: 0.25 } },
          { event: "progress.heartbeat", data: { t: "progress.heartbeat", elapsed_s: 120, last_change_at: "x" } },
          { event: "artifact.ready", data: { t: "artifact.ready", meta: { type: "image" } } },
          { event: "task.status", data: { status: "completed" } },
        ])
      return json({ task_id: "t6", status: "completed", result: { text: "done", artifact_ids: [] } })
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    const progress = parts.filter((p) => p.type === "raw" && p.raw.__gizzi === "progress").map((p) => p.raw)
    expect(progress).toEqual([
      { __gizzi: "progress", label: "ChatGPT is working on it" },
      { __gizzi: "progress", label: "Searching the web", fraction: 0.25 },
      { __gizzi: "progress", elapsedS: 120 },
      { __gizzi: "progress", label: "Image ready" },
    ])
    // Progress never leaks into the reply text.
    expect(parts.filter((p) => p.type === "text-delta").map((p) => p.delta).join("")).toBe("done")
  })

  test("artifacts: downloaded through the forwarder, sha256-checked, emitted as file parts after the text", async () => {
    const png = new Uint8Array([137, 80, 78, 71, 1, 2, 3])
    const sha = createHash("sha256").update(png).digest("hex")
    const calls = fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t7", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t7/events")
        return sse([
          { event: "reply", data: { event: { type: "reply.text.delta", delta: "Here it is." } } },
          { event: "task.status", data: { status: "completed" } },
        ])
      if (c.path === "/v1/tasks/t7")
        return json({ task_id: "t7", status: "completed", result: { text: "Here it is.", artifact_ids: ["art_1"] } })
      if (c.path === "/v1/artifacts/art_1")
        return json({ artifact_id: "art_1", type: "image", mime_type: "image/png", format: "png", title: "A cat", storage: { sha256: sha, size_bytes: png.length } })
      if (c.path === "/v1/artifacts/art_1/download")
        return new Response(png, {
          status: 200,
          headers: { "content-type": "image/png", "content-disposition": 'attachment; filename="art_1.png"', "x-artifact-sha256": sha },
        })
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("draw a cat")], headers: headers() })

    expect(calls.find((c) => c.path === "/v1/artifacts/art_1/download")?.headers["authorization"]).toBe("Bearer runtime-token")
    const types = parts.map((p) => (p.type === "raw" ? `raw:${p.raw.__gizzi}` : p.type))
    const fileIdx = types.indexOf("file")
    expect(fileIdx).toBeGreaterThan(types.indexOf("text-end"))
    expect(types[fileIdx - 1]).toBe("raw:generated_file")
    expect(parts[fileIdx - 1].raw).toEqual({
      __gizzi: "generated_file",
      title: "A cat",
      sourceUri: "fabric-artifact://art_1",
      mediaType: "image/png",
      filename: "art_1.png",
    })
    expect(parts[fileIdx]).toMatchObject({ type: "file", mediaType: "image/png" })
    expect(Array.from(parts[fileIdx].data)).toEqual(Array.from(png))
    expect(parts.at(-1)).toMatchObject({ type: "finish", finishReason: "stop" })
  })

  test("artifacts: a sha256 mismatch drops the file and says so in the reply", async () => {
    fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t8", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t8/events") return sse([{ event: "task.status", data: { status: "completed" } }])
      if (c.path === "/v1/tasks/t8") return json({ task_id: "t8", status: "completed", result: { text: "ok", artifact_ids: ["art_2"] } })
      if (c.path === "/v1/artifacts/art_2") return json({ artifact_id: "art_2", mime_type: "image/png", storage: { sha256: "00".repeat(32) } })
      if (c.path === "/v1/artifacts/art_2/download")
        return new Response(new Uint8Array([1, 2, 3]), { status: 200, headers: { "content-type": "image/png", "x-artifact-sha256": "00".repeat(32) } })
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    expect(parts.some((p) => p.type === "file")).toBe(false)
    const text = parts.filter((p) => p.type === "text-delta").map((p) => p.delta).join("")
    expect(text).toContain("could not be downloaded intact")
    expect(text).toContain("art_2")
  })

  test("artifacts: a file over the inline limit is not downloaded; it points at the forwarder URL", async () => {
    const calls = fakeForwarder((c) => {
      if (c.method === "POST") return json({ task_id: "t9", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t9/events") return sse([{ event: "task.status", data: { status: "completed" } }])
      if (c.path === "/v1/tasks/t9") return json({ task_id: "t9", status: "completed", result: { text: "deck", artifact_ids: ["big"] } })
      if (c.path === "/v1/artifacts/big")
        return json({ artifact_id: "big", mime_type: "application/pdf", format: "pdf", storage: { sha256: "ab", size_bytes: FABRIC_INLINE_LIMIT_BYTES + 1 } })
      return json({}, 404)
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers() })
    expect(calls.some((c) => c.path.endsWith("/download"))).toBe(false)
    const raw = parts.find((p) => p.type === "raw" && p.raw.__gizzi === "generated_file").raw
    expect(raw).toMatchObject({ url: "/api/v1/subscriptions/gateway/v1/artifacts/big/download", filename: "big.pdf", mediaType: "application/pdf" })
    expect(parts.some((p) => p.type === "file")).toBe(false)
  })

  test("progressFromEvent ignores non-progress events", () => {
    expect(progressFromEvent("reply", {}, "ChatGPT")).toBeNull()
    expect(progressFromEvent("task.status", { status: "running" }, "ChatGPT")).toBeNull()
    expect(progressFromEvent("progress", { label: "  " }, "ChatGPT")).toBeNull()
  })

  test("abort cancels the gateway task", async () => {
    const controller = new AbortController()
    let cancelled = false
    fakeForwarder((c) => {
      if (c.method === "POST" && c.path === "/v1/tasks") return json({ task_id: "t5", status: "queued" }, 201)
      if (c.path === "/v1/tasks/t5/cancel") {
        cancelled = true
        return json({ task_id: "t5", status: "cancelled" })
      }
      if (c.path === "/v1/tasks/t5/events") {
        setTimeout(() => controller.abort(), 20)
        return sse([{ event: "task.status", data: { status: "running" } }], { holdOpen: true })
      }
      return json({ task_id: "t5", status: "cancelled", result: null })
    })
    const model = new SubscriptionFabricLanguageModel("subs-chatgpt", "chatgpt", "fast")
    const parts = await run(model, { prompt: [userTurn("hi")], headers: headers(), abortSignal: controller.signal })
    await new Promise((r) => setTimeout(r, 20))
    expect(cancelled).toBe(true)
    expect(parts.at(-1)).toMatchObject({ type: "finish", finishReason: "stop" })
  })
})
