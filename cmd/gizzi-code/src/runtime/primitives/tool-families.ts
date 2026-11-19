/**
 * Tool deferral (Primitives Wave B, lane L3): one family map, on by default
 * for every provider. Deferred families are left out of a step's tool list
 * until the model loads them with `tool_search`, or calls one of their tools
 * by name (the call loads the family and tells the model to call again; see
 * `loadOnCall` and session/llm.ts experimental_repairToolCall).
 * GIZZI_DISABLE_TOOL_SEARCH turns deferral off: every tool loads every step.
 *
 * Groups. A built-in family (FAMILY_TOOLS) is one group. MCP tools are
 * grouped per server: `mcp__<server>__<tool>` belongs to group
 * `mcp:<server>` (family "mcp"). Any tool not in the map (core tools, the
 * Wave A/B tools, plugin and custom tools, the computer toolset) is always
 * loaded, so a new tool is never hidden by accident.
 *
 * Loaded set. Per session: groups loaded by keywords or a call by name, and
 * single tools loaded by `select:`. It is kept in memory and in
 * `<state>/tool-families/<sessionID>.json`, so it survives a restart, and is
 * also rebuilt from the session's history (tool_search results and calls to
 * deferred tools). If both are gone, the model reloads in one call.
 *
 * Description. `filter` rewrites this step's `tool_search` description to
 * list the names (only names) of the tools still deferred for this session,
 * generated from the map and the session's MCP catalog (`description`).
 */
import fs from "fs"
import path from "path"
import type { Tool as AITool } from "ai"
import type { MessageV2 } from "@/runtime/session/message-v2"
import { Global } from "@/runtime/context/global"
import {
  DISABLE_TOOL_SEARCH_ENV,
  TOOL_SEARCH_TOOL_ID,
  type ToolFamily,
  type ToolSearchInput,
  type ToolSearchResult,
} from "@/runtime/primitives/wave-b-types"
import {
  END_CONVERSATION_TOOL_ID,
  FEEDBACK_NOTE_TOOL_ID,
  PR_COMMENTS_TOOL_ID,
  SECOND_OPINION_TOOL_ID,
  STATUS_UPDATE_TOOL_ID,
  USER_LOCATION_TOOL_ID,
  VOICE_CONTROL_TOOL_ID,
  WORKTREE_ENTER_TOOL_ID,
  WORKTREE_EXIT_TOOL_ID,
  type GapToolFamily,
} from "@/runtime/primitives/gap-types"

export namespace ToolFamilies {
  /** Every deferred family: the Wave B set plus the prompt-gap families (gap-types.ts). */
  export type Family = ToolFamily | GapToolFamily

  /** Built-in tools of each deferred family ("mcp" is filled per server at run time). */
  export const FAMILY_TOOLS: Record<Exclude<Family, "mcp">, readonly string[]> = {
    "skill-growth": [
      "skill_propose",
      "skill_evaluate",
      "skill_decide",
      "skill_activate",
      "skill_rollback",
      "skill_import_preview",
      "skill_import_apply",
    ],
    scratchpad: ["scratchpad_list", "scratchpad_read", "scratchpad_write", "scratchpad_remove"],
    goals: ["create_goal", "get_goal", "set_goal_budget", "update_goal"],
    artifacts: [
      "artifact_create",
      "artifact_update",
      "artifact_read",
      "artifact_list",
      "artifact_delete",
      "artifact_share",
      "artifact_comment",
      "artifact_suggest",
      "artifact_claim",
      "artifact_release",
      "artifact_storage",
      "artifact_check",
    ],
    "vault-memory": ["memory_write", "memory_recall", "vault_query", "vault_write"],
    email: ["send_agent_email", "get_agent_email_status"],
    automation: ["automation"],
    "notebook-graph": ["notebook_edit", "mdx_graph"],
    threads: ["thread_update", "thread_close", "thread_search"],
    projects: ["project_list", "project_send", "project_status"],
    computers: ["computers"],
    context: ["get_context_remaining", "history_recall"],
    // Prompt-gap tools (G3). A family only matters once its tools are
    // registered (each stub's `available`); until then nothing is listed.
    "code-review": [WORKTREE_ENTER_TOOL_ID, WORKTREE_EXIT_TOOL_ID, SECOND_OPINION_TOOL_ID, PR_COMMENTS_TOOL_ID],
    location: [USER_LOCATION_TOOL_ID],
    voice: [VOICE_CONTROL_TOOL_ID],
    session: [FEEDBACK_NOTE_TOOL_ID, END_CONVERSATION_TOOL_ID],
  }

