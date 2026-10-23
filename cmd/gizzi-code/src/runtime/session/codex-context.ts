import { ContextBreakdown } from "@/runtime/session/context-breakdown"

/**
 * Codex's context window by category, from what Codex itself records: the
 * thread's rollout (`~/.codex/sessions/…/rollout-*-<thread>.jsonl`: base
 * instructions, the developer and context blocks, every item) and the app
 * server's `mcpServerStatus/list` (each MCP server's tools). The rollout's
 * first `token_count` is the provider's measure of the first request, so
 * the tool schemas (which the rollout does not hold) are what that request
 * held beyond the prompt blocks; Messages is the rest of the latest total.
 */
export namespace CodexContext {
  const estimate = ContextBreakdown.estimate

  type Item = { type?: string; role?: string; content?: Array<{ text?: unknown }> }

  /** InstructionPrompt block headers (see ContextBreakdown.build). */
  const MEMORY_HEADER = /^(?:Instructions from|Auto-memory from): (.+)$/m
  const MCP_INSTRUCTIONS_HEADING = "# MCP Server Instructions"

  function instructionBlocks(text: string) {
    return text.split(/\n(?=(?:Instructions from|Auto-memory from): |# MCP Server Instructions)/)
  }

  /** `- name: description (file: …)` lines of the skills block. */
  function skillItems(block: string) {
    const start = block.indexOf("### Available skills")
    if (start === -1) return []
    return block
      .slice(start)
      .split("\n")
      .filter((line) => /^- [^:\n]+:/.test(line))
      .map((line) => ({ name: line.slice(2, line.indexOf(":")).trim(), tokens: estimate(line) }))
  }

  /** MCP tools per server from `mcpServerStatus/list` (`{ data: [{ name, tools: { … } }] }`). */
  export function mcpTools(status: unknown): ContextBreakdown.Snapshot["mcpTools"] {
    const servers = (status as { data?: unknown } | undefined)?.data
    if (!Array.isArray(servers)) return []
    return servers.flatMap((server: { name?: unknown; tools?: unknown }) => {
      const name = typeof server?.name === "string" ? server.name : ""
      const tools = server?.tools && typeof server.tools === "object" ? Object.values(server.tools as Record<string, unknown>) : []
      return tools.map((tool) => {
        const t = tool as { name?: unknown; description?: unknown; inputSchema?: unknown }
        const toolName = typeof t.name === "string" ? t.name : "tool"
        return {
          name: `mcp__${name}__${toolName}`,
          server: name,
          tokens: estimate({ name: toolName, description: t.description, input_schema: t.inputSchema }),
        }
      })
    })
  }

  export interface BuildInput {
    /** The rollout file's text (JSON lines). */
    rollout: string
    /** `mcpServerStatus/list` result, when the app server answered. */
    mcpStatus?: unknown
    /** Instruction files to name the AGENTS.md block after: path → text. */
    agentFiles?: Record<string, string>
    /** The latest request's tokens in context and the model window (thread/tokenUsage/updated). */
    used: number
    size: number
    sessionID: string
    providerID: string
    modelID: string
    now?: number
  }

  export function build(input: BuildInput): ContextBreakdown.Snapshot | undefined {
    if (!(input.size > 0)) return undefined
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
    const seen = new Set<string>()
    let skills: Array<{ name: string; tokens: number }> = []
    const memoryBlocks: string[] = []
    const gizziMemory: Array<{ path: string; tokens: number }> = []
    let firstInput = 0
    let firstRequestMessages = 0
    let model = input.modelID

    for (const line of input.rollout.split("\n")) {
      if (!line.trim()) continue
      let entry: { type?: string; payload?: Record<string, any> }
      try {
        entry = JSON.parse(line)
      } catch {
        continue
      }
      const payload = entry.payload ?? {}
      if (entry.type === "session_meta") {
        const base = payload.base_instructions
        const text = typeof base === "string" ? base : typeof base?.text === "string" ? base.text : ""
        if (text) parts.system_prompt += estimate(text)
        continue
      }
      if (entry.type === "event_msg" && payload.type === "token_count" && !firstInput) {
        firstInput = Number(payload.info?.last_token_usage?.input_tokens) || 0
        continue
      }
      if (entry.type === "event_msg" && payload.type === "thread_settings_applied") {
        const picked = payload.thread_settings?.model
        if (typeof picked === "string" && picked) model = picked
        continue
      }
      if (entry.type !== "response_item" || payload.type !== "message") continue
      const item = payload as Item
      // One message can carry several blocks (developer: gizzi's instructions,
      // skills, permissions…; user: AGENTS.md, environment): sort each.
      for (const part of item.content ?? []) {
        const text = typeof part.text === "string" ? part.text : ""
        if (!text) continue
        const contextBlock = item.role === "developer" || /^(# AGENTS\.md instructions|<environment_context>)/.test(text)
        // Context blocks Codex re-sends unchanged are counted once.
        if (contextBlock) {
          if (seen.has(text)) continue
          seen.add(text)
        }
        if (item.role === "developer") {
          if (text.startsWith("<skills_instructions>")) {
            parts.skills += estimate(text)
            skills = skillItems(text)
          } else if (/^<(apps|plugins|mcp)[_a-z]*instructions>/.test(text)) {
            parts.mcp_instructions += estimate(text)
          } else {
            // gizzi's developer instructions hold its memory files and MCP server instructions.
            for (const block of instructionBlocks(text)) {
              const memory = block.match(MEMORY_HEADER)
              if (memory && memory.index === 0) {
                parts.memory_files += estimate(block)
                gizziMemory.push({ path: memory[1].trim(), tokens: estimate(block) })
              } else if (block.startsWith(MCP_INSTRUCTIONS_HEADING)) parts.mcp_instructions += estimate(block)
              else parts.system_prompt += estimate(block)
            }
          }
        } else if (text.startsWith("<environment_context>")) {
          parts.system_prompt += estimate(text)
        } else if (text.startsWith("# AGENTS.md instructions")) {
          parts.memory_files += estimate(text)
          memoryBlocks.push(text)
        } else if (!firstInput) {
          firstRequestMessages += estimate(text)
        }
      }
    }

    // What the first request held beyond the prompt blocks: the tool schemas.
    const prompt = parts.system_prompt + parts.skills + parts.memory_files + parts.mcp_instructions
    const toolTokens = firstInput > 0 ? Math.max(0, firstInput - prompt - firstRequestMessages) : 0
    const mcp = mcpTools(input.mcpStatus)
    const mcpTotal = mcp.reduce((sum, tool) => sum + tool.tokens, 0)
    const deferred: ContextBreakdown.DeferredParts = {}
    if (mcpTotal > 0 && mcpTotal <= toolTokens) {
      parts.mcp_tools = mcpTotal
      parts.system_tools = toolTokens - mcpTotal
    } else {
      // More MCP schema than the request held: Codex searches for them (tool search / code mode).
      parts.system_tools = toolTokens
      if (mcpTotal > 0) deferred.mcp_tools_deferred = mcpTotal
    }
    const fixed = prompt + parts.system_tools + parts.mcp_tools
    parts.messages = Math.max(0, input.used - fixed)

    return ContextBreakdown.fromParts({
      sessionID: input.sessionID,
      providerID: input.providerID,
      modelID: model,
      contextWindow: input.size,
      parts,
      deferred,
      memoryFiles: [...gizziMemory, ...memoryFiles(memoryBlocks, input.agentFiles ?? {})],
      mcpTools: mcp,
      skills,
      basis: "provider",
      now: input.now,
    })
  }

  /** The AGENTS.md block, split into the instruction files whose text it holds. */
  function memoryFiles(blocks: string[], files: Record<string, string>) {
    return blocks.flatMap((block) => {
      const found = Object.entries(files).filter(([, text]) => text.trim() && block.includes(text.trim()))
      if (found.length === 0) return [{ path: "AGENTS.md", tokens: estimate(block) }]
      return found.map(([path, text]) => ({ path, tokens: estimate(text) }))
    })
  }
}
