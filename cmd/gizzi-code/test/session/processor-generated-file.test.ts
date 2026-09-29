// @ts-nocheck
/**
 * Generated files and progress through the session processor: an AI SDK
 * `file` stream part (any provider) becomes a FilePart on the assistant
 * message; a raw `generated_file` part names it; a raw `progress` part is a
 * `session.progress` bus event (never stored); text deltas carry their
 * durable trace sequence so chat bridges can resume a dropped stream.
 */
import { describe, expect, mock, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionProcessor } from "../../src/runtime/session/processor"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { SessionProgress } from "../../src/runtime/session/progress-event"
import { SessionTrace } from "../../src/runtime/session/trace"
import { generatedFileMeta, generatedFilePart } from "../../src/runtime/session/generated-file"
import { Identifier } from "../../src/shared/id/id"
import { Bus } from "../../src/shared/bus"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

let streamEvents: unknown[] = []

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async () => ({
      fullStream: (async function* () {
        for (const event of streamEvents) yield event
      })(),
    }),
  },
}))

function fakeModel(providerID: string, modelID: string) {
  return {
    id: modelID,
    providerID,
    api: { id: modelID, url: "https://api.test", npm: "@ai-sdk/openai-compatible" },
    name: modelID,
    cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
    limit: { context: 1_000_000, output: 8192 },
    options: {},
    headers: {},
    status: "active",
    release_date: "2025-01-01",
  }
}

mock.module("../../src/runtime/providers/provider", () => ({
  Provider: {
    parseModel: (model: string) => {
      const [providerID, ...rest] = model.split("/")
      return { providerID, modelID: rest.join("/") }
    },
    getModel: async (providerID: string, modelID: string) => fakeModel(providerID, modelID),
    createRotationState: () => ({ triedProfileIds: new Set<string>() }),
    rotateAuth: async () => undefined,
    prepareAuth: async () => undefined,
  },
}))

async function run(events: unknown[]) {
  streamEvents = events
  const tmp = await tmpdir({
    git: true,
    init: async (dir) => {
      await Bun.write(path.join(dir, "gizzi.json"), JSON.stringify({}))
    },
  })
  try {
    return await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const user = await Session.updateMessage({
          id: Identifier.ascending("message"),
          sessionID: session.id,
          role: "user",
          agent: "build",
          model: { providerID: "subs-chatgpt", modelID: "fast" },
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
          modelID: "fast",
          providerID: "subs-chatgpt",
          time: { created: Date.now() },
        })
        const model = fakeModel("subs-chatgpt", "fast")
        const abort = new AbortController()
        const processor = SessionProcessor.create({ assistantMessage, sessionID: session.id, model, abort: abort.signal })
        const bus = { progress: [] as any[], deltas: [] as any[], parts: [] as any[] }
        const unsubs = [
          Bus.subscribe(SessionProgress.Event.Updated, (e: any) => bus.progress.push(e.properties)),
          Bus.subscribe(MessageV2.Event.PartDelta, (e: any) => bus.deltas.push(e.properties)),
          Bus.subscribe(MessageV2.Event.PartUpdated, (e: any) => bus.parts.push(e.properties.part)),
        ]
        await processor.process({
          user,
          sessionID: session.id,
          model,
          agent: { name: "build" },
          system: [],
          messages: [{ role: "user", content: "draw a cat" }],
          tools: {},
          abort: abort.signal,
        })
        for (const u of unsubs) u()
        const stored = (await Session.messages({ sessionID: session.id })).find((m) => m.info.id === assistantMessage.id)
        const entries = SessionTrace.list({ sessionID: session.id })
        const trace = (after: number) => entries.filter((e) => e.sequence > after)
        return { session, bus, trace, parts: stored?.parts ?? [] }
      },
    })
  } finally {
    await tmp[Symbol.asyncDispose]()
  }
}

const file = (mediaType: string, base64: string) => ({ type: "file", file: { mediaType, base64 } })

