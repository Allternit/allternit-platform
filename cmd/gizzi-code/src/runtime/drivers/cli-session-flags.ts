import type { AgentTask } from "@/runtime/runtime-driver"

/**
 * Claude Code flags that bring the gizzi session along: its instructions
 * (appended to Claude's own system prompt) and gizzi's session tools as an
 * MCP server (see CliBridge).
 */
export function claudeSessionFlags(ctx: {
  systemPrompt?: string
  mcp?: AgentTask["mcp"]
  vendorSessionId?: string
}): string[] {
  const flags: string[] = []
  // Continue the vendor's own conversation from the previous turn.
  if (ctx.vendorSessionId) flags.push("--resume", ctx.vendorSessionId)
  if (ctx.systemPrompt?.trim()) flags.push("--append-system-prompt", ctx.systemPrompt)
  if (ctx.mcp) {
    flags.push(
      "--mcp-config",
      JSON.stringify({ mcpServers: { [ctx.mcp.name]: { type: "http", url: ctx.mcp.url, headers: ctx.mcp.headers } } }),
    )
  }
  return flags
}

/** Codex app-server thread config that adds gizzi's session tools (see CliBridge). */
export function codexMcpConfig(mcp: AgentTask["mcp"]): Record<string, unknown> | undefined {
  if (!mcp) return undefined
  return {
    [`mcp_servers.${mcp.name}.url`]: mcp.url,
    [`mcp_servers.${mcp.name}.http_headers`]: mcp.headers,
  }
}

/** ACP session MCP servers: gizzi's bridge, when the agent speaks HTTP MCP. */
export function acpMcpServers(mcp: AgentTask["mcp"], agentCapabilities: unknown): unknown[] {
  const http = (agentCapabilities as { mcpCapabilities?: { http?: boolean } } | undefined)?.mcpCapabilities?.http
  if (!mcp || !http) return []
  return [
    {
      type: "http",
      name: mcp.name,
      url: mcp.url,
      headers: Object.entries(mcp.headers).map(([name, value]) => ({ name, value })),
    },
  ]
}

/** ACP has no system prompt: the session's instructions lead the first prompt. */
export function withInstructions(prompt: string, systemPrompt?: string): string {
  if (!systemPrompt?.trim()) return prompt
  return `<session_instructions>\n${systemPrompt.trim()}\n</session_instructions>\n\n${prompt}`
}

/** Claude stream-json events carry the vendor session id as `session_id` (system init and result). */
export function claudeSessionIdFromEvent(evt: unknown): string | undefined {
  const e = evt as { type?: string; session_id?: unknown } | null
  if (!e || (e.type !== "system" && e.type !== "result")) return undefined
  return typeof e.session_id === "string" && e.session_id ? e.session_id : undefined
}

/** ACP `session/load` is only available when the agent advertises `loadSession`. */
export function acpCanLoadSession(agentCapabilities: unknown): boolean {
  return (agentCapabilities as { loadSession?: boolean } | undefined)?.loadSession === true
}
