// @ts-nocheck
import { beforeEach, describe, expect, mock, test } from "bun:test"
import path from "path"
import { APICallError } from "ai"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionProcessor } from "../../src/runtime/session/processor"
import { Identifier } from "../../src/shared/id/id"
import { Bus } from "../../src/shared/bus"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

// Scriptable LLM.stream stub. Each call consumes one script entry: either it throws the
// given provider error, or it returns a stream yielding the given events.
const streamCalls: { providerID: string; modelID: string }[] = []
let streamScript: Array<{ throw?: unknown; events?: unknown[] }> = []

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async (input: any) => {
      streamCalls.push({ providerID: input.model.providerID, modelID: input.model.id })
      const step = streamScript.shift()
      if (!step) throw new Error("unexpected LLM.stream call")
      if (step.throw) throw step.throw
      const events = step.events ?? []
      return {
        fullStream: (async function* () {
          for (const event of events) yield event
        })(),
      }
    },
  },
}))

// Provider stub: resolves every model to a fake Provider.Model unless the model was
// marked unavailable (used to exercise "fallback model unavailable, skipping").
const unavailableModels = new Set<string>()

mock.module("../../src/runtime/providers/provider", () => ({
  Provider: {
    parseModel: (model: string) => {
      const [providerID, ...rest] = model.split("/")
      return { providerID, modelID: rest.join("/") }
    },
    getModel: async (providerID: string, modelID: string) => {
      if (unavailableModels.has(`${providerID}/${modelID}`)) {
        throw new Error(`Model not found: ${providerID}/${modelID}`)
      }
      return fakeModel(providerID, modelID)
    },
    createRotationState: () => ({ triedProfileIds: new Set<string>() }),
    rotateAuth: async () => undefined,
    prepareAuth: async () => undefined,
  },
}))

function fakeModel(providerID: string, modelID: string) {
  return {
    id: modelID,
    providerID,
    api: { id: modelID, url: "https://api.test", npm: "@ai-sdk/openai-compatible" },
    name: modelID,
    cost: { input: 1, output: 1, cache: { read: 0, write: 0 } },
    limit: { context: 1_000_000, output: 8192 },
    options: {},
    headers: {},
    status: "active",
    release_date: "2025-01-01",
  }
}

// Retryable provider error: classified as rate_limit by shared/util/provider-error.ts.
function rateLimitError() {
  return new APICallError({
    message: "Rate limit reached",
    url: "https://api.test/v1/chat/completions",
    requestBodyValues: {},
    statusCode: 429,
    responseHeaders: {},
    responseBody: '{"error":{"code":"rate_limit","message":"Rate limit reached"}}',
    isRetryable: true,
  })
}

// Non-retryable provider error: classified as auth by shared/util/provider-error.ts.
function authError(providerID: string) {
  return new APICallError({
    message: `Unauthorized: {"error":{"code":"invalid_api_key","message":"Invalid API key for ${providerID}"}}`,
    url: "https://api.test/v1/chat/completions",
    requestBodyValues: {},
    statusCode: 401,
    responseHeaders: {},
    responseBody: `{"error":{"code":"invalid_api_key","message":"Invalid API key for ${providerID}"}}`,
    isRetryable: false,
  })
}

function okStep() {
  return { events: [{ type: "start" }] }
}

function baseConfig(extra?: Record<string, unknown>) {
  return {
    // keep same-model retries fast and deterministic in tests
    experimental: { retry_max_attempts: 2, retry_max_delay_ms: 1 },
    ...extra,
  }
}

async function withTmpdir<T>(config: Record<string, unknown>, fn: (tmp: { path: string }) => Promise<T>): Promise<T> {
  const tmp = await tmpdir({
    git: true,
    // NOTE: the config loader reads gizzi.json{,c} (not the legacy name), so write it directly
    init: async (dir) => {
      await Bun.write(path.join(dir, "gizzi.json"), JSON.stringify(config))
    },
  })
  try {
    return await Instance.provide({
      directory: tmp.path,
      fn: () => fn(tmp),
    })
  } finally {
    await tmp[Symbol.asyncDispose]()
  }
}

