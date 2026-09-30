import { afterEach, describe, expect, test } from "bun:test"
import { MCP } from "../../src/runtime/tools/mcp"
import { Instance } from "../../src/runtime/context/project/instance"
import { tmpdir } from "../fixture/fixture"
import { McpUserProxy } from "../../src/runtime/tools/mcp/user-proxy"
import { buildMcpAppFrame, mcpAppHtml } from "../../src/runtime/tools/mcp/apps"

const TOKEN = "proxy-token-abc"
const APP_HTML = "<html><body>dash</body></html>"

type Seen = { method: string; auth: string | null; session: string | null }

/** A stand-in for allternit-api's /mcp/user-proxy: stateless streamable HTTP, namespaced tools. */
function fakeProxy() {
  const seen: Seen[] = []
  const server = Bun.serve({
    port: 0,
    async fetch(req) {
      if (req.method !== "POST") return new Response(null, { status: 405 })
      const body: any = await req.json()
      seen.push({
        method: body.method,
        auth: req.headers.get("authorization"),
        session: req.headers.get("x-allternit-session"),
      })
      if (req.headers.get("authorization") !== `Bearer ${TOKEN}`) return new Response("no", { status: 401 })
      if (body.id === undefined) return new Response(null, { status: 202 })
      const ok = (result: unknown) => Response.json({ jsonrpc: "2.0", id: body.id, result })
      switch (body.method) {
        case "initialize":
          return ok({
            protocolVersion: body.params.protocolVersion,
            capabilities: { tools: {}, resources: {} },
            serverInfo: { name: "allternit-connectors", version: "1" },
          })
        case "tools/list":
          return ok({
            tools: [
              {
                name: "dash-server__show_dashboard",
                title: "Dashboard",
                description: "Show the dashboard",
                inputSchema: { type: "object", properties: { range: { type: "string" } } },
                _meta: { ui: { resourceUri: "ui://dash/app" }, "allternit/connector": { id: "conn-1", name: "Dash" } },
              },
              { name: "dash-server__plain", inputSchema: { type: "object" } },
              {
                name: "dash-server__model_only",
                inputSchema: { type: "object" },
                _meta: { ui: { visibility: ["model"] } },
              },
              // a proxy never sends this, but a host must not trust that
              {
                name: "dash-server__refresh_data",
                inputSchema: { type: "object" },
                _meta: { ui: { visibility: ["app"] } },
              },
            ],
          })
        case "resources/read":
          return ok({
            contents: [
              {
                uri: body.params.uri,
                mimeType: "text/html;profile=mcp-app",
                text: APP_HTML,
                _meta: { ui: { csp: { connectDomains: ["https://api.dash.example"] }, permissions: { camera: {} } } },
              },
            ],
          })
        default:
          return Response.json({ jsonrpc: "2.0", id: body.id, error: { code: -32601, message: "no" } })
      }
    },
  })
  return { server, seen, url: `http://127.0.0.1:${server.port}/mcp/user-proxy` }
}

const entryFor = (url: string, overrides: Record<string, unknown> = {}) => ({
  server: "allternit-connectors",
  url,
  sessionId: "ses_1",
  token: TOKEN,
  ...overrides,
})

/** Run `fn` inside a project instance, as a real turn does. */
async function inInstance<T>(fn: () => Promise<T>): Promise<T> {
  await using tmp = await tmpdir()
  return Instance.provide({ directory: tmp.path, fn })
}

const servers: Array<{ stop(force?: boolean): void }> = []
afterEach(() => {
  while (servers.length) servers.pop()!.stop(true)
})

describe("McpUserProxy.parse", () => {
  test("accepts a complete entry and rejects everything else without throwing", () => {
    expect(McpUserProxy.parse(entryFor("http://127.0.0.1:1/mcp/user-proxy"))?.server).toBe("allternit-connectors")
    expect(McpUserProxy.parse(undefined)).toBeUndefined()
    expect(McpUserProxy.parse(null)).toBeUndefined()
    expect(McpUserProxy.parse({})).toBeUndefined()
    expect(McpUserProxy.parse(entryFor("ftp://x/y"))).toBeUndefined()
    expect(McpUserProxy.parse(entryFor("not a url"))).toBeUndefined()
    expect(McpUserProxy.parse(entryFor("http://x/y", { token: "" }))).toBeUndefined()
    expect(McpUserProxy.parse(entryFor("http://x/y", { sessionId: undefined }))).toBeUndefined()
  })
})

