import { describe, expect, test } from "bun:test"
import { CliBridge } from "../../src/runtime/integrations/cli-bridge"
import {
  acpMcpServers,
  CLI_PERMISSION_TOOL,
  claudePermissionFlags,
  claudeSessionFlags,
  codexPlanParams,
  planInstructions,
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
    expect(names).toEqual([
      "pane_artifact",
      "pane_browser",
      "media_generate",
      "todowrite",
      "artifact_create",
      "artifact_update",
      "artifact_read",
      "artifact_list",
      "artifact_delete",
      "artifact_share",
      "artifact_comment",
      "artifact_storage",
      "question",
    ])
    const question = list.result.tools.find((t: any) => t.name === "question")
    expect(question.description).toContain("no AskUserQuestion")
    expect(question.inputSchema.properties.questions).toBeDefined()
    const media = list.result.tools.find((t: any) => t.name === "media_generate")
    expect(media.inputSchema.type).toBe("object")
    expect(media.inputSchema.properties.kind).toBeDefined()
    expect(media.inputSchema.$schema).toBeUndefined()
  })

  test("question shows the card and returns at once; bad input is an error", async () => {
    const call = (args: unknown) =>
      CliBridge.handle("ses_1", { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "question", arguments: args } }, signal) as Promise<any>
    const bad = await call({ questions: "nope" })
    expect(bad.result.isError).toBe(true)
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

describe("plan turns on a CLI", () => {
  const mcp = { name: "allternit", url: "http://127.0.0.1:4096/cli-bridge/s/mcp", headers: {} }
  const plan = { file: "/repo/.gizzi/plans/1-x.md" }

  test("Claude runs its own plan mode and asks gizzi's bridge about what that mode would prompt for", () => {
    expect(claudePermissionFlags({ mcp })).toEqual(["--permission-mode", "bypassPermissions"])
    expect(claudePermissionFlags({ mcp, plan })).toEqual([
      "--permission-mode",
      "plan",
      "--permission-prompt-tool",
      `mcp__allternit__${CLI_PERMISSION_TOOL}`,
    ])
    expect(claudePermissionFlags({ plan })).toEqual(["--permission-mode", "plan"])
  })

  test("Codex plans in a read-only sandbox; other turns keep their settings", () => {
    expect(codexPlanParams(plan)).toEqual({ sandbox: "read-only" })
    expect(codexPlanParams(undefined)).toEqual({})
    // A tools-off turn (internal completion) runs read-only too.
    expect(codexPlanParams(undefined, true)).toEqual({ sandbox: "read-only" })
  })

  test("the instructions name the plan file and the CLI's own way to hand the plan over", () => {
    expect(planInstructions("claude-cli", plan)).toContain("ExitPlanMode")
    expect(planInstructions("codex-cli", plan)).toContain("plan_exit")
    expect(planInstructions("codex-cli", plan)).toContain(plan.file)
  })

  test("outside a plan turn the bridge lists no plan tools", async () => {
    const list = (await CliBridge.handle("ses_none", { jsonrpc: "2.0", id: 9, method: "tools/list" }, signal)) as any
    const names = list.result.tools.map((t: any) => t.name)
    expect(names).not.toContain("plan_exit")
    expect(names).not.toContain(CLI_PERMISSION_TOOL)
  })
})

describe("acpErrorMessage", () => {
  test("a JSON-RPC error object reads as its message, not [object Object]", async () => {
    const { acpErrorMessage } = await import("../../src/runtime/drivers/local-cli-driver")
    expect(acpErrorMessage({ code: -32000, message: "Authentication required: 403 upgrade your plan" })).toBe(
      "Authentication required: 403 upgrade your plan",
    )
    expect(acpErrorMessage({ code: 1 })).toBe('{"code":1}')
  })
})
