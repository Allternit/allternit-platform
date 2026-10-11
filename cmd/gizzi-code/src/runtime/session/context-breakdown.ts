import type { ModelMessage } from "ai"
import z from "zod/v4"
import { Token } from "@/shared/util/token"

/**
 * What fills a session's context window, by category — the data behind the
 * "Context window" panel (gizzi `/context`, Desktop/web composer popover).
 *
 * LLM.stream records a snapshot for every request it sends, from the exact
 * system blocks, tool schemas and projected messages. Counts are estimates
 * (Token.estimate) until the provider reports the turn's real input size;
 * `reconcile` then attributes the difference to Messages, the only category
 * we cannot measure exactly up front.
 */
export namespace ContextBreakdown {
  export const CategoryID = z.enum([
    "messages",
    "system_tools",
    "memory_files",
    "skills",
    "system_prompt",
    "mcp_tools",
    "mcp_instructions",
    "autocompact_buffer",
    "free_space",
  ])
  export type CategoryID = z.infer<typeof CategoryID>

  export const Category = z.object({
    id: CategoryID,
    name: z.string(),
    tokens: z.number(),
  })
  export type Category = z.infer<typeof Category>

  export const Item = z.object({
    name: z.string(),
    tokens: z.number(),
  })

  export const Snapshot = z
    .object({
      sessionID: z.string(),
      messageID: z.string().optional(),
      providerID: z.string(),
      modelID: z.string(),
      /** Model context window in tokens. */
      contextWindow: z.number(),
      /** Tokens in context (every category except the buffer and free space). */
      usedTokens: z.number(),
      /** usedTokens / contextWindow, 0–1. */
      utilization: z.number(),
      basis: z.enum(["provider", "estimated"]),
      autoCompactEnabled: z.boolean(),
      /** Tokens auto-compaction keeps free (configured size, before clamping to what is left). */
      autoCompactBuffer: z.number(),
      /** Categories in display order, including the buffer and free space. */
      categories: Category.array(),
      memoryFiles: z.object({ path: z.string(), tokens: z.number() }).array(),
      mcpTools: z.object({ name: z.string(), server: z.string(), tokens: z.number() }).array(),
      systemTools: Item.array(),
      measuredAt: z.number(),
    })
    .meta({ ref: "SessionContextBreakdown" })
  export type Snapshot = z.infer<typeof Snapshot>

  const NAMES: Record<CategoryID, string> = {
    messages: "Messages",
    system_tools: "System tools",
    memory_files: "Memory files",
    skills: "Skills",
    system_prompt: "System prompt",
    mcp_tools: "MCP tools",
    mcp_instructions: "MCP server instructions",
    autocompact_buffer: "Autocompact buffer",
    free_space: "Free space",
  }

  /** Instruction blocks as InstructionPrompt.system() renders them. */
  const MEMORY_HEADER = /^(?:Instructions from|Auto-memory from): (.+)$/m
  /** MCP.INSTRUCTIONS_HEADING (not imported: the MCP module pulls in the whole runtime). */
  const MCP_INSTRUCTIONS_HEADING = "# MCP Server Instructions"
  const MCP_PREFIX = "mcp__"
  const SKILL_TOOL = "skill"

  function estimate(value: unknown) {
    try {
      return Token.estimate(typeof value === "string" ? value : JSON.stringify(value))
    } catch {
      return 0
    }
  }

  /** `mcp__server__tool` → `server` (server names never contain `__`). */
  function mcpServer(name: string) {
    const rest = name.slice(MCP_PREFIX.length)
    const cut = rest.indexOf("__")
    return cut === -1 ? rest : rest.slice(0, cut)
  }

  export interface BuildInput {
    sessionID: string
    messageID?: string
    providerID: string
    modelID: string
    contextWindow: number
    /** Every system block sent, joined header included. */
    system: string[]
    /** The turn's composed instruction blocks (one block per memory file). */
    instructions: string[]
    tools: Record<string, unknown>
    messages: ModelMessage[]
    autoCompactBuffer: number
    now?: number
  }

  export function build(input: BuildInput): Snapshot {
    const memoryFiles = input.instructions.flatMap((block) => {
      const match = block.match(MEMORY_HEADER)
      if (!match || match.index !== 0) return []
      return [{ path: match[1].trim(), tokens: estimate(block) }]
    })
    const memoryTokens = memoryFiles.reduce((sum, file) => sum + file.tokens, 0)
    const mcpInstructionTokens = input.instructions
      .filter((block) => block.startsWith(MCP_INSTRUCTIONS_HEADING))
      .reduce((sum, block) => sum + estimate(block), 0)
    const systemTokens = input.system.reduce((sum, block) => sum + estimate(block), 0)

    const mcpTools: Snapshot["mcpTools"] = []
    const systemTools: Snapshot["systemTools"] = []
    let skillTokens = 0
    for (const [name, schema] of Object.entries(input.tools)) {
      if (name === "invalid") continue
      const tokens = estimate(schema)
      if (name.startsWith(MCP_PREFIX)) mcpTools.push({ name, server: mcpServer(name), tokens })
      else if (name === SKILL_TOOL) skillTokens += tokens
      else systemTools.push({ name, tokens })
    }
    const byTokens = (a: { tokens: number }, b: { tokens: number }) => b.tokens - a.tokens

    return finish({
      sessionID: input.sessionID,
      messageID: input.messageID,
      providerID: input.providerID,
      modelID: input.modelID,
      contextWindow: input.contextWindow,
      basis: "estimated",
      autoCompactEnabled: input.autoCompactBuffer > 0,
      autoCompactBuffer: input.autoCompactBuffer,
      memoryFiles: memoryFiles.sort(byTokens),
      mcpTools: mcpTools.sort(byTokens),
      systemTools: systemTools.sort(byTokens),
      measuredAt: input.now ?? Date.now(),
      parts: {
        messages: input.messages.reduce((sum, message) => sum + estimate(message), 0),
        system_tools: systemTools.reduce((sum, item) => sum + item.tokens, 0),
        memory_files: memoryTokens,
        skills: skillTokens,
        system_prompt: Math.max(0, systemTokens - memoryTokens - mcpInstructionTokens),
        mcp_tools: mcpTools.reduce((sum, item) => sum + item.tokens, 0),
        mcp_instructions: mcpInstructionTokens,
      },
    })
  }

