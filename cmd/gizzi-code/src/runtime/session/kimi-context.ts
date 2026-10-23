import { ContextBreakdown } from "@/runtime/session/context-breakdown"

/**
 * Kimi Code's context window by category, from its own session record
 * (`~/.kimi-code/sessions/<workspace>/<session>/agents/main/wire.jsonl`):
 * `profile.bind` holds the system prompt and the AGENTS.md paths it read,
 * `llm.tools_snapshot` the exact tool schemas sent to the model,
 * `context.append_message` the conversation.
 */
export namespace KimiContext {
  const estimate = ContextBreakdown.estimate

  /** `# Skills` … up to the next top-level heading of the system prompt. */
  function skillsSection(prompt: string) {
    const start = prompt.search(/^# Skills\s*$/m)
    if (start === -1) return ""
    const rest = prompt.slice(start + 1)
    const end = rest.search(/^# (?!#)/m)
    return end === -1 ? prompt.slice(start) : prompt.slice(start, start + 1 + end)
  }

  /** Skill entries: `- name: description` / `- **name**: …` lines of the skills section. */
  function skillItems(section: string) {
    return section
      .split("\n")
      .map((line) => line.match(/^\s*[-*]\s+\**`?([A-Za-z0-9_.:-]+)`?\**\s*[:—-]/))
      .filter((match): match is RegExpMatchArray => Boolean(match))
      .map((match) => ({ name: match[1], tokens: estimate(match.input ?? "") }))
  }

  function mcpServer(name: string) {
    const rest = name.slice("mcp__".length)
    const cut = rest.indexOf("__")
    return cut === -1 ? rest : rest.slice(0, cut)
  }

  export interface BuildInput {
    /** wire.jsonl text. */
    wire: string
    /** The AGENTS.md files the record names: path → text. */
    agentFiles?: Record<string, string>
    /** Kimi's own context total (ACP usage_update), when it sent one. */
    used?: number
    /** The model window. */
    size: number
    sessionID: string
    providerID: string
    modelID: string
    now?: number
  }

  /** The AGENTS.md paths of the record's latest `profile.bind`. */
  export function agentPaths(wire: string): string[] {
    let paths: string[] = []
    for (const line of wire.split("\n")) {
      if (!line.includes('"profile.bind"')) continue
      try {
        const entry = JSON.parse(line)
        if (Array.isArray(entry.agentsMdPaths)) paths = entry.agentsMdPaths.filter((p: unknown) => typeof p === "string")
      } catch {}
    }
    return paths
  }

  export function build(input: BuildInput): ContextBreakdown.Snapshot | undefined {
    if (!(input.size > 0)) return undefined
    let prompt = ""
    let tools: Array<{ name?: string; description?: unknown; parameters?: unknown; input_schema?: unknown }> = []
    const messages: unknown[] = []
    let model = input.modelID
    for (const line of input.wire.split("\n")) {
      if (!line.trim()) continue
      let entry: Record<string, any>
      try {
        entry = JSON.parse(line)
      } catch {
        continue
      }
      // The main agent's view only; subagents keep their own wire files.
      if (entry.agentId && entry.agentId !== "main") continue
      if (entry.type === "profile.bind" && typeof entry.systemPrompt === "string") {
        prompt = entry.systemPrompt
        if (typeof entry.modelAlias === "string") model = entry.modelAlias
      } else if (entry.type === "llm.tools_snapshot" && Array.isArray(entry.tools)) {
        tools = entry.tools
      } else if (entry.type === "context.append_message" && entry.message) {
        messages.push(entry.message)
      } else if (entry.type === "context.compacted" || entry.type === "context.cleared") {
        messages.length = 0
      }
    }
    if (!prompt && tools.length === 0) return undefined

    const parts: ContextBreakdown.Parts = {
      messages: 0,
      system_tools: 0,
      memory_files: 0,
      skills: 0,
      system_prompt: 0,
      mcp_tools: 0,
      mcp_instructions: 0,
      repo_map: 0,
    }
    // Memory files: the AGENTS.md text Kimi folded into its prompt.
    let rest = prompt
    const memoryFiles: Array<{ path: string; tokens: number }> = []
    for (const [path, text] of Object.entries(input.agentFiles ?? {})) {
      const body = text.trim()
      if (!body || !rest.includes(body)) continue
      memoryFiles.push({ path, tokens: estimate(body) })
      parts.memory_files += estimate(body)
      rest = rest.replace(body, "")
    }
    const skills = skillsSection(rest)
    if (skills) {
      parts.skills = estimate(skills)
      rest = rest.replace(skills, "")
    }
    parts.system_prompt = estimate(rest)

    const mcpTools: ContextBreakdown.Snapshot["mcpTools"] = []
    const systemTools: ContextBreakdown.Snapshot["systemTools"] = []
    for (const tool of tools) {
      const name = typeof tool?.name === "string" ? tool.name : ""
      if (!name) continue
      const tokens = estimate(tool)
      if (name.startsWith("mcp__")) mcpTools.push({ name, server: mcpServer(name), tokens })
      else systemTools.push({ name, tokens })
    }
    parts.mcp_tools = mcpTools.reduce((sum, tool) => sum + tool.tokens, 0)
    parts.system_tools = systemTools.reduce((sum, tool) => sum + tool.tokens, 0)
    // gizzi's instructions ride in the first prompt (ACP has no system prompt):
    // they are prompt and memory files, not conversation.
    let counted = 0
    for (const message of messages) {
      const text = JSON.stringify(message)
      const block = text.match(/<session_instructions>([\s\S]*?)<\/session_instructions>/)?.[1]
      if (!block) {
        counted += estimate(message)
        continue
      }
      const instructions = JSON.parse(`"${block}"`) as string
      for (const piece of instructions.split(/\n(?=(?:Instructions from|Auto-memory from): |# MCP Server Instructions)/)) {
        const memory = piece.match(/^(?:Instructions from|Auto-memory from): (.+)$/m)
        if (memory && memory.index === 0) {
          memoryFiles.push({ path: memory[1].trim(), tokens: estimate(piece) })
          parts.memory_files += estimate(piece)
        } else if (piece.startsWith("# MCP Server Instructions")) parts.mcp_instructions += estimate(piece)
        else parts.system_prompt += estimate(piece)
      }
      counted += Math.max(0, estimate(message) - estimate(instructions))
    }
    const fixed = Object.values(parts).reduce((sum, tokens) => sum + tokens, 0)
    parts.messages = Math.max(counted, (input.used ?? 0) - fixed)

    return ContextBreakdown.fromParts({
      sessionID: input.sessionID,
      providerID: input.providerID,
      modelID: model,
      contextWindow: input.size,
      parts,
      memoryFiles,
      mcpTools,
      systemTools,
      skills: skillItems(skills),
      basis: input.used ? "provider" : "estimated",
      now: input.now,
    })
  }
}
