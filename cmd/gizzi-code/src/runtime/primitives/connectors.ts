/**
 * Connectors (Primitives Wave D, D2): find the user's connectors, mount their
 * tools on demand, and suggest one to connect. Shared core for the
 * `connector_search` and `connector_suggest` tools (runtime builtin
 * tools/builtins/connector.ts and the Ink adapter in
 * cli/ui/ink-app/tools/ConnectorTool).
 *
 * - Cloud connectors and their OAuth tokens live in allternit-api. The
 *   runtime reaches a connected one only as proxied MCP tools
 *   (`mcp__<server>__<connector>__<tool>`, tools/mcp/user-proxy.ts), each
 *   tagged with the id of the MCP connector it belongs to
 *   (`MCP.ToolDescriptor.connectorId`). Catalog apps connected through the
 *   app-connector sidecar are reached through its `execute_action` tool and
 *   have nothing to mount.
 * - search: `POST /bot-routing/connector/search` (bots/routing/connectors.ts)
 *   matches the catalog and the user's own connectors. `load` mounts the
 *   tools of at most MAX_LOAD connected connectors for the rest of the
 *   session; their normal permission rules apply (the mounted names are the
 *   same qualified names the rules match on).
 * - Deferral (decision 10): when more than DEFER_THRESHOLD connector tools
 *   would be mounted in a turn, none are mounted until a search loads them.
 *   `mountFilter` is the hook SessionPrompt.resolveTools calls per MCP tool.
 * - suggest: shows the Connect kit card for 1 to MAX_SUGGEST ids. Each id
 *   must come from a search made in this session. The card never connects
 *   anything itself and carries no link or code; the user taps Connect.
 *
 * State (what was searched, what was loaded) is per session and held in this
 * process's memory, capped at MAX_SESSIONS sessions.
 * Free of UI imports so the Ink adapter and bun tests can use it.
 */
import z from "zod/v4"
import { searchConnectors, type ConnectorSearchHit } from "@/runtime/bots/routing/connectors"

export namespace Connectors {
  /** More connector tools than this in one turn: mount none until `load`. */
  export const DEFER_THRESHOLD = 30
  export const MAX_LIMIT = 10
  export const DEFAULT_LIMIT = 5
  /** Connected connectors one search can mount. */
  export const MAX_LOAD = 3
  export const MAX_SUGGEST = 3
  export const MAX_REASON = 200
  /** Sessions whose search/load state is kept; the oldest is dropped first. */
  const MAX_SESSIONS = 500

  export const SearchInput = z.object({
    query: z.string().min(1).max(200).describe("What the user wants to reach, e.g. \"calendar\", \"Linear issues\", \"send a Slack message\""),
    limit: z.number().int().min(1).max(MAX_LIMIT).optional().describe(`Most results to return (default ${DEFAULT_LIMIT}, max ${MAX_LIMIT})`),
    load: z
      .boolean()
      .optional()
      .describe(`Mount the tools of the connected matches (at most ${MAX_LOAD}) so you can call them in this session`),
  })
  export type SearchInput = z.infer<typeof SearchInput>

  export const SuggestInput = z.object({
    ids: z.array(z.string().min(1).max(200)).min(1).max(MAX_SUGGEST).describe("Connector ids from a connector_search made in this session"),
    reason: z.string().min(1).max(MAX_REASON).describe("One line the user sees: why connecting helps with what they asked"),
  })
  export type SuggestInput = z.infer<typeof SuggestInput>

  /**
   * connected: usable now. available: the user can connect it here (`reason`
   * says when an existing connection needs signing in again). unavailable:
   * can't be connected here (`reason` says why).
   */
  export type State = "connected" | "available" | "unavailable"

  export interface Hit {
    id: string
    name: string
    /** Catalog group, e.g. "Calendar", "Docs", "Developer". */
    group: string
    state: State
    /** False when the user can't connect it here (state `unavailable`). */
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

  /** What the Connect kit card shows. No link, code or token, ever. */
  export interface SuggestCard {
    type: "connector_suggest"
    connectors: Array<{ id: string; name: string; group: string }>
    reason: string
  }

  export interface SuggestResult {
    /** Ids the card shows. */
    shown: string[]
    /** Ids left off the card, and why. */
    dropped: Array<{ id: string; why: "not_searched" | "connected" | "unavailable" }>
    card?: SuggestCard
  }

  export class NotReadyError extends Error {
    constructor(what: string) {
      super(`${what} is not available in this build yet.`)
      this.name = "ConnectorsNotReadyError"
    }
  }

