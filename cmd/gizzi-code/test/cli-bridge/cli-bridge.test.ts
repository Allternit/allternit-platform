import { describe, expect, test } from "bun:test"
import { CliBridge } from "../../src/runtime/integrations/cli-bridge"
import {
  acpMcpServers,
  claudeSessionFlags,
  codexMcpConfig,
  withInstructions,
} from "../../src/runtime/drivers/cli-session-flags"
import { extractSystemText } from "../../src/runtime/providers/adapters/loaders/system-text"

const signal = new AbortController().signal

describe("CliBridge MCP server", () => {
  test("initialize, then list gizzi's session tools with JSON schemas", async () => {
    const init = (await CliBridge.handle("ses_1", { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } }, signal)) as any
    expect(init.result.serverInfo.name).toBe("allternit")
    expect(init.result.capabilities.tools).toBeDefined()

    const list = (await CliBridge.handle("ses_1", { jsonrpc: "2.0", id: 2, method: "tools/list" }, signal)) as any
    const names = list.result.tools.map((t: any) => t.name)
    expect(names).toEqual(["pane_artifact", "pane_browser", "media_generate", "artifact_create", "artifact_update", "artifact_read"])
    const media = list.result.tools.find((t: any) => t.name === "media_generate")
    expect(media.inputSchema.type).toBe("object")
    expect(media.inputSchema.properties.kind).toBeDefined()
    expect(media.inputSchema.$schema).toBeUndefined()
  })

  test("notifications get no reply; unknown methods get a JSON-RPC error", async () => {
    expect(await CliBridge.handle("ses_1", { jsonrpc: "2.0", method: "notifications/initialized" }, signal)).toBeNull()
    const bad = (await CliBridge.handle("ses_1", { jsonrpc: "2.0", id: 3, method: "resources/list" }, signal)) as any
    expect(bad.error.code).toBe(-32601)
  })

  test("the token is required and compared exactly", () => {
    const { headers } = CliBridge.serverConfig({ baseURL: "http://127.0.0.1:4096", sessionID: "ses_1" })
    expect(CliBridge.authorized(headers[CliBridge.TOKEN_HEADER])).toBe(true)
    expect(CliBridge.authorized(undefined)).toBe(false)
    expect(CliBridge.authorized("nope")).toBe(false)
    expect(headers.Authorization ?? "").not.toStartWith("Bearer") // never a Bearer the auth middleware would misread
  })

  test("a password-protected server gets basic auth alongside the bridge token", () => {
    const saved = { p: process.env.GIZZI_SERVER_PASSWORD, u: process.env.GIZZI_SERVER_USERNAME }
    try {
      delete process.env.GIZZI_SERVER_PASSWORD
      expect(CliBridge.serverConfig({ baseURL: "http://127.0.0.1:4096", sessionID: "s" }).headers.Authorization).toBeUndefined()
      process.env.GIZZI_SERVER_PASSWORD = "pw"
      process.env.GIZZI_SERVER_USERNAME = "gizzi"
      const { headers } = CliBridge.serverConfig({ baseURL: "http://127.0.0.1:4096", sessionID: "s" })
      expect(headers.Authorization).toBe(`Basic ${Buffer.from("gizzi:pw").toString("base64")}`)
      expect(headers[CliBridge.TOKEN_HEADER]).toBeDefined()
    } finally {
      if (saved.p === undefined) delete process.env.GIZZI_SERVER_PASSWORD
      else process.env.GIZZI_SERVER_PASSWORD = saved.p
      if (saved.u === undefined) delete process.env.GIZZI_SERVER_USERNAME
      else process.env.GIZZI_SERVER_USERNAME = saved.u
    }
  })

  test("the config points at this session on this server, in its project directory", () => {
    const cfg = CliBridge.serverConfig({ baseURL: "http://127.0.0.1:4096", sessionID: "ses_A b", directory: "/Users/x/proj" })
    const url = new URL(cfg.url)
    expect(url.pathname).toBe("/cli-bridge/ses_A%20b/mcp")
    expect(url.searchParams.get("directory")).toBe("/Users/x/proj")
  })
})

describe("handing the bridge to each kind of CLI", () => {
  const mcp = { name: "allternit", url: "http://127.0.0.1:4096/cli-bridge/ses_1/mcp", headers: { "X-Allternit-Bridge-Token": "t" } }

  test("Claude Code: appended system prompt and an HTTP MCP config", () => {
    const flags = claudeSessionFlags({ systemPrompt: "Edit the doc with pane_artifact.", mcp })
    expect(flags.slice(0, 2)).toEqual(["--append-system-prompt", "Edit the doc with pane_artifact."])
    const config = JSON.parse(flags[flags.indexOf("--mcp-config") + 1])
    expect(config.mcpServers.allternit).toEqual({ type: "http", url: mcp.url, headers: mcp.headers })
    expect(claudeSessionFlags({})).toEqual([])
  })

  test("Codex app-server: mcp_servers thread config", () => {
    expect(codexMcpConfig(mcp)).toEqual({
      "mcp_servers.allternit.url": mcp.url,
      "mcp_servers.allternit.http_headers": mcp.headers,
    })
    expect(codexMcpConfig(undefined)).toBeUndefined()
  })

  test("ACP: only to agents that speak HTTP MCP; instructions lead the prompt", () => {
    expect(acpMcpServers(mcp, { mcpCapabilities: { http: true } })).toEqual([
      { type: "http", name: "allternit", url: mcp.url, headers: [{ name: "X-Allternit-Bridge-Token", value: "t" }] },
    ])
    expect(acpMcpServers(mcp, { mcpCapabilities: { http: false } })).toEqual([])
    expect(acpMcpServers(mcp, undefined)).toEqual([])
    expect(withInstructions("hi", "Be brief.")).toBe("<session_instructions>\nBe brief.\n</session_instructions>\n\nhi")
    expect(withInstructions("hi")).toBe("hi")
  })

  test("the session's system messages become the CLI's instructions", () => {
    expect(
      extractSystemText([
        { role: "system", content: "Mode: Docs." },
        { role: "user", content: "hi" },
        { role: "system", content: [{ type: "text", text: "Artifact session." }] },
      ]),
    ).toBe("Mode: Docs.\n\nArtifact session.")
  })
})
