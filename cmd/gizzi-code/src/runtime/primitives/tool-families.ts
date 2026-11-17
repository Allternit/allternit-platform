/**
 * Tool deferral (Primitives Wave B, lane L3): one family map, on by default
 * for every provider. Deferred families are left out of a step's tool list
 * until the model loads them with `tool_search`, or calls one of their tools
 * by name (the call loads the family and tells the model to call again).
 * GIZZI_DISABLE_TOOL_SEARCH turns deferral off.
 *
 * Contract stub (B1): `filter` returns every tool unchanged ("load
 * everything"), so SessionPrompt.resolveTools behaves as before; `familyOf`
 * returns undefined; `search` throws.
 */
import type { Tool as AITool } from "ai"
import type { MessageV2 } from "@/runtime/session/message-v2"
import {
  notImplemented,
  type ToolFamily,
  type ToolSearchInput,
  type ToolSearchResult,
} from "@/runtime/primitives/wave-b-types"

export namespace ToolFamilies {
  /** The deferred family a tool id belongs to, or undefined when it is always loaded. */
  export function familyOf(_toolID: string): ToolFamily | undefined {
    return undefined
  }

  export interface FilterInput {
    sessionID: string
    /** Every tool this step could offer, after gating (resolveTools' record). */
    tools: Record<string, AITool>
    messages: MessageV2.WithParts[]
  }

  export interface FilterResult {
    /** The tools to send this step. */
    tools: Record<string, AITool>
    /** Tool ids held back (loadable with tool_search). */
    deferred: string[]
  }

  /** Drops the tools of families this session has not loaded. Never throws. */
  export function filter(input: FilterInput): FilterResult {
    return { tools: input.tools, deferred: [] }
  }

  /** Loads the families (or exact tools, for "select:") the query matches, for this session. */
  export function search(_input: ToolSearchInput & { sessionID: string; messages: MessageV2.WithParts[] }): Promise<ToolSearchResult> {
    return notImplemented("tool_search")
  }
}
