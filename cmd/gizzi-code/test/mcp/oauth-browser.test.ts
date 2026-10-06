// @ts-nocheck
import { test, expect, mock, beforeEach } from "bun:test"
import { EventEmitter } from "events"

// Track open() calls and control failure behavior
let openShouldFail = false
let openCalledWith: string | undefined

mock.module("open", () => ({
  default: async (url: string) => {
    openCalledWith = url

    // Return a mock subprocess that emits an error if openShouldFail is true
    const subprocess = new EventEmitter()
    if (openShouldFail) {
      // Emit error asynchronously like a real subprocess would
      setTimeout(() => {
        subprocess.emit("error", new Error("spawn xdg-open ENOENT"))
      }, 10)
    }
    return subprocess
  },
}))

// Track what options were passed to each transport constructor
const transportCalls: Array<{
  type: "streamable" | "sse"
  url: string
  options: { authProvider?: unknown }
}> = []

// SDK v2 is one package entry, and bun's module mock is process-wide: subclass the real
// classes and only intercept the example.com URL these tests use.
const realClient = await import("@modelcontextprotocol/client")
const MockUnauthorizedError = realClient.UnauthorizedError
const isTestUrl = (url: URL | string) => new URL(String(url)).hostname === "example.com"

class MockStreamableHTTP extends realClient.StreamableHTTPClientTransport {
  testUrl: boolean
  mockAuthProvider: { redirectToAuthorization?: (url: URL) => Promise<void> } | undefined
  constructor(url: URL, options?: { authProvider?: { redirectToAuthorization?: (url: URL) => Promise<void> } }) {
    super(url, options as any)
    this.testUrl = isTestUrl(url)
    this.mockAuthProvider = options?.authProvider
    if (this.testUrl) transportCalls.push({ type: "streamable", url: url.toString(), options: options ?? {} })
  }
  async mockStart() {
    // Simulate OAuth redirect by calling the authProvider's redirectToAuthorization
    if (this.mockAuthProvider?.redirectToAuthorization) {
      await this.mockAuthProvider.redirectToAuthorization(new URL("https://auth.example.com/authorize?client_id=test"))
    }
    throw new MockUnauthorizedError("Unauthorized")
  }
  async send(...args: any[]) {
    if (!this.testUrl) return (super.send as any)(...args)
    return this.mockStart()
  }
  async finishAuth(...args: any[]) {
    if (!this.testUrl) return (super.finishAuth as any)(...args)
  }
}

class MockSSE extends realClient.SSEClientTransport {
  testUrl: boolean
  constructor(url: URL, options?: any) {
    super(url, options)
    this.testUrl = isTestUrl(url)
    if (this.testUrl) transportCalls.push({ type: "sse", url: url.toString(), options: {} })
  }
  async mockStart() {
    throw new Error("Mock SSE transport cannot connect")
  }
  async send(...args: any[]) {
    if (!this.testUrl) return (super.send as any)(...args)
    return this.mockStart()
  }
}

// The Client triggers the OAuth flow for the test URL; everything else connects for real.
class MockClient extends realClient.Client {
  async connect(transport: any, options?: any) {
    if (transport?.testUrl) return transport.mockStart()
    return super.connect(transport, options)
  }
}

mock.module("@modelcontextprotocol/client", () => ({
  ...realClient,
  Client: MockClient,
  StreamableHTTPClientTransport: MockStreamableHTTP,
  SSEClientTransport: MockSSE,
}))

beforeEach(() => {
  openShouldFail = false
  openCalledWith = undefined
  transportCalls.length = 0
})

// Import modules after mocking
const { MCP } = await import("../../src/runtime/tools/mcp")
const { Bus } = await import("../../src/shared/bus")
const { McpOAuthCallback } = await import("../../src/runtime/tools/mcp/oauth-callback")
const { Instance } = await import("../../src/project/instance")
const { tmpdir } = await import("../fixture/fixture")