  /** Words that load a family by keyword (besides its name and its tool names). */
  export const FAMILY_KEYWORDS: Record<Family, readonly string[]> = {
    "skill-growth": ["skill", "skills", "propose", "evaluate", "rollback", "import", "grow", "learn", "improve"],
    scratchpad: ["scratchpad", "scratch", "note", "notes", "jot", "draft"],
    goals: ["goal", "goals", "budget", "objective", "target"],
    artifacts: ["artifact", "artifacts", "document", "doc", "page", "publish", "share", "comment", "storage"],
    "vault-memory": ["memory", "memories", "remember", "recall", "vault", "knowledge", "forget"],
    email: ["email", "emails", "mail", "inbox"],
    automation: ["schedule", "scheduled", "reminder", "remind", "cron", "recurring", "routine", "automation", "heartbeat", "later"],
    "notebook-graph": ["notebook", "jupyter", "ipynb", "cell", "graph", "mdx", "backlinks"],
    threads: ["thread", "threads", "conversation"],
    projects: ["project", "projects", "status"],
    computers: ["computer", "computers", "machine", "sandbox", "vm"],
    context: ["context", "tokens", "window", "remaining", "compaction", "compacted", "history", "earlier"],
    mcp: ["mcp", "connector", "connectors", "integration", "integrations", "server"],
    // Kept narrow: common words ("call", "bug", "end", "where") would load these on ordinary coding turns.
    "code-review": ["worktree", "worktrees", "review", "reviewer", "opinion", "pull", "comments"],
    location: ["location", "nearby", "city", "distance"],
    voice: ["voice", "speak", "speech", "spoken", "accent"],
    session: ["feedback", "diagnostic", "abuse", "abusive", "terminate"],
  }

  /** Always loaded (for reference and tests; anything outside FAMILY_TOOLS is always loaded too). */
  export const ALWAYS_LOADED: readonly string[] = [
    "invalid",
    "read",
    "write",
    "edit",
    "multiedit",
    "apply_patch",
    "symbol_edit",
    "glob",
    "grep",
    "list",
    "bash",
    "run_code",
    "task",
    "agent_swarm",
    "todowrite",
    "question",
    "webfetch",
    "websearch",
    "codesearch",
    "skill",
    TOOL_SEARCH_TOOL_ID,
    "current_time",
    "send_user_message",
    "wait_agents",
    "agent_control",
    STATUS_UPDATE_TOOL_ID,
  ]

  const MCP_PREFIX = "mcp__"
  const MCP_GROUP_PREFIX = "mcp:"
  const DESCRIPTION_MARKER = "\n\nTools you can load"
  /** Names listed per group in the tool_search description. */
  const DESCRIPTION_NAMES_PER_GROUP = 40
  /** Sessions kept in memory (catalog and loaded set); the file keeps the rest. */
  const MAX_SESSIONS = 500

  const builtinFamily = new Map<string, Exclude<Family, "mcp">>()
  for (const [family, ids] of Object.entries(FAMILY_TOOLS) as [Exclude<Family, "mcp">, readonly string[]][]) {
    for (const id of ids) builtinFamily.set(id, family)
  }

  type Loaded = { groups: Set<string>; tools: Set<string> }
  /** The loaded set per session (memory copy of the state file). */
  const loadedBySession = new Map<string, Loaded>()
  /** The session's tools as of its last step, before deferral: id → description. */
  const catalogBySession = new Map<string, Map<string, string>>()
  let stateDirOverride: string | undefined

  export function disabled(): boolean {
    const value = process.env[DISABLE_TOOL_SEARCH_ENV]?.trim().toLowerCase()
    return !!value && value !== "0" && value !== "false" && value !== "off" && value !== "no"
  }

  function mcpServer(toolID: string): string | undefined {
    if (!toolID.startsWith(MCP_PREFIX)) return undefined
    const rest = toolID.slice(MCP_PREFIX.length)
    const end = rest.indexOf("__")
    const server = end > 0 ? rest.slice(0, end) : ""
    return server || undefined
  }

