import type { AgentTask } from "@/runtime/runtime-driver"
import { claudeBotFlags, claudeDisallowed } from "@/runtime/bots/bot-turn"
import { getRuntime, listRuntimes, type SystemLayerVia } from "@/runtime/providers/discovery/runtime-registry"

/**
 * MCP tool-call timeout for gizzi's bridge, seconds. A long run_code (first
 * run installs Python, 60-120 s; runs up to 120 s) must not be cut off.
 * Claude Code's per-server `timeout` (ms) and Codex's per-server
 * `tool_timeout_sec`, both read from the CLIs' own binaries.
 */
export const CLI_BRIDGE_TOOL_TIMEOUT_SEC = 180

/** The CLI bridge tool Claude Code's plan mode asks for permission (`--permission-prompt-tool`). */
export const CLI_PERMISSION_TOOL = "permission_prompt"

const CLAUDE_PERMISSION_MODES = new Set(["default", "acceptEdits", "plan", "bypassPermissions"])

/**
 * Claude Code permission + isolation flags for a session turn. Normal
 * sessions keep the long-standing `bypassPermissions`; a bot turn gets
 * settings isolation and its own tool policy (see claudeBotFlags). A plan
 * turn runs Claude's own plan mode (read-only), and what that mode would ask
 * about goes to gizzi's bridge: the plan file may be edited, ExitPlanMode
 * becomes gizzi's "Build Agent" question, everything else is refused. A
 * caller that approves tools itself (the Gizzi TUI) gets Claude's own mode
 * with every approval sent to it over stdio.
 */
export function claudePermissionFlags(ctx: {
  bot?: AgentTask["bot"]
  mcp?: AgentTask["mcp"]
  plan?: AgentTask["plan"]
  /** The caller approves tools itself (AgentTask.approveTool) over the stdio control channel. */
  approval?: { mode: string }
}): string[] {
  if (ctx.bot) return claudeBotFlags(ctx.bot, ctx.mcp?.name)
  if (ctx.plan) {
    return ["--permission-mode", "plan", ...(ctx.mcp ? ["--permission-prompt-tool", `mcp__${ctx.mcp.name}__${CLI_PERMISSION_TOOL}`] : [])]
  }
  if (ctx.approval) {
    const mode = CLAUDE_PERMISSION_MODES.has(ctx.approval.mode) ? ctx.approval.mode : "default"
    return ["--permission-mode", mode, "--permission-prompt-tool", "stdio"]
  }
  return ["--permission-mode", "bypassPermissions"]
}

/**
 * Claude Code's `--disallowedTools` for a session turn: AskUserQuestion
 * (it never reaches the user) and the tools the account blocks, from the bot
 * marker or, for a normal session, the list pushed with the permission mode
 * (BlockedTools). One flag, so the CLI never has to merge two.
 */
