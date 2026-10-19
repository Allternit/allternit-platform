import { ContextBreakdown } from "@/runtime/session/context-breakdown"

/**
 * Qwen Code's own `/context` report (`qwen -p /context --resume <id>`, a
 * local command: no model call) as a snapshot. Its rows read
 * `  <Name>   <n>k tokens (<pct>%)` under a `Context window: <n>k tokens` header.
 */
export namespace QwenContext {
  const ROWS: Array<[RegExp, ContextBreakdown.CategoryID]> = [
    [/^system prompt$/i, "system_prompt"],
    [/^(built-in|system) tools$/i, "system_tools"],
    [/^mcp tools$/i, "mcp_tools"],
    [/^mcp (server )?instructions$/i, "mcp_instructions"],
    [/^memory files$/i, "memory_files"],
    [/^skills$/i, "skills"],
    [/^(messages|conversation|chat history)$/i, "messages"],
    [/^(autocompact|compression|compact) buffer$/i, "autocompact_buffer"],
    [/^(system|built-in) tools \(deferred\)$/i, "system_tools_deferred"],
    [/^mcp tools \(deferred\)$/i, "mcp_tools_deferred"],
  ]

  export function parse(input: { text: string; sessionID: string; providerID: string; modelID: string; now?: number }) {
    const window = ContextBreakdown.parseTokenCount(input.text.match(/Context window:\s*([\d.,]+\s*[kKmM]?)/)?.[1] ?? "")
    if (!(window > 0)) return undefined
    const model = input.text.match(/Model:\s*(\S+)/)?.[1] ?? input.modelID
    const parts: ContextBreakdown.Parts = {
      messages: 0,
      system_tools: 0,
      memory_files: 0,
      skills: 0,
      system_prompt: 0,
      mcp_tools: 0,
      mcp_instructions: 0,
    }
    const deferred: ContextBreakdown.DeferredParts = {}
    let buffer = 0
    let seen = false
    for (const line of input.text.split("\n")) {
      const row = line.match(/^\s{1,}([A-Za-z][A-Za-z ()-]*?)\s{2,}([\d.,]+\s*[kKmM]?)\s*tokens/)
      if (!row) continue
      const id = ROWS.find(([pattern]) => pattern.test(row[1].trim()))?.[1]
      if (!id) continue
      seen = true
      const tokens = ContextBreakdown.parseTokenCount(row[2])
      if (id === "autocompact_buffer") buffer = tokens
      else if (id === "system_tools_deferred" || id === "mcp_tools_deferred") deferred[id] = tokens
      else if (id !== "free_space") parts[id as keyof ContextBreakdown.Parts] += tokens
    }
    if (!seen) return undefined
    return ContextBreakdown.fromParts({
      sessionID: input.sessionID,
      providerID: input.providerID,
      modelID: model,
      contextWindow: window,
      parts,
      deferred,
      autoCompactBuffer: buffer,
      now: input.now,
    })
  }
}
