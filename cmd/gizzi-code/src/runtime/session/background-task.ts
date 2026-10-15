import { Bus } from "@/shared/bus"
import { BusEvent } from "@/shared/bus/bus-event"
import { Instance } from "@/runtime/context/project/instance"
import { Database, and, desc, eq, notLike } from "@/runtime/session/storage/db"
import { BackgroundTaskTable } from "@/runtime/session/session.sql"
import z from "zod/v4"
import { HookDispatcher } from "@/runtime/hooks/dispatcher"

export namespace BackgroundTask {
  export const Status = z.enum(["queued", "running", "completed", "failed", "cancelled", "interrupted"])
  export type Status = z.infer<typeof Status>
  export const PrintPolicy = z.enum(["exit", "drain", "steer"])
  export type PrintPolicy = z.infer<typeof PrintPolicy>

  export const Info = z.object({
    id: z.string(),
    parentSessionID: z.string(),
    childSessionID: z.string().nullable(),
    kind: z.string(),
    status: Status,
    description: z.string(),
    output: z.string().nullable(),
    error: z.string().nullable(),
    time: z.object({ created: z.number(), updated: z.number(), finished: z.number().nullable() }),
  })
  export type Info = z.infer<typeof Info>

  export const Event = {
    Started: BusEvent.define("background.task.started", Info),
    Finished: BusEvent.define("background.task.finished", Info),
    // Live progress for a running task (e.g. a background shell's output tail).
    Updated: BusEvent.define("background.task.updated", Info),
  }

  // Cap on the output kept for a running task; the newest bytes win.
  export const MAX_LIVE_OUTPUT = 64_000

  /** Id prefix of rows another process owns (the TUI's mirrored tasks); restart cleanup leaves them alone. */
  export const EXTERNAL_ID_PREFIX = "tui_"

  // Restart cleanup is per process, not per Instance: a daemon serving several
  // directories must not mark one project's live tasks interrupted when it
  // opens another.
  let initialized = false

  const state = Instance.state(() => ({
    policies: {} as Record<string, PrintPolicy>,
    // In-process cancel hooks (kill a shell, abort a child session). They do
    // not survive a restart, which `initialize` settles as `interrupted`.
    cancellers: {} as Record<string, () => void | Promise<void>>,
  }))

  function fromRow(row: typeof BackgroundTaskTable.$inferSelect): Info {
    return {
      id: row.id,
      parentSessionID: row.parent_session_id,
      childSessionID: row.child_session_id,
      kind: row.kind,
      status: Status.parse(row.status),
      description: row.description,
      output: row.output,
      error: row.error,
      time: {
        created: row.time_created,
        updated: row.time_updated,
        finished: row.time_finished,
      },
    }
  }

  export async function initialize(): Promise<void> {
    if (initialized) return
    initialized = true
    const now = Date.now()
    const owned = notLike(BackgroundTaskTable.id, `${EXTERNAL_ID_PREFIX}%`)
    Database.use((db) =>
      db
        .update(BackgroundTaskTable)
        .set({
          status: "interrupted",
          error: "The daemon restarted before this background task reached a terminal state.",
          time_finished: now,
          time_updated: now,
        })
        .where(and(eq(BackgroundTaskTable.status, "running"), owned))
        .run(),
    )
    Database.use((db) =>
      db
        .update(BackgroundTaskTable)
        .set({
          status: "interrupted",
          error: "The daemon restarted before this queued background task started.",
          time_finished: now,
          time_updated: now,
        })
        .where(and(eq(BackgroundTaskTable.status, "queued"), owned))
        .run(),
    )
  }

  export function setPrintPolicy(sessionID: string, policy: PrintPolicy): void {
    state().policies[sessionID] = policy
  }

  export function getPrintPolicy(sessionID: string): PrintPolicy {
    return state().policies[sessionID] ?? "steer"
  }

  export async function create(input: {
    id: string
    parentSessionID: string
    childSessionID?: string
    kind: string
    description: string
  }): Promise<Info> {
    await initialize()
    const now = Date.now()
    const row = Database.use((db) =>
      db
        .insert(BackgroundTaskTable)
        .values({
          id: input.id,
          parent_session_id: input.parentSessionID,
          child_session_id: input.childSessionID,
          kind: input.kind,
          status: "running",
          description: input.description,
          time_created: now,
          time_updated: now,
        })
        .returning()
        .get(),
    )
    const info = fromRow(row)
    Bus.publish(Event.Started, info)
    return info
  }

