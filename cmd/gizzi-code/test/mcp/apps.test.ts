import { describe, expect, test } from "bun:test"
import { Client } from "@modelcontextprotocol/sdk/client/index.js"
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js"
import {
  MCP_APPS_CLIENT_CAPABILITIES,
  isVisibleToModel,
  mcpAppMetadata,
  mcpAppResourceUri,
} from "../../src/runtime/tools/mcp/apps"

describe("MCP Apps tool metadata", () => {
  test("resourceUri: nested, legacy flat, and openai template keys", () => {
    expect(mcpAppResourceUri({ _meta: { ui: { resourceUri: "ui://a/b" } } })).toBe("ui://a/b")
    expect(mcpAppResourceUri({ _meta: { "ui/resourceUri": "ui://legacy" } })).toBe("ui://legacy")
    expect(mcpAppResourceUri({ _meta: { "openai/outputTemplate": "ui://widget" } })).toBe("ui://widget")
    expect(mcpAppResourceUri({ _meta: {} })).toBeUndefined()
    expect(mcpAppResourceUri({})).toBeUndefined()
  })

  test("app-only tools are hidden from the model; everything else is visible", () => {
    expect(isVisibleToModel({})).toBe(true)
    expect(isVisibleToModel({ _meta: { ui: { resourceUri: "ui://x" } } })).toBe(true)
    expect(isVisibleToModel({ _meta: { ui: { visibility: ["model", "app"] } } })).toBe(true)
    expect(isVisibleToModel({ _meta: { ui: { visibility: ["model"] } } })).toBe(true)
    expect(isVisibleToModel({ _meta: { ui: { visibility: ["app"] } } })).toBe(false)
    // unknown-only visibility falls back to the default (both)
    expect(isVisibleToModel({ _meta: { ui: { visibility: ["bogus"] } } })).toBe(true)
  })
})

describe("MCP Apps host capability", () => {
  test("initialize advertises io.modelcontextprotocol/ui with the mcp-app mime type", async () => {
    const bodies: any[] = []
    const server = Bun.serve({
      port: 0,
      async fetch(req) {
        if (req.method !== "POST") return new Response(null, { status: 405 })
        const body = await req.json()
        bodies.push(body)
        if (body.method === "initialize") {
          return Response.json({
            jsonrpc: "2.0",
            id: body.id,
            result: {
              protocolVersion: body.params.protocolVersion,
              capabilities: {},
              serverInfo: { name: "t", version: "0" },
            },
          })
        }
        return new Response(null, { status: 202 })
      },
    })
    try {
      const client = new Client(
        { name: "gizzi", version: "0" },
        { capabilities: MCP_APPS_CLIENT_CAPABILITIES },
      )
      await client.connect(new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${server.port}/mcp`)))
      const init = bodies.find((b) => b.method === "initialize")
      expect(init.params.capabilities.extensions["io.modelcontextprotocol/ui"]).toEqual({
        mimeTypes: ["text/html;profile=mcp-app"],
      })
      await client.close()
    } finally {
      server.stop(true)
    }
  })
})

describe("MCP Apps tool-part metadata", () => {
  const descriptor = { serverName: "demo", originalName: "show", uiResourceUri: "ui://demo/app" }

  test("carries server, tool and the raw result for ui tools only", () => {
    const result = { content: [{ type: "text", text: "hi" }], structuredContent: { n: 1 }, _meta: { k: "v" } }
    expect(mcpAppMetadata(descriptor, result)).toEqual({
      mcp: { server: "demo", tool: "show", result },
    })
    expect(mcpAppMetadata({ serverName: "demo", originalName: "plain" }, result)).toEqual({})
    expect(mcpAppMetadata(undefined, result)).toEqual({})
  })

  test("oversized results are flagged instead of persisted", () => {
    const big = { content: [{ type: "text", text: "x".repeat(300 * 1024) }] }
    expect(mcpAppMetadata(descriptor, big)).toEqual({
      mcp: { server: "demo", tool: "show", resultOmitted: "too_large" },
    })
  })
})
