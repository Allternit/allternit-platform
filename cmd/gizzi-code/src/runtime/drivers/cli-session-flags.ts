import type { AgentTask } from "@/runtime/runtime-driver"
import { claudeBotFlags } from "@/runtime/bots/bot-turn"

/** The CLI bridge tool Claude Code's plan mode asks for permission (`--permission-prompt-tool`). */
export const CLI_PERMISSION_TOOL = "permission_prompt"

/**
 * Claude Code permission + isolation flags for a session turn. Normal
 * sessions keep the long-standing `bypassPermissions`; a bot turn gets
 * settings isolation and its own tool policy (see claudeBotFlags). A plan
 * turn runs Claude's own plan mode (read-only), and what that mode would ask
 * about goes to gizzi's bridge: the plan file may be edited, ExitPlanMode
 * becomes gizzi's "Build Agent" question, everything else is refused.
 */
export function claudePermissionFlags(ctx: { bot?: AgentTask["bot"]; mcp?: AgentTask["mcp"]; plan?: AgentTask["plan"] }): string[] {
  if (ctx.bot) return claudeBotFlags(ctx.bot, ctx.mcp?.name)
  if (ctx.plan) {
    return ["--permission-mode", "plan", ...(ctx.mcp ? ["--permission-prompt-tool", `mcp__${ctx.mcp.name}__${CLI_PERMISSION_TOOL}`] : [])]
  }
  return ["--permission-mode", "bypassPermissions"]
}

/** What a CLI needs to know in a plan turn: where the plan goes and how to hand it over. */
export function planInstructions(cli: string, plan: NonNullable<AgentTask["plan"]>): string {
  const handoff =
    cli === "claude-cli"
      ? "When the plan is ready, present it with ExitPlanMode."
      : "When the plan is ready, call the allternit plan_exit tool with the whole plan as Markdown."
  return [
    "Gizzi plan mode is on: research and plan, do not build. Take read-only actions only.",
    `${handoff} Gizzi saves it as the plan file (${plan.file}) and asks the user to approve it.`,
    "If they approve, end your turn: the session switches to the build agent, which carries the plan out.",
  ].join(" ")
}

/** Codex thread settings for a plan turn: a read-only sandbox, so the plan agent cannot change files. */
export function codexPlanParams(plan: AgentTask["plan"]): Record<string, unknown> {
  return plan ? { sandbox: "read-only" } : {}
}

/**
 * Claude Code flags that bring the gizzi session along: its instructions
 * (appended to Claude's own system prompt) and gizzi's session tools as an
 * MCP server (see CliBridge).
 */
export function claudeSessionFlags(ctx: {
  systemPrompt?: string
  mcp?: AgentTask["mcp"]
  vendorSessionId?: string
  bot?: AgentTask["bot"]
}): string[] {
  const flags: string[] = []
  // Continue the vendor's own conversation from the previous turn.
  if (ctx.vendorSessionId) flags.push("--resume", ctx.vendorSessionId)
  // A bot turn replaces Claude Code's own "coding CLI" prompt with the bot's
  // (persona first); other sessions append to it.
  if (ctx.systemPrompt?.trim()) flags.push(ctx.bot ? "--system-prompt" : "--append-system-prompt", ctx.systemPrompt)
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

/**
 * Vendor session id from one stream-json event of `cli`. Claude and Qwen share
 * the `session_id` shape; OpenCode stamps `sessionID` on every event.
 */
export function vendorSessionIdFromEvent(cli: string, evt: unknown): string | undefined {
  if (cli === "claude-cli" || cli === "qwen-cli") return claudeSessionIdFromEvent(evt)
  if (cli === "opencode") {
    const id = (evt as { sessionID?: unknown } | null)?.sessionID
    return typeof id === "string" && id ? id : undefined
  }
  return undefined
}

/** Qwen Code resumes with `--resume <id>` (same as Claude). */
export function qwenResumeFlags(vendorSessionId?: string): string[] {
  return vendorSessionId ? ["--resume", vendorSessionId] : []
}

/** OpenCode resumes with `run --session <id>`. */
export function opencodeResumeFlags(vendorSessionId?: string): string[] {
  return vendorSessionId ? ["--session", vendorSessionId] : []
}

/** Codex app-server: resume the prior thread (`thread/resume`) instead of `thread/start`. */
export function codexThreadRequest(
  vendorSessionId: string | undefined,
  startParams: Record<string, unknown>,
): { method: "thread/start" | "thread/resume"; params: Record<string, unknown> } {
  return vendorSessionId
    ? { method: "thread/resume", params: { ...startParams, threadId: vendorSessionId } }
    : { method: "thread/start", params: startParams }
}

/** ACP `session/load` is only available when the agent advertises `loadSession`. */
export function acpCanLoadSession(agentCapabilities: unknown): boolean {
  return (agentCapabilities as { loadSession?: boolean } | undefined)?.loadSession === true
}

/** Turn-only Claude flags that a `/context` read must not repeat (value-taking ones list 1). */
const CLAUDE_TURN_ONLY: Record<string, number> = {
  "--output-format": 1,
  "--input-format": 1,
  "--verbose": 0,
  "--include-partial-messages": 0,
  "--resume": 1,
  "--permission-prompt-tool": 1,
}

/**
 * Argv for Claude Code's own `/context` report on the turn's vendor session:
 * the turn's argv with the same model, system prompt, MCP config and
 * settings isolation, but a local slash command instead of a model call
 * (no tokens spent) and no write to the session's transcript.
 */
export function claudeContextArgv(turnArgv: string[], vendorSessionId: string): string[] {
  const out: string[] = []
  for (let i = 0; i < turnArgv.length; i++) {
    const skip = CLAUDE_TURN_ONLY[turnArgv[i]]
    if (skip !== undefined) {
      i += skip
      continue
    }
    out.push(turnArgv[i])
  }
  return [...out, "--output-format", "json", "--no-session-persistence", "--resume", vendorSessionId, "/context"]
}