// Builds a session with a user + assistant message and drives one processor.process call
// with the scripted stream. Returns the result plus captured bus events.
async function runProcessor(
  tmp: { path: string },
  options?: { fallbackModels?: { providerID: string; modelID: string }[] },
) {
  const session = await Session.create({})
  const user = await Session.updateMessage({
    id: Identifier.ascending("message"),
    sessionID: session.id,
    role: "user",
    agent: "build",
    model: { providerID: "provider-a", modelID: "model-a" },
    time: { created: Date.now() },
  })
  const assistantMessage = await Session.updateMessage({
    id: Identifier.ascending("message"),
    parentID: user.id,
    role: "assistant",
    mode: "build",
    agent: "build",
    sessionID: session.id,
    path: { cwd: tmp.path, root: tmp.path },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    modelID: "model-a",
    providerID: "provider-a",
    time: { created: Date.now() },
  })
  const model = fakeModel("provider-a", "model-a")
  const abort = new AbortController()
  const processor = SessionProcessor.create({
    assistantMessage,
    sessionID: session.id,
    model,
    abort: abort.signal,
    fallbackModels: options?.fallbackModels,
  })
  const events = { fallback: [] as any[], error: [] as any[] }
  const unsubFallback = Bus.subscribe(Session.Event.ModelFallback, (event: any) => events.fallback.push(event.properties))
  const unsubError = Bus.subscribe(Session.Event.Error, (event: any) => events.error.push(event.properties))
  const result = await processor.process({
    user,
    sessionID: session.id,
    model,
    agent: { name: "build" },
    system: [],
    messages: [{ role: "user", content: "hello" }],
    tools: {},
    abort: abort.signal,
  })
  unsubFallback()
  unsubError()
  return { session, assistantMessage, processor, result, events }
}

describe("session.processor fallback", () => {
  beforeEach(() => {
    streamCalls.length = 0
    streamScript = []
    unavailableModels.clear()
  })

  // NOTE: these "no chain" tests assume the global gizzi config on the test machine does
  // not define routing.fallbacks (the field is introduced by this change).

  test("no chain: retryable error retries same model then surfaces (current behavior)", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: rateLimitError() }, { throw: rateLimitError() }, { throw: rateLimitError() }]
      const { session, assistantMessage, result, events } = await runProcessor(tmp)

      expect(result).toBe("stop")
      // initial attempt + 2 configured retries, all on the primary model
      expect(streamCalls).toEqual([
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-a", modelID: "model-a" },
      ])
      expect(events.fallback.length).toBe(0)
      expect(assistantMessage.error?.name).toBe("APIError")
      expect(assistantMessage.error?.data?.isRetryable).toBe(false)
      expect(assistantMessage.error?.data?.message).toContain("retry limit reached after 2 attempts")
      expect(events.error.length).toBeGreaterThan(0)
      await Session.remove(session.id)
    })
  })

  test("no chain: non-retryable provider error surfaces immediately (current behavior)", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: authError("provider-a") }]
      const { session, assistantMessage, result, events } = await runProcessor(tmp)

      expect(result).toBe("stop")
      expect(streamCalls).toEqual([{ providerID: "provider-a", modelID: "model-a" }])
      expect(events.fallback.length).toBe(0)
      expect(assistantMessage.error?.name).toBe("APIError")
      expect(events.error.length).toBeGreaterThan(0)
      await Session.remove(session.id)
    })
  })

  test("retry exhaustion fails over to the next per-request model and publishes session.model_fallback", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: rateLimitError() }, { throw: rateLimitError() }, { throw: rateLimitError() }, okStep()]
      const { session, assistantMessage, result, events } = await runProcessor(tmp, {
        fallbackModels: [{ providerID: "provider-b", modelID: "model-b" }],
      })

      expect(result).toBe("continue")
      expect(streamCalls).toEqual([
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-b", modelID: "model-b" },
      ])
      expect(events.fallback.length).toBe(1)
      expect(events.fallback[0].sessionID).toBe(session.id)
      expect(events.fallback[0].from).toEqual({ providerID: "provider-a", modelID: "model-a" })
      expect(events.fallback[0].to).toEqual({ providerID: "provider-b", modelID: "model-b" })
      expect(events.fallback[0].reason).toContain("retry limit reached after 2 attempts")
      expect(assistantMessage.error).toBeUndefined()
      expect(assistantMessage.providerID).toBe("provider-b")
      expect(assistantMessage.modelID).toBe("model-b")
      await Session.remove(session.id)
    })
  })

  test("non-retryable error skips straight to the next model without same-model retries", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: authError("provider-a") }, okStep()]
      const { session, assistantMessage, result, events } = await runProcessor(tmp, {
        fallbackModels: [{ providerID: "provider-b", modelID: "model-b" }],
      })

      expect(result).toBe("continue")
      // no same-model retry attempts before the switch
      expect(streamCalls).toEqual([
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-b", modelID: "model-b" },
      ])
      expect(events.fallback.length).toBe(1)
      expect(events.fallback[0].from).toEqual({ providerID: "provider-a", modelID: "model-a" })
      expect(events.fallback[0].to).toEqual({ providerID: "provider-b", modelID: "model-b" })
      expect(events.fallback[0].reason).toBe("auth")
      expect(assistantMessage.error).toBeUndefined()
      await Session.remove(session.id)
    })
  })

  test("per-request fallbackModels override config routing.fallbacks", async () => {
    await withTmpdir(
      baseConfig({
        routing: {
          tiers: {
            simple: "provider-c/model-c",
            standard: "provider-c/model-c",
            complex: "provider-c/model-c",
            reasoning: "provider-c/model-c",
          },
          fallbacks: ["provider-c/model-c"],
        },
      }),
      async (tmp) => {
        streamScript = [{ throw: authError("provider-a") }, okStep()]
        const { session, result, events } = await runProcessor(tmp, {
          fallbackModels: [{ providerID: "provider-b", modelID: "model-b" }],
        })

        expect(result).toBe("continue")
        expect(streamCalls).toEqual([
          { providerID: "provider-a", modelID: "model-a" },
          { providerID: "provider-b", modelID: "model-b" },
        ])
        expect(events.fallback.length).toBe(1)
        expect(events.fallback[0].to).toEqual({ providerID: "provider-b", modelID: "model-b" })
        await Session.remove(session.id)
      },
    )
  })

  test("config routing.fallbacks is used when no per-request chain is given", async () => {
    await withTmpdir(
      baseConfig({
        routing: {
          tiers: {
            simple: "provider-c/model-c",
            standard: "provider-c/model-c",
            complex: "provider-c/model-c",
            reasoning: "provider-c/model-c",
          },
          fallbacks: ["provider-c/model-c"],
        },
      }),
      async (tmp) => {
        streamScript = [{ throw: authError("provider-a") }, okStep()]
        const { session, result, events } = await runProcessor(tmp)

        expect(result).toBe("continue")
        expect(streamCalls).toEqual([
          { providerID: "provider-a", modelID: "model-a" },
          { providerID: "provider-c", modelID: "model-c" },
        ])
        expect(events.fallback.length).toBe(1)
        expect(events.fallback[0].to).toEqual({ providerID: "provider-c", modelID: "model-c" })
        await Session.remove(session.id)
      },
    )
  })

  test("chain exhaustion surfaces the error as today", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: authError("provider-a") }, { throw: authError("provider-b") }]
      const { session, assistantMessage, result, events } = await runProcessor(tmp, {
        fallbackModels: [{ providerID: "provider-b", modelID: "model-b" }],
      })

      expect(result).toBe("stop")
      expect(streamCalls).toEqual([
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-b", modelID: "model-b" },
      ])
      // one fallback happened, then the chain was exhausted and the error surfaced
      expect(events.fallback.length).toBe(1)
      expect(assistantMessage.error?.name).toBe("APIError")
      expect(assistantMessage.error?.data?.message).toContain("Invalid API key for provider-b")
      expect(events.error.length).toBeGreaterThan(0)
      await Session.remove(session.id)
    })
  })

  test("chain never loops back to a model that was already tried", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [{ throw: authError("provider-a") }, { throw: authError("provider-b") }]
      const { session, result, events } = await runProcessor(tmp, {
        fallbackModels: [
          { providerID: "provider-b", modelID: "model-b" },
          // duplicates and the primary itself must be skipped
          { providerID: "provider-b", modelID: "model-b" },
          { providerID: "provider-a", modelID: "model-a" },
        ],
      })

      expect(result).toBe("stop")
      expect(streamCalls).toEqual([
        { providerID: "provider-a", modelID: "model-a" },
        { providerID: "provider-b", modelID: "model-b" },
      ])
      expect(events.fallback.length).toBe(1)
      await Session.remove(session.id)
    })
  })
})

