import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import { discoverSiwc } from "../../src/runtime/providers/siwc/discovery"
import { siwcConfigured } from "../../src/runtime/providers/siwc/broker"
import {
  SiwcLanguageModel,
  fabricClassFor,
  toResponsesBody,
} from "../../src/runtime/providers/siwc/language-model"

const originalFetch = globalThis.fetch

beforeEach(() => {
  process.env.ALLTERNIT_SIWC_BROKER_URL = "http://broker.test"
  process.env.ALLTERNIT_SIWC_BROKER_TOKEN = "launch-secret"
})

afterEach(() => {
  globalThis.fetch = originalFetch
  delete process.env.ALLTERNIT_SIWC_BROKER_URL
  delete process.env.ALLTERNIT_SIWC_BROKER_TOKEN
})

const json = (body: unknown, status = 200, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } })

function sse(events: Array<{ type: string; [k: string]: unknown }>): Response {
  const enc = new TextEncoder()
  const body = new ReadableStream<Uint8Array>({
    start(c) {
      for (const e of events) c.enqueue(enc.encode(`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`))
      c.close()
    },
  })
  return new Response(body, { status: 200, headers: { "content-type": "text/event-stream", "x-request-id": "req_1" } })
}

type Call = { url: string; method: string; headers: Record<string, string>; body: any }

/** A fake broker + a fake api.openai.com. */
function fakeNetwork(opts: {
  token?: string | null
  models?: unknown[]
  responses?: () => Response | Promise<Response>
}) {
  const calls: Call[] = []
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = String(input)
    const headers: Record<string, string> = {}
    new Headers(init?.headers).forEach((v, k) => (headers[k] = v))
    calls.push({ url, method: init?.method ?? "GET", headers, body: init?.body ? JSON.parse(String(init.body)) : undefined })
    if (url === "http://broker.test/v1/token") {
      return opts.token === null ? json({ error: "not_available", state: "signed_out" }, 409) : json({ access_token: opts.token ?? "at-1" })
    }
    if (url === "http://broker.test/v1/models") return json({ models: opts.models ?? [] })
    if (url === "https://api.openai.com/v1/responses") return opts.responses!()
    throw new Error(`unexpected fetch ${url}`)
  }) as typeof fetch
  return calls
}

