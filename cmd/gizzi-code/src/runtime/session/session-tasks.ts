import z from "zod/v4"
import { Session } from "@/runtime/session"
import { SessionStatus } from "@/runtime/session/status"
import { BackgroundTask } from "@/runtime/session/background-task"

// One list of everything a session has delegated: durable background tasks
// (shells, background subagents) plus foreground subagents, which exist only
// as child sessions. Surfaces render this as their background-tasks pane and
// open `childSessionID` as the task's transcript.
export namespace SessionTasks {
  export const Item = z.object({
    id: z.string(),
    kind: z.string(),
    status: BackgroundTask.Status,
    background: z.boolean(),
    description: z.string(),
    agent: z.string().nullable(),
    childSessionID: z.string().nullable(),
    output: z.string().nullable(),
    error: z.string().nullable(),
    time: z.object({ created: z.number(), updated: z.number(), finished: z.number().nullable() }),
  })
  export type Item = z.infer<typeof Item>

  const SUBAGENT_TITLE = /^(.*) \(@([^)\s]+) subagent\)$/

  export function parseChildTitle(title: string): { description: string; agent: string | null } {
    const match = SUBAGENT_TITLE.exec(title)
    return match ? { description: match[1], agent: match[2] } : { description: title, agent: null }
  }

  export async function list(sessionID: string): Promise<Item[]> {
    const [tasks, children] = await Promise.all([
      BackgroundTask.list(sessionID),
      Session.children(sessionID).catch(() => [] as Session.Info[]),
    ])
    const byChild = new Map(children.map((child) => [child.id, child]))
    const items: Item[] = tasks.map((task) => ({
      ...task,
      background: true,
      agent: task.childSessionID ? parseChildTitle(byChild.get(task.childSessionID)?.title ?? "").agent : null,
    }))
    const owned = new Set(tasks.map((task) => task.childSessionID).filter(Boolean))
    for (const child of children) {
      if (owned.has(child.id)) continue
      const { description, agent } = parseChildTitle(child.title)
      const busy = SessionStatus.get(child.id).type !== "idle"
      items.push({
        id: child.id,
        kind: "subagent",
        status: busy ? "running" : "completed",
        background: false,
        description,
        agent,
        childSessionID: child.id,
        output: null,
        error: null,
        time: { created: child.time.created, updated: child.time.updated, finished: busy ? null : child.time.updated },
      })
    }
    return items.sort((a, b) => b.time.created - a.time.created)
  }
}