  type Measured = Exclude<CategoryID, "autocompact_buffer" | "free_space">

  function finish(
    input: Omit<Snapshot, "categories" | "usedTokens" | "utilization"> & { parts: Record<Measured, number> },
  ): Snapshot {
    const { parts, ...rest } = input
    const used = Object.values(parts).reduce((sum, tokens) => sum + tokens, 0)
    const window = input.contextWindow
    const buffer = window > 0 ? Math.min(input.autoCompactBuffer, Math.max(0, window - used)) : 0
    const order: Measured[] = [
      "messages",
      "system_tools",
      "memory_files",
      "skills",
      "system_prompt",
      "mcp_tools",
      "mcp_instructions",
    ]
    const categories: Category[] = order
      .filter((id) => parts[id] > 0 || id === "messages")
      .map((id) => ({ id, name: NAMES[id], tokens: parts[id] }))
    if (buffer > 0) categories.push({ id: "autocompact_buffer", name: NAMES.autocompact_buffer, tokens: buffer })
    if (window > 0)
      categories.push({ id: "free_space", name: NAMES.free_space, tokens: Math.max(0, window - used - buffer) })
    return {
      ...rest,
      usedTokens: used,
      utilization: window > 0 ? Math.min(1, used / window) : 0,
      categories,
    }
  }

  function partsOf(snapshot: Snapshot): Record<Measured, number> {
    const parts = {
      messages: 0,
      system_tools: 0,
      memory_files: 0,
      skills: 0,
      system_prompt: 0,
      mcp_tools: 0,
      mcp_instructions: 0,
    }
    for (const category of snapshot.categories) {
      if (category.id in parts) parts[category.id as Measured] = category.tokens
    }
    return parts
  }

  /**
   * Fold in what is only known at read time:
   * - `providerContextTokens`: what the provider reported for the latest
   *   turn (input + cache read + cache write + output — the reply is in the
   *   next request's context). Estimates for prompts, tools and memory stay;
   *   Messages absorbs the difference and never drops below its own estimate.
   * - `autoCompactBuffer`: the window share auto-compaction keeps free.
   */
  export function reconcile(
    snapshot: Snapshot,
    input: { providerContextTokens?: number; autoCompactBuffer?: number },
  ): Snapshot {
    const parts = partsOf(snapshot)
    const provider = input.providerContextTokens && input.providerContextTokens > 0 ? input.providerContextTokens : undefined
    if (provider !== undefined) {
      const fixed = snapshot.usedTokens - parts.messages
      parts.messages = Math.max(parts.messages, provider - fixed)
    }
    const buffer = input.autoCompactBuffer ?? snapshot.autoCompactBuffer
    const { categories: _categories, usedTokens: _used, utilization: _utilization, ...rest } = snapshot
    return finish({
      ...rest,
      basis: provider !== undefined ? "provider" : snapshot.basis,
      autoCompactBuffer: buffer,
      autoCompactEnabled: buffer > 0,
      parts,
    })
  }

  /** A breakdown when no request was measured in this process (e.g. after a restart). */
  export function fromUsage(input: {
    sessionID: string
    messageID?: string
    providerID: string
    modelID: string
    contextWindow: number
    providerContextTokens: number
    autoCompactBuffer: number
    now?: number
  }): Snapshot {
    return finish({
      sessionID: input.sessionID,
      messageID: input.messageID,
      providerID: input.providerID,
      modelID: input.modelID,
      contextWindow: input.contextWindow,
      basis: "provider",
      autoCompactEnabled: input.autoCompactBuffer > 0,
      autoCompactBuffer: input.autoCompactBuffer,
      memoryFiles: [],
      mcpTools: [],
      systemTools: [],
      measuredAt: input.now ?? Date.now(),
      parts: {
        messages: input.providerContextTokens,
        system_tools: 0,
        memory_files: 0,
        skills: 0,
        system_prompt: 0,
        mcp_tools: 0,
        mcp_instructions: 0,
      },
    })
  }

  const MAX_SESSIONS = 256
  const latest = new Map<string, Snapshot>()

  export function record(snapshot: Snapshot) {
    latest.delete(snapshot.sessionID)
    latest.set(snapshot.sessionID, snapshot)
    while (latest.size > MAX_SESSIONS) latest.delete(latest.keys().next().value!)
  }

  export function get(sessionID: string): Snapshot | undefined {
    return latest.get(sessionID)
  }

  /** Test hook. */
  export function clear() {
    latest.clear()
  }
}