async function run(model: { doStream(o: any): Promise<{ stream: ReadableStream<any> }> }, options: any) {
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

const prompt = [
  { role: "system", content: "Be brief." },
  { role: "user", content: [{ type: "text", text: "hi" }] },
  { role: "assistant", content: [{ type: "text", text: "hello" }] },
  { role: "user", content: [{ type: "text", text: "and now?" }] },
]

describe("request shape", () => {
  test("follows the documented preview limits", () => {
    const body = toResponsesBody("gpt-x", prompt)
    expect(body).toEqual({
      model: "gpt-x",
      instructions: "Be brief.",
      input: [
        { role: "user", content: [{ type: "input_text", text: "hi" }] },
        { role: "assistant", content: [{ type: "output_text", text: "hello" }] },
        { role: "user", content: [{ type: "input_text", text: "and now?" }] },
      ],
      store: false,
      stream: true,
    })
    for (const banned of ["temperature", "max_output_tokens", "previous_response_id", "tools", "metadata", "user", "top_p", "conversation"]) {
      expect(banned in body).toBe(false)
    }
    // no explicit system items in input
    expect((body.input as any[]).some((i) => i.role === "system")).toBe(false)
  })

  test("maps model slugs to fabric classes for the web-chat fallback", () => {
    expect(fabricClassFor("gpt-5-mini")).toBe("fast")
    expect(fabricClassFor("o3-pro")).toBe("reasoning")
    expect(fabricClassFor("gpt-5")).toBe("standard")
  })
})

describe("discovery", () => {
  test("is off without the Desktop broker env", async () => {
    delete process.env.ALLTERNIT_SIWC_BROKER_URL
    expect(siwcConfigured()).toBe(false)
    globalThis.fetch = (() => {
      throw new Error("must not call the network")
    }) as unknown as typeof fetch
    expect(await discoverSiwc()).toEqual([])
  })

  test("yields nothing while signed out or the flag is off (broker 409 for models)", async () => {
    globalThis.fetch = (async () => json({ error: "not_available" }, 409)) as unknown as typeof fetch
    expect(await discoverSiwc()).toEqual([])
  })

  test("takes over the subs-chatgpt lane with the account's real models", async () => {
    fakeNetwork({ models: [{ slug: "gpt-x", display_name: "GPT X" }, { slug: "gpt-x-mini", display_name: "GPT X Mini" }] })
    const [p] = await discoverSiwc()
    expect(p.id).toBe("subs-chatgpt")
    expect(p.options).toEqual({ runtime: "siwc", fabricProvider: "chatgpt" })
    expect(p.models.map((m) => m.id)).toEqual(["gpt-x", "gpt-x-mini"])
  })
})

describe("SiwcLanguageModel", () => {
  test("streams a Responses turn with the brokered token", async () => {
    const calls = fakeNetwork({
      responses: () =>
        sse([
          { type: "response.output_text.delta", delta: "Hel" },
          { type: "response.output_text.delta", delta: "lo" },
          { type: "response.completed", response: { usage: { input_tokens: 5, output_tokens: 2 } } },
        ]),
    })
    const parts = await run(new SiwcLanguageModel("subs-chatgpt", "gpt-x"), { prompt })
    expect(parts.filter((p) => p.type === "text-delta").map((p) => p.delta).join("")).toBe("Hello")
    const finish = parts.find((p) => p.type === "finish")
    expect(finish.finishReason).toBe("stop")
    expect(finish.usage).toEqual({ inputTokens: 5, outputTokens: 2, totalTokens: 7 })

    const req = calls.find((c) => c.url.endsWith("/v1/responses"))!
    expect(req.method).toBe("POST")
    expect(req.headers["authorization"]).toBe("Bearer at-1")
    expect(req.body.store).toBe(false)
    expect(req.body.stream).toBe(true)
    const tokenCall = calls.find((c) => c.url.endsWith("/v1/token"))!
    expect(tokenCall.headers["authorization"]).toBe("Bearer launch-secret")
  })

  test("a stream that ends without response.completed is a failure", async () => {
    fakeNetwork({ responses: () => sse([{ type: "response.output_text.delta", delta: "part" }]) })
    const parts = await run(new SiwcLanguageModel("subs-chatgpt", "gpt-x"), { prompt })
    const err = parts.find((p) => p.type === "error")
    expect(err.error.code).toBe("stream_interrupted")
    expect(parts.find((p) => p.type === "finish").finishReason).toBe("error")
  })

  test("a usage-limit failure mid-stream is surfaced with the usage link, not retried elsewhere", async () => {
    fakeNetwork({
      responses: () =>
        sse([{ type: "response.failed", response: { error: { code: "subscription_sharing_usage_limit_exceeded" } } }]),
    })
    const fallback = { doStream: () => { throw new Error("fallback must not run") } } as any
    const parts = await run(new SiwcLanguageModel("subs-chatgpt", "gpt-x", () => fallback), { prompt })
    const err = parts.find((p) => p.type === "error").error
    expect(err.code).toBe("subscription_sharing_usage_limit_exceeded")
    expect(err.message).toContain("https://chatgpt.com/settings/usage")
    expect(err.message).toContain("req_1")
  })

  test("429 before the stream opens keeps status, code and request id, and does not fall back", async () => {
    fakeNetwork({
      responses: () =>
        json({ error: { code: "subscription_sharing_usage_limit_exceeded" } }, 429, { "x-request-id": "req_9" }),
    })
    const fallback = { doStream: () => { throw new Error("fallback must not run") } } as any
    const model = new SiwcLanguageModel("subs-chatgpt", "gpt-x", () => fallback)
    const error: any = await model.doStream({ prompt }).catch((e) => e)
    expect(error.status).toBe(429)
    expect(error.code).toBe("subscription_sharing_usage_limit_exceeded")
    expect(error.requestID).toBe("req_9")
    expect(error.message).toContain("usage")
  })

  test("falls back to the web-chat adapter when there is no signed-in session", async () => {
    fakeNetwork({ token: null })
    const seen: any[] = []
    const fallback = {
      doStream: async (o: any) => {
        seen.push(o)
        return { stream: new ReadableStream({ start: (c) => c.close() }), rawCall: { rawPrompt: "", rawSettings: {} } }
      },
    } as any
    await new SiwcLanguageModel("subs-chatgpt", "gpt-x", () => fallback).doStream({ prompt })
    expect(seen).toHaveLength(1)
  })

  test("falls back on 401 and on a network failure", async () => {
    for (const responses of [() => json({ detail: "bad token" }, 401), () => { throw new Error("offline") }]) {
      fakeNetwork({ responses })
      let ran = 0
      const fallback = {
        doStream: async () => {
          ran++
          return { stream: new ReadableStream({ start: (c) => c.close() }), rawCall: { rawPrompt: "", rawSettings: {} } }
        },
      } as any
      await new SiwcLanguageModel("subs-chatgpt", "gpt-x", () => fallback).doStream({ prompt })
      expect(ran).toBe(1)
    }
  })

  test("with no fallback lane, a missing session says how to sign in", async () => {
    fakeNetwork({ token: null })
    const error: any = await new SiwcLanguageModel("subs-chatgpt", "gpt-x", () => undefined).doStream({ prompt }).catch((e) => e)
    expect(error.message).toContain("Sign in with ChatGPT")
  })
})