describe("session.processor generated files", () => {
  test("a file part becomes a FilePart on the assistant message, named by the raw meta before it", async () => {
    const { parts, bus } = await run([
      { type: "start" },
      {
        type: "raw",
        raw: { __gizzi: "generated_file", filename: "a1.png", title: "A cat", mediaType: "image/png", sourceUri: "fabric-artifact://a1" },
      },
      file("image/png", "aGk="),
    ])
    const filePart = parts.find((p) => p.type === "file")
    expect(filePart).toMatchObject({
      type: "file",
      mime: "image/png",
      filename: "a1.png",
      url: "data:image/png;base64,aGk=",
      source: { type: "resource", uri: "fabric-artifact://a1", text: { value: "A cat" } },
    })
    // Published live so bridges can frame it.
    expect(bus.parts.some((p) => p.type === "file" && p.id === filePart.id)).toBe(true)
  })

  test("a bare file part (any provider, no meta) still lands, unnamed", async () => {
    const { parts } = await run([{ type: "start" }, file("image/webp", "AAAA")])
    expect(parts.find((p) => p.type === "file")).toMatchObject({ mime: "image/webp", url: "data:image/webp;base64,AAAA" })
  })

  test("an oversized file (raw meta with url, no bytes) points at its download URL", async () => {
    const { parts } = await run([
      { type: "start" },
      {
        type: "raw",
        raw: {
          __gizzi: "generated_file",
          filename: "big.pptx",
          mediaType: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
          url: "/api/v1/subscriptions/gateway/v1/artifacts/big/download",
        },
      },
    ])
    expect(parts.find((p) => p.type === "file")).toMatchObject({
      filename: "big.pptx",
      url: "/api/v1/subscriptions/gateway/v1/artifacts/big/download",
    })
  })

  test("progress is a live bus event, never a stored part", async () => {
    const { parts, bus } = await run([
      { type: "start" },
      { type: "raw", raw: { __gizzi: "progress", label: "Searching the web", fraction: 0.4 } },
      { type: "raw", raw: { __gizzi: "progress", elapsedS: 95 } },
    ])
    expect(bus.progress.map(({ label, fraction, elapsedS }) => ({ label, fraction, elapsedS }))).toEqual([
      { label: "Searching the web", fraction: 0.4, elapsedS: undefined },
      { label: undefined, fraction: undefined, elapsedS: 95 },
    ])
    expect(parts.some((p) => (p.type as string) === "progress")).toBe(false)
  })

  test("text deltas carry their durable trace sequence (the resume cursor)", async () => {
    const { bus, trace } = await run(
      [
        { type: "start" },
        { type: "text-start" },
        { type: "text-delta", text: "Hello" },
        { type: "text-delta", text: " there" },
        { type: "text-end" },
      ],
    )
    const seqs = bus.deltas.map((d) => d.traceSeq)
    expect(seqs).toHaveLength(2)
    expect(seqs[0]).toBeGreaterThan(0)
    expect(seqs[1]).toBeGreaterThan(seqs[0])
    // Replaying the trace after the first cursor yields exactly the second delta.
    const after = trace(seqs[0]).filter((e) => e.kind === "part.delta")
    expect(after.map((e) => e.sequence)).toEqual([seqs[1]])
    expect((after[0].data as any).delta).toBe(" there")
  })
})

describe("generated-file helpers", () => {
  test("generatedFileMeta only reads generated_file raw parts", () => {
    expect(generatedFileMeta({ __gizzi: "observed_context" })).toBeUndefined()
    expect(generatedFileMeta({ __gizzi: "generated_file", filename: " ", title: "T" })).toEqual({
      filename: undefined,
      title: "T",
      mediaType: undefined,
      sourceUri: undefined,
      url: undefined,
    })
  })

  test("no bytes and no url → no part", () => {
    expect(generatedFilePart({ id: "p", messageID: "m", sessionID: "s", mediaType: "image/png" })).toBeUndefined()
  })
})
