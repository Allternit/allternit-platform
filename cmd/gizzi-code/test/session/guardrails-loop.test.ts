// @ts-nocheck
/**
 * O12 guardrails end to end through SessionPrompt with a scripted model:
 * the step cap, the tool-call cap and the stuck detector each end the turn
 * with a guardrail.tripped event instead of looping on.
 */
import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { Identifier } from "../../src/shared/id/id"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

type Step = { finish: "tool-calls" | "stop"; tool?: { name: string; input: any; output?: string; error?: string } }
let script: Step[] = []
const calls: { system: string[]; tail: string[] }[] = []
let callSeq = 0

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async (input: any) => {
      // Title generation and other side agents: an empty, finished stream.
      if (input.agent?.name !== "build") {
        return { fullStream: (async function* () {})(), text: Promise.resolve("") }
      }
      calls.push({ system: input.system, tail: input.tail ?? [] })
      const step = script.shift() ?? { finish: "stop" }
      const id = `call_${++callSeq}`
      return {
        fullStream: (async function* () {
          yield { type: "start" }
          yield { type: "start-step" }
          if (step.tool) {
            yield { type: "tool-input-start", id, toolName: step.tool.name }
            yield { type: "tool-call", toolCallId: id, toolName: step.tool.name, input: step.tool.input }
            if (step.tool.error !== undefined) yield { type: "tool-error", toolCallId: id, input: step.tool.input, error: new Error(step.tool.error) }
            else yield { type: "tool-result", toolCallId: id, input: step.tool.input, output: { output: step.tool.output ?? "", metadata: {}, title: "" } }
          }
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
const { Guardrails } = await import("../../src/runtime/session/guardrails")
const { Bus } = await import("../../src/shared/bus")

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

async function run(steps: Step[]) {
  script = steps
  const trips: any[] = []
  const unsub = Bus.subscribe(Guardrails.Event.Tripped, (e) => trips.push(e.properties.trip))
  const s = await Session.create({ title: "Guardrails" })
  await SessionPrompt.prompt({
    sessionID: s.id,
    agent: "build",
    model: { providerID: "plan", modelID: "model-a" },
    parts: [{ id: Identifier.ascending("part"), type: "text", text: "Keep going" }],
  })
  unsub()
  return trips
}

const loop = (n: number, step: (i: number) => Step) => Array.from({ length: n }, (_, i) => step(i))

beforeEach(() => {
  script = []
  calls.length = 0
})

describe("O12 guardrails in the loop", () => {
  test("step cap: the agent's steps limit ends a turn that keeps calling tools", async () => {
    await withProject({ agent: { build: { steps: 3 } } }, async () => {
      const trips = await run(loop(10, () => ({ finish: "tool-calls" })))
      expect(calls).toHaveLength(3)
      expect(trips.map((t) => t.kind)).toEqual(["max_steps"])
    })
  }, 60_000)

  test("tool-call cap ends the turn after the step that crossed it", async () => {
    await withProject({ agent: { build: { max_tool_calls: 2 } } }, async () => {
      const trips = await run(loop(10, (i) => ({ finish: "tool-calls", tool: { name: "read", input: { i }, output: `r${i}` } })))
      expect(calls).toHaveLength(3)
      expect(trips.map((t) => t.kind)).toEqual(["max_tool_calls"])
    })
  }, 60_000)

  test("stuck: same action + same observation x4 ends the turn", async () => {
    await withProject({}, async () => {
      const trips = await run(loop(10, () => ({ finish: "tool-calls", tool: { name: "read", input: { path: "a" }, output: "same" } })))
      expect(calls).toHaveLength(4)
      expect(trips.map((t) => t.kind)).toEqual(["repeat_action_observation"])
    })
  }, 60_000)

  test("stuck: the same failing action x3 ends the turn", async () => {
    await withProject({}, async () => {
      const trips = await run(loop(10, () => ({ finish: "tool-calls", tool: { name: "read", input: { path: "b" }, error: "boom" } })))
      expect(calls).toHaveLength(3)
      expect(trips.map((t) => t.kind)).toEqual(["repeat_error"])
    })
  }, 60_000)

  test("a normal turn is untouched", async () => {
    await withProject({}, async () => {
      const trips = await run([
        { finish: "tool-calls", tool: { name: "read", input: { path: "a" }, output: "1" } },
        { finish: "stop" },
      ])
      expect(calls).toHaveLength(2)
      expect(trips).toEqual([])
    })
  }, 60_000)
})
