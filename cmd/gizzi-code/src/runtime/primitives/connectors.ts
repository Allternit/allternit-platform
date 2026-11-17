/**
 * Connectors (Primitives Wave D, D2): find the user's connectors, mount their
 * tools on demand, and suggest one to connect. Shared core for the
 * `connector_search` and `connector_suggest` tools (runtime builtin
 * tools/builtins/connector.ts and the Ink adapter in
 * cli/ui/ink-app/tools/ConnectorTool).
 *
 * - Cloud connectors and their OAuth tokens live in allternit-api. The
 *   runtime reaches a connected one only as proxied MCP tools
 *   (`mcp__<server>__<connector>__<tool>`, tools/mcp/user-proxy.ts).
 * - search: `POST /bot-routing/connector/search` (bots/routing/connectors.ts)
 *   matches the catalog and the user's own connectors. `load` mounts the
 *   tools of at most MAX_LOAD connected connectors for the rest of the
 *   session; their normal permission rules apply.
 * - Deferral (decision 10): when more than DEFER_THRESHOLD connector tools
 *   would be mounted in a turn, none are mounted until a search loads them.
 *   `mountFilter` is the hook SessionPrompt.resolveTools calls per MCP tool.
 * - suggest: shows the Connect kit card for 1 to MAX_SUGGEST ids. Each id
 *   must come from a search made in this session. The card never connects
 *   anything itself and carries no link or code; the user taps Connect.
 *
 * W2 stub: final signatures and limits; lane C fills the bodies.
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"

export namespace Connectors {
  /** More connector tools than this in one turn: mount none until `load`. */
  export const DEFER_THRESHOLD = 30
  export const MAX_LIMIT = 10
  /** Connected connectors one search can mount. */
  export const MAX_LOAD = 3
  export const MAX_SUGGEST = 3
  export const MAX_REASON = 200

  export const SearchInput = z.object({
    query: z.string().min(1).max(200).describe("What the user wants to reach, e.g. \"calendar\", \"Linear issues\", \"send a Slack message\""),
    limit: z.number().int().min(1).max(MAX_LIMIT).optional().describe(`Most results to return (default 5, max ${MAX_LIMIT})`),
    load: z
      .boolean()
      .optional()
      .describe(`Mount the tools of the connected matches (at most ${MAX_LOAD}) so you can call them in this session`),
  })
  export type SearchInput = z.infer<typeof SearchInput>

  export const SuggestInput = z.object({
    ids: z.array(z.string().min(1)).min(1).max(MAX_SUGGEST).describe("Connector ids from a connector_search made in this session"),
    reason: z.string().min(1).max(MAX_REASON).describe("One line the user sees: why connecting helps with what they asked"),
  })
  export type SuggestInput = z.infer<typeof SuggestInput>

  export type State = "connected" | "not_connected" | "needs_reconnect"

  export interface Hit {
    id: string
    name: string
    /** Catalog group, e.g. "Calendar", "Docs", "Dev tools". */
    group: string
    state: State
    /** False when the user can't connect it here (plan, platform, org policy). */
    available: boolean
    /** Why it is not available, or what needs doing. */
    reason?: string
    /** Mounted tool names, when `load` mounted this connector. */
    tools?: string[]
  }

  export interface SearchResult {
    hits: Hit[]
    /** Tool names `load` mounted in this call. */
    loaded?: string[]
  }

  export interface SuggestResult {
    /** Ids the card shows (ids not searched in this session are dropped). */
    shown: string[]
  }

  export class NotReadyError extends Error {
    constructor(what: string) {
      super(`${what} is not available in this build yet.`)
      this.name = "ConnectorsNotReadyError"
    }
  }

  /** The part of an MCP tool descriptor the deferral needs (MCP.ToolDescriptor). */
  export interface MountDescriptor {
    connectorId?: string
  }

  /** A connector tool: one the per-user proxy tagged with a connector id. */
  export function isConnectorTool(descriptor: MountDescriptor | undefined): boolean {
    return Boolean(descriptor?.connectorId)
  }

  /**
   * Which MCP tools SessionPrompt.resolveTools mounts this turn. Tools that
   * are not connector tools always mount. When more than DEFER_THRESHOLD
   * connector tools are present, only those of connectors a `load` mounted
   * in this session mount.
   *
   * W2 stub: mounts every tool; lane C fills the threshold and load state.
   */
  export function mountFilter(_sessionID: string, _descriptors: Record<string, MountDescriptor>): (key: string) => boolean {
    return () => true
  }

  /** Mount the tools of these connected connectors for the rest of the session. */
  export function load(_sessionID: string, _connectorIDs: string[]): string[] {
    return []
  }

  export async function search(_sessionID: string, _input: SearchInput, _signal?: AbortSignal): Promise<SearchResult> {
    throw new NotReadyError("connector_search")
  }

  export async function suggest(_sessionID: string, _input: SuggestInput): Promise<SuggestResult> {
    throw new NotReadyError("connector_suggest")
  }
}