  /** The deferred family a tool id belongs to, or undefined when it is always loaded. */
  export function familyOf(toolID: string): Family | undefined {
    if (mcpServer(toolID)) return "mcp"
    return builtinFamily.get(toolID)
  }

  /** The loadable group: a family name, or `mcp:<server>` for an MCP tool. Undefined = always loaded. */
  export function groupOf(toolID: string): string | undefined {
    const server = mcpServer(toolID)
    if (server) return MCP_GROUP_PREFIX + server
    return builtinFamily.get(toolID)
  }

  // ---------------------------------------------------------------------------
  // Loaded set: memory + state file + history
  // ---------------------------------------------------------------------------

  export function stateDirectory(): string {
    return stateDirOverride ?? path.join(Global.Path.state, "tool-families")
  }

  function stateFile(sessionID: string): string {
    return path.join(stateDirectory(), `${sessionID.replace(/[^A-Za-z0-9_-]/g, "_")}.json`)
  }

  function remember<V>(map: Map<string, V>, key: string, value: V) {
    map.delete(key)
    map.set(key, value)
    while (map.size > MAX_SESSIONS) {
      const oldest = map.keys().next().value
      if (oldest === undefined) break
      map.delete(oldest)
    }
  }

  function stored(sessionID: string): Loaded {
    const cached = loadedBySession.get(sessionID)
    if (cached) return cached
    const loaded: Loaded = { groups: new Set(), tools: new Set() }
    try {
      const raw = JSON.parse(fs.readFileSync(stateFile(sessionID), "utf8"))
      for (const g of Array.isArray(raw?.groups) ? raw.groups : []) if (typeof g === "string") loaded.groups.add(g)
      for (const t of Array.isArray(raw?.tools) ? raw.tools : []) if (typeof t === "string") loaded.tools.add(t)
    } catch {
      // No file yet, or unreadable: start empty.
    }
    remember(loadedBySession, sessionID, loaded)
    return loaded
  }

  function persist(sessionID: string, loaded: Loaded) {
    remember(loadedBySession, sessionID, loaded)
    try {
      fs.mkdirSync(stateDirectory(), { recursive: true })
      fs.writeFileSync(
        stateFile(sessionID),
        JSON.stringify({ groups: [...loaded.groups].sort(), tools: [...loaded.tools].sort() }),
      )
    } catch {
      // Memory still holds it; history rebuilds it if the process restarts.
    }
  }

  /** What history shows as loaded: tool_search results and calls to deferred tools. */
  function fromHistory(messages: MessageV2.WithParts[] | undefined): Loaded {
    const loaded: Loaded = { groups: new Set(), tools: new Set() }
    for (const message of messages ?? []) {
      for (const part of message.parts ?? []) {
        if (part.type !== "tool") continue
        if (part.tool === TOOL_SEARCH_TOOL_ID) {
          const names = part.state.status === "completed" ? part.state.metadata?.loaded : undefined
          if (Array.isArray(names)) for (const n of names) if (typeof n === "string") loaded.tools.add(n)
          continue
        }
        const group = groupOf(part.tool)
        if (group) loaded.groups.add(group)
      }
    }
    return loaded
  }

  /** The session's loaded set (state file/memory plus history). */
  export function loaded(sessionID: string, messages?: MessageV2.WithParts[]): Loaded {
    const base = stored(sessionID)
    const history = fromHistory(messages)
    return {
      groups: new Set([...base.groups, ...history.groups]),
      tools: new Set([...base.tools, ...history.tools]),
    }
  }

  function isAvailable(toolID: string, set: Loaded): boolean {
    const group = groupOf(toolID)
    return !group || set.groups.has(group) || set.tools.has(toolID)
  }

  /** The session's catalog, or the built-in map when no step has run in this process yet. */
  function catalogIDs(sessionID: string): string[] {
    const catalog = catalogBySession.get(sessionID)
    if (catalog) return [...catalog.keys()]
    return [...builtinFamily.keys()]
  }

  // ---------------------------------------------------------------------------
  // filter
  // ---------------------------------------------------------------------------

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