export function claudeDisallowedTools(ctx: { bot?: AgentTask["bot"]; blockedTools?: AgentTask["blockedTools"] }): string[] {
  return [...new Set(["AskUserQuestion", ...claudeDisallowed(ctx.bot?.blockedTools), ...claudeDisallowed(ctx.blockedTools)])]
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

/**
 * Codex thread settings for a plan turn or a tools-off turn: a read-only
 * sandbox, so the agent cannot change files (Codex can't drop its shell).
 */
export function codexPlanParams(plan: AgentTask["plan"], toolsOff?: boolean): Record<string, unknown> {
  return plan || toolsOff ? { sandbox: "read-only" } : {}
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
  // Allternit's layer (a bot's persona included) is appended: Claude Code's own
  // system prompt stays (Harness & Bots 2.0 D10). Bot policy is enforced by
  // the tool flags and gizzi's broker, not by the prompt.
  if (ctx.systemPrompt?.trim()) flags.push("--append-system-prompt", ctx.systemPrompt)
  if (ctx.mcp) {
    flags.push(
      "--mcp-config",
      JSON.stringify({ mcpServers: { [ctx.mcp.name]: { type: "http", url: ctx.mcp.url, headers: ctx.mcp.headers, timeout: CLI_BRIDGE_TOOL_TIMEOUT_SEC * 1000 } } }),
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
    [`mcp_servers.${mcp.name}.tool_timeout_sec`]: CLI_BRIDGE_TOOL_TIMEOUT_SEC,
  }
}

/**
 * Codex's developer instructions for a session turn. Codex shows MCP tools to
 * the model only through its exec tool, so "use your question tool" made it
 * pick its own request_user_input, which never reaches the user (it returns
 * at once and the model sleeps waiting for an answer). With gizzi's bridge
 * attached, name the bridge's question tool outright.
 */
export function codexDeveloperInstructions(systemPrompt: string | undefined, mcp: AgentTask["mcp"]): string | undefined {
  if (!mcp) return systemPrompt
  const ask =
    `To ask the user anything, call the ${mcp.name} MCP server's question tool (mcp__${mcp.name}__question; through exec, tools.mcp__${mcp.name}__question) and then end your turn: their answers arrive as their next message. ` +
    "Never use request_user_input or request_user_input_async: the user can't see or answer those."
  return systemPrompt?.trim() ? `${systemPrompt}\n\n${ask}` : ask
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

/**
 * How a CLI's runtime adds Allternit's layer, from its registry entry
 * (cli-catalog.json `runtimes[].systemLayer.via`, matched by `tool`, then by
 * id or alias). A CLI the registry does not list (a user's own tool) gets
 * "agents-md", which today means the layer leads the message.
 */
export function systemLayerVia(cliName: string): SystemLayerVia {
  const runtime = listRuntimes().find((r) => r.tool === cliName) ?? getRuntime(cliName)
  return runtime?.systemLayer?.via ?? "agents-md"
}

/**
 * Where the session's instructions go on this turn. The driver only ever
 * appends: the runtime's own system prompt stays.
 * - "channel": the CLI's own append channel. `append-flag` is Claude Code's
 *   `--append-system-prompt` (stream-json) or Codex's developer instructions
 *   (codex-app-server, added on top of Codex's base instructions);
 *   `acp-session` is the first prompt of the ACP turn (runAcp leads it with
 *   the instructions; ACP has no system prompt field).
 * - "message": the instructions lead the message (withInstructions).
 *   `agents-md` lands here because gizzi has no AGENTS.md writer, `plugin`
 *   because no runtime has a plugin hook yet, and so does any turn whose
 *   adapter has no channel (agy's `--print` fallback, a mismatched entry).
 * - "none": no layer.
 */
export function systemLayerPath(cliName: string, mode: string): "channel" | "message" | "none" {
  const via = systemLayerVia(cliName)
  if (via === "none") return "none"
  if (via === "append-flag" && ((cliName === "claude-cli" && mode === "stream-json") || mode === "codex-app-server")) return "channel"
  if (via === "acp-session" && mode === "acp") return "channel"
  return "message"
}

/**
 * The message a CLI turn sends. A CLI with no append channel (Gemini,
 * Antigravity's print fallback, Qwen, Cursor, OpenCode, text CLIs…) gets the
 * session's instructions leading it, the way ACP does, so every brain gets
 * the same instructions (answer shapes, mode contract) instead of only the
 * last message.
 */
export function cliTurnMessage(prompt: string, systemPrompt: string | undefined, cliName: string, mode: string): string {
  return systemLayerPath(cliName, mode) === "message" ? withInstructions(prompt, systemPrompt) : prompt
}

/**
 * The instructions a CLI's own append channel carries this turn (Claude's
 * flag, Codex's developer instructions), so a layer never goes in twice.
 */
export function channelSystemPrompt(systemPrompt: string | undefined, cliName: string, mode: string): string | undefined {
  return systemLayerPath(cliName, mode) === "channel" ? systemPrompt : undefined
}

/**
 * Whether a CLI's tool calls reach gizzi's tool broker: the caller's
 * AgentTask.approveTool (the Gizzi TUI), a bot's tool policy, and for ACP the
 * Allternit spawn gate. The broker is enforced outside the runtime; a prompt
 * is not a boundary.
 * - Claude Code (stream-json): `--permission-prompt-tool stdio`, every
 *   can_use_tool request goes to the broker.
 * - Codex (codex-app-server): exec and file-change approval requests go to
 *   the broker (approval policy on-request while a caller approves).
 * - ACP: `session/request_permission` goes to the gate, then approveTool.
 *   CLIs gizzi starts with their own auto-approve flag (Grok
 *   `--always-approve`, Qoder and Trae `--yolo`) never ask, so they are not
 *   brokered.
 * - No permission channel: the other stream-json CLIs (started with their
 *   own skip-permissions flag), OpenClaw, one-shot JSON (Pi, OMP) and text
 *   CLIs (and agy's print fallback). These run under the session's file and
 *   shell sandbox only.
 */
export function toolCallsBrokered(cliName: string, mode: string): boolean {
  if (mode === "codex-app-server") return true
  if (mode === "stream-json") return cliName === "claude-cli"
  if (mode === "acp") return !ACP_SELF_APPROVING.has(cliName)
  return false
}

/** ACP CLIs gizzi starts with their own auto-approve flag (see CLI_ADAPTERS). */
const ACP_SELF_APPROVING = new Set(["grok", "qodercli", "qoderclicn", "traecli"])

/**
 * Codex thread settings while a caller approves tools itself
 * (AgentTask.approveTool): Codex asks before exec and file changes
 * (`on-request`), unless the caller's mode is bypassPermissions. No caller:
 * Codex keeps its own configured policy.
 */
export function codexApprovalParams(approval: { mode: string } | undefined): Record<string, unknown> {
  if (!approval) return {}
  return { approvalPolicy: approval.mode === "bypassPermissions" ? "never" : "on-request" }
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
  if (cli === "claude-cli" || cli === "qwen-cli" || cli === "codebuddy") return claudeSessionIdFromEvent(evt)
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
