// @ts-nocheck
/**
 * Self-hosted Agents Server custom tools: an agent's `type: custom` tools ride
 * each turn's `metadata.selfhost.custom_tools` (loop.rs message_body). With
 * GIZZI_TOOL_HOST=remote they are offered to the model for that session, and
 * a call to one is POSTed to the server's internal tool_calls route (the
 * remote tool host), never run here. A custom tool may not shadow a built-in
 * or an MCP tool of the session, and the server can ask before it stores a
 * message (POST /session/:id/custom-tools/check).
 */
import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import path from "path"
import { Instance } from "../../src/runtime/context/project/instance"
import { Session } from "../../src/runtime/session"
import { MessageV2 } from "../../src/runtime/session/message-v2"
import { RemoteToolHost } from "../../src/runtime/tools/remote-host"
import { Identifier } from "../../src/shared/id/id"
import { Log } from "../../src/shared/util/log"
import { tmpdir } from "../fixture/fixture"

Log.init({ print: false })

// Bun keeps a mock.module override for the rest of the run, other test files
// included, so the real modules go back once this file is done.
const realLLMModule = { ...(await import("../../src/runtime/session/llm")) }
const realProviderModule = { ...(await import("../../src/runtime/providers/provider")) }
afterAll(() => {
  mock.module("../../src/runtime/session/llm", () => realLLMModule)
  mock.module("../../src/runtime/providers/provider", () => realProviderModule)
})

/** The tool record the model got on each build-agent call. */
const toolLists: Record<string, any>[] = []

