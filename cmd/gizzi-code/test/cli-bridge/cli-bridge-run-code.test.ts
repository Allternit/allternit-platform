// @ts-nocheck
/**
 * run_code over the CLI bridge: a CLI brain (Claude Code, Codex…) calls
 * gizzi's run_code through the "allternit" MCP server and reports the call
 * back under its own name (mcp__allternit__run_code) with text only. The
 * stored session part must still read `tool == "run_code"` with
 * `state.metadata.files`, which allternit-api ingests (gizzi_chat_stream.rs).
 *
 * Real session store, real bridge, real run_code; only the model stream
 * (the CLI brain's observed events) is fed in.
 */
import { describe, expect, mock, test } from "bun:test"
import os from "os"
import path from "path"
import fs from "fs/promises"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { SessionProcessor } from "../../src/runtime/session/processor"
import { CliBridge } from "../../src/runtime/integrations/cli-bridge"
import { CliBridgeResults } from "../../src/runtime/integrations/cli-bridge/results"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { Identifier } from "../../src/shared/id/id"
import { Bus } from "../../src/shared/bus"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

// Same env dir as test/tool/run-code.test.ts; CI without network sets GIZZI_SKIP_RUN_CODE_E2E=1.
process.env.GIZZI_PYTHON_ENV_DIR ??= path.join(os.tmpdir(), "gizzi-run-code-test-python")
const skipPython = process.env.GIZZI_SKIP_RUN_CODE_E2E === "1" || process.platform === "win32"

let streamEvents: unknown[] = []

// The edge: the CLI brain's stream, as the subprocess loader forwards it.
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

const CSV = "month,revenue\nJan,10\nFeb,30\nMar,20\n"
const signal = new AbortController().signal

/** A session whose last user message carries sales.csv, inside a real project instance. */
async function withSession<T>(fn: (s: { sessionID: string; user: any; dir: string }) => Promise<T>): Promise<T> {
  const tmp = await tmpdir({ git: true, init: async (dir) => Bun.write(path.join(dir, "gizzi.json"), "{}") })
  try {
    return await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const session = await Session.create({})
        const older = await Session.updateMessage({
          id: Identifier.ascending("message"),
          sessionID: session.id,
          role: "user",
          agent: "build",
          model: { providerID: "claude-cli", modelID: "default" },
          time: { created: Date.now() - 1000 },
        })
        await Session.updatePart({
          id: Identifier.ascending("part"),
          messageID: older.id,
          sessionID: session.id,
          type: "file",
          mime: "text/plain",
          filename: "old.txt",
          url: "data:text/plain;base64,b2xk",
        })
        const user = await Session.updateMessage({
          id: Identifier.ascending("message"),
          sessionID: session.id,
          role: "user",
          agent: "build",
          model: { providerID: "claude-cli", modelID: "default" },
          time: { created: Date.now() },
        })
        await Session.updatePart({ id: Identifier.ascending("part"), messageID: user.id, sessionID: session.id, type: "text", text: "Analyze this" })
        await Session.updatePart({
          id: Identifier.ascending("part"),
          messageID: user.id,
          sessionID: session.id,
          type: "file",
          mime: "text/csv",
          filename: "sales.csv",
          url: `data:text/csv;base64,${Buffer.from(CSV).toString("base64")}`,
        })
        return fn({ sessionID: session.id, user, dir: tmp.path })
      },
    })
  } finally {
    await tmp[Symbol.asyncDispose]()
  }
}

/** One CLI turn through the real processor; returns the stored tool parts. */
async function processTurn(sessionID: string, user: any, dir: string, events: unknown[]) {
  streamEvents = [{ type: "start" }, ...events]
  const assistantMessage = await Session.updateMessage({
    id: Identifier.ascending("message"),
    parentID: user.id,
    role: "assistant",
    mode: "build",
    agent: "build",
    sessionID,
    path: { cwd: dir, root: dir },
    cost: 0,
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    modelID: "default",
    providerID: "claude-cli",
    time: { created: Date.now() },
  })
  const model = fakeModel("claude-cli", "default")
  const abort = new AbortController()
  const processor = SessionProcessor.create({ assistantMessage, sessionID, model, abort: abort.signal })
  await processor.process({ user, sessionID, model, agent: { name: "build" }, system: [], messages: [{ role: "user", content: "x" }], tools: {}, abort: abort.signal })
  const stored = (await Session.messages({ sessionID })).find((m) => m.info.id === assistantMessage.id)
  return (stored?.parts ?? []).filter((p) => p.type === "tool")
}

const observed = (id: string, name: string, args: unknown, content = "ok") => [
  { type: "raw", raw: { __gizzi: "observed_tool_call", id, name, arguments: args } },
  { type: "raw", raw: { __gizzi: "observed_tool_result", id, content, isError: false } },
]

