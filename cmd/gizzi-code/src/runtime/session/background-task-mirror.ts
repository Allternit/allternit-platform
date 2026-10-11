import { Database, and, eq, inArray } from "@/runtime/session/storage/db"
import { BackgroundTaskTable, SessionTable } from "@/runtime/session/session.sql"
import { BackgroundTask } from "@/runtime/session/background-task"

/**
 * Background tasks owned by another process (the interactive TUI runs its own
 * shells and agents) written straight to the shared task table, so surfaces
 * reading `GET /session/:id/tasks` see them too.
 *
 * Plain DB access: no Instance, no Bus (events published here would never
 * reach the daemon's SSE), and no `BackgroundTask.initialize()` — that marks
 * every running row interrupted and would clobber the daemon's own tasks.
 * Surfaces pick these rows up on their list refresh.
 */
export namespace BackgroundTaskMirror {
  /** Statuses the owning process may still overwrite; anything else was settled elsewhere (e.g. a surface's Stop). */
  const OWNED: BackgroundTask.Status[] = ["queued", "running", "interrupted"]

  export function sessionExists(sessionID: string): boolean {
    const row = Database.use((db) =>
      db.select({ id: SessionTable.id }).from(SessionTable).where(eq(SessionTable.id, sessionID)).get(),
    )
    return !!row
  }

  export function start(input: {
    id: string
    parentSessionID: string
    kind: string
    description: string
    status: BackgroundTask.Status
    created: number
  }): void {
    Database.use((db) =>
      db
        .insert(BackgroundTaskTable)
        .values({
          id: input.id,
          parent_session_id: input.parentSessionID,
          kind: input.kind,
          status: input.status,
          description: input.description,
          time_created: input.created,
          time_updated: Date.now(),
        })
        .onConflictDoNothing()
        .run(),
    )
  }

  /** Update a mirrored row unless it was already settled by someone else; returns whether it changed. */
  export function update(
    id: string,
    fields: { status: BackgroundTask.Status; output?: string | null; error?: string | null; finished?: number | null },
  ): boolean {
    const row = Database.use((db) =>
      db
        .update(BackgroundTaskTable)
        .set({
          status: fields.status,
          ...(fields.output !== undefined ? { output: fields.output === null ? null : BackgroundTask.tail(fields.output) } : {}),
          ...(fields.error !== undefined ? { error: fields.error } : {}),
          ...(fields.finished !== undefined ? { time_finished: fields.finished } : {}),
          // Still running: clear any terminal residue.
          ...(fields.status === "running" || fields.status === "queued" ? { time_finished: null, error: null } : {}),
          time_updated: Date.now(),
        })
        .where(and(eq(BackgroundTaskTable.id, id), inArray(BackgroundTaskTable.status, OWNED)))
        .returning({ id: BackgroundTaskTable.id })
        .get(),
    )
    return !!row
  }

  export function statuses(ids: string[]): Record<string, BackgroundTask.Status> {
    if (ids.length === 0) return {}
    const rows = Database.use((db) =>
      db
        .select({ id: BackgroundTaskTable.id, status: BackgroundTaskTable.status })
        .from(BackgroundTaskTable)
        .where(inArray(BackgroundTaskTable.id, ids))
        .all(),
    )
    return Object.fromEntries(rows.map((row) => [row.id, BackgroundTask.Status.parse(row.status)]))
  }
}
