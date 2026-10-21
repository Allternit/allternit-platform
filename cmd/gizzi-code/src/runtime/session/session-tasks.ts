import z from "zod/v4"
import { Session } from "@/runtime/session"
import { SessionStatus } from "@/runtime/session/status"
import { BackgroundTask } from "@/runtime/session/background-task"
import { MessageV2 } from "@/runtime/session/message-v2"

// One list of everything a session has delegated: durable background tasks
// (shells, background subagents) plus foreground subagents, which exist only
// as child sessions. Surfaces render this as their background-tasks pane and
// as the agent run tree (model, cost, tokens and nested subagents per card),
// and open `childSessionID` as the task's transcript.
export namespace SessionTasks {
  /** Nested subagents are listed this many levels below the requested session. */
  export const MAX_DEPTH = 3

  export interface Item {
    id: string
    kind: string
    status: BackgroundTask.Status
    background: boolean
    description: string
    agent: string | null
    childSessionID: string | null
    output: string | null
    error: string | null
    time: { created: number; updated: number; finished: number | null }
    /** "providerID/modelID" of the child session's latest assistant message. */
    model: string | null
    /** Summed assistant cost of the child session; null without a child session. */
    cost: number | null
    /** Summed assistant tokens (output includes reasoning; cache reads are not input). */
    tokens: { input: number; output: number } | null
    /** The child session's own tasks, up to MAX_DEPTH below the requested session. */
    children: Item[]
  }

  // Cast: zod can't infer a recursive object through the `children` getter,
  // so the explicit interface above is the type.
  export const Item = z
    .object({
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
      model: z.string().nullable(),
      cost: z.number().nullable(),
      tokens: z.object({ input: z.number(), output: z.number() }).nullable(),
      get children() {
        return z.array(Item)
      },
    })
    .meta({ ref: "SessionTaskItem" }) as unknown as z.ZodType<Item>

  const SUBAGENT_TITLE = /^(.*) \(@([^)\s]+) subagent\)$/

  export function parseChildTitle(title: string): { description: string; agent: string | null } {
    const match = SUBAGENT_TITLE.exec(title)
    return match ? { description: match[1], agent: match[2] } : { description: title, agent: null }
  }

  const ERROR_MAX = 240

  /** A short human message from a MessageV2 assistant error (name + data.message). */
  export function errorText(error: { name?: string; message?: string; data?: any } | undefined): string | null {
    if (!error) return null
    const raw =
      (typeof error.data?.message === "string" && error.data.message) ||
      (typeof error.message === "string" && error.message) ||
      error.name ||
      "Failed"
    const text = String(raw).replace(/\s+/g, " ").trim()
    return text.length > ERROR_MAX ? text.slice(0, ERROR_MAX - 1) + "…" : text
  }

  interface Rollup {
    model: string | null
    cost: number
    tokens: { input: number; output: number }
    lastError: MessageV2.Assistant["error"]
  }

  // Info-only read (no part payloads): this endpoint is polled while tasks run.
  function rollup(sessionID: string): Rollup {
    const result: Rollup = { model: null, cost: 0, tokens: { input: 0, output: 0 }, lastError: undefined }
    for (const info of MessageV2.infos(sessionID)) {
      if (info.role !== "assistant") continue
      result.cost += info.cost ?? 0
      result.tokens.input += info.tokens?.input ?? 0
      result.tokens.output += (info.tokens?.output ?? 0) + (info.tokens?.reasoning ?? 0)
      result.model = `${info.providerID}/${info.modelID}`
      result.lastError = info.error
    }
    return result
  }

  type Detail = Pick<Item, "model" | "cost" | "tokens" | "children"> & { lastError: Rollup["lastError"] }

  const EMPTY: Detail = { model: null, cost: null, tokens: null, children: [], lastError: undefined }

  async function detail(childSessionID: string | null | undefined, depth: number): Promise<Detail> {
    if (!childSessionID) return EMPTY
    const stats = (() => {
      try {
        return rollup(childSessionID)
      } catch {
        return undefined
      }
    })()
    const children = depth < MAX_DEPTH ? await listAt(childSessionID, depth + 1).catch(() => [] as Item[]) : []
    if (!stats) return { ...EMPTY, children }
    return { model: stats.model, cost: stats.cost, tokens: stats.tokens, children, lastError: stats.lastError }
  }

  export async function list(sessionID: string): Promise<Item[]> {
    return listAt(sessionID, 1)
  }

  // `depth` is the depth of the items being listed (1 = direct tasks of the
  // requested session). Items at MAX_DEPTH carry `children: []`.
  async function listAt(sessionID: string, depth: number): Promise<Item[]> {
    const [tasks, children] = await Promise.all([
      BackgroundTask.list(sessionID),
      Session.children(sessionID).catch(() => [] as Session.Info[]),
    ])
    const byChild = new Map(children.map((child) => [child.id, child]))
    const items: Item[] = await Promise.all(
      tasks.map(async (task) => {
        const { lastError: _ignored, ...extra } = await detail(task.childSessionID, depth)
        const item: Item = {
          id: task.id,
          kind: task.kind,
          status: task.status,
          background: true,
          description: task.description,
          agent: task.childSessionID ? parseChildTitle(byChild.get(task.childSessionID)?.title ?? "").agent : null,
          childSessionID: task.childSessionID ?? null,
          output: task.output ?? null,
          error: task.error ?? null,
          time: { created: task.time.created, updated: task.time.updated, finished: task.time.finished ?? null },
          ...extra,
        }
        return item
      }),
    )
    const owned = new Set(tasks.map((task) => task.childSessionID).filter(Boolean))
    for (const child of children) {
      if (owned.has(child.id)) continue
      const { description, agent } = parseChildTitle(child.title)
      const busy = SessionStatus.get(child.id).type !== "idle"
      const { lastError, ...extra } = await detail(child.id, depth)
      // A stopped run (user abort) reads as cancelled; any other error as failed.
      const status: BackgroundTask.Status = busy
        ? "running"
        : lastError
          ? lastError.name === "MessageAbortedError"
            ? "cancelled"
            : "failed"
          : "completed"
      items.push({
        id: child.id,
        kind: "subagent",
        status,
        background: false,
        description,
        agent,
        childSessionID: child.id,
        output: null,
        error: busy ? null : errorText(lastError),
        time: { created: child.time.created, updated: child.time.updated, finished: busy ? null : child.time.updated },
        ...extra,
      })
    }
    return items.sort((a, b) => b.time.created - a.time.created)
  }
}