describe("session.processor streaming parts", () => {
  beforeEach(() => {
    streamCalls.length = 0
    streamScript = []
    unavailableModels.clear()
  })

  // Bridges (allternit-api agent-chat) hold a part's deltas until the part is
  // declared. Declaring a text part only at text-end made every reply arrive
  // in one burst at the end (found live on the Subscription Fabric).
  test("a text part is declared (PartUpdated) before its first delta", async () => {
    await withTmpdir(baseConfig(), async (tmp) => {
      streamScript = [
        {
          events: [
            { type: "start" },
            { type: "text-start", id: "t1" },
            { type: "text-delta", id: "t1", text: "Hel" },
            { type: "text-delta", id: "t1", text: "lo" },
            { type: "text-end", id: "t1" },
          ],
        },
      ]
      const order: string[] = []
      const { MessageV2 } = await import("../../src/runtime/session/message-v2")
      const unsubUpdated = Bus.subscribe(MessageV2.Event.PartUpdated, (e: any) => {
        if (e.properties.part.type === "text") order.push(`updated:${e.properties.part.id}`)
      })
      const unsubDelta = Bus.subscribe(MessageV2.Event.PartDelta, (e: any) => order.push(`delta:${e.properties.partID}`))
      await runProcessor(tmp)
      unsubUpdated()
      unsubDelta()
      const firstDelta = order.findIndex((x) => x.startsWith("delta:"))
      expect(firstDelta).toBeGreaterThan(-1)
      const partID = order[firstDelta].slice("delta:".length)
      expect(order.indexOf(`updated:${partID}`)).toBeGreaterThan(-1)
      expect(order.indexOf(`updated:${partID}`)).toBeLessThan(firstDelta)
    })
  })
})
