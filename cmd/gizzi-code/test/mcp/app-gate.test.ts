import { afterEach, describe, expect, test } from "bun:test"
import { MCP } from "../../src/runtime/tools/mcp"
import { Instance } from "../../src/runtime/context/project/instance"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"
import { McpUserProxy } from "../../src/runtime/tools/mcp/user-proxy"
import { tmpdir } from "../fixture/fixture"

const TOKEN = "proxy-token-gate"
const SHOW = "mcp__allternit-connectors__dash-server__show_dashboard"
const READ = "mcp__allternit-connectors__dash-server__read_only"

/** allternit-api's proxy as gizzi sees it: one tool marked requiresConfirmation, one not; records tools/call params. */
function fakeProxy() {
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
          return ok({
            protocolVersion: body.params.protocolVersion,
            capabilities: { tools: {} },
            serverInfo: { name: "allternit-connectors", version: "1" },
          })
        case "tools/list":
          return ok({
            tools: [
              {
                name: "dash-server__show_dashboard",
                title: "Dashboard",
                inputSchema: { type: "object", properties: { range: { type: "string" } } },
                _meta: {
                  "allternit/connector": { id: "conn-1", name: "Dash App" },
                  "allternit/requiresConfirmation": true,
                },
              },
              {
                name: "dash-server__read_only",
                inputSchema: { type: "object" },
                _meta: { "allternit/connector": { id: "conn-1", name: "Dash App" }, "allternit/requiresConfirmation": false },
              },
            ],
          })
        case "tools/call":
          calls.push(body.params)
          return ok({ content: [{ type: "text", text: "ran" }] })
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

type Ask = McpUserProxy.GateInput["ask"]
const approving: Ask = async () => undefined
const declining: Ask = async () => {
  throw new PermissionNext.RejectedError()
}

describe("permission class", () => {
  test("mcp_app asks in every mode, even ones that allow everything else", () => {
    for (const mode of ["default", "yolo", "auto", "bypassPermissions", "acceptEdits"]) {
      expect(PermissionNext.evaluatePolicy("mcp_app", "x", { configured: [], mode }).action).toBe("ask")
    }
    // a session-wide approval of "*" does not waive it
    const approvals = [{ permission: "mcp_app", pattern: "*", action: "allow" as const }]
    expect(PermissionNext.evaluatePolicy("mcp_app", "x", { configured: [], approvals, mode: "default" }).action).toBe("ask")
  })
})

describe("McpUserProxy.gate", () => {
  test("shows app, tool title and arguments; approval is one-shot and per call", async () => {
    let seen: Parameters<Ask>[0] | undefined
    await McpUserProxy.gate({
      callID: "c1",
      tool: "show_dashboard",
      title: "Dashboard",
      app: "Dash App",
      args: { range: "7d" },
      ask: async (req) => void (seen = req),
    })
    expect(seen!.permission).toBe("mcp_app")
    expect(seen!.always).toEqual([]) // never remembered
    expect(seen!.patterns[0]).toContain("Dash App")
    expect(seen!.patterns[0]).toContain("Dashboard")
    expect(seen!.patterns[0]).toContain('"range": "7d"')
    expect(seen!.metadata).toMatchObject({ app: "Dash App", title: "Dashboard" })
    expect(McpUserProxy.consumeApproval("c1")).toBe(true)
    expect(McpUserProxy.consumeApproval("c1")).toBe(false)
    expect(McpUserProxy.consumeApproval("other")).toBe(false)
  })

  test("declining throws the fixed message and grants nothing", async () => {
    await expect(
      McpUserProxy.gate({ callID: "c2", tool: "t", args: {}, ask: declining }),
    ).rejects.toThrow("The user declined this tool call")
    expect(McpUserProxy.consumeApproval("c2")).toBe(false)
    // any refusal of the ask (plan mode deny, catastrophic, etc.) reads the same
    await expect(
      McpUserProxy.gate({
        callID: "c3",
        tool: "t",
        args: {},
        ask: async () => {
          throw new Error("denied by mode")
        },
      }),
    ).rejects.toThrow("The user declined this tool call")
  })

  test("long arguments are cut with an explicit marker", async () => {
    const big = { text: "x".repeat(5000) }
    const shown = McpUserProxy.describeArguments(big)
    const total = JSON.stringify(big, null, 2).length
    expect(shown).toContain(`… (${total - 1000} more characters not shown)`)
    expect(shown.length).toBeLessThan(1100)
    expect(McpUserProxy.describeArguments({ a: 1 })).not.toContain("not shown")
    const circular: any = {}
    circular.self = circular
    expect(McpUserProxy.describeArguments(circular)).toBe("(arguments could not be shown)")
  })
})

describe("gate + catalog + proxy call", () => {
  async function withCatalog<T>(fn: (catalog: Awaited<ReturnType<typeof MCP.toolCatalog>>, calls: any[]) => Promise<T>) {
    const proxy = fakeProxy()
    servers.push(proxy.server)
    const release = McpUserProxy.register("ses_gate", {
      server: "allternit-connectors",
      url: proxy.url,
      sessionId: "ses_gate",
      token: TOKEN,
    })
    try {
      const client = await McpUserProxy.client("ses_gate")
      expect(client).toBeDefined()
      await using tmp = await tmpdir()
      return await Instance.provide({
        directory: tmp.path,
        fn: async () => fn(await MCP.toolCatalog({ "allternit-connectors": client! }), proxy.calls),
      })
    } finally {
      release()
    }
  }

  test("descriptors carry the marker, the app name and the title", () =>
    withCatalog(async (catalog) => {
      const d = catalog.descriptors[SHOW]
      expect(d.requiresConfirmation).toBe(true)
      expect(d.appName).toBe("Dash App")
      expect(d.title).toBe("Dashboard")
      expect(catalog.descriptors[READ].requiresConfirmation).toBe(false)
    }), 60_000)

  test("approve → the call carries the approval flag, once", () =>
    withCatalog(async (catalog, calls) => {
      const opts: any = { toolCallId: "call-a", messages: [] }
      await McpUserProxy.gate({ callID: "call-a", tool: "show_dashboard", args: { range: "7d" }, ask: approving })
      await catalog.tools[SHOW].execute!({ range: "7d" }, opts)
      expect(calls[0]).toMatchObject({
        name: "dash-server__show_dashboard",
        arguments: { range: "7d" },
        _meta: { "allternit/approved": true },
      })
      // a second execution for the same call id has no approval left
      await catalog.tools[SHOW].execute!({ range: "7d" }, opts)
      expect(calls[1]._meta?.["allternit/approved"]).toBeUndefined()
    }), 60_000)

  test("deny → tool error and the proxy is never called", () =>
    withCatalog(async (catalog, calls) => {
      const run = async () => {
        await McpUserProxy.gate({ callID: "call-d", tool: "show_dashboard", args: {}, ask: declining })
        await catalog.tools[SHOW].execute!({}, { toolCallId: "call-d", messages: [] } as any)
      }
      await expect(run()).rejects.toThrow("The user declined this tool call")
      expect(calls.length).toBe(0)
    }), 60_000)

  test("unmarked tools never send the flag, even if a stale approval exists", () =>
    withCatalog(async (catalog, calls) => {
      await McpUserProxy.gate({ callID: "call-r", tool: "read_only", args: {}, ask: approving })
      await catalog.tools[READ].execute!({}, { toolCallId: "call-r", messages: [] } as any)
      expect(calls[0]._meta?.["allternit/approved"]).toBeUndefined()
      McpUserProxy.clearApproval("call-r")
    }), 60_000)
})

describe("gate through gizzi's real permission system", () => {
  async function nextAsk(): Promise<PermissionNext.Request> {
    for (let i = 0; i < 500; i++) {
      const [ask] = await PermissionNext.list()
      if (ask) return ask
      await new Promise((r) => setTimeout(r, 10))
    }
    throw new Error("no permission ask was opened")
  }

  const realAsk = (sessionID: string): Ask => (req) =>
    PermissionNext.ask({ ...req, sessionID, ruleset: [], mode: "yolo" } as any)

  test("even in yolo the user is asked; once approves this call only, reject declines", async () => {
    await using tmp = await tmpdir({ git: true })
    await Instance.provide({
      directory: tmp.path,
      fn: async () => {
        const approvedRun = McpUserProxy.gate({
          callID: "real-1",
          tool: "show_dashboard",
          title: "Dashboard",
          app: "Dash App",
          args: { range: "7d" },
          ask: realAsk("ses_real"),
        })
        const ask = await nextAsk()
        expect(ask.permission).toBe("mcp_app")
        expect(ask.metadata).toMatchObject({ app: "Dash App", title: "Dashboard" })
        expect(McpUserProxy.consumeApproval("real-1")).toBe(false) // nothing granted before the reply
        await PermissionNext.reply({ requestID: ask.id, reply: "once" })
        await approvedRun
        expect(McpUserProxy.consumeApproval("real-1")).toBe(true)

        // "always" is not remembered for this class: the next call asks again
        const again = McpUserProxy.gate({ callID: "real-2", tool: "t", args: {}, ask: realAsk("ses_real") })
        const second = await nextAsk()
        await PermissionNext.reply({ requestID: second.id, reply: "always" })
        await again
        const third = McpUserProxy.gate({ callID: "real-3", tool: "t", args: {}, ask: realAsk("ses_real") })
        const thirdAsk = await nextAsk()
        const outcome = third.then(
          () => "approved",
          (e: Error) => e.message,
        )
        await PermissionNext.reply({ requestID: thirdAsk.id, reply: "reject" })
        expect(await outcome).toBe("The user declined this tool call")
        expect(McpUserProxy.consumeApproval("real-3")).toBe(false)
      },
    })
  }, 60_000)
})
