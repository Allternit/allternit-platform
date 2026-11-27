import { describe, expect, test } from "bun:test"
import { ScriptSandbox } from "@/runtime/services/mcp/script-sandbox"
import { McpScriptShared as McpScript } from "@/runtime/tools/builtins/mcp-script-shared"
import { MCPTool } from "@/cli/ui/ink-app/tools/MCPTool/MCPTool"
import { bridgeMcpTools, shouldOfferMcpScript } from "@/cli/ui/ink-app/tools/MCPScriptTool/bridge"
import type { Tool, ToolUseContext } from "@/cli/ui/ink-app/Tool"
import { clearRegisteredHooks, registerHookCallbacks } from "@/cli/ui/ink-app/bootstrap/state"

// Code Mode in the terminal: every bridged call goes through canUseTool, the
// same permission function the ink-app tool loop uses (the real one is the
// permission dialog; here it denies one tool the way the user would).
const ran: string[] = []

function mcpTool(server: string, name: string, value: (args: any) => unknown): Tool {
  return {
    ...MCPTool,
    name: `mcp__${server}__${name}`,
    isMcp: true,
    mcpInfo: { serverName: server, toolName: name },
    async call(args: any) {
      ran.push(`${server}.${name}`)
      return { data: [{ type: "text", text: JSON.stringify(value(args)) }] }
    },
  } as unknown as Tool
}

const tools = [
  mcpTool("github", "list_issues", () => [{ id: 1 }, { id: 2 }]),
  mcpTool("linear", "create_issue", (args) => ({ created: args.title })),
]

const denyLinear = (async (tool: Tool, input: Record<string, unknown>) =>
  tool.mcpInfo?.serverName === "linear"
    ? { behavior: "deny", message: "The user rejected permission to use this specific tool call.", decisionReason: { type: "other", reason: "test" } }
    : { behavior: "allow", updatedInput: input }) as any

const parentMessage = { message: { content: [] } } as any
const context = {
  abortController: new AbortController(),
  options: { tools, mcpClients: [] },
  getAppState: () => ({ toolPermissionContext: { mode: "default" }, sessionHooks: new Map() }),
} as unknown as ToolUseContext
const allowAll = (async (_tool: Tool, input: Record<string, unknown>) => ({ behavior: "allow", updatedInput: input })) as any

describe("mcp_script in the terminal", () => {
  test("a denied tool rejects inside the script and is never called", async () => {
    ran.length = 0
    const r = await ScriptSandbox.run({
      js: McpScript.toJavaScript(`
        const issues: Array<{ id: number }> = await github.list_issues({})
        try { await linear.create_issue({ title: "copy" }); return "ran" }
        catch (e) { return "denied after " + issues.length + ": " + e.message }
      `),
      bridges: bridgeMcpTools({ tools, context, canUseTool: denyLinear, parentMessage, toolUseID: "toolu_1" }),
    })
    expect(r.ok).toBe(true)
    expect((r.result)).toBe("denied after 2: The user rejected permission to use this specific tool call.")
    expect(ran).toEqual(["github.list_issues"])
    expect(r.calls.map((c) => [c.server, c.tool, c.ok])).toEqual([
      ["github", "list_issues", true],
      ["linear", "create_issue", false],
    ])
  })

  test("offered only with 2+ MCP tools and the flag on", () => {
    expect(shouldOfferMcpScript(tools)).toBe(true)
    expect(shouldOfferMcpScript(tools.slice(0, 1))).toBe(false)
    process.env.GIZZI_DISABLE_MCP_CODE_MODE = "1"
    try {
      expect(shouldOfferMcpScript(tools)).toBe(false)
    } finally {
      delete process.env.GIZZI_DISABLE_MCP_CODE_MODE
    }
  })

  test("a denying PreToolUse hook makes the bridged call reject", async () => {
    ran.length = 0
    // A user hook (registered the way SDK/plugin hooks are) that blocks linear.create_issue.
    registerHookCallbacks({
      PreToolUse: [
        {
          matcher: "mcp__linear__create_issue",
          hooks: [
            {
              type: "callback",
              callback: async () => ({
                reason: "blocked by policy",
                hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny" },
              }),
            },
          ],
        },
      ],
    } as any)
    try {
      const r = await ScriptSandbox.run({
        js: McpScript.toJavaScript(`
          await github.list_issues({})
          try { await linear.create_issue({ title: "copy" }); return "ran" }
          catch (e) { return "denied" }
        `),
        // Permission would allow everything: only the hook can block.
        bridges: bridgeMcpTools({ tools, context, canUseTool: allowAll, parentMessage, toolUseID: "toolu_2" }),
      })
      expect(r.ok).toBe(true)
      expect((r.result)).toBe("denied")
      expect(ran).toEqual(["github.list_issues"])
      expect(r.calls.map((c) => c.ok)).toEqual([true, false])
    } finally {
      clearRegisteredHooks()
    }
  })
})
