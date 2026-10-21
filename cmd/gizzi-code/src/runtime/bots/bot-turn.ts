/**
 * Bot turns: a session turn that runs as one of the user's platform bots
 * (allternit-api marks it with `bot` on the /v1/session/:id/message body).
 *
 * A bot turn is the bot, not the coding agent and not the user's own Claude
 * Code / gizzi setup:
 *
 * - Identity: the bot's persona leads the system prompt and the "Allternit
 *   Shell … coding agent" header is replaced by neutral operating guidance
 *   (see `system-header.ts`).
 * - Instructions: no personal instruction files. The user's ~/.claude/CLAUDE.md,
 *   ~/.gizzi/* identity and legacy prompts, and CLAUDE.md / AGENTS.md /
 *   GIZZI.md found by walking up from the session folder are not loaded. The
 *   bot's own workspace files (SOUL.md, AGENTS.md… in its agent workspace)
 *   arrive in the persona text allternit-api composes.
 * - Skills: no personal skills (~/.claude/skills, ~/.agents/skills,
 *   ~/.openclaw/skills, project .claude/skills).
 * - Claude CLI brain: settings isolation (`--setting-sources ""`,
 *   `--disable-slash-commands`, `--strict-mcp-config`), the persona as the
 *   system prompt, and the bot's tool allowlist / approval gates instead of
 *   `bypassPermissions`.
 *
 * This module is dependency-light on purpose (zod only) so the policy can be
 * unit tested without the runtime.
 */
import os from "node:os"
import path from "node:path"
import z from "zod/v4"

export const BotTurnInfo = z.object({
  /** Platform agent id of the bot. */
  id: z.string().min(1),
  /** Display name (logs only). */
  name: z.string().optional(),
  /**
   * The bot's tool allowlist (`agents.allowed_tools`): platform tool ids
   * (web_search, file_read, …) or vendor tool names (Bash, mcp__x__y).
   * Empty/absent = no allowlist configured.
   */
  allowedTools: z.array(z.string()).optional(),
  /** Tools a person approves each time (`tool_permissions` = always_ask). */
  askTools: z.array(z.string()).optional(),
  /**
   * The bot has approval gates (template `config.approvals`): a tool outside
   * its allowlist goes to the app's approval flow instead of being refused.
   */
  gated: z.boolean().optional(),
})
export type BotTurnInfo = z.infer<typeof BotTurnInfo>

/* -------------------------------------------------------------------------- */
/* Per-session registry                                                       */
/* -------------------------------------------------------------------------- */

// A session is a bot's session for its whole life; the marker is sticky so
// synthetic follow-up turns (background task results, queued messages) stay
// bot turns. Lives in memory; `resolve` re-derives it from stored messages
// after a restart.
const active = new Map<string, BotTurnInfo>()

export function mark(sessionID: string, bot: BotTurnInfo | undefined): void {
  if (!sessionID || !bot) return
  active.set(sessionID, bot)
}

export function get(sessionID: string | undefined): BotTurnInfo | undefined {
  return sessionID ? active.get(sessionID) : undefined
}

/** Test hook. */
export function reset(): void {
  active.clear()
}

/**
 * The bot this session's turn runs as: the newest user message carrying a
 * `bot` marker, else the registry. Records the result in the registry.
 */
export function resolve(
  sessionID: string,
  messages: ReadonlyArray<{ info?: unknown }>,
): BotTurnInfo | undefined {
  for (let i = messages.length - 1; i >= 0; i--) {
    const info = messages[i]!.info as { role?: string; bot?: BotTurnInfo } | undefined
    if (info?.role === "user" && info.bot) {
      mark(sessionID, info.bot)
      return info.bot
    }
  }
  return get(sessionID)
}

/* -------------------------------------------------------------------------- */
/* Skills                                                                     */
/* -------------------------------------------------------------------------- */

/** Personal skill roots a bot never sees. */
function personalSkillRoots(home = os.homedir()): string[] {
  return [".claude", ".agents", ".openclaw", ".codex"].map((brand) => path.join(home, brand) + path.sep)
}

/**
 * Whether a bot turn may see this skill. Built-in skills, skills the
 * Allternit app manages (~/.allternit/skills) and gizzi config skills stay;
 * personal Claude/agents/openclaw/codex skills and project-folder skills
 * (found walking up from the session folder) do not.
 */
export function skillVisibleToBot(
  skill: { location: string; source?: string; builtin?: boolean },
  home = os.homedir(),
): boolean {
  if (skill.builtin) return true
  if (skill.source === "project") return false
  const location = path.resolve(skill.location)
  return !personalSkillRoots(home).some((root) => location.startsWith(root))
}