test("BrowserOpenFailed event is published when open() throws", async () => {
  await using tmp = await tmpdir({
    init: async (dir) => {
      await Bun.write(
        `${dir}/gizzi.json`,
        JSON.stringify({
          $schema: "https://docs.gizziio.com/config.json",
          mcp: {
            "test-oauth-server": {
              type: "remote",
              url: "https://example.com/mcp",
            },
          },
        }),
      )
    },
  })

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      openShouldFail = true

      const events: Array<{ mcpName: string; url: string }> = []
      const unsubscribe = Bus.subscribe(MCP.BrowserOpenFailed, (evt) => {
        events.push(evt.properties)
      })

      // Run authenticate with a timeout to avoid waiting forever for the callback
      // Attach a handler immediately so callback shutdown rejections
      // don't show up as unhandled between tests.
      const authPromise = MCP.authenticate("test-oauth-server").catch(() => undefined)

      // Config.get() can be slow in tests, so give it plenty of time.
      await new Promise((resolve) => setTimeout(resolve, 2_000))

      // Stop the callback server and cancel any pending auth
      await McpOAuthCallback.stop()

      await authPromise

      unsubscribe()

      // Verify the BrowserOpenFailed event was published
      expect(events.length).toBe(1)
      expect(events[0].mcpName).toBe("test-oauth-server")
      expect(events[0].url).toContain("https://")
    },
  })
})

test("BrowserOpenFailed event is NOT published when open() succeeds", async () => {
  await using tmp = await tmpdir({
    init: async (dir) => {
      await Bun.write(
        `${dir}/gizzi.json`,
        JSON.stringify({
          $schema: "https://docs.gizziio.com/config.json",
          mcp: {
            "test-oauth-server-2": {
              type: "remote",
              url: "https://example.com/mcp",
            },
          },
        }),
      )
    },
  })

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      openShouldFail = false

      const events: Array<{ mcpName: string; url: string }> = []
      const unsubscribe = Bus.subscribe(MCP.BrowserOpenFailed, (evt) => {
        events.push(evt.properties)
      })

      // Run authenticate with a timeout to avoid waiting forever for the callback
      const authPromise = MCP.authenticate("test-oauth-server-2").catch(() => undefined)

      // Config.get() can be slow in tests; also covers the ~500ms open() error-detection window.
      await new Promise((resolve) => setTimeout(resolve, 2_000))

      // Stop the callback server and cancel any pending auth
      await McpOAuthCallback.stop()

      await authPromise

      unsubscribe()

      // Verify NO BrowserOpenFailed event was published
      expect(events.length).toBe(0)
      // Verify open() was still called
      expect(openCalledWith).toBeDefined()
    },
  })
})

test("open() is called with the authorization URL", async () => {
  await using tmp = await tmpdir({
    init: async (dir) => {
      await Bun.write(
        `${dir}/gizzi.json`,
        JSON.stringify({
          $schema: "https://docs.gizziio.com/config.json",
          mcp: {
            "test-oauth-server-3": {
              type: "remote",
              url: "https://example.com/mcp",
            },
          },
        }),
      )
    },
  })

  await Instance.provide({
    directory: tmp.path,
    fn: async () => {
      openShouldFail = false
      openCalledWith = undefined

      // Run authenticate with a timeout to avoid waiting forever for the callback
      const authPromise = MCP.authenticate("test-oauth-server-3").catch(() => undefined)

      // Config.get() can be slow in tests; also covers the ~500ms open() error-detection window.
      await new Promise((resolve) => setTimeout(resolve, 2_000))

      // Stop the callback server and cancel any pending auth
      await McpOAuthCallback.stop()

      await authPromise

      // Verify open was called with a URL
      expect(openCalledWith).toBeDefined()
      expect(typeof openCalledWith).toBe("string")
      expect(openCalledWith!).toContain("https://")
    },
  })
})
