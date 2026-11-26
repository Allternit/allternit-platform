import { afterEach, describe, expect, test } from "bun:test"
import { MCP } from "../../src/runtime/tools/mcp"
import { McpTaskStream } from "../../src/runtime/tools/mcp/task-stream"
import { McpUserProxy } from "../../src/runtime/tools/mcp/user-proxy"
import { Instance } from "../../src/runtime/context/project/instance"
import { Bus } from "../../src/shared/bus"
import { tmpdir } from "../fixture/fixture"

/** An MCP server with one tool that returns at once; records tools/call params. */
function fakeServer() {
  const calls: any[] = []
  const server = Bun.serve({
    port: 0,
    async fetch(req) {
      if (req.method !== "POST") return new Response(null, { status: 405 })
      const body: any = await req.json()
      if (body.id === undefined) return new Response(null, { status: 202 })
      const ok = (result: unknown) => Response.json({ jsonrpc: "2.0", id: body.id, result })
      switch (body.method) {
        case "initialize":
          return ok({ protocolVersion: body.params.protocolVersion, capabilities: { tools: {} }, serverInfo: { name: "allternit-connectors", version: "1" } })
        case "tools/list":
          return ok({ tools: [{ name: "app__ping", inputSchema: { type: "object", properties: { n: { type: "number" } } } }] })
        case "tools/call":
          calls.push(body.params)
          return ok({ content: [{ type: "text", text: "pong" }] })
        default:
          return Response.json({ jsonrpc: "2.0", id: body.id, error: { code: -32601, message: "no" } })
      }
    },
  })
  return { server, calls, url: `http://127.0.0.1:${server.port}/mcp/user-proxy` }
}

const servers: Array<{ stop(force?: boolean): void }> = []
afterEach(() => {
  while (servers.length) servers.pop()!.stop(true)
})

describe("MCP.callDirect (Island bar run path)", () => {
  test("an instant tool emits one mcp.task part, working then completed, in the given session; meta reaches _meta", async () => {
    const fake = fakeServer()
    servers.push(fake.server)
    const release = McpUserProxy.register("ses_direct", { server: "allternit-connectors", url: fake.url, sessionId: "ses_direct", token: "t" })
    try {
      const client = await McpUserProxy.client("ses_direct")
      await using tmp = await tmpdir()
      await Instance.provide({
        directory: tmp.path,
        fn: async () => {
          const catalog = await MCP.toolCatalog({ "allternit-connectors": client! })
          const parts: Array<{ sessionID: string; status: string }> = []
          const unsub = Bus.subscribe(McpTaskStream.TaskEvent, (e) => parts.push({ sessionID: e.properties.sessionID, status: e.properties.part.status }))
          const out = await MCP.callDirect({
            sessionID: "bar:job-1",
            server: "allternit-connectors",
            tool: "app__ping",
            args: { n: 1 },
            meta: { "allternit/approvalId": "apr-1" },
            catalog: catalog.tools,
          })
          await new Promise((r) => setTimeout(r, 50))
          unsub()
          expect(out?.result).toMatchObject({ content: [{ text: "pong" }] })
          expect(parts).toEqual([
            { sessionID: "bar:job-1", status: "working" },
            { sessionID: "bar:job-1", status: "completed" },
          ])
          expect(fake.calls[0]).toMatchObject({ name: "app__ping", arguments: { n: 1 }, _meta: { "allternit/approvalId": "apr-1" } })
        },
      })
    } finally {
      release()
    }
  }, 60_000)

  test("a call carrying mcpProxy resolves the tool from the user proxy, uses toolCallID as task id, and releases the proxy", async () => {
    const fake = fakeServer()
    servers.push(fake.server)
    await using tmp = await tmpdir()
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const tasks: Array<{ taskId: string; status: string }> = []
        const unsub = Bus.subscribe(McpTaskStream.TaskEvent, (e) =>
          tasks.push({ taskId: (e.properties.part as any).taskId, status: e.properties.part.status }),
        )
        const out = await MCP.callDirect({
          sessionID: "bar:job-2",
          server: "allternit-connectors",
          tool: "app__ping",
          args: { n: 2 },
          toolCallID: "bar_job-2",
          mcpProxy: { server: "allternit-connectors", url: fake.url, sessionId: "bar:job-2", token: "t" },
        })
        await new Promise((r) => setTimeout(r, 50))
        unsub()
        expect(out?.toolCallID).toBe("bar_job-2")
        expect(out?.result).toMatchObject({ content: [{ text: "pong" }] })
        expect(tasks).toEqual([
          { taskId: "direct:bar_job-2", status: "working" },
          { taskId: "direct:bar_job-2", status: "completed" },
        ])
        expect(fake.calls[0]).toMatchObject({ name: "app__ping", arguments: { n: 2 } })
        expect(McpUserProxy.current("bar:job-2")).toBeUndefined()
      },
    })
  }, 60_000)
})
