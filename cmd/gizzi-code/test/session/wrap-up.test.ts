// @ts-nocheck
/**
 * Usage-limit wrap-up, end to end through SessionPrompt: a turn that crosses
 * limits.land_at mid-task gets `wrap_up_steps` more steps (with the wrap-up
 * reminder), then lands — paused until the window resets, limit state
 * "wrapped". The provider's windows arrive the way live headers do
 * (ProviderQuotas.record), in the middle of the turn.
 */
import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { Identifier } from "../../src/shared/id/id"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

type Step = { finish: "tool-calls" | "stop"; before?: () => void }
let script: Step[] = []
const calls: { system: string[] }[] = []

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async (input: any) => {
      // Title generation and other side agents: an empty, finished stream.
      if (input.agent?.name !== "build") {
        return { fullStream: (async function* () {})(), text: Promise.resolve("") }
      }
      calls.push({ system: input.system })
      const step = script.shift() ?? { finish: "stop" }
      step.before?.()
      return {
        fullStream: (async function* () {
          yield { type: "start" }
          yield { type: "start-step" }
          yield {
            type: "finish-step",
            finishReason: step.finish,
            usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 },
            providerMetadata: {},
          }
          yield { type: "finish", finishReason: step.finish }
        })(),
      }
    },
  },
}))

function fakeModel(providerID: string, modelID: string) {
  return {
    id: modelID,
    providerID,
    api: { id: modelID, url: "https://api.test", npm: "@ai-sdk/openai-compatible" },
    name: modelID,
    capabilities: { toolcall: true, attachment: false, reasoning: false, temperature: true, input: { text: true }, output: { text: true } },
    cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
    limit: { context: 1_000_000, output: 8192 },
    options: {},
    headers: {},
    variants: {},
    status: "active",
    release_date: "2025-01-01",
  }
}

const real = await import("../../src/runtime/providers/provider")
mock.module("../../src/runtime/providers/provider", () => ({
  Provider: {
    ...real.Provider,
    getModel: async (providerID: string, modelID: string) => fakeModel(providerID, modelID),
    getSmallModel: async () => undefined,
    defaultModel: async () => ({ providerID: "plan", modelID: "model-a" }),
    list: async () => ({}),
    resolveAuto: async (m: any) => m,
    prepareAuth: async () => undefined,
    isFabricModel: () => false,
    createRotationState: () => ({ triedProfileIds: new Set<string>() }),
    rotateAuth: async () => undefined,
  },
}))

const { SessionPrompt } = await import("../../src/runtime/session/prompt")
const { SessionPause } = await import("../../src/runtime/session/pause")
const { SessionLimit } = await import("../../src/runtime/session/limit")
const { ProviderQuotas } = await import("../../src/runtime/providers/quota")

const RESET = Date.now() + 3 * 3_600_000

function hit(ratio: number) {
  return () => ProviderQuotas.record("plan", [{ id: "5h", label: "5-hour", usedRatio: ratio, resetAt: new Date(RESET).toISOString() }])
}

async function withProject<T>(config: Record<string, unknown>, fn: () => Promise<T>) {
  const tmp = await tmpdir({
    git: true,
    init: async (dir) => {
      await Bun.write(path.join(dir, "gizzi.json"), JSON.stringify(config))
    },
  })
  try {
    return await Instance.provide({ directory: tmp.path, fn })
  } finally {
    await tmp[Symbol.asyncDispose]()
  }
}

async function send(sessionID: string) {
  return SessionPrompt.prompt({
    sessionID,
    agent: "build",
    model: { providerID: "plan", modelID: "model-a" },
    parts: [{ id: Identifier.ascending("part"), type: "text", text: "Fix the session cookie and update the tests" }],
  })
}

const wrapUpIn = (c: { system: string[] }) => c.system.some((s) => s.includes("Usage limit reached"))

beforeEach(() => {
  script = []
  calls.length = 0
  ProviderQuotas.clearCache()
  SessionLimit.reset()
})
afterEach(() => SessionPause.clearTimers())

describe("usage-limit wrap-up", () => {
  test("crossing land_at mid-turn wraps up within wrap_up_steps, then lands as wrapped", async () => {
    await withProject({ limits: { wrap_up_steps: 2 } }, async () => {
      const s = await Session.create({ title: "Session cookie" })
      // Step 1 runs at 50%; the provider reports 97% on step 2; the model would keep going.
      script = [
        { finish: "tool-calls", before: hit(0.5) },
        { finish: "tool-calls", before: hit(0.97) },
        { finish: "tool-calls" },
        { finish: "tool-calls" },
        { finish: "tool-calls" },
        { finish: "tool-calls" },
      ]
      await send(s.id)

      // 2 steps before the crossing is seen + 2 wrap-up steps, not all 6.
      expect(calls).toHaveLength(4)
      expect(calls.slice(0, 2).some(wrapUpIn)).toBe(false)
      expect(calls.slice(2).every(wrapUpIn)).toBe(true)

      const after = await Session.get(s.id)
      expect(after.paused).toMatchObject({ reason: "quota", providerID: "plan", until: RESET })
      expect(after.limit).toMatchObject({ state: "wrapped", providerID: "plan", windowID: "5h", resetAt: RESET })
      expect(SessionPause.scheduled(s.id)).toBe(true)

      // The next turn holds: the session is paused until the reset.
      calls.length = 0
      await send(s.id)
      expect(calls).toHaveLength(0)
    })
  }, 60_000)

  test("a model that finishes during wrap-up lands right away", async () => {
    await withProject({ limits: { wrap_up_steps: 3 } }, async () => {
      const s = await Session.create({ title: "Finish" })
      script = [{ finish: "tool-calls", before: hit(0.96) }, { finish: "stop" }, { finish: "tool-calls" }]
      await send(s.id)
      expect(calls).toHaveLength(2)
      expect((await Session.get(s.id)).limit?.state).toBe("wrapped")
    })
  }, 60_000)

  test("approaching publishes without changing the turn; resume clears the state", async () => {
    await withProject({}, async () => {
      const s = await Session.create({ title: "Approach" })
      hit(0.85)()
      script = [{ finish: "tool-calls" }, { finish: "stop" }]
      await send(s.id)
      expect(calls).toHaveLength(2)
      const after = await Session.get(s.id)
      expect(after.paused).toBeUndefined()
      expect(after.limit).toMatchObject({ state: "approaching", usedRatio: 0.85, windowID: "5h" })

      // Held before the turn at 96% → paused (not wrapped); resume clears.
      hit(0.96)()
      calls.length = 0
      await send(s.id)
      expect(calls).toHaveLength(0)
      expect((await Session.get(s.id)).limit?.state).toBe("paused")
      ProviderQuotas.clearCache()
      script = [{ finish: "stop" }]
      await SessionPause.resume(s.id)
      expect((await Session.get(s.id)).limit).toBeUndefined()
    })
  }, 60_000)
})