describe("McpUserProxy turn registration", () => {
  test("is held for the turn, per session, and gone after release", () => {
    const url = "http://127.0.0.1:1/mcp/user-proxy"
    const release = McpUserProxy.register("ses_a", entryFor(url))
    expect(McpUserProxy.current("ses_a")?.token).toBe(TOKEN)
    expect(McpUserProxy.current("ses_b")).toBeUndefined()
    release()
    expect(McpUserProxy.current("ses_a")).toBeUndefined()
  })

  test("a newer turn replaces the older one; the older release does not evict it", () => {
    const url = "http://127.0.0.1:1/mcp/user-proxy"
    const first = McpUserProxy.register("ses_c", entryFor(url, { token: "t1" }))
    const second = McpUserProxy.register("ses_c", entryFor(url, { token: "t2" }))
    expect(McpUserProxy.current("ses_c")?.token).toBe("t2")
    first()
    expect(McpUserProxy.current("ses_c")?.token).toBe("t2")
    second()
    expect(McpUserProxy.current("ses_c")).toBeUndefined()
  })

  test("nothing to register is a no-op", () => {
    const release = McpUserProxy.register("ses_d", undefined)
    expect(McpUserProxy.current("ses_d")).toBeUndefined()
    release()
  })
})

describe("the connector proxy as a turn-scoped MCP server", () => {
  test("its tools join the catalog under the proxy's server key; state is not recorded; credentials ride headers", async () => {
    const proxy = fakeProxy()
    servers.push(proxy.server)
    const release = McpUserProxy.register("ses_1", entryFor(proxy.url))
    try {
      const client = await McpUserProxy.client("ses_1")
      expect(client).toBeDefined()
      const catalog = await inInstance(() => MCP.toolCatalog({ "allternit-connectors": client! }))

      const names = Object.keys(catalog.tools).filter((n) => n.includes("dash-server"))
      expect(names.sort()).toEqual([
        "mcp__allternit-connectors__dash-server__model_only",
        "mcp__allternit-connectors__dash-server__plain",
        "mcp__allternit-connectors__dash-server__show_dashboard",
      ])
      // app-only tools never reach the model even if a server lists them
      expect(names.some((n) => n.includes("refresh_data"))).toBe(false)

      const d = catalog.descriptors["mcp__allternit-connectors__dash-server__show_dashboard"]
      expect(d.serverName).toBe("allternit-connectors")
      expect(d.originalName).toBe("dash-server__show_dashboard")
      expect(d.uiResourceUri).toBe("ui://dash/app")

      // the instance's own MCP state never learned about the proxy
      expect(Object.keys(await inInstance(() => MCP.status()))).not.toContain("allternit-connectors")

      // token and session binding travel as headers on every request
      expect(proxy.seen.length).toBeGreaterThan(0)
      for (const s of proxy.seen) {
        expect(s.auth).toBe(`Bearer ${TOKEN}`)
        expect(s.session).toBe("ses_1")
      }
    } finally {
      release()
    }
  }, 60_000)

  test("a proxy that rejects the token yields no client instead of an error", async () => {
    const proxy = fakeProxy()
    servers.push(proxy.server)
    const release = McpUserProxy.register("ses_2", entryFor(proxy.url, { token: "wrong" }))
    try {
      expect(await McpUserProxy.client("ses_2")).toBeUndefined()
      // and the catalog is simply what it was without the proxy
      const catalog = await inInstance(() => MCP.toolCatalog({}))
      expect(Object.keys(catalog.tools).some((n) => n.includes("dash-server"))).toBe(false)
    } finally {
      release()
    }
  }, 60_000)

  test("without a registration there is no client", async () => {
    expect(await McpUserProxy.client("ses_nobody")).toBeUndefined()
  })
})

