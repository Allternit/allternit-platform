/**
 * System prompt assembly order, kept apart from the runtime so the order is
 * unit tested. `composeTurnSystem` builds a turn's instruction list
 * (SessionPrompt loop); `headerParts` puts the base header around it
 * (LLM.stream).
 *
 * Normal sessions (Code mode, gizzi CLI, chat without a bot): unchanged —
 * provider/agent header first ("You are Allternit Shell…"), then environment,
 * instruction files, workspace identity, the live repo map, MCP server instructions, bot-chat
 * persona, scratchpad, and the caller's "+" appended instructions last.
 *
 * Bot turns (see runtime/bots/bot-turn.ts): the bot's persona (the caller's
 * "+" text, composed by allternit-api) leads, no instruction files or
 * workspace identity are loaded, and the coding-agent identity header is
 * replaced by neutral operating guidance after the persona.
 */
import type { BotTurnInfo } from "@/runtime/bots/bot-turn"

type Lazy = () => Promise<string[]> | string[]

export interface TurnSystemInput {
  /** The last user message's `system` ("+…" appends, anything else replaces). */
  userSystem?: string
  bot?: BotTurnInfo
  environment: Lazy
  /** Instruction files (InstructionPrompt.system). Never called for bot turns. */
  instructions: Lazy
  /** Workspace identity (.gizzi / ~/.gizzi). Never called for bot turns. */
  workspace: Lazy
  /** Live repo map block (codemap/repo-map.ts RepoMap.prompt). Never called for bot turns. */
  repoMap?: Lazy
  /** Connected MCP servers' own instructions (MCP.instructionsPrompt). Never called for bot turns. */
  mcpInstructions?: Lazy
  /** Local gizzi bot canonical-chat persona, if any. */
  botChat?: string
  scratchpad?: string
}

export async function composeTurnSystem(input: TurnSystemInput): Promise<string[]> {
  if (input.userSystem && !input.userSystem.startsWith("+")) return [input.userSystem]
  const appended = input.userSystem?.startsWith("+") ? [input.userSystem.slice(1)] : []
  if (input.bot) {
    return [
      ...appended,
      ...(input.botChat ? [input.botChat] : []),
      ...(await input.environment()),
      ...(input.scratchpad ? [input.scratchpad] : []),
    ].filter(Boolean)
  }
  return [
    ...(await input.environment()),
    ...(await input.instructions()),
    ...(await input.workspace()),
    ...(input.repoMap ? await input.repoMap() : []),
    ...(input.mcpInstructions ? await input.mcpInstructions() : []),
    ...(input.botChat ? [input.botChat] : []),
    ...(input.scratchpad ? [input.scratchpad] : []),
    ...appended,
  ].filter(Boolean)
}

export interface HeaderInput {
  bot?: BotTurnInfo
  /** The gizzi agent's own prompt (custom agents); replaces the provider header. */
  agentPrompt?: string
  /** Codex OAuth sends its header through `options.instructions` instead. */
  codex?: boolean
  /** Provider header prompts (SystemPrompt.provider). */
  provider: () => string[]
  /** Bot operating guidance (SystemPrompt.bot). */
  botGuidance: () => string[]
  /** The turn's composed instructions (composeTurnSystem). */
  system: string[]
}

export function headerParts(input: HeaderInput): string[] {
  if (input.bot) {
    // The agent's own prompt (title, compaction, custom agents) still
    // replaces the generic header, as it does for normal sessions.
    return [...input.system, ...(input.agentPrompt ? [input.agentPrompt] : input.botGuidance())].filter((x) => x)
  }
  return [
    ...(input.agentPrompt ? [input.agentPrompt] : input.codex ? [] : input.provider()),
    ...input.system,
  ].filter((x) => x)
}
