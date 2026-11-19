/**
 * Local store for feedback_note (decision D6: notes stay on the device).
 * One JSON file under Global.Path.data. Nothing here sends anything anywhere;
 * the person sends a note through the existing report flow.
 */
import path from "path"
import fs from "fs/promises"
import { randomUUID } from "crypto"
import z from "zod/v4"
import { Global } from "@/runtime/context/global"

export namespace FeedbackNotes {
  export const Note = z.object({
    id: z.string(),
    kind: z.enum(["bug", "confusion", "missing", "other"]),
    summary: z.string(),
    details: z.string().optional(),
    session_id: z.string().optional(),
    created_at: z.number(),
  })
  export type Note = z.infer<typeof Note>

  const MAX_NOTES = 200
  let queue: Promise<unknown> = Promise.resolve()
  let filepathOverride: string | undefined

  function filepath() {
    return filepathOverride ?? path.join(Global.Path.data, "feedback-notes.json")
  }

  export function setFileForTesting(file: string | undefined) {
    filepathOverride = file
  }

  async function read(): Promise<Note[]> {
    const raw = await fs.readFile(filepath(), "utf8").catch(() => undefined)
    if (!raw) return []
    try {
      const json = JSON.parse(raw)
      if (!Array.isArray(json)) return []
      return json.flatMap((n) => {
        const p = Note.safeParse(n)
        return p.success ? [p.data] : []
      })
    } catch {
      return []
    }
  }

  async function write(notes: Note[]) {
    await fs.mkdir(path.dirname(filepath()), { recursive: true })
    await fs.writeFile(filepath(), JSON.stringify(notes, null, 2), { mode: 0o600 })
  }

  function serial<T>(fn: () => Promise<T>): Promise<T> {
    const next = queue.then(fn, fn)
    queue = next.catch(() => undefined)
    return next
  }

  /** Newest first. */
  export async function list(): Promise<Note[]> {
    const notes = await serial(read)
    return notes.sort((a, b) => b.created_at - a.created_at)
  }

  export function add(input: Omit<Note, "id" | "created_at">): Promise<Note> {
    return serial(async () => {
      const note: Note = { ...input, id: `fn_${randomUUID().slice(0, 12)}`, created_at: Date.now() }
      const notes = await read()
      notes.push(note)
      await write(notes.slice(-MAX_NOTES))
      return note
    })
  }

  export function remove(id: string): Promise<boolean> {
    return serial(async () => {
      const notes = await read()
      const next = notes.filter((n) => n.id !== id)
      if (next.length === notes.length) return false
      await write(next)
      return true
    })
  }
}
