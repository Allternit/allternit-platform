import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import z from "zod/v4"
import { Database, eq, asc } from "@/runtime/session/storage/db"
import { TodoTable } from "@/runtime/session/session.sql"

export namespace Todo {
  export const Info = z.object({
    id: z.string().optional().describe("Stable id; assigned when missing. Other items name it in blockedBy"),
    content: z.string().describe("The outcome, written as a result not an activity"),
    activeForm: z.string().optional().describe("Present-continuous label shown while in progress"),
    status: z
      .string()
      .describe("pending, in_progress, completed, cancelled or interrupted"),
    priority: z.string().describe("Priority level of the task: high, medium, low"),
    blockedBy: z.array(z.string()).optional().describe("Ids of items that must finish first"),
  })

  export type Info = z.infer<typeof Info>

  export const STATUSES = ["pending", "in_progress", "completed", "cancelled", "interrupted"]

  /** Assign ids, validate blockedBy, keep at most one in_progress. Throws on bad input. */
  export function normalize(input: Info[]): { todos: Info[]; warnings: string[] } {
    const warnings: string[] = []
    const used = new Set(input.map((t) => t.id).filter((x): x is string => !!x))
    const dupes = new Set<string>()
    const seen = new Set<string>()
    for (const t of input) {
      if (!t.id) continue
      if (seen.has(t.id)) dupes.add(t.id)
      seen.add(t.id)
    }
    if (dupes.size) throw new Error(`Duplicate todo ids: ${[...dupes].join(", ")}`)
    let n = 0
    const todos = input.map((t) => {
      for (const st of [t.status]) {
        if (!STATUSES.includes(st)) throw new Error(`Unknown todo status "${st}" (use ${STATUSES.join(", ")})`)
      }
      if (t.id) return { ...t }
      let id: string
      do id = `t${++n}`
      while (used.has(id))
      used.add(id)
      return { ...t, id }
    })
    const ids = new Set(todos.map((t) => t.id!))
    for (const t of todos) {
      for (const dep of t.blockedBy ?? []) {
        if (!ids.has(dep)) throw new Error(`Todo "${t.id}" is blocked by unknown id "${dep}"`)
        if (dep === t.id) throw new Error(`Todo "${t.id}" cannot block itself`)
      }
    }
    // cycle check: depth-first with colors
    const deps = new Map(todos.map((t) => [t.id!, t.blockedBy ?? []]))
    const color = new Map<string, 1 | 2>()
    const visit = (id: string, path: string[]) => {
      if (color.get(id) === 2) return
      if (color.get(id) === 1) throw new Error(`Todo blockedBy cycle: ${[...path, id].join(" -> ")}`)
      color.set(id, 1)
      for (const d of deps.get(id) ?? []) visit(d, [...path, id])
      color.set(id, 2)
    }
    for (const id of ids) visit(id, [])
    let active = false
    for (const t of todos) {
      if (t.status !== "in_progress") continue
      if (active) {
        t.status = "pending"
        warnings.push(`Only one item can be in progress; "${t.content}" set back to pending`)
      }
      active = true
    }
    return { todos, warnings }
  }

  export const Event = {
    Updated: BusEvent.define(
      "todo.updated",
      z.object({
        sessionID: z.string(),
        todos: z.array(Info),
      }),
    ),
  }

  export function update(raw: { sessionID: string; todos: Info[] }) {
    const input = { sessionID: raw.sessionID, todos: normalize(raw.todos).todos }
    Database.transaction((db) => {
      db.delete(TodoTable).where(eq(TodoTable.session_id, input.sessionID)).run()
      if (input.todos.length === 0) return
      db.insert(TodoTable)
        .values(
          input.todos.map((todo, position) => ({
            session_id: input.sessionID,
            content: todo.content,
            status: todo.status,
            priority: todo.priority,
            position,
            item_id: todo.id,
            active_form: todo.activeForm ?? null,
            blocked_by: todo.blockedBy ?? null,
          })),
        )
        .run()
    })
    Bus.publish(Event.Updated, input)
  }

  export function get(sessionID: string) {
    const rows = Database.use((db) =>
      db.select().from(TodoTable).where(eq(TodoTable.session_id, sessionID)).orderBy(asc(TodoTable.position)).all(),
    )
    return rows.map((row) => ({
      id: row.item_id ?? `t${row.position + 1}`,
      content: row.content,
      ...(row.active_form ? { activeForm: row.active_form } : {}),
      status: row.status,
      priority: row.priority,
      ...(row.blocked_by?.length ? { blockedBy: row.blocked_by } : {}),
    }))
  }
}
