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
 *
 * A brain that runs its own loop (Claude Code, Codex and other CLI brains,
 * signed-in app fabric) never sees gizzi's tools or prompt, so its snapshot
 * comes from the brain instead (`source: "brain"`): Claude Code's own
 * `/context` report (`fromClaudeContext`), or the context total it reports
 * (`fromUsage`).
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
    // Loaded on demand (tool search): listed, not in the context window.
    "system_tools_deferred",
    "mcp_tools_deferred",
  ])
  export type CategoryID = z.infer<typeof CategoryID>

  export const Category = z.object({
    id: CategoryID,
    name: z.string(),
    tokens: z.number(),
    /** Not in the window: tools the brain loads only when it searches for them. */
    deferred: z.boolean().optional(),
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
      /** Who measured it: gizzi's own request ("gizzi") or the brain itself ("brain"). */
      source: z.enum(["gizzi", "brain"]).optional(),
      /** The brain's provider id when `source` is "brain" (claude-cli, codex-cli…). */
      brain: z.string().optional(),
      /** Display name of `brain`, filled in at read time. */
      brainName: z.string().optional(),
      /**
       * Where a brain's rows come from: the brain's own report ("brain":
       * Claude's /context, Codex's rollout) or gizzi's measure of what it
       * sends the brain ("sent"), with Messages from the brain's total.
       */
      split: z.enum(["brain", "sent"]).optional(),
      autoCompactEnabled: z.boolean(),
      /** Tokens auto-compaction keeps free (configured size, before clamping to what is left). */
      autoCompactBuffer: z.number(),
      /** Categories in display order, including the buffer and free space. */
      categories: Category.array(),
      memoryFiles: z.object({ path: z.string(), tokens: z.number() }).array(),
      mcpTools: z.object({ name: z.string(), server: z.string(), tokens: z.number() }).array(),
      systemTools: Item.array(),
      /** Skills listed to the model, one entry each. */
      skills: Item.array().optional(),
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
    system_tools_deferred: "System tools (deferred)",
    mcp_tools_deferred: "MCP tools (deferred)",
  }

  /** Instruction blocks as InstructionPrompt.system() renders them. */
  const MEMORY_HEADER = /^(?:Instructions from|Auto-memory from): (.+)$/m
  /** MCP.INSTRUCTIONS_HEADING (not imported: the MCP module pulls in the whole runtime). */
  const MCP_INSTRUCTIONS_HEADING = "# MCP Server Instructions"
  const MCP_PREFIX = "mcp__"
  const SKILL_TOOL = "skill"

  /** One `<skill>` entry of the skill tool's `<available_skills>` list (tools/builtins/skill.ts). */
  const SKILL_ENTRY = /<skill>\s*<name>([^<]+)<\/name>[\s\S]*?<\/skill>/g

  function skillItems(schema: unknown) {
    const description = (schema as { description?: unknown } | null)?.description
    if (typeof description !== "string") return []
    return [...description.matchAll(SKILL_ENTRY)].map((match) => ({ name: match[1].trim(), tokens: estimate(match[0]) }))
  }

  export function estimate(value: unknown) {
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
    let skills: Snapshot["systemTools"] = []
    for (const [name, schema] of Object.entries(input.tools)) {
      if (name === "invalid") continue
      const tokens = estimate(schema)
      if (name.startsWith(MCP_PREFIX)) mcpTools.push({ name, server: mcpServer(name), tokens })
      else if (name === SKILL_TOOL) {
        skillTokens += tokens
        skills = skillItems(schema)
      }
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
      source: "gizzi",
      autoCompactEnabled: input.autoCompactBuffer > 0,
      autoCompactBuffer: input.autoCompactBuffer,
      memoryFiles: memoryFiles.sort(byTokens),
      mcpTools: mcpTools.sort(byTokens),
      systemTools: systemTools.sort(byTokens),
      skills: skills.sort(byTokens),
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

  type Measured = Exclude<CategoryID, "autocompact_buffer" | "free_space" | Deferred>
  type Deferred = "system_tools_deferred" | "mcp_tools_deferred"
  const DEFERRED: Deferred[] = ["system_tools_deferred", "mcp_tools_deferred"]

  function finish(
    input: Omit<Snapshot, "categories" | "usedTokens" | "utilization"> & {
      parts: Record<Measured, number>
      deferred?: Partial<Record<Deferred, number>>
    },
  ): Snapshot {
    const { parts, deferred, ...rest } = input
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
    // Every row, every brain: a category the brain does not fill reads 0.
    const categories: Category[] = order.map((id) => ({ id, name: NAMES[id], tokens: parts[id] }))
    categories.push({ id: "autocompact_buffer", name: NAMES.autocompact_buffer, tokens: buffer })
    categories.push({ id: "free_space", name: NAMES.free_space, tokens: window > 0 ? Math.max(0, window - used - buffer) : 0 })
    for (const id of DEFERRED) categories.push({ id, name: NAMES[id], tokens: deferred?.[id] ?? 0, deferred: true })
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

  function deferredOf(snapshot: Snapshot): Partial<Record<Deferred, number>> {
    const out: Partial<Record<Deferred, number>> = {}
    for (const category of snapshot.categories) {
      if (category.deferred) out[category.id as Deferred] = category.tokens
    }
    return out
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
    input: { providerContextTokens?: number; autoCompactBuffer?: number; contextWindow?: number },
  ): Snapshot {
    const parts = partsOf(snapshot)
    const provider = input.providerContextTokens && input.providerContextTokens > 0 ? input.providerContextTokens : undefined
    if (provider !== undefined) {
      const fixed = snapshot.usedTokens - parts.messages
      // gizzi's own estimate is a floor; a brain's earlier report is just older.
      parts.messages = Math.max(snapshot.source === "brain" ? 0 : parts.messages, provider - fixed)
    }
    const buffer = input.autoCompactBuffer ?? snapshot.autoCompactBuffer
    const { categories: _categories, usedTokens: _used, utilization: _utilization, ...rest } = snapshot
    return finish({
      ...rest,
      contextWindow: input.contextWindow && input.contextWindow > 0 ? input.contextWindow : snapshot.contextWindow,
      basis: provider !== undefined ? "provider" : snapshot.basis,
      autoCompactBuffer: buffer,
      autoCompactEnabled: buffer > 0,
      parts,
      deferred: deferredOf(snapshot),
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
    /** Set when the brain itself reported the total (CLI brains, fabric). */
    brain?: string
    now?: number
  }): Snapshot {
    return finish({
      sessionID: input.sessionID,
      messageID: input.messageID,
      providerID: input.providerID,
      modelID: input.modelID,
      contextWindow: input.contextWindow,
      basis: "provider",
      ...(input.brain ? { source: "brain" as const, brain: input.brain } : {}),
      autoCompactEnabled: input.autoCompactBuffer > 0,
      autoCompactBuffer: input.autoCompactBuffer,
      memoryFiles: [],
      mcpTools: [],
      systemTools: [],
      skills: [],
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

  export type Parts = Record<Measured, number>
  export type DeferredParts = Partial<Record<Deferred, number>>

  /**
   * A brain's snapshot from per-category counts it (or its own logs)
   * reported: Codex's rollout + MCP status, a CLI brain's sent instructions.
   */
  export function fromParts(input: {
    sessionID: string
    messageID?: string
    providerID: string
    modelID: string
    contextWindow: number
    parts: Parts
    deferred?: DeferredParts
    autoCompactBuffer?: number
    memoryFiles?: Snapshot["memoryFiles"]
    mcpTools?: Snapshot["mcpTools"]
    systemTools?: Snapshot["systemTools"]
    skills?: Snapshot["skills"]
    basis?: Snapshot["basis"]
    now?: number
  }): Snapshot {
    const byTokens = (a: { tokens: number }, b: { tokens: number }) => b.tokens - a.tokens
    const buffer = input.autoCompactBuffer ?? 0
    return finish({
      sessionID: input.sessionID,
      messageID: input.messageID,
      providerID: input.providerID,
      modelID: input.modelID,
      contextWindow: input.contextWindow,
      basis: input.basis ?? "estimated",
      source: "brain",
      brain: input.providerID,
      split: "brain",
      autoCompactEnabled: buffer > 0,
      autoCompactBuffer: buffer,
      memoryFiles: [...(input.memoryFiles ?? [])].sort(byTokens),
      mcpTools: [...(input.mcpTools ?? [])].sort(byTokens),
      systemTools: [...(input.systemTools ?? [])].sort(byTokens),
      skills: [...(input.skills ?? [])].sort(byTokens),
      measuredAt: input.now ?? Date.now(),
      parts: input.parts,
      deferred: input.deferred,
    })
  }

  /** Claude Code `/context` category names → ours. */
  const CLAUDE_CATEGORIES: Record<string, CategoryID> = {
    "system prompt": "system_prompt",
    "system tools": "system_tools",
    "mcp tools": "mcp_tools",
    "mcp server instructions": "mcp_instructions",
    "custom agents": "system_prompt",
    "memory files": "memory_files",
    skills: "skills",
    messages: "messages",
    "free space": "free_space",
    "autocompact buffer": "autocompact_buffer",
    "system tools (deferred)": "system_tools_deferred",
    "mcp tools (deferred)": "mcp_tools_deferred",
  }

  /** "144.6k" → 144600, "1m" → 1000000, "~140" → 140. */
  export function parseTokenCount(text: string): number {
    const match = text.trim().replace(/^~/, "").replace(/,/g, "").match(/^([\d.]+)\s*([kKmM]?)/)
    if (!match) return 0
    const value = Number(match[1])
    const scale = match[2].toLowerCase() === "m" ? 1_000_000 : match[2].toLowerCase() === "k" ? 1_000 : 1
    return Number.isFinite(value) ? Math.round(value * scale) : 0
  }

  /** The rows of the Markdown table under `### <heading>`, cells trimmed, header and rule dropped. */
  function tableRows(markdown: string, heading: RegExp): string[][] {
    const lines = markdown.split("\n")
    const start = lines.findIndex((line) => /^#{2,4}\s/.test(line) && heading.test(line))
    if (start === -1) return []
    const rows: string[][] = []
    for (const line of lines.slice(start + 1)) {
      if (/^#{2,4}\s/.test(line)) break
      if (!line.trim().startsWith("|")) continue
      const cells = line.trim().replace(/^\||\|$/g, "").split("|").map((cell) => cell.trim())
      if (cells.every((cell) => /^:?-+:?$/.test(cell))) continue
      rows.push(cells)
    }
    return rows.slice(1)
  }

  /**
   * Claude Code's own `/context` report (`claude -p /context --resume <id>
   * --output-format json`, Markdown in `result`) as a snapshot: every
   * category it shows, deferred tools, MCP tools, memory files and skills.
   */
  export function fromClaudeContext(input: {
    markdown: string
    sessionID: string
    messageID?: string
    providerID: string
    modelID: string
    now?: number
  }): Snapshot | undefined {
    const header = input.markdown.match(/\*\*Tokens:\*\*\s*([^/\n]+)\/\s*([^\s(]+)/)
    const window = header ? parseTokenCount(header[2]) : 0
    const parts: Record<Measured, number> = {
      messages: 0,
      system_tools: 0,
      memory_files: 0,
      skills: 0,
      system_prompt: 0,
      mcp_tools: 0,
      mcp_instructions: 0,
    }
    const deferred: Partial<Record<Deferred, number>> = {}
    let buffer = 0
    let seen = false
    for (const [name, tokens] of tableRows(input.markdown, /usage by category/i)) {
      const id = CLAUDE_CATEGORIES[name?.toLowerCase() ?? ""]
      if (!id || tokens === undefined) continue
      seen = true
      const count = parseTokenCount(tokens)
      if (id === "autocompact_buffer") buffer = count
      else if (id === "system_tools_deferred" || id === "mcp_tools_deferred") deferred[id] = count
      else if (id !== "free_space") parts[id] += count
    }
    if (!seen || window <= 0) return undefined
    const byTokens = (a: { tokens: number }, b: { tokens: number }) => b.tokens - a.tokens
    const mcpTools = tableRows(input.markdown, /^#+\s*MCP Tools/i)
      .filter((row) => row.length >= 3)
      .map(([name, server, tokens]) => ({ name, server, tokens: parseTokenCount(tokens) }))
    const memoryFiles = tableRows(input.markdown, /^#+\s*Memory Files/i)
      .filter((row) => row.length >= 3)
      .map(([, path, tokens]) => ({ path, tokens: parseTokenCount(tokens) }))
    const skills = tableRows(input.markdown, /^#+\s*Skills/i)
      .filter((row) => row.length >= 2)
      .map((row) => ({ name: row[0], tokens: parseTokenCount(row[row.length - 1]) }))
    const systemTools = tableRows(input.markdown, /^#+\s*System Tools/i)
      .filter((row) => row.length >= 2)
      .map((row) => ({ name: row[0], tokens: parseTokenCount(row[row.length - 1]) }))
    return finish({
      sessionID: input.sessionID,
      messageID: input.messageID,
      providerID: input.providerID,
      modelID: input.modelID,
      contextWindow: window,
      basis: "estimated",
      source: "brain",
      brain: input.providerID,
      split: "brain",
      autoCompactEnabled: buffer > 0,
      autoCompactBuffer: buffer,
      memoryFiles: memoryFiles.sort(byTokens),
      mcpTools: mcpTools.sort(byTokens),
      systemTools: systemTools.sort(byTokens),
      skills: skills.sort(byTokens),
      measuredAt: input.now ?? Date.now(),
      parts,
      deferred,
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
