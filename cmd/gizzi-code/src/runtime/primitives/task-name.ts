/**
 * Task names, fork and effort (Primitives Wave B, lane L2).
 *
 * - TaskName: a child's name lives on its session (`session.task_name`) as a
 *   path such as "/root/build/tests", at most TASK_NAME_MAX_DEPTH levels,
 *   unique within the tree. `task_id` and agent_control targets accept a path.
 * - TaskFork: `fork: "all"` copies the parent's history up to its last
 *   finished message and refuses a model or effort override.
 * - TaskEffort: low/medium/high maps to the model's existing `variant`.
 *
 * Paths: a named child's path is its nearest named ancestor's path (or
 * TASK_NAME_ROOT when none) plus its own segment, so an unnamed middle level
 * does not break addressing. A taken path gets "-2", "-3"… on its last segment.
 */
import { Database, eq, inArray } from "@/runtime/session/storage/db"
import { SessionTable } from "@/runtime/session/session.sql"
import { Session } from "@/runtime/session"
import { MessageV2 } from "@/runtime/session/message-v2"
import { Identifier } from "@/shared/id/id"
import { Provider } from "@/runtime/providers/provider"
import { ModelThinking } from "@/runtime/providers/discovery/model-thinking"
import {
  TASK_NAME_MAX_DEPTH,
  TASK_NAME_PATTERN,
  TASK_NAME_ROOT,
  type TaskEffort as TaskEffortValue,
  type TaskFork as TaskForkValue,
} from "@/runtime/primitives/wave-b-types"

/** How far up/down the session tree we walk. Unnamed levels don't count toward the name depth, so allow slack. */
const TREE_WALK_LIMIT = 8

type Row = { id: string; parent_id: string | null; task_name: string | null }

function row(sessionID: string): Row | undefined {
  return Database.use((db) =>
    db
      .select({ id: SessionTable.id, parent_id: SessionTable.parent_id, task_name: SessionTable.task_name })
      .from(SessionTable)
      .where(eq(SessionTable.id, sessionID))
      .get(),
  )
}

function childrenOf(parentIDs: string[]): Row[] {
  if (parentIDs.length === 0) return []
  return Database.use((db) =>
    db
      .select({ id: SessionTable.id, parent_id: SessionTable.parent_id, task_name: SessionTable.task_name })
      .from(SessionTable)
      .where(inArray(SessionTable.parent_id, parentIDs))
      .all(),
  )
}

/** Every descendant of `sessionID` (not itself), breadth first. */
function descendants(sessionID: string): Row[] {
  const out: Row[] = []
  let frontier = [sessionID]
  for (let level = 0; level < TREE_WALK_LIMIT && frontier.length > 0; level++) {
    const next = childrenOf(frontier)
    out.push(...next)
    frontier = next.map((r) => r.id)
  }
  return out
}

/** The nearest ancestor-or-self that has a name, or the tree's top session when none does. */
function anchorOf(sessionID: string): { id: string; path: string } {
  let current = row(sessionID)
  let top = sessionID
  for (let i = 0; current && i < TREE_WALK_LIMIT; i++) {
    if (current.task_name) return { id: current.id, path: current.task_name }
    top = current.id
    if (!current.parent_id) break
    current = row(current.parent_id)
  }
  return { id: top, path: TASK_NAME_ROOT }
}

function depthOf(path: string): number {
  const rest = path.startsWith(TASK_NAME_ROOT) ? path.slice(TASK_NAME_ROOT.length) : path
  return rest.split("/").filter(Boolean).length
}

/** `name` with "-n" on the end, cut so the segment still fits 32 characters. */
function suffixed(name: string, n: number): string {
  const tail = `-${n}`
  return name.slice(0, 32 - tail.length) + tail
}

function isDescendant(sessionID: string, candidateID: string): boolean {
  let current = row(candidateID)
  for (let i = 0; current && i < TREE_WALK_LIMIT; i++) {
    if (!current.parent_id) return false
    if (current.parent_id === sessionID) return true
    current = row(current.parent_id)
  }
  return false
}

export namespace TaskName {
  /** `name` when it matches TASK_NAME_PATTERN; throws a model-readable error otherwise. */
  export function validate(name: string): string {
    if (!TASK_NAME_PATTERN.test(name))
      throw new Error(
        `Invalid task name "${name}": use 1-32 characters from a-z, 0-9, "_" and "-" (one segment, no "/").`,
      )
    return name
  }

