import type { ModelMessage } from "ai"
import { PROMPT_SEGMENT_ORDER } from "./guardrail-defaults"

/**
 * O8 stable-prefix contract, enforced where LLM.stream assembles a request:
 *
 *   [system] [tools] [pinned context] [history] [variable tail]
 *
 * - system + pinned: `LLM.StreamInput.system` (agent/provider prompt,
 *   environment, instructions, workspace, bot persona, scratchpad). Stable
 *   for the life of a turn; rendered as the leading system message(s).
 * - tools: passed to the SDK separately; key order is made deterministic
 *   here (`orderTools`) so a tool registry reshuffle can't bust the cache.
 * - history: the projected conversation.
 * - tail: anything that changes step to step (goal progress, validation
 *   retry notes, usage wrap-up). It goes AFTER history, never into the
 *   system block, so the cached prefix only grows.
 */
export namespace PromptSegments {
  export const ORDER = PROMPT_SEGMENT_ORDER

  export function orderTools<T>(tools: Record<string, T>): Record<string, T> {
    const ordered: Record<string, T> = {}
    for (const key of Object.keys(tools).sort()) ordered[key] = tools[key]
    return ordered
  }

  function render(tail: string[]) {
    return ["<system-reminder>", ...tail, "</system-reminder>"].join("\n")
  }

  /**
   * Append the variable tail after history. A trailing assistant prefill
   * (e.g. the MAX_STEPS notice) stays last. When the message before that
   * point is a user message the tail joins it as a text part (no consecutive
   * user turns); otherwise it becomes its own user message.
   */
  export function withTail(history: ModelMessage[], tail: string[] | undefined): ModelMessage[] {
    const items = (tail ?? []).filter((x) => x && x.trim())
    if (!items.length) return history
    const text = render(items)
    let at = history.length
    while (at > 0 && history[at - 1].role === "assistant") at--
    // Only an assistant-only history (nothing to anchor on) puts the tail last.
    if (at === 0) at = history.length
    const before = history.slice(0, at)
    const after = history.slice(at)
    const prev = before[before.length - 1]
    if (prev?.role === "user") {
      const content =
        typeof prev.content === "string"
          ? [{ type: "text" as const, text: prev.content }, { type: "text" as const, text }]
          : [...prev.content, { type: "text" as const, text }]
      return [...before.slice(0, -1), { ...prev, content }, ...after]
    }
    return [...before, { role: "user", content: text }, ...after]
  }
}