/* -------------------------------------------------------------------------- */
/* Claude CLI policy                                                          */
/* -------------------------------------------------------------------------- */

/** Platform tool ids (allternit-ai bot-tool-registry) → Claude Code tools. */
const CLAUDE_TOOLS: Record<string, string[]> = {
  web_search: ["WebSearch"],
  web_fetch: ["WebFetch"],
  file_read: ["Read", "Glob", "Grep"],
  file_write: ["Write", "Edit", "NotebookEdit"],
  str_replace_editor: ["Edit"],
  code_execution: ["Bash"],
  bash: ["Bash"],
  // Served by gizzi's own session tools over the CLI bridge (always allowed).
  memory: [],
  computer: [],
}

/** Claude Code tool names for a list of platform / vendor tool ids. */
export function claudeToolNames(ids: readonly string[] | undefined): string[] {
  const out = new Set<string>()
  for (const raw of ids ?? []) {
    const id = raw.trim()
    if (!id) continue
    const mapped = CLAUDE_TOOLS[id.toLowerCase()]
    if (mapped) mapped.forEach((name) => out.add(name))
    else out.add(id)
  }
  return [...out]
}

/** True when the bot's tools are restricted (allowlist or approval gates). */
export function restricted(bot: BotTurnInfo): boolean {
  return Boolean(bot.gated) || (bot.allowedTools?.length ?? 0) > 0 || (bot.askTools?.length ?? 0) > 0
}

/** Tools the CLI may run without asking: the allowlist minus ask-each-time tools, plus gizzi's bridge. */
export function claudePreapproved(bot: BotTurnInfo, bridgeName?: string): string[] {
  const ask = new Set(claudeToolNames(bot.askTools))
  const allowed = claudeToolNames(bot.allowedTools).filter((name) => !ask.has(name))
  if (bridgeName) allowed.push(`mcp__${bridgeName}`)
  return [...new Set(allowed)]
}

/**
 * Claude Code flags for a bot turn.
 *
 * Isolation: `--setting-sources ""` loads no user/project/local settings —
 * that is also what makes Claude Code skip ~/.claude/CLAUDE.md and every
 * project CLAUDE.md (verified with Claude Code 2.1.x). Auth is unaffected:
 * OAuth login lives in the keychain / ~/.claude.json, which setting sources
 * do not gate, so no separate CLAUDE_CONFIG_DIR is used (on macOS a different
 * CLAUDE_CONFIG_DIR changes the keychain entry name and would log the bot
 * out). `--disable-slash-commands` drops personal skills; `--strict-mcp-config`
 * keeps the user's own MCP servers out (only gizzi's bridge loads).
 *
 * Permissions: a bot with no allowlist and no gates keeps
 * `bypassPermissions` (unchanged behavior). A restricted bot runs in
 * `default` mode with its allowlist pre-approved and every other permission
 * request sent to gizzi over stdio (`--permission-prompt-tool stdio`), where
 * `claudeToolDecision` refuses it, or routes it to the app's approval flow
 * when the tool is ask-each-time or the bot has approval gates.
 */
export function claudeBotFlags(bot: BotTurnInfo, bridgeName?: string): string[] {
  const flags = ["--setting-sources", "", "--disable-slash-commands"]
  if (bridgeName) flags.push("--strict-mcp-config")
  if (!restricted(bot)) {
    flags.push("--permission-mode", "bypassPermissions")
    return flags
  }
  flags.push("--permission-mode", "default", "--permission-prompt-tool", "stdio")
  const preapproved = claudePreapproved(bot, bridgeName)
  if (preapproved.length > 0) flags.push("--allowedTools", preapproved.join(","))
  return flags
}

export type ToolDecision = "allow" | "ask" | "deny"

/**
 * What gizzi answers when a restricted bot's Claude CLI asks to use a tool
 * that was not pre-approved: allowlisted → allow; ask-each-time, or any
 * other tool on a bot with approval gates → ask a person (the app's
 * approval flow); anything else → deny with a clear reason.
 */
export function claudeToolDecision(bot: BotTurnInfo, toolName: string, bridgeName?: string): ToolDecision {
  if (claudeToolNames(bot.askTools).includes(toolName)) return "ask"
  const allowed = claudePreapproved(bot, bridgeName)
  if (allowed.includes(toolName)) return "allow"
  if (allowed.some((name) => name.startsWith("mcp__") && toolName.startsWith(name + "__"))) return "allow"
  return bot.gated ? "ask" : "deny"
}

export function deniedMessage(bot: BotTurnInfo, toolName: string): string {
  return `${toolName} is not in ${bot.name ? `${bot.name}'s` : "this bot's"} allowed tools. Tell the user it needs to be added in the bot's tool settings.`
}