  /** Stores the child's path (parent path + "/" + name) on the child session and returns it. Throws when too deep. A taken path gets "-2", "-3"… */
  export async function assign(input: { parentSessionID: string; childSessionID: string; name: string }): Promise<string> {
    const name = validate(input.name)
    const anchor = anchorOf(input.parentSessionID)
    const base = anchor.path
    if (depthOf(base) + 1 > TASK_NAME_MAX_DEPTH)
      throw new Error(
        `Task name "${name}" would sit ${depthOf(base) + 1} levels below ${TASK_NAME_ROOT}; names go at most ${TASK_NAME_MAX_DEPTH} levels deep (parent is ${base}).`,
      )
    const taken = new Set(
      descendants(anchor.id)
        .filter((r) => r.id !== input.childSessionID && r.task_name)
        .map((r) => r.task_name as string),
    )
    let path = `${base}/${name}`
    for (let n = 2; taken.has(path); n++) path = `${base}/${suffixed(name, n)}`
    Database.use((db) =>
      db.update(SessionTable).set({ task_name: path }).where(eq(SessionTable.id, input.childSessionID)).run(),
    )
    return path
  }

  /**
   * The descendant session id of `sessionID` that `target` names, or undefined.
   * `target` is a session id, an absolute path ("/root/build/tests"), or a
   * path relative to this session's own name ("tests", "build/tests").
   */
  export async function resolve(input: { sessionID: string; target: string }): Promise<string | undefined> {
    const target = input.target.trim()
    if (!target) return undefined
    if (!target.startsWith("/") && isDescendant(input.sessionID, target)) return target
    const all = descendants(input.sessionID)
    const byPath = (path: string) => all.find((r) => r.task_name === path)?.id
    if (target.startsWith("/")) return byPath(target.replace(/\/+$/, ""))
    const base = anchorOf(input.sessionID).path
    return byPath(`${base}/${target.replace(/^\.\//, "").replace(/\/+$/, "")}`)
  }

  /** The stored path of a session, or null when it has no name. */
  export async function pathOf(sessionID: string): Promise<string | null> {
    return row(sessionID)?.task_name ?? null
  }
}

export namespace TaskFork {
  /** Throws when `fork: "all"` comes with a model or effort override. */
  export function check(input: { fork?: TaskForkValue; model?: string; effort?: TaskEffortValue }): void {
    if (input.fork !== "all") return
    const overrides = [
      input.model && input.model !== "inherit" ? `model: ${input.model}` : undefined,
      input.effort ? `effort: ${input.effort}` : undefined,
    ].filter(Boolean)
    if (overrides.length > 0)
      throw new Error(
        `fork: "all" keeps the child on your own model and effort, so it cannot take ${overrides.join(" or ")}. Drop the override, or use fork: "none" and put the context the child needs in the prompt.`,
      )
  }

  /** Whether a message is done: user messages always are; an assistant message once it completed. */
  function finished(msg: MessageV2.WithParts): boolean {
    if (msg.info.role === "user") return true
    return msg.info.time.completed !== undefined
  }

  /** The prefix of `msgs` up to (not including) the first unfinished message. */
  export function cutoff(msgs: MessageV2.WithParts[]): MessageV2.WithParts[] {
    const stop = msgs.findIndex((m) => !finished(m))
    return stop === -1 ? msgs : msgs.slice(0, stop)
  }

  /** Copies the parent's messages up to its last finished one into the child session. Returns how many were copied. */
  export async function copyHistory(input: { parentSessionID: string; childSessionID: string }): Promise<number> {
    const msgs = cutoff(await Session.messages({ sessionID: input.parentSessionID }))
    const ids = new Map<string, string>()
    for (const msg of msgs) {
      const id = Identifier.ascending("message")
      ids.set(msg.info.id, id)
      const parentID = msg.info.role === "assistant" ? ids.get(msg.info.parentID) : undefined
      await Session.updateMessage({
        ...msg.info,
        sessionID: input.childSessionID,
        id,
        ...(parentID ? { parentID } : {}),
      } as MessageV2.Info)
      for (const part of msg.parts) {
        await Session.updatePart({
          ...part,
          id: Identifier.ascending("part"),
          messageID: id,
          sessionID: input.childSessionID,
        } as MessageV2.Part)
      }
    }
    return msgs.length
  }
}

export namespace TaskEffort {
  /** Reasoning levels from least to most; levels outside this scale are never picked. */
  const SCALE = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]

  /** The level in `levels` nearest to `effort` (ties go to the lower level), or undefined when none is on the scale. */
  export function nearest(effort: TaskEffortValue, levels: string[]): string | undefined {
    const want = SCALE.indexOf(effort)
    let best: { level: string; distance: number; rank: number } | undefined
    for (const level of levels) {
      const rank = SCALE.indexOf(level.toLowerCase())
      if (rank === -1) continue
      const distance = Math.abs(rank - want)
      if (!best || distance < best.distance || (distance === best.distance && rank < best.rank))
        best = { level, distance, rank }
    }
    return best?.level
  }

  /** The model's variant name for an effort, or undefined when the model has no levels. */
  export async function variant(input: {
    effort: TaskEffortValue
    providerID: string
    modelID: string
  }): Promise<string | undefined> {
    const model = await Provider.getModel(input.providerID, input.modelID).catch(() => undefined)
    const levels = model?.variants
      ? Object.keys(model.variants)
      : (ModelThinking.of(input.modelID)?.levels ?? [])
    return nearest(input.effort, levels)
  }
}
