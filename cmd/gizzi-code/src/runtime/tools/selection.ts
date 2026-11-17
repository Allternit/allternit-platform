/**
 * Legacy MCP tool selection (the removed select_tools tool). Deferral now lives
 * in primitives/tool-families.ts (tool_search, load on call); nothing in the
 * runtime calls this module.
 */
import type { MessageV2 } from "@/runtime/session/message-v2"

const selected = new Map<string, Set<string>>()

function fromHistory(messages: MessageV2.WithParts[]) {
  const result = new Set<string>()
  for (const message of messages) {
    for (const part of message.parts) {
      if (part.type !== "tool") continue
      if (part.tool.includes(":")) result.add(part.tool)
    }
  }
  return result
}

export namespace ToolSelection {
  export function loaded(sessionID: string, messages: MessageV2.WithParts[]) {
    const result = fromHistory(messages)
    for (const name of selected.get(sessionID) ?? []) result.add(name)
    return result
  }

  export function load(sessionID: string, requested: string[], available: string[]) {
    const catalog = new Set(available)
    const current = selected.get(sessionID) ?? new Set<string>()
    const loaded: string[] = []
    const alreadyAvailable: string[] = []
    const unknown: string[] = []
    for (const name of new Set(requested)) {
      if (!catalog.has(name)) unknown.push(name)
      else if (current.has(name)) alreadyAvailable.push(name)
      else {
        current.add(name)
        loaded.push(name)
      }
    }
    selected.set(sessionID, current)
    return {
      loaded: loaded.sort(),
      alreadyAvailable: alreadyAvailable.sort(),
      unknown: unknown.sort(),
    }
  }

  /** @internal */
  export function reset() {
    selected.clear()
  }
}
