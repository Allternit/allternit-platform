/** The bridge's MCP tool timeout reaches each vendor CLI (a long run_code must not be cut off). */
import { describe, expect, test } from "bun:test"
import { claudeSessionFlags, codexMcpConfig, CLI_BRIDGE_TOOL_TIMEOUT_SEC } from "@/runtime/drivers/cli-session-flags"

const mcp = { name: "allternit", url: "http://127.0.0.1:1/mcp", headers: { Authorization: "Bearer x" } } as any

describe("bridge tool timeout", () => {
  test("is at least 180 s", () => expect(CLI_BRIDGE_TOOL_TIMEOUT_SEC).toBeGreaterThanOrEqual(180))

  test("Claude Code: per-server timeout in ms on the bridge only", () => {
    const flags = claudeSessionFlags({ mcp })
    const cfg = JSON.parse(flags[flags.indexOf("--mcp-config") + 1])
    expect(Object.keys(cfg.mcpServers)).toEqual(["allternit"])
    expect(cfg.mcpServers.allternit.timeout).toBe(CLI_BRIDGE_TOOL_TIMEOUT_SEC * 1000)
  })

  test("Codex: tool_timeout_sec on the bridge server", () => {
    const cfg = codexMcpConfig(mcp)!
    expect(cfg["mcp_servers.allternit.tool_timeout_sec"]).toBe(CLI_BRIDGE_TOOL_TIMEOUT_SEC)
  })
})
