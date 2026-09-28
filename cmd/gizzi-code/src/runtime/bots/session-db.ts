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
