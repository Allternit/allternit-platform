/**
 * Direct, read-only access to the sqlite session store for bot-mode
 * bookkeeping (canonical-chat pin checks, roster unread counts).
 *
 * The runtime session namespace (`Session.get`, `MessageV2.stream`) assumes
 * the CLI bootstrap ran — `Global.Path.data` exists and migrations are
 * applied. Outside that context (plain bun tests, thin tooling calls) the
 * data directory may not exist yet and the lazy `Database.Client()` open
 * fails. The helpers here close that gap: they ensure the data directory
 * exists (mkdir is a no-op in real runs) and then issue plain COUNT queries
 * through the same drizzle store `Database.use` provides, so results are
 * identical to the Instance-context path.
 */

async function ensureSessionStoreDir(): Promise<void> {
  const { Global } = await import("@/runtime/context/global")
  const { mkdir } = await import("node:fs/promises")
  await mkdir(Global.Path.data, { recursive: true })
}

/** Row count of the session's messages; throws when the store is unreadable. */
export async function countSessionMessages(sessionId: string): Promise<number> {
  const { Database, eq, count } = await import("@/runtime/session/storage/db")
  const { MessageTable } = await import("@/runtime/session/session.sql")
  await ensureSessionStoreDir()
  return (
    Database.use((db) =>
      db
        .select({ n: count() })
        .from(MessageTable)
        .where(eq(MessageTable.session_id, sessionId))
        .get(),
    )?.n ?? 0
  )
}

/** True when the session id exists in the store; throws when unreadable. */
export async function sessionExistsInStore(sessionId: string): Promise<boolean> {
  const { Database, eq, count } = await import("@/runtime/session/storage/db")
  const { SessionTable } = await import("@/runtime/session/session.sql")
  await ensureSessionStoreDir()
  const row = Database.use((db) =>
    db
      .select({ n: count() })
      .from(SessionTable)
      .where(eq(SessionTable.id, sessionId))
      .get(),
  )
  return (row?.n ?? 0) > 0
}

/**
 * The newest window of a session's handoff lineage (spec P3.16): follows
 * `session.handoff.sessionID` forward. Returns the id itself when it was
 * never handed off. Throws when the store is unreadable.
 */
export async function sessionHandoffHead(sessionId: string, maxHops = 64): Promise<string> {
  const { Database, eq } = await import("@/runtime/session/storage/db")
  const { SessionTable } = await import("@/runtime/session/session.sql")
  await ensureSessionStoreDir()
  let id = sessionId
  for (let i = 0; i < maxHops; i++) {
    const row = Database.use((db) =>
      db.select({ handoff: SessionTable.handoff }).from(SessionTable).where(eq(SessionTable.id, id)).get(),
    )
    const next = row?.handoff?.sessionID
    if (!next || next === id) return id
    id = next
  }
  return id
}

/**
 * The seed of a fresh window: the text part gizzi tagged `metadata.handoff`
 * when it wrote the checkpoint as the window's first user message.
 */
export async function sessionHandoffSeed(
  sessionId: string,
): Promise<{ text: string; from: string; generation?: number; reason?: string; at?: number } | null> {
  const { Database, eq } = await import("@/runtime/session/storage/db")
  const { PartTable } = await import("@/runtime/session/session.sql")
  await ensureSessionStoreDir()
  const rows = Database.use((db) =>
    db.select({ data: PartTable.data }).from(PartTable).where(eq(PartTable.session_id, sessionId)).all(),
  )
  for (const { data } of rows) {
    const part = data as { type?: string; text?: string; metadata?: { handoff?: any }; time?: { start?: number } }
    const h = part?.type === "text" ? part.metadata?.handoff : undefined
    if (h && typeof h.from === "string") {
      return { text: String(part.text ?? ""), from: h.from, generation: h.generation, reason: h.reason, at: part.time?.start }
    }
  }
  return null
}

export interface StoredTextMessage {
  id: string
  role: "user" | "assistant"
  /** Text parts joined, as the agent-sessions API's `content` (tools/files as brackets). */
  content: string
  at?: number
  /** The model that wrote an assistant message (`modelID`). */
  model?: string
  /** Set on a window's seed: the checkpoint that continued from `from`. */
  handoff?: { from: string; generation?: number; reason?: string }
}

/**
 * A window's conversation as text, oldest first, capped to the newest
 * `limit` messages (spec P3.16: reading the earlier window after a rip).
 * The same rows and text the agent-sessions API serves Desktop
 * (`transformMessage` in routes/agent-compat.ts), read from the store so
 * the TUI needs no running server. Messages with no text are skipped.
 */
export async function sessionTextMessages(
  sessionId: string,
  limit = 50,
): Promise<{ total: number; messages: StoredTextMessage[] }> {
  const { Database, eq, asc } = await import("@/runtime/session/storage/db")
  const { MessageTable, PartTable } = await import("@/runtime/session/session.sql")
  await ensureSessionStoreDir()
  const rows = Database.use((db) =>
    db
      .select({ id: MessageTable.id, data: MessageTable.data, created: MessageTable.time_created })
      .from(MessageTable)
      .where(eq(MessageTable.session_id, sessionId))
      .orderBy(asc(MessageTable.time_created), asc(MessageTable.id))
      .all(),
  )
  const parts = Database.use((db) =>
    db
      .select({ message: PartTable.message_id, data: PartTable.data })
      .from(PartTable)
      .where(eq(PartTable.session_id, sessionId))
      .orderBy(asc(PartTable.id))
      .all(),
  )
  const byMessage = new Map<string, any[]>()
  for (const p of parts) {
    const list = byMessage.get(p.message) ?? []
    list.push(p.data)
    byMessage.set(p.message, list)
  }
  const out: StoredTextMessage[] = []
  for (const row of rows) {
    const info = row.data as { role?: string; modelID?: string }
    const role = info?.role
    if (role !== "user" && role !== "assistant") continue
    const own = byMessage.get(row.id) ?? []
    const text: string[] = []
    let handoff: StoredTextMessage["handoff"]
    for (const part of own) {
      const h = part?.metadata?.handoff
      if (h && typeof h.from === "string") handoff = { from: h.from, generation: h.generation, reason: h.reason }
      if ((part?.type === "text" || part?.type === "agent") && part.text) text.push(part.text)
      else if (part?.type === "file") text.push(`[File ${part.filename ?? part.url ?? "attachment"}]`)
      else if (part?.type === "tool" && part.tool) text.push(`[Tool ${part.tool}]`)
    }
    const content = text.join("\n").trim()
    if (!content && !handoff) continue
    out.push({
      id: row.id,
      role,
      content,
      at: row.created,
      ...(role === "assistant" && info.modelID ? { model: info.modelID } : {}),
      ...(handoff ? { handoff } : {}),
    })
  }
  return { total: out.length, messages: out.slice(-limit) }
}
