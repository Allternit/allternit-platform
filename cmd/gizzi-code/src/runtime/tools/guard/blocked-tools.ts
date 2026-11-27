/**
 * Tools the account blocks (Settings > Connections > Tool permissions, saved
 * on the account by allternit-api). Block wins over everything in the runtime:
 * a bot's allowlist, `always_allow`, Autopilot and the session's mode. A
 * blocked tool is refused before it runs, with REASON.
 *
 * allternit-api sends the list in the runtime's own name forms (C0 note 2):
 * - `mcp__<server>__<tool>`
 * - `mcp__<server>__<connector>__<tool>` (connector-mounted MCP tools)
 * - `execute_action:<connector>:<action>`: one action of an app connector,
 *   reached through the connector MCP's `execute_action` tool. `<connector>`
 *   is the app id and `<action>` the action id after the app, so the call
 *   `execute_action({ actionId: "gmail.send_email" })` is
 *   `execute_action:gmail:send_email`. The full action id is accepted as
 *   `<action>` too (`execute_action:gmail:gmail.send_email`).
 *
 * Where the list comes from:
 * - the session: `PUT /permission/mode/:sessionID` `{ mode, blockedTools }`
 *   (the same push as the permission mode; `set` here);
 * - the bot marker: `bot.blockedTools` (BotTurnInfo).
 * A session with neither behaves exactly as before.
 *
 * No static imports on purpose: bot-turn.ts (dependency-light) and the
 * ink-app TUI use the matcher too. Only `storedParentOf` loads the session
 * store, on use.
 */
export namespace BlockedTools {
  export const REASON = "Blocked in your tool permissions"

  /** The name form for one app connector action (see the module comment). */
  export function executeActionName(connector: string, action: string): string {
    return `execute_action:${connector}:${action}`
  }

  /** Trimmed, non-empty, de-duplicated names; anything that is not a string list is empty. */
  export function normalize(list: unknown): string[] {
    if (!Array.isArray(list)) return []
    const out = new Set<string>()
    for (const raw of list) {
      if (typeof raw !== "string") continue
      const name = raw.trim()
      if (name) out.add(name)
    }
    return [...out]
  }

  /** Union of several lists (most restrictive: a name blocked anywhere is blocked). */
  export function union(...lists: ReadonlyArray<readonly string[] | undefined>): string[] {
    return normalize(lists.flatMap((list) => (list ? [...list] : [])))
  }

  /** Whether a tool name is the connector MCP's `execute_action` (bare or `mcp__<server>__execute_action`). */
  export function isExecuteAction(toolName: string): boolean {
    return toolName === "execute_action" || toolName.endsWith("__execute_action")
  }

  /** The app connector and action an `execute_action` call targets, from its `actionId` input. */
  export function actionOf(args: unknown): { connector: string; action: string; actionId: string } | undefined {
    if (!args || typeof args !== "object") return undefined
    const actionId = (args as { actionId?: unknown }).actionId
    if (typeof actionId !== "string") return undefined
    const trimmed = actionId.trim()
    const dot = trimmed.indexOf(".")
    if (dot <= 0 || dot === trimmed.length - 1) return undefined
    return { connector: trimmed.slice(0, dot).toLowerCase(), action: trimmed.slice(dot + 1), actionId: trimmed }
  }

  function same(a: string, b: string): boolean {
    return a.toLowerCase() === b.toLowerCase()
  }

  /**
   * The blocked entry that covers this call, else undefined. A tool name
   * matches its exact entry (case-insensitive, since a wrong allow is the
   * harm). An `execute_action` call also matches the entry for its connector
   * and action, read from the call's input.
   */
  export function match(blocked: readonly string[] | undefined, toolName: string, args?: unknown): string | undefined {
    if (!blocked || blocked.length === 0 || !toolName) return undefined
    const direct = blocked.find((entry) => same(entry, toolName))
    if (direct) return direct
    if (!isExecuteAction(toolName)) return undefined
    const target = actionOf(args)
    if (!target) return undefined
    return blocked.find((entry) => {
      if (!entry.toLowerCase().startsWith("execute_action:")) return false
      const rest = entry.slice("execute_action:".length)
      const sep = rest.indexOf(":")
      if (sep <= 0) return false
      const connector = rest.slice(0, sep)
      const action = rest.slice(sep + 1)
      if (!same(connector, target.connector)) return false
      return same(action, target.action) || same(action, target.actionId)
    })
  }

  /** The message a refused call returns to the model. */
  export function message(toolName: string): string {
    return `${REASON}: ${toolName}. Tell the user it is blocked; they can change it in Settings > Connections > Tool permissions.`
  }

  /* ------------------------------------------------------------------------ */
  /* Per-session list (pushed with the permission mode)                       */
  /* ------------------------------------------------------------------------ */

  // In memory, like the bot registry. allternit-api pushes it with the mode
  // before a turn streams; a child session (task, agent batch) inherits its
  // parent's list through `parentOf` in `forSession`.
  const sessions = new Map<string, string[]>()

  /** Replace a session's list. An empty list clears it. */
  export function set(sessionID: string, list: unknown): void {
    if (!sessionID) return
    const names = normalize(list)
    if (names.length === 0) sessions.delete(sessionID)
    else sessions.set(sessionID, names)
  }

  /** The list pushed for this session itself (not its parents). */
  export function get(sessionID: string | undefined): string[] {
    return (sessionID && sessions.get(sessionID)) || []
  }

  /** Test hook. */
  export function reset(): void {
    sessions.clear()
  }

  /**
   * The session's list with every parent's list added (a subagent's session
   * is blocked from what its parent is blocked from). `parentOf` reads a
   * session's parent id; with no lists pushed at all it is never called.
   */
  export async function forSession(
    sessionID: string | undefined,
    parentOf: (id: string) => Promise<string | undefined>,
  ): Promise<string[]> {
    if (!sessionID || sessions.size === 0) return []
    const out: string[] = []
    const seen = new Set<string>()
    let id: string | undefined = sessionID
    // Bounded walk: a broken parent chain must never loop.
    while (id && !seen.has(id) && seen.size < 32) {
      seen.add(id)
      out.push(...get(id))
      id = await parentOf(id).catch(() => undefined)
    }
    return normalize(out)
  }

  /** A session's parent id from the session store (loaded on use: the store pulls in the database). */
  export async function storedParentOf(sessionID: string): Promise<string | undefined> {
    const [{ Database, eq }, { SessionTable }] = await Promise.all([
      import("@/runtime/session/storage/db"),
      import("@/runtime/session/session.sql"),
    ])
    const row = Database.use((db) =>
      db.select({ parent: SessionTable.parent_id }).from(SessionTable).where(eq(SessionTable.id, sessionID)).get(),
    )
    return row?.parent ?? undefined
  }

  /**
   * Everything blocked for one call's session: the session's (and its
   * parents') pushed list plus the bot marker's list. Most restrictive: the
   * union.
   */
  export async function resolve(sessionID: string | undefined, botBlocked?: readonly string[]): Promise<string[]> {
    return union(await forSession(sessionID, storedParentOf), botBlocked)
  }
}
