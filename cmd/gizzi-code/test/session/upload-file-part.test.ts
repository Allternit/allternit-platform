// @ts-nocheck
/**
 * allternit-api sends upload attachments as file parts with a RELATIVE url
 * (`/api/v1/uploads/<id>`; iOS does this, web sends data: URLs). The prompt
 * must not crash on them: gizzi fetches the bytes from the API it talks to
 * (with its credential) and stores the part as a data: URL, which the model
 * and run_code can both read.
 */
import { describe, expect, mock, test } from "bun:test"
import path from "path"

const seen: { path: string; auth: string | null }[] = []
const server = Bun.serve({
  port: 0,
  fetch(req) {
    const u = new URL(req.url)
    seen.push({ path: u.pathname, auth: req.headers.get("authorization") })
    if (u.pathname === "/api/v1/uploads/up1") return new Response("a,b\n1,2\n", { headers: { "content-type": "text/csv" } })
    return new Response("nope", { status: 404 })
  },
})
process.env.ALLTERNIT_API_URL = `http://127.0.0.1:${server.port}`
process.env.ALLTERNIT_API_TOKEN = "tok-test"


const { Log } = await import("../../src/shared/util/log")
Log.init({ print: false })
mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async () => ({
      fullStream: (async function* () {
        yield { type: "start" }
        yield { type: "start-step" }
        yield { type: "finish-step", finishReason: "stop", usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }, providerMetadata: {} }
        yield { type: "finish", finishReason: "stop" }
      })(),
      text: Promise.resolve(""),
    }),
  },
}))
const real = await import("../../src/runtime/providers/provider")
mock.module("../../src/runtime/providers/provider", () => ({
  Provider: {
    ...real.Provider,
    getModel: async (providerID: string, modelID: string) => ({
      id: modelID, providerID,
      api: { id: modelID, url: "https://api.test", npm: "@ai-sdk/openai-compatible" },
      name: modelID,
      capabilities: { toolcall: true, attachment: true, reasoning: false, temperature: true, input: { text: true }, output: { text: true } },
      cost: { input: 0, output: 0, cache: { read: 0, write: 0 } },
      limit: { context: 1_000_000, output: 8192 }, options: {}, headers: {}, variants: {}, status: "active", release_date: "2025-01-01",
    }),
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

const { Instance } = await import("../../src/runtime/context/project/instance")
const { Session } = await import("../../src/runtime/session")
const { SessionPrompt } = await import("../../src/runtime/session/prompt")
const { Identifier } = await import("../../src/shared/id/id")
const { tmpdir } = await import("../fixture/fixture")

async function userParts(files: any[]) {
  const tmp = await tmpdir({ git: true, init: async (dir) => { await Bun.write(path.join(dir, "gizzi.json"), "{}") } })
  try {
    return await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const s = await Session.create({ title: "uploads" })
        await SessionPrompt.prompt({
          sessionID: s.id, agent: "build", model: { providerID: "plan", modelID: "model-a" },
          parts: [{ id: Identifier.ascending("part"), type: "text", text: "analyze" }, ...files],
        })
        const all = await Session.messages({ sessionID: s.id })
        return all.find((m: any) => m.info.role === "user").parts
      },
    })
  } finally {
    await tmp[Symbol.asyncDispose]()
  }
}

const filePart = (url: string) => ({ id: Identifier.ascending("part"), type: "file", url, mime: "text/csv", filename: "data.csv" })

describe("upload file parts with a relative url", () => {
  test("are fetched from the API with the credential and kept as data: urls", async () => {
    seen.length = 0
    const parts = await userParts([filePart("/api/v1/uploads/up1")])
    expect(seen.filter((r) => r.path.startsWith("/api/v1/uploads/"))).toEqual([{ path: "/api/v1/uploads/up1", auth: "Bearer tok-test" }])
    const kept = parts.filter((p: any) => p.type === "file")
    expect(kept).toHaveLength(1)
    expect(kept[0].url).toBe(`data:text/csv;base64,${Buffer.from("a,b\n1,2\n").toString("base64")}`)
    expect(kept[0].filename).toBe("data.csv")
  })

  test("an upload that cannot be fetched becomes a note, not a crash", async () => {
    const parts = await userParts([filePart("/api/v1/uploads/missing")])
    expect(parts.some((p: any) => p.type === "file")).toBe(false)
    expect(parts.some((p: any) => p.type === "text" && p.synthetic && p.text.includes("data.csv"))).toBe(true)
  })

  test("a relative url that is not an upload is refused as a note, not a crash", async () => {
    const parts = await userParts([filePart("/etc/passwd")])
    expect(parts.some((p: any) => p.type === "file")).toBe(false)
    expect(parts.some((p: any) => p.type === "text" && p.synthetic && p.text.includes("data.csv"))).toBe(true)
  })
})

describe("text/plain data: parts", () => {
  test("the model gets the file's text, not the url re-decoded", async () => {
    const url = `data:text/plain;base64,${Buffer.from("hello notes").toString("base64")}`
    const parts = await userParts([{ id: Identifier.ascending("part"), type: "file", url, mime: "text/plain", filename: "n.txt" }])
    expect(parts.some((p: any) => p.type === "text" && p.synthetic && p.text === "hello notes")).toBe(true)
  })
})

describe("data files on the stored message", () => {
  test("a big text/plain file is stored as a note, not its whole text", async () => {
    const big = "line\n".repeat(5000)
    const url = `data:text/plain;base64,${Buffer.from(big).toString("base64")}`
    const parts = await userParts([{ id: Identifier.ascending("part"), type: "file", url, mime: "text/plain", filename: "big.txt" }])
    const texts = parts.filter((p: any) => p.type === "text" && p.synthetic).map((p: any) => p.text)
    expect(texts.some((t: string) => t.includes("line\nline"))).toBe(false)
    expect(texts).toContain("Attached file big.txt (text/plain, 24.4 KB): too large to show inline; use run_code to read it.")
    expect(parts.filter((p: any) => p.type === "file")).toHaveLength(1)
  })

  test.skipIf(process.env.GIZZI_SKIP_RUN_CODE_E2E === "1" || process.platform === "win32")(
    "a CSV upload keeps its file part: the model gets text and run_code stages the file",
    async () => {
      const parts = await userParts([filePart("/api/v1/uploads/up1")])
      const { MessageV2 } = await import("../../src/runtime/session/message-v2")
      const messages = [{ info: { id: "msg_u", role: "user" }, parts }]
      const [model] = MessageV2.toModelMessages(messages as any, {
        id: "m", providerID: "p", api: { id: "m", url: "", npm: "@ai-sdk/anthropic" },
      } as any)
      expect((model.content as any[]).some((p) => p.type === "file")).toBe(false)
      expect((model.content as any[]).some((p) => p.type === "text" && p.text.includes("[Attached file: data.csv]\na,b\n1,2\n"))).toBe(true)

      process.env.GIZZI_PYTHON_ENV_DIR ??= path.join((await import("os")).tmpdir(), "gizzi-run-code-test-python")
      const { RunCodeTool } = await import("../../src/runtime/tools/builtins/run_code")
      const tool = await RunCodeTool.init()
      const result = await tool.execute({ language: "python", code: "print(open('in/data.csv').read().strip())" }, {
        sessionID: "ses_upload", messageID: "msg_a", callID: "c1", agent: "build",
        abort: AbortSignal.any([]), messages, metadata: () => {}, ask: async () => {},
      } as any)
      expect(result.metadata.inputs).toEqual(["data.csv"])
      expect(result.metadata.stdout).toContain("a,b\n1,2")
    },
    15 * 60_000,
  )
})