  export function onCancel(id: string, cancel: () => void | Promise<void>): void {
    state().cancellers[id] = cancel
  }

  export function tail(output: string, max = MAX_LIVE_OUTPUT): string {
    return output.length > max ? output.slice(output.length - max) : output
  }

  export async function progress(id: string, output: string): Promise<Info | undefined> {
    await initialize()
    const row = Database.use((db) =>
      db
        .update(BackgroundTaskTable)
        .set({ output: tail(output), time_updated: Date.now() })
        .where(and(eq(BackgroundTaskTable.id, id), eq(BackgroundTaskTable.status, "running")))
        .returning()
        .get(),
    )
    if (!row) return undefined
    const info = fromRow(row)
    Bus.publish(Event.Updated, info)
    return info
  }

  async function settle(id: string, status: Status, fields: { output?: string; error?: string }): Promise<Info> {
    await initialize()
    delete state().cancellers[id]
    const now = Date.now()
    const row = Database.use((db) =>
      db
        .update(BackgroundTaskTable)
        .set({
          status,
          ...(fields.output !== undefined ? { output: fields.output } : {}),
          error: fields.error,
          time_finished: now,
          time_updated: now,
        })
        .where(and(eq(BackgroundTaskTable.id, id), eq(BackgroundTaskTable.status, "running")))
        .returning()
        .get(),
    )
    if (!row) {
      const existing = await get(id)
      if (existing) return existing
      throw new Error(`Background task ${id} does not exist`)
    }
    const info = fromRow(row)
    Bus.publish(Event.Finished, info)
    await HookDispatcher.emit({
      name: "Notification",
      timestamp: Date.now(),
      sessionId: info.parentSessionID,
      payload: { type: `task.${info.status}`, taskID: info.id, kind: info.kind },
    })
    return info
  }

  export function complete(id: string, output: string): Promise<Info> {
    return settle(id, "completed", { output })
  }

  export function fail(id: string, error: unknown, output?: string): Promise<Info> {
    return settle(id, "failed", { error: error instanceof Error ? error.message : String(error), output })
  }

  /** Provider-owned work reports its outcome without sending another stop request. */
  export function recordCancellation(id: string, reason: string, output?: string): Promise<Info> {
    return settle(id, "cancelled", { error: reason, output })
  }

  export function interrupt(id: string, reason: string, output?: string): Promise<Info> {
    return settle(id, "interrupted", { error: reason, output })
  }

  export async function cancel(id: string, reason = "Cancelled by user"): Promise<Info> {
    const canceller = state().cancellers[id]
    if (canceller) await Promise.resolve(canceller())
    return settle(id, "cancelled", { error: reason })
  }

  export async function get(id: string): Promise<Info | undefined> {
    await initialize()
    const row = Database.use((db) => db.select().from(BackgroundTaskTable).where(eq(BackgroundTaskTable.id, id)).get())
    return row ? fromRow(row) : undefined
  }

  export async function list(parentSessionID: string, activeOnly = false): Promise<Info[]> {
    await initialize()
    const rows = Database.use((db) => {
      const condition = activeOnly
        ? and(eq(BackgroundTaskTable.parent_session_id, parentSessionID), eq(BackgroundTaskTable.status, "running"))
        : eq(BackgroundTaskTable.parent_session_id, parentSessionID)
      return db.select().from(BackgroundTaskTable).where(condition).orderBy(desc(BackgroundTaskTable.time_created)).all()
    })
    return rows.map(fromRow)
  }

  export async function countActive(parentSessionID: string): Promise<number> {
    return (await list(parentSessionID, true)).length
  }

  export async function wait(id: string, timeoutMs: number, signal?: AbortSignal): Promise<Info | undefined> {
    const current = await get(id)
    if (!current || !["queued", "running"].includes(current.status)) return current
    return new Promise((resolve, reject) => {
      let settled = false
      const finish = (value: Info | undefined) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        signal?.removeEventListener("abort", onAbort)
        unsubscribe()
        resolve(value)
      }
      const onAbort = () => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        unsubscribe()
        reject(signal?.reason ?? new DOMException("Aborted", "AbortError"))
      }
      const unsubscribe = Bus.subscribe(Event.Finished, (event) => {
        if (event.properties.id === id) finish(event.properties)
      })
      const timer = setTimeout(() => void get(id).then(finish, () => finish(undefined)), Math.max(0, timeoutMs))
      if (signal?.aborted) onAbort()
      else signal?.addEventListener("abort", onAbort, { once: true })
    })
  }
}