  /** A refusal the model should read and act on. */
  export class RefusedError extends Error {
    constructor(message: string) {
      super(message)
      this.name = "ConnectorsRefusedError"
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

  // ── per-session state ─────────────────────────────────────────────────────

  interface SessionState {
    /** Every hit a search in this session returned, by id. */
    searched: Map<string, ConnectorSearchHit>
    /** MCP connector ids a `load` mounted. */
    loaded: Set<string>
    /** The MCP tool descriptors of the latest turn (from mountFilter). */
    descriptors: Record<string, MountDescriptor>
  }
  const sessions = new Map<string, SessionState>()

  function stateOf(sessionID: string): SessionState {
    let s = sessions.get(sessionID)
    if (s) {
      // Most recently used last, so the oldest is evicted first.
      sessions.delete(sessionID)
      sessions.set(sessionID, s)
      return s
    }
    s = { searched: new Map(), loaded: new Set(), descriptors: {} }
    sessions.set(sessionID, s)
    while (sessions.size > MAX_SESSIONS) {
      const oldest = sessions.keys().next().value
      if (oldest === undefined) break
      sessions.delete(oldest)
    }
    return s
  }

  /** Forget a session's searches and loads (tests, session end). */
  export function reset(sessionID?: string) {
    if (sessionID === undefined) sessions.clear()
    else sessions.delete(sessionID)
  }

  /** MCP connector ids `load` mounted in this session. */
  export function loadedConnectors(sessionID: string): string[] {
    return [...(sessions.get(sessionID)?.loaded ?? [])]
  }

  function connectorToolCount(descriptors: Record<string, MountDescriptor>): number {
    let n = 0
    for (const d of Object.values(descriptors)) if (isConnectorTool(d)) n++
    return n
  }

  /** True when this many connector tools are deferred until a search loads them. */
  export function defers(descriptors: Record<string, MountDescriptor>): boolean {
    return connectorToolCount(descriptors) > DEFER_THRESHOLD
  }

  /**
   * Which MCP tools SessionPrompt.resolveTools mounts this turn. Tools that
   * are not connector tools always mount. When more than DEFER_THRESHOLD
   * connector tools are present, only those of connectors a `load` mounted
   * in this session mount. Mounting never renames a tool, so the permission
   * rules keyed on its qualified name still apply.
   */
  export function mountFilter(sessionID: string, descriptors: Record<string, MountDescriptor>): (key: string) => boolean {
    const s = stateOf(sessionID)
    s.descriptors = descriptors
    if (!defers(descriptors)) return () => true
    return (key) => {
      const d = descriptors[key]
      if (!isConnectorTool(d)) return true
      return s.loaded.has(d!.connectorId!)
    }
  }

  /** Qualified tool names of one MCP connector, from the latest turn's descriptors. */
  function toolsOf(s: SessionState, mcpConnectorID: string): string[] {
    return Object.entries(s.descriptors)
      .filter(([, d]) => d?.connectorId === mcpConnectorID)
      .map(([key]) => key)
      .sort()
  }

  /**
   * Mount the tools of these MCP connectors for the rest of the session (at
   * most MAX_LOAD). Returns the qualified tool names now mounted for them.
   */
  export function load(sessionID: string, mcpConnectorIDs: string[]): string[] {
    const s = stateOf(sessionID)
    const out: string[] = []
    for (const id of [...new Set(mcpConnectorIDs)].slice(0, MAX_LOAD)) {
      s.loaded.add(id)
      out.push(...toolsOf(s, id))
    }
    return out
  }

  function publicHit(h: ConnectorSearchHit): Hit {
    const hit: Hit = {
      id: h.id,
      name: h.name,
      group: h.group,
      state: h.state,
      available: h.state !== "unavailable",
    }
    if (h.reason) hit.reason = h.reason
    return hit
  }

  export async function search(sessionID: string, input: SearchInput, _signal?: AbortSignal): Promise<SearchResult> {
    const parsed = SearchInput.parse(input)
    const limit = Math.min(parsed.limit ?? DEFAULT_LIMIT, MAX_LIMIT)
    const r = await searchConnectors(sessionID, { query: parsed.query, limit })
    const raw = (r?.hits ?? []).slice(0, limit)
    const s = stateOf(sessionID)
    for (const h of raw) s.searched.set(h.id, h)
    const hits = raw.map(publicHit)
    if (!parsed.load) return { hits }

    const loaded: string[] = []
    let mounted = 0
    for (let i = 0; i < raw.length && mounted < MAX_LOAD; i++) {
      const h = raw[i]!
      if (h.state !== "connected" || !h.mcpConnectorId) continue
      mounted++
      const tools = load(sessionID, [h.mcpConnectorId])
      hits[i]!.tools = tools
      loaded.push(...tools)
    }
    return { hits, loaded }
  }

  export async function suggest(sessionID: string, input: SuggestInput): Promise<SuggestResult> {
    const parsed = SuggestInput.parse(input)
    const searched = sessions.get(sessionID)?.searched
    const shown: SuggestCard["connectors"] = []
    const dropped: SuggestResult["dropped"] = []
    for (const id of [...new Set(parsed.ids)]) {
      const h = searched?.get(id)
      if (!h) dropped.push({ id, why: "not_searched" })
      else if (h.state === "connected") dropped.push({ id, why: "connected" })
      else if (h.state === "unavailable") dropped.push({ id, why: "unavailable" })
      else shown.push({ id: h.id, name: h.name, group: h.group })
    }
    if (!shown.length) {
      const why = dropped.every((d) => d.why === "not_searched")
        ? "Those ids did not come from a connector_search in this session. Search first, then suggest ids from its results."
        : "None of those connectors can be offered: they are already connected or can't be connected here."
      throw new RefusedError(why)
    }
    const reason = parsed.reason.trim().slice(0, MAX_REASON)
    return {
      shown: shown.map((c) => c.id),
      dropped,
      card: { type: "connector_suggest", connectors: shown, reason },
    }
  }
}
