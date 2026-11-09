// MCP tasks (2025-11-25): a task-capable tool runs as a task the client polls,
// the result comes from tasks/result, and stopping the call sends tasks/cancel.
// Real SDK Client against a raw JSON-RPC server (the edge): SDK 2.3.1's own
// Server refuses to return a task from tools/call, so it can't stand in here.
import { describe, expect, test } from "bun:test"
import { Client, InMemoryTransport } from "@modelcontextprotocol/client"
import { callToolAsTask, formatTaskUpdate, shouldCallAsTask } from "../../src/shared/util/mcp-tasks"

type Handler = (params: any) => any
/** A raw JSON-RPC MCP server (2025-11-25) on one end of an in-memory pair. */
function rawServer(capabilities: Record<string, unknown>, handlers: Record<string, Handler>) {
  const [clientSide, serverSide] = InMemoryTransport.createLinkedPair()
  serverSide.onmessage = async (msg: any) => {
    if (msg.id === undefined) return
    const reply = (body: Record<string, unknown>) => serverSide.send({ jsonrpc: "2.0", id: msg.id, ...body } as any)
    if (msg.method === "initialize") {
      return reply({ result: { protocolVersion: "2025-11-25", capabilities, serverInfo: { name: "slow", version: "1.0.0" } } })
    }
    const handler = handlers[msg.method]
    if (!handler) return reply({ error: { code: -32601, message: "Method not found" } })
    return reply({ result: await handler(msg.params) })
  }
  return { clientSide, start: () => serverSide.start() }
}

function taskServer(opts: { workingPolls: number }) {
  const seen = { methods: [] as string[], ttl: undefined as unknown, cancelled: [] as string[] }
  let polls = 0
  let status = "working"
  const task = (extra: Record<string, unknown> = {}) => {
    const now = new Date().toISOString()
    return { taskId: "t1", status, createdAt: now, lastUpdatedAt: now, ttl: 60000, pollInterval: 1, ...extra }
  }
  const server = rawServer(
    { tools: {}, tasks: { requests: { tools: { call: {} } }, cancel: {} } },
    {
      "tools/list": () => ({
        tools: [{ name: "index", inputSchema: { type: "object", properties: {} }, execution: { taskSupport: "required" } }],
      }),
      "tools/call": params => {
        seen.methods.push("tools/call")
        seen.ttl = params.task?.ttl
        return { task: task({ statusMessage: "queued" }) }
      },
      "tasks/get": () => {
        seen.methods.push("tasks/get")
        polls++
        if (status === "working" && polls > opts.workingPolls) status = "completed"
        return task({ statusMessage: `step ${polls}` })
      },
      "tasks/result": () => {
        seen.methods.push("tasks/result")
        return { content: [{ type: "text", text: "indexed 3 files" }] }
      },
      "tasks/cancel": params => {
        seen.cancelled.push(params.taskId)
        status = "cancelled"
        return task()
      },
    },
  )
  return { server, seen }
}

async function connect(server: ReturnType<typeof rawServer>) {
  const client = new Client({ name: "gizzi-test", version: "1.0.0" })
  await server.start()
  await client.connect(server.clientSide)
  return client
}

describe("MCP tasks", () => {
  test("a task tool is polled to completion and returns the tool result", async () => {
    const { server, seen } = taskServer({ workingPolls: 2 })
    const client = await connect(server)
    const { tools } = await client.listTools()
    expect(shouldCallAsTask(client as any, tools[0]!.execution?.taskSupport)).toBe(true)

    const lines: string[] = []
    const result = await callToolAsTask({
      client: client as any,
      name: "index",
      args: {},
      onUpdate: u => lines.push(formatTaskUpdate(u)),
      wait: async () => {},
    })
    expect(result.content[0].text).toBe("indexed 3 files")
    expect(seen.methods).toEqual(["tools/call", "tasks/get", "tasks/get", "tasks/get", "tasks/result"])
    expect(typeof seen.ttl).toBe("number")
    expect(lines[0]).toContain("MCP task Working")
    expect(lines.at(-2)).toContain("step 2")
    await client.close()
  })

  test("aborting the call sends tasks/cancel and rejects with AbortError", async () => {
    const { server, seen } = taskServer({ workingPolls: 1_000 })
    const client = await connect(server)
    const controller = new AbortController()
    let polls = 0
    const call = callToolAsTask({
      client: client as any,
      name: "index",
      args: {},
      signal: controller.signal,
      wait: async () => {
        if (++polls === 3) controller.abort()
        if (controller.signal.aborted) throw Object.assign(new Error("aborted"), { name: "AbortError" })
      },
    })
    await expect(call).rejects.toMatchObject({ name: "AbortError" })
    expect(seen.cancelled).toEqual(["t1"])
    await client.close()
  })

  test("a server without the tasks capability gets a plain call", async () => {
    const server = rawServer({ tools: {} }, { "tools/list": () => ({ tools: [] }) })
    const client = await connect(server)
    expect(shouldCallAsTask(client as any, "optional")).toBe(false)
    expect(shouldCallAsTask(client as any, undefined)).toBe(false)
    await client.close()
  })
})