mock.module("../../src/runtime/session/llm", () => ({
  LLM: {
    stream: async (input: any) => {
      if (input.agent?.name === "build") toolLists.push(input.tools)
      return {
        fullStream: (async function* () {
          yield { type: "start" }
          yield { type: "start-step" }
          yield { type: "text-start", id: "t1" }
          yield { type: "text-delta", id: "t1", text: "ok" }
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

const { SessionPrompt } = await import("../../src/runtime/session/prompt")
const { SessionRoutes } = await import("../../src/runtime/server/routes/session")

/** A real stdio MCP server with one tool, `echo` (offered as mcp__modern__echo). */
const MCP_FIXTURE = path.join(import.meta.dir, "../fixture/mcp-modern-stdio-server.ts")
const MCP_CONFIG = { mcp: { modern: { type: "local", command: [process.execPath, MCP_FIXTURE] } } }

const LOOKUP = {
  type: "custom",
  name: "lookup_order",
  description: "Look up an order by id in the shop's database.",
  input_schema: {
    type: "object",
    properties: { order_id: { type: "string" } },
    required: ["order_id"],
  },
}

async function withProject<T>(fn: () => Promise<T>, config: Record<string, unknown> = {}) {
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

async function send(sessionID: string, customTools: unknown[]) {
  return SessionPrompt.prompt({
    sessionID,
    agent: "build",
    model: { providerID: "claude-cli", modelID: "claude-sonnet" },
    parts: [{ id: Identifier.ascending("part"), type: "text", text: "Where is order 42?" }],
    metadata: { selfhost: { session_id: "sesn_1", agent_id: "agent_1", custom_tools: customTools } },
  })
}

const savedEnv = {
  host: process.env.GIZZI_TOOL_HOST,
  url: process.env.GIZZI_TOOL_HOST_URL,
  token: process.env.GIZZI_TOOL_HOST_TOKEN,
}
const realFetch = globalThis.fetch

function restoreEnv(key: string, value: string | undefined) {
  if (value === undefined) delete process.env[key]
  else process.env[key] = value
}

beforeEach(() => {
  toolLists.length = 0
  process.env.GIZZI_TOOL_HOST = "remote"
  process.env.GIZZI_TOOL_HOST_URL = "http://127.0.0.1:4999/"
  process.env.GIZZI_TOOL_HOST_TOKEN = "internal-token"
})

afterEach(() => {
  globalThis.fetch = realFetch
  restoreEnv("GIZZI_TOOL_HOST", savedEnv.host)
  restoreEnv("GIZZI_TOOL_HOST_URL", savedEnv.url)
  restoreEnv("GIZZI_TOOL_HOST_TOKEN", savedEnv.token)
})

// Each test opens a fresh project, which starts its MCP servers (bundled ones
// included) on the first turn: more than bun's default 5s on a cold run.
describe("self-hosted custom tools", () => {
  test("a turn's custom tools reach the model and run through the remote tool host", async () => {
    await withProject(async () => {
      const s = await Session.create({ title: "Custom tools" })
      await send(s.id, [LOOKUP])

      expect(toolLists.length).toBeGreaterThan(0)
      const tools = toolLists[0]
      expect(Object.keys(tools)).toContain("lookup_order")
      expect(Object.keys(tools)).toContain("bash")
      expect(tools.lookup_order.description).toBe(LOOKUP.description)
      const schema = tools.lookup_order.inputSchema.jsonSchema
      expect(schema.type).toBe("object")
      expect(schema.properties.order_id.type).toBe("string")
      expect(schema.required).toEqual(["order_id"])
      expect(RemoteToolHost.isCustomTool(s.id, "lookup_order")).toBe(true)
      expect(RemoteToolHost.handles("lookup_order", s.id)).toBe(true)
      expect(RemoteToolHost.handles("lookup_order", "ses_other")).toBe(false)

      const requests: { url: string; init: RequestInit }[] = []
      globalThis.fetch = (async (url: string, init: RequestInit) => {
        requests.push({ url: String(url), init })
        return new Response(JSON.stringify({ output: "Order 42 shipped on Monday.", is_error: false }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        })
      }) as typeof fetch

      const result = await tools.lookup_order.execute(
        { order_id: "42" },
        { toolCallId: "toolu_lookup_1", messages: [], abortSignal: new AbortController().signal },
      )

      expect(requests).toHaveLength(1)
      expect(requests[0].url).toBe(
        `http://127.0.0.1:4999/internal/v1/sessions/${encodeURIComponent(s.id)}/tool_calls`,
      )
      expect(requests[0].init.method).toBe("POST")
      expect(requests[0].init.headers.Authorization).toBe("Bearer internal-token")
      const body = JSON.parse(String(requests[0].init.body))
      expect(body.tool_call_id).toBe("toolu_lookup_1")
      expect(body.name).toBe("lookup_order")
      expect(body.input).toEqual({ order_id: "42" })
      expect(body.timeout_ms).toBeGreaterThan(0)

      expect(result.output).toBe("Order 42 shipped on Monday.")
      expect(result.metadata.remote).toBe(true)
      expect(result.metadata.custom).toBe(true)
    })
  }, 30_000)

  test("a worker error comes back as a tool error", async () => {
    await withProject(async () => {
      const s = await Session.create({ title: "Custom tool error" })
      await send(s.id, [LOOKUP])
      globalThis.fetch = (async () =>
        new Response(JSON.stringify({ output: "No handler for lookup_order.", is_error: true }), {
          status: 200,
        })) as typeof fetch
      await expect(
        toolLists[0].lookup_order.execute(
          { order_id: "42" },
          { toolCallId: "toolu_lookup_2", messages: [], abortSignal: new AbortController().signal },
        ),
      ).rejects.toThrow("No handler for lookup_order.")
    })
  }, 30_000)

  test("a custom tool that shadows a built-in refuses the turn before anything is stored", async () => {
    await withProject(async () => {
      const s = await Session.create({ title: "Shadowing" })
      await expect(send(s.id, [{ ...LOOKUP, name: "Bash" }])).rejects.toThrow(
        'The custom tool "Bash" has the same name as the built-in tool "bash"',
      )
      expect(toolLists).toHaveLength(0)
      const stored = []
      for await (const m of MessageV2.stream(s.id)) stored.push(m)
      expect(stored).toHaveLength(0)
      expect(RemoteToolHost.isCustomTool(s.id, "Bash")).toBe(false)
    })
  }, 30_000)

  test("a custom tool named like an MCP tool of the session refuses the turn before anything is stored", async () => {
    await withProject(async () => {
      const s = await Session.create({ title: "MCP clash" })
      await expect(send(s.id, [{ ...LOOKUP, name: "mcp__modern__echo" }])).rejects.toThrow(
        'The custom tool "mcp__modern__echo" has the same name as the tool "echo" of the MCP server "modern"',
      )
      expect(toolLists).toHaveLength(0)
      const stored = []
      for await (const m of MessageV2.stream(s.id)) stored.push(m)
      expect(stored).toHaveLength(0)
      expect(RemoteToolHost.isCustomTool(s.id, "mcp__modern__echo")).toBe(false)
      const refused = await send(s.id, [{ ...LOOKUP, name: "mcp__modern__echo" }]).catch((e) => e)
      expect(refused.code).toBe("custom_tool_conflict")
      expect(refused.conflict).toEqual({ tool: "mcp__modern__echo", clashes_with: "mcp:modern:echo" })

      // A name the MCP server does not hold still goes through, and the MCP tool is offered beside it.
      await send(s.id, [LOOKUP])
      expect(Object.keys(toolLists[0])).toEqual(expect.arrayContaining(["lookup_order", "mcp__modern__echo"]))
    }, MCP_CONFIG)
  }, 60_000)

  test("without remote mode the metadata is ignored", async () => {
    delete process.env.GIZZI_TOOL_HOST
    await withProject(async () => {
      const s = await Session.create({ title: "Local" })
      await send(s.id, [LOOKUP])
      expect(Object.keys(toolLists[0])).not.toContain("lookup_order")
    })
  }, 30_000)
})

describe("custom tool check route", () => {
  async function check(sessionID: string, body: unknown) {
    const res = await SessionRoutes().request(`/${sessionID}/custom-tools/check`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    })
    return { status: res.status, body: await res.json() }
  }

  test("409 for an MCP or built-in clash, 200 otherwise, and nothing is stored", async () => {
    await withProject(async () => {
      const s = await Session.create({ title: "Preflight" })

      // The message route's own body (loop.rs message_body), sent as is.
      const mcp = await check(s.id, {
        model: { providerID: "claude-cli", modelID: "claude-sonnet" },
        agent: "build",
        parts: [{ type: "text", text: "hi" }],
        metadata: { selfhost: { session_id: "sesn_1", agent_id: "agent_1", custom_tools: [LOOKUP, { ...LOOKUP, name: "MCP__modern__echo" }] } },
      })
      expect(mcp.status).toBe(409)
      expect(mcp.body.error.code).toBe("custom_tool_conflict")
      expect(mcp.body.error.tool).toBe("MCP__modern__echo")
      expect(mcp.body.error.type).toBe("conflict_error")
      expect(mcp.body.error.clashes_with).toBe("mcp:modern:echo")
      expect(mcp.body.error.message).toContain('the tool "echo" of the MCP server "modern"')

      const builtin = await check(s.id, { custom_tools: [{ ...LOOKUP, name: "ls" }] })
      expect(builtin.status).toBe(409)
      expect(builtin.body.error).toMatchObject({
        type: "conflict_error",
        code: "custom_tool_conflict",
        tool: "ls",
        clashes_with: "builtin:list",
      })

      const webfetch = await check(s.id, { custom_tools: [{ ...LOOKUP, name: "WebFetch" }] })
      expect(webfetch.status).toBe(409)
      expect(webfetch.body.error.clashes_with).toBe("builtin:webfetch")

      const ok = await check(s.id, { metadata: { selfhost: { custom_tools: [LOOKUP] } } })
      expect(ok).toEqual({ status: 200, body: { ok: true } })
      expect(await check(s.id, {})).toEqual({ status: 200, body: { ok: true } })

      const bad = await check(s.id, { custom_tools: [{ name: "has space", input_schema: {} }] })
      expect(bad.status).toBe(400)
      expect(bad.body.error).toMatchObject({ type: "invalid_request_error", code: "invalid_custom_tool" })

      // A first message has no gizzi session yet: the check still answers.
      expect((await check("ses_not_created_yet", { custom_tools: [{ ...LOOKUP, name: "mcp__modern__echo" }] })).status).toBe(409)

      const stored = []
      for await (const m of MessageV2.stream(s.id)) stored.push(m)
      expect(stored).toHaveLength(0)
      expect(RemoteToolHost.isCustomTool(s.id, "lookup_order")).toBe(false)
      expect(toolLists).toHaveLength(0)
    }, MCP_CONFIG)
  }, 60_000)
})

describe("RemoteToolHost custom tool parsing", () => {
  test("reads metadata.selfhost.custom_tools", () => {
    expect(RemoteToolHost.customToolsFrom({ selfhost: { custom_tools: [LOOKUP] } })).toEqual([
      { name: "lookup_order", description: LOOKUP.description, inputSchema: LOOKUP.input_schema },
    ])
    expect(RemoteToolHost.customToolsFrom(undefined)).toEqual([])
    expect(RemoteToolHost.customToolsFrom({ selfhost: { custom_tools: [] } })).toEqual([])
  })

  test("a schema without a type becomes an object schema", () => {
    const [def] = RemoteToolHost.customToolsFrom({
      selfhost: { custom_tools: [{ name: "ping", input_schema: { properties: {} } }] },
    })
    expect(def).toEqual({ name: "ping", description: "", inputSchema: { properties: {}, type: "object" } })
  })

  test("refuses entries the model could not be given", () => {
    const bad = (tools: unknown) => () => RemoteToolHost.customToolsFrom({ selfhost: { custom_tools: tools } })
    expect(bad("nope")).toThrow("must be a list")
    expect(bad([{ name: "has space", input_schema: {} }])).toThrow("custom tool name must be")
    expect(bad([{ name: "x" }])).toThrow('"x" needs an input_schema')
    expect(bad([{ name: "x", input_schema: { type: "string" } }])).toThrow('must be type "object"')
    expect(bad([LOOKUP, LOOKUP])).toThrow('two custom tools named "lookup_order"')
  })

  test("registration refuses sandbox tool names and aliases, and replaces the session's set", () => {
    const def = (name: string) => ({ name, description: "", inputSchema: { type: "object" } })
    expect(() => RemoteToolHost.registerCustomTools("ses_a", [def("read")], [])).toThrow('the built-in tool "read"')
    expect(() => RemoteToolHost.registerCustomTools("ses_a", [def("ls")], [])).toThrow('the built-in tool "list"')
    expect(() => RemoteToolHost.registerCustomTools("ses_a", [def("MyMcp_tool")], ["mymcp_tool"])).toThrow(
      'the built-in tool "mymcp_tool"',
    )
    expect(() =>
      RemoteToolHost.registerCustomTools("ses_a", [def("MCP__Docs__Search")], [
        { kind: "mcp", name: "mcp__docs__search", server: "docs", tool: "search" },
      ]),
    ).toThrow('the tool "search" of the MCP server "docs"')
    RemoteToolHost.registerCustomTools("ses_a", [def("one")], [])
    expect(RemoteToolHost.isCustomTool("ses_a", "one")).toBe(true)
    RemoteToolHost.registerCustomTools("ses_a", [def("two")], [])
    expect(RemoteToolHost.isCustomTool("ses_a", "one")).toBe(false)
    expect(RemoteToolHost.isCustomTool("ses_a", "two")).toBe(true)
    RemoteToolHost.registerCustomTools("ses_a", [], [])
    expect(RemoteToolHost.isCustomTool("ses_a", "two")).toBe(false)
  })
})
