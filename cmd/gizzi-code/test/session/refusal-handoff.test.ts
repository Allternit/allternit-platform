// @ts-nocheck
/**
 * Open-lane refusal handoff, end to end through SessionPrompt: a brain that
 * refuses (the person's own subscription or key) has the turn run once more
 * on the Allternit Cloud open model, without the refused message in context,
 * and the answer is marked with `handoff`. Off unless the person turned it on.
 */
import { beforeEach, describe, expect, mock, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { Identifier } from "../../src/shared/id/id"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

const REFUSAL = "I'm sorry, but I can't help with that."
const ANSWER = "Here is the walkthrough you asked for."

let replies: string[] = []
const calls: { model: string; messages: string }[] = []

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async (input: any) => {
      if (input.agent?.name !== "build") {
        return { fullStream: (async function* () {})(), text: Promise.resolve("") }
      }
      calls.push({ model: `${input.model.providerID}/${input.model.id}`, messages: JSON.stringify(input.messages) })
      const text = replies.shift() ?? ANSWER
      return {
        fullStream: (async function* () {
          yield { type: "start" }
          yield { type: "start-step" }
          yield { type: "text-start", id: "t1" }
          yield { type: "text-delta", id: "t1", text }
          yield { type: "text-end", id: "t1" }
          yield {
            type: "finish-step",
            finishReason: "stop",
            usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 },
            providerMetadata: {},
          }
          yield { type: "finish", finishReason: "stop" }
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
    defaultModel: async () => ({ providerID: "claude-cli", modelID: "claude-sonnet" }),
    list: async () => ({}),
    resolveAuto: async (m: any) => m,
    prepareAuth: async () => undefined,
    isFabricModel: () => false,
    createRotationState: () => ({ triedProfileIds: new Set<string>() }),
    rotateAuth: async () => undefined,
  },
}))

let target: { providerID: string; modelID: string } | undefined
const realHandoff = await import("../../src/runtime/providers/handoff")
mock.module("../../src/runtime/providers/handoff", () => ({
  ...realHandoff,
  handoffTarget: async () => target,
}))

const { SessionPrompt } = await import("../../src/runtime/session/prompt")

async function withProject<T>(fn: () => Promise<T>) {
  const tmp = await tmpdir({
    git: true,
    init: async (dir) => {
      await Bun.write(path.join(dir, "gizzi.json"), JSON.stringify({}))
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
    model: { providerID: "claude-cli", modelID: "claude-sonnet" },
    parts: [{ id: Identifier.ascending("part"), type: "text", text: "Walk me through it" }],
  })
}

async function assistants(sessionID: string) {
  const out = []
  for await (const m of MessageV2.stream(sessionID)) if (m.info.role === "assistant") out.push(m)
  return out.reverse()
}

beforeEach(() => {
  replies = []
  calls.length = 0
  target = undefined
})

describe("open-lane refusal handoff", () => {
  test("a refused turn is answered by the open model and marked", async () => {
    await withProject(async () => {
      target = { providerID: "allternit", modelID: "deepseek-v4.1-flash-open" }
      replies = [REFUSAL, ANSWER]
      const s = await Session.create({ title: "Handoff" })
      await send(s.id)

      expect(calls.map((c) => c.model)).toEqual(["claude-cli/claude-sonnet", "allternit/deepseek-v4.1-flash-open"])
      // The open model does not see the refusal.
      expect(calls[1].messages).not.toContain("I can't help with that")

      const list = await assistants(s.id)
      expect(list).toHaveLength(2)
      expect(list[0].info.handoff).toBeUndefined()
      expect(list[1].info.handoff).toEqual({
        from: "claude-cli/claude-sonnet",
        to: "allternit/deepseek-v4.1-flash-open",
      })
    })
  })

  test("off unless the person turned it on", async () => {
    await withProject(async () => {
      replies = [REFUSAL]
      const s = await Session.create({ title: "No handoff" })
      await send(s.id)
      expect(calls.map((c) => c.model)).toEqual(["claude-cli/claude-sonnet"])
    })
  })

  test("a real answer is never handed off", async () => {
    await withProject(async () => {
      target = { providerID: "allternit", modelID: "deepseek-v4.1-flash-open" }
      replies = [ANSWER]
      const s = await Session.create({ title: "Answer" })
      await send(s.id)
      expect(calls).toHaveLength(1)
    })
  })

  test("hands off at most once per turn", async () => {
    await withProject(async () => {
      target = { providerID: "allternit", modelID: "deepseek-v4.1-flash-open" }
      replies = [REFUSAL, REFUSAL, ANSWER]
      const s = await Session.create({ title: "Twice" })
      await send(s.id)
      expect(calls).toHaveLength(2)
    })
  })
})
