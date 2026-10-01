import z from "zod/v4"
import { and, asc, desc, eq, gt, lte } from "drizzle-orm"
import { Database } from "@/runtime/session/storage/db"
import { SessionTraceTable } from "@/runtime/session/session.sql"
import { Flag } from "@/runtime/context/flag/flag"

export namespace SessionTrace {
  export const Kind = z.enum([
    "message.updated",
    "message.removed",
    "part.updated",
    "part.delta",
    "part.removed",
    "request.started",
    "request.completed",
    "request.failed",
    "request.retry_hint",
    "compaction.started",
    "compaction.pruned",
    "compaction.completed",
    "handoff.completed",
    "session.error",
    "scratchpad.read",
    "scratchpad.written",
    "scratchpad.removed",
    "guardrail.tripped",
  ])
  export type Kind = z.infer<typeof Kind>

  export const Entry = z.object({
    sequence: z.number().int().positive(),
    sessionID: z.string(),
    kind: Kind,
    messageID: z.string().optional(),
    partID: z.string().optional(),
    data: z.unknown(),
    time: z.number().int(),
  })
  export type Entry = z.infer<typeof Entry>

  export function append(input: {
    sessionID: string
    kind: Kind
    messageID?: string
    partID?: string
    data: unknown
    time?: number
  }) {
    if (Flag.GIZZI_DISABLE_DURABLE_TRACE) return 0
    return Database.use((db) => {
      const result = db.insert(SessionTraceTable).values({
        session_id: input.sessionID,
        kind: input.kind,
        message_id: input.messageID,
        part_id: input.partID,
        data: input.data,
        time_created: input.time ?? Date.now(),
        // drizzle types .run() as void, but bun:sqlite returns
        // { changes, lastInsertRowid } — the sequence id we hand back.
      }).run() as unknown as { lastInsertRowid: number | bigint }
      return Number(result.lastInsertRowid)
    })
  }

  export function list(input: { sessionID: string; after?: number; through?: number; limit?: number }): Entry[] {
    const after = Math.max(0, input.after ?? 0)
    const limit = Math.min(5_000, Math.max(1, input.limit ?? 500))
    const filters = [eq(SessionTraceTable.session_id, input.sessionID), gt(SessionTraceTable.sequence, after)]
    if (input.through !== undefined) filters.push(lte(SessionTraceTable.sequence, input.through))
    const rows = Database.use((db) => db.select().from(SessionTraceTable)
      .where(and(...filters)).orderBy(asc(SessionTraceTable.sequence)).limit(limit).all())
    return rows.map((row) => ({
      sequence: row.sequence,
      sessionID: row.session_id,
      kind: Kind.parse(row.kind),
      messageID: row.message_id ?? undefined,
      partID: row.part_id ?? undefined,
      data: row.data,
      time: row.time_created,
    }))
  }

  export function head(sessionID: string) {
    const row = Database.use((db) => db.select({ sequence: SessionTraceTable.sequence })
      .from(SessionTraceTable).where(eq(SessionTraceTable.session_id, sessionID))
      .orderBy(desc(SessionTraceTable.sequence)).limit(1).get())
    return row?.sequence ?? 0
  }
}