  /**
   * Drops the tools of groups this session has not loaded, and rewrites this
   * step's tool_search description (in place: resolveTools builds a new tool
   * object every step) to list the deferred names. Never throws.
   */
  export function filter(input: FilterInput): FilterResult {
    try {
      if (disabled()) {
        const tools = { ...input.tools }
        delete tools[TOOL_SEARCH_TOOL_ID]
        return { tools, deferred: [] }
      }
      const catalog = new Map<string, string>()
      for (const [id, t] of Object.entries(input.tools)) {
        catalog.set(id, typeof (t as { description?: unknown }).description === "string" ? (t as { description: string }).description : "")
      }
      remember(catalogBySession, input.sessionID, catalog)

      const set = loaded(input.sessionID, input.messages)
      const tools: Record<string, AITool> = {}
      const deferred: string[] = []
      for (const [id, t] of Object.entries(input.tools)) {
        if (isAvailable(id, set)) tools[id] = t
        else deferred.push(id)
      }
      const search = tools[TOOL_SEARCH_TOOL_ID] as { description?: string } | undefined
      if (search && typeof search.description === "string") {
        search.description = description(search.description, deferred)
      }
      return { tools, deferred }
    } catch {
      return { tools: input.tools, deferred: [] }
    }
  }

  /**
   * The tool_search description: `base` (tool-search.txt) plus the names of
   * the deferred tools, one line per group. Names only, no schemas. Replaces
   * a list added before, so it is safe to call on the same text twice.
   */
  export function description(base: string, deferred: readonly string[]): string {
    const head = base.includes(DESCRIPTION_MARKER) ? base.slice(0, base.indexOf(DESCRIPTION_MARKER)) : base
    const byGroup = new Map<string, string[]>()
    for (const id of deferred) {
      const group = groupOf(id)
      if (!group) continue
      const list = byGroup.get(group) ?? []
      list.push(id)
      byGroup.set(group, list)
    }
    if (!byGroup.size) return head
    const lines = [...byGroup.keys()].sort().map((group) => {
      const names = byGroup.get(group)!.sort()
      const shown = names.slice(0, DESCRIPTION_NAMES_PER_GROUP).join(", ")
      const more = names.length > DESCRIPTION_NAMES_PER_GROUP ? `, and ${names.length - DESCRIPTION_NAMES_PER_GROUP} more` : ""
      return `- ${group}: ${shown}${more}`
    })
    return `${head}${DESCRIPTION_MARKER} (by group; call tool_search with keywords or the group name):\n${lines.join("\n")}`
  }

  // ---------------------------------------------------------------------------
  // search (tool_search)
  // ---------------------------------------------------------------------------

  const STOPWORDS = new Set([
    "the", "and", "for", "with", "that", "this", "from", "into", "tool", "tools", "use", "using", "need",
    "want", "can", "how", "what", "all", "any", "some", "load", "find", "get", "make", "let", "please",
  ])

  function words(text: string): string[] {
    return text
      .toLowerCase()
      .split(/[^a-z0-9]+/)
      .filter((w) => w.length >= 3 && !STOPWORDS.has(w))
  }

  function wordHits(word: string, keyword: string): boolean {
    if (word === keyword) return true
    // "reminders" → "reminder", "schedul" → "schedule"
    if (keyword.length >= 4 && word.startsWith(keyword)) return true
    if (word.length >= 4 && keyword.startsWith(word)) return true
    return false
  }

  /** Groups whose name, keywords, tool names (or, for MCP, server and descriptions) match a query word. */
  function matchGroups(query: string, sessionID: string): Set<string> {
    const queryWords = words(query)
    const raw = query.trim().toLowerCase()
    const matched = new Set<string>()
    const catalog = catalogBySession.get(sessionID)
    const groupTools = new Map<string, string[]>()
    for (const id of catalogIDs(sessionID)) {
      const group = groupOf(id)
      if (!group) continue
      const list = groupTools.get(group) ?? []
      list.push(id)
      groupTools.set(group, list)
    }
    for (const [group, ids] of groupTools) {
      if (raw === group) {
        matched.add(group)
        continue
      }
      const family = group.startsWith(MCP_GROUP_PREFIX) ? "mcp" : (group as Family)
      const keywords = new Set<string>([...words(group), ...FAMILY_KEYWORDS[family]])
      for (const id of ids) for (const w of words(id)) if (w !== "mcp") keywords.add(w)
      const exact = new Set<string>()
      if (family === "mcp" && catalog) for (const id of ids) for (const w of words(catalog.get(id) ?? "")) exact.add(w)
      const hit = queryWords.some((w) => [...keywords].some((k) => wordHits(w, k)) || exact.has(w))
      if (hit) matched.add(group)
    }
    return matched
  }

