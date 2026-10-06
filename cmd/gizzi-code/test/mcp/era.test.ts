// Dual-era MCP connect: a 2026-07-28 server is spoken to statelessly (server/discover, no
// initialize), a 2025-era server through initialize, and the per-server verdict cache
// skips the probe on the next connect.
import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test"
import fs from "fs/promises"
import os from "os"
import path from "path"
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client"
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio"
import { createRequire } from "module"
import { Server, createMcpHandler } from "@modelcontextprotocol/server"
import { McpEra } from "../../src/runtime/tools/mcp/era"
import { MCP_APPROVED_META, MCP_APPS_CLIENT_CAPABILITIES, MCP_APPS_EXTENSION_ID } from "../../src/runtime/tools/mcp/apps"

const tmp = await fs.mkdtemp(path.join(os.tmpdir(), "gizzi-mcp-era-"))
const cacheFile = path.join(tmp, "mcp-era.json")

beforeEach(async () => {
  await fs.rm(cacheFile, { force: true })
  McpEra.setCacheFileForTesting(cacheFile)
})
afterEach(() => McpEra.setCacheFileForTesting(undefined))
afterAll(() => fs.rm(tmp, { recursive: true, force: true }))

type Seen = { methods: string[]; callMeta: Record<string, unknown>[]; wireCallMeta: Record<string, unknown>[] }

function modernServer() {
  const seen: Seen = { methods: [], callMeta: [], wireCallMeta: [] }
  const handler = createMcpHandler(
    () => {
      const server = new Server({ name: "modern", version: "1.0.0" }, { capabilities: { tools: {} } })
      server.setRequestHandler("tools/list", async () => ({
        tools: [{ name: "echo", inputSchema: { type: "object", properties: {} } }],
      }))
      server.setRequestHandler("tools/call", async (request) => {
        seen.callMeta.push((request.params._meta ?? {}) as Record<string, unknown>)
        return { content: [{ type: "text", text: "ok" }] }
      })
      return server
    },
    { legacy: "reject" },
  )
  const http = Bun.serve({
    port: 0,
    async fetch(req) {
      if (req.method === "POST") {
        const body = (await req.clone().json().catch(() => undefined)) as
          | { method?: string; params?: { _meta?: Record<string, unknown> } }
          | undefined
        if (body?.method) seen.methods.push(body.method)
        if (body?.method === "tools/call") seen.wireCallMeta.push(body.params?._meta ?? {})
      }
      return handler.fetch(req)
    },
  })
  return { url: `http://127.0.0.1:${http.port}/mcp`, seen, stop: () => http.stop(true) }
}

/** A 2025-era server: no server/discover, sessionless initialize + tools/list. */
function legacyServer() {
  const seen: Seen = { methods: [], callMeta: [], wireCallMeta: [] }
  const http = Bun.serve({
    port: 0,
    async fetch(req) {
      if (req.method !== "POST") return new Response(null, { status: 405 })
      const body = (await req.json()) as { id?: number; method: string; params?: any }
      seen.methods.push(body.method)
      if (body.id === undefined) return new Response(null, { status: 202 })
      const reply = (result: unknown) => Response.json({ jsonrpc: "2.0", id: body.id, result })
      switch (body.method) {
        case "initialize":
          return reply({
            protocolVersion: body.params.protocolVersion,
            capabilities: { tools: {} },
            serverInfo: { name: "legacy", version: "0.1.0" },
          })
        case "tools/list":
          return reply({ tools: [{ name: "echo", inputSchema: { type: "object", properties: {} } }] })
        case "tools/call":
          seen.callMeta.push(body.params?._meta ?? {})
          return reply({ content: [{ type: "text", text: "ok" }] })
        default:
          return Response.json({ jsonrpc: "2.0", id: body.id, error: { code: -32601, message: "Method not found" } })
      }
    },
  })
  return { url: `http://127.0.0.1:${http.port}/mcp`, seen, stop: () => http.stop(true) }
}

function connect(key: string, url: string) {
  return McpEra.connect({
    key,
    target: url,
    timeoutMs: 5_000,
    client: (versionNegotiation) =>
      new Client({ name: "gizzi", version: "0" }, { capabilities: MCP_APPS_CLIENT_CAPABILITIES, versionNegotiation }),
    transport: () => new StreamableHTTPClientTransport(new URL(url)),
  })
}

