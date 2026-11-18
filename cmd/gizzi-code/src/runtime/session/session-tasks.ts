import z from "zod/v4"
import { Session } from "@/runtime/session"
import { SessionStatus } from "@/runtime/session/status"
import { BackgroundTask } from "@/runtime/session/background-task"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Database, inArray } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"

// One list of everything a session has delegated: durable background tasks
// (shells, background subagents) plus foreground subagents, which exist only
// as child sessions. Surfaces render this as their background-tasks pane and
// as the agent run tree (model, cost, tokens and nested subagents per card),
// and open `childSessionID` as the task's transcript.
//
// One item per child: a child that was resumed or followed up has one
// background_task row per run (rows settle only from `running`). The newest
// run is the item (id, status, output, error, updated/finished time); `runs`
// counts them and `time.created` is the first run's, so the row keeps its
// place in the tree.
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
    /** Subagent card identity: display title, bot-avatar face, model class. */
    title: string | null
    face: string | null
    modelClass: string | null
    childSessionID: string | null
    /** The child's own name (last segment of `path`), or null when it has none. */
    name: string | null
    /** The child's name path from the root ("/root/build/tests"), or null. */
    path: string | null
    /** Background runs of this child (resume and follow-up add one each); 1 for shells and foreground children. */
    runs: number
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
      title: z.string().nullable(),
      face: z.string().nullable(),
      modelClass: z.string().nullable(),
      childSessionID: z.string().nullable(),
      name: z.string().nullable(),
      path: z.string().nullable(),
      runs: z.number(),
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

  type Identity = Pick<Item, "title" | "face" | "modelClass">
  const NO_IDENTITY: Identity = { title: null, face: null, modelClass: null }

  /** The subagent type's card identity (title, face, model class), if the agent is known. */
  async function identity(agent: string | null): Promise<Identity> {
    if (!agent) return NO_IDENTITY
    try {
      const { Agent } = await import("@/runtime/loop/agent")
      const info = await Agent.get(agent)
      if (!info) return NO_IDENTITY
      return { title: info.title ?? null, face: info.face ?? null, modelClass: info.modelClass ?? null }
    } catch {
      return NO_IDENTITY
    }
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

  /** The last segment of a name path ("/root/build/tests" -> "tests"). */
  export function nameFromPath(path: string | null | undefined): string | null {
    if (!path) return null
    const segment = path.split("/").filter(Boolean).pop()
    return segment ?? null
  }

  /** Stored name paths (`session.task_name`) by session id; sessions without one are absent. */
  export function paths(sessionIDs: string[]): Map<string, string> {
    const ids = [...new Set(sessionIDs.filter(Boolean))]
    if (!ids.length) return new Map()
    try {
      const rows = Database.use((db) =>
        db
          .select({ id: SessionTable.id, path: SessionTable.task_name })
          .from(SessionTable)
          .where(inArray(SessionTable.id, ids))
          .all(),
      )
      return new Map(rows.filter((row) => row.path).map((row) => [row.id, row.path as string]))
    } catch {
      return new Map()
    }
  }

  /** Newest row per child session, with how many runs it has; rows without a child stay one item each. */
  export function groupRuns(tasks: BackgroundTask.Info[]): Array<{ task: BackgroundTask.Info; runs: number; firstCreated: number }> {
    const newer = (a: BackgroundTask.Info, b: BackgroundTask.Info) =>
      a.time.created !== b.time.created ? a.time.created > b.time.created : a.id > b.id
    const byChild = new Map<string, { task: BackgroundTask.Info; runs: number; firstCreated: number }>()
    const result: Array<{ task: BackgroundTask.Info; runs: number; firstCreated: number }> = []
    for (const task of tasks) {
      if (!task.childSessionID) {
        result.push({ task, runs: 1, firstCreated: task.time.created })
        continue
      }
      const group = byChild.get(task.childSessionID)
      if (!group) {
        const entry = { task, runs: 1, firstCreated: task.time.created }
        byChild.set(task.childSessionID, entry)
        result.push(entry)
        continue
      }
      group.runs += 1
      group.firstCreated = Math.min(group.firstCreated, task.time.created)
      if (newer(task, group.task)) group.task = task
    }
    return result
  }

  export interface ChildState {
    status: BackgroundTask.Status
    /** The newest run row, when the child ever ran in the background. */
    run: BackgroundTask.Info | undefined
    /** The newest run's output, or the child's last reply for a foreground child. */
    output: string | null
    error: string | null
  }

  /**
   * One child session's status as its parent sees it: the newest run row
   * while it is running or when the session is idle; a busy session reads as
   * running (a follow-up queued behind a settled row); a child with no rows
   * reads like a foreground subagent (busy, aborted, errored or completed).
   */
  export async function childState(childSessionID: string): Promise<ChildState> {
    const run = await BackgroundTask.latestForChild(childSessionID).catch(() => undefined)
    const busy = SessionStatus.get(childSessionID).type !== "idle"
    if (run && (run.status === "running" || run.status === "queued" || !busy)) {
      return { status: run.status, run, output: run.output, error: run.error }
    }
    if (busy) return { status: "running", run, output: null, error: null }
    let lastError: Rollup["lastError"]
    try {
      lastError = rollup(childSessionID).lastError
    } catch {
      lastError = undefined
    }
    const status: BackgroundTask.Status = lastError
      ? lastError.name === "MessageAbortedError"
        ? "cancelled"
        : "failed"
      : "completed"
    return { status, run, output: status === "completed" ? await lastReply(childSessionID) : null, error: errorText(lastError) }
  }

  /** The text of a session's last assistant reply, or null. */
  async function lastReply(sessionID: string): Promise<string | null> {
    try {
      const last = MessageV2.infos(sessionID).findLast((info) => info.role === "assistant")
      if (!last) return null
      const parts = await MessageV2.parts(last.id)
      const text = parts.findLast((part) => part.type === "text") as MessageV2.TextPart | undefined
      return text?.text ?? null
    } catch {
      return null
    }
  }

  // `depth` is the depth of the items being listed (1 = direct tasks of the
  // requested session). Items at MAX_DEPTH carry `children: []`.
  async function listAt(sessionID: string, depth: number): Promise<Item[]> {
    const [tasks, children] = await Promise.all([
      BackgroundTask.list(sessionID),
      Session.children(sessionID).catch(() => [] as Session.Info[]),
    ])
    const byChild = new Map(children.map((child) => [child.id, child]))
    const named = paths(children.map((child) => child.id))
    const items: Item[] = await Promise.all(
      groupRuns(tasks).map(async ({ task, runs, firstCreated }) => {
        const { lastError: _ignored, ...extra } = await detail(task.childSessionID, depth)
        const agent = task.childSessionID ? parseChildTitle(byChild.get(task.childSessionID)?.title ?? "").agent : null
        const item: Item = {
          id: task.id,
          kind: task.kind,
          status: task.status,
          background: true,
          description: task.description,
          agent,
          ...(await identity(agent)),
          childSessionID: task.childSessionID ?? null,
          name: nameFromPath(task.childSessionID ? named.get(task.childSessionID) : null),
          path: (task.childSessionID ? named.get(task.childSessionID) : null) ?? null,
          runs,
          output: task.output ?? null,
          error: task.error ?? null,
          time: { created: firstCreated, updated: task.time.updated, finished: task.time.finished ?? null },
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
        ...(await identity(agent)),
        childSessionID: child.id,
        name: nameFromPath(named.get(child.id)),
        path: named.get(child.id) ?? null,
        runs: 1,
        output: null,
        error: busy ? null : errorText(lastError),
        time: { created: child.time.created, updated: child.time.updated, finished: busy ? null : child.time.updated },
        ...extra,
      })
    }
    return items.sort((a, b) => b.time.created - a.time.created)
  }
}