  /** Loads the families (or exact tools, for "select:") the query matches, for this session. */
  export async function search(
    input: ToolSearchInput & { sessionID: string; messages: MessageV2.WithParts[] },
  ): Promise<ToolSearchResult> {
    const query = input.query.trim()
    const ids = catalogIDs(input.sessionID)
    const catalog = new Set(ids)
    const current = loaded(input.sessionID, input.messages)
    const next = stored(input.sessionID)
    const result = { loaded: new Set<string>(), already: new Set<string>(), unknown: new Set<string>() }
    const groupIDs = (group: string) => ids.filter((id) => groupOf(id) === group)

    const loadGroup = (group: string) => {
      for (const id of groupIDs(group)) (isAvailable(id, current) ? result.already : result.loaded).add(id)
      next.groups.add(group)
    }

    if (disabled()) {
      // Deferral is off: every tool is in the list already.
      for (const name of query.toLowerCase().startsWith("select:") ? splitSelect(query) : ids) {
        ;(catalog.has(name) ? result.already : result.unknown).add(name)
      }
    } else if (query.toLowerCase().startsWith("select:")) {
      const groups = new Set(ids.map(groupOf).filter((g): g is string => !!g))
      for (const name of splitSelect(query)) {
        if (groups.has(name)) loadGroup(name)
        else if (!catalog.has(name)) result.unknown.add(name)
        else if (isAvailable(name, current)) result.already.add(name)
        else {
          result.loaded.add(name)
          next.tools.add(name)
        }
      }
    } else {
      for (const group of matchGroups(query, input.sessionID)) loadGroup(group)
    }

    if (result.loaded.size) persist(input.sessionID, next)
    return {
      loaded: [...result.loaded].sort(),
      already: [...result.already].filter((id) => !result.loaded.has(id)).sort(),
      unknown: [...result.unknown].sort(),
    }
  }

  function splitSelect(query: string): string[] {
    return [
      ...new Set(
        query
          .slice("select:".length)
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean),
      ),
    ]
  }

  // ---------------------------------------------------------------------------
  // load on call
  // ---------------------------------------------------------------------------

  export interface LoadOnCallResult {
    /** The group that was loaded. */
    group: string
    /** The tool name as it is offered (case fixed). */
    tool: string
    /** The message the model gets back through the `invalid` tool. */
    message: string
  }

  /**
   * The model called `toolName`, which is not in this step's tool list. When it
   * is a deferred tool of this session, load its group (from the next step on)
   * and return the message to send back; else undefined (the caller keeps its
   * usual "invalid tool" handling). Never throws.
   */
  export function loadOnCall(sessionID: string, toolName: string): LoadOnCallResult | undefined {
    try {
      if (disabled()) return undefined
      const ids = catalogIDs(sessionID)
      const tool = ids.includes(toolName)
        ? toolName
        : ids.find((id) => id.toLowerCase() === toolName.toLowerCase())
      if (!tool) return undefined
      const group = groupOf(tool)
      if (!group) return undefined
      const next = stored(sessionID)
      next.groups.add(group)
      persist(sessionID, next)
      const names = ids.filter((id) => groupOf(id) === group).sort()
      return {
        group,
        tool,
        message: `${tool} was not loaded yet, so it did not run. Loaded the ${group} tools (${names.join(", ")}); call ${tool} again.`,
      }
    } catch {
      return undefined
    }
  }

  // ---------------------------------------------------------------------------
  // tests
  // ---------------------------------------------------------------------------

  /** @internal Forget the memory copies (simulates a restart); the state files stay. */
  export function reset() {
    loadedBySession.clear()
    catalogBySession.clear()
  }

  /** @internal Point the state files at another folder (undefined restores the default). */
  export function setStateDirectory(dir: string | undefined) {
    stateDirOverride = dir
  }
}