describe("McpEra.connect", () => {
  test("2026-07-28 server: server/discover, no initialize, approvals _meta and MCP Apps capability carried", async () => {
    const srv = modernServer()
    try {
      const { client } = await connect("modern", srv.url)
      expect(client.getProtocolEra()).toBe("modern")
      expect(client.getNegotiatedProtocolVersion()).toBe("2026-07-28")
      expect(srv.seen.methods).toContain("server/discover")
      expect(srv.seen.methods).not.toContain("initialize")

      const tools = await client.listTools()
      expect(tools.tools.map((t) => t.name)).toEqual(["echo"])
      await client.callTool({ name: "echo", arguments: {}, _meta: { [MCP_APPROVED_META]: true } })
      const meta = srv.seen.callMeta[0]
      expect(meta[MCP_APPROVED_META]).toBe(true)
      // Stateless: the client's capabilities ride on every request's `_meta` envelope (the
      // server SDK lifts the reserved keys off before the handler sees them).
      const wire = srv.seen.wireCallMeta[0]
      expect(wire["io.modelcontextprotocol/protocolVersion"]).toBe("2026-07-28")
      const caps = wire["io.modelcontextprotocol/clientCapabilities"] as any
      expect(caps?.extensions?.[MCP_APPS_EXTENSION_ID]).toBeDefined()
      await client.close()

      // Second connect adopts the cached DiscoverResult and confirms it with one discover on
      // the live connection; still no initialize.
      const before = srv.seen.methods.filter((m) => m === "server/discover").length
      const again = await connect("modern", srv.url)
      expect(again.client.getProtocolEra()).toBe("modern")
      expect(srv.seen.methods.filter((m) => m === "server/discover").length).toBe(before + 1)
      expect(srv.seen.methods).not.toContain("initialize")
      await again.client.close()
    } finally {
      srv.stop()
    }
  })

  test("2025-era server: falls back to initialize, caches legacy, skips the probe next time", async () => {
    const srv = legacyServer()
    try {
      const { client } = await connect("legacy", srv.url)
      expect(client.getProtocolEra()).toBe("legacy")
      expect(srv.seen.methods).toContain("initialize")
      await client.callTool({ name: "echo", arguments: {}, _meta: { [MCP_APPROVED_META]: true } })
      expect(srv.seen.callMeta[0][MCP_APPROVED_META]).toBe(true)
      await client.close()

      const cached = JSON.parse(await fs.readFile(cacheFile, "utf8"))
      expect(cached.legacy.era).toBe("legacy")

      srv.seen.methods.length = 0
      const again = await connect("legacy", srv.url)
      expect(again.client.getProtocolEra()).toBe("legacy")
      expect(srv.seen.methods).not.toContain("server/discover")
      await again.client.close()
    } finally {
      srv.stop()
    }
  })

  test("a stale modern verdict is dropped and the server re-probed", async () => {
    const modern = modernServer()
    const { client } = await connect("moved", modern.url)
    await client.close()
    modern.stop()

    // Same key + target now served by a 2025-era server on the same URL is not possible
    // with ephemeral ports, so point the cached verdict at the legacy server's URL.
    const legacy = legacyServer()
    try {
      const data = JSON.parse(await fs.readFile(cacheFile, "utf8"))
      data.moved.target = legacy.url
      await fs.writeFile(cacheFile, JSON.stringify(data))
      McpEra.setCacheFileForTesting(cacheFile)

      const again = await connect("moved", legacy.url)
      expect(again.client.getProtocolEra()).toBe("legacy")
      const cached = JSON.parse(await fs.readFile(cacheFile, "utf8"))
      expect(cached.moved.era).toBe("legacy")
      await again.client.close()
    } finally {
      legacy.stop()
    }
  })

  test("a changed target ignores the cached verdict", async () => {
    const srv = legacyServer()
    try {
      await (await connect("t", srv.url)).client.close()
      expect(await McpEra.prior("t", srv.url)).toEqual({ kind: "legacy" })
      expect(await McpEra.prior("t", `${srv.url}/other`)).toBeUndefined()
      await McpEra.forget("t")
      expect(await McpEra.prior("t", srv.url)).toBeUndefined()
    } finally {
      srv.stop()
    }
  })

  test("stdio: 2026-07-28 server is modern, a 2025-era SDK server falls back to initialize", async () => {
    const stdio = (key: string, args: string[]) =>
      McpEra.connect({
        key,
        target: JSON.stringify(args),
        timeoutMs: 20_000,
        probeTimeoutMs: McpEra.STDIO_PROBE_TIMEOUT_MS,
        client: (versionNegotiation) => new Client({ name: "gizzi", version: "0" }, { versionNegotiation }),
        transport: () => new StdioClientTransport({ command: process.execPath, args, stderr: "pipe" }),
      })

    const modern = await stdio("stdio-modern", [path.join(import.meta.dir, "../fixture/mcp-modern-stdio-server.ts")])
    expect(modern.client.getProtocolEra()).toBe("modern")
    expect((await modern.client.listTools()).tools.map((t) => t.name)).toEqual(["echo"])
    await modern.client.close()

    // The bundled sequential-thinking server is built on the v1 SDK (2025 era).
    const entry = createRequire(import.meta.url).resolve("@modelcontextprotocol/server-sequential-thinking/dist/index.js")
    const legacy = await stdio("stdio-legacy", [entry])
    expect(legacy.client.getProtocolEra()).toBe("legacy")
    expect((await legacy.client.listTools()).tools.length).toBeGreaterThan(0)
    await legacy.client.close()
    expect(await McpEra.prior("stdio-legacy", JSON.stringify([entry]))).toEqual({ kind: "legacy" })
  }, 60_000)

  test("negotiation modes", () => {
    expect(McpEra.negotiation("legacy", 1000)).toEqual({ mode: "legacy" })
    expect(McpEra.negotiation("auto", 1000)).toEqual({ mode: "auto", probe: { timeoutMs: 1000 } })
    expect(McpEra.negotiation("modern", 1000)).toEqual({ mode: { pin: "2026-07-28" }, probe: { timeoutMs: 1000 } })
  })
})