describe("mcp_app frames for proxied tools (gizzi agent-chat mirror)", () => {
  const completed = (overrides: Record<string, unknown> = {}) => ({
    type: "tool",
    callID: "call-7",
    tool: "mcp__allternit-connectors__dash-server__show_dashboard",
    state: {
      status: "completed",
      input: { range: "7d" },
      output: "ran it",
      metadata: {
        mcp: {
          server: "allternit-connectors",
          tool: "dash-server__show_dashboard",
          result: { content: [{ type: "text", text: "ran it" }], structuredContent: { rows: [1, 2, 3] } },
        },
      },
    },
    ...overrides,
  })

  test("a completed ui tool yields the frame allternit-api would emit", async () => {
    const proxy = fakeProxy()
    servers.push(proxy.server)
    const release = McpUserProxy.register("ses_1", entryFor(proxy.url))
    try {
      const frame: any = await McpUserProxy.appFrameForPart("ses_1", completed(), "msg_1")
      expect(frame.type).toBe("mcp_app")
      expect(frame.messageId).toBe("msg_1")
      expect(frame.toolCallId).toBe("call-7")
      expect(frame.toolName).toBe("dash-server__show_dashboard".split("__")[1])
      expect(frame.connectorId).toBe("conn-1")
      expect(frame.connectorName).toBe("Dash")
      expect(frame.title).toBe("Dashboard")
      expect(frame.resourceUri).toBe("ui://dash/app")
      expect(frame.html).toBe(APP_HTML)
      expect(frame.allow).toBe("camera")
      expect(frame.csp.connectDomains).toEqual(["https://api.dash.example"])
      expect(frame.toolInput).toEqual({ range: "7d" })
      expect(frame.toolResult.structuredContent).toEqual({ rows: [1, 2, 3] })
      // every field the web client's buildMcpAppPart requires
      for (const key of ["toolCallId", "toolName", "connectorId", "connectorName", "resourceUri", "html", "title"]) {
        expect(typeof frame[key]).toBe("string")
        expect(frame[key].length).toBeGreaterThan(0)
      }
    } finally {
      release()
    }
  })

  test("no frame for tools without a ui resource, other servers, or when no proxy is registered", async () => {
    const proxy = fakeProxy()
    servers.push(proxy.server)
    const release = McpUserProxy.register("ses_1", entryFor(proxy.url))
    try {
      const plain = completed()
      plain.state.metadata.mcp.tool = "dash-server__plain"
      expect(await McpUserProxy.appFrameForPart("ses_1", plain, "m")).toBeUndefined()
      const other = completed()
      other.state.metadata.mcp.server = "someone-else"
      expect(await McpUserProxy.appFrameForPart("ses_1", other, "m")).toBeUndefined()
      const ghost = completed()
      ghost.state.metadata.mcp.tool = "dash-server__ghost"
      expect(await McpUserProxy.appFrameForPart("ses_1", ghost, "m")).toBeUndefined()
      expect(await McpUserProxy.appFrameForPart("ses_1", { type: "tool", callID: "c", state: {} }, "m")).toBeUndefined()
    } finally {
      release()
    }
    expect(await McpUserProxy.appFrameForPart("ses_1", completed(), "m")).toBeUndefined()
  })
})

describe("frame builder", () => {
  test("reads html from text or blob and _meta.ui from content then result level", () => {
    expect(mcpAppHtml({ contents: [{ mimeType: "text/html", text: "h", _meta: { ui: { domain: "a" } } }] })?.ui.domain).toBe("a")
    expect(mcpAppHtml({ _meta: { ui: { domain: "b" } }, contents: [{ mimeType: "text/html;profile=mcp-app", text: "h" }] })?.ui.domain).toBe("b")
    expect(mcpAppHtml({ contents: [{ mimeType: "text/html", blob: Buffer.from("<p>x</p>").toString("base64") }] })?.html).toBe("<p>x</p>")
    expect(mcpAppHtml({ contents: [{ mimeType: "text/plain", text: "h" }] })).toBeUndefined()
  })

  test("oversized documents are not emitted; splits original tool name at the first __", () => {
    const base = {
      messageId: "m",
      callId: "c",
      connector: { id: "1", name: "n" },
      tool: { name: "t" },
      resourceUri: "ui://x",
      ui: {},
      toolInput: {},
      toolResult: {},
    }
    expect(buildMcpAppFrame({ ...base, html: "x".repeat(3 * 1024 * 1024) })).toBeUndefined()
    expect(buildMcpAppFrame({ ...base, html: "ok" })?.prefersBorder).toBe(true)
    expect(McpUserProxy.originalToolName("a__b__c")).toBe("b__c")
    expect(McpUserProxy.originalToolName("a___private")).toBe("_private")
  })
})