describe.skipIf(skipPython)("bridged run_code, really run", () => {
  test(
    "the CLI's mcp__allternit__run_code lands as run_code with the PNG in state.metadata.files",
    async () => {
      await withSession(async ({ sessionID, user, dir }) => {
        const args = {
          language: "python",
          code: [
            "import pandas as pd, matplotlib.pyplot as plt",
            "df = pd.read_csv('in/sales.csv')",
            "print('total', df['revenue'].sum())",
            "df.plot(x='month', y='revenue'); plt.savefig('out/chart.png')",
          ].join("\n"),
        }
        // The bridged call goes through gizzi's own permission ask; the card carries the script.
        const asks: any[] = []
        const unsubscribe = Bus.subscribe(PermissionNext.Event.Asked, (e: any) => {
          if (e.properties.sessionID !== sessionID) return
          asks.push(e.properties)
          void PermissionNext.reply({ requestID: e.properties.id, reply: "once" })
        })
        const reply = (await CliBridge.handle(
          sessionID,
          { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "run_code", arguments: args } },
          signal,
        )) as any
        unsubscribe()
        expect(reply.result.isError).toBe(false)
        expect(asks).toHaveLength(1)
        expect(asks[0].permission).toBe("run_code")
        expect(asks[0].metadata.code).toContain("pd.read_csv('in/sales.csv')")
        const text = JSON.parse(reply.result.content[0].text)
        // Staged from the latest user message (not the older one's old.txt).
        expect(text.inputs).toEqual(["sales.csv"])
        expect(text.stdout).toContain("total 60")

        // Claude Code reports it under its MCP name, with the args re-serialized in another key order.
        const tools = await processTurn(sessionID, user, dir, observed("toolu_1", "mcp__allternit__run_code", { code: args.code, language: "python" }, reply.result.content[0].text))
        expect(tools).toHaveLength(1)
        const part = tools[0]
        expect(part.tool).toBe("run_code")
        expect(part.state.status).toBe("completed")
        expect(part.state.title).toBe("Python: ok, 1 file")
        const files = part.state.metadata.files
        expect(files.map((f) => ({ name: f.name, mediaType: f.mediaType }))).toEqual([{ name: "chart.png", mediaType: "image/png" }])
        expect((await fs.readFile(files[0].path)).subarray(1, 4).toString()).toBe("PNG")
        expect(part.state.metadata.exit_code).toBe(0)
        expect(part.state.metadata.runDir).toBe(path.dirname(path.dirname(files[0].path)))
        // Claimed once: nothing left over for a later call.
        expect(CliBridgeResults.take(sessionID, "run_code", args)).toBeUndefined()
      })
    },
    15 * 60_000,
  )
})

describe("bridged run_code result reaching the stored part", () => {
  test("args the CLI re-serialized (no exact match) still merge, oldest call first", async () => {
    await withSession(async ({ sessionID, user, dir }) => {
      const meta = (n: number) => ({ files: [{ name: `c${n}.png`, mediaType: "image/png", size: 1, path: `/runs/${n}/out/c${n}.png` }], exit_code: 0, runDir: `/runs/${n}` })
      CliBridgeResults.put(sessionID, "run_code", { language: "python", code: "a()", timeout_s: 60 }, { title: "Python: ok, 1 file", metadata: meta(1) })
      CliBridgeResults.put(sessionID, "run_code", { language: "python", code: "b()" }, { title: "Python: ok, 1 file", metadata: meta(2) })
      // Codex dropped timeout_s and turned the code into something else: neither matches exactly.
      const tools = await processTurn(sessionID, user, dir, [
        ...observed("call_a", "mcp__allternit__run_code", { language: "python", code: "a( )" }),
        ...observed("call_b", "allternit_run_code", { language: "python", code: "b( )" }),
      ])
      expect(tools.map((t) => t.tool)).toEqual(["run_code", "run_code"])
      expect(tools.map((t) => t.state.metadata.files[0].name)).toEqual(["c1.png", "c2.png"])
      expect(tools[0].state.metadata.runDir).toBe("/runs/1")
    })
  })

  test("other bridged tools keep the name the CLI reported and take no run_code result", async () => {
    await withSession(async ({ sessionID, user, dir }) => {
      CliBridgeResults.put(sessionID, "run_code", { language: "python", code: "x" }, { metadata: { files: [] } })
      const tools = await processTurn(sessionID, user, dir, [
        ...observed("t1", "mcp__allternit__artifact_create", { title: "Brief" }),
        ...observed("t2", "Bash", { command: "ls" }),
      ])
      expect(tools.map((t) => t.tool)).toEqual(["mcp__allternit__artifact_create", "Bash"])
      expect(tools.map((t) => t.state.metadata)).toEqual([{}, {}])
      // The run_code result is still there for its own call.
      expect(CliBridgeResults.take(sessionID, "run_code", { code: "x", language: "python" })?.metadata).toEqual({ files: [] })
    })
  })

  test("every CLI's name for the bridged run_code maps to run_code; nothing else is renamed", () => {
    for (const name of [
      "run_code",
      "mcp__allternit__run_code", // Claude Code; Codex app-server (codexToolCall)
      "allternit__run_code", // Gemini qualified name
      "allternit_run_code", // OpenCode
      "allternit.run_code",
      "allternit/run_code",
      "run_code (allternit MCP Server)", // Gemini ACP title
    ]) {
      expect(CliBridge.canonicalTool(name)).toBe("run_code")
    }
    for (const name of ["mcp__allternit__artifact_create", "mcp__other__run_code", "my_run_code", "run_code_v2", "mcp__allternit__media_generate"]) {
      expect(CliBridge.canonicalTool(name)).toBe(name)
    }
  })
})
