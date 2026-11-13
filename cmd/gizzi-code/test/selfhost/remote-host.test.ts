import { afterEach, beforeEach, describe, expect, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { SettingsHooksBridge } from "../../src/runtime/hooks/settings-bridge"
import { ToolDispatcher } from "../../src/runtime/tools/dispatch"
import { tmpdir } from "../fixture/fixture"

// Remote tool host (GIZZI_TOOL_HOST=remote): sandbox tools go to the Agents
// Server's loopback queue and never run in the agent loop process, even when
// the server is missing or has no worker.

type Received = { path: string; auth: string | null; body: any }

const ENV_KEYS = ["GIZZI_TOOL_HOST", "GIZZI_TOOL_HOST_URL", "GIZZI_TOOL_HOST_TOKEN", "GIZZI_CONFIG_DIR", "CLAUDE_CONFIG_DIR"]
let saved: Record<string, string | undefined> = {}
let userHome: Awaited<ReturnType<typeof tmpdir>> | undefined

beforeEach(async () => {
  saved = Object.fromEntries(ENV_KEYS.map((k) => [k, process.env[k]]))
  userHome = await tmpdir()
  process.env.GIZZI_CONFIG_DIR = userHome.path
  process.env.CLAUDE_CONFIG_DIR = path.join(userHome.path, "legacy")
  SettingsHooksBridge.resetCache()
})

afterEach(async () => {
  for (const [k, v] of Object.entries(saved)) {
    if (v === undefined) delete process.env[k]
    else process.env[k] = v
  }
  SettingsHooksBridge.resetCache()
  await userHome?.[Symbol.asyncDispose]()
})

function fakeCtx(callID = "toolu_1") {
  return {
    sessionID: "ses_remote_1",
    messageID: "msg_1",
    callID,
    agent: "build",
    abort: new AbortController().signal,
    messages: [],
    metadata: () => {},
    ask: async () => {},
  } as any
}

function server(respond: (received: Received) => Response) {
  const received: Received[] = []
  const srv = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    async fetch(req) {
      const r = { path: new URL(req.url).pathname, auth: req.headers.get("authorization"), body: await req.json() }
      received.push(r)
      return respond(r)
    },
  })
  return { srv, received, url: `http://127.0.0.1:${srv.port}` }
}

async function inProject<T>(fn: () => Promise<T>) {
  await using tmp = await tmpdir({ git: true })
  return Instance.provide({ directory: tmp.path, fn })
}

describe("remote tool host", () => {
  test("bash runs on the worker: the server's result comes back in the local tool shape", async () => {
    const s = server(() => Response.json({ output: "ok\n", is_error: false }))
    try {
      process.env.GIZZI_TOOL_HOST = "remote"
      process.env.GIZZI_TOOL_HOST_URL = s.url
      process.env.GIZZI_TOOL_HOST_TOKEN = "internal-token"
      await inProject(async () => {
        let ranLocally = false
        const result: any = await ToolDispatcher.executeInitialized(
          "bash",
          { command: "echo ok > /workspace/a && cat /workspace/a", description: "Write and read a" },
          fakeCtx(),
          async () => {
            ranLocally = true
            return { title: "local", output: "local", metadata: {} }
          },
        )
        expect(ranLocally).toBe(false)
        expect(result.output).toBe("ok\n")
        expect(result.title).toBe("Write and read a")
        expect(result.metadata.output).toBe("ok\n")
      })
      expect(s.received).toHaveLength(1)
      expect(s.received[0]!.path).toBe("/internal/v1/sessions/ses_remote_1/tool_calls")
      expect(s.received[0]!.auth).toBe("Bearer internal-token")
      expect(s.received[0]!.body.tool_call_id).toBe("toolu_1")
      expect(s.received[0]!.body.name).toBe("bash")
      expect(s.received[0]!.body.input.command).toContain("/workspace/a")
      expect(s.received[0]!.body.timeout_ms).toBeGreaterThanOrEqual(120_000)
    } finally {
      s.srv.stop(true)
    }
  })

  test("no worker (504) is a tool error, and the tool does not run locally", async () => {
    const s = server(() =>
      Response.json({ error: { type: "api_error", code: "no_worker", message: "no worker attached" } }, { status: 504 }),
    )
    try {
      process.env.GIZZI_TOOL_HOST = "remote"
      process.env.GIZZI_TOOL_HOST_URL = s.url
      process.env.GIZZI_TOOL_HOST_TOKEN = "internal-token"
      await inProject(async () => {
        let ranLocally = false
        const call = ToolDispatcher.executeInitialized("read", { filePath: "a.txt" }, fakeCtx("toolu_2"), async () => {
          ranLocally = true
          return { title: "local", output: "local", metadata: {} }
        })
        await expect(call).rejects.toThrow(/No worker is running/)
        expect(ranLocally).toBe(false)
      })
    } finally {
      s.srv.stop(true)
    }
  })

  test("a worker error comes back as a tool error with the worker's text", async () => {
    const s = server(() => Response.json({ output: "File not found: /workspace/missing", is_error: true }))
    try {
      process.env.GIZZI_TOOL_HOST = "remote"
      process.env.GIZZI_TOOL_HOST_URL = s.url
      process.env.GIZZI_TOOL_HOST_TOKEN = "internal-token"
      await inProject(async () => {
        const call = ToolDispatcher.executeInitialized("read", { filePath: "/workspace/missing" }, fakeCtx("toolu_3"), async () => ({
          title: "local",
          output: "local",
          metadata: {},
        }))
        await expect(call).rejects.toThrow("File not found: /workspace/missing")
      })
    } finally {
      s.srv.stop(true)
    }
  })

  test("remote mode without a URL or token refuses instead of running locally", async () => {
    process.env.GIZZI_TOOL_HOST = "remote"
    delete process.env.GIZZI_TOOL_HOST_URL
    delete process.env.GIZZI_TOOL_HOST_TOKEN
    await inProject(async () => {
      let ranLocally = false
      const call = ToolDispatcher.executeInitialized("write", { filePath: "a.txt", content: "x" }, fakeCtx("toolu_4"), async () => {
        ranLocally = true
        return { title: "local", output: "local", metadata: {} }
      })
      await expect(call).rejects.toThrow(/GIZZI_TOOL_HOST_URL/)
      expect(ranLocally).toBe(false)
    })
  })

  test("tools outside the sandbox set still run in the agent loop", async () => {
    process.env.GIZZI_TOOL_HOST = "remote"
    process.env.GIZZI_TOOL_HOST_URL = "http://127.0.0.1:9"
    process.env.GIZZI_TOOL_HOST_TOKEN = "internal-token"
    await inProject(async () => {
      const result: any = await ToolDispatcher.executeInitialized("todowrite", { todos: [] }, fakeCtx("toolu_5"), async () => ({
        title: "todos",
        output: "local todo",
        metadata: {},
      }))
      expect(result.output).toBe("local todo")
    })
  })
})
