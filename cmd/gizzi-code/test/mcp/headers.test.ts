// @ts-nocheck
import { test, expect, mock, beforeEach } from "bun:test"

// Track what options were passed to each transport constructor
const transportCalls: Array<{
  type: "streamable" | "sse"
  url: string
  options: { authProvider?: unknown; requestInit?: RequestInit }
}> = []

// Mock the transport constructors to capture their arguments (SDK v2: one package entry)
const realClient = await import("@modelcontextprotocol/client")
// The mock is process-wide in the shared smoke run, so subclass the real transports and only
// fail the example.com URL these tests use; every other test keeps real transports.
const isTestUrl = (url: URL | string) => new URL(String(url)).hostname === "example.com"
class MockStreamableHTTP extends realClient.StreamableHTTPClientTransport {
    constructor(url: URL, options?: { authProvider?: unknown; requestInit?: RequestInit }) {
      super(url, options as any)
      this.testUrl = isTestUrl(url)
      if (!this.testUrl) return
      transportCalls.push({
        type: "streamable",
        url: url.toString(),
        options: options ?? {},
      })
    }
    testUrl: boolean
    async start() {
      if (!this.testUrl) return super.start()
      throw new Error("Mock transport cannot connect")
    }
    async send(...args: any[]) {
      if (!this.testUrl) return (super.send as any)(...args)
      throw new Error("Mock transport cannot connect")
    }
}

class MockSSE extends realClient.SSEClientTransport {
    constructor(url: URL, options?: { authProvider?: unknown; requestInit?: RequestInit }) {
      super(url, options as any)
      this.testUrl = isTestUrl(url)
      if (!this.testUrl) return
      transportCalls.push({
        type: "sse",
        url: url.toString(),
        options: options ?? {},
      })
    }
    testUrl: boolean
    async start() {
      if (!this.testUrl) return super.start()
      throw new Error("Mock transport cannot connect")
    }
    async send(...args: any[]) {
      if (!this.testUrl) return (super.send as any)(...args)
      throw new Error("Mock transport cannot connect")
    }
}

mock.module("@modelcontextprotocol/client", () => ({
  ...realClient,
  StreamableHTTPClientTransport: MockStreamableHTTP,
  SSEClientTransport: MockSSE,
}))

beforeEach(() => {
  transportCalls.length = 0
})

// Import MCP after mocking
const { MCP } = await import("../../src/runtime/tools/mcp")
const { Instance } = await import("../../src/project/instance")
const { tmpdir } = await import("../fixture/fixture")

test("headers are passed to transports when oauth is enabled (default)", async () => {
  await using tmp = await tmpdir({
    init: async (dir) => {
      await Bun.write(
        `${dir}/gizzi.json`,
        JSON.stringify({
          $schema: "https://docs.gizziio.com/config.json",
          mcp: {
            "test-server": {
              type: "remote",
              url: "https://example.com/mcp",
              headers: {
                Authorization: "Bearer test-token",
                "X-Custom-Header": "custom-value",
              },
            },
          },
        }),
      )
    },
  })

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      // Trigger MCP initialization - it will fail to connect but we can check the transport options
      await MCP.add("test-server", {
        type: "remote",
        url: "https://example.com/mcp",
        headers: {
          Authorization: "Bearer test-token",
          "X-Custom-Header": "custom-value",
        },
      }).catch(() => {})

      // Both transports should have been created with headers
      expect(transportCalls.length).toBeGreaterThanOrEqual(1)

      for (const call of transportCalls) {
        expect(call.options.requestInit).toBeDefined()
        expect(call.options.requestInit?.headers).toEqual({
          Authorization: "Bearer test-token",
          "X-Custom-Header": "custom-value",
        })
        // OAuth should be enabled by default, so authProvider should exist
        expect(call.options.authProvider).toBeDefined()
      }
    },
  })
})

test("headers are passed to transports when oauth is explicitly disabled", async () => {
  await using tmp = await tmpdir()

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      transportCalls.length = 0

      await MCP.add("test-server-no-oauth", {
        type: "remote",
        url: "https://example.com/mcp",
        oauth: false,
        headers: {
          Authorization: "Bearer test-token",
        },
      }).catch(() => {})

      expect(transportCalls.length).toBeGreaterThanOrEqual(1)

      for (const call of transportCalls) {
        expect(call.options.requestInit).toBeDefined()
        expect(call.options.requestInit?.headers).toEqual({
          Authorization: "Bearer test-token",
        })
        // OAuth is disabled, so no authProvider
        expect(call.options.authProvider).toBeUndefined()
      }
    },
  })
})

test("no requestInit when headers are not provided", async () => {
  await using tmp = await tmpdir()

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      transportCalls.length = 0

      await MCP.add("test-server-no-headers", {
        type: "remote",
        url: "https://example.com/mcp",
      }).catch(() => {})

      expect(transportCalls.length).toBeGreaterThanOrEqual(1)

      for (const call of transportCalls) {
        // No headers means requestInit should be undefined
        expect(call.options.requestInit).toBeUndefined()
      }
    },
  })
})
